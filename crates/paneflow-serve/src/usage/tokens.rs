use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ledger::Calendar;

pub const MAX_PASS_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_CURSORS: usize = 200;
pub const MAX_SEEN_IDS: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptKind {
    Claude,
    Codex,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCursor {
    pub offset: u64,
    #[serde(default)]
    pub touched_ms: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skipping: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub totals: Option<(u64, u64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seen_day: Option<NaiveDate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub seen: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenEntry {
    pub day: NaiveDate,
    pub input: u64,
    pub output: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenPass {
    pub path: String,
    pub start: TokenCursor,
    pub cursor: TokenCursor,
    pub entries: Vec<TokenEntry>,
}

pub fn read_tokens(
    path: &Path,
    kind: TranscriptKind,
    start: TokenCursor,
    calendar: &impl Calendar,
    now_ms: u64,
) -> Option<TokenPass> {
    let (mut file, metadata) = paneflow_home::open_regular_for_reading(path).ok()?;
    let mut cursor = start.clone();
    if metadata.len() < cursor.offset {
        cursor.offset = 0;
        cursor.skipping = false;
    }
    file.seek(SeekFrom::Start(cursor.offset)).ok()?;
    let mut chunk = Vec::new();
    file.take(MAX_PASS_BYTES).read_to_end(&mut chunk).ok()?;
    let today = calendar.day(now_ms);
    if cursor.seen_day != Some(today) {
        cursor.seen_day = Some(today);
        cursor.seen.clear();
    }
    let mut seen: HashSet<String> = cursor.seen.iter().cloned().collect();
    let mut entries = Vec::new();
    let mut at = 0;
    if cursor.skipping {
        match chunk.iter().position(|byte| *byte == b'\n') {
            Some(end) => {
                at = end + 1;
                cursor.skipping = false;
            }
            None => at = chunk.len(),
        }
    }
    while at < chunk.len() {
        let Some(length) = chunk[at..].iter().position(|byte| *byte == b'\n') else {
            if chunk.len() - at > MAX_LINE_BYTES {
                cursor.skipping = true;
                at = chunk.len();
            }
            break;
        };
        let line = &chunk[at..at + length];
        at += length + 1;
        if line.len() > MAX_LINE_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(line.trim_ascii()) else {
            continue;
        };
        let entry = match kind {
            TranscriptKind::Claude => claude_entry(&value, &mut seen, &mut cursor, today),
            TranscriptKind::Codex => codex_entry(&value, &mut cursor),
        };
        if let Some((at_ms, input, output)) = entry
            && (input > 0 || output > 0)
        {
            entries.push(TokenEntry {
                day: calendar.day(at_ms),
                input,
                output,
            });
        }
    }
    cursor.offset += at as u64;
    cursor.touched_ms = now_ms;
    Some(TokenPass {
        path: path.display().to_string(),
        start,
        cursor,
        entries,
    })
}

fn entry_ms(value: &Value) -> Option<u64> {
    let stamp = value.get("timestamp")?.as_str()?;
    let parsed = chrono::DateTime::parse_from_rfc3339(stamp).ok()?;
    u64::try_from(parsed.timestamp_millis()).ok()
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

fn claude_entry(
    value: &Value,
    seen: &mut HashSet<String>,
    cursor: &mut TokenCursor,
    today: NaiveDate,
) -> Option<(u64, u64, u64)> {
    if value.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let message = value.get("message")?;
    let id = message.get("id")?.as_str()?;
    let usage = message.get("usage").filter(|usage| usage.is_object())?;
    let at_ms = entry_ms(value)?;
    if !seen.insert(id.to_string()) {
        return None;
    }
    if cursor.seen_day == Some(today) {
        if cursor.seen.len() >= MAX_SEEN_IDS {
            cursor.seen.remove(0);
        }
        cursor.seen.push(id.to_string());
    }
    Some((
        at_ms,
        count(usage, "input_tokens"),
        count(usage, "output_tokens"),
    ))
}

fn codex_entry(value: &Value, cursor: &mut TokenCursor) -> Option<(u64, u64, u64)> {
    if value.get("type").and_then(Value::as_str) != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if payload.get("type").and_then(Value::as_str) != Some("token_count") {
        return None;
    }
    let total = payload
        .get("info")
        .filter(|info| info.is_object())?
        .get("total_token_usage")
        .filter(|total| total.is_object())?;
    let at_ms = entry_ms(value)?;
    let input = count(total, "input_tokens").saturating_sub(count(total, "cached_input_tokens"));
    let output = count(total, "output_tokens");
    let (last_input, last_output) = cursor.totals.unwrap_or((0, 0));
    cursor.totals = Some((last_input.max(input), last_output.max(output)));
    Some((
        at_ms,
        input.saturating_sub(last_input),
        output.saturating_sub(last_output),
    ))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use chrono::FixedOffset;
    use serde_json::json;

    use super::*;
    use crate::usage::ledger::ZoneCalendar;

    const NOW_MS: u64 = 1_793_481_752_042;

    fn calendar() -> ZoneCalendar<FixedOffset> {
        ZoneCalendar(FixedOffset::east_opt(3600).unwrap())
    }

    fn assistant(id: &str, stamp: &str, input: u64, output: u64) -> String {
        json!({
            "type": "assistant",
            "timestamp": stamp,
            "message": {
                "id": id,
                "role": "assistant",
                "usage": {
                    "input_tokens": input,
                    "cache_creation_input_tokens": 36_935,
                    "cache_read_input_tokens": 30_180,
                    "output_tokens": output,
                },
            },
        })
        .to_string()
    }

    fn token_count(stamp: &str, info: Option<(u64, u64, u64)>) -> String {
        json!({
            "timestamp": stamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": info.map(|(input, cached, output)| json!({
                    "total_token_usage": {
                        "input_tokens": input,
                        "cached_input_tokens": cached,
                        "output_tokens": output,
                    },
                })),
            },
        })
        .to_string()
    }

    fn append(path: &Path, lines: &[String]) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn pass(path: &Path, kind: TranscriptKind, cursor: TokenCursor) -> TokenPass {
        read_tokens(path, kind, cursor, &calendar(), NOW_MS).expect("a readable transcript")
    }

    fn sum(pass: &TokenPass) -> (u64, u64) {
        pass.entries.iter().fold((0, 0), |(input, output), entry| {
            (input + entry.input, output + entry.output)
        })
    }

    #[test]
    fn claude_counts_each_message_once_without_its_cache_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        append(
            &path,
            &[
                json!({"type": "user", "timestamp": "2026-10-31T21:00:00Z"}).to_string(),
                assistant("msg_1", "2026-10-31T21:00:01Z", 2, 172),
                assistant("msg_1", "2026-10-31T21:00:02Z", 2, 172),
                assistant("msg_2", "2026-10-31T23:30:00Z", 10, 40),
            ],
        );
        let first = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert_eq!(sum(&first), (12, 212));
        assert_eq!(
            first
                .entries
                .iter()
                .map(|entry| entry.day)
                .collect::<Vec<_>>(),
            [
                NaiveDate::from_ymd_opt(2026, 10, 31).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 1).unwrap(),
            ],
            "each entry counts for the local day of its timestamp"
        );
        assert_eq!(first.cursor.offset, std::fs::metadata(&path).unwrap().len());

        append(&path, &[assistant("msg_3", "2026-11-01T00:10:00Z", 1, 5)]);
        let second = pass(&path, TranscriptKind::Claude, first.cursor.clone());
        assert_eq!(sum(&second), (1, 5), "only the appended bytes are read");
    }

    #[test]
    fn a_truncated_claude_transcript_restarts_without_counting_twice_today() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        append(
            &path,
            &[
                assistant("msg_1", "2026-11-01T00:00:01Z", 5, 50),
                assistant("msg_2", "2026-11-01T00:00:02Z", 5, 50),
            ],
        );
        let first = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert_eq!(sum(&first), (10, 100));
        std::fs::write(&path, "").unwrap();
        append(
            &path,
            &[
                assistant("msg_2", "2026-11-01T00:00:02Z", 5, 50),
                assistant("msg_9", "2026-11-01T00:05:00Z", 1, 1),
            ],
        );
        let second = pass(&path, TranscriptKind::Claude, first.cursor);
        assert_eq!(sum(&second), (1, 1));
    }

    #[test]
    fn codex_adds_the_growth_of_its_cumulative_totals_minus_cached_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollout-a.jsonl");
        append(
            &path,
            &[
                token_count("2026-10-31T22:08:40Z", None),
                token_count("2026-10-31T22:08:49Z", Some((25_891, 10_752, 5))),
                token_count("2026-10-31T22:09:10Z", Some((40_000, 20_000, 300))),
            ],
        );
        let first = pass(&path, TranscriptKind::Codex, TokenCursor::default());
        assert_eq!(sum(&first), (20_000, 300));
        assert_eq!(first.entries.len(), 2, "the first info: null is ignored");

        append(
            &path,
            &[token_count(
                "2026-10-31T22:20:00Z",
                Some((41_000, 20_500, 320)),
            )],
        );
        let second = pass(&path, TranscriptKind::Codex, first.cursor);
        assert_eq!(sum(&second), (500, 20));
    }

    #[test]
    fn invalid_and_oversized_lines_are_skipped_and_the_read_goes_on() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let huge = format!("{{\"pad\":\"{}\"}}", "x".repeat(MAX_LINE_BYTES + 10));
        append(
            &path,
            &[
                "{not json".into(),
                huge,
                assistant("msg_1", "2026-11-01T00:00:01Z", 3, 4),
            ],
        );
        let read = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert_eq!(sum(&read), (3, 4));
    }

    #[test]
    fn a_pass_reads_at_most_eight_megabytes_and_the_rest_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let filler = format!("{{\"pad\":\"{}\"}}", "x".repeat(512 * 1024));
        let mut lines: Vec<String> = (0..17).map(|_| filler.clone()).collect();
        lines.push(assistant("msg_late", "2026-11-01T00:00:01Z", 7, 8));
        append(&path, &lines);
        let first = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert!(first.cursor.offset <= MAX_PASS_BYTES);
        assert_eq!(sum(&first), (0, 0));
        let second = pass(&path, TranscriptKind::Claude, first.cursor);
        assert_eq!(sum(&second), (7, 8));
        assert_eq!(
            second.cursor.offset,
            std::fs::metadata(&path).unwrap().len()
        );
    }

    #[test]
    fn a_line_still_being_written_waits_for_its_newline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let line = assistant("msg_1", "2026-11-01T00:00:01Z", 3, 4);
        std::fs::write(&path, &line[..line.len() / 2]).unwrap();
        let partial = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert_eq!(partial.cursor.offset, 0);
        std::fs::write(&path, format!("{line}\n")).unwrap();
        let whole = pass(&path, TranscriptKind::Claude, partial.cursor);
        assert_eq!(sum(&whole), (3, 4));
    }

    #[test]
    fn an_overlong_line_without_a_newline_is_skipped_across_passes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "y".repeat(MAX_LINE_BYTES + 1)).unwrap();
        let first = pass(&path, TranscriptKind::Claude, TokenCursor::default());
        assert!(first.cursor.skipping);
        append(
            &path,
            &[
                "tail-of-the-long-line".into(),
                assistant("msg_1", "2026-11-01T00:00:01Z", 3, 4),
            ],
        );
        let second = pass(&path, TranscriptKind::Claude, first.cursor);
        assert!(!second.cursor.skipping);
        assert_eq!(sum(&second), (3, 4));
    }
}
