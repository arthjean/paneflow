use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::agent_sessions::{AssistantUsage, SessionAgent, SessionMeta, clean_session_label};

const TITLE_SCAN_LIMIT: usize = 2048;

const TITLE_SCAN_BYTES: u64 = 1024 * 1024;

const MODEL_USAGE_SCAN_LIMIT: usize = 20_000;

use crate::limits::MAX_LINE_BYTES;

const LABEL_MAX_CHARS: usize = 80;

const SYNTHETIC_USER_PREFIXES: [&str; 2] = ["<local-command-", "<system-reminder>"];

const CONTEXT_RESET_COMMANDS: [&str; 2] = ["clear", "compact"];

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

pub fn slug_for_cwd(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

pub fn project_dir_for_cwd(cwd: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let slug = slug_for_cwd(normalize_cwd_for_slug(cwd));
    Some(home.join(".claude").join("projects").join(slug))
}

fn normalize_cwd_for_slug(cwd: &str) -> &str {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    if trimmed.is_empty() || trimmed.ends_with(':') {
        cwd
    } else {
        trimmed
    }
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

pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    let Some(project_dir) = project_dir_for_cwd(cwd) else {
        return (Vec::new(), 0);
    };
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

fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    read_session_meta_inner(path, false)
}

pub fn read_generated_title(path: &Path) -> Option<String> {
    scan_session_head(path, false)?.ai_title
}

struct SessionHead {
    envelope: FirstLineEnvelope,
    ai_title: Option<String>,
    user_fallback: Option<String>,
    model: Option<String>,
    usage: Option<AssistantUsage>,
}

fn read_session_meta_inner(path: &Path, scan_usage: bool) -> Option<SessionMeta> {
    let head = scan_session_head(path, scan_usage)?;
    Some(SessionMeta {
        agent: SessionAgent::Claude,
        session_id: head.envelope.session_id,
        timestamp: head.envelope.timestamp,
        cwd: head.envelope.cwd,
        git_branch: head.envelope.git_branch,
        summary: head.ai_title.or(head.user_fallback),
        model: head.model,
        usage: head.usage,
    })
}

fn scan_session_head(path: &Path, scan_usage: bool) -> Option<SessionHead> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    let mut envelope: Option<FirstLineEnvelope> = None;
    let mut ai_title: Option<String> = None;
    let mut user_fallback: Option<String> = None;
    let mut model: Option<String> = None;
    let mut usage = AssistantUsage::default();
    let mut saw_usage = false;

    let scan_limit = if scan_usage {
        MODEL_USAGE_SCAN_LIMIT
    } else {
        TITLE_SCAN_LIMIT
    };
    let mut title_budget = TITLE_SCAN_BYTES;
    for _ in 0..scan_limit {
        if !scan_usage && title_budget < MAX_LINE_BYTES {
            break;
        }
        buf.clear();
        let n = reader
            .by_ref()
            .take(MAX_LINE_BYTES)
            .read_line(&mut buf)
            .ok()?;
        if n == 0 {
            break;
        }
        title_budget = title_budget.saturating_sub(n as u64);
        if n as u64 == MAX_LINE_BYTES && !buf.ends_with('\n') {
            let more_follows = match reader.fill_buf() {
                Ok(b) => !b.is_empty(),
                Err(_) => return None,
            };
            if more_follows {
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
                        return None;
                    }
                    if let Some(nl) = chunk.iter().position(|&b| b == b'\n') {
                        reader.consume(nl + 1);
                        break;
                    }
                    let consumed = chunk.len();
                    reader.consume(consumed);
                    title_budget = title_budget.saturating_sub(consumed as u64);
                }
                continue;
            }
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
                    if envelope.is_some() && !scan_usage {
                        break;
                    }
                }
            }
            Some("user") if user_fallback.is_none() && !json_flag(&value, "isSidechain") => {
                if let Some(text) = extract_user_content(&value)
                    && let Some(cleaned) = clean_user_message(&text, json_flag(&value, "isMeta"))
                {
                    user_fallback = Some(cleaned);
                }
            }
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

    Some(SessionHead {
        envelope: envelope?,
        ai_title,
        user_fallback,
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

fn json_flag(value: &serde_json::Value, key: &str) -> bool {
    value.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn clean_user_message(raw: &str, is_meta: bool) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || is_meta {
        return None;
    }
    if SYNTHETIC_USER_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
    {
        return None;
    }

    if let Some(name) = extract_xml_block(trimmed, "command-name") {
        if CONTEXT_RESET_COMMANDS.contains(&name.trim_start_matches('/')) {
            return None;
        }
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
        assert_eq!(
            slug_for_cwd("/home/alice/my project"),
            "-home-alice-my-project"
        );
    }

    #[test]
    fn slug_replaces_dots() {
        assert_eq!(slug_for_cwd("/home/alice/.config"), "-home-alice--config");
    }

    #[test]
    fn slug_windows_path_replaces_drive_colon() {
        assert_eq!(
            slug_for_cwd("C:\\Users\\alice\\myapp"),
            "C--Users-alice-myapp"
        );
    }

    #[test]
    fn slug_matches_real_windows_project_dir() {
        assert_eq!(slug_for_cwd("C:\\dev\\paneflow"), "C--dev-paneflow");
    }

    #[test]
    fn trailing_separator_resolves_to_the_same_project_dir() {
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
        assert_eq!(normalize_cwd_for_slug("/"), "/");
        assert_eq!(normalize_cwd_for_slug("C:\\"), "C:\\");
        assert_eq!(slug_for_cwd(normalize_cwd_for_slug("/")), "-");
        assert_eq!(slug_for_cwd(normalize_cwd_for_slug("C:\\")), "C--");
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
    fn read_generated_title_refuses_the_first_message_fallback() {
        let dir = tempfile::tempdir().expect("tempdir");
        let envelope = r#"{"parentUuid":null,"type":"user","message":{"role":"user","content":"Ne penses-tu pas qu'il y a trop de features"},"uuid":"u","timestamp":"2026-04-26T13:38:41.095Z","cwd":"/tmp/proj","sessionId":"s"}"#;

        let untitled = dir.path().join("untitled.jsonl");
        std::fs::write(&untitled, format!("{envelope}\n")).expect("write fixture");
        assert_eq!(
            read_generated_title(&untitled),
            None,
            "no ai-title yet means ask again after the next turn"
        );
        assert_eq!(
            read_session_meta(&untitled)
                .expect("meta")
                .summary
                .as_deref(),
            Some("Ne penses-tu pas qu'il y a trop de features"),
            "the sessions popover still gets its fallback label"
        );

        let titled = dir.path().join("titled.jsonl");
        std::fs::write(
            &titled,
            format!(
                "{envelope}\n{}\n",
                r#"{"type":"ai-title","aiTitle":"Pyxis feature-count review","sessionId":"s"}"#
            ),
        )
        .expect("write fixture");
        assert_eq!(
            read_generated_title(&titled).as_deref(),
            Some("Pyxis feature-count review")
        );
    }

    #[test]
    fn read_generated_title_survives_a_missing_file() {
        assert_eq!(
            read_generated_title(std::path::Path::new("/nonexistent/session.jsonl")),
            None
        );
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

        let title_only = read_session_meta_inner(&path, false).expect("meta");
        assert!(title_only.model.is_none());
        assert!(title_only.usage.is_none());
        assert_eq!(title_only.summary.as_deref(), Some("Some title"));

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
    fn meta_caveat_record_never_becomes_the_title() {
        assert_eq!(
            clean_user_message(
                "<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>",
                true,
            ),
            None,
        );
        assert_eq!(clean_user_message("<local-command-stdout>ok", false), None);
        assert_eq!(
            clean_user_message("<system-reminder>context</system-reminder>", false),
            None,
        );
        assert_eq!(clean_user_message("some injected note", true), None);
        assert_eq!(
            clean_user_message("Corrige la sidebar", false).as_deref(),
            Some("Corrige la sidebar"),
        );
    }

    #[test]
    fn context_reset_commands_do_not_become_the_title() {
        assert_eq!(
            clean_user_message(
                "<command-name>/clear</command-name>\n<command-message>clear</command-message>\n<command-args></command-args>",
                false,
            ),
            None,
        );
        assert_eq!(
            clean_user_message(
                "<command-name>/review-epic</command-name>\n<command-args>tasks/prd.md EP-005</command-args>",
                false,
            )
            .as_deref(),
            Some("/review-epic tasks/prd.md EP-005"),
        );
    }

    #[test]
    fn fallback_title_skips_meta_and_sidechain_records() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("session.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","isMeta":true,"message":{"content":"<local-command-caveat>Caveat: The messages below were generated by the user while running local commands.</local-command-caveat>"},"sessionId":"aaaaaaaa-1111-2222-3333-444444444444","cwd":"/home/arthur/dev/paneflow","gitBranch":"main","timestamp":"2026-08-25T10:00:00Z"}"#,
                "\n",
                r#"{"type":"user","isSidechain":true,"message":{"content":"You are a sub-agent, review the diff"}}"#,
                "\n",
                r#"{"type":"user","message":{"content":"Corrige la sidebar agent sessions"}}"#,
                "\n",
            ),
        )
        .expect("write fixture");

        let meta = read_session_meta(&path).expect("meta");
        assert_eq!(
            meta.summary.as_deref(),
            Some("Corrige la sidebar agent sessions")
        );
        assert_eq!(meta.git_branch, "main");
    }

    #[test]
    fn truncate_label_caps_long_text() {
        let long = "a".repeat(120);
        let label = clean_user_message(&long, false).expect("label");
        assert_eq!(label.chars().count(), LABEL_MAX_CHARS + 1);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn read_session_meta_truncates_oversize_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversize.jsonl");
        let big = "x".repeat(1024 * 1024);
        std::fs::write(&path, &big).expect("write fixture");
        let meta = std::fs::metadata(&path).expect("metadata");
        assert_eq!(meta.len(), 1024 * 1024);
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

    #[test]
    fn session_cache_round_trips_and_invalidates_on_mtime_change() {
        use crate::agent_sessions::cache;
        cache::clear();

        let dir = tempfile::tempdir().expect("tempdir");
        let cwd = "/some/cwd";
        let project_dir = dir.path();

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

        std::thread::sleep(std::time::Duration::from_millis(2500));
        std::fs::write(project_dir.join("touch.tmp"), b"x").expect("touch");

        assert!(
            cache::lookup(SessionAgent::Claude, cwd, project_dir).is_none(),
            "mtime bump must invalidate the cached entry"
        );
    }

    #[test]
    fn exactly_max_final_line_is_parsed_not_dropped() {
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
        std::fs::write(&path, &line).expect("write");
        let meta = read_session_meta(&path).expect("exactly-MAX complete record must parse");
        assert_eq!(meta.cwd, "/tmp/proj");
    }

    #[test]
    fn genuinely_oversized_line_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("oversized.jsonl");
        let line = format!(
            r#"{{"cwd":"/tmp/proj","p":"{}"#,
            "x".repeat(MAX_LINE_BYTES as usize + 2000)
        );
        std::fs::write(&path, &line).expect("write");
        assert!(
            read_session_meta(&path).is_none(),
            "an oversized line must be skipped, not parsed"
        );
    }
}
