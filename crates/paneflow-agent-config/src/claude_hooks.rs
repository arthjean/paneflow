use std::fmt;
use std::path::Path;

use serde_json::{json, Value};

pub use crate::hook_command::{
    command_program_token, display_hook_program, is_paneflow_hook_command,
    paneflow_hook_program_token, render_bare_hook_command, render_hook_command, shell_program_path,
};

pub const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "Notification",
    "Stop",
    "PreToolUse",
    "PostToolUse",
];
pub const MANAGED_MARKER: &str = "_paneflow_managed";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookConfigError(String);

impl HookConfigError {
    fn invalid(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for HookConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for HookConfigError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconcileResult {
    pub had_prior: bool,
    pub changed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookStatus {
    NotInstalled,
    Installed {
        path: String,
    },
    Stale {
        found: String,
        expected: String,
    },
    NeedsRepair {
        path: Option<String>,
        reason: String,
    },
}

pub fn managed_group(path: &Path, event: &str) -> Value {
    managed_group_for_command(render_hook_command(path, event))
}

fn managed_group_for_command(command: String) -> Value {
    json!({
        MANAGED_MARKER: true,
        "hooks": [command_handler(command)],
    })
}

pub fn command_handler(command: String) -> Value {
    let mut handler = serde_json::Map::new();
    handler.insert("type".into(), Value::String("command".into()));
    handler.insert("command".into(), Value::String(command.clone()));
    handler.insert("timeout".into(), json!(5));
    #[cfg(windows)]
    handler.insert("commandWindows".into(), Value::String(command));

    Value::Object(handler)
}

pub fn is_managed_group(group: &Value) -> bool {
    if group.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true) {
        return true;
    }
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hooks| hooks.iter().any(is_managed_handler))
}

fn is_managed_handler(handler: &Value) -> bool {
    handler
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_paneflow_hook_command)
}

fn strip_managed_handlers(groups: &mut Vec<Value>) -> bool {
    let mut removed = false;
    let mut index = 0;
    while index < groups.len() {
        let Some(group) = groups[index].as_object_mut() else {
            index += 1;
            continue;
        };
        let marker_removed = group.get(MANAGED_MARKER).and_then(Value::as_bool) == Some(true);
        let mut stripped_from_group = marker_removed;
        if marker_removed {
            group.remove(MANAGED_MARKER);
        }
        let hooks_became_empty =
            if let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = hooks.len();
                hooks.retain(|handler| !is_managed_handler(handler));
                let removed_handler = hooks.len() != before;
                stripped_from_group |= removed_handler;
                hooks.is_empty() && (removed_handler || marker_removed)
            } else {
                false
            };
        removed |= stripped_from_group;
        if hooks_became_empty || (stripped_from_group && group.is_empty()) {
            groups.remove(index);
        } else {
            index += 1;
        }
    }
    removed
}

fn validate_shape(root: &Value) -> Result<(), HookConfigError> {
    validate_matcher_shape(root, CLAUDE_HOOK_EVENTS)
}

fn validate_matcher_shape(root: &Value, events: &[&str]) -> Result<(), HookConfigError> {
    let object = root
        .as_object()
        .ok_or_else(|| HookConfigError::invalid("config root must be a JSON object"))?;
    let Some(hooks) = object.get("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object()
        .ok_or_else(|| HookConfigError::invalid("config key `hooks` must be an object"))?;
    for event in events {
        if let Some(value) = hooks.get(*event) {
            if !value.is_array() {
                return Err(HookConfigError::invalid(format!(
                    "hook event `{event}` must be an array"
                )));
            }
        }
    }
    Ok(())
}

pub fn reconcile_hooks(
    root: &mut Value,
    command_for_event: impl Fn(&str) -> String,
) -> Result<ReconcileResult, HookConfigError> {
    validate_shape(root)?;
    reconcile_valid_matcher_hooks(root, CLAUDE_HOOK_EVENTS, |event| {
        managed_group_for_command(command_for_event(event))
    })
}

pub fn reconcile_hooks_replacing_invalid_container(
    root: &mut Value,
    command_for_event: impl Fn(&str) -> String,
) -> Result<ReconcileResult, HookConfigError> {
    if !root.is_object() {
        *root = json!({});
    }
    let object = root
        .as_object_mut()
        .ok_or_else(|| HookConfigError::invalid("config root must be a JSON object"))?;
    if object.get("hooks").is_some_and(|hooks| !hooks.is_object()) {
        object.insert("hooks".into(), json!({}));
    }
    validate_shape(root)?;
    reconcile_valid_matcher_hooks(root, CLAUDE_HOOK_EVENTS, |event| {
        managed_group_for_command(command_for_event(event))
    })
}

pub fn reconcile_matcher_hooks_replacing_invalid_container(
    root: &mut Value,
    events: &[&str],
    group_for_event: impl Fn(&str) -> Value,
) -> Result<ReconcileResult, HookConfigError> {
    if !root.is_object() {
        *root = json!({});
    }
    let object = root
        .as_object_mut()
        .ok_or_else(|| HookConfigError::invalid("config root must be a JSON object"))?;
    if object.get("hooks").is_some_and(|hooks| !hooks.is_object()) {
        object.insert("hooks".into(), json!({}));
    }
    validate_matcher_shape(root, events)?;
    reconcile_valid_matcher_hooks(root, events, group_for_event)
}

fn reconcile_valid_matcher_hooks(
    root: &mut Value,
    events: &[&str],
    group_for_event: impl Fn(&str) -> Value,
) -> Result<ReconcileResult, HookConfigError> {
    let before = root.clone();
    let object = root
        .as_object_mut()
        .ok_or_else(|| HookConfigError::invalid("config root must be a JSON object"))?;
    let hooks = object.entry("hooks").or_insert_with(|| json!({}));
    let hooks = hooks
        .as_object_mut()
        .ok_or_else(|| HookConfigError::invalid("config key `hooks` must be an object"))?;
    let had_prior = events.iter().any(|event| {
        hooks
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|groups| groups.iter().any(is_managed_group))
    });

    for event in events {
        let groups = hooks.entry(*event).or_insert_with(|| json!([]));
        let groups = groups.as_array_mut().ok_or_else(|| {
            HookConfigError::invalid(format!("hook event `{event}` must be an array"))
        })?;
        strip_managed_handlers(groups);
        groups.push(group_for_event(event));
    }

    Ok(ReconcileResult {
        had_prior,
        changed: *root != before,
    })
}

pub fn remove_hooks(root: &mut Value) -> Result<bool, HookConfigError> {
    validate_shape(root)?;
    Ok(remove_hooks_lenient(root))
}

pub fn remove_hooks_lenient(root: &mut Value) -> bool {
    remove_matcher_hooks_lenient(root, CLAUDE_HOOK_EVENTS)
}

pub fn remove_matcher_hooks_lenient(root: &mut Value, events: &[&str]) -> bool {
    let object = root.as_object_mut();
    let Some(object) = object else {
        return false;
    };
    let Some(hooks) = object.get_mut("hooks").and_then(Value::as_object_mut) else {
        return false;
    };
    let mut removed = false;
    for event in events {
        if let Some(groups) = hooks.get_mut(*event).and_then(Value::as_array_mut) {
            let removed_from_event = strip_managed_handlers(groups);
            removed |= removed_from_event;
            if removed_from_event && groups.is_empty() {
                hooks.remove(*event);
            }
        }
    }
    if removed && hooks.is_empty() {
        object.remove("hooks");
    }
    removed
}

pub fn inspect_hooks(root: &Value, expected: Option<&Path>) -> HookStatus {
    let Some(object) = root.as_object() else {
        return repair(None, "config root must be a JSON object");
    };
    let Some(hooks_value) = object.get("hooks") else {
        return HookStatus::NotInstalled;
    };
    let Some(hooks) = hooks_value.as_object() else {
        return repair(None, "config key `hooks` must be an object");
    };

    let any_managed = CLAUDE_HOOK_EVENTS.iter().any(|event| {
        hooks
            .get(*event)
            .and_then(Value::as_array)
            .is_some_and(|groups| groups.iter().any(is_managed_group))
    });
    if !any_managed {
        if let Some(event) = CLAUDE_HOOK_EVENTS
            .iter()
            .find(|event| hooks.get(**event).is_some_and(|value| !value.is_array()))
        {
            return repair(None, format!("hook event `{event}` must be an array"));
        }
        return HookStatus::NotInstalled;
    }

    let mut paths = Vec::with_capacity(CLAUDE_HOOK_EVENTS.len());
    for event in CLAUDE_HOOK_EVENTS {
        let Some(groups) = hooks.get(*event).and_then(Value::as_array) else {
            return repair(
                paths.first().cloned(),
                format!("hook event `{event}` is missing"),
            );
        };
        let managed: Vec<&Value> = groups
            .iter()
            .filter(|group| is_managed_group(group))
            .collect();
        if managed.len() != 1 {
            return repair(
                paths.first().cloned(),
                format!("hook event `{event}` must contain exactly one Paneflow group"),
            );
        }
        match validate_group(managed[0], event) {
            Ok(path) => paths.push(path),
            Err(reason) => return repair(paths.first().cloned(), reason),
        }
    }

    let found = paths[0].clone();
    if paths.iter().any(|path| path != &found) {
        return repair(
            Some(found),
            "Paneflow hook events point at different binaries",
        );
    }
    if let Some(expected) = expected {
        let expected = display_hook_program(expected);
        if found != expected {
            return HookStatus::Stale { found, expected };
        }
    }
    HookStatus::Installed { path: found }
}

fn validate_group(group: &Value, event: &str) -> Result<String, String> {
    let hooks = group
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("Paneflow group for `{event}` has no hook array"))?;
    if hooks.len() != 1 {
        return Err(format!(
            "Paneflow group for `{event}` must contain one hook"
        ));
    }
    let handler = hooks[0]
        .as_object()
        .ok_or_else(|| format!("Paneflow hook for `{event}` must be an object"))?;
    if handler.get("type").and_then(Value::as_str) != Some("command")
        || handler.get("timeout").and_then(Value::as_u64) != Some(5)
    {
        return Err(format!(
            "Paneflow hook for `{event}` must be a five-second command hook"
        ));
    }
    let command = handler
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("Paneflow hook for `{event}` has no command"))?;
    let path = paneflow_hook_program_token(command)
        .ok_or_else(|| format!("Paneflow hook for `{event}` has an invalid command"))?;
    let canonical = render_hook_command(Path::new(&path), event);
    if group != &managed_group_for_command(canonical) {
        return Err(format!(
            "Paneflow group for `{event}` does not match the canonical shape"
        ));
    }
    Ok(path)
}

fn repair(path: Option<String>, reason: impl Into<String>) -> HookStatus {
    HookStatus::NeedsRepair {
        path,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command_for(path: &Path) -> impl Fn(&str) -> String + '_ {
        move |event| render_hook_command(path, event)
    }

    #[test]
    fn reconcile_and_remove_preserve_user_state() {
        let path = Path::new("/opt/Pane Flow/paneflow-ai-hook");
        let mut root = json!({
            "theme": "dark",
            "hooks": {
                "Stop": [{ "hooks": [{ "type": "command", "command": "my-hook" }] }]
            }
        });

        let result = reconcile_hooks(&mut root, command_for(path)).unwrap();
        assert!(!result.had_prior && result.changed);
        assert_eq!(
            inspect_hooks(&root, Some(path)),
            HookStatus::Installed {
                path: display_hook_program(path),
            }
        );
        assert_eq!(root["theme"], json!("dark"));
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 2);

        assert!(remove_hooks(&mut root).unwrap());
        assert_eq!(root["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert_eq!(root["theme"], json!("dark"));
    }

    #[test]
    fn reconcile_and_remove_preserve_handlers_in_mixed_groups() {
        let path = Path::new("/bin/paneflow-ai-hook");
        let mut mixed = managed_group(path, "Stop");
        mixed["matcher"] = json!("Write");
        mixed["hooks"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "type": "command", "command": "my-hook" }));
        let mut root = json!({ "hooks": { "Stop": [mixed] } });

        reconcile_hooks(&mut root, command_for(path)).unwrap();
        let groups = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["matcher"], json!("Write"));
        assert_eq!(groups[0]["hooks"][0]["command"], json!("my-hook"));

        assert!(remove_hooks(&mut root).unwrap());
        let groups = root["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], json!("my-hook"));
    }

    #[test]
    fn custom_matcher_events_preserve_handlers_in_mixed_groups() {
        const EVENTS: &[&str] = &["BeforeAgent", "AfterAgent"];
        let path = Path::new("/bin/paneflow-ai-hook");
        let mut mixed = managed_group(path, "BeforeAgent");
        mixed.as_object_mut().unwrap().remove(MANAGED_MARKER);
        mixed["matcher"] = json!("*");
        mixed["hooks"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "type": "command", "command": "my-hook" }));
        let mut root = json!({ "hooks": { "BeforeAgent": [mixed] } });

        reconcile_matcher_hooks_replacing_invalid_container(&mut root, EVENTS, |event| {
            json!({
                "matcher": "*",
                "hooks": [command_handler(render_hook_command(path, event))],
            })
        })
        .unwrap();

        let groups = root["hooks"]["BeforeAgent"].as_array().unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["hooks"][0]["command"], json!("my-hook"));

        assert!(remove_matcher_hooks_lenient(&mut root, EVENTS));
        let groups = root["hooks"]["BeforeAgent"].as_array().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0]["hooks"][0]["command"], json!("my-hook"));
    }

    #[test]
    fn reconcile_refuses_invalid_boundaries_without_mutating() {
        for mut root in [
            json!([]),
            json!({ "hooks": "broken" }),
            json!({ "hooks": { "Stop": "broken" } }),
        ] {
            let before = root.clone();
            assert!(
                reconcile_hooks(&mut root, command_for(Path::new("/bin/paneflow-ai-hook")))
                    .is_err()
            );
            assert_eq!(root, before);
        }
    }

    #[test]
    fn inspect_rejects_partial_and_inconsistent_sets() {
        let path = Path::new("/bin/paneflow-ai-hook");
        let mut root = json!({});
        reconcile_hooks(&mut root, command_for(path)).unwrap();
        root["hooks"].as_object_mut().unwrap().remove("Stop");
        assert!(matches!(
            inspect_hooks(&root, Some(path)),
            HookStatus::NeedsRepair { .. }
        ));

        reconcile_hooks(&mut root, command_for(path)).unwrap();
        root["hooks"]["Stop"][0] = managed_group(Path::new("/old/paneflow-ai-hook"), "Stop");
        assert!(matches!(
            inspect_hooks(&root, Some(path)),
            HookStatus::NeedsRepair { .. }
        ));

        reconcile_hooks(&mut root, command_for(path)).unwrap();
        root["hooks"]["Stop"][0]["matcher"] = json!("Write");
        assert!(matches!(
            inspect_hooks(&root, Some(path)),
            HookStatus::NeedsRepair { .. }
        ));
    }

    #[test]
    fn lenient_cleanup_skips_invalid_events_but_cleans_valid_ones() {
        let path = Path::new("/bin/paneflow-ai-hook");
        let mut root = json!({
            "hooks": {
                "Stop": "broken",
                "Notification": [managed_group(path, "Notification")]
            }
        });
        assert!(remove_hooks_lenient(&mut root));
        assert_eq!(root["hooks"]["Stop"], json!("broken"));
        assert!(root["hooks"].get("Notification").is_none());
    }

    #[test]
    fn lenient_cleanup_removes_empty_managed_groups() {
        let mut root = json!({
            "hooks": {
                "Stop": [{ MANAGED_MARKER: true, "hooks": [] }]
            }
        });

        assert!(remove_hooks_lenient(&mut root));
        assert_eq!(root, json!({}));
    }

    #[test]
    fn quoted_program_round_trips() {
        let path = Path::new("/tmp/O'Brien/Pane Flow/paneflow-ai-hook");
        let command = render_hook_command(path, "Stop");
        assert_eq!(
            paneflow_hook_program_token(&command).as_deref(),
            Some(display_hook_program(path).as_str())
        );

        let powershell = "powershell.exe -NoProfile -ExecutionPolicy Bypass -Command \"& 'C:/Users/O''Brien/Pane Flow/paneflow-ai-hook.exe' Stop\"";
        assert_eq!(
            paneflow_hook_program_token(powershell).as_deref(),
            Some("C:/Users/O'Brien/Pane Flow/paneflow-ai-hook.exe")
        );
    }
}
