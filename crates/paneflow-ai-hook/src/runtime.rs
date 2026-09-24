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
use crate::transport::send_agent_event;
use crate::MAX_STDIN_BYTES;

const TOOL_ENV: &str = "PANEFLOW_AI_TOOL";
const PID_ENV: &str = "PANEFLOW_AI_PID";
const SURFACE_ID_ENV: &str = "PANEFLOW_SURFACE_ID";
const EXIT_CODE_ENV: &str = "PANEFLOW_AI_EXIT_CODE";
const EVENT_SOURCE_ENV: &str = "PANEFLOW_AI_EVENT_SOURCE";
const HOOK_LOG_ENV: &str = "PANEFLOW_HOOK_LOG";
const HOST_ENDPOINT_ENV: &str = "PANEFLOW_HOST_ENDPOINT";
const SESSION_ID_ENV: &str = "PANEFLOW_SESSION_ID";
const SESSION_DIR_ENV: &str = "PANEFLOW_SESSION_DIR";
const RUNTIME_GENERATION_ENV: &str = "PANEFLOW_RUNTIME_GENERATION";

pub(crate) fn resolve_target(
    host_endpoint: Option<&OsStr>,
    session: Option<&str>,
) -> Option<(PathBuf, String)> {
    let (Some(endpoint), Some(session)) = (
        host_endpoint_from(host_endpoint).filter(|path| path.is_absolute()),
        session_id_from(session),
    ) else {
        return None;
    };
    Some((endpoint, session))
}

pub(crate) fn dispatch() {
    let Ok(session) = env::var(SESSION_ID_ENV) else {
        return;
    };
    let Some(event_name) = env::args().nth(1) else {
        diagnose("missing argv[1] hook event name");
        return;
    };
    let Ok(event) = event_name.parse::<HookEvent>() else {
        diagnose(&format!("{event_name}: unhandled hook event"));
        return;
    };
    let Some((endpoint, session)) =
        resolve_target(env::var_os(HOST_ENDPOINT_ENV).as_deref(), Some(&session))
    else {
        diagnose(&format!("{}: no local host endpoint", event.name()));
        return;
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
    let session_dir = env::var_os(SESSION_DIR_ENV);
    let context = FrameContext {
        workspace_id: 0,
        tool,
        pid: read_ai_pid_from(env::var(PID_ENV).ok().as_deref()),
        surface_id: read_surface_id_from(env::var(SURFACE_ID_ENV).ok().as_deref()),
        event_source: read_event_source_from(env::var(EVENT_SOURCE_ENV).ok().as_deref()),
        runtime_generation: read_runtime_generation_from(
            env::var(RUNTIME_GENERATION_ENV).ok().as_deref(),
        ),
    };
    if let Err(error) = crate::background::record(
        event,
        session_dir.as_deref().map(Path::new),
        context.runtime_generation,
        &hook_payload,
    ) {
        diagnose(&format!("{}: {error}", event.name()));
    }

    match build_frame(event, context, hook_payload) {
        Ok(BuildOutcome::Send(frame)) => {
            let delivered = send_agent_event(&endpoint, &session, &frame);
            if let Err(error) = delivered {
                if error.kind() != std::io::ErrorKind::InvalidData {
                    write_last_hook_event(session_dir.as_deref().map(Path::new), &frame);
                }
                diagnose(&format!("{}: delivery failed: {error}", event.name()));
            }
        }
        Ok(BuildOutcome::Drop(reason)) => diagnose(&format!("{}: {reason}", event.name())),
        Err(error) => diagnose(&format!("{}: {error}", event.name())),
    }
}

fn read_payload(event: HookEvent) -> Option<Value> {
    match event.input_source() {
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

fn read_runtime_generation_from(raw: Option<&str>) -> Option<u64> {
    raw?.parse::<u64>().ok().filter(|value| *value > 0)
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
    if bytes.iter().all(u8::is_ascii_whitespace) && event == HookEvent::SessionEnd {
        return Some(serde_json::json!({}));
    }
    if bytes.iter().all(u8::is_ascii_whitespace) {
        diagnose(&format!("{}: empty stdin", event.name()));
        return None;
    }
    match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|text| serde_json::from_str(text).ok())
    {
        Some(value) => Some(value),
        None => {
            diagnose(&format!("{}: invalid stdin JSON", event.name()));
            None
        }
    }
}

fn write_last_hook_event(
    session_dir: Option<&Path>,
    frame: &paneflow_ipc_client::ai_hook::AiHookFrame,
) {
    let Some(session_dir) = session_dir.filter(|path| path.is_dir()) else {
        return;
    };
    let Some(event) = frame
        .params
        .hook_payload
        .get("hook_event_name")
        .and_then(Value::as_str)
    else {
        return;
    };
    let mut seed = serde_json::Map::new();
    seed.insert("hook_event_name".into(), Value::String(event.to_string()));
    if let Some(tool_name) = frame.params.tool_name.as_deref().or_else(|| {
        frame
            .params
            .hook_payload
            .get("tool_name")
            .and_then(Value::as_str)
    }) {
        seed.insert("tool_name".into(), Value::String(tool_name.to_string()));
    }
    if let Some(notification_type) = frame
        .params
        .hook_payload
        .get("notification_type")
        .and_then(Value::as_str)
    {
        seed.insert(
            "notification_type".into(),
            Value::String(notification_type.to_string()),
        );
    }
    if let Some(generation) = frame.params.runtime_generation {
        seed.insert("runtime_generation".into(), Value::from(generation));
    }
    let Ok(bytes) = serde_json::to_vec(&Value::Object(seed)) else {
        return;
    };
    let target = session_dir.join("last-hook-event.json");
    let temporary = session_dir.join(format!(".last-hook-event.json.{}", std::process::id()));
    if std::fs::write(&temporary, bytes).is_ok() && std::fs::rename(&temporary, &target).is_err() {
        let _ = std::fs::remove_file(temporary);
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
    fn a_host_target_requires_both_an_absolute_endpoint_and_a_session() {
        let absolute_endpoint = if cfg!(windows) {
            OsString::from(r"\\.\pipe\paneflow-host-test")
        } else {
            OsString::from("/tmp/paneflow-host.sock")
        };

        let target = resolve_target(
            Some(&absolute_endpoint),
            Some("11112222-3333-4444-5555-666677778888"),
        );
        assert_eq!(
            target,
            Some((
                PathBuf::from(&absolute_endpoint),
                "11112222-3333-4444-5555-666677778888".to_string()
            ))
        );

        assert!(
            resolve_target(Some(&absolute_endpoint), None).is_none(),
            "a host endpoint without a durable session addresses nothing"
        );
        assert!(resolve_target(Some(OsStr::new("relative.sock")), Some("id")).is_none());
        assert!(resolve_target(None, Some("id")).is_none());
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
