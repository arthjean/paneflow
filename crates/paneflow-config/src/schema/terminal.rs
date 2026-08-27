use super::config::{
    lenient_opt_bool, lenient_opt_cursor_blink, lenient_opt_cursor_shape, lenient_opt_f32,
    lenient_opt_string, lenient_opt_string_map, lenient_opt_usize,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

// Manual `Deserialize` for the terminal enums. A derived `Deserialize` hard-
// errors on an unrecognised variant; that error propagates up to
// `parse_and_validate` (loader.rs), which discards the ENTIRE user config and
// returns defaults. A typo (`"cursor_shape": "squiggle"`) would silently wipe
// the theme, shell, shortcuts, and agent settings. Instead fall back with a
// logged warning.
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
    /// to `[100, 100_000]`. Ghostty's own knob is a byte-count
    /// `scrollback-limit`, so Paneflow keeps a line-count budget in the
    /// config and converts it at spawn time.
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
    /// new terminal PTY. Per-surface `env`
    /// ([`crate::schema::SurfaceDefinition::env`]) is
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
