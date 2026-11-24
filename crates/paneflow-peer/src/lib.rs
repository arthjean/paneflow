#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]

use std::path::{Path, PathBuf};

mod lineage;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
mod membership;
mod start_time;
#[cfg(windows)]
mod windows;

pub use lineage::{Ancestor, LeaderStamp, Lineage, LineageReader, LiveLineage};
pub use membership::{Membership, PaneLeader, classify};
#[cfg(unix)]
pub use start_time::process_session;
pub use start_time::{parent_process, process_start_time};
#[cfg(windows)]
pub use windows::{PaneJob, pane_job_name};

#[cfg(target_os = "linux")]
type Anchor = std::os::fd::OwnedFd;
#[cfg(windows)]
type Anchor = std::os::windows::io::OwnedHandle;
#[cfg(not(any(target_os = "linux", windows)))]
type Anchor = ();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerSource {
    PidFd,
    PeerCred,
    AuditToken,
    PipeClient,
}

impl PeerSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::PidFd => "SO_PEERPIDFD",
            Self::PeerCred => "SO_PEERCRED",
            Self::AuditToken => "LOCAL_PEERTOKEN",
            Self::PipeClient => "GetNamedPipeClientProcessId",
        }
    }
}

#[derive(Debug)]
pub struct PeerIdentity {
    pid: u32,
    started_at: u64,
    executable: Option<PathBuf>,
    source: PeerSource,
    _anchor: Option<Anchor>,
}

impl PeerIdentity {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn started_at(&self) -> u64 {
        self.started_at
    }

    pub fn executable(&self) -> Option<&Path> {
        self.executable.as_deref()
    }

    pub fn source(&self) -> PeerSource {
        self.source
    }

    #[cfg(test)]
    pub(crate) fn recorded(pid: u32, started_at: u64) -> Self {
        Self {
            pid,
            started_at,
            executable: None,
            source: PeerSource::PeerCred,
            _anchor: None,
        }
    }
}

impl PartialEq for PeerIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.pid == other.pid && self.started_at == other.started_at
    }
}

impl Eq for PeerIdentity {}

#[derive(Debug, PartialEq, Eq)]
pub enum PeerRead {
    Identified(PeerIdentity),
    Unverified { reason: String },
}

impl PeerRead {
    pub(crate) fn unverified(reason: impl Into<String>) -> Self {
        Self::Unverified {
            reason: reason.into(),
        }
    }

    pub fn identity(&self) -> Option<&PeerIdentity> {
        match self {
            Self::Identified(identity) => Some(identity),
            Self::Unverified { .. } => None,
        }
    }
}

#[cfg(unix)]
pub fn read_unix_peer(socket: std::os::fd::BorrowedFd<'_>) -> PeerRead {
    #[cfg(target_os = "linux")]
    {
        linux::read(socket)
    }
    #[cfg(target_os = "macos")]
    {
        macos::read(socket)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = socket;
        PeerRead::unverified("peer identity is not implemented on this operating system")
    }
}

#[cfg(windows)]
pub fn read_pipe_peer(pipe: std::os::windows::io::BorrowedHandle<'_>) -> PeerRead {
    windows::read(pipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_reads_of_one_process_are_equal_and_a_reused_pid_is_not() {
        let first = PeerIdentity::recorded(4242, 1_000);
        let again = PeerIdentity::recorded(4242, 1_000);
        let reused = PeerIdentity::recorded(4242, 2_000);
        assert_eq!(first, again);
        assert_ne!(first, reused);
    }

    fn internal_dependencies(manifest: &str) -> Vec<String> {
        manifest
            .lines()
            .map(str::trim_start)
            .filter_map(|line| {
                let name = line.split(['=', ' ', '.']).next()?;
                name.starts_with("paneflow-").then(|| name.to_string())
            })
            .filter(|name| !name.is_empty())
            .collect()
    }

    fn manifest_of(crate_name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(crate_name)
            .join("Cargo.toml");
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    fn dependency_closure(root: &str) -> Vec<String> {
        let mut seen = vec![root.to_string()];
        let mut pending = vec![root.to_string()];
        while let Some(next) = pending.pop() {
            for dependency in internal_dependencies(&manifest_of(&next)) {
                if dependency != next && !seen.contains(&dependency) {
                    seen.push(dependency.clone());
                    pending.push(dependency);
                }
            }
        }
        seen
    }

    #[test]
    fn the_shim_and_the_hook_never_link_the_peer_reader() {
        for helper in ["paneflow-shim", "paneflow-ai-hook"] {
            let closure = dependency_closure(helper);
            assert!(
                !closure.iter().any(|name| name == "paneflow-peer"),
                "{helper} reaches paneflow-peer through {closure:?}"
            );
        }
    }

    #[test]
    fn the_peer_reader_depends_on_no_crate_beyond_the_os_bindings() {
        let manifest = include_str!("../Cargo.toml");
        let mut in_dependencies = false;
        let mut declared = Vec::new();
        for line in manifest.lines().map(str::trim) {
            if line.starts_with('[') {
                in_dependencies =
                    line.ends_with("dependencies]") && !line.contains("dev-dependencies");
                continue;
            }
            if in_dependencies && line.contains('=') && !line.starts_with('"') {
                let name = line.split(['=', '.']).next().unwrap_or_default().trim();
                declared.push(name.to_string());
            }
        }
        assert_eq!(declared, ["libc", "windows-sys"]);
    }
}
