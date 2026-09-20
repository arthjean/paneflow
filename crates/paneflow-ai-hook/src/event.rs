use std::fmt;
use std::str::FromStr;

use paneflow_ipc_client::ai_hook::{
    AiHookFrame, AiHookMethod, AiHookParams, AiToolName, LifecycleEventSource, SessionPid,
    SurfaceId,
};
use serde_json::Value;

pub(crate) const MAX_HOOK_TEXT_BYTES: usize = 4096;

pub(crate) const MAX_HOOK_PROMPT_BYTES: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookEvent {
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    Notification,
    Stop,
    StopFailure,
    Interrupt,
    SubagentStart,
    SubagentStop,
    PreToolUse,
    PostToolUse,
    PermissionRequest,
    Exit,
}

impl HookEvent {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::Notification => "Notification",
            Self::Stop => "Stop",
            Self::StopFailure => "StopFailure",
            Self::Interrupt => "Interrupt",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PermissionRequest => "PermissionRequest",
            Self::Exit => "Exit",
        }
    }

    pub(crate) const fn input_source(self) -> InputSource {
        match self {
            Self::Exit => InputSource::ExitCodeEnvironment,
            _ => InputSource::Stdin,
        }
    }

    const fn carries_interrupt_source(self) -> bool {
        matches!(
            self,
            Self::Stop | Self::StopFailure | Self::Interrupt | Self::Exit | Self::SessionEnd
        )
    }
}

impl FromStr for HookEvent {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match raw {
            "SessionStart" => Ok(Self::SessionStart),
            "SessionEnd" => Ok(Self::SessionEnd),
            "UserPromptSubmit" => Ok(Self::UserPromptSubmit),
            "Notification" => Ok(Self::Notification),
            "Stop" => Ok(Self::Stop),
            "StopFailure" => Ok(Self::StopFailure),
            "Interrupt" => Ok(Self::Interrupt),
            "SubagentStart" => Ok(Self::SubagentStart),
            "SubagentStop" => Ok(Self::SubagentStop),
            "PreToolUse" => Ok(Self::PreToolUse),
            "PostToolUse" => Ok(Self::PostToolUse),
            "PermissionRequest" => Ok(Self::PermissionRequest),
            "Exit" => Ok(Self::Exit),
            _ => Err(()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputSource {
    Stdin,
    ExitCodeEnvironment,
}

pub(crate) struct FrameContext {
    pub(crate) workspace_id: u64,
    pub(crate) tool: AiToolName,
    pub(crate) pid: Option<SessionPid>,
    pub(crate) surface_id: Option<SurfaceId>,
    pub(crate) event_source: Option<LifecycleEventSource>,
    pub(crate) runtime_generation: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DropReason {
    InformationalNotification(Option<String>),
    LlmCallContinuesWithToolCalls(u64),
}

impl fmt::Display for DropReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InformationalNotification(kind) => {
                write!(formatter, "dropping notification_type={kind:?}")
            }
            Self::LlmCallContinuesWithToolCalls(count) => {
                write!(
                    formatter,
                    "dropping PostLLMCall with tool_call_count={count}"
                )
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BuildError {
    MissingOrInvalidExitCode,
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingOrInvalidExitCode => formatter.write_str("missing or invalid exit_code"),
        }
    }
}

#[derive(Debug)]
pub(crate) enum BuildOutcome {
    Send(AiHookFrame),
    Drop(DropReason),
}

pub(crate) fn build_frame(
    event: HookEvent,
    context: FrameContext,
    hook_payload: Value,
) -> Result<BuildOutcome, BuildError> {
    let ask_user_question = is_ask_user_question(event, &hook_payload);
    let session_pid = context.pid.or_else(|| {
        hook_payload
            .get("pid")
            .and_then(Value::as_u64)
            .and_then(SessionPid::from_u64)
    });

    let method = match event {
        HookEvent::SessionStart | HookEvent::SubagentStart | HookEvent::SubagentStop => {
            AiHookMethod::SessionStart
        }
        HookEvent::SessionEnd => AiHookMethod::SessionEnd,
        HookEvent::UserPromptSubmit => AiHookMethod::PromptSubmit,
        HookEvent::Notification => {
            let notification_type = hook_payload
                .get("notification_type")
                .and_then(Value::as_str);
            if !matches!(
                notification_type,
                Some("permission_prompt" | "elicitation_dialog")
            ) {
                return Ok(BuildOutcome::Drop(DropReason::InformationalNotification(
                    notification_type.map(|kind| truncate_utf8(kind, 128)),
                )));
            }
            AiHookMethod::Notification
        }
        HookEvent::Stop | HookEvent::StopFailure | HookEvent::Interrupt => {
            if let Some(count) = pending_tool_calls_after_llm_call(&hook_payload) {
                return Ok(BuildOutcome::Drop(
                    DropReason::LlmCallContinuesWithToolCalls(count),
                ));
            }
            AiHookMethod::Stop
        }
        HookEvent::PreToolUse if ask_user_question => AiHookMethod::SessionStart,
        HookEvent::PreToolUse | HookEvent::PostToolUse => AiHookMethod::ToolUse,
        HookEvent::PermissionRequest => AiHookMethod::Notification,
        HookEvent::Exit => AiHookMethod::Exit,
    };

    let compact_payload = compact_hook_payload(event, &hook_payload);
    let mut params = AiHookParams::new(context.workspace_id, context.tool, compact_payload);
    params.pid = session_pid;
    params.emitted_at_ms = paneflow_ipc_client::ai_hook::epoch_millis();
    params.surface_id = context.surface_id;
    params.runtime_generation = context.runtime_generation;

    if matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse) {
        params.tool_name = hook_payload
            .get("tool_name")
            .and_then(Value::as_str)
            .map(|name| truncate_utf8(name, 128));
    }
    if event == HookEvent::Exit {
        params.exit_code = hook_payload
            .get("exit_code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok());
        if params.exit_code.is_none() {
            return Err(BuildError::MissingOrInvalidExitCode);
        }
    }
    if event.carries_interrupt_source() {
        params.event_source = if event == HookEvent::Interrupt {
            Some(LifecycleEventSource::Interrupt)
        } else {
            context.event_source
        };
    }

    Ok(BuildOutcome::Send(AiHookFrame::new(method, params)))
}

fn pending_tool_calls_after_llm_call(payload: &Value) -> Option<u64> {
    if payload.get("hook_event_name").and_then(Value::as_str) != Some("PostLLMCall") {
        return None;
    }
    payload
        .get("tool_call_count")
        .and_then(Value::as_u64)
        .filter(|count| *count > 0)
}

fn is_ask_user_question(event: HookEvent, payload: &Value) -> bool {
    event == HookEvent::PreToolUse
        && payload.get("tool_name").and_then(Value::as_str) == Some("AskUserQuestion")
}

fn compact_hook_payload(event: HookEvent, payload: &Value) -> Value {
    let mut compact = serde_json::Map::new();
    compact.insert(
        "hook_event_name".into(),
        Value::String(
            match event {
                HookEvent::SessionStart | HookEvent::SubagentStart | HookEvent::SubagentStop => {
                    "HookSeen"
                }
                HookEvent::PreToolUse if is_ask_user_question(event, payload) => "HookSeen",
                _ => event.name(),
            }
            .to_string(),
        ),
    );
    copy_string_field(payload, &mut compact, "session_id", 256);
    copy_u64_field(payload, &mut compact, "pid");

    match event {
        HookEvent::SessionStart => {
            copy_string_field(payload, &mut compact, "cwd", 2048);
            copy_string_field(payload, &mut compact, "transcript_path", 2048);
        }
        HookEvent::UserPromptSubmit => {
            copy_string_field(payload, &mut compact, "prompt", MAX_HOOK_PROMPT_BYTES);
        }
        HookEvent::Notification => {
            copy_string_field(payload, &mut compact, "notification_type", 128);
            copy_string_field(payload, &mut compact, "message", MAX_HOOK_TEXT_BYTES);
        }
        HookEvent::PermissionRequest => {
            copy_string_field(payload, &mut compact, "message", MAX_HOOK_TEXT_BYTES);
            copy_string_field(payload, &mut compact, "tool_name", 128);
        }
        HookEvent::PreToolUse | HookEvent::PostToolUse => {
            copy_string_field(payload, &mut compact, "tool_name", 128);
        }
        HookEvent::Stop | HookEvent::StopFailure | HookEvent::Interrupt | HookEvent::SessionEnd => {
            copy_string_field(
                payload,
                &mut compact,
                "last_assistant_message",
                MAX_HOOK_TEXT_BYTES,
            );
            copy_string_field(payload, &mut compact, "summary", MAX_HOOK_TEXT_BYTES);
            copy_string_field(payload, &mut compact, "last_result", MAX_HOOK_TEXT_BYTES);
            copy_string_field(payload, &mut compact, "transcript_path", 2048);
            copy_string_field(payload, &mut compact, "reason", 128);
            copy_background_task_count(payload, &mut compact);
        }
        HookEvent::Exit => {
            copy_i64_field(payload, &mut compact, "exit_code");
            copy_string_field(payload, &mut compact, "summary", MAX_HOOK_TEXT_BYTES);
        }
        HookEvent::SubagentStart | HookEvent::SubagentStop => {}
    }

    Value::Object(compact)
}

fn copy_background_task_count(source: &Value, target: &mut serde_json::Map<String, Value>) {
    let Some(tasks) = source.get("background_tasks") else {
        return;
    };
    let count = match tasks {
        Value::Array(entries) => entries.len() as u64,
        Value::Number(number) => number.as_u64().unwrap_or(0),
        Value::Bool(pending) => u64::from(*pending),
        _ => return,
    };
    target.insert("background_tasks".to_owned(), Value::from(count));
}

fn copy_string_field(
    source: &Value,
    target: &mut serde_json::Map<String, Value>,
    key: &str,
    max_bytes: usize,
) {
    if let Some(value) = source.get(key).and_then(Value::as_str) {
        target.insert(
            key.to_owned(),
            Value::String(truncate_utf8(value, max_bytes)),
        );
    }
}

fn copy_u64_field(source: &Value, target: &mut serde_json::Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_u64) {
        target.insert(key.to_owned(), Value::from(value));
    }
}

fn copy_i64_field(source: &Value, target: &mut serde_json::Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).and_then(Value::as_i64) {
        target.insert(key.to_owned(), Value::from(value));
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    const MARKER: &str = "...[truncated]";
    if max_bytes <= MARKER.len() {
        return MARKER[..max_bytes].to_owned();
    }
    let keep = max_bytes - MARKER.len();
    let mut boundary = keep.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}{}", &value[..boundary], MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_ipc_client::ai_hook::{AiToolName, LifecycleEventSource, SessionPid};
    use serde_json::json;

    fn test_context() -> FrameContext {
        FrameContext {
            workspace_id: 7,
            tool: AiToolName::parse("claude").expect("valid test tool"),
            pid: None,
            surface_id: None,
            event_source: None,
            runtime_generation: Some(3),
        }
    }

    fn sent_frame(outcome: BuildOutcome) -> Value {
        match outcome {
            BuildOutcome::Send(frame) => frame.to_value(),
            BuildOutcome::Drop(reason) => panic!("unexpected drop: {reason}"),
        }
    }

    #[test]
    fn supported_events_map_to_methods() {
        let cases = [
            (HookEvent::UserPromptSubmit, json!({}), "ai.prompt_submit"),
            (
                HookEvent::Notification,
                json!({"notification_type": "permission_prompt"}),
                "ai.notification",
            ),
            (HookEvent::Stop, json!({}), "ai.stop"),
            (
                HookEvent::Stop,
                json!({"hook_event_name": "PostLLMCall", "tool_call_count": 0}),
                "ai.stop",
            ),
            (HookEvent::SubagentStart, json!({}), "ai.session_start"),
            (HookEvent::SubagentStop, json!({}), "ai.session_start"),
            (HookEvent::Interrupt, json!({}), "ai.stop"),
            (
                HookEvent::PreToolUse,
                json!({"tool_name": "Bash"}),
                "ai.tool_use",
            ),
            (
                HookEvent::PostToolUse,
                json!({"tool_name": "Edit"}),
                "ai.tool_use",
            ),
            (HookEvent::PermissionRequest, json!({}), "ai.notification"),
            (HookEvent::SessionEnd, json!({}), "ai.session_end"),
        ];

        for (event, payload, expected_method) in cases {
            let frame =
                sent_frame(build_frame(event, test_context(), payload).expect("valid frame"));
            assert_eq!(frame["method"], expected_method, "event={}", event.name());
        }
    }

    #[test]
    fn a_stop_carries_its_background_task_count_and_its_failure_reason_without_the_task_payloads() {
        let frame = sent_frame(
            build_frame(
                HookEvent::Stop,
                test_context(),
                json!({
                    "last_assistant_message": "The change is ready.",
                    "background_tasks": [
                        {"id": "build", "prompt": "secret prompt"},
                        {"id": "tests"}
                    ],
                }),
            )
            .expect("valid frame"),
        );
        assert_eq!(frame["params"]["hook_payload"]["background_tasks"], 2);
        assert_eq!(
            frame["params"]["hook_payload"]["last_assistant_message"],
            "The change is ready."
        );
        assert!(
            !frame["params"]["hook_payload"]
                .to_string()
                .contains("secret"),
            "only the count crosses the wire, never the task payloads"
        );

        let empty = sent_frame(
            build_frame(
                HookEvent::Stop,
                test_context(),
                json!({"background_tasks": []}),
            )
            .expect("valid frame"),
        );
        assert_eq!(empty["params"]["hook_payload"]["background_tasks"], 0);

        let failed = sent_frame(
            build_frame(
                HookEvent::StopFailure,
                test_context(),
                json!({"reason": "matcher rejected the answer"}),
            )
            .expect("valid frame"),
        );
        assert_eq!(
            failed["params"]["hook_payload"]["reason"],
            "matcher rejected the answer"
        );

        let bare = sent_frame(build_frame(HookEvent::Stop, test_context(), json!({})).expect("ok"));
        assert!(bare["params"]["hook_payload"]["background_tasks"].is_null());
    }

    #[test]
    fn stop_from_a_post_llm_call_with_tool_calls_is_dropped() {
        let outcome = build_frame(
            HookEvent::Stop,
            test_context(),
            json!({"hook_event_name": "PostLLMCall", "tool_call_count": 2}),
        )
        .expect("valid payload");
        match outcome {
            BuildOutcome::Drop(reason) => {
                assert_eq!(reason, DropReason::LlmCallContinuesWithToolCalls(2));
            }
            BuildOutcome::Send(frame) => panic!("unexpected frame: {:?}", frame.to_value()),
        }
    }

    #[test]
    fn session_start_accepts_a_hand_typed_runtime_without_a_pid() {
        let untagged = sent_frame(
            build_frame(HookEvent::SessionStart, test_context(), json!({}))
                .expect("hand typed runtime"),
        );
        assert!(untagged["params"].get("pid").is_none());
        let frame = sent_frame(
            build_frame(HookEvent::SessionStart, test_context(), json!({"pid": 42}))
                .expect("payload pid is valid"),
        );
        assert_eq!(frame["params"]["pid"], 42);
        assert_eq!(frame["params"]["runtime_generation"], 3);
        assert_eq!(
            frame["params"]["hook_payload"]["hook_event_name"],
            "HookSeen"
        );
    }

    #[test]
    fn ask_user_question_pre_tool_use_is_latch_only() {
        let frame = sent_frame(
            build_frame(
                HookEvent::PreToolUse,
                test_context(),
                json!({"tool_name": "AskUserQuestion"}),
            )
            .expect("valid AskUserQuestion hook"),
        );
        assert_eq!(frame["method"], "ai.session_start");
        assert_eq!(frame["params"]["tool_name"], "AskUserQuestion");
        assert_eq!(
            frame["params"]["hook_payload"]["hook_event_name"],
            "HookSeen"
        );
    }

    #[test]
    fn environment_pid_wins_over_payload_pid() {
        let mut context = test_context();
        context.pid = SessionPid::new(4242);
        let frame = sent_frame(
            build_frame(HookEvent::Stop, context, json!({"pid": 7777})).expect("valid stop"),
        );
        assert_eq!(frame["params"]["pid"], 4242);
        assert_eq!(frame["params"]["hook_payload"]["pid"], 7777);
    }

    #[test]
    fn informational_notification_has_an_explicit_drop_outcome() {
        let outcome = build_frame(
            HookEvent::Notification,
            test_context(),
            json!({"notification_type": "idle_prompt"}),
        )
        .expect("drop is not an error");
        match outcome {
            BuildOutcome::Drop(DropReason::InformationalNotification(Some(kind))) => {
                assert_eq!(kind, "idle_prompt");
            }
            BuildOutcome::Drop(other) => panic!("unexpected drop: {other}"),
            BuildOutcome::Send(_) => panic!("informational notification was sent"),
        }
    }

    #[test]
    fn permission_request_does_not_emit_speculative_metadata() {
        let frame = sent_frame(
            build_frame(
                HookEvent::PermissionRequest,
                test_context(),
                json!({"message": "Allow?"}),
            )
            .expect("valid permission request"),
        );
        assert!(frame["params"].get("notification_type").is_none());
        assert_eq!(frame["params"]["hook_payload"]["message"], "Allow?");
    }

    #[test]
    fn exit_code_is_typed_at_the_boundary() {
        assert_eq!(
            build_frame(
                HookEvent::Exit,
                test_context(),
                json!({"exit_code": i64::MAX}),
            )
            .unwrap_err(),
            BuildError::MissingOrInvalidExitCode
        );
        let frame = sent_frame(
            build_frame(
                HookEvent::Exit,
                test_context(),
                json!({"exit_code": -1_073_741_510_i32}),
            )
            .expect("Windows NTSTATUS fits i32"),
        );
        assert_eq!(frame["params"]["exit_code"], -1_073_741_510_i64);
    }

    #[test]
    fn interrupt_source_is_limited_to_lifecycle_events() {
        let mut context = test_context();
        context.event_source = Some(LifecycleEventSource::Interrupt);
        let stop =
            sent_frame(build_frame(HookEvent::Stop, context, json!({})).expect("valid stop frame"));
        assert_eq!(stop["params"]["event_source"], "interrupt");

        let mut context = test_context();
        context.event_source = Some(LifecycleEventSource::Interrupt);
        let prompt = sent_frame(
            build_frame(HookEvent::UserPromptSubmit, context, json!({})).expect("valid prompt"),
        );
        assert!(prompt["params"].get("event_source").is_none());
    }

    #[test]
    fn payload_compaction_caps_text() {
        let payload = json!({
            "session_id": "s1",
            "prompt": "x".repeat(10_000),
            "message": "é".repeat(10_000),
            "notification_type": "permission_prompt",
        });
        let submit = compact_hook_payload(HookEvent::UserPromptSubmit, &payload);
        let prompt = submit["prompt"].as_str().expect("prompt string");
        assert_eq!(prompt.len(), MAX_HOOK_PROMPT_BYTES);
        assert_eq!(submit["session_id"], json!("s1"));

        let notification = compact_hook_payload(HookEvent::Notification, &payload);
        let message = notification["message"].as_str().expect("message string");
        assert!(message.len() <= MAX_HOOK_TEXT_BYTES);
        assert!(message.is_char_boundary(message.len()));
    }

    #[test]
    fn a_multibyte_prompt_is_truncated_on_a_character_boundary() {
        let payload = json!({ "prompt": "é".repeat(1_000) });
        let submit = compact_hook_payload(HookEvent::UserPromptSubmit, &payload);
        let prompt = submit["prompt"].as_str().expect("prompt string");
        assert!(prompt.len() <= MAX_HOOK_PROMPT_BYTES);
        assert!(prompt.is_char_boundary(prompt.len()));
        assert!(
            prompt
                .trim_end_matches("...[truncated]")
                .chars()
                .all(|ch| ch == 'é'),
            "only the shared truncation marker follows the kept text"
        );
    }

    #[test]
    fn payload_compaction_keeps_only_the_prompt_and_its_identifiers() {
        let payload = json!({
            "session_id": "s1",
            "pid": 4242,
            "prompt": "fix the flaky worktree test",
            "cwd": "/home/user/project",
            "transcript_path": "/home/user/.claude/projects/x.jsonl",
        });
        assert_eq!(
            compact_hook_payload(HookEvent::UserPromptSubmit, &payload),
            json!({
                "hook_event_name": "UserPromptSubmit",
                "session_id": "s1",
                "pid": 4242,
                "prompt": "fix the flaky worktree test",
            })
        );
    }
}
