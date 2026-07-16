//! Default keybinding tables (cross-platform + macOS-only layer).

/// A default keybinding entry: keystroke string, action name, GPUI context filter.
pub(super) struct DefaultBinding {
    pub(super) key: &'static str,
    pub(super) action_name: &'static str,
    pub(super) context: Option<&'static str>,
}

/// All default keybindings. Order matches the original registration order.
pub(super) const DEFAULTS: &[DefaultBinding] = &[
    // US-009: app-global split/workspace bindings use the `secondary`
    // modifier so GPUI resolves to `cmd` on macOS and `ctrl` on Linux/Windows.
    // `secondary` keeps Linux on Ctrl+Shift+… (no user-visible regression)
    // while giving macOS users the expected Cmd+Shift+… shortcuts.
    DefaultBinding {
        key: "secondary-shift-d",
        action_name: "split_horizontally",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-e",
        action_name: "split_vertically",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-w",
        action_name: "close_pane",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-n",
        action_name: "new_workspace",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-q",
        action_name: "close_workspace",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-shift-alt-c",
        action_name: "copy_workspace_path",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-alt-r",
        action_name: "reveal_workspace_in_file_manager",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-alt-z",
        action_name: "open_workspace_in_zed",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-alt-c",
        action_name: "open_workspace_in_cursor",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-alt-v",
        action_name: "open_workspace_in_vscode",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-alt-w",
        action_name: "open_workspace_in_windsurf",
        context: None,
    },
    DefaultBinding {
        key: "secondary-tab",
        action_name: "next_workspace",
        context: None,
    },
    DefaultBinding {
        key: "alt-left",
        action_name: "focus_left",
        context: None,
    },
    DefaultBinding {
        key: "alt-right",
        action_name: "focus_right",
        context: None,
    },
    DefaultBinding {
        key: "alt-up",
        action_name: "focus_up",
        context: None,
    },
    DefaultBinding {
        key: "alt-down",
        action_name: "focus_down",
        context: None,
    },
    // US-019 (orchestration-v2): cycle to the next pane whose agent waits
    // for input, cross-workspace. `secondary-shift-j` is unclaimed (the
    // taken set: d/e/w/n/q/t/z/=/s/a/g).
    DefaultBinding {
        key: "secondary-shift-j",
        action_name: "jump_next_waiting",
        context: None,
    },
    DefaultBinding {
        key: "secondary-1",
        action_name: "select_workspace_1",
        context: None,
    },
    DefaultBinding {
        key: "secondary-2",
        action_name: "select_workspace_2",
        context: None,
    },
    DefaultBinding {
        key: "secondary-3",
        action_name: "select_workspace_3",
        context: None,
    },
    DefaultBinding {
        key: "secondary-4",
        action_name: "select_workspace_4",
        context: None,
    },
    DefaultBinding {
        key: "secondary-5",
        action_name: "select_workspace_5",
        context: None,
    },
    DefaultBinding {
        key: "secondary-6",
        action_name: "select_workspace_6",
        context: None,
    },
    DefaultBinding {
        key: "secondary-7",
        action_name: "select_workspace_7",
        context: None,
    },
    DefaultBinding {
        key: "secondary-8",
        action_name: "select_workspace_8",
        context: None,
    },
    DefaultBinding {
        key: "secondary-9",
        action_name: "select_workspace_9",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-t",
        action_name: "undo_close_pane",
        context: None,
    },
    DefaultBinding {
        key: "secondary-alt-t",
        action_name: "new_tab",
        context: None,
    },
    DefaultBinding {
        key: "secondary-w",
        action_name: "close_tab",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-shift-c",
        action_name: "terminal_copy",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "ctrl-shift-v",
        action_name: "terminal_paste",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "shift-pageup",
        action_name: "scroll_page_up",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "shift-pagedown",
        action_name: "scroll_page_down",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "secondary-shift-up",
        action_name: "jump_prev_prompt",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "secondary-shift-down",
        action_name: "jump_next_prompt",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "secondary-shift-z",
        action_name: "toggle_zoom",
        context: None,
    },
    DefaultBinding {
        key: "secondary-alt-1",
        action_name: "layout_even_horizontal",
        context: None,
    },
    DefaultBinding {
        key: "secondary-alt-2",
        action_name: "layout_even_vertical",
        context: None,
    },
    DefaultBinding {
        key: "secondary-alt-3",
        action_name: "layout_main_vertical",
        context: None,
    },
    DefaultBinding {
        key: "secondary-alt-4",
        action_name: "layout_tiled",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-=",
        action_name: "split_equalize",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-s",
        action_name: "swap_pane",
        context: None,
    },
    DefaultBinding {
        key: "ctrl-shift-x",
        action_name: "toggle_copy_mode",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "ctrl-shift-f",
        action_name: "toggle_search",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "enter",
        action_name: "search_next",
        context: Some("Search"),
    },
    DefaultBinding {
        key: "shift-enter",
        action_name: "search_prev",
        context: Some("Search"),
    },
    DefaultBinding {
        key: "escape",
        action_name: "dismiss_search",
        context: Some("Search"),
    },
    DefaultBinding {
        key: "alt-r",
        action_name: "toggle_search_regex",
        context: Some("Search"),
    },
    // EP-006 US-018 - fan the open search out to every pane (fleet grep).
    // Search context only, so no terminal chord is shadowed.
    DefaultBinding {
        key: "alt-f",
        action_name: "toggle_fleet_search",
        context: Some("Search"),
    },
    // EP-006 US-019 - per-pane font zoom. These DO shadow readline's
    // C-- (undo) / C-0 (digit-argument) in the focused terminal: the
    // PRD's documented, remappable exception (Hard Constraint clavier),
    // matching the zoom convention of gnome-terminal/Ghostty on Linux.
    DefaultBinding {
        key: "secondary-=",
        action_name: "font_size_increase",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "secondary--",
        action_name: "font_size_decrease",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "secondary-0",
        action_name: "font_size_reset",
        context: Some("Terminal"),
    },
    // US-022 - markdown pane navigation. Same chord vocabulary as the
    // terminal pane so muscle memory transfers cleanly between pane types.
    DefaultBinding {
        key: "shift-pageup",
        action_name: "markdown_scroll_page_up",
        context: Some("Markdown"),
    },
    DefaultBinding {
        key: "shift-pagedown",
        action_name: "markdown_scroll_page_down",
        context: Some("Markdown"),
    },
    DefaultBinding {
        key: "ctrl-f",
        action_name: "markdown_find_open",
        context: Some("Markdown"),
    },
    DefaultBinding {
        key: "ctrl-shift-c",
        action_name: "markdown_copy",
        context: Some("Markdown"),
    },
    DefaultBinding {
        key: "enter",
        action_name: "markdown_find_next",
        context: Some("MarkdownSearch"),
    },
    DefaultBinding {
        key: "shift-enter",
        action_name: "markdown_find_prev",
        context: Some("MarkdownSearch"),
    },
    DefaultBinding {
        key: "escape",
        action_name: "markdown_find_dismiss",
        context: Some("MarkdownSearch"),
    },
    // US-005 (prd-agents-view.md): `secondary-shift-a` is Ctrl+Shift+A
    // on Linux/Windows and Cmd+Shift+A on macOS. US-008 will reuse the
    // same binding when it lands the full AppMode toggle.
    DefaultBinding {
        key: "secondary-shift-a",
        action_name: "open_agents_view",
        context: None,
    },
    // US-003 (prd-git-diff-mode-2026-Q3.md): `secondary-shift-g` is
    // Ctrl+Shift+G on Linux/Windows and Cmd+Shift+G on macOS. Toggles
    // the dedicated Git Diff mode (AppMode::Diff).
    DefaultBinding {
        key: "secondary-shift-g",
        action_name: "open_diff_view",
        context: None,
    },
    // Files right-sidebar toggle. Uses `secondary-alt-f` instead of
    // `secondary-shift-f` so it never shadows the terminal search chord
    // (`ctrl-shift-f` on Linux/Windows).
    DefaultBinding {
        key: "secondary-alt-f",
        action_name: "toggle_files_sidebar",
        context: None,
    },
    // US-003 (prd-ai-in-diff-2026-Q3.md): copy the hunk under the cursor as a
    // unified diff, only while the Git Diff view holds focus. Same chord as the
    // terminal / markdown copies - disambiguated by the `DiffView` context.
    DefaultBinding {
        key: "ctrl-shift-c",
        action_name: "copy_diff_hunk",
        context: Some("DiffView"),
    },
    // EP-003 US-009 (review redesign): keyboard-first review loop.
    // Bare keys, scoped away from terminals and text widgets so focus children
    // of the DiffView do not lose a keystroke.
    DefaultBinding {
        key: "]",
        action_name: "diff_next_hunk",
        context: Some("DiffView && !Terminal && !TextInput && !PaneflowTextArea"),
    },
    DefaultBinding {
        key: "[",
        action_name: "diff_prev_hunk",
        context: Some("DiffView && !Terminal && !TextInput && !PaneflowTextArea"),
    },
    DefaultBinding {
        key: "u",
        action_name: "diff_toggle_view",
        context: Some("DiffView && !Terminal && !TextInput && !PaneflowTextArea"),
    },
    DefaultBinding {
        key: "s",
        action_name: "diff_toggle_sync",
        context: Some("DiffView && !Terminal && !TextInput && !PaneflowTextArea"),
    },
    DefaultBinding {
        key: "escape",
        action_name: "diff_dismiss",
        context: Some("DiffView && !Terminal && !TextInput && !PaneflowTextArea"),
    },
    // EP-001 (CLI Cockpit): Composer + broadcast
    // groups. All three are unclaimed `secondary-shift-…` slots (taken set
    // before this block: d/e/w/n/q/j/t/z/=/s/a/g) and none shadows a common
    // shell/readline/TUI chord (FR-12) - Ctrl+Shift+Space/B/M mean nothing to
    // readline, vim or nano. Remappable like every entry in this table.
    DefaultBinding {
        key: "secondary-shift-space",
        action_name: "open_composer",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-b",
        action_name: "toggle_broadcast_member",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-m",
        action_name: "open_broadcast_groups",
        context: None,
    },
    // EP-002 (cli-cockpit): Attention Queue + Launch Pad. `secondary-shift-k`
    // and `secondary-shift-l` are unclaimed (taken set before this block:
    // d/e/w/n/q/j/t/z/=/s/a/g/space/b/m) and shadow no shell/readline/TUI
    // chord (FR-12 - Ctrl+K kill-line is BARE ctrl, not ctrl+shift).
    DefaultBinding {
        key: "secondary-shift-k",
        action_name: "open_attention_queue",
        context: None,
    },
    DefaultBinding {
        key: "secondary-shift-l",
        action_name: "open_launch_pad",
        context: None,
    },
];

/// Platform-specific default bindings layered on top of [`DEFAULTS`].
///
/// US-010 binds `cmd-c` / `cmd-v` to terminal copy/paste on macOS so muscle
/// memory from iTerm2 / Terminal.app / WezTerm works. Kept empty on Linux
/// (AC5) because Linux keyboards don't have a `cmd` key by default. The
/// existing `ctrl-shift-c/v` Terminal bindings stay intact on both platforms -
/// these are purely additive.
#[cfg(target_os = "macos")]
pub(super) const MACOS_ONLY_DEFAULTS: &[DefaultBinding] = &[
    DefaultBinding {
        key: "cmd-c",
        action_name: "terminal_copy",
        context: Some("Terminal"),
    },
    DefaultBinding {
        key: "cmd-v",
        action_name: "terminal_paste",
        context: Some("Terminal"),
    },
    // US-012: Cmd+Q quits the app and populates the "⌘Q" shortcut next to
    // the Quit PaneFlow menu item. Global context so the menu picks it up
    // whether or not a terminal pane holds focus.
    DefaultBinding {
        key: "cmd-q",
        action_name: "quit",
        context: None,
    },
];

#[cfg(not(target_os = "macos"))]
pub(super) const MACOS_ONLY_DEFAULTS: &[DefaultBinding] = &[];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bindings_cover_all_core_actions() {
        let action_names: Vec<&str> = DEFAULTS.iter().map(|d| d.action_name).collect();
        for name in &[
            "split_horizontally",
            "split_vertically",
            "close_pane",
            "next_workspace",
            "focus_left",
            "focus_right",
            "focus_up",
            "focus_down",
            "terminal_copy",
            "terminal_paste",
            "toggle_zoom",
            "toggle_copy_mode",
            "toggle_search",
            "split_equalize",
            "swap_pane",
            "undo_close_pane",
            "toggle_files_sidebar",
        ] {
            assert!(
                action_names.contains(name),
                "Action '{name}' missing from DEFAULTS"
            );
        }
    }

    // -- US-009 ---------------------------------------------------------

    #[test]
    fn us009_migrated_defaults_use_secondary() {
        // AC1: the seven migrated actions must carry a `secondary-` prefix.
        let migrated = [
            "split_horizontally",
            "split_vertically",
            "close_pane",
            "new_workspace",
            "close_workspace",
            "next_workspace",
            "select_workspace_1",
            "select_workspace_2",
            "select_workspace_3",
            "select_workspace_4",
            "select_workspace_5",
            "select_workspace_6",
            "select_workspace_7",
            "select_workspace_8",
            "select_workspace_9",
        ];
        for action in migrated {
            let entry = DEFAULTS
                .iter()
                .find(|d| d.action_name == action)
                .unwrap_or_else(|| panic!("missing DEFAULTS entry for {action}"));
            assert!(
                entry.key.starts_with("secondary-"),
                "action `{action}` still uses `{}` - US-009 requires `secondary-` prefix",
                entry.key,
            );
        }
    }

    #[test]
    fn us009_terminal_copy_paste_untouched() {
        // AC4: terminal copy/paste must keep `ctrl-shift-c/v` so Linux users
        // retain the terminal-standard bindings and Ctrl+C stays SIGINT-safe.
        let copy = DEFAULTS
            .iter()
            .find(|d| d.action_name == "terminal_copy")
            .expect("terminal_copy must be a default");
        assert_eq!(copy.key, "ctrl-shift-c");
        assert_eq!(copy.context, Some("Terminal"));

        let paste = DEFAULTS
            .iter()
            .find(|d| d.action_name == "terminal_paste")
            .expect("terminal_paste must be a default");
        assert_eq!(paste.key, "ctrl-shift-v");
        assert_eq!(paste.context, Some("Terminal"));
    }

    // -- US-010 ---------------------------------------------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn us010_cmd_c_cmd_v_bound_on_macos() {
        let copy = MACOS_ONLY_DEFAULTS
            .iter()
            .find(|d| d.key == "cmd-c")
            .expect("cmd-c must be a macOS default");
        assert_eq!(copy.action_name, "terminal_copy");
        assert_eq!(copy.context, Some("Terminal"));

        let paste = MACOS_ONLY_DEFAULTS
            .iter()
            .find(|d| d.key == "cmd-v")
            .expect("cmd-v must be a macOS default");
        assert_eq!(paste.action_name, "terminal_paste");
        assert_eq!(paste.context, Some("Terminal"));

        // Base DEFAULTS still hold the ctrl-shift-c/v entries - the macOS
        // bindings are ADDITIVE, not replacements.
        assert!(
            DEFAULTS
                .iter()
                .any(|d| d.key == "ctrl-shift-c" && d.action_name == "terminal_copy")
        );
        assert!(
            DEFAULTS
                .iter()
                .any(|d| d.key == "ctrl-shift-v" && d.action_name == "terminal_paste")
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn us010_no_cmd_bindings_on_linux() {
        assert!(
            MACOS_ONLY_DEFAULTS.is_empty(),
            "Linux build should carry zero macOS-only defaults, got {} entries",
            MACOS_ONLY_DEFAULTS.len()
        );
    }

    #[test]
    fn us010_ctrl_c_never_bound_to_terminal_copy() {
        // AC4: plain `ctrl-c` (without shift) must never reach terminal_copy
        // on any platform - the PTY needs to receive it so running
        // processes still get SIGINT.
        let leaked_actions: Vec<&'static str> = DEFAULTS
            .iter()
            .chain(MACOS_ONLY_DEFAULTS.iter())
            .filter(|d| d.key == "ctrl-c")
            .map(|d| d.action_name)
            .collect();
        assert!(
            leaked_actions.is_empty(),
            "ctrl-c must not appear in defaults (SIGINT safety); bound to: {leaked_actions:?}"
        );
    }

    // -- US-012 ---------------------------------------------------------

    #[cfg(target_os = "macos")]
    #[test]
    fn us012_cmd_q_bound_to_quit() {
        let quit = MACOS_ONLY_DEFAULTS
            .iter()
            .find(|d| d.key == "cmd-q")
            .expect("cmd-q must be a macOS default");
        assert_eq!(quit.action_name, "quit");
        assert_eq!(quit.context, None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn us012_no_cmd_q_on_linux() {
        assert!(MACOS_ONLY_DEFAULTS.iter().all(|d| d.key != "cmd-q"));
    }
}
