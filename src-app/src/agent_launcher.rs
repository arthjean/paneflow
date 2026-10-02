use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use paneflow_agent_config::{
    RUNTIMES, Runtime, launcher_runtimes, runtime_by_command_alias, runtime_by_id,
    runtime_by_preset_id,
};
use paneflow_config::schema::{AgentProfileConfig, PaneFlowConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TerminalAgent(&'static str);

paneflow_agent_config::runtime_identity_constants!(TerminalAgent);

impl TerminalAgent {
    pub fn all() -> impl Iterator<Item = TerminalAgent> {
        RUNTIMES.iter().map(|runtime| TerminalAgent(runtime.id))
    }

    #[allow(
        clippy::expect_used,
        reason = "TerminalAgent values are private catalog identities"
    )]
    pub fn runtime(self) -> &'static Runtime {
        runtime_by_id(self.0).expect("catalog identity must resolve")
    }

    pub fn visibility_config_key(self) -> &'static str {
        self.runtime().display.visibility_config_key
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
            TerminalAgent::all()
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
        usize::from(self.runtime().display.order)
    }

    pub fn display_name(self) -> &'static str {
        self.runtime().label
    }

    pub fn icon_path(self) -> &'static str {
        self.runtime().display.icon_asset_path
    }

    pub fn accent(self) -> Option<u32> {
        self.runtime().display.tint
    }

    pub fn icon_multicolor(self) -> bool {
        self.runtime().display.icon_multicolor
    }

    pub fn tag(self) -> &'static str {
        self.runtime().suggested_presets[0].id
    }

    pub fn from_binary(name: &str) -> Option<TerminalAgent> {
        runtime_by_command_alias(name).map(|runtime| TerminalAgent(runtime.id))
    }

    pub fn from_launch_command(command: &str) -> Option<TerminalAgent> {
        command.split(['&', '|', ';', '\n']).find_map(|segment| {
            let token = segment
                .split_whitespace()
                .find(|token| !is_env_assignment(token))?;
            TerminalAgent::from_binary(executable_stem(token))
        })
    }

    pub fn from_runtime_id(id: &str) -> Option<TerminalAgent> {
        runtime_by_id(id).map(|runtime| TerminalAgent(runtime.id))
    }

    pub fn from_tag(tag: &str) -> Option<TerminalAgent> {
        runtime_by_preset_id(tag).map(|runtime| TerminalAgent(runtime.id))
    }

    pub fn is_visible(self, config: &PaneFlowConfig) -> bool {
        config
            .agent_button_visible(self.visibility_config_key())
            .unwrap_or_else(|| self.is_installed())
    }

    pub fn binary(self) -> &'static str {
        self.runtime().detection.command_aliases[0]
    }

    pub fn is_installed(self) -> bool {
        installed_binaries_contains(self.binary())
    }

    pub fn is_installed_now(self) -> bool {
        if installed_binary_scan_pending() {
            refresh_installed_binaries();
        }
        installed_binaries_contains(self.binary())
    }

    fn launch_spec(self, config: &PaneFlowConfig) -> AgentCommandSpec {
        let mut tokens = self.runtime().suggested_presets[0]
            .command
            .split_whitespace();
        let mut spec = AgentCommandSpec::new(tokens.next().unwrap_or(self.binary()));
        spec.extend_args(tokens);
        self.push_launch_flags(&mut spec, config);
        spec
    }

    pub(crate) fn push_launch_flags(self, spec: &mut AgentCommandSpec, config: &PaneFlowConfig) {
        if self == TerminalAgent::ClaudeCode
            && config.claude_code_bypass_permissions.unwrap_or(false)
        {
            spec.push_arg("--permission-mode");
            spec.push_arg("bypassPermissions");
        }
    }

    fn command(self, config: &PaneFlowConfig) -> String {
        self.launch_spec(config).render_shell_command()
    }

    pub fn session_agent(self) -> Option<crate::agent_sessions::SessionAgent> {
        crate::agent_sessions::SessionAgent::of(self)
    }

    pub fn launch_command(self, config: &PaneFlowConfig) -> String {
        wrap_for_shell(&self.command(config), config)
    }

    pub fn visible(config: &PaneFlowConfig) -> Vec<TerminalAgent> {
        launcher_runtimes(
            |key| config.agent_button_visible(key),
            |runtime| installed_binaries_contains(runtime.detection.command_aliases[0]),
        )
        .into_iter()
        .map(|runtime| TerminalAgent(runtime.id))
        .collect()
    }

    pub fn supports_current_platform(self) -> bool {
        self.runtime().supports_current_platform()
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
    refreshing: bool,
    found: HashSet<&'static str>,
}

impl InstalledBinaryCache {
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
            refreshing: false,
            found: HashSet::new(),
        })
    })
}

fn lock_installed_binary_cache() -> std::sync::MutexGuard<'static, InstalledBinaryCache> {
    match installed_binary_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => {
            tracing::warn!(
                target: "paneflow_app::agent_launcher",
                "installed binary cache mutex poisoned; using recovered state"
            );
            poisoned.into_inner()
        }
    }
}

fn scan_installed_binaries() -> HashSet<&'static str> {
    TerminalAgent::all()
        .map(TerminalAgent::binary)
        .filter(|bin| which::which(bin).is_ok())
        .collect()
}

pub(crate) fn refresh_installed_binaries() {
    let found = scan_installed_binaries();
    let mut cache = lock_installed_binary_cache();
    cache.found = found;
    cache.checked_at = Some(Instant::now());
    cache.refreshing = false;
}

pub(crate) fn installed_binary_scan_pending() -> bool {
    let cache = lock_installed_binary_cache();
    cache.checked_at.is_none() || cache.refreshing
}

fn installed_binaries_contains(binary: &'static str) -> bool {
    let mut cache = lock_installed_binary_cache();
    if cache.is_stale() && !cache.refreshing {
        cache.refreshing = true;
        let spawned = std::thread::Builder::new()
            .name("paneflow-agent-scan".into())
            .spawn(refresh_installed_binaries);
        if let Err(error) = spawned {
            cache.refreshing = false;
            tracing::warn!(
                target: "paneflow_app::agent_launcher",
                "installed binary scan thread failed to start: {error}"
            );
        }
    }
    cache.found.contains(binary)
}

fn version_cache() -> &'static Mutex<HashMap<TerminalAgent, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<TerminalAgent, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(3);
const VERSION_PROBE_STDOUT_CAP: u64 = 4096;

fn probe_version(binary: &std::path::Path) -> Option<String> {
    let mut command = std::process::Command::new(binary);
    command.arg("--version");
    let output = paneflow_process::run_with_timeout(
        command,
        VERSION_PROBE_TIMEOUT,
        VERSION_PROBE_STDOUT_CAP,
    )
    .ok()?;
    parse_version(std::str::from_utf8(&output.stdout).ok()?)
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

pub(crate) fn executable_stem(token: &str) -> &str {
    let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
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
        let lookup = |name: &str| std::env::var(name).ok();
        self.env
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    expand_profile_value(key, value, home.as_deref(), &lookup),
                )
            })
            .collect()
    }
}

fn is_env_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn expand_profile_value(
    key: &str,
    value: &str,
    home: Option<&std::path::Path>,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> String {
    match crate::env_expand::expand_env_value(value, home, lookup) {
        Ok(expanded) => expanded,
        Err(name) => {
            log::warn!(
                "agent_profiles: '{key}' references '{name}', which is not set; passing the value through unchanged"
            );
            value.to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentLaunch {
    Builtin(TerminalAgent),
    Profile(AgentProfile),
}

impl AgentLaunch {
    pub fn all(config: &PaneFlowConfig) -> Vec<AgentLaunch> {
        TerminalAgent::all()
            .filter(|agent| agent.supports_current_platform())
            .map(AgentLaunch::Builtin)
            .chain(
                AgentProfile::all(config)
                    .into_iter()
                    .filter(|profile| profile.agent.supports_current_platform())
                    .map(AgentLaunch::Profile),
            )
            .collect()
    }

    pub fn visible(config: &PaneFlowConfig) -> Vec<AgentLaunch> {
        TerminalAgent::visible(config)
            .into_iter()
            .filter(|agent| agent.supports_current_platform())
            .map(AgentLaunch::Builtin)
            .chain(
                AgentProfile::all(config)
                    .into_iter()
                    .filter(|profile| profile.agent.supports_current_platform())
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
fn prime_installed_binary_cache(found: HashSet<&'static str>) {
    let mut cache = lock_installed_binary_cache();
    cache.found = found;
    cache.checked_at = Some(Instant::now());
    cache.refreshing = false;
}

#[cfg(test)]
mod tests {
    #[test]
    fn installed_binary_reads_never_scan_on_the_caller_thread_once_warm() {
        super::prime_installed_binary_cache(std::collections::HashSet::from(["claude"]));
        assert!(!super::installed_binary_scan_pending());
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            assert!(super::installed_binaries_contains("claude"));
            assert!(!super::installed_binaries_contains(
                "paneflow-no-such-agent-binary"
            ));
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "warm reads must not walk PATH"
        );
    }

    use super::*;

    #[test]
    fn launch_command_declares_its_own_agent() {
        let config = PaneFlowConfig::default();
        for agent in TerminalAgent::all() {
            assert_eq!(
                TerminalAgent::from_launch_command(&agent.launch_command(&config)),
                Some(agent),
                "{} launch command must declare itself",
                agent.display_name()
            );
        }
    }

    #[test]
    fn executable_stem_strips_paths_and_windows_wrappers() {
        assert_eq!(executable_stem(r"C:\tools\codex.exe"), "codex");
        assert_eq!(executable_stem("vite.CMD"), "vite");
        assert_eq!(executable_stem("vite.cmd"), "vite");
        assert_eq!(executable_stem("script.ps1"), "script");
        assert_eq!(executable_stem("/usr/local/bin/claude"), "claude");
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
        for agent in TerminalAgent::all() {
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
        for agent in TerminalAgent::all() {
            assert_eq!(TerminalAgent::from_binary(agent.binary()), Some(agent));
        }
        assert_eq!(TerminalAgent::from_binary("bash"), None);
        assert_eq!(TerminalAgent::from_binary("claude-code-cli"), None);
    }

    #[test]
    fn binary_is_launch_command_leading_token() {
        let cfg = PaneFlowConfig::default();
        for agent in TerminalAgent::all() {
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
        let shown: PaneFlowConfig =
            serde_json::from_value(serde_json::json!({ "gemini_button_visible": true })).unwrap();
        assert!(TerminalAgent::Gemini.is_visible(&shown));

        let hidden: PaneFlowConfig =
            serde_json::from_value(serde_json::json!({ "gemini_button_visible": false })).unwrap();
        assert!(!TerminalAgent::Gemini.is_visible(&hidden));
    }

    #[test]
    fn icon_paths_are_embedded_assets() {
        for agent in TerminalAgent::all() {
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
        for agent in TerminalAgent::all() {
            assert!(
                is_plain_shell_token(agent.binary()),
                "{} binary must stay a plain shell token",
                agent.display_name()
            );
            for arg in agent.runtime().suggested_presets[0]
                .command
                .split_whitespace()
                .skip(1)
            {
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
    fn catalog_tables_match_the_pre_catalog_reference() {
        let reference: [(&str, &str, Option<&str>); 18] = [
            (
                "com.anthropic.claude-code",
                "claude_code_button_visible",
                Some("claude"),
            ),
            ("com.openai.codex", "codex_button_visible", Some("codex")),
            (
                "ai.opencode.cli",
                "opencode_button_visible",
                Some("opencode"),
            ),
            ("dev.mariozechner.pi", "pi_button_visible", Some("pi")),
            ("ai.hermes.agent", "hermes_agent_button_visible", None),
            ("ai.x.grok-cli", "grok_button_visible", Some("grok")),
            ("com.sourcegraph.amp", "amp_button_visible", None),
            ("com.cursor.agent", "cursor_button_visible", None),
            (
                "com.google.gemini-cli",
                "gemini_button_visible",
                Some("gemini"),
            ),
            ("com.amazon.kiro-cli", "kiro_button_visible", Some("kiro")),
            (
                "com.google.antigravity-cli",
                "antigravity_button_visible",
                None,
            ),
            ("com.github.copilot-cli", "copilot_button_visible", None),
            ("com.tencent.codebuddy", "codebuddy_button_visible", None),
            ("com.factory.droid", "factory_button_visible", None),
            ("com.alibaba.qoder-cli", "qoder_button_visible", None),
            ("ai.openclaw.cli", "openclaw_button_visible", None),
            (
                "ai.deepseek.harness",
                "deepseek_harness_button_visible",
                None,
            ),
            ("com.muse.code", "muse_button_visible", None),
        ];
        let generated = TerminalAgent::all()
            .map(|agent| {
                (
                    agent.runtime().id,
                    agent.visibility_config_key(),
                    agent.session_agent().map(|session| session.reader().name()),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(generated, reference);

        let everything_visible: PaneFlowConfig = serde_json::from_value(serde_json::Value::Object(
            reference
                .iter()
                .map(|(_, key, _)| (key.to_string(), serde_json::Value::Bool(true)))
                .collect(),
        ))
        .unwrap();
        assert_eq!(
            TerminalAgent::visible(&everything_visible)
                .into_iter()
                .map(|agent| agent.runtime().id)
                .collect::<Vec<_>>(),
            reference.iter().map(|(id, _, _)| *id).collect::<Vec<_>>(),
            "launcher order"
        );
        assert_eq!(
            crate::agent_sessions::SessionAgent::all()
                .map(|session| session.terminal_agent().runtime().id)
                .collect::<Vec<_>>(),
            [
                "com.anthropic.claude-code",
                "com.openai.codex",
                "ai.opencode.cli",
                "dev.mariozechner.pi",
                "ai.x.grok-cli",
                "com.google.gemini-cli",
                "com.amazon.kiro-cli",
            ],
            "session sidebar groups keep the order of the former SessionAgent enum"
        );
        assert_eq!(
            crate::auto_naming::summarizers().collect::<Vec<_>>(),
            [
                TerminalAgent::ClaudeCode,
                TerminalAgent::Codex,
                TerminalAgent::Opencode,
                TerminalAgent::Pi,
            ],
            "summarizer order"
        );
    }

    #[test]
    fn bare_commands_preserve_multi_token_agent_commands() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(TerminalAgent::Kiro.command(&cfg), "kiro-cli chat");
        assert_eq!(TerminalAgent::Openclaw.command(&cfg), "openclaw tui");
        assert_eq!(
            TerminalAgent::DeepseekHarness.command(&cfg),
            "dsh --profile tui"
        );
        assert_eq!(TerminalAgent::MuseCode.command(&cfg), "muse");
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
            TerminalAgent::all()
                .filter(|agent| agent.supports_current_platform())
                .count()
                + 1
        );
    }

    #[test]
    fn profile_env_expands_the_home_prefix_and_known_placeholders() {
        let home = std::path::Path::new("/home/u");
        let lookup = |name: &str| (name == "BASE").then(|| "/opt/base".to_string());
        assert_eq!(
            expand_profile_value("K", "~/.claude-perso", Some(home), &lookup),
            home.join(".claude-perso").display().to_string()
        );
        assert_eq!(
            expand_profile_value("K", "$BASE/x", Some(home), &lookup),
            "/opt/base/x"
        );
        assert_eq!(
            expand_profile_value("K", "%BASE%/x", Some(home), &lookup),
            "/opt/base/x"
        );
    }

    #[test]
    fn profile_env_passes_an_unresolvable_value_through_unchanged() {
        let home = std::path::Path::new("/home/u");
        let lookup = |_: &str| None;
        assert_eq!(
            expand_profile_value("K", "$NOPE/.claude-perso", Some(home), &lookup),
            "$NOPE/.claude-perso"
        );
        assert_eq!(
            expand_profile_value("K", "a%b%c", Some(home), &lookup),
            "a%b%c"
        );
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
    fn launch_presets_follow_catalog_platforms_without_hiding_presentation() {
        let config = PaneFlowConfig::default();
        let builtins = AgentLaunch::all(&config)
            .into_iter()
            .filter_map(|launch| match launch {
                AgentLaunch::Builtin(agent) => Some(agent),
                AgentLaunch::Profile(_) => None,
            })
            .collect::<Vec<_>>();
        assert!(
            builtins
                .iter()
                .all(|agent| agent.supports_current_platform())
        );
        if cfg!(windows) {
            assert_eq!(builtins.len(), 5);
            assert_eq!(
                TerminalAgent::from_tag("antigravity").map(TerminalAgent::display_name),
                Some("Antigravity")
            );
            assert!(!builtins.contains(&TerminalAgent::Antigravity));
        } else {
            assert_eq!(builtins.len(), TerminalAgent::all().count());
        }
    }

    #[test]
    fn visibility_config_key_matches_is_visible_field() {
        for agent in TerminalAgent::all() {
            let json = serde_json::json!({ agent.visibility_config_key(): false });
            let config: PaneFlowConfig = serde_json::from_value(json).unwrap();
            assert!(!agent.is_visible(&config), "{}", agent.display_name());
        }
    }
}
