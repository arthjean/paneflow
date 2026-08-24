use super::{
    home_unavailable, merge_strict_matcher_hooks_for_events, paneflow_ipc_reachable,
    refuse_symlink, with_last_lease, with_orphan_lease, HookInstall, HookInstallResult,
    HookInstallSkip, HookLease,
};
use paneflow_agent_config::{home_dir, with_config_lock, write_json_atomic, write_text_atomic};
use std::path::{Path, PathBuf};

pub(crate) const PANEFLOW_TS_BASENAME: &str = "paneflow-status.ts";
const PI_EXTENSION_SOURCE: &str = include_str!("../../assets/pi-paneflow-status.ts");

pub(crate) struct PiExtensionGuard {
    path: PathBuf,
    lease: HookLease,
}

impl PiExtensionGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let home = home_dir().ok_or_else(home_unavailable)?;
        let directory = home.join(".pi").join("agent").join("extensions");
        let path = directory.join(PANEFLOW_TS_BASENAME);
        if !paneflow_ipc_reachable() {
            sweep_owned_file(&path);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        refuse_symlink(directory, "Pi extension")?;
        std::fs::create_dir_all(directory)?;
        let path = directory.join(PANEFLOW_TS_BASENAME);
        let lease = HookLease::acquire(&path)?;
        with_config_lock(&path, || write_text_atomic(&path, PI_EXTENSION_SOURCE))?;
        Ok(Self { path, lease })
    }
}

impl Drop for PiExtensionGuard {
    fn drop(&mut self) {
        cleanup_owned_file(&self.path, &mut self.lease);
    }
}

const GROK_HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];

pub(crate) struct GrokHookFileGuard {
    path: PathBuf,
    lease: HookLease,
}

impl GrokHookFileGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let home = home_dir().ok_or_else(home_unavailable)?;
        let directory = home.join(".grok").join("hooks");
        let path = directory.join("paneflow.json");
        if !paneflow_ipc_reachable() {
            sweep_owned_file(&path);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        refuse_symlink(directory, "Grok hook")?;
        std::fs::create_dir_all(directory)?;
        let path = directory.join("paneflow.json");
        let lease = HookLease::acquire(&path)?;
        let mut root = serde_json::json!({});
        merge_strict_matcher_hooks_for_events(&mut root, GROK_HOOK_EVENTS)?;
        with_config_lock(&path, || write_json_atomic(&path, &root))?;
        Ok(Self { path, lease })
    }
}

impl Drop for GrokHookFileGuard {
    fn drop(&mut self) {
        cleanup_owned_file(&self.path, &mut self.lease);
    }
}

fn sweep_owned_file(path: &Path) {
    let _ = with_orphan_lease(path, path, |_| match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    });
}

fn cleanup_owned_file(path: &Path, lease: &mut HookLease) {
    let _ = with_last_lease(path, lease, |_| match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    });
}
