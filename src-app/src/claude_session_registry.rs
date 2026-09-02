use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ai_types::AgentLifecycleEvent;

const MAX_RECORD_BYTES: u64 = 64 * 1024;

const MAX_RECORDS: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeSessionStatus {
    Busy,
    Shell,
    Waiting,
    Idle,
}

impl ClaudeSessionStatus {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "busy" => Some(Self::Busy),
            "shell" => Some(Self::Shell),
            "waiting" => Some(Self::Waiting),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSessionRecord {
    pub pid: u32,
    pub status: ClaudeSessionStatus,
    pub waiting_for: Option<String>,
    pub proc_start: Option<String>,
}

impl ClaudeSessionRecord {
    pub fn lifecycle_event(&self) -> AgentLifecycleEvent {
        match self.status {
            ClaudeSessionStatus::Busy | ClaudeSessionStatus::Shell => AgentLifecycleEvent::Working,
            ClaudeSessionStatus::Waiting => AgentLifecycleEvent::Notification {
                message: self.waiting_for.clone(),
            },
            ClaudeSessionStatus::Idle => AgentLifecycleEvent::Idle,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default, rename = "waitingFor")]
    waiting_for: Option<String>,
    #[serde(default, rename = "procStart")]
    proc_start: Option<String>,
    #[serde(default, rename = "procStartFt")]
    proc_start_ft: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

const MAX_WAITING_REASON_CHARS: usize = 256;

fn clamp_waiting_reason(raw: Option<String>) -> Option<String> {
    let bounded: String = raw?.trim().chars().take(MAX_WAITING_REASON_CHARS).collect();
    let clean = crate::markdown::strip_bidi_zero_width(bounded);
    (!clean.trim().is_empty()).then_some(clean)
}

pub fn parse_record(bytes: &[u8], file_pid: u32) -> Option<ClaudeSessionRecord> {
    let raw: RawRecord = serde_json::from_slice(bytes).ok()?;
    if raw.pid.is_some_and(|pid| pid != file_pid) {
        return None;
    }
    if raw
        .kind
        .as_deref()
        .is_some_and(|kind| kind != "interactive")
    {
        return None;
    }
    let status = ClaudeSessionStatus::parse(raw.status.as_deref()?)?;
    Some(ClaudeSessionRecord {
        pid: file_pid,
        status,
        waiting_for: clamp_waiting_reason(raw.waiting_for),
        proc_start: raw.proc_start_ft.or(raw.proc_start),
    })
}

pub fn pid_from_file_name(name: &str) -> Option<u32> {
    name.strip_suffix(".json")?.parse().ok()
}

pub fn sessions_dir() -> Option<PathBuf> {
    let base = match std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(explicit) => explicit,
        None => dirs::home_dir()?.join(".claude"),
    };
    Some(base.join("sessions"))
}

pub fn read_live_sessions(dir: &Path) -> Vec<ClaudeSessionRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten().take(MAX_RECORDS) {
        let Some(pid) = entry.file_name().to_str().and_then(pid_from_file_name) else {
            continue;
        };
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_RECORD_BYTES)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if let Some(record) = parse_record(&bytes, pid) {
            records.push(record);
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL_RECORD: &str = r#"{"pid":14404,"sessionId":"517dd24b-54a9-47e9-b512-bc121e09408e",
        "cwd":"C:\\dev\\paneflow","startedAt":1787915941365,
        "procStart":"134323895399231254","version":"2.1.250","peerProtocol":1,
        "peerFeatures":["notify_idle","artifact_yield"],"kind":"interactive",
        "entrypoint":"cli","name":"paneflow-f4","nameSource":"derived",
        "status":"busy","updatedAt":1787916617655,"statusUpdatedAt":1787916617655}"#;

    #[test]
    fn parses_the_real_record_shape() {
        let record = parse_record(REAL_RECORD.as_bytes(), 14404).expect("real record parses");
        assert_eq!(record.pid, 14404);
        assert_eq!(record.status, ClaudeSessionStatus::Busy);
        assert_eq!(record.proc_start.as_deref(), Some("134323895399231254"));
        assert_eq!(record.waiting_for, None);
    }

    #[test]
    fn every_status_maps_to_the_event_it_implies() {
        let event = |status: &str, waiting: &str| {
            let body = format!(r#"{{"status":"{status}","waitingFor":"{waiting}"}}"#);
            parse_record(body.as_bytes(), 7)
                .expect("known status parses")
                .lifecycle_event()
        };
        assert_eq!(event("busy", ""), AgentLifecycleEvent::Working);
        assert_eq!(event("shell", ""), AgentLifecycleEvent::Working);
        assert_eq!(event("idle", ""), AgentLifecycleEvent::Idle);
        assert_eq!(
            event("waiting", "input needed"),
            AgentLifecycleEvent::Notification {
                message: Some("input needed".into())
            }
        );
    }

    #[test]
    fn an_unknown_or_absent_status_says_nothing_rather_than_guessing() {
        assert!(parse_record(br#"{"status":"parked"}"#, 7).is_none());
        assert!(parse_record(br#"{"pid":7}"#, 7).is_none());
        assert!(parse_record(b"", 7).is_none());
        assert!(parse_record(b"{ truncated", 7).is_none());
    }

    #[test]
    fn a_record_that_is_not_this_pane_is_refused() {
        assert!(parse_record(br#"{"pid":99,"status":"busy"}"#, 7).is_none());
        for kind in ["bg", "daemon", "daemon-worker"] {
            let body = format!(r#"{{"status":"busy","kind":"{kind}"}}"#);
            assert!(
                parse_record(body.as_bytes(), 7).is_none(),
                "{kind} sessions must not claim a surface"
            );
        }
        assert!(parse_record(br#"{"status":"busy"}"#, 7).is_some());
    }

    #[test]
    fn the_newer_start_time_field_wins_and_both_are_optional() {
        let record = parse_record(
            br#"{"status":"idle","procStart":"old","procStartFt":"new"}"#,
            7,
        )
        .expect("parses");
        assert_eq!(record.proc_start.as_deref(), Some("new"));
        let record = parse_record(br#"{"status":"idle"}"#, 7).expect("parses");
        assert_eq!(record.proc_start, None);
    }

    #[test]
    fn wait_reasons_are_trimmed_bounded_and_never_blank() {
        assert_eq!(clamp_waiting_reason(None), None);
        assert_eq!(clamp_waiting_reason(Some("   ".into())), None);
        assert_eq!(
            clamp_waiting_reason(Some("  input needed  ".into())).as_deref(),
            Some("input needed")
        );
        let long = clamp_waiting_reason(Some("é".repeat(MAX_WAITING_REASON_CHARS * 2)))
            .expect("a long reason is kept, not dropped");
        assert_eq!(
            long.chars().count(),
            MAX_WAITING_REASON_CHARS,
            "the bound counts characters, so it cannot split a multi-byte one"
        );
        assert_eq!(
            clamp_waiting_reason(Some("in\u{202e}put\u{200b} needed".into())).as_deref(),
            Some("input needed"),
            "bidi and zero-width controls must not reach the sidebar"
        );
        assert_eq!(clamp_waiting_reason(Some("\u{200b}\u{202e}".into())), None);
    }

    #[test]
    fn only_session_json_files_name_a_pid() {
        assert_eq!(pid_from_file_name("14404.json"), Some(14404));
        assert_eq!(
            pid_from_file_name("14404.9d4f4756.key"),
            None,
            "a peer-token file is not a session record"
        );
        assert_eq!(pid_from_file_name("14404"), None);
        assert_eq!(pid_from_file_name("notapid.json"), None);
        assert_eq!(pid_from_file_name(".json"), None);
    }

    #[test]
    fn a_missing_directory_is_the_empty_answer() {
        let dir = std::env::temp_dir().join("paneflow-claude-registry-absent");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(read_live_sessions(&dir).is_empty());
    }

    #[test]
    fn reads_records_and_skips_everything_it_cannot_speak_for() {
        let dir = std::env::temp_dir().join("paneflow-claude-registry-read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("14404.json"), REAL_RECORD).expect("write record");
        std::fs::write(
            dir.join("21.json"),
            br#"{"status":"waiting","waitingFor":"input needed"}"#,
        )
        .expect("write record");
        std::fs::write(dir.join("99.9d4f.key"), b"token").expect("write token");
        std::fs::write(dir.join("77.json"), b"{ truncated").expect("write junk");

        let mut records = read_live_sessions(&dir);
        records.sort_by_key(|record| record.pid);
        let pids: Vec<u32> = records.iter().map(|record| record.pid).collect();
        assert_eq!(pids, vec![21, 14404]);
        assert_eq!(records[0].status, ClaudeSessionStatus::Waiting);
        assert_eq!(records[0].waiting_for.as_deref(), Some("input needed"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
