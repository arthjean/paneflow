use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::agent_sessions::{SessionAgent, SessionMeta, clean_session_label};

const COMMAND_DEADLINE: Duration = Duration::from_secs(15);
const COMMAND_STDOUT_CAP: u64 = 4 * 1024 * 1024;
const STDERR_LOG_CAP: usize = 200;

struct CommandSessionConfig {
    agent: SessionAgent,
    program: &'static str,
    args: &'static [&'static str],
    parse_line: fn(&str, SessionAgent, &str) -> Option<SessionMeta>,
}

pub(crate) fn read_gemini_sessions_for_cwd(cwd: &str) -> (Vec<SessionMeta>, usize) {
    read_command_sessions(
        CommandSessionConfig {
            agent: SessionAgent::Gemini,
            program: "gemini",
            args: &["--list-sessions"],
            parse_line: parse_gemini_line,
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
            parse_line: parse_session_line,
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
            parse_line: parse_session_line,
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
    parse_command_sessions(&stdout, config.agent, cwd, config.parse_line)
}

fn run_list_command(config: &CommandSessionConfig, cwd: &str) -> Option<Vec<u8>> {
    let Some(mut cmd) = list_command(config.program) else {
        log::info!(
            "{} binary not found on PATH; {:?} sessions will be empty",
            config.program,
            config.agent
        );
        return None;
    };
    cmd.args(config.args);
    cmd.current_dir(cwd);

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

#[cfg(windows)]
fn list_command(program: &str) -> Option<Command> {
    which::which(program).ok().map(Command::new)
}

#[cfg(not(windows))]
fn list_command(program: &str) -> Option<Command> {
    Some(Command::new(program))
}

fn parse_command_sessions(
    stdout: &[u8],
    agent: SessionAgent,
    cwd: &str,
    parse_line: fn(&str, SessionAgent, &str) -> Option<SessionMeta>,
) -> (Vec<SessionMeta>, usize) {
    let text = String::from_utf8_lossy(stdout);
    let sessions = text.lines().filter_map(|line| parse_line(line, agent, cwd));
    crate::agent_sessions::collect_recent_sessions(
        sessions,
        crate::agent_sessions::SIDEBAR_SESSION_RETAINED_PER_SOURCE,
    )
}

fn parse_gemini_line(line: &str, agent: SessionAgent, cwd: &str) -> Option<SessionMeta> {
    let line = line.trim();
    let bracketed = line.strip_suffix(']')?;
    let open = bracketed.rfind('[')?;
    let session_id = &bracketed[open + 1..];
    if !crate::agent_sessions::is_valid_session_id(session_id) {
        return None;
    }
    let listing = bracketed[..open].trim_end();
    let title = listing
        .rfind(" (")
        .filter(|_| listing.ends_with(')'))
        .map_or(listing, |age| &listing[..age]);
    let title = title
        .split_once(". ")
        .filter(|(index, _)| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()))
        .map_or(title, |(_, rest)| rest);
    Some(SessionMeta {
        agent,
        session_id: session_id.to_string(),
        timestamp: String::new(),
        cwd: cwd.to_string(),
        summary: clean_session_label(title.trim(), 120),
    })
}

fn parse_session_line(line: &str, agent: SessionAgent, cwd: &str) -> Option<SessionMeta> {
    let line = line.trim();
    if line.is_empty() || is_header_or_separator(line) {
        return None;
    }

    let session_id = extract_session_id(line)?;
    let timestamp = extract_iso8601(line).unwrap_or_default();
    let summary = line_summary(line, &session_id);

    Some(SessionMeta {
        agent,
        session_id,
        timestamp,
        cwd: cwd.to_string(),
        summary,
    })
}

fn is_header_or_separator(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let headerish = lower.contains("session")
        && lower.contains("id")
        && (lower.contains("title") || lower.contains("summary"))
        && extract_session_id(line).is_none();
    headerish
        || line
            .chars()
            .all(|c| matches!(c, '-' | '=' | '+' | '|' | ' '))
}

fn extract_session_id(line: &str) -> Option<String> {
    let tokens: Vec<String> = line
        .split_whitespace()
        .map(clean_token)
        .filter(|token| !token.is_empty())
        .collect();

    extract_labeled_session_id(&tokens).or_else(|| {
        tokens
            .iter()
            .find(|token| is_candidate_session_id(token, false))
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

fn extract_labeled_session_id(tokens: &[String]) -> Option<String> {
    for (idx, token) in tokens.iter().enumerate() {
        if let Some((label, value)) = split_labeled_token(token) {
            let label = label.to_ascii_lowercase();
            if is_id_label(&label) && is_candidate_session_id(value, true) {
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
            && is_candidate_session_id(candidate, true)
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

fn is_candidate_session_id(token: &str, explicit_id_label: bool) -> bool {
    if token.is_empty() || !crate::agent_sessions::is_valid_session_id(token) {
        return false;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        return false;
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
    fn parse_command_sessions_extracts_uuid_from_a_listed_line() {
        let out = b"550e8400-e29b-41d4-a716-446655440000 2026-06-29T09:10:11Z Refactor auth flow\n";
        let (sessions, omitted) =
            parse_command_sessions(out, SessionAgent::Grok, "/repo", parse_session_line);
        assert_eq!(omitted, 0);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(sessions[0].agent, SessionAgent::Grok);
    }

    #[test]
    fn parse_gemini_sessions_returns_only_the_trailing_bracketed_ids() {
        let out = b"\nAvailable sessions for this project (3):\n  1. Fix the flaky auth test (2 days ago) [5f0c2a7e-1b3d-4e8a-9c21-7d4b6e0f9a13]\n  2. [draft] Refactor the parser (3 hours ago) [a8e1d9b2-6c4f-4f1a-8b7e-2d9c0e5f4a61]\n  3. Update docs (Just now, current) [0d7e3f4a-9b2c-4a6e-b1d8-5f3c2e1a7b90]\n";
        let (sessions, omitted) =
            parse_command_sessions(out, SessionAgent::Gemini, "/repo", parse_gemini_line);
        assert_eq!(omitted, 0);
        let ids: Vec<&str> = sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "5f0c2a7e-1b3d-4e8a-9c21-7d4b6e0f9a13",
                "a8e1d9b2-6c4f-4f1a-8b7e-2d9c0e5f4a61",
                "0d7e3f4a-9b2c-4a6e-b1d8-5f3c2e1a7b90",
            ]
        );
        assert_eq!(
            sessions[0].summary.as_deref(),
            Some("Fix the flaky auth test")
        );
        assert_eq!(
            sessions[1].summary.as_deref(),
            Some("[draft] Refactor the parser")
        );
    }

    #[test]
    fn parse_gemini_sessions_skips_lines_without_a_bracketed_id() {
        let out = b"No previous sessions found for this project.\n  1. Untitled (2 days ago)\n  2. Bad id (1 day ago) [--resume]\n";
        let (sessions, _) =
            parse_command_sessions(out, SessionAgent::Gemini, "/repo", parse_gemini_line);
        assert!(sessions.is_empty(), "got {sessions:?}");
    }

    #[test]
    fn parse_command_sessions_accepts_short_explicit_session_id() {
        let out = b"Session ID: abc123\n";
        let (sessions, _) =
            parse_command_sessions(out, SessionAgent::Kiro, "/repo", parse_session_line);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "abc123");
        assert_eq!(sessions[0].summary, None);
    }

    #[test]
    fn parse_command_sessions_does_not_pick_long_summary_word_as_id() {
        let out =
            b"550e8400-e29b-41d4-a716-446655440000 2026-06-29T09:10:11Z Refactor authentication\n";
        let (sessions, omitted) =
            parse_command_sessions(out, SessionAgent::Grok, "/repo", parse_session_line);
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
        let (sessions, _) =
            parse_command_sessions(out, SessionAgent::Kiro, "/repo", parse_session_line);
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
    fn parse_grok_sessions_table_output() {
        let out = br#"
(no label)
SESSION ID                            CREATED     UPDATED     STATUS      SUMMARY
019f1501-50e7-76d0-bb9e-4a72ede6b35d  2026-06-29  2026-06-29  local  List Sessions Command in Software Codebase
019f1501-69f1-7800-bc1e-cb269e1d985b  2026-06-29  2026-06-29  local  (no summary)
"#;
        let (sessions, omitted) =
            parse_command_sessions(out, SessionAgent::Grok, "/repo", parse_session_line);
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
