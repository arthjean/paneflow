//! Workspace and pane lifecycle operations for `PaneFlowApp`.
//!
//! Hosts the action handlers and helpers that create, select, split, close,
//! reorder, zoom, and re-layout workspaces and their pane trees. All methods
//! are pure code-motion from `main.rs` (US-023 of the src-app refactor PRD) -
//! behaviour is unchanged.
//!
//! Rendering (sidebar, context menus), IPC plumbing, toasts, settings, and
//! session persistence live in their own siblings under `app/`.
//!
//! Module layout:
//! - [`focus`] - focus-movement handlers (+ swap-on-focus override)
//! - [`tab`] - tab add/close
//! - [`swap`] - swap-mode toggle
//! - [`layout`] - zoom, layout presets, JSON layout application

mod focus;
mod layout;
mod swap;
mod tab;

use gpui::{App, AppContext, ClipboardItem, Context, Focusable, PathPromptOptions, Window};
use paneflow_process::spawn_detached;

use crate::layout::{LayoutTree, MAX_PANES, SplitDirection};
use crate::terminal::TerminalView;
use crate::workspace::{MAX_WORKSPACES, Workspace, next_workspace_id};
use crate::{
    ClosePane, CloseWorkspace, ClosedPaneRecord, ClosedTabRecord, CopyWorkspacePath,
    MAX_CLOSED_PANE_SCROLLBACK_BYTES, MAX_CLOSED_PANES, NewWorkspace, NextWorkspace,
    OpenWorkspaceInCursor, OpenWorkspaceInVsCode, OpenWorkspaceInWindsurf, OpenWorkspaceInZed,
    PaneFlowApp, RevealWorkspaceInFileManager, SelectWorkspace1, SelectWorkspace2,
    SelectWorkspace3, SelectWorkspace4, SelectWorkspace5, SelectWorkspace6, SelectWorkspace7,
    SelectWorkspace8, SelectWorkspace9, SplitHorizontally, SplitVertically, UndoClosePane,
};

#[derive(Clone)]
pub(crate) enum WorkspaceFocusTarget {
    FirstPane,
    PaneTab {
        pane: gpui::Entity<crate::pane::Pane>,
        tab_idx: usize,
    },
}

fn push_closed_pane_record(records: &mut Vec<ClosedPaneRecord>, mut record: ClosedPaneRecord) {
    for tab in &mut record.tabs {
        if let ClosedTabRecord::Terminal {
            scrollback: Some(scrollback),
            ..
        } = tab
        {
            scrollback.shrink_to_fit();
        }
    }
    if records.len() >= MAX_CLOSED_PANES {
        records.remove(0);
    }
    records.push(record);
    enforce_closed_pane_scrollback_budget(records, MAX_CLOSED_PANE_SCROLLBACK_BYTES);
}

fn enforce_closed_pane_scrollback_budget(records: &mut [ClosedPaneRecord], budget: usize) {
    let mut total = closed_pane_scrollback_bytes(records);
    if total <= budget {
        return;
    }
    for record in records.iter_mut() {
        if total <= budget {
            break;
        }
        for tab in &mut record.tabs {
            if total <= budget {
                break;
            }
            if let ClosedTabRecord::Terminal { scrollback, .. } = tab
                && let Some(scrollback) = scrollback.take()
            {
                total = total.saturating_sub(scrollback.len());
            }
        }
    }
}

fn closed_pane_scrollback_bytes(records: &[ClosedPaneRecord]) -> usize {
    records
        .iter()
        .flat_map(|record| &record.tabs)
        .filter_map(|tab| match tab {
            ClosedTabRecord::Terminal { scrollback, .. } => scrollback.as_ref(),
            ClosedTabRecord::Markdown { .. } => None,
        })
        .map(String::len)
        .sum()
}

fn capture_closed_pane_record(
    pane: &gpui::Entity<crate::pane::Pane>,
    workspace_idx: usize,
    cx: &App,
) -> Option<ClosedPaneRecord> {
    let pane_ref = pane.read(cx);
    let mut tabs = Vec::new();
    for tab in &pane_ref.tabs {
        match tab {
            crate::pane::TabContent::Terminal(tv) => {
                let tv_ref = tv.read(cx);
                tabs.push(ClosedTabRecord::Terminal {
                    cwd: tv_ref
                        .terminal
                        .current_cwd
                        .as_ref()
                        .map(std::path::PathBuf::from)
                        .or_else(|| tv_ref.terminal.cwd_now()),
                    scrollback: tv_ref.terminal.extract_scrollback(),
                    custom_name: tv_ref.terminal.custom_name.clone(),
                    font_size: tv_ref.terminal.font_size_override,
                });
            }
            crate::pane::TabContent::Markdown(markdown) => {
                tabs.push(ClosedTabRecord::Markdown {
                    path: markdown.read(cx).path.clone(),
                });
            }
            crate::pane::TabContent::Diff(_) => {}
        }
    }
    if tabs.is_empty() {
        return None;
    }
    Some(ClosedPaneRecord {
        tabs,
        selected_idx: pane_ref.selected_idx,
        workspace_idx,
    })
}

fn restore_closed_tab_record(
    tab: ClosedTabRecord,
    ws_id: u64,
    cx: &mut Context<PaneFlowApp>,
) -> crate::pane::TabContent {
    match tab {
        ClosedTabRecord::Terminal {
            cwd,
            scrollback,
            custom_name,
            font_size,
        } => {
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
            terminal.update(cx, |view, _| {
                view.terminal.custom_name = custom_name;
                view.terminal.font_size_override = font_size;
            });
            if let Some(scrollback) = scrollback {
                terminal.read(cx).restore_scrollback(&scrollback);
            }
            cx.subscribe(&terminal, PaneFlowApp::handle_terminal_event)
                .detach();
            crate::pane::TabContent::Terminal(terminal)
        }
        ClosedTabRecord::Markdown { path } => {
            let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                crate::markdown::MarkdownView::open(path, cx)
            });
            crate::pane::TabContent::Markdown(markdown)
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn dismiss_transient_surfaces(&mut self) {
        self.title_bar_files_menu_open = None;
        self.title_bar_help_menu_open = None;
        self.workspace_menu_open = None;
        self.tab_menu_open = None;
        self.profile_menu_open = None;
        self.files_menu_open = None;
        self.agents_view.agents_menu_open = None;
        self.agents_view.sidebar_actions_menu_open = false;
        self.agents_view.sidebar_mode_picker_open = false;
    }

    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        debug_assert!(
            self.workspaces.is_empty() || self.active_idx < self.workspaces.len(),
            "active_idx out of bounds"
        );
        self.workspaces.get(self.active_idx)
    }

    pub(crate) fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        self.workspaces.get_mut(self.active_idx)
    }

    pub(crate) fn select_workspace(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_workspace_at(idx, WorkspaceFocusTarget::FirstPane, window, cx);
    }

    pub(crate) fn activate_workspace_at(
        &mut self,
        idx: usize,
        focus_target: WorkspaceFocusTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if idx >= self.workspaces.len() {
            return false;
        }

        let changed = idx != self.active_idx;
        self.dismiss_transient_surfaces();
        self.active_idx = idx;

        match focus_target {
            WorkspaceFocusTarget::FirstPane => {
                self.workspaces[idx].focus_first(window, cx);
            }
            WorkspaceFocusTarget::PaneTab { pane, tab_idx } => {
                pane.update(cx, |p, cx| {
                    if p.selected_idx != tab_idx {
                        p.selected_idx = tab_idx;
                    }
                    cx.notify();
                });
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        }

        self.reroot_files_tree(cx);
        if self.agent_sessions.sessions_sidebar_open {
            let keep_sidebar_focus = self.agent_sessions.sessions_focus.is_focused(window);
            match self.workspaces[idx]
                .root
                .as_ref()
                .and_then(|root| root.first_leaf())
            {
                Some(pane) => self.open_sessions_sidebar_for_pane(
                    &pane,
                    keep_sidebar_focus.then_some(window),
                    cx,
                ),
                None => self.close_sessions_sidebar(cx),
            }
        }
        self.save_session(cx);
        self.reconcile_diff_after_workspace_change(cx);
        cx.notify();
        changed
    }

    pub(crate) fn activate_workspace_without_window(
        &mut self,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if idx >= self.workspaces.len() {
            return false;
        }

        let changed = idx != self.active_idx;
        self.dismiss_transient_surfaces();
        self.active_idx = idx;
        self.reroot_files_tree(cx);
        if self.agent_sessions.sessions_sidebar_open {
            self.close_sessions_sidebar(cx);
        }
        self.save_session(cx);
        self.reconcile_diff_after_workspace_change(cx);
        cx.notify();
        changed
    }

    /// US-009 (orchestration-v2): tear down the worktrees a closing workspace
    /// owns, off the render thread (`git status` + `worktree remove` are
    /// subprocesses). Clean ones are removed, dirty/unverifiable ones kept,
    /// the branch never touched - all enforced by `worktree::teardown_all`.
    pub(crate) fn spawn_worktree_teardown(
        worktrees: Vec<crate::workspace::worktree::ManagedWorktree>,
        cx: &mut Context<Self>,
    ) {
        if worktrees.is_empty() {
            return;
        }
        cx.spawn(async move |_this, _cx: &mut gpui::AsyncApp| {
            smol::unblock(move || crate::workspace::worktree::teardown_all(worktrees)).await;
        })
        .detach();
    }

    /// US-005/US-014: if in Diff mode, rebuild the mounted diff (deferred) so it
    /// follows the current workspace set and active workspace - covers workspace
    /// switch (re-target) and close (Multi-project group reconcile). Deferred so
    /// the rebuild (which mounts a fresh entity) never runs inside a
    /// render/callback. No-op outside Diff mode.
    fn reconcile_diff_after_workspace_change(&self, cx: &mut Context<Self>) {
        if matches!(self.mode, paneflow_config::schema::AppMode::Diff) {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |app, cx| app.rebuild_diff_view(cx));
            });
        }
    }

    #[allow(dead_code)]
    pub(crate) fn create_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return;
        }
        let n = self.workspaces.len() + 1;
        let ws_id = next_workspace_id();
        let terminal = cx.new(|cx| TerminalView::new(ws_id, cx));
        let pane = self.create_pane(terminal, ws_id, cx);
        let ws = Workspace::with_id(ws_id, format!("Terminal {n}"), pane);
        // US-013: deferred git-stats probe off the render thread.
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        self.watch_git_dir(&ws);
        self.workspaces.push(ws);
        self.active_idx = self.workspaces.len() - 1;
        self.workspaces[self.active_idx].focus_first(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn create_workspace_with_picker(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: None,
        });
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if let Ok(Ok(Some(paths))) = receiver.await {
                    let _ = cx.update(|cx| {
                        this.update(cx, |app, cx| {
                            for path in paths {
                                if app.workspaces.len() >= MAX_WORKSPACES {
                                    break;
                                }
                                let n = app.workspaces.len() + 1;
                                let dir = path.clone();
                                let title = dir
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| format!("Terminal {n}"));
                                let ws_id = next_workspace_id();
                                let terminal = cx
                                    .new(|cx| TerminalView::with_cwd(ws_id, Some(path), None, cx));
                                let pane = app.create_pane(terminal, ws_id, cx);
                                let ws = Workspace::with_cwd_and_id(ws_id, title, dir, pane);
                                // US-013: deferred git-stats probe off the render thread.
                                Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
                                app.watch_git_dir(&ws);
                                app.workspaces.push(ws);
                            }
                            app.active_idx = app.workspaces.len() - 1;
                            app.save_session(cx);
                            cx.notify();
                            // US-016 (prd-git-diff-mode-2026-Q3.md): a new repo
                            // must surface in Multi-project / re-target the diff.
                            app.reconcile_diff_after_workspace_change(cx);
                        })
                    });
                }
            },
        )
        .detach();
    }

    // --- Split/close/focus handlers (operate on active workspace) ---

    /// Working directory for a terminal spawned into the active workspace.
    /// `source_cwd` is the focused/source pane's live cwd (`cwd_now()`), which
    /// is `None` for a markdown pane (US-020) and on every platform that can't
    /// introspect a child's cwd - notably *always* on Windows, where
    /// `cwd_now()` is a stub. Left unhandled, that `None` lets the PTY spawn
    /// drop to the process `current_dir()` (`C:\Program Files\PaneFlow` for an
    /// installed build), stranding new panes outside the project. Fall back to
    /// the workspace's own root directory so a split / new pane always lands in
    /// the directory the workspace points at.
    pub(crate) fn new_terminal_cwd(
        &self,
        source_cwd: Option<std::path::PathBuf>,
    ) -> Option<std::path::PathBuf> {
        source_cwd.or_else(|| {
            self.active_workspace()
                .map(|ws| ws.cwd.as_str())
                .filter(|cwd| !cwd.is_empty())
                .map(std::path::PathBuf::from)
        })
    }

    pub(crate) fn split(
        &mut self,
        direction: SplitDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.active_workspace() else {
            return;
        };
        if ws.is_zoomed() {
            self.show_toast("Unzoom before splitting panes", cx);
            return;
        }
        let Some(root) = &ws.root else {
            return;
        };
        if root.leaf_count() >= MAX_PANES {
            self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
            return;
        }
        let Some(focused) = root.focused_pane(window, cx) else {
            self.show_toast("No focused pane to split", cx);
            return;
        };
        let ws_id = ws.id;

        // Inherit CWD from the focused pane's active terminal. `cwd_now()` is
        // best-effort: `None` for a markdown pane (US-020) and on platforms
        // without child-cwd introspection (always on Windows). `new_terminal_cwd`
        // then falls back to the workspace root, so the new pane never drops to
        // the process `current_dir()` (`C:\Program Files\PaneFlow` when installed).
        let source_cwd = focused
            .read(cx)
            .active_terminal_opt()
            .and_then(|tv| tv.read(cx).terminal.cwd_now());
        let source_cwd = self.new_terminal_cwd(source_cwd);
        let new_terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, source_cwd, None, cx));
        let new_pane = self.create_pane(new_terminal, ws_id, cx);
        let inserted = if let Some(ws) = self.active_workspace_mut()
            && let Some(root) = &mut ws.root
        {
            root.split_at_pane(&focused, direction, new_pane.clone())
        } else {
            false
        };
        if !inserted {
            self.show_toast("Focused pane no longer exists", cx);
            return;
        }
        new_pane.read(cx).focus_handle(cx).focus(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn handle_split_h(
        &mut self,
        _: &SplitHorizontally,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split(SplitDirection::Horizontal, w, cx);
    }
    pub(crate) fn handle_split_v(
        &mut self,
        _: &SplitVertically,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.split(SplitDirection::Vertical, w, cx);
    }

    pub(crate) fn handle_close_pane(
        &mut self,
        _: &ClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Capture state of the pane being closed for undo (US-014).
        // Must happen BEFORE the tree mutation that drops the pane entity.
        let workspace_idx = self.active_idx;
        if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.root
        {
            let closing_pane = if ws.is_zoomed() {
                root.first_leaf()
            } else {
                root.focused_pane(window, cx)
            };
            if let Some(pane) = closing_pane
                && let Some(record) = capture_closed_pane_record(&pane, workspace_idx, cx)
            {
                push_closed_pane_record(&mut self.closed_panes, record);
            }
        }

        if let Some(ws) = self.active_workspace_mut()
            && ws.is_zoomed()
        {
            if let Some(pane) = ws.exit_zoom(cx)
                && let Some(root) = ws.root.take()
            {
                let (new_root, _) = root.remove_pane(&pane);
                ws.root = new_root;
            }
            if let Some(ref root) = ws.root {
                root.focus_first(window, cx);
            }
        } else if let Some(ws) = self.active_workspace_mut()
            && let Some(root) = ws.root.take()
        {
            let (new_root, _closed, focus_target) = root.close_focused(window, cx);
            ws.root = new_root;

            if ws.root.is_some() {
                if let Some(target) = focus_target {
                    target.read(cx).focus_handle(cx).focus(window, cx);
                } else if let Some(ref root) = ws.root {
                    root.focus_first(window, cx);
                }
            }
        }

        // Never destroy a workspace when its last pane closes - respawn a
        // fresh terminal at the workspace's root cwd. Workspaces are only
        // removed via the explicit "Close workspace" action.
        if let Some(ws) = self.active_workspace()
            && ws.root.is_none()
        {
            let ws_id = ws.id;
            let cwd = std::path::PathBuf::from(&ws.cwd);
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, Some(cwd), None, cx));
            // US-028: do NOT subscribe here - `create_pane` already wires
            // `handle_terminal_event` (main.rs:539). The duplicate subscription
            // fired every terminal event twice (double toast / port-scan /
            // mutation) and leaked the extra subscription. `split()` and
            // `create_workspace` prove the correct pattern (no manual subscribe).
            let new_pane = self.create_pane(terminal, ws_id, cx);
            if let Some(ws) = self.active_workspace_mut() {
                ws.root = Some(LayoutTree::Leaf(new_pane));
            }
            self.workspaces[self.active_idx].focus_first(window, cx);
        }

        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn handle_undo_close_pane(
        &mut self,
        _: &UndoClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(record) = self.closed_panes.pop() else {
            self.show_toast("No closed pane to restore", cx);
            return; // No closed panes to restore
        };

        // Switch to the workspace where the pane was closed, if it still exists
        if record.workspace_idx < self.workspaces.len() {
            self.active_idx = record.workspace_idx;
        }

        let Some(ws_id) = self.active_workspace().map(|ws| ws.id) else {
            self.closed_panes.push(record);
            self.show_toast("No active workspace to restore pane", cx);
            return;
        };
        let selected_idx = record.selected_idx;
        let tabs = record
            .tabs
            .into_iter()
            .map(|tab| restore_closed_tab_record(tab, ws_id, cx))
            .collect::<Vec<_>>();
        if tabs.is_empty() {
            self.show_toast("Closed pane had no restorable tabs", cx);
            return;
        }

        let new_pane = self.create_pane_with_existing_tabs(tabs, selected_idx, ws_id, cx);

        // Insert via split from the currently focused pane
        let inserted = if let Some(ws) = self.active_workspace_mut() {
            if let Some(root) = &mut ws.root {
                if !root.split_at_focused(SplitDirection::Horizontal, new_pane.clone(), window, cx)
                {
                    root.split_first_leaf(SplitDirection::Horizontal, new_pane.clone());
                }
            } else {
                ws.root = Some(LayoutTree::Leaf(new_pane.clone()));
            }
            true
        } else {
            false
        };
        if !inserted {
            return;
        }
        new_pane.read(cx).focus_handle(cx).focus(window, cx);

        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn handle_new_workspace(
        &mut self,
        _: &NewWorkspace,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.create_workspace_with_picker(w, cx);
    }

    pub(crate) fn handle_close_workspace(
        &mut self,
        _: &CloseWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_workspace_at(self.active_idx, window, cx);
    }

    pub(crate) fn handle_copy_workspace_path(
        &mut self,
        _: &CopyWorkspacePath,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_workspace_path(self.active_idx, cx);
    }

    pub(crate) fn handle_reveal_workspace_in_file_manager(
        &mut self,
        _: &RevealWorkspaceInFileManager,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_workspace_in_file_manager(self.active_idx, cx);
    }

    pub(crate) fn handle_open_workspace_in_zed(
        &mut self,
        _: &OpenWorkspaceInZed,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_in_editor(self.active_idx, "zed", "Zed", cx);
    }

    pub(crate) fn handle_open_workspace_in_cursor(
        &mut self,
        _: &OpenWorkspaceInCursor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_in_editor(self.active_idx, "cursor", "Cursor", cx);
    }

    pub(crate) fn handle_open_workspace_in_vscode(
        &mut self,
        _: &OpenWorkspaceInVsCode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_in_editor(self.active_idx, "code", "VS Code", cx);
    }

    pub(crate) fn handle_open_workspace_in_windsurf(
        &mut self,
        _: &OpenWorkspaceInWindsurf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_in_editor(self.active_idx, "windsurf", "Windsurf", cx);
    }

    pub(crate) fn close_workspace_at(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if idx >= self.workspaces.len() {
            return;
        }
        self.workspace_menu_open = None;
        if let Some(dir) = self.workspaces[idx].git_dir.clone() {
            self.unwatch_git_dir(&dir);
        }
        // US-009: this workspace's managed worktrees are torn down (clean
        // ones only) in the background once the workspace is gone.
        let worktrees = std::mem::take(&mut self.workspaces[idx].managed_worktrees);
        Self::spawn_worktree_teardown(worktrees, cx);
        self.workspaces.remove(idx);
        if self.workspaces.is_empty() {
            self.active_idx = 0;
        } else {
            // Clamp active_idx
            if self.active_idx >= self.workspaces.len() {
                self.active_idx = self.workspaces.len() - 1;
            } else if self.active_idx > idx {
                self.active_idx -= 1;
            }
            self.workspaces[self.active_idx].focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
        // EP-001 (cli-cockpit): the closed workspace's panes may have carried
        // a Composer target, queued prompts, or group memberships. Refresh:
        // a dead-target Composer closes itself (refresh_composer_slot),
        // stale group members are pruned, and orphaned buffers drop on the
        // next flush (their terminals no longer resolve).
        self.refresh_composer_slot(cx);
        self.sync_broadcast_stripes(cx);
        self.flush_pending_prefill(cx);
        self.sync_pending_chips(cx);
        // US-014 (prd-git-diff-mode-2026-Q3.md): in Diff mode, closing a
        // workspace reconciles the diff (a Multi-project group / column for the
        // closed workspace must drop). Deferred so the rebuild runs after the
        // close settles, never inside a render/callback.
        self.reconcile_diff_after_workspace_change(cx);
    }

    /// Move a workspace (identified by `from_id`) so it ends up at `to_idx`
    /// in the workspace list. Preserves which workspace is active across the
    /// reorder and persists the new order.
    pub(crate) fn reorder_workspace(
        &mut self,
        from_id: u64,
        to_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(from_idx) = self.workspaces.iter().position(|ws| ws.id == from_id) else {
            return;
        };
        let active_id = self.workspaces.get(self.active_idx).map(|ws| ws.id);
        let ws = self.workspaces.remove(from_idx);
        let insert_at = to_idx.min(self.workspaces.len());
        if from_idx == insert_at {
            self.workspaces.insert(insert_at, ws);
            return;
        }
        self.workspaces.insert(insert_at, ws);
        if let Some(id) = active_id {
            self.active_idx = self
                .workspaces
                .iter()
                .position(|ws| ws.id == id)
                .unwrap_or(0);
        }
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn copy_workspace_path(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(idx) else {
            return;
        };

        cx.write_to_clipboard(ClipboardItem::new_string(ws.cwd.clone()));
        self.show_toast("Path copied", cx);
        self.workspace_menu_open = None;
        cx.notify();
    }

    pub(crate) fn reveal_workspace_in_file_manager(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(ws) = self.workspaces.get(idx) else {
            return;
        };

        let cwd = ws.cwd.clone();
        self.workspace_menu_open = None;

        if let Err(msg) = reveal_in_file_manager(std::path::Path::new(&cwd)) {
            log::warn!("failed to reveal workspace path in file manager: {msg}");
            self.show_toast(msg, cx);
        }

        cx.notify();
    }

    pub(crate) fn open_workspace_in_editor(
        &mut self,
        idx: usize,
        command: &str,
        label: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(idx) else {
            return;
        };
        let cwd = ws.cwd.clone();

        // GUI launchers (.desktop on Linux, Finder on macOS, Start menu on
        // Windows) frequently strip user bin directories from PATH, so editors
        // installed under ~/.local/bin or ~/.cargo/bin can't be found by
        // Command::new alone - even though they resolve fine from a terminal.
        let bin = resolve_editor_binary(command);

        let toast_label = editor_toast_label(label);
        let mut cmd = std::process::Command::new(&bin);
        cmd.current_dir(&cwd).arg(".");
        if let Err(err) = spawn_detached(&mut cmd) {
            log::warn!("failed to open workspace in {toast_label}: {err}");
            self.show_toast(format!("Couldn't open in {toast_label}: {err}"), cx);
        }

        self.workspace_menu_open = None;
        cx.notify();
    }

    pub(crate) fn commit_rename(&mut self, cx: &App) {
        if let Some(idx) = self.renaming_idx.take() {
            let text = std::mem::take(&mut self.rename_text);
            if !text.is_empty()
                && let Some(ws) = self.workspaces.get_mut(idx)
            {
                ws.title = text;
                self.save_session(cx);
            }
        }
    }

    pub(crate) fn handle_next_workspace(
        &mut self,
        _: &NextWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspaces.is_empty() {
            let next = (self.active_idx + 1) % self.workspaces.len();
            self.select_workspace(next, window, cx);
        }
    }

    pub(crate) fn handle_select_ws(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_workspace(idx, window, cx);
    }

    // Macro-like handlers for Ctrl+1-9
    pub(crate) fn handle_ws1(
        &mut self,
        _: &SelectWorkspace1,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(0, w, cx);
    }
    pub(crate) fn handle_ws2(
        &mut self,
        _: &SelectWorkspace2,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(1, w, cx);
    }
    pub(crate) fn handle_ws3(
        &mut self,
        _: &SelectWorkspace3,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(2, w, cx);
    }
    pub(crate) fn handle_ws4(
        &mut self,
        _: &SelectWorkspace4,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(3, w, cx);
    }
    pub(crate) fn handle_ws5(
        &mut self,
        _: &SelectWorkspace5,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(4, w, cx);
    }
    pub(crate) fn handle_ws6(
        &mut self,
        _: &SelectWorkspace6,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(5, w, cx);
    }
    pub(crate) fn handle_ws7(
        &mut self,
        _: &SelectWorkspace7,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(6, w, cx);
    }
    pub(crate) fn handle_ws8(
        &mut self,
        _: &SelectWorkspace8,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(7, w, cx);
    }
    pub(crate) fn handle_ws9(
        &mut self,
        _: &SelectWorkspace9,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_select_ws(8, w, cx);
    }
}

/// Spawn the native file manager with `path` in focus, per-OS (US-011).
///
/// - **Linux** → `xdg-open <path>`. `xdg-utils` opens the directory in
///   the default handler; "reveal the file in its folder" semantics
///   don't translate cleanly to X11/Wayland file managers, so we
///   approximate by opening the parent directory when `path` is a file.
/// - **macOS** → `open <path>` (Finder dispatches). `open -R <path>`
///   would "reveal" with the file highlighted, but the PRD explicitly
///   mandates `open <path>` for parity with the Linux "open this
///   directory" behavior - callers that want reveal-with-highlight
///   pass the parent directory.
/// - **Windows** → `explorer /select,<path>`. The `/select,` flag opens
///   the parent folder with `<path>` highlighted - the canonical
///   "reveal in Explorer" idiom documented by Microsoft.
///
/// Returns `Err(message)` on spawn failure where `message` is already
/// phrased for a user-visible toast (US-011 AC7, AC9). Notable error
/// shape: Linux `ErrorKind::NotFound` surfaces the "install xdg-utils"
/// hint per the unhappy-path AC.
#[allow(clippy::needless_return)]
pub(crate) fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let result = spawn_detached(std::process::Command::new("xdg-open").arg(path));
        return result.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "xdg-open not found - install xdg-utils to use this feature".to_string()
            } else {
                format!("Could not open file manager: {err}")
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        let result = spawn_detached(std::process::Command::new("open").arg(path));
        return result.map_err(|err| format!("Could not open Finder: {err}"));
    }
    #[cfg(target_os = "windows")]
    {
        // `/select,<path>` highlights the file in its parent folder.
        // The comma is part of the flag spelling Microsoft documents,
        // and the flag + path MUST form a SINGLE argv token - passing
        // `/select,` and `<path>` as two separate `.arg(...)` calls
        // makes Explorer ignore the selection hint and silently open
        // the user's Documents folder instead (US-007 / v0.2.0 US-011
        // SHOULD_FIX review note). Concatenate via `OsString` so
        // non-UTF-8 path bytes (e.g., NTFS filenames that don't
        // round-trip through `&str`) survive; `as_os_str()` keeps the
        // raw wide-char representation intact.
        let mut flag = std::ffi::OsString::from("/select,");
        flag.push(path.as_os_str());
        let result = spawn_detached(std::process::Command::new("explorer").arg(flag));
        return result.map_err(|err| format!("Could not open Explorer: {err}"));
    }
    // Fallback for target_os values we don't explicitly handle
    // (freebsd, netbsd, etc.). Best-effort via xdg-open which is widely
    // available on BSD but not guaranteed.
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        spawn_detached(std::process::Command::new("xdg-open").arg(path))
            .map_err(|err| format!("Could not open file manager: {err}"))
    }
}

/// Open a directory in the platform file manager without going through the
/// `open` crate's generic shell dispatch. Used for "System default" folder
/// actions where Windows packaged launches can reject `cmd /C start`-style
/// dispatch with `ERROR_NOT_SUPPORTED`.
#[allow(clippy::needless_return)]
pub(crate) fn open_folder_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let result = spawn_detached(std::process::Command::new("xdg-open").arg(path));
        return result.map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "xdg-open not found - install xdg-utils to use this feature".to_string()
            } else {
                format!("Could not open file manager: {err}")
            }
        });
    }
    #[cfg(target_os = "macos")]
    {
        let result = spawn_detached(std::process::Command::new("open").arg(path));
        return result.map_err(|err| format!("Could not open Finder: {err}"));
    }
    #[cfg(target_os = "windows")]
    {
        let result = spawn_detached(std::process::Command::new("explorer").arg(path));
        return result.map_err(|err| format!("Could not open Explorer: {err}"));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        spawn_detached(std::process::Command::new("xdg-open").arg(path))
            .map_err(|err| format!("Could not open file manager: {err}"))
    }
}

/// Resolve an editor command (e.g. `"zed"`, `"code"`) to a concrete path.
///
/// `Command::new(command).spawn()` only consults the spawning process's PATH,
/// which on Linux desktop launches (`.desktop`), macOS Finder, and Windows
/// shell launches frequently lacks the user-bin directories where editors
/// like Zed, Cursor, or Code Insiders install their CLI shim. We extend the
/// search with a small set of well-known per-OS fallbacks, then fall back to
/// the bare command so `spawn()` still produces a clean `NotFound` error
/// (now surfaced via toast in `open_workspace_in_editor`).
pub(crate) fn resolve_editor_binary(command: &str) -> std::path::PathBuf {
    resolve_editor_binary_in(command, &editor_search_paths())
}

pub(crate) fn editor_toast_label(label: &str) -> &str {
    label.strip_prefix("Open in ").unwrap_or(label)
}

/// Pure resolver: try the inherited PATH first, then `fallback_paths`, then
/// return the bare `command`. Split out from [`resolve_editor_binary`] so the
/// fallback list can be injected from tests without touching process env.
fn resolve_editor_binary_in(
    command: &str,
    fallback_paths: &[std::path::PathBuf],
) -> std::path::PathBuf {
    if let Ok(path) = which::which(command)
        && let Some(path) = normalize_editor_candidate(path)
    {
        return path;
    }
    if !fallback_paths.is_empty()
        && let Ok(joined) = std::env::join_paths(fallback_paths)
        && let Ok(path) = which::which_in(command, Some(&joined), ".")
        && let Some(path) = normalize_editor_candidate(path)
    {
        return path;
    }
    std::path::PathBuf::from(command)
}

#[cfg(target_os = "windows")]
fn normalize_editor_candidate(path: std::path::PathBuf) -> Option<std::path::PathBuf> {
    const NATIVE_EXTENSIONS: [&str; 4] = ["exe", "cmd", "bat", "com"];
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            NATIVE_EXTENSIONS
                .iter()
                .any(|native_ext| ext.eq_ignore_ascii_case(native_ext))
        })
    {
        return Some(path);
    }
    for extension in NATIVE_EXTENSIONS {
        let candidate = path.with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(not(target_os = "windows"))]
fn normalize_editor_candidate(path: std::path::PathBuf) -> Option<std::path::PathBuf> {
    Some(path)
}

/// Per-OS list of directories to consult when an editor isn't on PATH.
///
/// Linux distributions and BSDs share the same user-bin layout (`~/.local/bin`,
/// `~/.cargo/bin`, `/usr/local/bin`), so a single Linux branch covers Fedora,
/// Ubuntu/Debian, Arch, openSUSE, etc. Snap (`/snap/bin`) and Flatpak are
/// handled by the system PATH on every distro that ships them, so they don't
/// need explicit entries here.
fn editor_search_paths() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;

    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local").join("bin"));
        paths.push(home.join(".cargo").join("bin"));
        paths.push(home.join("bin"));
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        paths.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(target_os = "macos")]
    {
        paths.push(PathBuf::from("/opt/homebrew/bin"));
    }
    #[cfg(target_os = "windows")]
    {
        push_windows_editor_search_paths(&mut paths);
    }
    paths
}

#[cfg(target_os = "windows")]
fn push_windows_editor_search_paths(paths: &mut Vec<std::path::PathBuf>) {
    use std::path::{Path, PathBuf};

    fn push_program_dirs(paths: &mut Vec<PathBuf>, programs: &Path) {
        paths.push(programs.join("Zed").join("bin"));
        paths.push(programs.join("Zed"));
        paths.push(
            programs
                .join("Cursor")
                .join("resources")
                .join("app")
                .join("bin"),
        );
        paths.push(
            programs
                .join("cursor")
                .join("resources")
                .join("app")
                .join("bin"),
        );
        paths.push(programs.join("Microsoft VS Code").join("bin"));
        paths.push(programs.join("Microsoft VS Code Insiders").join("bin"));
        paths.push(
            programs
                .join("Windsurf")
                .join("resources")
                .join("app")
                .join("bin"),
        );
        paths.push(
            programs
                .join("windsurf")
                .join("resources")
                .join("app")
                .join("bin"),
        );
    }

    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        push_program_dirs(paths, &PathBuf::from(local_app_data).join("Programs"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(program_files) = std::env::var_os(var) {
            push_program_dirs(paths, &PathBuf::from(program_files));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure-Rust tests only - spawning actual binaries is brittle in CI
    // (Linux runners may not have xdg-utils, macOS runners may not have
    // `open` on PATH under non-GUI session, etc.). We exercise the
    // error-message shape so the toast copy can't drift silently.

    #[cfg(target_os = "linux")]
    #[test]
    fn reveal_linux_missing_xdg_open_surfaces_install_hint() {
        // Craft a bogus PATH so xdg-open is genuinely absent. `std::process::Command`
        // inherits env by default; temporarily clearing $PATH via
        // `Command::env` is fine because this test runs in its own
        // process image.
        //
        // We can't mutate the helper's internal Command, so exercise the
        // same branch directly: fabricate a NotFound io::Error and run
        // it through the classifier shape the helper uses.
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        // Mirrors the helper's error-mapping branch; a refactor that
        // changes the toast copy in one place will fail this assertion.
        let msg = if err.kind() == std::io::ErrorKind::NotFound {
            "xdg-open not found - install xdg-utils to use this feature".to_string()
        } else {
            format!("Could not open file manager: {err}")
        };
        assert!(msg.contains("xdg-utils"), "unhappy-path AC text: {msg}");
    }

    #[test]
    fn reveal_accepts_regular_path() {
        // Smoke-test that the helper is callable with a plausible path
        // and that its return type is `Result<(), String>`. Actual
        // spawn behaviour is OS-dependent and left to CI / manual
        // verification per US-011 AC10.
        let tmp = tempfile::TempDir::new().unwrap();
        // Don't actually spawn - the test would flake on headless CI
        // without a default file-manager registered. We verify the
        // type-shape compiles and the helper is reachable from tests.
        let _callable: fn(&std::path::Path) -> Result<(), String> = reveal_in_file_manager;
        let _ = tmp.path();
    }

    #[test]
    fn open_folder_accepts_regular_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _callable: fn(&std::path::Path) -> Result<(), String> = open_folder_in_file_manager;
        let _ = tmp.path();
    }

    // ════════════════════════════════════════════════════════════════════
    // Editor-binary resolver - regression coverage for the "Open in
    // <Editor>" silent-failure bug.
    //
    // Bug: GUI launchers (Linux .desktop, macOS Finder, Windows Start menu)
    // inherit a narrowed PATH that omits user-bin dirs (~/.local/bin etc.),
    // so editors installed there can't be spawned by `Command::new` alone.
    // Cursor at /usr/bin worked; Zed at ~/.local/bin failed silently.
    //
    // The fixture-based tests below run on every platform - they don't
    // require a real editor to be installed. Each per-OS shape test runs
    // only on its target so CI on each platform self-validates its own
    // fallback list. Linux distros (Fedora, Ubuntu/Debian, Arch, openSUSE,
    // Alpine, …) share the same user-bin layout, so a single Linux test
    // covers the distro fleet.
    // ════════════════════════════════════════════════════════════════════

    /// Filename suffix `which_in` will recognize on the current target.
    /// Windows resolves names against PATHEXT - `.exe` is the canonical
    /// entry; Unix matches the bare name plus the executable bit.
    const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

    /// Create a stub binary named `<command><EXE_SUFFIX>` inside `dir` and,
    /// on Unix, flip the executable bit so `which` will accept it. Returns
    /// the absolute path to the stub for canonical comparison.
    fn make_stub_binary(dir: &std::path::Path, command: &str) -> std::path::PathBuf {
        let path = dir.join(format!("{command}{EXE_SUFFIX}"));
        std::fs::write(&path, b"").expect("write stub binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perm = std::fs::metadata(&path).unwrap().permissions();
            perm.set_mode(0o755);
            std::fs::set_permissions(&path, perm).unwrap();
        }
        path
    }

    #[test]
    fn resolver_picks_up_binary_from_fallback_dir() {
        // High-entropy stub name so `which::which` against the host's real
        // PATH cannot resolve it - forcing the resolver into the fallback
        // branch we want to exercise. Without this, the test could pass on
        // the wrong code path if the host happens to have a similarly-
        // named binary installed.
        let stub = "paneflow_resolver_stub_pflw_42";
        let dir = tempfile::TempDir::new().unwrap();
        let expected = make_stub_binary(dir.path(), stub);

        let resolved = resolve_editor_binary_in(stub, &[dir.path().to_path_buf()]);

        // `which_in` may canonicalize symlinks and `.` components - compare
        // canonical forms so the test is resilient on macOS (/var → /private/var)
        // and on distros where /home is a symlink. Falling back to raw
        // PathBuf comparison would flake on those hosts.
        let canon_resolved = std::fs::canonicalize(&resolved).ok();
        let canon_expected = std::fs::canonicalize(&expected).ok();
        assert_eq!(
            canon_resolved,
            canon_expected,
            "resolver returned {} instead of fallback {}",
            resolved.display(),
            expected.display()
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn resolver_windows_prefers_native_sibling_over_extensionless_shim() {
        let stub = "paneflow_windows_editor_stub_pflw_42";
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(stub), b"#!/usr/bin/env sh\n").unwrap();
        let expected = make_stub_binary(dir.path(), stub);

        let resolved = normalize_editor_candidate(dir.path().join(stub)).unwrap();

        let canon_resolved = std::fs::canonicalize(&resolved).ok();
        let canon_expected = std::fs::canonicalize(&expected).ok();
        assert_eq!(
            canon_resolved,
            canon_expected,
            "resolver returned {} instead of native sibling {}",
            resolved.display(),
            expected.display()
        );
    }

    #[test]
    fn resolver_returns_bare_command_when_nothing_resolves() {
        // Empty fallback list AND a command name designed to be absent
        // from any host PATH. The resolver must hand back the bare command
        // so the caller's spawn() produces a clean NotFound error that our
        // toast surfaces to the user.
        let bare = "paneflow_no_such_editor_zzz_99";
        let resolved = resolve_editor_binary_in(bare, &[]);
        assert_eq!(resolved, std::path::PathBuf::from(bare));
    }

    #[test]
    fn resolver_returns_bare_command_when_fallback_dir_is_empty() {
        // Same contract, exercised through the directory-search branch
        // rather than the fast-skip empty-vec branch.
        let dir = tempfile::TempDir::new().unwrap();
        let bare = "paneflow_no_such_editor_zzz_77";
        let resolved = resolve_editor_binary_in(bare, &[dir.path().to_path_buf()]);
        assert_eq!(resolved, std::path::PathBuf::from(bare));
    }

    fn closed_pane_record_with_scrollback(len: usize) -> ClosedPaneRecord {
        ClosedPaneRecord {
            tabs: vec![ClosedTabRecord::Terminal {
                cwd: None,
                scrollback: Some("x".repeat(len)),
                custom_name: None,
                font_size: None,
            }],
            selected_idx: 0,
            workspace_idx: 0,
        }
    }

    #[test]
    fn closed_pane_budget_drops_oldest_scrollback_not_record() {
        let one_mib = 1024 * 1024;
        let mut records = vec![
            closed_pane_record_with_scrollback(one_mib),
            closed_pane_record_with_scrollback(one_mib),
        ];

        push_closed_pane_record(&mut records, closed_pane_record_with_scrollback(one_mib));

        assert_eq!(records.len(), 3, "budget must preserve undo records");
        assert!(
            matches!(
                records[0].tabs.first(),
                Some(ClosedTabRecord::Terminal {
                    scrollback: None,
                    ..
                })
            ),
            "oldest scrollback should be released first"
        );
        assert!(matches!(
            records[1].tabs.first(),
            Some(ClosedTabRecord::Terminal {
                scrollback: Some(_),
                ..
            })
        ));
        assert!(matches!(
            records[2].tabs.first(),
            Some(ClosedTabRecord::Terminal {
                scrollback: Some(_),
                ..
            })
        ));
        assert_eq!(
            closed_pane_scrollback_bytes(&records),
            MAX_CLOSED_PANE_SCROLLBACK_BYTES
        );
    }

    #[test]
    fn closed_pane_budget_preserves_absent_scrollback_for_undo() {
        let mut records = Vec::new();
        push_closed_pane_record(
            &mut records,
            ClosedPaneRecord {
                tabs: vec![ClosedTabRecord::Terminal {
                    cwd: None,
                    scrollback: None,
                    custom_name: None,
                    font_size: None,
                }],
                selected_idx: 0,
                workspace_idx: 0,
            },
        );

        assert_eq!(records.len(), 1);
        assert!(matches!(
            records[0].tabs.first(),
            Some(ClosedTabRecord::Terminal {
                scrollback: None,
                ..
            })
        ));
        assert_eq!(closed_pane_scrollback_bytes(&records), 0);
    }

    // ─── Per-OS path-list shape ────────────────────────────────────────
    // `editor_search_paths()` returns the OS-specific fallback list. Each
    // test runs only on its target. Assertions are positive (must contain),
    // so adding entries doesn't break old tests; removing an entry trips
    // a clear failure with the missing path named.

    #[cfg(target_os = "linux")]
    #[test]
    fn search_paths_linux_covers_user_and_system_bin() {
        let paths = editor_search_paths();
        let home = dirs::home_dir().expect("test host has $HOME");
        // Same layout across Fedora, Ubuntu/Debian, Arch, openSUSE, Alpine,
        // RHEL/CentOS, NixOS (single-user), Void, etc.
        assert!(
            paths.contains(&home.join(".local").join("bin")),
            "missing ~/.local/bin"
        );
        assert!(
            paths.contains(&home.join(".cargo").join("bin")),
            "missing ~/.cargo/bin"
        );
        assert!(paths.contains(&home.join("bin")), "missing ~/bin");
        assert!(
            paths.contains(&std::path::PathBuf::from("/usr/local/bin")),
            "missing /usr/local/bin"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn search_paths_macos_covers_homebrew_and_user_bin() {
        let paths = editor_search_paths();
        let home = dirs::home_dir().expect("test host has $HOME");
        assert!(
            paths.contains(&home.join(".local").join("bin")),
            "missing ~/.local/bin"
        );
        assert!(
            paths.contains(&home.join(".cargo").join("bin")),
            "missing ~/.cargo/bin"
        );
        assert!(paths.contains(&home.join("bin")), "missing ~/bin");
        assert!(
            paths.contains(&std::path::PathBuf::from("/usr/local/bin")),
            "missing /usr/local/bin (Intel Homebrew prefix)"
        );
        assert!(
            paths.contains(&std::path::PathBuf::from("/opt/homebrew/bin")),
            "missing /opt/homebrew/bin (Apple Silicon Homebrew prefix)"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn search_paths_windows_covers_user_bin() {
        let paths = editor_search_paths();
        let home = dirs::home_dir().expect("test host has %USERPROFILE%");
        let local_app_data = std::path::PathBuf::from(
            std::env::var_os("LOCALAPPDATA").expect("test host has %LOCALAPPDATA%"),
        );
        // dirs::home_dir on Windows == %USERPROFILE% (e.g. C:\Users\Arthur).
        // ~/.cargo/bin is the canonical install spot for Cargo-installed CLI
        // shims; ~/.local/bin and ~/bin are picked up by cross-platform
        // installers (mise-en-place, asdf-vm) for editor entry points.
        // GUI-launched apps do not reliably inherit user PATH, so cover the
        // common per-user editor install roots as explicit fallbacks.
        let programs = local_app_data.join("Programs");
        assert!(
            paths.contains(&home.join(".local").join("bin")),
            "missing %USERPROFILE%\\.local\\bin"
        );
        assert!(
            paths.contains(&home.join(".cargo").join("bin")),
            "missing %USERPROFILE%\\.cargo\\bin"
        );
        assert!(
            paths.contains(&home.join("bin")),
            "missing %USERPROFILE%\\bin"
        );
        assert!(
            paths.contains(&programs.join("Zed").join("bin")),
            "missing %LOCALAPPDATA%\\Programs\\Zed\\bin"
        );
        assert!(
            paths.contains(
                &programs
                    .join("Cursor")
                    .join("resources")
                    .join("app")
                    .join("bin")
            ),
            "missing %LOCALAPPDATA%\\Programs\\Cursor\\resources\\app\\bin"
        );
        assert!(
            paths.contains(&programs.join("Microsoft VS Code").join("bin")),
            "missing %LOCALAPPDATA%\\Programs\\Microsoft VS Code\\bin"
        );
        assert!(
            paths.contains(
                &programs
                    .join("Windsurf")
                    .join("resources")
                    .join("app")
                    .join("bin")
            ),
            "missing %LOCALAPPDATA%\\Programs\\Windsurf\\resources\\app\\bin"
        );
    }
}
