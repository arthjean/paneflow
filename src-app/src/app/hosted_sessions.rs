use std::collections::HashSet;

use gpui::{App, AppContext, Context, Entity, Focusable, Window};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};

use crate::layout::{LayoutTree, SplitDirection};
use crate::pane::{Pane, PaneSurface};
use crate::terminal::TerminalView;
use crate::terminal::host_link;
use crate::workspace::{Tab, Workspace};
use crate::{HidePane, PaneFlowApp};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HiddenSession {
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
    pub(crate) workspace: Option<WorkspaceId>,
    pub(crate) title: Option<String>,
    pub(crate) cwd: String,
}

#[derive(Default)]
pub(crate) struct HiddenSessions {
    live: Vec<HiddenSession>,
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

impl PaneFlowApp {
    pub(crate) fn stop_sessions_in_panes(&self, panes: &[Entity<Pane>], cx: &mut Context<Self>) {
        let terminals: Vec<_> = panes
            .iter()
            .flat_map(|pane| pane_terminals(pane, cx))
            .collect();
        self.stop_terminals(terminals, cx);
    }

    pub(crate) fn stop_sessions_in_tab(&self, tab: &Tab, cx: &mut Context<Self>) {
        let terminals = tab_terminals(tab, cx);
        self.stop_terminals(terminals, cx);
    }

    pub(crate) fn stop_terminals(
        &self,
        terminals: Vec<Entity<TerminalView>>,
        cx: &mut Context<Self>,
    ) {
        let targets: Vec<_> = terminals
            .iter()
            .filter_map(|terminal| terminal.read(cx).hosted_stop_target())
            .collect();
        if targets.is_empty() {
            return;
        }
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let failures = executor
                .spawn(async move {
                    let mut failures = Vec::new();
                    for (endpoint, session, generation) in targets {
                        match host_link::stop_session(&endpoint, &session, generation) {
                            Ok(summary) => log::info!(
                                "paneflow: hosted session {session} stopped ({})",
                                summary.manifest.lifecycle.label()
                            ),
                            Err(error) => {
                                log::warn!(
                                    "paneflow: hosted session {session} stop outcome unknown: {error}"
                                );
                                failures.push(session);
                            }
                        }
                    }
                    failures
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                if !failures.is_empty() {
                    app.show_toast(
                        format!(
                            "{} session(s) may still be running: the stop request did not complete. Check the hidden sessions list.",
                            failures.len()
                        ),
                        cx,
                    );
                }
                app.refresh_hidden_sessions(cx);
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

    pub(crate) fn hide_pane_from_layout(&mut self, pane: Entity<Pane>, cx: &mut Context<Self>) {
        let terminals = pane_terminals(&pane, cx);
        let mut hidden = 0usize;
        for terminal in &terminals {
            terminal.update(cx, |view, _| {
                if view.terminal.hosted.is_some() {
                    hidden += 1;
                }
                view.terminal.leave_running_on_close = true;
            });
        }
        pane.update(cx, |pane, cx| pane.close(cx));
        if hidden > 0 {
            self.show_toast(
                format!(
                    "{hidden} session(s) hidden from the layout and still running; reopen them from the sidebar."
                ),
                cx,
            );
        }
        self.save_session(cx);
        self.refresh_hidden_sessions(cx);
        cx.notify();
    }

    pub(crate) fn refresh_hidden_sessions(&mut self, cx: &mut Context<Self>) {
        self.hidden_sessions.refresh_generation =
            self.hidden_sessions.refresh_generation.wrapping_add(1);
        let generation = self.hidden_sessions.refresh_generation;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut gpui::AsyncApp| {
            let listed = executor
                .spawn(async move { host_link::list_sessions(None) })
                .await;
            let live = match listed {
                Ok(sessions) => sessions
                    .into_iter()
                    .filter(|summary| summary.live && summary.owned)
                    .map(|summary| HiddenSession {
                        session: summary.manifest.session,
                        generation: summary.manifest.generation,
                        workspace: summary.manifest.workspace,
                        title: summary.manifest.title,
                        cwd: summary.manifest.current_cwd.unwrap_or(summary.manifest.cwd),
                    })
                    .collect(),
                Err(error) => {
                    log::debug!("paneflow: hidden session refresh skipped: {error}");
                    Vec::new()
                }
            };
            let _ = this.update(cx, |app, cx| {
                if app.hidden_sessions.refresh_generation != generation {
                    return;
                }
                app.hidden_sessions.live = live;
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn attached_session_ids(&self, cx: &App) -> HashSet<SessionId> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.collect_panes())
            .flat_map(|pane| pane_terminals(&pane, cx))
            .chain(self.diff_dock.diff_tabs.iter().filter_map(|tab| match tab {
                crate::app::diff_dock::DiffDockTab::Terminal(terminal) => Some(terminal.clone()),
                _ => None,
            }))
            .map(|terminal| terminal.read(cx).terminal.session_id.clone())
            .collect()
    }

    pub(crate) fn hidden_sessions_for_workspace(
        &self,
        ws: &Workspace,
        cx: &App,
    ) -> Vec<HiddenSession> {
        if self.hidden_sessions.live.is_empty() {
            return Vec::new();
        }
        let attached = self.attached_session_ids(cx);
        self.hidden_sessions
            .live
            .iter()
            .filter(|session| session.workspace.as_ref() == Some(&ws.durable_id))
            .filter(|session| !attached.contains(&session.session))
            .cloned()
            .collect()
    }

    pub(crate) fn reopen_hidden_session(
        &mut self,
        ws_idx: usize,
        hidden: HiddenSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ws_idx >= self.workspaces.len() {
            return;
        }
        self.active_idx = ws_idx;
        let ws_id = self.workspaces[ws_idx].id;
        let cwd = std::path::PathBuf::from(&hidden.cwd);
        let terminal = cx
            .new(|cx| TerminalView::attach_existing(ws_id, Some(cwd), hidden.session.clone(), cx));
        if let Some(title) = hidden.title.clone() {
            terminal.update(cx, |view, _| view.terminal.title = title);
        }
        let new_pane = self.create_pane(terminal, ws_id, cx);
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
        self.hidden_sessions
            .live
            .retain(|session| session.session != hidden.session);
        new_pane.read(cx).focus_handle(cx).focus(window, cx);
        self.save_session(cx);
        cx.notify();
    }
}
