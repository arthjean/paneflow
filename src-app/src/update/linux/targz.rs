use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

const MAX_TARBALL_BYTES: u64 = 500 * 1024 * 1024;

const UPDATE_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

pub fn run_update(asset_url: &str) -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME environment variable is not set")?;
    let app_dir = home.join(".local").join("paneflow.app");
    let cache_base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".cache"));
    let cache_dir = cache_base.join(crate::runtime_paths::APP_SUBDIR);
    run_update_in(asset_url, &app_dir, &cache_dir)?;
    Ok(app_dir.join("bin").join("paneflow"))
}

fn run_update_in(asset_url: &str, app_dir: &Path, cache_dir: &Path) -> Result<()> {
    let (old_dir, new_dir) = staging_dirs(app_dir)?;
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory - refusing to swap at filesystem root")?;

    let _update_lock = acquire_update_lock(parent)?;

    recover_and_clean_staging(app_dir, &old_dir)?;

    if new_dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&new_dir)
    {
        log::warn!(
            "self-update/targz: could not clean stale {}: {e}",
            new_dir.display()
        );
    }

    std::fs::create_dir_all(cache_dir)
        .with_context(|| format!("create cache dir {}", cache_dir.display()))?;

    let tarball = cache_dir.join(format!("update-{}.tar.gz", std::process::id()));
    let download_result = download_with_verification(asset_url, &tarball);
    if let Err(e) = download_result {
        let _ = std::fs::remove_file(&tarball);
        return Err(e);
    }

    let extract_result = extract_and_swap(&tarball, app_dir, &new_dir, &old_dir);
    let _ = std::fs::remove_file(&tarball);
    extract_result
}

fn staging_dirs(app_dir: &Path) -> Result<(PathBuf, PathBuf)> {
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory - refusing to swap at filesystem root")?;
    let name = app_dir
        .file_name()
        .context("app_dir has no file name - refusing to swap")?;
    let name = name.to_string_lossy();
    Ok((
        parent.join(format!("{name}.old")),
        parent.join(format!("{name}.new")),
    ))
}

fn acquire_update_lock(parent: &Path) -> Result<Option<std::fs::File>> {
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let lock_path = parent.join(".paneflow-update.lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("open update lock {}", lock_path.display()))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                bail!(
                    "Another PaneFlow update is already in progress. Wait for it to finish, then retry."
                );
            }
            return Err(err).context("flock update lock");
        }
        Ok(Some(file))
    }
    #[cfg(not(unix))]
    {
        let _ = parent;
        Ok(None)
    }
}

fn recover_and_clean_staging(app_dir: &Path, old_dir: &Path) -> Result<()> {
    if !old_dir.exists() {
        return Ok(());
    }
    if !app_dir.exists() {
        std::fs::rename(old_dir, app_dir).with_context(|| {
            format!(
                "recover live install {} ← {}",
                app_dir.display(),
                old_dir.display()
            )
        })?;
        log::warn!(
            "self-update/targz: recovered live install from a crashed prior update ({})",
            app_dir.display()
        );
        return Ok(());
    }
    if let Err(e) = std::fs::remove_dir_all(old_dir) {
        log::warn!(
            "self-update/targz: could not remove stale {}: {e}",
            old_dir.display()
        );
    }
    Ok(())
}

fn download_with_verification(asset_url: &str, dest: &Path) -> Result<()> {
    super::super::verified_download::download_verified_asset(
        asset_url,
        dest,
        MAX_TARBALL_BYTES,
        UPDATE_HTTP_TIMEOUT,
        "tarball",
    )
}

fn extract_and_swap(tarball: &Path, app_dir: &Path, new_dir: &Path, old_dir: &Path) -> Result<()> {
    let parent = app_dir
        .parent()
        .context("app_dir has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create parent {}", parent.display()))?;

    let scratch = parent.join(format!(".paneflow-extract-{}", std::process::id()));
    if scratch.exists() {
        let _ = std::fs::remove_dir_all(&scratch);
    }
    std::fs::create_dir_all(&scratch)
        .with_context(|| format!("create scratch {}", scratch.display()))?;

    let extract_result = (|| -> Result<()> {
        extract_hardened(tarball, &scratch)?;

        let top = find_single_top_level(&scratch)?;
        if new_dir.exists() {
            let _ = std::fs::remove_dir_all(new_dir);
        }
        std::fs::rename(&top, new_dir)
            .with_context(|| format!("rename {} → {}", top.display(), new_dir.display()))?;

        #[cfg(unix)]
        {
            let bin_path = new_dir.join("bin").join("paneflow");
            if bin_path.exists() {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&bin_path)
                    .with_context(|| format!("stat {}", bin_path.display()))?
                    .permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&bin_path, perms)
                    .with_context(|| format!("chmod 0o755 {}", bin_path.display()))?;
            }
        }
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&scratch);

    extract_result.inspect_err(|_| {
        let _ = std::fs::remove_dir_all(new_dir);
    })?;

    if app_dir.exists() {
        std::fs::rename(app_dir, old_dir)
            .with_context(|| format!("rename {} → {}", app_dir.display(), old_dir.display()))?;
    }
    if let Err(e) = std::fs::rename(new_dir, app_dir) {
        let rollback_msg = if old_dir.exists() {
            match std::fs::rename(old_dir, app_dir) {
                Ok(()) => None,
                Err(rb) => Some(format!(
                    "rollback also failed ({rb}); your previous install is at {}",
                    old_dir.display()
                )),
            }
        } else {
            None
        };
        let _ = std::fs::remove_dir_all(new_dir);
        return match rollback_msg {
            Some(msg) => Err(e).context(format!("rename .new → app_dir ({msg})")),
            None => Err(e).context("rename .new → app_dir"),
        };
    }

    if old_dir.exists() {
        let _ = std::fs::remove_dir_all(old_dir);
    }

    Ok(())
}

fn extract_hardened(tarball: &Path, scratch: &Path) -> Result<()> {
    let f = std::fs::File::open(tarball).with_context(|| format!("open {}", tarball.display()))?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(false);
    archive.set_unpack_xattrs(false);
    archive.set_preserve_mtime(false);

    let canonical_root = std::fs::canonicalize(scratch)
        .with_context(|| format!("canonicalize scratch root {}", scratch.display()))?;

    for entry in archive.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry")?;
        let entry_type = entry.header().entry_type();

        if entry_type.is_symlink() || entry_type.is_hard_link() {
            let where_at = entry
                .path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "<unreadable path>".to_string());
            bail!(
                "Update archive contains a link entry at {where_at}, which PaneFlow refuses to install. Download the release manually from the releases page."
            );
        }

        let path = entry.path().context("read tar entry path")?.into_owned();
        validate_extract_path(&path)?;

        let unpacked = entry
            .unpack_in(&canonical_root)
            .with_context(|| format!("unpack entry {}", path.display()))?;
        if !unpacked {
            bail!(
                "Update archive entry {} escapes the extraction root - refusing to install. Download the release manually from the releases page.",
                path.display()
            );
        }
    }
    Ok(())
}

fn validate_extract_path(path: &Path) -> Result<()> {
    use std::path::Component;
    for comp in path.components() {
        match comp {
            Component::Prefix(_) | Component::RootDir => bail!(
                "Update archive contains an absolute path ({}) - refusing to install.",
                path.display()
            ),
            Component::ParentDir => bail!(
                "Update archive contains a `..` traversal ({}) - refusing to install.",
                path.display()
            ),
            Component::CurDir | Component::Normal(_) => {}
        }
    }
    Ok(())
}

fn find_single_top_level(dir: &Path) -> Result<PathBuf> {
    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("read_dir {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .context("collect dir entries")?;
    entries.sort_by_key(|e| e.file_name());
    match entries.as_slice() {
        [only] => {
            let ty = only.file_type().context("inspect top-level entry")?;
            if !ty.is_dir() {
                bail!("tarball top-level entry is not a directory");
            }
            Ok(only.path())
        }
        [] => bail!("tarball is empty - no top-level entry"),
        multi => bail!("tarball has {} top-level entries, expected 1", multi.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_dirs_derives_sibling_paths() {
        let (old, new) = staging_dirs(Path::new("/home/u/.local/paneflow.app")).unwrap();
        assert_eq!(old, PathBuf::from("/home/u/.local/paneflow.app.old"));
        assert_eq!(new, PathBuf::from("/home/u/.local/paneflow.app.new"));
    }

    fn make_fixture_tarball(root: &Path, marker: &[u8]) -> PathBuf {
        let out = root.join("fixture.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);

        let bin_path = root.join("fixture-src/paneflow.app/bin/paneflow");
        std::fs::create_dir_all(bin_path.parent().unwrap()).unwrap();
        std::fs::write(&bin_path, marker).unwrap();

        builder
            .append_dir_all("paneflow.app", root.join("fixture-src/paneflow.app"))
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        out
    }

    #[test]
    fn extract_and_swap_replaces_existing_app_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let tarball = make_fixture_tarball(root, b"new-version");

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"old-version").unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();

        let content = std::fs::read(app_dir.join("bin/paneflow")).unwrap();
        assert_eq!(content, b"new-version");
        assert!(!old_dir.exists(), ".old should be cleaned up");
        assert!(!new_dir.exists(), ".new should be gone post-swap");
    }

    #[test]
    fn extract_and_swap_works_when_app_dir_absent() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let tarball = make_fixture_tarball(root, b"fresh-install");

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"fresh-install"
        );
    }

    #[test]
    fn extract_and_swap_rolls_back_on_corrupt_tarball() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let corrupt = root.join("corrupt.tar.gz");
        std::fs::write(&corrupt, b"not actually a gzip").unwrap();

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"keep-me").unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        let r = extract_and_swap(&corrupt, &app_dir, &new_dir, &old_dir);
        assert!(r.is_err(), "corrupt tarball must fail");

        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"keep-me"
        );
        assert!(!new_dir.exists(), ".new must be cleaned up on failure");
        assert!(!old_dir.exists(), ".old must not exist on failure");
    }

    #[test]
    fn recover_restores_live_install_when_app_dir_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        std::fs::create_dir_all(old_dir.join("bin")).unwrap();
        std::fs::write(old_dir.join("bin/paneflow"), b"prev-version").unwrap();

        recover_and_clean_staging(&app_dir, &old_dir).unwrap();

        assert!(app_dir.exists(), "live install must be restored from .old");
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"prev-version"
        );
        assert!(!old_dir.exists(), ".old must be consumed by the recovery");
    }

    #[test]
    fn recover_removes_stale_old_when_app_dir_intact() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        std::fs::create_dir_all(app_dir.join("bin")).unwrap();
        std::fs::write(app_dir.join("bin/paneflow"), b"live").unwrap();
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("junk"), b"x").unwrap();

        recover_and_clean_staging(&app_dir, &old_dir).unwrap();

        assert!(!old_dir.exists(), "stale .old must be removed");
        assert_eq!(
            std::fs::read(app_dir.join("bin/paneflow")).unwrap(),
            b"live",
            "live install must be untouched"
        );
    }

    #[test]
    fn recover_is_noop_when_no_old_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app_dir = tmp.path().join(".local/paneflow.app");
        std::fs::create_dir_all(&app_dir).unwrap();
        let (old_dir, _new) = staging_dirs(&app_dir).unwrap();
        recover_and_clean_staging(&app_dir, &old_dir).unwrap();
        assert!(app_dir.exists());
    }

    #[cfg(unix)]
    #[test]
    fn update_lock_is_exclusive_then_released_on_drop() {
        let tmp = tempfile::TempDir::new().unwrap();
        let parent = tmp.path();
        let guard = acquire_update_lock(parent).unwrap();
        assert!(guard.is_some());
        let second = acquire_update_lock(parent);
        assert!(second.is_err(), "second concurrent lock must be refused");
        drop(guard);
        let third = acquire_update_lock(parent).unwrap();
        assert!(third.is_some(), "lock must be re-acquirable after release");
    }

    #[test]
    fn never_writes_outside_its_roots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        let outside = root.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("a"), b"a-body").unwrap();
        std::fs::write(outside.join("b"), b"b-body").unwrap();
        std::fs::create_dir(outside.join("sub")).unwrap();
        std::fs::write(outside.join("sub/c"), b"c-body").unwrap();
        let before = snapshot_tree(&outside);

        let tarball = make_fixture_tarball(root, b"body");
        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        extract_and_swap(&tarball, &app_dir, &new_dir, &old_dir).unwrap();

        let after = snapshot_tree(&outside);
        assert_eq!(before, after, "updater touched the outside subtree");
    }

    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut out: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            for entry in std::fs::read_dir(&p).unwrap().flatten() {
                let ft = entry.file_type().unwrap();
                if ft.is_dir() {
                    stack.push(entry.path());
                } else if ft.is_file() {
                    let rel = entry.path().strip_prefix(root).unwrap().to_path_buf();
                    out.push((rel, std::fs::read(entry.path()).unwrap()));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn validate_extract_path_accepts_normal_relative_paths() {
        assert!(validate_extract_path(Path::new("paneflow.app/bin/paneflow")).is_ok());
        assert!(validate_extract_path(Path::new("./paneflow.app/lib/x.so")).is_ok());
    }

    #[test]
    fn validate_extract_path_rejects_traversal_and_absolute() {
        assert!(validate_extract_path(Path::new("../evil")).is_err());
        assert!(validate_extract_path(Path::new("paneflow.app/../../etc/passwd")).is_err());
        assert!(validate_extract_path(Path::new("/etc/passwd")).is_err());
    }

    #[test]
    fn extract_rejects_path_traversal_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let out = root.join("evil.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        let payload = b"pwned";
        let mut header = tar::Header::new_gnu();
        header.set_size(payload.len() as u64);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o644);
        {
            let name = b"../evil";
            let bytes = header.as_mut_bytes();
            bytes[..name.len()].copy_from_slice(name);
        }
        header.set_cksum();
        builder.append(&header, &payload[..]).unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).unwrap();
        let err = extract_hardened(&out, &scratch).unwrap_err().to_string();
        assert!(
            err.contains("traversal") || err.contains("escapes"),
            "expected traversal rejection, got: {err}"
        );
        assert!(
            !root.join("evil").exists(),
            "traversal entry must not write outside the scratch root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn extract_rejects_tarball_with_symlink() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();

        let fixture_src = root.join("src/paneflow.app/bin");
        std::fs::create_dir_all(&fixture_src).unwrap();
        std::fs::write(fixture_src.join("paneflow"), b"real-bin").unwrap();
        let evil_link = fixture_src.join("evil-link");
        std::os::unix::fs::symlink("/etc/passwd", &evil_link).unwrap();

        let out = root.join("fixture.tar.gz");
        let f = std::fs::File::create(&out).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut builder = tar::Builder::new(gz);
        builder.follow_symlinks(false);
        builder
            .append_dir_all("paneflow.app", root.join("src/paneflow.app"))
            .unwrap();
        builder.into_inner().unwrap().finish().unwrap();

        let app_dir = root.join("home/.local/paneflow.app");
        std::fs::create_dir_all(app_dir.parent().unwrap()).unwrap();
        let (old_dir, new_dir) = staging_dirs(&app_dir).unwrap();

        let r = extract_and_swap(&out, &app_dir, &new_dir, &old_dir);
        let err = r.unwrap_err().to_string();
        assert!(
            err.contains("link entry"),
            "expected link rejection, got: {err}"
        );
        assert!(!app_dir.exists(), "a failed update must not leave app_dir");
        assert!(!new_dir.exists(), ".new must be cleaned up");
    }
}
