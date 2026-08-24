use std::env;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use paneflow_ipc_client::ai_hook::{AiToolName, LifecycleEventSource, SessionPid, SurfaceId};
use serde_json::Value;

use crate::event::{build_frame, BuildOutcome, FrameContext, HookEvent, InputSource};
use crate::transport::send_frame;
use crate::MAX_STDIN_BYTES;

const SOCKET_PATH_ENV: &str = "PANEFLOW_SOCKET_PATH";
const WORKSPACE_ID_ENV: &str = "PANEFLOW_WORKSPACE_ID";
const TOOL_ENV: &str = "PANEFLOW_AI_TOOL";
const PID_ENV: &str = "PANEFLOW_AI_PID";
const SURFACE_ID_ENV: &str = "PANEFLOW_SURFACE_ID";
const EXIT_CODE_ENV: &str = "PANEFLOW_AI_EXIT_CODE";
const EVENT_SOURCE_ENV: &str = "PANEFLOW_AI_EVENT_SOURCE";
const HOOK_LOG_ENV: &str = "PANEFLOW_HOOK_LOG";

pub(crate) fn dispatch() {
    let Some(event_name) = env::args().nth(1) else {
        diagnose("missing argv[1] hook event name");
        return;
    };
    let Ok(event) = event_name.parse::<HookEvent>() else {
        diagnose(&format!("{event_name}: unhandled hook event"));
        return;
    };
    let Some(socket_path) = read_socket_path() else {
        return;
    };
    let Some(workspace_id) = read_workspace_id() else {
        return;
    };
    let Some(hook_payload) = read_payload(event) else {
        return;
    };
    let tool = match detect_tool_from(env::var(TOOL_ENV).ok().as_deref()) {
        Ok(tool) => tool,
        Err(error) => {
            diagnose(&format!("{TOOL_ENV}: {error}"));
            return;
        }
    };
    let context = FrameContext {
        workspace_id,
        tool,
        pid: read_ai_pid_from(env::var(PID_ENV).ok().as_deref()),
        surface_id: read_surface_id_from(env::var(SURFACE_ID_ENV).ok().as_deref()),
        event_source: read_event_source_from(env::var(EVENT_SOURCE_ENV).ok().as_deref()),
    };

    match build_frame(event, context, hook_payload) {
        Ok(BuildOutcome::Send(frame)) => {
            if let Err(error) = send_frame(&socket_path, &frame) {
                diagnose(&format!("{}: send_frame failed: {error}", event.name()));
            }
        }
        Ok(BuildOutcome::Drop(reason)) => diagnose(&format!("{}: {reason}", event.name())),
        Err(error) => diagnose(&format!("{}: {error}", event.name())),
    }
}

fn read_payload(event: HookEvent) -> Option<Value> {
    match event.input_source() {
        InputSource::Empty => Some(serde_json::json!({})),
        InputSource::ExitCodeEnvironment => {
            let Some(exit_code) = read_exit_code_from(env::var(EXIT_CODE_ENV).ok().as_deref())
            else {
                diagnose(&format!(
                    "{}: missing or invalid {EXIT_CODE_ENV}",
                    event.name()
                ));
                return None;
            };
            Some(serde_json::json!({"exit_code": exit_code}))
        }
        InputSource::Stdin => read_stdin_json(event),
    }
}

fn read_socket_path() -> Option<PathBuf> {
    let path = read_socket_path_from(env::var_os(SOCKET_PATH_ENV).as_deref())?;
    if !path.is_absolute() {
        diagnose(&format!("{SOCKET_PATH_ENV} is not absolute"));
        return None;
    }
    Some(path)
}

fn read_socket_path_from(raw: Option<&OsStr>) -> Option<PathBuf> {
    raw.map(PathBuf::from)
}

fn read_workspace_id() -> Option<u64> {
    let raw = env::var(WORKSPACE_ID_ENV).ok()?;
    match raw.parse::<u64>() {
        Ok(workspace_id) => Some(workspace_id),
        Err(_) => {
            diagnose(&format!("{WORKSPACE_ID_ENV} is not u64"));
            None
        }
    }
}

fn detect_tool_from(
    raw: Option<&str>,
) -> Result<AiToolName, paneflow_ipc_client::ai_hook::InvalidToolName> {
    raw.map_or_else(|| Ok(AiToolName::legacy_default()), AiToolName::parse)
}

fn read_ai_pid_from(raw: Option<&str>) -> Option<SessionPid> {
    raw?.parse::<u32>().ok().and_then(SessionPid::new)
}

fn read_surface_id_from(raw: Option<&str>) -> Option<SurfaceId> {
    raw?.parse::<u64>().ok().and_then(SurfaceId::new)
}

fn read_exit_code_from(raw: Option<&str>) -> Option<i32> {
    raw?.parse::<i32>().ok()
}

fn read_event_source_from(raw: Option<&str>) -> Option<LifecycleEventSource> {
    raw.and_then(LifecycleEventSource::parse)
}

fn read_stdin_json(event: HookEvent) -> Option<Value> {
    let mut bytes = Vec::new();
    if std::io::stdin()
        .take(MAX_STDIN_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        diagnose(&format!("{}: stdin read error", event.name()));
        return None;
    }
    if bytes.len() > MAX_STDIN_BYTES {
        diagnose(&format!(
            "{}: stdin exceeds {MAX_STDIN_BYTES} bytes",
            event.name()
        ));
        return None;
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        diagnose(&format!("{}: empty stdin", event.name()));
        return None;
    }
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(_) => {
            diagnose(&format!("{}: invalid stdin JSON", event.name()));
            None
        }
    }
}

fn diagnose(message: &str) {
    diagnose_to(message, env::var_os(HOOK_LOG_ENV).as_deref().map(Path::new));
}

fn diagnose_to(message: &str, log_path: Option<&Path>) {
    let Some(log_path) = log_path else {
        return;
    };
    let line = format!("paneflow-ai-hook: {message}\n");
    let _ = OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_ipc_client::ai_hook::MAX_SESSION_PID;

    #[test]
    fn missing_tool_uses_the_legacy_default_but_malformed_tool_is_rejected() {
        assert_eq!(
            detect_tool_from(None).expect("legacy default").as_str(),
            "claude"
        );
        assert_eq!(
            detect_tool_from(Some("cursor-agent"))
                .expect("valid tool")
                .as_str(),
            "cursor-agent"
        );
        assert!(detect_tool_from(Some("tool/../etc")).is_err());
    }

    #[test]
    fn pid_parser_uses_the_shared_server_range() {
        assert_eq!(
            read_ai_pid_from(Some(&MAX_SESSION_PID.to_string())).map(SessionPid::get),
            Some(MAX_SESSION_PID)
        );
        assert!(read_ai_pid_from(Some(&(MAX_SESSION_PID + 1).to_string())).is_none());
        assert!(read_ai_pid_from(Some("0")).is_none());
        assert!(read_ai_pid_from(Some("abc")).is_none());
    }

    #[test]
    fn optional_identifiers_and_event_source_are_validated() {
        assert_eq!(read_surface_id_from(Some("7")).map(SurfaceId::get), Some(7));
        assert!(read_surface_id_from(Some("0")).is_none());
        assert_eq!(
            read_event_source_from(Some("interrupt")),
            Some(LifecycleEventSource::Interrupt)
        );
        assert!(read_event_source_from(Some("other")).is_none());
    }

    #[test]
    fn exit_code_accepts_negative_windows_status() {
        assert_eq!(
            read_exit_code_from(Some("-1073741510")),
            Some(-1_073_741_510)
        );
        assert!(read_exit_code_from(Some("abc")).is_none());
    }

    #[test]
    fn diagnose_appends_complete_lines() {
        let directory = tempfile::TempDir::new().expect("temp directory");
        let path = directory.path().join("hook.log");
        diagnose_to("first", Some(&path));
        diagnose_to("second", Some(&path));
        let contents = std::fs::read_to_string(path).expect("read hook log");
        assert_eq!(
            contents,
            "paneflow-ai-hook: first\npaneflow-ai-hook: second\n"
        );
    }
}
