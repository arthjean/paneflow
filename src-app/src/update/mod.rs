pub mod checker;
pub mod error;
pub mod install_method;
pub mod linux;
pub mod macos;
pub mod release_notes;
pub mod signature;
pub(crate) mod swap;
pub(crate) mod verified_download;
pub mod windows;

#[cfg(target_os = "linux")]
pub mod migrations;

pub use error::UpdateError;

pub(crate) const LONGEST_PLATFORM_INSTALL: std::time::Duration =
    std::time::Duration::from_secs(15 * 60);

#[derive(Clone, Debug, Default)]
pub enum SelfUpdateStatus {
    #[default]
    Idle,
    Downloading,
    Installing,
    ReadyToRestart,
    Errored,
}

impl SelfUpdateStatus {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            SelfUpdateStatus::Downloading | SelfUpdateStatus::Installing
        )
    }
}
