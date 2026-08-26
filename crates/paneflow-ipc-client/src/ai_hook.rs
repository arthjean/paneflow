//! Canonical wire contract for AI lifecycle notifications.

use std::fmt;

use serde_json::{Map, Value};

pub use crate::MAX_FRAME_BYTES;

pub const METHOD_SESSION_START: &str = "ai.session_start";
pub const METHOD_SESSION_END: &str = "ai.session_end";
pub const METHOD_PROMPT_SUBMIT: &str = "ai.prompt_submit";
pub const METHOD_NOTIFICATION: &str = "ai.notification";
pub const METHOD_STOP: &str = "ai.stop";
pub const METHOD_TOOL_USE: &str = "ai.tool_use";
pub const METHOD_EXIT: &str = "ai.exit";

pub const METHODS: &[&str] = &[
    METHOD_SESSION_START,
    METHOD_PROMPT_SUBMIT,
    METHOD_TOOL_USE,
    METHOD_NOTIFICATION,
    METHOD_STOP,
    METHOD_EXIT,
    METHOD_SESSION_END,
];

pub const DEFAULT_TOOL: &str = "claude";
/// How far a frame may be stamped BEHIND a session's last accepted frame and
/// still be treated as a reordered delivery rather than a wall-clock jump.
///
/// The producer side can only delay a frame by its own retry budget (two
/// connect backoffs plus one write timeout, under a second today), so a
/// second-scale window separates reordering from a clock that moved.
pub const EVENT_REORDER_TOLERANCE_MS: u64 = 5_000;
pub const MAX_TOOL_NAME_BYTES: usize = 64;
pub const MAX_SESSION_PID: u32 = i32::MAX as u32;
pub const EVENT_SOURCE_INTERRUPT: &str = "interrupt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiHookMethod {
    SessionStart,
    SessionEnd,
    PromptSubmit,
    Notification,
    Stop,
    ToolUse,
    Exit,
}

impl AiHookMethod {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => METHOD_SESSION_START,
            Self::SessionEnd => METHOD_SESSION_END,
            Self::PromptSubmit => METHOD_PROMPT_SUBMIT,
            Self::Notification => METHOD_NOTIFICATION,
            Self::Stop => METHOD_STOP,
            Self::ToolUse => METHOD_TOOL_USE,
            Self::Exit => METHOD_EXIT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidToolName;

impl fmt::Display for InvalidToolName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("tool name must be 1-64 ASCII alphanumeric or hyphen bytes")
    }
}

impl std::error::Error for InvalidToolName {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AiToolName(String);

impl AiToolName {
    pub fn parse(raw: &str) -> Result<Self, InvalidToolName> {
        if raw.is_empty()
            || raw.len() > MAX_TOOL_NAME_BYTES
            || !raw
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(InvalidToolName);
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn legacy_default() -> Self {
        Self(DEFAULT_TOOL.to_owned())
    }

    pub fn from_wire_params(params: &Value) -> Result<Self, InvalidToolName> {
        let payload = params.get("hook_payload");
        let raw = if let Some(value) = params.get("tool") {
            value.as_str().ok_or(InvalidToolName)?
        } else if let Some(value) = payload.and_then(|value| value.get("tool")) {
            value.as_str().ok_or(InvalidToolName)?
        } else {
            DEFAULT_TOOL
        };
        Self::parse(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionPid(u32);

impl SessionPid {
    pub fn new(value: u32) -> Option<Self> {
        (value > 0 && value <= MAX_SESSION_PID).then_some(Self(value))
    }

    pub fn from_u64(value: u64) -> Option<Self> {
        u32::try_from(value).ok().and_then(Self::new)
    }

    pub fn from_wire_params(params: &Value) -> Option<Self> {
        params
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(Self::from_u64)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceId(u64);

impl SurfaceId {
    pub fn new(value: u64) -> Option<Self> {
        (value > 0).then_some(Self(value))
    }

    pub fn from_wire_params(params: &Value) -> Option<Self> {
        let payload = params.get("hook_payload");
        params
            .get("surface_id")
            .or_else(|| payload.and_then(|value| value.get("surface_id")))
            .and_then(Value::as_u64)
            .and_then(Self::new)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Wall-clock stamp, in epoch milliseconds, taken by the producing hook
/// process at the moment it builds the frame.
///
/// Delivery order is not causal order: every frame travels through its own
/// short-lived process and its own socket connection, so a frame emitted first
/// can land second. The stamp is what lets the server keep a per-session
/// watermark and drop a frame that describes a state the session already left.
/// It is a source stamp on purpose - an ordinal assigned on arrival would just
/// re-encode the arrival order, which is the problem itself.
pub fn epoch_millis() -> Option<u64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
}

pub fn emitted_at_ms_from_wire_params(params: &Value) -> Option<u64> {
    params.get("emitted_at_ms").and_then(Value::as_u64)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleEventSource {
    Interrupt,
}

impl LifecycleEventSource {
    pub fn parse(raw: &str) -> Option<Self> {
        (raw == EVENT_SOURCE_INTERRUPT).then_some(Self::Interrupt)
    }

    pub fn from_wire_params(params: &Value) -> Option<Self> {
        let payload = params.get("hook_payload");
        params
            .get("event_source")
            .or_else(|| payload.and_then(|value| value.get("event_source")))
            .and_then(Value::as_str)
            .and_then(Self::parse)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Interrupt => EVENT_SOURCE_INTERRUPT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AiHookParams {
    pub workspace_id: u64,
    pub tool: AiToolName,
    pub pid: Option<SessionPid>,
    pub surface_id: Option<SurfaceId>,
    pub tool_name: Option<String>,
    pub exit_code: Option<i32>,
    pub event_source: Option<LifecycleEventSource>,
    /// See [`epoch_millis`]. `None` on frames from a hook older than this
    /// field, which the server accepts unconditionally.
    pub emitted_at_ms: Option<u64>,
    pub hook_payload: Value,
}

impl AiHookParams {
    pub fn new(workspace_id: u64, tool: AiToolName, hook_payload: Value) -> Self {
        Self {
            workspace_id,
            tool,
            pid: None,
            surface_id: None,
            tool_name: None,
            exit_code: None,
            event_source: None,
            emitted_at_ms: None,
            hook_payload,
        }
    }

    pub fn to_value(&self) -> Value {
        let mut value = Map::new();
        value.insert("workspace_id".into(), Value::from(self.workspace_id));
        value.insert("tool".into(), Value::String(self.tool.as_str().to_owned()));
        if let Some(pid) = self.pid {
            value.insert("pid".into(), Value::from(pid.get()));
        }
        if let Some(surface_id) = self.surface_id {
            value.insert("surface_id".into(), Value::from(surface_id.get()));
        }
        if let Some(tool_name) = &self.tool_name {
            value.insert("tool_name".into(), Value::String(tool_name.clone()));
        }
        if let Some(exit_code) = self.exit_code {
            value.insert("exit_code".into(), Value::from(exit_code));
        }
        if let Some(event_source) = self.event_source {
            value.insert(
                "event_source".into(),
                Value::String(event_source.as_str().to_owned()),
            );
        }
        if let Some(emitted_at_ms) = self.emitted_at_ms {
            value.insert("emitted_at_ms".into(), Value::from(emitted_at_ms));
        }
        value.insert("hook_payload".into(), self.hook_payload.clone());
        Value::Object(value)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AiHookFrame {
    pub method: AiHookMethod,
    pub params: AiHookParams,
}

impl AiHookFrame {
    pub fn new(method: AiHookMethod, params: AiHookParams) -> Self {
        Self { method, params }
    }

    pub fn to_value(&self) -> Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": self.method.as_str(),
            "params": self.params.to_value(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_pid_matches_server_range() {
        assert_eq!(SessionPid::new(1).map(SessionPid::get), Some(1));
        assert_eq!(
            SessionPid::new(MAX_SESSION_PID).map(SessionPid::get),
            Some(MAX_SESSION_PID)
        );
        assert!(SessionPid::new(0).is_none());
        assert!(SessionPid::new(MAX_SESSION_PID + 1).is_none());
    }

    #[test]
    fn tool_name_rejects_malformed_wire_values() {
        assert!(AiToolName::parse("codex").is_ok());
        assert!(AiToolName::parse("").is_err());
        assert!(AiToolName::parse("tool/../etc").is_err());
        assert!(AiToolName::parse(&"x".repeat(MAX_TOOL_NAME_BYTES + 1)).is_err());
        assert!(AiToolName::from_wire_params(&json!({"tool": 42})).is_err());
        assert!(AiToolName::from_wire_params(&json!({"hook_payload": {"tool": false}})).is_err());
        assert_eq!(
            AiToolName::from_wire_params(&json!({}))
                .expect("legacy default")
                .as_str(),
            DEFAULT_TOOL
        );
    }

    #[test]
    fn frame_serializes_only_canonical_top_level_fields() {
        let mut params = AiHookParams::new(
            7,
            AiToolName::parse("codex").expect("valid test tool"),
            json!({"message": "Approve?"}),
        );
        params.pid = SessionPid::new(42);
        let frame = AiHookFrame::new(AiHookMethod::Notification, params).to_value();

        assert_eq!(frame["method"], METHOD_NOTIFICATION);
        assert_eq!(frame["params"]["pid"], 42);
        assert!(frame["params"].get("notification_type").is_none());
        assert_eq!(frame["params"]["hook_payload"]["message"], "Approve?");
    }
}
