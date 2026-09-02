use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::agent_sessions::{AssistantUsage, SessionAgent, SessionMeta, clean_session_label};

const TITLE_SCAN_LIMIT: usize = 256;

const TITLE_SCAN_BYTES: u64 = 1024 * 1024;

const SESSION_META_MAX_BYTES: u64 = 1024 * 1024;

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

const MODEL_USAGE_SCAN_LIMIT: usize = 20_000;

use crate::limits::MAX_LINE_BYTES;

const LABEL_MAX_CHARS: usize = 80;

pub fn sessions_root() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".codex").join("sessions"))
}

pub fn read_sessions_for_cwd(cwd: &str) -> Vec<SessionMeta> {
    read_sessions_for_cwd_with_omitted(cwd).0
}

pub fn read_sessions_for_cwd_with_omitted(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_sessions_for_cwd_inner(
        cwd,
        false,
        Some(crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE),
    )
}

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
        .map(|(fallback, path)| read_session_meta_inner(&path, true, Some(cwd)).unwrap_or(fallback))
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

const MAX_WALK_DEPTH: u32 = 8;

fn walk_jsonl_files(dir: &Path, visit: &mut impl FnMut(&Path)) {
    walk_jsonl_files_bounded(dir, MAX_WALK_DEPTH, visit);
}

fn walk_jsonl_files_bounded(dir: &Path, depth_left: u32, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
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

#[cfg(test)]
fn read_session_meta(path: &Path) -> Option<SessionMeta> {
    read_session_meta_inner(path, false, None)
}

fn read_session_meta_inner(
    path: &Path,
    scan_usage: bool,
    cwd_filter: Option<&str>,
) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = String::new();

    buf.clear();
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
    if !crate::agent_sessions::is_valid_session_id(&session_id)
        || cwd.chars().any(|c| c.is_control())
    {
        log::warn!(
            "codex_sessions: dropped {} -- payload carries an invalid id or control chars in cwd",
            path.display(),
        );
        return None;
    }
    let timestamp = payload
        .get("timestamp")
        .and_then(|v| v.as_str())
        .or_else(|| first_value.get("timestamp").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();

    let git_branch = payload
        .get("git")
        .and_then(|g| g.get("branch"))
        .and_then(|v| v.as_str())
        .filter(|b| !b.chars().any(char::is_control))
        .unwrap_or("")
        .to_string();

    let scan = if scan_usage {
        scan_tail_with_usage(&mut reader)
    } else {
        scan_head_for_title(&mut reader)
    };

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

#[derive(Default)]
struct RolloutScan {
    summary: Option<String>,
    model: Option<String>,
    usage: Option<AssistantUsage>,
    saw_activity: bool,
}

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

fn scan_head_for_title(reader: &mut BufReader<fs::File>) -> RolloutScan {
    let mut scan = RolloutScan::default();
    let mut buf = String::new();
    let mut budget = TITLE_SCAN_BYTES;
    for _ in 0..TITLE_SCAN_LIMIT {
        if budget == 0 {
            break;
        }
        buf.clear();
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

fn is_activity_record(record_type: Option<&str>) -> bool {
    matches!(record_type, Some("response_item") | Some("event_msg"))
}

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

fn first_labelable_block(blocks: &[serde_json::Value], kind: &str) -> Option<String> {
    blocks
        .iter()
        .filter(|b| b.get("type").and_then(|v| v.as_str()) == Some(kind))
        .filter_map(|b| b.get("text").and_then(|v| v.as_str()))
        .find_map(clean_user_message)
}

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
        assert_eq!(meta.git_branch, "main");
    }

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

    #[test]
    fn walk_discovers_jsonl_in_deep_acyclic_tree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let leaf_dir = dir.path().join("2026/06/08/extra");
        std::fs::create_dir_all(&leaf_dir).expect("mkdir -p");
        let jsonl = leaf_dir.join("rollout.jsonl");
        std::fs::write(&jsonl, b"{}\n").expect("write");
        std::fs::write(leaf_dir.join("not-a-session.txt"), b"ignore me").expect("write");

        let mut found = Vec::new();
        walk_jsonl_files(dir.path(), &mut |p| found.push(p.to_path_buf()));
        assert_eq!(found, vec![jsonl], "the one real .jsonl must be discovered");
    }

    #[test]
    fn walk_stops_past_depth_bound() {
        let dir = tempfile::tempdir().expect("tempdir");
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

    #[cfg(unix)]
    #[test]
    fn walk_does_not_follow_symlink_cycle() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("2026/06/08");
        std::fs::create_dir_all(&real).expect("mkdir -p");
        let jsonl = real.join("rollout.jsonl");
        std::fs::write(&jsonl, b"{}\n").expect("write");
        std::os::unix::fs::symlink(dir.path(), dir.path().join("2026/loop"))
            .expect("create symlink cycle");

        let mut found = Vec::new();
        walk_jsonl_files(dir.path(), &mut |p| found.push(p.to_path_buf()));
        assert_eq!(found, vec![jsonl]);
    }
}
