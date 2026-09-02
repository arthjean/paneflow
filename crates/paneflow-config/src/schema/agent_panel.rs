use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentPanelConfig {
    pub max_content_width: Option<u32>,
    pub thinking_display: Option<ThinkingDisplayMode>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_when_agent_waiting: Option<NotifyWhenAgentWaiting>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProfileConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum ThinkingDisplayMode {
    #[default]
    Auto,
    Preview,
    AlwaysExpanded,
    AlwaysCollapsed,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum NotifyWhenAgentWaiting {
    PrimaryScreen,
    AllScreens,
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
    pub const DEFAULT_MAX_CONTENT_WIDTH: u32 = 760;
    pub const MIN_CONTENT_WIDTH_PX: u32 = 320;
    pub const MAX_CONTENT_WIDTH_PX: u32 = 4000;

    pub fn resolved_thinking_display(&self) -> ThinkingDisplayMode {
        self.thinking_display.unwrap_or_default()
    }

    pub fn resolved_notify_when_agent_waiting(&self) -> NotifyWhenAgentWaiting {
        self.notify_when_agent_waiting.unwrap_or_default()
    }

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

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TelemetryConfig {
    pub enabled: Option<bool>,
}
