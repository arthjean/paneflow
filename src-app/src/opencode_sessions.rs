use std::io;
use std::process::Command;

use serde_json::Value;

use crate::agent_sessions::{SessionAgent, SessionMeta, clean_session_label};

const STDERR_LOG_CAP: usize = 200;

const OPENCODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

const OPENCODE_STDOUT_CAP: u64 = 8 * 1024 * 1024;

pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_sessions_with_program("opencode", cwd)
}

fn read_sessions_with_program(program: &str, cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(stdout) = run_opencode_list(program) else {
        return (Vec::new(), 0);
    };
    parse_sessions(&stdout, cwd)
}

fn run_opencode_list(program: &str) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    cmd.args(["session", "list", "--format", "json"]);

    let output =
        match paneflow_process::run_with_timeout(cmd, OPENCODE_DEADLINE, OPENCODE_STDOUT_CAP) {
            Ok(out) => out,
            Err(paneflow_process::ProcError::Spawn(err))
                if err.kind() == io::ErrorKind::NotFound =>
            {
                log::info!("opencode binary not found on PATH; OpenCode tab will be empty");
                return None;
            }
            Err(paneflow_process::ProcError::Timeout) => {
                log::warn!("opencode session list timed out; OpenCode tab will be empty");
                return None;
            }
            Err(err) => {
                log::warn!("failed to spawn opencode: {err}");
                return None;
            }
        };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let snippet: String = stderr
            .chars()
            .take(STDERR_LOG_CAP)
            .map(|c| if c.is_control() && c != '\n' { '?' } else { c })
            .collect();
        log::warn!(
            "opencode session list exited with {}: {}",
            output.status,
            snippet
        );
        return None;
    }

    Some(output.stdout)
}

fn parse_sessions(stdout: &[u8], cwd: &str) -> (Vec<SessionMeta>, usize) {
    if stdout.is_empty() {
        return (Vec::new(), 0);
    }
    let array: Vec<Value> = match serde_json::from_slice(stdout) {
        Ok(Value::Array(arr)) => arr,
        Ok(_) | Err(_) => return (Vec::new(), 0),
    };

    let sessions = array
        .into_iter()
        .filter_map(|record| record_to_session(&record, cwd));
    crate::agent_sessions::collect_recent_sessions(
        sessions,
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    )
}

fn record_to_session(record: &Value, cwd: &str) -> Option<SessionMeta> {
    let session_id = record.get("id").and_then(|v| v.as_str())?.to_string();
    if !crate::agent_sessions::is_valid_session_id(&session_id) {
        return None;
    }
    let record_cwd = record
        .get("directory")
        .and_then(|v| v.as_str())?
        .to_string();
    if !crate::agent_sessions::cwd_matches(&record_cwd, cwd) {
        return None;
    }

    let timestamp_ms = record
        .get("updated")
        .and_then(value_as_i64)
        .or_else(|| record.get("created").and_then(value_as_i64))
        .unwrap_or(0);
    let timestamp = unix_ms_to_iso8601(timestamp_ms);

    let summary = record
        .get("title")
        .and_then(|v| v.as_str())
        .and_then(|title| clean_session_label(title, 80));

    Some(SessionMeta {
        agent: SessionAgent::OpenCode,
        session_id,
        timestamp,
        cwd: record_cwd,
        git_branch: String::new(),
        summary,
        model: None,
        usage: None,
    })
}

fn value_as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

fn unix_ms_to_iso8601(ms: i64) -> String {
    let secs = ms.div_euclid(1_000);
    let days_since_epoch = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/opencode-session-list.json");

    #[test]
    fn parse_sessions_happy_path_extracts_real_cli_record() {
        let (sessions, omitted) = parse_sessions(FIXTURE.as_bytes(), "/home/arthur");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1, "fixture has one record at /home/arthur");
        let meta = &sessions[0];
        assert_eq!(meta.agent, SessionAgent::OpenCode);
        assert_eq!(meta.session_id, "ses_1f80d49aeffeaKV4Lq4mc0c3cu");
        assert_eq!(meta.cwd, "/home/arthur");
        assert!(meta.git_branch.is_empty());
        assert_eq!(
            meta.summary.as_deref(),
            Some("New session - 2026-05-08T14:16:47.441Z")
        );
        assert_eq!(meta.timestamp, "2026-05-08T14:16:47Z");
    }

    #[test]
    fn parse_sessions_filters_by_cwd_and_sorts_descending() {
        let multi = br#"[
            {"id":"a","directory":"/p","title":"older","updated":1000},
            {"id":"b","directory":"/p","title":"newer","updated":2000},
            {"id":"c","directory":"/elsewhere","title":"other","updated":9000}
        ]"#;
        let (sessions, omitted) = parse_sessions(multi, "/p");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 2, "the /elsewhere record must be filtered");
        assert_eq!(sessions[0].session_id, "b", "newer first");
        assert_eq!(sessions[1].session_id, "a", "older second");
    }

    #[test]
    fn parse_sessions_normalizes_title_labels() {
        let payload = br#"[
            {"id":"ses_clean","directory":"/p","title":"  messy\n\tlabel\u001b  ","updated":1000}
        ]"#;
        let (sessions, omitted) = parse_sessions(payload, "/p");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summary.as_deref(), Some("messy label"));
    }

    #[test]
    fn parse_sessions_rejects_session_id_with_carriage_return() {
        let payload = br#"[
            {"id":"ses_abc\rrm -rf /","directory":"/p","title":"evil","updated":1000},
            {"id":"ses_clean","directory":"/p","title":"ok","updated":2000}
        ]"#;
        let (sessions, omitted) = parse_sessions(payload, "/p");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1, "the \\r-tainted record must be dropped");
        assert_eq!(sessions[0].session_id, "ses_clean");
    }

    #[test]
    fn parse_sessions_skips_records_missing_id() {
        let mixed = br#"[
            {"directory":"/p","title":"no id here","updated":1000},
            {"id":"keepme","directory":"/p","title":"valid","updated":2000}
        ]"#;
        let (sessions, omitted) = parse_sessions(mixed, "/p");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "keepme");
    }

    #[test]
    fn parse_sessions_handles_empty_stdout() {
        let (sessions, omitted) = parse_sessions(b"", "/anywhere");
        assert_eq!(omitted, 0);
        assert!(sessions.is_empty());
    }

    #[test]
    fn parse_sessions_handles_malformed_json() {
        let (sessions, omitted) = parse_sessions(b"{not valid json", "/anywhere");
        assert_eq!(omitted, 0);
        assert!(sessions.is_empty());
    }

    #[test]
    fn read_sessions_returns_empty_when_binary_missing() {
        let (sessions, omitted) =
            read_sessions_with_program("opencode-does-not-exist-zzz-9d2c1a", "/home/arthur");
        assert_eq!(omitted, 0);
        assert!(sessions.is_empty());
    }

    #[test]
    fn unix_ms_to_iso8601_matches_known_epoch() {
        assert_eq!(
            unix_ms_to_iso8601(1_736_944_245_000),
            "2025-01-15T12:30:45Z"
        );
    }

    #[test]
    fn value_as_i64_rejects_u64_overflow() {
        assert_eq!(value_as_i64(&serde_json::json!(i64::MAX as u64 + 1)), None);
    }
}
