use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const MAX_LINK_HOPS: usize = 40;

static STAGED_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn resolve_write_target(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..MAX_LINK_HOPS {
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound && current == path => {
                return Ok(current);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!(
                        "{} is a symlink to {}, which does not exist",
                        path.display(),
                        current.display()
                    ),
                ));
            }
            Err(error) => return Err(error),
        };
        if !metadata.file_type().is_symlink() {
            return Ok(current);
        }
        let link = fs::read_link(&current)?;
        current = match current.parent() {
            Some(parent) => parent.join(link),
            None => link,
        };
    }
    Err(io::Error::other(format!(
        "{} has too many levels of symlinks",
        path.display()
    )))
}

pub struct StagedWrite {
    temporary: PathBuf,
    target: PathBuf,
    committed: bool,
}

impl StagedWrite {
    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn metadata(&self) -> io::Result<fs::Metadata> {
        fs::metadata(&self.temporary)
    }

    pub fn commit(&mut self) -> io::Result<()> {
        let target = self.target.clone();
        self.promote(&target)
    }

    pub fn promote(&mut self, destination: &Path) -> io::Result<()> {
        if self.committed {
            return Ok(());
        }
        fs::rename(&self.temporary, destination)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for StagedWrite {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.temporary);
        }
    }
}

pub fn stage_write(path: &Path, contents: &[u8]) -> io::Result<StagedWrite> {
    let target = resolve_write_target(path)?;
    let existing = match fs::metadata(&target) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error),
    };
    if cfg!(windows)
        && existing
            .as_ref()
            .is_some_and(|meta| meta.permissions().readonly())
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("{} is read-only", target.display()),
        ));
    }
    let directory = target
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let (temporary, file) = create_temporary_beside(&directory, &target)?;
    let staged = StagedWrite {
        temporary,
        target,
        committed: false,
    };
    fill_temporary(file, contents, existing.as_ref())?;
    Ok(staged)
}

pub fn write_atomically(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    stage_write(path, contents)?.commit()
}

fn create_temporary_beside(directory: &Path, target: &Path) -> io::Result<(PathBuf, File)> {
    let name = target
        .file_name()
        .map_or_else(|| "file".into(), |name| name.to_string_lossy());
    loop {
        let sequence = STAGED_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let temporary = directory.join(format!(".{name}.{}.{sequence}.tmp", std::process::id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

fn fill_temporary(
    mut file: File,
    contents: &[u8],
    existing: Option<&fs::Metadata>,
) -> io::Result<()> {
    file.write_all(contents)?;
    file.sync_all()?;
    #[cfg(unix)]
    if let Some(metadata) = existing {
        file.set_permissions(metadata.permissions())?;
    }
    #[cfg(not(unix))]
    let _ = existing;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn link(target: &Path, link: &Path) -> io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn link(target: &Path, link: &Path) -> io::Result<()> {
        std::os::windows::fs::symlink_file(target, link)
    }

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn writing_through_a_symlink_updates_the_target_and_keeps_the_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dotfiles = dir.path().join("dotfiles");
        let home = dir.path().join("home");
        fs::create_dir_all(&dotfiles).expect("dotfiles");
        fs::create_dir_all(&home).expect("home");
        let target = dotfiles.join("paneflow.json");
        fs::write(&target, "old").expect("target");
        let linked = home.join("paneflow.json");
        link(&target, &linked).expect("symlink");

        write_atomically(&linked, b"new").expect("write");

        assert!(
            fs::symlink_metadata(&linked)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).expect("target"), "new");
        assert_eq!(entries(&dotfiles), vec!["paneflow.json".to_string()]);
        assert_eq!(entries(&home), vec!["paneflow.json".to_string()]);
    }

    #[test]
    fn a_chain_of_relative_symlinks_is_followed_to_the_real_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("store");
        fs::create_dir_all(&store).expect("store");
        fs::write(store.join("real.json"), "old").expect("real");
        link(Path::new("real.json"), &store.join("middle.json")).expect("middle");
        let outer = dir.path().join("outer.json");
        link(Path::new("store").join("middle.json").as_path(), &outer).expect("outer");

        assert_eq!(
            resolve_write_target(&outer).expect("resolve"),
            dir.path().join("store").join("real.json")
        );
        write_atomically(&outer, b"new").expect("write");

        assert_eq!(
            fs::read_to_string(store.join("real.json")).expect("real"),
            "new"
        );
        assert!(
            fs::symlink_metadata(&outer)
                .expect("outer")
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(store.join("middle.json"))
                .expect("middle")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_dangling_symlink_is_an_error_and_the_link_is_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.json");
        let linked = dir.path().join("paneflow.json");
        link(&missing, &linked).expect("symlink");

        let error = write_atomically(&linked, b"new").expect_err("dangling link");

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("does not exist"), "{error}");
        assert!(
            fs::symlink_metadata(&linked)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert!(!missing.exists());
        assert_eq!(entries(dir.path()), vec!["paneflow.json".to_string()]);
    }

    #[test]
    fn the_temporary_file_is_created_beside_the_target_not_the_link() {
        let dir = tempfile::tempdir().expect("tempdir");
        let volume = dir.path().join("other-volume");
        let home = dir.path().join("home");
        fs::create_dir_all(&volume).expect("volume");
        fs::create_dir_all(&home).expect("home");
        let target = volume.join("session.json");
        fs::write(&target, "old").expect("target");
        let linked = home.join("session.json");
        link(&target, &linked).expect("symlink");

        let staged = stage_write(&linked, b"new").expect("stage");

        assert_eq!(staged.target(), target.as_path());
        assert_eq!(entries(&home), vec!["session.json".to_string()]);
        assert_eq!(entries(&volume).len(), 2);
        drop(staged);
        assert_eq!(entries(&volume), vec!["session.json".to_string()]);
        assert_eq!(fs::read_to_string(&target).expect("target"), "old");
    }

    #[test]
    fn a_plain_path_is_created_and_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("window-state.json");

        write_atomically(&path, b"one").expect("create");
        write_atomically(&path, b"two").expect("replace");

        assert_eq!(fs::read_to_string(&path).expect("read"), "two");
        assert_eq!(
            entries(&dir.path().join("nested")),
            vec!["window-state.json".to_string()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_mode_survives_the_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        for mode in [0o444, 0o555] {
            let path = dir.path().join(format!("file-{mode:o}"));
            fs::write(&path, "old").expect("write");
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");

            write_atomically(&path, b"new").expect("write");

            let metadata = fs::metadata(&path).expect("metadata");
            assert_eq!(metadata.permissions().mode() & 0o777, mode);
            assert_eq!(fs::read_to_string(&path).expect("read"), "new");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_read_only_target_behind_a_symlink_is_refused_and_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.json");
        fs::write(&target, "old").expect("target");
        let mut permissions = fs::metadata(&target).expect("metadata").permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&target, permissions).expect("read-only");
        let linked = dir.path().join("paneflow.json");
        link(&target, &linked).expect("symlink");

        let error = write_atomically(&linked, b"new").expect_err("read-only");

        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&target).expect("target"), "old");
        assert!(
            fs::symlink_metadata(&linked)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        let mut permissions = fs::metadata(&target).expect("metadata").permissions();
        #[allow(clippy::permissions_set_readonly_false, reason = "test cleanup")]
        permissions.set_readonly(false);
        fs::set_permissions(&target, permissions).expect("writable");
    }
}
