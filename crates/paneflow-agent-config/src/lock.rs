use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(25);
/// Cross-process lock for Paneflow's agent-configuration mutations.
///
/// The file is intentionally persistent. The operating system owns the
/// actual lock and releases it when the process exits, including after a
/// crash. Keeping one global lock outside agent-owned directories also lets
/// ephemeral `.claude` and `.codex` directories be removed safely.
pub struct ConfigLock {
    _file: File,
}

fn lock_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "could not resolve the user configuration directory",
        )
    })?;
    let paneflow_dir = config_dir.join("paneflow");
    std::fs::create_dir_all(&paneflow_dir)?;
    Ok(paneflow_dir.join("agent-config.lock"))
}

/// Acquire the shared Paneflow lock for `path`.
///
/// All agent configurations share one lock because their read-modify-write
/// sections are short. This avoids per-project lockfile debris while retaining
/// correct crash recovery on Linux, macOS, and Windows.
pub fn lock_config(path: &Path) -> Result<ConfigLock> {
    acquire_lock(&lock_path()?, path, LOCK_TIMEOUT)
}

fn acquire_lock(lock_path: &Path, target: &Path, timeout: Duration) -> Result<ConfigLock> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(ConfigLock { _file: file }),
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(Error::new(
                        ErrorKind::TimedOut,
                        format!(
                            "timed out waiting for the Paneflow config lock while editing {}",
                            target.display()
                        ),
                    ));
                }
                std::thread::sleep(LOCK_RETRY);
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

/// Run `operation` while holding the shared lock for `path`.
pub fn with_config_lock<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    let _lock = lock_config(path)?;
    operation()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let dir = tempfile_path();
        let config = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("agent-config.lock");
        let first = acquire_lock(&lock_path, &config, Duration::from_secs(1)).unwrap();
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        assert!(matches!(
            contender.try_lock(),
            Err(TryLockError::WouldBlock)
        ));
        drop(first);
        contender.try_lock().unwrap();
        drop(contender);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unlocked_persistent_lockfile_is_recoverable() {
        let dir = tempfile_path();
        let config = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        let lock_path = dir.join("agent-config.lock");
        std::fs::write(&lock_path, b"left by a terminated process").unwrap();
        let first = acquire_lock(&lock_path, &config, Duration::from_secs(1)).unwrap();
        drop(first);
        let second = acquire_lock(&lock_path, &config, Duration::from_secs(1)).unwrap();
        drop(second);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn tempfile_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "paneflow-agent-config-lock-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ))
    }
}
