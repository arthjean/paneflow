mod focus;
mod git_watch;
mod layout;
mod swap;
pub(crate) use swap::swap_mode_active_in;
mod tab;
mod template_launch;
pub(crate) mod templates;

use std::collections::HashSet;
use std::time::Instant;

use gpui::{App, AppContext, ClipboardItem, Context, Entity, Focusable, PathPromptOptions, Window};
use paneflow_config::schema::{LayoutNode, SessionId, TabTitleSource, TerminalSurfaceProfile};

use crate::app::close_policy::CloseTarget;
use crate::app::hosted_sessions::StopTarget;
use crate::layout::{LayoutTree, MAX_PANES, SplitDirection};
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{Workspace, next_workspace_id};
use crate::{
    ClosePane, CloseWorkspace, ClosedPaneRecord, ClosedSurfaceRecord, CopyWorkspacePath,
    HeldSession, MAX_CLOSED_PANES, NewWorkspace, NextWorkspace, OpenWorkspaceInCursor,
    OpenWorkspaceInVsCode, OpenWorkspaceInWindsurf, OpenWorkspaceInZed, PaneFlowApp,
    RevealWorkspaceInFileManager, SelectWorkspace1, SelectWorkspace2, SelectWorkspace3,
    SelectWorkspace4, SelectWorkspace5, SelectWorkspace6, SelectWorkspace7, SelectWorkspace8,
    SelectWorkspace9, SessionHold, SplitHorizontally, SplitVertically, UndoClosePane,
};

#[derive(Clone)]
pub(crate) enum WorkspaceFocusTarget {
    FirstPane,
    Pane {
        pane: gpui::Entity<crate::pane::Pane>,
    },
}

pub(crate) struct ClosedTabRecord {
    pub(crate) tab_id: u64,
    pub(crate) workspace_id: u64,
    pub(crate) tab_idx: usize,
    pub(crate) title: String,
    pub(crate) title_source: TabTitleSource,
    pub(crate) worktree: Option<std::path::PathBuf>,
    pub(crate) layout: LayoutNode,
    pub(crate) surfaces: Vec<ClosedSurfaceRecord>,
}

pub(crate) enum ClosedRecord {
    Pane(ClosedPaneRecord),
    Tab(ClosedTabRecord),
}

impl ClosedRecord {
    fn surfaces(&self) -> &[ClosedSurfaceRecord] {
        match self {
            Self::Pane(record) => std::slice::from_ref(&record.surface),
            Self::Tab(record) => &record.surfaces,
        }
    }

    fn surfaces_mut(&mut self) -> &mut [ClosedSurfaceRecord] {
        match self {
            Self::Pane(record) => std::slice::from_mut(&mut record.surface),
            Self::Tab(record) => &mut record.surfaces,
        }
    }
}

fn record_sessions_mut(
    records: &mut [ClosedRecord],
) -> impl Iterator<Item = &mut Option<HeldSession>> {
    records
        .iter_mut()
        .flat_map(ClosedRecord::surfaces_mut)
        .filter_map(|surface| match surface {
            ClosedSurfaceRecord::Terminal { session, .. } => Some(session),
            ClosedSurfaceRecord::Markdown { .. } => None,
        })
}

fn take_held_sessions(
    records: &mut [ClosedRecord],
    mut releases: impl FnMut(&HeldSession) -> bool,
) -> Vec<StopTarget> {
    let mut released = Vec::new();
    for slot in record_sessions_mut(records) {
        if slot.as_ref().is_some_and(&mut releases)
            && let Some(held) = slot.take()
        {
            released.push(held.target);
        }
    }
    released
}

fn is_undo_window(held: &HeldSession) -> bool {
    matches!(held.hold, SessionHold::UndoWindow { .. })
}

pub(crate) fn push_closed_record(
    records: &mut Vec<ClosedRecord>,
    record: ClosedRecord,
) -> Vec<StopTarget> {
    let superseded: HashSet<SessionId> = record
        .surfaces()
        .iter()
        .filter_map(|surface| match surface {
            ClosedSurfaceRecord::Terminal {
                session: Some(held),
                ..
            } => Some(held.target.1.clone()),
            ClosedSurfaceRecord::Terminal { .. } | ClosedSurfaceRecord::Markdown { .. } => None,
        })
        .collect();
    settle_closed_sessions(records, &superseded, None);
    let mut evicted = Vec::new();
    if records.len() >= MAX_CLOSED_PANES {
        let mut oldest = [records.remove(0)];
        evicted = take_held_sessions(&mut oldest, is_undo_window);
    }
    records.push(record);
    evicted
}

pub(crate) fn settle_closed_sessions(
    records: &mut [ClosedRecord],
    closed: &HashSet<SessionId>,
    hold: Option<SessionHold>,
) -> HashSet<SessionId> {
    let mut kept = HashSet::new();
    for slot in record_sessions_mut(records) {
        let Some(held) = slot.as_mut() else {
            continue;
        };
        if !closed.contains(&held.target.1) {
            continue;
        }
        match hold {
            Some(hold) => {
                held.hold = hold;
                kept.insert(held.target.1.clone());
            }
            None => *slot = None,
        }
    }
    kept
}

pub(crate) fn expire_held_sessions(records: &mut [ClosedRecord], now: Instant) -> Vec<StopTarget> {
    take_held_sessions(
        records,
        |held| matches!(held.hold, SessionHold::UndoWindow { until } if until <= now),
    )
}

pub(crate) fn take_undo_window_sessions(records: &mut [ClosedRecord]) -> Vec<StopTarget> {
    take_held_sessions(records, is_undo_window)
}

pub(crate) fn undo_window_session_ids(records: &[ClosedRecord]) -> HashSet<SessionId> {
    records
        .iter()
        .flat_map(ClosedRecord::surfaces)
        .filter_map(|surface| match surface {
            ClosedSurfaceRecord::Terminal {
                session: Some(held),
                ..
            } if is_undo_window(held) => Some(held.target.1.clone()),
            ClosedSurfaceRecord::Terminal { .. } | ClosedSurfaceRecord::Markdown { .. } => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SurfaceReopen {
    Fresh,
    Reattach(SessionId),
    AlreadyOpen,
}

fn surface_reopen(session: Option<&HeldSession>, attached: &HashSet<SessionId>) -> SurfaceReopen {
    match session {
        None => SurfaceReopen::Fresh,
        Some(held) if attached.contains(&held.target.1) => SurfaceReopen::AlreadyOpen,
        Some(held) => SurfaceReopen::Reattach(held.target.1.clone()),
    }
}

fn take_closed_tab_record(records: &mut Vec<ClosedRecord>, tab_id: u64) -> Option<ClosedTabRecord> {
    let position = records
        .iter()
        .rposition(|record| matches!(record, ClosedRecord::Tab(tab) if tab.tab_id == tab_id))?;
    match records.remove(position) {
        ClosedRecord::Tab(tab) => Some(tab),
        ClosedRecord::Pane(_) => None,
    }
}

fn capture_closed_surface_record(
    pane: &gpui::Entity<crate::pane::Pane>,
    cx: &App,
) -> ClosedSurfaceRecord {
    let pane_ref = pane.read(cx);
    match pane_ref.surface() {
        crate::pane::PaneSurface::Terminal(tv) => {
            let tv_ref = tv.read(cx);
            ClosedSurfaceRecord::Terminal {
                cwd: tv_ref
                    .terminal
                    .current_cwd
                    .as_ref()
                    .map(std::path::PathBuf::from)
                    .or_else(|| tv_ref.terminal.cwd_now()),
                session: tv_ref
                    .terminal
                    .hosted_stop_target()
                    .map(|target| HeldSession {
                        target,
                        hold: SessionHold::Detached,
                    }),
                custom_name: tv_ref.terminal.custom_name.clone(),
                font_size: tv_ref.terminal.font_size_override,
            }
        }
        crate::pane::PaneSurface::Markdown(markdown) => ClosedSurfaceRecord::Markdown {
            path: markdown.read(cx).path.clone(),
        },
    }
}

fn capture_closed_pane_record(
    pane: &gpui::Entity<crate::pane::Pane>,
    workspace: &Workspace,
    cx: &App,
) -> ClosedPaneRecord {
    let tab = workspace
        .tab_for_pane(pane)
        .unwrap_or_else(|| workspace.active_tab());
    ClosedPaneRecord {
        surface: capture_closed_surface_record(pane, cx),
        workspace_id: workspace.id,
        tab_id: tab.id,
        worktree: tab.worktree.clone(),
    }
}

fn undo_pane_destination(
    workspaces: &[Workspace],
    active_idx: usize,
    record: &ClosedPaneRecord,
) -> Result<(usize, usize), String> {
    let ws_idx = workspaces
        .iter()
        .position(|ws| ws.id == record.workspace_id)
        .or((active_idx < workspaces.len()).then_some(active_idx))
        .ok_or_else(|| "No active workspace to restore pane".to_string())?;
    let ws = &workspaces[ws_idx];
    let tab_idx = ws
        .tabs()
        .iter()
        .position(|tab| tab.id == record.tab_id)
        .unwrap_or_else(|| ws.active_tab_idx());
    let tab = &ws.tabs()[tab_idx];
    if tab.is_zoomed() {
        return Err("Unzoom before restoring a closed pane".to_string());
    }
    if !tab.can_add_pane() {
        return Err(format!("Maximum pane count reached ({MAX_PANES})"));
    }
    Ok((ws_idx, tab_idx))
}

fn release_foreign_session(
    surface: &mut ClosedSurfaceRecord,
    origin: Option<&std::path::Path>,
    destination: Option<&std::path::Path>,
) -> Option<StopTarget> {
    if origin == destination {
        return None;
    }
    let ClosedSurfaceRecord::Terminal { session, .. } = surface else {
        return None;
    };
    session
        .take()
        .filter(is_undo_window)
        .map(|held| held.target)
}

fn capture_closed_tab_record(
    tab: &crate::workspace::Tab,
    workspace_id: u64,
    tab_idx: usize,
    cx: &App,
) -> Option<ClosedTabRecord> {
    let tree = tab.saved_layout.as_ref().or(tab.root.as_ref())?;
    let surfaces = tree
        .collect_leaves()
        .iter()
        .map(|pane| capture_closed_surface_record(pane, cx))
        .collect();
    Some(ClosedTabRecord {
        tab_id: tab.id,
        workspace_id,
        tab_idx,
        title: tab.title().to_string(),
        title_source: tab.title_source(),
        worktree: tab.worktree.clone(),
        layout: tree.serialize_without_scrollback(cx),
        surfaces,
    })
}

fn restore_closed_surface_record(
    tab: ClosedSurfaceRecord,
    ws_id: u64,
    worktree: Option<&std::path::Path>,
    attached: &HashSet<SessionId>,
    cx: &mut Context<PaneFlowApp>,
) -> (crate::pane::PaneSurface, SurfaceReopen) {
    match tab {
        ClosedSurfaceRecord::Terminal {
            cwd,
            session,
            custom_name,
            font_size,
        } => {
            let reopen = surface_reopen(session.as_ref(), attached);
            let terminal = cx.new(|cx| match &reopen {
                SurfaceReopen::Reattach(session) => {
                    TerminalView::attach_existing(ws_id, cwd, session.clone(), cx)
                }
                SurfaceReopen::Fresh | SurfaceReopen::AlreadyOpen => match worktree {
                    Some(worktree) => TerminalView::spawned(
                        ws_id,
                        crate::workspace::SpawnCwd::within(Some(worktree), cwd),
                        None,
                        TerminalSurfaceProfile::Normal,
                        cx,
                    ),
                    None => TerminalView::with_cwd(ws_id, cwd, None, cx),
                },
            });
            terminal.update(cx, |view, _| {
                view.terminal.custom_name = custom_name;
                view.terminal.font_size_override = font_size;
            });
            cx.subscribe(&terminal, PaneFlowApp::handle_terminal_event)
                .detach();
            (crate::pane::PaneSurface::Terminal(terminal), reopen)
        }
        ClosedSurfaceRecord::Markdown { path } => {
            let markdown = cx.new(|cx: &mut Context<crate::markdown::MarkdownView>| {
                crate::markdown::MarkdownView::open(path, cx)
            });
            (
                crate::pane::PaneSurface::Markdown(markdown),
                SurfaceReopen::Fresh,
            )
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SurfaceLaunch {
    pub(crate) command: Option<String>,
    pub(crate) env: Option<std::collections::HashMap<String, String>>,
}

impl PaneFlowApp {
    fn finish_surface_reopen(
        &mut self,
        surface: &crate::pane::PaneSurface,
        reopen: SurfaceReopen,
        ws_id: u64,
        cx: &mut Context<Self>,
    ) {
        match reopen {
            SurfaceReopen::Fresh => {}
            SurfaceReopen::Reattach(session) => {
                if let crate::pane::PaneSurface::Terminal(terminal) = surface {
                    let surface_id = terminal.entity_id().as_u64();
                    self.seed_surface_from_host(&session, ws_id, surface_id, cx);
                }
            }
            SurfaceReopen::AlreadyOpen => self.show_toast(
                "That session is already open in another view, so a new shell opened instead",
                cx,
            ),
        }
    }

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
        self.session_menu_open = None;
        self.sidebar_customize_menu_open = false;
        self.sidebar_show_submenu_open = false;
        self.tab_menu_open = None;
        self.pane_menu_open = None;
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
                self.workspaces[idx].reveal_pane(&pane, cx);
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
        self.save_session(cx);
        self.acknowledge_visible_completions(cx);
        self.refresh_owned_sessions(cx);
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
        cx.notify();
        changed
    }

    pub(crate) fn spawn_worktree_teardown(
        &self,
        worktrees: Vec<crate::workspace::worktree::ManagedWorktree>,
        cx: &mut Context<Self>,
    ) {
        if worktrees.is_empty() || !self.cached_config.worktrees.auto_remove_enabled() {
            return;
        }
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let kept = smol::unblock(move || {
                let worktrees = match crate::terminal::host_link::live_sessions() {
                    crate::terminal::host_link::LiveSessionProbe::Sessions(sessions) => {
                        let cwds: Vec<std::path::PathBuf> =
                            sessions.into_iter().map(|session| session.cwd).collect();
                        crate::workspace::worktree::without_live_sessions(worktrees, &cwds)
                    }
                    crate::terminal::host_link::LiveSessionProbe::NoHost => worktrees,
                    crate::terminal::host_link::LiveSessionProbe::Unknown(error) => {
                        log::warn!("worktree teardown skipped: live session probe failed: {error}");
                        return Vec::new();
                    }
                };
                crate::workspace::worktree::teardown_all(worktrees)
            })
            .await;
            if kept.is_empty() {
                return;
            }
            let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                for message in kept {
                    app.show_toast(message, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_workspace_folders(
        &mut self,
        paths: &[std::path::PathBuf],
        cx: &mut Context<Self>,
    ) {
        let mut opened = false;
        for path in paths {
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
            self.workspaces.push(ws);
            self.active_idx = self.workspaces.len() - 1;
            opened = true;
        }
        if !opened {
            return;
        }
        self.record_recent_workspaces(paths, cx);
        self.save_session(cx);
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
            cx.new(|cx| TerminalView::spawned(ws_id, source_cwd, launch.env, profile, cx));
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
        if let Some(command) = launch.command.as_deref() {
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
        let Some(workspace) = self.active_workspace() else {
            return;
        };
        let record = capture_closed_pane_record(&pane, workspace, cx);
        let evicted = push_closed_record(&mut self.closed_panes, ClosedRecord::Pane(record));
        self.stop_hosted_sessions(evicted, cx);
        pane.read(cx).focus_handle(cx).focus(window, cx);

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
            let terminal = cx.new(|cx| {
                TerminalView::spawned(
                    ws_id,
                    cwd,
                    None,
                    paneflow_config::schema::TerminalSurfaceProfile::Normal,
                    cx,
                )
            });
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
        let record = match self.closed_panes.pop() {
            Some(ClosedRecord::Pane(record)) => record,
            Some(ClosedRecord::Tab(record)) => {
                self.restore_closed_tab(record, window, cx);
                return;
            }
            None => {
                self.show_toast("No closed pane to restore", cx);
                return;
            }
        };

        let (ws_idx, tab_idx) =
            match undo_pane_destination(&self.workspaces, self.active_idx, &record) {
                Ok(destination) => destination,
                Err(message) => {
                    self.closed_panes.push(ClosedRecord::Pane(record));
                    self.show_toast(message, cx);
                    return;
                }
            };
        self.dismiss_transient_surfaces();
        self.active_idx = ws_idx;
        self.workspaces[ws_idx].set_active_tab(tab_idx);
        let ws_id = self.workspaces[ws_idx].id;
        let worktree = self.workspaces[ws_idx].active_tab().worktree.clone();
        let ClosedPaneRecord {
            mut surface,
            worktree: origin,
            ..
        } = record;
        if let Some(target) =
            release_foreign_session(&mut surface, origin.as_deref(), worktree.as_deref())
        {
            self.stop_hosted_sessions(vec![target], cx);
        }
        let attached = self.attached_session_ids(cx);
        let (surface, reopen) =
            restore_closed_surface_record(surface, ws_id, worktree.as_deref(), &attached, cx);
        self.finish_surface_reopen(&surface, reopen, ws_id, cx);
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
        if let Some(dir) = self.workspaces[idx].git_dir.clone() {
            self.unwatch_git_dir(&dir);
        }
        let worktrees = std::mem::take(&mut self.workspaces[idx].managed_worktrees);
        self.prune_worktree_states();
        self.spawn_worktree_teardown(worktrees, cx);
        let removed = self.workspaces.remove(idx);
        if self
            .renaming_tab
            .is_some_and(|key| key.workspace_id == removed.id)
        {
            self.renaming_tab = None;
        }
        self.dismiss_transient_surfaces();
        if self.workspaces.is_empty() {
            self.active_idx = 0;
        } else {
            if self.active_idx >= self.workspaces.len() {
                self.active_idx = self.workspaces.len() - 1;
            } else if self.active_idx > idx {
                self.active_idx -= 1;
            }
            if let Some(window) = window {
                self.workspaces[self.active_idx].focus_first(window, cx);
            }
        }
        self.save_session(cx);
        cx.notify();
        self.refresh_composer_slot(cx);
        self.sync_broadcast_stripes(cx);
        self.flush_pending_prefill(cx);
        self.sync_pending_chips(cx);
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
        if commit_tab_title(&mut self.workspaces, key, &text) {
            self.save_session(cx);
        }
    }

    pub(crate) fn handle_next_workspace(
        &mut self,
        _: &NextWorkspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let order = Self::compute_display_order(&self.workspaces);
        if let Some(next) = next_in_display_order(&order, self.active_idx) {
            self.select_workspace(next, window, cx);
        }
    }

    pub(crate) fn handle_select_ws(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspaces.is_empty() {
            self.open_recent_workspace(idx, window, cx);
            return;
        }
        if let Some(target) = Self::compute_display_order(&self.workspaces).get(idx) {
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

fn next_in_display_order(order: &[usize], active: usize) -> Option<usize> {
    let position = order.iter().position(|&idx| idx == active);
    let next = position.map_or(0, |position| (position + 1) % order.len());
    order.get(next).copied()
}

fn commit_tab_title(workspaces: &mut [Workspace], key: crate::TabKey, text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return false;
    }
    let Some((ws_idx, tab_idx)) = key.resolve(workspaces) else {
        return false;
    };
    workspaces[ws_idx]
        .tab_mut(tab_idx)
        .is_some_and(|tab| tab.set_title(text, TabTitleSource::User))
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

    fn terminal_surface(session: Option<HeldSession>) -> ClosedSurfaceRecord {
        ClosedSurfaceRecord::Terminal {
            cwd: None,
            session,
            custom_name: None,
            font_size: None,
        }
    }

    fn closed_pane_record(session: Option<HeldSession>) -> ClosedRecord {
        ClosedRecord::Pane(pane_record(session, 1, 1, None))
    }

    fn pane_record(
        session: Option<HeldSession>,
        workspace_id: u64,
        tab_id: u64,
        worktree: Option<&str>,
    ) -> ClosedPaneRecord {
        ClosedPaneRecord {
            surface: terminal_surface(session),
            workspace_id,
            tab_id,
            worktree: worktree.map(std::path::PathBuf::from),
        }
    }

    #[gpui::test]
    fn undo_close_resolves_the_workspace_by_id_and_refuses_a_zoomed_or_full_tab(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::workspace::Tab;

        let cx = cx.add_empty_window();
        let pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        };
        let origin = Tab::new("origin", Some(LayoutTree::Leaf(pane(cx))));
        let origin_id = origin.id;
        let zoomed = pane(cx);
        let mut zoomed_tab = Tab::new("zoomed", Some(LayoutTree::Leaf(zoomed.clone())));
        let mut saved = LayoutTree::Leaf(zoomed);
        saved.split_first_leaf(SplitDirection::Horizontal, pane(cx));
        zoomed_tab.saved_layout = Some(saved);
        let zoomed_id = zoomed_tab.id;
        let mut full = LayoutTree::Leaf(pane(cx));
        for _ in 1..MAX_PANES {
            full.split_first_leaf(SplitDirection::Horizontal, pane(cx));
        }
        let full_tab = Tab::new("full", Some(full));
        let full_id = full_tab.id;
        let workspaces = vec![
            Workspace::restored_with_id(
                7,
                "first",
                std::path::PathBuf::new(),
                vec![Tab::new("spare", Some(LayoutTree::Leaf(pane(cx))))],
                0,
            ),
            Workspace::restored_with_id(
                9,
                "second",
                std::path::PathBuf::new(),
                vec![origin, zoomed_tab, full_tab],
                1,
            ),
        ];
        let unzoom = Err("Unzoom before restoring a closed pane".to_string());

        assert_eq!(
            undo_pane_destination(&workspaces, 0, &pane_record(None, 9, origin_id, None)),
            Ok((1, 0)),
            "the record follows its workspace id, not the active index"
        );
        assert_eq!(
            undo_pane_destination(&workspaces, 0, &pane_record(None, 9, zoomed_id, None)),
            unzoom
        );
        assert_eq!(
            undo_pane_destination(&workspaces, 0, &pane_record(None, 9, 404, None)),
            unzoom,
            "a vanished tab falls back to the active tab, which is zoomed here"
        );
        assert_eq!(
            undo_pane_destination(&workspaces, 0, &pane_record(None, 9, full_id, None)),
            Err(format!("Maximum pane count reached ({MAX_PANES})"))
        );
        assert_eq!(
            undo_pane_destination(&workspaces, 0, &pane_record(None, 42, origin_id, None)),
            Ok((0, 0)),
            "a closed workspace falls back to the active one"
        );
    }

    #[test]
    fn a_pane_restored_into_another_worktree_opens_a_confined_shell() {
        let session = SessionId::new();
        let mut same = terminal_surface(Some(held(&session, SessionHold::Detached)));
        assert_eq!(
            release_foreign_session(
                &mut same,
                Some(std::path::Path::new("/repo/wt-a")),
                Some(std::path::Path::new("/repo/wt-a")),
            ),
            None
        );
        assert!(matches!(
            same,
            ClosedSurfaceRecord::Terminal {
                session: Some(_),
                ..
            }
        ));

        let until = Instant::now() + std::time::Duration::from_secs(30);
        let mut undo = terminal_surface(Some(held(&session, SessionHold::UndoWindow { until })));
        let stopped =
            release_foreign_session(&mut undo, None, Some(std::path::Path::new("/repo/wt-b")));
        assert_eq!(stopped.map(|target| target.1), Some(session.clone()));
        assert!(matches!(
            undo,
            ClosedSurfaceRecord::Terminal { session: None, .. }
        ));

        let mut detached = terminal_surface(Some(held(&session, SessionHold::Detached)));
        assert_eq!(
            release_foreign_session(
                &mut detached,
                Some(std::path::Path::new("/repo/wt-a")),
                Some(std::path::Path::new("/repo/wt-b")),
            ),
            None,
            "a detached session stays listed instead of being stopped"
        );
        assert!(matches!(
            detached,
            ClosedSurfaceRecord::Terminal { session: None, .. }
        ));

        let spawn = crate::workspace::SpawnCwd::within(
            Some(std::path::Path::new("/repo/wt-b")),
            Some(std::path::PathBuf::from("/repo/wt-a/src")),
        );
        assert_eq!(
            spawn.confine_to.as_deref(),
            Some(std::path::Path::new("/repo/wt-b"))
        );
        assert!(spawn.needs_resolving());
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
        let order = PaneFlowApp::compute_display_order(&workspaces);
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
        assert_eq!(next_in_display_order(&order, 0), Some(2));
        assert_eq!(next_in_display_order(&order, 2), Some(1));
        assert_eq!(next_in_display_order(&order, 1), Some(0));
        assert_eq!(next_in_display_order(&[], 0), None);
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
        let before = titles(&workspaces);
        assert!(!commit_tab_title(&mut workspaces, key, "typed title"));
        assert_eq!(titles(&workspaces), before);

        let mut workspaces = vec![named_workspace(1, None, &["left", "renamed"])];
        let key = crate::TabKey::of(&workspaces[0], 1).expect("the tab exists");
        workspaces[0].close_tab(0);
        assert!(commit_tab_title(&mut workspaces, key, "  typed title  "));
        assert_eq!(titles(&workspaces), vec!["typed title".to_string()]);
    }

    fn held(session: &SessionId, hold: SessionHold) -> HeldSession {
        HeldSession {
            target: (
                std::path::PathBuf::from("host-endpoint"),
                session.clone(),
                paneflow_config::schema::SessionGeneration::default(),
            ),
            hold,
        }
    }

    fn first_session(record: &ClosedRecord) -> Option<&HeldSession> {
        match &record.surfaces()[0] {
            ClosedSurfaceRecord::Terminal { session, .. } => session.as_ref(),
            ClosedSurfaceRecord::Markdown { .. } => None,
        }
    }

    fn stopped_sessions(targets: &[StopTarget]) -> Vec<SessionId> {
        targets
            .iter()
            .map(|(_, session, _)| session.clone())
            .collect()
    }

    fn closed_tab_record(tab_id: u64, surfaces: usize) -> ClosedRecord {
        ClosedRecord::Tab(ClosedTabRecord {
            tab_id,
            workspace_id: 1,
            tab_idx: 0,
            title: String::new(),
            title_source: TabTitleSource::Preset,
            worktree: None,
            layout: LayoutNode::Pane {
                surfaces: Vec::new(),
            },
            surfaces: (0..surfaces).map(|_| terminal_surface(None)).collect(),
        })
    }

    fn tab_ids(records: &[ClosedRecord]) -> Vec<Option<u64>> {
        records
            .iter()
            .map(|record| match record {
                ClosedRecord::Tab(tab) => Some(tab.tab_id),
                ClosedRecord::Pane(_) => None,
            })
            .collect()
    }

    #[test]
    fn a_silent_close_keeps_the_session_for_undo_until_its_deadline_then_stops_it() {
        let session = SessionId::new();
        let mut records = vec![closed_pane_record(Some(held(
            &session,
            SessionHold::Detached,
        )))];
        let closed_at = Instant::now();
        let until = closed_at + std::time::Duration::from_millis(crate::CLOSED_SESSION_GRACE_MS);

        let kept = settle_closed_sessions(
            &mut records,
            &HashSet::from([session.clone()]),
            Some(SessionHold::UndoWindow { until }),
        );

        assert_eq!(kept, HashSet::from([session.clone()]));
        assert_eq!(
            undo_window_session_ids(&records),
            HashSet::from([session.clone()])
        );
        assert_eq!(
            surface_reopen(first_session(&records[0]), &HashSet::new()),
            SurfaceReopen::Reattach(session.clone()),
            "an undo inside the window reattaches the same session"
        );
        assert!(
            expire_held_sessions(&mut records, until - std::time::Duration::from_millis(1))
                .is_empty(),
            "nothing stops before the deadline"
        );

        let expired = expire_held_sessions(&mut records, until);

        assert_eq!(stopped_sessions(&expired), vec![session]);
        assert!(first_session(&records[0]).is_none());
        assert!(undo_window_session_ids(&records).is_empty());
        assert_eq!(
            surface_reopen(first_session(&records[0]), &HashSet::new()),
            SurfaceReopen::Fresh,
            "after the deadline the undo opens a new shell"
        );
    }

    #[test]
    fn an_explicit_stop_forgets_the_session_so_undo_opens_a_new_shell() {
        let session = SessionId::new();
        let mut records = vec![closed_pane_record(Some(held(
            &session,
            SessionHold::Detached,
        )))];

        let kept = settle_closed_sessions(&mut records, &HashSet::from([session]), None);

        assert!(kept.is_empty());
        assert_eq!(
            surface_reopen(first_session(&records[0]), &HashSet::new()),
            SurfaceReopen::Fresh
        );
    }

    #[test]
    fn a_detached_close_never_expires_stays_listed_and_undo_reattaches() {
        let session = SessionId::new();
        let mut records = vec![closed_pane_record(Some(held(
            &session,
            SessionHold::Detached,
        )))];

        let kept = settle_closed_sessions(
            &mut records,
            &HashSet::from([session.clone()]),
            Some(SessionHold::Detached),
        );

        assert_eq!(kept, HashSet::from([session.clone()]));
        assert!(
            expire_held_sessions(
                &mut records,
                Instant::now() + std::time::Duration::from_secs(3600)
            )
            .is_empty()
        );
        assert!(
            undo_window_session_ids(&records).is_empty(),
            "a detached session stays in the sidebar"
        );
        assert!(take_undo_window_sessions(&mut records).is_empty());
        assert_eq!(
            surface_reopen(first_session(&records[0]), &HashSet::new()),
            SurfaceReopen::Reattach(session)
        );
    }

    #[test]
    fn a_session_reclosed_after_a_sidebar_reopen_survives_the_undo_of_its_latest_close() {
        let session = SessionId::new();
        let mut records = Vec::new();
        push_closed_record(
            &mut records,
            closed_pane_record(Some(held(&session, SessionHold::Detached))),
        );
        push_closed_record(
            &mut records,
            closed_pane_record(Some(held(&session, SessionHold::Detached))),
        );
        let until =
            Instant::now() + std::time::Duration::from_millis(crate::CLOSED_SESSION_GRACE_MS);
        settle_closed_sessions(
            &mut records,
            &HashSet::from([session.clone()]),
            Some(SessionHold::UndoWindow { until }),
        );

        let undone = records.pop().expect("the latest close");
        assert_eq!(
            surface_reopen(first_session(&undone), &HashSet::new()),
            SurfaceReopen::Reattach(session)
        );
        assert!(
            first_session(&records[0]).is_none(),
            "the earlier close no longer owns the session"
        );
        assert!(
            expire_held_sessions(&mut records, until).is_empty(),
            "the reattached session must not be stopped by the earlier close"
        );
    }

    #[test]
    fn a_close_that_leaves_nothing_to_undo_keeps_no_session() {
        let recorded = SessionId::new();
        let unrecorded = SessionId::new();
        let mut records = vec![closed_pane_record(Some(held(
            &recorded,
            SessionHold::Detached,
        )))];

        let kept = settle_closed_sessions(
            &mut records,
            &HashSet::from([unrecorded]),
            Some(SessionHold::UndoWindow {
                until: Instant::now(),
            }),
        );

        assert!(kept.is_empty(), "the caller stops what no record keeps");
        assert_eq!(
            first_session(&records[0]).map(|held| held.hold),
            Some(SessionHold::Detached),
            "an unrelated record is untouched"
        );
    }

    #[test]
    fn evicting_the_oldest_record_stops_only_its_undo_window_sessions() {
        let held_for_undo = SessionId::new();
        let detached = SessionId::new();
        let until = Instant::now() + std::time::Duration::from_secs(60);
        let mut records = vec![
            closed_pane_record(Some(held(
                &held_for_undo,
                SessionHold::UndoWindow { until },
            ))),
            closed_pane_record(Some(held(&detached, SessionHold::Detached))),
        ];
        for tab_id in 0..(MAX_CLOSED_PANES - 2) as u64 {
            assert!(push_closed_record(&mut records, closed_tab_record(tab_id, 1)).is_empty());
        }

        let first_eviction = push_closed_record(&mut records, closed_pane_record(None));
        let second_eviction = push_closed_record(&mut records, closed_pane_record(None));

        assert_eq!(stopped_sessions(&first_eviction), vec![held_for_undo]);
        assert!(
            second_eviction.is_empty(),
            "a detached session outlives its undo entry"
        );
        assert_eq!(records.len(), MAX_CLOSED_PANES);
    }

    #[test]
    fn quitting_takes_every_undo_window_session_and_leaves_detached_ones() {
        let (first, second, detached) = (SessionId::new(), SessionId::new(), SessionId::new());
        let until = Instant::now() + std::time::Duration::from_secs(60);
        let mut records = vec![
            closed_pane_record(Some(held(&first, SessionHold::UndoWindow { until }))),
            closed_pane_record(Some(held(&detached, SessionHold::Detached))),
            closed_pane_record(Some(held(&second, SessionHold::UndoWindow { until }))),
        ];

        let taken = take_undo_window_sessions(&mut records);

        assert_eq!(stopped_sessions(&taken), vec![first, second]);
        assert!(undo_window_session_ids(&records).is_empty());
        assert_eq!(
            first_session(&records[1]).map(|held| held.target.1.clone()),
            Some(detached)
        );
    }

    #[test]
    fn undo_never_attaches_a_session_that_another_view_already_shows() {
        let session = SessionId::new();
        let held = held(&session, SessionHold::Detached);

        assert_eq!(
            surface_reopen(Some(&held), &HashSet::from([session])),
            SurfaceReopen::AlreadyOpen
        );
        assert_eq!(surface_reopen(None, &HashSet::new()), SurfaceReopen::Fresh);
    }

    #[test]
    fn closed_record_cap_counts_a_tab_as_one_entry() {
        let mut records = Vec::new();
        for tab_id in 0..MAX_CLOSED_PANES as u64 {
            push_closed_record(&mut records, closed_tab_record(tab_id, 3));
        }
        push_closed_record(&mut records, closed_pane_record(None));

        assert_eq!(records.len(), MAX_CLOSED_PANES);
        assert_eq!(
            tab_ids(&records)[0],
            Some(1),
            "the oldest record is evicted"
        );
        assert_eq!(tab_ids(&records).last(), Some(&None));
    }

    #[test]
    fn take_closed_tab_record_removes_only_the_matching_tab() {
        let mut records = vec![
            closed_tab_record(3, 1),
            closed_pane_record(None),
            closed_tab_record(4, 2),
        ];

        let taken = take_closed_tab_record(&mut records, 3).map(|tab| tab.tab_id);

        assert_eq!(taken, Some(3));
        assert_eq!(tab_ids(&records), vec![None, Some(4)]);
        assert!(take_closed_tab_record(&mut records, 3).is_none());
        assert!(take_closed_tab_record(&mut records, 99).is_none());
        assert_eq!(records.len(), 2);
    }

    #[gpui::test]
    fn closed_tab_record_keeps_every_pane_in_layout_order(cx: &mut gpui::TestAppContext) {
        use crate::workspace::Tab;

        let cx = cx.add_empty_window();
        let pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        };
        let leaf = || LayoutNode::Pane {
            surfaces: Vec::new(),
        };
        let split =
            |direction: &str, ratios: Vec<f64>, children: Vec<LayoutNode>| LayoutNode::Split {
                direction: direction.to_string(),
                ratio: None,
                ratios: Some(ratios),
                children,
            };
        let skeleton = split(
            "vertical",
            vec![0.25, 0.75],
            vec![
                leaf(),
                split("horizontal", vec![0.5, 0.5], vec![leaf(), leaf()]),
            ],
        );
        let (a, b, c) = (pane(cx), pane(cx), pane(cx));
        let mut original: std::collections::VecDeque<_> = vec![a, b, c].into();
        let tree = LayoutTree::from_layout_node(&skeleton, &mut original, &mut |_| {
            unreachable!("the skeleton has three leaves")
        });
        let mut tab = Tab::new("build", Some(tree));
        tab.worktree = Some(std::path::PathBuf::from("wt"));

        let record = cx
            .update(|_, cx| capture_closed_tab_record(&tab, 9, 2, cx))
            .expect("a tab with panes yields a record");

        assert_eq!(record.tab_id, tab.id);
        assert_eq!(record.workspace_id, 9);
        assert_eq!(record.tab_idx, 2);
        assert_eq!(record.title, "build");
        assert_eq!(record.worktree, tab.worktree);
        assert_eq!(record.surfaces.len(), 3);

        let (x, y, z) = (pane(cx), pane(cx), pane(cx));
        let mut fresh: std::collections::VecDeque<_> = vec![x.clone(), y.clone(), z.clone()].into();
        let rebuilt = LayoutTree::from_layout_node(&record.layout, &mut fresh, &mut |_| {
            unreachable!("every leaf has a captured surface")
        });

        assert!(fresh.is_empty());
        assert_eq!(rebuilt.collect_leaves(), vec![x, y, z]);
        let rebuilt_layout = cx.update(|_, cx| rebuilt.serialize_without_scrollback(cx));
        assert_eq!(
            layout_shape(&rebuilt_layout),
            "vertical[0.25,0.75](pane,horizontal[0.5,0.5](pane,pane))"
        );
        assert_eq!(layout_shape(&record.layout), layout_shape(&rebuilt_layout));
    }

    fn layout_shape(node: &LayoutNode) -> String {
        match node {
            LayoutNode::Pane { .. } => "pane".to_string(),
            LayoutNode::Split {
                direction,
                children,
                ..
            } => {
                let ratios: Vec<String> = node
                    .resolved_ratios()
                    .iter()
                    .map(|ratio| ratio.to_string())
                    .collect();
                let children: Vec<String> = children.iter().map(layout_shape).collect();
                format!("{direction}[{}]({})", ratios.join(","), children.join(","))
            }
        }
    }

    #[gpui::test]
    fn closed_tab_record_is_absent_for_an_empty_tab(cx: &mut gpui::TestAppContext) {
        let tab = crate::workspace::Tab::empty();

        let record = cx.update(|cx| capture_closed_tab_record(&tab, 1, 0, cx));

        assert!(record.is_none());
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
