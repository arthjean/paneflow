//! Session-scoped agent hook installation.

mod agents;
mod claude;
mod codex;
mod hermes;
mod opencode;
mod owned_files;

pub(crate) use agents::{
    merge_cursor_hooks, merge_gemini_hooks, merge_qoder_hooks, remove_cursor_hooks,
    remove_gemini_hooks, remove_qoder_hooks, ManagedHookConfigGuard, ManagedHookSpec,
};
pub(crate) use claude::HookConfigGuard;
#[cfg(unix)]
pub(crate) use codex::CodexHookConfigGuard;
#[cfg(all(test, unix))]
pub(crate) use codex::{enable_codex_feature_flag, CODEX_HOOK_EVENTS, CODEX_TOML_MARKER};
#[cfg(not(unix))]
pub(crate) use codex::{merge_codex_hooks, remove_codex_hooks};
pub(crate) use hermes::HermesHookConfigGuard;
#[cfg(test)]
pub(crate) use hermes::{hermes_managed_block, strip_hermes_managed_block, HERMES_BLOCK_BEGIN};
pub(crate) use opencode::OpenCodePluginGuard;
#[cfg(test)]
pub(crate) use owned_files::PANEFLOW_TS_BASENAME;
pub(crate) use owned_files::{GrokHookFileGuard, PiExtensionGuard};

use crate::locate_sibling_hook_binary;
#[cfg(test)]
use paneflow_agent_config::claude_hooks::command_program_token;
#[cfg(all(test, not(windows)))]
use paneflow_agent_config::claude_hooks::{display_hook_program, shell_program_path};
pub(crate) use paneflow_agent_config::claude_hooks::{
    is_managed_group as is_paneflow_matcher_group, is_paneflow_hook_command, CLAUDE_HOOK_EVENTS,
};
use paneflow_agent_config::claude_hooks::{
    paneflow_hook_program_token, reconcile_matcher_hooks_replacing_invalid_container,
    remove_matcher_hooks_lenient, render_bare_hook_command, render_hook_command, HookConfigError,
};
use paneflow_agent_config::{read_optional_text, with_config_lock, write_json_atomic, ConfigLease};
use std::env;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InvalidJsonPolicy {
    Replace,
    Refuse,
}

pub(crate) type HookLease = ConfigLease;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookInstallSkip {
    IpcUnavailable,
    PersistentClaudeHook,
    UnsupportedTool,
}

pub(crate) enum HookInstall<T> {
    Installed(T),
    Skipped(HookInstallSkip),
}

impl<T> HookInstall<T> {
    pub(crate) fn map<U>(self, map: impl FnOnce(T) -> U) -> HookInstall<U> {
        match self {
            Self::Installed(value) => HookInstall::Installed(map(value)),
            Self::Skipped(reason) => HookInstall::Skipped(reason),
        }
    }
}

pub(crate) type HookInstallResult<T> = std::io::Result<HookInstall<T>>;

pub(crate) fn safe_path_display(path: &Path) -> String {
    safe_log_text(&path.display().to_string())
}

fn safe_log_text(text: &str) -> String {
    text.chars()
        .map(|character| {
            if (' '..='~').contains(&character) {
                character
            } else {
                '?'
            }
        })
        .collect()
}

pub(crate) fn paneflow_ipc_reachable() -> bool {
    reachable_from_socket_env(env::var_os("PANEFLOW_SOCKET_PATH").as_deref())
}

fn reachable_from_socket_env(raw: Option<&OsStr>) -> bool {
    let Some(raw) = raw.filter(|value| !value.is_empty()) else {
        return false;
    };
    #[cfg(not(windows))]
    {
        Path::new(raw).exists()
    }
    #[cfg(windows)]
    {
        let _ = raw;
        true
    }
}

pub(crate) fn config_dir_is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
}

fn home_unavailable() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "home directory is unavailable",
    )
}

fn refuse_symlink(path: &Path, label: &str) -> std::io::Result<()> {
    if config_dir_is_symlink(path) {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing to write through a symlinked {label} directory"),
        ))
    } else {
        Ok(())
    }
}

pub(super) fn with_last_lease<T>(
    lock_path: &Path,
    lease: &mut HookLease,
    cleanup: impl FnOnce(bool) -> std::io::Result<T>,
) -> std::io::Result<Option<T>> {
    with_config_lock(lock_path, || {
        let Some(mut last) = lease.try_take_last()? else {
            return Ok(None);
        };
        let created = last.take_created()?;
        cleanup(created).map(Some)
    })
}

pub(super) fn with_orphan_lease<T>(
    resource_path: &Path,
    lock_path: &Path,
    cleanup: impl FnOnce(bool) -> std::io::Result<T>,
) -> std::io::Result<Option<T>> {
    let mut lease = HookLease::acquire(resource_path)?;
    with_last_lease(lock_path, &mut lease, cleanup)
}

pub(crate) fn sweep_orphan_hook_config(path: &Path, remove: fn(&mut serde_json::Value)) {
    if path.parent().is_some_and(config_dir_is_symlink) {
        return;
    }
    let _ = with_orphan_lease(path, path, |created_file| {
        let Some(content) = read_optional_text(path)? else {
            return Ok(());
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Ok(());
        };
        let before = root.clone();
        remove(&mut root);
        if root == before {
            return Ok(());
        }
        if created_file && root.as_object().is_some_and(serde_json::Map::is_empty) {
            std::fs::remove_file(path)
        } else {
            write_json_atomic(path, &root)
        }
    });
}

pub(crate) struct InstalledHookConfig {
    pub(crate) path: PathBuf,
    pub(crate) created_file: bool,
    pub(crate) created_directory: bool,
    pub(crate) lease: HookLease,
}

pub(crate) fn install_hook_config_file(
    directory: &Path,
    filename: &str,
    label: &str,
    merge: impl FnOnce(&mut serde_json::Value) -> std::io::Result<()>,
    invalid_json: InvalidJsonPolicy,
) -> std::io::Result<InstalledHookConfig> {
    if config_dir_is_symlink(directory) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("refusing to write {label} hooks through a symlink"),
        ));
    }
    let directory_existed = directory.is_dir();
    if directory.exists() && !directory_existed {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            format!("{} is not a directory", safe_path_display(directory)),
        ));
    }
    if !directory_existed {
        std::fs::create_dir_all(directory)?;
    }

    let path = directory.join(filename);
    let mut lease = match HookLease::acquire(&path) {
        Ok(lease) => lease,
        Err(error) => {
            if !directory_existed {
                let _ = std::fs::remove_dir(directory);
            }
            return Err(error);
        }
    };
    let result = with_config_lock(&path, || {
        let existing = read_optional_text(&path)?;
        let created_file = existing.is_none();
        let existing = existing.unwrap_or_default();
        let mut root = if existing.trim().is_empty() {
            serde_json::json!({})
        } else {
            match serde_json::from_str(&existing) {
                Ok(root) => root,
                Err(error) if invalid_json == InvalidJsonPolicy::Replace => {
                    eprintln!(
                        "paneflow-shim: {} contained invalid JSON ({error}); replacing it",
                        safe_path_display(&path)
                    );
                    serde_json::json!({})
                }
                Err(error) => {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
                }
            }
        };
        merge(&mut root)?;
        write_json_atomic(&path, &root)?;
        if created_file {
            lease.mark_created()?;
        }
        Ok(created_file)
    });
    let created_file = match result {
        Ok(created_file) => created_file,
        Err(error) => {
            if !directory_existed {
                let _ = std::fs::remove_dir(directory);
            }
            return Err(error);
        }
    };
    Ok(InstalledHookConfig {
        path,
        created_file,
        created_directory: !directory_existed,
        lease,
    })
}

pub(crate) fn cleanup_hook_config_file(
    path: &Path,
    directory: &Path,
    created_file: bool,
    created_directory: bool,
    remove: fn(&mut serde_json::Value),
    lease: &mut HookLease,
) {
    let remove_directory = with_last_lease(path, lease, |lease_created_file| {
        let Some(content) = read_optional_text(path)? else {
            return Ok(false);
        };
        let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&content) else {
            return Ok(false);
        };
        remove(&mut root);
        let empty = root.as_object().is_some_and(serde_json::Map::is_empty);
        let owned_file = created_file || lease_created_file;
        if empty && owned_file {
            let _ = std::fs::remove_file(path);
        } else {
            write_json_atomic(path, &root)?;
        }
        Ok(empty && owned_file && created_directory)
    })
    .ok()
    .flatten()
    .unwrap_or(false);
    if remove_directory {
        let _ = std::fs::remove_dir(directory);
    }
}

pub(crate) fn resolve_hook_command(event: &str) -> String {
    locate_sibling_hook_binary().map_or_else(
        || render_bare_hook_command(event),
        |path| render_hook_command(&path, event),
    )
}

fn resolve_plain_hook_command(event: &str) -> String {
    resolve_hook_command(event)
}

pub(crate) fn merge_paneflow_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    paneflow_agent_config::claude_hooks::reconcile_hooks_replacing_invalid_container(
        root,
        resolve_hook_command,
    )
    .map(|_| ())
    .map_err(hook_config_error)
}

pub(crate) fn merge_codebuddy_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    merge_strict_matcher_hooks_for_events(root, CLAUDE_HOOK_EVENTS)
}

pub(crate) fn remove_paneflow_hooks(root: &mut serde_json::Value) {
    paneflow_agent_config::claude_hooks::remove_hooks_lenient(root);
}

fn plain_hook_handler(event: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": resolve_plain_hook_command(event),
        "timeout": 5,
    })
}

fn merge_strict_matcher_hooks_for_events(
    root: &mut serde_json::Value,
    events: &[&str],
) -> std::io::Result<()> {
    reconcile_matcher_hooks_replacing_invalid_container(
        root,
        events,
        |event| serde_json::json!({ "hooks": [plain_hook_handler(event)] }),
    )
    .map(|_| ())
    .map_err(hook_config_error)
}

fn remove_matcher_hooks_for_events(root: &mut serde_json::Value, events: &[&str]) {
    remove_matcher_hooks_lenient(root, events);
}

fn hook_config_error(error: HookConfigError) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, error)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use serde_json::json;

    #[test]
    fn socket_reachability_rejects_absent_values() {
        assert!(!reachable_from_socket_env(None));
        assert!(!reachable_from_socket_env(Some(OsStr::new(""))));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_probe_is_passive() {
        let temp = tempfile::TempDir::new().unwrap();
        let socket = temp.path().join("paneflow.sock");
        assert!(!reachable_from_socket_env(Some(socket.as_os_str())));
        std::fs::File::create(&socket).unwrap();
        assert!(reachable_from_socket_env(Some(socket.as_os_str())));
    }

    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_is_not_opened_by_the_probe() {
        assert!(reachable_from_socket_env(Some(OsStr::new(
            r"\\.\pipe\paneflow"
        ))));
    }

    #[test]
    fn orphan_sweep_preserves_file_when_ownership_is_unknown() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("hooks.json");
        let mut root = serde_json::json!({});
        merge_paneflow_hooks(&mut root).unwrap();
        write_json_atomic(&path, &root).unwrap();

        sweep_orphan_hook_config(&path, remove_paneflow_hooks);

        assert!(path.exists());
        let cleaned: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(cleaned, serde_json::json!({}));
    }

    #[test]
    fn quoted_hook_commands_round_trip() {
        assert_eq!(
            command_program_token("'/tmp/Pane Flow/paneflow-ai-hook' Stop").as_deref(),
            Some("/tmp/Pane Flow/paneflow-ai-hook")
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_shell_path_quotes_spaces() {
        let path = Path::new("/tmp/Pane Flow/paneflow-ai-hook");
        let command = format!("{} Stop", shell_program_path(path));
        assert!(is_paneflow_hook_command(&command));
        assert_eq!(
            display_hook_program(path),
            "/tmp/Pane Flow/paneflow-ai-hook"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_strict_commands_use_the_canonical_renderer() {
        let mut root = json!({});
        merge_codebuddy_hooks(&mut root).unwrap();
        for event in CLAUDE_HOOK_EVENTS {
            let handler = &root["hooks"][*event][0]["hooks"][0];
            let command = handler["command"].as_str().unwrap();
            assert!(command.starts_with("powershell.exe -NoProfile"));
            assert!(handler.get("commandWindows").is_none());
        }
    }
}
