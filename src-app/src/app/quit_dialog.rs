use std::collections::HashSet;

use gpui::{
    AnyElement, AsyncApp, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, Styled, Window, div, prelude::*, px, svg,
};
use paneflow_config::schema::OnQuit;
use serde_json::Value;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::app::unsaved_dialog::UnsavedContinuation;
use crate::settings::components::{
    MODAL_PADDING, destructive_button, menu_panel, modal_backdrop, modal_card, modal_footer,
    modal_header, secondary_button, setting_text, solid_button, switch_blue, toggle_pill,
    with_alpha,
};
use crate::terminal::host_link::{self, HostLinkState, StopAllOutcome};
use crate::ui_primitives::{BODY, LABEL_SM};
use crate::update;

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;

pub(crate) use crate::app::hosted_sessions::StopTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitPlan {
    QuitNow,
    StopEverything,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitKind {
    Quit,
    UpdateRestart,
}

pub(crate) fn quit_plan(policy: OnQuit, live_sessions: usize) -> QuitPlan {
    if live_sessions == 0 {
        return QuitPlan::StopEverything;
    }
    match policy {
        OnQuit::Ask => QuitPlan::Ask,
        OnQuit::Keep => QuitPlan::QuitNow,
        OnQuit::Stop => QuitPlan::StopEverything,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UpdatePreflight {
    pub(crate) replacement_ready: bool,
    pub(crate) host_serving: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateRestartPlan {
    Proceed(QuitPlan),
    Deferred(String),
}

pub(crate) fn update_restart_plan(
    policy: OnQuit,
    live_sessions: usize,
    preflight: UpdatePreflight,
) -> UpdateRestartPlan {
    if !preflight.replacement_ready {
        return UpdateRestartPlan::Deferred(
            "The replacement is not staged on disk; download the update again before restarting."
                .to_string(),
        );
    }
    let Some(serving) = preflight.host_serving else {
        return UpdateRestartPlan::Deferred(
            "The running session host could not be checked; retry before replacing it.".into(),
        );
    };
    let serving = serving.max(live_sessions);
    if serving == 0 || policy == OnQuit::Stop {
        UpdateRestartPlan::Proceed(QuitPlan::StopEverything)
    } else {
        UpdateRestartPlan::Proceed(QuitPlan::Ask)
    }
}

fn busy_agent_counts<'a>(
    agents: impl Iterator<Item = &'a crate::ai_types::AgentSession>,
) -> (usize, usize) {
    let mut by_surface = std::collections::HashMap::new();
    let mut unbound = Vec::new();
    for agent in agents {
        match agent.surface_id {
            Some(surface) => {
                by_surface.insert(surface, agent.state);
            }
            None => unbound.push(agent.state),
        }
    }
    by_surface
        .into_values()
        .chain(unbound)
        .fold((0, 0), |(working, waiting), state| match state {
            AgentState::Thinking => (working + 1, waiting),
            AgentState::WaitingForInput => (working, waiting + 1),
            AgentState::Finished | AgentState::Errored => (working, waiting),
        })
}

pub(crate) fn quit_summary(sessions: usize, working: usize, waiting: usize) -> String {
    let mut text = format!(
        "{} running.",
        super::plural(sessions, "session is", "sessions are")
    );
    match (working, waiting) {
        (0, 0) => {}
        (w, 0) => text.push_str(&format!(
            " {} working.",
            super::plural(w, "agent is", "agents are")
        )),
        (0, i) => text.push_str(&format!(
            " {} waiting for input.",
            super::plural(i, "agent is", "agents are")
        )),
        (w, i) => text.push_str(&format!(
            " {} working and {} waiting for input.",
            super::plural(w, "agent is", "agents are"),
            super::plural(i, "is", "are")
        )),
    }
    text
}

pub(crate) struct QuitDialog {
    kind: ExitKind,
    sessions: usize,
    working: usize,
    waiting: usize,
    remember: bool,
    stopping: bool,
    focused: bool,
    return_focus: crate::FocusReturn,
    failure: Option<StopAllOutcome>,
    details_open: bool,
    selected: Option<QuitAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitFailure {
    None,
    Unresolved,
    DurabilityOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitAction {
    ToggleRemember,
    ToggleDetails,
    Cancel,
    KeepSessions,
    StopEverything,
    RetrySave,
    QuitUnsaved,
}

#[derive(Debug, PartialEq, Eq)]
struct QuitFooter {
    leading: Option<QuitAction>,
    trailing: [QuitAction; 2],
    default: QuitAction,
}

impl QuitFooter {
    fn for_exit(kind: ExitKind, failure: QuitFailure) -> Self {
        use QuitAction::*;
        match (kind, failure) {
            (_, QuitFailure::DurabilityOnly) => Self {
                leading: Some(QuitUnsaved),
                trailing: [Cancel, RetrySave],
                default: RetrySave,
            },
            (ExitKind::Quit, _) => Self {
                leading: Some(StopEverything),
                trailing: [Cancel, KeepSessions],
                default: KeepSessions,
            },
            (ExitKind::UpdateRestart, QuitFailure::None) => Self {
                leading: None,
                trailing: [Cancel, StopEverything],
                default: Cancel,
            },
            (ExitKind::UpdateRestart, QuitFailure::Unresolved) => Self {
                leading: Some(StopEverything),
                trailing: [Cancel, KeepSessions],
                default: Cancel,
            },
        }
    }

    fn focus_order(&self, body_toggle: Option<QuitAction>) -> Vec<QuitAction> {
        body_toggle
            .into_iter()
            .chain(self.leading)
            .chain(self.trailing)
            .collect()
    }
}

fn quit_action_label(action: QuitAction, kind: ExitKind, failure: QuitFailure) -> &'static str {
    match (action, kind, failure) {
        (QuitAction::ToggleRemember, _, _) => "Remember my choice",
        (QuitAction::ToggleDetails, _, _) => "Show details",
        (QuitAction::Cancel, _, _) => "Cancel",
        (QuitAction::KeepSessions, _, QuitFailure::None) => "Keep sessions running",
        (QuitAction::KeepSessions, _, _) => "Keep running and quit",
        (QuitAction::StopEverything, ExitKind::Quit, QuitFailure::None) => {
            "Stop everything and quit"
        }
        (QuitAction::StopEverything, ExitKind::UpdateRestart, QuitFailure::None) => {
            "Stop everything and restart"
        }
        (QuitAction::StopEverything, ExitKind::Quit, _) => "Retry stopping",
        (QuitAction::StopEverything, ExitKind::UpdateRestart, _) => "Retry update",
        (QuitAction::RetrySave, _, _) => "Retry save",
        (QuitAction::QuitUnsaved, ExitKind::Quit, _) => "Quit anyway",
        (QuitAction::QuitUnsaved, ExitKind::UpdateRestart, _) => "Quit without updating",
    }
}

impl PaneFlowApp {
    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
        if self.quit_dialog.is_some() || self.session_exit_pending {
            return;
        }
        let unsaved = self.all_unsaved_views(cx);
        if self.ask_about_unsaved(unsaved, UnsavedContinuation::Quit, cx) {
            return;
        }
        self.request_quit_checked(cx);
    }

    pub(crate) fn request_quit_checked(&mut self, cx: &mut Context<Self>) {
        if self.quit_dialog.is_some() || self.session_exit_pending {
            return;
        }
        let sessions = self.live_session_targets(cx).len();
        match quit_plan(self.cached_config.resolved_on_quit(), sessions) {
            QuitPlan::QuitNow => self.quit_keeping_sessions(cx),
            QuitPlan::StopEverything => self.exit_stopping_everything(ExitKind::Quit, cx),
            QuitPlan::Ask => self.open_exit_dialog(ExitKind::Quit, sessions, cx),
        }
    }

    pub(crate) fn request_update_restart(&mut self, cx: &mut Context<Self>) {
        if self.quit_dialog.is_some() || self.session_exit_pending {
            return;
        }
        let unsaved = self.all_unsaved_views(cx);
        if self.ask_about_unsaved(unsaved, UnsavedContinuation::UpdateRestart, cx) {
            return;
        }
        self.request_update_restart_checked(cx);
    }

    pub(crate) fn request_update_restart_checked(&mut self, cx: &mut Context<Self>) {
        if self.quit_dialog.is_some() || self.session_exit_pending {
            return;
        }
        let staged = self.self_update.staged_msi.clone();
        self.session_exit_pending = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let preflight = executor
                .spawn(async move {
                    let artifacts_ready = std::env::current_exe()
                        .ok()
                        .zip(paneflow_home::paneflow_home())
                        .is_some_and(|(controller, home)| {
                            paneflow_host::bootstrap::preflight_replacement(&controller, &home)
                                .is_ok()
                        });
                    UpdatePreflight {
                        replacement_ready: artifacts_ready
                            && staged
                                .as_ref()
                                .is_none_or(update::windows::msi::StagedMsiUpdate::is_staged),
                        host_serving: host_link::host_is_serving(),
                    }
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.session_exit_pending = false;
                let sessions = app.live_session_targets(cx).len();
                match update_restart_plan(app.cached_config.resolved_on_quit(), sessions, preflight)
                {
                    UpdateRestartPlan::Deferred(reason) => {
                        log::warn!("self-update: restart deferred: {reason}");
                        app.show_toast(format!("Update deferred: {reason}"), cx);
                    }
                    UpdateRestartPlan::Proceed(QuitPlan::StopEverything | QuitPlan::QuitNow) => {
                        app.exit_stopping_everything(ExitKind::UpdateRestart, cx);
                    }
                    UpdateRestartPlan::Proceed(QuitPlan::Ask) => {
                        app.open_exit_dialog(ExitKind::UpdateRestart, sessions, cx);
                    }
                }
            });
        })
        .detach();
    }

    fn open_exit_dialog(&mut self, kind: ExitKind, sessions: usize, cx: &mut Context<Self>) {
        let (working, waiting) = self.busy_agent_counts();
        self.quit_dialog = Some(QuitDialog {
            kind,
            sessions,
            working,
            waiting,
            remember: false,
            stopping: false,
            focused: false,
            return_focus: crate::FocusReturn::default(),
            failure: None,
            details_open: false,
            selected: None,
        });
        cx.notify();
    }

    fn report_stop_failure(
        &mut self,
        kind: ExitKind,
        outcome: StopAllOutcome,
        cx: &mut Context<Self>,
    ) {
        log::warn!(
            "paneflow: the stop-everything exit could not be confirmed: {}",
            outcome.detail()
        );
        self.session_exit_pending = false;
        let sessions = self.live_session_targets(cx).len();
        let (working, waiting) = self.busy_agent_counts();
        let (remember, details_open, return_focus) = self.quit_dialog.as_ref().map_or(
            (false, false, crate::FocusReturn::default()),
            |dialog| {
                (
                    dialog.remember,
                    dialog.details_open,
                    dialog.return_focus.clone(),
                )
            },
        );
        self.quit_dialog = Some(QuitDialog {
            kind,
            sessions,
            working,
            waiting,
            remember,
            stopping: false,
            focused: false,
            return_focus,
            failure: Some(outcome),
            details_open,
            selected: None,
        });
        cx.notify();
    }

    fn quit_with_unsaved_final_state(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.quit_dialog.as_ref() else {
            return;
        };
        if !dialog
            .failure
            .as_ref()
            .is_some_and(StopAllOutcome::durability_only)
        {
            return;
        }
        self.finish_quit(cx);
    }

    fn finish_update_restart(&mut self, cx: &mut Context<Self>) {
        match self.self_update.staged_msi.clone() {
            Some(staged) => {
                let executor = cx.background_executor().clone();
                cx.spawn(async move |this, cx: &mut AsyncApp| {
                    let result = executor
                        .spawn(async move { update::windows::msi::spawn_relay(staged) })
                        .await;
                    let _ = this.update(cx, |app, cx| match result {
                        Ok(()) => {
                            app.self_update.staged_msi = None;
                            log::info!(
                                "self-update/msi: sessions stopped and host shut down - relay spawned, quitting so msiexec can replace the binaries"
                            );
                            app.self_update.self_update_status =
                                update::SelfUpdateStatus::Installing;
                            app.finish_quit(cx);
                        }
                        Err(err) => {
                            app.session_exit_pending = false;
                            app.quit_dialog = None;
                            app.record_update_failure("msi-relay", &err, cx);
                        }
                    });
                })
                .detach();
            }
            None => {
                log::info!("self-update: sessions stopped - invoking cx.restart()");
                cx.restart();
            }
        }
    }

    pub(crate) fn close_quit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.session_exit_pending
            && matches!(self.quit_dialog.as_ref(), Some(dialog) if !dialog.stopping)
        {
            if let Some(dialog) = self.quit_dialog.take() {
                self.return_focus(&dialog.return_focus, window, cx);
            }
            cx.notify();
        }
    }

    fn busy_agent_counts(&self) -> (usize, usize) {
        busy_agent_counts(
            self.workspaces
                .iter()
                .flat_map(|ws| ws.agent_sessions.values()),
        )
    }

    pub(crate) fn live_session_targets(&self, cx: &gpui::App) -> Vec<StopTarget> {
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for terminal in self.attached_terminals(cx) {
            let state = &terminal.read(cx).terminal;
            let Some(hosted) = state.hosted.as_ref() else {
                continue;
            };
            let live = state.exited.is_none()
                && matches!(
                    state.host_link,
                    HostLinkState::Attaching
                        | HostLinkState::Attached
                        | HostLinkState::Reconnecting
                );
            if live && seen.insert(state.session_id.clone()) {
                targets.push((
                    hosted.endpoint.clone(),
                    state.session_id.clone(),
                    hosted.generation,
                ));
            }
        }
        let held = crate::app::workspace_ops::undo_window_session_ids(&self.closed_panes);
        if let Some(target) = host_link::host_endpoint() {
            for listed in self.live_owned_sessions() {
                if !held.contains(&listed.session) && seen.insert(listed.session.clone()) {
                    targets.push((
                        target.endpoint.clone(),
                        listed.session.clone(),
                        listed.generation,
                    ));
                }
            }
        }
        targets
    }

    fn remember_quit_choice(&mut self, choice: OnQuit, cx: &mut Context<Self>) {
        let Some(dialog) = self.quit_dialog.as_ref() else {
            return;
        };
        if !dialog.remember {
            return;
        }
        let value = Value::String(choice.wire_str().to_string());
        let Ok(next) =
            crate::config_writer::with_field(&self.cached_config, false, "on_quit", value.clone())
        else {
            return;
        };
        self.cached_config = next;
        crate::config_snapshot::publish(&self.cached_config, cx);
        self.quit_choice_write = Some(cx.background_spawn(smol::unblock(move || {
            crate::config_writer::save_config_value_checked("on_quit", value)
        })));
    }

    fn quit_keeping_sessions(&mut self, cx: &mut Context<Self>) {
        self.remember_quit_choice(OnQuit::Keep, cx);
        let held = self.release_undo_window_sessions();
        if held.is_empty() {
            self.quit_now(cx);
            return;
        }
        self.session_exit_pending = true;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            executor
                .spawn(async move { crate::app::hosted_sessions::stop_and_forget(held) })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.session_exit_pending = false;
                app.quit_now(cx);
            });
        })
        .detach();
    }

    fn exit_stopping_everything(&mut self, kind: ExitKind, cx: &mut Context<Self>) {
        if kind == ExitKind::Quit {
            self.remember_quit_choice(OnQuit::Stop, cx);
        }
        self.save_session_before_exit(cx, move |app, cx| {
            let mut targets = app.live_session_targets(cx);
            let endpoint = host_link::host_endpoint().map(|target| target.endpoint);
            app.session_exit_pending = true;
            if let Some(dialog) = app.quit_dialog.as_mut() {
                dialog.stopping = true;
                dialog.sessions = targets.len();
            }
            targets.extend(app.release_undo_window_sessions());
            cx.notify();
            let executor = cx.background_executor().clone();
            cx.spawn(async move |this, cx: &mut AsyncApp| {
                let outcome = executor
                    .spawn(async move { host_link::stop_sessions_and_shutdown(targets, endpoint) })
                    .await;
                let _ = this.update(cx, |app, cx| {
                    if !outcome.confirmed() {
                        app.report_stop_failure(kind, outcome, cx);
                        return;
                    }
                    app.session_exit_pending = false;
                    app.save_stopped_session_before_exit(cx, move |app, cx| {
                        app.session_exit_pending = true;
                        match kind {
                            ExitKind::Quit => app.finish_quit(cx),
                            ExitKind::UpdateRestart => app.finish_update_restart(cx),
                        }
                    });
                });
            })
            .detach();
        });
    }

    fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.save_session_before_exit(cx, |app, cx| app.finish_quit(cx));
    }

    fn finish_quit(&mut self, cx: &mut Context<Self>) {
        if let Some(write) = self.quit_choice_write.take() {
            cx.spawn(async move |this, cx| {
                if !write.await {
                    log::warn!("quit: the remembered choice could not be saved");
                }
                let _ = this.update(cx, |app, cx| app.finish_quit(cx));
            })
            .detach();
            return;
        }
        self.emit_app_exited_and_flush();
        cx.quit();
    }

    fn quit_dialog_footer_state(&self) -> Option<(QuitFooter, Vec<QuitAction>, QuitAction)> {
        let dialog = self.quit_dialog.as_ref()?;
        let footer = QuitFooter::for_exit(dialog.kind, quit_failure(dialog.failure.as_ref()));
        let order = footer.focus_order(quit_body_toggle(dialog));
        let selected = dialog
            .selected
            .filter(|action| order.contains(action))
            .unwrap_or(footer.default);
        Some((footer, order, selected))
    }

    fn handle_quit_dialog_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            self.quit_dialog.as_ref(),
            None | Some(QuitDialog { stopping: true, .. })
        ) {
            return;
        }
        let Some((_, order, selected)) = self.quit_dialog_footer_state() else {
            return;
        };
        let position = order
            .iter()
            .position(|action| *action == selected)
            .unwrap_or(0);
        let backward = event.keystroke.modifiers.shift;
        let step = match event.keystroke.key.as_str() {
            "escape" => {
                self.close_quit_dialog(window, cx);
                cx.stop_propagation();
                return;
            }
            "enter" | "space" => {
                self.activate_quit_action(selected, window, cx);
                cx.stop_propagation();
                return;
            }
            "tab" if backward => order.len() - 1,
            "tab" | "right" => 1,
            "left" => order.len() - 1,
            _ => return,
        };
        if let Some(dialog) = self.quit_dialog.as_mut() {
            dialog.selected = Some(order[(position + step) % order.len()]);
            cx.notify();
        }
        cx.stop_propagation();
    }

    fn activate_quit_action(
        &mut self,
        action: QuitAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(kind) = self.quit_dialog.as_ref().map(|dialog| dialog.kind) else {
            return;
        };
        match action {
            QuitAction::ToggleRemember => {
                if let Some(dialog) = self.quit_dialog.as_mut() {
                    dialog.remember = !dialog.remember;
                    dialog.selected = Some(QuitAction::ToggleRemember);
                    cx.notify();
                }
            }
            QuitAction::ToggleDetails => {
                if let Some(dialog) = self.quit_dialog.as_mut() {
                    dialog.details_open = !dialog.details_open;
                    dialog.selected = Some(QuitAction::ToggleDetails);
                    cx.notify();
                }
            }
            QuitAction::Cancel => self.close_quit_dialog(window, cx),
            QuitAction::KeepSessions => self.quit_keeping_sessions(cx),
            QuitAction::StopEverything | QuitAction::RetrySave => {
                self.exit_stopping_everything(kind, cx)
            }
            QuitAction::QuitUnsaved => self.quit_with_unsaved_final_state(cx),
        }
    }

    pub(crate) fn render_quit_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(dialog) = self.quit_dialog.as_mut() else {
            return div().into_any_element();
        };
        if !dialog.focused {
            dialog.focused = true;
            dialog.return_focus.capture_once(window, cx);
            self.quit_dialog_focus.focus(window, cx);
        }
        let Some(dialog) = self.quit_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let stopping = dialog.stopping;
        let kind = dialog.kind;
        let failure = dialog.failure.clone();
        let durability_only = failure
            .as_ref()
            .is_some_and(StopAllOutcome::durability_only);
        let (question, explanation_text): (&str, String) = match (kind, &failure) {
            (ExitKind::Quit, None) => (
                "Quit Paneflow?",
                "Kept sessions come back the next time Paneflow opens. \
                 Stopping ends them and every process they started."
                    .to_string(),
            ),
            (ExitKind::UpdateRestart, None) => (
                "Restart to update?",
                "Updating ends every session and every process they started.".to_string(),
            ),
            (ExitKind::Quit, Some(outcome)) if durability_only => (
                "Unable to save session state",
                format!("{} Retry, or quit anyway.", outcome.summary()),
            ),
            (ExitKind::UpdateRestart, Some(outcome)) if durability_only => (
                "Unable to save session state",
                format!(
                    "{} Nothing was replaced. Retry, or quit without updating.",
                    outcome.summary()
                ),
            ),
            (ExitKind::Quit, Some(outcome)) => (
                "Some sessions may still be running",
                format!(
                    "{} Retry, or quit and leave them running.",
                    outcome.summary()
                ),
            ),
            (ExitKind::UpdateRestart, Some(outcome)) => (
                "Unable to update yet",
                format!(
                    "{} Nothing was replaced. Retry, or update later.",
                    outcome.summary()
                ),
            ),
        };
        let summary = if stopping {
            format!(
                "Stopping {}...",
                super::plural(dialog.sessions, "session", "sessions")
            )
        } else if durability_only {
            "All sessions stopped.".to_string()
        } else {
            quit_summary(dialog.sessions, dialog.working, dialog.waiting)
        };

        let header = modal_header(ui, question, summary);

        let explanation = div()
            .px(MODAL_PADDING)
            .pb(px(14.))
            .text_size(BODY)
            .line_height(px(18.))
            .text_color(ui.muted)
            .child(explanation_text);

        let remember = dialog.remember;
        let failure_state = quit_failure(failure.as_ref());
        let remember_shown = quit_remember_shown(dialog);
        let Some((footer_actions, _, selected)) = self.quit_dialog_footer_state() else {
            return div().into_any_element();
        };
        let remember_row = div()
            .id("quit-dialog-remember")
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .mx(MODAL_PADDING)
            .px(px(12.))
            .py(px(10.))
            .rounded(px(QUIT_REMEMBER_RADIUS))
            .bg(with_alpha(ui.subtle, 0.5))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.activate_quit_action(QuitAction::ToggleRemember, window, cx);
                cx.stop_propagation();
            }))
            .child(setting_text(
                ui,
                quit_action_label(QuitAction::ToggleRemember, kind, failure_state),
                "Change it in Settings > General.",
            ))
            .child(toggle_pill(remember, ui))
            .when(selected == QuitAction::ToggleRemember, |row| {
                row.child(quit_focus_ring(px(QUIT_REMEMBER_RADIUS), ui))
            });

        let details_open = dialog.details_open;
        let details = failure
            .as_ref()
            .filter(|_| quit_details_shown(dialog))
            .map(|outcome| {
                let trigger = div()
                    .id(quit_action_id(QuitAction::ToggleDetails))
                    .relative()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .cursor(CursorStyle::PointingHand)
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.activate_quit_action(QuitAction::ToggleDetails, window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .path(if details_open {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            })
                            .size(px(12.))
                            .flex_none()
                            .text_color(ui.muted),
                    )
                    .child(if details_open {
                        "Hide details"
                    } else {
                        quit_action_label(QuitAction::ToggleDetails, kind, failure_state)
                    })
                    .when(selected == QuitAction::ToggleDetails, |row| {
                        row.child(quit_focus_ring(px(QUIT_DETAILS_TRIGGER_RADIUS), ui))
                    });
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(8.))
                    .px(MODAL_PADDING)
                    .pb(px(14.))
                    .child(trigger)
                    .when(details_open, |block| {
                        block.child(
                            menu_panel(div().id("quit-dialog-details-text"), ui)
                                .w_full()
                                .max_h(QUIT_DETAILS_MAX_HEIGHT)
                                .overflow_x_hidden()
                                .overflow_y_scroll()
                                .children(outcome.detail_lines().into_iter().map(|line| {
                                    div()
                                        .min_w_0()
                                        .px(px(3.))
                                        .font_family(QUIT_DETAILS_FONT)
                                        .text_size(LABEL_SM)
                                        .line_height(QUIT_DETAILS_LINE_HEIGHT)
                                        .text_color(ui.text)
                                        .child(line)
                                })),
                        )
                    })
            });

        let mut button = |action: QuitAction| {
            let label = quit_action_label(action, kind, failure_state);
            let id = quit_action_id(action);
            let on_click = cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.activate_quit_action(action, window, cx);
                cx.stop_propagation();
            });
            let control = if action == footer_actions.default && action != QuitAction::Cancel {
                solid_button(id, label, switch_blue())
                    .on_click(on_click)
                    .into_any_element()
            } else if matches!(action, QuitAction::StopEverything | QuitAction::QuitUnsaved) {
                destructive_button(id, label)
                    .on_click(on_click)
                    .into_any_element()
            } else {
                secondary_button(id, label, ui, on_click).into_any_element()
            };
            div()
                .relative()
                .flex_none()
                .child(control)
                .when(selected == action, |slot| {
                    slot.child(quit_focus_ring(crate::ui_primitives::ROW_RADIUS, ui))
                })
                .into_any_element()
        };
        let leading = footer_actions.leading.map(&mut button);
        let trailing = footer_actions.trailing.map(&mut button);
        let footer = modal_footer().when(!stopping, |footer| {
            footer
                .children(leading)
                .child(div().flex_1())
                .children(trailing)
        });

        let card = modal_card(
            "quit-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div()
                .child(header)
                .child(explanation)
                .children(details)
                .when(remember_shown, |body| body.child(remember_row))
                .child(footer),
        )
        .track_focus(&self.quit_dialog_focus)
        .on_key_down(cx.listener(Self::handle_quit_dialog_key_down));

        modal_backdrop(
            "quit-dialog-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_quit_dialog(window, cx);
            }),
        )
    }
}

const QUIT_FOCUS_RING_GAP: f32 = 3.;
const QUIT_REMEMBER_RADIUS: f32 = 8.;
const QUIT_DETAILS_TRIGGER_RADIUS: f32 = 4.;
const QUIT_DETAILS_MAX_HEIGHT: Pixels = px(160.);
const QUIT_DETAILS_LINE_HEIGHT: Pixels = px(16.);
const QUIT_DETAILS_FONT: &str = "Geist Mono";

fn quit_failure(failure: Option<&StopAllOutcome>) -> QuitFailure {
    match failure {
        None => QuitFailure::None,
        Some(outcome) if outcome.durability_only() => QuitFailure::DurabilityOnly,
        Some(_) => QuitFailure::Unresolved,
    }
}

fn quit_remember_shown(dialog: &QuitDialog) -> bool {
    !dialog.stopping && dialog.kind == ExitKind::Quit && dialog.failure.is_none()
}

fn quit_details_shown(dialog: &QuitDialog) -> bool {
    !dialog.stopping && dialog.failure.is_some()
}

fn quit_body_toggle(dialog: &QuitDialog) -> Option<QuitAction> {
    if quit_remember_shown(dialog) {
        Some(QuitAction::ToggleRemember)
    } else if quit_details_shown(dialog) {
        Some(QuitAction::ToggleDetails)
    } else {
        None
    }
}

fn quit_action_id(action: QuitAction) -> &'static str {
    match action {
        QuitAction::ToggleRemember => "quit-dialog-remember",
        QuitAction::ToggleDetails => "quit-dialog-details",
        QuitAction::Cancel => "quit-dialog-cancel",
        QuitAction::KeepSessions => "quit-dialog-keep",
        QuitAction::StopEverything => "quit-dialog-stop",
        QuitAction::RetrySave => "quit-dialog-retry-save",
        QuitAction::QuitUnsaved => "quit-dialog-unsaved",
    }
}

fn quit_focus_ring(radius: Pixels, ui: crate::theme::UiColors) -> impl IntoElement {
    let gap = px(QUIT_FOCUS_RING_GAP);
    div()
        .absolute()
        .top(-gap)
        .right(-gap)
        .bottom(-gap)
        .left(-gap)
        .child(crate::ui_primitives::squircle::squircle_border(
            radius + gap,
            px(1.),
            ui.text.opacity(0.35),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_listed_under_two_workspaces_is_counted_once() {
        let agent = |surface: u64, state: AgentState| {
            let mut session = crate::ai_types::AgentSession::new(
                crate::agent_launcher::TerminalAgent::ClaudeCode,
                state,
            );
            session.surface_id = Some(surface);
            session
        };
        let source = [agent(7, AgentState::Thinking)];
        let dest = [
            agent(7, AgentState::Thinking),
            agent(8, AgentState::WaitingForInput),
        ];
        assert_eq!(busy_agent_counts(source.iter().chain(dest.iter())), (1, 1));
    }

    #[test]
    fn no_live_session_shuts_the_idle_host_down_without_asking_whatever_the_policy() {
        for policy in [OnQuit::Ask, OnQuit::Keep, OnQuit::Stop] {
            assert_eq!(quit_plan(policy, 0), QuitPlan::StopEverything);
        }
    }

    #[test]
    fn live_sessions_follow_the_remembered_policy() {
        assert_eq!(quit_plan(OnQuit::Ask, 2), QuitPlan::Ask);
        assert_eq!(quit_plan(OnQuit::Keep, 2), QuitPlan::QuitNow);
        assert_eq!(quit_plan(OnQuit::Stop, 1), QuitPlan::StopEverything);
    }

    #[test]
    fn an_update_restart_never_pretends_sessions_can_be_kept() {
        let ready = UpdatePreflight {
            replacement_ready: true,
            host_serving: Some(0),
        };
        for policy in [OnQuit::Ask, OnQuit::Keep, OnQuit::Stop] {
            assert_eq!(
                update_restart_plan(policy, 0, ready),
                UpdateRestartPlan::Proceed(QuitPlan::StopEverything)
            );
        }
        assert_eq!(
            update_restart_plan(OnQuit::Ask, 2, ready),
            UpdateRestartPlan::Proceed(QuitPlan::Ask)
        );
        assert_eq!(
            update_restart_plan(OnQuit::Keep, 2, ready),
            UpdateRestartPlan::Proceed(QuitPlan::Ask)
        );
        assert_eq!(
            update_restart_plan(OnQuit::Stop, 2, ready),
            UpdateRestartPlan::Proceed(QuitPlan::StopEverything)
        );
    }

    #[test]
    fn an_update_preflight_defers_a_missing_replacement_and_counts_the_host_sessions() {
        let missing = UpdatePreflight {
            replacement_ready: false,
            host_serving: Some(0),
        };
        assert!(matches!(
            update_restart_plan(OnQuit::Stop, 0, missing),
            UpdateRestartPlan::Deferred(reason) if reason.contains("not staged")
        ));
        let host_busy = UpdatePreflight {
            replacement_ready: true,
            host_serving: Some(3),
        };
        assert_eq!(
            update_restart_plan(OnQuit::Keep, 0, host_busy),
            UpdateRestartPlan::Proceed(QuitPlan::Ask),
            "sessions the desktop does not show still hold the host binary in use"
        );
    }

    #[test]
    fn summary_counts_sessions_and_agents_in_plain_words() {
        assert_eq!(quit_summary(1, 0, 0), "1 session is running.");
        assert_eq!(
            quit_summary(3, 2, 0),
            "3 sessions are running. 2 agents are working."
        );
        assert_eq!(
            quit_summary(2, 0, 1),
            "2 sessions are running. 1 agent is waiting for input."
        );
        assert_eq!(
            quit_summary(4, 1, 2),
            "4 sessions are running. 1 agent is working and 2 are waiting for input."
        );
    }

    #[test]
    fn the_enter_default_is_the_emphasized_action_and_the_destructive_one_stands_apart() {
        use QuitAction::*;
        let quit = QuitFooter::for_exit(ExitKind::Quit, QuitFailure::None);
        assert_eq!(quit.leading, Some(StopEverything));
        assert_eq!(quit.trailing, [Cancel, KeepSessions]);
        assert_eq!(quit.default, KeepSessions);
        assert_eq!(
            quit.focus_order(Some(ToggleRemember)),
            vec![ToggleRemember, StopEverything, Cancel, KeepSessions]
        );

        let restart = QuitFooter::for_exit(ExitKind::UpdateRestart, QuitFailure::None);
        assert_eq!(restart.trailing, [Cancel, StopEverything]);
        assert_eq!(restart.default, Cancel);

        for kind in [ExitKind::Quit, ExitKind::UpdateRestart] {
            let unsaved = QuitFooter::for_exit(kind, QuitFailure::DurabilityOnly);
            assert_eq!(unsaved.default, RetrySave);
            assert_eq!(
                unsaved.focus_order(Some(ToggleDetails)),
                vec![ToggleDetails, QuitUnsaved, Cancel, RetrySave]
            );
        }
        assert_eq!(
            QuitFooter::for_exit(ExitKind::UpdateRestart, QuitFailure::Unresolved).default,
            Cancel
        );
    }

    #[test]
    fn a_failure_offers_collapsed_details_while_the_first_question_offers_remember() {
        let dialog = |kind, stopping, failure: Option<StopAllOutcome>| QuitDialog {
            kind,
            sessions: 1,
            working: 0,
            waiting: 0,
            remember: false,
            stopping,
            focused: false,
            return_focus: crate::FocusReturn::default(),
            failure,
            details_open: false,
            selected: None,
        };
        let unsaved = StopAllOutcome {
            unsaved: vec![(
                paneflow_config::schema::SessionId::new(),
                "disk full".into(),
            )],
            ..StopAllOutcome::default()
        };
        assert_eq!(
            quit_body_toggle(&dialog(ExitKind::Quit, false, None)),
            Some(QuitAction::ToggleRemember)
        );
        assert_eq!(
            quit_body_toggle(&dialog(ExitKind::UpdateRestart, false, None)),
            None
        );
        for kind in [ExitKind::Quit, ExitKind::UpdateRestart] {
            let failed = dialog(kind, false, Some(unsaved.clone()));
            assert_eq!(quit_body_toggle(&failed), Some(QuitAction::ToggleDetails));
            assert!(!failed.details_open, "details start collapsed");
        }
        assert_eq!(
            quit_body_toggle(&dialog(ExitKind::Quit, true, Some(unsaved))),
            None,
            "nothing to toggle while the stop runs"
        );
    }
}
