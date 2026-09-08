use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use paneflow_config::schema::{AgentProfileConfig, PaneFlowConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalAgent {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
    Hermes,
    Grok,
    Amp,
    Cursor,
    Gemini,
    Kiro,
    Antigravity,
    Copilot,
    CodeBuddy,
    Factory,
    Qoder,
    Openclaw,
}

impl TerminalAgent {
    pub const ALL: [TerminalAgent; 16] = [
        TerminalAgent::ClaudeCode,
        TerminalAgent::Codex,
        TerminalAgent::OpenCode,
        TerminalAgent::Pi,
        TerminalAgent::Hermes,
        TerminalAgent::Grok,
        TerminalAgent::Amp,
        TerminalAgent::Cursor,
        TerminalAgent::Gemini,
        TerminalAgent::Kiro,
        TerminalAgent::Antigravity,
        TerminalAgent::Copilot,
        TerminalAgent::CodeBuddy,
        TerminalAgent::Factory,
        TerminalAgent::Qoder,
        TerminalAgent::Openclaw,
    ];

    pub const PRIMARY: [TerminalAgent; 5] = [
        TerminalAgent::ClaudeCode,
        TerminalAgent::Codex,
        TerminalAgent::OpenCode,
        TerminalAgent::Pi,
        TerminalAgent::Grok,
    ];

    pub fn secondary() -> impl Iterator<Item = TerminalAgent> {
        Self::ALL
            .into_iter()
            .filter(|agent| !Self::PRIMARY.contains(agent))
    }

    pub fn visibility_config_key(self) -> &'static str {
        match self {
            TerminalAgent::ClaudeCode => "claude_code_button_visible",
            TerminalAgent::Codex => "codex_button_visible",
            TerminalAgent::OpenCode => "opencode_button_visible",
            TerminalAgent::Pi => "pi_button_visible",
            TerminalAgent::Hermes => "hermes_agent_button_visible",
            TerminalAgent::Grok => "grok_button_visible",
            TerminalAgent::Amp => "amp_button_visible",
            TerminalAgent::Cursor => "cursor_button_visible",
            TerminalAgent::Gemini => "gemini_button_visible",
            TerminalAgent::Kiro => "kiro_button_visible",
            TerminalAgent::Antigravity => "antigravity_button_visible",
            TerminalAgent::Copilot => "copilot_button_visible",
            TerminalAgent::CodeBuddy => "codebuddy_button_visible",
            TerminalAgent::Factory => "factory_button_visible",
            TerminalAgent::Qoder => "qoder_button_visible",
            TerminalAgent::Openclaw => "openclaw_button_visible",
        }
    }

    pub fn cached_version(self) -> Option<String> {
        version_cache()
            .lock()
            .ok()
            .and_then(|cache| cache.get(&self).cloned().flatten())
    }

    pub fn probe_missing_versions() {
        let pending: Vec<TerminalAgent> = {
            let Ok(cache) = version_cache().lock() else {
                return;
            };
            TerminalAgent::ALL
                .into_iter()
                .filter(|agent| agent.is_installed() && !cache.contains_key(agent))
                .collect()
        };
        for agent in pending {
            let version = which::which(agent.binary())
                .ok()
                .and_then(|path| probe_version(&path));
            if let Ok(mut cache) = version_cache().lock() {
                cache.insert(agent, version);
            }
        }
    }

    pub fn display_rank(self) -> usize {
        Self::ALL
            .iter()
            .position(|a| *a == self)
            .unwrap_or(usize::MAX)
    }

    pub fn display_name(self) -> &'static str {
        match self {
            TerminalAgent::ClaudeCode => "Claude Code",
            TerminalAgent::Codex => "Codex",
            TerminalAgent::OpenCode => "OpenCode",
            TerminalAgent::Pi => "Pi",
            TerminalAgent::Hermes => "Hermes Agent",
            TerminalAgent::Grok => "Grok",
            TerminalAgent::Amp => "Amp",
            TerminalAgent::Cursor => "Cursor",
            TerminalAgent::Gemini => "Gemini",
            TerminalAgent::Kiro => "Kiro",
            TerminalAgent::Antigravity => "Antigravity",
            TerminalAgent::Copilot => "Copilot",
            TerminalAgent::CodeBuddy => "CodeBuddy",
            TerminalAgent::Factory => "Factory",
            TerminalAgent::Qoder => "Qoder",
            TerminalAgent::Openclaw => "Openclaw",
        }
    }

    pub fn icon_path(self) -> &'static str {
        match self {
            TerminalAgent::ClaudeCode => "icons/claude-color.svg",
            TerminalAgent::Codex => "icons/codex.svg",
            TerminalAgent::OpenCode => "icons/opencode-color.svg",
            TerminalAgent::Pi => "icons/pi-coding-agent.svg",
            TerminalAgent::Hermes => "icons/hermesagent.svg",
            TerminalAgent::Grok => "agents/grok.svg",
            TerminalAgent::Amp => "agents/amp-color.svg",
            TerminalAgent::Cursor => "agents/cursor.svg",
            TerminalAgent::Gemini => "agents/gemini-color.svg",
            TerminalAgent::Kiro => "agents/kiro-color.svg",
            TerminalAgent::Antigravity => "agents/antigravity-color.svg",
            TerminalAgent::Copilot => "agents/githubcopilot.svg",
            TerminalAgent::CodeBuddy => "agents/codebuddy-color.svg",
            TerminalAgent::Factory => "agents/factory.svg",
            TerminalAgent::Qoder => "agents/qoder-color.svg",
            TerminalAgent::Openclaw => "agents/openclaw-color.svg",
        }
    }

    pub fn accent(self) -> Option<u32> {
        match self {
            TerminalAgent::ClaudeCode => Some(0xd97757),
            TerminalAgent::Amp => Some(0xF34E3F),
            TerminalAgent::Qoder => Some(0x2ADB5C),
            TerminalAgent::Codex
            | TerminalAgent::OpenCode
            | TerminalAgent::Pi
            | TerminalAgent::Hermes
            | TerminalAgent::Grok
            | TerminalAgent::Cursor
            | TerminalAgent::Gemini
            | TerminalAgent::Kiro
            | TerminalAgent::Antigravity
            | TerminalAgent::Copilot
            | TerminalAgent::CodeBuddy
            | TerminalAgent::Factory
            | TerminalAgent::Openclaw => None,
        }
    }

    pub fn icon_multicolor(self) -> bool {
        matches!(
            self,
            TerminalAgent::Antigravity
                | TerminalAgent::CodeBuddy
                | TerminalAgent::Gemini
                | TerminalAgent::Kiro
                | TerminalAgent::Openclaw
        )
    }

    pub fn tag(self) -> &'static str {
        match self {
            TerminalAgent::ClaudeCode => "claude_code",
            TerminalAgent::Codex => "codex",
            TerminalAgent::OpenCode => "opencode",
            TerminalAgent::Pi => "pi",
            TerminalAgent::Hermes => "hermes",
            TerminalAgent::Grok => "grok",
            TerminalAgent::Amp => "amp",
            TerminalAgent::Cursor => "cursor",
            TerminalAgent::Gemini => "gemini",
            TerminalAgent::Kiro => "kiro",
            TerminalAgent::Antigravity => "antigravity",
            TerminalAgent::Copilot => "copilot",
            TerminalAgent::CodeBuddy => "codebuddy",
            TerminalAgent::Factory => "factory",
            TerminalAgent::Qoder => "qoder",
            TerminalAgent::Openclaw => "openclaw",
        }
    }

    pub fn from_binary(name: &str) -> Option<TerminalAgent> {
        TerminalAgent::ALL
            .iter()
            .copied()
            .find(|a| a.binary() == name)
    }

    pub fn from_launch_command(command: &str) -> Option<TerminalAgent> {
        command.split(['&', '|', ';', '\n']).find_map(|segment| {
            let token = segment
                .split_whitespace()
                .find(|token| !is_env_assignment(token))?;
            let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
            TerminalAgent::from_binary(strip_windows_exec_suffix(base))
        })
    }

    pub fn from_tag(tag: &str) -> Option<TerminalAgent> {
        match tag {
            "claude_code" => Some(TerminalAgent::ClaudeCode),
            "codex" => Some(TerminalAgent::Codex),
            "opencode" => Some(TerminalAgent::OpenCode),
            "pi" => Some(TerminalAgent::Pi),
            "hermes" => Some(TerminalAgent::Hermes),
            "grok" => Some(TerminalAgent::Grok),
            "amp" => Some(TerminalAgent::Amp),
            "cursor" => Some(TerminalAgent::Cursor),
            "gemini" => Some(TerminalAgent::Gemini),
            "kiro" => Some(TerminalAgent::Kiro),
            "antigravity" => Some(TerminalAgent::Antigravity),
            "copilot" => Some(TerminalAgent::Copilot),
            "codebuddy" => Some(TerminalAgent::CodeBuddy),
            "factory" => Some(TerminalAgent::Factory),
            "qoder" => Some(TerminalAgent::Qoder),
            "openclaw" => Some(TerminalAgent::Openclaw),
            _ => None,
        }
    }

    pub fn is_visible(self, config: &PaneFlowConfig) -> bool {
        let explicit: Option<bool> = match self {
            TerminalAgent::ClaudeCode => config.claude_code_button_visible,
            TerminalAgent::Codex => config.codex_button_visible,
            TerminalAgent::OpenCode => config.opencode_button_visible,
            TerminalAgent::Pi => config.pi_button_visible,
            TerminalAgent::Hermes => config.hermes_agent_button_visible,
            TerminalAgent::Grok => config.grok_button_visible,
            TerminalAgent::Amp => config.amp_button_visible,
            TerminalAgent::Cursor => config.cursor_button_visible,
            TerminalAgent::Gemini => config.gemini_button_visible,
            TerminalAgent::Kiro => config.kiro_button_visible,
            TerminalAgent::Antigravity => config.antigravity_button_visible,
            TerminalAgent::Copilot => config.copilot_button_visible,
            TerminalAgent::CodeBuddy => config.codebuddy_button_visible,
            TerminalAgent::Factory => config.factory_button_visible,
            TerminalAgent::Qoder => config.qoder_button_visible,
            TerminalAgent::Openclaw => config.openclaw_button_visible,
        };
        explicit.unwrap_or_else(|| self.is_installed())
    }

    pub fn binary(self) -> &'static str {
        match self {
            TerminalAgent::ClaudeCode => "claude",
            TerminalAgent::Codex => "codex",
            TerminalAgent::OpenCode => "opencode",
            TerminalAgent::Pi => "pi",
            TerminalAgent::Hermes => "hermes",
            TerminalAgent::Grok => "grok",
            TerminalAgent::Amp => "amp",
            TerminalAgent::Cursor => "cursor-agent",
            TerminalAgent::Gemini => "gemini",
            TerminalAgent::Kiro => "kiro-cli",
            TerminalAgent::Antigravity => "agy",
            TerminalAgent::Copilot => "copilot",
            TerminalAgent::CodeBuddy => "codebuddy",
            TerminalAgent::Factory => "droid",
            TerminalAgent::Qoder => "qodercli",
            TerminalAgent::Openclaw => "openclaw",
        }
    }

    pub fn is_installed(self) -> bool {
        installed_binaries_contains(self.binary())
    }

    fn command_args(self) -> &'static [&'static str] {
        match self {
            TerminalAgent::Kiro => &["chat"],
            TerminalAgent::Openclaw => &["tui"],
            _ => &[],
        }
    }

    fn launch_spec(self, config: &PaneFlowConfig) -> AgentCommandSpec {
        let mut spec = AgentCommandSpec::new(self.binary());
        spec.extend_args(self.command_args().iter().copied());
        if self == TerminalAgent::ClaudeCode
            && config.claude_code_bypass_permissions.unwrap_or(false)
        {
            spec.push_arg("--permission-mode");
            spec.push_arg("bypassPermissions");
        }
        spec
    }

    fn command(self, config: &PaneFlowConfig) -> String {
        self.launch_spec(config).render_shell_command()
    }

    pub fn session_agent(self) -> Option<crate::agent_sessions::SessionAgent> {
        use crate::agent_sessions::SessionAgent;
        match self {
            TerminalAgent::ClaudeCode => Some(SessionAgent::Claude),
            TerminalAgent::Codex => Some(SessionAgent::Codex),
            TerminalAgent::OpenCode => Some(SessionAgent::OpenCode),
            TerminalAgent::Pi => Some(SessionAgent::Pi),
            TerminalAgent::Hermes => Some(SessionAgent::Hermes),
            TerminalAgent::Grok => Some(SessionAgent::Grok),
            TerminalAgent::Cursor => Some(SessionAgent::Cursor),
            TerminalAgent::Gemini => Some(SessionAgent::Gemini),
            TerminalAgent::Kiro => Some(SessionAgent::Kiro),
            _ => None,
        }
    }

    pub fn launch_command(self, config: &PaneFlowConfig) -> String {
        wrap_for_shell(&self.command(config), config)
    }

    pub fn visible(config: &PaneFlowConfig) -> Vec<TerminalAgent> {
        TerminalAgent::ALL
            .into_iter()
            .filter(|a| a.is_visible(config))
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentCommandSpec {
    program: &'static str,
    args: Vec<String>,
}

impl AgentCommandSpec {
    pub(crate) fn new(program: &'static str) -> Self {
        Self {
            program,
            args: Vec::new(),
        }
    }

    pub(crate) fn push_arg(&mut self, arg: impl Into<String>) {
        self.args.push(arg.into());
    }

    fn extend_args(&mut self, args: impl IntoIterator<Item = &'static str>) {
        self.args.extend(args.into_iter().map(str::to_string));
    }

    pub(crate) fn render_shell_command(&self) -> String {
        debug_assert!(is_plain_shell_token(self.program));
        let mut command = self.program.to_string();
        for arg in &self.args {
            debug_assert!(is_plain_shell_token(arg));
            command.push(' ');
            command.push_str(arg);
        }
        command
    }
}

pub(crate) fn is_plain_shell_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'='))
}

struct InstalledBinaryCache {
    checked_at: Option<Instant>,
    found: HashSet<&'static str>,
}

impl InstalledBinaryCache {
    fn refresh(&mut self) {
        self.found = TerminalAgent::ALL
            .into_iter()
            .map(TerminalAgent::binary)
            .filter(|bin| which::which(bin).is_ok())
            .collect();
        self.checked_at = Some(Instant::now());
    }

    fn is_stale(&self) -> bool {
        self.checked_at
            .is_none_or(|checked_at| checked_at.elapsed() >= INSTALLED_BINARIES_TTL)
    }
}

const INSTALLED_BINARIES_TTL: Duration = Duration::from_secs(2);

fn installed_binary_cache() -> &'static Mutex<InstalledBinaryCache> {
    static CACHE: OnceLock<Mutex<InstalledBinaryCache>> = OnceLock::new();
    CACHE.get_or_init(|| {
        Mutex::new(InstalledBinaryCache {
            checked_at: None,
            found: HashSet::new(),
        })
    })
}

fn installed_binaries_contains(binary: &'static str) -> bool {
    let mut cache = match installed_binary_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => {
            tracing::warn!(
                target: "paneflow_app::agent_launcher",
                "installed binary cache mutex poisoned; refreshing recovered state"
            );
            poisoned.into_inner()
        }
    };
    if cache.is_stale() {
        cache.refresh();
    }
    cache.found.contains(binary)
}

fn version_cache() -> &'static Mutex<HashMap<TerminalAgent, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<TerminalAgent, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

fn probe_version(binary: &std::path::Path) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(binary)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if started.elapsed() < VERSION_PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(50));
            }
            _ => break false,
        }
    };
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    }
    let mut output = String::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_string(&mut output)
        .ok()?;
    parse_version(&output)
}

fn parse_version(output: &str) -> Option<String> {
    output
        .lines()
        .take(3)
        .flat_map(str::split_whitespace)
        .map(|token| {
            token
                .trim_start_matches(['v', 'V'])
                .trim_end_matches([',', ')', ';'])
        })
        .find(|token| {
            token.starts_with(|c: char| c.is_ascii_digit())
                && token.contains('.')
                && token.len() <= 32
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        })
        .map(str::to_string)
}

fn is_env_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((key, _)) => {
            !key.is_empty()
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !key.starts_with(|c: char| c.is_ascii_digit())
        }
        None => false,
    }
}

fn strip_windows_exec_suffix(base: &str) -> &str {
    for suffix in [".exe", ".cmd", ".bat", ".ps1"] {
        if base
            .get(base.len().saturating_sub(suffix.len())..)
            .is_some_and(|s| s.eq_ignore_ascii_case(suffix))
        {
            return &base[..base.len() - suffix.len()];
        }
    }
    base
}

fn wrap_for_shell(command: &str, config: &PaneFlowConfig) -> String {
    let shell = config
        .default_shell
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    crate::terminal::shell::clear_then(command, shell)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub name: String,
    pub agent: TerminalAgent,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

impl AgentProfile {
    pub fn from_config(entry: &AgentProfileConfig) -> Result<AgentProfile, String> {
        let name = entry.name.trim();
        if name.is_empty() {
            return Err("profile name is empty".to_string());
        }
        let Some(agent) = TerminalAgent::from_tag(entry.agent.trim()) else {
            return Err(format!("unknown base agent '{}'", entry.agent));
        };
        if let Some(key) = entry.env.keys().find(|key| !is_env_key(key)) {
            return Err(format!("invalid environment variable name '{key}'"));
        }
        if let Some(arg) = entry.args.iter().find(|arg| !is_plain_shell_token(arg)) {
            return Err(format!(
                "argument '{arg}' may only contain letters, digits, '-', '_', '.', and '='"
            ));
        }
        Ok(AgentProfile {
            name: name.to_string(),
            agent,
            env: entry.env.clone(),
            args: entry.args.clone(),
        })
    }

    pub fn all(config: &PaneFlowConfig) -> Vec<AgentProfile> {
        config
            .agent_profiles
            .iter()
            .filter_map(|entry| match Self::from_config(entry) {
                Ok(profile) => Some(profile),
                Err(reason) => {
                    log::warn!("agent_profiles: skipping '{}': {reason}", entry.name);
                    None
                }
            })
            .collect()
    }

    pub fn launch_command(&self, config: &PaneFlowConfig) -> String {
        let mut spec = self.agent.launch_spec(config);
        for arg in &self.args {
            spec.push_arg(arg.clone());
        }
        wrap_for_shell(&spec.render_shell_command(), config)
    }

    pub fn process_env(&self) -> HashMap<String, String> {
        let home = dirs::home_dir();
        self.env
            .iter()
            .map(|(key, value)| (key.clone(), expand_home(value, home.as_deref())))
            .collect()
    }
}

fn is_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn expand_home(value: &str, home: Option<&std::path::Path>) -> String {
    let Some(home) = home else {
        return value.to_string();
    };
    if value == "~" {
        return home.display().to_string();
    }
    match value
        .strip_prefix("~/")
        .or_else(|| value.strip_prefix("~\\"))
    {
        Some(rest) => home.join(rest).display().to_string(),
        None => value.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLaunch {
    Builtin(TerminalAgent),
    Profile(AgentProfile),
}

impl AgentLaunch {
    pub fn all(config: &PaneFlowConfig) -> Vec<AgentLaunch> {
        TerminalAgent::ALL
            .into_iter()
            .map(AgentLaunch::Builtin)
            .chain(
                AgentProfile::all(config)
                    .into_iter()
                    .map(AgentLaunch::Profile),
            )
            .collect()
    }

    pub fn visible(config: &PaneFlowConfig) -> Vec<AgentLaunch> {
        TerminalAgent::visible(config)
            .into_iter()
            .map(AgentLaunch::Builtin)
            .chain(
                AgentProfile::all(config)
                    .into_iter()
                    .map(AgentLaunch::Profile),
            )
            .collect()
    }

    pub fn agent(&self) -> TerminalAgent {
        match self {
            AgentLaunch::Builtin(agent) => *agent,
            AgentLaunch::Profile(profile) => profile.agent,
        }
    }

    pub fn label(&self) -> String {
        match self {
            AgentLaunch::Builtin(agent) => agent.display_name().to_string(),
            AgentLaunch::Profile(profile) => profile.name.clone(),
        }
    }

    pub fn key(&self) -> String {
        match self {
            AgentLaunch::Builtin(agent) => agent.tag().to_string(),
            AgentLaunch::Profile(profile) => format!("profile-{}", profile.name),
        }
    }

    pub fn is_installed(&self) -> bool {
        self.agent().is_installed()
    }

    pub fn launch_command(&self, config: &PaneFlowConfig) -> String {
        match self {
            AgentLaunch::Builtin(agent) => agent.launch_command(config),
            AgentLaunch::Profile(profile) => profile.launch_command(config),
        }
    }

    pub fn process_env(&self) -> Option<HashMap<String, String>> {
        match self {
            AgentLaunch::Builtin(_) => None,
            AgentLaunch::Profile(profile) => {
                Some(profile.process_env()).filter(|env| !env.is_empty())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_command_declares_its_own_agent() {
        let config = PaneFlowConfig::default();
        for agent in TerminalAgent::ALL {
            assert_eq!(
                TerminalAgent::from_launch_command(&agent.launch_command(&config)),
                Some(agent),
                "{} launch command must declare itself",
                agent.display_name()
            );
        }
    }

    #[test]
    fn from_launch_command_handles_paths_env_and_windows_suffixes() {
        assert_eq!(
            TerminalAgent::from_launch_command("/usr/local/bin/claude --resume abc"),
            Some(TerminalAgent::ClaudeCode)
        );
        assert_eq!(
            TerminalAgent::from_launch_command("RUST_LOG=info NO_COLOR=1 codex"),
            Some(TerminalAgent::Codex)
        );
        assert_eq!(
            TerminalAgent::from_launch_command("C:\\Users\\a\\bin\\claude.CMD"),
            Some(TerminalAgent::ClaudeCode)
        );
        assert_eq!(TerminalAgent::from_launch_command("npm run claude"), None);
        assert_eq!(TerminalAgent::from_launch_command("claude-wrapper"), None);
        assert_eq!(TerminalAgent::from_launch_command(""), None);
        assert_eq!(TerminalAgent::from_launch_command("   "), None);
        assert_eq!(TerminalAgent::from_launch_command("--model=x codex"), None);
    }

    #[test]
    fn tag_roundtrip() {
        for agent in TerminalAgent::ALL {
            assert_eq!(TerminalAgent::from_tag(agent.tag()), Some(agent));
        }
        assert_eq!(TerminalAgent::from_tag("unknown"), None);
    }

    #[test]
    fn from_tag_rejects_hostile_session_values() {
        assert_eq!(TerminalAgent::from_tag(""), None);
        assert_eq!(
            TerminalAgent::from_tag("Claude_Code"),
            None,
            "case-sensitive"
        );
        assert_eq!(TerminalAgent::from_tag("claude_code "), None, "no trim");
        assert_eq!(TerminalAgent::from_tag("claude_code\u{202e}"), None);
        assert_eq!(TerminalAgent::from_tag("codex\n"), None);
        assert_eq!(TerminalAgent::from_tag(&"x".repeat(10_000)), None);
    }

    #[test]
    fn binary_roundtrip_via_from_binary() {
        for agent in TerminalAgent::ALL {
            assert_eq!(TerminalAgent::from_binary(agent.binary()), Some(agent));
        }
        assert_eq!(TerminalAgent::from_binary("bash"), None);
        assert_eq!(TerminalAgent::from_binary("claude-code-cli"), None);
    }

    #[test]
    fn binary_is_launch_command_leading_token() {
        let cfg = PaneFlowConfig::default();
        for agent in TerminalAgent::ALL {
            let command = agent.command(&cfg);
            let leading = command.split_whitespace().next().unwrap_or_default();
            assert_eq!(
                leading,
                agent.binary(),
                "{} binary must match its launch command's leading token",
                agent.display_name()
            );
        }
    }

    #[test]
    fn explicit_visibility_overrides_install_detection() {
        let shown = PaneFlowConfig {
            gemini_button_visible: Some(true),
            ..Default::default()
        };
        assert!(TerminalAgent::Gemini.is_visible(&shown));

        let hidden = PaneFlowConfig {
            gemini_button_visible: Some(false),
            ..Default::default()
        };
        assert!(!TerminalAgent::Gemini.is_visible(&hidden));
    }

    #[test]
    fn icon_paths_are_embedded_assets() {
        for agent in TerminalAgent::ALL {
            let p = agent.icon_path();
            assert!(
                p.starts_with("icons/") || p.starts_with("agents/"),
                "{} icon path `{p}` is not under an embedded asset root",
                agent.display_name()
            );
        }
    }

    #[test]
    fn claude_bypass_flag_toggles_command() {
        let off = PaneFlowConfig {
            claude_code_bypass_permissions: Some(false),
            ..Default::default()
        };
        assert_eq!(TerminalAgent::ClaudeCode.command(&off), "claude");
        let on = PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..Default::default()
        };
        assert_eq!(
            TerminalAgent::ClaudeCode.command(&on),
            "claude --permission-mode bypassPermissions"
        );
    }

    #[test]
    fn non_claude_agents_ignore_bypass() {
        let config = PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..Default::default()
        };
        assert_eq!(TerminalAgent::Codex.command(&config), "codex");
        assert_eq!(TerminalAgent::Pi.command(&config), "pi");
        assert_eq!(TerminalAgent::Hermes.command(&config), "hermes");
    }

    #[test]
    fn launch_spec_keeps_program_and_args_structured_until_render() {
        let cfg = PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..Default::default()
        };

        let spec = TerminalAgent::ClaudeCode.launch_spec(&cfg);

        assert_eq!(spec.program, "claude");
        assert_eq!(spec.args, vec!["--permission-mode", "bypassPermissions"]);
        assert_eq!(
            spec.render_shell_command(),
            "claude --permission-mode bypassPermissions"
        );
    }

    #[test]
    fn launch_spec_plain_token_guard_matches_agent_command_surface() {
        for agent in TerminalAgent::ALL {
            assert!(
                is_plain_shell_token(agent.binary()),
                "{} binary must stay a plain shell token",
                agent.display_name()
            );
            for arg in agent.command_args() {
                assert!(
                    is_plain_shell_token(arg),
                    "{} arg `{arg}` must stay a plain shell token",
                    agent.display_name()
                );
            }
        }
        assert!(is_plain_shell_token(SAMPLE_UUID));
        assert!(!is_plain_shell_token("two words"));
        assert!(!is_plain_shell_token("$(reboot)"));
    }

    const SAMPLE_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn session_agent_maps_only_readable_stores() {
        use crate::agent_sessions::SessionAgent;
        assert_eq!(
            TerminalAgent::ClaudeCode.session_agent(),
            Some(SessionAgent::Claude)
        );
        assert_eq!(
            TerminalAgent::Codex.session_agent(),
            Some(SessionAgent::Codex)
        );
        assert_eq!(
            TerminalAgent::OpenCode.session_agent(),
            Some(SessionAgent::OpenCode)
        );
        assert_eq!(TerminalAgent::Pi.session_agent(), Some(SessionAgent::Pi));
        assert_eq!(
            TerminalAgent::Hermes.session_agent(),
            Some(SessionAgent::Hermes)
        );
        assert_eq!(
            TerminalAgent::Grok.session_agent(),
            Some(SessionAgent::Grok)
        );
        assert_eq!(
            TerminalAgent::Cursor.session_agent(),
            Some(SessionAgent::Cursor)
        );
        assert_eq!(
            TerminalAgent::Gemini.session_agent(),
            Some(SessionAgent::Gemini)
        );
        assert_eq!(
            TerminalAgent::Kiro.session_agent(),
            Some(SessionAgent::Kiro)
        );
        assert_eq!(TerminalAgent::Amp.session_agent(), None);
        assert_eq!(TerminalAgent::Antigravity.session_agent(), None);
        assert_eq!(TerminalAgent::Copilot.session_agent(), None);
        assert_eq!(TerminalAgent::CodeBuddy.session_agent(), None);
        assert_eq!(TerminalAgent::Factory.session_agent(), None);
        assert_eq!(TerminalAgent::Qoder.session_agent(), None);
        assert_eq!(TerminalAgent::Openclaw.session_agent(), None);
    }

    #[test]
    fn bare_commands_preserve_multi_token_agent_commands() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(TerminalAgent::Kiro.command(&cfg), "kiro-cli chat");
        assert_eq!(TerminalAgent::Openclaw.command(&cfg), "openclaw tui");
    }

    fn profile_entry(name: &str, agent: &str) -> AgentProfileConfig {
        AgentProfileConfig {
            name: name.to_string(),
            agent: agent.to_string(),
            env: BTreeMap::from([(
                "CLAUDE_CONFIG_DIR".to_string(),
                "~/.claude-perso".to_string(),
            )]),
            args: vec!["--model".to_string(), "opus".to_string()],
        }
    }

    #[test]
    fn profile_launch_command_extends_base_agent_and_declares_it() {
        let config = PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..PaneFlowConfig::default()
        };
        let profile =
            AgentProfile::from_config(&profile_entry("Claude perso", "claude_code")).unwrap();
        let command = profile.launch_command(&config);
        assert!(
            command.ends_with("claude --permission-mode bypassPermissions --model opus"),
            "{command}"
        );
        assert_eq!(
            TerminalAgent::from_launch_command(&command),
            Some(TerminalAgent::ClaudeCode)
        );
    }

    #[test]
    fn profile_rejects_unknown_agent_blank_name_and_unsafe_tokens() {
        assert!(AgentProfile::from_config(&profile_entry("x", "claude")).is_err());
        assert!(AgentProfile::from_config(&profile_entry("  ", "claude_code")).is_err());
        let mut bad_arg = profile_entry("x", "claude_code");
        bad_arg.args = vec!["--model opus; rm -rf /".to_string()];
        assert!(AgentProfile::from_config(&bad_arg).is_err());
        let mut bad_env = profile_entry("x", "claude_code");
        bad_env.env = BTreeMap::from([("1BAD KEY".to_string(), "v".to_string())]);
        assert!(AgentProfile::from_config(&bad_env).is_err());
    }

    #[test]
    fn invalid_profiles_are_skipped_not_fatal() {
        let config = PaneFlowConfig {
            agent_profiles: vec![profile_entry("Good", "codex"), profile_entry("Bad", "nope")],
            ..PaneFlowConfig::default()
        };
        let profiles = AgentProfile::all(&config);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].agent, TerminalAgent::Codex);
        assert_eq!(
            AgentLaunch::all(&config).len(),
            TerminalAgent::ALL.len() + 1
        );
    }

    #[test]
    fn profile_env_expands_leading_tilde_only() {
        let home = std::path::Path::new("/home/u");
        assert_eq!(
            expand_home("~/.claude-perso", Some(home)),
            home.join(".claude-perso").display().to_string()
        );
        assert_eq!(expand_home("~", Some(home)), "/home/u");
        assert_eq!(expand_home("~user/x", Some(home)), "~user/x");
        assert_eq!(expand_home("/abs/~/x", Some(home)), "/abs/~/x");
        assert_eq!(expand_home("~/x", None), "~/x");
    }

    #[test]
    fn profile_keeps_name_env_and_args_from_config() {
        let entry = profile_entry("  Claude perso ", "claude_code");
        let profile = AgentProfile::from_config(&entry).unwrap();
        assert_eq!(profile.name, "Claude perso");
        assert_eq!(profile.agent, TerminalAgent::ClaudeCode);
        assert_eq!(profile.env, entry.env);
        assert_eq!(profile.args, entry.args);
    }

    #[test]
    fn parse_version_takes_the_first_dotted_number() {
        assert_eq!(parse_version("2.1.0 (Claude Code)"), Some("2.1.0".into()));
        assert_eq!(parse_version("codex-cli 0.58.0"), Some("0.58.0".into()));
        assert_eq!(
            parse_version("v1.2.4-beta.1\n"),
            Some("1.2.4-beta.1".into())
        );
        assert_eq!(
            parse_version("Gemini CLI version: 0.22.1,"),
            Some("0.22.1".into())
        );
        assert_eq!(parse_version("no numbers here"), None);
        assert_eq!(parse_version("build 20260908"), None);
    }

    #[test]
    fn primary_and_secondary_agents_partition_all() {
        let mut seen: Vec<TerminalAgent> = TerminalAgent::PRIMARY.to_vec();
        seen.extend(TerminalAgent::secondary());
        assert_eq!(seen.len(), TerminalAgent::ALL.len());
        for agent in TerminalAgent::ALL {
            assert!(seen.contains(&agent));
        }
    }

    #[test]
    fn visibility_config_key_matches_is_visible_field() {
        for agent in TerminalAgent::ALL {
            let json = serde_json::json!({ agent.visibility_config_key(): false });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert!(!agent.is_visible(&config), "{}", agent.display_name());
        }
    }
}
