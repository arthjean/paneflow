use std::io::Write;
use std::path::Path;
use std::time::Duration;

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Map, Value};

use crate::event::HookEvent;

pub(crate) const ANSWER_ARG: &str = "answer";
pub(crate) const RESOLVE_METHOD: &str = "agent_prompt.resolve";
pub(crate) const RESOLVE_DEADLINE: Duration = Duration::from_secs(330);
const WORKER_CLIENT_NAME: &str = "paneflow-ai-hook";
const ASK_USER_QUESTION: &str = "AskUserQuestion";

pub(crate) fn resolve_event(event: HookEvent, payload: &Value) -> Option<&'static str> {
    let tool = payload.get("tool_name").and_then(Value::as_str);
    match event {
        HookEvent::PreToolUse if tool == Some(ASK_USER_QUESTION) => Some("ask_user_question"),
        HookEvent::PermissionRequest if tool.is_some_and(|tool| tool != ASK_USER_QUESTION) => {
            Some("permission_request")
        }
        _ => None,
    }
}

pub(crate) fn resolve(
    host_endpoint: &Path,
    connect_deadline: Duration,
    params: Value,
) -> Option<Value> {
    let endpoint = paneflow_home::serve_endpoint_beside_host_endpoint(host_endpoint)?;
    let reply = HostControl::connect_with_deadline(&endpoint, WORKER_CLIENT_NAME, connect_deadline)
        .ok()?
        .request_with_deadline(RESOLVE_METHOD, params, RESOLVE_DEADLINE)
        .ok()?;
    (reply.get("status").and_then(Value::as_str) == Some("answered")
        && reply.get("output").is_some_and(Value::is_object))
    .then_some(reply)
}

pub(crate) fn resolve_params(
    event: &str,
    agent: Option<&str>,
    session: &str,
    payload: Value,
) -> Value {
    let mut params = Map::new();
    params.insert("event".into(), Value::from(event));
    if let Some(agent) = agent {
        params.insert("agent".into(), Value::from(agent));
    }
    params.insert("session".into(), Value::from(session));
    params.insert("payload".into(), payload);
    Value::Object(params)
}

pub(crate) fn print(reply: &Value) {
    let Some(output) = reply.get("output") else {
        return;
    };
    let Ok(bytes) = serde_json::to_vec(output) else {
        return;
    };
    let _ = stdout().write_all(&bytes);
}

#[cfg(unix)]
fn stdout() -> std::mem::ManuallyDrop<std::fs::File> {
    use std::os::fd::FromRawFd;
    std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) })
}

#[cfg(not(unix))]
fn stdout() -> std::io::Stdout {
    std::io::stdout()
}

#[cfg(unix)]
pub(crate) struct Stdin;

#[cfg(unix)]
const EBADF: i32 = 9;

#[cfg(unix)]
impl std::io::Read for Stdin {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::os::fd::FromRawFd;
        let stdin = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(0) });
        match (&*stdin).read(buf) {
            Err(error) if error.raw_os_error() == Some(EBADF) => Ok(0),
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_a_question_or_a_permission_other_than_a_question_is_resolved() {
        let ask = json!({"tool_name": "AskUserQuestion"});
        let bash = json!({"tool_name": "Bash"});
        assert_eq!(
            resolve_event(HookEvent::PreToolUse, &ask),
            Some("ask_user_question")
        );
        assert_eq!(
            resolve_event(HookEvent::PermissionRequest, &bash),
            Some("permission_request")
        );
        assert_eq!(resolve_event(HookEvent::PermissionRequest, &ask), None);
        assert_eq!(resolve_event(HookEvent::PreToolUse, &bash), None);
        assert_eq!(
            resolve_event(HookEvent::PermissionRequest, &json!({})),
            None
        );
        assert_eq!(resolve_event(HookEvent::Stop, &ask), None);
    }

    #[test]
    fn the_helper_waits_past_the_workers_300_seconds_and_inside_the_600_second_hook_timeout() {
        assert!(RESOLVE_DEADLINE > Duration::from_secs(300));
        assert!(RESOLVE_DEADLINE < Duration::from_secs(600));
    }

    #[test]
    fn the_resolve_params_carry_the_hook_input_as_it_was_read() {
        let payload = json!({"tool_name": "Bash", "tool_input": {"command": "echo hi"}});
        assert_eq!(
            resolve_params("permission_request", Some("claude"), "s-1", payload.clone()),
            json!({"event": "permission_request", "agent": "claude", "session": "s-1", "payload": payload})
        );
        assert!(resolve_params("ask_user_question", None, "s-1", json!({}))
            .get("agent")
            .is_none());
    }
}
