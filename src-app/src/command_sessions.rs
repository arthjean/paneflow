use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::agent_sessions::{SessionAgent, SessionMeta, clean_session_label};

const COMMAND_DEADLINE: Duration = Duration::from_secs(15);
const COMMAND_STDOUT_CAP: u64 = 4 * 1024 * 1024;
const STDERR_LOG_CAP: usize = 200;

#[derive(Clone, Copy)]
enum CommandScope {
    CurrentDirectory,
    LineMustMentionCwd,
}

struct CommandSessionConfig {
    agent: SessionAgent,
    program: &'static str,
    args: &'static [&'static str],
    allow_numeric_ids: bool,
    scope: CommandScope,
}

pub(crate) fn read_gemini_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Gemini,
            program: "gemini",
            args: &["--list-sessions"],
            allow_numeric_ids: true,
            scope: CommandScope::CurrentDirectory,
        },
        cwd,
    )
}

pub(crate) fn read_cursor_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Cursor,
            program: "cursor-agent",
            args: &["ls"],
            allow_numeric_ids: false,
            scope: CommandScope::CurrentDirectory,
        },
        cwd,
    )
}

pub(crate) fn read_kiro_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Kiro,
            program: "kiro-cli",
            args: &["chat", "--list-sessions"],
            allow_numeric_ids: false,
            scope: CommandScope::CurrentDirectory,
        },
        cwd,
    )
}

pub(crate) fn read_grok_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Grok,
            program: "grok",
            args: &["sessions", "list", "--limit", "100"],
            allow_numeric_ids: false,
            scope: CommandScope::CurrentDirectory,
        },
        cwd,
    )
}

pub(crate) fn read_hermes_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Hermes,
            program: "hermes",
            args: &["sessions", "list", "--source", "cli", "--limit", "100"],
            allow_numeric_ids: false,
            scope: CommandScope::LineMustMentionCwd,
        },
        cwd,
    )
}

fn read_command_sessions(config: CommandSessionConfig, cwd: &str) -> (Vec<SessionMeta>, usize) {
    if !Path::new(cwd).is_dir() {
        return (Vec::new(), 0);
    }
    let Some(stdout) = run_list_command(&config, cwd) else {
        return (Vec::new(), 0);
    };
    parse_command_sessions(
        &stdout,
        config.agent,
        cwd,
        config.allow_numeric_ids,
        config.scope,
    )
}

fn run_list_command(config: &CommandSessionConfig, cwd: &str) -> Option<Vec<u8>> {
    let mut cmd = Command::new(config.program);
    cmd.args(config.args);
    if matches!(config.scope, CommandScope::CurrentDirectory) {
        cmd.current_dir(cwd);
    }

    let output = match paneflow_process::run_with_timeout(cmd, COMMAND_DEADLINE, COMMAND_STDOUT_CAP)
    {
        Ok(out) => out,
        Err(paneflow_process::ProcError::Spawn(err)) if err.kind() == io::ErrorKind::NotFound => {
            log::info!(
                "{} binary not found on PATH; {:?} sessions will be empty",
                config.program,
                config.agent
            );
            return None;
        }
        Err(paneflow_process::ProcError::Timeout) => {
            log::warn!(
                "{} session list timed out; {:?} sessions will be empty",
                config.program,
                config.agent
            );
            return None;
        }
        Err(err) => {
            log::warn!(
                "failed to spawn {} for {:?} sessions: {err}",
                config.program,
                config.agent
            );
            return None;
        }
    };

    if !output.status.success() {
        let stderr = sanitized_stderr(&output.stderr);
        log::warn!(
            "{} session list exited with {}: {}",
            config.program,
            output.status,
            stderr
        );
        return None;
    }

    Some(output.stdout)
}

fn parse_command_sessions(
    stdout: &[u8],
    agent: SessionAgent,
    cwd: &str,
    allow_numeric_ids: bool,
    scope: CommandScope,
) -> (Vec<SessionMeta>, usize) {
    let text = String::from_utf8_lossy(stdout);
    let sessions = text
        .lines()
        .filter_map(|line| parse_session_line(line, agent, cwd, allow_numeric_ids, scope));
    crate::agent_sessions::collect_recent_sessions(
        sessions,
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    )
}

fn parse_session_line(
    line: &str,
    agent: SessionAgent,
    cwd: &str,
    allow_numeric_ids: bool,
    scope: CommandScope,
) -> Option<SessionMeta> {
    let line = line.trim();
    if line.is_empty() || is_header_or_separator(line) {
        return None;
    }
    if matches!(scope, CommandScope::LineMustMentionCwd) && !line_mentions_cwd(line, cwd) {
        return None;
    }

    let session_id = extract_session_id(line, allow_numeric_ids)?;
    let timestamp = extract_iso8601(line).unwrap_or_default();
    let summary = line_summary(line, &session_id);

    Some(SessionMeta {
        agent,
        session_id,
        timestamp,
        cwd: cwd.to_string(),
        git_branch: String::new(),
        summary,
        model: None,
        usage: None,
    })
}

fn is_header_or_separator(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let headerish = lower.contains("session")
        && lower.contains("id")
        && (lower.contains("title") || lower.contains("summary"))
        && extract_session_id(line, false).is_none();
    headerish
        || line
            .chars()
            .all(|c| matches!(c, '-' | '=' | '+' | '|' | ' '))
}

fn extract_session_id(line: &str, allow_numeric_ids: bool) -> Option<String> {
    let tokens: Vec<String> = line
        .split_whitespace()
        .map(clean_token)
        .filter(|token| !token.is_empty())
        .collect();

    extract_labeled_session_id(&tokens, allow_numeric_ids).or_else(|| {
        tokens
            .iter()
            .find(|token| is_candidate_session_id(token, allow_numeric_ids, false))
            .cloned()
    })
}

fn clean_token(token: &str) -> String {
    token
        .trim_matches(|c: char| {
            matches!(
                c,
                '"' | '\'' | '`' | ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ':'
            )
        })
        .to_string()
}

fn extract_labeled_session_id(tokens: &[String], allow_numeric_ids: bool) -> Option<String> {
    for (idx, token) in tokens.iter().enumerate() {
        if let Some((label, value)) = split_labeled_token(token) {
            let label = label.to_ascii_lowercase();
            if is_id_label(&label) && is_candidate_session_id(value, allow_numeric_ids, true) {
                return Some(value.to_string());
            }
        }

        let lower = token.to_ascii_lowercase();
        let candidate_index = if lower == "session" {
            tokens
                .get(idx + 1)
                .is_some_and(|next| next.eq_ignore_ascii_case("id"))
                .then_some(idx + 2)
        } else if is_id_label(&lower) {
            Some(idx + 1)
        } else {
            None
        };
        if let Some(candidate_index) = candidate_index
            && let Some(candidate) = tokens.get(candidate_index)
            && is_candidate_session_id(candidate, allow_numeric_ids, true)
        {
            return Some(candidate.clone());
        }
    }
    None
}

fn split_labeled_token(token: &str) -> Option<(&str, &str)> {
    token
        .split_once('=')
        .or_else(|| token.split_once(':'))
        .filter(|(_, value)| !value.is_empty())
}

fn is_id_label(label: &str) -> bool {
    matches!(label, "id" | "session_id" | "sessionid")
}

fn is_candidate_session_id(token: &str, allow_numeric_ids: bool, explicit_id_label: bool) -> bool {
    if token.is_empty() || !crate::agent_sessions::is_valid_session_id(token) {
        return false;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        return allow_numeric_ids;
    }
    if looks_like_iso_date(token) {
        return false;
    }
    if explicit_id_label && token.len() >= 3 {
        let lower = token.to_ascii_lowercase();
        if matches!(lower.as_str(), "title" | "summary" | "created" | "updated") {
            return false;
        }
        return true;
    }
    token.starts_with("ses_")
        || token.starts_with("sess_")
        || token.starts_with("T-")
        || (token.len() >= 8 && (token.contains('-') || token.contains('_')))
}

fn looks_like_iso_date(token: &str) -> bool {
    let bytes = token.as_bytes();
    token.len() >= 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes
            .iter()
            .take(10)
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
}

fn extract_iso8601(line: &str) -> Option<String> {
    line.split_whitespace()
        .map(|token| token.trim_matches(|c: char| matches!(c, ',' | ';' | ')' | '(' | '[' | ']')))
        .find(|token| looks_like_iso_date(token))
        .map(|token| {
            if token.contains('T') {
                token.trim_end_matches('Z').to_string() + "Z"
            } else {
                format!("{}T00:00:00Z", &token[..10])
            }
        })
}

fn line_summary(line: &str, session_id: &str) -> Option<String> {
    let without_id = line.replace(session_id, " ");
    let mut summary = without_id.trim();
    summary = trim_leading_id_label(summary);
    summary = trim_leading_table_metadata(summary);
    summary = summary.trim_start_matches(|c: char| {
        c.is_ascii_digit() || matches!(c, '.' | ')' | '#' | '[' | ']' | '-' | '|' | ' ')
    });
    summary = trim_leading_id_label(summary);
    summary = summary.trim_matches(|c: char| matches!(c, '|' | '-' | ' '));
    if summary.is_empty()
        || summary.eq_ignore_ascii_case("session id")
        || summary.eq_ignore_ascii_case("(no summary)")
    {
        None
    } else {
        clean_session_label(summary, 120)
    }
}

fn trim_leading_id_label(summary: &str) -> &str {
    let trimmed = summary.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    for prefix in [
        "session id:",
        "session_id:",
        "session_id=",
        "sessionid:",
        "sessionid=",
        "id:",
        "id=",
    ] {
        if lower.starts_with(prefix) {
            return trimmed[prefix.len()..].trim_start();
        }
    }
    trimmed
}

fn trim_leading_table_metadata(mut summary: &str) -> &str {
    loop {
        let trimmed = summary.trim_start();
        let Some((first_token, rest)) = trimmed.split_once(char::is_whitespace) else {
            return trimmed;
        };
        if looks_like_iso_date(first_token) {
            summary = rest;
            continue;
        }
        if matches!(
            first_token,
            "local" | "remote" | "archived" | "running" | "done"
        ) {
            summary = rest;
            continue;
        }
        return trimmed;
    }
}

fn line_mentions_cwd(line: &str, cwd: &str) -> bool {
    #[cfg(windows)]
    {
        line.replace('/', "\\")
            .to_ascii_lowercase()
            .contains(&cwd.replace('/', "\\").to_ascii_lowercase())
    }
    #[cfg(not(windows))]
    {
        line.contains(cwd)
    }
}

fn sanitized_stderr(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .chars()
        .take(STDERR_LOG_CAP)
        .map(|c| if c.is_control() && c != '\n' { '?' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_sessions_extracts_uuid_from_cursorish_line() {
        let out = b"550e8400-e29b-41d4-a716-446655440000 2026-06-29T09:10:11Z Refactor auth flow\n";
        let (sessions, omitted) = parse_command_sessions(
            out,
            SessionAgent::Cursor,
            "/repo",
            false,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(sessions[0].agent, SessionAgent::Cursor);
    }

    #[test]
    fn parse_command_sessions_accepts_gemini_numeric_index() {
        let out = b"[2] 2026-06-29T09:10:11Z latest working thread\n";
        let (sessions, _) = parse_command_sessions(
            out,
            SessionAgent::Gemini,
            "/repo",
            true,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "2");
    }

    #[test]
    fn parse_command_sessions_accepts_short_explicit_session_id() {
        let out = b"Session ID: abc123\n";
        let (sessions, _) = parse_command_sessions(
            out,
            SessionAgent::Kiro,
            "/repo",
            false,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc123");
        assert_eq!(sessions[0].summary, None);
    }

    #[test]
    fn parse_command_sessions_does_not_pick_long_summary_word_as_id() {
        let out =
            b"550e8400-e29b-41d4-a716-446655440000 2026-06-29T09:10:11Z Refactor authentication\n";
        let (sessions, omitted) = parse_command_sessions(
            out,
            SessionAgent::Cursor,
            "/repo",
            false,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Refactor authentication")
        );
    }

    #[test]
    fn parse_command_sessions_accepts_labeled_token_id() {
        let out = b"id=abc123 label from command\n";
        let (sessions, _) = parse_command_sessions(
            out,
            SessionAgent::Kiro,
            "/repo",
            false,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc123");
        assert_eq!(sessions[0].summary.as_deref(), Some("label from command"));
    }

    #[test]
    fn line_summary_collapses_whitespace_and_controls() {
        let summary = line_summary(
            "ses_current_123456   first\n\tsecond\u{1b}   third",
            "ses_current_123456",
        );
        assert_eq!(summary.as_deref(), Some("first second third"));
    }

    #[test]
    fn line_must_mention_cwd_filters_unrelated_global_rows() {
        let out = b"550e8400-e29b-41d4-a716-446655440000 /elsewhere old\nses_current_123456 /repo current\n";
        let (sessions, _) = parse_command_sessions(
            out,
            SessionAgent::Grok,
            "/repo",
            false,
            CommandScope::LineMustMentionCwd,
        );
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "ses_current_123456");
    }

    #[test]
    fn parse_grok_sessions_table_output() {
        let out = br#"
(no label)
SESSION ID                            CREATED     UPDATED     STATUS      SUMMARY
019f1501-50e7-76d0-bb9e-4a72ede6b35d  2026-06-29  2026-06-29  local  List Sessions Command in Software Codebase
019f1501-69f1-7800-bc1e-cb269e1d985b  2026-06-29  2026-06-29  local  (no summary)
"#;
        let (sessions, omitted) = parse_command_sessions(
            out,
            SessionAgent::Grok,
            "/repo",
            false,
            CommandScope::CurrentDirectory,
        );
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].session_id,
            "019f1501-50e7-76d0-bb9e-4a72ede6b35d"
        );
        assert_eq!(sessions[0].timestamp, "2026-06-29T00:00:00Z");
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("List Sessions Command in Software Codebase")
        );
        assert_eq!(sessions[1].summary, None);
    }
}
