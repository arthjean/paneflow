//! GPUI action types dispatched through the focus chain.
//!
//! The `actions!` macro generates zero-sized types in the enclosing module,
//! all publicly visible, and registers them under the `paneflow` namespace
//! for JSON dispatch via `cx.dispatch_action`.

use gpui::actions;

actions!(
    paneflow,
    [
        SplitHorizontally,
        SplitVertically,
        ClosePane,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        FocusLeft,
        FocusRight,
        FocusUp,
        FocusDown,
        JumpNextWaiting,
        NewWorkspace,
        CloseWorkspace,
        CopyWorkspacePath,
        RevealWorkspaceInFileManager,
        OpenWorkspaceInZed,
        OpenWorkspaceInCursor,
        OpenWorkspaceInVsCode,
        OpenWorkspaceInWindsurf,
        NextWorkspace,
        SelectWorkspace1,
        SelectWorkspace2,
        SelectWorkspace3,
        SelectWorkspace4,
        SelectWorkspace5,
        SelectWorkspace6,
        SelectWorkspace7,
        SelectWorkspace8,
        SelectWorkspace9,
        TerminalCopy,
        TerminalPaste,
        ScrollPageUp,
        ScrollPageDown,
        ToggleZoom,
        LayoutEvenHorizontal,
        LayoutEvenVertical,
        LayoutMainVertical,
        LayoutTiled,
        SplitEqualize,
        SwapPane,
        ToggleSearch,
        ToggleSearchRegex,
        UndoClosePane,
        SearchNext,
        SearchPrev,
        DismissSearch,
        ToggleCopyMode,
        ClearScrollHistory,
        ResetTerminal,
        JumpPrevPrompt,
        JumpNextPrompt,
        // EP-006 US-019 (cli-cockpit) - per-pane font zoom. Terminal
        // context; the secondary-=/-/-0 default chords follow the Linux
        // terminal-emulator zoom convention (gnome-terminal, Ghostty) -
        // the readline shadow is the PRD's documented, remappable
        // exception to the no-shadow rule.
        FontSizeIncrease,
        FontSizeDecrease,
        FontSizeReset,
        // EP-006 US-018 - widen the open search to every pane of every
        // workspace (fleet grep). Search context (find bar open).
        ToggleFleetSearch,
        StartSelfUpdate,
        /// US-007 AC3: dismiss the update pill for the current launch
        /// (no persistence - re-prompts on next start). Dispatched by
        /// the `×` button on the Idle / Errored pill states.
        DismissUpdate,
        // US-012: macOS native menu-bar actions. Dispatched by `cx.set_menus`
        // via GPUI's `on_app_menu_action` → `cx.dispatch_action`, then caught
        // by the `.on_action(...)` handlers on the PaneFlowApp render root.
        Quit,
        About,
        Copy,
        Paste,
        SelectAll,
        OpenHelp,
        // US-022 (cmux port 2026-Q2) - markdown pane navigation. Scoped to
        // the `Markdown` key context (root) and `MarkdownSearch` (when the
        // find overlay is open). Defined as separate actions from terminal
        // scroll/copy so the keybinding registry can scope them cleanly.
        MarkdownScrollPageUp,
        MarkdownScrollPageDown,
        MarkdownFindOpen,
        MarkdownFindNext,
        MarkdownFindPrev,
        MarkdownFindDismiss,
        MarkdownCopy,
        // US-003 of tasks/prd-multi-worktree-diff-2026-Q3.md - open the
        // multi-worktree diff view for the active workspace's repo. Resolves
        // the repo from `active_idx`'s `repo_root` and opens a `DiffView` tab
        // seeded with every sibling worktree. Also invoked directly by the
        // sidebar group header's "Diff all" button.
        OpenMultiDiff,
        // US-003 of tasks/prd-git-diff-mode-2026-Q3.md - toggle the
        // dedicated Git Diff mode (AppMode::Diff): a full-screen diff
        // surface entered via the CLI / Diff sidebar toggle. Distinct from
        // `OpenMultiDiff` (the ephemeral tab path), which stays alive as a
        // secondary entry.
        OpenDiffView,
        // US-003 of tasks/prd-ai-in-diff-2026-Q3.md - copy the hunk under the
        // cursor as a unified diff (Ctrl+Shift+C inside the DiffView context).
        CopyDiffHunk,
        // EP-003 US-009 (review redesign) - keyboard-first review
        // loop. All scoped to `DiffView && !Terminal && !TextInput` so they drive
        // the diff body without stealing keystrokes from an embedded review/shell
        // terminal or the base-branch filter input.
        // `[`/`]` step hunks (wired to `goto_hunk`), `u` toggles unified/split,
        // `s` toggles cross-column scroll sync, `Esc` dismisses any open
        // popover/menu and refocuses the body.
        DiffNextHunk,
        DiffPrevHunk,
        DiffToggleView,
        DiffToggleSync,
        DiffDismiss,
        // EP-001 (CLI Cockpit) - CLI cockpit
        // steering. `OpenComposer` (US-001) anchors the multi-line prompt
        // Composer to the focused pane; `ToggleBroadcastMember` and
        // `OpenBroadcastGroups` (US-002) manage the named pane groups the
        // Composer's broadcast mode targets (US-003).
        OpenComposer,
        ToggleBroadcastMember,
        OpenBroadcastGroups,
        // EP-002 (CLI Cockpit) - triage & launch.
        // `OpenAttentionQueue` (US-004) lists every WaitingForInput session
        // cross-workspace with its question + wait time; `OpenLaunchPad`
        // (US-005) is the worktree + split + agent + prefill one-gesture
        // modal.
        ToggleFilesSidebar,
        OpenAttentionQueue,
        OpenLaunchPad,
        // EP-005 US-018 (prd-file-editor-2026-Q3): the diff dock's `+` menu
        // advertises Ctrl+G / Ctrl+J on its two rows. These make both chords
        // real. Both no-op unless the dock is open, and both are scoped away
        // from terminals and text widgets so a shell keeps its own Ctrl+G
        // (BEL) and Ctrl+J (LF).
        DiffNewFileTab,
        DiffNewTerminalTab
    ]
);
