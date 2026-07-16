// US-017: Config schema types

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;

/// Current on-disk schema version for [`SessionState`].
pub const SESSION_SCHEMA_VERSION: u32 = 1;

/// Apple system blue, used by built-in themes as the default terminal cursor.
pub const APPLE_SYSTEM_BLUE_HEX: &str = "#007AFF";

/// Normalize a user-provided RGB hex color to `#RRGGBB`.
pub fn normalize_hex_color(raw: &str) -> Option<String> {
    let hex = raw.trim().strip_prefix('#').unwrap_or(raw.trim());
    let expanded = match hex.len() {
        3 => {
            let mut out = String::with_capacity(6);
            for ch in hex.chars() {
                out.push(ch);
                out.push(ch);
            }
            out
        }
        6 => hex.to_string(),
        _ => return None,
    };
    if expanded.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(format!("#{}", expanded.to_ascii_uppercase()))
    } else {
        None
    }
}

/// Top-level PaneFlow configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneFlowConfig {
    /// Key-action shortcut mappings (e.g. "ctrl+t" -> "new_tab").
    pub shortcuts: HashMap<String, String>,
    /// Default shell binary path. `None` uses the system default.
    pub default_shell: Option<String>,
    /// Terminal color theme name (e.g. "One Dark", "PaneFlow Light", "Vercel", "Claude", "Cursor").
    pub theme: Option<String>,
    /// Theme selection mode: `"light"`, `"dark"`, or `"system"`. `theme`
    /// stores the currently resolved concrete bundled theme for compatibility.
    pub theme_mode: Option<String>,
    /// Workspace command definitions (cmux-compatible format).
    pub commands: Vec<CommandDefinition>,
    /// Window decoration mode: `"client"` (CSD, default) or `"server"` (SSD).
    pub window_decorations: Option<String>,
    /// Native window backdrop: `"auto"` (default), `"mica"`, `"blurred"` /
    /// `"acrylic"`, `"transparent"`, or `"opaque"` / `"off"`. Read at
    /// startup; `PANEFLOW_WINDOW_BACKDROP` overrides it for one launch.
    pub window_backdrop: Option<String>,
    /// Windows-only: when enabled, the CLI terminal's default background cells
    /// are transparent so the active native backdrop can show through.
    pub windows_terminal_material: Option<bool>,
    /// Windows-only: when enabled, sidebars/title bar use transparent chrome so
    /// the active native backdrop can show through around the terminal.
    pub windows_chrome_material: Option<bool>,
    /// Terminal line height multiplier (default: 1.2, valid range: 1.0-2.5).
    pub line_height: Option<f32>,
    /// Terminal cell width multiplier (default: 0.6, valid range: 0.3-2.0).
    pub cell_width: Option<f32>,
    /// Terminal font family (default: bundled JetBrainsMono Nerd Font Mono).
    pub font_family: Option<String>,
    /// Ordered fallback font families, consulted in order for glyphs the
    /// primary `font_family` does not cover - e.g. a Nerd Font for the
    /// Powerline / icon glyphs used by Starship, oh-my-posh or Terminal-Icons,
    /// which no system font provides on Windows. `None` (or an empty list)
    /// keeps GPUI's built-in fallback stack only. Mirrors Zed's
    /// `terminal.font_fallbacks`. Hot-reloaded via the 500 ms font cache, so a
    /// config edit takes effect on the next new terminal without a restart.
    pub font_fallbacks: Option<Vec<String>>,
    /// Terminal font size in points (default: 13.0, valid range: 8.0-32.0).
    pub font_size: Option<f32>,
    /// Terminal font weight (default: "normal").
    pub font_weight: Option<String>,
    /// Treat Alt key as Meta (send ESC prefix). Default: true on Linux.
    /// Set to false for future macOS where Option produces Unicode characters.
    pub option_as_meta: Option<bool>,
    /// EP-003 US-007 (cli-cockpit): master switch for the per-shell rc
    /// injection (OSC 7 CWD reporting + OSC 133 command marks). `None`/`true`
    /// = enabled (the long-standing default behavior); `false` = no snippet
    /// is written or wired - the shell starts exactly as it would outside
    /// Paneflow.
    pub shell_integration: Option<bool>,
    /// EP-004 US-011 (cli-cockpit): master switch for Stalled detection.
    /// `None`/`true` = enabled (default ON): a `Thinking` agent session with
    /// no hook activity past the silence threshold is flagged `Stalled` and
    /// notified ONCE per stall episode (the flag clears on the next hook
    /// event, so a legitimately long turn costs at most one notification).
    /// `false` = kill switch - no `Stalled` state is ever produced.
    pub agent_stall_detection: Option<bool>,
    /// EP-004 US-011: silence threshold in seconds before a `Thinking`
    /// session is flagged `Stalled`. `None` resolves to 60 s; values are
    /// clamped to `[30, 86400]`. Checked by the 30 s sweep, so the
    /// effective detection latency is threshold + up to 30 s.
    pub agent_stall_threshold_secs: Option<u64>,
    /// EP-003 US-011 (prd-review-redesign-2026-Q3.md): delay in milliseconds
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
    pub claude_code_button_visible: Option<bool>,
    /// Show the built-in "Codex" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub codex_button_visible: Option<bool>,
    /// Show the built-in "Opencode" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub opencode_button_visible: Option<bool>,
    /// Show the built-in "Pi" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub pi_button_visible: Option<bool>,
    /// Show the built-in "Hermes Agent" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub hermes_agent_button_visible: Option<bool>,
    /// Show the built-in "Grok" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub grok_button_visible: Option<bool>,
    /// Show the built-in "Amp" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub amp_button_visible: Option<bool>,
    /// Show the built-in "Cursor" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub cursor_button_visible: Option<bool>,
    /// Show the built-in "Gemini" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub gemini_button_visible: Option<bool>,
    /// Show the built-in "Kiro" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub kiro_button_visible: Option<bool>,
    /// Show the built-in "Antigravity" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub antigravity_button_visible: Option<bool>,
    /// Show the built-in "Copilot" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub copilot_button_visible: Option<bool>,
    /// Show the built-in "CodeBuddy" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub codebuddy_button_visible: Option<bool>,
    /// Show the built-in "Factory" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub factory_button_visible: Option<bool>,
    /// Show the built-in "Qoder" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
    pub qoder_button_visible: Option<bool>,
    /// Show the built-in "Openclaw" command button in the tab bar.
    /// Same semantics as `claude_code_button_visible`.
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
    pub telemetry: Option<TelemetryConfig>,
    /// Terminal-scoped settings block for renderer and PTY behavior.
    pub terminal: Option<TerminalConfig>,
    /// Agents-view-scoped settings block (US-103 + future Phase B-E
    /// stories of `tasks/prd-agent-ui-refactor-2026-Q3.md`). Lives in
    /// its own struct so the dozen-or-so fields the refactor introduces
    /// stay namespaced under `"agent_panel": { ... }`.
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
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
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

    /// Resolve the desktop chrome material switch. `window_backdrop = "opaque"`
    /// or `"off"` disables transparent chrome on every platform.
    pub fn cockpit_chrome_material_enabled(&self) -> bool {
        if self.window_backdrop.as_deref().is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("opaque") || value.eq_ignore_ascii_case("off")
        }) {
            return false;
        }
        !cfg!(target_os = "windows") || self.windows_chrome_material.unwrap_or(false)
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
fn lenient_opt_bool<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "boolean config toggle")
}

fn lenient_opt_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "string config value")
}

fn lenient_opt_usize<'de, D>(d: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "positive integer config value")
}

fn lenient_opt_f32<'de, D>(d: D) -> Result<Option<f32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "number config value")
}

fn lenient_opt_cursor_shape<'de, D>(d: D) -> Result<Option<CursorShapeConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor shape")
}

fn lenient_opt_cursor_blink<'de, D>(d: D) -> Result<Option<CursorBlinkConfig>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    lenient_opt_value(d, "terminal cursor blink mode")
}

fn lenient_opt_string_map<'de, D>(d: D) -> Result<Option<HashMap<String, String>>, D::Error>
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

/// US-007: configurable default cursor shape, applied as the fallback before
/// any app-driven DECSCUSR escape. Mapped to the renderer's cursor shapes in
/// the app layer (this crate stays free of the terminal backend).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShapeConfig {
    /// Vintage console cursor: a thicker bottom block.
    Vintage,
    /// Solid block `█` (historical default).
    #[default]
    Block,
    /// Vertical bar `⎸`.
    Beam,
    /// Underline `_`.
    Underline,
    /// Double underline `‿`.
    DoubleUnderline,
    /// Hollow box `▯`.
    Hollow,
}

/// US-008: cursor blink override. `TerminalControlled` (default) defers to the
/// program's DECSCUSR cursor-style setting; `On`/`Off` force the behavior.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorBlinkConfig {
    /// Force the cursor to blink regardless of what the program requests.
    On,
    /// Force the cursor solid regardless of what the program requests.
    Off,
    /// Defer to the program's DECSCUSR setting (historical default).
    #[default]
    TerminalControlled,
}

/// Terminal engine requested for newly-created sessions. `Auto` selects
/// Ghostty in standard Linux builds and Alacritty on macOS, Windows, or Linux
/// builds using `--no-default-features`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalBackendConfig {
    #[default]
    Auto,
    Ghostty,
    Alacritty,
}

// Manual `Deserialize` for the terminal enums. A derived `Deserialize` hard-
// errors on an unrecognised variant; that error propagates up to
// `parse_and_validate` (loader.rs), which discards the ENTIRE user config and
// returns defaults. A typo (`"cursor_shape": "squiggle"`) would silently wipe
// the theme, shell, shortcuts, and agent settings. Instead fall back to the
// variant default with a logged warning, mirroring `ThinkingDisplayMode`.
// `Serialize` stays derived (snake_case), so round-tripping a valid value is
// unchanged.
impl<'de> Deserialize<'de> for CursorShapeConfig {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        Ok(match raw.as_str() {
            "vintage" => Self::Vintage,
            "block" => Self::Block,
            "filled_box" | "filledBox" => Self::Block,
            "beam" | "bar" => Self::Beam,
            "underline" | "underscore" => Self::Underline,
            "double_underline" | "double_underscore" | "doubleUnderline" | "doubleUnderscore" => {
                Self::DoubleUnderline
            }
            "hollow" | "empty_box" | "emptyBox" => Self::Hollow,
            other => {
                tracing::warn!(
                    target: "paneflow_config::terminal",
                    value = other,
                    "terminal.cursor_shape value not recognized, defaulting to block",
                );
                Self::Block
            }
        })
    }
}

impl<'de> Deserialize<'de> for CursorBlinkConfig {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        Ok(match raw.as_str() {
            "on" => Self::On,
            "off" => Self::Off,
            "terminal_controlled" => Self::TerminalControlled,
            other => {
                tracing::warn!(
                    target: "paneflow_config::terminal",
                    value = other,
                    "terminal.cursor_blink value not recognized, defaulting to terminal_controlled",
                );
                Self::TerminalControlled
            }
        })
    }
}

impl<'de> Deserialize<'de> for TerminalBackendConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.as_str() {
            "auto" => Self::Auto,
            "ghostty" => Self::Ghostty,
            "alacritty" => Self::Alacritty,
            other => {
                tracing::warn!(
                    target: "paneflow_config::terminal",
                    value = other,
                    "terminal.backend value not recognized, defaulting to auto",
                );
                Self::Auto
            }
        })
    }
}

/// Memory budget profile for a terminal surface.
///
/// Normal and Agent terminals keep the standard interactive scrollback default so
/// long-lived CLI transcripts retain commands, diffs and tool output. Review
/// and Cached remain reserved for fresh cold surfaces; live cached PTYs are not
/// rebuilt just to shrink history because dropping them would kill processes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TerminalSurfaceProfile {
    #[default]
    Normal,
    Agent,
    Review,
    Cached,
}

impl TerminalSurfaceProfile {
    fn scrollback_cap(self) -> Option<usize> {
        match self {
            Self::Normal => None,
            Self::Agent => Some(TerminalConfig::AGENT_SCROLLBACK_LINES),
            Self::Review => Some(TerminalConfig::REVIEW_SCROLLBACK_LINES),
            Self::Cached => Some(TerminalConfig::CACHED_SCROLLBACK_LINES),
        }
    }
}

/// Terminal-scoped configuration block (US-008).
///
/// Lives in its own struct so future renderer settings (cursor shape,
/// blink interval, alternate scroll, …) can be added without expanding
/// the top-level `PaneFlowConfig` further.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    /// Backend requested for new sessions. `auto` resolves to Ghostty in
    /// standard Linux builds and to Alacritty on macOS, Windows, or Linux
    /// builds using `--no-default-features`.
    /// `alacritty` is the explicit Linux rollback.
    #[serde(default, deserialize_with = "lenient_terminal_backend")]
    pub backend: TerminalBackendConfig,
    /// Render programming-font ligatures (FiraCode `=>`, `!=`, …) when
    /// `Some(true)`. `None` and `Some(false)` both keep the historical
    /// behavior of disabling ligatures via GPUI's `FontFeatures`.
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ligatures: Option<bool>,
    /// Draw built-in block-element glyphs as filled quads instead of using the
    /// font glyph. `None` resolves to enabled, matching Paneflow's historical
    /// renderer behavior.
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub integrated_glyphs: Option<bool>,
    /// Render emoji with the platform color-emoji path. `None` resolves to
    /// enabled, matching Windows Terminal and GPUI's default behavior.
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub color_emoji: Option<bool>,
    /// Override the terminal cursor color with a `#RRGGBB` value. `None` keeps
    /// the active color scheme cursor color.
    #[serde(default, deserialize_with = "lenient_opt_string")]
    pub cursor_color: Option<String>,
    /// Maximum scrollback history in lines (`max_scroll_history_lines`).
    /// `None` resolves to
    /// [`TerminalConfig::DEFAULT_SCROLLBACK_LINES`]; values are clamped
    /// to `[100, 100_000]`. Alacritty exposes a line-count limit rather
    /// than Ghostty's byte-count `scrollback-limit`, so the default stays
    /// conservative while advanced users can opt into a larger line budget.
    /// Read once at PTY spawn time; changing this value takes effect on
    /// the next new terminal.
    #[serde(default, deserialize_with = "lenient_opt_usize")]
    pub scrollback_lines: Option<usize>,
    /// US-007: default cursor shape before any app-driven DECSCUSR escape.
    /// `None` resolves to `Block`. Read once at terminal construction.
    #[serde(default, deserialize_with = "lenient_opt_cursor_shape")]
    pub cursor_shape: Option<CursorShapeConfig>,
    /// US-008: cursor blink override. `None` resolves to `TerminalControlled`
    /// (defer to DECSCUSR). Read once at terminal construction.
    #[serde(default, deserialize_with = "lenient_opt_cursor_blink")]
    pub cursor_blink: Option<CursorBlinkConfig>,
    /// US-014: global default extra environment variables injected into every
    /// new terminal PTY. Per-surface `env` ([`SurfaceDefinition::env`]) is
    /// merged on top of these (surface wins on key collision). `TERM`,
    /// `COLORTERM`, and Paneflow identity keys (`PANEFLOW_WORKSPACE_ID`,
    /// `PANEFLOW_SURFACE_ID`, `PANEFLOW_SOCKET_PATH`, `PANEFLOW_BIN_DIR`) are
    /// protected and cannot be overridden. `LD_*` and `DYLD_*` keys are dropped
    /// before PTY spawn. A custom `PATH` is allowed, but Paneflow re-prepends
    /// `PANEFLOW_BIN_DIR` afterward so agent commands still route through the
    /// shim. On Windows, env names are case-insensitive, so user keys are
    /// normalised to uppercase before merging to avoid a `Path`/`PATH` clash.
    /// `None` (block absent) and `Some({})` both inject nothing.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_opt_string_map"
    )]
    pub env: Option<HashMap<String, String>>,
    /// US-022: scroll-wheel multiplier for the non-mouse-mode scrollback path.
    /// Multiplies the pixel delta before the line accumulator, so `> 1.0` speeds
    /// up trackpad/wheel scrollback and `< 1.0` slows it. Forced to `1.0` in
    /// mouse-reporting mode (the PTY owns scroll there; altering the delta would
    /// corrupt the report) and in the alt-screen alternate-scroll path. `None`
    /// resolves to `1.0`. Clamped to `[0.1, 10.0]`. Read when a TerminalView is
    /// constructed, so existing terminals keep their current scroll feel.
    #[serde(default, deserialize_with = "lenient_opt_f32")]
    pub scroll_multiplier: Option<f32>,
}

impl TerminalConfig {
    /// Default scrollback length for interactive CLI sessions.
    pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
    /// Agent terminal profile target. Applied as a cap over the user setting.
    pub const AGENT_SCROLLBACK_LINES: usize = 10_000;
    /// Review terminal profile target. Applied as a cap over the user setting.
    pub const REVIEW_SCROLLBACK_LINES: usize = 2_000;
    /// Cold cached terminal profile target for fresh cached surfaces.
    pub const CACHED_SCROLLBACK_LINES: usize = 1_000;
    /// Lower bound: below 100 lines the buffer is too small to be useful.
    pub const MIN_SCROLLBACK_LINES: usize = 100;
    /// Upper bound: high enough for long-lived agent terminals while keeping
    /// runaway output within a bounded memory budget.
    pub const MAX_SCROLLBACK_LINES: usize = 100_000;

    /// Default scroll multiplier: no amplification.
    pub const DEFAULT_SCROLL_MULTIPLIER: f32 = 1.0;
    /// Lower bound: below 0.1× scrollback would be nearly frozen.
    pub const MIN_SCROLL_MULTIPLIER: f32 = 0.1;
    /// Upper bound: beyond 10× a single tick jumps multiple screens.
    pub const MAX_SCROLL_MULTIPLIER: f32 = 10.0;

    pub fn resolved_integrated_glyphs(&self) -> bool {
        self.integrated_glyphs.unwrap_or(true)
    }

    pub fn resolved_color_emoji(&self) -> bool {
        self.color_emoji.unwrap_or(true)
    }

    pub fn normalized_cursor_color(&self) -> Option<String> {
        self.cursor_color.as_deref().and_then(normalize_hex_color)
    }

    /// Resolve `scroll_multiplier` to a usable value: default `1.0`, clamped to
    /// `[MIN_SCROLL_MULTIPLIER, MAX_SCROLL_MULTIPLIER]`. Emits a `warn!` when the
    /// user value is out of range so they notice the clamp.
    pub fn resolved_scroll_multiplier(&self) -> f32 {
        let raw = self
            .scroll_multiplier
            .unwrap_or(Self::DEFAULT_SCROLL_MULTIPLIER);
        // Guard NaN/infinity (serde rejects them from JSON, but an in-memory or
        // future caller could supply one): `f32::NAN.clamp(..)` is NaN and every
        // NaN comparison is false, which would slip a NaN through and freeze the
        // scroll accumulator. Fall back to the default instead.
        if !raw.is_finite() {
            return Self::DEFAULT_SCROLL_MULTIPLIER;
        }
        let clamped = raw.clamp(Self::MIN_SCROLL_MULTIPLIER, Self::MAX_SCROLL_MULTIPLIER);
        if (clamped - raw).abs() > f32::EPSILON {
            tracing::warn!(
                target: "paneflow_config::terminal",
                requested = raw,
                clamped,
                "terminal.scroll_multiplier out of range [{min}, {max}], clamped",
                min = Self::MIN_SCROLL_MULTIPLIER,
                max = Self::MAX_SCROLL_MULTIPLIER,
            );
        }
        clamped
    }

    /// Resolve the configured `scrollback_lines` to a usable value,
    /// applying default + clamp. Out-of-range values are clamped (a
    /// `warn!` is emitted on the first read so the user notices their
    /// config did not take effect verbatim).
    pub fn resolved_scrollback_lines(&self) -> usize {
        let raw = self
            .scrollback_lines
            .unwrap_or(Self::DEFAULT_SCROLLBACK_LINES);
        let clamped = raw.clamp(Self::MIN_SCROLLBACK_LINES, Self::MAX_SCROLLBACK_LINES);
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::terminal",
                requested = raw,
                clamped,
                "terminal.scrollback_lines out of range [{min}, {max}], clamped",
                min = Self::MIN_SCROLLBACK_LINES,
                max = Self::MAX_SCROLLBACK_LINES,
            );
        }
        clamped
    }

    /// Resolve scrollback for a specific terminal surface profile. The user
    /// setting still provides the base value, then agent/review/cached surfaces
    /// cap it to their documented memory budget.
    pub fn resolved_scrollback_lines_for_profile(&self, profile: TerminalSurfaceProfile) -> usize {
        let base = self.resolved_scrollback_lines();
        profile.scrollback_cap().map_or(base, |cap| base.min(cap))
    }
}

fn lenient_terminal_backend<'de, D>(deserializer: D) -> Result<TerminalBackendConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    TerminalBackendConfig::deserialize(deserializer)
}

/// Agents-view-scoped configuration block (US-103).
///
/// Lives in its own struct so future Phase B-E stories (thinking
/// display mode, profile selector, OS notification gate, ...) can
/// add fields without bloating the top-level [`PaneFlowConfig`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentPanelConfig {
    /// Max width in pixels of the centered conversation column.
    /// `None` resolves to [`AgentPanelConfig::DEFAULT_MAX_CONTENT_WIDTH`]
    /// at the rendering layer; out-of-range values are clamped to
    /// `[MIN_CONTENT_WIDTH_PX, MAX_CONTENT_WIDTH_PX]` by
    /// [`AgentPanelConfig::resolved_max_content_width`] (US-103 AC #5).
    pub max_content_width: Option<u32>,
    /// How thinking / reasoning blocks render in the message stream.
    /// `None` resolves to [`ThinkingDisplayMode::Auto`] -- the v1
    /// behavior where the live burst is expanded and previous bursts
    /// collapse on their own (US-109 AC #1 / #2). An unknown string
    /// in this slot deserialises as `None` via the custom
    /// [`ThinkingDisplayMode`] deserialiser and a `warn!` is logged
    /// at first read (US-109 AC #7).
    pub thinking_display: Option<ThinkingDisplayMode>,
    /// US-115: user-saved named snapshots of
    /// (agent + model + mode + effort + tools). The composer's profile
    /// pill writes here when the user clicks "Save current as profile";
    /// the three built-in profiles (Write / Ask / Minimal) are NOT
    /// persisted -- they are seeded in-memory by the runtime and only
    /// appear here when the user explicitly customises one. Keys are
    /// the human-readable profile names.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ProfileConfig>,
    /// US-115: name of the profile applied on the next panel open.
    /// `None` falls back to the last-used profile (in-memory), and
    /// ultimately to the `Write` built-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    /// US-116: gates OS notifications fired when a turn ends, refuses,
    /// or errors out while Paneflow is not the foreground window.
    /// `None` resolves to [`NotifyWhenAgentWaiting::Never`] so native
    /// notifications are user opt-in. Unknown strings also fail closed
    /// through the custom [`NotifyWhenAgentWaiting`] deserialiser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_when_agent_waiting: Option<NotifyWhenAgentWaiting>,
}

/// US-115: persisted shape of one named profile in `paneflow.json`.
///
/// Every field is optional so a partial profile (e.g. "just lock the
/// effort to Low") round-trips cleanly. The apply path skips `None`
/// fields rather than treating them as a reset -- the user's current
/// state remains untouched for any field the profile does not pin.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProfileConfig {
    /// `AgentKind` discriminant string (`"claude_code"` | `"codex"`).
    /// Stored as `String` so this crate stays free of `paneflow-acp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Model id (e.g. `"claude-sonnet-4-5"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// ACP session mode id (e.g. `"default"`, `"acceptEdits"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `ThinkingEffort` discriminant string (`"low"` | `"medium"` |
    /// `"high"` | `"xhigh"`). Composer maps the string back to its
    /// internal enum on apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Snake_case tool-kind keys (matches the persistence shape used
    /// by `tool_permissions` -- `read`, `edit`, `execute`, ...).
    /// Treated as the set the profile would prefer to "have on" for
    /// the picker UI; the actual permission resolution still goes
    /// through `tool_permissions`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

/// Per-thread display mode for thinking / reasoning blocks
/// (US-109 of `tasks/prd-agent-ui-refactor-2026-Q3.md`).
///
/// Mirrors Zed's `ThinkingBlockDisplay` enum cited in §12 of
/// `docs/ZED_AGENT_REFERENCE.md`. The default is [`Auto`] -- last
/// burst expanded, previous bursts collapsed to header-only.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ThinkingDisplayMode {
    /// Latest streaming burst expanded; previously-completed bursts
    /// collapse to header-only on next chunk arrival.
    #[default]
    Auto,
    /// Header + a fixed `max_h(256px)` body with a top gradient fade
    /// from `panel_bg.opacity(0.8)` to `transparent`. Lets the user
    /// skim every burst at a glance.
    Preview,
    /// Every thinking block stays expanded regardless of recency.
    AlwaysExpanded,
    /// Every thinking block stays collapsed to header-only; the user
    /// can still expand a single block manually.
    AlwaysCollapsed,
}

/// US-116: where OS notifications are surfaced when an agent turn
/// completes (or refuses / errors) while Paneflow is not foregrounded.
///
/// Mirrors Zed's `NotifyWhenAgentWaiting` setting cited in §23 of
/// `docs/ZED_AGENT_REFERENCE.md`. PaneFlow keeps the setting opt-in by
/// defaulting to [`NotifyWhenAgentWaiting::Never`]. Native OS notification
/// APIs do not expose reliable per-display fan-out on every platform;
/// `PrimaryScreen` and `AllScreens` therefore share the same
/// foreground-window gate.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum NotifyWhenAgentWaiting {
    /// Fire a notification only when Paneflow is not the focused window.
    /// Native OS backends do not guarantee a Paneflow-controlled
    /// primary-display filter.
    PrimaryScreen,
    /// Zed-compatible spelling for every-display popups. The native OS
    /// toast path currently treats this like `PrimaryScreen` because the
    /// per-display placement is owned by the notification server.
    AllScreens,
    /// Never fire a notification. Disables the entire US-116 surface;
    /// no DBus / NSNotification / WinRT toast call is issued.
    #[default]
    Never,
}

impl<'de> Deserialize<'de> for NotifyWhenAgentWaiting {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        match raw.as_str() {
            "PrimaryScreen" => Ok(Self::PrimaryScreen),
            "AllScreens" => Ok(Self::AllScreens),
            "Never" => Ok(Self::Never),
            other => {
                tracing::warn!(
                    target: "paneflow_config::agent_panel",
                    value = other,
                    "agent_panel.notify_when_agent_waiting value not recognized, defaulting to Never",
                );
                Ok(Self::Never)
            }
        }
    }
}

impl<'de> Deserialize<'de> for ThinkingDisplayMode {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(d)?;
        match raw.as_str() {
            "Auto" => Ok(Self::Auto),
            "Preview" => Ok(Self::Preview),
            "AlwaysExpanded" => Ok(Self::AlwaysExpanded),
            "AlwaysCollapsed" => Ok(Self::AlwaysCollapsed),
            other => {
                tracing::warn!(
                    target: "paneflow_config::agent_panel",
                    value = other,
                    "agent_panel.thinking_display value not recognized, defaulting to Auto",
                );
                Ok(Self::Auto)
            }
        }
    }
}

impl AgentPanelConfig {
    /// Default cap matching Zed's empirical sweet spot
    /// (`agent_panel.rs:4831`, cited in PRD §"Best Practices Applied").
    pub const DEFAULT_MAX_CONTENT_WIDTH: u32 = 760;
    /// Smallest cap the renderer accepts. Below this, lines start
    /// wrapping every few words and the column becomes unreadable.
    pub const MIN_CONTENT_WIDTH_PX: u32 = 320;
    /// Largest cap the renderer accepts. Above 4000px the cap is
    /// effectively a no-op on every monitor sold today.
    pub const MAX_CONTENT_WIDTH_PX: u32 = 4000;

    /// Resolve the configured `thinking_display` to a concrete mode,
    /// applying the [`ThinkingDisplayMode::Auto`] default when the
    /// field is missing (US-109 AC #1 / #7). Unknown string values are
    /// already filtered by the custom [`ThinkingDisplayMode`]
    /// deserialiser, so the only mapping needed here is `None` -> Auto.
    pub fn resolved_thinking_display(&self) -> ThinkingDisplayMode {
        self.thinking_display.unwrap_or_default()
    }

    /// Resolve the configured `notify_when_agent_waiting` to a concrete
    /// gate, applying the [`NotifyWhenAgentWaiting::Never`] default when
    /// the field is missing. Unknown strings are already filtered by the
    /// custom [`NotifyWhenAgentWaiting`] deserialiser so the only mapping
    /// needed here is `None` -> `Never`.
    pub fn resolved_notify_when_agent_waiting(&self) -> NotifyWhenAgentWaiting {
        self.notify_when_agent_waiting.unwrap_or_default()
    }

    /// Resolve the configured `max_content_width` to a usable pixel
    /// value, applying default + clamp + a `warn!` line on out-of-range
    /// input (US-103 AC #1 / #5).
    pub fn resolved_max_content_width(&self) -> u32 {
        let raw = self
            .max_content_width
            .unwrap_or(Self::DEFAULT_MAX_CONTENT_WIDTH);
        let clamped = raw.clamp(Self::MIN_CONTENT_WIDTH_PX, Self::MAX_CONTENT_WIDTH_PX);
        if clamped != raw {
            tracing::warn!(
                target: "paneflow_config::agent_panel",
                requested = raw,
                clamped,
                "agent_panel.max_content_width out of range [{min}, {max}], clamped",
                min = Self::MIN_CONTENT_WIDTH_PX,
                max = Self::MAX_CONTENT_WIDTH_PX,
            );
        }
        clamped
    }
}

/// Desktop telemetry consent state.
///
/// Kept in its own struct (rather than a bare `Option<bool>` on
/// `PaneFlowConfig`) so future telemetry-scoped settings (e.g. a per-user
/// `distinct_id` override, or per-event category toggles) can be added
/// without schema churn.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TelemetryConfig {
    /// Consent tri-state. `None` = unanswered, `Some(true)` = opted in,
    /// `Some(false)` = opted out. `PANEFLOW_NO_TELEMETRY=1` env var
    /// overrides this unconditionally at the client layer (US-012).
    pub enabled: Option<bool>,
}

/// A single command definition, compatible with the cmux workspace format.
///
/// Each entry is either a workspace definition (with `workspace`) or a simple
/// shell command (with `command`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandDefinition {
    /// Display name (must not be blank).
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Search keywords for fuzzy matching.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Workspace layout definition (mutually exclusive with `command`).
    pub workspace: Option<WorkspaceDefinition>,
    /// Shell command string (mutually exclusive with `workspace`).
    pub command: Option<String>,
}

/// Workspace definition containing layout, working directory, and visual config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceDefinition {
    /// Workspace display name.
    pub name: Option<String>,
    /// Default working directory for the workspace.
    pub cwd: Option<String>,
    /// Layout preset used by the visual workspace builder.
    ///
    /// Accepted values mirror `paneflow up`: `"even_h"`, `"even_v"`,
    /// `"main_vertical"`, and `"tiled"`. Older configs may omit this and rely
    /// on `layout` alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_preset: Option<String>,
    /// Color as a 6-digit hex string (e.g. "ff6600").
    pub color: Option<String>,
    /// Root layout node describing pane arrangement.
    pub layout: Option<LayoutNode>,
}

/// A node in the layout tree: either a leaf pane or a split container.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNode {
    /// A leaf pane containing one or more surfaces.
    Pane {
        /// Surfaces within this pane (must have >= 1).
        #[serde(default)]
        surfaces: Vec<SurfaceDefinition>,
    },
    /// A split container dividing space between 2 or more children.
    Split {
        /// Split direction: "horizontal" or "vertical".
        direction: String,
        /// Legacy: single split ratio for binary (2-child) layouts.
        /// Ignored when `ratios` is present. Defaults to 0.5 if omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratio: Option<f64>,
        /// Per-child ratios for N-ary layouts. When present, must have
        /// the same length as `children`. Values should sum to ~1.0.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ratios: Option<Vec<f64>>,
        /// 2 or more child layout nodes.
        #[serde(default)]
        children: Vec<LayoutNode>,
    },
}

impl LayoutNode {
    /// Count the number of leaf (Pane) nodes in the layout tree.
    pub fn leaf_count(&self) -> usize {
        match self {
            LayoutNode::Pane { .. } => 1,
            LayoutNode::Split { children, .. } => children.iter().map(|c| c.leaf_count()).sum(),
        }
    }

    /// Resolve per-child ratios for a Split node.
    ///
    /// Returns `ratios` if present, else converts legacy `ratio` to binary
    /// `[ratio, 1-ratio]`, else returns equal ratios for the child count.
    ///
    /// US-056: persisted ratios are untrusted input - a hand-edited or corrupt
    /// `session.json` can carry NaN, negative, zero, or wrong-length values. Any
    /// user-supplied set is run through [`sanitize_ratios`] (clamp into
    /// `[MIN_RATIO, 1.0]`, reject non-finite/negative, normalize to sum 1.0)
    /// before it reaches layout construction; the internally generated
    /// equal-share fallback is already valid and returned verbatim.
    pub fn resolved_ratios(&self) -> Vec<f64> {
        match self {
            LayoutNode::Pane { .. } => vec![1.0],
            LayoutNode::Split {
                ratio,
                ratios,
                children,
                ..
            } => {
                let n = children.len().max(1);
                let raw = if let Some(rs) = ratios {
                    rs.clone()
                } else if let Some(r) = ratio {
                    if children.len() == 2 {
                        vec![*r, 1.0 - *r]
                    } else {
                        return vec![1.0 / n as f64; n];
                    }
                } else {
                    return vec![1.0 / n as f64; n];
                };
                sanitize_ratios(raw, n)
            }
        }
    }
}

/// Floor for any single persisted split ratio. Clamping to this keeps every
/// pane visible and prevents a divide-by-zero when the set is normalized.
const MIN_RATIO: f64 = 0.01;

/// Clamp every ratio into `[MIN_RATIO, 1.0]` (mapping NaN/inf/negative to the
/// floor), then normalize so the set sums to 1.0. A length mismatch with the
/// child count is unrecoverable - we cannot know which child a stale ratio was
/// meant for - so it degrades to equal shares.
fn sanitize_ratios(mut ratios: Vec<f64>, n: usize) -> Vec<f64> {
    if ratios.len() != n {
        return vec![1.0 / n as f64; n];
    }
    for r in ratios.iter_mut() {
        *r = if r.is_finite() {
            r.clamp(MIN_RATIO, 1.0)
        } else {
            MIN_RATIO
        };
    }
    let sum: f64 = ratios.iter().sum();
    if sum > 0.0 && (sum - 1.0).abs() > 1e-9 {
        for r in ratios.iter_mut() {
            *r /= sum;
        }
    }
    // US-056 (EP-010 review): re-clamp after normalize. Dividing by a sum > 1
    // can push a just-clamped ratio back below `MIN_RATIO` (e.g. raw
    // `[1.0, 0.005]` → clamp `[1.0, 0.01]` → normalize `[0.990, 0.0099]`),
    // silently violating the floor this fn promises. The config-loader sibling
    // (`loader::validate_layout`) already re-clamps for this exact reason
    // (US-057); the session path must match so both frontiers honour the same
    // 0.01 floor. The renderer re-normalizes proportionally at paint time, so
    // the post-re-clamp sum need not be exactly 1.0 - the floor is the invariant.
    for r in ratios.iter_mut() {
        *r = r.clamp(MIN_RATIO, 1.0);
    }
    ratios
}

/// Top-level UI mode (US-007/US-008 of `prd-agents-view.md`;
/// `Diff` added by US-001 of `prd-git-diff-mode-2026-Q3.md`).
///
/// `Cli` is the traditional terminal-multiplexer view. `Diff` is the
/// dedicated git/worktree diff surface (left git panel + diff area).
/// `Agents` is the Agents view (project + thread sidebar + chat thread).
/// Default is `Cli` so existing users see no behaviour change on first
/// launch after upgrading. Variant order mirrors the on-screen segment
/// order (CLI / Diff / Agents) in `render_mode_toggle`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    #[default]
    Cli,
    Diff,
    Agents,
}

/// Stable selection target for the Agents view center surface.
///
/// The runtime target uses vector indices because the UI mutates those
/// collections directly. The persisted shape stores stable IDs instead so a
/// session restore after project/thread capping or reordering never points at
/// the wrong row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentsTargetSession {
    /// A thread inside the project with `project_id`.
    Thread { project_id: u64, thread_id: u64 },
    /// A free chat thread.
    Chat { thread_id: u64 },
}

/// Persisted session state written to `~/.cache/paneflow/session.json`.
///
/// Backward-compat note: the three Agents-view fields (`projects`,
/// `active_project`, `mode`) all carry `#[serde(default)]`. Loading a
/// session.json written by a pre-US-007 build deserialises cleanly --
/// the missing keys resolve to an empty project list and `AppMode::Cli`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionState {
    /// Schema version for forward-compatible migrations.
    pub version: u32,
    /// Index of the active workspace at save time.
    pub active_workspace: usize,
    /// Ordered list of workspace snapshots.
    pub workspaces: Vec<WorkspaceSession>,
    /// Ordered list of project snapshots for the Agents view.
    /// US-007 of `tasks/prd-agents-view.md`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectSession>,
    /// Index of the active project at save time. `0` when no projects
    /// exist (the sidebar treats `projects.is_empty()` as the empty
    /// state regardless of this value).
    #[serde(default)]
    pub active_project: usize,
    /// Free chats - terminal threads not attached to any project, anchored
    /// on the user's home dir (US-002 of
    /// `prd-agents-ui-codex-redesign-2026-Q3.md`). A separate list from
    /// `projects` by design (no implicit "~" project). `skip_serializing_if`
    /// mirrors the `projects` field, so a pre-refonte session.json
    /// (without this key) restores as an empty chat list without touching
    /// the project serialization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chats: Vec<ThreadSession>,
    /// Last selected Agents-view center target. Stored by stable IDs rather
    /// than indices so restore can remap through capped/filtered project and
    /// chat lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agents_target: Option<AgentsTargetSession>,
    /// Last UI mode the user was in. The bootstrap reads this to
    /// reopen the Agents view if it was active at quit time (US-009).
    #[serde(default)]
    pub mode: AppMode,
    /// US-015 (prd-git-diff-mode-2026-Q3.md): the Git Diff view scope at save
    /// time, snake_case (`"project"` / `"multi_project"` / `"worktree"`),
    /// restored into `AppMode::Diff` on boot when reconstructable. Stored as a
    /// string so this config crate stays independent of the app's `DiffScope`
    /// type. Absent / `None` on sessions written before this field - defaults
    /// to the app's `DiffScope::default()` (Project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_scope: Option<String>,
}

/// Persisted shape of one [`crate::project::Project`] (the runtime type
/// lives in `src-app/src/project/mod.rs`). The `id` is the in-memory
/// monotonic counter at save time -- it is restored on load so the
/// counter stays monotonic across restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSession {
    /// Monotonic in-memory ID at save time.
    pub id: u64,
    /// Human-readable title (sidebar header).
    pub title: String,
    /// Root cwd for new threads in this project.
    pub cwd: String,
    /// Whether the sidebar header was expanded at save time. `true`
    /// is the default for backward-compat (a missing key restores as
    /// "expanded" so an old session.json doesn't ghost the threads).
    #[serde(default = "default_true")]
    pub is_expanded: bool,
    /// Ordered list of thread snapshots in this project.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<ThreadSession>,
}

/// Persisted shape of one thread (the runtime type lives in
/// `src-app/src/project/mod.rs`). Thread *content* (messages, tool
/// calls, attachments) is NOT stored here -- that lives in the
/// `paneflow-threads` SQLite DB (US-006). This struct holds only the
/// sidebar-visible metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThreadSession {
    /// Monotonic in-memory ID at save time.
    pub id: u64,
    /// Human-readable title (sidebar row).
    pub title: String,
    /// Wire-format tag for the agent. Canonical values: `"claude_code"`,
    /// `"codex"`. Stored as a `String` rather than a typed enum so
    /// `paneflow-config` does not need to depend on `paneflow-acp`
    /// (which would pull tokio + ACP into this lightweight crate).
    pub agent: String,
    /// Per-thread cwd. May differ from the parent project's cwd if the
    /// user explicitly forked into a subdirectory.
    pub cwd: String,
    /// Creation timestamp (unix-epoch milliseconds UTC). Used by the
    /// sidebar for relative-time labels.
    pub created_at: u64,
    /// Last selected model name from the agent's `session/new` response.
    /// `None` means "use the agent's default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Last selected ACP mode (Claude: `default`/`acceptEdits`/`plan`...).
    /// `None` means "use the agent's default".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Foreign key into the `paneflow-threads` SQLite DB. `None` for
    /// threads that have never been persisted (the runtime layer sets
    /// this on first `append_message`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_id: Option<String>,
    /// Runtime kind discriminant for the v1.x "Terminal Thread" surface
    /// (mirrors Zed's `AgentPanelEntryKind`). `None` (the default for
    /// every pre-Terminal-Thread session.json) restores as the legacy
    /// `Agent` kind. `Some("terminal")` restores as a Terminal Thread
    /// (PTY surface in the main area instead of a chat). Unknown
    /// strings fall back to `Agent` so a forward-rolled session from a
    /// future build does not ghost the row.
    ///
    /// Stored as a `String` rather than a typed enum so this crate
    /// stays free of the runtime `ThreadKind` enum (which lives in
    /// `src-app` to keep `paneflow-config` a leaf crate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Which CLI coding agent a Terminal Thread launches on first mount.
    /// Valid values are runtime `TerminalAgent::tag()` strings such as
    /// `"claude_code"`, `"codex"`, `"opencode"`, `"pi"`, `"hermes"`,
    /// `"grok"`, `"cursor"`, `"gemini"`, `"kiro"`, and other launcher tags.
    /// Drives the sidebar row icon and the auto-run command. `None`
    /// restores as a bare shell (legacy Terminal Threads + plain
    /// "New terminal thread" rows). Stored as a tag string so this crate
    /// stays free of the runtime `TerminalAgent` enum (which lives in
    /// `src-app`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_agent: Option<String>,
    /// Whether the user pinned this thread (US-001 of
    /// `prd-agents-ui-codex-redesign-2026-Q3.md`). Pinned threads are
    /// surfaced in the rail's PINNED section across both projects and
    /// free chats. `#[serde(default)]` so a session.json written before
    /// this field deserialises cleanly as `false` - no migration.
    #[serde(default)]
    pub pinned: bool,
    /// Forced agent session UUID for a Claude Terminal Thread, passed as
    /// `claude --session-id <uuid>` on launch so the thread binds 1:1 to
    /// its on-disk session file (`~/.claude/projects/<slug>/<uuid>.jsonl`).
    /// Persisting it means a restart relaunches the SAME session (Claude
    /// resumes + appends on an existing id) and the sidebar can backfill
    /// that session's LLM `ai-title`. `None` for agents that don't support
    /// a forced id (everything but Claude) and pre-feature sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Whether the user manually renamed this row. Once set, OSC title
    /// updates and the `ai-title` backfill stop overwriting the label so a
    /// deliberate name is never clobbered by agent activity. `#[serde(default)]`
    /// so older session.json files restore as `false`.
    #[serde(default)]
    pub title_user_set: bool,
}

fn default_true() -> bool {
    true
}

/// Snapshot of a single workspace for session persistence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceSession {
    /// Workspace display title.
    pub title: String,
    /// Root working directory of the workspace.
    pub cwd: String,
    /// Layout tree (splits + panes). `None` means a single default pane.
    pub layout: Option<LayoutNode>,
    /// User-defined command buttons rendered in this workspace's tab bar.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub custom_buttons: Vec<ButtonCommand>,
    /// Workspace-relative directory paths expanded in the Files tree sidebar
    /// (PRD files-tree US-007). Additive + optional: absent in older
    /// `session.json` files, which deserialize to an empty list and never
    /// break restore of the other fields. The sidebar's open/closed state is
    /// deliberately NOT persisted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expanded_paths: Vec<String>,
    /// Git worktrees Paneflow created for this workspace via `paneflow up`
    /// (EP-002, prd-orchestration-v2). Persisted so a crash/restart keeps the
    /// ownership record (teardown at close, `git worktree prune` at startup).
    /// Additive + optional like `expanded_paths`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub managed_worktrees: Vec<ManagedWorktreeDef>,
}

/// A git worktree created (and therefore owned) by Paneflow for one pane of a
/// `paneflow up` workspace. Paths are stored absolute; `teardown` is `"auto"`
/// (remove at close when clean) or `"keep"`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ManagedWorktreeDef {
    /// Worktree checkout directory.
    pub path: String,
    /// Main repository root (where `git worktree` commands run).
    pub repo_root: String,
    /// Branch checked out in the worktree (diagnostics only - never deleted).
    pub branch: String,
    /// Teardown policy: `"auto"` | `"keep"`. Unknown values read as `"auto"`;
    /// the data-loss protection is the clean-check, not this flag.
    pub teardown: String,
}

/// A user-defined command button rendered in a workspace's tab bar.
/// Clicking the button sends `{command}\r` to the active terminal.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ButtonCommand {
    /// Stable identifier (opaque string) - survives reorderings and renames.
    pub id: String,
    /// Display name (also used as hover tooltip).
    pub name: String,
    /// Icon asset path relative to the `assets/` folder (e.g. `"icons/rocket.svg"`).
    pub icon: String,
    /// Shell command string, executed verbatim in the active terminal
    /// with a trailing `\r` appended (no bracketed-paste wrapping).
    pub command: String,
}

/// A surface within a pane (terminal, browser, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SurfaceDefinition {
    /// Surface type identifier: "terminal", "browser", etc.
    pub surface_type: Option<String>,
    /// Display name for this surface.
    pub name: Option<String>,
    /// User-assigned custom name (US-013). When set, it overrides the
    /// auto-derived surface name everywhere (sidebar/IPC `surface.list`/MCP),
    /// and survives restart via this field. Cleared by renaming to empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_name: Option<String>,
    /// Shell command to run in this surface.
    pub command: Option<String>,
    /// Prompt text to prefill after launching an agent command.
    ///
    /// Kept optional so session persistence and plain command panes do not
    /// carry template-only state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Working directory override for this surface.
    pub cwd: Option<String>,
    /// File path for non-terminal surfaces such as markdown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Extra environment variables merged over `terminal.env`. The same
    /// protected-key and loader-key filtering applies at PTY spawn.
    pub env: Option<HashMap<String, String>>,
    /// Whether this surface should receive initial focus.
    pub focus: Option<bool>,
    /// Saved scrollback text (plain, ANSI stripped). Up to 4000 lines / 400K chars.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<String>,
    /// EP-005 US-013: stable tag of the agent CLI last detected in this
    /// surface's PTY subtree (e.g. `"claude_code"`), so the identity pill
    /// survives restart as a dimmed "last known" until the first scan
    /// confirms it. Whitelisted at ingress against the known agent tags;
    /// unknown or malformed values are dropped silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// EP-006 US-019: per-pane font-size override in points. `None` =
    /// follow the global config. Validated at restore ingress (NaN/inf
    /// dropped, finite values clamped to [8.0, 32.0]) - never fed raw to
    /// the cell geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};

    fn object_keys(value: &serde_json::Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("expected JSON object")
            .keys()
            .cloned()
            .collect()
    }

    fn key_set(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    fn assert_doc_mentions_property_keys(doc: &str, value: &serde_json::Value, context: &str) {
        for key in value
            .as_object()
            .expect("expected schema properties")
            .keys()
        {
            let needle = format!("`{key}`");
            let dotted = format!("`{context}.{key}`");
            assert!(
                doc.contains(&needle) || doc.contains(&dotted),
                "configuration docs do not mention public schema key {context}.{key}"
            );
        }
    }

    #[test]
    fn public_json_schema_covers_every_config_field() {
        let mut permissions = HashMap::new();
        permissions.insert(
            "read".to_string(),
            ToolPermissionsEntry {
                always_allow: vec!["src/".to_string()],
                always_deny: vec!["secrets/".to_string()],
            },
        );
        let mut profiles = HashMap::new();
        profiles.insert(
            "Write".to_string(),
            ProfileConfig {
                agent: Some("codex".to_string()),
                model: Some("default".to_string()),
                mode: Some("default".to_string()),
                effort: Some("medium".to_string()),
                tools: vec!["read".to_string()],
            },
        );

        // Deliberately exhaustive struct literals: adding a Rust config field
        // fails this test at compile time until the public schema is updated.
        let config = PaneFlowConfig {
            shortcuts: HashMap::new(),
            default_shell: Some("sh".to_string()),
            theme: Some("One Dark".to_string()),
            theme_mode: Some("dark".to_string()),
            commands: Vec::new(),
            window_decorations: Some("client".to_string()),
            window_backdrop: Some("auto".to_string()),
            windows_terminal_material: Some(true),
            windows_chrome_material: Some(true),
            line_height: Some(1.2),
            cell_width: Some(0.6),
            font_family: Some("Geist Mono".to_string()),
            font_fallbacks: Some(vec!["FiraCode Nerd Font Mono".to_string()]),
            font_size: Some(13.0),
            font_weight: Some("normal".to_string()),
            option_as_meta: Some(true),
            shell_integration: Some(true),
            agent_stall_detection: Some(true),
            agent_stall_threshold_secs: Some(300),
            review_prefill_delay_ms: Some(2000),
            submit_paste_delay_ms: Some(70),
            external_editor: Some("auto".to_string()),
            claude_code_bypass_permissions: Some(false),
            ai_unrestricted: Some(true),
            ai_injection_fence: Some(false),
            claude_code_button_visible: Some(true),
            codex_button_visible: Some(true),
            opencode_button_visible: Some(true),
            pi_button_visible: Some(true),
            hermes_agent_button_visible: Some(true),
            grok_button_visible: Some(true),
            amp_button_visible: Some(true),
            cursor_button_visible: Some(true),
            gemini_button_visible: Some(true),
            kiro_button_visible: Some(true),
            antigravity_button_visible: Some(true),
            copilot_button_visible: Some(true),
            codebuddy_button_visible: Some(true),
            factory_button_visible: Some(true),
            qoder_button_visible: Some(true),
            openclaw_button_visible: Some(true),
            telemetry: Some(TelemetryConfig {
                enabled: Some(false),
            }),
            terminal: Some(TerminalConfig {
                backend: TerminalBackendConfig::Auto,
                ligatures: Some(false),
                integrated_glyphs: Some(true),
                color_emoji: Some(true),
                cursor_color: Some(APPLE_SYSTEM_BLUE_HEX.to_string()),
                scrollback_lines: Some(10_000),
                cursor_shape: Some(CursorShapeConfig::Block),
                cursor_blink: Some(CursorBlinkConfig::TerminalControlled),
                env: Some(HashMap::new()),
                scroll_multiplier: Some(1.0),
            }),
            agent_panel: Some(AgentPanelConfig {
                max_content_width: Some(760),
                thinking_display: Some(ThinkingDisplayMode::Auto),
                profiles,
                default_profile: Some("Write".to_string()),
                notify_when_agent_waiting: Some(NotifyWhenAgentWaiting::PrimaryScreen),
            }),
            tool_permissions: permissions,
        };

        let schema_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../schemas/paneflow.schema.json");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
        let serialized = serde_json::to_value(config).unwrap();
        let mut schema_top_level = object_keys(&schema["properties"]);
        schema_top_level.remove("$schema");
        schema_top_level.remove("$schemaVersion");

        assert_eq!(
            object_keys(&serialized),
            schema_top_level,
            "top-level PaneFlowConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["terminal"]),
            object_keys(&schema["properties"]["terminal"]["properties"]),
            "TerminalConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["agent_panel"]),
            object_keys(&schema["properties"]["agent_panel"]["properties"]),
            "AgentPanelConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["agent_panel"]["profiles"]["Write"]),
            object_keys(&schema["definitions"]["profileConfig"]["properties"]),
            "ProfileConfig and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized["tool_permissions"]["read"]),
            object_keys(&schema["definitions"]["toolPermissionsEntry"]["properties"]),
            "ToolPermissionsEntry and public JSON Schema drifted"
        );

        let command = CommandDefinition {
            name: "Dev".to_string(),
            description: Some("Open dev workspace".to_string()),
            keywords: vec!["dev".to_string()],
            workspace: Some(WorkspaceDefinition {
                name: Some("Dev".to_string()),
                cwd: Some("~/dev".to_string()),
                layout_preset: Some("even_h".to_string()),
                color: Some("007aff".to_string()),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition {
                        surface_type: Some("terminal".to_string()),
                        name: Some("Claude".to_string()),
                        custom_name: Some("Agent".to_string()),
                        command: Some("claude".to_string()),
                        prompt: Some("Review this".to_string()),
                        cwd: Some("~/dev/app".to_string()),
                        path: None,
                        env: Some(HashMap::new()),
                        focus: Some(true),
                        scrollback: Some("previous output".to_string()),
                        agent: Some("claude_code".to_string()),
                        font_size: Some(13.0),
                    }],
                }),
            }),
            command: None,
        };
        let serialized_command = serde_json::to_value(command).unwrap();
        assert_eq!(
            object_keys(&serialized_command),
            object_keys(&schema["definitions"]["commandDefinition"]["properties"]),
            "CommandDefinition and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized_command["workspace"]),
            object_keys(&schema["definitions"]["workspaceDefinition"]["properties"]),
            "WorkspaceDefinition and public JSON Schema drifted"
        );
        assert_eq!(
            key_set(&["type", "surfaces"]),
            object_keys(&schema["definitions"]["layoutNode"]["oneOf"][0]["properties"]),
            "Pane layout node and public JSON Schema drifted"
        );
        assert_eq!(
            key_set(&["type", "direction", "ratio", "ratios", "children"]),
            object_keys(&schema["definitions"]["layoutNode"]["oneOf"][1]["properties"]),
            "Split layout node and public JSON Schema drifted"
        );
        assert_eq!(
            object_keys(&serialized_command["workspace"]["layout"]["surfaces"][0]),
            object_keys(&schema["definitions"]["surface"]["properties"]),
            "SurfaceDefinition and public JSON Schema drifted"
        );
    }

    #[test]
    fn public_configuration_schema_doc_mentions_schema_keys() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let schema_path = root.join("schemas/paneflow.schema.json");
        let doc_path = root.join("docs/user/configuration/schema.md");
        let schema: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(schema_path).unwrap()).unwrap();
        let doc = std::fs::read_to_string(doc_path).unwrap();

        assert_doc_mentions_property_keys(&doc, &schema["properties"], "top-level");
        assert_doc_mentions_property_keys(
            &doc,
            &schema["properties"]["terminal"]["properties"],
            "terminal",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["properties"]["agent_panel"]["properties"],
            "agent_panel",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["profileConfig"]["properties"],
            "profileConfig",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["toolPermissionsEntry"]["properties"],
            "toolPermissionsEntry",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["commandDefinition"]["properties"],
            "commandDefinition",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["workspaceDefinition"]["properties"],
            "workspaceDefinition",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["layoutNode"]["oneOf"][0]["properties"],
            "paneLayoutNode",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["layoutNode"]["oneOf"][1]["properties"],
            "splitLayoutNode",
        );
        assert_doc_mentions_property_keys(
            &doc,
            &schema["definitions"]["surface"]["properties"],
            "surface",
        );

        assert!(
            doc.contains("| `font_size` | number or null | `13.0` |"),
            "configuration docs must publish the runtime font_size default"
        );
        assert!(
            doc.contains("| `line_height` | number or null | `1.2` |"),
            "configuration docs must publish the runtime line_height default"
        );
        assert!(
            doc.contains("Windows: configured -> `pwsh.exe` -> `powershell.exe`"),
            "configuration docs must describe the Windows shell fallback chain"
        );
    }

    #[test]
    fn terminal_scrollback_profiles_resolve_defaults_and_caps() {
        let cfg = TerminalConfig::default();
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Normal),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Review),
            2_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Cached),
            1_000
        );

        let cfg = TerminalConfig {
            scrollback_lines: Some(50_000),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Normal),
            50_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            10_000
        );
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Review),
            2_000
        );

        let cfg = TerminalConfig {
            scrollback_lines: Some(500),
            ..Default::default()
        };
        assert_eq!(
            cfg.resolved_scrollback_lines_for_profile(TerminalSurfaceProfile::Agent),
            500
        );
    }

    #[test]
    fn agent_stall_settings_resolve_with_defaults_and_clamp() {
        // EP-004 US-011 + US-013: default ON, threshold 60 s (tightened from
        // 300 s so a lost ai.stop surfaces in seconds, not minutes).
        let cfg = PaneFlowConfig::default();
        assert!(cfg.agent_stall_detection_enabled());
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 60);

        // Kill switch.
        let cfg = PaneFlowConfig {
            agent_stall_detection: Some(false),
            ..Default::default()
        };
        assert!(!cfg.agent_stall_detection_enabled());

        // Clamp both ends.
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(1),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 30);
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 86_400);
        let cfg = PaneFlowConfig {
            agent_stall_threshold_secs: Some(600),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_agent_stall_threshold_secs(), 600);
    }

    #[test]
    fn cockpit_chrome_material_respects_windows_switch_only() {
        let cfg = PaneFlowConfig::default();
        assert_eq!(
            cfg.cockpit_chrome_material_enabled(),
            !cfg!(target_os = "windows")
        );

        let cfg = PaneFlowConfig {
            windows_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(cfg.cockpit_chrome_material_enabled());

        let cfg = PaneFlowConfig {
            windows_chrome_material: Some(false),
            ..Default::default()
        };
        assert_eq!(
            cfg.cockpit_chrome_material_enabled(),
            !cfg!(target_os = "windows")
        );

        let cfg = PaneFlowConfig {
            window_backdrop: Some("opaque".to_string()),
            windows_chrome_material: Some(true),
            ..Default::default()
        };
        assert!(!cfg.cockpit_chrome_material_enabled());
    }

    #[test]
    fn submit_paste_delay_resolves_with_default_and_clamp() {
        // EP-001 US-001 (agent-control-plane-hardening): default 70 ms,
        // clamped to [10, 5000].
        assert_eq!(
            PaneFlowConfig::default().resolved_submit_paste_delay_ms(),
            70
        );
        // Below the floor clamps up.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(0),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 10);
        // Above the ceiling clamps down.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(u64::MAX),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 5_000);
        // In-range passes through untouched.
        let cfg = PaneFlowConfig {
            submit_paste_delay_ms: Some(120),
            ..Default::default()
        };
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 120);
    }

    #[test]
    fn submit_paste_delay_serde_roundtrips() {
        // The knob travels through the public JSON shape unchanged
        // (`#[serde(default)]` on the struct fills every other field).
        let cfg: PaneFlowConfig =
            serde_json::from_str(r#"{"submit_paste_delay_ms": 90}"#).expect("valid config");
        assert_eq!(cfg.submit_paste_delay_ms, Some(90));
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 90);
        // Absent -> None -> default.
        let cfg: PaneFlowConfig = serde_json::from_str("{}").expect("empty config");
        assert!(cfg.submit_paste_delay_ms.is_none());
        assert_eq!(cfg.resolved_submit_paste_delay_ms(), 70);
    }

    #[test]
    fn ai_access_toggles_default_safe_and_tolerate_garbage() {
        // EP-003 US-008 AC #1/#5: a fresh config never opens free-access and
        // always fences.
        let cfg = PaneFlowConfig::default();
        assert!(!cfg.ai_unrestricted_enabled(), "unrestricted defaults OFF");
        assert!(cfg.ai_injection_fence_enabled(), "fence defaults ON");

        // Explicit booleans round-trip through the lenient deserializer.
        let cfg: PaneFlowConfig =
            serde_json::from_str(r#"{"ai_unrestricted": true, "ai_injection_fence": false}"#)
                .unwrap();
        assert!(cfg.ai_unrestricted_enabled());
        assert!(!cfg.ai_injection_fence_enabled());

        // AC #3: a non-boolean value fails CLOSED (unrestricted -> false, fence
        // -> true) instead of erroring the whole parse, and does NOT wipe the
        // sibling settings the all-or-nothing loader fallback would have lost.
        let cfg: PaneFlowConfig = serde_json::from_str(
            r#"{"theme": "One Dark", "ai_unrestricted": "yes", "ai_injection_fence": 0}"#,
        )
        .unwrap();
        assert!(
            !cfg.ai_unrestricted_enabled(),
            "a garbage value must never open the mode"
        );
        assert!(
            cfg.ai_injection_fence_enabled(),
            "a garbage value must never drop the fence"
        );
        assert_eq!(
            cfg.theme.as_deref(),
            Some("One Dark"),
            "siblings survive a malformed AI-access toggle"
        );
    }

    #[test]
    fn agent_panel_notifications_are_opt_in_by_default() {
        let cfg: AgentPanelConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.notify_when_agent_waiting.is_none());
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::Never
        );

        let cfg: AgentPanelConfig =
            serde_json::from_str(r#"{"notify_when_agent_waiting": "Bogus"}"#).unwrap();
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::Never
        );

        let cfg: AgentPanelConfig =
            serde_json::from_str(r#"{"notify_when_agent_waiting": "PrimaryScreen"}"#).unwrap();
        assert_eq!(
            cfg.resolved_notify_when_agent_waiting(),
            NotifyWhenAgentWaiting::PrimaryScreen
        );
    }

    #[test]
    fn agent_panel_thinking_display_pascal_case_roundtrip() {
        // US-109 AC #1: PascalCase tags as documented in the PRD.
        let raw = r#"{"thinking_display": "Preview"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Preview));

        let raw = r#"{"thinking_display": "AlwaysExpanded"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            cfg.thinking_display,
            Some(ThinkingDisplayMode::AlwaysExpanded)
        );

        let raw = r#"{"thinking_display": "AlwaysCollapsed"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(
            cfg.thinking_display,
            Some(ThinkingDisplayMode::AlwaysCollapsed)
        );

        let raw = r#"{"thinking_display": "Auto"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Auto));
    }

    #[test]
    fn agent_panel_thinking_display_unknown_falls_back_to_auto() {
        // US-109 AC #7: unknown string deserialises as Auto (the
        // custom deserialiser logs a warn! line; this test asserts
        // only the surface behavior since `warn!` is not captured).
        let raw = r#"{"thinking_display": "Bogus"}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(cfg.thinking_display, Some(ThinkingDisplayMode::Auto));
    }

    #[test]
    fn agent_panel_thinking_display_missing_resolves_to_auto() {
        // US-109 AC #1: missing field resolves to Auto via the
        // resolver (the on-disk Option stays `None`).
        let raw = r#"{}"#;
        let cfg: AgentPanelConfig = serde_json::from_str(raw).unwrap();
        assert!(cfg.thinking_display.is_none());
        assert_eq!(cfg.resolved_thinking_display(), ThinkingDisplayMode::Auto);
    }

    #[test]
    fn cursor_shape_and_blink_config_serde() {
        // US-007 / US-008: snake_case config values + historical defaults.
        assert_eq!(CursorShapeConfig::default(), CursorShapeConfig::Block);
        assert_eq!(
            CursorBlinkConfig::default(),
            CursorBlinkConfig::TerminalControlled
        );

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "beam", "cursor_blink": "off"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Beam));
        assert_eq!(cfg.cursor_blink, Some(CursorBlinkConfig::Off));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_shape": "hollow"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Hollow));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_shape": "vintage"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Vintage));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "double_underline"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::DoubleUnderline));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_shape": "filled_box"}"#).unwrap();
        assert_eq!(cfg.cursor_shape, Some(CursorShapeConfig::Block));

        // Missing → None → resolves to historical defaults.
        let cfg: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.cursor_shape.is_none() && cfg.cursor_blink.is_none());
        assert_eq!(
            cfg.cursor_shape.unwrap_or_default(),
            CursorShapeConfig::Block
        );
        assert_eq!(
            cfg.cursor_blink.unwrap_or_default(),
            CursorBlinkConfig::TerminalControlled
        );
    }

    #[test]
    fn terminal_backend_is_lenient_and_defaults_to_auto() {
        let automatic: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(automatic.backend, TerminalBackendConfig::Auto);

        let ghostty: TerminalConfig = serde_json::from_str(r#"{"backend":"ghostty"}"#).unwrap();
        assert_eq!(ghostty.backend, TerminalBackendConfig::Ghostty);

        let unknown: TerminalConfig =
            serde_json::from_str(r#"{"backend":"future-engine"}"#).unwrap();
        assert_eq!(unknown.backend, TerminalBackendConfig::Auto);
    }

    #[test]
    fn cursor_color_hex_normalizes_and_defaults_to_theme_when_absent() {
        let cfg: TerminalConfig = serde_json::from_str(r##"{"cursor_color": "#0a84ff"}"##).unwrap();
        assert_eq!(cfg.normalized_cursor_color().as_deref(), Some("#0A84FF"));

        let cfg: TerminalConfig = serde_json::from_str(r#"{"cursor_color": "abc"}"#).unwrap();
        assert_eq!(cfg.normalized_cursor_color().as_deref(), Some("#AABBCC"));

        let cfg: TerminalConfig =
            serde_json::from_str(r#"{"cursor_color": "not-a-color"}"#).unwrap();
        assert!(cfg.normalized_cursor_color().is_none());

        let cfg: TerminalConfig = serde_json::from_str(r#"{}"#).unwrap();
        assert!(cfg.cursor_color.is_none());
        assert!(cfg.normalized_cursor_color().is_none());
    }
}
