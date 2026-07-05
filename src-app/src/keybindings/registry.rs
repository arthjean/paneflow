//! Unified action registry.
//!
//! A single [`ActionMeta`] table (`ACTIONS`) replaces three parallel match
//! statements - `action_from_name`, `context_for_action`, `action_description`
//! so adding an action requires exactly one edit.

use gpui::Action;

use crate::{
    ClearScrollHistory, ClosePane, CloseTab, CloseWindow, CloseWorkspace, CopyWorkspacePath,
    DismissSearch, FocusDown, FocusLeft, FocusRight, FocusUp, JumpNextWaiting,
    LayoutEvenHorizontal, LayoutEvenVertical, LayoutMainVertical, LayoutTiled, MarkdownCopy,
    MarkdownFindDismiss, MarkdownFindNext, MarkdownFindOpen, MarkdownFindPrev,
    MarkdownScrollPageDown, MarkdownScrollPageUp, NewTab, NewWorkspace, NextWorkspace,
    OpenWorkspaceInCursor, OpenWorkspaceInVsCode, OpenWorkspaceInWindsurf, OpenWorkspaceInZed,
    Quit, ResetTerminal, RevealWorkspaceInFileManager, ScrollPageDown, ScrollPageUp, SearchNext,
    SearchPrev, SelectWorkspace1, SelectWorkspace2, SelectWorkspace3, SelectWorkspace4,
    SelectWorkspace5, SelectWorkspace6, SelectWorkspace7, SelectWorkspace8, SelectWorkspace9,
    SplitEqualize, SplitHorizontally, SplitVertically, SwapPane, TerminalCopy, TerminalPaste,
    ToggleCopyMode, ToggleSearch, ToggleSearchRegex, ToggleZoom, UndoClosePane,
};
use crate::{FontSizeDecrease, FontSizeIncrease, FontSizeReset, ToggleFleetSearch};

/// Metadata for a single dispatchable action.
///
/// Empty `context` means the action is global (no `KeyBindingContextPredicate`).
/// `factory` boxes a fresh action instance on each call so GPUI's
/// `KeyBinding::load` can own it.
pub(super) struct ActionMeta {
    pub(super) name: &'static str,
    pub(super) factory: fn() -> Box<dyn Action>,
    pub(super) context: &'static str,
    pub(super) description: &'static str,
}

/// The one source of truth for every action dispatched by `keybindings/`.
///
/// Order matches the historical groupings (splits, workspaces, focus, tabs,
/// terminal, layouts, search, scroll-backward) so the settings page preserves
/// its visual grouping when iterating.
pub(super) const ACTIONS: &[ActionMeta] = &[
    ActionMeta {
        name: "split_horizontally",
        factory: || Box::new(SplitHorizontally),
        context: "",
        description: "Split horizontal",
    },
    ActionMeta {
        name: "split_vertically",
        factory: || Box::new(SplitVertically),
        context: "",
        description: "Split vertical",
    },
    ActionMeta {
        name: "close_pane",
        factory: || Box::new(ClosePane),
        context: "",
        description: "Close pane",
    },
    ActionMeta {
        name: "new_workspace",
        factory: || Box::new(NewWorkspace),
        context: "",
        description: "New workspace",
    },
    ActionMeta {
        name: "close_workspace",
        factory: || Box::new(CloseWorkspace),
        context: "",
        description: "Close workspace",
    },
    ActionMeta {
        name: "copy_workspace_path",
        factory: || Box::new(CopyWorkspacePath),
        context: "",
        description: "Copy path",
    },
    ActionMeta {
        name: "reveal_workspace_in_file_manager",
        factory: || Box::new(RevealWorkspaceInFileManager),
        context: "",
        description: "Reveal in file manager",
    },
    ActionMeta {
        name: "open_workspace_in_zed",
        factory: || Box::new(OpenWorkspaceInZed),
        context: "",
        description: "Open in Zed",
    },
    ActionMeta {
        name: "open_workspace_in_cursor",
        factory: || Box::new(OpenWorkspaceInCursor),
        context: "",
        description: "Open in Cursor",
    },
    ActionMeta {
        name: "open_workspace_in_vscode",
        factory: || Box::new(OpenWorkspaceInVsCode),
        context: "",
        description: "Open in VS Code",
    },
    ActionMeta {
        name: "open_workspace_in_windsurf",
        factory: || Box::new(OpenWorkspaceInWindsurf),
        context: "",
        description: "Open in Windsurf",
    },
    ActionMeta {
        name: "next_workspace",
        factory: || Box::new(NextWorkspace),
        context: "",
        description: "Next workspace",
    },
    ActionMeta {
        name: "focus_left",
        factory: || Box::new(FocusLeft),
        context: "",
        description: "Focus left",
    },
    ActionMeta {
        name: "focus_right",
        factory: || Box::new(FocusRight),
        context: "",
        description: "Focus right",
    },
    ActionMeta {
        name: "focus_up",
        factory: || Box::new(FocusUp),
        context: "",
        description: "Focus up",
    },
    ActionMeta {
        name: "focus_down",
        factory: || Box::new(FocusDown),
        context: "",
        description: "Focus down",
    },
    ActionMeta {
        name: "jump_next_waiting",
        factory: || Box::new(JumpNextWaiting),
        context: "",
        description: "Jump to next waiting agent",
    },
    ActionMeta {
        name: "select_workspace_1",
        factory: || Box::new(SelectWorkspace1),
        context: "",
        description: "Select workspace 1",
    },
    ActionMeta {
        name: "select_workspace_2",
        factory: || Box::new(SelectWorkspace2),
        context: "",
        description: "Select workspace 2",
    },
    ActionMeta {
        name: "select_workspace_3",
        factory: || Box::new(SelectWorkspace3),
        context: "",
        description: "Select workspace 3",
    },
    ActionMeta {
        name: "select_workspace_4",
        factory: || Box::new(SelectWorkspace4),
        context: "",
        description: "Select workspace 4",
    },
    ActionMeta {
        name: "select_workspace_5",
        factory: || Box::new(SelectWorkspace5),
        context: "",
        description: "Select workspace 5",
    },
    ActionMeta {
        name: "select_workspace_6",
        factory: || Box::new(SelectWorkspace6),
        context: "",
        description: "Select workspace 6",
    },
    ActionMeta {
        name: "select_workspace_7",
        factory: || Box::new(SelectWorkspace7),
        context: "",
        description: "Select workspace 7",
    },
    ActionMeta {
        name: "select_workspace_8",
        factory: || Box::new(SelectWorkspace8),
        context: "",
        description: "Select workspace 8",
    },
    ActionMeta {
        name: "select_workspace_9",
        factory: || Box::new(SelectWorkspace9),
        context: "",
        description: "Select workspace 9",
    },
    ActionMeta {
        name: "new_tab",
        factory: || Box::new(NewTab),
        context: "",
        description: "New tab",
    },
    ActionMeta {
        name: "close_tab",
        factory: || Box::new(CloseTab),
        context: "",
        description: "Close tab",
    },
    ActionMeta {
        name: "terminal_copy",
        factory: || Box::new(TerminalCopy),
        context: "Terminal",
        description: "Copy",
    },
    ActionMeta {
        name: "terminal_paste",
        factory: || Box::new(TerminalPaste),
        context: "Terminal",
        description: "Paste",
    },
    ActionMeta {
        name: "scroll_page_up",
        factory: || Box::new(ScrollPageUp),
        context: "Terminal",
        description: "Scroll up",
    },
    ActionMeta {
        name: "scroll_page_down",
        factory: || Box::new(ScrollPageDown),
        context: "Terminal",
        description: "Scroll down",
    },
    ActionMeta {
        name: "close_window",
        factory: || Box::new(CloseWindow),
        context: "",
        description: "Close window",
    },
    ActionMeta {
        name: "toggle_zoom",
        factory: || Box::new(ToggleZoom),
        context: "",
        description: "Toggle zoom",
    },
    ActionMeta {
        name: "layout_even_horizontal",
        factory: || Box::new(LayoutEvenHorizontal),
        context: "",
        description: "Layout even horizontal",
    },
    ActionMeta {
        name: "layout_even_vertical",
        factory: || Box::new(LayoutEvenVertical),
        context: "",
        description: "Layout even vertical",
    },
    ActionMeta {
        name: "layout_main_vertical",
        factory: || Box::new(LayoutMainVertical),
        context: "",
        description: "Layout main vertical",
    },
    ActionMeta {
        name: "layout_tiled",
        factory: || Box::new(LayoutTiled),
        context: "",
        description: "Layout tiled",
    },
    ActionMeta {
        name: "split_equalize",
        factory: || Box::new(SplitEqualize),
        context: "",
        description: "Equalize panes",
    },
    ActionMeta {
        name: "swap_pane",
        factory: || Box::new(SwapPane),
        context: "",
        description: "Swap pane",
    },
    ActionMeta {
        name: "undo_close_pane",
        factory: || Box::new(UndoClosePane),
        context: "",
        description: "Undo close pane",
    },
    ActionMeta {
        name: "toggle_copy_mode",
        factory: || Box::new(ToggleCopyMode),
        context: "Terminal",
        description: "Toggle copy mode",
    },
    ActionMeta {
        name: "toggle_search",
        factory: || Box::new(ToggleSearch),
        context: "Terminal",
        description: "Toggle search",
    },
    ActionMeta {
        name: "font_size_increase",
        factory: || Box::new(FontSizeIncrease),
        context: "Terminal",
        description: "Increase pane font size",
    },
    ActionMeta {
        name: "font_size_decrease",
        factory: || Box::new(FontSizeDecrease),
        context: "Terminal",
        description: "Decrease pane font size",
    },
    ActionMeta {
        name: "font_size_reset",
        factory: || Box::new(FontSizeReset),
        context: "Terminal",
        description: "Reset pane font size",
    },
    ActionMeta {
        name: "toggle_fleet_search",
        factory: || Box::new(ToggleFleetSearch),
        context: "Search",
        description: "Search across all panes",
    },
    ActionMeta {
        name: "toggle_search_regex",
        factory: || Box::new(ToggleSearchRegex),
        context: "Search",
        description: "Toggle search regex",
    },
    ActionMeta {
        name: "search_next",
        factory: || Box::new(SearchNext),
        context: "Search",
        description: "Search next",
    },
    ActionMeta {
        name: "search_prev",
        factory: || Box::new(SearchPrev),
        context: "Search",
        description: "Search previous",
    },
    ActionMeta {
        name: "dismiss_search",
        factory: || Box::new(DismissSearch),
        context: "Search",
        description: "Dismiss search",
    },
    ActionMeta {
        name: "clear_scroll_history",
        factory: || Box::new(ClearScrollHistory),
        context: "Terminal",
        description: "Clear scroll history",
    },
    ActionMeta {
        name: "reset_terminal",
        factory: || Box::new(ResetTerminal),
        context: "Terminal",
        description: "Reset terminal",
    },
    // US-012: Quit menu action (bound to cmd-q on macOS via
    // MACOS_ONLY_DEFAULTS; also reachable from PaneFlow > Quit PaneFlow).
    ActionMeta {
        name: "quit",
        factory: || Box::new(Quit),
        context: "",
        description: "Quit",
    },
    // US-022: Markdown pane navigation. Scroll + copy bind on the root
    // `Markdown` context; find-overlay actions bind on `MarkdownSearch`
    // (active only while the search bar is open).
    ActionMeta {
        name: "markdown_scroll_page_up",
        factory: || Box::new(MarkdownScrollPageUp),
        context: "Markdown",
        description: "Markdown: scroll up one page",
    },
    ActionMeta {
        name: "markdown_scroll_page_down",
        factory: || Box::new(MarkdownScrollPageDown),
        context: "Markdown",
        description: "Markdown: scroll down one page",
    },
    ActionMeta {
        name: "markdown_find_open",
        factory: || Box::new(MarkdownFindOpen),
        context: "Markdown",
        description: "Markdown: open find bar",
    },
    ActionMeta {
        name: "markdown_copy",
        factory: || Box::new(MarkdownCopy),
        context: "Markdown",
        description: "Markdown: copy selection / current match",
    },
    ActionMeta {
        name: "markdown_find_next",
        factory: || Box::new(MarkdownFindNext),
        context: "MarkdownSearch",
        description: "Markdown: jump to next match",
    },
    ActionMeta {
        name: "markdown_find_prev",
        factory: || Box::new(MarkdownFindPrev),
        context: "MarkdownSearch",
        description: "Markdown: jump to previous match",
    },
    ActionMeta {
        name: "markdown_find_dismiss",
        factory: || Box::new(MarkdownFindDismiss),
        context: "MarkdownSearch",
        description: "Markdown: close find bar",
    },
    // US-005 (prd-agents-view.md): toggle the lightweight Agents view.
    ActionMeta {
        name: "open_agents_view",
        factory: || Box::new(crate::OpenAgentsView),
        context: "",
        description: "Toggle Agents view",
    },
    // US-003 (prd-git-diff-mode-2026-Q3.md): toggle the dedicated Git
    // Diff mode (AppMode::Diff).
    ActionMeta {
        name: "open_diff_view",
        factory: || Box::new(crate::OpenDiffView),
        context: "",
        description: "Toggle Git Diff view",
    },
    ActionMeta {
        name: "toggle_files_sidebar",
        factory: || Box::new(crate::ToggleFilesSidebar),
        context: "",
        description: "Toggle Files sidebar",
    },
    // US-003 (prd-ai-in-diff-2026-Q3.md): copy the hunk under the cursor as a
    // unified diff. Scoped to the DiffView context so Ctrl+Shift+C there never
    // collides with the global markdown / terminal copy bindings.
    ActionMeta {
        name: "copy_diff_hunk",
        factory: || Box::new(crate::CopyDiffHunk),
        context: "DiffView",
        description: "Copy hunk as diff",
    },
    // EP-003 US-009 (prd-review-redesign-2026-Q3.md): keyboard-first review loop.
    // Keep these off embedded terminals and text widgets inside DiffView.
    ActionMeta {
        name: "diff_next_hunk",
        factory: || Box::new(crate::DiffNextHunk),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: next hunk",
    },
    ActionMeta {
        name: "diff_prev_hunk",
        factory: || Box::new(crate::DiffPrevHunk),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: previous hunk",
    },
    ActionMeta {
        name: "diff_toggle_view",
        factory: || Box::new(crate::DiffToggleView),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: toggle unified / split",
    },
    ActionMeta {
        name: "diff_toggle_sync",
        factory: || Box::new(crate::DiffToggleSync),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: toggle scroll sync",
    },
    ActionMeta {
        name: "diff_dismiss",
        factory: || Box::new(crate::DiffDismiss),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: close popover / refocus body",
    },
    // EP-001 (prd-cli-cockpit-ergonomics-2026-Q3.md): Cli-cockpit steering.
    // Global context - the handlers gate on `AppMode::Cli` themselves.
    ActionMeta {
        name: "open_composer",
        factory: || Box::new(crate::OpenComposer),
        context: "",
        description: "Open prompt composer",
    },
    ActionMeta {
        name: "toggle_broadcast_member",
        factory: || Box::new(crate::ToggleBroadcastMember),
        context: "",
        description: "Toggle pane in broadcast group",
    },
    ActionMeta {
        name: "open_broadcast_groups",
        factory: || Box::new(crate::OpenBroadcastGroups),
        context: "",
        description: "Broadcast groups",
    },
    // EP-002 (prd-cli-cockpit-ergonomics-2026-Q3.md): triage & launch.
    ActionMeta {
        name: "open_attention_queue",
        factory: || Box::new(crate::OpenAttentionQueue),
        context: "",
        description: "Attention queue",
    },
    ActionMeta {
        name: "open_launch_pad",
        factory: || Box::new(crate::OpenLaunchPad),
        context: "",
        description: "Launch Pad",
    },
];

fn find(name: &str) -> Option<&'static ActionMeta> {
    ACTIONS.iter().find(|a| a.name == name)
}

/// Resolve an action name string to a boxed GPUI action.
pub(super) fn action_from_name(name: &str) -> Option<Box<dyn Action>> {
    find(name).map(|meta| (meta.factory)())
}

/// Context predicate for a given action name. `None` is global.
pub(super) fn context_for_action(name: &str) -> Option<&'static str> {
    find(name)
        .map(|meta| meta.context)
        .filter(|ctx| !ctx.is_empty())
}

/// Human-readable description for an action name, or `"Unknown"`.
pub(super) fn action_description(name: &str) -> &'static str {
    find(name).map(|meta| meta.description).unwrap_or("Unknown")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_from_name_known_actions() {
        assert!(action_from_name("split_horizontally").is_some());
        assert!(action_from_name("close_pane").is_some());
        assert!(action_from_name("toggle_zoom").is_some());
        assert!(action_from_name("undo_close_pane").is_some());
        assert!(action_from_name("swap_pane").is_some());
        assert!(action_from_name("split_equalize").is_some());
        assert!(action_from_name("toggle_copy_mode").is_some());
        assert!(action_from_name("toggle_files_sidebar").is_some());
    }

    #[test]
    fn action_from_name_unknown_returns_none() {
        assert!(action_from_name("nonexistent_action").is_none());
        assert!(action_from_name("").is_none());
    }

    #[test]
    fn context_for_terminal_actions() {
        assert_eq!(context_for_action("terminal_copy"), Some("Terminal"));
        assert_eq!(context_for_action("toggle_copy_mode"), Some("Terminal"));
        assert_eq!(context_for_action("toggle_search"), Some("Terminal"));
        assert_eq!(context_for_action("split_horizontally"), None);
        assert_eq!(context_for_action("toggle_files_sidebar"), None);
    }

    #[test]
    fn registry_has_unique_action_names() {
        // A duplicate name would silently shadow another entry's context or
        // description. Catch it early.
        let mut seen = std::collections::HashSet::new();
        for meta in ACTIONS {
            assert!(
                seen.insert(meta.name),
                "duplicate action name `{}` in ACTIONS",
                meta.name
            );
        }
    }

    #[test]
    fn us012_quit_action_name_resolves() {
        // Cross-platform: `action_from_name` must resolve "quit" to a real
        // Action instance so MACOS_ONLY_DEFAULTS registration succeeds on
        // macOS and user config overrides like `"quit": "secondary-alt-q"`
        // work on any platform.
        assert!(action_from_name("quit").is_some());
    }
}
