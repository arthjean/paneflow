//! OpenCode CLI session discovery - shells out to
//! `opencode session list --format json` and produces unified
//! [`SessionMeta`](crate::agent_sessions::SessionMeta) entries for the
//! sessions popover.
//!
//! Why shell-out and not `rusqlite`: OpenCode v1.x stores sessions in a
//! Drizzle-managed SQLite database whose schema has no public stability
//! promise (the JSON→SQLite cutover in v1.1.53 shipped with active
//! migration bugs through early 2026). The CLI's `--format json` output
//! is the contract OpenCode publishes for third-party tools and survives
//! schema rewrites because the maintainers update the JSON serialiser
//! when columns move. The trade-off is a ~ 0.65 s wall-clock cost per
//! popover open and a runtime requirement that `opencode` be on `PATH`
//! both validated by the US-001 spike (see
//! `tasks/prd-opencode-sessions-decisions.md`).
//!
//! All process I/O happens off the GPUI main thread - call
//! [`read_sessions_for_cwd`] from inside `smol::unblock`.

use std::io;
use std::process::Command;

use serde_json::Value;

use crate::agent_sessions::{SessionAgent, SessionMeta, clean_session_label};

/// Cap stderr captured into the warn log when `opencode` exits non-zero.
/// Keeps the log line readable when the CLI dumps a multi-kilobyte panic.
const STDERR_LOG_CAP: usize = 200;

/// Wall-clock deadline for `opencode session list` (U-032). A hijacked or hung
/// binary can't block the smol::unblock worker past this.
const OPENCODE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// stdout cap for `opencode session list --format json` (U-032). 8 MiB is far
/// beyond any realistic session list while bounding a malicious binary that
/// streams gigabytes; past the cap the stream is drained and discarded.
const OPENCODE_STDOUT_CAP: u64 = 8 * 1024 * 1024;

/// Read all OpenCode CLI sessions whose recorded `directory` matches the
/// given cwd. Returns sessions sorted by `timestamp` descending (most
/// recent first).
///
/// **Blocking I/O** - call from inside `smol::unblock` or
/// `cx.background_executor`. The CLI is invoked with no caching: each
/// call spawns a fresh process. Caching is intentionally out of scope
/// for v1 (PRD US-002 AC6).
///
/// Returns an empty `Vec` (no panic) when:
/// - the `opencode` binary is missing from `PATH`,
/// - the spawned process exits non-zero,
/// - the CLI emits an empty stdout (zero sessions for this project).
pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

/// Like [`read_sessions_for_cwd`], but also reports how many older matching
/// sessions were omitted by the sidebar retention cap.
pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_sessions_with_program("opencode", cwd)
}

/// Test-only seam: lets the ENOENT test point at a deliberately missing
/// program name without mutating the process environment.
fn read_sessions_with_program(program: &str, cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(stdout) = run_opencode_list(program) else {
        return (Vec::new(), 0);
    };
    parse_sessions(&stdout, cwd)
}

/// Spawn `opencode session list --format json` and return its stdout
/// bytes on success, `None` on every failure mode (missing binary,
/// non-zero exit, or any spawn error). Logging side effects:
/// `info!` for ENOENT (expected when the user doesn't have OpenCode
/// installed), `warn!` for other failures (capped at
/// [`STDERR_LOG_CAP`] chars).
fn run_opencode_list(program: &str) -> Option<Vec<u8>> {
    let mut cmd = Command::new(program);
    cmd.args(["session", "list", "--format", "json"]);

    // U-032: bound the subprocess. run_with_timeout nulls stdin (mandatory: a
    // GUI process on Windows would otherwise inherit the parent stdin and a
    // child reading it - auth/consent prompt - would block forever) and caps
    // stdout at OPENCODE_STDOUT_CAP so a hijacked/chatty binary can't stream
    // gigabytes into memory or hang the smol::unblock worker.
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
        // Replace control characters (ANSI escapes, NUL, etc.) with `?`
        // before logging so a misbehaving binary cannot inject terminal
        // control sequences into journalctl / log aggregators that pass
        // ANSI through. Newlines are preserved for readability.
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

/// Parse the CLI's JSON output and project it onto [`SessionMeta`],
/// filtering by `directory == cwd`. Tolerant: unknown fields are
/// ignored; records missing `id` or `directory` are skipped silently;
/// bad UTF-8 / malformed JSON yields an empty list rather than a panic.
///
/// Field map (validated against opencode 1.14.41 in the US-001 spike):
/// - `id` → [`SessionMeta::session_id`]
/// - `directory` → [`SessionMeta::cwd`] (filtered for exact equality with
///   `cwd`; the CLI already pre-filters by project, this is
///   defence-in-depth per FR-02)
/// - `title` → [`SessionMeta::summary`] (`Some` when non-empty)
/// - `updated` (Unix epoch ms, i64) → [`SessionMeta::timestamp`] (ISO
///   8601 string for parity with the other readers and so the popover's
///   relative-time formatter parses it). Falls back to `created` when
///   `updated` is absent.
fn parse_sessions(stdout: &[u8], cwd: &str) -> (Vec<SessionMeta>, usize) {
    if stdout.is_empty() {
        // Spike confirmed: zero sessions for the project yields 0 stdout
        // bytes (not "[]"). Short-circuit before serde_json complains.
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

/// Convert one CLI record into a [`SessionMeta`] iff its `directory`
/// matches `cwd` exactly and the mandatory fields (`id`, `directory`)
/// are present and non-empty.
fn record_to_session(record: &Value, cwd: &str) -> Option<SessionMeta> {
    let session_id = record.get("id").and_then(|v| v.as_str())?.to_string();
    // Strict allow-list (`^[A-Za-z0-9_-]+$`) before the id flows verbatim into
    // `opencode --session <id>` and then to the user's PTY. The CLI's
    // documented id format is `ses_<base62>`, so anything else (a control char
    // that would auto-submit injected text, or a `;`/space that would chain a
    // second shell command) means a malformed record or a tampered binary.
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
        // OpenCode doesn't surface a git branch in `session list` output.
        // Empty matches the Codex reader's behaviour and collapses the
        // row to `<time>` only, which is what `opencode --continue`'s
        // own picker shows.
        git_branch: String::new(),
        summary,
        // EP-004 US-016: OpenCode's `session list` CLI contract carries no
        // model or token usage, so attribution degrades to agent + recency.
        model: None,
        usage: None,
    })
}

/// `serde_json::Value` exposes `as_i64` only for numbers that fit. Some
/// CLI builds (notably node-side serialisers) may emit timestamps as
/// strings; accept both.
fn value_as_i64(v: &Value) -> Option<i64> {
    v.as_i64()
        .or_else(|| v.as_u64().and_then(|u| i64::try_from(u).ok()))
        .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
}

/// Convert a Unix epoch in milliseconds into an ISO 8601 string of the
/// form `YYYY-MM-DDTHH:MM:SSZ`. Uses Howard Hinnant's "civil from days"
/// algorithm - inverse of `parse_iso8601_to_unix_secs` in
/// `agent_sessions.rs`. UTC; sub-second precision is dropped (the
/// popover's relative-time formatter rounds to the nearest minute
/// anyway).
fn unix_ms_to_iso8601(ms: i64) -> String {
    let secs = ms.div_euclid(1_000);
    let days_since_epoch = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3_600;
    let minute = (secs_of_day % 3_600) / 60;
    let second = secs_of_day % 60;

    // civil_from_days: inverse of the days_from_civil math used by
    // parse_iso8601_to_unix_secs.
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

    /// Real CLI output captured from `opencode 1.14.41` during the
    /// US-001 spike. Single record, `directory: "/home/arthur"`,
    /// `updated: 1778249807482` ms.
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
        // updated = 1778249807482 ms → 2026-05-08T14:16:47Z (sub-second
        // dropped by design - relative-time formatter rounds to minute).
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
        // A malicious or PATH-hijacked `opencode` binary could return an id
        // containing `\r` / `\n` to inject a second command into the user's
        // terminal when `send_command` appends its own `\r`. Reject these
        // records outright rather than skip the control-char check.
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
        // The spike confirmed: zero sessions yields 0 bytes, not "[]".
        // Don't let serde_json's EOF error become a Vec full of nothing.
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
        // Deterministic ENOENT - pick a name no shell will resolve. This
        // covers AC3 without depending on the test runner's PATH.
        let (sessions, omitted) =
            read_sessions_with_program("opencode-does-not-exist-zzz-9d2c1a", "/home/arthur");
        assert_eq!(omitted, 0);
        assert!(sessions.is_empty());
    }

    #[test]
    fn unix_ms_to_iso8601_matches_known_epoch() {
        // 2025-01-15T12:30:45Z - same anchor used by
        // agent_sessions::tests::iso8601_parses_z. 1_736_944_245 secs
        // → 1_736_944_245_000 ms.
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
