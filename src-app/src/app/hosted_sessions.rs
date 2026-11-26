use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use gpui::{App, Context, Entity, Window};
use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_host::SessionLifecycle;
use paneflow_host::protocol::ERR_PROCESS_UNVERIFIED;

use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;
use crate::terminal::host_link::{self, HostLinkState};
use crate::workspace::Tab;
use crate::{PaneFlowApp, StopSession};

pub(crate) type StopTarget = (PathBuf, SessionId, SessionGeneration);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwnedSession {
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
}

pub(crate) struct OwnedSessions {
    rows: Vec<OwnedSession>,
    forgetting: HashSet<SessionId>,
    refresh_generation: u64,
    live_sweep_pending: bool,
}

impl Default for OwnedSessions {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            forgetting: HashSet::new(),
            refresh_generation: 0,
            live_sweep_pending: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedRecord {
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
    pub(crate) owned: bool,
    pub(crate) live: bool,
    pub(crate) lifecycle: SessionLifecycle,
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

pub(crate) fn stop_and_forget(targets: Vec<StopTarget>) -> Vec<SessionId> {
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
            Err(error) if error.code() == Some(ERR_PROCESS_UNVERIFIED) => {
                match host_link::remove_session(&endpoint, &session) {
                    Ok(()) => log::info!(
                        "paneflow: hosted session {session} had no process to stop; record removed"
                    ),
                    Err(remove_error) => {
                        log::warn!(
                            "paneflow: hosted session {session} stop outcome unknown: {error}; removal refused: {remove_error}"
                        );
                        failures.push(session);
                    }
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

    pub(crate) fn expire_closed_sessions(&mut self, cx: &mut Context<Self>) {
        self.expire_closed_sessions_at(Instant::now(), cx);
    }

    pub(crate) fn expire_closed_sessions_at(&mut self, now: Instant, cx: &mut Context<Self>) {
        self.expire_layout_holds(now, cx);
    }

    pub(crate) fn release_undo_window_sessions(&mut self) -> Vec<StopTarget> {
        self.release_layout_holds()
    }

    fn held_session_ids(&self, cx: &App) -> HashSet<SessionId> {
        let mut held = self.attached_session_ids(cx);
        held.extend(self.layout_held_sessions());
        held
    }

    pub(crate) fn stop_hosted_sessions(
        &mut self,
        targets: Vec<StopTarget>,
        cx: &mut Context<Self>,
    ) {
        let targets: Vec<StopTarget> = targets
            .into_iter()
            .filter(|(_, session, _)| !self.owned_sessions.forgetting.contains(session))
            .collect();
        if targets.is_empty() {
            return;
        }
        #[cfg(test)]
        if crate::app::fake_host::installed(cx) {
            for (_, session, _) in targets {
                crate::app::fake_host::record(
                    cx,
                    crate::app::fake_host::HostRequest::Stop { session },
                );
            }
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
                            "{} session(s) may still be running: the stop request did not complete.",
                            failures.len()
                        ),
                        cx,
                    );
                }
                app.refresh_owned_sessions(cx);
            });
        })
        .detach();
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
        self.stop_listed_session(&session, cx);
    }

    pub(crate) fn sweep_live_orphans_on_next_refresh(&mut self, cx: &mut Context<Self>) {
        self.owned_sessions.live_sweep_pending = true;
        self.refresh_owned_sessions(cx);
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
            let records: Vec<ListedRecord> = match listed {
                Ok(sessions) => sessions
                    .into_iter()
                    .map(|row| ListedRecord {
                        session: row.session,
                        generation: row.generation,
                        owned: row.owned,
                        live: row.live,
                        lifecycle: row.lifecycle,
                    })
                    .collect(),
                Err(error) => {
                    log::debug!("paneflow: session refresh kept the previous list: {error}");
                    return;
                }
            };
            let _ = this.update(cx, |app, cx| {
                if app.owned_sessions.refresh_generation != generation {
                    return;
                }
                app.apply_session_listing(records, cx);
            });
        })
        .detach();
    }

    fn apply_session_listing(&mut self, records: Vec<ListedRecord>, cx: &mut Context<Self>) {
        self.owned_sessions.rows = records
            .iter()
            .filter(|record| record.owned && record.live)
            .map(|record| OwnedSession {
                session: record.session.clone(),
                generation: record.generation,
            })
            .collect();
        if self.session_restore_failed {
            return;
        }
        let Some(sweeper) = self.layout.sync.sweeper() else {
            return;
        };
        let live = std::mem::take(&mut self.owned_sessions.live_sweep_pending);
        let mut keep: Vec<String> = self
            .held_session_ids(cx)
            .iter()
            .chain(self.owned_sessions.forgetting.iter())
            .map(ToString::to_string)
            .collect();
        keep.sort();
        let params = serde_json::json!({"live": live, "keep": keep});
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let swept = executor.spawn(async move { sweeper.sweep(params) }).await;
            let _ = this.update(cx, |app, cx| {
                let Some(swept) = swept else {
                    if live {
                        app.owned_sessions.live_sweep_pending = true;
                    }
                    return;
                };
                let changed = ["forgotten", "stopped"]
                    .iter()
                    .any(|key| swept[key].as_array().is_some_and(|list| !list.is_empty()));
                if changed {
                    app.refresh_owned_sessions(cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn owned_sessions_sweep_waits(&self) -> bool {
        self.owned_sessions.live_sweep_pending
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
        self.owned_sessions.rows.iter()
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
                .map(|terminal| terminal.read(cx).surface_id())
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
        let surface = terminal.read(cx).surface_id();
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

pub(crate) struct ResumeBatch {
    pending: VecDeque<u64>,
    requested: usize,
    resumed: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

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
                appearance: None,
                cell: None,
                sandbox: false,
                sandbox_unix_scope: false,
                resume: None,
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
            "an explicit stop deletes the record instead of leaving an ended one"
        );
        let listed = client
            .call("session.list", serde_json::json!({}))
            .expect("session list");
        assert!(
            listed["sessions"]
                .as_array()
                .expect("a sessions array")
                .is_empty(),
            "the host has nothing left to list"
        );
        server.stop().expect("the host server stops");
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
}
