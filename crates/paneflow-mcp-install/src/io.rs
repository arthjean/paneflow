//! Safe-write primitives (EP-002 US-006).
//!
//! Every agent-config writer goes through [`write_if_changed`], which is:
//! - **idempotent** - a write only happens when the bytes actually differ,
//!   so a re-run of `paneflow mcp install` produces zero disk churn (no
//!   mtime bump, no backup spam);
//! - **backed up** - the previous contents are copied to `<file>.bak`
//!   *before* the new bytes land, and a backup failure aborts the write
//!   (we never modify the original if we could not preserve it first);
//! - **atomic** - bytes are written to a temp file in the same directory
//!   and `rename`d into place, mirroring `session.rs`'s tmp+rename pattern.
//!   A crash mid-write leaves the temp file, never a half-written config.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(50);

/// Best-effort inter-process lock for one config file. Paneflow writers honor
/// it before any read-modify-write pass, so two Paneflow invocations cannot
/// race each other and lose an edit between read and atomic persist.
pub struct ConfigLock {
    path: PathBuf,
    _file: File,
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path(path: &Path) -> PathBuf {
    let mut lock = path.as_os_str().to_owned();
    lock.push(".lock");
    PathBuf::from(lock)
}

/// Acquire the Paneflow lock for `path`, retrying briefly if another
/// Paneflow process is already editing the same config.
pub fn lock_config(path: &Path) -> Result<ConfigLock> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("create parent dir {} failed", parent.display()))?;

    let lock = lock_path(path);
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(file) => {
                return Ok(ConfigLock {
                    path: lock,
                    _file: file,
                })
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists && Instant::now() < deadline => {
                std::thread::sleep(LOCK_RETRY);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                anyhow::bail!("timed out waiting for config lock {}", lock.display());
            }
            Err(e) => return Err(e).with_context(|| format!("lock {} failed", lock.display())),
        }
    }
}

/// Run a closure while holding the Paneflow config lock for `path`.
pub fn with_config_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = lock_config(path)?;
    f()
}

/// Copy `path` to `path` + `.bak` when it exists. Returns the backup path
/// (or `None` if the original did not exist - nothing to preserve).
///
/// A copy failure is an error: callers MUST abort the write rather than
/// risk clobbering a config they could not back up first (US-006 AC).
pub fn backup(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut bak = path.as_os_str().to_owned();
    bak.push(".bak");
    let bak = PathBuf::from(bak);
    std::fs::copy(path, &bak)
        .with_context(|| format!("backup {} -> {} failed", path.display(), bak.display()))?;
    Ok(Some(bak))
}

/// Atomically write `contents` to `path`: temp file in the same directory,
/// flush + fsync, then `rename`. The rename is atomic on POSIX and on
/// Windows NTFS (`MoveFileEx` semantics inside `persist`).
pub fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("create parent dir {} failed", parent.display()))?;

    let mut tmp = tempfile::NamedTempFile::new_in(&parent)
        .with_context(|| format!("tempfile in {} failed", parent.display()))?;
    std::io::Write::write_all(&mut tmp, contents).context("write_all to tempfile failed")?;
    tmp.as_file_mut()
        .sync_all()
        .context("sync_all on tempfile failed")?;
    tmp.persist(path).map_err(|e| {
        anyhow::anyhow!("atomic rename into {} failed: {}", path.display(), e.error)
    })?;
    Ok(())
}

/// Backup-then-atomic-write `contents` to `path`, **only if** the bytes
/// differ from what is already on disk.
///
/// Returns `true` when a write happened, `false` when the on-disk bytes
/// already matched (a no-op - no backup, no rename, no mtime change). This
/// is the idempotency knob every writer relies on.
pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<bool> {
    with_config_lock(path, || write_if_changed_unlocked(path, contents))
}

/// Same as [`write_if_changed`], but assumes the caller already holds
/// [`ConfigLock`] for `path`.
pub(crate) fn write_if_changed_unlocked(path: &Path, contents: &[u8]) -> Result<bool> {
    // Edition 2021 (workspace default) - no let-chains, so nest the guard.
    if let Ok(existing) = std::fs::read(path) {
        if existing == contents {
            return Ok(false);
        }
    }
    // Bytes differ (or the file is absent / unreadable): back up the old
    // contents first, then publish the new bytes atomically.
    backup(path)?;
    write_atomic(path, contents)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_noop_when_absent() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("missing.json");
        assert_eq!(backup(&p).unwrap(), None);
    }

    #[test]
    fn backup_copies_existing() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.json");
        std::fs::write(&p, b"original").unwrap();
        let bak = backup(&p).unwrap().unwrap();
        assert_eq!(std::fs::read(&bak).unwrap(), b"original");
        // Original untouched.
        assert_eq!(std::fs::read(&p).unwrap(), b"original");
    }

    #[test]
    fn write_atomic_creates_file_and_parents() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("nested").join("deep").join("config.json");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
    }

    #[test]
    fn write_if_changed_is_noop_when_identical() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.json");
        std::fs::write(&p, b"same").unwrap();
        let mtime_before = std::fs::metadata(&p).unwrap().modified().unwrap();

        std::thread::sleep(std::time::Duration::from_millis(50));
        let wrote = write_if_changed(&p, b"same").unwrap();

        assert!(!wrote, "identical bytes must not be rewritten");
        let mtime_after = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(mtime_before, mtime_after, "no-op must not bump mtime");
        // No backup was created on the no-op path.
        let mut bak = p.as_os_str().to_owned();
        bak.push(".bak");
        assert!(!PathBuf::from(bak).exists(), "no-op must not write a .bak");
    }

    #[test]
    fn write_if_changed_writes_and_backs_up_on_diff() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("config.json");
        std::fs::write(&p, b"old").unwrap();

        let wrote = write_if_changed(&p, b"new").unwrap();
        assert!(wrote);
        assert_eq!(std::fs::read(&p).unwrap(), b"new");

        let mut bak = p.as_os_str().to_owned();
        bak.push(".bak");
        assert_eq!(
            std::fs::read(PathBuf::from(bak)).unwrap(),
            b"old",
            "backup must hold the pre-write contents"
        );
    }

    #[test]
    fn write_if_changed_creates_new_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("fresh.json");
        let wrote = write_if_changed(&p, b"data").unwrap();
        assert!(wrote);
        assert_eq!(std::fs::read(&p).unwrap(), b"data");
    }
}
