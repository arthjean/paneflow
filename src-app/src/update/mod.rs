pub mod checker;
pub mod error;
pub mod install_method;
pub mod linux;
pub mod macos;
pub mod release_notes;
pub mod signature;
pub(crate) mod verified_download;
pub mod windows;

#[cfg(target_os = "linux")]
pub mod migrations;

pub use error::UpdateError;

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
