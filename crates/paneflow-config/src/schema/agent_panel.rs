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
            "PrimaryScreen" | "AllScreens" => Ok(Self::PrimaryScreen),
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
pub struct AgentsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_conversations: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_preassign_session_id: Option<bool>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub update_methods: std::collections::BTreeMap<String, String>,
}

impl AgentsConfig {
    pub fn resolved_restore_conversations(&self) -> bool {
        self.restore_conversations.unwrap_or(true)
    }

    pub fn resolved_claude_preassign_session_id(&self) -> bool {
        self.claude_preassign_session_id.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UsageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_limits: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_agents: Option<Vec<String>>,
}

impl UsageConfig {
    pub fn plan_limits_enabled(&self) -> bool {
        self.plan_limits.unwrap_or(true)
    }

    pub fn shows_in_sidebar(&self, runtime_slug: &str) -> bool {
        self.sidebar_agents
            .as_ref()
            .is_none_or(|agents| agents.iter().any(|agent| agent == runtime_slug))
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TelemetryConfig {
    pub enabled: Option<bool>,
}

#[cfg(test)]
mod usage_tests {
    use super::UsageConfig;

    #[test]
    fn the_sidebar_shows_every_agent_until_a_list_names_some() {
        let unset = UsageConfig::default();
        assert!(unset.shows_in_sidebar("claude-code"));
        assert!(unset.shows_in_sidebar("codex"));

        let claude_only: UsageConfig =
            serde_json::from_str(r#"{"sidebar_agents": ["claude-code"]}"#).unwrap();
        assert!(claude_only.shows_in_sidebar("claude-code"));
        assert!(!claude_only.shows_in_sidebar("codex"));
        assert!(claude_only.plan_limits_enabled());

        let none: UsageConfig = serde_json::from_str(r#"{"sidebar_agents": []}"#).unwrap();
        assert!(!none.shows_in_sidebar("claude-code"));
        assert_eq!(
            serde_json::to_value(&unset).unwrap(),
            serde_json::json!({}),
            "an unset list is not written back"
        );
    }
}
