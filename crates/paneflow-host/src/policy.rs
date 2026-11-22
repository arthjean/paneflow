use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use paneflow_api::policy::{Policy, Section, Switches};
use paneflow_config::schema::{IpcSection, PaneFlowConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Default)]
struct Cached {
    stamp: Option<Option<Stamp>>,
    policy: Policy,
}

#[derive(Debug)]
pub struct PolicySource {
    path: PathBuf,
    environment: Switches,
    cached: Mutex<Cached>,
    #[cfg(test)]
    pinned: Mutex<Option<Policy>>,
}

pub fn section_of(config: &PaneFlowConfig) -> Section {
    match config.ipc_section() {
        IpcSection::Absent => Section::Absent,
        IpcSection::Set {
            scripting,
            orchestration,
        } => Section::Set(Switches::from_keys(scripting, orchestration)),
        IpcSection::Malformed { key, .. } => Section::Malformed { key },
    }
}

pub fn policy_from_config(config: &PaneFlowConfig, environment: Switches) -> Policy {
    Policy {
        section: section_of(config),
        ai_unrestricted: config.ai_unrestricted_enabled(),
        environment,
    }
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    std::fs::metadata(path).ok().map(|metadata| Stamp {
        modified: metadata.modified().ok(),
        len: metadata.len(),
    })
}

fn read_policy(path: &Path, environment: Switches) -> Policy {
    let contents = match paneflow_config::loader::read_config_string(path) {
        Ok(Some(contents)) => contents,
        Ok(None) => return policy_from_config(&PaneFlowConfig::default(), environment),
        Err(error) => {
            log::warn!("IPC policy: {error}; every gate stays closed for callers in a pane");
            return unreadable(environment);
        }
    };
    match paneflow_config::loader::try_parse_and_validate(&contents) {
        Ok(config) => policy_from_config(&config, environment),
        Err(error) => {
            log::warn!(
                "IPC policy: invalid config {}: {error}; every gate stays closed for callers in a pane",
                path.display()
            );
            unreadable(environment)
        }
    }
}

fn unreadable(environment: Switches) -> Policy {
    Policy {
        section: Section::Malformed {
            key: "paneflow.json".to_string(),
        },
        ai_unrestricted: false,
        environment,
    }
}

impl PolicySource {
    pub fn new(path: PathBuf, environment: Switches) -> Self {
        Self {
            path,
            environment,
            cached: Mutex::new(Cached::default()),
            #[cfg(test)]
            pinned: Mutex::new(None),
        }
    }

    pub fn for_home(home: &Path) -> Self {
        Self::new(
            paneflow_home::config_path_in(home),
            Switches::from_environment(),
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn current(&self) -> Policy {
        #[cfg(test)]
        if let Some(pinned) = self
            .pinned
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
        {
            return pinned;
        }
        let stamp = stamp_of(&self.path);
        let mut cached = self.cached.lock().unwrap_or_else(PoisonError::into_inner);
        if cached.stamp != Some(stamp) {
            cached.policy = read_policy(&self.path, self.environment);
            cached.stamp = Some(stamp);
        }
        cached.policy.clone()
    }

    #[cfg(test)]
    pub fn pin(&self, policy: Option<Policy>) {
        *self.pinned.lock().unwrap_or_else(PoisonError::into_inner) = policy;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::Principal;

    fn pane() -> Principal {
        Principal::Pane {
            session: "s-1".into(),
            workspace: None,
        }
    }

    fn write(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(1);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(later).unwrap();
    }

    #[test]
    fn a_changed_ipc_section_applies_to_the_next_request() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("paneflow.json");
        let source = PolicySource::new(path.clone(), Switches::CLOSED);
        assert!(!source.current().gates_for(&pane()).scripting);
        write(&path, r#"{"ipc": {"scripting": true}}"#);
        assert!(source.current().gates_for(&pane()).scripting);
        assert!(source.current().gates_for(&Principal::External).scripting);
        write(
            &path,
            r#"{"ipc": {"scripting": false, "orchestration": true}}"#,
        );
        let gates = source.current().gates_for(&pane());
        assert!(!gates.scripting && gates.orchestration);
    }

    #[test]
    fn an_unparsable_config_closes_every_gate_of_a_pane_and_keeps_the_environment_for_others() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("paneflow.json");
        write(&path, "{ not json");
        let source = PolicySource::new(path, Switches::OPEN);
        let policy = source.current();
        assert_eq!(
            policy.gates_for(&pane()),
            paneflow_api::policy::Gates::CLOSED
        );
        assert!(policy.gates_for(&Principal::External).scripting);
    }

    #[test]
    fn ai_unrestricted_from_the_file_opens_both_gates() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("paneflow.json");
        write(&path, r#"{"ai_unrestricted": true}"#);
        let source = PolicySource::new(path, Switches::CLOSED);
        assert_eq!(
            source.current().gates_for(&pane()),
            paneflow_api::policy::Gates::OPEN
        );
    }
}
