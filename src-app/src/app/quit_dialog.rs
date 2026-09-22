use std::collections::HashSet;

use gpui::{
    AnyElement, AsyncApp, ClickEvent, Context, CursorStyle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseButton, ParentElement, Pixels, Styled, Window, deferred, div,
    hsla, prelude::*, px,
};
use paneflow_config::schema::OnQuit;
use serde_json::Value;

use crate::PaneFlowApp;
use crate::ai_types::AgentState;
use crate::settings::components::{
    card_color, destructive_button, secondary_button, setting_text, toggle_pill, with_alpha,
};
use crate::terminal::host_link::{self, HostLinkState, StopAllOutcome};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{BODY, LABEL_SM, TITLE};
use crate::update;

const DIALOG_WIDTH: Pixels = px(460.);
const CARD_RADIUS: Pixels = crate::app::constants::PANE_CARD_RADIUS;
const CARD_PADDING: Pixels = px(20.);

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

fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("1 {singular}")
    } else {
        format!("{n} {plural}")
    }
}

pub(crate) fn quit_summary(sessions: usize, working: usize, waiting: usize) -> String {
    let mut text = format!(
        "{} still running.",
        count(sessions, "session is", "sessions are")
    );
    match (working, waiting) {
        (0, 0) => {}
        (w, 0) => text.push_str(&format!(
            " {} still working.",
            count(w, "agent is", "agents are")
        )),
        (0, i) => text.push_str(&format!(
            " {} waiting for your input.",
            count(i, "agent is", "agents are")
        )),
        (w, i) => text.push_str(&format!(
            " {} working and {} waiting for your input.",
            count(w, "agent is", "agents are"),
            count(i, "is", "are")
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
    failure: Option<StopAllOutcome>,
}

impl PaneFlowApp {
    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
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
            failure: None,
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
            outcome.user_message()
        );
        self.session_exit_pending = false;
        let sessions = self.live_session_targets(cx).len();
        let (working, waiting) = self.busy_agent_counts();
        let remember = self
            .quit_dialog
            .as_ref()
            .is_some_and(|dialog| dialog.remember);
        self.quit_dialog = Some(QuitDialog {
            kind,
            sessions,
            working,
            waiting,
            remember,
            stopping: false,
            focused: false,
            failure: Some(outcome),
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
            self.quit_dialog = None;
            if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
                ws.focus_first(window, cx);
            }
            cx.notify();
        }
    }

    fn busy_agent_counts(&self) -> (usize, usize) {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.agent_sessions.values())
            .fold((0, 0), |(working, waiting), agent| match agent.state {
                AgentState::Thinking => (working + 1, waiting),
                AgentState::WaitingForInput => (working, waiting + 1),
                AgentState::Finished | AgentState::Errored => (working, waiting),
            })
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
        if let Some(target) = host_link::host_endpoint() {
            for listed in self.live_owned_sessions() {
                if seen.insert(listed.session.clone()) {
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

    fn remember_quit_choice(&mut self, choice: OnQuit) {
        let Some(dialog) = self.quit_dialog.as_ref() else {
            return;
        };
        if !dialog.remember {
            return;
        }
        let value = Value::String(choice.wire_str().to_string());
        self.cached_config =
            crate::config_writer::with_field(&self.cached_config, false, "on_quit", value.clone());
        crate::config_writer::save_config_value_checked("on_quit", value);
    }

    fn quit_keeping_sessions(&mut self, cx: &mut Context<Self>) {
        self.remember_quit_choice(OnQuit::Keep);
        self.quit_now(cx);
    }

    fn exit_stopping_everything(&mut self, kind: ExitKind, cx: &mut Context<Self>) {
        if kind == ExitKind::Quit {
            self.remember_quit_choice(OnQuit::Stop);
        }
        self.save_session_before_exit(cx, move |app, cx| {
            let targets = app.live_session_targets(cx);
            let endpoint = host_link::host_endpoint().map(|target| target.endpoint);
            app.session_exit_pending = true;
            if let Some(dialog) = app.quit_dialog.as_mut() {
                dialog.stopping = true;
                dialog.sessions = targets.len();
            }
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
                    match kind {
                        ExitKind::Quit => app.finish_quit(cx),
                        ExitKind::UpdateRestart => app.finish_update_restart(cx),
                    }
                });
            })
            .detach();
        });
    }

    fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.save_session_before_exit(cx, |app, cx| app.finish_quit(cx));
    }

    fn finish_quit(&mut self, cx: &mut Context<Self>) {
        self.emit_app_exited_and_flush();
        #[cfg(target_os = "linux")]
        crate::window_chrome::linux_backdrop::clear_subtle_chrome_material();
        cx.quit();
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
        match event.keystroke.key.as_str() {
            "escape" => {
                self.close_quit_dialog(window, cx);
                cx.stop_propagation();
            }
            "enter" if matches!(self.quit_dialog.as_ref(), Some(dialog) if dialog.kind == ExitKind::Quit) =>
            {
                self.quit_keeping_sessions(cx);
                cx.stop_propagation();
            }
            _ => {}
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
        let (question, explanation_text, stop_label): (&str, String, &str) = match (kind, &failure)
        {
            (ExitKind::Quit, None) => (
                "Quit Paneflow?",
                "Keep them running and they are right there the next time Paneflow opens. \
                 Stop everything to end every session and the processes it started."
                    .to_string(),
                "Stop everything and quit",
            ),
            (ExitKind::UpdateRestart, None) => (
                "Restart into the new version?",
                "The update replaces the session host, which ends every running session and \
                 the processes it started. Cancel to keep them going and restart later."
                    .to_string(),
                "Stop everything and restart",
            ),
            (_, Some(outcome)) if durability_only => (
                "The final state could not be saved",
                format!(
                    "Every session stopped. {}. Retry the save, or leave with the final state unsaved.",
                    outcome.user_message()
                ),
                "Retry",
            ),
            (ExitKind::Quit, Some(outcome)) => (
                "Some sessions could not be confirmed stopped",
                format!(
                    "{}. The local host keeps owning them; nothing was assumed. Retry, keep them running and quit, or cancel.",
                    outcome.user_message()
                ),
                "Retry",
            ),
            (ExitKind::UpdateRestart, Some(outcome)) => (
                "The update cannot replace the host yet",
                format!(
                    "{}. The local host keeps owning them, so the replacement is deferred. Retry, or cancel and update later.",
                    outcome.user_message()
                ),
                "Retry",
            ),
        };
        let summary = if stopping {
            format!(
                "Stopping {}...",
                count(dialog.sessions, "session", "sessions")
            )
        } else {
            quit_summary(dialog.sessions, dialog.working, dialog.waiting)
        };

        let header = div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(CARD_PADDING)
            .pt(px(16.))
            .pb(px(12.))
            .child(
                div()
                    .text_size(TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child(question),
            )
            .child(
                div()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(summary),
            );

        let explanation = div()
            .px(CARD_PADDING)
            .pb(px(14.))
            .text_size(BODY)
            .line_height(px(18.))
            .text_color(ui.muted)
            .child(explanation_text);

        let remember = dialog.remember;
        let remember_row = div()
            .id("quit-dialog-remember")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .mx(CARD_PADDING)
            .px(px(12.))
            .py(px(10.))
            .rounded(px(8.))
            .bg(with_alpha(ui.subtle, 0.5))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                if let Some(dialog) = this.quit_dialog.as_mut() {
                    dialog.remember = !dialog.remember;
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(setting_text(
                ui,
                "Remember my choice",
                "Change it later in Settings > General.",
            ))
            .child(toggle_pill(remember, ui));

        let footer = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_end()
            .gap(px(8.))
            .px(CARD_PADDING)
            .pt(px(18.))
            .pb(px(16.))
            .when(!stopping, |footer| {
                footer
                    .child(secondary_button(
                        "quit-dialog-cancel",
                        "Cancel",
                        ui,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.close_quit_dialog(window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(destructive_button("quit-dialog-stop", stop_label).on_click(
                        cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.exit_stopping_everything(kind, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .when(durability_only, |footer| {
                        footer.child(secondary_button(
                            "quit-dialog-unsaved",
                            "Quit with unsaved final state",
                            ui,
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.quit_with_unsaved_final_state(cx);
                                cx.stop_propagation();
                            }),
                        ))
                    })
                    .when(
                        (kind == ExitKind::Quit || failure.is_some()) && !durability_only,
                        |footer| {
                            footer.child(secondary_button(
                                "quit-dialog-keep",
                                if failure.is_some() {
                                    "Keep running and quit"
                                } else {
                                    "Keep sessions running"
                                },
                                ui,
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    this.quit_keeping_sessions(cx);
                                    cx.stop_propagation();
                                }),
                            ))
                        },
                    )
            });

        let card = div()
            .id("quit-dialog")
            .occlude()
            .track_focus(&self.quit_dialog_focus)
            .on_key_down(cx.listener(Self::handle_quit_dialog_key_down))
            .relative()
            .w(DIALOG_WIDTH)
            .rounded(CARD_RADIUS)
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(squircle_fill(CARD_RADIUS, card_color()))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(explanation)
                    .when(
                        !stopping && kind == ExitKind::Quit && failure.is_none(),
                        |body| body.child(remember_row),
                    )
                    .child(footer),
            )
            .child(squircle_border(
                CARD_RADIUS,
                px(1.),
                with_alpha(ui.border, 0.6),
            ));

        deferred(
            div()
                .id("quit-dialog-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.55))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.close_quit_dialog(window, cx);
                    }),
                )
                .child(card),
        )
        .with_priority(10)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(quit_summary(1, 0, 0), "1 session is still running.");
        assert_eq!(
            quit_summary(3, 2, 0),
            "3 sessions are still running. 2 agents are still working."
        );
        assert_eq!(
            quit_summary(2, 0, 1),
            "2 sessions are still running. 1 agent is waiting for your input."
        );
        assert_eq!(
            quit_summary(4, 1, 2),
            "4 sessions are still running. 1 agent is working and 2 are waiting for your input."
        );
    }
}
