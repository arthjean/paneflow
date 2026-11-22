use crate::methods::system::WhoAmIResult;

pub const REASON_PRINCIPAL_REFUSED: &str = "principal_refused";

pub const ERR_PRINCIPAL_REFUSED: i64 = -32001;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    Window,
    Worker,
    Pane {
        session: String,
        workspace: Option<String>,
    },
    External,
    Unverified {
        reason: String,
    },
}

impl Principal {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Worker => "worker",
            Self::Pane { .. } => "pane",
            Self::External => "external",
            Self::Unverified { .. } => "unverified",
        }
    }

    pub const fn is_pane(&self) -> bool {
        matches!(self, Self::Pane { .. })
    }

    pub fn whoami(&self) -> WhoAmIResult {
        let (session, workspace) = match self {
            Self::Pane { session, workspace } => (Some(session.clone()), workspace.clone()),
            _ => (None, None),
        };
        WhoAmIResult {
            principal: self.kind().to_string(),
            session,
            workspace,
            reason: match self {
                Self::Unverified { reason } => Some(reason.clone()),
                _ => None,
            },
        }
    }

    pub fn whoami_value(&self) -> serde_json::Value {
        serde_json::to_value(self.whoami()).unwrap_or(serde_json::Value::Null)
    }

    pub fn refusal(&self, method: &str) -> (i64, String, serde_json::Value) {
        (
            ERR_PRINCIPAL_REFUSED,
            format!(
                "{method} does not answer a caller identified as {}",
                self.kind()
            ),
            serde_json::json!({"reason": REASON_PRINCIPAL_REFUSED, "principal": self.kind()}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whoami_names_the_kind_and_only_a_pane_carries_its_session_and_workspace() {
        let pane = Principal::Pane {
            session: "s-1".into(),
            workspace: Some("w-1".into()),
        };
        assert_eq!(
            pane.whoami_value(),
            serde_json::json!({"principal": "pane", "session": "s-1", "workspace": "w-1"})
        );
        assert_eq!(
            Principal::External.whoami_value(),
            serde_json::json!({"principal": "external"})
        );
        assert_eq!(
            Principal::Unverified {
                reason: "gone".into()
            }
            .whoami_value(),
            serde_json::json!({"principal": "unverified", "reason": "gone"})
        );
    }
}
