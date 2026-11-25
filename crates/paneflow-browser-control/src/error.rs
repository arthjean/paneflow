use paneflow_api::methods::browser::{
    ERR_BROWSER_FAILED, ERR_DENIED, ERR_NOT_INSTALLED, ERR_NOT_OWNER, ERR_STALE_REFERENCE,
    ERR_TIMED_OUT, ERR_UNKNOWN_TAB,
};
use paneflow_browser_proto::{CommandError, NavigationRefusal, TabId};

pub const INVALID_PARAMS: i64 = -32602;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlError {
    pub code: i64,
    pub reason: &'static str,
    pub message: String,
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ControlError {}

impl ControlError {
    pub fn new(code: i64, reason: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            reason,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_PARAMS, "invalid_params", message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ERR_BROWSER_FAILED, "failed", message)
    }

    pub fn not_owner(message: impl Into<String>) -> Self {
        Self::new(ERR_NOT_OWNER, "not_owner", message)
    }

    pub fn stale(message: impl Into<String>) -> Self {
        Self::new(ERR_STALE_REFERENCE, "stale_reference", message)
    }

    pub fn timed_out(message: impl Into<String>) -> Self {
        Self::new(ERR_TIMED_OUT, "timeout", message)
    }

    pub fn denied(message: impl Into<String>) -> Self {
        Self::new(ERR_DENIED, "denied", message)
    }

    pub fn not_installed(message: impl Into<String>) -> Self {
        Self::new(ERR_NOT_INSTALLED, "not_installed", message)
    }

    pub fn unknown_tab(tab: TabId) -> Self {
        Self::new(
            ERR_UNKNOWN_TAB,
            "unknown_tab",
            format!(
                "there is no browser tab {}; browser.list names the open tabs",
                tab.0
            ),
        )
    }

    pub fn from_refusal(refusal: &NavigationRefusal) -> Self {
        match refusal {
            NavigationRefusal::OriginDenied { .. } => {
                Self::denied(format!("navigation refused: {refusal}"))
            }
            _ => Self::new(
                ERR_BROWSER_FAILED,
                "navigation_refused",
                format!("navigation refused: {refusal}"),
            ),
        }
    }

    pub fn from_command(error: CommandError) -> Self {
        match &error {
            CommandError::TabClosed { tab } => Self::unknown_tab(*tab),
            CommandError::Timeout { .. } => Self::timed_out(error.to_string()),
            CommandError::NavigationRefused { refusal } => Self::from_refusal(refusal),
            CommandError::HostUnavailable { reason } if reason.contains("not installed") => {
                Self::not_installed(error.to_string())
            }
            _ => Self::failed(error.to_string()),
        }
    }
}
