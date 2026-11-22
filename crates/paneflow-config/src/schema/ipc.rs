use serde_json::Value;

use super::PaneFlowConfig;

pub const IPC_KEY: &str = "ipc";
pub const IPC_SWITCH_KEYS: [&str; 2] = ["scripting", "orchestration"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum IpcSection {
    #[default]
    Absent,
    Set {
        scripting: Option<bool>,
        orchestration: Option<bool>,
    },
    Malformed {
        key: String,
        reason: String,
    },
}

impl IpcSection {
    pub fn from_value(value: Option<&Value>) -> Self {
        let Some(value) = value else {
            return Self::Absent;
        };
        let Some(fields) = value.as_object() else {
            return Self::Malformed {
                key: IPC_KEY.to_string(),
                reason: "expected an object".to_string(),
            };
        };
        let mut switches = [None, None];
        for (slot, name) in switches.iter_mut().zip(IPC_SWITCH_KEYS) {
            match fields.get(name) {
                None | Some(Value::Null) => {}
                Some(Value::Bool(on)) => *slot = Some(*on),
                Some(_) => {
                    return Self::Malformed {
                        key: format!("{IPC_KEY}.{name}"),
                        reason: "expected true or false".to_string(),
                    };
                }
            }
        }
        let [scripting, orchestration] = switches;
        Self::Set {
            scripting,
            orchestration,
        }
    }

    pub fn error(&self) -> Option<String> {
        match self {
            Self::Malformed { key, reason } => Some(format!(
                "invalid config key {key}: {reason}; every IPC gate stays closed for callers in a pane"
            )),
            Self::Absent | Self::Set { .. } => None,
        }
    }
}

impl PaneFlowConfig {
    pub fn ipc_section(&self) -> IpcSection {
        IpcSection::from_value(self.ipc.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_ipc_section_reads_its_two_switches() {
        assert_eq!(IpcSection::from_value(None), IpcSection::Absent);
        assert_eq!(
            IpcSection::from_value(Some(&json!({"scripting": true}))),
            IpcSection::Set {
                scripting: Some(true),
                orchestration: None
            }
        );
        assert_eq!(
            IpcSection::from_value(Some(&json!({"orchestration": false, "future": 1}))),
            IpcSection::Set {
                scripting: None,
                orchestration: Some(false)
            }
        );
    }

    #[test]
    fn a_malformed_ipc_section_names_the_key() {
        let wrong_type = IpcSection::from_value(Some(&json!({"scripting": "yes"})));
        assert_eq!(
            wrong_type.error().as_deref(),
            Some(
                "invalid config key ipc.scripting: expected true or false; every IPC gate stays closed for callers in a pane"
            )
        );
        let not_object = IpcSection::from_value(Some(&json!(true)));
        assert!(not_object
            .error()
            .is_some_and(|error| error.starts_with("invalid config key ipc:")));
    }

    #[tracing_test::traced_test]
    #[test]
    fn loading_a_config_with_a_malformed_ipc_section_warns_naming_the_key() {
        let config = crate::loader::try_parse_and_validate(
            r#"{"theme": "One Dark", "ipc": {"orchestration": 1}}"#,
        )
        .expect("the rest of the config still loads");
        assert_eq!(config.theme.as_deref(), Some("One Dark"));
        assert!(matches!(config.ipc_section(), IpcSection::Malformed { .. }));
        assert!(logs_contain("invalid config key ipc.orchestration"));
    }
}
