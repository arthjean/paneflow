use super::{
    AgentPanelConfig, CommandDefinition, CursorBlinkConfig, CursorShapeConfig, TelemetryConfig,
    TerminalConfig,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaneFlowConfig {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shortcuts: HashMap<String, String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub default_shell: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub theme_mode: Option<String>,
    #[serde(default, deserialize_with = "lenient_commands")]
    pub commands: Vec<CommandDefinition>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_decorations: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub window_backdrop: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_terminal_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub windows_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub macos_chrome_material: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub unfocused_pane_opacity: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub reduce_motion: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub sidebar_show: SidebarShow,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub line_height: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub cell_width: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_family: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_fallbacks: Option<Vec<String>>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_size: Option<f32>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub font_weight: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub option_as_meta: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub shell_integration: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_stall_detection: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_stall_threshold_secs: Option<u64>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub review_prefill_delay_ms: Option<u64>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub submit_paste_delay_ms: Option<u64>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub external_editor: Option<String>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub claude_code_bypass_permissions: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_unrestricted: Option<bool>,
    #[serde(default, deserialize_with = "lenient_opt_bool")]
    pub ai_injection_fence: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub claude_code_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub codex_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub opencode_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub pi_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub hermes_agent_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub grok_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub amp_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub cursor_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub gemini_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub kiro_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub antigravity_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub copilot_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub codebuddy_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub factory_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub qoder_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub openclaw_button_visible: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub telemetry: Option<TelemetryConfig>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub terminal: Option<TerminalConfig>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub agent_panel: Option<AgentPanelConfig>,
    #[serde(
        default,
        skip_serializing_if = "HashMap::is_empty",
        deserialize_with = "lenient_value_or_default"
    )]
    pub tool_permissions: HashMap<String, ToolPermissionsEntry>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarShow {
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub branch: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub diffstat: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub pr: Option<bool>,
    #[serde(default, deserialize_with = "lenient_value_or_default")]
    pub indent_guide: Option<bool>,
}

impl SidebarShow {
    pub fn branch_enabled(&self) -> bool {
        self.branch.unwrap_or(false)
    }

    pub fn diffstat_enabled(&self) -> bool {
        self.diffstat.unwrap_or(false)
    }

    pub fn pr_enabled(&self) -> bool {
        self.pr.unwrap_or(false)
    }

    pub fn indent_guide_enabled(&self) -> bool {
        self.indent_guide.unwrap_or(false)
    }

    pub fn any_enabled(&self) -> bool {
        self.branch_enabled() || self.diffstat_enabled()
    }
}

impl PaneFlowConfig {
    pub const DEFAULT_AGENT_STALL_THRESHOLD_SECS: u64 = 60;
    pub const MIN_AGENT_STALL_THRESHOLD_SECS: u64 = 30;
    pub const MAX_AGENT_STALL_THRESHOLD_SECS: u64 = 86_400;

    pub const DEFAULT_REVIEW_PREFILL_DELAY_MS: u64 = 2000;
    pub const MIN_REVIEW_PREFILL_DELAY_MS: u64 = 250;
    pub const MAX_REVIEW_PREFILL_DELAY_MS: u64 = 10_000;

    pub const DEFAULT_UNFOCUSED_PANE_OPACITY: f32 = 0.7;
    pub const MIN_UNFOCUSED_PANE_OPACITY: f32 = 0.15;
    pub const MAX_UNFOCUSED_PANE_OPACITY: f32 = 1.0;

    pub const DEFAULT_SUBMIT_PASTE_DELAY_MS: u64 = 70;
    pub const MIN_SUBMIT_PASTE_DELAY_MS: u64 = 10;
    pub const MAX_SUBMIT_PASTE_DELAY_MS: u64 = 5_000;

    pub fn agent_stall_detection_enabled(&self) -> bool {
        self.agent_stall_detection.unwrap_or(true)
    }

    pub fn windows_terminal_material_enabled(&self) -> bool {
        cfg!(target_os = "windows") && self.windows_terminal_material.unwrap_or(false)
    }

    fn window_backdrop_disables_chrome_material(&self) -> bool {
        self.window_backdrop.as_deref().is_some_and(|value| {
            let value = value.trim();
            value.eq_ignore_ascii_case("opaque") || value.eq_ignore_ascii_case("off")
        })
    }

    pub fn macos_chrome_material_enabled(&self) -> bool {
        !self.window_backdrop_disables_chrome_material()
            && !self
                .window_backdrop
                .as_deref()
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("transparent"))
            && self.macos_chrome_material.unwrap_or(true)
    }

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

    pub fn reduce_motion_enabled(&self) -> bool {
        self.reduce_motion.unwrap_or(false)
    }

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

    pub fn ai_unrestricted_enabled(&self) -> bool {
        self.ai_unrestricted.unwrap_or(false)
    }

    pub fn ai_injection_fence_enabled(&self) -> bool {
        self.ai_injection_fence.unwrap_or(true)
    }
}

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ToolPermissionsEntry {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub always_deny: Vec<String>,
}
