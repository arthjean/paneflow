use std::env;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use paneflow_ipc_client::ai_hook::KIND_STATUS_LINE;
use serde_json::{Map, Value};

use crate::MAX_STDIN_BYTES;

pub(crate) const STATUS_LINE_ARG: &str =
    paneflow_agent_config::usage::status_line::STATUS_LINE_EVENT;
const STATUS_LINE_DEADLINE: Duration = Duration::from_millis(40);
const MAX_PROVIDER_SESSION_BYTES: usize = 128;
const NO_LIMITS_MARKER_PREFIX: &str = "status-line-no-limits-";
const CLAUDE_TOOL: &str = "claude";

pub(crate) struct RelayContext<'a> {
    pub(crate) session: &'a str,
    pub(crate) config_dir: &'a Path,
    pub(crate) session_dir: Option<&'a Path>,
    pub(crate) runtime_generation: Option<u64>,
    pub(crate) received_at_ms: u64,
}

pub(crate) fn relay(endpoint: &Path, session: &str, generation: Option<u64>) {
    let Some(input) = crate::runtime::read_stdin_bytes().and_then(|bytes| parse_input(&bytes))
    else {
        return;
    };
    let Some(config_dir) = claude_config_dir() else {
        return;
    };
    let session_dir = env::var_os("PANEFLOW_SESSION_DIR").map(PathBuf::from);
    let context = RelayContext {
        session,
        config_dir: &config_dir,
        session_dir: session_dir.as_deref(),
        runtime_generation: generation,
        received_at_ms: paneflow_ipc_client::ai_hook::epoch_millis().unwrap_or(0),
    };
    if let Some(params) = relay_params(input, &context) {
        let _ = crate::transport::send_event_params(endpoint, params, STATUS_LINE_DEADLINE);
    }
}

pub(crate) fn parse_input(bytes: &[u8]) -> Option<Value> {
    if bytes.len() > MAX_STDIN_BYTES {
        return None;
    }
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .filter(Value::is_object)
}

fn claude_config_dir() -> Option<PathBuf> {
    env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| {
            env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .filter(|home| home.is_absolute())
                .map(|home| home.join(".claude"))
        })
}

pub(crate) fn relay_params(mut input: Value, context: &RelayContext<'_>) -> Option<Value> {
    let reported = input.get_mut("rate_limits").map(Value::take);
    let provider_session = input
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|id| is_safe_session_id(id));
    let rate_limits = match reported {
        Some(limits) if limits.is_object() => limits,
        _ if has_answered(&input) => {
            if !claim_no_limits_report(context.session_dir, provider_session?) {
                return None;
            }
            Value::Null
        }
        _ => return None,
    };
    let mut payload = Map::new();
    payload.insert(
        "session_id".into(),
        provider_session.map_or(Value::Null, |id| Value::String(id.to_owned())),
    );
    payload.insert(
        "config_dir".into(),
        Value::String(context.config_dir.display().to_string()),
    );
    payload.insert("rate_limits".into(), rate_limits);
    payload.insert("received_at_ms".into(), Value::from(context.received_at_ms));
    let mut params = Map::new();
    params.insert("session".into(), Value::String(context.session.to_owned()));
    params.insert("kind".into(), Value::String(KIND_STATUS_LINE.to_owned()));
    params.insert("tool".into(), Value::String(CLAUDE_TOOL.to_owned()));
    params.insert("emitted_at_ms".into(), Value::from(context.received_at_ms));
    if let Some(generation) = context.runtime_generation {
        params.insert("runtime_generation".into(), Value::from(generation));
    }
    params.insert("hook_payload".into(), Value::Object(payload));
    Some(Value::Object(params))
}

fn has_answered(input: &Value) -> bool {
    input
        .get("cost")
        .and_then(|cost| cost.get("total_api_duration_ms"))
        .and_then(Value::as_u64)
        .is_some_and(|duration| duration > 0)
}

fn is_safe_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_PROVIDER_SESSION_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn claim_no_limits_report(session_dir: Option<&Path>, provider_session: &str) -> bool {
    let Some(session_dir) = session_dir.filter(|dir| dir.is_dir()) else {
        return false;
    };
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(session_dir.join(format!("{NO_LIMITS_MARKER_PREFIX}{provider_session}")))
        .is_ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SESSION: &str = "11112222-3333-4444-5555-666677778888";

    fn context<'a>(config_dir: &'a Path, session_dir: Option<&'a Path>) -> RelayContext<'a> {
        RelayContext {
            session: SESSION,
            config_dir,
            session_dir,
            runtime_generation: Some(3),
            received_at_ms: 1_793_481_752_042,
        }
    }

    fn captured() -> Value {
        json!({
            "session_id": "b08fea1d-0871-4dd1-acbb-f05e3b9d348b",
            "transcript_path": "C:\\Users\\u\\.claude\\projects\\p\\b08fea1d.jsonl",
            "cwd": "C:\\dev\\paneflow",
            "model": {"id": "claude-opus-5-5", "display_name": "Opus 5.5"},
            "version": "2.1.316",
            "cost": {"total_cost_usd": 0.83, "total_api_duration_ms": 50_442},
            "context_window": {"total_input_tokens": 108_045},
            "rate_limits": {
                "five_hour": {"used_percentage": 13, "resets_at": 1_793_492_400},
                "seven_day": {"used_percentage": 59, "resets_at": 1_793_527_200}
            }
        })
    }

    #[test]
    fn the_frame_carries_only_the_session_directory_limits_and_time() {
        let config_dir = Path::new("/home/u/.claude-work");
        let params = relay_params(captured(), &context(config_dir, None)).expect("a frame");
        assert_eq!(
            params,
            json!({
                "session": SESSION,
                "kind": "status_line",
                "tool": "claude",
                "emitted_at_ms": 1_793_481_752_042_u64,
                "runtime_generation": 3,
                "hook_payload": {
                    "session_id": "b08fea1d-0871-4dd1-acbb-f05e3b9d348b",
                    "config_dir": config_dir.display().to_string(),
                    "rate_limits": {
                        "five_hour": {"used_percentage": 13, "resets_at": 1_793_492_400},
                        "seven_day": {"used_percentage": 59, "resets_at": 1_793_527_200}
                    },
                    "received_at_ms": 1_793_481_752_042_u64,
                }
            })
        );
        let encoded = params.to_string();
        for leaked in [
            "transcript_path",
            "total_cost_usd",
            "paneflow",
            "Opus",
            "context_window",
        ] {
            assert!(!encoded.contains(leaked), "{leaked} leaked into {encoded}");
        }
    }

    #[test]
    fn a_session_without_an_answer_sends_nothing() {
        let mut input = captured();
        input.as_object_mut().unwrap().remove("rate_limits");
        input["cost"]["total_api_duration_ms"] = json!(0);
        let dir = tempfile::tempdir().unwrap();
        assert!(relay_params(input.clone(), &context(Path::new("/c"), Some(dir.path()))).is_none());
    }

    #[test]
    fn missing_limits_after_an_answer_are_reported_once_per_session() {
        let mut input = captured();
        input["rate_limits"] = Value::Null;
        let dir = tempfile::tempdir().unwrap();
        let first = relay_params(input.clone(), &context(Path::new("/c"), Some(dir.path())))
            .expect("a first null report");
        assert_eq!(first["hook_payload"]["rate_limits"], Value::Null);
        assert!(relay_params(input.clone(), &context(Path::new("/c"), Some(dir.path()))).is_none());

        input["session_id"] = json!("another-session");
        assert!(relay_params(input.clone(), &context(Path::new("/c"), Some(dir.path()))).is_some());
        assert!(
            relay_params(input.clone(), &context(Path::new("/c"), None)).is_none(),
            "without a session directory the once-only rule cannot hold, so nothing is sent"
        );
        input["session_id"] = json!("../escape");
        assert!(relay_params(input.clone(), &context(Path::new("/c"), Some(dir.path()))).is_none());
    }

    #[test]
    fn invalid_or_oversized_input_is_rejected() {
        assert!(parse_input(b"not json").is_none());
        assert!(parse_input(b"[1,2]").is_none());
        assert!(parse_input(b"").is_none());
        assert!(parse_input(&vec![b' '; MAX_STDIN_BYTES + 1]).is_none());
        assert!(parse_input(br#"{"session_id":"a"}"#).is_some());
    }
}
