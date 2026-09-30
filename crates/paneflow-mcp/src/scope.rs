use std::env::VarError;
use std::fmt;

use serde_json::{json, Map, Value};

use crate::output::sanitize_attr;

const MCP_SCOPE_ENV: &str = "PANEFLOW_MCP_SCOPE";
const SESSION_ENV: &str = "PANEFLOW_SESSION_ID";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeScope {
    All,
    Session(String),
    Unavailable(ScopeConfigError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeConfigError {
    MissingSession,
    InvalidScope(String),
    NonUnicodeValue(&'static str),
}

impl fmt::Display for ScopeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSession => write!(
                f,
                "{SESSION_ENV} is missing, so the bridge cannot tell which workspace it may read; \
                 launch the agent from a Paneflow pane or set {MCP_SCOPE_ENV}=all explicitly"
            ),
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
    pub fn from_env() -> Self {
        match (read_env(MCP_SCOPE_ENV), read_env(SESSION_ENV)) {
            (Ok(scope), Ok(session)) => Self::from_values(scope.as_deref(), session.as_deref()),
            (Err(error), _) | (_, Err(error)) => Self::Unavailable(error),
        }
    }

    fn from_values(scope: Option<&str>, session: Option<&str>) -> Self {
        match scope {
            Some(value) if value.eq_ignore_ascii_case("all") => Self::All,
            None => Self::session(session),
            Some(value) if value.eq_ignore_ascii_case("workspace") => Self::session(session),
            Some(value) => Self::Unavailable(ScopeConfigError::InvalidScope(value.to_string())),
        }
    }

    fn session(session: Option<&str>) -> Self {
        match session.map(str::trim).filter(|session| !session.is_empty()) {
            Some(session) => Self::Session(session.to_string()),
            None => Self::Unavailable(ScopeConfigError::MissingSession),
        }
    }

    pub(crate) fn error(&self) -> Option<&ScopeConfigError> {
        match self {
            Self::Unavailable(error) => Some(error),
            Self::All | Self::Session(_) => None,
        }
    }

    pub(crate) fn as_json(&self) -> Value {
        match self {
            Self::All => json!({ "mode": "all" }),
            Self::Session(session) => json!({ "mode": "session", "session": session }),
            Self::Unavailable(error) => {
                json!({ "mode": "unavailable", "error": error.to_string() })
            }
        }
    }

    pub(crate) fn attr(&self) -> String {
        match self {
            Self::All => "scope=\"all\"".to_string(),
            Self::Session(session) => format!("scope=\"session:{}\"", sanitize_attr(session)),
            Self::Unavailable(_) => "scope=\"unavailable\"".to_string(),
        }
    }

    pub(crate) fn insert_ipc_param(&self, params: &mut Map<String, Value>) {
        if let Self::Session(session) = self {
            params.insert("scope_session".into(), json!(session));
        }
    }

    pub(crate) fn ipc_params(&self) -> Value {
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
    fn the_session_of_the_pane_is_the_default_scope() {
        assert_eq!(
            BridgeScope::from_values(None, Some("0a9e5266")),
            BridgeScope::Session("0a9e5266".into())
        );
        assert_eq!(
            BridgeScope::from_values(Some("workspace"), Some(" 0a9e5266 ")),
            BridgeScope::Session("0a9e5266".into())
        );
        assert_eq!(
            BridgeScope::ipc_params(&BridgeScope::Session("0a9e5266".into())),
            json!({ "scope_session": "0a9e5266" })
        );
    }

    #[test]
    fn a_bridge_without_a_session_is_refused_unless_all_is_explicit() {
        for session in [None, Some(""), Some("  ")] {
            assert_eq!(
                BridgeScope::from_values(None, session),
                BridgeScope::Unavailable(ScopeConfigError::MissingSession)
            );
        }
        assert_eq!(
            BridgeScope::from_values(Some("all"), None),
            BridgeScope::All
        );
        assert_eq!(BridgeScope::ipc_params(&BridgeScope::All), json!({}));
    }

    #[test]
    fn malformed_scope_configuration_fails_closed() {
        assert_eq!(
            BridgeScope::from_values(Some("GLOBAL"), Some("0a9e5266")),
            BridgeScope::Unavailable(ScopeConfigError::InvalidScope("GLOBAL".into()))
        );
        assert!(BridgeScope::from_values(Some("everything"), None)
            .error()
            .is_some());
    }
}
