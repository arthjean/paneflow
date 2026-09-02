use super::config::{
    lenient_opt_bool, lenient_opt_cursor_blink, lenient_opt_cursor_shape, lenient_opt_f32,
    lenient_opt_string, lenient_opt_string_map, lenient_opt_usize,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const APPLE_SYSTEM_BLUE_HEX: &str = "#007AFF";

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorShapeConfig {
    Vintage,
    #[default]
    Block,
    Beam,
    Underline,
    DoubleUnderline,
    Hollow,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CursorBlinkConfig {
    On,
    Off,
    #[default]
    TerminalControlled,
}

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ligatures: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub integrated_glyphs: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub color_emoji: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_string")]
    pub cursor_color: Option<String>,
    #[serde(default, deserialize_with = "lenient_opt_usize")]
    pub scrollback_lines: Option<usize>,
    #[serde(default, deserialize_with = "lenient_opt_cursor_shape")]
    pub cursor_shape: Option<CursorShapeConfig>,
    #[serde(default, deserialize_with = "lenient_opt_cursor_blink")]
    pub cursor_blink: Option<CursorBlinkConfig>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_opt_string_map"
    )]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, deserialize_with = "lenient_opt_f32")]
    pub scroll_multiplier: Option<f32>,
}

impl TerminalConfig {
    pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
    pub const AGENT_SCROLLBACK_LINES: usize = 10_000;
    pub const REVIEW_SCROLLBACK_LINES: usize = 2_000;
    pub const CACHED_SCROLLBACK_LINES: usize = 1_000;
    pub const MIN_SCROLLBACK_LINES: usize = 100;
    pub const MAX_SCROLLBACK_LINES: usize = 100_000;

    pub const DEFAULT_SCROLL_MULTIPLIER: f32 = 1.0;
    pub const MIN_SCROLL_MULTIPLIER: f32 = 0.1;
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

    pub fn resolved_scroll_multiplier(&self) -> f32 {
        let raw = self
            .scroll_multiplier
            .unwrap_or(Self::DEFAULT_SCROLL_MULTIPLIER);
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

    pub fn resolved_scrollback_lines_for_profile(&self, profile: TerminalSurfaceProfile) -> usize {
        let base = self.resolved_scrollback_lines();
        profile.scrollback_cap().map_or(base, |cap| base.min(cap))
    }
}
