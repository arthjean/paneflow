use std::collections::{HashMap, HashSet};

use gpui::Keystroke;

use super::apply::canonical_keystroke;
use super::defaults::{DEFAULTS, PLATFORM_DEFAULTS};
use super::registry::{ACTIONS, action_description};

pub struct ShortcutEntry {
    pub key: String,
    pub raw_key: Option<String>,
    pub extra_raw_keys: Vec<String>,
    pub customized: bool,
    pub description: String,
    pub action_name: &'static str,
    pub group: super::registry::ShortcutGroup,
    pub search_key: String,
}

pub fn keystroke_caps(formatted: &str) -> Vec<String> {
    if formatted == "Unassigned" {
        return Vec::new();
    }
    if cfg!(target_os = "macos") {
        let mut caps = Vec::new();
        let mut rest = String::new();
        for ch in formatted.chars() {
            if rest.is_empty() && matches!(ch, '\u{2318}' | '\u{2303}' | '\u{21E7}' | '\u{2325}') {
                caps.push(ch.to_string());
            } else {
                rest.push(ch);
            }
        }
        if !rest.is_empty() {
            caps.push(rest);
        }
        caps
    } else {
        match formatted.strip_suffix("+-") {
            Some(modifiers) => modifiers
                .split('+')
                .map(str::to_string)
                .chain(std::iter::once("-".to_string()))
                .collect(),
            None => formatted.split('+').map(str::to_string).collect(),
        }
    }
}

pub fn default_keys(action_name: &str) -> Vec<&'static str> {
    DEFAULTS
        .iter()
        .chain(PLATFORM_DEFAULTS.iter())
        .filter(|d| d.action_name == action_name)
        .map(|d| d.key)
        .collect()
}

fn split_keystroke(raw_key: &str) -> (&str, &str) {
    match raw_key.strip_suffix("--") {
        Some(modifiers) => (modifiers, "-"),
        None => match raw_key.rsplit_once('-') {
            Some((modifiers, key)) => (modifiers, key),
            None => ("", raw_key),
        },
    }
}

pub fn format_keystroke(key: &str) -> String {
    let is_macos = cfg!(target_os = "macos");
    let (modifier_part, key) = split_keystroke(key);
    let tokens = modifier_part
        .split('-')
        .filter(|part| !part.is_empty())
        .chain(std::iter::once(key));
    let parts = tokens.map(|part| match part {
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
        other => key_label(other),
    });
    if is_macos {
        parts.collect::<String>()
    } else {
        parts.collect::<Vec<_>>().join("+")
    }
}

fn key_label(key: &str) -> String {
    let mut chars = key.chars();
    match chars.next() {
        Some(first) if key.chars().count() > 1 => first.to_uppercase().chain(chars).collect(),
        _ => key.to_uppercase(),
    }
}

fn ascii_key_forms(raw_key: &str) -> String {
    let (modifier_part, key) = split_keystroke(raw_key);

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
    let mut user_by_action: HashMap<&str, Vec<&str>> = HashMap::new();
    for (key, action_name) in user_shortcuts {
        if action_name != "none" && ACTIONS.iter().any(|a| a.name == action_name) {
            user_by_action
                .entry(action_name.as_str())
                .or_default()
                .push(key.as_str());
        }
    }
    for keys in user_by_action.values_mut() {
        keys.sort_unstable();
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

    let platform_key_by_action: HashMap<&str, &str> = PLATFORM_DEFAULTS
        .iter()
        .map(|d| (d.action_name, d.key))
        .collect();

    for d in DEFAULTS.iter().chain(PLATFORM_DEFAULTS.iter()) {
        let Some(meta) = ACTIONS.iter().find(|a| a.name == d.action_name) else {
            continue;
        };

        if seen_actions.contains(meta.name) {
            continue;
        }

        let default_key = match platform_key_by_action.get(d.action_name).copied() {
            Some(native) if !is_unbound(native) && !is_user_claimed(native) => native,
            _ => d.key,
        };

        let user_keys = user_by_action.get(d.action_name);
        let keys: &[&str] = match user_keys {
            Some(user_keys) => user_keys,
            None => {
                if is_unbound(default_key) || is_user_claimed(default_key) {
                    continue;
                }
                std::slice::from_ref(&default_key)
            }
        };

        let Some((primary, extras)) = keys.split_first() else {
            continue;
        };
        seen_actions.insert(meta.name);
        entries.push(bound_entry(meta, primary, extras, user_keys.is_some()));
    }

    for meta in ACTIONS {
        if let Some((primary, extras)) = user_by_action
            .get(meta.name)
            .and_then(|keys| keys.split_first())
            && seen_actions.insert(meta.name)
        {
            entries.push(bound_entry(meta, primary, extras, true));
        }
    }

    for meta in ACTIONS {
        if seen_actions.insert(meta.name) {
            entries.push(ShortcutEntry {
                key: "Unassigned".to_string(),
                raw_key: None,
                extra_raw_keys: Vec::new(),
                customized: !default_keys(meta.name).is_empty(),
                description: action_description(meta.name).to_string(),
                action_name: meta.name,
                group: meta.group,
                search_key: String::new(),
            });
        }
    }

    entries
}

fn bound_entry(
    meta: &'static super::registry::ActionMeta,
    primary: &str,
    extras: &[&str],
    customized: bool,
) -> ShortcutEntry {
    ShortcutEntry {
        key: format_keystroke(primary),
        raw_key: Some((*primary).to_string()),
        extra_raw_keys: extras.iter().map(|key| (*key).to_string()).collect(),
        customized,
        description: meta.description.to_string(),
        action_name: meta.name,
        group: meta.group,
        search_key: std::iter::once(primary)
            .chain(extras.iter().copied())
            .map(ascii_key_forms)
            .collect::<Vec<_>>()
            .join(" "),
    }
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
    fn paste_shortcut_display_tracks_available_bindings() {
        let native = if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        };
        let alternate = "ctrl-shift-v";
        let cases = [
            (vec![], Some(native)),
            (vec![(native, "none")], Some(alternate)),
            (vec![(alternate, "none")], Some(native)),
            (vec![(native, "none"), (alternate, "none")], None),
            (vec![(native, "split_horizontally")], Some(alternate)),
            (vec![(alternate, "split_horizontally")], Some(native)),
            (vec![("ctrl-alt-v", "terminal_paste")], Some("ctrl-alt-v")),
        ];
        for (overrides, expected) in cases {
            let overrides = overrides
                .into_iter()
                .map(|(key, action)| (key.to_string(), action.to_string()))
                .collect();
            let entries = effective_shortcuts(&overrides);
            let paste: Vec<_> = entries
                .iter()
                .filter(|entry| entry.action_name == "terminal_paste")
                .collect();
            assert_eq!(paste.len(), 1);
            assert_eq!(
                paste[0].key,
                expected.map_or_else(|| "Unassigned".to_string(), format_keystroke),
                "overrides: {overrides:?}"
            );
            assert_eq!(
                paste[0].search_key,
                expected.map_or_else(String::new, ascii_key_forms),
                "overrides: {overrides:?}"
            );
        }
    }

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
        assert!(
            descriptions.contains(&"Close pane (stops its sessions, asks when an agent is busy)"),
            "Missing close pane"
        );
        assert!(
            descriptions.contains(&"Next workspace"),
            "Missing next workspace"
        );
        assert!(descriptions.contains(&"Focus left"), "Missing focus left");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn named_keys_render_in_sentence_case_and_letters_upper() {
        let enter = format_keystroke("shift-enter");
        let space = format_keystroke("secondary-shift-space");
        assert!(enter.ends_with("Enter"), "{enter}");
        assert!(space.ends_with("Space"), "{space}");
        assert!(format_keystroke("secondary-a").ends_with('A'));
    }

    #[test]
    fn keystroke_caps_split_modifiers_and_keep_a_minus_key() {
        if cfg!(target_os = "macos") {
            assert_eq!(
                keystroke_caps("\u{2318}\u{21E7}D"),
                ["\u{2318}", "\u{21E7}", "D"]
            );
        } else {
            assert_eq!(keystroke_caps("Ctrl+Shift+D"), ["Ctrl", "Shift", "D"]);
            assert_eq!(keystroke_caps("Ctrl+-"), ["Ctrl", "-"]);
            assert_eq!(keystroke_caps("Ctrl+Shift+="), ["Ctrl", "Shift", "="]);
        }
        assert!(keystroke_caps("Unassigned").is_empty());
    }

    #[test]
    fn effective_shortcuts_flag_overrides_and_masked_defaults_as_customized() {
        let mut user = HashMap::new();
        user.insert("ctrl-alt-h".to_string(), "split_horizontally".to_string());
        user.insert("secondary-shift-e".to_string(), "none".to_string());
        let entries = effective_shortcuts(&user);
        let split_h = entries
            .iter()
            .find(|e| e.action_name == "split_horizontally")
            .expect("split_horizontally is listed");
        assert!(split_h.customized);
        assert_eq!(split_h.raw_key.as_deref(), Some("ctrl-alt-h"));
        let split_v = entries
            .iter()
            .find(|e| e.action_name == "split_vertically")
            .expect("split_vertically is listed");
        assert!(split_v.customized);
        assert_eq!(split_v.key, "Unassigned");
        assert_eq!(split_v.raw_key, None);
        let close = entries
            .iter()
            .find(|e| e.action_name == "close_pane")
            .expect("close_pane is listed");
        assert!(!close.customized);
        let clone = entries
            .iter()
            .find(|e| e.action_name == "clone_repository")
            .expect("clone_repository is listed");
        assert!(
            !clone.customized,
            "an action without a default is not a change"
        );
    }

    #[test]
    fn effective_shortcuts_user_override_replaces_key() {
        let mut overrides = HashMap::new();
        overrides.insert("ctrl-alt-h".to_string(), "split_horizontally".to_string());
        let entries = effective_shortcuts(&overrides);
        let split_h = entries
            .iter()
            .find(|e| e.description == "Split horizontal")
            .expect("Split horizontal should be in effective list");
        let expected = if cfg!(target_os = "macos") {
            "\u{2303}\u{2325}H"
        } else {
            "Ctrl+Alt+H"
        };
        assert_eq!(
            split_h.key, expected,
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
        for d in DEFAULTS.iter().chain(PLATFORM_DEFAULTS.iter()) {
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
            .find(|e| {
                e.description == "Close pane (stops its sessions, asks when an agent is busy)"
            })
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

    #[test]
    fn format_keystroke_keeps_the_minus_key() {
        let (secondary, shift) = if cfg!(target_os = "macos") {
            ("\u{2318}", "\u{21E7}")
        } else {
            ("Ctrl+", "Shift+")
        };
        assert_eq!(format_keystroke("secondary--"), format!("{secondary}-"));
        assert_eq!(
            format_keystroke("secondary-shift--"),
            format!("{secondary}{shift}-")
        );
        assert_eq!(format_keystroke("secondary-="), format!("{secondary}="));
        assert_eq!(format_keystroke("tab"), "Tab");
        assert_eq!(
            keystroke_caps(&format_keystroke("secondary--"))
                .last()
                .map(String::as_str),
            Some("-")
        );
    }

    #[test]
    fn font_size_decrease_shows_its_minus_key_in_the_shortcut_list() {
        let entries = effective_shortcuts(&HashMap::new());
        let decrease = entries
            .iter()
            .find(|entry| entry.action_name == "font_size_decrease")
            .expect("font_size_decrease is listed");
        assert_eq!(decrease.raw_key.as_deref(), Some("secondary--"));
        assert!(decrease.key.ends_with('-'), "{}", decrease.key);
    }

    #[test]
    fn every_user_shortcut_of_an_action_is_listed_in_a_stable_order() {
        let user: HashMap<String, String> = [
            ("ctrl-alt-z", "split_horizontally"),
            ("ctrl-alt-a", "split_horizontally"),
            ("ctrl-alt-m", "split_horizontally"),
        ]
        .into_iter()
        .map(|(key, action)| (key.to_string(), action.to_string()))
        .collect();
        for _ in 0..8 {
            let entries = effective_shortcuts(&user);
            let split: Vec<_> = entries
                .iter()
                .filter(|entry| entry.action_name == "split_horizontally")
                .collect();
            assert_eq!(split.len(), 1);
            assert_eq!(split[0].raw_key.as_deref(), Some("ctrl-alt-a"));
            assert_eq!(split[0].extra_raw_keys, ["ctrl-alt-m", "ctrl-alt-z"]);
            assert!(split[0].customized);
            for form in ["ctrl+alt+a", "ctrl+alt+m", "ctrl+alt+z"] {
                assert!(split[0].search_key.contains(form), "{form}");
            }
        }
    }

    #[test]
    fn user_only_actions_list_all_their_shortcuts() {
        let unbound_by_default = ACTIONS
            .iter()
            .find(|meta| default_keys(meta.name).is_empty())
            .expect("at least one action ships without a default");
        let user: HashMap<String, String> = [
            ("ctrl-alt-y", unbound_by_default.name),
            ("ctrl-alt-b", unbound_by_default.name),
        ]
        .into_iter()
        .map(|(key, action)| (key.to_string(), action.to_string()))
        .collect();
        let entries = effective_shortcuts(&user);
        let entry = entries
            .iter()
            .find(|entry| entry.action_name == unbound_by_default.name)
            .expect("the user-bound action is listed");
        assert_eq!(entry.raw_key.as_deref(), Some("ctrl-alt-b"));
        assert_eq!(entry.extra_raw_keys, ["ctrl-alt-y"]);
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
