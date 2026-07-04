//! Terminal-agent launcher: the CLI coding agents Paneflow starts in a
//! terminal pane (Claude Code, Codex, OpenCode, Pi, Hermes, plus the
//! cmux-derived set: Grok, Amp, Cursor, Gemini, Kiro, Antigravity,
//! Copilot, CodeBuddy, Factory, Qoder, plus Openclaw). Both the tab-bar
//! launcher buttons
//! (`pane.rs`) and the Agents-view "New thread" picker iterate this single
//! source of truth so the per-agent visibility gate and the "respect
//! bypass" contract can never drift between them.
//!
//! Each variant maps to a display name, an icon, an accent tint, a
//! Settings → AI Agent visibility flag (`*_button_visible`), a stable
//! persistence tag, and a launch command. The launch command honors
//! `claude_code_bypass_permissions` exactly as the tab bar does.

use std::collections::HashSet;
use std::sync::OnceLock;

use paneflow_config::schema::PaneFlowConfig;

/// One of the CLI coding agents Paneflow can launch in a terminal.
///
/// Distinct from [`paneflow_acp::AgentKind`] (Claude/Codex only, the ACP
/// wire agents): this is the broader set surfaced as terminal launchers
/// and bound to Agents-view Terminal Threads.
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
    /// Every variant, in display order (matches the tab-bar button row).
    /// The original five lead; the cmux-derived launchers follow so the
    /// button order is stable for users who upgraded from a 5-agent build.
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

    /// Stable display rank - index in [`Self::ALL`]. Used by the sidebar to
    /// order multi-tool status rows deterministically instead of letting
    /// `HashMap` iteration order leak into the UI.
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
            TerminalAgent::Codex => "icons/codex-color.svg",
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

    /// Brand accent for the icon tint, as a packed `0xRRGGBB`. `None`
    /// means "use the theme's primary text color" -- the OpenCode / Pi /
    /// Hermes logos are monochrome `currentColor` SVGs.
    pub fn accent(self) -> Option<u32> {
        match self {
            TerminalAgent::ClaudeCode => Some(0xd97757),
            TerminalAgent::Codex => Some(0x7a9dff),
            // Single-color brand logos: `svg()` renders a monochrome alpha
            // mask, so the silhouette is painted in this brand color.
            TerminalAgent::Amp => Some(0xF34E3F),
            TerminalAgent::Qoder => Some(0x2ADB5C),
            // The rest are either monochrome `currentColor` logos (tinted
            // with the theme's primary text color so they stay readable on
            // every theme) or multi-color logos rendered in their native
            // palette via `img()` (see `icon_multicolor`), where `accent`
            // is unused.
            TerminalAgent::OpenCode
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

    /// Whether the icon must be rendered in its native colors via `img()`
    /// (multi-color logos: gradients or several distinct fills) instead of
    /// a `text_color`-tinted monochrome `svg()` mask. GPUI's `svg()`
    /// flattens every path to one tint, which would destroy these palettes;
    /// `img()` rasterizes the SVG (resvg) and preserves every fill. A
    /// single-color brand logo stays monochrome and uses `accent()`.
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

    /// Stable persistence tag for the session.json `terminal_agent`
    /// field. Kept distinct from the binary name so a future rename of
    /// the CLI does not invalidate persisted threads.
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

    /// Map an ACP [`paneflow_acp::AgentKind`] (Claude/Codex only) to its
    /// terminal launcher. Used to relaunch legacy chat threads (which
    /// stored an `AgentKind`) as terminal sessions of the same agent.
    pub fn from_agent_kind(kind: paneflow_acp::AgentKind) -> TerminalAgent {
        match kind {
            paneflow_acp::AgentKind::ClaudeCode => TerminalAgent::ClaudeCode,
            paneflow_acp::AgentKind::Codex => TerminalAgent::Codex,
        }
    }

    /// EP-005 US-013: map a detected process basename back to its agent
    /// (reverse of [`Self::binary`]). Exact match only - the per-pane scan
    /// matches `/proc/<pid>/comm` verbatim, so a wrapper script or a
    /// suffixed binary never produces a pill.
    pub fn from_binary(name: &str) -> Option<TerminalAgent> {
        TerminalAgent::ALL
            .iter()
            .copied()
            .find(|a| a.binary() == name)
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

    /// Whether this launcher is shown in the tab bar / Agents-view picker.
    ///
    /// Tri-state on the `*_button_visible` config key:
    /// - `Some(true)`  - user explicitly enabled it: always shown.
    /// - `Some(false)` - user explicitly disabled it: always hidden.
    /// - `None` (key absent, the default) - shown only if the agent's CLI
    ///   binary is installed ([`Self::is_installed`]), so a fresh config
    ///   surfaces exactly the agents present on the machine. The user can
    ///   still force-show an uninstalled agent by toggling it on.
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

    /// The CLI executable looked up on `PATH` to decide default visibility;
    /// also the leading token of [`Self::launch_command`]. Cross-platform:
    /// `which` resolves Windows `.exe`/`PATHEXT` extensions.
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

    /// Whether this agent's CLI binary is found on `PATH`. Drives the
    /// default visibility in [`Self::is_visible`].
    ///
    /// Probed once per process and cached: `which` walks `PATH` for every
    /// agent, too costly to repeat on the render thread each frame. The
    /// cache is filled lazily on first call (after `main.rs` patches `PATH`
    /// for GUI launches), so installing an agent while Paneflow runs needs a
    /// restart to flip the default - the user can toggle it on immediately
    /// in Settings meanwhile.
    pub fn is_installed(self) -> bool {
        installed_binaries().contains(self.binary())
    }

    /// Static arguments appended after [`Self::binary`] for interactive agents
    /// whose CLI entry point is a subcommand rather than the bare executable.
    fn command_args(self) -> &'static [&'static str] {
        match self {
            TerminalAgent::Kiro => &["chat"],
            TerminalAgent::Openclaw => &["tui"],
            _ => &[],
        }
    }

    fn command_parts(self, config: &PaneFlowConfig) -> Vec<String> {
        let mut parts = Vec::with_capacity(4);
        parts.push(self.binary().to_string());
        parts.extend(self.command_args().iter().map(|arg| (*arg).to_string()));
        if self == TerminalAgent::ClaudeCode
            && config.claude_code_bypass_permissions.unwrap_or(false)
        {
            parts.push("--permission-mode".to_string());
            parts.push("bypassPermissions".to_string());
        }
        parts
    }

    /// Bare command that starts the agent. Honors
    /// `claude_code_bypass_permissions` for Claude Code.
    fn command(self, config: &PaneFlowConfig) -> String {
        render_command_parts(&self.command_parts(config))
    }

    /// Whether the CLI accepts a caller-forced session UUID via
    /// `--session-id <uuid>`. Only Claude Code does (verified against the
    /// CLI: a fresh id starts a new session, an existing id resumes +
    /// appends, so a stable per-thread id is safe across restarts). Other
    /// agents fall back to the newest-session heuristic in the title
    /// backfill.
    pub fn supports_forced_session_id(self) -> bool {
        matches!(self, TerminalAgent::ClaudeCode)
    }

    /// Map this launcher to the session reader PaneFlow can safely use.
    /// `None` means the CLI does not expose a documented local list+resume
    /// contract suitable for the sidebar yet.
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

    /// Like [`Self::command`] but injects a forced `--session-id <uuid>` for
    /// Claude when `session_id` is `Some` and passes the PTY allow-list. The
    /// flag lands right after the binary so it composes with the optional
    /// `--permission-mode bypassPermissions` already baked into the base
    /// command. Any other agent (or `None`) yields the plain base command.
    fn command_with_session(self, config: &PaneFlowConfig, session_id: Option<&str>) -> String {
        if self != TerminalAgent::ClaudeCode {
            return self.command(config);
        }
        let Some(id) = session_id.filter(|id| crate::agent_sessions::is_valid_session_id(id))
        else {
            return self.command(config);
        };
        let mut parts = self.command_parts(config);
        parts.insert(1, "--session-id".to_string());
        parts.insert(2, id.to_string());
        render_command_parts(&parts)
    }

    /// Shell-aware launch command. The clear prefix is selected for the
    /// configured shell (`clear`, `cls`, or `Clear-Host`) so the agent TUI owns
    /// the viewport from the first frame on every platform.
    pub fn launch_command(self, config: &PaneFlowConfig) -> String {
        self.launch_command_with_session(config, None)
    }

    /// [`Self::launch_command`] with a forced agent session id (Claude
    /// only - see [`Self::command_with_session`]). The Agents-view PTY
    /// mount passes the thread's bound `session_id` here so the live thread
    /// maps 1:1 to its on-disk session file.
    pub fn launch_command_with_session(
        self,
        config: &PaneFlowConfig,
        session_id: Option<&str>,
    ) -> String {
        // US-042: trim + drop-empty exactly like the PTY session does when it
        // resolves the shell (`pty_session.rs:442`). A config such as
        // `"default_shell": "  pwsh  "` otherwise reaches `clear_then`
        // untrimmed, fails the `which::which` probe, falls back to `cmd.exe`,
        // and emits the wrong clear arm (`cls && claude` for a POSIX command).
        let shell = config
            .default_shell
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        crate::terminal::shell::clear_then(&self.command_with_session(config, session_id), shell)
    }

    /// Visible variants for the given config, in display order. Drives
    /// both the Agents-view picker and (via the same gates) the tab bar.
    pub fn visible(config: &PaneFlowConfig) -> Vec<TerminalAgent> {
        TerminalAgent::ALL
            .into_iter()
            .filter(|a| a.is_visible(config))
            .collect()
    }
}

fn render_command_parts(parts: &[String]) -> String {
    parts.join(" ")
}

/// Agent binaries found on `PATH`, probed once and cached for the process
/// lifetime. Backing store for [`TerminalAgent::is_installed`].
fn installed_binaries() -> &'static HashSet<&'static str> {
    static CACHE: OnceLock<HashSet<&'static str>> = OnceLock::new();
    CACHE.get_or_init(|| {
        TerminalAgent::ALL
            .into_iter()
            .map(TerminalAgent::binary)
            .filter(|bin| which::which(bin).is_ok())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_roundtrip() {
        for agent in TerminalAgent::ALL {
            assert_eq!(TerminalAgent::from_tag(agent.tag()), Some(agent));
        }
        assert_eq!(TerminalAgent::from_tag("unknown"), None);
    }

    // EP-005 US-013: `from_tag` is the session.json ingress whitelist for
    // the persisted `agent` field - hostile or malformed values (oversized,
    // control chars, near-misses) must all map to None so no pill renders.
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
        // EP-005 US-013: the scan's comm match resolves back to the agent.
        for agent in TerminalAgent::ALL {
            assert_eq!(TerminalAgent::from_binary(agent.binary()), Some(agent));
        }
        assert_eq!(TerminalAgent::from_binary("bash"), None);
        assert_eq!(TerminalAgent::from_binary("claude-code-cli"), None);
    }

    #[test]
    fn binary_is_launch_command_leading_token() {
        // The PATH probe (`binary`) must match the actual executable the
        // launcher runs, or default visibility detects the wrong binary.
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
        // `Some(true)`/`Some(false)` win over PATH detection, so the result
        // is deterministic on any machine (and never touches the filesystem
        // here - the `unwrap_or_else` install probe is short-circuited).
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
        // Every icon must live under an embedded asset root (`icons/` or
        // `agents/`) or the tab-bar `svg()` silently renders nothing.
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

    const SAMPLE_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn only_claude_supports_forced_session_id() {
        assert!(TerminalAgent::ClaudeCode.supports_forced_session_id());
        for agent in TerminalAgent::ALL
            .into_iter()
            .filter(|a| *a != TerminalAgent::ClaudeCode)
        {
            assert!(
                !agent.supports_forced_session_id(),
                "{} must not force a session id",
                agent.display_name()
            );
        }
    }

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
    fn claude_session_id_is_injected_after_binary() {
        let cfg = PaneFlowConfig::default();
        let cmd = TerminalAgent::ClaudeCode.command_with_session(&cfg, Some(SAMPLE_UUID));
        assert_eq!(cmd, format!("claude --session-id {SAMPLE_UUID}"));
        // Leading token stays `claude` (the PATH-probe invariant).
        assert_eq!(cmd.split_whitespace().next(), Some("claude"));
    }

    #[test]
    fn claude_session_id_composes_with_bypass() {
        let cfg = PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..Default::default()
        };
        let cmd = TerminalAgent::ClaudeCode.command_with_session(&cfg, Some(SAMPLE_UUID));
        assert_eq!(
            cmd,
            format!("claude --session-id {SAMPLE_UUID} --permission-mode bypassPermissions")
        );
    }

    #[test]
    fn invalid_session_id_is_not_injected() {
        // Flag-shaped / shell-meta ids fail the allow-list, so a tampered
        // session.json can never smuggle a second argument into the launch.
        let cfg = PaneFlowConfig::default();
        for hostile in [
            "--dangerously-skip-permissions",
            "-x",
            "x; rm -rf ~",
            "$(reboot)",
        ] {
            assert_eq!(
                TerminalAgent::ClaudeCode.command_with_session(&cfg, Some(hostile)),
                "claude",
                "hostile id {hostile:?} must be dropped"
            );
        }
    }

    #[test]
    fn bare_commands_preserve_multi_token_agent_commands() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(TerminalAgent::Kiro.command(&cfg), "kiro-cli chat");
        assert_eq!(TerminalAgent::Openclaw.command(&cfg), "openclaw tui");
    }

    #[test]
    fn non_claude_ignores_forced_session_id() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(
            TerminalAgent::Codex.command_with_session(&cfg, Some(SAMPLE_UUID)),
            "codex"
        );
        assert_eq!(
            TerminalAgent::OpenCode.command_with_session(&cfg, Some(SAMPLE_UUID)),
            "opencode"
        );
    }

    #[test]
    fn claude_without_session_id_is_bare_command() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(
            TerminalAgent::ClaudeCode.command_with_session(&cfg, None),
            "claude"
        );
    }
}
