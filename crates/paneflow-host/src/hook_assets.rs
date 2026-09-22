use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const CANCELLATION_FILE: &str = "hook-cancellation.json";

pub const MAX_MARKER_BYTES: u64 = 4 * 1024;

const LOCK_TIMEOUT: Duration = Duration::from_secs(5);
const LOCK_RETRY: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Cancellation {
    pub runtime_generation: u64,
    pub cancelled_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submitted_at: Option<u64>,
}

pub fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub fn read_capped(path: &Path, cap: u64) -> Option<(Vec<u8>, SystemTime)> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if metadata.len() > cap {
        log::warn!(
            "paneflow: {} exceeds {cap} bytes and is ignored",
            path.display()
        );
        return None;
    }
    let modified = metadata.modified().ok()?;
    let mut bytes = Vec::new();
    file.take(cap).read_to_end(&mut bytes).ok()?;
    Some((bytes, modified))
}

pub fn read_cancellation(session_dir: &Path) -> Option<Cancellation> {
    let (bytes, _) = read_capped(&session_dir.join(CANCELLATION_FILE), MAX_MARKER_BYTES)?;
    serde_json::from_slice(&bytes).ok()
}

pub fn record_cancellation(
    session_dir: &Path,
    generation: u64,
    cancelled_at: SystemTime,
) -> io::Result<Option<Cancellation>> {
    if !session_dir.is_dir() {
        return Ok(None);
    }
    let path = session_dir.join(CANCELLATION_FILE);
    let _lock = lock_exclusive(&path)?;
    let held = read_cancellation(session_dir);
    let cancelled_at = unix_ms(cancelled_at);
    if held.as_ref().is_some_and(|held| {
        held.runtime_generation > generation
            || (held.runtime_generation == generation && held.cancelled_at >= cancelled_at)
    }) {
        return Ok(None);
    }
    let marker = Cancellation {
        runtime_generation: generation,
        cancelled_at,
        submitted_at: None,
    };
    write_marker(&path, &marker)?;
    Ok(Some(marker))
}

pub fn record_submission(
    session_dir: &Path,
    generation: u64,
    submitted_at: SystemTime,
) -> io::Result<Option<Cancellation>> {
    if !session_dir.is_dir() {
        return Ok(None);
    }
    let path = session_dir.join(CANCELLATION_FILE);
    let _lock = lock_exclusive(&path)?;
    let Some(held) = read_cancellation(session_dir) else {
        return Ok(None);
    };
    let submitted_at = unix_ms(submitted_at);
    if held.runtime_generation != generation
        || held.submitted_at.is_some()
        || submitted_at <= held.cancelled_at
    {
        return Ok(None);
    }
    let marker = Cancellation {
        submitted_at: Some(submitted_at),
        ..held
    };
    write_marker(&path, &marker)?;
    Ok(Some(marker))
}

fn write_marker(path: &Path, marker: &Cancellation) -> io::Result<()> {
    let bytes = serde_json::to_vec(marker).map_err(io::Error::other)?;
    crate::manifest::write_atomically_in_existing_dir(path, &bytes)
}

pub struct AssetLock {
    _file: File,
}

pub fn lock_exclusive(target: &Path) -> io::Result<AssetLock> {
    let path = target.with_extension("lock");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)?;
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(AssetLock { _file: file }),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(LOCK_RETRY);
            }
            Err(TryLockError::WouldBlock) => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out locking {}", path.display()),
                ));
            }
            Err(TryLockError::Error(error)) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(milliseconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(milliseconds)
    }

    #[test]
    fn a_cancellation_is_written_once_and_never_moves_backwards() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_cancellation(dir.path()), None);

        let written = record_cancellation(dir.path(), 2, at(900)).unwrap();
        assert_eq!(
            written,
            Some(Cancellation {
                runtime_generation: 2,
                cancelled_at: 900,
                submitted_at: None,
            })
        );
        assert!(
            record_cancellation(dir.path(), 2, at(800))
                .unwrap()
                .is_none(),
            "an older escape never rewrites the fence"
        );
        assert_eq!(read_cancellation(dir.path()).unwrap().cancelled_at, 900);

        let moved = record_cancellation(dir.path(), 2, at(1_000)).unwrap();
        assert_eq!(moved.map(|marker| marker.cancelled_at), Some(1_000));
    }

    #[test]
    fn only_the_first_submission_after_a_cancellation_is_recorded() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            record_submission(dir.path(), 1, at(500)).unwrap().is_none(),
            "a submission without a fence records nothing"
        );

        record_cancellation(dir.path(), 1, at(900)).unwrap();
        assert!(
            record_submission(dir.path(), 1, at(800)).unwrap().is_none(),
            "an Enter delivered before the escape is not the resumption"
        );
        let first = record_submission(dir.path(), 1, at(1_100)).unwrap();
        assert_eq!(first.and_then(|marker| marker.submitted_at), Some(1_100));
        assert!(
            record_submission(dir.path(), 1, at(1_200))
                .unwrap()
                .is_none(),
            "the paste recipe's second Enter does not replace the first"
        );
        assert_eq!(
            read_cancellation(dir.path()).unwrap().submitted_at,
            Some(1_100)
        );
    }

    #[test]
    fn a_marker_from_another_generation_is_replaced_and_never_extended() {
        let dir = tempfile::tempdir().unwrap();
        record_cancellation(dir.path(), 1, at(900)).unwrap();
        assert!(
            record_submission(dir.path(), 2, at(1_100))
                .unwrap()
                .is_none(),
            "a new generation never resumes the previous turn"
        );
        let next = record_cancellation(dir.path(), 2, at(400)).unwrap();
        assert_eq!(
            next,
            Some(Cancellation {
                runtime_generation: 2,
                cancelled_at: 400,
                submitted_at: None,
            })
        );
    }

    #[test]
    fn a_session_directory_that_does_not_exist_is_never_created_by_a_fence() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("gone");
        assert!(record_cancellation(&missing, 1, at(1)).unwrap().is_none());
        assert!(record_submission(&missing, 1, at(2)).unwrap().is_none());
        assert!(!missing.exists());
    }
}
