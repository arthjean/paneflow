use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

use gpui::{App, AppContext, Context, Entity, Focusable, Window};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::{HostClientError, SessionLifecycle};

use crate::app::close_policy::{CloseIntent, CloseTarget};
use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;
use crate::terminal::host_link::{self, HostLinkState};
use crate::workspace::{Tab, Workspace};
use crate::{HidePane, PaneFlowApp, RemoveEndedSessions, ResumeEndedSessions, StopSession};

pub(crate) type StopTarget = (PathBuf, SessionId, SessionGeneration);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedSession {
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
    pub(crate) workspace: Option<WorkspaceId>,
    pub(crate) title: Option<String>,
    pub(crate) cwd: String,
    pub(crate) live: bool,
    pub(crate) lifecycle: SessionLifecycle,
    pub(crate) updated_at_ms: u64,
}

impl OwnedSession {
    pub(crate) fn label(&self) -> String {
        self.title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| {
                std::path::Path::new(&self.cwd)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| self.cwd.clone())
            })
    }
}

#[derive(Default)]
pub(crate) struct OwnedSessions {
    rows: Vec<OwnedSession>,
    stale: bool,
    unknown: HashSet<SessionId>,
    forgetting: HashSet<SessionId>,
    expanded: HashSet<WorkspaceId>,
    refresh_generation: u64,
}

pub(crate) fn pane_terminals(pane: &Entity<Pane>, cx: &App) -> Vec<Entity<TerminalView>> {
    pane.read(cx).terminals().cloned().collect()
}

pub(crate) fn tab_terminals(tab: &Tab, cx: &App) -> Vec<Entity<TerminalView>> {
    tab.collect_panes()
        .iter()
        .flat_map(|pane| pane_terminals(pane, cx))
        .collect()
}

pub(crate) fn surface_terminal(surface: &PaneSurface) -> Option<Entity<TerminalView>> {
    match surface {
        PaneSurface::Terminal(terminal) => Some(terminal.clone()),
        PaneSurface::Markdown(_) => None,
    }
}

pub(crate) fn order_session_rows(rows: &mut [OwnedSession]) {
    rows.sort_by(|a, b| {
        b.live
            .cmp(&a.live)
            .then(b.updated_at_ms.cmp(&a.updated_at_ms))
            .then(a.session.as_str().cmp(b.session.as_str()))
    });
}

pub(crate) fn relative_age(now_ms: u64, updated_at_ms: u64) -> String {
    let seconds = now_ms.saturating_sub(updated_at_ms) / 1000;
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

pub(crate) fn lifecycle_sentence(lifecycle: &SessionLifecycle) -> String {
    match lifecycle {
        SessionLifecycle::Starting => "Starting".to_string(),
        SessionLifecycle::Running => "Running".to_string(),
        SessionLifecycle::Exited { code, signal } => match signal {
            Some(signal) => format!("Exited with code {code} ({signal})"),
            None => format!("Exited with code {code}"),
        },
        SessionLifecycle::Failed { reason } => format!("Failed to start: {reason}"),
        SessionLifecycle::Lost => "Lost: its process did not survive".to_string(),
    }
}

fn visible_session_rows(
    rows: &[OwnedSession],
    workspace: &WorkspaceId,
    attached: &HashSet<SessionId>,
    forgetting: &HashSet<SessionId>,
) -> Vec<OwnedSession> {
    let mut visible: Vec<OwnedSession> = rows
        .iter()
        .filter(|session| session.workspace.as_ref() == Some(workspace))
        .filter(|session| !attached.contains(&session.session))
        .filter(|session| !forgetting.contains(&session.session))
        .cloned()
        .collect();
    order_session_rows(&mut visible);
    visible
}

fn stop_and_forget(targets: Vec<StopTarget>) -> Vec<SessionId> {
    let mut failures = Vec::new();
    for (endpoint, session, generation) in targets {
        match host_link::stop_session(&endpoint, &session, generation) {
            Ok(summary) => {
                log::info!(
                    "paneflow: hosted session {session} stopped ({})",
                    summary.manifest.lifecycle.label()
                );
                match host_link::remove_session(&endpoint, &session) {
                    Ok(()) => log::info!("paneflow: hosted session {session} record removed"),
                    Err(error) => log::warn!(
                        "paneflow: hosted session {session} stopped but its record remains: {error}"
                    ),
                }
            }
            Err(error) => {
                log::warn!("paneflow: hosted session {session} stop outcome unknown: {error}");
                failures.push(session);
            }
        }
    }
    failures
}

impl PaneFlowApp {
    pub(crate) fn stop_terminals(
        &mut self,
        terminals: Vec<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        let mut targets = Vec::new();
        let mut unknown = 0usize;
        for terminal in &terminals {
            let state = &terminal.read(cx).terminal;
            if state.hosted.is_none() {
                continue;
            }
            if matches!(state.host_link, HostLinkState::Unavailable(_)) {
                unknown += 1;
                continue;
            }
            if let Some(target) = state.hosted_stop_target() {
                targets.push(target);
            }
        }
        if unknown > 0 {
            self.show_toast(
                format!(
                    "{unknown} view(s) closed while the local host was unreachable: their session state is unknown and no stop was attempted."
                ),
                cx,
            );
        }
        self.stop_hosted_sessions(targets, cx);
    }

    pub(crate) fn stop_listed_session(&mut self, session: &SessionId, cx: &mut Context<Self>) {
        let Some(target) = self.stop_target_for_listed_session(session, cx) else {
            return;
        };
        self.stop_hosted_sessions(vec![target], cx);
    }

    fn stop_target_for_listed_session(&self, session: &SessionId, cx: &App) -> Option<StopTarget> {
        let attached = self
            .attached_terminals(cx)
            .into_iter()
            .find_map(|terminal| {
                let state = &terminal.read(cx).terminal;
                (state.session_id == *session).then(|| state.hosted_stop_target())?
            });
        if attached.is_some() {
            return attached;
        }
        let endpoint = host_link::host_endpoint()?.endpoint;
        let row = self
            .owned_sessions
            .rows
            .iter()
            .find(|row| &row.session == session)?;
        Some((endpoint, row.session.clone(), row.generation))
    }

    fn stop_hosted_sessions(&mut self, targets: Vec<StopTarget>, cx: &mut Context<Self>) {
        if targets.is_empty() {
            return;
        }
        let attempted: Vec<SessionId> = targets
            .iter()
            .map(|(_, session, _)| session.clone())
            .collect();
        self.owned_sessions
            .forgetting
            .extend(attempted.iter().cloned());
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let failures = executor.spawn(async move { stop_and_forget(targets) }).await;
            let _ = this.update(cx, |app, cx| {
                for session in &attempted {
                    app.owned_sessions.forgetting.remove(session);
                }
                app.owned_sessions.rows.retain(|row| {
                    failures.contains(&row.session) || !attempted.contains(&row.session)
                });
                if !failures.is_empty() {
                    app.show_toast(
                        format!(
                            "{} session(s) may still be running: the stop request did not complete. The session list reconciles it.",
                            failures.len()
                        ),
                        cx,
                    );
                    for session in failures {
                        app.owned_sessions.unknown.insert(session);
                    }
                }
                app.refresh_owned_sessions(cx);
            });
        })
        .detach();
    }

    pub(crate) fn remove_listed_session(&mut self, session: SessionId, cx: &mut Context<Self>) {
        let Some(endpoint) = host_link::host_endpoint().map(|target| target.endpoint) else {
            return;
        };
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let outcome = executor
                .spawn({
                    let session = session.clone();
                    async move { host_link::remove_session(&endpoint, &session) }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                match outcome {
                    Ok(()) => {
                        app.owned_sessions.unknown.remove(&session);
                    }
                    Err(HostClientError::Rpc { message, .. }) => {
                        app.show_toast(format!("The session was not removed: {message}"), cx)
                    }
                    Err(error) => {
                        app.owned_sessions.unknown.insert(session.clone());
                        app.show_toast(
                            format!("The removal outcome is unknown: {error}. The session list reconciles it."),
                            cx,
                        )
                    }
                }
                app.refresh_owned_sessions(cx);
            });
        })
        .detach();
    }

    pub(crate) fn handle_hide_pane(
        &mut self,
        _: &HidePane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(pane) = self
            .nav_root()
            .and_then(|root| root.focused_pane(window, cx))
        {
            self.hide_pane_from_layout(pane, cx);
        }
    }

    pub(crate) fn handle_stop_session(
        &mut self,
        _: &StopSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .nav_root()
            .and_then(|root| root.focused_pane(window, cx))
        else {
            return;
        };
        let Some(terminal) = pane.read(cx).active_terminal_opt().cloned() else {
            return;
        };
        if terminal.read(cx).terminal.hosted.is_none() {
            return;
        }
        let session = terminal.read(cx).terminal.session_id.clone();
        self.request_close(CloseTarget::Session(session), Some(window), cx);
    }

    pub(crate) fn handle_resume_ended_sessions(
        &mut self,
        _: &ResumeEndedSessions,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resume_ended_sessions_in_workspace(self.active_idx, cx);
    }

    pub(crate) fn handle_remove_ended_sessions(
        &mut self,
        _: &RemoveEndedSessions,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(self.active_idx) else {
            return;
        };
        let ended: Vec<SessionId> = self
            .owned_sessions_for_workspace(ws, cx)
            .into_iter()
            .filter(|session| !session.live)
            .map(|session| session.session)
            .collect();
        if ended.is_empty() {
            self.show_toast("No ended session to remove in this workspace", cx);
            return;
        }
        for session in ended {
            self.remove_listed_session(session, cx);
        }
    }

    pub(crate) fn resume_ended_sessions_in_workspace(
        &mut self,
        ws_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let resuming = self.resume_ended_panes(ws_idx, cx);
        if resuming.is_empty() {
            self.show_toast("No ended session to resume in this workspace", cx);
            return;
        }
        self.track_resume_batch(resuming, cx);
    }

    pub(crate) fn hide_pane_from_layout(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        let hidden = pane_terminals(&pane, cx)
            .iter()
            .filter(|terminal| terminal.read(cx).terminal.hosted.is_some())
            .count();
        self.perform_close(CloseTarget::Pane(pane), CloseIntent::Detach, None, cx);
        if hidden > 0 {
            self.show_toast(
                format!(
                    "{hidden} session(s) hidden from the layout and still running; reopen them from the sidebar."
                ),
                cx,
            );
        }
    }

    pub(crate) fn refresh_owned_sessions(&mut self, cx: &mut Context<Self>) {
        self.owned_sessions.refresh_generation =
            self.owned_sessions.refresh_generation.wrapping_add(1);
        let generation = self.owned_sessions.refresh_generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let listed = executor
                .spawn(async move { host_link::list_sessions(None) })
                .await;
            let rows = match listed {
                Ok(sessions) => Some(
                    sessions
                        .into_iter()
                        .filter(|summary| summary.owned)
                        .map(|summary| OwnedSession {
                            session: summary.manifest.session,
                            generation: summary.manifest.generation,
                            workspace: summary.manifest.workspace,
                            title: summary.manifest.title,
                            cwd: summary.manifest.current_cwd.unwrap_or(summary.manifest.cwd),
                            live: summary.live,
                            lifecycle: summary.manifest.lifecycle,
                            updated_at_ms: summary.manifest.updated_at_ms,
                        })
                        .collect::<Vec<_>>(),
                ),
                Err(error) => {
                    log::debug!("paneflow: session refresh kept the previous list: {error}");
                    None
                }
            };
            let _ = this.update(cx, |app, cx| {
                if app.owned_sessions.refresh_generation != generation {
                    return;
                }
                match rows {
                    Some(mut rows) => {
                        order_session_rows(&mut rows);
                        app.owned_sessions.rows = rows;
                        app.owned_sessions.stale = false;
                        app.owned_sessions.unknown.clear();
                    }
                    None => app.owned_sessions.stale = true,
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn attached_terminals(&self, cx: &App) -> Vec<Entity<TerminalView>> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .flat_map(|pane| pane_terminals(&pane, cx))
            .chain(self.diff_dock.diff_tabs.iter().filter_map(|tab| match tab {
                crate::app::diff_dock::DiffDockTab::Terminal(terminal) => Some(terminal.clone()),
                _ => None,
            }))
            .collect()
    }

    pub(crate) fn attached_session_ids(&self, cx: &App) -> HashSet<SessionId> {
        self.attached_terminals(cx)
            .into_iter()
            .map(|terminal| terminal.read(cx).terminal.session_id.clone())
            .collect()
    }

    pub(crate) fn live_owned_sessions(&self) -> impl Iterator<Item = &OwnedSession> {
        self.owned_sessions.rows.iter().filter(|row| row.live)
    }

    pub(crate) fn owned_sessions_are_stale(&self) -> bool {
        self.owned_sessions.stale
    }

    pub(crate) fn session_outcome_unknown(&self, session: &SessionId) -> bool {
        self.owned_sessions.unknown.contains(session)
    }

    pub(crate) fn listed_session_label(&self, session: &SessionId) -> String {
        self.owned_sessions
            .rows
            .iter()
            .find(|row| &row.session == session)
            .map(OwnedSession::label)
            .unwrap_or_else(|| session.to_string())
    }

    pub(crate) fn ended_sessions_expanded(&self, ws: &Workspace) -> bool {
        self.owned_sessions.expanded.contains(&ws.durable_id)
    }

    pub(crate) fn expand_ended_sessions(&mut self, workspace: WorkspaceId, cx: &mut Context<Self>) {
        self.owned_sessions.expanded.insert(workspace);
        cx.notify();
    }

    pub(crate) fn owned_sessions_for_workspace(
        &self,
        ws: &Workspace,
        cx: &App,
    ) -> Vec<OwnedSession> {
        if self.owned_sessions.rows.is_empty() {
            return Vec::new();
        }
        let attached = self.attached_session_ids(cx);
        visible_session_rows(
            &self.owned_sessions.rows,
            &ws.durable_id,
            &attached,
            &self.owned_sessions.forgetting,
        )
    }

    pub(crate) fn open_session_in_layout(
        &mut self,
        ws_idx: usize,
        listed: OwnedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ws_idx >= self.workspaces.len() {
            return;
        }
        self.active_idx = ws_idx;
        let ws_id = self.workspaces[ws_idx].id;
        let cwd = std::path::PathBuf::from(&listed.cwd);
        let terminal = cx
            .new(|cx| TerminalView::attach_existing(ws_id, Some(cwd), listed.session.clone(), cx));
        if let Some(title) = listed.title.clone() {
            terminal.update(cx, |view, _| view.terminal.title = title);
        }
        let surface_id = terminal.entity_id().as_u64();
        let new_pane = self.create_pane(terminal, ws_id, cx);
        self.seed_surface_from_host(&listed.session, ws_id, surface_id, cx);
        let Some(ws) = self.workspaces.get_mut(ws_idx) else {
            return;
        };
        if let Some(root) = &mut ws.active_tab_mut().root {
            if !root.split_at_focused(SplitDirection::Horizontal, new_pane.clone(), window, cx) {
                root.split_first_leaf(SplitDirection::Horizontal, new_pane.clone());
            }
        } else {
            ws.active_tab_mut().root = Some(LayoutTree::Leaf(new_pane.clone()));
        }
        self.owned_sessions
            .rows
            .retain(|row| row.session != listed.session);
        new_pane.read(cx).focus_handle(cx).focus(window, cx);
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn resume_listed_session(
        &mut self,
        ws_idx: usize,
        listed: OwnedSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let holder = self.attached_terminals(cx).into_iter().find(|terminal| {
            let state = &terminal.read(cx).terminal;
            state.session_id == listed.session && matches!(state.host_link, HostLinkState::Ended(_))
        });
        if let Some(terminal) = holder {
            terminal.update(cx, |view, cx| view.resume_hosted_session(cx));
            if let Some(pane) = self.pane_holding_terminal(&terminal, cx) {
                pane.read(cx).focus_handle(cx).focus(window, cx);
            }
            self.owned_sessions
                .rows
                .retain(|row| row.session != listed.session);
            cx.notify();
            return;
        }
        self.open_session_in_layout(ws_idx, listed, window, cx);
    }

    fn pane_holding_terminal(
        &self,
        terminal: &Entity<TerminalView>,
        cx: &App,
    ) -> Option<Entity<Pane>> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .find(|pane| pane.read(cx).contains_terminal(terminal))
    }

    pub(crate) fn resume_ended_panes(
        &mut self,
        ws_idx: usize,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<TerminalView>> {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return Vec::new();
        };
        let resumable: Vec<_> = ws
            .collect_panes()
            .iter()
            .flat_map(|pane| pane_terminals(pane, cx))
            .filter(|terminal| terminal_is_resumable(terminal, cx))
            .collect();
        for terminal in &resumable {
            terminal.update(cx, |view, cx| view.resume_hosted_session(cx));
        }
        if !resumable.is_empty() {
            cx.notify();
        }
        resumable
    }

    pub(crate) fn resumable_ended_panes(&self, ws_idx: usize, cx: &App) -> usize {
        self.workspaces
            .get(ws_idx)
            .map(|ws| {
                ws.collect_panes()
                    .iter()
                    .flat_map(|pane| pane_terminals(pane, cx))
                    .filter(|terminal| terminal_is_resumable(terminal, cx))
                    .count()
            })
            .unwrap_or(0)
    }

    pub(crate) fn track_resume_batch(
        &mut self,
        terminals: Vec<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        if terminals.is_empty() {
            return;
        }
        self.resume_batch = Some(ResumeBatch {
            pending: terminals
                .iter()
                .map(|terminal| terminal.entity_id().as_u64())
                .collect(),
            requested: terminals.len(),
            resumed: 0,
        });
        cx.notify();
    }

    pub(crate) fn note_host_link_resolved(
        &mut self,
        terminal: &Entity<TerminalView>,
        cx: &mut Context<Self>,
    ) {
        let surface = terminal.entity_id().as_u64();
        let resumed = terminal.read(cx).terminal.host_link.accepts_input();
        let Some(batch) = self.resume_batch.as_mut() else {
            return;
        };
        if let Some(position) = batch.pending.iter().position(|id| *id == surface) {
            batch.pending.remove(position);
            if resumed {
                batch.resumed += 1;
            }
        }
        if batch.pending.is_empty() {
            let (resumed, requested) = (batch.resumed, batch.requested);
            self.resume_batch = None;
            if let Some(notice) = resume_batch_failure_notice(resumed, requested) {
                self.show_toast(notice, cx);
            }
        }
    }
}

pub(crate) fn resume_batch_failure_notice(resumed: usize, requested: usize) -> Option<String> {
    let failed = requested.saturating_sub(resumed);
    (failed > 0).then(|| format!("{failed} of {requested} panes could not be resumed"))
}

pub(crate) fn host_link_is_resumable(link: &HostLinkState) -> bool {
    matches!(link, HostLinkState::Ended(end) if end.restartable())
}

fn terminal_is_resumable(terminal: &Entity<TerminalView>, cx: &App) -> bool {
    host_link_is_resumable(&terminal.read(cx).terminal.host_link)
}

pub(crate) struct ResumeBatch {
    pending: VecDeque<u64>,
    requested: usize,
    resumed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(live: bool, updated_at_ms: u64) -> OwnedSession {
        OwnedSession {
            session: SessionId::new(),
            generation: SessionGeneration::FIRST,
            workspace: None,
            title: None,
            cwd: "/repo/web".to_string(),
            live,
            lifecycle: if live {
                SessionLifecycle::Running
            } else {
                SessionLifecycle::Exited {
                    code: 0,
                    signal: None,
                }
            },
            updated_at_ms,
        }
    }

    #[test]
    fn a_session_whose_stop_is_in_flight_is_held_back_from_the_sidebar() {
        let workspace = WorkspaceId::new();
        let mut closing = row(true, 30);
        closing.workspace = Some(workspace.clone());
        let mut attached = row(true, 20);
        attached.workspace = Some(workspace.clone());
        let mut kept = row(false, 10);
        kept.workspace = Some(workspace.clone());
        let mut elsewhere = row(true, 40);
        elsewhere.workspace = Some(WorkspaceId::new());
        let rows = vec![
            closing.clone(),
            attached.clone(),
            kept.clone(),
            elsewhere.clone(),
        ];

        let visible = visible_session_rows(
            &rows,
            &workspace,
            &HashSet::from([attached.session.clone()]),
            &HashSet::from([closing.session.clone()]),
        );

        let listed: Vec<_> = visible.into_iter().map(|session| session.session).collect();
        assert_eq!(
            listed,
            vec![kept.session.clone()],
            "a session whose stop is in flight never flashes a row before it goes"
        );
    }

    #[test]
    fn a_stopped_session_leaves_no_record_behind() {
        use paneflow_host::server::ServerHandle;
        use paneflow_host::{ClientHello, CreateSession, HostClient, SessionHost};

        let home = tempfile::tempdir().expect("host home");
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default()
        );
        #[cfg(windows)]
        let endpoint = std::path::PathBuf::from(format!(r"\\.\pipe\paneflow-forget-test-{unique}"));
        #[cfg(unix)]
        let endpoint = home.path().join(format!("forget-test-{unique}.sock"));
        let host = SessionHost::open(home.path(), &endpoint).expect("session host");
        let server = ServerHandle::spawn(std::sync::Arc::clone(&host), endpoint.clone())
            .expect("host server");
        let hello = ClientHello::local("paneflow-desktop-test");
        let mut client = HostClient::connect(&endpoint, &hello).expect("control connection");

        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<String>::new());
        let created = client
            .create(&CreateSession {
                session: None,
                workspace: None,
                cwd: Some(std::env::temp_dir().display().to_string()),
                shell: Some(shell.to_string()),
                args,
                env: Default::default(),
                cols: Some(80),
                rows: Some(24),
                title: None,
            })
            .expect("hosted session");
        let session = created.manifest.session.clone();
        let manifest = paneflow_host::manifest::manifest_path(home.path(), &session);
        assert!(manifest.is_file(), "a live session owns a manifest");

        let failures = stop_and_forget(vec![(
            endpoint.clone(),
            session.clone(),
            created.manifest.generation,
        )]);

        assert!(failures.is_empty(), "the stop reported no failure");
        assert!(
            !manifest.exists(),
            "an explicit stop deletes the record instead of leaving an ended row"
        );
        let listed = client
            .call("session.list", serde_json::json!({}))
            .expect("session list");
        assert!(
            listed["sessions"]
                .as_array()
                .expect("a sessions array")
                .is_empty(),
            "the sidebar source has nothing left to list"
        );
        server.stop().expect("the host server stops");
    }

    #[test]
    fn live_sessions_come_first_then_the_most_recent_ended_ones() {
        let mut rows = vec![row(false, 10), row(true, 5), row(false, 30), row(true, 40)];
        let expected: Vec<_> = vec![
            rows[3].session.clone(),
            rows[1].session.clone(),
            rows[2].session.clone(),
            rows[0].session.clone(),
        ];
        order_session_rows(&mut rows);
        let ordered: Vec<_> = rows.iter().map(|row| row.session.clone()).collect();
        assert_eq!(ordered, expected);
    }

    #[test]
    fn a_row_falls_back_to_its_directory_when_it_has_no_title() {
        let mut listed = row(false, 0);
        assert_eq!(listed.label(), "web");
        listed.title = Some("   ".to_string());
        assert_eq!(listed.label(), "web");
        listed.title = Some("claude".to_string());
        assert_eq!(listed.label(), "claude");
    }

    #[test]
    fn a_fully_resumed_batch_stays_silent_and_a_partial_one_counts_the_failures() {
        assert_eq!(resume_batch_failure_notice(4, 4), None);
        assert_eq!(resume_batch_failure_notice(0, 0), None);
        assert_eq!(
            resume_batch_failure_notice(3, 5).as_deref(),
            Some("2 of 5 panes could not be resumed")
        );
        assert_eq!(
            resume_batch_failure_notice(0, 1).as_deref(),
            Some("1 of 1 panes could not be resumed")
        );
    }

    #[test]
    fn ages_read_in_the_largest_unit_that_fits() {
        assert_eq!(relative_age(1_000, 1_000), "just now");
        assert_eq!(relative_age(120_000, 0), "2 min ago");
        assert_eq!(relative_age(7_200_000, 0), "2 h ago");
        assert_eq!(relative_age(172_800_000, 0), "2 d ago");
        assert_eq!(relative_age(0, 5_000), "just now");
    }

    #[test]
    fn a_pane_restored_into_an_ended_session_resumes_without_an_attachment() {
        let mut state = crate::terminal::TerminalState::new_display_only(24, 80);
        assert!(
            state.hosted.is_none(),
            "a reattach that lands on an ended session never promotes an attachment"
        );
        state.host_link = HostLinkState::Ended(host_link::HostLinkEnd::exited(0, None));
        assert!(host_link_is_resumable(&state.host_link));

        state.host_link = HostLinkState::Ended(host_link::HostLinkEnd::incompatible(
            "source_sha".to_string(),
        ));
        assert!(
            !host_link_is_resumable(&state.host_link),
            "an incompatible host is left alone"
        );

        for live in [
            HostLinkState::Attaching,
            HostLinkState::Attached,
            HostLinkState::Reconnecting,
            HostLinkState::Unavailable("no host".to_string()),
        ] {
            assert!(!host_link_is_resumable(&live));
        }
    }

    #[test]
    fn a_lifecycle_sentence_names_the_exit_or_the_failure() {
        assert_eq!(
            lifecycle_sentence(&SessionLifecycle::Exited {
                code: 1,
                signal: None
            }),
            "Exited with code 1"
        );
        assert_eq!(
            lifecycle_sentence(&SessionLifecycle::Exited {
                code: 0,
                signal: Some("SIGTERM".to_string())
            }),
            "Exited with code 0 (SIGTERM)"
        );
        assert_eq!(
            lifecycle_sentence(&SessionLifecycle::Failed {
                reason: "shell not found".to_string()
            }),
            "Failed to start: shell not found"
        );
        assert_eq!(lifecycle_sentence(&SessionLifecycle::Running), "Running");
    }
}
