//! Claude Code's own live session registry, read as an agent-state source.
//!
//! Claude Code maintains one JSON file per running process under
//! `<config dir>/sessions/<pid>.json`. It exists for its own peer discovery
//! (cross-session messaging, `claude agents`) and it is not a documented
//! interface, but it is the only channel that reports the full turn state of
//! an interactive session **without any hook running**:
//!
//! ```json
//! {"pid":14404,"sessionId":"517dd24b-…","cwd":"C:\\dev\\paneflow",
//!  "procStart":"134323895399231254","kind":"interactive",
//!  "status":"busy","statusUpdatedAt":1787916617655}
//! ```
//!
//! That matters because an organization can switch every hook off from managed
//! settings (`disableAllHooks`, which also disables `statusLine`, or
//! `allowManagedHooksOnly`, which drops Paneflow's entries in silence). The
//! sidebar then has nothing to show but "an agent is running". This file is
//! what puts `Thinking` / `WaitingForInput` / `Finished` back.
//!
//! Two properties make it usable rather than merely available:
//!
//! - **It is written on transition, not on a timer.** Claude Code writes it
//!   from an effect keyed on `[status, waitingFor]`, so the state is current
//!   the moment the file changes and there is no polling latency to add.
//! - **It is PID-keyed and PID-reuse safe.** The filename is the process id
//!   and `procStart` pins the OS start time, which is the same invariant
//!   `agent_sessions` already carries, so a record binds to a pane through the
//!   existing ancestor walk (`workspace::pid_resolve`).
//!
//! Everything here is deliberately tolerant. The schema is internal to another
//! program and can change without notice, so an unknown `status`, a missing
//! field, an unparseable file or a vanished directory all degrade to "this
//! source has nothing to say" and never to an error the user sees. Verified
//! against Claude Code 2.1.250.
//!
//! All filesystem work belongs off the GPUI main thread - call
//! [`read_live_sessions`] from inside `smol::unblock`.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::ai_types::AgentLifecycleEvent;

/// Upper bound on one record. The real files measure ~600 bytes; this is a
/// guard against a corrupt or hostile file, not a tuning knob.
const MAX_RECORD_BYTES: u64 = 64 * 1024;

/// Upper bound on records read per sweep. A machine with more concurrent
/// Claude Code processes than this has bigger problems, and the cap keeps a
/// directory someone filled with junk from turning into unbounded work on a
/// background thread.
const MAX_RECORDS: usize = 256;

/// The status vocabulary Claude Code writes. Closed set: an unknown string
/// parses to `None` rather than being guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaudeSessionStatus {
    /// A turn is in flight (`isLoading`, or a delegated sub-agent is active).
    Busy,
    /// Idle at the prompt, but shell tasks the agent started are still
    /// running. Reported as work, because that is what the user sees.
    Shell,
    /// Blocked on the user, with `waitingFor` naming what for.
    Waiting,
    /// The turn ended and the prompt is back.
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

/// One live interactive session, as far as this source can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeSessionRecord {
    /// The `claude` process id. Also the file's name, which is what makes a
    /// record bindable even when its body is partially unreadable.
    pub pid: u32,
    pub status: ClaudeSessionStatus,
    /// Why the session is waiting, verbatim from the record
    /// (`"input needed"`, `"sandbox request"`, the title of the dialog on
    /// top, …). UNTRUSTED text from another program: display only, never
    /// interpreted, and truncated by [`clamp_waiting_reason`].
    pub waiting_for: Option<String>,
    /// Opaque OS start time, compared only for equality against the value
    /// Paneflow probed for the same PID. `None` on a record that omits it.
    pub proc_start: Option<String>,
}

impl ClaudeSessionRecord {
    /// The lifecycle event this record implies.
    ///
    /// `shell` maps to the same event as `busy` on purpose: the distinction
    /// Claude Code draws (idle prompt, background shell task) is not one the
    /// sidebar makes, and reporting it as idle would clear the spinner while
    /// a `Bash` tool call is still running.
    ///
    /// `waiting` is only a request when the AGENT opened the wait. Claude Code
    /// (2.1.258, `KBe`/`g_o` in the bundle) computes `waitingFor` in this
    /// order: `"input needed"` (a queued elicitation), the title of the dialog
    /// on top (permission, plan approval, …), `"worker request"`,
    /// `"sandbox request"`, and last `"dialog open"` when the USER is inside
    /// a local slash command (`/resume`, `/model`, `/config`) with no response
    /// streaming. That last one is the user's own UI, not the agent asking
    /// for anything, and promoting it lit the sidebar bell every time
    /// `/resume` was typed. It reads as the idle prompt it sits on. cmux draws
    /// the same line: only `permission_prompt` and a real question raise its
    /// "Needs input" pill.
    pub fn lifecycle_event(&self) -> AgentLifecycleEvent {
        match self.status {
            ClaudeSessionStatus::Busy | ClaudeSessionStatus::Shell => AgentLifecycleEvent::Working,
            ClaudeSessionStatus::Waiting if self.is_user_opened_dialog() => {
                AgentLifecycleEvent::Idle
            }
            ClaudeSessionStatus::Waiting => AgentLifecycleEvent::Notification {
                message: self.waiting_for.clone(),
            },
            ClaudeSessionStatus::Idle => AgentLifecycleEvent::Idle,
        }
    }

    fn is_user_opened_dialog(&self) -> bool {
        self.waiting_for
            .as_deref()
            .is_some_and(|reason| reason.eq_ignore_ascii_case(USER_DIALOG_REASON))
    }
}

/// The `waitingFor` Claude Code writes while the user sits in a slash-command
/// dialog. Matched exactly: every other reason is the agent's.
const USER_DIALOG_REASON: &str = "dialog open";

/// Raw record shape. Every field is optional so a forward-compatible schema
/// change degrades to "less information" instead of "no information".
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
    /// `interactive` | `bg` | `daemon` | `daemon-worker`. Only an interactive
    /// session has a pane to speak for.
    #[serde(default)]
    kind: Option<String>,
}

/// Cap on the stored wait reason. It reaches a sidebar row and a tooltip, and
/// the longest real value is a dialog title.
const MAX_WAITING_REASON_CHARS: usize = 256;

/// Trim a wait reason to something a row can hold, dropping it entirely when
/// it is blank.
///
/// The value is UNTRUSTED: one of its sources is the title of whatever dialog
/// Claude Code has on top, which can carry text the agent was handed. It goes
/// straight to a sidebar row, so it gets the same treatment every other
/// CLI-written label does - bidi and zero-width control characters stripped,
/// then a hard character bound. Pure, so both are testable.
fn clamp_waiting_reason(raw: Option<String>) -> Option<String> {
    let bounded: String = raw?.trim().chars().take(MAX_WAITING_REASON_CHARS).collect();
    let clean = crate::markdown::strip_bidi_zero_width(bounded);
    (!clean.trim().is_empty()).then_some(clean)
}

/// Parse one record. `file_pid` is the PID from the filename, used when the
/// body omits or disagrees about `pid` - the filename is the authority,
/// because it is what the writer keys the file on.
///
/// Returns `None` for anything this source cannot speak for: unparseable
/// JSON, an unknown or absent status, or a non-interactive session (a
/// background job has no pane of its own).
pub fn parse_record(bytes: &[u8], file_pid: u32) -> Option<ClaudeSessionRecord> {
    let raw: RawRecord = serde_json::from_slice(bytes).ok()?;
    // A record that names a different PID than its own filename is either a
    // stale copy or something Paneflow does not understand; either way the
    // binding it implies cannot be trusted.
    if raw.pid.is_some_and(|pid| pid != file_pid) {
        return None;
    }
    // `kind` was added alongside background agents. An older record without
    // it is interactive by construction, so a missing value is accepted.
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

/// The PID a session filename encodes (`14404.json`), or `None` for any other
/// name. The directory also holds `<pid>.<hash>.key` peer-token files, which
/// this rejects by construction rather than by extension matching.
pub fn pid_from_file_name(name: &str) -> Option<u32> {
    name.strip_suffix(".json")?.parse().ok()
}

/// Absolute path of Claude Code's session-registry directory.
///
/// `CLAUDE_CONFIG_DIR` wins when set, exactly as the CLI resolves it; the
/// fallback is `~/.claude`. `None` when neither can be resolved, which is the
/// ordinary answer on a machine with no Claude Code install.
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

/// Read every parseable interactive record in `dir`.
///
/// Blocking I/O - callers run it off the render thread. A missing directory
/// (no Claude Code on this machine, or none started yet) is the empty answer,
/// not an error.
pub fn read_live_sessions(dir: &Path) -> Vec<ClaudeSessionRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten().take(MAX_RECORDS) {
        let Some(pid) = entry.file_name().to_str().and_then(pid_from_file_name) else {
            continue;
        };
        // Size-check before reading so a huge file is skipped rather than
        // loaded and then rejected.
        if entry
            .metadata()
            .is_ok_and(|meta| meta.len() > MAX_RECORD_BYTES)
        {
            continue;
        }
        // A read that loses its race with the writer's rename is ordinary:
        // the next sweep sees the new file.
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

    /// The exact shape observed from Claude Code 2.1.250, trimmed of the
    /// fields this reader does not consume.
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
        // A background `Bash` call still reads as work to the user, so
        // `shell` must not clear the spinner.
        assert_eq!(event("shell", ""), AgentLifecycleEvent::Working);
        assert_eq!(event("idle", ""), AgentLifecycleEvent::Idle);
        assert_eq!(
            event("waiting", "input needed"),
            AgentLifecycleEvent::Notification {
                message: Some("input needed".into())
            }
        );
        // The agent's own waits, whatever the dialog is titled.
        for reason in [
            "sandbox request",
            "worker request",
            "goal proposal",
            "Permission",
        ] {
            assert_eq!(
                event("waiting", reason),
                AgentLifecycleEvent::Notification {
                    message: Some(reason.into())
                },
                "{reason:?}"
            );
        }
    }

    #[test]
    fn a_slash_command_dialog_is_the_users_wait_not_the_agents() {
        // The failure this prevents: typing `/resume` raising the sidebar
        // bell as if Claude Code had asked for something.
        let event = |waiting: &str| {
            let body = format!(r#"{{"status":"waiting","waitingFor":"{waiting}"}}"#);
            parse_record(body.as_bytes(), 7)
                .expect("known status parses")
                .lifecycle_event()
        };
        assert_eq!(event("dialog open"), AgentLifecycleEvent::Idle);
        assert_eq!(event("Dialog Open"), AgentLifecycleEvent::Idle);
        // A blank reason is still a wait this source cannot explain, and it
        // stays a request rather than being guessed idle.
        assert_eq!(
            event(""),
            AgentLifecycleEvent::Notification { message: None }
        );
    }

    #[test]
    fn an_unknown_or_absent_status_says_nothing_rather_than_guessing() {
        // The schema belongs to another program: a status this version has
        // never seen must not be coerced into the nearest known one.
        assert!(parse_record(br#"{"status":"parked"}"#, 7).is_none());
        assert!(parse_record(br#"{"pid":7}"#, 7).is_none());
        assert!(parse_record(b"", 7).is_none());
        assert!(parse_record(b"{ truncated", 7).is_none());
    }

    #[test]
    fn a_record_that_is_not_this_pane_is_refused() {
        // A body disagreeing with its filename cannot be bound to a process.
        assert!(parse_record(br#"{"pid":99,"status":"busy"}"#, 7).is_none());
        // Background and daemon sessions have no pane to speak for.
        for kind in ["bg", "daemon", "daemon-worker"] {
            let body = format!(r#"{{"status":"busy","kind":"{kind}"}}"#);
            assert!(
                parse_record(body.as_bytes(), 7).is_none(),
                "{kind} sessions must not claim a surface"
            );
        }
        // A record predating the field is interactive by construction.
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
        // Same treatment as any other CLI-written label reaching a row: a
        // dialog title can carry text the agent was handed.
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
        // The registry directory also holds per-session peer-token files.
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
