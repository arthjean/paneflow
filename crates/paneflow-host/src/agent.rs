use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_ipc_client::agent::{
    AgentLifecycleEvent, AgentState, AgentStateSource, FieldUpdate, SOURCE_TAKEOVER_SILENCE,
    accepts_event, accepts_source, next_waiting_since, reduce_lifecycle_event,
};
use paneflow_ipc_client::ai_hook::{
    LifecycleEventSource, METHOD_EXIT, METHOD_NOTIFICATION, METHOD_PROMPT_SUBMIT,
    METHOD_SESSION_END, METHOD_SESSION_START, METHOD_STOP, METHOD_TOOL_USE,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manifest::{AgentSummary, SessionLifecycle};

pub const MAX_AGENT_TEXT_BYTES: usize = 4 * 1024;

pub const SUBSCRIBER_QUEUE_SLOTS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentEventKind {
    SessionStart,
    PromptSubmit,
    ToolUse,
    Notification,
    Stop,
    Exit,
    SessionEnd,
}

impl AgentEventKind {
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::SessionStart => METHOD_SESSION_START,
            Self::PromptSubmit => METHOD_PROMPT_SUBMIT,
            Self::ToolUse => METHOD_TOOL_USE,
            Self::Notification => METHOD_NOTIFICATION,
            Self::Stop => METHOD_STOP,
            Self::Exit => METHOD_EXIT,
            Self::SessionEnd => METHOD_SESSION_END,
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            METHOD_SESSION_START => Some(Self::SessionStart),
            METHOD_PROMPT_SUBMIT => Some(Self::PromptSubmit),
            METHOD_TOOL_USE => Some(Self::ToolUse),
            METHOD_NOTIFICATION => Some(Self::Notification),
            METHOD_STOP => Some(Self::Stop),
            METHOD_EXIT => Some(Self::Exit),
            METHOD_SESSION_END => Some(Self::SessionEnd),
            _ => None,
        }
    }

    pub fn ends_run(self) -> bool {
        matches!(self, Self::Stop | Self::Exit | Self::SessionEnd)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentEvent {
    pub session: SessionId,
    pub generation: Option<SessionGeneration>,
    pub kind: AgentEventKind,
    pub tool: String,
    pub pid: Option<u32>,
    pub tool_name: Option<String>,
    pub message: Option<String>,
    pub summary: Option<String>,
    pub exit_code: Option<i32>,
    pub emitted_at_ms: Option<u64>,
    pub received_at_ms: Option<u64>,
    pub source: AgentStateSource,
    pub event_source: Option<LifecycleEventSource>,
    pub payload: Value,
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

fn optional_text(params: &Value, keys: &[&str]) -> Option<String> {
    let payload = params.get("hook_payload");
    keys.iter().find_map(|key| {
        params
            .get(*key)
            .or_else(|| payload.and_then(|payload| payload.get(*key)))
            .and_then(Value::as_str)
            .map(clamp_text)
            .filter(|text| !text.trim().is_empty())
    })
}

impl AgentEvent {
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let raw_session = params
            .get("session")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing session".to_string())?;
        let session = SessionId::parse(raw_session).map_err(|error| error.to_string())?;
        let generation = match params
            .get("runtime_generation")
            .or_else(|| params.get("generation"))
        {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                serde_json::from_value(value.clone())
                    .map_err(|error| format!("invalid generation: {error}"))?,
            ),
        };
        let raw_kind = params
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing kind".to_string())?;
        let kind = AgentEventKind::parse(raw_kind)
            .ok_or_else(|| format!("unsupported agent event kind: {raw_kind}"))?;
        let tool = params
            .get("tool")
            .and_then(Value::as_str)
            .map(clamp_text)
            .filter(|tool| !tool.is_empty())
            .ok_or_else(|| "missing tool".to_string())?;
        let source = params
            .get("source")
            .and_then(Value::as_str)
            .map_or(Some(AgentStateSource::Hook), AgentStateSource::parse)
            .ok_or_else(|| "unsupported agent state source".to_string())?;
        let pid = params
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok())
            .filter(|pid| *pid > 0);
        let payload = params.get("hook_payload");
        let exit_code = params
            .get("exit_code")
            .or_else(|| payload.and_then(|payload| payload.get("exit_code")))
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok());
        if kind == AgentEventKind::Exit && exit_code.is_none() {
            return Err("an exit event needs an exit_code".to_string());
        }
        Ok(Self {
            session,
            generation,
            kind,
            tool,
            pid,
            tool_name: optional_text(params, &["tool_name"]),
            message: optional_text(params, &["message"]),
            summary: optional_text(params, &["last_result", "summary", "result"]),
            exit_code,
            emitted_at_ms: params.get("emitted_at_ms").and_then(Value::as_u64),
            received_at_ms: None,
            source,
            event_source: LifecycleEventSource::from_wire_params(params),
            payload: params
                .get("hook_payload")
                .cloned()
                .unwrap_or_else(|| json!({})),
        })
    }

    pub fn is_interrupt(&self) -> bool {
        self.event_source == Some(LifecycleEventSource::Interrupt)
    }

    pub fn lifecycle(&self) -> Option<AgentLifecycleEvent> {
        match self.kind {
            AgentEventKind::SessionStart | AgentEventKind::SessionEnd => None,
            AgentEventKind::PromptSubmit => Some(AgentLifecycleEvent::PromptSubmit),
            AgentEventKind::ToolUse => Some(AgentLifecycleEvent::ToolUse {
                tool_name: self.tool_name.clone(),
            }),
            AgentEventKind::Notification => Some(AgentLifecycleEvent::Notification {
                message: self.message.clone(),
            }),
            AgentEventKind::Stop => Some(AgentLifecycleEvent::Stop {
                summary: if self.is_interrupt() {
                    None
                } else {
                    self.summary.clone()
                },
            }),
            AgentEventKind::Exit => Some(AgentLifecycleEvent::Exit {
                exit_code: self.exit_code.unwrap_or(0),
            }),
        }
    }

    pub fn to_frame(&self, generation: SessionGeneration, state: Option<&AgentSummary>) -> Value {
        json!({
            "type": "event",
            "session": self.session,
            "generation": generation,
            "kind": self.kind.wire_str(),
            "tool": self.tool,
            "pid": self.pid,
            "tool_name": self.tool_name,
            "exit_code": self.exit_code,
            "emitted_at_ms": self.emitted_at_ms,
            "received_at_ms": self.received_at_ms,
            "source": self.source.wire_str(),
            "event_source": self.event_source.map(LifecycleEventSource::as_str),
            "hook_payload": self.payload,
            "agent": state,
        })
    }
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
            Some(summary) if summary.state == AgentState::Errored.wire_str() => {
                AgentDecision::Stale("an errored run keeps its outcome")
            }
            _ => AgentDecision::Clear,
        };
    }

    if let Some(summary) = existing {
        let silence = silence_since(summary.updated_at_ms, now_ms);
        if !accepts_event(summary.last_event_at_ms, event.emitted_at_ms) {
            return AgentDecision::Stale("an out-of-order event never rewrites a newer run");
        }
        let held = AgentStateSource::parse(&summary.source).unwrap_or(AgentStateSource::Hook);
        if !accepts_source(Some((held, silence)), event.source) {
            return AgentDecision::Stale("a weaker source never talks over a live stronger one");
        }
        if !accepts_process(summary, event, silence) {
            return AgentDecision::Stale("a stale process identity never ends a newer run");
        }
    }

    let Some(lifecycle) = event.lifecycle() else {
        let mut summary = existing.cloned().unwrap_or_else(|| AgentSummary {
            tool: event.tool.clone(),
            state: AgentState::Finished.wire_str().to_string(),
            source: event.source.wire_str().to_string(),
            active_tool_name: None,
            message: None,
            last_result: None,
            provider_session_id: None,
            transcript_path: None,
            pid: event.pid,
            waiting_since_ms: None,
            last_event_at_ms: event.emitted_at_ms,
            stale: false,
            updated_at_ms: now_ms,
        });
        summary.tool = event.tool.clone();
        summary.pid = event.pid.or(summary.pid);
        summary.stale = false;
        summary.last_event_at_ms = event.emitted_at_ms.or(summary.last_event_at_ms);
        summary.provider_session_id =
            optional_payload_text(event, "session_id").or(summary.provider_session_id);
        summary.transcript_path =
            optional_payload_text(event, "transcript_path").or(summary.transcript_path);
        summary.updated_at_ms = now_ms;
        return AgentDecision::Update(Box::new(summary));
    };

    let transition = reduce_lifecycle_event(lifecycle);
    let previous_state = existing.and_then(|summary| AgentState::parse(&summary.state));
    let waiting_since_ms = next_waiting_since(
        previous_state
            .as_ref()
            .map(|state| (state, existing.and_then(|summary| summary.waiting_since_ms))),
        &transition.state,
        now_ms,
    );
    let message = match transition.message {
        FieldUpdate::Keep => existing.and_then(|summary| summary.message.clone()),
        FieldUpdate::Set(message) => message,
    };
    let last_result = match transition.last_result {
        FieldUpdate::Keep => existing.and_then(|summary| summary.last_result.clone()),
        FieldUpdate::Set(result) => result,
    };
    AgentDecision::Update(Box::new(AgentSummary {
        tool: event.tool.clone(),
        state: transition.state.wire_str().to_string(),
        source: event.source.wire_str().to_string(),
        active_tool_name: transition.active_tool_name,
        message,
        last_result,
        provider_session_id: optional_payload_text(event, "session_id")
            .or_else(|| existing.and_then(|summary| summary.provider_session_id.clone())),
        transcript_path: optional_payload_text(event, "transcript_path")
            .or_else(|| existing.and_then(|summary| summary.transcript_path.clone())),
        pid: event
            .pid
            .or_else(|| existing.and_then(|summary| summary.pid)),
        waiting_since_ms,
        last_event_at_ms: event
            .emitted_at_ms
            .or_else(|| existing.and_then(|summary| summary.last_event_at_ms)),
        stale: false,
        updated_at_ms: now_ms,
    }))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshotEntry {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub live: bool,
    pub lifecycle: SessionLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<paneflow_config::schema::WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSummary>,
}

pub struct AgentSubscription {
    pub id: u64,
    pub frames: Receiver<Value>,
}

#[derive(Default)]
pub struct AgentBus {
    next_id: AtomicU64,
    subscribers: Mutex<Vec<(u64, SyncSender<Value>)>>,
}

impl AgentBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> AgentSubscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, frames) = sync_channel(SUBSCRIBER_QUEUE_SLOTS);
        self.lock().push((id, tx));
        AgentSubscription { id, frames }
    }

    pub fn unsubscribe(&self, id: u64) {
        self.lock().retain(|(held, _)| *held != id);
    }

    pub fn subscriber_count(&self) -> usize {
        self.lock().len()
    }

    pub fn broadcast(&self, frame: &Value) {
        let mut subscribers = self.lock();
        subscribers.retain(|(id, tx)| match tx.try_send(frame.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                log::warn!("paneflow-host: agent subscriber {id} fell behind and was dropped");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, SyncSender<Value>)>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_prompt_then_a_notification_carries_the_question_and_stamps_waiting() {
        let prompt = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, Some(10), Some(42)),
            1_000,
        ));
        assert_eq!(prompt.state, "thinking");
        assert_eq!(prompt.pid, Some(42));
        assert_eq!(prompt.waiting_since_ms, None);

        let mut asking = event(AgentEventKind::Notification, Some(20), Some(42));
        asking.message = Some("Approve edit?".to_string());
        let waiting = updated(apply_event(Some(&prompt), &asking, 2_000));
        assert_eq!(waiting.state, "waiting_for_input");
        assert_eq!(waiting.message.as_deref(), Some("Approve edit?"));
        assert_eq!(waiting.waiting_since_ms, Some(2_000));

        let renotified = updated(apply_event(Some(&waiting), &asking, 3_000));
        assert_eq!(
            renotified.waiting_since_ms,
            Some(2_000),
            "a second question does not restart the wait"
        );
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
        assert!(
            matches!(late, AgentDecision::Stale(_)),
            "a frame that arrives out of order inside the reorder window is refused"
        );

        let fresh = apply_event(
            Some(&current),
            &event(AgentEventKind::Stop, Some(100_001), Some(42)),
            1_200,
        );
        assert_eq!(updated(fresh).state, "finished");

        let clock_jump = apply_event(
            Some(&current),
            &event(AgentEventKind::Stop, Some(1_000), Some(42)),
            1_300,
        );
        assert_eq!(
            updated(clock_jump).state,
            "finished",
            "a wall-clock jump is not reordering and still reduces"
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
        assert_eq!(
            updated(foreign_start).pid,
            Some(77),
            "a new run in the same session still takes over"
        );

        let long_after = apply_event(
            Some(&current),
            &event(AgentEventKind::Exit, None, Some(77)),
            1_000 + SOURCE_TAKEOVER_SILENCE.as_millis() as u64,
        );
        assert_eq!(updated(long_after).state, "finished");
    }

    #[test]
    fn a_weaker_source_never_talks_over_a_live_hook() {
        let hooked = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, None, Some(42)),
            1_000,
        ));
        let mut observed = event(AgentEventKind::Stop, None, Some(42));
        observed.source = AgentStateSource::Terminal;
        assert!(matches!(
            apply_event(Some(&hooked), &observed, 1_500),
            AgentDecision::Stale(_)
        ));
    }

    #[test]
    fn session_end_clears_unless_the_run_errored_or_a_foreign_process_asks() {
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
        assert_eq!(errored.state, "errored");
        assert!(matches!(
            apply_event(
                Some(&errored),
                &event(AgentEventKind::SessionEnd, None, Some(42)),
                1_300
            ),
            AgentDecision::Stale(_)
        ));
    }

    #[test]
    fn a_declaration_records_the_tool_without_inventing_a_turn() {
        let declared = updated(apply_event(
            None,
            &event(AgentEventKind::SessionStart, Some(5), Some(42)),
            1_000,
        ));
        assert_eq!(declared.tool, "claude");
        assert_eq!(declared.state, "finished");

        let working = updated(apply_event(
            Some(&declared),
            &event(AgentEventKind::PromptSubmit, Some(6), Some(42)),
            1_100,
        ));
        let redeclared = updated(apply_event(
            Some(&working),
            &event(AgentEventKind::SessionStart, Some(7), Some(42)),
            1_200,
        ));
        assert_eq!(
            redeclared.state, "thinking",
            "a declaration never resets a running turn"
        );
    }

    #[test]
    fn a_host_restart_marks_a_busy_record_stale_instead_of_idle() {
        let mut busy = updated(apply_event(
            None,
            &event(AgentEventKind::PromptSubmit, None, Some(42)),
            1_000,
        ));
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
        reconcile_adopted(&mut finished, &SessionLifecycle::Lost, 3_000);
        assert!(!finished.stale, "a finished run is already an outcome");
    }

    #[test]
    fn wire_params_are_validated_before_any_state_change() {
        let session = SessionId::new();
        let params = json!({
            "session": session.to_string(),
            "kind": "ai.tool_use",
            "tool": "codex",
            "pid": 42,
            "tool_name": "Edit",
            "emitted_at_ms": 7,
            "hook_payload": {"tool_name": "Edit"},
        });
        let event = AgentEvent::from_params(&params).expect("valid event");
        assert_eq!(event.session, session);
        assert_eq!(event.kind, AgentEventKind::ToolUse);
        assert_eq!(event.tool_name.as_deref(), Some("Edit"));
        assert_eq!(event.source, AgentStateSource::Hook);

        let stopped = AgentEvent::from_params(&json!({
            "session": session.to_string(),
            "kind": "ai.stop",
            "tool": "claude",
            "hook_payload": {"summary": "3 files changed"},
        }))
        .expect("valid stop");
        assert_eq!(stopped.summary.as_deref(), Some("3 files changed"));

        let exited = AgentEvent::from_params(&json!({
            "session": session.to_string(),
            "kind": "ai.exit",
            "tool": "claude",
            "hook_payload": {"exit_code": 130},
        }))
        .expect("valid exit");
        assert_eq!(exited.exit_code, Some(130));

        assert!(AgentEvent::from_params(&json!({"kind": "ai.stop", "tool": "c"})).is_err());
        assert!(
            AgentEvent::from_params(
                &json!({"session": session.to_string(), "kind": "ai.nope", "tool": "c"})
            )
            .is_err()
        );
        assert!(
            AgentEvent::from_params(
                &json!({"session": session.to_string(), "kind": "ai.exit", "tool": "c"})
            )
            .is_err(),
            "an exit without a code is not an outcome"
        );
    }

    #[test]
    fn an_interrupted_stop_keeps_its_marker_and_drops_the_summary() {
        let session = SessionId::new();
        let interrupted = AgentEvent::from_params(&json!({
            "session": session.to_string(),
            "kind": "ai.stop",
            "tool": "claude",
            "event_source": "interrupt",
            "hook_payload": {"last_result": "partial answer"},
        }))
        .expect("valid stop");
        assert!(interrupted.is_interrupt());
        assert_eq!(
            interrupted.lifecycle(),
            Some(AgentLifecycleEvent::Stop { summary: None }),
            "a Ctrl+C never records a completion summary"
        );
        let frame = interrupted.to_frame(SessionGeneration::FIRST, None);
        assert_eq!(
            frame["event_source"], "interrupt",
            "the controller must see the interrupt to skip its completion flow"
        );

        let natural = AgentEvent::from_params(&json!({
            "session": session.to_string(),
            "kind": "ai.stop",
            "tool": "claude",
            "hook_payload": {"last_result": "done", "event_source": "natural"},
        }))
        .expect("valid stop");
        assert!(!natural.is_interrupt());
        assert_eq!(
            natural.lifecycle(),
            Some(AgentLifecycleEvent::Stop {
                summary: Some("done".to_string())
            })
        );
        assert!(natural.to_frame(SessionGeneration::FIRST, None)["event_source"].is_null());
    }

    #[test]
    fn a_broadcast_frame_carries_everything_a_controller_reduces_from() {
        let mut exited = event(AgentEventKind::Exit, Some(9), Some(42));
        exited.exit_code = Some(130);
        exited.tool_name = Some("Edit".to_string());
        exited.payload = json!({"summary": "interrupted"});
        let frame = exited.to_frame(SessionGeneration::FIRST, None);

        assert_eq!(frame["type"], "event");
        assert_eq!(frame["kind"], "ai.exit");
        assert_eq!(frame["exit_code"], 130);
        assert_eq!(frame["tool_name"], "Edit");
        assert_eq!(frame["pid"], 42);
        assert_eq!(frame["emitted_at_ms"], 9);
        assert_eq!(frame["hook_payload"]["summary"], "interrupted");
        assert!(frame["agent"].is_null());
    }

    #[test]
    fn a_subscriber_that_falls_behind_is_dropped_not_blocked() {
        let bus = AgentBus::new();
        let subscription = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);
        for _ in 0..SUBSCRIBER_QUEUE_SLOTS + 1 {
            bus.broadcast(&json!({"type": "event"}));
        }
        assert_eq!(bus.subscriber_count(), 0);
        drop(subscription);

        let subscription = bus.subscribe();
        bus.broadcast(&json!({"type": "event"}));
        assert_eq!(
            subscription.frames.try_recv().map(|v| v["type"].clone()),
            Ok(json!("event"))
        );
        bus.unsubscribe(subscription.id);
        assert_eq!(bus.subscriber_count(), 0);
    }
}
