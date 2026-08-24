use super::{
    cleanup_hook_config_file, home_unavailable, hook_config_error, install_hook_config_file,
    is_paneflow_hook_command, merge_strict_matcher_hooks_for_events, paneflow_ipc_reachable,
    reconcile_matcher_hooks_replacing_invalid_container, remove_matcher_hooks_for_events,
    remove_matcher_hooks_lenient, resolve_plain_hook_command, sweep_orphan_hook_config,
    HookInstall, HookInstallResult, HookInstallSkip, HookLease, InvalidJsonPolicy,
};
use paneflow_agent_config::home_dir;
use std::env;
use std::path::{Path, PathBuf};

const QODER_HOOK_EVENTS: &[&str] = &["UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"];

const GEMINI_HOOK_EVENTS: &[(&str, &str)] = &[
    ("BeforeAgent", "UserPromptSubmit"),
    ("AfterAgent", "Stop"),
    ("BeforeTool", "PreToolUse"),
    ("AfterTool", "PostToolUse"),
];

const CURSOR_HOOK_EVENTS: &[(&str, &str)] = &[
    ("beforeSubmitPrompt", "UserPromptSubmit"),
    ("stop", "Stop"),
    ("preToolUse", "PreToolUse"),
    ("postToolUse", "PostToolUse"),
];

pub(crate) fn merge_qoder_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    merge_strict_matcher_hooks_for_events(root, QODER_HOOK_EVENTS)
}

pub(crate) fn remove_qoder_hooks(root: &mut serde_json::Value) {
    remove_matcher_hooks_for_events(root, QODER_HOOK_EVENTS);
}

pub(crate) fn merge_gemini_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    let events: Vec<&str> = GEMINI_HOOK_EVENTS
        .iter()
        .map(|(foreign, _)| *foreign)
        .collect();
    reconcile_matcher_hooks_replacing_invalid_container(root, &events, |foreign| {
        let canonical = GEMINI_HOOK_EVENTS
            .iter()
            .find_map(|(candidate, canonical)| (*candidate == foreign).then_some(*canonical))
            .unwrap_or(foreign);
        serde_json::json!({
            "matcher": "*",
            "hooks": [{
                "name": "paneflow-status",
                "type": "command",
                "command": resolve_plain_hook_command(canonical),
                "timeout": 5000,
            }]
        })
    })
    .map(|_| ())
    .map_err(hook_config_error)
}

pub(crate) fn remove_gemini_hooks(root: &mut serde_json::Value) {
    let events: Vec<&str> = GEMINI_HOOK_EVENTS
        .iter()
        .map(|(foreign, _)| *foreign)
        .collect();
    remove_matcher_hooks_lenient(root, &events);
}

pub(crate) fn merge_cursor_hooks(root: &mut serde_json::Value) -> std::io::Result<()> {
    merge_flat_hooks(root, CURSOR_HOOK_EVENTS, true);
    Ok(())
}

pub(crate) fn remove_cursor_hooks(root: &mut serde_json::Value) {
    remove_flat_hooks(root, CURSOR_HOOK_EVENTS);
}

fn merge_flat_hooks(root: &mut serde_json::Value, events: &[(&str, &str)], add_version: bool) {
    if !root.is_object() {
        *root = serde_json::json!({});
    }
    let Some(root) = root.as_object_mut() else {
        return;
    };
    if add_version {
        root.entry("version")
            .or_insert_with(|| serde_json::json!(1));
    }
    let hooks = root.entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let Some(hooks) = hooks.as_object_mut() else {
        return;
    };

    for (foreign, canonical) in events {
        let entries = hooks
            .entry(*foreign)
            .or_insert_with(|| serde_json::json!([]));
        let Some(entries) = entries.as_array_mut() else {
            continue;
        };
        entries.retain(|entry| !is_paneflow_flat_entry(entry));
        entries.push(serde_json::json!({
            "command": resolve_plain_hook_command(canonical),
            "timeout": 5,
        }));
    }
}

fn remove_flat_hooks(root: &mut serde_json::Value, events: &[(&str, &str)]) {
    let Some(root) = root.as_object_mut() else {
        return;
    };
    if let Some(hooks) = root
        .get_mut("hooks")
        .and_then(|value| value.as_object_mut())
    {
        for (foreign, _) in events {
            if let Some(entries) = hooks
                .get_mut(*foreign)
                .and_then(|value| value.as_array_mut())
            {
                entries.retain(|entry| !is_paneflow_flat_entry(entry));
            }
        }
        hooks.retain(|_, value| value.as_array().is_none_or(|entries| !entries.is_empty()));
    }
    let hooks_empty = root
        .get("hooks")
        .and_then(|value| value.as_object())
        .is_none_or(serde_json::Map::is_empty);
    if hooks_empty {
        root.remove("hooks");
        if root.len() == 1 && root.contains_key("version") {
            root.remove("version");
        }
    }
}

fn is_paneflow_flat_entry(value: &serde_json::Value) -> bool {
    value
        .get("command")
        .and_then(serde_json::Value::as_str)
        .is_some_and(is_paneflow_hook_command)
}

pub(crate) struct ManagedHookConfigGuard {
    settings_path: PathBuf,
    config_dir: PathBuf,
    created_file: bool,
    created_dir: bool,
    remove_fn: fn(&mut serde_json::Value),
    lease: HookLease,
}

#[derive(Clone, Copy)]
pub(crate) struct ManagedHookSpec {
    directory_name: &'static str,
    config_filename: &'static str,
    tool_label: &'static str,
    merge: fn(&mut serde_json::Value) -> std::io::Result<()>,
    remove: fn(&mut serde_json::Value),
}

impl ManagedHookSpec {
    pub(crate) const fn new(
        directory_name: &'static str,
        config_filename: &'static str,
        tool_label: &'static str,
        merge: fn(&mut serde_json::Value) -> std::io::Result<()>,
        remove: fn(&mut serde_json::Value),
    ) -> Self {
        Self {
            directory_name,
            config_filename,
            tool_label,
            merge,
            remove,
        }
    }
}

impl ManagedHookConfigGuard {
    pub(crate) fn install_in_cwd(spec: ManagedHookSpec) -> HookInstallResult<Self> {
        Self::install_anchored(
            &env::current_dir()?.join(spec.directory_name),
            spec,
            InvalidJsonPolicy::Replace,
        )
    }

    pub(crate) fn install_in_home(spec: ManagedHookSpec) -> HookInstallResult<Self> {
        let home = home_dir().ok_or_else(home_unavailable)?;
        Self::install_anchored(
            &home.join(spec.directory_name),
            spec,
            InvalidJsonPolicy::Refuse,
        )
    }

    fn install_anchored(
        config_dir: &Path,
        spec: ManagedHookSpec,
        invalid_json_policy: InvalidJsonPolicy,
    ) -> HookInstallResult<Self> {
        if !paneflow_ipc_reachable() {
            sweep_orphan_hook_config(&config_dir.join(spec.config_filename), spec.remove);
            return Ok(HookInstall::Skipped(HookInstallSkip::IpcUnavailable));
        }
        Self::install_at(config_dir, spec, invalid_json_policy).map(HookInstall::Installed)
    }

    pub(crate) fn install_at(
        config_dir: &Path,
        spec: ManagedHookSpec,
        invalid_json_policy: InvalidJsonPolicy,
    ) -> std::io::Result<Self> {
        let installed = install_hook_config_file(
            config_dir,
            spec.config_filename,
            spec.tool_label,
            spec.merge,
            invalid_json_policy,
        )?;
        Ok(Self {
            settings_path: installed.path,
            config_dir: config_dir.to_path_buf(),
            created_file: installed.created_file,
            created_dir: installed.created_directory,
            remove_fn: spec.remove,
            lease: installed.lease,
        })
    }
}

impl Drop for ManagedHookConfigGuard {
    fn drop(&mut self) {
        cleanup_hook_config_file(
            &self.settings_path,
            &self.config_dir,
            self.created_file,
            self.created_dir,
            self.remove_fn,
            &mut self.lease,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primary_config_survives_invalid_utf8() {
        let temp = tempfile::TempDir::new().unwrap();
        let directory = temp.path().join(".gemini");
        std::fs::create_dir_all(&directory).unwrap();
        let config = directory.join("settings.json");
        std::fs::write(&config, [0xff]).unwrap();

        assert!(ManagedHookConfigGuard::install_at(
            &directory,
            ManagedHookSpec::new(
                ".gemini",
                "settings.json",
                "Gemini",
                merge_gemini_hooks,
                remove_gemini_hooks,
            ),
            InvalidJsonPolicy::Refuse,
        )
        .is_err());
        assert_eq!(std::fs::read(config).unwrap(), [0xff]);
    }

    #[test]
    fn gemini_mixed_groups_preserve_user_handlers() {
        let mut root = serde_json::json!({
            "hooks": {
                "BeforeAgent": [{
                    "matcher": "*",
                    "hooks": [
                        { "type": "command", "command": "paneflow-ai-hook UserPromptSubmit" },
                        { "type": "command", "command": "my-user-hook" }
                    ]
                }]
            }
        });

        merge_gemini_hooks(&mut root).unwrap();
        remove_gemini_hooks(&mut root);

        let groups = root["hooks"]["BeforeAgent"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], "my-user-hook");
    }
}
