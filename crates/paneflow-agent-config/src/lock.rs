use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Error, ErrorKind, Result};
use std::path::Path;
use std::time::{Duration, Instant};

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(25);

#[must_use = "the config lock is released as soon as this guard is dropped"]
pub struct ConfigLock {
    _file: File,
}

pub fn lock_config(path: &Path) -> Result<ConfigLock> {
    let paneflow_dir = paneflow_home::paneflow_home().ok_or_else(|| {
        Error::new(
            ErrorKind::NotFound,
            "could not resolve the Paneflow home directory",
        )
    })?;
    lock_config_in(&paneflow_dir, path)
}

pub fn lock_config_in(paneflow_dir: &Path, path: &Path) -> Result<ConfigLock> {
    std::fs::create_dir_all(paneflow_dir)?;
    acquire_lock(&paneflow_dir.join("agent-config.lock"), path, LOCK_TIMEOUT)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
