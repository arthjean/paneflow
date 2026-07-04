//! Structured, programmatic API over the agent writers (EP-004 US-012).
//!
//! [`crate::cli`] formats human output for `paneflow mcp …`; this module
//! returns the same orchestration as plain data so a GUI (the Settings
//! button) can render a per-agent recap and derive a single
//! [`OverallState`] for its button label - without parsing stdout.
//!
//! These functions perform blocking filesystem / process I/O; callers on a
//! UI thread MUST run them on a background executor (the Settings button
//! uses `smol::unblock`).

use std::path::Path;

use crate::agents::{self, AgentConfigWriter, InstallOutcome, StatusOutcome, UninstallOutcome};

/// Per-agent outcome of an install pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    Installed,
    Updated,
    AlreadyCurrent,
    SkippedAbsent,
    Error(String),
}

/// Per-agent read-only state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusKind {
    NotDetected,
    Installed {
        path: String,
    },
    Stale {
        found: String,
        expected: String,
    },
    NeedsRepair {
        path: Option<String>,
        reason: String,
    },
    NotInstalled,
    Error(String),
}

/// Per-agent outcome of an uninstall pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UninstallKind {
    Removed,
    NothingToRemove,
    NotDetected,
    Error(String),
}

/// One agent's result, carrying its id + label for display.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentResult<K> {
    pub id: String,
    pub label: String,
    pub kind: K,
}

/// Ergonomic aliases for GUI/state code.
pub type InstallReport = AgentResult<InstallKind>;
pub type StatusReport = AgentResult<StatusKind>;
pub type UninstallReport = AgentResult<UninstallKind>;

/// Aggregate state used to pick the Settings button's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverallState {
    /// No supported agent is installed on this machine.
    NoAgents,
    /// At least one detected agent has no `paneflow` entry yet.
    NeedsInstall,
    /// A detected agent points at a stale bridge path (post-update).
    NeedsRepair,
    /// Every detected agent is installed and current.
    AllInstalled,
}

// ---------------------------------------------------------------------------
// Install
// ---------------------------------------------------------------------------

/// Register the bridge with every detected agent. `bridge` is the resolved
/// stable path (`runtime_paths::bridge_binary_path()`), which must already
/// exist on disk. `Err` is a whole-operation refusal (bridge missing / data
/// dir unresolved) that wrote nothing; `Ok` carries one entry per agent.
pub fn install_all(bridge: Option<&Path>) -> Result<Vec<AgentResult<InstallKind>>, String> {
    install_with(bridge, &agents::default_writers())
}

pub(crate) fn install_with(
    bridge: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
) -> Result<Vec<AgentResult<InstallKind>>, String> {
    // US-038: resolve each writer's presence EXACTLY ONCE. `presence()` does a
    // PATH scan (via `which::which`, heavier on Windows `PATH × PATHEXT`); the
    // old code called it twice per writer - both wasteful and a benign TOCTOU
    // (the two non-atomic reads could disagree within one pass). Compute up
    // front, derive `any_present`, and iterate the cached booleans.
    let presences: Vec<bool> = writers.iter().map(|w| w.presence().is_present()).collect();
    let any_present = presences.iter().any(|&p| p);
    // Only require the bridge binary when there is at least one agent to
    // write to - a machine with no agents is "nothing to do", not an error.
    let bridge = if any_present {
        match bridge {
            Some(p) if p.exists() => Some(p),
            Some(p) => {
                return Err(format!(
                    "MCP bridge binary is missing at {} - launch Paneflow once to extract it, then retry. Nothing was written.",
                    p.display()
                ));
            }
            None => {
                return Err(
                    "could not resolve the Paneflow data directory, so the bridge path is unknown. Nothing was written."
                        .to_string(),
                );
            }
        }
    } else {
        None
    };

    let mut out = Vec::with_capacity(writers.len());
    for (w, &present) in writers.iter().zip(&presences) {
        let kind = match (present, bridge) {
            (false, _) => InstallKind::SkippedAbsent,
            (true, Some(b)) => match w.install(b) {
                Ok(InstallOutcome::Installed) => InstallKind::Installed,
                Ok(InstallOutcome::Updated) => InstallKind::Updated,
                Ok(InstallOutcome::AlreadyCurrent) => InstallKind::AlreadyCurrent,
                Err(e) => InstallKind::Error(format!("{e:#}")),
            },
            // Unreachable: any_present implies bridge is Some. Defensive.
            (true, None) => InstallKind::Error("bridge path unavailable".to_string()),
        };
        out.push(AgentResult {
            id: w.id().to_string(),
            label: w.label().to_string(),
            kind,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Read-only state of the bridge registration per agent. Never writes.
/// `bridge` is the current expected path used to flag staleness.
#[must_use]
pub fn status_all(bridge: Option<&Path>) -> Vec<AgentResult<StatusKind>> {
    status_with(bridge, &agents::default_writers())
}

pub(crate) fn status_with(
    bridge: Option<&Path>,
    writers: &[Box<dyn AgentConfigWriter>],
) -> Vec<AgentResult<StatusKind>> {
    writers
        .iter()
        .map(|w| {
            let kind = if w.presence().is_present() {
                match w.status(bridge) {
                    Ok(StatusOutcome::Installed { path }) => StatusKind::Installed { path },
                    Ok(StatusOutcome::StalePath { found, expected }) => {
                        StatusKind::Stale { found, expected }
                    }
                    Ok(StatusOutcome::NeedsRepair { path, reason }) => {
                        StatusKind::NeedsRepair { path, reason }
                    }
                    Ok(StatusOutcome::NotInstalled) => StatusKind::NotInstalled,
                    Err(e) => StatusKind::Error(format!("{e:#}")),
                }
            } else {
                StatusKind::NotDetected
            };
            AgentResult {
                id: w.id().to_string(),
                label: w.label().to_string(),
                kind,
            }
        })
        .collect()
}

/// Collapse per-agent statuses into the single state the Settings button
/// renders. `NotDetected` agents are ignored for the aggregate.
#[must_use]
pub fn overall_state(statuses: &[AgentResult<StatusKind>]) -> OverallState {
    let detected: Vec<&StatusKind> = statuses
        .iter()
        .map(|s| &s.kind)
        .filter(|k| !matches!(k, StatusKind::NotDetected))
        .collect();
    if detected.is_empty() {
        return OverallState::NoAgents;
    }
    if detected
        .iter()
        .any(|k| matches!(k, StatusKind::Stale { .. } | StatusKind::NeedsRepair { .. }))
    {
        return OverallState::NeedsRepair;
    }
    if detected
        .iter()
        .any(|k| matches!(k, StatusKind::NotInstalled | StatusKind::Error(_)))
    {
        return OverallState::NeedsInstall;
    }
    OverallState::AllInstalled
}

// ---------------------------------------------------------------------------
// Uninstall
// ---------------------------------------------------------------------------

/// Remove the `paneflow` entry from every detected agent.
pub fn uninstall_all() -> Vec<AgentResult<UninstallKind>> {
    uninstall_with(&agents::default_writers())
}

pub(crate) fn uninstall_with(
    writers: &[Box<dyn AgentConfigWriter>],
) -> Vec<AgentResult<UninstallKind>> {
    writers
        .iter()
        .map(|w| {
            let kind = if w.presence().is_present() {
                match w.uninstall() {
                    Ok(UninstallOutcome::Removed) => UninstallKind::Removed,
                    Ok(UninstallOutcome::NothingToRemove) => UninstallKind::NothingToRemove,
                    Err(e) => UninstallKind::Error(format!("{e:#}")),
                }
            } else {
                UninstallKind::NotDetected
            };
            AgentResult {
                id: w.id().to_string(),
                label: w.label().to_string(),
                kind,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::testutil::Mock;

    fn boxed(m: Mock) -> Box<dyn AgentConfigWriter> {
        Box::new(m)
    }

    #[test]
    fn install_refuses_when_bridge_missing_and_agents_present() {
        let writers = vec![boxed(Mock::present("claude"))];
        let err = install_with(Some(Path::new("/no/such/bin")), &writers).unwrap_err();
        assert!(err.contains("missing"));
    }

    #[test]
    fn install_no_agents_is_ok_empty() {
        // No agents present → no refusal even with a missing/None bridge.
        let writers = vec![boxed(Mock::absent("claude"))];
        let res = install_with(None, &writers).unwrap();
        assert_eq!(res[0].kind, InstallKind::SkippedAbsent);
    }

    #[test]
    fn install_maps_outcomes_per_agent() {
        let dir = tempfile::TempDir::new().unwrap();
        let bridge = dir.path().join("paneflow-mcp");
        std::fs::write(&bridge, b"x").unwrap();
        let writers = vec![
            boxed(Mock::present("a").with_install(Ok(InstallOutcome::Installed))),
            boxed(Mock::present("b").with_install(Ok(InstallOutcome::AlreadyCurrent))),
            boxed(Mock::absent("c")),
        ];
        let res = install_with(Some(&bridge), &writers).unwrap();
        assert_eq!(res[0].kind, InstallKind::Installed);
        assert_eq!(res[1].kind, InstallKind::AlreadyCurrent);
        assert_eq!(res[2].kind, InstallKind::SkippedAbsent);
    }

    #[test]
    fn overall_state_transitions() {
        // No agents.
        let none = vec![AgentResult {
            id: "a".into(),
            label: "A".into(),
            kind: StatusKind::NotDetected,
        }];
        assert_eq!(overall_state(&none), OverallState::NoAgents);

        // Stale wins over everything.
        let stale = vec![
            AgentResult {
                id: "a".into(),
                label: "A".into(),
                kind: StatusKind::Installed { path: "/p".into() },
            },
            AgentResult {
                id: "b".into(),
                label: "B".into(),
                kind: StatusKind::Stale {
                    found: "/old".into(),
                    expected: "/new".into(),
                },
            },
        ];
        assert_eq!(overall_state(&stale), OverallState::NeedsRepair);

        let repair = vec![AgentResult {
            id: "a".into(),
            label: "A".into(),
            kind: StatusKind::NeedsRepair {
                path: Some("/p".into()),
                reason: "disabled".into(),
            },
        }];
        assert_eq!(overall_state(&repair), OverallState::NeedsRepair);

        // Some not installed → needs install.
        let partial = vec![
            AgentResult {
                id: "a".into(),
                label: "A".into(),
                kind: StatusKind::Installed { path: "/p".into() },
            },
            AgentResult {
                id: "b".into(),
                label: "B".into(),
                kind: StatusKind::NotInstalled,
            },
        ];
        assert_eq!(overall_state(&partial), OverallState::NeedsInstall);

        // All installed.
        let all = vec![AgentResult {
            id: "a".into(),
            label: "A".into(),
            kind: StatusKind::Installed { path: "/p".into() },
        }];
        assert_eq!(overall_state(&all), OverallState::AllInstalled);
    }

    #[test]
    fn uninstall_maps_per_agent() {
        let writers = vec![boxed(Mock::present("a")), boxed(Mock::absent("b"))];
        let res = uninstall_with(&writers);
        assert_eq!(res[0].kind, UninstallKind::Removed);
        assert_eq!(res[1].kind, UninstallKind::NotDetected);
    }
}
