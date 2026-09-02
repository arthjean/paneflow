use std::path::Path;

use anyhow::Result;

use crate::detect::Presence;

pub mod claude_code;
pub mod codex;
pub mod gemini;
pub mod opencode;
mod support;

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
    vec![
        Box::new(claude_code::ClaudeCode::new()),
        Box::new(codex::Codex::new()),
        Box::new(gemini::Gemini::new()),
        Box::new(opencode::OpenCode::new()),
    ]
}
