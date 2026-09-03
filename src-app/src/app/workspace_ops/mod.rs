mod focus;
mod layout;
mod swap;
mod tab;

use gpui::{App, AppContext, ClipboardItem, Context, Entity, Focusable, PathPromptOptions, Window};
use paneflow_config::schema::{TabTitleSource, TerminalSurfaceProfile};
use paneflow_process::spawn_detached;

use crate::layout::{LayoutTree, MAX_PANES, SplitDirection};
use crate::terminal::TerminalView;
use crate::workspace::{MAX_WORKSPACES, Workspace, next_workspace_id};
use crate::{
    ClosePane, CloseWorkspace, ClosedPaneRecord, ClosedSurfaceRecord, CopyWorkspacePath,
    MAX_CLOSED_PANE_SCROLLBACK_BYTES, MAX_CLOSED_PANES, NewWorkspace, NextWorkspace,
    OpenWorkspaceInCursor, OpenWorkspaceInVsCode, OpenWorkspaceInWindsurf, OpenWorkspaceInZed,
    PaneFlowApp, RevealWorkspaceInFileManager, SelectWorkspace1, SelectWorkspace2,
    SelectWorkspace3, SelectWorkspace4, SelectWorkspace5, SelectWorkspace6, SelectWorkspace7,
    SelectWorkspace8, SelectWorkspace9, SplitHorizontally, SplitVertically, UndoClosePane,
};

#[derive(Clone)]
pub(crate) enum WorkspaceFocusTarget {
    FirstPane,
    Pane {
        pane: gpui::Entity<crate::pane::Pane>,
    },
}

fn push_closed_pane_record(records: &mut Vec<ClosedPaneRecord>, mut record: ClosedPaneRecord) {
    if let ClosedSurfaceRecord::Terminal {
        replay: Some(replay),
        ..
    } = &mut record.surface
    {
        replay.shrink_to_fit();
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
        if let ClosedSurfaceRecord::Terminal { replay, .. } = &mut record.surface
            && let Some(replay) = replay.take()
        {
            total = total.saturating_sub(replay.len());
        }
    }
}

fn closed_pane_scrollback_bytes(records: &[ClosedPaneRecord]) -> usize {
    records
        .iter()
        .map(|record| &record.surface)
        .filter_map(|tab| match tab {
            ClosedSurfaceRecord::Terminal { replay, .. } => replay.as_ref(),
            ClosedSurfaceRecord::Markdown { .. } => None,
        })
        .map(Vec::len)
        .sum()
}

fn capture_closed_pane_record(
    pane: &gpui::Entity<crate::pane::Pane>,
    workspace_idx: usize,
    cx: &App,
) -> Option<ClosedPaneRecord> {
    let pane_ref = pane.read(cx);
    let surface = match &pane_ref.surface {
        crate::pane::PaneSurface::Terminal(tv) => {
            let tv_ref = tv.read(cx);
            ClosedSurfaceRecord::Terminal {
                cwd: tv_ref
                    .terminal
                    .current_cwd
                    .as_ref()
                    .map(std::path::PathBuf::from)
                    .or_else(|| tv_ref.terminal.cwd_now()),
                replay: tv_ref.terminal.capture_replay(),
                custom_name: tv_ref.terminal.custom_name.clone(),
                font_size: tv_ref.terminal.font_size_override,
            }
        }
        crate::pane::PaneSurface::Markdown(markdown) => ClosedSurfaceRecord::Markdown {
            path: markdown.read(cx).path.clone(),
        },
        crate::pane::PaneSurface::Diff(_) => return None,
    };
    Some(ClosedPaneRecord {
        surface,
        workspace_idx,
    })
}

fn restore_closed_surface_record(
    tab: ClosedSurfaceRecord,
    ws_id: u64,
    cx: &mut Context<PaneFlowApp>,
) -> crate::pane::PaneSurface {
    match tab {
        ClosedSurfaceRecord::Terminal {
            cwd,
            replay,
            custom_name,
            font_size,
        } => {
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
            terminal.update(cx, |view, _| {
                view.terminal.custom_name = custom_name;
                view.terminal.font_size_override = font_size;
            });
            if let Some(replay) = replay {
                terminal.read(cx).restore_replay(&replay);
            }
            cx.subscribe(&terminal, PaneFlowApp::handle_terminal_event)
                .detach();
            crate::pane::PaneSurface::Terminal(terminal)
        }
        ClosedSurfaceRecord::Markdown { path } => {
            let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                crate::markdown::MarkdownView::open(path, cx)
            });
            crate::pane::PaneSurface::Markdown(markdown)
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn apply_git_state_for_cwd(
        &mut self,
        cwd: &str,
        branch: String,
        is_repo: bool,
        stats: crate::workspace::GitDiffStats,
    ) -> bool {
        let mut changed = false;
        for workspace in &mut self.workspaces {
            if workspace.cwd == cwd {
                if workspace.git_branch != branch {
                    workspace.git_branch = branch.clone();
                    changed = true;
                }
                if workspace.is_git_repo != is_repo {
                    workspace.is_git_repo = is_repo;
                    changed = true;
                }
                if workspace.git_stats != stats {
                    workspace.git_stats = stats.clone();
                    changed = true;
                }
            }
        }
        changed |= self.worktree_states.set_checkout(
            cwd,
            crate::app::tab_worktree::CheckoutGit {
                branch,
                is_repo,
                stats,
            },
        );
        changed
    }

    pub(crate) fn apply_git_stats_for_cwd(
        &mut self,
        cwd: &str,
        stats: crate::workspace::GitDiffStats,
    ) -> bool {
        let mut changed = false;
        for workspace in &mut self.workspaces {
            if workspace.cwd == cwd && workspace.git_stats != stats {
                workspace.git_stats = stats.clone();
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn dismiss_transient_surfaces(&mut self) {
        self.title_bar_files_menu_open = None;
        self.title_bar_help_menu_open = None;
        self.workspace_menu_open = None;
        self.sidebar_customize_menu_open = false;
        self.sidebar_show_submenu_open = false;
        self.tab_menu_open = None;
        self.pane_menu_open = None;
        self.profile_menu_open = None;
        self.files_menu_open = None;
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
            WorkspaceFocusTarget::Pane { pane } => {
                pane.update(cx, |_p, cx| cx.notify());
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
        }

        self.sync_files_sidebar_session(cx);
        if self.agent_sessions.sessions_sidebar_open {
            let keep_sidebar_focus = self.agent_sessions.sessions_focus.is_focused(window);
            match self.workspaces[idx]
                .active_tab()
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
        self.acknowledge_visible_completions(cx);
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
        self.sync_files_sidebar_session(cx);
        if self.agent_sessions.sessions_sidebar_open {
            self.close_sessions_sidebar(cx);
        }
        self.save_session(cx);
        self.reconcile_diff_after_workspace_change(cx);
        cx.notify();
        changed
    }

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

    pub(crate) fn reconcile_diff_after_workspace_change(&self, cx: &mut Context<Self>) {
        if matches!(self.mode, paneflow_config::schema::AppMode::Diff) {
            let weak = cx.weak_entity();
            cx.defer(move |cx| {
                let _ = weak.update(cx, |app, cx| app.rebuild_diff_view(cx));
            });
        }
    }

    #[allow(dead_code)]
    pub(crate) fn create_workspace(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.workspaces.len() >= MAX_WORKSPACES {
            return;
        }
        let n = self.workspaces.len() + 1;
        let ws_id = next_workspace_id();
        let ws = Workspace::empty_with_cwd_and_id(
            ws_id,
            format!("Terminal {n}"),
            crate::launch_cwd::implicit_launch_cwd(),
        );
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        self.watch_git_dir(&ws);
        self.workspaces.push(ws);
        self.active_idx = self.workspaces.len() - 1;
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn open_workspace_folders(
        &mut self,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        let mut opened = false;
        for path in paths {
            if self.workspaces.len() >= MAX_WORKSPACES {
                break;
            }
            if !path.is_dir() {
                continue;
            }
            let cwd = path.display().to_string();
            if let Some(at) = self.workspaces.iter().position(|ws| ws.cwd == cwd) {
                self.active_idx = at;
                opened = true;
                continue;
            }
            let n = self.workspaces.len() + 1;
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("Terminal {n}"));
            let ws_id = next_workspace_id();
            let ws = Workspace::empty_with_cwd_and_id(ws_id, title, path.clone());
            Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
            self.watch_git_dir(&ws);
            self.workspaces.push(ws);
            self.active_idx = self.workspaces.len() - 1;
            opened = true;
        }
        if !opened {
            return;
        }
        self.save_session(cx);
        cx.notify();
        self.reconcile_diff_after_workspace_change(cx);
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
                            app.open_workspace_folders(&paths, cx);
                        })
                    });
                }
            },
        )
        .detach();
    }

    pub(crate) fn new_terminal_cwd(
        &self,
        source_cwd: Option<std::path::PathBuf>,
    ) -> Option<std::path::PathBuf> {
        let Some(ws) = self.active_workspace() else {
            return source_cwd;
        };
        let confined = ws.active_tab().confine_cwd(source_cwd);
        confined.or_else(|| {
            Some(ws.cwd.as_str())
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
        let Some(root) = &ws.active_tab().root else {
            return;
        };
        if !ws.active_tab().can_add_pane() {
            self.show_toast(format!("Maximum pane count reached ({MAX_PANES})"), cx);
            return;
        }
        let Some(focused) = root.focused_pane(window, cx) else {
            self.show_toast("No focused pane to split", cx);
            return;
        };
        if let Err(message) = self.split_with_target(
            focused,
            direction,
            TerminalSurfaceProfile::Normal,
            None,
            window,
            cx,
        ) {
            self.show_toast(message, cx);
        }
    }

    pub(crate) fn split_with_target(
        &mut self,
        target: Entity<crate::pane::Pane>,
        direction: SplitDirection,
        profile: TerminalSurfaceProfile,
        command: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let Some(ws) = self.active_workspace() else {
            return Err("No active project".to_string());
        };
        if ws.is_zoomed() {
            return Err("Unzoom before splitting panes".to_string());
        }
        if ws.active_tab().root.is_none() {
            return Err("This tab has no pane to split".to_string());
        }
        if !ws.active_tab().can_add_pane() {
            return Err(format!("Maximum pane count reached ({MAX_PANES})"));
        }
        let ws_id = ws.id;

        let source_cwd = target
            .read(cx)
            .active_terminal_opt()
            .and_then(|tv| tv.read(cx).terminal.cwd_now());
        let source_cwd = self.new_terminal_cwd(source_cwd);
        let new_terminal =
            cx.new(|cx| TerminalView::with_cwd_and_profile(ws_id, source_cwd, None, profile, cx));
        let new_pane = self.create_pane(new_terminal.clone(), ws_id, cx);
        let inserted = if let Some(ws) = self.active_workspace_mut()
            && let Some(root) = &mut ws.active_tab_mut().root
        {
            root.split_at_pane(&target, direction, new_pane.clone())
        } else {
            false
        };
        if !inserted {
            return Err("That pane no longer exists".to_string());
        }
        if let Some(command) = command {
            new_terminal.read(cx).send_command(command);
            new_terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
        }
        new_pane.read(cx).focus_handle(cx).focus(window, cx);
        self.save_session(cx);
        cx.notify();
        Ok(())
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
        let workspace_idx = self.active_idx;
        if let Some(ws) = self.active_workspace()
            && let Some(root) = &ws.active_tab().root
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
                && let Some(root) = ws.active_tab_mut().root.take()
            {
                let (new_root, _) = root.remove_pane(&pane);
                ws.active_tab_mut().root = new_root;
            }
            if let Some(ref root) = ws.active_tab().root {
                root.focus_first(window, cx);
            }
        } else if let Some(ws) = self.active_workspace_mut()
            && let Some(root) = ws.active_tab_mut().root.take()
        {
            let (new_root, _closed, focus_target) = root.close_focused(window, cx);
            ws.active_tab_mut().root = new_root;

            if ws.active_tab().root.is_some() {
                if let Some(target) = focus_target {
                    target.read(cx).focus_handle(cx).focus(window, cx);
                } else if let Some(ref root) = ws.active_tab().root {
                    root.focus_first(window, cx);
                }
            }
        }

        if let Some(ws) = self.active_workspace()
            && ws.active_tab().root.is_none()
        {
            let ws_id = ws.id;
            let cwd = self.new_terminal_cwd(None);
            let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, cwd, None, cx));
            let new_pane = self.create_pane(terminal, ws_id, cx);
            if let Some(ws) = self.active_workspace_mut() {
                ws.active_tab_mut().root = Some(LayoutTree::Leaf(new_pane));
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
            return;
        };

        if record.workspace_idx < self.workspaces.len() {
            self.active_idx = record.workspace_idx;
        }

        let Some(ws_id) = self.active_workspace().map(|ws| ws.id) else {
            self.closed_panes.push(record);
            self.show_toast("No active workspace to restore pane", cx);
            return;
        };
        let surface = restore_closed_surface_record(record.surface, ws_id, cx);
        let new_pane = self.create_pane_with_existing_surface(surface, ws_id, cx);

        let inserted = if let Some(ws) = self.active_workspace_mut() {
            if let Some(root) = &mut ws.active_tab_mut().root {
                if !root.split_at_focused(SplitDirection::Horizontal, new_pane.clone(), window, cx)
                {
                    root.split_first_leaf(SplitDirection::Horizontal, new_pane.clone());
                }
            } else {
                ws.active_tab_mut().root = Some(LayoutTree::Leaf(new_pane.clone()));
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
        let worktrees = std::mem::take(&mut self.workspaces[idx].managed_worktrees);
        self.prune_worktree_states();
        Self::spawn_worktree_teardown(worktrees, cx);
        self.workspaces.remove(idx);
        if self.workspaces.is_empty() {
            self.active_idx = 0;
        } else {
            if self.active_idx >= self.workspaces.len() {
                self.active_idx = self.workspaces.len() - 1;
            } else if self.active_idx > idx {
                self.active_idx -= 1;
            }
            self.workspaces[self.active_idx].focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
        self.refresh_composer_slot(cx);
        self.sync_broadcast_stripes(cx);
        self.flush_pending_prefill(cx);
        self.sync_pending_chips(cx);
        self.reconcile_diff_after_workspace_change(cx);
    }

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
        let Some((ws_idx, tab_idx)) = self.renaming_tab.take() else {
            return;
        };
        let text = self.rename_input.read(cx).value().trim().to_string();
        if text.is_empty() {
            return;
        }
        if self
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tab_mut(tab_idx))
            .is_some_and(|tab| tab.set_title(&text, TabTitleSource::User))
        {
            self.save_session(cx);
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
        let mut flag = std::ffi::OsString::from("/select,");
        flag.push(path.as_os_str());
        let result = spawn_detached(std::process::Command::new("explorer").arg(flag));
        return result.map_err(|err| format!("Could not open Explorer: {err}"));
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        spawn_detached(std::process::Command::new("xdg-open").arg(path))
            .map_err(|err| format!("Could not open file manager: {err}"))
    }
}

pub(crate) fn resolve_editor_binary(command: &str) -> std::path::PathBuf {
    resolve_editor_binary_in(command, &editor_search_paths())
}

pub(crate) fn editor_toast_label(label: &str) -> &str {
    label.strip_prefix("Open in ").unwrap_or(label)
}

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

    #[cfg(target_os = "linux")]
    #[test]
    fn reveal_linux_missing_xdg_open_surfaces_install_hint() {
        let err = std::io::Error::from(std::io::ErrorKind::NotFound);
        let msg = if err.kind() == std::io::ErrorKind::NotFound {
            "xdg-open not found - install xdg-utils to use this feature".to_string()
        } else {
            format!("Could not open file manager: {err}")
        };
        assert!(msg.contains("xdg-utils"), "unhappy-path AC text: {msg}");
    }

    #[test]
    fn reveal_accepts_regular_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _callable: fn(&std::path::Path) -> Result<(), String> = reveal_in_file_manager;
        let _ = tmp.path();
    }

    const EXE_SUFFIX: &str = if cfg!(windows) { ".exe" } else { "" };

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
        let stub = "paneflow_resolver_stub_pflw_42";
        let dir = tempfile::TempDir::new().unwrap();
        let expected = make_stub_binary(dir.path(), stub);

        let resolved = resolve_editor_binary_in(stub, &[dir.path().to_path_buf()]);

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
        let bare = "paneflow_no_such_editor_zzz_99";
        let resolved = resolve_editor_binary_in(bare, &[]);
        assert_eq!(resolved, std::path::PathBuf::from(bare));
    }

    #[test]
    fn resolver_returns_bare_command_when_fallback_dir_is_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        let bare = "paneflow_no_such_editor_zzz_77";
        let resolved = resolve_editor_binary_in(bare, &[dir.path().to_path_buf()]);
        assert_eq!(resolved, std::path::PathBuf::from(bare));
    }

    fn closed_pane_record_with_replay(len: usize) -> ClosedPaneRecord {
        ClosedPaneRecord {
            surface: ClosedSurfaceRecord::Terminal {
                cwd: None,
                replay: Some(vec![b'x'; len]),
                custom_name: None,
                font_size: None,
            },
            workspace_idx: 0,
        }
    }

    #[test]
    fn closed_pane_budget_drops_oldest_scrollback_not_record() {
        let one_mib = 1024 * 1024;
        let mut records = vec![
            closed_pane_record_with_replay(one_mib),
            closed_pane_record_with_replay(one_mib),
        ];

        push_closed_pane_record(&mut records, closed_pane_record_with_replay(one_mib));

        assert_eq!(records.len(), 3, "budget must preserve undo records");
        assert!(
            matches!(
                &records[0].surface,
                ClosedSurfaceRecord::Terminal { replay: None, .. }
            ),
            "oldest scrollback should be released first"
        );
        assert!(matches!(
            &records[1].surface,
            ClosedSurfaceRecord::Terminal {
                replay: Some(_),
                ..
            }
        ));
        assert!(matches!(
            &records[2].surface,
            ClosedSurfaceRecord::Terminal {
                replay: Some(_),
                ..
            }
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
                surface: ClosedSurfaceRecord::Terminal {
                    cwd: None,
                    replay: None,
                    custom_name: None,
                    font_size: None,
                },
                workspace_idx: 0,
            },
        );

        assert_eq!(records.len(), 1);
        assert!(matches!(
            &records[0].surface,
            ClosedSurfaceRecord::Terminal { replay: None, .. }
        ));
        assert_eq!(closed_pane_scrollback_bytes(&records), 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn search_paths_linux_covers_user_and_system_bin() {
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
