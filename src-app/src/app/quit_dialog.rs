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
use crate::terminal::host_link::{self, HostLinkState};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{BODY, LABEL_SM, TITLE};

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
    sessions: usize,
    working: usize,
    waiting: usize,
    remember: bool,
    stopping: bool,
    focused: bool,
}

impl PaneFlowApp {
    pub(crate) fn request_quit(&mut self, cx: &mut Context<Self>) {
        if self.quit_dialog.is_some() {
            return;
        }
        let sessions = self.live_session_targets(cx).len();
        match quit_plan(self.cached_config.resolved_on_quit(), sessions) {
            QuitPlan::QuitNow => self.quit_keeping_sessions(cx),
            QuitPlan::StopEverything => self.quit_stopping_everything(cx),
            QuitPlan::Ask => {
                let (working, waiting) = self.busy_agent_counts();
                self.quit_dialog = Some(QuitDialog {
                    sessions,
                    working,
                    waiting,
                    remember: false,
                    stopping: false,
                    focused: false,
                });
                cx.notify();
            }
        }
    }

    pub(crate) fn close_quit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.quit_dialog.as_ref(), Some(dialog) if !dialog.stopping) {
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

    fn quit_stopping_everything(&mut self, cx: &mut Context<Self>) {
        self.remember_quit_choice(OnQuit::Stop);
        let targets = self.live_session_targets(cx);
        let endpoint = host_link::host_endpoint().map(|target| target.endpoint);
        if let Some(dialog) = self.quit_dialog.as_mut() {
            dialog.stopping = true;
            dialog.sessions = targets.len();
        }
        cx.notify();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            executor
                .spawn(async move { host_link::stop_sessions_and_shutdown(targets, endpoint) })
                .await;
            let _ = this.update(cx, |app, cx| app.quit_now(cx));
        })
        .detach();
    }

    fn quit_now(&mut self, cx: &mut Context<Self>) {
        self.save_session_blocking(cx);
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
            "enter" => {
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
                    .child("Quit Paneflow?"),
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
            .child(
                "Keep them running and they are right there the next time Paneflow opens. \
                 Stop everything to end every session and the processes it started.",
            );

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
                    .child(
                        destructive_button("quit-dialog-stop", "Stop everything and quit")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.quit_stopping_everything(cx);
                                cx.stop_propagation();
                            })),
                    )
                    .child(secondary_button(
                        "quit-dialog-keep",
                        "Keep sessions running",
                        ui,
                        cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.quit_keeping_sessions(cx);
                            cx.stop_propagation();
                        }),
                    ))
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
                    .when(!stopping, |body| body.child(remember_row))
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
