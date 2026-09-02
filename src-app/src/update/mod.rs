pub mod checker;
pub mod error;
pub mod install_method;
pub mod linux;
pub mod macos;
pub mod signature;
pub(crate) mod verified_download;
pub mod windows;

#[cfg(target_os = "linux")]
pub mod migrations;

pub use error::UpdateError;

use std::path::PathBuf;

#[cfg(any(target_os = "windows", all(unix, not(target_os = "macos"))))]
use anyhow::Context;
use anyhow::Result;

#[derive(Clone, Debug, Default)]
pub enum SelfUpdateStatus {
    #[default]
    Idle,
    Downloading,
    Installing,
    ReadyToRestart,
    Errored(#[allow(dead_code)] UpdateError),
}

impl SelfUpdateStatus {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            SelfUpdateStatus::Downloading | SelfUpdateStatus::Installing
        )
    }
}

#[allow(dead_code)]
pub fn installed_binary_path() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Ok(PathBuf::from(
            "/Applications/PaneFlow.app/Contents/MacOS/paneflow",
        ))
    }
    #[cfg(target_os = "windows")]
    {
        let program_files = std::env::var_os("ProgramFiles")
            .context("ProgramFiles environment variable is not set")?;
        let mut exe = PathBuf::from(program_files)
            .join("PaneFlow")
            .join("paneflow");
        if !std::env::consts::EXE_EXTENSION.is_empty() {
            exe.set_extension(std::env::consts::EXE_EXTENSION);
        }
        Ok(exe)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let home = std::env::var_os("HOME").context("HOME environment variable is not set")?;
        Ok(PathBuf::from(home).join(".local/bin/paneflow"))
    }
}
