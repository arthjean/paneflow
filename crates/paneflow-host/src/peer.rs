use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_api::Principal;
use paneflow_api::methods::host::PeerClass;
use paneflow_peer::{Membership, PaneLeader, PeerRead};
use serde_json::json;

use crate::manifest::{list_manifest_paths, read_manifest};
use crate::protocol::ClientHello;

pub const METHOD_PEER_CLASSIFY: &str =
    <paneflow_api::methods::host::PeerClassify as paneflow_api::Method>::SPEC.name;

pub const WORKER_CLIENT_PREFIX: &str = "paneflow-serve";

pub const WINDOW_CLIENT_PREFIX: &str = "paneflow-desktop";

const CLASSIFY_DEADLINE: Duration = Duration::from_secs(4);

const _: () = assert!(
    CLASSIFY_DEADLINE.as_millis() > crate::host::PEER_LAUNCH_SETTLE_DEADLINE.as_millis(),
    "a caller of peer.classify must outwait the host's launch settle wait, or it classifies from manifests that do not name the pane being launched"
);

#[derive(Debug, Default)]
pub(crate) struct PaneContainment {
    pub(crate) process_session: Option<u32>,
    #[cfg(windows)]
    pub(crate) job: Option<std::sync::Arc<paneflow_peer::PaneJob>>,
}

#[cfg(windows)]
static JOB_FAILURE_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

impl PaneContainment {
    pub(crate) fn of(pid: u32) -> Self {
        if pid == 0 {
            return Self::default();
        }
        #[cfg(unix)]
        {
            Self {
                process_session: paneflow_peer::process_session(pid),
            }
        }
        #[cfg(windows)]
        {
            let job = match paneflow_peer::PaneJob::contain(pid) {
                Ok(job) => Some(std::sync::Arc::new(job)),
                Err(error) => {
                    if !JOB_FAILURE_LOGGED.swap(true, std::sync::atomic::Ordering::AcqRel) {
                        log::warn!(
                            "paneflow-host: pane process {pid} could not be placed in a Job Object ({error}); its pane membership falls back to the parent chain"
                        );
                    }
                    None
                }
            };
            Self {
                process_session: None,
                job,
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            Self::default()
        }
    }
}

pub fn membership_reply(membership: &Membership) -> PeerClass {
    let mut class = PeerClass {
        in_pane: false,
        session: None,
        workspace: None,
        stale: None,
        unverified: None,
    };
    match membership {
        Membership::Pane { session, workspace } => {
            class.in_pane = true;
            class.session = Some(session.clone());
            class.workspace = workspace.clone();
        }
        Membership::NotInPane => {}
        Membership::Stale => class.stale = Some(true),
        Membership::Unverified { reason } => class.unverified = Some(reason.clone()),
    }
    class
}

fn membership_of(class: &PeerClass) -> Membership {
    if let Some(reason) = &class.unverified {
        return Membership::Unverified {
            reason: reason.clone(),
        };
    }
    if class.stale == Some(true) {
        return Membership::Stale;
    }
    match (&class.in_pane, &class.session) {
        (true, Some(session)) => Membership::Pane {
            session: session.clone(),
            workspace: class.workspace.clone(),
        },
        (true, None) => Membership::Unverified {
            reason: "the host named a pane without its session".into(),
        },
        (false, _) => Membership::NotInPane,
    }
}

fn not_in_pane_or(membership: Membership, otherwise: Principal) -> Principal {
    match membership {
        Membership::Pane { session, workspace } => Principal::Pane { session, workspace },
        Membership::NotInPane => otherwise,
        Membership::Stale => Principal::Unverified {
            reason: "the calling process exited while it was classified".into(),
        },
        Membership::Unverified { reason } => Principal::Unverified { reason },
    }
}

pub fn host_principal(membership: Membership, hello: &ClientHello) -> Principal {
    let declared = if hello.client.starts_with(WORKER_CLIENT_PREFIX) {
        Principal::Worker
    } else if hello.attaches() || hello.client.starts_with(WINDOW_CLIENT_PREFIX) {
        Principal::Window
    } else {
        Principal::External
    };
    not_in_pane_or(membership, declared)
}

pub fn leaders_from_manifests(home: &Path) -> Result<Vec<PaneLeader>, String> {
    let paths = list_manifest_paths(home)
        .map_err(|error| format!("cannot list the session manifests: {error}"))?;
    let mut leaders = Vec::new();
    for path in paths {
        let manifest = read_manifest(&path).map_err(|error| {
            format!(
                "the session manifest {} cannot be read: {error}",
                path.display()
            )
        })?;
        let leader = manifest
            .process
            .and_then(|process| Some((process.pid, process.started_at?)));
        let Some((pid, started_at)) = leader else {
            if manifest.lifecycle.is_running() {
                return Err(format!(
                    "the running session {} records no pane leader yet",
                    manifest.session
                ));
            }
            continue;
        };
        leaders.push(PaneLeader {
            session: manifest.session.to_string(),
            workspace: manifest.workspace.as_ref().map(ToString::to_string),
            pid,
            started_at,
            process_session: manifest.process_session,
            #[cfg(windows)]
            job: None,
        });
    }
    Ok(leaders)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedBy {
    Local,
    Host,
    Manifests,
    Unreachable,
    UnidentifiedPeer,
}

impl ClassifiedBy {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Local => "the host's pane leaders",
            Self::Host => "peer.classify",
            Self::Manifests => "session manifests",
            Self::Unreachable => "no source",
            Self::UnidentifiedPeer => "peer read",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PeerClassifier {
    host_endpoint: Option<PathBuf>,
    home: Option<PathBuf>,
    client: String,
}

impl PeerClassifier {
    pub fn new(
        host_endpoint: Option<PathBuf>,
        home: Option<PathBuf>,
        client: impl Into<String>,
    ) -> Self {
        Self {
            host_endpoint,
            home,
            client: client.into(),
        }
    }

    fn ask_host(&self, pid: u32, started_at: u64) -> Result<Membership, String> {
        let endpoint = self
            .host_endpoint
            .as_deref()
            .ok_or_else(|| "no host endpoint resolves".to_string())?;
        let mut control = paneflow_ipc_client::host_control::HostControl::open(
            endpoint,
            &self.client,
            CLASSIFY_DEADLINE,
        )
        .map_err(|error| format!("{error:?}"))?;
        let reply = control.request_with_deadline(
            METHOD_PEER_CLASSIFY,
            json!({"pid": pid, "started_at": started_at}),
            CLASSIFY_DEADLINE,
        )?;
        let class: PeerClass = serde_json::from_value(reply)
            .map_err(|error| format!("the host's peer.classify reply is malformed: {error}"))?;
        Ok(membership_of(&class))
    }

    fn membership_from_manifests(&self, pid: u32, started_at: u64) -> Membership {
        #[cfg(windows)]
        {
            let _ = (pid, started_at, &self.home);
            Membership::Unverified {
                reason: "the host is unreachable, and pane membership on Windows needs the host's job handles".into(),
            }
        }
        #[cfg(not(windows))]
        {
            let Some(home) = self.home.as_deref() else {
                return Membership::Unverified {
                    reason: "the host is unreachable and no Paneflow home resolves".into(),
                };
            };
            match leaders_from_manifests(home) {
                Ok(leaders) => paneflow_peer::classify(pid, started_at, &leaders),
                Err(reason) => Membership::Unverified { reason },
            }
        }
    }

    pub fn membership(&self, read: &PeerRead) -> (Membership, ClassifiedBy) {
        let identity = match read {
            PeerRead::Identified(identity) => identity,
            PeerRead::Unverified { reason } => {
                return (
                    Membership::Unverified {
                        reason: reason.clone(),
                    },
                    ClassifiedBy::UnidentifiedPeer,
                );
            }
        };
        match self.ask_host(identity.pid(), identity.started_at()) {
            Ok(membership) => (membership, ClassifiedBy::Host),
            Err(error) => {
                log::debug!(
                    "paneflow: peer.classify unavailable ({error}); classifying process {} from the session manifests",
                    identity.pid()
                );
                let membership =
                    self.membership_from_manifests(identity.pid(), identity.started_at());
                let by = if cfg!(windows) {
                    ClassifiedBy::Unreachable
                } else {
                    ClassifiedBy::Manifests
                };
                (membership, by)
            }
        }
    }

    pub fn principal(&self, read: &PeerRead, server: &str) -> Principal {
        let started = Instant::now();
        let (membership, by) = self.membership(read);
        let principal = not_in_pane_or(membership, Principal::External);
        log_classification(server, read, &principal, by, started.elapsed());
        principal
    }
}

pub fn log_classification(
    server: &str,
    read: &PeerRead,
    principal: &Principal,
    by: ClassifiedBy,
    took: Duration,
) {
    log::debug!("{}", classification_line(server, read, principal, by, took));
}

fn classification_line(
    server: &str,
    read: &PeerRead,
    principal: &Principal,
    by: ClassifiedBy,
    took: Duration,
) -> String {
    match read {
        PeerRead::Identified(identity) => format!(
            "paneflow {server}: process {} ({}) classified {} through {} in {} us",
            identity.pid(),
            identity.source().label(),
            principal.kind(),
            by.label(),
            took.as_micros()
        ),
        PeerRead::Unverified { reason } => format!(
            "paneflow {server}: caller classified {} in {} us: {reason}",
            principal.kind(),
            took.as_micros()
        ),
    }
}

#[cfg(unix)]
pub fn read_wire_peer(wire: &paneflow_ipc_client::line_wire::Wire) -> PeerRead {
    paneflow_peer::read_unix_peer(wire.socket_fd())
}

#[cfg(windows)]
pub fn read_wire_peer(wire: &paneflow_ipc_client::line_wire::Wire) -> PeerRead {
    paneflow_peer::read_pipe_peer(wire.pipe_handle())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{SessionManifest, write_manifest};

    fn manifest_naming_this_process(home: &Path) -> SessionManifest {
        let pid = std::process::id();
        let session = paneflow_config::schema::SessionId::new();
        let workspace = paneflow_config::schema::WorkspaceId::new();
        let mut manifest: SessionManifest = serde_json::from_value(json!({
            "schema": crate::manifest::MANIFEST_SCHEMA_VERSION,
            "session": session,
            "workspace": workspace,
            "generation": 1,
            "host_instance": paneflow_config::schema::HostInstanceToken::new(),
            "cwd": "/",
            "launch": {"shell": "sh", "cols": 80, "rows": 24},
            "lifecycle": {"state": "running"},
            "created_at_ms": 1,
            "updated_at_ms": 1
        }))
        .unwrap();
        manifest.process = Some(crate::process::ProcessIdentity::capture(pid));
        #[cfg(unix)]
        {
            manifest.process_session = paneflow_peer::process_session(pid);
        }
        write_manifest(home, &manifest).unwrap();
        manifest
    }

    #[cfg(unix)]
    fn identified_self() -> PeerRead {
        use std::os::fd::AsFd;
        let (client, _server) = std::os::unix::net::UnixStream::pair().unwrap();
        paneflow_peer::read_unix_peer(client.as_fd())
    }

    #[test]
    fn an_unidentified_peer_is_unverified_whatever_the_host_says() {
        let classifier = PeerClassifier::new(None, None, "paneflow-test");
        let read = PeerRead::Unverified {
            reason: "the peer exited".into(),
        };
        assert_eq!(
            classifier.principal(&read, "test"),
            Principal::Unverified {
                reason: "the peer exited".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn with_the_host_unreachable_the_session_manifests_decide_membership() {
        let home = tempfile::tempdir().unwrap();
        let unreachable = Some(home.path().join("no-host.sock"));
        let classifier = PeerClassifier::new(
            unreachable,
            Some(home.path().to_path_buf()),
            "paneflow-test",
        );
        let read = identified_self();
        assert_eq!(classifier.principal(&read, "test"), Principal::External);
        let manifest = manifest_naming_this_process(home.path());
        assert_eq!(
            classifier.principal(&read, "test"),
            Principal::Pane {
                session: manifest.session.to_string(),
                workspace: manifest.workspace.as_ref().map(ToString::to_string),
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_manifest_that_cannot_name_its_running_pane_leader_leaves_the_peer_unverified() {
        let read = identified_self();
        for break_manifest in [
            (|home: &Path, manifest: &mut SessionManifest| {
                manifest.process = None;
                write_manifest(home, manifest).unwrap();
            }) as fn(&Path, &mut SessionManifest),
            |home, manifest| {
                let path = paneflow_home::host_sessions_dir_in(home)
                    .join(format!("{}.json", manifest.session));
                std::fs::write(path, b"{").unwrap();
            },
        ] {
            let home = tempfile::tempdir().unwrap();
            let classifier = PeerClassifier::new(
                Some(home.path().join("no-host.sock")),
                Some(home.path().to_path_buf()),
                "paneflow-test",
            );
            let mut manifest = manifest_naming_this_process(home.path());
            break_manifest(home.path(), &mut manifest);
            let principal = classifier.principal(&read, "test");
            assert!(
                matches!(principal, Principal::Unverified { .. }),
                "{principal:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_session_that_ended_without_a_leader_does_not_block_the_manifest_fallback() {
        let home = tempfile::tempdir().unwrap();
        let classifier = PeerClassifier::new(
            Some(home.path().join("no-host.sock")),
            Some(home.path().to_path_buf()),
            "paneflow-test",
        );
        let mut manifest = manifest_naming_this_process(home.path());
        manifest.process = None;
        manifest.lifecycle = crate::manifest::SessionLifecycle::Lost;
        write_manifest(home.path(), &manifest).unwrap();
        assert_eq!(
            classifier.principal(&identified_self(), "test"),
            Principal::External
        );
    }

    #[cfg(windows)]
    #[test]
    fn with_the_host_unreachable_a_windows_peer_is_unverified() {
        let home = tempfile::tempdir().unwrap();
        let classifier = PeerClassifier::new(
            Some(std::path::PathBuf::from(
                r"\\.\pipe\paneflow-no-host-for-this-test",
            )),
            Some(home.path().to_path_buf()),
            "paneflow-test",
        );
        manifest_naming_this_process(home.path());
        let me = std::process::id();
        let membership = classifier
            .membership_from_manifests(me, crate::process::process_start_time(me).unwrap());
        assert!(
            matches!(membership, Membership::Unverified { .. }),
            "{membership:?}"
        );
    }

    #[test]
    fn a_hello_names_a_peer_outside_every_pane_and_never_overrides_a_pane() {
        let pane = Membership::Pane {
            session: "s".into(),
            workspace: None,
        };
        let engine = ClientHello::local("paneflow-desktop");
        assert_eq!(
            host_principal(pane.clone(), &engine),
            Principal::Pane {
                session: "s".into(),
                workspace: None
            }
        );
        assert_eq!(
            host_principal(Membership::NotInPane, &engine),
            Principal::Window
        );
        assert_eq!(
            host_principal(
                Membership::NotInPane,
                &ClientHello::local("paneflow-serve-launch")
            ),
            Principal::Worker
        );
        assert_eq!(
            host_principal(Membership::NotInPane, &ClientHello::control("cli")),
            Principal::External
        );
        assert_eq!(
            host_principal(
                Membership::NotInPane,
                &ClientHello::control("paneflow-desktop-bootstrap")
            ),
            Principal::Window
        );
        assert_eq!(
            host_principal(pane, &ClientHello::control("paneflow-desktop")),
            Principal::Pane {
                session: "s".into(),
                workspace: None
            }
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_failed_job_assignment_leaves_the_pane_uncontained_and_is_logged_once() {
        assert!(PaneContainment::of(u32::MAX - 3).job.is_none());
        assert!(JOB_FAILURE_LOGGED.load(std::sync::atomic::Ordering::Acquire));
        assert!(PaneContainment::of(u32::MAX - 3).job.is_none());
    }

    #[test]
    fn a_classification_line_records_the_principal_and_how_long_it_took() {
        let unverified = PeerRead::Unverified {
            reason: "no peer".into(),
        };
        let line = classification_line(
            "worker",
            &unverified,
            &Principal::Unverified {
                reason: "no peer".into(),
            },
            ClassifiedBy::UnidentifiedPeer,
            Duration::from_micros(1234),
        );
        assert!(line.contains("classified unverified in 1234 us"), "{line}");
    }

    #[cfg(unix)]
    #[test]
    fn a_classification_line_names_the_process_and_its_identity_source() {
        let read = identified_self();
        let line = classification_line(
            "host",
            &read,
            &Principal::External,
            ClassifiedBy::Local,
            Duration::from_micros(42),
        );
        let pid = std::process::id();
        assert!(line.contains(&format!("process {pid} (")), "{line}");
        assert!(line.contains("classified external through"), "{line}");
        assert!(line.ends_with("in 42 us"), "{line}");
    }
}
