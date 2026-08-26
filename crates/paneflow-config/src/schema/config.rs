use super::{
    AgentPanelConfig, CommandDefinition, CursorBlinkConfig, CursorShapeConfig, TelemetryConfig,
    TerminalConfig,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level PaneFlow configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneFlowConfig {
    /// Key-action shortcut mappings (e.g. "ctrl+t" -> "new_tab").
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shortcuts: HashMap<String, String>,
    /// Default shell binary path. `None` uses the system default.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub default_shell: Option<String>,
    /// Terminal color theme name (e.g. "One Dark", "PaneFlow Light", "Vercel", "Claude", "Cursor").
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme: Option<String>,
    /// Theme selection mode: `"light"`, `"dark"`, or `"system"`. `theme`
    /// stores the currently resolved concrete bundled theme for compatibility.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme_mode: Option<String>,
    /// Workspace command definitions (cmux-compatible format).
    #[serde(default, deserialize_with = "lenient_commands")]
    pub commands: Vec<CommandDefinition>,
    /// Window decoration mode: `"client"` (CSD, default) or `"server"` (SSD).
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_decorations: Option<String>,
    /// Native window backdrop: `"auto"` (default), `"mica"`, `"blurred"` /
    /// `"acrylic"`, `"transparent"`, or `"opaque"` / `"off"`. Read at
    /// startup; `PANEFLOW_WINDOW_BACKDROP` overrides it for one launch.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_backdrop: Option<String>,
    /// Windows-only: when enabled, the CLI terminal's default background cells
    /// are transparent so the active native backdrop can show through.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_terminal_material: Option<bool>,
    /// Windows-only: when enabled, the primary sidebar card reveals the active
    /// native backdrop.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_chrome_material: Option<bool>,
    /// macOS-only: when enabled (default), the primary sidebar card reveals
    /// AppKit's native Sidebar material. Other platforms ignore this field.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub macos_chrome_material: Option<bool>,
    /// Opacity of panes that do not hold focus, when the workspace holds more
    /// than one pane (default: 0.7, valid range: 0.15-1.0). `1.0` disables the
    /// effect. Rendered as a single compositing layer over the pane content, so
    /// it never touches the terminal renderer.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub unfocused_pane_opacity: Option<f32>,
    /// Minimize non-essential interface motion: hover transitions settle
    /// instantly and GPUI's decorative animations (spinners, animated images)
    /// render a static frame. `None`/`false` keeps the full motion. Applied
    /// through `App::set_reduce_motion`, so it hot-reloads.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub reduce_motion: Option<bool>,
    /// Terminal line height multiplier (default: 1.2, valid range: 1.0-2.5).
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub line_height: Option<f32>,
    /// Terminal cell width multiplier (default: 0.6, valid range: 0.3-2.0).
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub cell_width: Option<f32>,
    /// Terminal font family (default: bundled JetBrainsMono Nerd Font Mono).
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_family: Option<String>,
    /// Ordered fallback font families, consulted in order for glyphs the
    /// primary `font_family` does not cover - e.g. a Nerd Font for the
    /// Powerline / icon glyphs used by Starship, oh-my-posh or Terminal-Icons,
    /// which no system font provides on Windows. `None` (or an empty list)
    /// keeps GPUI's built-in fallback stack only. Mirrors Zed's
    /// `terminal.font_fallbacks`. Hot-reloaded via the 500 ms font cache, so a
    /// config edit takes effect on the next new terminal without a restart.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_fallbacks: Option<Vec<String>>,
    /// Terminal font size in points (default: 13.0, valid range: 8.0-32.0).
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_size: Option<f32>,
    /// Terminal font weight (default: "normal").
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_weight: Option<String>,
    /// Treat Alt key as Meta (send ESC prefix). Default: true on Linux.
    /// Set to false for future macOS where Option produces Unicode characters.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub option_as_meta: Option<bool>,
    /// EP-003 US-007 (cli-cockpit): master switch for the per-shell rc
    /// injection (OSC 7 CWD reporting + OSC 133 command marks). `None`/`true`
    /// = enabled (the long-standing default behavior); `false` = no snippet
    /// is written or wired - the shell starts exactly as it would outside
    /// Paneflow.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shell_integration: Option<bool>,
    /// EP-004 US-011 (cli-cockpit): master switch for Stalled detection.
    /// `None`/`true` = enabled (default ON): a `Thinking` agent session with
    /// no hook activity past the silence threshold is flagged `Stalled` and
    /// notified ONCE per stall episode (the flag clears on the next hook
    /// event, so a legitimately long turn costs at most one notification).
    /// `false` = kill switch - no `Stalled` state is ever produced.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_stall_detection: Option<bool>,
    /// EP-004 US-011: silence threshold in seconds before a `Thinking`
    /// session is flagged `Stalled`. `None` resolves to 60 s; values are
    /// clamped to `[30, 86400]`. Checked by the 30 s sweep, so the
    /// effective detection latency is threshold + up to 30 s.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_stall_threshold_secs: Option<u64>,
    /// EP-003 US-011 (review redesign): delay in milliseconds
    /// before the Review view pre-fills a freshly-launched review CLI's input
    /// (tmux send-keys style). `None` resolves to 2000 ms; values are clamped to
    /// `[250, 10000]`.
    ///
    /// The fixed delay exists because there is no reliable cross-platform
    /// "readline is ready" signal: firing too early (on the shell's echo of the
    /// launch command, before the CLI's prompt exists) sends the prefill into a
    /// not-ready buffer and LOSES it - a regression impossible to verify on
    /// Windows ConPTY cold-start from here. The prompt is therefore ALWAYS copied
    /// to the clipboard as a synchronous safety net (surfaced in the review
    /// terminal header), so a missed window degrades to a one-keystroke paste
    /// rather than silent failure. This setting lets a user on a slow cold-start
    /// raise the delay instead of fighting the race.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub review_prefill_delay_ms: Option<u64>,
    /// EP-001 US-001 (agent-control-plane-hardening): base delay in
    /// milliseconds between writing a bracketed-paste burst to an agent and
    /// the SEPARATE carriage-return that submits it. The split exists because
    /// a TUI agent (Claude Code, Codex) treats a burst as an unconfirmed paste
    /// (`[Pasted text #1]`) and swallows a `\r` that rides the same burst, so
    /// `submit:true` silently fails. After this floor the server waits for the
    /// agent's paste echo (an `output_generation` bump) before sending the
    /// `\r`, capped so it never loops; this knob sets the floor only. `None`
    /// resolves to 70 ms (mid the empirically safe 60-80 ms band); values are
    /// clamped to `[10, 5000]`. Scheduled off the GPUI render thread, so a
    /// larger value never blocks the UI.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub submit_paste_delay_ms: Option<u64>,
    /// External editor used to open markdown links (file paths shipped
    /// by the agent as `[foo](src/foo.rs)` or `[foo](src/foo.rs:42)`).
    ///
    /// Accepted values:
    /// - `"auto"` (default when absent): detect the first CLI present
    ///   on PATH from the preferred order `zed`, `cursor`, `windsurf`,
    ///   `code`. Falls back to the system opener (xdg-open / open /
    ///   start) when none are installed.
    /// - `"system"`: always defer to the OS-level opener.
    /// - `"zed"` | `"cursor"` | `"windsurf"` | `"code"`: force the
    ///   named CLI even if other editors are also installed.
    ///
    /// The chosen CLI is spawned with `<editor> <abs_path>[:line[:col]]`;
    /// all four support that suffix natively to jump to the target
    /// position.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub external_editor: Option<String>,
    /// When `Some(true)`, the Claude Code terminal launcher adds
    /// `--permission-mode bypassPermissions` to the spawned CLI in the tab bar,
    /// Agents view, Launch Pad, and session resume paths.
    ///
    /// `Some(false)` or `None` (the default) keeps the per-tool confirmation
    /// prompts enabled.
    /// Per Anthropic's docs bypass mode offers no protection against
    /// prompt injection - opt out (toggle off in Settings -> AI Agent)
    /// if you want explicit confirmation for every tool call. The key
    /// retains its `claude_code_` prefix for backwards compatibility
    /// with existing user configs.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub claude_code_bypass_permissions: Option<bool>,
    /// EP-003 US-008 (agent-control-plane): "AI free access" master switch.
    /// `Some(true)` debrays the *bridling* guardrails so a conductor (a CLI
    /// agent or external orchestrator) can drive its peers without friction:
    /// `surface.send_text submit:true` is authorized without the
    /// `PANEFLOW_IPC_SCRIPTING` env gate, and every such write is traced.
    /// `Some(false)` / `None` (the default) keeps the current behavior
    /// strictly unchanged (prefill-not-submitted + env-gated writes).
    /// Re-evaluated per IPC call, so the mode takes effect (or is revoked)
    /// hot with no residual capability. A non-boolean value resolves to
    /// `None` (false) with a warn, never an accidentally-open state.
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_unrestricted: Option<bool>,
    /// EP-003 US-008/US-011 (agent-control-plane): anti-injection fence on
    /// the `surface.read` CLI/IPC path, INDEPENDENT of `ai_unrestricted`.
    /// `Some(true)` / `None` (the default) wraps returned terminal text in
    /// the `<untrusted_terminal_output id="…">` marker (parity with the MCP
    /// bridge) so a malicious peer pane cannot hijack a conductor reading it.
    /// `Some(false)` returns raw text (historical behavior), a risk the user
    /// assumes. The fence PROTECTS the AI from being redirected; it does not
    /// bridle it, so it stays ON by default even in free-access mode. A
    /// non-boolean value resolves to `None` (fence ON) with a warn.
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_injection_fence: Option<bool>,
    /// Show the built-in "Claude Code" command button in the tab bar.
    /// `Some(true)` always renders the button, `Some(false)` hides it, and
    /// `None` (default) renders it only when the CLI binary is installed.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub claude_code_button_visible: Option<bool>,
    /// Show the built-in "Codex" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub codex_button_visible: Option<bool>,
    /// Show the built-in "Opencode" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub opencode_button_visible: Option<bool>,
    /// Show the built-in "Pi" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub pi_button_visible: Option<bool>,
    /// Show the built-in "Hermes Agent" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub hermes_agent_button_visible: Option<bool>,
    /// Show the built-in "Grok" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub grok_button_visible: Option<bool>,
    /// Show the built-in "Amp" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub amp_button_visible: Option<bool>,
    /// Show the built-in "Cursor" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub cursor_button_visible: Option<bool>,
    /// Show the built-in "Gemini" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub gemini_button_visible: Option<bool>,
    /// Show the built-in "Kiro" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub kiro_button_visible: Option<bool>,
    /// Show the built-in "Antigravity" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub antigravity_button_visible: Option<bool>,
    /// Show the built-in "Copilot" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub copilot_button_visible: Option<bool>,
    /// Show the built-in "CodeBuddy" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub codebuddy_button_visible: Option<bool>,
    /// Show the built-in "Factory" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub factory_button_visible: Option<bool>,
    /// Show the built-in "Qoder" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub qoder_button_visible: Option<bool>,
    /// Show the built-in "Openclaw" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub openclaw_button_visible: Option<bool>,
    /// Opt-in desktop telemetry block.
    ///
    /// Tri-state semantics:
    /// - `None` (block missing from config): user has never been prompted.
    /// - `Some(TelemetryConfig { enabled: None })`: block exists but the
    ///   consent question is still unanswered (e.g. user dismissed the
    ///   first-run modal without choosing).
    /// - `Some(TelemetryConfig { enabled: Some(true|false) })`: explicit
    ///   user answer - consent granted or refused.
    ///
    /// The consent modal (US-011) only appears when `telemetry.enabled`
    /// resolves to `None` under both the outer and inner Option layers.
    /// No event is ever sent unless `enabled == Some(true)`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub telemetry: Option<TelemetryConfig>,
    /// Terminal-scoped settings block for renderer and PTY behavior.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub terminal: Option<TerminalConfig>,
    /// Agents-view-scoped settings block (US-103 + future Phase B-E
    /// stories of `tasks/prd-agent-ui-refactor-2026-Q3.md`). Lives in
    /// its own struct so the dozen-or-so fields the refactor introduces
    /// stay namespaced under `"agent_panel": { ... }`.
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_panel: Option<AgentPanelConfig>,
    /// Per-tool permission patterns (US-111 of
    /// `tasks/prd-agent-ui-refactor-2026-Q3.md`). The key is the
    /// `ToolKind` discriminant (e.g. `"read"`, `"edit"`, `"execute"`)
    /// -- matching Zed §13's `ToolPermissions` shape. An entry's
    /// `always_allow` patterns auto-resolve future
    /// `WaitingForConfirmation` callbacks; `always_deny` patterns
    /// auto-reject them. A bare entry with no patterns matches every
    /// call of that tool kind, which is what the "Allow Always for
    /// this tool" UI writes today.
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "lenient_value_or_default"
    )]
    pub tool_permissions: HashMap<String, ToolPermissionsEntry>,
}

impl PaneFlowConfig {
    /// EP-004 US-011 (cli-cockpit) + US-013 (agent-control-plane): default
    /// Stalled silence threshold. Tightened from 300 s to 60 s so a likely-lost
    /// `ai.stop` surfaces in seconds, not minutes (a wedged Thinking agent was
    /// ~330 s = 300 s + the 30 s sweep before this). 60 s still tolerates a
    /// normal tool-free reasoning stretch; the flip is non-sticky, so a long
    /// legitimate think that resumes activity clears itself at the next hook.
    pub const DEFAULT_AGENT_STALL_THRESHOLD_SECS: u64 = 60;
    /// Lower bound: below the 30 s sweep cadence the threshold cannot be
    /// honored and every long tool call would false-positive.
    pub const MIN_AGENT_STALL_THRESHOLD_SECS: u64 = 30;
    /// Upper bound: a day - past this the feature is effectively off, so
    /// use [`PaneFlowConfig::agent_stall_detection`] instead.
    pub const MAX_AGENT_STALL_THRESHOLD_SECS: u64 = 86_400;

    /// EP-003 US-011: default review-prefill delay. 2000 ms is a slightly safer
    /// floor than the historical 1800 ms - enough headroom for `claude` /
    /// `codex` / `opencode` / `pi` to boot their readline on a warm start, while
    /// the clipboard fallback covers any cold-start miss.
    pub const DEFAULT_REVIEW_PREFILL_DELAY_MS: u64 = 2000;
    /// Lower bound: below this the prefill almost certainly races the CLI's own
    /// boot echo and lands in a not-ready buffer.
    pub const MIN_REVIEW_PREFILL_DELAY_MS: u64 = 250;
    /// Upper bound: past this the wait is more annoying than the race it avoids;
    /// the clipboard fallback already covers the long tail.
    pub const MAX_REVIEW_PREFILL_DELAY_MS: u64 = 10_000;

    /// Default opacity of an unfocused pane. Matches Ghostty's
    /// `unfocused-split-opacity` default: enough contrast to read focus at a
    /// glance without making background agent output unreadable.
    pub const DEFAULT_UNFOCUSED_PANE_OPACITY: f32 = 0.7;
    /// Lower bound: below this the unfocused pane is effectively blanked and
    /// streaming agent output can no longer be monitored out of the corner of
    /// the eye.
    pub const MIN_UNFOCUSED_PANE_OPACITY: f32 = 0.15;
    /// Upper bound and off switch: `1.0` paints no dim layer at all.
    pub const MAX_UNFOCUSED_PANE_OPACITY: f32 = 1.0;

    /// EP-001 US-001 (agent-control-plane-hardening): default paste->submit
    /// floor. 70 ms sits in the middle of the 60-80 ms band that reliably lets
    /// Claude Code / Codex finish buffering a bracketed paste before the `\r`.
    pub const DEFAULT_SUBMIT_PASTE_DELAY_MS: u64 = 70;
    /// Lower bound: a few ms still flush the paste write, but below ~10 ms the
    /// `\r` can outrun the agent's paste-buffer commit on a warm path.
    pub const MIN_SUBMIT_PASTE_DELAY_MS: u64 = 10;
    /// Upper bound: past this the dispatch feels laggy; the echo-confirm path
    /// already adapts to a genuinely slow agent without a huge fixed floor.
    pub const MAX_SUBMIT_PASTE_DELAY_MS: u64 = 5_000;

    /// Resolve the Stalled-detection master switch (default ON).
    pub fn agent_stall_detection_enabled(&self) -> bool {
        self.agent_stall_detection.unwrap_or(true)
    }

    /// Resolve the Windows terminal material switch. Other platforms always
    /// stay opaque even if the field exists in a shared config file.
    pub fn windows_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "windows") && self.windows_terminal_material.unwrap_or(false)
    }

    fn window_backdrop_disables_chrome_material(&self) -> bool {
        self.window_backdrop.as_deref().is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("opaque") || value.eq_ignore_ascii_case("off")
        })
    }

    /// Resolve the macOS Sidebar material switch. Missing values default ON;
    /// opaque and raw-transparent backdrops remain master off switches.
    pub fn macos_chrome_material_enabled(&self) -> bool {
        !self.window_backdrop_disables_chrome_material()
            && !self
                .window_backdrop
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("transparent"))
            && self.macos_chrome_material.unwrap_or(true)
    }

    /// Resolve the desktop chrome material switch for the current platform.
    /// Linux keeps its existing platform policy and has no settings toggle.
    pub fn cockpit_chrome_material_enabled(&self) -> bool {
        if self.window_backdrop_disables_chrome_material() {
            return false;
        }

        if cfg!(target_os = "windows") {
            self.windows_chrome_material.unwrap_or(false)
        } else if cfg!(target_os = "macos") {
            self.macos_chrome_material_enabled()
        } else {
            true
        }
    }

    /// Resolve the reduce-motion switch. Absent means full motion.
    pub fn reduce_motion_enabled(&self) -> bool {
        self.reduce_motion.unwrap_or(false)
    }

    /// Resolve `agent_stall_threshold_secs`: default 60, clamped to
    /// `[30, 86400]` with a `warn!` so an out-of-range value is noticed.
    pub fn resolved_agent_stall_threshold_secs(&self) -> u64 {
        let raw = self
            .agent_stall_threshold_secs
            .unwrap_or(Self::DEFAULT_AGENT_STALL_THRESHOLD_SECS);
        let clamped = raw.clamp(
            Self::MIN_AGENT_STALL_THRESHOLD_SECS,
            Self::MAX_AGENT_STALL_THRESHOLD_SECS,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::agent",
                requested = raw,
                clamped,
                "agent_stall_threshold_secs out of range [{min}, {max}], clamped",
                min = Self::MIN_AGENT_STALL_THRESHOLD_SECS,
                max = Self::MAX_AGENT_STALL_THRESHOLD_SECS,
            );
        }
        clamped
    }

    /// EP-003 US-011: resolve `review_prefill_delay_ms`: default 2000, clamped to
    /// `[250, 10000]` with a `warn!` so an out-of-range value is noticed.
    pub fn resolved_review_prefill_delay_ms(&self) -> u64 {
        let raw = self
            .review_prefill_delay_ms
            .unwrap_or(Self::DEFAULT_REVIEW_PREFILL_DELAY_MS);
        let clamped = raw.clamp(
            Self::MIN_REVIEW_PREFILL_DELAY_MS,
            Self::MAX_REVIEW_PREFILL_DELAY_MS,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::review",
                requested = raw,
                clamped,
                "review_prefill_delay_ms out of range [{min}, {max}], clamped",
                min = Self::MIN_REVIEW_PREFILL_DELAY_MS,
                max = Self::MAX_REVIEW_PREFILL_DELAY_MS,
            );
        }
        clamped
    }

    /// EP-001 US-001 (agent-control-plane-hardening): resolve
    /// `submit_paste_delay_ms`: default 70, clamped to `[10, 5000]` with a
    /// `warn!` so an out-of-range value is noticed.
    pub fn resolved_submit_paste_delay_ms(&self) -> u64 {
        let raw = self
            .submit_paste_delay_ms
            .unwrap_or(Self::DEFAULT_SUBMIT_PASTE_DELAY_MS);
        let clamped = raw.clamp(
            Self::MIN_SUBMIT_PASTE_DELAY_MS,
            Self::MAX_SUBMIT_PASTE_DELAY_MS,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::submit",
                requested = raw,
                clamped,
                "submit_paste_delay_ms out of range [{min}, {max}], clamped",
                min = Self::MIN_SUBMIT_PASTE_DELAY_MS,
                max = Self::MAX_SUBMIT_PASTE_DELAY_MS,
            );
        }
        clamped
    }

    /// Resolve the alpha of the dim layer painted over unfocused panes.
    ///
    /// This is the single point where the configured *opacity* is inverted into
    /// a *fill alpha*, so no caller can get the direction wrong: `0.7` opacity
    /// yields a `0.3` overlay, and `1.0` yields `0.0` (no layer). Non-finite
    /// values fall back to the default; out-of-range values are clamped with a
    /// `warn!`.
    pub fn resolved_unfocused_pane_dim_alpha(&self) -> f32 {
        let raw = self
            .unfocused_pane_opacity
            .filter(|value| value.is_finite())
            .unwrap_or(Self::DEFAULT_UNFOCUSED_PANE_OPACITY);
        let clamped = raw.clamp(
            Self::MIN_UNFOCUSED_PANE_OPACITY,
            Self::MAX_UNFOCUSED_PANE_OPACITY,
        );
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::appearance",
                requested = raw,
                clamped,
                "unfocused_pane_opacity out of range [{min}, {max}], clamped",
                min = Self::MIN_UNFOCUSED_PANE_OPACITY,
                max = Self::MAX_UNFOCUSED_PANE_OPACITY,
            );
        }
        1.0 - clamped
    }

    /// EP-003 US-008 (agent-control-plane): resolve the AI free-access master
    /// switch. Default OFF (`false`) so a fresh config never opens the mode.
    pub fn ai_unrestricted_enabled(&self) -> bool {
        self.ai_unrestricted.unwrap_or(false)
    }

    /// EP-003 US-008/US-011 (agent-control-plane): resolve the anti-injection
    /// fence. Default ON (`true`): a missing or malformed value fails closed
    /// to fenced, even when free-access mode is on (the fence protects the
    /// conductor, it does not bridle it).
    pub fn ai_injection_fence_enabled(&self) -> bool {
        self.ai_injection_fence.unwrap_or(true)
    }
}

/// Lenient `Option<bool>` deserializer for optional config toggles. A
/// non-boolean value (e.g. the string `"true"`)
/// deserializes to `None` with a `warn!` instead of hard-erroring, which would
/// propagate to `parse_and_validate` and wipe EVERY sibling setting on a single
/// typo (the all-or-nothing fallback the terminal enums avoid for the same
/// reason). `None` then resolves through each field's resolver.
pub(super) fn lenient_opt_bool<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "boolean config toggle")
}

pub(super) fn lenient_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "string config value")
}

pub(super) fn lenient_opt_usize<'de, D>(d: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "positive integer config value")
}

pub(super) fn lenient_opt_f32<'de, D>(d: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "number config value")
}

pub(super) fn lenient_opt_cursor_shape<'de, D>(d: D) -> Result<Option<CursorShapeConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor shape")
}

pub(super) fn lenient_opt_cursor_blink<'de, D>(d: D) -> Result<Option<CursorBlinkConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor blink mode")
}

pub(super) fn lenient_opt_string_map<'de, D>(
    d: D,
) -> Result<Option<HashMap<String, String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "string map config value")
}

fn lenient_opt_value<'de, D, T>(d: D, expected: &'static str) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned,
{
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(match v {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => match serde_json::from_value::<T>(value.clone()) {
            Ok(parsed) => Some(parsed),
            Err(_) => {
                tracing::warn!(
                    target: "paneflow_config",
                    value = %value,
                    expected,
                    "config value has an unexpected type, ignoring value and using resolver default",
                );
                None
            }
        },
    })
}

/// Deserialize one top-level config field independently. A malformed field is
/// ignored without discarding valid siblings, so the derived `PaneFlowConfig`
/// deserializer remains the single source of truth for the public schema.
fn lenient_value_or_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(match serde_json::from_value::<T>(value.clone()) {
        Ok(parsed) => parsed,
        Err(error) => {
            tracing::warn!(
                target: "paneflow_config",
                value = %value,
                %error,
                "ignoring malformed config field and using its default",
            );
            T::default()
        }
    })
}

/// Commands are lenient per entry rather than per vector: one malformed
/// command must not discard its valid siblings.
fn lenient_commands<'de, D>(d: D) -> Result<Vec<CommandDefinition>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(d)?;
    let Some(items) = value.as_array() else {
        tracing::warn!("ignoring config field `commands`: expected an array");
        return Ok(Vec::new());
    };

    Ok(items
        .iter()
        .enumerate()
        .filter_map(
            |(index, raw)| match serde_json::from_value::<CommandDefinition>(raw.clone()) {
                Ok(command) => Some(command),
                Err(error) => {
                    tracing::warn!("skipping invalid command entry at index {index}: {error}");
                    None
                }
            },
        )
        .collect())
}

/// Per-tool permission patterns persisted under `"tool_permissions"`
/// in `paneflow.json` (US-111). Patterns are matched as substrings
/// against the tool call's raw input pretty-printed JSON; an empty
/// `always_allow` list with an existing entry counts as "always
/// allow every call of this tool" (the v1 UI does not yet expose
/// pattern-scoped persistence and uses this shape).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToolPermissionsEntry {
    /// Substring patterns whose presence in the tool input auto-
    /// resolves `Allow`. An empty vec means "always allow".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_allow: Vec<String>,
    /// Substring patterns whose presence auto-resolves `Reject`.
    /// Auto-promotion from `always_allow` to `always_deny` happens
    /// at the UI layer when the user explicitly rejects a call that
    /// previously matched -- treated as a correction signal per Zed
    /// §13 / PRD US-111 AC #8.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_deny: Vec<String>,
}
