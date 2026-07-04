//! Config file I/O: read-modify-write helpers for `paneflow.json`.
//!
//! All functions operate on raw JSON to preserve unknown fields and formatting.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

/// US-016: serialize every read-modify-write of `paneflow.json`.
///
/// Settings-tab control handlers persist off the GPUI main thread - each
/// `persist_setting` spawns its own `cx.background_spawn → smol::unblock` task
/// (`settings/window.rs`). Without this lock two rapid toggles run two
/// independent `load_raw_config → mutate → write_config_checked` cycles on the
/// blocking pool: they read the same pre-change file and the later `rename`
/// wins, silently dropping the other key's update (CWE-362 lost update). They
/// also share the PID-suffixed temp path, so concurrent writes can clobber it.
/// Holding this guard across the whole load→write of each writer makes the RMW
/// atomic w.r.t. other writers, so each one observes the previous one's result.
///
/// (It does NOT order two writes of the *same* key - the last task to acquire
/// wins regardless of spawn order - but `cached_config` in memory stays the
/// source of truth for the live session, so a same-key reorder only matters
/// across a restart, and is self-healed by the next write or external reload.)
static CONFIG_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Acquire the config-write lock, recovering from a poisoned mutex (the guarded
/// value is `()`, so a prior panic-while-held left nothing to corrupt). Avoids
/// an `.unwrap()` on the lock per the repo's prod-unwrap lint.
fn config_write_guard() -> MutexGuard<'static, ()> {
    CONFIG_WRITE_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Load the raw JSON config.
///
/// Missing file means a fresh empty object. Existing but unreadable, oversized,
/// non-regular, or syntactically invalid files are rejected so a settings write
/// cannot overwrite the user's recoverable `paneflow.json` with `{}`.
fn load_raw_config(path: &Path) -> Result<serde_json::Value, ()> {
    match paneflow_config::loader::read_config_string(path) {
        paneflow_config::loader::ConfigRead::Absent => Ok(serde_json::json!({})),
        paneflow_config::loader::ConfigRead::Rejected => Err(()),
        paneflow_config::loader::ConfigRead::Contents(contents) => {
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

/// Write a JSON value back to the config file, creating parent dirs if needed.
fn write_config(path: &PathBuf, value: &serde_json::Value) {
    let _ = write_config_checked(path, value);
}

/// Result-returning variant of [`write_config`]. Returns `true` on
/// successful write, `false` otherwise (serialization or I/O error -
/// logged at WARN in both cases).
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

    // US-031: write atomically (tmp + rename) so a crash mid-write can't
    // truncate `paneflow.json`. A truncated file parses as invalid JSON, which
    // `load_raw_config` silently swallows as an empty object - discarding the
    // user's entire config. The temp file is PID-suffixed and lives in the
    // target's own directory so the rename stays on one filesystem (a
    // cross-FS rename is neither atomic nor, on some platforms, permitted).
    // `std::fs::rename` replaces the destination atomically on all three OSes.
    let Some(parent) = path.parent() else {
        // No parent component (not expected for a real config path): fall back
        // to a best-effort direct write rather than refusing outright.
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

/// Save a top-level config field (e.g. `"font_size"`, `"line_height"`).
pub fn save_config_value(key: &str, value: serde_json::Value) {
    let _ = save_config_value_checked(key, value);
}

/// Same as `save_config_value`, but returns `true` on success and `false`
/// when the config path could not be resolved or the file write failed.
///
/// Callers that need to surface persistence failures to the user (e.g. the
/// telemetry consent modal in US-011, which must honor the choice
/// in-memory and show a toast when the disk write fails) should use this
/// variant. The void `save_config_value` wrapper above is kept for
/// fire-and-forget call sites that already accept best-effort writes.
pub fn save_config_value_checked(key: &str, value: serde_json::Value) -> bool {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return false;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return false;
    };
    if let Some(root) = json.as_object_mut() {
        if value.is_null() {
            root.remove(key);
        } else {
            root.insert(key.to_string(), value);
        }
    }
    write_config_checked(&path, &json)
}

/// Pure read-modify-write of the `shortcuts` map. Extracted from
/// [`save_shortcut`] so the dedupe + collision semantics can be unit-tested
/// without touching the real config path (mirrors [`apply_terminal_field`]).
///
/// Removes (a) any prior binding for `action_name` (so a remap doesn't leave
/// the old key live) and (b) any binding whose key collides with `new_key`
/// (US-021: `new_key` now belongs to `action_name`, so its previous owner
/// loses it - otherwise two user entries on the same physical chord would
/// produce a GPUI-ambiguous double binding). The collision test is
/// normalization-aware (`ctrl+shift+f` stored vs `ctrl-shift-f` recorded).
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

/// Save a single shortcut override to `paneflow.json`.
///
/// Merges the new binding into `shortcuts`, removing any previous key for the
/// same action and any other action that already held `new_key`.
pub fn save_shortcut(new_key: &str, action_name: &str) {
    let Some(path) = paneflow_config::loader::config_path() else {
        log::warn!("config: cannot determine config path, not saving");
        return;
    };
    let _guard = config_write_guard();
    let Ok(mut json) = load_raw_config(&path) else {
        return;
    };

    // Guard rather than `.expect()` so a future loader contract change cannot
    // panic on the UI thread.
    let Some(root) = json.as_object_mut() else {
        log::warn!("config: root is not a JSON object, not saving shortcut");
        return;
    };
    // Ensure `shortcuts` exists and is an object (replace a non-object).
    let shortcuts = root
        .entry("shortcuts")
        .or_insert_with(|| serde_json::json!({}));
    if !shortcuts.is_object() {
        *shortcuts = serde_json::json!({});
    }
    let Some(shortcuts_obj) = shortcuts.as_object_mut() else {
        return;
    };

    merge_shortcut(shortcuts_obj, new_key, action_name);

    write_config(&path, &json);
}

/// Remove all user shortcut overrides from `paneflow.json`, restoring defaults.
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

/// Pure read-modify-write of the `"terminal"` sub-object. Extracted from
/// [`save_terminal_field_checked`] so the nesting/removal semantics can be unit-tested
/// without resolving (or touching) the real config path.
fn apply_terminal_field(json: &mut serde_json::Value, key: &str, value: serde_json::Value) {
    let Some(root) = json.as_object_mut() else {
        return;
    };
    // Ensure `terminal` exists and is an object (replace a non-object).
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

/// Pure read-modify-write of the `"agent_panel"` sub-object. Mirrors
/// [`apply_terminal_field`] for settings that are scoped to the Agents panel,
/// such as the native OS notification gate.
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

/// US-016: return a copy of `config` with a single field updated *in memory*,
/// mirroring the on-disk merge of [`save_config_value`] / [`save_terminal_field_checked`]
/// without touching disk. A settings handler uses this to refresh its render
/// cache instantly, then persists asynchronously. `nested` routes the field
/// into the `terminal` block; a `Null` value clears it. The config is the typed
/// view (no unknown fields), so the JSON round-trip is lossless for it.
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

/// In-memory companion for [`save_agent_panel_field_checked`].
pub fn with_agent_panel_field(
    config: &paneflow_config::schema::PaneFlowConfig,
    key: &str,
    value: serde_json::Value,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut json = serde_json::to_value(config).unwrap_or_else(|_| serde_json::json!({}));
    apply_agent_panel_field(&mut json, key, value);
    serde_json::from_value(json).unwrap_or_else(|_| config.clone())
}

/// In-memory companion for [`save_commands_checked`]. Replaces the full
/// user-defined command/template list while preserving every other config
/// field.
pub fn with_commands(
    config: &paneflow_config::schema::PaneFlowConfig,
    commands: Vec<paneflow_config::schema::CommandDefinition>,
) -> paneflow_config::schema::PaneFlowConfig {
    let mut next = config.clone();
    next.commands = commands;
    next
}

/// Save a single field inside the `"terminal": { ... }` block in `paneflow.json`
/// (US-016 Terminal settings tab). A `Null` value removes the key (restoring
/// the schema default on next load); the `"terminal"` object itself is left in
/// place (an empty block is harmless - `#[serde(default)]` handles it).
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

/// Save a single field inside the `"agent_panel": { ... }` block in
/// `paneflow.json`, preserving sibling agent-panel settings and unknown fields.
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

/// Save the full `commands` array in `paneflow.json`, preserving unknown
/// top-level keys and sibling settings.
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
        // US-031: the write goes through tmp+rename, the target ends up with
        // the full content, and no temp file is left behind in the directory.
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
        // A second write fully replaces the first (no partial/truncated file).
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
        // Rebinding an action moves its key: the old key must not stay live.
        let mut m = shortcuts(&[("ctrl-alt-h", "split_horizontally")]);
        merge_shortcut(&mut m, "ctrl-alt-j", "split_horizontally");
        assert!(!m.contains_key("ctrl-alt-h"), "old key should be removed");
        assert_eq!(m["ctrl-alt-j"], json!("split_horizontally"));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn merge_shortcut_collision_evicts_other_action() {
        // US-021: binding a key already owned by another action must evict the
        // previous owner, not leave both entries live (GPUI double binding).
        let mut m = shortcuts(&[("ctrl-shift-f", "toggle_search")]);
        merge_shortcut(&mut m, "ctrl-shift-f", "close_pane");
        assert_eq!(m["ctrl-shift-f"], json!("close_pane"));
        assert_eq!(m.len(), 1, "no leftover binding for the evicted action");
    }

    #[test]
    fn merge_shortcut_collision_is_normalization_aware() {
        // A stored "+"-separated key and a recorded "-"-separated key denote
        // the same chord; the collision filter must collapse them.
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
