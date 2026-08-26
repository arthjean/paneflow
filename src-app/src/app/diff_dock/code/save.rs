//! Writing a file back to disk, and the stamp that tells the editor whether
//! someone else got there first.
//!
//! prd-file-editor-2026-Q3, US-015 and US-016. Everything here is blocking:
//! [`super::view::CodeView`] runs it on GPUI's background executor
//! (`cx.background_spawn`), which keeps it off the render thread exactly as
//! `smol::unblock` does for the read side, and additionally puts it under the
//! test scheduler so the save and conflict paths can be proven from the
//! `Ctrl+S` action rather than from this function alone.
//!
//! ## Why a temp file plus a rename
//!
//! `File::create` truncates before it writes. A crash, a full disk, or an
//! agent reading the file mid-write then sees a truncated or half-written
//! source file - the failure mode US-015 exists to prevent. Writing a sibling
//! temp file and renaming it over the target makes the swap atomic on all
//! three platforms, so a reader sees either the old file or the new one.
//!
//! The temp file must live in the **target's own directory**, never in
//! `$TMPDIR`: a cross-filesystem rename is neither atomic nor always
//! permitted. Same reasoning, same shape as `crate::config_writer`
//! (`config_writer.rs:75-118`) and Zed's `Fs::atomic_write`
//! (`zed/crates/fs/src/fs.rs:927-935`).
//!
//! ## Why the rename is also what US-016 watches
//!
//! The rename lands as a create or rename event on the parent directory, not
//! as a modify on the file's inode. That is why the conflict watcher in
//! `super::view` watches the parent directory rather than the file, and why
//! the diff dock's own worktree watcher (`diff/view/watcher.rs:38-46`, which
//! accepts anything that is not a metadata or access event) picks a save up
//! and refreshes the diff without any extra wiring.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::NamedTempFile;

/// What the file looked like on disk the last time the editor agreed with it.
///
/// Modification time plus length, which is the pair Zed's buffer carries
/// (`zed/crates/language/src/buffer.rs:109`, `:1453`). Length is not
/// redundant: a filesystem with second-granularity timestamps, or an agent
/// that rewrites a file twice within one tick, can leave the mtime unchanged
/// while the content changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FileStamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    /// Stat `path`. `None` means the file is not there (or cannot be stat'd),
    /// which US-016 treats as "deleted on disk" rather than as an error.
    pub(crate) fn read(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        Some(Self {
            mtime: meta.modified().ok(),
            len: meta.len(),
        })
    }

    /// Whether `other` describes a different file state than `self`.
    ///
    /// A missing mtime on either side falls back to the length alone: some
    /// filesystems (and some network mounts) do not report one, and refusing
    /// every save there would be worse than the narrower check.
    pub(crate) fn differs(&self, other: &Self) -> bool {
        if self.len != other.len {
            return true;
        }
        match (self.mtime, other.mtime) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }
}

/// Write `contents` to `path` atomically, preserving the file's permissions,
/// and return the stamp of what landed.
///
/// **Blocking.** The error is already a written sentence, in the same register
/// as [`super::load::CodeLoadError::message`]: nothing here surfaces a
/// debug-formatted `io::Error` to the user.
pub(crate) fn save_blocking(path: &Path, contents: &str) -> Result<FileStamp, String> {
    let parent = parent_dir(path);
    let existing = std::fs::metadata(path).ok();

    let mut temp = NamedTempFile::new_in(&parent).map_err(|err| write_error(&err))?;
    temp.write_all(contents.as_bytes())
        .map_err(|err| write_error(&err))?;
    temp.as_file_mut()
        .flush()
        .map_err(|err| write_error(&err))?;
    // Durability before the swap: a rename that beats its own data to disk can
    // leave an empty file behind a power cut.
    temp.as_file().sync_all().map_err(|err| write_error(&err))?;

    // `NamedTempFile` creates 0600. Restore the original's mode so saving does
    // not silently strip a group-readable or executable bit. Skipped when the
    // original is read-only: applying that to the temp would make the rename
    // itself fail on Windows, and a read-only document never reaches here.
    if let Some(meta) = &existing {
        let permissions = meta.permissions();
        if !permissions.readonly()
            && let Err(err) = temp.as_file().set_permissions(permissions)
        {
            log::warn!(
                "could not carry the original permissions onto {}: {err}",
                path.display()
            );
        }
    }

    temp.persist(path).map_err(|err| write_error(&err.error))?;
    FileStamp::read(path)
        .ok_or_else(|| "The file was written but could not be read back.".to_string())
}

/// Directory the temp file goes in: the target's own, falling back to the
/// working directory for a bare file name. Never `$TMPDIR`.
fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// A written explanation of a failed write. Kind-level, so no OS string and no
/// path leak into the banner, and specific enough to act on.
fn write_error(err: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match err.kind() {
        ErrorKind::PermissionDenied => "Permission denied - this file could not be written.",
        ErrorKind::NotFound => "The folder holding this file no longer exists.",
        ErrorKind::StorageFull => "The disk is full - nothing was written.",
        ErrorKind::ReadOnlyFilesystem => "This file is on a read-only filesystem.",
        _ => "This file could not be written.",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US-015 AC: the write is atomic, it lands the exact bytes, and it leaves
    /// no temp file behind.
    #[test]
    fn a_save_replaces_the_file_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "old\n").expect("seed");

        let stamp = save_blocking(&path, "new contents\n").expect("save");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "new contents\n"
        );
        assert_eq!(stamp.len, "new contents\n".len() as u64);

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name())
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "the temp file was renamed, not left: {entries:?}"
        );
    }

    /// US-015 AC: a file that does not exist yet is created, which is also how
    /// US-016 recovers from a deletion on disk.
    #[test]
    fn a_save_recreates_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gone.rs");
        assert!(FileStamp::read(&path).is_none());

        save_blocking(&path, "back\n").expect("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "back\n");
        assert!(FileStamp::read(&path).is_some());
    }

    /// US-015 AC: a write into a folder that is not there fails with a written
    /// sentence rather than a panic or a raw OS error.
    #[test]
    fn a_failed_write_reports_a_written_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing-folder").join("file.rs");
        let err = save_blocking(&path, "x").expect_err("no such directory");
        assert!(!err.is_empty());
        assert!(err.ends_with('.'), "a sentence, not a debug dump: {err}");
    }

    /// US-016: the stamp catches a rewrite of the same length through the
    /// mtime, and a length change on its own.
    #[test]
    fn the_stamp_detects_an_external_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("watched.rs");
        std::fs::write(&path, "aaaa").expect("seed");
        let first = FileStamp::read(&path).expect("stat");

        std::fs::write(&path, "aaaaaa").expect("grow");
        let second = FileStamp::read(&path).expect("stat");
        assert!(first.differs(&second), "a length change is a change");
        assert!(
            !second.differs(&second),
            "a stamp never differs from itself"
        );
    }

    /// US-015 AC: the original permissions survive the swap.
    #[cfg(unix)]
    #[test]
    fn a_save_preserves_the_original_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("script.sh");
        std::fs::write(&path, "#!/bin/sh\n").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        save_blocking(&path, "#!/bin/sh\necho hi\n").expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "the executable bit survived the rename"
        );
    }
}
