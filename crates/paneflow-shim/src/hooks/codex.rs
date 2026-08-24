#[cfg(unix)]
use super::{
    cleanup_hook_config_file, install_hook_config_file, paneflow_ipc_reachable,
    sweep_orphan_hook_config, with_last_lease, with_orphan_lease, HookInstall, HookInstallResult,
    HookInstallSkip, HookLease, InvalidJsonPolicy,
};
use super::{hook_config_error, resolve_hook_command};
use paneflow_agent_config::claude_hooks::{
    command_handler, reconcile_matcher_hooks_replacing_invalid_container,
    remove_matcher_hooks_lenient, MANAGED_MARKER,
};
#[cfg(unix)]
use paneflow_agent_config::{home_dir, read_optional_text, with_config_lock, write_text_atomic};
#[cfg(unix)]
use std::env;
#[cfg(unix)]
use std::path::{Path, PathBuf};

pub(crate) const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
];

#[cfg(unix)]
pub(crate) const CODEX_TOML_MARKER: &str = "# _paneflow_managed: true";
#[cfg(unix)]
const CODEX_TOML_CREATED_FILE_MARKER: &str = "# _paneflow_created_file: true";

pub(crate) fn merge_codex_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    reconcile_matcher_hooks_replacing_invalid_container(root, CODEX_HOOK_EVENTS, |event| {
        serde_json::json!({
            MANAGED_MARKER: true,
            "hooks": [command_handler(resolve_hook_command(event))],
        })
    })
    .map(|_| ())
    .map_err(hook_config_error)
}

pub(crate) fn remove_codex_hooks(root: &mut serde_json::Value) {
    remove_matcher_hooks_lenient(root, CODEX_HOOK_EVENTS);
}

#[cfg(unix)]
fn global_config_toml() -> Option<PathBuf> {
    home_dir().map(|home| home.join(".codex").join("config.toml"))
}

#[cfg(unix)]
pub(crate) struct CodexHookConfigGuard {
    hooks_path: PathBuf,
    project_dir: PathBuf,
    created_hooks_file: bool,
    created_project_dir: bool,
    hooks_lease: HookLease,
    feature_path: Option<PathBuf>,
    feature_lease: Option<HookLease>,
}

#[cfg(unix)]
impl CodexHookConfigGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let project_dir = env::current_dir()?.join(".codex");
        if !paneflow_ipc_reachable() {
            sweep_orphan_hook_config(&project_dir.join("hooks.json"), remove_codex_hooks);
            if let Some(path) = global_config_toml() {
                sweep_orphan_codex_feature_flag(&path);
            }
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(&project_dir, global_config_toml().as_deref()).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(
        project_dir: &Path,
        feature_path: Option<&Path>,
    ) -> std::io::Result<Self> {
        let installed = install_hook_config_file(
            project_dir,
            "hooks.json",
            "Codex",
            merge_codex_hooks,
            InvalidJsonPolicy::Replace,
        )?;
        let mut guard = Self {
            hooks_path: installed.path,
            project_dir: project_dir.to_path_buf(),
            created_hooks_file: installed.created_file,
            created_project_dir: installed.created_directory,
            hooks_lease: installed.lease,
            feature_path: feature_path.map(Path::to_path_buf),
            feature_lease: None,
        };
        if let Some(path) = feature_path {
            guard.feature_lease = Some(HookLease::acquire(path)?);
            enable_codex_feature_flag(path)?;
        }
        Ok(guard)
    }
}

#[cfg(unix)]
impl Drop for CodexHookConfigGuard {
    fn drop(&mut self) {
        if let (Some(path), Some(lease)) =
            (self.feature_path.as_deref(), self.feature_lease.as_mut())
        {
            cleanup_codex_feature_flag(path, lease);
        }
        cleanup_hook_config_file(
            &self.hooks_path,
            &self.project_dir,
            self.created_hooks_file,
            self.created_project_dir,
            remove_codex_hooks,
            &mut self.hooks_lease,
        );
    }
}

#[cfg(unix)]
fn sweep_orphan_codex_feature_flag(path: &Path) {
    let _ = with_orphan_lease(path, path, |_| disable_codex_feature_flag_unlocked(path));
}

#[cfg(unix)]
fn cleanup_codex_feature_flag(path: &Path, lease: &mut HookLease) {
    let _ = with_last_lease(path, lease, |_| disable_codex_feature_flag_unlocked(path));
}

#[cfg(unix)]
pub(crate) fn enable_codex_feature_flag(path: &Path) -> std::io::Result<bool> {
    with_config_lock(path, || {
        enable_codex_feature_flag_unlocked(path).map(|install| install.changed)
    })
}

#[cfg(unix)]
struct CodexFeatureInstall {
    changed: bool,
}

#[cfg(unix)]
fn enable_codex_feature_flag_unlocked(path: &Path) -> std::io::Result<CodexFeatureInstall> {
    let existing = read_optional_text(path)?;
    let created_file = existing.is_none();
    let existing = existing.unwrap_or_default();
    if has_hooks_flag(&existing) {
        return Ok(CodexFeatureInstall { changed: false });
    }
    if has_features_section(&existing) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Codex config already has a features section without hooks",
        ));
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    if !next.is_empty() {
        next.push('\n');
    }
    next.push_str(CODEX_TOML_MARKER);
    if created_file {
        next.push('\n');
        next.push_str(CODEX_TOML_CREATED_FILE_MARKER);
    }
    next.push_str("\n[features]\nhooks = true\n");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_text_atomic(path, &next)?;
    Ok(CodexFeatureInstall { changed: true })
}

#[cfg(unix)]
fn disable_codex_feature_flag_unlocked(path: &Path) -> std::io::Result<()> {
    let Some(existing) = read_optional_text(path)? else {
        return Ok(());
    };
    let Some(block) = strip_codex_feature_block(&existing) else {
        return Ok(());
    };
    if block.created_file && block.content.trim().is_empty() {
        std::fs::remove_file(path)
    } else {
        write_text_atomic(path, &block.content)
    }
}

#[cfg(unix)]
fn has_hooks_flag(content: &str) -> bool {
    content.lines().any(|line| {
        let line = line.trim_start();
        if line.starts_with('#') {
            return false;
        }
        let assignment = line.split_once('#').map_or(line, |(value, _)| value);
        assignment.split_once('=').is_some_and(|(key, value)| {
            matches!(key.trim(), "hooks" | "codex_hooks") && value.trim() == "true"
        })
    })
}

#[cfg(unix)]
fn has_features_section(content: &str) -> bool {
    content.lines().any(|line| line.trim() == "[features]")
}

#[cfg(unix)]
struct StrippedCodexFeatureBlock {
    content: String,
    created_file: bool,
}

#[cfg(unix)]
fn strip_codex_feature_block(content: &str) -> Option<StrippedCodexFeatureBlock> {
    let lines: Vec<&str> = content.lines().collect();
    let marker = lines
        .iter()
        .position(|line| line.trim() == CODEX_TOML_MARKER)?;
    let created_file = lines
        .get(marker + 1)
        .is_some_and(|line| line.trim() == CODEX_TOML_CREATED_FILE_MARKER);
    let section = marker + 1 + usize::from(created_file);
    let managed = lines.get(section..section + 2)?;
    let flag = managed[1].trim();
    if managed[0].trim() != "[features]" || !matches!(flag, "hooks = true" | "codex_hooks = true") {
        return None;
    }

    let mut head_end = marker;
    if head_end > 0 && lines[head_end - 1].is_empty() {
        head_end -= 1;
    }
    let mut output = String::new();
    for line in lines[..head_end].iter().chain(lines[section + 2..].iter()) {
        output.push_str(line);
        output.push('\n');
    }
    if !content.ends_with('\n') && output.ends_with('\n') {
        output.pop();
    }
    Some(StrippedCodexFeatureBlock {
        content: output,
        created_file,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_codex_group_preserves_user_handler() {
        let mut root = serde_json::json!({
            "hooks": {
                "Stop": [{
                    MANAGED_MARKER: true,
                    "hooks": [
                        { "type": "command", "command": "paneflow-ai-hook Stop" },
                        { "type": "command", "command": "my-user-hook" }
                    ]
                }]
            }
        });

        merge_codex_hooks(&mut root).unwrap();
        remove_codex_hooks(&mut root);

        let groups = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "my-user-hook");
    }

    #[cfg(unix)]
    #[test]
    fn global_feature_flag_lives_until_the_last_session() {
        let temp = tempfile::TempDir::new().unwrap();
        let feature = temp.path().join("global").join("config.toml");
        let first_project = temp.path().join("first").join(".codex");
        let second_project = temp.path().join("second").join(".codex");
        let first = CodexHookConfigGuard::install_at(&first_project, Some(&feature)).unwrap();
        let second = CodexHookConfigGuard::install_at(&second_project, Some(&feature)).unwrap();

        drop(first);
        assert!(std::fs::read_to_string(&feature)
            .unwrap()
            .contains("hooks = true"));

        drop(second);
        assert!(!feature.exists());
    }

    #[cfg(unix)]
    #[test]
    fn orphan_sweep_waits_for_live_feature_lease() {
        let temp = tempfile::TempDir::new().unwrap();
        let feature = temp.path().join("config.toml");
        enable_codex_feature_flag(&feature).unwrap();
        let live = HookLease::acquire(&feature).unwrap();

        sweep_orphan_codex_feature_flag(&feature);
        assert!(feature.exists());

        drop(live);
        sweep_orphan_codex_feature_flag(&feature);
        assert!(!feature.exists());
    }

    #[cfg(unix)]
    #[test]
    fn feature_cleanup_preserves_preexisting_empty_file() {
        let temp = tempfile::TempDir::new().unwrap();
        let feature = temp.path().join("config.toml");
        std::fs::write(&feature, "").unwrap();
        let project = temp.path().join("project").join(".codex");

        let guard = CodexHookConfigGuard::install_at(&project, Some(&feature)).unwrap();
        drop(guard);

        assert!(feature.exists());
        assert_eq!(std::fs::read_to_string(feature).unwrap(), "");
    }
}
