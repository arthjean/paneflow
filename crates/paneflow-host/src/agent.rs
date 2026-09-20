use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};

use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_ipc_client::agent::{AgentLifecycleEvent, AgentStateSource};
use paneflow_ipc_client::ai_hook::{
    LifecycleEventSource, METHOD_EXIT, METHOD_NOTIFICATION, METHOD_PROMPT_SUBMIT,
    METHOD_SESSION_END, METHOD_SESSION_START, METHOD_STOP, METHOD_TOOL_USE,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::manifest::{HookRecord, SessionLifecycle};

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
            summary: optional_text(
                params,
                &["last_assistant_message", "last_result", "summary", "result"],
            ),
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

    pub fn to_frame(&self, generation: SessionGeneration) -> Value {
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
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshotEntry {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub launch_shell: String,
    pub live: bool,
    pub lifecycle: SessionLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<crate::process::ProcessIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<paneflow_config::schema::WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_hook: Option<HookRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_changed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_started_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_changed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub screen_activity: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub menu_prompt_active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_runtime: Option<String>,
    #[serde(default)]
    pub host_protocol_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_build_id: String,
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
        let frame = interrupted.to_frame(SessionGeneration::FIRST);
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
        assert!(natural.to_frame(SessionGeneration::FIRST)["event_source"].is_null());
    }

    #[test]
    fn a_broadcast_frame_carries_everything_a_controller_reduces_from() {
        let mut exited = event(AgentEventKind::Exit, Some(9), Some(42));
        exited.exit_code = Some(130);
        exited.tool_name = Some("Edit".to_string());
        exited.payload = json!({"summary": "interrupted"});
        let frame = exited.to_frame(SessionGeneration::FIRST);

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
