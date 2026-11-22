#[cfg(unix)]
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Membership {
    Pane {
        session: String,
        workspace: Option<String>,
    },
    NotInPane,
    Stale,
    Unverified {
        reason: String,
    },
}

impl Membership {
    pub(crate) fn pane(leader: &PaneLeader) -> Self {
        Self::Pane {
            session: leader.session.clone(),
            workspace: leader.workspace.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PaneLeader {
    pub session: String,
    pub workspace: Option<String>,
    pub pid: u32,
    pub started_at: u64,
    pub process_session: Option<u32>,
    #[cfg(windows)]
    pub job: Option<std::sync::Arc<crate::PaneJob>>,
}

#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProcessFacts {
    pub(crate) parent: u32,
    pub(crate) session: u32,
    pub(crate) started_at: u64,
}

#[cfg(unix)]
pub(crate) trait ProcessTable {
    fn facts(&self, pid: u32) -> io::Result<ProcessFacts>;
}

#[cfg(unix)]
const MAX_ANCESTORS: usize = 1024;

pub fn classify(pid: u32, started_at: u64, leaders: &[PaneLeader]) -> Membership {
    #[cfg(target_os = "linux")]
    {
        classify_in(&crate::linux::LiveKernel, pid, started_at, leaders)
    }
    #[cfg(target_os = "macos")]
    {
        classify_in(&crate::macos::LiveTable, pid, started_at, leaders)
    }
    #[cfg(windows)]
    {
        crate::windows::classify(pid, started_at, leaders)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = (pid, started_at, leaders);
        Membership::Unverified {
            reason: "pane membership is not implemented on this operating system".into(),
        }
    }
}

#[cfg(unix)]
enum SessionMatch<'a> {
    Leader(&'a PaneLeader),
    None,
    Unreadable(String),
}

#[cfg(unix)]
fn session_leader<'a>(
    table: &impl ProcessTable,
    session: u32,
    leaders: &'a [PaneLeader],
) -> SessionMatch<'a> {
    for leader in leaders
        .iter()
        .filter(|leader| leader.process_session.unwrap_or(leader.pid) == session)
    {
        match table.facts(leader.pid) {
            Ok(facts) if facts.started_at == leader.started_at => {
                return SessionMatch::Leader(leader);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return SessionMatch::Leader(leader);
            }
            Err(error) => {
                return SessionMatch::Unreadable(format!(
                    "cannot read the pane leader {}: {error}",
                    leader.pid
                ));
            }
        }
    }
    SessionMatch::None
}

#[cfg(unix)]
pub(crate) fn classify_in(
    table: &impl ProcessTable,
    pid: u32,
    started_at: u64,
    leaders: &[PaneLeader],
) -> Membership {
    let mut current = match table.facts(pid) {
        Ok(facts) => facts,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Membership::Stale,
        Err(error) => {
            return Membership::Unverified {
                reason: format!("cannot read process {pid}: {error}"),
            };
        }
    };
    if current.started_at != started_at {
        return Membership::Stale;
    }
    let mut current_pid = pid;
    for _ in 0..MAX_ANCESTORS {
        if let Some(leader) = leaders
            .iter()
            .find(|leader| leader.pid == current_pid && leader.started_at == current.started_at)
        {
            return Membership::pane(leader);
        }
        match session_leader(table, current.session, leaders) {
            SessionMatch::Leader(leader) => return Membership::pane(leader),
            SessionMatch::Unreadable(reason) => return Membership::Unverified { reason },
            SessionMatch::None => {}
        }
        let parent = current.parent;
        if parent == 0 || parent == current_pid {
            return Membership::NotInPane;
        }
        let facts = match table.facts(parent) {
            Ok(facts) => facts,
            Err(error) => {
                return Membership::Unverified {
                    reason: format!(
                        "cannot read the parent {parent} of process {current_pid}: {error}"
                    ),
                };
            }
        };
        if facts.started_at > current.started_at {
            return Membership::Unverified {
                reason: format!(
                    "the parent {parent} of process {current_pid} started after it, so its PID was reused"
                ),
            };
        }
        current_pid = parent;
        current = facts;
    }
    Membership::Unverified {
        reason: format!("the parent chain of {pid} is longer than {MAX_ANCESTORS} processes"),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Table {
        processes: HashMap<u32, ProcessFacts>,
        unreadable: Vec<u32>,
    }

    impl Table {
        fn with(mut self, pid: u32, parent: u32, session: u32, started_at: u64) -> Self {
            self.processes.insert(
                pid,
                ProcessFacts {
                    parent,
                    session,
                    started_at,
                },
            );
            self
        }

        fn unreadable(mut self, pid: u32) -> Self {
            self.unreadable.push(pid);
            self
        }
    }

    impl ProcessTable for Table {
        fn facts(&self, pid: u32) -> io::Result<ProcessFacts> {
            if self.unreadable.contains(&pid) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.processes
                .get(&pid)
                .copied()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn leader(pid: u32, started_at: u64) -> PaneLeader {
        PaneLeader {
            session: "pane-session".into(),
            workspace: Some("workspace-7".into()),
            pid,
            started_at,
            process_session: Some(pid),
            #[cfg(windows)]
            job: None,
        }
    }

    fn pane() -> Membership {
        Membership::Pane {
            session: "pane-session".into(),
            workspace: Some("workspace-7".into()),
        }
    }

    fn base() -> Table {
        Table::default()
            .with(1, 0, 1, 1)
            .with(900, 1, 900, 50)
            .with(100, 900, 100, 100)
    }

    #[test]
    fn a_process_started_by_the_pane_shell_is_that_pane() {
        let table = base().with(200, 100, 100, 200);
        assert_eq!(classify_in(&table, 200, 200, &[leader(100, 100)]), pane());
    }

    #[test]
    fn a_process_that_called_setsid_and_kept_its_parent_reaches_the_leader() {
        let table = base().with(200, 100, 100, 200).with(300, 200, 300, 300);
        assert_eq!(classify_in(&table, 300, 300, &[leader(100, 100)]), pane());
    }

    #[test]
    fn a_double_fork_without_setsid_keeps_the_pane_session_id() {
        let table = base().with(400, 1, 100, 400);
        assert_eq!(classify_in(&table, 400, 400, &[leader(100, 100)]), pane());
    }

    #[test]
    fn a_double_fork_survives_its_pane_leader_while_the_session_lives() {
        let table = Table::default().with(1, 0, 1, 1).with(400, 1, 100, 400);
        assert_eq!(classify_in(&table, 400, 400, &[leader(100, 100)]), pane());
    }

    #[test]
    fn a_process_unrelated_to_every_pane_is_not_in_a_pane() {
        let table = base().with(500, 900, 900, 500);
        assert_eq!(
            classify_in(&table, 500, 500, &[leader(100, 100)]),
            Membership::NotInPane
        );
    }

    #[test]
    fn a_reused_leader_pid_puts_the_new_owner_outside_the_pane() {
        let table = Table::default()
            .with(1, 0, 1, 1)
            .with(100, 1, 100, 7_000)
            .with(7_100, 100, 100, 7_100);
        assert_eq!(
            classify_in(&table, 7_100, 7_100, &[leader(100, 100)]),
            Membership::NotInPane
        );
    }

    #[test]
    fn an_unreadable_parent_chain_is_unverified() {
        let table = Table::default().with(500, 900, 500, 500).unreadable(900);
        assert!(matches!(
            classify_in(&table, 500, 500, &[leader(100, 100)]),
            Membership::Unverified { .. }
        ));
    }

    #[test]
    fn a_parent_younger_than_its_child_is_a_reused_pid_and_unverified() {
        let table = Table::default()
            .with(500, 900, 500, 500)
            .with(900, 1, 900, 9_000);
        assert!(matches!(
            classify_in(&table, 500, 500, &[leader(100, 100)]),
            Membership::Unverified { .. }
        ));
    }

    #[test]
    fn a_peer_whose_start_time_changed_is_stale() {
        let table = base().with(200, 100, 100, 200);
        assert_eq!(
            classify_in(&table, 200, 199, &[leader(100, 100)]),
            Membership::Stale
        );
        assert_eq!(
            classify_in(&table, 201, 201, &[leader(100, 100)]),
            Membership::Stale
        );
    }
}
