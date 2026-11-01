use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use paneflow_agent_config::usage::model::{PlanLimits, plausible_codex_rate_limits};
use serde_json::Value;

pub const ROLLOUT_TAIL_BYTES: u64 = 256 * 1024;
pub const CODEX_READ_FLOOR: Duration = Duration::from_secs(30);
pub const CODEX_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);
const RECENT_DAY_DIRS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodexReading {
    pub limits: PlanLimits,
    pub observed_at_ms: u64,
}

pub fn read_latest_limits(codex_home: &Path, now_secs: u64) -> Option<CodexReading> {
    let rollout = latest_rollout(&codex_home.join("sessions"))?;
    let (tail, from_start, modified_ms) = read_tail(&rollout)?;
    last_rate_limits(&tail, from_start, now_secs).map(|(limits, observed_at_ms)| CodexReading {
        limits,
        observed_at_ms: observed_at_ms.unwrap_or(modified_ms),
    })
}

pub fn latest_rollout_in(codex_home: &Path) -> Option<PathBuf> {
    latest_rollout(&codex_home.join("sessions"))
}

fn numbered_children(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut children: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let number = entry.file_name().to_str()?.parse::<u32>().ok()?;
            Some((number, entry.path()))
        })
        .collect();
    children.sort_by_key(|child| std::cmp::Reverse(child.0));
    children.into_iter().map(|(_, path)| path).collect()
}

fn recent_day_dirs(sessions: &Path) -> Vec<PathBuf> {
    let mut days = Vec::new();
    for year in numbered_children(sessions) {
        for month in numbered_children(&year) {
            for day in numbered_children(&month) {
                days.push(day);
                if days.len() == RECENT_DAY_DIRS {
                    return days;
                }
            }
        }
    }
    days
}

fn is_rollout(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn latest_rollout(sessions: &Path) -> Option<PathBuf> {
    recent_day_dirs(sessions)
        .iter()
        .filter_map(|day| std::fs::read_dir(day).ok())
        .flat_map(|entries| entries.flatten())
        .filter(|entry| entry.file_name().to_str().is_some_and(is_rollout))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            metadata
                .is_file()
                .then(|| Some((metadata.modified().ok()?, entry.path())))
                .flatten()
        })
        .max_by(|left, right| left.0.cmp(&right.0))
        .map(|(_, path)| path)
}

fn read_tail(path: &Path) -> Option<(Vec<u8>, bool, u64)> {
    let (mut file, metadata) = paneflow_home::open_regular_for_reading(path).ok()?;
    let length = metadata.len();
    let start = length.saturating_sub(ROLLOUT_TAIL_BYTES);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut tail = Vec::with_capacity(usize::try_from(length - start).unwrap_or(0));
    file.take(ROLLOUT_TAIL_BYTES).read_to_end(&mut tail).ok()?;
    let modified_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        });
    Some((tail, start == 0, modified_ms))
}

fn last_rate_limits(
    tail: &[u8],
    from_start: bool,
    now_secs: u64,
) -> Option<(PlanLimits, Option<u64>)> {
    let mut lines = tail.split(|byte| *byte == b'\n');
    if !from_start {
        lines.next();
    }
    let lines: Vec<&[u8]> = lines.collect();
    lines.into_iter().rev().find_map(|line| {
        let entry: Value = serde_json::from_slice(line.trim_ascii()).ok()?;
        if entry.get("type").and_then(Value::as_str) != Some("event_msg") {
            return None;
        }
        let payload = entry.get("payload")?;
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            return None;
        }
        let rate_limits = payload
            .get("rate_limits")
            .filter(|limits| limits.is_object())?;
        let limits = plausible_codex_rate_limits(rate_limits, now_secs);
        limits.windows().next()?;
        let observed_at_ms = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
            .and_then(|stamp| u64::try_from(stamp.timestamp_millis()).ok());
        Some((limits, observed_at_ms))
    })
}

#[cfg(test)]
mod tests {
    use paneflow_agent_config::usage::model::WindowLabel;
    use serde_json::json;

    use super::*;

    const NOW: u64 = 1_793_481_752;

    fn token_count(percent: f64, rate_limits: bool) -> String {
        let limits = rate_limits.then(|| {
            json!({
                "limit_id": "codex",
                "primary": {"used_percent": percent, "window_minutes": 43_200, "resets_at": NOW + 20 * 86_400},
                "secondary": null,
                "plan_type": "free"
            })
        });
        json!({
            "timestamp": "2026-10-31T22:09:12.042Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {"total_token_usage": {"input_tokens": 10, "output_tokens": 2}},
                "rate_limits": limits
            }
        })
        .to_string()
    }

    fn write_rollout(codex_home: &Path, day: &str, name: &str, lines: &[String]) -> PathBuf {
        let dir = codex_home
            .join("sessions")
            .join("2026")
            .join("10")
            .join(day);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    #[test]
    fn the_last_token_count_with_limits_of_the_newest_rollout_is_read_and_labelled_by_duration() {
        let codex_home = tempfile::tempdir().unwrap();
        write_rollout(
            codex_home.path(),
            "30",
            "rollout-2026-10-30T10-00-00-a.jsonl",
            &[token_count(40.0, true)],
        );
        std::thread::sleep(Duration::from_millis(20));
        write_rollout(
            codex_home.path(),
            "31",
            "rollout-2026-10-31T23-08-26-b.jsonl",
            &[
                json!({"type": "session_meta", "payload": {}}).to_string(),
                token_count(2.0, true),
                token_count(3.0, true),
                token_count(9.0, false),
                "{ not json".to_string(),
            ],
        );
        std::fs::write(
            codex_home
                .path()
                .join("sessions/2026/10/31/rollout-2026-10-31T01-00-00-c.jsonl.zst"),
            b"compressed",
        )
        .unwrap();
        let reading = read_latest_limits(codex_home.path(), NOW).expect("a reading");
        let primary = reading.limits.primary.expect("a primary window");
        assert_eq!(primary.used_percent, 3.0);
        assert_eq!(primary.label(), WindowLabel::Month);
        assert_eq!(reading.limits.secondary, None);
        assert_eq!(reading.observed_at_ms, 1_793_484_552_042);
    }

    #[test]
    fn only_the_last_quarter_megabyte_is_read() {
        let codex_home = tempfile::tempdir().unwrap();
        let padding = format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"text\":\"{}\"}}}}",
            "x".repeat(ROLLOUT_TAIL_BYTES as usize)
        );
        let path = write_rollout(
            codex_home.path(),
            "31",
            "rollout-big.jsonl",
            &[token_count(5.0, true), padding],
        );
        assert!(std::fs::metadata(&path).unwrap().len() > ROLLOUT_TAIL_BYTES);
        assert_eq!(
            read_latest_limits(codex_home.path(), NOW),
            None,
            "a token_count before the tail is out of reach"
        );
    }

    #[test]
    fn no_rollout_or_no_limits_yields_nothing() {
        let codex_home = tempfile::tempdir().unwrap();
        assert_eq!(read_latest_limits(codex_home.path(), NOW), None);
        write_rollout(
            codex_home.path(),
            "31",
            "rollout-failed.jsonl",
            &[
                json!({"type": "session_meta", "payload": {}}).to_string(),
                token_count(1.0, false),
            ],
        );
        assert_eq!(read_latest_limits(codex_home.path(), NOW), None);
    }
}
