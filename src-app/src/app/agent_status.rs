use std::collections::HashMap;

use gpui::Context;

use crate::agent_launcher::TerminalAgent;
use crate::ai_types::{self, AgentLifecycleEvent, AgentStateSource};
use crate::claude_session_registry::{self, ClaudeSessionRecord, ClaudeSessionStatus};

use crate::PaneFlowApp;
use crate::app::ipc_handler::upsert_session_state;

const FINISHED_LINGER: std::time::Duration = std::time::Duration::from_secs(5);

pub(crate) const REGISTRY_POLL_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(400);

pub(crate) type RegistryWatermark = HashMap<u32, (ClaudeSessionStatus, Option<String>)>;

struct PendingRecord {
    record: ClaudeSessionRecord,
    surface_id: Option<u64>,
}

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
        if self.settings_section.is_some()
            || !matches!(self.mode, paneflow_config::schema::AppMode::Cli)
            || !crate::agents::notifications::window_active()
        {
            return None;
        }
        self.workspaces
            .get(self.active_idx)
            .filter(|ws| ws.id == workspace_id)
            .map(|ws| ws.active_tab().surface_ids(cx))
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
                    .record_finished(seen, Some(surface_id));
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

    fn registry_surface_candidates(&self, cx: &gpui::App) -> Option<HashMap<u32, u64>> {
        let mut candidates = HashMap::new();
        let mut backed = false;
        for ws in &self.workspaces {
            for pane in ws.collect_panes() {
                for terminal in pane.read(cx).terminals() {
                    let view = terminal.read(cx);
                    backed |= matches!(
                        view.terminal.detected_agent,
                        Some(TerminalAgent::ClaudeCode)
                    );
                    if view.terminal.child_pid > 0 {
                        candidates.insert(view.terminal.child_pid, terminal.entity_id().as_u64());
                    }
                }
            }
        }
        backed.then_some(candidates)
    }

    pub(crate) fn sweep_claude_session_registry(&mut self, cx: &mut Context<Self>) {
        let Some(candidates) = self.registry_surface_candidates(cx) else {
            self.claude_registry_seen.clear();
            return;
        };
        let Some(dir) = claude_session_registry::sessions_dir() else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let records = smol::unblock(move || {
                    let records = claude_session_registry::read_live_sessions(&dir);
                    records
                        .into_iter()
                        .map(|record| {
                            let surface_id = candidates.get(&record.pid).copied().or_else(|| {
                                crate::workspace::pid_resolve::resolve_surface_for_pid(
                                    record.pid,
                                    &candidates,
                                )
                            });
                            PendingRecord { record, surface_id }
                        })
                        .collect::<Vec<_>>()
                })
                .await;
                cx.update(|cx| {
                    let _ = this.update(cx, |app, cx| {
                        app.apply_registry_records(records, cx);
                    });
                });
            },
        )
        .detach();
    }

    fn apply_registry_records(&mut self, records: Vec<PendingRecord>, cx: &mut Context<Self>) {
        let live: std::collections::HashSet<u32> =
            records.iter().map(|pending| pending.record.pid).collect();
        self.claude_registry_seen
            .retain(|pid, _| live.contains(pid));

        for pending in records {
            let Some(surface_id) = pending.surface_id else {
                continue;
            };
            let record = pending.record;
            let observation = (record.status, record.waiting_for.clone());
            if self.claude_registry_seen.get(&record.pid) == Some(&observation) {
                continue;
            }
            let applied = self.apply_observed_agent_state(
                surface_id,
                TerminalAgent::ClaudeCode,
                Some(record.pid),
                record.lifecycle_event(),
                AgentStateSource::SessionRegistry,
                cx,
            );
            if applied != Observation::Refused {
                self.claude_registry_seen.insert(record.pid, observation);
            }
        }
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
