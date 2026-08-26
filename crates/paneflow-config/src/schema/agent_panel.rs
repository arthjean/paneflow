use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Agent-scoped configuration block (US-103).
///
/// Lives in its own struct so future stories (thinking display mode,
/// profile selector, OS notification gate, ...) can add fields without
/// bloating the top-level [`crate::schema::PaneFlowConfig`].
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
    /// Stable terminal-agent tag such as `"claude_code"` or `"codex"`.
    /// Stored as `String` so this config crate stays independent of the app's
    /// runtime launcher model.
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
/// `docs/ZED_AGENT_REFERENCE.md`. The default is
/// [`ThinkingDisplayMode::Auto`] -- last
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
