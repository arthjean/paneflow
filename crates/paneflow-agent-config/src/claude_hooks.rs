use serde_json::Value;

pub use crate::hook_command::{
    cmd_command_word, command_program_token, display_hook_program, is_paneflow_hook_command,
    paneflow_hook_program_token, render_hook_command, sh_command_word, shell_program_path,
};

pub const CLAUDE_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "Stop",
    "StopFailure",
    "PreToolUse",
    "PermissionRequest",
    "SubagentStart",
    "SubagentStop",
    "Notification",
];
pub const CLAUDE_RETIRED_HOOK_EVENTS: &[&str] = &["PostToolUse"];
pub const MANAGED_MARKER: &str = "_paneflow_managed";

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

pub fn remove_hooks_lenient(root: &mut Value) -> bool {
    let events: Vec<&str> = CLAUDE_HOOK_EVENTS
        .iter()
        .chain(CLAUDE_RETIRED_HOOK_EVENTS)
        .copied()
        .collect();
    remove_matcher_hooks_lenient(root, &events)
}

fn remove_matcher_hooks_lenient(root: &mut Value, events: &[&str]) -> bool {
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

    fn managed_group(path: &Path, event: &str) -> Value {
        json!({
            MANAGED_MARKER: true,
            "hooks": [{ "type": "command", "command": render_hook_command(path, event), "timeout": 5 }],
        })
    }

    #[test]
    fn lenient_cleanup_also_removes_a_retired_event() {
        let path = Path::new("/bin/paneflow-ai-hook");
        let mut root = json!({
            "hooks": {
                "PostToolUse": [managed_group(path, "PostToolUse")],
                "Stop": [managed_group(path, "Stop"), { "hooks": [{ "type": "command", "command": "my-hook" }] }]
            }
        });
        assert!(remove_hooks_lenient(&mut root));
        assert!(root["hooks"].get("PostToolUse").is_none());
        assert_eq!(root["hooks"]["Stop"].as_array().map(Vec::len), Some(1));
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
