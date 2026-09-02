use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
pub use paneflow_agent_config::ConfigLock;

pub fn lock_config(path: &Path) -> Result<ConfigLock> {
    paneflow_agent_config::lock_config(path)
        .with_context(|| format!("lock {} failed", path.display()))
}

pub fn with_config_lock<T>(path: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = lock_config(path)?;
    f()
}

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

pub fn write_if_changed(path: &Path, contents: &[u8]) -> Result<bool> {
    with_config_lock(path, || write_if_changed_unlocked(path, contents))
}

pub(crate) fn write_if_changed_unlocked(path: &Path, contents: &[u8]) -> Result<bool> {
    if let Ok(existing) = std::fs::read(path) {
        if existing == contents {
            return Ok(false);
        }
    }
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
