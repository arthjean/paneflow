use std::time::Duration;

use paneflow_host::agent::{AgentEvent, AgentEventKind, MAX_AGENT_TEXT_BYTES};
use paneflow_host::manifest::SessionLifecycle;
use paneflow_ipc_client::agent::{
    AgentState, SOURCE_TAKEOVER_SILENCE, accepts_event, state_for_exit,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    pub tool: String,
    pub state: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_since_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub errored: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    pub updated_at_ms: u64,
}

impl AgentSummary {
    pub fn declared(tool: &str, now_ms: u64) -> Self {
        Self {
            tool: tool.to_owned(),
            state: AgentState::Finished.wire_str().to_string(),
            source: "hook".to_string(),
            active_tool_name: None,
            message: None,
            last_result: None,
            provider_session_id: None,
            transcript_path: None,
            pid: None,
            waiting_since_ms: None,
            last_event_at_ms: None,
            errored: false,
            stale: false,
            updated_at_ms: now_ms,
        }
    }
}

fn clamp_text(raw: &str) -> String {
    let mut text: String = raw
        .chars()
        .filter(|character| *character != '\0')
        .take(MAX_AGENT_TEXT_BYTES)
        .collect();
    text.truncate(MAX_AGENT_TEXT_BYTES);
    text
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentDecision {
    Update(Box<AgentSummary>),
    Clear,
    Stale(&'static str),
}

fn silence_since(updated_at_ms: u64, now_ms: u64) -> Duration {
    Duration::from_millis(now_ms.saturating_sub(updated_at_ms))
}

fn accepts_process(existing: &AgentSummary, event: &AgentEvent, silence: Duration) -> bool {
    match (existing.pid, event.pid) {
        (Some(held), Some(incoming)) if held != incoming => {
            !event.kind.ends_run() || silence >= SOURCE_TAKEOVER_SILENCE
        }
        _ => true,
    }
}

pub fn apply_event(
    existing: Option<&AgentSummary>,
    event: &AgentEvent,
    now_ms: u64,
) -> AgentDecision {
    if let Some(summary) = existing
        && !accepts_event(summary.last_event_at_ms, event.emitted_at_ms)
    {
        return AgentDecision::Stale("an out-of-order event never rewrites a newer run");
    }
    if event.kind == AgentEventKind::SessionEnd {
        return match existing {
            Some(summary)
                if !accepts_process(
                    summary,
                    event,
                    silence_since(summary.updated_at_ms, now_ms),
                ) =>
            {
                AgentDecision::Stale("a foreign process cannot end this run")
            }
            Some(summary) if summary.errored => {
                AgentDecision::Stale("an errored run keeps its outcome")
            }
            _ => AgentDecision::Clear,
        };
    }

    if let Some(summary) = existing {
        let silence = silence_since(summary.updated_at_ms, now_ms);
        if !accepts_process(summary, event, silence) {
            return AgentDecision::Stale("a stale process identity never ends a newer run");
        }
    }

    let mut summary = existing
        .cloned()
        .unwrap_or_else(|| AgentSummary::declared(&event.tool, now_ms));
    summary.tool = event.tool.clone();
    summary.pid = event.pid.or(summary.pid);
    summary.stale = false;
    summary.last_event_at_ms = event.emitted_at_ms.or(summary.last_event_at_ms);
    summary.provider_session_id =
        optional_payload_text(event, "session_id").or(summary.provider_session_id);
    summary.transcript_path =
        optional_payload_text(event, "transcript_path").or(summary.transcript_path);
    summary.updated_at_ms = now_ms;

    match event.kind {
        AgentEventKind::PromptSubmit => {
            summary.active_tool_name = None;
            summary.message = None;
            summary.errored = false;
        }
        AgentEventKind::ToolUse => {
            summary.active_tool_name = event.tool_name.clone();
        }
        AgentEventKind::Notification => {
            summary.active_tool_name = None;
            summary.message = event.message.clone();
        }
        AgentEventKind::Stop => {
            summary.active_tool_name = None;
            summary.message = None;
            if !event.is_interrupt() {
                summary.last_result = event.summary.clone();
            }
        }
        AgentEventKind::Exit => {
            summary.active_tool_name = None;
            summary.message = None;
            summary.errored = state_for_exit(event.exit_code.unwrap_or(0)) == AgentState::Errored;
        }
        AgentEventKind::SessionStart | AgentEventKind::SessionEnd => {}
    }

    AgentDecision::Update(Box::new(summary))
}

fn optional_payload_text(event: &AgentEvent, key: &str) -> Option<String> {
    event
        .payload
        .get(key)
        .and_then(Value::as_str)
        .map(clamp_text)
        .filter(|value| !value.is_empty())
}

pub fn reconcile_adopted(summary: &mut AgentSummary, lifecycle: &SessionLifecycle, now_ms: u64) {
    let busy = AgentState::parse(&summary.state).is_some_and(|state| state.is_busy());
    if lifecycle.is_running() || !busy {
        return;
    }
    summary.stale = true;
    summary.updated_at_ms = now_ms;
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::SessionId;
    use paneflow_ipc_client::agent::AgentStateSource;
    use serde_json::json;

    fn event(kind: AgentEventKind, emitted_at_ms: Option<u64>, pid: Option<u32>) -> AgentEvent {
        AgentEvent {
            session: SessionId::new(),
            generation: None,
            kind,
            tool: "claude".to_string(),
            pid,
            tool_name: None,
            message: None,
            summary: None,
            exit_code: (kind == AgentEventKind::Exit).then_some(0),
            emitted_at_ms,
            received_at_ms: None,
            source: AgentStateSource::Hook,
            event_source: None,
            payload: json!({}),
        }
    }

    fn updated(decision: AgentDecision) -> AgentSummary {
        match decision {
            AgentDecision::Update(summary) => *summary,
            other => panic!("expected an update, got {other:?}"),
        }
    }

    #[test]
    fn a_prompt_then_a_notification_carries_the_question_and_the_tool_identity() {
        let prompt = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, Some(10), Some(42)),
            1_000,
        ));
        assert_eq!(prompt.tool, "claude");
        assert_eq!(prompt.pid, Some(42));
        assert_eq!(prompt.message, None);

        let mut asking = event(AgentEventKind::Notification, Some(20), Some(42));
        asking.message = Some("Approve edit?".to_string());
        let waiting = updated(apply_event(Some(&prompt), &asking, 2_000));
        assert_eq!(waiting.message.as_deref(), Some("Approve edit?"));
    }

    #[test]
    fn an_out_of_order_frame_never_rewrites_a_newer_run() {
        let current = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, Some(100_000), Some(42)),
            1_000,
        ));
        let late = apply_event(
            Some(&current),
            &event(AgentEventKind::Stop, Some(99_000), Some(42)),
            1_100,
        );
        assert!(matches!(late, AgentDecision::Stale(_)));

        let fresh = apply_event(
            Some(&current),
            &event(AgentEventKind::Stop, Some(100_001), Some(42)),
            1_200,
        );
        assert_eq!(updated(fresh).last_event_at_ms, Some(100_001));

        let clock_jump = apply_event(
            Some(&current),
            &event(AgentEventKind::Stop, Some(1_000), Some(42)),
            1_300,
        );
        assert!(
            matches!(clock_jump, AgentDecision::Update(_)),
            "a wall-clock jump is not reordering and still records"
        );
    }

    #[test]
    fn a_stale_process_cannot_finish_a_newer_run() {
        let current = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, None, Some(42)),
            1_000,
        ));
        let foreign = apply_event(
            Some(&current),
            &event(AgentEventKind::Exit, None, Some(77)),
            1_500,
        );
        assert!(matches!(foreign, AgentDecision::Stale(_)));

        let foreign_start = apply_event(
            Some(&current),
            &event(AgentEventKind::PromptSubmit, None, Some(77)),
            1_500,
        );
        assert_eq!(updated(foreign_start).pid, Some(77));

        let long_after = apply_event(
            Some(&current),
            &event(AgentEventKind::Exit, None, Some(77)),
            1_000 + SOURCE_TAKEOVER_SILENCE.as_millis() as u64,
        );
        assert!(matches!(long_after, AgentDecision::Update(_)));
    }

    #[test]
    fn a_crash_records_the_error_and_session_end_keeps_that_outcome() {
        let running = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, None, Some(42)),
            1_000,
        ));
        assert_eq!(
            apply_event(
                Some(&running),
                &event(AgentEventKind::SessionEnd, None, Some(42)),
                1_100
            ),
            AgentDecision::Clear
        );
        assert!(matches!(
            apply_event(
                Some(&running),
                &event(AgentEventKind::SessionEnd, None, Some(77)),
                1_100
            ),
            AgentDecision::Stale(_)
        ));

        let mut crashed = event(AgentEventKind::Exit, None, Some(42));
        crashed.exit_code = Some(1);
        let errored = updated(apply_event(Some(&running), &crashed, 1_200));
        assert!(errored.errored);
        assert!(matches!(
            apply_event(
                Some(&errored),
                &event(AgentEventKind::SessionEnd, None, Some(42)),
                1_300
            ),
            AgentDecision::Stale(_)
        ));

        let mut interrupted = event(AgentEventKind::Exit, None, Some(42));
        interrupted.exit_code = Some(130);
        assert!(
            !updated(apply_event(Some(&running), &interrupted, 1_400)).errored,
            "a human interrupt is not a crash"
        );
    }

    #[test]
    fn an_older_session_end_from_the_same_process_cannot_clear_a_newer_submission() {
        let running = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, Some(2_000), Some(42)),
            2_000,
        ));
        assert!(matches!(
            apply_event(
                Some(&running),
                &event(AgentEventKind::SessionEnd, Some(1_999), Some(42)),
                2_100,
            ),
            AgentDecision::Stale(_)
        ));
    }

    #[test]
    fn a_host_restart_marks_a_busy_record_stale_instead_of_idle() {
        let mut busy = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, None, Some(42)),
            1_000,
        ));
        busy.state = AgentState::Thinking.wire_str().to_string();
        reconcile_adopted(&mut busy, &SessionLifecycle::Running, 2_000);
        assert!(!busy.stale, "a session the host still owns stays live");

        reconcile_adopted(&mut busy, &SessionLifecycle::Lost, 3_000);
        assert!(busy.stale);
        assert_eq!(busy.state, "thinking", "the last known state is not faked");

        let mut finished = updated(apply_event(
            None,
            &event(AgentEventKind::Stop, None, Some(42)),
            1_000,
        ));
        finished.state = AgentState::Finished.wire_str().to_string();
        reconcile_adopted(&mut finished, &SessionLifecycle::Lost, 3_000);
        assert!(!finished.stale, "a finished run is already an outcome");
    }
}
