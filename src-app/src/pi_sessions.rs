use std::collections::VecDeque;
use std::fs;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent_sessions::{CappedLine, SessionAgent, SessionMeta, clean_session_label};
use crate::limits::MAX_LINE_BYTES;

const MAX_WALK_DEPTH: usize = 10;
const MAX_HEADER_LINES: usize = 256;
const MAX_WALK_ENTRIES: usize = 10_000;
const MAX_DISCOVERY_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) fn sessions_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".pi").join("agent").join("sessions"))
}

pub(crate) fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(root) = sessions_root() else {
        return (Vec::new(), 0);
    };
    read_sessions_under_root(&root, cwd)
}

fn read_sessions_under_root(root: &Path, cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_sessions_within(root, cwd, MAX_WALK_ENTRIES, MAX_DISCOVERY_BYTES)
}

fn read_sessions_within(
    root: &Path,
    cwd: &str,
    max_entries: usize,
    max_bytes: u64,
) -> (Vec<SessionMeta>, usize) {
    if !root.is_dir() {
        return (Vec::new(), 0);
    }
    let paths = jsonl_files(root, max_entries);
    let mut budget = max_bytes;
    let sessions = paths
        .iter()
        .map_while(|path| (budget > 0).then(|| read_session_meta(path, &mut budget)))
        .flatten()
        .filter(|meta| crate::agent_sessions::cwd_matches(&meta.cwd, cwd));
    crate::agent_sessions::collect_recent_sessions(
        sessions,
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    )
}

fn jsonl_files(root: &Path, max_entries: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut visited = 0usize;
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    'walk: while let Some((dir, depth)) = queue.pop_front() {
        if depth > MAX_WALK_DEPTH {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited > max_entries {
                log::warn!(
                    target: "paneflow_app::pi_sessions",
                    "stopped the session walk under {} after {max_entries} entries",
                    root.display()
                );
                break 'walk;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                queue.push_back((path, depth + 1));
            } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
                let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
                out.push((modified, path));
            }
        }
    }
    out.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    out.into_iter().map(|(_, path)| path).collect()
}

fn read_session_meta(path: &Path, budget: &mut u64) -> Option<SessionMeta> {
    let mut reader = BufReader::new(crate::agent_sessions::open_session_file(path)?);
    let mut header: Option<PiHeader> = None;
    let mut summary: Option<String> = None;

    for _ in 0..MAX_HEADER_LINES {
        if *budget == 0 {
            break;
        }
        let line = match crate::agent_sessions::read_capped_line(&mut reader, *budget) {
            Ok(CappedLine::Eof) => break,
            Ok(CappedLine::Oversized(consumed)) => {
                *budget = budget.saturating_sub(consumed);
                log::debug!(
                    target: "paneflow_app::pi_sessions",
                    "skipped an oversized (>{} B) line in {}; continuing scan for the session header",
                    MAX_LINE_BYTES,
                    path.display(),
                );
                continue;
            }
            Ok(CappedLine::Line(line, consumed)) => {
                *budget = budget.saturating_sub(consumed);
                line
            }
            Err(error) => {
                crate::agent_sessions::log_unreadable_session(path, &error);
                break;
            }
        };
        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        if header.is_none() {
            header = PiHeader::from_value(&value);
        }
        if summary.is_none() {
            summary = user_summary_from_value(&value);
        }
        if header.is_some() && summary.is_some() {
            break;
        }
    }

    let header = header?;
    Some(SessionMeta {
        agent: SessionAgent::Pi,
        session_id: header.id,
        timestamp: header.timestamp,
        cwd: header.cwd,
        summary,
    })
}

struct PiHeader {
    id: String,
    timestamp: String,
    cwd: String,
}

impl PiHeader {
    fn from_value(value: &Value) -> Option<Self> {
        if value.get("type").and_then(Value::as_str) != Some("session") {
            return None;
        }
        let id = value.get("id").and_then(Value::as_str)?.to_string();
        let cwd = value.get("cwd").and_then(Value::as_str)?.to_string();
        if !crate::agent_sessions::is_valid_session_id(&id)
            || cwd.is_empty()
            || cwd.chars().any(char::is_control)
        {
            return None;
        }
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        Some(Self { id, timestamp, cwd })
    }
}

fn user_summary_from_value(value: &Value) -> Option<String> {
    let role = value
        .get("message")
        .and_then(|message| message.get("role"))
        .or_else(|| value.get("role"))
        .and_then(Value::as_str)?;
    if role != "user" {
        return None;
    }
    let content = value
        .get("message")
        .and_then(|message| message.get("content"))
        .or_else(|| value.get("content"))?;
    content_to_string(content).and_then(|s| clean_session_label(&s, 120))
}

fn content_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Array(items) => {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = item
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| item.as_str())
                {
                    let text = text.trim();
                    if !text.is_empty() {
                        parts.push(text);
                    }
                }
            }
            (!parts.is_empty()).then(|| parts.join(" "))
        }
        Value::Object(obj) => obj
            .get("text")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_documented_pi_jsonl_header_and_filters_by_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("nested");
        fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("session.jsonl");
        fs::write(
            &path,
            concat!(
                r#"{"type":"session","version":3,"id":"550e8400-e29b-41d4-a716-446655440000","timestamp":"2026-06-29T09:10:11Z","cwd":"/repo"}"#,
                "\n",
                r#"{"type":"message","message":{"role":"user","content":"Ship the sidebar sessions"}} "#
            ),
        )
        .unwrap();

        let (sessions, omitted) = read_sessions_under_root(dir.path(), "/repo");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].agent, SessionAgent::Pi);
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Ship the sidebar sessions")
        );
        assert!(read_sessions_under_root(dir.path(), "/other").0.is_empty());
    }

    #[test]
    fn skips_oversized_leading_line_and_reads_following_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let oversized = format!(
            r#"{{"type":"noise","blob":"{}"}}"#,
            "x".repeat(MAX_LINE_BYTES as usize + 1024)
        );
        fs::write(
            &path,
            format!(
                "{oversized}\n{}\n{}\n",
                r#"{"type":"session","version":3,"id":"550e8400-e29b-41d4-a716-446655440000","timestamp":"2026-06-29T09:10:11Z","cwd":"/repo"}"#,
                r#"{"type":"message","message":{"role":"user","content":"Still readable"}}"#
            ),
        )
        .unwrap();

        let (sessions, omitted) = read_sessions_under_root(dir.path(), "/repo");
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summary.as_deref(), Some("Still readable"));
    }

    #[test]
    fn user_summary_collapses_controls_and_whitespace() {
        let value: Value = serde_json::json!({
            "type": "message",
            "message": {
                "role": "user",
                "content": "  Ship\n\tthis\u{1b} now  "
            }
        });

        assert_eq!(
            user_summary_from_value(&value).as_deref(),
            Some("Ship this now")
        );
    }

    #[test]
    fn drops_header_with_unsafe_session_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        fs::write(
            &path,
            r#"{"type":"session","version":3,"id":"ses_x; rm -rf ~","timestamp":"2026-06-29T09:10:11Z","cwd":"/repo"}"#,
        )
        .unwrap();
        assert!(read_sessions_under_root(dir.path(), "/repo").0.is_empty());
    }

    const HEADER: &str = r#"{"type":"session","version":3,"id":"550e8400-e29b-41d4-a716-446655440000","timestamp":"2026-06-29T09:10:11Z","cwd":"/repo"}"#;

    fn write_session(path: &Path, message: &str) {
        fs::write(
            path,
            format!(
                "{HEADER}\n{{\"type\":\"message\",\"message\":{{\"role\":\"user\",\"content\":\"{message}\"}}}}\n"
            ),
        )
        .unwrap();
    }

    #[test]
    fn an_emoji_cut_at_the_line_cap_keeps_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = r#"{"type":"noise","blob":""#;
        let pad = MAX_LINE_BYTES as usize - prefix.len() - 2;
        let split = format!("{prefix}{}😀\"}}", "x".repeat(pad));
        fs::write(
            dir.path().join("session.jsonl"),
            format!(
                "{HEADER}\n{split}\n{}\n",
                r#"{"type":"message","message":{"role":"user","content":"After the emoji"}}"#
            ),
        )
        .unwrap();
        let (sessions, _) = read_sessions_under_root(dir.path(), "/repo");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summary.as_deref(), Some("After the emoji"));
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_session_file_leaves_the_others_listed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        write_session(&dir.path().join("good.jsonl"), "kept");
        let locked = dir.path().join("locked.jsonl");
        write_session(&locked, "locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::File::open(&locked).is_ok() {
            return;
        }
        let (sessions, _) = read_sessions_under_root(dir.path(), "/repo");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].summary.as_deref(), Some("kept"));
    }

    #[test]
    fn discovery_stops_at_its_entry_and_byte_caps() {
        let dir = tempfile::tempdir().unwrap();
        for index in 0..6 {
            write_session(&dir.path().join(format!("{index}.jsonl")), "s");
        }
        assert_eq!(jsonl_files(dir.path(), 4).len(), 4);
        assert_eq!(jsonl_files(dir.path(), 10).len(), 6);

        let one_file = fs::metadata(dir.path().join("0.jsonl")).unwrap().len();
        let (sessions, omitted) = read_sessions_within(dir.path(), "/repo", 10, one_file * 2);
        assert_eq!(
            sessions.len() + omitted,
            2,
            "no file is read once the byte budget is spent"
        );
    }
}
