use gpui::{
    AnyElement, ClickEvent, Context, Entity, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Pixels, SharedString, Window, div, prelude::*, px,
};
use paneflow_config::schema::SessionId;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::diff_dock::code::view::CodeView;
use crate::app::hosted_sessions::{pane_terminals, tab_terminals};
use crate::app::unsaved_dialog::UnsavedContinuation;
use crate::app::workspace_ops::settle_closed_sessions;
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
    Hold,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseRefusal {
    Unsaved(Vec<String>),
    Busy(Vec<CloseDialogRow>),
}

impl CloseRefusal {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::Unsaved(names) => format!(
                "{}; save or discard them in Paneflow first",
                crate::app::unsaved_dialog::unsaved_close_error(names).unwrap_or_default()
            ),
            Self::Busy(rows) => {
                let unknown = rows.iter().filter(|row| row.state.is_none()).count();
                format!(
                    "{} Confirm the close in Paneflow.",
                    close_summary(rows.len() - unknown, unknown)
                )
            }
        }
    }
}

pub(crate) struct CloseDialog {
    target: CloseTarget,
    rows: Vec<CloseDialogRow>,
    discard: Vec<Entity<CodeView>>,
    tabs: Vec<u64>,
    focused: bool,
    return_focus: crate::FocusReturn,
}

pub(crate) const STALE_CLOSE_MESSAGE: &str =
    "Nothing was closed: the tabs changed while the dialog was open";

pub(crate) fn close_target_tabs(
    workspaces: &[crate::workspace::Workspace],
    target: &CloseTarget,
) -> Vec<u64> {
    match target {
        CloseTarget::Tab { ws_idx, tab_idx } => workspaces
            .get(*ws_idx)
            .and_then(|ws| ws.tabs().get(*tab_idx))
            .map(|tab| vec![tab.id])
            .unwrap_or_default(),
        CloseTarget::Workspace(idx) => workspaces
            .get(*idx)
            .map(|ws| ws.tabs().iter().map(|tab| tab.id).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn discard_unsaved(views: Vec<Entity<CodeView>>, cx: &mut gpui::App) {
    for view in views {
        view.update(cx, |view, _| view.discard_unsaved());
    }
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

    pub(crate) fn close_refusal(
        &self,
        target: &CloseTarget,
        cx: &gpui::App,
    ) -> Option<CloseRefusal> {
        let unsaved = self.unsaved_views_for_close(target, cx);
        if !unsaved.is_empty() {
            let names = crate::app::unsaved_dialog::file_rows(&unsaved, cx)
                .into_iter()
                .map(|(name, _)| name.to_string())
                .collect();
            return Some(CloseRefusal::Unsaved(names));
        }
        let rows = self.close_dialog_rows(target, cx);
        (!rows.is_empty()).then_some(CloseRefusal::Busy(rows))
    }

    pub(crate) fn close_without_prompt(
        &mut self,
        target: CloseTarget,
        cx: &mut Context<Self>,
    ) -> Result<(), CloseRefusal> {
        if let Some(refusal) = self.close_refusal(&target, cx) {
            return Err(refusal);
        }
        self.perform_close(target, CloseIntent::Hold, None, cx);
        Ok(())
    }

    pub(crate) fn request_close(
        &mut self,
        target: CloseTarget,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.close_dialog.is_some() || self.unsaved_dialog.is_some() {
            return;
        }
        let unsaved = self.unsaved_views_for_close(&target, cx);
        if self.ask_about_unsaved(unsaved, UnsavedContinuation::Close(target.clone()), cx) {
            return;
        }
        self.request_close_checked(target, Vec::new(), window, cx);
    }

    pub(crate) fn request_close_checked(
        &mut self,
        target: CloseTarget,
        discard: Vec<Entity<CodeView>>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.close_dialog.is_some() {
            return;
        }
        let rows = self.close_dialog_rows(&target, cx);
        if rows.is_empty() {
            discard_unsaved(discard, cx);
            self.perform_close(target, CloseIntent::Hold, window, cx);
            return;
        }
        self.dismiss_transient_surfaces();
        let tabs = close_target_tabs(&self.workspaces, &target);
        self.close_dialog = Some(CloseDialog {
            target,
            rows,
            discard,
            tabs,
            focused: false,
            return_focus: crate::FocusReturn::default(),
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
        let terminals = self.close_target_terminals(&target, cx);
        match (&target, intent) {
            (CloseTarget::Session(session), CloseIntent::Stop | CloseIntent::Hold) => {
                self.stop_listed_session(session, cx);
            }
            (_, CloseIntent::Stop) => self.stop_terminals(terminals.clone(), cx),
            (_, CloseIntent::Hold | CloseIntent::Detach) => {}
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
            CloseTarget::Workspace(idx) => self.remove_workspace(idx, window, cx),
            CloseTarget::DiffTerminal(index) => {
                self.remove_diff_tab(index, cx);
                if let Some(window) = window {
                    self.focus_diff_tab(self.diff_dock.diff_active_tab, window, cx);
                }
            }
        }
        match intent {
            CloseIntent::Stop => {
                let closed = terminals
                    .iter()
                    .map(|terminal| terminal.read(cx).terminal.session_id.clone())
                    .collect();
                settle_closed_sessions(&mut self.closed_panes, &closed, None);
            }
            CloseIntent::Hold => self.hold_closed_sessions(terminals, cx),
            CloseIntent::Detach => self.refresh_owned_sessions(cx),
        }
    }

    pub(crate) fn close_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = self.close_dialog.take() {
            self.return_focus(&dialog.return_focus, window, cx);
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
        self.return_focus(&dialog.return_focus, window, cx);
        if close_target_tabs(&self.workspaces, &dialog.target) != dialog.tabs {
            self.show_toast(STALE_CLOSE_MESSAGE, cx);
            cx.notify();
            return;
        }
        discard_unsaved(dialog.discard, cx);
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
            dialog.return_focus.capture_once(window, cx);
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

    #[gpui::test]
    fn a_close_target_resolves_to_other_tabs_once_the_indices_shift(cx: &mut gpui::TestAppContext) {
        use crate::workspace::{Tab, Workspace};
        use gpui::AppContext as _;

        let cx = cx.add_empty_window();
        let mut workspaces: Vec<Workspace> = (1..=2)
            .map(|id| {
                let terminal = cx.new(|cx| TerminalView::display_only_for_test(id, cx));
                let pane = cx.new(|cx| Pane::new(terminal, id, cx));
                Workspace::with_id(id, format!("ws{id}"), pane)
            })
            .collect();
        assert!(workspaces[0].open_tab(Tab::new("second", None)));
        let tab = CloseTarget::Tab {
            ws_idx: 0,
            tab_idx: 1,
        };
        let workspace = CloseTarget::Workspace(1);
        let tab_seen = close_target_tabs(&workspaces, &tab);
        let workspace_seen = close_target_tabs(&workspaces, &workspace);
        assert_eq!(tab_seen.len(), 1);
        assert_eq!(workspace_seen.len(), 1);

        workspaces[0].close_tab(0);
        assert_ne!(
            close_target_tabs(&workspaces, &tab),
            tab_seen,
            "a closed neighbor shifts the tab index to another tab"
        );

        workspaces.remove(0);
        assert_ne!(
            close_target_tabs(&workspaces, &workspace),
            workspace_seen,
            "a removed workspace shifts the workspace index"
        );
    }

    #[gpui::test]
    fn cancelling_the_close_dialog_returns_focus_to_the_pane_it_came_from(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::{AppContext as _, Focusable as _};

        let cx = cx.add_empty_window();
        let pane = |cx: &mut gpui::VisualTestContext| {
            let terminal = cx.new(|cx| TerminalView::display_only_for_test(1, cx));
            cx.new(|cx| Pane::new(terminal, 1, cx))
        };
        let (a, b, c) = (pane(cx), pane(cx), pane(cx));
        let dialog = cx.update(|_, cx| cx.focus_handle());
        let handle = |pane: &Entity<Pane>, cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| pane.read(cx).focus_handle(cx))
        };
        let (a_focus, b_focus, c_focus) = (handle(&a, cx), handle(&b, cx), handle(&c, cx));

        let mut origin = crate::FocusReturn::default();
        cx.update(|window, cx| {
            window.focus(&c_focus, cx);
            origin.capture_once(window, cx);
            window.focus(&dialog, cx);
            b_focus.focus(window, cx);
            origin.capture_once(window, cx);
            window.focus(&dialog, cx);
        });

        assert!(cx.update(|window, cx| origin.restore(window, cx)));
        cx.update(|window, _| {
            assert!(c_focus.is_focused(window));
            assert!(!a_focus.is_focused(window));
        });

        let gone = cx.update(|window, cx| {
            let closing = cx.focus_handle();
            window.focus(&closing, cx);
            crate::FocusReturn::capture(window, cx)
        });
        cx.update(|window, cx| window.focus(&dialog, cx));
        assert!(
            !cx.update(|window, cx| gone.restore(window, cx)),
            "a closed origin reports that the active pane must take focus"
        );
    }

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
