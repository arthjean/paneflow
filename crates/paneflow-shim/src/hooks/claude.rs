use super::{
    cleanup_hook_config_file, install_hook_config_file, is_paneflow_hook_command,
    is_paneflow_matcher_group, merge_paneflow_hooks, paneflow_hook_program_token,
    paneflow_ipc_reachable, remove_paneflow_hooks, safe_log_text, sweep_orphan_hook_config,
    HookInstall, HookInstallResult, HookInstallSkip, HookLease, InvalidJsonPolicy,
    CLAUDE_HOOK_EVENTS,
};
use paneflow_agent_config::{home_dir, read_optional_text};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
enum PersistentHookState {
    Absent,
    Alive { command: String },
    Stale { command: Option<String> },
}

fn persistent_claude_hooks_state() -> std::io::Result<PersistentHookState> {
    let home = home_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "home directory is unavailable",
        )
    })?;
    let path = home.join(".claude").join("settings.json");
    persistent_claude_hooks_state_at(&path)
}

fn persistent_claude_hooks_state_at(path: &Path) -> std::io::Result<PersistentHookState> {
    let Some(content) = read_optional_text(path)? else {
        return Ok(PersistentHookState::Absent);
    };
    let root = serde_json::from_str(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(settings_managed_hook_state(&root))
}

#[cfg(test)]
fn settings_has_managed_hook(root: &serde_json::Value) -> bool {
    settings_managed_hook_state(root) != PersistentHookState::Absent
}

fn settings_managed_hook_state(root: &serde_json::Value) -> PersistentHookState {
    let Some(hooks) = root.get("hooks").and_then(serde_json::Value::as_object) else {
        return PersistentHookState::Absent;
    };
    let mut stale = None;
    let mut managed_without_command = false;
    for event in CLAUDE_HOOK_EVENTS {
        let Some(groups) = hooks.get(*event).and_then(serde_json::Value::as_array) else {
            continue;
        };
        for group in groups
            .iter()
            .filter(|group| is_paneflow_matcher_group(group))
        {
            let commands = paneflow_hook_commands_in_group(group);
            managed_without_command |= commands.is_empty();
            for command in commands {
                if paneflow_hook_command_program_exists(&command) {
                    return PersistentHookState::Alive { command };
                }
                stale.get_or_insert(command);
            }
        }
    }
    if stale.is_some() || managed_without_command {
        PersistentHookState::Stale { command: stale }
    } else {
        PersistentHookState::Absent
    }
}

pub(crate) struct HookConfigGuard {
    path: PathBuf,
    directory: PathBuf,
    created_file: bool,
    created_directory: bool,
    lease: HookLease,
}

impl HookConfigGuard {
    pub(crate) fn install() -> HookInstallResult<Self> {
        let directory = env::current_dir()?.join(".claude");
        let path = directory.join("settings.local.json");
        if !paneflow_ipc_reachable() {
            sweep_orphan_hook_config(&path, remove_paneflow_hooks);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        match persistent_claude_hooks_state()? {
            PersistentHookState::Alive { command } => {
                crate::diagnose(&format!(
                    "claude: using persistent hook ({})",
                    safe_log_text(&command)
                ));
                sweep_orphan_hook_config(&path, remove_paneflow_hooks);
                return Ok(HookInstall::Skipped(HookInstallSkip::PersistentClaudeHook));
            }
            PersistentHookState::Stale { command } => {
                crate::diagnose(&format!(
                    "claude: ignoring stale persistent hook ({})",
                    command
                        .as_deref()
                        .map(safe_log_text)
                        .unwrap_or_else(|| "missing command".into())
                ));
            }
            PersistentHookState::Absent => {}
        }
        Self::install_at(&directory).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(directory: &Path) -> std::io::Result<Self> {
        let installed = install_hook_config_file(
            directory,
            "settings.local.json",
            "Claude Code",
            merge_paneflow_hooks,
            InvalidJsonPolicy::Replace,
        )?;
        Ok(Self {
            path: installed.path,
            directory: directory.to_path_buf(),
            created_file: installed.created_file,
            created_directory: installed.created_directory,
            lease: installed.lease,
        })
    }
}

impl Drop for HookConfigGuard {
    fn drop(&mut self) {
        cleanup_hook_config_file(
            &self.path,
            &self.directory,
            self.created_file,
            self.created_directory,
            remove_paneflow_hooks,
            &mut self.lease,
        );
    }
}

fn paneflow_hook_commands_in_group(group: &serde_json::Value) -> Vec<String> {
    group
        .get("hooks")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|hook| hook.get("command").and_then(serde_json::Value::as_str))
        .filter(|command| is_paneflow_hook_command(command))
        .map(ToOwned::to_owned)
        .collect()
}

fn paneflow_hook_command_program_exists(command: &str) -> bool {
    paneflow_hook_program_token(command)
        .as_deref()
        .is_some_and(program_exists)
}

fn program_exists(program: &str) -> bool {
    let path = Path::new(program);
    if path.is_file() {
        return true;
    }
    if path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return false;
    }
    let Some(search_path) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&search_path).any(|directory| {
        let candidate = directory.join(program);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        if Path::new(program).extension().is_none() {
            let extensions = env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            return extensions
                .split(';')
                .filter(|extension| !extension.is_empty())
                .any(|extension| directory.join(format!("{program}{extension}")).is_file());
        }
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_agent_config::claude_hooks::MANAGED_MARKER;
    use serde_json::json;

    #[test]
    fn persistent_hook_requires_an_existing_program() {
        let stale = json!({
            "hooks": { "Stop": [{
                MANAGED_MARKER: true,
                "hooks": [{ "command": "/missing/paneflow-ai-hook Stop" }]
            }]}
        });
        assert!(matches!(
            settings_managed_hook_state(&stale),
            PersistentHookState::Stale { .. }
        ));

        let temp = tempfile::TempDir::new().unwrap();
        let executable = temp.path().join(if cfg!(windows) {
            "paneflow-ai-hook.exe"
        } else {
            "paneflow-ai-hook"
        });
        std::fs::File::create(&executable).unwrap();
        let alive = json!({
            "hooks": { "Stop": [{
                MANAGED_MARKER: true,
                "hooks": [{ "command": format!("{} Stop", executable.display()) }]
            }]}
        });
        assert!(matches!(
            settings_managed_hook_state(&alive),
            PersistentHookState::Alive { .. }
        ));
        assert!(settings_has_managed_hook(&alive));
    }

    #[test]
    fn persistent_hook_read_errors_are_not_treated_as_absence() {
        let temp = tempfile::TempDir::new().unwrap();
        let path = temp.path().join("settings.json");
        std::fs::write(&path, [0xff]).unwrap();

        assert_eq!(
            persistent_claude_hooks_state_at(&path).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(std::fs::read(path).unwrap(), [0xff]);
    }
}
