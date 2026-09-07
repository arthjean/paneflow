use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::agent_launcher::TerminalAgent;

pub const MIN_INTERVAL: Duration = Duration::from_secs(180);
pub const SUMMARIZER_DEADLINE: Duration = Duration::from_secs(60);
const SUMMARIZER_STDOUT_CAP: u64 = 64 * 1024;
const KEPT_MESSAGES: usize = 12;
const CONTEXT_TAIL_MESSAGES: usize = 4;
const MESSAGE_MAX_CHARS: usize = 600;
const TITLE_MAX_CHARS: usize = 48;
const SUMMARIZER_ORDER: [TerminalAgent; 4] = [
    TerminalAgent::ClaudeCode,
    TerminalAgent::Codex,
    TerminalAgent::OpenCode,
    TerminalAgent::Pi,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SessionNaming {
    messages: Vec<Message>,
    named_message_count: usize,
    last_attempt: Option<Instant>,
    in_flight: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Run,
    SkipInFlight,
    SkipInterval,
    SkipNoGrowth,
    SkipEmpty,
}

impl SessionNaming {
    pub fn record(&mut self, role: Role, text: &str) {
        let text: String = text.trim().chars().take(MESSAGE_MAX_CHARS).collect();
        if text.is_empty() {
            return;
        }
        let message = Message { role, text };
        if self.messages.last() == Some(&message) {
            return;
        }
        self.messages.push(message);
        if self.messages.len() > KEPT_MESSAGES {
            let overflow = self.messages.len() - KEPT_MESSAGES;
            self.messages.drain(..overflow);
            self.named_message_count = self.named_message_count.saturating_sub(overflow);
        }
    }

    pub fn decide(&self, now: Instant) -> Decision {
        if self.in_flight {
            return Decision::SkipInFlight;
        }
        if self.messages.is_empty() {
            return Decision::SkipEmpty;
        }
        if self
            .last_attempt
            .is_some_and(|at| now.duration_since(at) < MIN_INTERVAL)
        {
            return Decision::SkipInterval;
        }
        if self.messages.len() <= self.named_message_count {
            return Decision::SkipNoGrowth;
        }
        Decision::Run
    }

    pub fn begin(&mut self, now: Instant) -> String {
        self.in_flight = true;
        self.last_attempt = Some(now);
        self.named_message_count = self.messages.len();
        build_context(&self.messages)
    }

    pub fn finish(&mut self) {
        self.in_flight = false;
    }
}

fn build_context(messages: &[Message]) -> String {
    let head = messages.iter().find(|m| m.role == Role::User);
    let tail = messages
        .iter()
        .skip(messages.len().saturating_sub(CONTEXT_TAIL_MESSAGES));
    let mut parts: Vec<String> = Vec::new();
    for message in head.into_iter().chain(tail) {
        let line = format!("{}: {}", message.role.label(), message.text);
        if !parts.contains(&line) {
            parts.push(line);
        }
    }
    parts.join("\n")
}

pub fn build_prompt(current_title: Option<&str>, context: &str) -> String {
    let mut prompt = String::from(
        "You name terminal workspace tabs for a developer running coding agents.\n\
         Given a conversation excerpt, output ONLY a short title: 2-5 words,\n\
         in the same language as the conversation, no quotes, no trailing punctuation.\n\n",
    );
    if let Some(title) = current_title.filter(|t| !t.is_empty()) {
        prompt.push_str(&format!(
            "The current title is: {title}\n\
             If that still accurately describes the conversation's main topic, output it EXACTLY as given.\n\n"
        ));
    }
    prompt.push_str("Conversation excerpt:\n");
    prompt.push_str(context);
    prompt
}

pub fn sanitize_response(raw: &str, current_title: Option<&str>) -> Option<String> {
    let first = raw.lines().map(str::trim).find(|line| !line.is_empty())?;
    let mut title = first.to_string();
    loop {
        let unwrapped = title
            .strip_prefix('"')
            .and_then(|t| t.strip_suffix('"'))
            .or_else(|| title.strip_prefix('\'').and_then(|t| t.strip_suffix('\'')))
            .or_else(|| {
                title
                    .strip_prefix('\u{201C}')
                    .and_then(|t| t.strip_suffix('\u{201D}'))
            })
            .map(str::trim)
            .map(str::to_string);
        match unwrapped {
            Some(inner) if inner != title => title = inner,
            _ => break,
        }
    }
    let mut title = crate::sidebar_title::clean_sidebar_title(&title)?;
    if title.chars().count() > TITLE_MAX_CHARS {
        let prefix: String = title.chars().take(TITLE_MAX_CHARS).collect();
        title = match prefix.rfind(' ') {
            Some(cut) if cut > 0 => prefix[..cut].to_string(),
            _ => prefix,
        };
    }
    let title = title
        .trim_end_matches(['.', ':', ';', ','])
        .trim()
        .to_string();
    if title.is_empty() || current_title == Some(title.as_str()) {
        return None;
    }
    Some(title)
}

pub fn pick_summarizer(session_agent: TerminalAgent) -> Option<TerminalAgent> {
    if SUMMARIZER_ORDER.contains(&session_agent) && session_agent.is_installed() {
        return Some(session_agent);
    }
    SUMMARIZER_ORDER
        .into_iter()
        .find(|agent| agent.is_installed())
}

fn summarizer_environment(agent: TerminalAgent) -> Vec<(String, String)> {
    const CODEX_ALLOWED: [&str; 19] = [
        "HOME",
        "PATH",
        "TMPDIR",
        "TMP",
        "TEMP",
        "USER",
        "USERNAME",
        "USERPROFILE",
        "LOGNAME",
        "SHELL",
        "CODEX_HOME",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "OPENAI_ORG_ID",
        "OPENAI_ORGANIZATION",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
    ];
    std::env::vars()
        .filter(|(key, _)| {
            !key.starts_with("PANEFLOW_")
                && key != "NODE_OPTIONS"
                && !crate::terminal::INHERITED_AGENT_SESSION_ENV.contains(&key.as_str())
        })
        .filter(|(key, _)| agent != TerminalAgent::Codex || CODEX_ALLOWED.contains(&key.as_str()))
        .collect()
}

fn claude_model() -> String {
    std::env::var("ANTHROPIC_SMALL_FAST_MODEL")
        .ok()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "haiku".to_string())
}

struct Scratch {
    dir: PathBuf,
}

impl Scratch {
    fn create() -> Option<Self> {
        let nonce = format!(
            "paneflow-autoname-{}-{}",
            std::process::id(),
            std::time::SystemTime::UNIX_EPOCH
                .elapsed()
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(nonce);
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self { dir })
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn summarize(
    agent: TerminalAgent,
    prompt: &str,
    current_title: Option<&str>,
) -> Option<String> {
    let binary = which::which(agent.binary()).ok()?;
    let scratch = Scratch::create()?;
    let (mut command, output_file) = summarizer_command(agent, &binary, prompt, &scratch);
    command.env_clear().envs(summarizer_environment(agent));
    let out =
        paneflow_process::run_with_timeout(command, SUMMARIZER_DEADLINE, SUMMARIZER_STDOUT_CAP)
            .ok()?;
    if !out.status.success() {
        log::debug!(
            "auto-naming: {} exited with {}: {}",
            agent.binary(),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return None;
    }
    let raw = match output_file {
        Some(path) => std::fs::read_to_string(path).ok()?,
        None => String::from_utf8_lossy(&out.stdout).into_owned(),
    };
    sanitize_response(&raw, current_title)
}

fn summarizer_command(
    agent: TerminalAgent,
    binary: &Path,
    prompt: &str,
    scratch: &Scratch,
) -> (Command, Option<PathBuf>) {
    let mut command = Command::new(binary);
    command.current_dir(&scratch.dir);
    match agent {
        TerminalAgent::Codex => {
            let output = scratch.path("title.txt");
            command.args([
                "exec",
                "-c",
                "default_tools_enabled=false",
                "-c",
                "tools={}",
                "-c",
                "mcp_servers={}",
                "-c",
                "web_search=false",
                "-c",
                "approval_policy=never",
                "-c",
                "shell_environment_policy.inherit=none",
                "--skip-git-repo-check",
                "--ephemeral",
                "--ignore-user-config",
                "--ignore-rules",
                "--sandbox",
                "read-only",
                "--cd",
            ]);
            command
                .arg(&scratch.dir)
                .arg("--output-last-message")
                .arg(&output)
                .arg(prompt);
            (command, Some(output))
        }
        TerminalAgent::OpenCode => {
            command
                .args(["run", "--pure", "--format", "default", "--dir"])
                .arg(&scratch.dir)
                .arg(prompt);
            (command, None)
        }
        TerminalAgent::Pi => {
            command
                .args([
                    "--print",
                    "--no-tools",
                    "--no-session",
                    "--no-extensions",
                    "--no-skills",
                    "--no-prompt-templates",
                    "--no-context-files",
                ])
                .arg(prompt);
            (command, None)
        }
        _ => {
            command
                .arg("-p")
                .arg(prompt)
                .args(["--model", &claude_model()])
                .args([
                    "--tools",
                    "",
                    "--disable-slash-commands",
                    "--no-session-persistence",
                    "--strict-mcp-config",
                    "--mcp-config",
                    r#"{"mcpServers":{}}"#,
                ]);
            (command, None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn the_first_turn_names_immediately_then_waits_for_growth_and_the_interval() {
        let mut naming = SessionNaming::default();
        assert_eq!(naming.decide(now()), Decision::SkipEmpty);

        naming.record(Role::User, "fix the flaky worktree test");
        naming.record(Role::Assistant, "Done, the race was in the watcher.");
        let start = now();
        assert_eq!(naming.decide(start), Decision::Run);

        let context = naming.begin(start);
        assert!(context.starts_with("user: fix the flaky worktree test\n"));
        assert_eq!(naming.decide(start), Decision::SkipInFlight);
        naming.finish();

        assert_eq!(naming.decide(start), Decision::SkipInterval);
        let later = start + MIN_INTERVAL;
        assert_eq!(naming.decide(later), Decision::SkipNoGrowth);

        naming.record(Role::User, "now port it to Windows");
        assert_eq!(naming.decide(later), Decision::Run);
    }

    #[test]
    fn the_excerpt_keeps_the_opening_request_and_the_recent_tail() {
        let mut naming = SessionNaming::default();
        naming.record(Role::User, "first ask");
        for i in 0..4 {
            naming.record(Role::Assistant, &format!("reply {i}"));
            naming.record(Role::User, &format!("follow-up {i}"));
        }
        let context = naming.begin(now());
        let lines: Vec<&str> = context.lines().collect();
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[0], "user: first ask");
        assert_eq!(lines[1], "assistant: reply 2");
        assert_eq!(lines[4], "user: follow-up 3");
    }

    #[test]
    fn repeated_and_blank_messages_are_dropped() {
        let mut naming = SessionNaming::default();
        naming.record(Role::User, "   ");
        naming.record(Role::User, "same");
        naming.record(Role::User, "same");
        assert_eq!(naming.messages.len(), 1);
    }

    #[test]
    fn the_prompt_carries_the_current_title_hint() {
        let prompt = build_prompt(Some("Worktree deflake"), "user: hi");
        assert!(prompt.contains("The current title is: Worktree deflake"));
        assert!(prompt.ends_with("Conversation excerpt:\nuser: hi"));
        assert!(!build_prompt(None, "user: hi").contains("current title"));
    }

    #[test]
    fn responses_are_unwrapped_capped_and_deduplicated_against_the_current_title() {
        assert_eq!(
            sanitize_response("\n  \"Worktree test deflake.\"\nextra", None).as_deref(),
            Some("Worktree test deflake")
        );
        assert_eq!(
            sanitize_response("\u{201C}Release checksum job\u{201D}", None).as_deref(),
            Some("Release checksum job")
        );
        assert_eq!(sanitize_response("Same title", Some("Same title")), None);
        assert_eq!(sanitize_response("", None), None);
        let long = "word ".repeat(30);
        let capped = sanitize_response(&long, None).expect("capped title");
        assert!(capped.chars().count() <= TITLE_MAX_CHARS);
        assert!(!capped.ends_with(' '));
    }

    #[test]
    fn the_summarizer_environment_never_leaks_paneflow_or_agent_session_markers() {
        let env = summarizer_environment(TerminalAgent::ClaudeCode);
        assert!(env.iter().all(|(key, _)| !key.starts_with("PANEFLOW_")));
        assert!(env.iter().all(|(key, _)| key != "CLAUDECODE"));
        let codex = summarizer_environment(TerminalAgent::Codex);
        assert!(codex.iter().all(|(key, _)| key != "ANTHROPIC_API_KEY"));
    }

    #[test]
    fn each_summarizer_runs_without_tools() {
        let scratch = Scratch::create().expect("scratch dir");
        let dir = scratch.dir.clone();
        let bin = Path::new("agent");
        let (claude, file) = summarizer_command(TerminalAgent::ClaudeCode, bin, "p", &scratch);
        let args: Vec<String> = claude
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(file.is_none());
        assert!(args.contains(&"--tools".to_string()));
        assert!(args.contains(&"--no-session-persistence".to_string()));

        let (codex, file) = summarizer_command(TerminalAgent::Codex, bin, "p", &scratch);
        let args: Vec<String> = codex
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert!(file.is_some_and(|f| f.starts_with(&dir)));
        assert!(args.contains(&"--ephemeral".to_string()));
        assert!(args.contains(&"default_tools_enabled=false".to_string()));

        let (pi, _) = summarizer_command(TerminalAgent::Pi, bin, "p", &scratch);
        assert!(pi.get_args().any(|a| a == "--no-tools"));
        drop(scratch);
        assert!(!dir.exists());
    }
}
