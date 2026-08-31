//! Unified action registry.
//!
//! A single [`ActionMeta`] table (`ACTIONS`) replaces three parallel match
//! statements - `action_from_name`, `context_for_action`, `action_description`
//! so adding an action requires exactly one edit.

use gpui::Action;

use crate::{
    ClearScrollHistory, ClosePane, CloseTab, CloseWorkspace, CopyWorkspacePath, DismissSearch,
    FocusDown, FocusLeft, FocusRight, FocusUp, JumpNextPrompt, JumpNextWaiting, JumpPrevPrompt,
    LayoutEvenHorizontal, LayoutEvenVertical, LayoutMainVertical, LayoutTiled, MarkdownCopy,
    MarkdownFindDismiss, MarkdownFindNext, MarkdownFindOpen, MarkdownFindPrev,
    MarkdownScrollPageDown, MarkdownScrollPageUp, NewTab, NewWorkspace, NextTab, NextWorkspace,
    OpenWorkspaceInCursor, OpenWorkspaceInVsCode, OpenWorkspaceInWindsurf, OpenWorkspaceInZed,
    PreviousTab, Quit, ResetTerminal, RevealWorkspaceInFileManager, ScrollPageDown, ScrollPageUp,
    SearchNext, SearchPrev, SelectWorkspace1, SelectWorkspace2, SelectWorkspace3, SelectWorkspace4,
    SelectWorkspace5, SelectWorkspace6, SelectWorkspace7, SelectWorkspace8, SelectWorkspace9,
    SplitEqualize, SplitHorizontally, SplitVertically, SwapPane, TerminalCopy, TerminalPaste,
    ToggleCopyMode, ToggleSearch, ToggleSearchRegex, ToggleZoom, UndoClosePane,
};
use crate::{FontSizeDecrease, FontSizeIncrease, FontSizeReset, ToggleFleetSearch};

/// The section an action belongs to on the Shortcuts settings page.
///
/// The registry's *order* used to carry this implicitly, which meant the
/// settings page could only ever render one flat list and any reordering of
/// `ACTIONS` silently reshuffled the visual grouping. Declaring the section
/// makes it survive sorting, filtering, and insertion in the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShortcutGroup {
    Panes,
    Workspaces,
    Tabs,
    Terminal,
    Search,
    Diff,
    Markdown,
    Agents,
    Application,
}

impl ShortcutGroup {
    /// Display order on the settings page, most-used first.
    pub const ALL: &'static [ShortcutGroup] = &[
        ShortcutGroup::Panes,
        ShortcutGroup::Workspaces,
        ShortcutGroup::Tabs,
        ShortcutGroup::Terminal,
        ShortcutGroup::Search,
        ShortcutGroup::Diff,
        ShortcutGroup::Markdown,
        ShortcutGroup::Agents,
        ShortcutGroup::Application,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShortcutGroup::Panes => "Panes & splits",
            ShortcutGroup::Workspaces => "Workspaces",
            ShortcutGroup::Tabs => "Tabs",
            ShortcutGroup::Terminal => "Terminal",
            ShortcutGroup::Search => "Search",
            ShortcutGroup::Diff => "Git diff",
            ShortcutGroup::Markdown => "Markdown",
            ShortcutGroup::Agents => "Agents & cockpit",
            ShortcutGroup::Application => "Application",
        }
    }
}

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
    pub(super) group: ShortcutGroup,
}

/// The one source of truth for every action dispatched by `keybindings/`.
pub(super) const ACTIONS: &[ActionMeta] = &[
    ActionMeta {
        name: "split_horizontally",
        factory: || Box::new(SplitHorizontally),
        context: "",
        description: "Split horizontal",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "split_vertically",
        factory: || Box::new(SplitVertically),
        context: "",
        description: "Split vertical",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "close_pane",
        factory: || Box::new(ClosePane),
        context: "",
        description: "Close pane",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "new_workspace",
        factory: || Box::new(NewWorkspace),
        context: "",
        description: "New workspace",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "close_workspace",
        factory: || Box::new(CloseWorkspace),
        context: "",
        description: "Close workspace",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "copy_workspace_path",
        factory: || Box::new(CopyWorkspacePath),
        context: "",
        description: "Copy path",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "reveal_workspace_in_file_manager",
        factory: || Box::new(RevealWorkspaceInFileManager),
        context: "",
        description: "Reveal in file manager",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "open_workspace_in_zed",
        factory: || Box::new(OpenWorkspaceInZed),
        context: "",
        description: "Open in Zed",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "open_workspace_in_cursor",
        factory: || Box::new(OpenWorkspaceInCursor),
        context: "",
        description: "Open in Cursor",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "open_workspace_in_vscode",
        factory: || Box::new(OpenWorkspaceInVsCode),
        context: "",
        description: "Open in VS Code",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "open_workspace_in_windsurf",
        factory: || Box::new(OpenWorkspaceInWindsurf),
        context: "",
        description: "Open in Windsurf",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "next_workspace",
        factory: || Box::new(NextWorkspace),
        context: "",
        description: "Next workspace",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "focus_left",
        factory: || Box::new(FocusLeft),
        context: "",
        description: "Focus left",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "focus_right",
        factory: || Box::new(FocusRight),
        context: "",
        description: "Focus right",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "focus_up",
        factory: || Box::new(FocusUp),
        context: "",
        description: "Focus up",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "focus_down",
        factory: || Box::new(FocusDown),
        context: "",
        description: "Focus down",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "jump_next_waiting",
        factory: || Box::new(JumpNextWaiting),
        context: "",
        description: "Jump to next waiting agent",
        group: ShortcutGroup::Agents,
    },
    ActionMeta {
        name: "select_workspace_1",
        factory: || Box::new(SelectWorkspace1),
        context: "",
        description: "Select workspace 1",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_2",
        factory: || Box::new(SelectWorkspace2),
        context: "",
        description: "Select workspace 2",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_3",
        factory: || Box::new(SelectWorkspace3),
        context: "",
        description: "Select workspace 3",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_4",
        factory: || Box::new(SelectWorkspace4),
        context: "",
        description: "Select workspace 4",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_5",
        factory: || Box::new(SelectWorkspace5),
        context: "",
        description: "Select workspace 5",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_6",
        factory: || Box::new(SelectWorkspace6),
        context: "",
        description: "Select workspace 6",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_7",
        factory: || Box::new(SelectWorkspace7),
        context: "",
        description: "Select workspace 7",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_8",
        factory: || Box::new(SelectWorkspace8),
        context: "",
        description: "Select workspace 8",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "select_workspace_9",
        factory: || Box::new(SelectWorkspace9),
        context: "",
        description: "Select workspace 9",
        group: ShortcutGroup::Workspaces,
    },
    ActionMeta {
        name: "new_tab",
        factory: || Box::new(NewTab),
        context: "",
        description: "New tab",
        group: ShortcutGroup::Tabs,
    },
    ActionMeta {
        name: "close_tab",
        factory: || Box::new(CloseTab),
        context: "",
        description: "Close tab",
        group: ShortcutGroup::Tabs,
    },
    ActionMeta {
        name: "next_tab",
        factory: || Box::new(NextTab),
        context: "",
        description: "Next tab",
        group: ShortcutGroup::Tabs,
    },
    ActionMeta {
        name: "previous_tab",
        factory: || Box::new(PreviousTab),
        context: "",
        description: "Previous tab",
        group: ShortcutGroup::Tabs,
    },
    ActionMeta {
        name: "terminal_copy",
        factory: || Box::new(TerminalCopy),
        context: "Terminal",
        description: "Copy",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "terminal_paste",
        factory: || Box::new(TerminalPaste),
        context: "Terminal",
        description: "Paste",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "scroll_page_up",
        factory: || Box::new(ScrollPageUp),
        context: "Terminal",
        description: "Scroll up",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "scroll_page_down",
        factory: || Box::new(ScrollPageDown),
        context: "Terminal",
        description: "Scroll down",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "jump_prev_prompt",
        factory: || Box::new(JumpPrevPrompt),
        context: "Terminal",
        description: "Jump to previous prompt",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "jump_next_prompt",
        factory: || Box::new(JumpNextPrompt),
        context: "Terminal",
        description: "Jump to next prompt",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "toggle_zoom",
        factory: || Box::new(ToggleZoom),
        context: "",
        description: "Toggle zoom",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "layout_even_horizontal",
        factory: || Box::new(LayoutEvenHorizontal),
        context: "",
        description: "Layout even horizontal",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "layout_even_vertical",
        factory: || Box::new(LayoutEvenVertical),
        context: "",
        description: "Layout even vertical",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "layout_main_vertical",
        factory: || Box::new(LayoutMainVertical),
        context: "",
        description: "Layout main vertical",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "layout_tiled",
        factory: || Box::new(LayoutTiled),
        context: "",
        description: "Layout tiled",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "split_equalize",
        factory: || Box::new(SplitEqualize),
        context: "",
        description: "Equalize panes",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "swap_pane",
        factory: || Box::new(SwapPane),
        context: "",
        description: "Swap pane",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "undo_close_pane",
        factory: || Box::new(UndoClosePane),
        context: "",
        description: "Undo close pane",
        group: ShortcutGroup::Panes,
    },
    ActionMeta {
        name: "toggle_copy_mode",
        factory: || Box::new(ToggleCopyMode),
        context: "Terminal",
        description: "Toggle copy mode",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "toggle_search",
        factory: || Box::new(ToggleSearch),
        context: "Terminal",
        description: "Toggle search",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "font_size_increase",
        factory: || Box::new(FontSizeIncrease),
        context: "Terminal",
        description: "Increase pane font size",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "font_size_decrease",
        factory: || Box::new(FontSizeDecrease),
        context: "Terminal",
        description: "Decrease pane font size",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "font_size_reset",
        factory: || Box::new(FontSizeReset),
        context: "Terminal",
        description: "Reset pane font size",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "toggle_fleet_search",
        factory: || Box::new(ToggleFleetSearch),
        context: "Search",
        description: "Search across all panes",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "toggle_search_regex",
        factory: || Box::new(ToggleSearchRegex),
        context: "Search",
        description: "Toggle search regex",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "search_next",
        factory: || Box::new(SearchNext),
        context: "Search",
        description: "Search next",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "search_prev",
        factory: || Box::new(SearchPrev),
        context: "Search",
        description: "Search previous",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "dismiss_search",
        factory: || Box::new(DismissSearch),
        context: "Search",
        description: "Dismiss search",
        group: ShortcutGroup::Search,
    },
    ActionMeta {
        name: "clear_scroll_history",
        factory: || Box::new(ClearScrollHistory),
        context: "Terminal",
        description: "Clear scroll history",
        group: ShortcutGroup::Terminal,
    },
    ActionMeta {
        name: "reset_terminal",
        factory: || Box::new(ResetTerminal),
        context: "Terminal",
        description: "Reset terminal",
        group: ShortcutGroup::Terminal,
    },
    // US-012: Quit menu action (bound to cmd-q on macOS via
    // MACOS_ONLY_DEFAULTS; also reachable from PaneFlow > Quit PaneFlow).
    ActionMeta {
        name: "quit",
        factory: || Box::new(Quit),
        context: "",
        description: "Quit",
        group: ShortcutGroup::Application,
    },
    // US-022: Markdown pane navigation. Scroll + copy bind on the root
    // `Markdown` context; find-overlay actions bind on `MarkdownSearch`
    // (active only while the search bar is open).
    ActionMeta {
        name: "markdown_scroll_page_up",
        factory: || Box::new(MarkdownScrollPageUp),
        context: "Markdown",
        description: "Markdown: scroll up one page",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_scroll_page_down",
        factory: || Box::new(MarkdownScrollPageDown),
        context: "Markdown",
        description: "Markdown: scroll down one page",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_find_open",
        factory: || Box::new(MarkdownFindOpen),
        context: "Markdown",
        description: "Markdown: open find bar",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_copy",
        factory: || Box::new(MarkdownCopy),
        context: "Markdown",
        description: "Markdown: copy selection / current match",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_find_next",
        factory: || Box::new(MarkdownFindNext),
        context: "MarkdownSearch",
        description: "Markdown: jump to next match",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_find_prev",
        factory: || Box::new(MarkdownFindPrev),
        context: "MarkdownSearch",
        description: "Markdown: jump to previous match",
        group: ShortcutGroup::Markdown,
    },
    ActionMeta {
        name: "markdown_find_dismiss",
        factory: || Box::new(MarkdownFindDismiss),
        context: "MarkdownSearch",
        description: "Markdown: close find bar",
        group: ShortcutGroup::Markdown,
    },
    // US-003 (prd-git-diff-mode-2026-Q3.md): toggle the dedicated Git
    // Diff mode (AppMode::Diff).
    ActionMeta {
        name: "open_diff_view",
        factory: || Box::new(crate::OpenDiffView),
        context: "",
        description: "Toggle Git Diff view",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "toggle_files_sidebar",
        factory: || Box::new(crate::ToggleFilesSidebar),
        context: "",
        description: "Toggle Files sidebar",
        group: ShortcutGroup::Diff,
    },
    // US-003 (prd-ai-in-diff-2026-Q3.md): copy the hunk under the cursor as a
    // unified diff. Scoped to the DiffView context so Ctrl+Shift+C there never
    // collides with the global markdown / terminal copy bindings.
    ActionMeta {
        name: "copy_diff_hunk",
        factory: || Box::new(crate::CopyDiffHunk),
        context: "DiffView",
        description: "Copy hunk as diff",
        group: ShortcutGroup::Diff,
    },
    // EP-003 US-009 (review redesign): keyboard-first review loop.
    // Keep these off embedded terminals and text widgets inside DiffView.
    ActionMeta {
        name: "diff_next_hunk",
        factory: || Box::new(crate::DiffNextHunk),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: next hunk",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "diff_prev_hunk",
        factory: || Box::new(crate::DiffPrevHunk),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: previous hunk",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "diff_toggle_view",
        factory: || Box::new(crate::DiffToggleView),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: toggle unified / split",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "diff_toggle_sync",
        factory: || Box::new(crate::DiffToggleSync),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: toggle scroll sync",
        group: ShortcutGroup::Diff,
    },
    // EP-005 US-018 (prd-file-editor-2026-Q3): the diff dock's new-tab chords.
    // Kept off terminals and text widgets - Ctrl+G is BEL and Ctrl+J is LF in a
    // shell, so a global binding would eat both. `CodeEditor` is excluded for
    // the same reason, and because it is the surface the file chord opens: a
    // caret inside the editor must keep its own keystrokes.
    ActionMeta {
        name: "diff_new_file_tab",
        factory: || Box::new(crate::DiffNewFileTab),
        context: "!Terminal && !TextInput && !PaneflowTextArea && !CodeEditor",
        description: "Diff dock: open a file tab",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "diff_new_terminal_tab",
        factory: || Box::new(crate::DiffNewTerminalTab),
        context: "!Terminal && !TextInput && !PaneflowTextArea && !CodeEditor",
        description: "Diff dock: open a terminal tab",
        group: ShortcutGroup::Diff,
    },
    ActionMeta {
        name: "diff_dismiss",
        factory: || Box::new(crate::DiffDismiss),
        context: "DiffView && !Terminal && !TextInput && !PaneflowTextArea",
        description: "Diff: close popover / refocus body",
        group: ShortcutGroup::Diff,
    },
    // EP-001 (CLI Cockpit): CLI cockpit steering.
    // Global context - the handlers gate on `AppMode::Cli` themselves.
    ActionMeta {
        name: "open_composer",
        factory: || Box::new(crate::OpenComposer),
        context: "",
        description: "Open prompt composer",
        group: ShortcutGroup::Agents,
    },
    ActionMeta {
        name: "toggle_broadcast_member",
        factory: || Box::new(crate::ToggleBroadcastMember),
        context: "",
        description: "Toggle pane in broadcast group",
        group: ShortcutGroup::Agents,
    },
    ActionMeta {
        name: "open_broadcast_groups",
        factory: || Box::new(crate::OpenBroadcastGroups),
        context: "",
        description: "Broadcast groups",
        group: ShortcutGroup::Agents,
    },
    // EP-002 (CLI Cockpit): triage & launch.
    ActionMeta {
        name: "open_attention_queue",
        factory: || Box::new(crate::OpenAttentionQueue),
        context: "",
        description: "Attention queue",
        group: ShortcutGroup::Agents,
    },
    ActionMeta {
        name: "open_launch_pad",
        factory: || Box::new(crate::OpenLaunchPad),
        context: "",
        description: "Launch Pad",
        group: ShortcutGroup::Agents,
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
    fn every_action_group_appears_in_all() {
        // `ShortcutGroup::ALL` is hand-maintained and is the settings page's
        // only iteration source, so a variant tagged on actions but forgotten
        // in `ALL` makes those rows vanish - unrebindable, with no error. A
        // test that iterates `ALL` cannot catch that; this one starts from the
        // actions instead.
        for meta in ACTIONS {
            assert!(
                ShortcutGroup::ALL.contains(&meta.group),
                "{:?} is missing from ShortcutGroup::ALL, so {} would never render",
                meta.group,
                meta.name
            );
        }
    }

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
