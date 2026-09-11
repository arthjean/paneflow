use super::owned_files::{cleanup_owned_file, sweep_owned_file};
use super::{
    home_unavailable, merge_strict_matcher_hooks_for_events, paneflow_ipc_reachable,
    refuse_symlink, HookInstall, HookInstallResult, HookInstallSkip, HookLease,
};
use paneflow_agent_config::{home_dir, with_config_lock, write_json_atomic, write_text_atomic};
use std::env;
use std::path::{Path, PathBuf};

const DSH_HOOK_EVENTS: &[&str] = &["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"];
pub(crate) const DSH_HOOKS_BASENAME: &str = "hooks.json";
pub(crate) const DSH_OVERLAY_BASENAME: &str = "paneflow-overlay.yml";
const BRIDGE_PACKAGE: &str = "@deepseek-ai/dsh-hooks-claude-code";
const BRIDGE_ROW_ID: &str = "paneflow-hooks";

pub(crate) struct DshOverlayGuard {
    hooks_path: PathBuf,
    overlay_path: PathBuf,
    hooks_lease: HookLease,
    overlay_lease: HookLease,
}

impl DshOverlayGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let home = dsh_home().ok_or_else(home_unavailable)?;
        let directory = home.join("paneflow");
        if !paneflow_ipc_reachable() {
            sweep_overlay(&directory);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        if !bridge_is_resolvable(&home) {
            sweep_overlay(&directory);
            return Ok(HookInstall::Skipped(HookInstallSkip::BridgeUnavailable));
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        refuse_symlink(directory, "DeepSeek Harness overlay")?;
        std::fs::create_dir_all(directory)?;
        let hooks_path = directory.join(DSH_HOOKS_BASENAME);
        let overlay_path = directory.join(DSH_OVERLAY_BASENAME);
        let hooks_lease = HookLease::acquire(&hooks_path)?;
        let overlay_lease = HookLease::acquire(&overlay_path)?;
        let mut root = serde_json::json!({});
        merge_strict_matcher_hooks_for_events(&mut root, DSH_HOOK_EVENTS)?;
        with_config_lock(&hooks_path, || write_json_atomic(&hooks_path, &root))?;
        let overlay = render_overlay(&hooks_path);
        with_config_lock(&overlay_path, || write_text_atomic(&overlay_path, &overlay))?;
        Ok(Self {
            hooks_path,
            overlay_path,
            hooks_lease,
            overlay_lease,
        })
    }

    pub(crate) fn overlay_path(&self) -> &Path {
        &self.overlay_path
    }
}

impl Drop for DshOverlayGuard {
    fn drop(&mut self) {
        cleanup_owned_file(&self.overlay_path, &mut self.overlay_lease);
        cleanup_owned_file(&self.hooks_path, &mut self.hooks_lease);
    }
}

pub(crate) fn dsh_home() -> Option<PathBuf> {
    match env::var_os("DSH_HOME") {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home_dir().map(|home| home.join(".dsh")),
    }
}

fn bridge_is_resolvable(dsh_home: &Path) -> bool {
    dsh_home
        .join("profiles")
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh-hooks-claude-code")
        .is_dir()
}

pub(crate) fn render_overlay(hooks_path: &Path) -> String {
    let config_path = yaml_single_quoted(&hooks_path.to_string_lossy());
    format!(
        "- insert:\n    - id: {BRIDGE_ROW_ID}\n      name: '{BRIDGE_PACKAGE}'\n      config:\n        configPath: '{config_path}'\n"
    )
}

fn yaml_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

fn sweep_overlay(directory: &Path) {
    sweep_owned_file(&directory.join(DSH_OVERLAY_BASENAME));
    sweep_owned_file(&directory.join(DSH_HOOKS_BASENAME));
}
