//! Register defaults and layer user overrides onto GPUI's keybinding registry.

use std::collections::HashMap;

use gpui::{Action, App, DummyKeyboardMapper, KeyBinding, KeyBindingContextPredicate, Keystroke};

use super::defaults::{DEFAULTS, MACOS_ONLY_DEFAULTS};
use super::registry::{action_from_name, context_for_action};

/// Normalize a user-friendly keystroke string to GPUI format.
///
/// Users may write `"ctrl+shift+d"` (plus separators) in `paneflow.json`,
/// but GPUI expects `"ctrl-shift-d"` (dash separators).
pub(super) fn normalize_keystroke(keystrokes: &str) -> String {
    keystrokes.replace('+', "-")
}

/// Canonical form of a keystroke string for *physical chord* comparison.
///
/// US-021: parsing through GPUI resolves `+`/`-` separators, modifier order,
/// and the `secondary` platform shorthand (→ `cmd` on macOS, `ctrl`
/// elsewhere) into the same `Keystroke` value, so `"ctrl+shift+d"`,
/// `"shift-ctrl-d"`, and `"secondary-shift-d"` all compare equal on Linux.
/// Returns `None` for unparseable input (which then only matches by raw
/// equality at the call site).
pub(super) fn canonical_keystroke(keystrokes: &str) -> Option<Keystroke> {
    Keystroke::parse(&normalize_keystroke(keystrokes)).ok()
}

/// True if two keystroke strings denote the same physical chord, normalization
/// applied (see [`canonical`]). Unparseable strings only match by exact
/// equality. Used by the settings writer to collapse a rebind onto a key that
/// is already taken instead of leaving two live entries (GPUI would resolve
/// the conflict order-dependently).
pub fn keystrokes_conflict(a: &str, b: &str) -> bool {
    match (canonical_keystroke(a), canonical_keystroke(b)) {
        (Some(ka), Some(kb)) => ka == kb,
        _ => a == b,
    }
}

/// Build a `KeyBinding` from a boxed action, using `KeyBinding::load` to avoid
/// the `A: Action` bound on `KeyBinding::new`. Returns `None` on invalid keystroke.
pub(super) fn make_binding(
    keystrokes: &str,
    action: Box<dyn Action>,
    context: Option<&str>,
) -> Option<KeyBinding> {
    let normalized = normalize_keystroke(keystrokes);
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
    match KeyBinding::load(
        &normalized,
        action,
        predicate,
        false,
        None,
        &DummyKeyboardMapper,
    ) {
        Ok(binding) => Some(binding),
        Err(e) => {
            log::warn!("shortcuts: invalid keystroke '{keystrokes}': {e}");
            None
        }
    }
}

/// Apply keybindings: clear all, register defaults, then layer user overrides.
///
/// User shortcuts map keystroke strings to action names. Special values:
/// - `"none"` - unbinds the key (no action registered for it)
/// - Any valid action name - overrides or adds a binding for that key
pub fn apply_keybindings(cx: &mut App, user_shortcuts: &HashMap<String, String>) {
    cx.clear_key_bindings();

    // Keys the user explicitly unbound via "none". US-021: canonicalized so
    // that an unbind written as "ctrl+shift+d" or "secondary-shift-d" actually
    // suppresses the matching default (whose key string uses the `secondary`
    // shorthand), instead of failing the raw `==` comparison and leaving the
    // default live.
    let unbound_canonical: std::collections::HashSet<Keystroke> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() == "none")
        .filter_map(|(k, _)| canonical_keystroke(k))
        .collect();

    // Actions the user remapped to a different key (drop their default key).
    let remapped_actions: std::collections::HashSet<&str> = user_shortcuts
        .iter()
        .filter(|(_, v)| v.as_str() != "none")
        .filter_map(|(_, action_name)| {
            if action_from_name(action_name).is_some() {
                Some(action_name.as_str())
            } else {
                None
            }
        })
        .collect();

    // Keys the user bound to some real action. US-021: a default that shares
    // one of these keys (for a *different* action) would otherwise stay active
    // alongside the override → GPUI-ambiguous double binding (the root cause at
    // the old `apply.rs:86`, e.g. a default `ctrl-shift-f → toggle_search`
    // surviving next to a user `ctrl-shift-f → close_pane`). Drop it: a chord
    // belongs to exactly one action, last writer wins.
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

    // Register defaults, skipping unbound keys, remapped actions, and keys the
    // user reassigned to another action.
    // US-010: chain macOS-only defaults (cmd-c/cmd-v in Terminal context).
    let default_bindings: Vec<KeyBinding> = DEFAULTS
        .iter()
        .chain(MACOS_ONLY_DEFAULTS.iter())
        .filter(|d| !is_unbound(d.key))
        .filter(|d| !remapped_actions.contains(d.action_name))
        .filter(|d| !is_user_claimed(d.key))
        .filter_map(|d| {
            let action = action_from_name(d.action_name)?;
            make_binding(d.key, action, d.context)
        })
        .collect();
    cx.bind_keys(default_bindings);

    // Wire copy actions for the markdown widget (Zed crate): selecting
    // text inside a rendered chat message + pressing the platform copy
    // shortcut now writes it to the clipboard. Mirrors Zed's default
    // bindings at `zed/assets/keymaps/default-{macos,linux,windows}.json`.
    // `secondary` resolves to Cmd on macOS and Ctrl elsewhere.
    cx.bind_keys([
        KeyBinding::new("secondary-c", markdown::Copy, Some("Markdown")),
        KeyBinding::new(
            "secondary-shift-c",
            markdown::CopyAsMarkdown,
            Some("Markdown"),
        ),
    ]);

    // Layer user overrides
    for (key, action_name) in user_shortcuts {
        if action_name == "none" {
            continue;
        }
        let Some(action) = action_from_name(action_name) else {
            log::warn!("shortcuts: unknown action '{action_name}' for key '{key}', skipping");
            continue;
        };
        let context = context_for_action(action_name);
        if let Some(binding) = make_binding(key, action, context) {
            cx.bind_keys([binding]);
        }
    }

    // `cx.clear_key_bindings()` at the top wiped EVERY binding, including the
    // global `TextInput` / `TextArea` widget bindings (caret movement, Home/End,
    // selection, Backspace/Delete, clipboard) that are registered once at
    // startup. Re-register them on every apply so text fields keep working after
    // a shortcut rebind, config reload, settings navigation, or IPC-driven
    // re-apply - otherwise a re-apply silently degrades every input to IME-only
    // typing (the field accepts characters but ignores arrows, selection, and
    // clipboard).
    crate::widgets::text_input::register_keybindings(cx);
    crate::widgets::text_area::register_keybindings(cx);
    // Same reasoning for the code editor's navigation bindings (EP-003
    // US-011): they are scoped to the `CodeEditor` key context, so they never
    // shadow a global shortcut, but the clear above takes them out with
    // everything else.
    crate::app::diff_dock::code::view::register_keybindings(cx);
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
        // US-021: `+`/`-` separators and modifier order are normalized away.
        assert!(keystrokes_conflict("ctrl+shift+f", "ctrl-shift-f"));
        assert!(keystrokes_conflict("shift-ctrl-f", "ctrl-shift-f"));
        assert!(!keystrokes_conflict("ctrl-shift-f", "ctrl-shift-g"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keystrokes_conflict_resolves_secondary_on_linux() {
        // `secondary` resolves to ctrl on Linux, so a default written with the
        // shorthand collides with a concrete ctrl chord.
        assert!(keystrokes_conflict("secondary-shift-d", "ctrl-shift-d"));
        assert!(!keystrokes_conflict("secondary-shift-d", "alt-shift-d"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keystrokes_conflict_resolves_secondary_on_macos() {
        // `secondary` resolves to cmd (platform) on macOS.
        assert!(keystrokes_conflict("secondary-shift-d", "cmd-shift-d"));
        assert!(!keystrokes_conflict("secondary-shift-d", "ctrl-shift-d"));
    }

    #[test]
    fn secondary_binding_parses_successfully() {
        // AC2/AC3: make_binding accepts the `secondary` prefix on both
        // platforms. GPUI's Keystroke::parse resolves it internally.
        let binding = make_binding("secondary-shift-d", Box::new(SplitHorizontally), None);
        assert!(
            binding.is_some(),
            "secondary-shift-d must parse into a valid KeyBinding"
        );
    }

    #[test]
    fn cmd_override_parses_on_any_platform() {
        // AC5: a user writing `"split_horizontally": "cmd-shift-d"` in
        // paneflow.json must produce a valid binding on Linux as well as
        // macOS (GPUI accepts `cmd` as a synonym for the platform modifier).
        let binding = make_binding("cmd-shift-d", Box::new(SplitHorizontally), None);
        assert!(
            binding.is_some(),
            "cmd-shift-d override must parse on any platform"
        );
    }

    /// US-020 (prd-cli-tab-hierarchy): the two tab-cycling defaults parse,
    /// claim free chords, and leave `secondary-tab` (next *workspace*) alone.
    #[test]
    fn tab_cycling_defaults_are_bindable_and_do_not_collide() {
        use super::super::defaults::DEFAULTS;

        for (key, action_name) in [("secondary-]", "next_tab"), ("secondary-[", "previous_tab")] {
            let action = action_from_name(action_name).expect("registered action");
            assert!(
                make_binding(key, action, None).is_some(),
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

        // `secondary-tab` keeps meaning "next workspace".
        assert!(
            DEFAULTS
                .iter()
                .any(|d| d.key == "secondary-tab" && d.action_name == "next_workspace"),
            "the tab shortcuts must not steal secondary-tab from next_workspace"
        );
    }

    /// US-020: a user who already bound `secondary-]` to something else keeps
    /// it. `apply_keybindings` drops the default sharing a user-claimed chord
    /// before registering it, so no ambiguous double binding - and no
    /// error-level conflict - is produced.
    #[test]
    fn user_override_of_a_tab_shortcut_wins_over_the_default() {
        use super::super::defaults::DEFAULTS;

        let user_key = "ctrl+]";
        let user_claimed = canonical_keystroke(user_key).expect("a parsable user chord");
        let dropped: Vec<&str> = DEFAULTS
            .iter()
            .filter(|d| canonical_keystroke(d.key).is_some_and(|k| k == user_claimed))
            .map(|d| d.action_name)
            .collect();
        // On Linux/Windows `secondary` is ctrl, so the default is dropped; on
        // macOS `secondary` is cmd and the two chords simply never collide.
        #[cfg(not(target_os = "macos"))]
        assert_eq!(dropped, vec!["next_tab"]);
        #[cfg(target_os = "macos")]
        assert!(dropped.is_empty());

        // Either way the user's own binding is registrable.
        assert!(
            make_binding(user_key, Box::new(SplitHorizontally), None).is_some(),
            "the user override must produce a valid binding"
        );
    }

    /// EP-005 US-018 (prd-file-editor-2026-Q3): the two diff-dock chords are
    /// bindable, claimed by exactly one default each, and free of any conflict
    /// with the rest of the action table. Written with `secondary-`, so the
    /// same assertion covers Ctrl on Linux/Windows and Cmd on macOS.
    #[test]
    fn diff_dock_tab_chords_are_bindable_and_do_not_collide() {
        use super::super::defaults::DEFAULTS;

        for (key, action_name) in [
            ("secondary-g", "diff_new_file_tab"),
            ("secondary-j", "diff_new_terminal_tab"),
        ] {
            let context = context_for_action(action_name);
            let action = action_from_name(action_name).expect("registered action");
            assert!(
                make_binding(key, action, context).is_some(),
                "{key} must parse into a valid KeyBinding with context {context:?}"
            );

            let claimants: Vec<&str> = DEFAULTS
                .iter()
                .chain(MACOS_ONLY_DEFAULTS.iter())
                .filter(|d| keystrokes_conflict(d.key, key))
                .map(|d| d.action_name)
                .collect();
            assert_eq!(
                claimants,
                vec![action_name],
                "{key} must be claimed by exactly one default on this platform"
            );

            // The chord must not reach a shell: Ctrl+G is BEL and Ctrl+J is LF,
            // and both stay the terminal's on every platform. Same for the text
            // widgets, where they are ordinary input, and for the code editor
            // the file chord itself opens.
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

    #[cfg(target_os = "macos")]
    #[test]
    fn us010_cmd_c_parses_as_binding() {
        use crate::TerminalCopy;
        let binding = make_binding("cmd-c", Box::new(TerminalCopy), Some("Terminal"));
        assert!(binding.is_some(), "cmd-c must parse as a valid KeyBinding");
    }
}
