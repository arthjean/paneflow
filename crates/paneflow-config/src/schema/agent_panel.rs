use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AgentPanelConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_when_agent_waiting: Option<NotifyWhenAgentWaiting>,
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

impl AgentPanelConfig {
    pub fn resolved_notify_when_agent_waiting(&self) -> NotifyWhenAgentWaiting {
        self.notify_when_agent_waiting.unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TelemetryConfig {
    pub enabled: Option<bool>,
}
