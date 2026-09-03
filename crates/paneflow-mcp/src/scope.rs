use std::env::VarError;
use std::fmt;

use serde_json::{json, Map, Value};

const MCP_SCOPE_ENV: &str = "PANEFLOW_MCP_SCOPE";
const WORKSPACE_ENV: &str = "PANEFLOW_WORKSPACE_ID";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeScope {
    All,
    Workspace(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeConfigError {
    MissingWorkspaceId,
    InvalidWorkspaceId(String),
    InvalidScope(String),
    NonUnicodeValue(&'static str),
}

impl fmt::Display for ScopeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingWorkspaceId => write!(
                f,
                "{WORKSPACE_ENV} is missing; launch from a Paneflow pane or set {MCP_SCOPE_ENV}=all explicitly"
            ),
            Self::InvalidWorkspaceId(value) => {
                write!(f, "{WORKSPACE_ENV} must be a non-negative integer, got '{value}'")
            }
            Self::InvalidScope(value) => write!(
                f,
                "{MCP_SCOPE_ENV} must be 'workspace' or 'all', got '{value}'"
            ),
            Self::NonUnicodeValue(name) => write!(f, "{name} contains non-Unicode data"),
        }
    }
}

impl std::error::Error for ScopeConfigError {}

impl BridgeScope {
    pub fn from_env() -> Result<Self, ScopeConfigError> {
        let scope = read_env(MCP_SCOPE_ENV)?;
        let workspace = read_env(WORKSPACE_ENV)?;
        Self::from_values(scope.as_deref(), workspace.as_deref())
    }

    fn from_values(scope: Option<&str>, workspace: Option<&str>) -> Result<Self, ScopeConfigError> {
        match scope {
            Some(value) if value.eq_ignore_ascii_case("all") => Ok(Self::All),
            None => {
                let raw = workspace.ok_or(ScopeConfigError::MissingWorkspaceId)?;
                raw.parse::<u64>()
                    .map(Self::Workspace)
                    .map_err(|_| ScopeConfigError::InvalidWorkspaceId(raw.to_string()))
            }
            Some(value) if value.eq_ignore_ascii_case("workspace") => {
                let raw = workspace.ok_or(ScopeConfigError::MissingWorkspaceId)?;
                raw.parse::<u64>()
                    .map(Self::Workspace)
                    .map_err(|_| ScopeConfigError::InvalidWorkspaceId(raw.to_string()))
            }
            Some(value) => Err(ScopeConfigError::InvalidScope(value.to_string())),
        }
    }

    pub(crate) fn as_json(self) -> Value {
        match self {
            Self::All => json!({ "mode": "all" }),
            Self::Workspace(workspace_id) => {
                json!({ "mode": "workspace", "workspace_id": workspace_id })
            }
        }
    }

    pub(crate) fn attr(self) -> String {
        match self {
            Self::All => "scope=\"all\"".to_string(),
            Self::Workspace(workspace_id) => {
                format!("scope=\"workspace:{workspace_id}\"")
            }
        }
    }

    pub(crate) fn workspace_id(self) -> Option<u64> {
        match self {
            Self::All => None,
            Self::Workspace(workspace_id) => Some(workspace_id),
        }
    }

    pub(crate) fn insert_ipc_param(self, params: &mut Map<String, Value>) {
        if let Self::Workspace(workspace_id) = self {
            params.insert("workspace_id".into(), json!(workspace_id));
        }
    }

    pub(crate) fn ipc_params(self) -> Value {
        let mut params = Map::new();
        self.insert_ipc_param(&mut params);
        Value::Object(params)
    }
}

fn read_env(name: &'static str) -> Result<Option<String>, ScopeConfigError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(VarError::NotUnicode(_)) => Err(ScopeConfigError::NonUnicodeValue(name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_all_is_the_only_global_path() {
        assert_eq!(
            BridgeScope::from_values(Some("all"), None),
            Ok(BridgeScope::All)
        );
        assert_eq!(
            BridgeScope::from_values(Some("GLOBAL"), Some("7")),
            Err(ScopeConfigError::InvalidScope("GLOBAL".into()))
        );
    }

    #[test]
    fn default_scope_requires_a_stable_workspace_id() {
        assert_eq!(
            BridgeScope::from_values(None, Some("42")),
            Ok(BridgeScope::Workspace(42))
        );
        assert_eq!(
            BridgeScope::from_values(Some("workspace"), Some("42")),
            Ok(BridgeScope::Workspace(42))
        );
        assert_eq!(
            BridgeScope::from_values(None, None),
            Err(ScopeConfigError::MissingWorkspaceId)
        );
    }

    #[test]
    fn malformed_scope_configuration_fails_closed() {
        assert_eq!(
            BridgeScope::from_values(None, Some("not-an-id")),
            Err(ScopeConfigError::InvalidWorkspaceId("not-an-id".into()))
        );
        assert_eq!(
            BridgeScope::from_values(Some("everything"), Some("42")),
            Err(ScopeConfigError::InvalidScope("everything".into()))
        );
    }
}
