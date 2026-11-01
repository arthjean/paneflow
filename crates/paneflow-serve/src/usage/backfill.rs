use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use chrono::NaiveDate;
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use serde::{Deserialize, Serialize};

use super::ledger::{Calendar, ImportedDays, LedgerAccount};
use super::tokens::{TokenCursor, TranscriptKind, read_tokens};

pub const MAX_IMPORT_FILES: usize = 10_000;
pub const MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const IMPORT_PACE: Duration = Duration::from_millis(1);
const PACED_FILES: u64 = 64;
const MAX_LISTED_FILES: usize = 200_000;
const MAX_WALK_DEPTH: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportStatus {
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub files: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_days: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportFile {
    pub account: LedgerAccount,
    pub path: PathBuf,
    pub kind: TranscriptKind,
    pub modified_ms: u64,
    pub len: u64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportOutcome {
    pub days: ImportedDays,
    pub files: u64,
    pub limited_days: Option<u64>,
}

pub fn import_files(accounts: &[(UsageAgent, PathBuf)]) -> Vec<ImportFile> {
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();
    for (agent, dir) in accounts {
        let dir = canonical_account_dir(dir);
        if !seen.insert((*agent, account_dir_key(&dir))) {
            continue;
        }
        let account = LedgerAccount {
            agent: agent.runtime_slug().to_string(),
            config_dir: dir.display().to_string(),
        };
        let (kind, roots, wanted): (TranscriptKind, &[&str], fn(&str) -> bool) = match agent {
            UsageAgent::ClaudeCode => (TranscriptKind::Claude, &["projects"], is_claude_transcript),
            UsageAgent::Codex => (
                TranscriptKind::Codex,
                &["sessions", "archived_sessions"],
                is_codex_rollout,
            ),
        };
        for root in roots {
            walk(&dir.join(root), 0, &mut |path, modified_ms, len| {
                let name = path.file_name().and_then(|name| name.to_str());
                if name.is_some_and(wanted) && files.len() < MAX_LISTED_FILES {
                    files.push(ImportFile {
                        account: account.clone(),
                        path: path.to_path_buf(),
                        kind,
                        modified_ms,
                        len,
                    });
                }
            });
        }
    }
    files.sort_by(|left, right| {
        right
            .modified_ms
            .cmp(&left.modified_ms)
            .then_with(|| left.path.cmp(&right.path))
    });
    files
}

fn is_claude_transcript(name: &str) -> bool {
    name.ends_with(".jsonl")
}

fn is_codex_rollout(name: &str) -> bool {
    name.starts_with("rollout-") && name.ends_with(".jsonl")
}

fn walk(dir: &Path, depth: usize, found: &mut impl FnMut(&Path, u64, u64)) {
    if depth > MAX_WALK_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            walk(&path, depth + 1, found);
        } else if file_type.is_file()
            && let Ok(metadata) = entry.metadata()
        {
            let modified_ms = metadata
                .modified()
                .ok()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .and_then(|since| u64::try_from(since.as_millis()).ok())
                .unwrap_or(0);
            found(&path, modified_ms, metadata.len());
        }
    }
}

pub fn run_import(
    files: Vec<ImportFile>,
    calendar: &impl Calendar,
    now_ms: u64,
    stop: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Option<ImportOutcome> {
    let mut outcome = ImportOutcome::default();
    let mut seen = SeenLines::new();
    let mut bytes = 0u64;
    let mut cutoff: Option<NaiveDate> = None;
    for file in files {
        let over_bytes = bytes.saturating_add(file.len) > MAX_IMPORT_BYTES;
        if outcome.files >= MAX_IMPORT_FILES as u64 || over_bytes {
            cutoff = Some(calendar.day(file.modified_ms));
            break;
        }
        bytes = bytes.saturating_add(file.len);
        let account_seen = seen.entry(file.account.clone()).or_default();
        if !import_file(
            &file,
            &mut outcome.days,
            account_seen,
            calendar,
            now_ms,
            stop,
        ) {
            return None;
        }
        outcome.files += 1;
        progress(outcome.files);
        if outcome.files.is_multiple_of(PACED_FILES) {
            std::thread::sleep(IMPORT_PACE);
        }
    }
    if let Some(cutoff) = cutoff {
        outcome.days.retain(|day, _| *day > cutoff);
        let today = calendar.day(now_ms);
        outcome.limited_days = u64::try_from((today - cutoff).num_days()).ok();
    }
    Some(outcome)
}

type SeenLines = HashMap<LedgerAccount, HashSet<u64>>;

fn first_sighting(seen: &mut HashSet<u64>, key: Option<u64>) -> bool {
    key.is_none_or(|key| seen.insert(key))
}

fn import_file(
    file: &ImportFile,
    days: &mut ImportedDays,
    seen: &mut HashSet<u64>,
    calendar: &impl Calendar,
    now_ms: u64,
    stop: &AtomicBool,
) -> bool {
    let mut cursor = TokenCursor::default();
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        let Some(pass) = read_tokens(&file.path, file.kind, cursor.clone(), calendar, now_ms)
        else {
            return true;
        };
        for entry in pass
            .entries
            .iter()
            .filter(|entry| first_sighting(seen, entry.key))
        {
            let tally = days
                .entry(entry.day)
                .or_default()
                .entry(file.account.clone())
                .or_default();
            tally.tokens_in += entry.input;
            tally.tokens_out += entry.output;
        }
        for turn in pass
            .turns
            .iter()
            .filter(|turn| first_sighting(seen, turn.key))
        {
            days.entry(turn.day)
                .or_default()
                .entry(file.account.clone())
                .or_default()
                .turns += 1;
        }
        std::thread::yield_now();
        if pass.cursor.offset == cursor.offset && pass.cursor.skipping == cursor.skipping {
            return true;
        }
        cursor = pass.cursor;
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use chrono::{FixedOffset, TimeZone};
    use serde_json::json;

    use super::*;
    use crate::usage::ledger::{DayTally, ZoneCalendar};

    const NOW_MS: u64 = 1_793_481_752_042;

    fn calendar() -> ZoneCalendar<FixedOffset> {
        ZoneCalendar(FixedOffset::east_opt(3600).unwrap())
    }

    fn day(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
    }

    fn write(path: &Path, lines: &[serde_json::Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn prompt(stamp: &str) -> serde_json::Value {
        json!({"type": "user", "timestamp": stamp, "message": {"role": "user", "content": "go"}})
    }

    fn answer(id: &str, stamp: &str, output: u64) -> serde_json::Value {
        json!({"type": "assistant", "timestamp": stamp, "message": {"id": id, "usage": {"input_tokens": 1, "output_tokens": output}}})
    }

    fn file(account: &LedgerAccount, path: PathBuf, modified: NaiveDate, len: u64) -> ImportFile {
        let modified_ms = calendar()
            .0
            .from_local_datetime(&modified.and_hms_opt(12, 0, 0).unwrap())
            .unwrap()
            .timestamp_millis();
        ImportFile {
            account: account.clone(),
            path,
            kind: TranscriptKind::Claude,
            modified_ms: u64::try_from(modified_ms).unwrap(),
            len,
        }
    }

    #[test]
    fn claude_transcripts_and_codex_rollouts_of_each_account_are_listed_newest_first() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let codex = home.path().join(".codex");
        write(
            &claude.join("projects/C--a/one.jsonl"),
            &[prompt("2026-10-01T10:00:00Z")],
        );
        write(
            &claude.join("projects/C--a/one/subagents/agent-1.jsonl"),
            &[prompt("2026-10-01T10:00:00Z")],
        );
        write(&claude.join("projects/C--a/notes.txt"), &[]);
        write(&claude.join("todos/x.jsonl"), &[]);
        write(&codex.join("sessions/2026/10/01/rollout-a.jsonl"), &[]);
        write(&codex.join("archived_sessions/rollout-b.jsonl"), &[]);
        write(&codex.join("archived_sessions/rollout-c.jsonl.zst"), &[]);
        let files = import_files(&[
            (UsageAgent::ClaudeCode, claude.clone()),
            (UsageAgent::ClaudeCode, claude.join(".")),
            (UsageAgent::Codex, codex),
        ]);
        let mut names: Vec<String> = files
            .iter()
            .map(|file| {
                file.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "agent-1.jsonl",
                "one.jsonl",
                "rollout-a.jsonl",
                "rollout-b.jsonl"
            ]
        );
        assert!(
            files
                .windows(2)
                .all(|pair| pair[0].modified_ms >= pair[1].modified_ms)
        );
    }

    #[test]
    fn an_import_counts_prompts_and_tokens_per_day_and_account() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        write(
            &claude.join("projects/C--a/s.jsonl"),
            &[
                prompt("2026-10-20T08:00:00Z"),
                answer("m1", "2026-10-20T08:00:05Z", 100),
                answer("m1", "2026-10-20T08:00:06Z", 100),
                json!({"type": "user", "timestamp": "2026-10-20T08:00:07Z", "message": {"content": [{"type": "tool_result", "tool_use_id": "t"}]}}),
                answer("m2", "2026-10-20T08:00:09Z", 20),
                prompt("2026-10-21T09:00:00Z"),
                answer("m3", "2026-10-21T09:00:05Z", 3),
            ],
        );
        let files = import_files(&[(UsageAgent::ClaudeCode, claude.clone())]);
        let outcome = run_import(files, &calendar(), NOW_MS, &AtomicBool::new(false), |_| {})
            .expect("an uninterrupted import");
        let account = LedgerAccount {
            agent: "claude-code".into(),
            config_dir: canonical_account_dir(&claude).display().to_string(),
        };
        assert_eq!(outcome.files, 1);
        assert_eq!(outcome.limited_days, None);
        assert_eq!(
            outcome.days[&day(10, 20)][&account],
            DayTally {
                turns: 1,
                tokens_in: 2,
                tokens_out: 120,
                ..DayTally::default()
            }
        );
        assert_eq!(outcome.days[&day(10, 21)][&account].turns, 1);
    }

    #[test]
    fn a_resumed_session_that_copies_earlier_lines_is_counted_once() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let prompt_with = |uuid: &str, stamp: &str| {
            let mut line = prompt(stamp);
            line["uuid"] = json!(uuid);
            line
        };
        let earlier = [
            prompt_with("u1", "2026-10-20T08:00:00Z"),
            answer("m1", "2026-10-20T08:00:05Z", 100),
        ];
        write(&claude.join("projects/C--a/first.jsonl"), &earlier);
        let resumed: Vec<serde_json::Value> = earlier
            .iter()
            .cloned()
            .chain([
                prompt_with("u2", "2026-10-22T09:00:00Z"),
                answer("m2", "2026-10-22T09:00:05Z", 7),
            ])
            .collect();
        write(&claude.join("projects/C--a/resumed.jsonl"), &resumed);
        let files = import_files(&[(UsageAgent::ClaudeCode, claude.clone())]);
        let outcome = run_import(files, &calendar(), NOW_MS, &AtomicBool::new(false), |_| {})
            .expect("an uninterrupted import");
        let account = LedgerAccount {
            agent: "claude-code".into(),
            config_dir: canonical_account_dir(&claude).display().to_string(),
        };
        assert_eq!(outcome.files, 2);
        assert_eq!(
            outcome.days[&day(10, 20)][&account],
            DayTally {
                turns: 1,
                tokens_in: 1,
                tokens_out: 100,
                ..DayTally::default()
            },
            "the copied prompt and answer count once"
        );
        assert_eq!(outcome.days[&day(10, 22)][&account].turns, 1);
        assert_eq!(outcome.days[&day(10, 22)][&account].tokens_out, 7);
    }

    #[test]
    fn a_stop_request_interrupts_the_import_without_an_outcome() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        write(
            &claude.join("projects/C--a/s.jsonl"),
            &[prompt("2026-10-20T08:00:00Z")],
        );
        let files = import_files(&[(UsageAgent::ClaudeCode, claude)]);
        assert_eq!(
            run_import(files, &calendar(), NOW_MS, &AtomicBool::new(true), |_| {}),
            None
        );
    }

    #[test]
    fn the_file_and_byte_limits_stop_the_walk_and_keep_only_whole_days() {
        let home = tempfile::tempdir().unwrap();
        let account = LedgerAccount {
            agent: "claude-code".into(),
            config_dir: home.path().display().to_string(),
        };
        let recent = home.path().join("recent.jsonl");
        write(
            &recent,
            &[
                prompt("2026-10-25T08:00:00Z"),
                prompt("2026-10-02T08:00:00Z"),
            ],
        );
        let unread = home.path().join("unread.jsonl");
        let by_files: Vec<ImportFile> =
            std::iter::once(file(&account, recent.clone(), day(10, 30), 10))
                .chain((0..MAX_IMPORT_FILES).map(|_| file(&account, unread.clone(), day(10, 2), 1)))
                .collect();
        let outcome = run_import(
            by_files,
            &calendar(),
            NOW_MS,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.files, MAX_IMPORT_FILES as u64);
        assert_eq!(
            outcome.limited_days,
            Some(29),
            "the files left unread end on October 2, twenty-nine days before October 31"
        );
        assert!(outcome.days.contains_key(&day(10, 25)));
        assert!(
            !outcome.days.contains_key(&day(10, 2)),
            "a day the unread files may also cover is left out"
        );

        let by_bytes = vec![
            file(&account, recent.clone(), day(10, 30), 10),
            file(&account, unread, day(10, 2), MAX_IMPORT_BYTES),
        ];
        let outcome = run_import(
            by_bytes,
            &calendar(),
            NOW_MS,
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap();
        assert_eq!(outcome.files, 1);
        assert_eq!(outcome.limited_days, Some(29));
    }
}
