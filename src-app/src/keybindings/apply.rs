use std::collections::HashMap;

use gpui::{Action, App, KeyBinding, KeyBindingContextPredicate, Keystroke, OperatingSystem};

use super::defaults::{DEFAULTS, PLATFORM_DEFAULTS};
use super::registry::{action_from_name, context_for_action};

pub(super) fn normalize_keystroke(keystrokes: &str) -> String {
    keystrokes.replace('+', "-")
}

pub fn host_os() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Mac
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

pub fn parse_keystroke(source: &str) -> Result<Keystroke, String> {
    match Keystroke::parse_with_os(source, host_os()) {
        Ok(keystroke) if !keystroke.key.is_empty() => Ok(keystroke),
        _ => Err(format!("invalid keystroke: {source}")),
    }
}

pub(super) fn canonical_keystroke(keystrokes: &str) -> Option<Keystroke> {
    parse_keystroke(&normalize_keystroke(keystrokes)).ok()
}

pub fn keystrokes_conflict(a: &str, b: &str) -> bool {
    match (canonical_keystroke(a), canonical_keystroke(b)) {
        (Some(ka), Some(kb)) => ka == kb,
        _ => a == b,
    }
}

pub(super) fn make_binding(
    keystrokes: &str,
    action: Box<dyn Action>,
    context: Option<&str>,
    cx: &App,
) -> Option<KeyBinding> {
    let normalized = normalize_keystroke(keystrokes);
    if normalized
        .split_whitespace()
        .any(|chord| parse_keystroke(chord).is_err())
    {
        log::warn!("shortcuts: invalid keystroke '{keystrokes}'");
        return None;
    }
    let predicate = match context {
        Some(ctx) => match KeyBindingContextPredicate::parse(ctx) {
            Ok(p) => Some(p.into()),
            Err(e) => {
                log::warn!("shortcuts: invalid context predicate '{ctx}': {e}");
                return None;
            }
        },
        None => None,
    };
    match KeyBinding::load(&normalized, action, predicate, false, None, cx) {
        Ok(binding) => Some(binding),
        Err(e) => {
            log::warn!("shortcuts: invalid keystroke '{keystrokes}': {e}");
            None
        }
    }
}

pub fn apply_keybindings(cx: &mut App, user_shortcuts: &HashMap<String, String>) {
    cx.clear_key_bindings();

    let unbound_canonical: std::collections::HashSet<Keystroke> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() == "none")
        .filter_map(|(k, _)| canonical_keystroke(k))
        .collect();

    let remapped_actions: std::collections::HashSet<&str> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() != "none")
        .filter(|(k, _)| canonical_keystroke(k).is_some())
        .filter_map(|(_, action_name)| {
            if action_from_name(action_name).is_some() {
                Some(action_name.as_str())
            } else {
                None
            }
        })
        .collect();

    let user_bound_canonical: std::collections::HashSet<Keystroke> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() != "none")
        .filter(|(_, action_name)| action_from_name(action_name).is_some())
        .filter_map(|(k, _)| canonical_keystroke(k))
        .collect();

    let is_unbound =
        |key: &str| canonical_keystroke(key).is_some_and(|k| unbound_canonical.contains(&k));
    let is_user_claimed =
        |key: &str| canonical_keystroke(key).is_some_and(|k| user_bound_canonical.contains(&k));

    let default_bindings: Vec<KeyBinding> = DEFAULTS
        .iter()
        .chain(PLATFORM_DEFAULTS.iter())
        .filter(|d| !is_unbound(d.key))
        .filter(|d| !remapped_actions.contains(d.action_name))
        .filter(|d| !is_user_claimed(d.key))
        .filter_map(|d| {
            let action = action_from_name(d.action_name)?;
            make_binding(d.key, action, d.context, cx)
        })
        .collect();
    cx.bind_keys(default_bindings);

    for (key, action_name) in user_shortcuts {
        if action_name == "none" {
            continue;
        }
        let Some(action) = action_from_name(action_name) else {
            log::warn!("shortcuts: unknown action '{action_name}' for key '{key}', skipping");
            continue;
        };
        let context = context_for_action(action_name);
        if let Some(binding) = make_binding(key, action, context, cx) {
            cx.bind_keys([binding]);
        }
    }

    crate::widgets::text_input::register_keybindings(cx);
    crate::widgets::text_area::register_keybindings(cx);
    crate::app::diff_dock::code::view::register_keybindings(cx);
}

pub fn keyboard_layout_changed(last_layout_id: &mut String, cx: &App) -> bool {
    let layout_id = cx.keyboard_layout().id();
    if layout_id == last_layout_id {
        return false;
    }
    *last_layout_id = layout_id.to_string();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SplitHorizontally;

    #[test]
    fn normalize_keystroke_converts_plus_to_dash() {
        assert_eq!(normalize_keystroke("ctrl+shift+d"), "ctrl-shift-d");
        assert_eq!(normalize_keystroke("alt+left"), "alt-left");
    }

    #[test]
    fn normalize_keystroke_already_dashed_unchanged() {
        assert_eq!(normalize_keystroke("ctrl-shift-d"), "ctrl-shift-d");
    }

    #[test]
    fn keystrokes_conflict_ignores_separator_and_order() {
        assert!(keystrokes_conflict("ctrl+shift+f", "ctrl-shift-f"));
        assert!(keystrokes_conflict("shift-ctrl-f", "ctrl-shift-f"));
        assert!(!keystrokes_conflict("ctrl-shift-f", "ctrl-shift-g"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keystrokes_conflict_resolves_secondary_on_linux() {
        assert!(keystrokes_conflict("secondary-shift-d", "ctrl-shift-d"));
        assert!(!keystrokes_conflict("secondary-shift-d", "alt-shift-d"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keystrokes_conflict_resolves_secondary_on_macos() {
        assert!(keystrokes_conflict("secondary-shift-d", "cmd-shift-d"));
        assert!(!keystrokes_conflict("secondary-shift-d", "ctrl-shift-d"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn host_os_is_mac_on_macos() {
        assert_eq!(host_os(), OperatingSystem::Mac);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn host_os_is_windows_on_windows() {
        assert_eq!(host_os(), OperatingSystem::Windows);
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[test]
    fn host_os_is_linux_on_other_builds() {
        assert_eq!(host_os(), OperatingSystem::Linux);
    }

    #[gpui::test]
    fn host_os_agrees_with_the_gpui_platform(cx: &mut gpui::TestAppContext) {
        assert_eq!(cx.update(|cx| cx.platform_info().os), host_os());
    }

    #[gpui::test]
    fn keyboard_layout_changed_fires_once_per_new_layout(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let mut last_layout_id = String::from("layout-before-the-switch");
            assert!(keyboard_layout_changed(&mut last_layout_id, cx));
            assert_eq!(last_layout_id, cx.keyboard_layout().id());
            assert!(!keyboard_layout_changed(&mut last_layout_id, cx));
        });
    }

    #[test]
    fn parse_keystroke_resolves_secondary_for_the_host() {
        let expected = if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        };
        assert_eq!(
            parse_keystroke("secondary-c"),
            Ok(Keystroke::parse_with_os(expected, host_os()).expect("a valid chord"))
        );
    }

    #[test]
    fn parse_keystroke_rejects_a_chord_without_a_key() {
        for source in ["ctrl-", "", "secondary-"] {
            assert_eq!(
                parse_keystroke(source),
                Err(format!("invalid keystroke: {source}"))
            );
        }
    }

    #[gpui::test]
    fn secondary_binding_parses_successfully(cx: &mut gpui::TestAppContext) {
        let binding = cx
            .update(|cx| make_binding("secondary-shift-d", Box::new(SplitHorizontally), None, cx));
        assert!(
            binding.is_some(),
            "secondary-shift-d must parse into a valid KeyBinding"
        );
    }

    #[gpui::test]
    fn cmd_override_parses_on_any_platform(cx: &mut gpui::TestAppContext) {
        let binding =
            cx.update(|cx| make_binding("cmd-shift-d", Box::new(SplitHorizontally), None, cx));
        assert!(
            binding.is_some(),
            "cmd-shift-d override must parse on any platform"
        );
    }

    #[gpui::test]
    fn an_invalid_user_chord_is_skipped_and_every_other_binding_applies(
        cx: &mut gpui::TestAppContext,
    ) {
        let shortcuts = HashMap::from([
            ("ctrl-".to_string(), "close_pane".to_string()),
            (
                "ctrl-alt-shift-y".to_string(),
                "split_horizontally".to_string(),
            ),
        ]);
        let valid = parse_keystroke("ctrl-alt-shift-y").expect("a valid chord");
        let default_chord = DEFAULTS
            .iter()
            .chain(PLATFORM_DEFAULTS.iter())
            .find(|d| d.action_name == "close_pane")
            .expect("a close_pane default");
        let default_keystroke = canonical_keystroke(default_chord.key).expect("a valid default");

        let (invalid_bound, valid_bound, default_bound, invalid_binding) = cx.update(|cx| {
            apply_keybindings(cx, &shortcuts);
            (
                cx.all_bindings_for_input(&[Keystroke::parse_with_os("ctrl-", host_os())
                    .expect("gpui accepts an empty key")])
                    .len(),
                cx.all_bindings_for_input(std::slice::from_ref(&valid)),
                cx.all_bindings_for_input(std::slice::from_ref(&default_keystroke))
                    .len(),
                make_binding("ctrl-", Box::new(SplitHorizontally), None, cx).is_none(),
            )
        });

        assert_eq!(invalid_bound, 0, "the invalid chord must not be bound");
        assert!(
            invalid_binding,
            "make_binding must refuse the invalid chord"
        );
        assert!(
            valid_bound
                .iter()
                .any(|binding| binding.action().partial_eq(&SplitHorizontally)),
            "the valid user chord must still apply"
        );
        assert!(
            default_bound > 0,
            "the close_pane default ({}) must survive an invalid user chord for close_pane",
            default_chord.key
        );
    }

    #[gpui::test]
    fn widget_keymaps_register_and_bind_secondary_commands_per_platform(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::app::diff_dock::code::view::{
            CeCopy, CeCut, CePaste, CeRedo, CeSave, CeSelectAll, CeUndo,
        };
        use crate::widgets::text_area::TaSubmitImmediate;

        let native = if cfg!(target_os = "macos") {
            "cmd"
        } else {
            "ctrl"
        };
        let expected: [(&dyn Action, String); 8] = [
            (&CeSelectAll, format!("{native}-a")),
            (&CeUndo, format!("{native}-z")),
            (&CeRedo, format!("{native}-shift-z")),
            (&CeCopy, format!("{native}-c")),
            (&CeCut, format!("{native}-x")),
            (&CePaste, format!("{native}-v")),
            (&CeSave, format!("{native}-s")),
            (&TaSubmitImmediate, format!("{native}-enter")),
        ];

        cx.update(|cx| {
            apply_keybindings(cx, &HashMap::new());
            let keymap = cx.key_bindings();
            let keymap = keymap.borrow();
            for (action, chord) in &expected {
                let want = parse_keystroke(chord).expect("a valid chord");
                let bound: Vec<Keystroke> = keymap
                    .bindings_for_action(*action)
                    .filter_map(|binding| binding.keystrokes().first())
                    .map(|keystroke| keystroke.inner().clone())
                    .collect();
                assert!(
                    bound.contains(&want),
                    "{} must be bound to {chord}, got {bound:?}",
                    action.name()
                );
            }

            if !cfg!(target_os = "macos") {
                let super_c = parse_keystroke("cmd-c").expect("a valid chord");
                let bound = cx.all_bindings_for_input(std::slice::from_ref(&super_c));
                assert!(
                    bound.is_empty(),
                    "Super+C must stay unbound off macOS, got {:?}",
                    bound
                        .iter()
                        .map(|binding| binding.action().name())
                        .collect::<Vec<_>>()
                );
            }
        });
    }

    #[gpui::test]
    fn tab_cycling_defaults_are_bindable_and_do_not_collide(cx: &mut gpui::TestAppContext) {
        use super::super::defaults::DEFAULTS;

        for (key, action_name) in [("secondary-]", "next_tab"), ("secondary-[", "previous_tab")] {
            let action = action_from_name(action_name).expect("registered action");
            assert!(
                cx.update(|cx| make_binding(key, action, None, cx))
                    .is_some(),
                "{key} must parse into a valid KeyBinding"
            );
            let claimants: Vec<&str> = DEFAULTS
                .iter()
                .filter(|d| keystrokes_conflict(d.key, key))
                .map(|d| d.action_name)
                .collect();
            assert_eq!(
                claimants,
                vec![action_name],
                "{key} must be claimed by exactly one default"
            );
        }

        assert!(
            DEFAULTS
                .iter()
                .any(|d| d.key == "ctrl-tab" && d.action_name == "next_workspace"),
            "the tab shortcuts must not steal ctrl-tab from next_workspace"
        );
    }

    #[gpui::test]
    fn user_override_of_a_tab_shortcut_wins_over_the_default(cx: &mut gpui::TestAppContext) {
        use super::super::defaults::DEFAULTS;

        let user_key = "secondary+]";
        let user_claimed = canonical_keystroke(user_key).expect("a parsable user chord");
        assert!(
            DEFAULTS.iter().any(|d| d.action_name == "next_tab"
                && canonical_keystroke(d.key).is_some_and(|k| k == user_claimed)),
            "the default next_tab must own the chord the user claims"
        );
        let shortcuts = HashMap::from([(user_key.to_string(), "split_horizontally".to_string())]);

        let bound = cx.update(|cx| {
            apply_keybindings(cx, &shortcuts);
            cx.all_bindings_for_input(std::slice::from_ref(&user_claimed))
        });

        assert!(!bound.is_empty(), "the user chord must stay bound");
        assert!(
            bound
                .iter()
                .all(|binding| binding.action().partial_eq(&SplitHorizontally)),
            "the default next_tab binding must not survive the user override: {:?}",
            bound
                .iter()
                .map(|binding| binding.action().name())
                .collect::<Vec<_>>()
        );
    }

    #[gpui::test]
    fn diff_dock_tab_chords_are_bindable_and_do_not_collide(cx: &mut gpui::TestAppContext) {
        use super::super::defaults::DEFAULTS;

        for (key, action_name) in [
            ("secondary-g", "diff_new_file_tab"),
            ("secondary-j", "diff_new_terminal_tab"),
        ] {
            let context = context_for_action(action_name);
            let action = action_from_name(action_name).expect("registered action");
            assert!(
                cx.update(|cx| make_binding(key, action, context, cx))
                    .is_some(),
                "{key} must parse into a valid KeyBinding with context {context:?}"
            );

            let claimants: Vec<&str> = DEFAULTS
                .iter()
                .chain(PLATFORM_DEFAULTS.iter())
                .filter(|d| keystrokes_conflict(d.key, key))
                .map(|d| d.action_name)
                .collect();
            assert_eq!(
                claimants,
                vec![action_name],
                "{key} must be claimed by exactly one default on this platform"
            );

            let context = context.expect("the dock chords must be context-scoped");
            for excluded in [
                "Terminal",
                "TextInput",
                "PaneflowTextArea",
                crate::app::diff_dock::code::view::CODE_KEY_CONTEXT,
            ] {
                assert!(
                    context.contains(&format!("!{excluded}")),
                    "{key} must be scoped away from {excluded}, got `{context}`"
                );
            }
        }
    }

    #[test]
    fn terminal_search_chord_is_claimed_only_by_toggle_search() {
        use super::super::defaults::DEFAULTS;

        let claimants: Vec<&str> = DEFAULTS
            .iter()
            .chain(PLATFORM_DEFAULTS.iter())
            .filter(|d| keystrokes_conflict(d.key, "ctrl-shift-f"))
            .map(|d| d.action_name)
            .collect();
        assert_eq!(claimants, vec!["toggle_search"]);
    }

    #[test]
    fn global_defaults_never_share_a_chord_with_another_default() {
        use super::super::defaults::DEFAULTS;

        let all: Vec<_> = DEFAULTS.iter().chain(PLATFORM_DEFAULTS.iter()).collect();
        let shadowing: Vec<String> = all
            .iter()
            .filter(|d| d.context.is_none())
            .flat_map(|global| {
                all.iter()
                    .filter(move |other| {
                        other.action_name != global.action_name
                            && keystrokes_conflict(other.key, global.key)
                    })
                    .map(move |other| {
                        format!(
                            "{} ({}) shadows {} ({}) on {}",
                            global.action_name,
                            global.key,
                            other.action_name,
                            other.key,
                            std::env::consts::OS
                        )
                    })
            })
            .collect();
        assert!(shadowing.is_empty(), "{}", shadowing.join("\n"));
    }

    #[cfg(target_os = "macos")]
    #[gpui::test]
    fn us010_cmd_c_parses_as_binding(cx: &mut gpui::TestAppContext) {
        use crate::TerminalCopy;
        let binding =
            cx.update(|cx| make_binding("cmd-c", Box::new(TerminalCopy), Some("Terminal"), cx));
        assert!(binding.is_some(), "cmd-c must parse as a valid KeyBinding");
    }
}
