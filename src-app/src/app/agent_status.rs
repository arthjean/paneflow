use gpui::Context;

use crate::agent_launcher::TerminalAgent;
use crate::ai_types::{self, AgentLifecycleEvent, AgentStateSource};

use crate::PaneFlowApp;
use crate::app::ipc_handler::upsert_session_state;

const FINISHED_LINGER: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Observation {
    Written,
    Settled,
    Refused,
}

pub(crate) fn completion_was_seen(
    visible: Option<&std::collections::HashSet<u64>>,
    surface_id: Option<u64>,
) -> bool {
    match surface_id {
        Some(id) => visible.is_some_and(|visible| visible.contains(&id)),
        None => visible.is_some(),
    }
}

impl PaneFlowApp {
    pub(crate) fn surfaces_under_user_eye(
        &self,
        workspace_id: u64,
        cx: &gpui::App,
    ) -> Option<std::collections::HashSet<u64>> {
        if !crate::agents::notifications::window_active() {
            return None;
        }
        let ws = self.workspaces.iter().find(|ws| ws.id == workspace_id)?;
        let main_visible = self.settings_section.is_none()
            && self
                .workspaces
                .get(self.active_idx)
                .is_some_and(|active| active.id == workspace_id)
            && cx.windows().into_iter().any(|window| {
                window.downcast::<PaneFlowApp>().is_some()
                    && crate::agents::notifications::is_window_active(window.window_id())
            });
        let mut visible = std::collections::HashSet::new();
        for pane in ws.collect_panes() {
            let state = pane.read(cx);
            let shown = match state.detached {
                Some(placement) => {
                    crate::agents::notifications::is_window_active(placement.window.window_id())
                }
                None => {
                    main_visible
                        && ws
                            .active_tab()
                            .root
                            .as_ref()
                            .is_some_and(|root| root.contains_leaf(&pane))
                }
            };
            if shown && let Some(terminal) = state.active_terminal_opt() {
                visible.insert(terminal.entity_id().as_u64());
            }
        }
        (!visible.is_empty()).then_some(visible)
    }

    pub(super) fn session_is_seen(&self, workspace_id: u64, key: u32, cx: &gpui::App) -> bool {
        let surface = self
            .workspaces
            .iter()
            .find(|ws| ws.id == workspace_id)
            .and_then(|ws| ws.agent_sessions.get(&key))
            .and_then(|session| session.surface_id);
        completion_was_seen(
            self.surfaces_under_user_eye(workspace_id, cx).as_ref(),
            surface,
        )
    }

    pub(crate) fn apply_observed_agent_state(
        &mut self,
        surface_id: u64,
        tool: TerminalAgent,
        pid: Option<u32>,
        event: AgentLifecycleEvent,
        source: AgentStateSource,
        cx: &mut Context<Self>,
    ) -> Observation {
        let Some(ws_id) = self.workspace_id_for_surface(surface_id, cx) else {
            return Observation::Refused;
        };
        let bound = self
            .workspaces
            .iter()
            .find(|ws| ws.id == ws_id)
            .and_then(|ws| {
                ws.agent_sessions
                    .iter()
                    .find(|(_, session)| session.surface_id == Some(surface_id))
                    .map(|(key, _)| *key)
            });
        if bound.is_none() && !opens_a_session(&event) {
            return Observation::Settled;
        }
        let Some(key_hint) = bound.or(pid).or_else(|| {
            self.surface_child_pid(surface_id, cx)
                .filter(|pid| *pid > 0)
        }) else {
            return Observation::Refused;
        };

        let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id) else {
            return Observation::Refused;
        };
        let Some(key) = upsert_session_state(
            &mut ws.agent_sessions,
            Some(key_hint),
            tool,
            ai_types::reduce_lifecycle_event(event),
            None,
            source,
        ) else {
            return Observation::Refused;
        };
        cx.notify();
        self.set_session_surface(ws_id, key, surface_id, cx);
        self.sync_attention(cx);
        self.agent_sessions_changed(cx);
        if matches!(
            self.workspaces
                .iter()
                .find(|ws| ws.id == ws_id)
                .and_then(|ws| ws.agent_sessions.get(&key))
                .map(|session| &session.state),
            Some(ai_types::AgentState::Finished)
        ) {
            let visible = self.surfaces_under_user_eye(ws_id, cx);
            let seen = completion_was_seen(visible.as_ref(), Some(surface_id));
            if let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id) {
                ws.agent_completion_notification
                    .record_finished(seen || ws.muted, Some(surface_id));
            }
            self.schedule_finished_sweep(ws_id, key, cx);
        }
        Observation::Written
    }

    fn schedule_finished_sweep(&mut self, ws_id: u64, key: u32, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(FINISHED_LINGER).await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        if let Some(ws) = app.workspaces.iter_mut().find(|ws| ws.id == ws_id)
                            && matches!(
                                ws.agent_sessions.get(&key).map(|s| &s.state),
                                Some(ai_types::AgentState::Finished)
                            )
                        {
                            ws.agent_sessions.remove(&key);
                            app.sync_attention(cx);
                            app.agent_sessions_changed(cx);
                            cx.notify();
                        }
                    });
                });
            },
        )
        .detach();
    }

    pub(super) fn workspace_id_for_surface(&self, surface_id: u64, cx: &gpui::App) -> Option<u64> {
        self.workspaces
            .iter()
            .find(|ws| {
                ws.collect_panes().iter().any(|pane| {
                    pane.read(cx)
                        .terminals()
                        .any(|terminal| terminal.entity_id().as_u64() == surface_id)
                })
            })
            .map(|ws| ws.id)
    }

    fn surface_child_pid(&self, surface_id: u64, cx: &gpui::App) -> Option<u32> {
        self.workspaces.iter().find_map(|ws| {
            ws.collect_panes().iter().find_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .find(|terminal| terminal.entity_id().as_u64() == surface_id)
                    .map(|terminal| terminal.read(cx).terminal.child_pid)
            })
        })
    }
}

pub(crate) fn opens_a_session(event: &AgentLifecycleEvent) -> bool {
    !matches!(event, AgentLifecycleEvent::Idle)
}

pub(crate) fn progress_lifecycle_event(busy: bool) -> AgentLifecycleEvent {
    if busy {
        AgentLifecycleEvent::Working
    } else {
        AgentLifecycleEvent::Idle
    }
}

pub(crate) fn notification_lifecycle_event(title: &str, body: &str) -> Option<AgentLifecycleEvent> {
    let message = if body.trim().is_empty() {
        title.trim()
    } else {
        body.trim()
    };
    if message.is_empty() {
        return None;
    }
    if is_turn_ended_notification(message) {
        return Some(AgentLifecycleEvent::Idle);
    }
    reads_as_a_request(message).then(|| AgentLifecycleEvent::Notification {
        message: Some(message.to_owned()),
    })
}

fn is_turn_ended_notification(message: &str) -> bool {
    message
        .trim_end_matches('.')
        .eq_ignore_ascii_case("Claude is waiting for your input")
}

fn reads_as_a_request(message: &str) -> bool {
    let lower = message.to_lowercase();
    ["permission", "approv", "input", "confirm"]
        .iter()
        .any(|needle| lower.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_is_only_seen_when_its_own_pane_is_the_one_on_screen() {
        let watched = std::collections::HashSet::from([7u64]);

        assert!(completion_was_seen(Some(&watched), Some(7)));
        assert!(!completion_was_seen(Some(&watched), Some(8)));
        assert!(!completion_was_seen(None, Some(7)));
    }

    #[test]
    fn an_unresolved_surface_falls_back_to_its_workspace() {
        let watched = std::collections::HashSet::from([7u64]);
        assert!(completion_was_seen(Some(&watched), None));
        assert!(completion_was_seen(
            Some(&std::collections::HashSet::new()),
            None
        ));
        assert!(!completion_was_seen(None, None));
    }

    #[test]
    fn only_evidence_of_activity_opens_a_session() {
        assert!(!opens_a_session(&AgentLifecycleEvent::Idle));

        for event in [
            AgentLifecycleEvent::Working,
            AgentLifecycleEvent::PromptSubmit,
            AgentLifecycleEvent::ToolUse { tool_name: None },
            AgentLifecycleEvent::Notification { message: None },
            AgentLifecycleEvent::Stop { summary: None },
            AgentLifecycleEvent::Exit { exit_code: 1 },
        ] {
            assert!(
                opens_a_session(&event),
                "{event:?} must be able to open a row"
            );
        }
    }

    #[test]
    fn progress_maps_to_the_two_states_it_can_prove() {
        assert_eq!(progress_lifecycle_event(true), AgentLifecycleEvent::Working);
        assert_eq!(progress_lifecycle_event(false), AgentLifecycleEvent::Idle);
    }

    #[test]
    fn a_notification_becomes_the_question_the_sidebar_shows() {
        assert_eq!(
            notification_lifecycle_event("Claude Code", "Claude needs your permission"),
            Some(AgentLifecycleEvent::Notification {
                message: Some("Claude needs your permission".into())
            })
        );
        assert_eq!(
            notification_lifecycle_event("Claude Code needs your input", ""),
            Some(AgentLifecycleEvent::Notification {
                message: Some("Claude Code needs your input".into())
            })
        );
    }

    #[test]
    fn the_idle_prompt_is_a_turn_that_ended_not_a_question() {
        for text in [
            "Claude is waiting for your input",
            "Claude is waiting for your input.",
            "claude is waiting for your input",
        ] {
            assert_eq!(
                notification_lifecycle_event(text, ""),
                Some(AgentLifecycleEvent::Idle),
                "{text:?}"
            );
        }
        assert_eq!(
            notification_lifecycle_event("Claude Code", "Claude is waiting for your input"),
            Some(AgentLifecycleEvent::Idle)
        );
    }

    #[test]
    fn an_empty_notification_is_not_an_agent_asking_for_something() {
        assert_eq!(notification_lifecycle_event("", ""), None);
        assert_eq!(notification_lifecycle_event("   ", "\n\t"), None);
    }

    #[test]
    fn a_notification_that_asks_for_nothing_says_nothing() {
        for text in [
            "Build finished",
            "Task completed",
            "Codex turn done",
            "ding",
        ] {
            assert_eq!(notification_lifecycle_event(text, ""), None, "{text:?}");
        }
        assert!(matches!(
            notification_lifecycle_event("Codex", "Approval required to run a command"),
            Some(AgentLifecycleEvent::Notification { .. })
        ));
    }
}
