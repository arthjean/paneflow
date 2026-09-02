use std::collections::{HashMap, HashSet};

use gpui::Keystroke;

use super::apply::canonical_keystroke;
use super::defaults::{DEFAULTS, MACOS_ONLY_DEFAULTS};
use super::registry::{ACTIONS, action_description};

pub struct ShortcutEntry {
    pub key: String,
    pub description: String,
    pub action_name: &'static str,
    pub group: super::registry::ShortcutGroup,
    pub search_key: String,
}

pub fn format_keystroke(key: &str) -> String {
    let is_macos = cfg!(target_os = "macos");
    let parts = key.split('-').map(|part| match part {
        "secondary" => {
            if is_macos {
                "\u{2318}".to_string()
            } else {
                "Ctrl".to_string()
            }
        }
        "cmd" | "super" | "win" => {
            if is_macos {
                "\u{2318}".to_string()
            } else {
                "Super".to_string()
            }
        }
        "ctrl" => {
            if is_macos {
                "\u{2303}".to_string()
            } else {
                "Ctrl".to_string()
            }
        }
        "shift" => {
            if is_macos {
                "\u{21E7}".to_string()
            } else {
                "Shift".to_string()
            }
        }
        "alt" => {
            if is_macos {
                "\u{2325}".to_string()
            } else {
                "Alt".to_string()
            }
        }
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
        parts.collect::<String>()
    } else {
        parts.collect::<Vec<_>>().join("+")
    }
}

fn ascii_key_forms(raw_key: &str) -> String {
    let (modifier_part, key) = match raw_key.strip_suffix("--") {
        Some(modifiers) => (modifiers, "-"),
        None => match raw_key.rsplit_once('-') {
            Some((modifiers, key)) => (modifiers, key),
            None => ("", raw_key),
        },
    };

    let alternatives: Vec<Vec<&str>> = modifier_part
        .split('-')
        .filter(|part| !part.is_empty())
        .chain(std::iter::once(key))
        .map(|part| match part {
            "secondary" => vec!["ctrl", "cmd", "command"],
            "cmd" | "super" | "win" => vec!["cmd", "command", "super", "win"],
            "ctrl" => vec!["ctrl", "control"],
            "alt" => vec!["alt", "option", "opt"],
            other => vec![other],
        })
        .collect();

    let mut forms: Vec<String> = vec![String::new()];
    for options in &alternatives {
        let mut next = Vec::with_capacity(forms.len() * options.len());
        for prefix in &forms {
            for option in options {
                if prefix.is_empty() {
                    next.push((*option).to_string());
                } else {
                    next.push(format!("{prefix}+{option}"));
                }
            }
        }
        forms = next;
    }
    forms.join(" ").to_lowercase()
}

pub fn effective_shortcuts(user_shortcuts: &HashMap<String, String>) -> Vec<ShortcutEntry> {
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

    let macos_key_by_action: HashMap<&str, &str> = MACOS_ONLY_DEFAULTS
        .iter()
        .map(|d| (d.action_name, d.key))
        .collect();

    for d in DEFAULTS.iter().chain(MACOS_ONLY_DEFAULTS.iter()) {
        let Some(meta) = ACTIONS.iter().find(|a| a.name == d.action_name) else {
            continue;
        };

        if seen_actions.contains(meta.name) {
            continue;
        }

        let default_key = match macos_key_by_action.get(d.action_name).copied() {
            Some(native) if !is_unbound(native) && !is_user_claimed(native) => native,
            _ => d.key,
        };

        let key = if let Some(user_key) = user_by_action.get(d.action_name) {
            format_keystroke(user_key)
        } else {
            if is_unbound(d.key) || is_user_claimed(d.key) {
                continue;
            }
            format_keystroke(default_key)
        };

        seen_actions.insert(meta.name);
        entries.push(ShortcutEntry {
            key,
            description: meta.description.to_string(),
            action_name: meta.name,
            group: meta.group,
            search_key: ascii_key_forms(
                user_by_action
                    .get(d.action_name)
                    .copied()
                    .unwrap_or(default_key),
            ),
        });
    }

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
                group: meta.group,
                search_key: ascii_key_forms(key),
            });
        }
    }

    for meta in ACTIONS {
        if seen_actions.insert(meta.name) {
            entries.push(ShortcutEntry {
                key: "Unassigned".to_string(),
                description: action_description(meta.name).to_string(),
                action_name: meta.name,
                group: meta.group,
                search_key: String::new(),
            });
        }
    }

    entries
}

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
        let expected = if cfg!(target_os = "macos") {
            "\u{2303}\u{2325}N"
        } else {
            "Ctrl+Alt+N"
        };
        assert_eq!(next_tab.key, expected);
    }

    #[test]
    fn effective_shortcuts_carry_matching_action_name() {
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
        overrides.insert("secondary-shift-d".to_string(), "none".to_string());
        let entries = effective_shortcuts(&overrides);
        let split_h = entries
            .iter()
            .find(|e| e.action_name == "split_horizontally")
            .expect("unbound actions remain visible for rebinding");
        assert_eq!(split_h.key, "Unassigned");
    }

    #[test]
    fn ascii_key_forms_handles_a_minus_key() {
        let forms = ascii_key_forms("secondary--");
        assert!(forms.contains("ctrl+-"), "{forms}");
        assert!(forms.contains("cmd+-"), "{forms}");
        assert!(!forms.contains("++"), "{forms}");
    }

    #[test]
    fn every_default_chord_round_trips_through_parse() {
        for d in DEFAULTS.iter().chain(MACOS_ONLY_DEFAULTS.iter()) {
            let parsed = Keystroke::parse(d.key)
                .unwrap_or_else(|_| panic!("default chord {} does not parse", d.key));
            let round_tripped = Keystroke::parse(&parsed.unparse())
                .unwrap_or_else(|_| panic!("unparse of {} does not re-parse", d.key));
            assert_eq!(
                round_tripped.key, parsed.key,
                "{} lost its key through unparse",
                d.key
            );
            assert_eq!(
                round_tripped.modifiers, parsed.modifiers,
                "{} lost its modifiers through unparse",
                d.key
            );
        }
    }

    #[test]
    fn ascii_key_forms_covers_both_readings_of_secondary() {
        let forms = ascii_key_forms("secondary-shift-d");
        assert!(forms.contains("ctrl+shift+d"), "{forms}");
        assert!(forms.contains("cmd+shift+d"), "{forms}");
        assert!(forms.contains("command+shift+d"), "{forms}");
    }

    #[test]
    fn ascii_key_forms_expands_modifier_aliases() {
        assert!(ascii_key_forms("alt-left").contains("option+left"));
        assert!(ascii_key_forms("ctrl-c").contains("control+c"));
        assert!(ascii_key_forms("cmd-q").contains("super+q"));
    }

    #[test]
    fn ascii_key_forms_is_lowercase_for_substring_matching() {
        let forms = ascii_key_forms("ctrl-shift-PageUp");
        assert_eq!(forms, forms.to_lowercase());
        assert!(forms.contains("pageup"), "{forms}");
    }

    #[test]
    fn the_page_lists_each_action_exactly_once() {
        let entries = effective_shortcuts(&HashMap::new());
        let mut seen: HashSet<&str> = HashSet::new();
        for entry in &entries {
            assert!(
                seen.insert(entry.action_name),
                "{} is listed more than once",
                entry.action_name
            );
        }
        assert_eq!(
            entries.len(),
            ACTIONS.len(),
            "every registry action gets exactly one row"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rows_show_the_platform_native_chord() {
        let entries = effective_shortcuts(&HashMap::new());
        let copy = entries
            .iter()
            .find(|e| e.action_name == "terminal_copy")
            .expect("copy is bound");
        assert_eq!(copy.key, format_keystroke("cmd-c"));
    }

    #[test]
    fn every_entry_carries_its_registry_group() {
        let entries = effective_shortcuts(&HashMap::new());
        for entry in &entries {
            let meta = ACTIONS
                .iter()
                .find(|a| a.name == entry.action_name)
                .expect("every entry comes from the registry");
            assert_eq!(
                entry.group, meta.group,
                "{} landed in the wrong section",
                entry.action_name
            );
        }
        for group in super::super::registry::ShortcutGroup::ALL {
            assert!(
                entries.iter().any(|e| e.group == *group),
                "{group:?} has no rows, so its header would render empty"
            );
        }
    }

    #[test]
    fn bound_entries_have_a_searchable_ascii_key() {
        let entries = effective_shortcuts(&HashMap::new());
        for entry in entries.iter().filter(|e| e.key != "Unassigned") {
            assert!(
                !entry.search_key.is_empty(),
                "{} is bound but not findable by key",
                entry.action_name
            );
        }
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
    fn effective_shortcuts_lists_every_registry_action() {
        let entries = effective_shortcuts(&HashMap::new());
        let listed: HashSet<&str> = entries.iter().map(|e| e.action_name).collect();
        let missing: Vec<&str> = ACTIONS
            .iter()
            .map(|meta| meta.name)
            .filter(|name| !listed.contains(name))
            .collect();
        assert!(
            missing.is_empty(),
            "registry actions absent from the shortcuts settings list: {missing:?}"
        );
    }

    #[test]
    fn effective_shortcuts_invalid_action_ignored() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl+x".to_string(), "bogus_action".to_string());
        let entries = effective_shortcuts(&overrides);
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
        assert_eq!(format_keystroke("secondary-shift-d"), "Ctrl+Shift+D");
        assert_eq!(format_keystroke("secondary-tab"), "Ctrl+Tab");
        assert_eq!(format_keystroke("secondary-1"), "Ctrl+1");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn secondary_renders_as_cmd_glyph_on_macos() {
        assert_eq!(format_keystroke("secondary-shift-d"), "\u{2318}\u{21E7}D");
        assert_eq!(format_keystroke("secondary-tab"), "\u{2318}Tab");
        assert_eq!(format_keystroke("secondary-1"), "\u{2318}1");
        assert_eq!(format_keystroke("cmd-shift-d"), "\u{2318}\u{21E7}D");
    }
}
