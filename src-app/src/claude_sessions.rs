//! Claude Code session discovery - reads the on-disk session store at
//! `~/.claude/projects/<slug>/<uuid>.jsonl` and produces unified
//! [`SessionMeta`](crate::agent_sessions::SessionMeta) entries for the
//! sessions popover.
//!
//! There is no public Claude Code API for listing sessions (issue #34318);
//! the `.jsonl` files are the source of truth. Each line is one event;
//! the *first* line that carries `cwd` is the session envelope. The
//! LLM-generated `type:"ai-title"` record (when present) provides the
//! human-readable label that the `claude --resume` picker shows.
//!
//! All filesystem work happens off the GPUI main thread - call
//! [`read_sessions_for_cwd`] from inside `smol::unblock`.

use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::agent_sessions::{AssistantUsage, SessionAgent, SessionMeta, clean_session_label};

/// Maximum number of leading lines to scan for envelope + title. The
/// first lines of a Claude Code session file are typically
/// `permission-mode` and `file-history-snapshot` records with no `cwd`;
/// the actual user/assistant events start on line 3+. The `ai-title`
/// record (when present) is usually around line 9 but can appear later.
/// 256 covers >95% of files in the wild without the scan being visible
/// to the user.
const TITLE_SCAN_LIMIT: usize = 256;

/// EP-004 US-016: deeper line cap for the attribution scan, which walks PAST
/// the title break to aggregate `message.usage` across assistant turns. A
/// session's turns are spread through the file, so this is much larger than
/// [`TITLE_SCAN_LIMIT`] - but still bounded, and it runs ONLY on the attribution
/// path (the diff column load), never on the popover title scan. 20k lines
/// covers very long sessions while keeping a pathological file bounded.
const MODEL_USAGE_SCAN_LIMIT: usize = 20_000;

// US-013: per-line JSONL read cap, centralized (see `crate::limits`).
use crate::limits::MAX_LINE_BYTES;

/// Cap rendered first-user-message labels at this character count to keep
/// the popover row from overflowing horizontally.
const LABEL_MAX_CHARS: usize = 80;

/// First-line envelope. Tolerant: any missing field falls back via
/// `serde(default)` so the parser never bails on a forward-compatible schema
/// change.
#[derive(Debug, Deserialize)]
struct FirstLineEnvelope {
    #[serde(default, rename = "sessionId")]
    session_id: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    cwd: String,
    #[serde(default, rename = "gitBranch")]
    git_branch: String,
}

/// Convert an absolute path into the slug Claude Code uses as the directory
/// name under `~/.claude/projects/`. Algorithm (matches Claude Code's own
/// encoder): every character that is **not** ASCII alphanumeric becomes `-`.
/// That covers `/`, `\`, the Windows drive `:` (so `C:\dev\paneflow` →
/// `C--dev-paneflow`, NOT `C:-dev-paneflow`), spaces (`C:\Program Files\..`
/// → `C--Program-Files-..`), and `.` (so `/home/u/.claude` → `-home-u--claude`,
/// the dir Claude Code actually writes). Runs of separators are NOT collapsed -
/// `C:\` produces the literal `C--`. No percent-encoding or hashing.
///
/// The previous encoder only replaced `/` and `\`, leaving the drive `:`
/// intact: on Windows it produced `C:-dev-paneflow` while the on-disk dir is
/// `C--dev-paneflow`, so `read_dir` opened a path that never existed and the
/// sessions sidebar came up empty.
pub fn slug_for_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Compute the absolute path of `~/.claude/projects/<slug>/`. Returns
/// `None` when `dirs::home_dir()` fails (no `$HOME` / `%USERPROFILE%`).
///
/// A trailing separator is normalized away first. Claude derives its own slug
/// from the agent process's cwd, which never carries one, so `/a/b/` has to
/// resolve to the same directory as `/a/b` - otherwise the lookup misses a
/// directory that exists, and [`session_file_exists`] reports "no session"
/// for a session that is very much there, sending `--session-id` for an id
/// the CLI already knows.
pub fn project_dir_for_cwd(cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let slug = slug_for_cwd(normalize_cwd_for_slug(cwd));
    Some(home.join(".claude").join("projects").join(slug))
}

/// Strip trailing path separators, unless that would reduce `cwd` to a bare
/// root. `/` is all separator and `C:\` is a drive root: trimming those
/// changes the slug (`-` → ``, `C--` → `C-`) instead of normalizing it, and
/// Claude keeps them intact.
fn normalize_cwd_for_slug(cwd: &str) -> &str {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        cwd
    } else {
        trimmed
    }
}

/// Whether the CLI already holds a session file for `session_id` under
/// `cwd`'s project dir.
///
/// Drives the mint-vs-resume choice in
/// [`crate::agent_launcher::SessionBinding::resolve`]: `--session-id` is only
/// legal for an id the CLI has never seen, so a thread may only pass it on
/// the launch that creates the session.
///
/// Unlike the readers below this is a single `stat` - it never walks or
/// parses the project dir - so it is cheap enough for the PTY-mount path on
/// the GPUI main thread. Ids are filtered through
/// [`crate::agent_sessions::is_valid_session_id`] first, whose allow-list
/// admits only ASCII alphanumerics plus `-`/`_` (and never a leading `-`):
/// no separator, no `.`, no space, no shell metacharacter. A tampered
/// `session.json` can therefore neither escape the project dir here nor
/// smuggle an extra argument into the launch command.
pub fn session_file_exists(cwd: &str, session_id: &str) -> bool {
    if !crate::agent_sessions::is_valid_session_id(session_id) {
        return false;
    }
    project_dir_for_cwd(cwd)
        .map(|dir| dir.join(format!("{session_id}.jsonl")).is_file())
        .unwrap_or(false)
}

fn project_snapshot_mtime(project_dir: &Path) -> Option<SystemTime> {
    let mut latest = fs::metadata(project_dir)
        .ok()
        .and_then(|m| m.modified().ok());
    let entries = fs::read_dir(project_dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !is_jsonl_file(&path) {
            continue;
        }
        let modified = entry.metadata().ok().and_then(|m| m.modified().ok());
        latest = max_mtime(latest, modified);
    }

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

/// Read all Claude Code session metadata for the given working directory.
/// Sessions are sorted by timestamp descending (most recent first) and only
/// those whose first-line `cwd` matches `cwd` (via
/// [`cwd_matches`](crate::agent_sessions::cwd_matches): exact on Unix,
/// case/separator-insensitive on Windows) are kept - dedupes the rare slug
/// collision where two distinct paths produce the same directory name
/// (`/a/b-c` and `/a/b/c` both slug to `-a-b-c`).
///
/// **Blocking I/O** - call from inside `smol::unblock` or
/// `cx.background_executor`. Never invoke on the GPUI main thread.
pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

/// Like [`read_sessions_for_cwd`], but also reports how many older matching
/// sessions were omitted by the sidebar retention cap.
pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(project_dir) = project_dir_for_cwd(cwd) else {
        return (Vec::new(), 0);
    };
    // Existing Claude sessions are append-only JSONL files. Appending to a
    // file does not reliably change the parent directory mtime, so include
    // leaf-file mtimes in the cache fingerprint.
    let snapshot_mtime = project_snapshot_mtime(&project_dir);
    if let Some(snapshot_mtime) = snapshot_mtime
        && let Some(cached) = crate::agent_sessions::cache::lookup_with_mtime(
            SessionAgent::Claude,
            cwd,
            snapshot_mtime,
        )
    {
        return cached;
    }
    let Ok(entries) = fs::read_dir(&project_dir) else {
        return (Vec::new(), 0);
    };

    let sessions = entries.flatten().filter_map(|entry| {
        let path = entry.path();
        if !is_jsonl_file(&path) {
            return None;
        }
        read_session_meta(&path).filter(|meta| crate::agent_sessions::cwd_matches(&meta.cwd, cwd))
    });

    let (sessions, omitted) = crate::agent_sessions::collect_recent_sessions(
        sessions,
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    );
    if let Some(snapshot_mtime) = project_snapshot_mtime(&project_dir) {
        crate::agent_sessions::cache::store_result_with_mtime(
            SessionAgent::Claude,
            cwd,
            snapshot_mtime,
            &sessions,
            omitted,
        );
    }
    (sessions, omitted)
}

/// EP-004 US-014/US-016: like [`read_sessions_for_cwd`] but the retained
/// attribution candidates are scanned deeper to populate `model` + aggregated
/// `usage`. Deliberately bypasses the title-scan mtime cache - that cache
/// stores usage-less rows for the popover, and the attribution result is
/// instead cached on the diff `Column` keyed to its diff fingerprint
/// (re-fetched only on re-diff). **Blocking I/O** - call from inside
/// `smol::unblock`.
pub fn read_sessions_with_usage_for_attribution(cwd: &str, branch: &str) -> Vec<SessionMeta> {
    let Some(project_dir) = project_dir_for_cwd(cwd) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&project_dir) else {
        return Vec::new();
    };

    let mut candidates: Vec<(SessionMeta, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_jsonl_file(&path) {
            continue;
        }
        if let Some(meta) = read_session_meta(&path)
            && crate::agent_sessions::cwd_matches(&meta.cwd, cwd)
        {
            crate::agent_sessions::push_ranked_attribution(
                &mut candidates,
                meta,
                path,
                branch,
                crate::agent_sessions::DIFF_ATTRIBUTION_MATCH_CAP,
            );
        }
    }

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

fn is_jsonl_file(path: &Path) -> bool {
    path.is_file()
        && path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jsonl"))
}

/// Read the head of a `.jsonl` and collect everything we need for a UI
/// row in a single pass: the first envelope carrying `cwd`, the
/// LLM-generated `ai-title` (when present), and the cleaned first
/// `type:"user"` message (used as a fallback title when `ai-title` is
/// absent - typical of sessions older than that feature's introduction).
///
/// Title priority:
/// 1. `type:"ai-title"` → `aiTitle` field. Matches what the
///    `claude --resume` picker shows for newer sessions.
/// 2. First `type:"user"` message, with `<command-*>` boilerplate
///    collapsed into `/<name> <args>` when present.
fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    read_session_meta_inner(path, false)
}

/// Shared session-head scan. `scan_usage = false` is the title-only popover
/// path: bounded by [`TITLE_SCAN_LIMIT`], it stops as soon as the envelope +
/// title are known. `scan_usage = true` is the EP-004 attribution path: it
/// walks past the title (bounded by [`MODEL_USAGE_SCAN_LIMIT`]) aggregating
/// `message.usage` across assistant turns and capturing `message.model`.
fn read_session_meta_inner(path: &Path, scan_usage: bool) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    let mut envelope: Option<FirstLineEnvelope> = None;
    let mut ai_title: Option<String> = None;
    let mut user_fallback: Option<String> = None;
    // US-016: model + aggregated usage (attribution path only).
    let mut model: Option<String> = None;
    let mut usage = AssistantUsage::default();
    let mut saw_usage = false;

    let scan_limit = if scan_usage {
        MODEL_USAGE_SCAN_LIMIT
    } else {
        TITLE_SCAN_LIMIT
    };
    for _ in 0..scan_limit {
        buf.clear();
        // US-010 (cli-hardening-followup-2026-Q3): cap each line read
        // at MAX_LINE_BYTES. An agent can write to
        // `~/.claude/projects/<slug>/` (it's the very directory Claude
        // Code persists sessions to), so a malicious 500 MB
        // single-line JSONL would otherwise allocate fully on a
        // background smol::unblock thread before the
        // TITLE_SCAN_LIMIT count guard fires. Truncation surfaces as
        // a partial line that fails serde_json::from_str and is
        // skipped on `continue` below; the file's session entry is
        // simply omitted, not the entire scan.
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES)
            .read_line(&mut buf)
            .ok()?;
        if n == 0 {
            break;
        }
        if n as u64 == MAX_LINE_BYTES && !buf.ends_with('\n') {
            // U-017: an exactly-MAX_LINE_BYTES line with no trailing newline is
            // ambiguous - it may be a genuinely TRUNCATED oversized line, or a
            // COMPLETE final record written without a final EOL. Peek one byte
            // to disambiguate: empty = EOF = the line is complete, fall through
            // and parse it (don't drop a valid final session). Non-empty = more
            // bytes follow = the cap truncated it mid-line → genuinely oversized.
            let more_follows = match reader.fill_buf() {
                Ok(b) => !b.is_empty(),
                // I/O error mid-read: abort like the drain loop below (don't
                // silently fall through and parse a possibly-truncated buf).
                Err(_) => return None,
            };
            if more_follows {
                // Newer Claude Code writes oversized records ahead of the
                // envelope -- notably a `type:"queue-operation"` first line
                // whose `content` blob can run to hundreds of KB and which
                // carries no `cwd`. Abandoning the file here dropped the
                // whole session from the sidebar (and logged a WARN per
                // file on every open). Instead, discard the rest of this
                // one overlong line in bounded chunks -- preserving the
                // US-010 anti-OOM guard, since we never buffer the tail --
                // and keep scanning: the envelope lands on a later,
                // normal-sized line.
                log::debug!(
                    target: "paneflow_app::claude_sessions",
                    "skipped an oversized (>{} B) line in {}; continuing scan for the envelope",
                    MAX_LINE_BYTES,
                    path.display(),
                );
                loop {
                    let chunk = match reader.fill_buf() {
                        Ok(b) => b,
                        Err(_) => return None,
                    };
                    if chunk.is_empty() {
                        return None; // EOF mid-line: nothing more to find.
                    }
                    if let Some(nl) = chunk.iter().position(|&b| b == b'\n') {
                        reader.consume(nl + 1);
                        break;
                    }
                    let consumed = chunk.len();
                    reader.consume(consumed);
                }
                continue;
            }
            // EOF after exactly MAX_LINE_BYTES: `buf` is a complete final
            // record - fall through to the normal parse below.
        }
        let trimmed = buf.trim_end();
        if !trimmed.starts_with('{') {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if envelope.is_none()
            && value
                .get("cwd")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
            && let Ok(parsed) = serde_json::from_value::<FirstLineEnvelope>(value.clone())
            && !parsed.cwd.is_empty()
        {
            // session_id lands in `claude --resume <id>`, so hold it to the
            // strict `^[A-Za-z0-9_-]+$` allow-list (Claude ids are UUIDs):
            // rejects a `\r`/`\n` that would submit injected text and a
            // `;`/space that would chain a second shell command. cwd lands in
            // display chrome today but a future `cd <cwd>` prefix would inherit
            // the gap; a path legitimately carries `/` + spaces, so keep the
            // control-char guard for it. Guard both at the gate, not the
            // consumer.
            if !crate::agent_sessions::is_valid_session_id(&parsed.session_id)
                || parsed.cwd.chars().any(|c| c.is_control())
            {
                log::warn!(
                    "claude_sessions: dropped {} -- envelope carries an invalid session_id or control chars in cwd",
                    path.display(),
                );
                continue;
            }
            envelope = Some(parsed);
        }

        match value.get("type").and_then(|v| v.as_str()) {
            Some("ai-title") => {
                if let Some(title) = value.get("aiTitle").and_then(|v| v.as_str())
                    && let Some(cleaned) = clean_session_label(title, LABEL_MAX_CHARS)
                {
                    ai_title = Some(cleaned);
                    // Title-only path: stop as soon as we have envelope + title.
                    // Attribution path: keep walking to aggregate usage/model.
                    if envelope.is_some() && !scan_usage {
                        break;
                    }
                }
            }
            Some("user") if user_fallback.is_none() => {
                if let Some(text) = extract_user_content(&value)
                    && let Some(cleaned) = clean_user_message(&text)
                {
                    user_fallback = Some(cleaned);
                }
            }
            // US-016: assistant turns carry `message.model` + `message.usage`.
            // Aggregate usage across turns; keep the most recent non-empty model
            // (overwrite - a session that switched models reports the last one,
            // which is the most representative for a single-figure estimate).
            Some("assistant") if scan_usage => {
                if let Some(message) = value.get("message") {
                    if let Some(m) = message.get("model").and_then(|v| v.as_str())
                        && !m.is_empty()
                    {
                        model = Some(m.to_string());
                    }
                    if let Some(u) = message.get("usage") {
                        let turn = AssistantUsage {
                            input: u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                            output: u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
                            cache_read: u
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                            cache_creation: u
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0),
                        };
                        if !turn.is_empty() {
                            usage.add(&turn);
                            saw_usage = true;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let envelope = envelope?;
    let summary = ai_title.or(user_fallback);

    Some(SessionMeta {
        agent: SessionAgent::Claude,
        session_id: envelope.session_id,
        timestamp: envelope.timestamp,
        cwd: envelope.cwd,
        git_branch: envelope.git_branch,
        summary,
        model,
        usage: saw_usage.then_some(usage),
    })
}

fn extract_user_content(line: &serde_json::Value) -> Option<String> {
    let content = line.get("message")?.get("content")?;
    if let Some(s) = content.as_str() {
        return Some(s.to_string());
    }
    if let Some(arr) = content.as_array() {
        for block in arr {
            if block.get("type").and_then(|v| v.as_str()) == Some("text")
                && let Some(text) = block.get("text").and_then(|v| v.as_str())
                && !text.is_empty()
            {
                return Some(text.to_string());
            }
        }
    }
    None
}

fn clean_user_message(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(name) = extract_xml_block(trimmed, "command-name") {
        let args = extract_xml_block(trimmed, "command-args").unwrap_or_default();
        let joined = if args.is_empty() {
            name
        } else {
            format!("{name} {args}")
        };
        return clean_session_label(&joined, LABEL_MAX_CHARS);
    }

    clean_session_label(trimmed, LABEL_MAX_CHARS)
}

fn extract_xml_block(haystack: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = haystack.find(&open)? + open.len();
    let end = haystack[start..].find(&close)? + start;
    Some(haystack[start..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_unix_path() {
        assert_eq!(slug_for_cwd("/home/alice/myapp"), "-home-alice-myapp");
    }

    #[test]
    fn slug_replaces_spaces() {
        // Spaces are non-alphanumeric, so they become `-` like every other
        // separator (real example: `C:\Program Files\PaneFlow` →
        // `C--Program-Files-PaneFlow`).
        assert_eq!(
            slug_for_cwd("/home/alice/my project"),
            "-home-alice-my-project"
        );
    }

    #[test]
    fn slug_replaces_dots() {
        // A leading-dot segment is NOT preserved: the `.` becomes `-`, so a
        // dotfile dir produces a double dash (`/home/arthur/.claude` →
        // `-home-arthur--claude`, the dir Claude Code writes on Linux).
        assert_eq!(slug_for_cwd("/home/alice/.config"), "-home-alice--config");
    }

    #[test]
    fn slug_windows_path_replaces_drive_colon() {
        // Regression guard: the drive `:` MUST become `-`. The old encoder left
        // it as `C:-Users-alice-myapp`, which never matched the on-disk
        // `C--Users-alice-myapp` and emptied the sidebar on Windows.
        assert_eq!(
            slug_for_cwd("C:\\Users\\alice\\myapp"),
            "C--Users-alice-myapp"
        );
    }

    #[test]
    fn slug_matches_real_windows_project_dir() {
        // Verified against a real install: Claude Code stores `C:\dev\paneflow`
        // sessions under `~/.claude/projects/C--dev-paneflow/`.
        assert_eq!(slug_for_cwd("C:\\dev\\paneflow"), "C--dev-paneflow");
    }

    #[test]
    fn trailing_separator_resolves_to_the_same_project_dir() {
        // Claude's own cwd never carries a trailing separator, so a stored
        // `thread.cwd` that does must still find the directory that exists.
        // Getting this wrong makes `session_file_exists` report "no session"
        // for a live one, which re-sends `--session-id` and reproduces
        // `Session ID <uuid> is already in use`.
        assert_eq!(
            project_dir_for_cwd("/home/alice/myapp/"),
            project_dir_for_cwd("/home/alice/myapp")
        );
        assert_eq!(
            project_dir_for_cwd("/home/alice/myapp///"),
            project_dir_for_cwd("/home/alice/myapp")
        );
        assert_eq!(
            project_dir_for_cwd("C:\\dev\\paneflow\\"),
            project_dir_for_cwd("C:\\dev\\paneflow")
        );
    }

    #[test]
    fn bare_roots_keep_their_slug() {
        // All-separator paths are not normalized: trimming would change the
        // slug rather than canonicalize it.
        assert_eq!(normalize_cwd_for_slug("/"), "/");
        assert_eq!(normalize_cwd_for_slug("C:\\"), "C:\\");
        assert_eq!(slug_for_cwd(normalize_cwd_for_slug("/")), "-");
        assert_eq!(slug_for_cwd(normalize_cwd_for_slug("C:\\")), "C--");
    }

    #[test]
    fn session_file_exists_rejects_ids_outside_the_allow_list() {
        // The allow-list gate runs before any path join, so no id can escape
        // the project dir or reach the filesystem as a traversal.
        for hostile in ["../../etc/passwd", "a/b", "a\\b", "with space", ".hidden"] {
            assert!(
                !session_file_exists("/home/alice/myapp", hostile),
                "hostile id {hostile:?} must be rejected before the path join"
            );
        }
    }

    #[test]
    fn slug_root() {
        assert_eq!(slug_for_cwd("/"), "-");
    }

    #[test]
    fn read_session_meta_skips_leading_metadata_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir
            .path()
            .join("aaaaaaaa-1111-2222-3333-444444444444.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"permission-mode","permissionMode":"default","sessionId":"aaaaaaaa-1111-2222-3333-444444444444"}"#,
                "\n",
                r#"{"type":"file-history-snapshot","messageId":"x","snapshot":{"trackedFileBackups":{}},"isSnapshotUpdate":false}"#,
                "\n",
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hi"},"uuid":"x","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"aaaaaaaa-1111-2222-3333-444444444444","version":"2.1.119","gitBranch":"main"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"Implement feature X","sessionId":"aaaaaaaa-1111-2222-3333-444444444444"}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let meta = read_session_meta(&path).expect("envelope extracted");
        assert_eq!(meta.agent, SessionAgent::Claude);
        assert_eq!(meta.session_id, "aaaaaaaa-1111-2222-3333-444444444444");
        assert_eq!(meta.cwd, "/tmp/proj");
        assert_eq!(meta.timestamp, "2026-04-26T13:38:41.095Z");
        assert_eq!(meta.git_branch, "main");
        assert_eq!(meta.summary.as_deref(), Some("Implement feature X"));
    }

    #[test]
    fn ai_title_wins_over_first_user_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ordering.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"first user message body"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"s"}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"Fix the thing","sessionId":"s"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(meta.summary.as_deref(), Some("Fix the thing"));
    }

    #[test]
    fn ai_title_uses_same_label_normalization_as_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("title-normalized.jsonl");
        let long_title = format!("Fix\n\t{}{}", "a".repeat(100), "\u{1b}");
        std::fs::write(
            &path,
            format!(
                r#"{{"parentUuid":null,"type":"user","message":{{"role":"user","content":"first user message body"}},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"s"}}
{{"type":"ai-title","aiTitle":{}}}
"#,
                serde_json::to_string(&long_title).expect("json string")
            ),
        )
        .expect("write fixture");

        let meta = read_session_meta(&path).expect("meta");
        let summary = meta.summary.as_deref().expect("summary");
        assert!(!summary.contains('\n'));
        assert!(!summary.contains('\t'));
        assert!(summary.chars().count() <= LABEL_MAX_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn falls_back_to_first_user_message_when_no_ai_title() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("legacy.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"Refactor the auth flow"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"s"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(meta.summary.as_deref(), Some("Refactor the auth flow"));
    }

    #[test]
    fn cleans_slash_command_boilerplate_in_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("slash.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"<command-message>implement-story</command-message>\n<command-name>/implement-story</command-name>\n<command-args>@tasks/prd-x.md US-001</command-args>"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"s"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(
            meta.summary.as_deref(),
            Some("/implement-story @tasks/prd-x.md US-001")
        );
    }

    #[test]
    fn usage_scan_aggregates_across_assistant_turns_and_captures_model() {
        // EP-004 US-016: the deeper scan (scan_usage=true) walks past the title
        // break, sums `message.usage` across assistant turns, and captures the
        // model. The title-only scan (false) leaves both None.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("usage.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"550e8400-e29b-41d4-a716-446655440000","gitBranch":"main"}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"claude-opus-4-8-20260101","usage":{"input_tokens":100,"output_tokens":40,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}"#,
                "\n",
                r#"{"type":"ai-title","aiTitle":"Some title"}"#,
                "\n",
                r#"{"type":"assistant","message":{"model":"claude-opus-4-8-20260101","usage":{"input_tokens":200,"output_tokens":60,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        // Title-only path: no model/usage.
        let title_only = read_session_meta_inner(&path, false).expect("meta");
        assert!(title_only.model.is_none());
        assert!(title_only.usage.is_none());
        assert_eq!(title_only.summary.as_deref(), Some("Some title"));

        // Attribution path: aggregated usage + model.
        let with_usage = read_session_meta_inner(&path, true).expect("meta");
        assert_eq!(
            with_usage.model.as_deref(),
            Some("claude-opus-4-8-20260101")
        );
        let usage = with_usage.usage.expect("usage aggregated");
        assert_eq!(usage.input, 300);
        assert_eq!(usage.output, 100);
        assert_eq!(usage.cache_read, 10);
        assert_eq!(usage.cache_creation, 5);
    }

    #[test]
    fn truncate_label_caps_long_text() {
        let long = "a".repeat(120);
        let label = clean_user_message(&long).expect("label");
        assert_eq!(label.chars().count(), LABEL_MAX_CHARS + 1);
        assert!(label.ends_with('…'));
    }

    /// US-010 (cli-hardening-followup-2026-Q3): a JSONL file whose
    /// first line exceeds [`MAX_LINE_BYTES`] must NOT be loaded
    /// fully into memory. The truncated line fails the
    /// `serde_json::from_str` parse and the file is skipped --
    /// `read_session_meta` returns `None` without OOMing.
    #[test]
    fn read_session_meta_truncates_oversize_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversize.jsonl");
        // 1 MB single line, no newline. Well above MAX_LINE_BYTES.
        let big = "x".repeat(1024 * 1024);
        std::fs::write(&path, &big).expect("write fixture");
        // Sanity: input file is 1 MB.
        let meta = std::fs::metadata(&path).expect("metadata");
        assert_eq!(meta.len(), 1024 * 1024);
        // The reader caps at MAX_LINE_BYTES and surfaces None.
        assert!(read_session_meta(&path).is_none());
    }

    #[test]
    fn read_session_meta_returns_none_when_no_cwd_envelope_in_header() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("no-cwd.jsonl");
        std::fs::write(
            &path,
            r#"{"type":"permission-mode","permissionMode":"default","sessionId":"x"}
{"type":"file-history-snapshot","snapshot":{}}
"#,
        )
        .expect("write fixture");
        assert!(read_session_meta(&path).is_none());
    }

    #[test]
    fn session_id_control_char_guard() {
        // sessionId carries CR+LF + an injected shell command. Without
        // the guard, this id would flow into `claude --resume <id>` and
        // submit `rm -rf ~` as a separate PTY command.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("malicious.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"abc\r\nrm -rf ~","version":"2.1.119","gitBranch":"main"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(
            read_session_meta(&path).is_none(),
            "session with control chars in sessionId must be dropped"
        );
    }

    #[test]
    fn session_id_legitimate_uuid_passes_guard() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ok.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"550e8400-e29b-41d4-a716-446655440000","version":"2.1.119","gitBranch":"main"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        let meta = read_session_meta(&path).expect("legitimate UUID must pass the guard");
        assert_eq!(meta.session_id, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn cwd_control_char_guard() {
        // cwd is display-only today (prettify_cwd in sessions_sidebar) but
        // the same JSONL field could leak into a future `cd <cwd>`
        // prefix. Guard at the gate, not at each future consumer.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("malicious-cwd.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"hi"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj\r\nrm -rf ~","sessionId":"550e8400-e29b-41d4-a716-446655440000","version":"2.1.119","gitBranch":"main"}"#,
                "\n",
            ),
        )
        .expect("write fixture");
        assert!(
            read_session_meta(&path).is_none(),
            "session with control chars in cwd must be dropped"
        );
    }

    /// US-017 (audit P2-5): a second read_session_meta call against
    /// the same path returns the same content, and the
    /// cache::lookup/store cycle round-trips a fixture vector. This
    /// test exercises the cache primitives directly (the
    /// `read_sessions_for_cwd` path hits real `~/.claude/projects/`
    /// which we cannot reliably set up in unit tests).
    #[test]
    fn session_cache_round_trips_and_invalidates_on_mtime_change() {
        use crate::agent_sessions::cache;
        cache::clear();

        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = "/some/cwd";
        let project_dir = dir.path();

        // Empty cache: lookup returns None.
        assert!(
            cache::lookup(SessionAgent::Claude, cwd, project_dir).is_none(),
            "freshly-cleared cache must miss"
        );

        let fixture = vec![SessionMeta {
            agent: SessionAgent::Claude,
            session_id: "abc".into(),
            timestamp: "2026-04-26T13:00:00Z".into(),
            cwd: cwd.into(),
            git_branch: String::new(),
            summary: None,
            model: None,
            usage: None,
        }];
        cache::store_result(SessionAgent::Claude, cwd, project_dir, &fixture, 7);

        let (hit, omitted) = cache::lookup(SessionAgent::Claude, cwd, project_dir)
            .expect("post-store lookup must hit");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].session_id, "abc");
        assert_eq!(omitted, 7);

        // Touch the directory to bump its mtime; sleep long enough to
        // cross every mtime-granularity floor we care about: ext4/
        // APFS/NTFS report sub-millisecond mtimes, but FAT32/exFAT
        // round to 2 s and NFS without `actimeo` typically rounds to
        // 1 s. 2500 ms is the safe floor across the matrix (FAT32
        // 2 s + a healthy guard band) -- a faster sleep would silently
        // flake on Windows CI runners that fall back to FAT32-style
        // semantics or on NFS-mounted CI volumes.
        std::thread::sleep(std::time::Duration::from_millis(2500));
        std::fs::write(project_dir.join("touch.tmp"), b"x").expect("touch");

        assert!(
            cache::lookup(SessionAgent::Claude, cwd, project_dir).is_none(),
            "mtime bump must invalidate the cached entry"
        );
    }

    #[test]
    fn exactly_max_final_line_is_parsed_not_dropped() {
        // U-017: a complete envelope exactly MAX_LINE_BYTES long with no
        // trailing newline (a final record written without a final EOL) must be
        // parsed, not misclassified as oversized and dropped.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("exact.jsonl");
        let prefix =
            r#"{"sessionId":"550e8400-e29b-41d4-a716-446655440000","cwd":"/tmp/proj","p":""#;
        let suffix = r#""}"#;
        let pad = MAX_LINE_BYTES as usize - prefix.len() - suffix.len();
        let line = format!("{prefix}{}{suffix}", "x".repeat(pad));
        assert_eq!(
            line.len() as u64,
            MAX_LINE_BYTES,
            "fixture must be exactly the cap"
        );
        std::fs::write(&path, &line).expect("write"); // no trailing newline
        let meta = read_session_meta(&path).expect("exactly-MAX complete record must parse");
        assert_eq!(meta.cwd, "/tmp/proj");
    }

    #[test]
    fn genuinely_oversized_line_is_skipped() {
        // U-017: a line longer than MAX_LINE_BYTES (the cap truncates it
        // mid-line, more bytes follow) is still classified oversized and
        // dropped - the peek sees a non-empty buffer, not EOF.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversized.jsonl");
        let line = format!(
            r#"{{"cwd":"/tmp/proj","p":"{}"#,
            "x".repeat(MAX_LINE_BYTES as usize + 2000)
        );
        std::fs::write(&path, &line).expect("write"); // truncated, no close/newline
        assert!(
            read_session_meta(&path).is_none(),
            "an oversized line must be skipped, not parsed"
        );
    }
}
