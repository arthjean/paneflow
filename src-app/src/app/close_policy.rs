use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, SharedString, Window, div, prelude::*, px,
};
use paneflow_config::schema::SessionId;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::hosted_sessions::{pane_terminals, tab_terminals};
use crate::pane::Pane;
use crate::settings::components::{
    ModalKey, confirmation_list, confirmation_warning, destructive_button, modal_backdrop,
    modal_card, modal_footer, modal_header, modal_key, secondary_button,
};
use crate::terminal::TerminalView;
use crate::terminal::host_link::HostLinkState;

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
const MAX_LISTED_SESSIONS: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloseIntent {
    Stop,
    Detach,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCloseDecision {
    Stop,
    Ask,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentReading {
    Known(Option<AgentState>),
    Unknown,
}

pub(crate) fn session_close_decision(
    link: &HostLinkState,
    agent: AgentReading,
) -> SessionCloseDecision {
    if matches!(link, HostLinkState::Unavailable(_)) {
        return SessionCloseDecision::Unknown;
    }
    match agent {
        AgentReading::Unknown => SessionCloseDecision::Ask,
        AgentReading::Known(Some(AgentState::Thinking | AgentState::WaitingForInput)) => {
            SessionCloseDecision::Ask
        }
        AgentReading::Known(_) => SessionCloseDecision::Stop,
    }
}

pub(crate) fn agent_state_word(state: AgentState) -> &'static str {
    match state {
        AgentState::Thinking => "working",
        AgentState::WaitingForInput => "waiting for your input",
        AgentState::Finished => "finished",
        AgentState::Errored => "errored",
    }
}

#[derive(Clone)]
pub(crate) enum CloseTarget {
    Pane(Entity<Pane>),
    FocusedPane(Entity<Pane>),
    Surface {
        pane: Entity<Pane>,
        terminal: Entity<TerminalView>,
    },
    Tab {
        ws_idx: usize,
        tab_idx: usize,
    },
    Workspace(usize),
    DiffTerminal(usize),
    Session(SessionId),
}

impl CloseTarget {
    fn question(&self) -> &'static str {
        match self {
            Self::Pane(_) | Self::FocusedPane(_) => "Close this pane?",
            Self::Surface { .. } | Self::DiffTerminal(_) => "Close this terminal?",
            Self::Tab { .. } => "Close this tab?",
            Self::Workspace(_) => "Close this workspace?",
            Self::Session(_) => "Stop this session?",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloseDialogRow {
    pub(crate) title: String,
    pub(crate) state: Option<AgentState>,
}

impl CloseDialogRow {
    fn state_word(&self) -> &'static str {
        self.state
            .map(agent_state_word)
            .unwrap_or("state unknown, the agent stream is down")
    }
}

pub(crate) struct CloseDialog {
    target: CloseTarget,
    rows: Vec<CloseDialogRow>,
    focused: bool,
}

impl PaneFlowApp {
    pub(crate) fn close_target_terminals(
        &self,
        target: &CloseTarget,
        cx: &gpui::App,
    ) -> Vec<Entity<TerminalView>> {
        match target {
            CloseTarget::Pane(pane) | CloseTarget::FocusedPane(pane) => pane_terminals(pane, cx),
            CloseTarget::Surface { terminal, .. } => vec![terminal.clone()],
            CloseTarget::Tab { ws_idx, tab_idx } => self
                .workspaces
                .get(*ws_idx)
                .and_then(|ws| ws.tabs().get(*tab_idx))
                .map(|tab| tab_terminals(tab, cx))
                .unwrap_or_default(),
            CloseTarget::Workspace(idx) => self
                .workspaces
                .get(*idx)
                .map(|ws| {
                    ws.collect_panes()
                        .iter()
                        .flat_map(|pane| pane_terminals(pane, cx))
                        .collect()
                })
                .unwrap_or_default(),
            CloseTarget::DiffTerminal(index) => match self.diff_dock.diff_tabs.get(*index) {
                Some(crate::app::diff_dock::DiffDockTab::Terminal(terminal)) => {
                    vec![terminal.clone()]
                }
                _ => Vec::new(),
            },
            CloseTarget::Session(_) => Vec::new(),
        }
    }

    fn close_dialog_rows(&self, target: &CloseTarget, cx: &gpui::App) -> Vec<CloseDialogRow> {
        if let CloseTarget::Session(session) = target {
            return busy_rows(vec![(
                self.listed_session_label(session),
                HostLinkState::Attached,
                self.agent_reading(session, None),
            )]);
        }
        let contained: Vec<_> = self
            .close_target_terminals(target, cx)
            .into_iter()
            .filter_map(|terminal| {
                let view = terminal.read(cx);
                view.terminal.hosted.as_ref()?;
                Some((
                    view.terminal.title.clone(),
                    view.terminal.host_link.clone(),
                    self.agent_reading(
                        &view.terminal.session_id,
                        Some(terminal.entity_id().as_u64()),
                    ),
                ))
            })
            .collect();
        busy_rows(contained)
    }

    fn agent_reading(&self, session: &SessionId, surface: Option<u64>) -> AgentReading {
        let local = surface.and_then(|surface| self.local_agent_state(surface));
        if matches!(
            local,
            Some(AgentState::Thinking | AgentState::WaitingForInput)
        ) {
            return AgentReading::Known(local);
        }
        if !self.host_agents_are_settled() {
            return AgentReading::Unknown;
        }
        let Some(row) = self.host_agent_row(session) else {
            return AgentReading::Unknown;
        };
        if row.stale {
            return AgentReading::Unknown;
        }
        AgentReading::Known(row.state.or(local))
    }

    fn local_agent_state(&self, surface: u64) -> Option<AgentState> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.agent_sessions.values())
            .find(|agent| agent.surface_id == Some(surface))
            .map(|agent| agent.state)
    }

    pub(crate) fn request_close(
        &mut self,
        target: CloseTarget,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.close_dialog.is_some() {
            return;
        }
        let rows = self.close_dialog_rows(&target, cx);
        if rows.is_empty() {
            self.perform_close(target, CloseIntent::Stop, window, cx);
            return;
        }
        self.dismiss_transient_surfaces();
        self.close_dialog = Some(CloseDialog {
            target,
            rows,
            focused: false,
        });
        cx.notify();
    }

    pub(crate) fn perform_close(
        &mut self,
        target: CloseTarget,
        intent: CloseIntent,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if intent == CloseIntent::Stop {
            match &target {
                CloseTarget::Session(session) => self.stop_listed_session(session, cx),
                other => {
                    let terminals = self.close_target_terminals(other, cx);
                    self.stop_terminals(terminals, cx);
                }
            }
        }
        match target {
            CloseTarget::Session(_) => {}
            CloseTarget::Pane(pane) => self.remove_pane_from_layout(pane, cx),
            CloseTarget::FocusedPane(pane) => {
                if let Some(window) = window {
                    self.remove_focused_pane(pane, window, cx);
                }
            }
            CloseTarget::Surface { pane, terminal } => {
                pane.update(cx, |pane, cx| pane.remove_surface(&terminal, cx));
                self.save_session(cx);
                cx.notify();
            }
            CloseTarget::Tab { ws_idx, tab_idx } => {
                if let Some(window) = window {
                    self.remove_workspace_tab(ws_idx, tab_idx, window, cx);
                }
            }
            CloseTarget::Workspace(idx) => {
                if let Some(window) = window {
                    self.remove_workspace(idx, window, cx);
                }
            }
            CloseTarget::DiffTerminal(index) => {
                self.remove_diff_tab(index, cx);
                if let Some(window) = window {
                    self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
                }
            }
        }
        if intent == CloseIntent::Detach {
            self.refresh_owned_sessions(cx);
        }
    }

    pub(crate) fn close_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.close_dialog.take().is_some() {
            if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
                ws.focus_first(window, cx);
            }
            cx.notify();
        }
    }

    fn resolve_close_dialog(
        &mut self,
        intent: CloseIntent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.close_dialog.take() else {
            return;
        };
        self.perform_close(dialog.target, intent, Some(window), cx);
        cx.notify();
    }

    fn handle_close_dialog_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.close_dialog.is_none() {
            return;
        }
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.close_close_dialog(window, cx),
            Some(ModalKey::Confirm) => self.resolve_close_dialog(CloseIntent::Detach, window, cx),
            None => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_close_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.close_dialog.as_mut() else {
            return div().into_any_element();
        };
        if !dialog.focused {
            dialog.focused = true;
            self.close_dialog_focus.focus(window, cx);
        }
        let Some(dialog) = self.close_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let unknown = dialog.rows.iter().filter(|row| row.state.is_none()).count();

        let header = modal_header(
            ui,
            dialog.target.question(),
            close_summary(dialog.rows.len() - unknown, unknown),
        );

        let list = confirmation_list(
            ui,
            dialog.rows.iter().map(|row| {
                (
                    SharedString::from(row.title.clone()),
                    SharedString::from(row.state_word()),
                )
            }),
            MAX_LISTED_SESSIONS,
        );

        let explanation = confirmation_warning(
            ui,
            "Keep them running and they stay in the session list, ready to reopen. \
             Stop ends every session this action contains and the processes it started.",
        );

        let footer = modal_footer()
            .child(secondary_button(
                "close-dialog-cancel",
                "Cancel",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.close_close_dialog(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(
                destructive_button("close-dialog-stop", "Stop").on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.resolve_close_dialog(CloseIntent::Stop, window, cx);
                        cx.stop_propagation();
                    },
                )),
            )
            .child(secondary_button(
                "close-dialog-keep",
                "Keep running",
                ui,
                cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.resolve_close_dialog(CloseIntent::Detach, window, cx);
                    cx.stop_propagation();
                }),
            ));

        let card = modal_card(
            "close-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div()
                .child(header)
                .child(list)
                .child(explanation)
                .child(footer),
        )
        .track_focus(&self.close_dialog_focus)
        .on_key_down(cx.listener(Self::handle_close_dialog_key_down));

        modal_backdrop(
            "close-dialog-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_close_dialog(window, cx);
            }),
        )
    }
}

pub(crate) fn busy_rows(
    contained: Vec<(String, HostLinkState, AgentReading)>,
) -> Vec<CloseDialogRow> {
    contained
        .into_iter()
        .filter_map(
            |(title, link, agent)| match (session_close_decision(&link, agent), agent) {
                (SessionCloseDecision::Ask, AgentReading::Known(state)) => {
                    Some(CloseDialogRow { title, state })
                }
                (SessionCloseDecision::Ask, AgentReading::Unknown) => {
                    Some(CloseDialogRow { title, state: None })
                }
                _ => None,
            },
        )
        .collect()
}

pub(crate) fn close_summary(busy: usize, unknown: usize) -> String {
    let mut parts = Vec::new();
    match busy {
        0 => {}
        1 => parts.push("1 session is still busy.".to_string()),
        n => parts.push(format!("{n} sessions are still busy.")),
    }
    match unknown {
        0 => {}
        1 => parts.push("1 session cannot be checked: the agent stream is down.".to_string()),
        n => parts.push(format!(
            "{n} sessions cannot be checked: the agent stream is down."
        )),
    }
    parts.push(if busy + unknown == 1 {
        "Closing this view does not have to end it.".to_string()
    } else {
        "Closing this view does not have to end them.".to_string()
    });
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(state: Option<AgentState>) -> AgentReading {
        AgentReading::Known(state)
    }

    #[test]
    fn an_idle_shell_stops_and_a_busy_agent_asks() {
        assert_eq!(
            session_close_decision(&HostLinkState::Attached, known(None)),
            SessionCloseDecision::Stop
        );
        for finished in [AgentState::Finished, AgentState::Errored] {
            assert_eq!(
                session_close_decision(&HostLinkState::Attached, known(Some(finished))),
                SessionCloseDecision::Stop
            );
        }
        for busy in [AgentState::Thinking, AgentState::WaitingForInput] {
            assert_eq!(
                session_close_decision(&HostLinkState::Attached, known(Some(busy))),
                SessionCloseDecision::Ask
            );
        }
    }

    #[test]
    fn an_unknown_agent_state_asks_instead_of_passing_for_an_idle_shell() {
        assert_eq!(
            session_close_decision(&HostLinkState::Attached, AgentReading::Unknown),
            SessionCloseDecision::Ask
        );
        let rows = busy_rows(vec![(
            "claude".to_string(),
            HostLinkState::Attached,
            AgentReading::Unknown,
        )]);
        assert_eq!(
            rows,
            vec![CloseDialogRow {
                title: "claude".to_string(),
                state: None
            }]
        );
        assert!(rows[0].state_word().starts_with("state unknown"));
    }

    #[test]
    fn an_unreachable_host_never_asks_and_never_stops_blindly() {
        for agent in [
            known(None),
            known(Some(AgentState::Thinking)),
            known(Some(AgentState::WaitingForInput)),
            AgentReading::Unknown,
        ] {
            assert_eq!(
                session_close_decision(&HostLinkState::Unavailable("no host".into()), agent),
                SessionCloseDecision::Unknown
            );
        }
    }

    #[test]
    fn one_action_asks_once_for_every_busy_session_it_contains() {
        let contained = vec![
            ("shell".to_string(), HostLinkState::Attached, known(None)),
            (
                "done".to_string(),
                HostLinkState::Attached,
                known(Some(AgentState::Finished)),
            ),
            (
                "claude".to_string(),
                HostLinkState::Attached,
                known(Some(AgentState::Thinking)),
            ),
            (
                "codex".to_string(),
                HostLinkState::Attached,
                known(Some(AgentState::WaitingForInput)),
            ),
            (
                "orphan".to_string(),
                HostLinkState::Unavailable("no host".to_string()),
                known(Some(AgentState::Thinking)),
            ),
        ];
        let rows = busy_rows(contained);
        assert_eq!(
            rows,
            vec![
                CloseDialogRow {
                    title: "claude".to_string(),
                    state: Some(AgentState::Thinking)
                },
                CloseDialogRow {
                    title: "codex".to_string(),
                    state: Some(AgentState::WaitingForInput)
                },
            ],
            "an idle shell, a finished agent and a session whose host is unreachable never ask"
        );
    }

    #[test]
    fn a_target_with_nothing_busy_closes_without_a_dialog() {
        assert!(busy_rows(Vec::new()).is_empty());
        assert!(
            busy_rows(vec![(
                "shell".to_string(),
                HostLinkState::Attached,
                known(Some(AgentState::Errored))
            )])
            .is_empty()
        );
    }

    #[test]
    fn the_summary_counts_the_busy_and_the_unchecked_sessions() {
        assert_eq!(
            close_summary(1, 0),
            "1 session is still busy. Closing this view does not have to end it."
        );
        assert!(close_summary(3, 0).starts_with("3 sessions are still busy. "));
        assert_eq!(
            close_summary(0, 1),
            "1 session cannot be checked: the agent stream is down. \
             Closing this view does not have to end it."
        );
        assert!(close_summary(1, 2).contains("2 sessions cannot be checked"));
    }
}
