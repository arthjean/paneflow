use std::path::Path;

use anyhow::Result;

use crate::detect::Presence;
use paneflow_agent_config::{RuntimeMcpConfig, RUNTIMES};

pub mod claude_code;
pub mod codex;
pub mod fx;
pub mod gemini;
pub mod opencode;
mod support;

#[cfg(test)]
pub(crate) use support::CODEX_BRIDGE_ENV_VARS;

#[cfg(test)]
pub(crate) mod testutil;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    Updated,
    AlreadyCurrent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UninstallOutcome {
    Removed,
    NothingToRemove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusOutcome {
    Installed {
        path: String,
    },
    StalePath {
        found: String,
        expected: String,
    },
    NeedsRepair {
        path: Option<String>,
        reason: String,
    },
    DisabledByUser {
        path: String,
    },
    NotInstalled,
}

pub trait AgentConfigWriter {
    fn id(&self) -> &'static str;

    fn label(&self) -> &'static str;

    fn presence(&self) -> Presence;

    fn install(&self, bridge_path: &Path) -> Result<InstallOutcome>;

    fn uninstall(&self) -> Result<UninstallOutcome>;

    fn status(&self, bridge_path: Option<&Path>) -> Result<StatusOutcome>;
}

#[must_use]
pub fn default_writers() -> Vec<Box<dyn AgentConfigWriter>> {
    RUNTIMES
        .iter()
        .filter_map(|runtime| runtime.integration.mcp_config)
        .map(writer_for)
        .collect()
}

#[must_use]
pub fn writer_for(config: RuntimeMcpConfig) -> Box<dyn AgentConfigWriter> {
    match config {
        RuntimeMcpConfig::Claude => Box::new(claude_code::ClaudeCode::new()),
        RuntimeMcpConfig::Codex => Box::new(codex::Codex::new()),
        RuntimeMcpConfig::Gemini => Box::new(gemini::Gemini::new()),
        RuntimeMcpConfig::OpenCode => Box::new(opencode::OpenCode::new()),
        RuntimeMcpConfig::Fx => Box::new(fx::Fx::new()),
    }
}
