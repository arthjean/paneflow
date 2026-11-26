use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::limits::HOST_IDLE_EXIT;

pub const PROTOCOL_VERSION: u32 = 7;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub idle_exit_ms: u64,
}

impl Hello {
    pub fn current() -> Self {
        Self::with_idle_exit(HOST_IDLE_EXIT)
    }

    pub fn with_idle_exit(idle_exit: Duration) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            idle_exit_ms: u64::try_from(idle_exit.as_millis()).unwrap_or(u64::MAX),
        }
    }

    pub fn idle_exit(&self) -> Duration {
        Duration::from_millis(self.idle_exit_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol_version: u32,
    pub host_pid: u32,
    #[serde(default)]
    pub engine: String,
    #[serde(default)]
    pub sandbox: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    #[error(
        "browser protocol mismatch: the window speaks version {window}, the browser host speaks version {host}"
    )]
    VersionMismatch { window: u32, host: u32 },
    #[error("the browser profiles in {} are locked by another browser host (lock file {})", root.display(), lock.display())]
    ProfileRootLocked { root: PathBuf, lock: PathBuf },
    #[error("the browser engine could not start: {reason}")]
    EngineFailed { reason: String },
}

pub fn check_version(window: u32, host: u32) -> Result<(), Refusal> {
    if window == host {
        Ok(())
    } else {
        Err(Refusal::VersionMismatch { window, host })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_and_a_host_of_different_versions_are_refused_naming_both() {
        let refusal = check_version(PROTOCOL_VERSION + 1, PROTOCOL_VERSION).unwrap_err();
        let message = refusal.to_string();
        assert!(message.contains(&format!("version {}", PROTOCOL_VERSION + 1)));
        assert!(message.contains(&format!("version {PROTOCOL_VERSION}")));
        assert_eq!(
            refusal,
            Refusal::VersionMismatch {
                window: PROTOCOL_VERSION + 1,
                host: PROTOCOL_VERSION
            }
        );
    }

    #[test]
    fn the_same_version_is_accepted() {
        assert!(check_version(PROTOCOL_VERSION, PROTOCOL_VERSION).is_ok());
    }

    #[test]
    fn the_window_asks_for_the_declared_idle_period() {
        assert_eq!(Hello::current().idle_exit(), HOST_IDLE_EXIT);
    }
}
