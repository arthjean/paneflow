use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn config_write_guard() -> MutexGuard<'static, ()> {
    CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

fn load_raw_config(path: &Path) -> Result<serde_json::Value, ()> {
    match paneflow_config::loader::read_config_string(path) {
        Ok(None) => Ok(serde_json::json!({})),
        Err(error) => {
            log::warn!("config: {error}; refusing to overwrite");
            Err(())
        }
        Ok(Some(contents)) => {
            let value: serde_json::Value = serde_json::from_str(&contents).map_err(|e| {
                log::warn!(
                    "config: invalid JSON at {}; refusing to overwrite: {e}",
                    path.display()
                );
            })?;
            if value.is_object() {
                Ok(value)
            } else {
                log::warn!(
                    "config: root at {} is not a JSON object; refusing to overwrite",
                    path.display()
                );
                Err(())
            }
        }
    }
}

fn write_config(path: &PathBuf, value: &serde_json::Value) {
    let _ = write_config_checked(path, value);
}

fn write_config_checked(path: &PathBuf, value: &serde_json::Value) -> bool {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json_str = match serde_json::to_string_pretty(value) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("config: failed to serialize: {e}");
            return false;
        }
    };

    let Some(parent) = path.parent() else {
        return std::fs::write(path, &json_str)
            .inspect_err(|e| log::warn!("config: failed to write: {e}"))
            .is_ok();
    };
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("paneflow.json");
    let tmp = parent.join(format!(".{file_name}.tmp.{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &json_str) {
        log::warn!("config: failed to write temp file: {e}");
        let _ = std::fs::remove_file(&tmp);
        return false;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => true,
        Err(e) => {
            log::warn!("config: failed to promote temp file: {e}");
            let _ = std::fs::remove_file(&tmp);
            false
        }
    }
}

pub fn save_config_value_checked(key: &str, value: serde_json::Value) -> bool {
    save_config_values_checked([(key, value)])
}

pub fn save_config_values_checked<const N: usize>(values: [(&str, serde_json::Value); N]) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    if let Some(root) = json.as_object_mut() {
        for (key, value) in values {
            if value.is_null() {
                root.remove(key);
            } else {
                root.insert(key.to_string(), value);
            }
        }
    }
    write_config_checked(&path, &json)
}

fn merge_shortcut(
    shortcuts_obj: &mut serde_json::Map<String, serde_json::Value>,
    new_key: &str,
    action_name: &str,
) {
    let keys_to_remove: Vec<String> = shortcuts_obj
        .iter()
        .filter(|(k, v)| {
            v.as_str() == Some(action_name) || crate::keybindings::keystrokes_conflict(k, new_key)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for k in keys_to_remove {
        shortcuts_obj.remove(&k);
    }

    shortcuts_obj.insert(
        new_key.to_string(),
        serde_json::Value::String(action_name.to_string()),
    );
}

pub fn save_shortcut_checked(new_key: &str, action_name: &str) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };

    let Some(root) = json.as_object_mut() else {
        log::warn!("config: root is not a JSON object, not saving shortcut");
        return false;
    };
    let shortcuts = root
        .entry("shortcuts")
        .or_insert_with(|| serde_json::json!({}));
    if !shortcuts.is_object() {
        *shortcuts = serde_json::json!({});
    }
    let Some(shortcuts_obj) = shortcuts.as_object_mut() else {
        return false;
    };

    merge_shortcut(shortcuts_obj, new_key, action_name);

    write_config_checked(&path, &json)
}

pub fn reset_shortcuts() {
    let Some(path) = paneflow_config::loader::config_path() else {
        return;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return;
    };
    if let Some(root) = json.as_object_mut() {
        root.remove("shortcuts");
    }
    write_config(&path, &json);
}

fn apply_terminal_field(json: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    let Some(root) = json.as_object_mut() else {
        return;
    };
    let terminal = root
        .entry("terminal")
        .or_insert_with(|| serde_json::json!({}));
    if !terminal.is_object() {
        *terminal = serde_json::json!({});
    }
    if let Some(obj) = terminal.as_object_mut() {
        if value.is_null() {
            obj.remove(key);
        } else {
            obj.insert(key.to_string(), value);
        }
    }
}

fn apply_agent_panel_field(json: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    let Some(root) = json.as_object_mut() else {
        return;
    };
    let agent_panel = root
        .entry("agent_panel")
        .or_insert_with(|| serde_json::json!({}));
    if !agent_panel.is_object() {
        *agent_panel = serde_json::json!({});
    }
    if let Some(obj) = agent_panel.as_object_mut() {
        if value.is_null() {
            obj.remove(key);
        } else {
            obj.insert(key.to_string(), value);
        }
    }
}

pub fn with_field(
    config: &paneflow_config::schema::PaneFlowConfig,
    nested: bool,
    key: &str,
    value: serde_json::Value,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut json = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
    if nested {
        apply_terminal_field(&mut json, key, value);
    } else if let Some(root) = json.as_object_mut() {
        if value.is_null() {
            root.remove(key);
        } else {
            root.insert(key.to_string(), value);
        }
    }
    serde_json::from_value(json).unwrap_or_else(|_| config.clone())
}

pub fn with_agent_panel_field(
    config: &paneflow_config::schema::PaneFlowConfig,
    key: &str,
    value: serde_json::Value,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut json = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
    apply_agent_panel_field(&mut json, key, value);
    serde_json::from_value(json).unwrap_or_else(|_| config.clone())
}

pub fn with_commands(
    config: &paneflow_config::schema::PaneFlowConfig,
    commands: Vec<paneflow_config::schema::CommandDefinition>,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut next = config.clone();
    next.commands = commands;
    next
}

pub fn save_terminal_field_checked(key: &str, value: serde_json::Value) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    apply_terminal_field(&mut json, key, value);
    write_config_checked(&path, &json)
}

pub fn save_agent_panel_field_checked(key: &str, value: serde_json::Value) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    apply_agent_panel_field(&mut json, key, value);
    write_config_checked(&path, &json)
}

pub fn save_commands_checked(commands: Vec<paneflow_config::schema::CommandDefinition>) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let value = match serde_json::to_value(commands) {
        Ok(value) => value,
        Err(e) => {
            log::warn!("config: failed to serialize commands: {e}");
            return false;
        }
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    if let Some(root) = json.as_object_mut() {
        root.insert("commands".to_string(), value);
    }
    write_config_checked(&path, &json)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_agent_panel_field, apply_terminal_field, load_raw_config, merge_shortcut,
        write_config_checked,
    };
    use serde_json::{Value, json};

    #[test]
    fn write_config_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        assert!(write_config_checked(
            &p,
            &json!({"theme": "One Dark", "font_size": 14.0})
        ));
        let got: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(got["theme"], "One Dark");
        let leftovers = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftovers, 0, "the temp file must be renamed away");
    }

    #[test]
    fn write_config_does_not_truncate_on_repeated_writes() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        assert!(write_config_checked(&p, &json!({"a": 1})));
        assert!(write_config_checked(&p, &json!({"b": 2})));
        let got: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert!(got.get("a").is_none() && got["b"] == 2);
    }

    #[test]
    fn load_raw_config_rejects_invalid_json_instead_of_emptying_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        std::fs::write(&p, "{").unwrap();

        assert!(
            load_raw_config(&p).is_err(),
            "invalid existing config must fail closed so writers do not replace it with an empty object"
        );
    }

    #[test]
    fn load_raw_config_rejects_non_object_roots() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        std::fs::write(&p, "[]").unwrap();

        assert!(
            load_raw_config(&p).is_err(),
            "a valid JSON non-object is not a writable paneflow config root"
        );
    }

    fn shortcuts(pairs: &[(&str, &str)]) -> serde_json::Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn merge_shortcut_dedupes_prior_key_for_same_action() {
        let mut m = shortcuts(&[("ctrl-alt-h", "split_horizontally")]);
        merge_shortcut(&mut m, "ctrl-alt-j", "split_horizontally");
        assert!(!m.contains_key("ctrl-alt-h"), "old key should be removed");
        assert_eq!(m["ctrl-alt-j"], json!("split_horizontally"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn merge_shortcut_collision_evicts_other_action() {
        let mut m = shortcuts(&[("ctrl-shift-f", "toggle_search")]);
        merge_shortcut(&mut m, "ctrl-shift-f", "close_pane");
        assert_eq!(m["ctrl-shift-f"], json!("close_pane"));
        assert_eq!(m.len(), 1, "no leftover binding for the evicted action");
    }

    #[test]
    fn merge_shortcut_collision_is_normalization_aware() {
        let mut m = shortcuts(&[("ctrl+shift+f", "toggle_search")]);
        merge_shortcut(&mut m, "ctrl-shift-f", "close_pane");
        assert!(
            !m.contains_key("ctrl+shift+f"),
            "the '+'-separated variant must be evicted"
        );
        assert_eq!(m["ctrl-shift-f"], json!("close_pane"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn upserts_into_terminal_block_creating_it() {
        let mut j = json!({});
        apply_terminal_field(&mut j, "ligatures", json!(true));
        assert_eq!(j["terminal"]["ligatures"], json!(true));
    }

    #[test]
    fn preserves_other_terminal_keys() {
        let mut j = json!({"terminal": {"cursor_shape": "beam"}});
        apply_terminal_field(&mut j, "ligatures", json!(true));
        assert_eq!(j["terminal"]["cursor_shape"], json!("beam"));
        assert_eq!(j["terminal"]["ligatures"], json!(true));
    }

    #[test]
    fn null_removes_key_but_keeps_block() {
        let mut j = json!({"terminal": {"cursor_shape": "beam", "ligatures": true}});
        apply_terminal_field(&mut j, "cursor_shape", Value::Null);
        assert!(j["terminal"].get("cursor_shape").is_none());
        assert_eq!(j["terminal"]["ligatures"], json!(true));
        assert!(j["terminal"].is_object());
    }

    #[test]
    fn replaces_non_object_terminal_value() {
        let mut j = json!({"terminal": "garbage"});
        apply_terminal_field(&mut j, "cursor_shape", json!("block"));
        assert_eq!(j["terminal"]["cursor_shape"], json!("block"));
    }

    #[test]
    fn leaves_top_level_keys_untouched() {
        let mut j = json!({"theme": "One Dark", "font_size": 14.0});
        apply_terminal_field(&mut j, "scrollback_lines", json!(5000));
        assert_eq!(j["theme"], json!("One Dark"));
        assert_eq!(j["font_size"], json!(14.0));
        assert_eq!(j["terminal"]["scrollback_lines"], json!(5000));
    }

    #[test]
    fn upserts_into_agent_panel_preserving_siblings() {
        let mut j = json!({
            "agent_panel": {
                "max_content_width": 760,
                "notify_when_agent_waiting": "PrimaryScreen"
            }
        });
        apply_agent_panel_field(&mut j, "notify_when_agent_waiting", json!("Never"));
        assert_eq!(j["agent_panel"]["max_content_width"], json!(760));
        assert_eq!(
            j["agent_panel"]["notify_when_agent_waiting"],
            json!("Never")
        );
    }

    #[test]
    fn replaces_non_object_agent_panel_value() {
        let mut j = json!({"agent_panel": "garbage"});
        apply_agent_panel_field(&mut j, "notify_when_agent_waiting", json!("Never"));
        assert_eq!(
            j["agent_panel"]["notify_when_agent_waiting"],
            json!("Never")
        );
    }
}
