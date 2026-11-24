use std::path::PathBuf;

#[cfg(unix)]
const MAX_ANCESTORS: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ancestor {
    pub pid: u32,
    pub started_at: u64,
    pub name: Option<String>,
    pub executable: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lineage {
    ToLeader(Vec<Ancestor>),
    Broken { reason: String },
}

impl Lineage {
    pub(crate) fn broken(reason: impl Into<String>) -> Self {
        Self::Broken {
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaderStamp {
    pub pid: u32,
    pub started_at: u64,
}

pub trait LineageReader {
    fn lineage(&self, pid: u32, started_at: u64, leader: LeaderStamp) -> Lineage;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LiveLineage;

impl LineageReader for LiveLineage {
    fn lineage(&self, pid: u32, started_at: u64, leader: LeaderStamp) -> Lineage {
        #[cfg(target_os = "linux")]
        {
            lineage_in(&crate::linux::LiveKernel, pid, started_at, leader)
        }
        #[cfg(target_os = "macos")]
        {
            lineage_in(&crate::macos::LiveTable, pid, started_at, leader)
        }
        #[cfg(windows)]
        {
            crate::windows::lineage(pid, started_at, leader)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (pid, started_at, leader);
            Lineage::broken(
                "reading a caller's parents is not implemented on this operating system",
            )
        }
    }
}

#[cfg(unix)]
pub(crate) fn lineage_in(
    table: &impl crate::membership::ProcessTable,
    pid: u32,
    started_at: u64,
    leader: LeaderStamp,
) -> Lineage {
    let mut current = match table.facts(pid) {
        Ok(facts) => facts,
        Err(error) => return Lineage::broken(format!("cannot read process {pid}: {error}")),
    };
    if current.started_at != started_at {
        return Lineage::broken(format!(
            "process {pid} ended and its id was reused before its parents were read"
        ));
    }
    let mut current_pid = pid;
    let mut chain = Vec::new();
    for _ in 0..MAX_ANCESTORS {
        let (name, executable) = table.describe(current_pid);
        chain.push(Ancestor {
            pid: current_pid,
            started_at: current.started_at,
            name,
            executable,
        });
        if current_pid == leader.pid {
            if current.started_at == leader.started_at {
                return Lineage::ToLeader(chain);
            }
            return Lineage::broken(format!(
                "the pane's shell {} ended and its id was reused",
                leader.pid
            ));
        }
        let parent = current.parent;
        if parent == 0 || parent == current_pid {
            return Lineage::broken(format!(
                "the parents of process {pid} end at {current_pid} without the pane's shell {}",
                leader.pid
            ));
        }
        let facts = match table.facts(parent) {
            Ok(facts) => facts,
            Err(error) => {
                return Lineage::broken(format!(
                    "cannot read the parent {parent} of process {current_pid}: {error}"
                ));
            }
        };
        if facts.started_at > current.started_at {
            return Lineage::broken(format!(
                "the parent {parent} of process {current_pid} started after it, so its id was reused"
            ));
        }
        current_pid = parent;
        current = facts;
    }
    Lineage::broken(format!(
        "the parents of process {pid} are more than {MAX_ANCESTORS} processes deep"
    ))
}

#[cfg(all(test, unix))]
mod tests {
    use std::collections::HashMap;
    use std::io;

    use super::*;
    use crate::membership::{ProcessFacts, ProcessTable};

    #[derive(Default)]
    struct Table {
        processes: HashMap<u32, (ProcessFacts, &'static str)>,
        unreadable: Vec<u32>,
    }

    impl Table {
        fn with(mut self, pid: u32, parent: u32, started_at: u64, name: &'static str) -> Self {
            self.processes.insert(
                pid,
                (
                    ProcessFacts {
                        parent,
                        session: 100,
                        started_at,
                    },
                    name,
                ),
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
                .map(|(facts, _)| *facts)
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn describe(&self, pid: u32) -> (Option<String>, Option<PathBuf>) {
            let name = self
                .processes
                .get(&pid)
                .map(|(_, name)| (*name).to_string());
            (name, None)
        }
    }

    const SHELL: LeaderStamp = LeaderStamp {
        pid: 100,
        started_at: 100,
    };

    fn base() -> Table {
        Table::default()
            .with(1, 0, 1, "init")
            .with(100, 1, 100, "zsh")
    }

    fn names(lineage: &Lineage) -> Vec<String> {
        match lineage {
            Lineage::ToLeader(chain) => chain
                .iter()
                .map(|ancestor| ancestor.name.clone().unwrap_or_default())
                .collect(),
            Lineage::Broken { reason } => panic!("broken: {reason}"),
        }
    }

    #[test]
    fn the_walk_names_every_process_from_the_caller_to_the_pane_shell() {
        let table = base()
            .with(200, 100, 200, "claude")
            .with(300, 200, 300, "bash")
            .with(400, 300, 400, "paneflow");
        assert_eq!(
            names(&lineage_in(&table, 400, 400, SHELL)),
            ["paneflow", "bash", "claude", "zsh"]
        );
        let typed = base().with(500, 100, 500, "paneflow");
        assert_eq!(
            names(&lineage_in(&typed, 500, 500, SHELL)),
            ["paneflow", "zsh"]
        );
    }

    #[test]
    fn a_reused_or_unreadable_process_breaks_the_walk() {
        let table = base()
            .with(200, 100, 200, "claude")
            .with(300, 200, 300, "paneflow");
        for (pid, started_at) in [(300, 299), (301, 301)] {
            assert!(matches!(
                lineage_in(&table, pid, started_at, SHELL),
                Lineage::Broken { .. }
            ));
        }
        let unreadable = base()
            .with(200, 100, 200, "claude")
            .with(300, 200, 300, "paneflow")
            .unreadable(200);
        assert!(matches!(
            lineage_in(&unreadable, 300, 300, SHELL),
            Lineage::Broken { .. }
        ));
        let younger_parent = base()
            .with(200, 100, 9_000, "claude")
            .with(300, 200, 300, "paneflow");
        assert!(matches!(
            lineage_in(&younger_parent, 300, 300, SHELL),
            Lineage::Broken { .. }
        ));
    }

    #[test]
    fn an_orphan_or_a_reused_shell_never_reaches_the_pane() {
        let orphan = base().with(300, 1, 300, "paneflow");
        assert!(matches!(
            lineage_in(&orphan, 300, 300, SHELL),
            Lineage::Broken { .. }
        ));
        let reused_shell = Table::default()
            .with(1, 0, 1, "init")
            .with(100, 1, 50, "zsh")
            .with(300, 100, 300, "paneflow");
        assert!(matches!(
            lineage_in(&reused_shell, 300, 300, SHELL),
            Lineage::Broken { .. }
        ));
    }
}
