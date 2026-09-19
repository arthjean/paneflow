use std::env;
use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use paneflow_agent_config::{canonical_command_for_alias, canonical_command_for_script_path};
use paneflow_ipc_client::ai_hook::{AiToolName, LifecycleEventSource, SessionPid, SurfaceId};
use paneflow_ipc_client::host_control::{host_endpoint_from, session_id_from};
use serde_json::Value;

use crate::event::{build_frame, BuildOutcome, FrameContext, HookEvent, InputSource};
use crate::transport::{send_agent_event, send_frame};
use crate::MAX_STDIN_BYTES;

const SOCKET_PATH_ENV: &str = "PANEFLOW_SOCKET_PATH";
const WORKSPACE_ID_ENV: &str = "PANEFLOW_WORKSPACE_ID";
const TOOL_ENV: &str = "PANEFLOW_AI_TOOL";
const PID_ENV: &str = "PANEFLOW_AI_PID";
const SURFACE_ID_ENV: &str = "PANEFLOW_SURFACE_ID";
const EXIT_CODE_ENV: &str = "PANEFLOW_AI_EXIT_CODE";
const EVENT_SOURCE_ENV: &str = "PANEFLOW_AI_EVENT_SOURCE";
const HOOK_LOG_ENV: &str = "PANEFLOW_HOOK_LOG";
const HOST_ENDPOINT_ENV: &str = "PANEFLOW_HOST_ENDPOINT";
const SESSION_ID_ENV: &str = "PANEFLOW_SESSION_ID";

pub(crate) enum Target {
    Host { endpoint: PathBuf, session: String },
    Controller { socket: PathBuf, workspace_id: u64 },
}

pub(crate) fn resolve_target(
    host_endpoint: Option<&OsStr>,
    session: Option<&str>,
    socket: Option<&OsStr>,
    workspace_id: Option<&str>,
) -> Option<Target> {
    if let (Some(endpoint), Some(session)) = (
        host_endpoint_from(host_endpoint).filter(|path| path.is_absolute()),
        session_id_from(session),
    ) {
        return Some(Target::Host { endpoint, session });
    }
    let socket = read_socket_path_from(socket)?;
    if !socket.is_absolute() {
        return None;
    }
    let workspace_id = workspace_id?.parse::<u64>().ok()?;
    Some(Target::Controller {
        socket,
        workspace_id,
    })
}

pub(crate) fn dispatch() {
    let Some(event_name) = env::args().nth(1) else {
        diagnose("missing argv[1] hook event name");
        return;
    };
    let Ok(event) = event_name.parse::<HookEvent>() else {
        diagnose(&format!("{event_name}: unhandled hook event"));
        return;
    };
    let Some(target) = resolve_target(
        env::var_os(HOST_ENDPOINT_ENV).as_deref(),
        env::var(SESSION_ID_ENV).ok().as_deref(),
        env::var_os(SOCKET_PATH_ENV).as_deref(),
        env::var(WORKSPACE_ID_ENV).ok().as_deref(),
    ) else {
        diagnose(&format!(
            "{}: no local host endpoint and no reachable {SOCKET_PATH_ENV}/{WORKSPACE_ID_ENV} pair",
            event.name()
        ));
        return;
    };
    let workspace_id = match &target {
        Target::Host { .. } => 0,
        Target::Controller { workspace_id, .. } => *workspace_id,
    };
    let Some(hook_payload) = read_payload(event) else {
        return;
    };
    let tool = match detect_tool_from(env::var(TOOL_ENV).ok().as_deref(), &hook_payload) {
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
            let delivered = match &target {
                Target::Host { endpoint, session } => send_agent_event(endpoint, session, &frame),
                Target::Controller { socket, .. } => send_frame(socket, &frame),
            };
            if let Err(error) = delivered {
                diagnose(&format!("{}: delivery failed: {error}", event.name()));
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

fn read_socket_path_from(raw: Option<&OsStr>) -> Option<PathBuf> {
    raw.filter(|value| !value.is_empty()).map(PathBuf::from)
}

fn detect_tool_from(
    raw: Option<&str>,
    payload: &Value,
) -> Result<AiToolName, paneflow_ipc_client::ai_hook::InvalidToolName> {
    if let Some(raw) = raw {
        return catalog_tool_name(raw).map_or_else(|| AiToolName::parse(raw), AiToolName::parse);
    }
    for key in [
        "runtime",
        "runtime_id",
        "agent",
        "agent_name",
        "client",
        "tool",
    ] {
        if let Some(candidate) = payload.get(key).and_then(Value::as_str) {
            if let Some(tool) = catalog_tool_name(candidate) {
                return AiToolName::parse(tool);
            }
        }
    }
    Ok(AiToolName::legacy_default())
}

fn catalog_tool_name(candidate: &str) -> Option<&'static str> {
    canonical_command_for_alias(candidate).or_else(|| canonical_command_for_script_path(candidate))
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
    use std::ffi::OsString;

    #[test]
    fn the_local_host_wins_over_the_controller_socket_when_both_are_present() {
        let absolute_socket = if cfg!(windows) {
            OsString::from(r"\\.\pipe\paneflow-test")
        } else {
            OsString::from("/tmp/paneflow.sock")
        };
        let absolute_endpoint = if cfg!(windows) {
            OsString::from(r"\\.\pipe\paneflow-host-test")
        } else {
            OsString::from("/tmp/paneflow-host.sock")
        };

        let target = resolve_target(
            Some(&absolute_endpoint),
            Some("11112222-3333-4444-5555-666677778888"),
            Some(&absolute_socket),
            Some("7"),
        );
        match target {
            Some(Target::Host { endpoint, session }) => {
                assert_eq!(endpoint, PathBuf::from(&absolute_endpoint));
                assert_eq!(session, "11112222-3333-4444-5555-666677778888");
            }
            _ => panic!("the durable host session must win"),
        }

        let target = resolve_target(None, None, Some(&absolute_socket), Some("7"));
        match target {
            Some(Target::Controller {
                socket,
                workspace_id,
            }) => {
                assert_eq!(socket, PathBuf::from(&absolute_socket));
                assert_eq!(workspace_id, 7);
            }
            _ => panic!("the controller socket stays the fallback"),
        }

        assert!(
            resolve_target(Some(&absolute_endpoint), None, None, None).is_none(),
            "a host endpoint without a durable session addresses nothing"
        );
        assert!(
            resolve_target(None, None, Some(&absolute_socket), None).is_none(),
            "the controller path still needs its workspace id"
        );
        assert!(
            resolve_target(None, None, Some(OsStr::new("relative.sock")), Some("7")).is_none(),
            "a relative socket path is refused"
        );
        assert!(resolve_target(None, None, None, Some("7")).is_none());
    }

    #[test]
    fn missing_tool_uses_the_legacy_default_but_malformed_tool_is_rejected() {
        assert_eq!(
            detect_tool_from(None, &serde_json::json!({}))
                .expect("legacy default")
                .as_str(),
            "claude"
        );
        assert_eq!(
            detect_tool_from(Some("cursor-agent"), &serde_json::json!({}))
                .expect("valid tool")
                .as_str(),
            "cursor-agent"
        );
        assert!(detect_tool_from(Some("tool/../etc"), &serde_json::json!({})).is_err());
        assert_eq!(
            detect_tool_from(
                None,
                &serde_json::json!({"runtime": "C:\\node_modules\\@openai\\codex\\bin\\codex.js"}),
            )
            .expect("payload runtime")
            .as_str(),
            "codex"
        );
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
