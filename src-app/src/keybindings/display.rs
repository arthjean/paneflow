//! Render shortcut entries for the settings UI + menu bar.

use std::collections::{HashMap, HashSet};

use gpui::Keystroke;

use super::apply::canonical_keystroke;
use super::defaults::{DEFAULTS, MACOS_ONLY_DEFAULTS};
use super::registry::{ACTIONS, action_description};

/// A resolved shortcut entry for display in the settings page.
pub struct ShortcutEntry {
    pub key: String,
    pub description: String,
    /// US-021: the action this row rebinds, as the canonical `&'static str`
    /// from the registry. The settings editor MUST key its rebind off this,
    /// not off the row's positional index: the displayed list chains
    /// `MACOS_ONLY_DEFAULTS`, skips unbound rows, and appends user-only
    /// actions, so `index → DEFAULTS[index]` is only correct in the trivial
    /// (zero-override) case. Indexing `DEFAULTS` by row would silently rebind
    /// the *wrong* action and corrupt `paneflow.json`.
    pub action_name: &'static str,
}

/// Format a GPUI keystroke string for display.
///
/// On Linux: `"secondary-shift-d"` → `"Ctrl+Shift+D"` (readable, plus-separated).
/// On macOS: `"secondary-shift-d"` → `"⌘⇧D"` (Apple HIG glyphs, no separator -
/// matches the native macOS menu bar convention consumed by US-012).
///
/// `secondary` is GPUI's cross-platform shorthand that resolves to `cmd` on
/// macOS and `ctrl` elsewhere (see `Keystroke::parse`). Rendering it here
/// mirrors that resolution so the menu bar always shows the actual key the
/// user will press.
pub fn format_keystroke(key: &str) -> String {
    let is_macos = cfg!(target_os = "macos");
    let parts = key.split('-').map(|part| match part {
        // Modifiers - platform-dependent rendering.
        "secondary" => {
            if is_macos {
                "\u{2318}".to_string() // ⌘
            } else {
                "Ctrl".to_string()
            }
        }
        "cmd" | "super" | "win" => {
            if is_macos {
                "\u{2318}".to_string() // ⌘
            } else {
                "Super".to_string()
            }
        }
        "ctrl" => {
            if is_macos {
                "\u{2303}".to_string() // ⌃
            } else {
                "Ctrl".to_string()
            }
        }
        "shift" => {
            if is_macos {
                "\u{21E7}".to_string() // ⇧
            } else {
                "Shift".to_string()
            }
        }
        "alt" => {
            if is_macos {
                "\u{2325}".to_string() // ⌥
            } else {
                "Alt".to_string()
            }
        }
        // Non-modifier tokens - same on both platforms, just capitalized.
        "tab" => "Tab".to_string(),
        "pageup" => "PageUp".to_string(),
        "pagedown" => "PageDown".to_string(),
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        other => other.to_uppercase(),
    });
    if is_macos {
        // Apple HIG: modifier glyphs flow directly into the key label, no `+`.
        parts.collect::<String>()
    } else {
        parts.collect::<Vec<_>>().join("+")
    }
}

/// Compute the effective shortcut list by merging defaults with user overrides.
///
/// User overrides replace default bindings for the same action. Additional user
/// bindings are appended. Registry actions with no binding are still listed as
/// `Unassigned` so every action exposed by the keybinding registry is rebindable
/// from Settings.
pub fn effective_shortcuts(user_shortcuts: &HashMap<String, String>) -> Vec<ShortcutEntry> {
    // Build reverse map: action_name -> user key (last one wins if duplicates).
    let mut user_by_action: HashMap<&str, &str> = HashMap::new();
    for (key, action_name) in user_shortcuts {
        if action_name != "none" && ACTIONS.iter().any(|a| a.name == action_name) {
            user_by_action.insert(action_name.as_str(), key.as_str());
        }
    }

    let unbound_canonical: HashSet<Keystroke> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() == "none")
        .filter_map(|(k, _)| canonical_keystroke(k))
        .collect();
    let user_bound_canonical: HashSet<Keystroke> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() != "none")
        .filter(|(_, action_name)| ACTIONS.iter().any(|a| a.name == *action_name))
        .filter_map(|(k, _)| canonical_keystroke(k))
        .collect();
    let is_unbound =
        |key: &str| canonical_keystroke(key).is_some_and(|k| unbound_canonical.contains(&k));
    let is_user_claimed =
        |key: &str| canonical_keystroke(key).is_some_and(|k| user_bound_canonical.contains(&k));

    let mut entries = Vec::new();
    let mut seen_actions: HashSet<&'static str> = HashSet::new();

    // Defaults first, with user overrides applied. US-010: include the
    // macOS-only defaults so the settings page reflects cmd-c/cmd-v on
    // macOS (and stays unchanged on Linux where MACOS_ONLY_DEFAULTS is empty).
    for d in DEFAULTS.iter().chain(MACOS_ONLY_DEFAULTS.iter()) {
        let Some(meta) = ACTIONS.iter().find(|a| a.name == d.action_name) else {
            continue;
        };

        // If user overrode this action to a different key, show that key. If a
        // different action claimed this default chord, mirror apply_keybindings
        // and hide the displaced default row until it is explicitly rebound.
        let key = if let Some(user_key) = user_by_action.get(d.action_name) {
            format_keystroke(user_key)
        } else {
            if is_unbound(d.key) || is_user_claimed(d.key) {
                continue;
            }
            format_keystroke(d.key)
        };

        seen_actions.insert(meta.name);
        entries.push(ShortcutEntry {
            key,
            description: meta.description.to_string(),
            action_name: meta.name,
        });
    }

    // Add user bindings for actions that are not in the default tables.
    for (key, action_name) in user_shortcuts {
        if action_name == "none" {
            continue;
        }
        if let Some(meta) = ACTIONS.iter().find(|a| a.name == action_name)
            && seen_actions.insert(meta.name)
        {
            entries.push(ShortcutEntry {
                key: format_keystroke(key),
                description: meta.description.to_string(),
                action_name: meta.name,
            });
        }
    }

    for meta in ACTIONS {
        if seen_actions.insert(meta.name) {
            entries.push(ShortcutEntry {
                key: "Unassigned".to_string(),
                description: action_description(meta.name).to_string(),
                action_name: meta.name,
            });
        }
    }

    entries
}

/// Returns `true` if the keystroke is a bare modifier press (no actual key).
pub fn is_bare_modifier(keystroke: &Keystroke) -> bool {
    matches!(
        keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_shortcuts_defaults_include_core_actions() {
        let entries = effective_shortcuts(&HashMap::new());
        let descriptions: Vec<&str> = entries.iter().map(|e| e.description.as_str()).collect();
        assert!(
            descriptions.contains(&"Split horizontal"),
            "Missing split horizontal"
        );
        assert!(
            descriptions.contains(&"Split vertical"),
            "Missing split vertical"
        );
        assert!(descriptions.contains(&"Close pane"), "Missing close pane");
        assert!(
            descriptions.contains(&"Next workspace"),
            "Missing next workspace"
        );
        assert!(descriptions.contains(&"Focus left"), "Missing focus left");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn effective_shortcuts_user_override_replaces_key() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl-alt-h".to_string(), "split_horizontally".to_string());
        let entries = effective_shortcuts(&overrides);
        let split_h = entries
            .iter()
            .find(|e| e.description == "Split horizontal")
            .expect("Split horizontal should be in effective list");
        assert_eq!(
            split_h.key, "Ctrl+Alt+H",
            "User override should replace the default key"
        );
    }

    /// US-020: the two tab-cycling shortcuts show up in Settings -> Shortcuts
    /// with their default chord, and a user override replaces it there too.
    #[test]
    fn effective_shortcuts_expose_tab_cycling() {
        let entries = effective_shortcuts(&HashMap::new());
        let row = |action: &str| {
            entries
                .iter()
                .find(|e| e.action_name == action)
                .unwrap_or_else(|| panic!("{action} must be listed in Settings -> Shortcuts"))
        };
        assert_eq!(row("next_tab").description, "Next tab");
        assert_eq!(row("previous_tab").description, "Previous tab");
        assert!(
            !row("next_tab").key.is_empty(),
            "the default chord is shown"
        );

        let mut overrides = HashMap::new();
        overrides.insert("ctrl-alt-n".to_string(), "next_tab".to_string());
        let overridden = effective_shortcuts(&overrides);
        let next_tab = overridden
            .iter()
            .find(|e| e.action_name == "next_tab")
            .expect("next_tab stays listed once overridden");
        assert_eq!(next_tab.key, "Ctrl+Alt+N");
    }

    #[test]
    fn effective_shortcuts_carry_matching_action_name() {
        // US-021: every row knows the action it rebinds. The editor keys off
        // this, so it must line up with the row's description.
        let entries = effective_shortcuts(&HashMap::new());
        for e in &entries {
            assert_eq!(
                e.description,
                action_description(e.action_name),
                "row description must match its action_name"
            );
        }
    }

    #[test]
    fn effective_shortcuts_action_name_survives_unbind_shift() {
        // Regression for the `action_name_at(idx) → DEFAULTS[idx]` bug: once a
        // default is unbound the displayed list shifts, so the row at index 0
        // is the SECOND default - not `DEFAULTS[0]`. Reading the carried
        // `action_name` must reflect the shifted row, otherwise the editor
        // rebinds the wrong action.
        let mut overrides = HashMap::new();
        overrides.insert("secondary-shift-d".to_string(), "none".to_string());
        let entries = effective_shortcuts(&overrides);
        assert_eq!(
            entries[0].action_name, "split_vertically",
            "first row should be the second default after the first is unbound"
        );
        assert_ne!(
            entries[0].action_name, "split_horizontally",
            "indexing DEFAULTS[0] here would rebind the wrong (unbound) action"
        );
    }

    #[test]
    fn effective_shortcuts_none_unbinds_key() {
        let mut overrides = HashMap::new();
        // US-009: default is now `secondary-shift-d`; unbinding requires the
        // canonical default key string.
        overrides.insert("secondary-shift-d".to_string(), "none".to_string());
        let entries = effective_shortcuts(&overrides);
        let split_h = entries
            .iter()
            .find(|e| e.action_name == "split_horizontally")
            .expect("unbound actions remain visible for rebinding");
        assert_eq!(split_h.key, "Unassigned");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn effective_shortcuts_none_unbinds_canonical_equivalent_key() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl+shift+d".to_string(), "none".to_string());
        let entries = effective_shortcuts(&overrides);
        let split_h = entries
            .iter()
            .find(|e| e.action_name == "split_horizontally")
            .expect("unbound actions remain visible for rebinding");
        assert_eq!(split_h.key, "Unassigned");
    }

    #[test]
    fn effective_shortcuts_lists_registry_actions_without_defaults() {
        let entries = effective_shortcuts(&HashMap::new());
        let close_window = entries
            .iter()
            .find(|e| e.action_name == "close_window")
            .expect("registry action should be visible in shortcuts settings");
        assert_eq!(close_window.key, "Unassigned");
    }

    #[test]
    fn effective_shortcuts_invalid_action_ignored() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl+x".to_string(), "bogus_action".to_string());
        let entries = effective_shortcuts(&overrides);
        // Invalid action should not appear
        let has_bogus = entries
            .iter()
            .any(|e| e.description == "Unknown" && e.key == "Ctrl+X");
        assert!(!has_bogus, "Invalid action should not be in effective list");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn effective_shortcuts_preserves_unoverridden_defaults() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl+alt+h".to_string(), "split_horizontally".to_string());
        let entries = effective_shortcuts(&overrides);
        // close_pane should still be at its default key. US-009: default is
        // `secondary-shift-w`, which renders as "Ctrl+Shift+W" on Linux.
        let close = entries
            .iter()
            .find(|e| e.description == "Close pane")
            .expect("Close pane should be in effective list");
        assert_eq!(
            close.key, "Ctrl+Shift+W",
            "Unoverridden action should keep default key"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn format_keystroke_produces_readable_output() {
        assert_eq!(format_keystroke("ctrl-shift-d"), "Ctrl+Shift+D");
        assert_eq!(format_keystroke("alt-left"), "Alt+Left");
        assert_eq!(format_keystroke("ctrl-1"), "Ctrl+1");
        assert_eq!(format_keystroke("shift-pageup"), "Shift+PageUp");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn secondary_renders_as_ctrl_on_linux() {
        // AC2: secondary resolves to Ctrl on Linux; format_keystroke mirrors
        // that so the menu bar / shortcut list shows the key the user will
        // actually press.
        assert_eq!(format_keystroke("secondary-shift-d"), "Ctrl+Shift+D");
        assert_eq!(format_keystroke("secondary-tab"), "Ctrl+Tab");
        assert_eq!(format_keystroke("secondary-1"), "Ctrl+1");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn secondary_renders_as_cmd_glyph_on_macos() {
        // AC6: macOS menu bar expects Apple HIG glyphs, no plus separator.
        assert_eq!(format_keystroke("secondary-shift-d"), "\u{2318}\u{21E7}D");
        assert_eq!(format_keystroke("secondary-tab"), "\u{2318}Tab");
        assert_eq!(format_keystroke("secondary-1"), "\u{2318}1");
        // Explicit `cmd` token also renders as ⌘ (user override form from AC5).
        assert_eq!(format_keystroke("cmd-shift-d"), "\u{2318}\u{21E7}D");
    }
}
