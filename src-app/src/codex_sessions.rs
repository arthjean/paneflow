//! Codex CLI session discovery - reads the on-disk transcript store at
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<TS>-<uuid>.jsonl` and produces
//! unified [`SessionMeta`](crate::agent_sessions::SessionMeta) entries
//! for the sessions popover.
//!
//! Format reference: PR openai/codex#3380 (RolloutItem envelope) and
//! community discussion #3827. The first line of every rollout file is a
//! `type:"session_meta"` envelope with `payload.id`, `payload.cwd`, and
//! `payload.timestamp`. Codex doesn't emit an `ai-title`-equivalent
//! record, so the title falls back to the first
//! `event_msg.user_message.message` content.
//!
//! All filesystem work happens off the GPUI main thread - call
//! [`read_sessions_for_cwd`] from inside `smol::unblock`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::agent_sessions::{AssistantUsage, SessionAgent, SessionMeta, clean_session_label};

/// Maximum number of leading lines to scan for the first user message.
/// In practice this lands within the first ~10 lines (after
/// `session_meta` + `turn_context` + a few state events). The cap is
/// generous so unusual prelude sequences still produce a label.
const TITLE_SCAN_LIMIT: usize = 256;

/// EP-004 US-016: deeper line cap for the attribution scan, which walks the
/// whole rollout to capture the model (`turn_context.payload.model`) and the
/// last cumulative `token_count` usage event. Bounded, and run ONLY on the
/// attribution path (the diff column load), never on the popover title scan.
const MODEL_USAGE_SCAN_LIMIT: usize = 20_000;

// US-013: per-line JSONL read cap, centralized (see `crate::limits`).
use crate::limits::MAX_LINE_BYTES;

/// Cap rendered first-user-message labels at this character count.
const LABEL_MAX_CHARS: usize = 80;

/// Compute the absolute path of `~/.codex/sessions/`. Returns `None` when
/// `dirs::home_dir()` fails.
pub fn sessions_root() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codex").join("sessions"))
}

/// Read all Codex CLI sessions whose recorded `cwd` matches the given
/// directory. Returns sessions sorted by timestamp descending (most
/// recent first).
///
/// **Blocking I/O** - call from inside `smol::unblock` or
/// `cx.background_executor`. Codex's flat date-bucketed layout
/// (`YYYY/MM/DD`) means we must scan every rollout file and read the
/// first line to filter by `cwd`. For the typical user (≤ 200 sessions)
/// this is comfortably under 100 ms; cap heavy users via the
/// per-file fast bail-out (we stop after the session_meta line if cwd
/// doesn't match).
pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

/// Like [`read_sessions_for_cwd`], but also reports how many older matching
/// sessions were omitted by the sidebar retention cap.
pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_sessions_for_cwd_inner(
        cwd,
        false,
        Some(crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE),
    )
}

/// EP-004 US-014/US-016: like [`read_sessions_for_cwd`] but the retained
/// attribution candidates are scanned deeper to populate `model`
/// (`turn_context`) + cumulative `usage` (last `token_count` event).
/// **Blocking I/O** - call from inside `smol::unblock`.
pub fn read_sessions_with_usage_for_attribution(cwd: &str, branch: &str) -> Vec<SessionMeta> {
    let Some(root) = sessions_root() else {
        return Vec::new();
    };

    let mut candidates: Vec<(SessionMeta, PathBuf)> = Vec::new();
    walk_jsonl_files(&root, &mut |path| {
        if let Some(meta) = read_session_meta_inner(path, false)
            && crate::agent_sessions::cwd_matches(&meta.cwd, cwd)
        {
            crate::agent_sessions::push_ranked_attribution(
                &mut candidates,
                meta,
                path.to_path_buf(),
                branch,
                crate::agent_sessions::DIFF_ATTRIBUTION_MATCH_CAP,
            );
        }
    });

    let enriched: Vec<SessionMeta> = candidates
        .into_iter()
        .filter_map(
            |(fallback, path)| match read_session_meta_inner(&path, true) {
                Some(meta) if crate::agent_sessions::cwd_matches(&meta.cwd, cwd) => Some(meta),
                Some(_) => None,
                None => Some(fallback),
            },
        )
        .collect();
    crate::agent_sessions::match_sessions_to_column(enriched, cwd, branch)
}

fn read_sessions_for_cwd_inner(
    cwd: &str,
    scan_usage: bool,
    cap: Option<usize>,
) -> (Vec<SessionMeta>, usize) {
    let Some(root) = sessions_root() else {
        return (Vec::new(), 0);
    };

    let cache_mtime = (!scan_usage && cap.is_some())
        .then(|| jsonl_tree_mtime(&root))
        .flatten();
    if let Some(cache_mtime) = cache_mtime
        && let Some(cached) =
            crate::agent_sessions::cache::lookup_with_mtime(SessionAgent::Codex, cwd, cache_mtime)
    {
        return cached;
    }

    let result = match cap {
        Some(cap) => {
            let mut collector = crate::agent_sessions::RecentSessionCollector::new(cap);
            walk_jsonl_files(&root, &mut |path| {
                if let Some(meta) = read_session_meta_inner(path, scan_usage)
                    && crate::agent_sessions::cwd_matches(&meta.cwd, cwd)
                {
                    collector.push(meta);
                }
            });
            collector.finish()
        }
        None => {
            let mut all = Vec::new();
            walk_jsonl_files(&root, &mut |path| {
                if let Some(meta) = read_session_meta_inner(path, scan_usage)
                    && crate::agent_sessions::cwd_matches(&meta.cwd, cwd)
                {
                    all.push(meta);
                }
            });
            all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
            (all, 0)
        }
    };

    if !scan_usage
        && cap.is_some()
        && let Some(cache_mtime) = jsonl_tree_mtime(&root)
    {
        crate::agent_sessions::cache::store_result_with_mtime(
            SessionAgent::Codex,
            cwd,
            cache_mtime,
            &result.0,
            result.1,
        );
    }

    result
}

/// Codex's layout is `YYYY/MM/DD/*.jsonl` - three levels below the root - so
/// a depth bound of 8 leaves generous slack while making a pathologically deep
/// tree (or any symlink cycle that slips past the `file_type` guard) terminate
/// instead of overflowing the stack (U-003).
const MAX_WALK_DEPTH: u32 = 8;

/// Walk Codex's `YYYY/MM/DD/*.jsonl` layout depth-first and invoke
/// `visit` on every `.jsonl` leaf.
fn walk_jsonl_files(dir: &Path, visit: &mut impl FnMut(&Path)) {
    walk_jsonl_files_bounded(dir, MAX_WALK_DEPTH, visit);
}

fn walk_jsonl_files_bounded(dir: &Path, depth_left: u32, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // U-003: `DirEntry::file_type()` reports the entry's *own* type (from
        // the readdir record, or an lstat) and does NOT follow symlinks -
        // unlike `Path::is_dir()`, which dereferences. A symlinked directory
        // therefore reports as neither dir nor file and is skipped, so a
        // planted cycle (`sessions/loop -> ../../sessions`) can never be
        // descended. Entries whose type can't be read are skipped.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            if depth_left > 0 {
                walk_jsonl_files_bounded(&path, depth_left - 1, visit);
            }
        } else if file_type.is_file() && is_jsonl_file(&path) {
            visit(&path);
        }
    }
}

fn jsonl_tree_mtime(root: &Path) -> Option<SystemTime> {
    let mut latest = fs::metadata(root).ok().and_then(|m| m.modified().ok());
    walk_jsonl_files(root, &mut |path| {
        let modified = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        latest = max_mtime(latest, modified);
    });
    latest
}

fn max_mtime(current: Option<SystemTime>, candidate: Option<SystemTime>) -> Option<SystemTime> {
    match (current, candidate) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn is_jsonl_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

/// Title-only wrapper, used by the unit tests (production routes through
/// [`read_sessions_for_cwd_inner`] → [`read_session_meta_inner`] directly).
#[cfg(test)]
fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    read_session_meta_inner(path, false)
}

/// Read the head of a rollout file: extract the `session_meta` envelope
/// (line 1) and the first user message (typically a few lines later). When
/// `scan_usage` (EP-004 attribution path) the tail scan also captures the model
/// (`turn_context`) and the last cumulative `token_count` usage.
fn read_session_meta_inner(path: &Path, scan_usage: bool) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    // Line 1 must be session_meta or we skip the file.
    buf.clear();
    // US-010 (cli-hardening-followup-2026-Q3): cap line read at
    // MAX_LINE_BYTES. Truncated line fails serde_json parse below
    // and the file is skipped -- same outcome as a malformed line.
    let n = reader
        .by_ref()
        .take(MAX_LINE_BYTES)
        .read_line(&mut buf)
        .ok()?;
    if n == 0 {
        return None;
    }
    if n as u64 == MAX_LINE_BYTES && !buf.ends_with('\n') {
        log::warn!(
            target: "paneflow_app::codex_sessions",
            "session JSONL line truncated at {} bytes for {} -- skipping file",
            MAX_LINE_BYTES,
            path.display(),
        );
        return None;
    }
    let first_value: serde_json::Value = serde_json::from_str(buf.trim_end()).ok()?;
    if first_value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = first_value.get("payload")?;
    let session_id = payload.get("id").and_then(|v| v.as_str())?.to_string();
    let cwd = payload.get("cwd").and_then(|v| v.as_str())?.to_string();
    if cwd.is_empty() {
        return None;
    }
    // session_id lands verbatim in `codex resume <id>`, so hold it to the
    // strict `^[A-Za-z0-9_-]+$` allow-list (Codex ids are UUIDs): rejects a
    // `\r`/`\n` that would submit injected text and a `;`/space that would
    // chain a second shell command. cwd is display-only today but a future
    // `cd <cwd>` prefix would inherit the gap, and a path legitimately carries
    // `/` + spaces, so keep the control-char guard for it. Mirrors (and
    // tightens) the guard in `opencode_sessions::record_to_session`.
    if !crate::agent_sessions::is_valid_session_id(&session_id)
        || cwd.chars().any(|c| c.is_control())
    {
        log::warn!(
            "codex_sessions: dropped {} -- payload carries an invalid id or control chars in cwd",
            path.display(),
        );
        return None;
    }
    // Inner timestamp is the session start; outer envelope timestamp is
    // the moment the file was opened. They're typically within seconds
    // of each other - prefer the inner (session-relative) value.
    let timestamp = payload
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| first_value.get("timestamp").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    // Title-only path keeps the cheap first-user-message scan untouched. The
    // attribution path runs the deeper tail scan (model + usage).
    let (summary, model, usage) = if scan_usage {
        scan_tail_with_usage(&mut reader)
    } else {
        (scan_first_user_message(&mut reader), None, None)
    };

    Some(SessionMeta {
        agent: SessionAgent::Codex,
        session_id,
        timestamp,
        cwd,
        // Codex doesn't record git branch in `session_meta`. Leave empty
        // so the row collapses to `<time>` only - matches what the user
        // sees when they run `codex resume`.
        git_branch: String::new(),
        summary,
        model,
        usage,
    })
}

/// EP-004 US-016: deeper tail scan for the attribution path. Walks up to
/// [`MODEL_USAGE_SCAN_LIMIT`] lines capturing the first user message (label),
/// the model (`turn_context.payload.model`), and the LAST cumulative
/// `token_count` usage event. Codex reports `token_count` as a running total,
/// so the last one wins (not summed). Usage is normalized to the shared
/// [`AssistantUsage`] tier semantics (input = uncached input, cache_read =
/// cached subset) so the pricing table treats Claude and Codex uniformly.
fn scan_tail_with_usage(
    reader: &mut BufReader<fs::File>,
) -> (Option<String>, Option<String>, Option<AssistantUsage>) {
    let mut summary: Option<String> = None;
    let mut model: Option<String> = None;
    let mut usage: Option<AssistantUsage> = None;
    let mut buf = String::new();
    for _ in 0..MODEL_USAGE_SCAN_LIMIT {
        buf.clear();
        let n = match reader.by_ref().take(MAX_LINE_BYTES).read_line(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        let trimmed = buf.trim_end();
        if !trimmed.starts_with('{') {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match value.get("type").and_then(|v| v.as_str()) {
            Some("turn_context") => {
                if model.is_none()
                    && let Some(m) = value
                        .get("payload")
                        .and_then(|p| p.get("model"))
                        .and_then(|v| v.as_str())
                    && !m.is_empty()
                {
                    model = Some(m.to_string());
                }
            }
            Some("event_msg") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                match payload.get("type").and_then(|v| v.as_str()) {
                    Some("user_message") if summary.is_none() => {
                        if let Some(message) = payload.get("message").and_then(|v| v.as_str())
                            && let Some(cleaned) = clean_user_message(message)
                        {
                            summary = Some(cleaned);
                        }
                    }
                    Some("token_count") => {
                        if let Some(total) =
                            payload.get("info").and_then(|i| i.get("total_token_usage"))
                        {
                            let input_total = total
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let cached = total
                                .get("cached_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let output = total
                                .get("output_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            let u = AssistantUsage {
                                input: input_total.saturating_sub(cached),
                                output,
                                cache_read: cached,
                                cache_creation: 0,
                            };
                            // Cumulative - last non-empty wins.
                            if !u.is_empty() {
                                usage = Some(u);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (summary, model, usage)
}

/// Scan up to [`TITLE_SCAN_LIMIT`] lines looking for the first
/// `event_msg.user_message`. Codex wraps user input as
/// `{"type":"event_msg","payload":{"type":"user_message","message":"..."}}`.
///
/// Signature is concrete on `BufReader<File>` rather than the
/// generic `R: BufRead` it used to be: the `by_ref().take()`
/// pattern needed by US-010 for the per-line byte cap fails to
/// type-check against `&mut R` (the compiler auto-derefs to `R`
/// and the move blocks the borrow). The only call site already
/// passes a `BufReader<File>`, so the generic was vestigial.
fn scan_first_user_message(reader: &mut BufReader<fs::File>) -> Option<String> {
    let mut buf = String::new();
    for _ in 0..TITLE_SCAN_LIMIT {
        buf.clear();
        // US-010 (cli-hardening-followup-2026-Q3): cap each line read.
        // Oversize lines fall through to `serde_json::from_str` which
        // errors and the loop `continue`s -- the scan moves on to the
        // next line without OOMing.
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES)
            .read_line(&mut buf)
            .ok()?;
        if n == 0 {
            return None;
        }
        let trimmed = buf.trim_end();
        if !trimmed.starts_with('{') {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = match value.get("payload") {
            Some(p) => p,
            None => continue,
        };
        if payload.get("type").and_then(|v| v.as_str()) != Some("user_message") {
            continue;
        }
        if let Some(message) = payload.get("message").and_then(|v| v.as_str())
            && let Some(cleaned) = clean_user_message(message)
        {
            return Some(cleaned);
        }
    }
    None
}

fn clean_user_message(raw: &str) -> Option<String> {
    clean_session_label(raw, LABEL_MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the real Codex rollout sequence observed in the wild:
    /// line 1 is `session_meta`, then a few state events, then the first
    /// `event_msg` `user_message`.
    #[test]
    fn read_session_meta_extracts_envelope_and_first_user_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"timestamp":"2026-04-26T13:11:10.338Z","type":"session_meta","payload":{"id":"019dc9ea-38d7-7372-9cc4-253ce944d41b","timestamp":"2026-04-26T13:11:03.694Z","cwd":"/home/arthur/dev/paneflow","originator":"codex-tui","cli_version":"0.123.0","model_provider":"openai"}}"#,
                "\n",
                r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
                "\n",
                r#"{"timestamp":"2026-04-26T13:11:10.345Z","type":"event_msg","payload":{"type":"user_message","message":"Explique le projet stp","images":[]}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let meta = read_session_meta(&path).expect("envelope extracted");
        assert_eq!(meta.agent, SessionAgent::Codex);
        assert_eq!(meta.session_id, "019dc9ea-38d7-7372-9cc4-253ce944d41b");
        assert_eq!(meta.cwd, "/home/arthur/dev/paneflow");
        assert_eq!(meta.timestamp, "2026-04-26T13:11:03.694Z");
        assert!(meta.git_branch.is_empty());
        assert_eq!(meta.summary.as_deref(), Some("Explique le projet stp"));
    }

    #[test]
    fn usage_scan_captures_model_and_normalizes_token_count() {
        // EP-004 US-016: scan_usage=true captures `turn_context.payload.model`
        // and the LAST cumulative `token_count`, normalized to the shared tier
        // semantics (input = uncached input, cache_read = cached subset). The
        // title-only path leaves model/usage None.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-usage.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"timestamp":"2026-04-26T13:11:10.338Z","type":"session_meta","payload":{"id":"019dc9ea-38d7-7372-9cc4-253ce944d41b","timestamp":"2026-04-26T13:11:03.694Z","cwd":"/home/arthur/dev/paneflow"}}"#,
                "\n",
                r#"{"type":"turn_context","payload":{"model":"gpt-5"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":500,"cached_input_tokens":200,"output_tokens":80}}}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":900,"cached_input_tokens":300,"output_tokens":150}}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let title_only = read_session_meta_inner(&path, false).expect("meta");
        assert!(title_only.model.is_none());
        assert!(title_only.usage.is_none());

        let with_usage = read_session_meta_inner(&path, true).expect("meta");
        assert_eq!(with_usage.model.as_deref(), Some("gpt-5"));
        let usage = with_usage.usage.expect("usage parsed");
        // Last cumulative event wins: 900 total input, 300 cached → 600 uncached.
        assert_eq!(usage.input, 600);
        assert_eq!(usage.cache_read, 300);
        assert_eq!(usage.output, 150);
        assert_eq!(usage.cache_creation, 0);
    }

    #[test]
    fn read_session_meta_returns_none_for_non_session_meta_first_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("not-codex.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"event_msg","payload":{"type":"user_message","message":"hi"}}
"#,
        )
        .expect("write fixture");
        assert!(read_session_meta(&path).is_none());
    }

    #[test]
    fn read_session_meta_returns_none_when_payload_missing_cwd() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("no-cwd.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"session_meta","payload":{"id":"x","timestamp":"2026-04-26T13:11:03.694Z"}}
"#,
        )
        .expect("write fixture");
        assert!(read_session_meta(&path).is_none());
    }

    #[test]
    fn user_message_label_is_truncated_with_ellipsis_when_long() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("long-prompt.jsonl");
        let long_prompt = "x".repeat(200);
        let session_meta_line = r#"{"type":"session_meta","payload":{"id":"s","cwd":"/p","timestamp":"2026-04-26T13:00:00Z"}}"#;
        let user_msg_line = format!(
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":"{long_prompt}"}}}}"#
        );
        std::fs::write(&path, format!("{session_meta_line}\n{user_msg_line}\n"))
            .expect("write fixture");
        let meta = read_session_meta(&path).expect("meta");
        let summary = meta.summary.expect("summary");
        assert_eq!(summary.chars().count(), LABEL_MAX_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn user_message_label_collapses_whitespace_and_controls() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("messy-prompt.jsonl");
        let session_meta_line = r#"{"type":"session_meta","payload":{"id":"s","cwd":"/p","timestamp":"2026-04-26T13:00:00Z"}}"#;
        let prompt = serde_json::to_string("Explain\n\tthis\u{1b} now").expect("json string");
        let user_msg_line = format!(
            r#"{{"type":"event_msg","payload":{{"type":"user_message","message":{prompt}}}}}"#
        );
        std::fs::write(&path, format!("{session_meta_line}\n{user_msg_line}\n"))
            .expect("write fixture");

        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(meta.summary.as_deref(), Some("Explain this now"));
    }

    #[test]
    fn session_id_control_char_guard() {
        // payload.id carries CR+LF + an injected shell command. Without
        // the guard, the id flows into `codex resume <id>` and submits
        // `rm -rf ~` as a separate PTY command.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("malicious.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"abc\r\nrm -rf ~","cwd":"/tmp/proj","timestamp":"2026-04-26T13:11:03.694Z"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(
            read_session_meta(&path).is_none(),
            "session with control chars in payload.id must be dropped"
        );
    }

    #[test]
    fn session_id_legitimate_uuid_passes_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ok.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"019dc9ea-38d7-7372-9cc4-253ce944d41b","cwd":"/tmp/proj","timestamp":"2026-04-26T13:11:03.694Z"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("legitimate UUID must pass the guard");
        assert_eq!(meta.session_id, "019dc9ea-38d7-7372-9cc4-253ce944d41b");
    }

    #[test]
    fn cwd_control_char_guard() {
        // Same class of injection as session_id, just one field over.
        // cwd is display-only today but a future `cd <cwd>` prefix
        // would inherit the gap without any git-blame signal.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("malicious-cwd.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"019dc9ea-38d7-7372-9cc4-253ce944d41b","cwd":"/tmp/proj\r\nrm -rf ~","timestamp":"2026-04-26T13:11:03.694Z"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(
            read_session_meta(&path).is_none(),
            "session with control chars in cwd must be dropped"
        );
    }

    /// U-003: a deep-but-acyclic tree within the depth bound still yields every
    /// real `.jsonl` leaf - the guard must not drop legitimate sessions.
    #[test]
    fn walk_discovers_jsonl_in_deep_acyclic_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Codex's real shape is 3 levels (YYYY/MM/DD); go a little deeper to
        // prove the bound (8) leaves slack.
        let leaf_dir = dir.path().join("2026/06/08/extra");
        std::fs::create_dir_all(&leaf_dir).expect("mkdir -p");
        let jsonl = leaf_dir.join("rollout.jsonl");
        std::fs::write(&jsonl, b"{}\n").expect("write");
        std::fs::write(leaf_dir.join("not-a-session.txt"), b"ignore me").expect("write");

        let mut found = Vec::new();
        walk_jsonl_files(dir.path(), &mut |p| found.push(p.to_path_buf()));
        assert_eq!(found, vec![jsonl], "the one real .jsonl must be discovered");
    }

    /// U-003: the depth bound stops recursion past `MAX_WALK_DEPTH`, so an
    /// arbitrarily deep tree terminates rather than overflowing the stack.
    #[test]
    fn walk_stops_past_depth_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Build MAX_WALK_DEPTH + 4 nested dirs, with a .jsonl just past the
        // bound. The walk must terminate and must NOT visit the too-deep file.
        let mut deep = dir.path().to_path_buf();
        for i in 0..(MAX_WALK_DEPTH + 4) {
            deep = deep.join(format!("d{i}"));
        }
        std::fs::create_dir_all(&deep).expect("mkdir -p");
        std::fs::write(deep.join("too-deep.jsonl"), b"{}\n").expect("write");

        let mut count = 0usize;
        walk_jsonl_files(dir.path(), &mut |_| count += 1);
        assert_eq!(count, 0, "a leaf past the depth bound must not be visited");
    }

    /// U-003: a symlink cycle pointing back at an ancestor must not be
    /// descended (it would otherwise recurse forever and stack-overflow).
    /// Unix-only because creating a symlink on Windows needs elevation/dev
    /// mode. The Windows equivalent (NTFS junction / `IO_REPARSE_TAG_*`) is
    /// reported by `DirEntry::file_type()` on the pinned toolchain (Rust 1.95)
    /// with `is_symlink() = true` and `is_dir() = false` for native Win10/11
    /// volumes - so the same `is_dir()` guard skips it. Treated as
    /// inspection-only per US-002 AC4 (no Win symlink CI leg yet); a junction
    /// on a CIFS/remote-mapped volume is the residual gap to revisit if a
    /// Windows integration test lands.
    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_symlink_cycle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("2026/06/08");
        std::fs::create_dir_all(&real).expect("mkdir -p");
        let jsonl = real.join("rollout.jsonl");
        std::fs::write(&jsonl, b"{}\n").expect("write");
        // sessions/2026/loop -> sessions (points at an ancestor: a cycle).
        std::os::unix::fs::symlink(dir.path(), dir.path().join("2026/loop"))
            .expect("create symlink cycle");

        let mut found = Vec::new();
        walk_jsonl_files(dir.path(), &mut |p| found.push(p.to_path_buf()));
        // Terminates (no stack overflow) and still finds the one real file
        // exactly once - the symlinked directory was never descended.
        assert_eq!(found, vec![jsonl]);
    }
}
