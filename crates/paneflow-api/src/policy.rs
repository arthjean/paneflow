use std::sync::atomic::{AtomicBool, Ordering};

use crate::principal::Principal;

pub const SCRIPTING_ENV: &str = "PANEFLOW_IPC_SCRIPTING";
pub const ORCHESTRATION_ENV: &str = "PANEFLOW_IPC_ORCHESTRATION";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Switches {
    pub scripting: bool,
    pub orchestration: bool,
}

impl Switches {
    pub const CLOSED: Self = Self {
        scripting: false,
        orchestration: false,
    };

    pub const OPEN: Self = Self {
        scripting: true,
        orchestration: true,
    };

    pub fn from_environment() -> Self {
        Self::from_values(
            std::env::var(SCRIPTING_ENV).ok().as_deref(),
            std::env::var(ORCHESTRATION_ENV).ok().as_deref(),
        )
    }

    pub fn from_values(scripting: Option<&str>, orchestration: Option<&str>) -> Self {
        let scripting = scripting == Some("1");
        Self {
            scripting,
            orchestration: scripting || orchestration == Some("1"),
        }
    }

    pub const fn from_keys(scripting: Option<bool>, orchestration: Option<bool>) -> Self {
        let scripting = matches!(scripting, Some(true));
        Self {
            scripting,
            orchestration: scripting || matches!(orchestration, Some(true)),
        }
    }

    const fn any(self) -> bool {
        self.scripting || self.orchestration
    }

    const fn or(self, other: Self) -> Self {
        Self {
            scripting: self.scripting || other.scripting,
            orchestration: self.orchestration || other.orchestration,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Absent,
    Set(Switches),
    Malformed {
        key: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    pub section: Section,
    pub ai_unrestricted: bool,
    pub environment: Switches,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Gates {
    pub scripting: bool,
    pub orchestration: bool,
    pub reach: bool,
    pub browser: bool,
    pub spawn_confined: bool,
    pub stale_grants: bool,
    pub from_deprecated_environment: bool,
}

impl Gates {
    pub const OPEN: Self = Self::of(Switches::OPEN);
    pub const CLOSED: Self = Self::of(Switches::CLOSED);

    const fn of(switches: Switches) -> Self {
        Self {
            scripting: switches.scripting,
            orchestration: switches.orchestration,
            reach: switches.orchestration,
            browser: switches.scripting,
            spawn_confined: false,
            stale_grants: false,
            from_deprecated_environment: false,
        }
    }
}

impl Policy {
    pub fn gates_for(&self, principal: &Principal) -> Gates {
        match principal {
            Principal::Window | Principal::Worker => Gates::OPEN,
            Principal::Unverified { .. } => Gates::CLOSED,
            Principal::External => self.external_gates(),
            Principal::Pane { .. } => self.pane_gates(),
        }
    }

    fn external_gates(&self) -> Gates {
        if self.ai_unrestricted {
            return Gates::OPEN;
        }
        let section = match self.section {
            Section::Set(switches) => switches,
            Section::Absent | Section::Malformed { .. } => Switches::CLOSED,
        };
        Gates::of(section.or(self.environment))
    }

    fn pane_gates(&self) -> Gates {
        match self.section {
            Section::Malformed { .. } => Gates::CLOSED,
            _ if self.ai_unrestricted => Gates::OPEN,
            Section::Set(switches) => Gates::of(switches),
            Section::Absent => Gates {
                from_deprecated_environment: self.environment.any(),
                ..Gates::of(self.environment)
            },
        }
    }

    pub fn replacement_key(&self) -> &'static str {
        if self.environment.scripting {
            "ipc.scripting"
        } else {
            "ipc.orchestration"
        }
    }
}

static DEPRECATION_ANNOUNCED: AtomicBool = AtomicBool::new(false);

pub fn deprecation_notice(policy: &Policy, gates: &Gates) -> Option<String> {
    if !gates.from_deprecated_environment || DEPRECATION_ANNOUNCED.swap(true, Ordering::AcqRel) {
        return None;
    }
    let variable = if policy.environment.scripting {
        SCRIPTING_ENV
    } else {
        ORCHESTRATION_ENV
    };
    let key = policy.replacement_key();
    let field = key.trim_start_matches("ipc.");
    Some(format!(
        "{variable} opened an IPC gate for a caller in a pane; this works until the next release only. \
         Set {key} in paneflow.json instead: \"ipc\": {{\"{field}\": true}}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane() -> Principal {
        Principal::Pane {
            session: "s-1".into(),
            workspace: Some("w-1".into()),
        }
    }

    fn policy(section: Section, ai_unrestricted: bool, environment: Switches) -> Policy {
        Policy {
            section,
            ai_unrestricted,
            environment,
        }
    }

    #[test]
    fn the_ipc_keys_open_the_gates_of_external_and_pane_callers() {
        let scripting = policy(
            Section::Set(Switches::from_keys(Some(true), None)),
            false,
            Switches::CLOSED,
        );
        for caller in [Principal::External, pane()] {
            let gates = scripting.gates_for(&caller);
            assert!(gates.scripting && gates.orchestration, "{caller:?}");
            assert!(!gates.from_deprecated_environment);
        }
        let orchestration = policy(
            Section::Set(Switches::from_keys(None, Some(true))),
            false,
            Switches::CLOSED,
        );
        for caller in [Principal::External, pane()] {
            let gates = orchestration.gates_for(&caller);
            assert!(!gates.scripting && gates.orchestration, "{caller:?}");
        }
    }

    #[test]
    fn the_environment_opens_an_external_gate_and_a_pane_gate_only_without_a_section() {
        let variable = Switches::from_values(Some("1"), None);
        let absent = policy(Section::Absent, false, variable);
        assert!(absent.gates_for(&Principal::External).scripting);
        let pane_gates = absent.gates_for(&pane());
        assert!(pane_gates.scripting && pane_gates.from_deprecated_environment);
        let section = policy(Section::Set(Switches::CLOSED), false, variable);
        assert!(section.gates_for(&Principal::External).scripting);
        assert_eq!(section.gates_for(&pane()), Gates::CLOSED);
    }

    #[test]
    fn ai_unrestricted_opens_both_gates_and_a_malformed_section_closes_a_pane() {
        let unrestricted = policy(Section::Absent, true, Switches::CLOSED);
        assert_eq!(unrestricted.gates_for(&pane()), Gates::OPEN);
        assert_eq!(unrestricted.gates_for(&Principal::External), Gates::OPEN);
        let malformed = policy(
            Section::Malformed {
                key: "ipc.scripting".into(),
            },
            true,
            Switches::OPEN,
        );
        assert_eq!(malformed.gates_for(&pane()), Gates::CLOSED);
    }

    #[test]
    fn paneflow_itself_is_never_gated_and_an_unverified_caller_always_is() {
        let closed = Policy::default();
        assert_eq!(closed.gates_for(&Principal::Window), Gates::OPEN);
        assert_eq!(closed.gates_for(&Principal::Worker), Gates::OPEN);
        let open = policy(Section::Set(Switches::OPEN), true, Switches::OPEN);
        assert_eq!(
            open.gates_for(&Principal::Unverified {
                reason: "gone".into()
            }),
            Gates::CLOSED
        );
    }

    #[test]
    fn only_the_documented_value_of_a_variable_opens_it() {
        for value in [None, Some(""), Some("0"), Some("true")] {
            assert_eq!(Switches::from_values(value, value), Switches::CLOSED);
        }
        assert_eq!(Switches::from_values(Some("1"), None), Switches::OPEN);
        assert_eq!(
            Switches::from_values(None, Some("1")),
            Switches {
                scripting: false,
                orchestration: true
            }
        );
    }

    #[test]
    fn the_deprecation_is_announced_once_and_names_the_replacing_key() {
        let absent = policy(
            Section::Absent,
            false,
            Switches::from_values(Some("1"), None),
        );
        let gates = absent.gates_for(&pane());
        let notice = deprecation_notice(&absent, &gates);
        let again = deprecation_notice(&absent, &gates);
        let first = notice.or(again.clone()).expect("one notice is given");
        assert!(first.contains("Set ipc.scripting"), "{first}");
        assert!(first.contains("\"ipc\": {\"scripting\": true}"), "{first}");
        assert!(first.contains(SCRIPTING_ENV), "{first}");
        assert!(deprecation_notice(&absent, &gates).is_none());
        assert!(deprecation_notice(&absent, &Gates::OPEN).is_none());
    }
}
