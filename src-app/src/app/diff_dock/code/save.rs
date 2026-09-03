use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tempfile::NamedTempFile;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct FileStamp {
    mtime: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
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

pub(crate) fn save_blocking(path: &Path, contents: &str) -> Result<FileStamp, String> {
    let parent = parent_dir(path);
    let existing = std::fs::metadata(path).ok();

    let mut temp = NamedTempFile::new_in(&parent).map_err(|err| write_error(&err))?;
    temp.write_all(contents.as_bytes())
        .map_err(|err| write_error(&err))?;
    temp.as_file_mut()
        .flush()
        .map_err(|err| write_error(&err))?;
    temp.as_file().sync_all().map_err(|err| write_error(&err))?;

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

fn parent_dir(path: &Path) -> PathBuf {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

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

    #[test]
    fn a_save_recreates_a_missing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("gone.rs");
        assert!(FileStamp::read(&path).is_none());

        save_blocking(&path, "back\n").expect("save");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "back\n");
        assert!(FileStamp::read(&path).is_some());
    }

    #[test]
    fn a_failed_write_reports_a_written_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing-folder").join("file.rs");
        let err = save_blocking(&path, "x").expect_err("no such directory");
        assert!(!err.is_empty());
        assert!(err.ends_with('.'), "a sentence, not a debug dump: {err}");
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

        save_blocking(&path, "#!/bin/sh\necho hi\n").expect("save");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o755,
            "the executable bit survived the rename"
        );
    }
}
