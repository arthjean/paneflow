use std::path::Path;
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FileStamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    pub(crate) fn from_metadata(meta: &std::fs::Metadata) -> Self {
        Self {
            mtime: meta.modified().ok(),
            len: meta.len(),
        }
    }

    pub(crate) fn read(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        if !meta.is_file() {
            return None;
        }
        Some(Self::from_metadata(&meta))
    }

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SaveFailure {
    ChangedOnDisk(Option<FileStamp>),
    Write(String),
}

pub(crate) fn changed_since(expected: Option<FileStamp>, current: Option<FileStamp>) -> bool {
    match (expected, current) {
        (Some(expected), Some(current)) => expected.differs(&current),
        (None, Some(_)) => true,
        _ => false,
    }
}

pub(crate) fn save_blocking(
    path: &Path,
    contents: &str,
    expected: Option<FileStamp>,
) -> Result<FileStamp, SaveFailure> {
    let before = FileStamp::read(path);
    if changed_since(expected, before) {
        return Err(SaveFailure::ChangedOnDisk(before));
    }
    let mut staged =
        paneflow_home::stage_write(path, contents.as_bytes()).map_err(|err| write_error(&err))?;
    let written = staged.metadata().map_err(|err| write_error(&err))?;
    let current = FileStamp::read(path);
    if changed_since(expected, current) {
        return Err(SaveFailure::ChangedOnDisk(current));
    }
    staged.commit().map_err(|err| write_error(&err))?;
    Ok(FileStamp::from_metadata(&written))
}

fn write_error(err: &std::io::Error) -> SaveFailure {
    use std::io::ErrorKind;
    SaveFailure::Write(
        match err.kind() {
            ErrorKind::PermissionDenied => "Permission denied - this file could not be written.",
            ErrorKind::NotFound => "This file, its symlink target or its folder no longer exists.",
            ErrorKind::StorageFull => "The disk is full - nothing was written.",
            ErrorKind::ReadOnlyFilesystem => "This file is on a read-only filesystem.",
            _ => "This file could not be written.",
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_replaces_the_file_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "old\n").expect("seed");

        let stamp = save_blocking(&path, "new contents\n", FileStamp::read(&path)).expect("save");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "new contents\n"
        );
        assert_eq!(stamp.len, "new contents\n".len() as u64);
        assert_eq!(
            Some(stamp),
            FileStamp::read(&path),
            "the stamp comes from the staged file, which the rename carries over unchanged"
        );

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

    #[test]
    fn a_save_recreates_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gone.rs");
        assert!(FileStamp::read(&path).is_none());

        save_blocking(&path, "back\n", None).expect("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "back\n");
        assert!(FileStamp::read(&path).is_some());
    }

    #[test]
    fn a_failed_write_reports_a_written_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing-folder").join("file.rs");
        let SaveFailure::Write(err) =
            save_blocking(&path, "x", None).expect_err("no such directory")
        else {
            panic!("a missing folder is a write failure");
        };
        assert!(!err.is_empty());
        assert!(err.ends_with('.'), "a sentence, not a debug dump: {err}");
    }

    #[test]
    fn a_save_against_a_stale_stamp_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("main.rs");
        std::fs::write(&path, "mine\n").expect("seed");
        let expected = FileStamp::read(&path);
        std::fs::write(&path, "the agent wrote more\n").expect("agent write");

        assert_eq!(
            save_blocking(&path, "mine, edited\n", expected),
            Err(SaveFailure::ChangedOnDisk(FileStamp::read(&path)))
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "the agent wrote more\n"
        );
        let entries = std::fs::read_dir(dir.path()).expect("read_dir").count();
        assert_eq!(entries, 1, "no temp file is left behind");
    }

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

    #[cfg(unix)]
    #[test]
    fn a_save_preserves_the_original_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("script.sh");
        std::fs::write(&path, "#!/bin/sh\n").expect("seed");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        save_blocking(&path, "#!/bin/sh\necho hi\n", FileStamp::read(&path)).expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "the executable bit survived the rename"
        );
    }

    fn link_to(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }

    fn is_link(path: &std::path::Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    #[test]
    fn a_save_through_a_symlink_updates_the_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("real.rs");
        std::fs::write(&target, "old\n").expect("seed");
        let link = dir.path().join("main.rs");
        link_to(&target, &link);

        save_blocking(&link, "new\n", FileStamp::read(&link)).expect("save");

        assert!(is_link(&link));
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "new\n");
    }
}
