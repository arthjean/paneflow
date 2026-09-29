use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());
static SETTING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static LAST_WRITTEN: LazyLock<Mutex<HashMap<String, u64>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingWrite {
    Written,
    Superseded,
    Failed,
}

pub fn next_setting_sequence() -> u64 {
    SETTING_SEQUENCE.fetch_add(1, Ordering::Relaxed) + 1
}

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

fn write_config_checked(path: &PathBuf, value: &serde_json::Value) -> bool {
    if std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly()) {
        log::warn!(
            "config: {} is read-only; not overwriting it",
            path.display()
        );
        return false;
    }
    let json_str = match serde_json::to_string_pretty(value) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("config: failed to serialize: {e}");
            return false;
        }
    };
    paneflow_home::write_atomically(path, json_str.as_bytes())
        .inspect_err(|e| log::warn!("config: failed to write {}: {e}", path.display()))
        .is_ok()
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

pub fn reset_shortcut(action_name: &str) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    if let Some(obj) = json
        .as_object_mut()
        .and_then(|root| root.get_mut("shortcuts"))
        .and_then(|shortcuts| shortcuts.as_object_mut())
    {
        restore_default_binding(obj, action_name);
    }
    write_config_checked(&path, &json)
}

fn restore_default_binding(
    shortcuts_obj: &mut serde_json::Map<String, serde_json::Value>,
    action_name: &str,
) {
    let defaults = crate::keybindings::default_keys(action_name);
    shortcuts_obj.retain(|key, value| {
        let owned = value.as_str() == Some(action_name);
        let masked_default = value.as_str() == Some("none")
            && defaults
                .iter()
                .any(|default| crate::keybindings::keystrokes_conflict(default, key));
        !owned && !masked_default
    });
}

pub fn unassign_shortcut(action_name: &str) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    let Some(root) = json.as_object_mut() else {
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
    unbind_action(shortcuts_obj, action_name);
    write_config_checked(&path, &json)
}

fn unbind_action(
    shortcuts_obj: &mut serde_json::Map<String, serde_json::Value>,
    action_name: &str,
) {
    shortcuts_obj.retain(|_, value| value.as_str() != Some(action_name));
    for default in crate::keybindings::default_keys(action_name) {
        let taken = shortcuts_obj
            .keys()
            .any(|key| crate::keybindings::keystrokes_conflict(key, default));
        if !taken {
            shortcuts_obj.insert(
                default.to_string(),
                serde_json::Value::String("none".to_string()),
            );
        }
    }
}

pub fn reset_shortcuts_checked() -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not resetting");
        return false;
    };
    reset_shortcuts_at(&path)
}

fn reset_shortcuts_at(path: &PathBuf) -> bool {
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(path) else {
        return false;
    };
    if std::fs::metadata(path).is_ok_and(|meta| meta.permissions().readonly()) {
        log::warn!("config: {} is read-only; not resetting it", path.display());
        return false;
    }
    if !back_up_before_reset(path) {
        return false;
    }
    if let Some(root) = json.as_object_mut() {
        root.remove("shortcuts");
    }
    write_config_checked(path, &json)
}

fn reset_backup_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "paneflow.json".to_string());
    path.with_file_name(format!("{name}.before-reset"))
}

fn back_up_before_reset(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => paneflow_home::write_atomically(&reset_backup_path(path), &bytes)
            .inspect_err(|e| log::warn!("config: could not back up before the reset: {e}"))
            .is_ok(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            log::warn!(
                "config: could not read {} to back it up: {e}",
                path.display()
            );
            false
        }
    }
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

fn config_json(
    config: &paneflow_config::schema::PaneFlowConfig,
    key: &str,
) -> Result<serde_json::Value, String> {
    serde_json::to_value(config).map_err(|e| {
        let message = format!("config: cannot serialize the settings to change {key}: {e}");
        log::error!("{message}");
        message
    })
}

fn config_from_json(
    json: serde_json::Value,
    key: &str,
) -> Result<paneflow_config::schema::PaneFlowConfig, String> {
    serde_json::from_value(json).map_err(|e| {
        let message = format!("config: {key} does not fit the settings schema: {e}");
        log::error!("{message}");
        message
    })
}

fn scalar_kept(requested: &serde_json::Value, actual: Option<&serde_json::Value>) -> bool {
    use serde_json::Value;
    match (requested, actual) {
        (Value::Null | Value::Array(_) | Value::Object(_), _) => true,
        (Value::Number(want), Some(Value::Number(got))) => match (want.as_f64(), got.as_f64()) {
            (Some(want), Some(got)) => (want - got).abs() <= 1e-3 * want.abs().max(1.0),
            _ => false,
        },
        (want, Some(got)) => want == got,
        (_, None) => false,
    }
}

fn kept_or_error(
    next: paneflow_config::schema::PaneFlowConfig,
    pointer: &str,
    requested: &serde_json::Value,
    key: &str,
) -> Result<paneflow_config::schema::PaneFlowConfig, String> {
    let written = config_json(&next, key)?;
    if scalar_kept(requested, written.pointer(pointer)) {
        return Ok(next);
    }
    let message = format!("config: {requested} is not a valid value for {key}");
    log::error!("{message}");
    Err(message)
}

pub fn with_field(
    config: &paneflow_config::schema::PaneFlowConfig,
    nested: bool,
    key: &str,
    value: serde_json::Value,
) -> Result<paneflow_config::schema::PaneFlowConfig, String> {
    let mut json = config_json(config, key)?;
    let requested = value.clone();
    if nested {
        apply_terminal_field(&mut json, key, value);
    } else if let Some(root) = json.as_object_mut() {
        if value.is_null() {
            root.remove(key);
        } else {
            root.insert(key.to_string(), value);
        }
    }
    let pointer = if nested {
        format!("/terminal/{key}")
    } else {
        format!("/{key}")
    };
    kept_or_error(config_from_json(json, key)?, &pointer, &requested, key)
}

pub fn with_agent_panel_field(
    config: &paneflow_config::schema::PaneFlowConfig,
    key: &str,
    value: serde_json::Value,
) -> Result<paneflow_config::schema::PaneFlowConfig, String> {
    let mut json = config_json(config, key)?;
    let requested = value.clone();
    apply_agent_panel_field(&mut json, key, value);
    kept_or_error(
        config_from_json(json, key)?,
        &format!("/agent_panel/{key}"),
        &requested,
        key,
    )
}

pub fn with_commands(
    config: &paneflow_config::schema::PaneFlowConfig,
    commands: Vec<paneflow_config::schema::CommandDefinition>,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut next = config.clone();
    next.commands = commands;
    next
}

pub fn save_setting_ordered(
    nested: bool,
    key: &str,
    value: serde_json::Value,
    sequence: u64,
) -> SettingWrite {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return SettingWrite::Failed;
    };
    save_setting_ordered_at(&path, nested, key, value, sequence)
}

fn save_setting_ordered_at(
    path: &PathBuf,
    nested: bool,
    key: &str,
    value: serde_json::Value,
    sequence: u64,
) -> SettingWrite {
    let _guard = config_write_guard();
    let slot = format!(
        "{}|{}{key}",
        path.display(),
        if nested { "terminal." } else { "" }
    );
    let mut last = LAST_WRITTEN.lock().unwrap_or_else(PoisonError::into_inner);
    if last.get(&slot).is_some_and(|written| *written >= sequence) {
        return SettingWrite::Superseded;
    }
    let Ok(mut json) = load_raw_config(path) else {
        return SettingWrite::Failed;
    };
    if nested {
        apply_terminal_field(&mut json, key, value);
    } else if let Some(root) = json.as_object_mut() {
        if value.is_null() {
            root.remove(key);
        } else {
            root.insert(key.to_string(), value);
        }
    }
    if !write_config_checked(path, &json) {
        return SettingWrite::Failed;
    }
    last.insert(slot, sequence);
    SettingWrite::Written
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
        SettingWrite, apply_agent_panel_field, apply_terminal_field, load_raw_config,
        merge_shortcut, reset_backup_path, reset_shortcuts_at, restore_default_binding,
        save_setting_ordered_at, unbind_action, with_field, write_config_checked,
    };
    use serde_json::{Value, json};

    #[test]
    fn the_minimum_contrast_ladder_writes_off_and_removes_the_key_on_auto() {
        use crate::settings::tabs::terminal::minimum_contrast_setting;

        let mut json = json!({"terminal": {"minimum_contrast": 75.0, "color_emoji": true}});
        apply_terminal_field(&mut json, "minimum_contrast", minimum_contrast_setting(1));
        assert_eq!(json["terminal"]["minimum_contrast"], json!(0.0));

        apply_terminal_field(&mut json, "minimum_contrast", minimum_contrast_setting(0));
        assert!(
            json["terminal"].get("minimum_contrast").is_none(),
            "Auto must remove the key: {json}"
        );
        assert_eq!(json["terminal"]["color_emoji"], json!(true));
    }

    #[test]
    fn an_older_setting_write_never_lands_after_a_newer_one() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        std::fs::write(&p, r#"{"font_size": 13.0}"#).unwrap();

        assert_eq!(
            save_setting_ordered_at(&p, false, "font_size", json!(15.0), 2),
            SettingWrite::Written
        );
        assert_eq!(
            save_setting_ordered_at(&p, false, "font_size", json!(14.0), 1),
            SettingWrite::Superseded,
            "the first click finished last and must not win"
        );
        assert_eq!(load_raw_config(&p).unwrap()["font_size"], json!(15.0));
        assert_eq!(
            save_setting_ordered_at(&p, true, "font_size", json!(12.0), 1),
            SettingWrite::Written,
            "a nested key has its own order"
        );
    }

    #[test]
    fn a_reset_backs_the_file_up_first_and_refuses_a_read_only_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("paneflow.json");
        let original = r#"{"theme": "One Dark", "shortcuts": {"ctrl-k": "close_pane"}}"#;
        std::fs::write(&p, original).unwrap();

        assert!(reset_shortcuts_at(&p));
        assert_eq!(
            std::fs::read_to_string(reset_backup_path(&p)).unwrap(),
            original
        );
        let reset = load_raw_config(&p).unwrap();
        assert!(reset.get("shortcuts").is_none());
        assert_eq!(reset["theme"], json!("One Dark"));

        std::fs::write(&p, original).unwrap();
        let mut permissions = std::fs::metadata(&p).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&p, permissions.clone()).unwrap();
        assert!(!reset_shortcuts_at(&p), "a read-only file is not reset");
        assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        std::fs::set_permissions(&p, permissions).unwrap();
    }

    #[test]
    fn a_value_the_schema_rejects_is_an_error_not_a_silent_no_op() {
        let config = paneflow_config::schema::PaneFlowConfig::default();
        assert!(with_field(&config, false, "font_size", json!("huge")).is_err());
        assert!(with_field(&config, false, "font_size", json!(15.0)).is_ok());
    }

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
    fn unbind_action_drops_its_key_and_masks_its_default() {
        let mut m = shortcuts(&[("ctrl-alt-h", "split_horizontally")]);
        unbind_action(&mut m, "split_horizontally");
        assert_eq!(m.get("ctrl-alt-h"), None);
        assert_eq!(
            m.get("secondary-shift-d").and_then(Value::as_str),
            Some("none")
        );
    }

    #[test]
    fn unbind_action_keeps_a_default_another_action_took() {
        let mut m = shortcuts(&[("secondary-shift-d", "close_pane")]);
        unbind_action(&mut m, "split_horizontally");
        assert_eq!(
            m.get("secondary-shift-d").and_then(Value::as_str),
            Some("close_pane")
        );
    }

    #[test]
    fn restore_default_binding_removes_override_and_mask() {
        let mut m = shortcuts(&[
            ("ctrl-alt-h", "split_horizontally"),
            ("secondary-shift-d", "none"),
            ("ctrl-alt-j", "split_vertically"),
        ]);
        restore_default_binding(&mut m, "split_horizontally");
        assert_eq!(m.len(), 1);
        assert_eq!(
            m.get("ctrl-alt-j").and_then(Value::as_str),
            Some("split_vertically")
        );
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

    fn link_to(target: &std::path::Path, link: &std::path::Path) {
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(target, link).expect("symlink");
    }

    fn is_link(path: &std::path::Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
    }

    #[test]
    fn settings_and_resets_write_through_a_symlinked_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let dotfiles = dir.path().join("dotfiles");
        std::fs::create_dir_all(&dotfiles).unwrap();
        let target = dotfiles.join("paneflow.json");
        std::fs::write(
            &target,
            r#"{"theme": "One Dark", "shortcuts": {"ctrl-k": "close_pane"}}"#,
        )
        .unwrap();
        let p = dir.path().join("paneflow.json");
        link_to(&target, &p);

        assert_eq!(
            save_setting_ordered_at(&p, false, "font_size", json!(15.0), 1),
            SettingWrite::Written
        );
        assert!(reset_shortcuts_at(&p));

        assert!(is_link(&p));
        let written = load_raw_config(&target).unwrap();
        assert_eq!(written["font_size"], json!(15.0));
        assert!(written.get("shortcuts").is_none());
    }

    #[test]
    fn a_dangling_config_symlink_is_refused_and_stays_a_link() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nix-store").join("paneflow.json");
        let p = dir.path().join("paneflow.json");
        link_to(&missing, &p);

        assert!(!write_config_checked(&p, &json!({"theme": "One Dark"})));

        assert!(is_link(&p));
        assert!(!missing.exists());
    }
}
