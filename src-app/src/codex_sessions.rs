//! Codex CLI session discovery - reads the on-disk transcript store at
//! `~/.codex/sessions/YYYY/MM/DD/rollout-<TS>-<uuid>.jsonl` and produces
//! unified [`SessionMeta`](crate::agent_sessions::SessionMeta) entries
//! for the sessions popover.
//!
//! Format reference: PR openai/codex#3380 (RolloutItem envelope) and
//! community discussion #3827. The first line of every rollout file is a
//! `type:"session_meta"` envelope with `payload.id`, `payload.cwd`,
//! `payload.timestamp`, `payload.thread_source` and `payload.git.branch`.
//! Codex doesn't emit an `ai-title`-equivalent record, so the title falls
//! back to the first human-authored message (see
//! [`user_text_from_record`] for the three record shapes that carry one).
//!
//! All filesystem work happens off the GPUI main thread - call
//! [`read_sessions_for_cwd`] from inside `smol::unblock`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::agent_sessions::{AssistantUsage, SessionAgent, SessionMeta, clean_session_label};

/// Maximum number of leading lines to scan for the first user message.
/// In practice this lands within the first ~10 lines (measured on real
/// 0.149.1 rollouts: `session_meta`, `task_started`, three `developer`
/// preludes, one injected `user` envelope, `world_state`, `turn_context`,
/// then the prompt on line 9). The cap is generous so unusual prelude
/// sequences still produce a label.
const TITLE_SCAN_LIMIT: usize = 256;

/// Byte budget for the same scan, and the binding constraint in practice:
/// those prelude lines are large (30-56 KB each), so line 9 sits at ~178 KB
/// into the file. 1 MiB leaves ~5x headroom while bounding the read - the
/// line cap alone would allow 256 x [`MAX_LINE_BYTES`] = 16 MiB per file, on
/// every sidebar refresh, for every rollout on disk.
const TITLE_SCAN_BYTES: u64 = 1024 * 1024;

/// Dedicated cap for line 1. [`MAX_LINE_BYTES`] (64 KiB) is the right bound
/// for body records but not for `session_meta`, which embeds
/// `base_instructions` + `dynamic_tools`: 49.6 KB measured on a current
/// rollout, and it grows with every tool Codex ships. Overrunning the cap
/// costs the whole file (id and cwd live nowhere else), so this one line gets
/// its own, much larger bound. Still bounded, and still one line per file.
const SESSION_META_MAX_BYTES: u64 = 1024 * 1024;

/// Prefixes of the synthetic `role:"user"` records Codex injects around a real
/// prompt: repo instructions, skill bodies, plugin catalogs, desktop context.
/// They are indistinguishable from human input at the record level, so the
/// title scan filters them by prefix - the same approach as cmux's
/// `realCodexUserMessage`, extended with the envelopes 0.149.1 added.
const SYNTHETIC_USER_PREFIXES: [&str; 8] = [
    "# AGENTS.md",
    "<app-context",
    "<environment_context",
    "<permissions",
    "<recommended_plugins",
    "<skill>",
    "<system",
    "<user_instructions",
];

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
/// this is comfortably under 100 ms, because a file whose `session_meta`
/// records another `cwd` is abandoned on line 1 (see the `cwd_filter`
/// argument of [`read_session_meta_inner`]) instead of having its body
/// scanned for a title that would then be thrown away.
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
        if let Some(meta) = read_session_meta_inner(path, false, Some(cwd)) {
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
        .map(|(fallback, path)| {
            // The deep re-read can fail where the cheap head scan succeeded
            // (an I/O error, a body that outruns the usage budget); keep the
            // already-matched head result rather than dropping the column.
            read_session_meta_inner(&path, true, Some(cwd)).unwrap_or(fallback)
        })
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
                if let Some(meta) = read_session_meta_inner(path, scan_usage, Some(cwd)) {
                    collector.push(meta);
                }
            });
            collector.finish()
        }
        None => {
            let mut all = Vec::new();
            walk_jsonl_files(&root, &mut |path| {
                if let Some(meta) = read_session_meta_inner(path, scan_usage, Some(cwd)) {
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
    read_session_meta_inner(path, false, None)
}

/// Read the head of a rollout file: extract the `session_meta` envelope
/// (line 1) and the first user message (typically a few lines later). When
/// `scan_usage` (EP-004 attribution path) the tail scan also captures the model
/// (`turn_context`) and the last cumulative `token_count` usage.
///
/// `cwd_filter` short-circuits the scan: Codex's store is one flat date tree
/// for every project, so most files on disk belong to another directory and
/// their body is pure waste to read. Passing the caller's cwd stops those
/// files at line 1, which is what makes [`TITLE_SCAN_BYTES`] affordable.
///
/// Returns `None` for a rollout that is not worth a row: a sub-agent thread
/// (it belongs to its parent, not to the sidebar) or a session that never ran
/// a turn.
fn read_session_meta_inner(
    path: &Path,
    scan_usage: bool,
    cwd_filter: Option<&str>,
) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    // Line 1 must be session_meta or we skip the file.
    buf.clear();
    // US-010 (cli-hardening-followup-2026-Q3): cap line read at
    // SESSION_META_MAX_BYTES. Truncated line fails serde_json parse below
    // and the file is skipped -- same outcome as a malformed line.
    let n = reader
        .by_ref()
        .take(SESSION_META_MAX_BYTES)
        .read_line(&mut buf)
        .ok()?;
    if n == 0 {
        return None;
    }
    if n as u64 == SESSION_META_MAX_BYTES && !buf.ends_with('\n') {
        log::warn!(
            target: "paneflow_app::codex_sessions",
            "session JSONL line truncated at {} bytes for {} -- skipping file",
            SESSION_META_MAX_BYTES,
            path.display(),
        );
        return None;
    }
    let first_value: serde_json::Value = serde_json::from_str(buf.trim_end()).ok()?;
    if first_value.get("type").and_then(|v| v.as_str()) != Some("session_meta") {
        return None;
    }
    let payload = first_value.get("payload")?;
    // A sub-agent spawned by `/root` gets its own rollout file, tagged
    // `thread_source:"subagent"` with the spawning thread in
    // `payload.source.subagent.thread_spawn.parent_thread_id`. Its
    // `payload.id` is its own, so without this guard every sub-agent turn
    // surfaced in the sidebar as a first-class session (four such rows in a
    // single day of real use, all untitled). The parent thread already
    // represents that work.
    if payload.get("thread_source").and_then(|v| v.as_str()) == Some("subagent") {
        return None;
    }
    let session_id = payload.get("id").and_then(|v| v.as_str())?.to_string();
    let cwd = payload.get("cwd").and_then(|v| v.as_str())?.to_string();
    if cwd.is_empty() {
        return None;
    }
    if let Some(want) = cwd_filter
        && !crate::agent_sessions::cwd_matches(&cwd, want)
    {
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

    // Codex records the branch it started on in `session_meta.payload.git`
    // (alongside `commit_hash` and `repository_url`). Control-char guard for
    // the same reason as `cwd` above: it is display-only today.
    let git_branch = payload
        .get("git")
        .and_then(|g| g.get("branch"))
        .and_then(|v| v.as_str())
        .filter(|b| !b.chars().any(char::is_control))
        .unwrap_or("")
        .to_string();

    // Title-only path keeps the cheap first-user-message scan untouched. The
    // attribution path runs the deeper tail scan (model + usage).
    let scan = if scan_usage {
        scan_tail_with_usage(&mut reader)
    } else {
        scan_head_for_title(&mut reader)
    };

    // A rollout whose only line is `session_meta` is a thread the user opened
    // and closed without sending anything. It has no title and nothing to
    // resume, so it must not take a sidebar row - it would render as a raw
    // hex id. Keyed on "the scan saw a body record at all" rather than on the
    // summary, so a session whose prompt is unlabelable still gets its row.
    if !scan.saw_activity {
        return None;
    }

    Some(SessionMeta {
        agent: SessionAgent::Codex,
        session_id,
        timestamp,
        cwd,
        git_branch,
        summary: scan.summary,
        model: scan.model,
        usage: scan.usage,
    })
}

/// What one head/tail scan pass recovered from a rollout body.
#[derive(Default)]
struct RolloutScan {
    summary: Option<String>,
    model: Option<String>,
    usage: Option<AssistantUsage>,
    /// Whether any body record was parsed at all - the empty-session signal.
    saw_activity: bool,
}

/// EP-004 US-016: deeper tail scan for the attribution path. Walks up to
/// [`MODEL_USAGE_SCAN_LIMIT`] lines capturing the first user message (label),
/// the model (`turn_context.payload.model`), and the LAST cumulative
/// `token_count` usage event. Codex reports `token_count` as a running total,
/// so the last one wins (not summed). Usage is normalized to the shared
/// [`AssistantUsage`] tier semantics (input = uncached input, cache_read =
/// cached subset) so the pricing table treats Claude and Codex uniformly.
fn scan_tail_with_usage(reader: &mut BufReader<fs::File>) -> RolloutScan {
    let mut scan = RolloutScan::default();
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
        let record_type = value.get("type").and_then(|v| v.as_str());
        if is_activity_record(record_type) {
            scan.saw_activity = true;
        }
        if scan.summary.is_none() {
            scan.summary = user_text_from_record(&value);
        }
        match record_type {
            Some("turn_context") => {
                if scan.model.is_none()
                    && let Some(m) = value
                        .get("payload")
                        .and_then(|p| p.get("model"))
                        .and_then(|v| v.as_str())
                    && !m.is_empty()
                {
                    scan.model = Some(m.to_string());
                }
            }
            Some("event_msg") => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("type").and_then(|v| v.as_str()) == Some("token_count")
                    && let Some(total) =
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
                        scan.usage = Some(u);
                    }
                }
            }
            _ => {}
        }
    }
    scan
}

/// Scan the head of a rollout for the first human-authored message, bounded by
/// [`TITLE_SCAN_LIMIT`] lines AND [`TITLE_SCAN_BYTES`].
///
/// Signature is concrete on `BufReader<File>` rather than the
/// generic `R: BufRead` it used to be: the `by_ref().take()`
/// pattern needed by US-010 for the per-line byte cap fails to
/// type-check against `&mut R` (the compiler auto-derefs to `R`
/// and the move blocks the borrow). The only call site already
/// passes a `BufReader<File>`, so the generic was vestigial.
fn scan_head_for_title(reader: &mut BufReader<fs::File>) -> RolloutScan {
    let mut scan = RolloutScan::default();
    let mut buf = String::new();
    let mut budget = TITLE_SCAN_BYTES;
    for _ in 0..TITLE_SCAN_LIMIT {
        if budget == 0 {
            break;
        }
        buf.clear();
        // US-010 (cli-hardening-followup-2026-Q3): cap each line read.
        // Oversize lines fall through to `serde_json::from_str` which
        // errors and the loop `continue`s -- the scan moves on to the
        // next chunk without OOMing.
        let n = match reader
            .by_ref()
            .take(MAX_LINE_BYTES.min(budget))
            .read_line(&mut buf)
        {
            Ok(n) => n,
            Err(_) => break,
        };
        if n == 0 {
            break;
        }
        budget = budget.saturating_sub(n as u64);
        let trimmed = buf.trim_end();
        if !trimmed.starts_with('{') {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if is_activity_record(value.get("type").and_then(|v| v.as_str())) {
            scan.saw_activity = true;
        }
        if let Some(text) = user_text_from_record(&value) {
            scan.summary = Some(text);
            break;
        }
    }
    scan
}

/// Whether this record proves the rollout carries a real turn. Codex writes a
/// `session_meta`-only file for a thread the user opened and closed without
/// sending anything; any body record at all means the session actually ran.
fn is_activity_record(record_type: Option<&str>) -> bool {
    matches!(record_type, Some("response_item") | Some("event_msg"))
}

/// Extract the first human-authored text carried by one rollout record.
/// Three shapes, all of which appear in the wild:
///
/// 1. `event_msg` / `item_completed` with `item.type == "UserMessage"` - the
///    authoritative marker in Codex 0.149.x. It fires exactly once per real
///    user turn (three occurrences in a 1675-line rollout with three prompts)
///    and never for an injected envelope.
/// 2. `response_item` / `message` / `role == "user"` with `input_text`
///    content - the same turn, one line earlier, and the only shape older
///    rollouts carry. It also carries every injected envelope, hence the
///    [`SYNTHETIC_USER_PREFIXES`] filter in [`clean_user_message`].
/// 3. `event_msg` / `user_message` / `message` - the pre-0.149 shape. Codex no
///    longer emits it (zero occurrences across current rollouts), which is why
///    the sidebar fell back to raw hex ids; kept so older sessions keep their
///    label.
fn user_text_from_record(value: &serde_json::Value) -> Option<String> {
    let payload = value.get("payload")?;
    match value.get("type").and_then(|v| v.as_str())? {
        "event_msg" => match payload.get("type").and_then(|v| v.as_str())? {
            "item_completed" => {
                let item = payload.get("item")?;
                if item.get("type").and_then(|v| v.as_str()) != Some("UserMessage") {
                    return None;
                }
                first_labelable_block(item.get("content")?.as_array()?, "text")
            }
            "user_message" => clean_user_message(payload.get("message")?.as_str()?),
            _ => None,
        },
        "response_item" => {
            if payload.get("type").and_then(|v| v.as_str()) != Some("message")
                || payload.get("role").and_then(|v| v.as_str()) != Some("user")
            {
                return None;
            }
            first_labelable_block(payload.get("content")?.as_array()?, "input_text")
        }
        _ => None,
    }
}

/// First content block of type `kind` that survives [`clean_user_message`].
/// Codex packs several blocks into one record (a real 0.149.1 prompt line
/// carries three), and the envelopes come first, so this walks past them
/// rather than judging the record on its opening block alone.
fn first_labelable_block(blocks: &[serde_json::Value], kind: &str) -> Option<String> {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some(kind))
        .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
        .find_map(clean_user_message)
}

/// Clean one candidate label, rejecting the synthetic envelopes Codex injects
/// as `role:"user"` records around the real prompt.
fn clean_user_message(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start();
    if SYNTHETIC_USER_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return None;
    }
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

        let title_only = read_session_meta_inner(&path, false, None).expect("meta");
        assert!(title_only.model.is_none());
        assert!(title_only.usage.is_none());

        let with_usage = read_session_meta_inner(&path, true, None).expect("meta");
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
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("legitimate UUID must pass the guard");
        assert_eq!(meta.session_id, "019dc9ea-38d7-7372-9cc4-253ce944d41b");
    }

    /// Codex 0.149.1 shape: the real prompt arrives as a `response_item`
    /// `role:"user"` line and again as an `event_msg` `item_completed`
    /// `UserMessage`. Neither existed in the matcher, which is why every
    /// recent Codex row rendered as a raw hex id. The injected envelopes that
    /// precede it (`<app-context>`, `# AGENTS.md`, `<recommended_plugins>`)
    /// must not win the title race.
    #[test]
    fn read_session_meta_skips_injected_envelopes_and_takes_the_real_prompt() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("rollout-0149.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"01a0323c-fb2b-7af3-9386-742cd0cfb4a6","cwd":"/home/arthur/dev/paneflow","timestamp":"2026-08-24T05:27:32.000Z","thread_source":"user","git":{"branch":"main","commit_hash":"04e0ae0b"}}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"<app-context>desktop</app-context>"}]}}"#,
                "\n",
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>\nnope\n</recommended_plugins>"},{"type":"input_text","text":"# AGENTS.md instructions for /home/arthur/dev/paneflow"}]}}"##,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Corrige la sidebar agent sessions"}]}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"Corrige la sidebar agent sessions"}]}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(
            meta.summary.as_deref(),
            Some("Corrige la sidebar agent sessions")
        );
        // session_meta.payload.git.branch, not the hardcoded empty string the
        // reader used to emit.
        assert_eq!(meta.git_branch, "main");
    }

    /// The `item_completed` marker alone must produce a label: it is the only
    /// user-turn signal a rollout carries once the `response_item` line
    /// outruns the per-line cap.
    #[test]
    fn item_completed_user_message_yields_the_label() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("item-completed.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"s","cwd":"/p","timestamp":"2026-08-24T05:27:32.000Z"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"ship it"}]}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(meta.summary.as_deref(), Some("ship it"));
    }

    /// A sub-agent thread carries its own `payload.id` and would otherwise
    /// list as a first-class session next to its parent.
    #[test]
    fn subagent_rollout_is_not_a_session_row() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("subagent.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"01a032cf-f359-7571-87e2-fb9c9d351de9","session_id":"01a032cd-d49f-7732-b6de-ab083bbcca92","cwd":"/p","timestamp":"2026-08-24T10:08:04.000Z","thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":"01a032cd-d49f-7732-b6de-ab083bbcca92","depth":1}}}}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"review the standards"}]}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(
            read_session_meta(&path).is_none(),
            "a subagent thread belongs to its parent, not to the sidebar"
        );
    }

    /// A thread opened and closed without a turn is a `session_meta`-only
    /// file. It has no title and nothing to resume.
    #[test]
    fn session_meta_only_rollout_is_dropped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"01a037fc-4515-7800-9a3c-000000000000","cwd":"/p","timestamp":"2026-08-25T10:14:34.000Z","thread_source":"user"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(read_session_meta(&path).is_none());
    }

    /// `cwd_filter` is what keeps the byte budget affordable: a rollout from
    /// another project must be abandoned before its body is read.
    #[test]
    fn cwd_filter_rejects_a_foreign_rollout_at_line_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("other-project.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"session_meta","payload":{"id":"s","cwd":"/home/arthur/dev/other","timestamp":"2026-08-24T05:27:32.000Z"}}"#,
                "\n",
                r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"hello"}]}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(read_session_meta_inner(&path, false, Some("/home/arthur/dev/paneflow")).is_none());
        assert!(
            read_session_meta_inner(&path, false, Some("/home/arthur/dev/other")).is_some(),
            "the matching cwd must still produce a row"
        );
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
