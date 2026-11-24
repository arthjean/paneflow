mod focus;
mod git_watch;
mod layout;
mod tab;
mod template_launch;
pub(crate) mod templates;

use gpui::{App, AppContext, ClipboardItem, Context, Entity, Focusable, PathPromptOptions, Window};
use paneflow_config::schema::{TabTitleSource, TerminalSurfaceProfile};

use crate::app::close_policy::CloseTarget;
use crate::layout::{MAX_PANES, SplitDirection};
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{Workspace, next_workspace_id_in};
use crate::{
    ClosePane, CloseWorkspace, CopyWorkspacePath, NewWorkspace, NextWorkspace,
    OpenWorkspaceInCursor, OpenWorkspaceInVsCode, OpenWorkspaceInWebStorm, OpenWorkspaceInZed,
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

#[derive(Debug, Default)]
pub(crate) struct SurfaceLaunch {
    pub(crate) command: Option<String>,
    pub(crate) env: Option<std::collections::HashMap<String, String>>,
    pub(crate) cwd: Option<std::path::PathBuf>,
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
                let stats = stats.clone().or_previous(&workspace.git_stats);
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
        if stats.unavailable {
            return false;
        }
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
        self.sidebar_new_tab_menu_open = false;
        self.sidebar_space_menu_open = false;
        self.agents_display_menu_open = false;
        self.tab_menu_open = None;
        self.sidebar_selection.close_menu();
        self.pane_menu_open = None;
        self.files_menu_open = None;
        self.comment_offer = None;
        self.diff_dock.tab_menu = None;
        self.diff_dock.crumb_menu = None;
        self.usage.close();
    }

    pub(crate) fn active_workspace(&self) -> Option<&Workspace> {
        debug_assert!(
            self.workspaces.is_empty() || self.active_idx < self.workspaces.len(),
            "active_idx out of bounds"
        );
        self.workspaces.get(self.shown_workspace_idx()?)
    }

    pub(crate) fn focused_or_first_pane(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Entity<Pane>> {
        let root = self.active_workspace()?.active_tab().root.as_ref()?;
        root.focused_pane(window, cx).or_else(|| root.first_leaf())
    }

    pub(crate) fn active_workspace_mut(&mut self) -> Option<&mut Workspace> {
        let idx = self.shown_workspace_idx()?;
        self.workspaces.get_mut(idx)
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

        let changed = self.select_workspace_through_model(idx, cx);
        self.after_workspace_activated(idx, changed, focus_target, window, cx)
    }

    pub(crate) fn after_workspace_activated(
        &mut self,
        idx: usize,
        changed: bool,
        focus_target: WorkspaceFocusTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.dismiss_transient_surfaces();

        match focus_target {
            WorkspaceFocusTarget::FirstPane => {
                self.workspaces[idx].focus_first(window, cx);
            }
            WorkspaceFocusTarget::Pane { pane } => {
                self.reveal_pane_through_model(idx, &pane, cx);
                pane.update(cx, |_p, cx| cx.notify());
                if !Self::focus_pane_window(pane.clone(), cx) {
                    pane.read(cx).focus_handle(cx).focus(window, cx);
                }
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
        self.acknowledge_visible_completions(cx);
        self.refresh_owned_sessions(cx);
        if changed {
            self.probe_workspace_git_now(idx, cx);
        }
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

        let changed = self.select_workspace_through_model(idx, cx);
        self.dismiss_transient_surfaces();
        self.sync_files_sidebar_session(cx);
        if self.agent_sessions.sessions_sidebar_open {
            self.close_sessions_sidebar(cx);
        }
        if changed {
            self.probe_workspace_git_now(idx, cx);
        }
        cx.notify();
        changed
    }

    pub(crate) fn spawn_worktree_teardown(
        &self,
        worktrees: Vec<crate::workspace::worktree::ManagedWorktree>,
        closed_sessions: Vec<paneflow_config::schema::SessionId>,
        cx: &mut Context<Self>,
    ) {
        use crate::workspace::worktree;
        if worktrees.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let seen = smol::unblock(move || {
                worktree::after_sessions_end(
                    &closed_sessions,
                    listed_live_sessions,
                    || std::thread::sleep(worktree::SESSION_END_PAUSE),
                    worktree::SESSION_END_POLLS,
                )
            })
            .await;
            let Ok((auto_remove, checkouts)) = this.update(cx, |app: &mut Self, _| {
                (
                    app.cached_config.worktrees.auto_remove_enabled(),
                    app.checkouts_in_use(),
                )
            }) else {
                return;
            };
            let outcome = smol::unblock(move || {
                worktree::close_teardown(worktrees, auto_remove, seen, checkouts)
            })
            .await;
            let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                app.settle_closed_worktrees(outcome, cx);
            });
        })
        .detach();
    }

    pub(crate) fn open_workspace_folders(
        &mut self,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        let existing = self.workspaces.len();
        let mut target = None;
        for path in paths {
            if !path.is_dir() {
                continue;
            }
            let cwd = path.display().to_string();
            if let Some(at) = self.workspaces.iter().position(|ws| ws.cwd == cwd) {
                target = Some(at);
                continue;
            }
            let n = self.workspaces.len() + 1;
            let title = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| format!("Terminal {n}"));
            let ws_id = next_workspace_id_in(self.active_space.as_ref());
            let ws = Workspace::empty_with_cwd_and_id(ws_id, title, path.clone());
            Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
            target = Some(self.add_workspace(ws, cx));
        }
        let Some(target) = target else {
            return;
        };
        self.record_recent_workspaces(paths, cx);
        if target < existing {
            self.activate_workspace_without_window(target, cx);
            return;
        }
        self.select_workspace_through_model(target, cx);
        cx.notify();
    }

    pub(crate) fn create_workspace_with_picker(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    ) -> crate::workspace::SpawnCwd {
        let Some(ws) = self.active_workspace() else {
            return crate::workspace::SpawnCwd {
                cwd: source_cwd,
                ..Default::default()
            };
        };
        let mut spawn = ws.active_tab().spawn_cwd(source_cwd);
        if spawn.cwd.is_none() {
            spawn.cwd = Some(ws.cwd.as_str())
                .filter(|cwd| !cwd.is_empty())
                .map(std::path::PathBuf::from);
        }
        spawn
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
            SurfaceLaunch::default(),
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
        launch: SurfaceLaunch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Entity<TerminalView>, String> {
        let Some(ws) = self.active_workspace() else {
            return Err("No active project".to_string());
        };
        if ws.is_zoomed() {
            return Err("Unzoom before splitting panes".to_string());
        }
        if ws.active_tab().root.is_none() {
            return Err("This tab has no pane to split".to_string());
        }
        if !ws.active_tab().contains_pane(&target) {
            return Err("That pane no longer exists".to_string());
        }
        let terminal = self.split_pane_through_model(&target, direction, profile, launch, cx)?;
        self.settle_layout_focus(window, cx);
        Ok(terminal)
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
        let closing_pane = self.active_workspace().and_then(|ws| {
            ws.active_tab().root.as_ref().and_then(|root| {
                if ws.is_zoomed() {
                    root.first_leaf()
                } else {
                    root.focused_pane(window, cx)
                }
            })
        });
        let Some(pane) = closing_pane else {
            return;
        };
        self.request_close(CloseTarget::FocusedPane(pane), Some(window), cx);
    }

    pub(crate) fn remove_focused_pane(
        &mut self,
        pane: Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_pane_through_model(&pane, true, cx) {
            self.settle_layout_focus(window, cx);
        }
    }

    pub(crate) fn handle_undo_close_pane(
        &mut self,
        _: &UndoClosePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.undo_close_through_model(None, window, cx);
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
        if let Some(idx) = self.shown_workspace_idx() {
            self.close_workspace_at(idx, window, cx);
        }
    }

    pub(crate) fn handle_copy_workspace_path(
        &mut self,
        _: &CopyWorkspacePath,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.copy_workspace_path(idx, cx);
        }
    }

    pub(crate) fn handle_reveal_workspace_in_file_manager(
        &mut self,
        _: &RevealWorkspaceInFileManager,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.reveal_workspace_in_file_manager(idx, cx);
        }
    }

    pub(crate) fn handle_open_workspace_in_zed(
        &mut self,
        _: &OpenWorkspaceInZed,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.open_workspace_in_editor(idx, "zed", "Zed", cx);
        }
    }

    pub(crate) fn handle_open_workspace_in_cursor(
        &mut self,
        _: &OpenWorkspaceInCursor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.open_workspace_in_editor(idx, "cursor", "Cursor", cx);
        }
    }

    pub(crate) fn handle_open_workspace_in_vscode(
        &mut self,
        _: &OpenWorkspaceInVsCode,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.open_workspace_in_editor(idx, "code", "VS Code", cx);
        }
    }

    pub(crate) fn handle_open_workspace_in_webstorm(
        &mut self,
        _: &OpenWorkspaceInWebStorm,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = self.shown_workspace_idx() {
            self.open_workspace_in_editor(idx, "webstorm", "WebStorm", cx);
        }
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
        self.request_close(CloseTarget::Workspace(idx), Some(window), cx);
    }

    pub(crate) fn remove_workspace(
        &mut self,
        idx: usize,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if idx >= self.workspaces.len() {
            return;
        }
        let shown_before = self.active_workspace().map(|workspace| workspace.id);
        if let Some(dir) = self.workspaces[idx].git_dir.clone() {
            self.unwatch_git_dir(&dir);
        }
        let worktrees = self.workspaces[idx].managed_worktrees.clone();
        let sessions = self.workspace_session_ids(idx, cx);
        let removed_id = self.workspaces[idx].id;
        if !self.close_workspace_through_model(idx, window.is_some(), cx) {
            return;
        }
        self.prune_worktree_states();
        self.spawn_worktree_teardown(worktrees, sessions, cx);
        if self
            .renaming_tab
            .is_some_and(|key| key.workspace_id == removed_id)
        {
            self.renaming_tab = None;
        }
        self.dismiss_transient_surfaces();
        if let Some(window) = window {
            if self.active_workspace().is_some() {
                self.settle_layout_focus(window, cx);
            } else {
                self.pending_pane_focus = None;
                window.focus(&self.welcome_focus, cx);
            }
        }
        self.probe_workspace_git_if_shown_changed(shown_before, cx);
    }

    pub(crate) fn reorder_workspace(
        &mut self,
        from_id: u64,
        to_idx: usize,
        cx: &mut Context<Self>,
    ) {
        self.adopt_layout(cx);
        let Some(workspace) = self.layout_workspace_id(from_id) else {
            return;
        };
        let _ = self.apply_layout_op(
            paneflow_layout::Op::ReorderWorkspace {
                workspace,
                to: to_idx,
            },
            cx,
        );
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

        let task = cx
            .background_executor()
            .spawn(async move { reveal_in_file_manager(std::path::Path::new(&cwd)).await });
        cx.spawn(async move |this, cx| {
            if let Err(message) = task.await {
                let _ = this.update(cx, |app, cx| app.show_toast(message, cx));
            }
        })
        .detach();
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

        let command = command.to_owned();
        let toast_label = editor_toast_label(label).to_owned();
        let task = cx.background_executor().spawn(async move {
            let cmd = smol::unblock(move || {
                let bin = resolve_editor_binary(&command);
                log::info!(
                    "workspace editor resolved: editor={command:?} binary={bin:?} cwd={cwd:?}"
                );
                let mut cmd = std::process::Command::new(bin);
                cmd.current_dir(cwd).arg(".");
                cmd
            })
            .await;
            match crate::external_open::run_workspace_command(cmd).await {
                Ok(status) if status.success() => Ok(()),
                Ok(status) => Err(format!(
                    "Couldn't open in {toast_label}: launcher exited with {status}"
                )),
                Err(error) => Err(format!("Couldn't open in {toast_label}: {error}")),
            }
        });
        cx.spawn(async move |this, cx| {
            if let Err(message) = task.await {
                log::warn!("{message}");
                let _ = this.update(cx, |app, cx| app.show_toast(message, cx));
            }
        })
        .detach();

        self.workspace_menu_open = None;
        cx.notify();
    }

    pub(crate) fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.renaming_tab.take() else {
            return;
        };
        let text = self.rename_input.read(cx).value().to_string();
        if let Some((ws_idx, tab_idx)) = key.resolve(&self.workspaces) {
            self.retitle_tab_through_model(ws_idx, tab_idx, &text, TabTitleSource::User, cx);
        }
    }

    pub(crate) fn handle_next_workspace(
        &mut self,
        _: &NextWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = Self::compute_display_order(&self.workspaces, self.active_space.as_ref());
        if let Some(next) = next_in_display_order(&order, self.shown_workspace_idx()) {
            self.select_workspace(next, window, cx);
        }
    }

    pub(crate) fn handle_select_ws(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = Self::compute_display_order(&self.workspaces, self.active_space.as_ref());
        if order.is_empty() {
            self.open_recent_workspace(idx, window, cx);
            return;
        }
        if let Some(target) = order.get(idx) {
            self.select_workspace(*target, window, cx);
        }
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

fn listed_live_sessions()
-> crate::workspace::worktree::SessionListing<paneflow_config::schema::SessionId> {
    match crate::terminal::host_link::live_sessions() {
        crate::terminal::host_link::LiveSessionProbe::Sessions(sessions) => Ok(Some(
            sessions
                .into_iter()
                .map(|session| (session.session, session.cwd))
                .collect(),
        )),
        crate::terminal::host_link::LiveSessionProbe::NoHost => Ok(None),
        crate::terminal::host_link::LiveSessionProbe::Unknown(error) => Err(error),
    }
}

fn next_in_display_order(order: &[usize], active: Option<usize>) -> Option<usize> {
    let position = order.iter().position(|&idx| Some(idx) == active);
    let next = position.map_or(0, |position| (position + 1) % order.len());
    order.get(next).copied()
}

pub(crate) async fn reveal_in_file_manager(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        open_workspace_folder("xdg-open", path)
            .await
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    "xdg-open not found - install xdg-utils to use this feature".to_string()
                } else {
                    format!("Could not open file manager: {err}")
                }
            })
    }
    #[cfg(target_os = "macos")]
    {
        open_workspace_folder("open", path)
            .await
            .map_err(|err| format!("Could not open Finder: {err}"))
    }
    #[cfg(target_os = "windows")]
    {
        open_workspace_folder("explorer.exe", path)
            .await
            .map_err(|err| format!("Could not open Explorer: {err}"))
    }
}

pub(crate) async fn open_workspace_folder(
    command: &str,
    path: &std::path::Path,
) -> std::io::Result<()> {
    let mut cmd = std::process::Command::new(command);
    cmd.arg(path);
    let status = crate::external_open::run_workspace_command(cmd).await?;
    #[cfg(not(target_os = "windows"))]
    if !status.success() {
        return Err(std::io::Error::other(format!(
            "{command} exited with {status}"
        )));
    }
    #[cfg(target_os = "windows")]
    let _ = status;
    Ok(())
}

pub(crate) fn resolve_editor_binary(command: &str) -> std::path::PathBuf {
    resolve_editor_binary_in(command, &editor_search_paths())
}

pub(crate) fn editor_toast_label(label: &str) -> &str {
    label.strip_prefix("Open in ").unwrap_or(label)
}

pub(crate) fn find_editor_binary(command: &str) -> Option<std::path::PathBuf> {
    find_editor_binary_in(command, &editor_search_paths())
}

fn resolve_editor_binary_in(
    command: &str,
    fallback_paths: &[std::path::PathBuf],
) -> std::path::PathBuf {
    find_editor_binary_in(command, fallback_paths)
        .unwrap_or_else(|| std::path::PathBuf::from(command))
}

fn find_editor_binary_in(
    command: &str,
    fallback_paths: &[std::path::PathBuf],
) -> Option<std::path::PathBuf> {
    if let Ok(path) = which::which(command)
        && let Some(path) = normalize_editor_candidate(path)
    {
        return Some(path);
    }
    if !fallback_paths.is_empty()
        && let Ok(joined) = std::env::join_paths(fallback_paths)
    {
        for name in editor_binary_names(command) {
            if let Ok(path) = which::which_in(name, Some(&joined), ".")
                && let Some(path) = normalize_editor_candidate(path)
            {
                return Some(path);
            }
        }
    }
    None
}

struct JetBrainsProduct {
    command: &'static str,
    install_name: &'static str,
}

const JETBRAINS_PRODUCTS: &[JetBrainsProduct] = &[JetBrainsProduct {
    command: "webstorm",
    install_name: "WebStorm",
}];

fn editor_binary_names(command: &str) -> Vec<String> {
    let mut names = Vec::new();
    if cfg!(target_os = "windows")
        && JETBRAINS_PRODUCTS
            .iter()
            .any(|product| product.command == command)
    {
        names.push(format!("{command}64"));
    }
    names.push(command.to_string());
    names
}

fn is_install_of(entry: &str, install_name: &str) -> bool {
    let entry = entry.strip_suffix(".app").unwrap_or(entry);
    entry == install_name
        || entry
            .strip_prefix(install_name)
            .is_some_and(|rest| rest.starts_with(' ') || rest.starts_with('-'))
}

fn push_jetbrains_install_dirs(
    paths: &mut Vec<std::path::PathBuf>,
    roots: &[std::path::PathBuf],
    executable_dir: &[&str],
) {
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut installs: Vec<std::path::PathBuf> = entries
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                JETBRAINS_PRODUCTS
                    .iter()
                    .any(|product| is_install_of(&name, product.install_name))
            })
            .map(|entry| {
                executable_dir
                    .iter()
                    .fold(entry.path(), |dir, part| dir.join(part))
            })
            .collect();
        installs.sort_unstable_by(|a, b| b.cmp(a));
        paths.extend(installs);
    }
}

fn push_jetbrains_search_paths(paths: &mut Vec<std::path::PathBuf>) {
    use std::path::PathBuf;

    #[cfg(target_os = "windows")]
    {
        let local_app_data = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        if let Some(local) = &local_app_data {
            paths.push(local.join("JetBrains").join("Toolbox").join("scripts"));
        }
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(local) = &local_app_data {
            roots.push(local.join("Programs"));
        }
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(dir) = std::env::var_os(var) {
                roots.push(PathBuf::from(dir));
            }
        }
        let roots: Vec<PathBuf> = roots
            .iter()
            .flat_map(|root| [root.clone(), root.join("JetBrains")])
            .collect();
        push_jetbrains_install_dirs(paths, &roots, &["bin"]);
    }
    #[cfg(target_os = "macos")]
    {
        let home = dirs::home_dir();
        if let Some(home) = &home {
            paths.push(
                home.join("Library")
                    .join("Application Support")
                    .join("JetBrains")
                    .join("Toolbox")
                    .join("scripts"),
            );
        }
        let mut roots: Vec<PathBuf> = home.iter().map(|home| home.join("Applications")).collect();
        roots.push(PathBuf::from("/Applications"));
        push_jetbrains_install_dirs(paths, &roots, &["Contents", "MacOS"]);
    }
    #[cfg(target_os = "linux")]
    {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute())
            .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")));
        if let Some(data_home) = data_home {
            paths.push(data_home.join("JetBrains").join("Toolbox").join("scripts"));
        }
        paths.push(PathBuf::from("/snap/bin"));
        push_jetbrains_install_dirs(paths, &[PathBuf::from("/opt")], &["bin"]);
    }
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
    push_jetbrains_search_paths(&mut paths);
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

    fn named_workspace(id: u64, repo: Option<&str>, tabs: &[&str]) -> Workspace {
        let mut ws = Workspace::restored_with_id(
            id,
            format!("ws{id}"),
            std::path::PathBuf::new(),
            tabs.iter()
                .map(|title| crate::workspace::Tab::new(*title, None))
                .collect(),
            0,
        );
        ws.repo_root = repo.map(std::path::PathBuf::from);
        ws
    }

    #[test]
    fn workspace_shortcuts_follow_the_sidebar_order() {
        let workspaces = vec![
            named_workspace(1, Some("/repo"), &["a"]),
            named_workspace(2, None, &["b"]),
            named_workspace(3, Some("/repo"), &["c"]),
        ];
        let order = PaneFlowApp::compute_display_order(&workspaces, None);
        assert_eq!(
            order,
            vec![0, 2, 1],
            "the sidebar groups checkouts of one repo"
        );

        assert_eq!(
            order.get(1).copied(),
            Some(2),
            "Cmd/Ctrl+2 opens the second row"
        );
        assert_eq!(next_in_display_order(&order, Some(0)), Some(2));
        assert_eq!(next_in_display_order(&order, Some(2)), Some(1));
        assert_eq!(next_in_display_order(&order, Some(1)), Some(0));
        assert_eq!(next_in_display_order(&[], Some(0)), None);
    }

    #[test]
    fn workspace_shortcuts_only_walk_the_active_space_and_keep_repo_groups() {
        let work = paneflow_config::schema::SpaceId::new();
        let mut workspaces = vec![
            named_workspace(1, Some("/repo"), &["a"]),
            named_workspace(2, None, &["b"]),
            named_workspace(3, Some("/repo"), &["c"]),
            named_workspace(4, Some("/repo"), &["d"]),
            named_workspace(5, None, &["e"]),
        ];
        for idx in [1, 2, 3] {
            workspaces[idx].space = Some(work.clone());
        }

        let order = PaneFlowApp::compute_display_order(&workspaces, Some(&work));
        assert_eq!(
            order,
            vec![1, 2, 3],
            "only the space's workspaces, its two /repo checkouts grouped"
        );
        assert_eq!(
            PaneFlowApp::compute_display_order(&workspaces, None),
            vec![0, 4],
            "a lone checkout of a repo shared with another space is not grouped"
        );
        assert_eq!(next_in_display_order(&order, Some(3)), Some(1));
        assert_eq!(
            next_in_display_order(&order, Some(0)),
            Some(1),
            "from a workspace outside the space, Next starts at the space's first"
        );
        assert_eq!(
            PaneFlowApp::compute_display_order(
                &workspaces,
                Some(&paneflow_config::schema::SpaceId::new())
            ),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn closing_the_workspace_under_an_open_rename_retitles_no_other_tab() {
        let mut workspaces = vec![
            named_workspace(1, None, &["renamed", "sibling"]),
            named_workspace(2, None, &["first", "second"]),
        ];
        let key = crate::TabKey::of(&workspaces[0], 0).expect("the tab exists");
        let titles = |workspaces: &[Workspace]| {
            workspaces
                .iter()
                .flat_map(|ws| ws.tabs().iter().map(|tab| tab.title().to_string()))
                .collect::<Vec<_>>()
        };

        workspaces.remove(0);
        assert_eq!(titles(&workspaces), vec!["first", "second"]);
        assert_eq!(key.resolve(&workspaces), None, "no other tab is renamed");

        let mut workspaces = vec![named_workspace(1, None, &["left", "renamed"])];
        let key = crate::TabKey::of(&workspaces[0], 1).expect("the tab exists");
        workspaces[0].close_tab(0);
        assert_eq!(
            key.resolve(&workspaces),
            Some((0, 0)),
            "the rename follows its tab once the indices shift"
        );
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
                &local_app_data
                    .join("JetBrains")
                    .join("Toolbox")
                    .join("scripts")
            ),
            "missing %LOCALAPPDATA%\\JetBrains\\Toolbox\\scripts"
        );
    }

    #[test]
    fn jetbrains_install_dirs_find_versioned_webstorm_installs_newest_first() {
        let root = tempfile::TempDir::new().unwrap();
        for name in [
            "WebStorm 2025.1",
            "WebStorm 2025.3",
            "WebStorm.app",
            "WebStorm-2024.2.app",
            "WebStormish",
            "PyCharm 2025.3",
        ] {
            std::fs::create_dir(root.path().join(name)).unwrap();
        }
        let mut paths = Vec::new();
        push_jetbrains_install_dirs(&mut paths, &[root.path().to_path_buf()], &["bin"]);
        let found: Vec<String> = paths
            .iter()
            .map(|path| {
                path.parent()
                    .and_then(|install| install.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(
            found,
            vec![
                "WebStorm.app",
                "WebStorm-2024.2.app",
                "WebStorm 2025.3",
                "WebStorm 2025.1",
            ]
        );
        assert!(paths.iter().all(|path| path.ends_with("bin")));
    }

    #[test]
    fn editor_binary_names_try_the_64_bit_jetbrains_launcher_on_windows() {
        let names = editor_binary_names("webstorm");
        if cfg!(target_os = "windows") {
            assert_eq!(names, vec!["webstorm64", "webstorm"]);
        } else {
            assert_eq!(names, vec!["webstorm"]);
        }
        assert_eq!(editor_binary_names("code"), vec!["code"]);
    }
}
