#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use paneflow_api::principal::Principal;
use paneflow_config::schema::WorkspaceId;
use paneflow_host::manifest::{manifest_path, read_manifest};
use paneflow_host::peer::{ClassifiedBy, PeerClassifier};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, SessionId};
use paneflow_peer::Membership;
use serde_json::json;

const OUTCOME_DEADLINE: Duration = Duration::from_secs(30);
const EXPECTED_SOURCE_ENV: &str = "PANEFLOW_EXPECT_PEER_SOURCE";

struct HostProcess {
    home: tempfile::TempDir,
    endpoint: PathBuf,
    log: PathBuf,
    child: Option<Child>,
}

impl HostProcess {
    fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("host.sock");
        let log = home.path().join("host.log");
        let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow-host"));
        command
            .arg("--home")
            .arg(home.path())
            .arg("serve")
            .arg("--endpoint")
            .arg(&endpoint)
            .env("RUST_LOG", "debug")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap());
        for name in paneflow_host::env::PANE_CONTEXT_ENV {
            command.env_remove(name);
        }
        let child = command.spawn().unwrap();
        let host = Self {
            home,
            endpoint,
            log,
            child: Some(child),
        };
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        while HostClient::connect(&host.endpoint, &ClientHello::control("kernel-matrix-ready"))
            .is_err()
        {
            assert!(
                Instant::now() < deadline,
                "the host never answered on {}: {}",
                host.endpoint.display(),
                host.log_text()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        host
    }

    fn client(&self, hello: &ClientHello) -> HostClient {
        HostClient::connect(&self.endpoint, hello).unwrap()
    }

    fn pane(&self, args: &[&str]) -> (SessionId, WorkspaceId) {
        let workspace = WorkspaceId::new();
        let request = CreateSession {
            workspace: Some(workspace.clone()),
            shell: Some(env!("CARGO_BIN_EXE_paneflow-session-fixture").to_string()),
            args: args.iter().map(|arg| arg.to_string()).collect(),
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(200),
            rows: Some(40),
            ..CreateSession::default()
        };
        let summary = self
            .client(&ClientHello::local("kernel-matrix-test"))
            .create(&request)
            .unwrap_or_else(|error| {
                panic!(
                    "creating the pane failed: {error}; host log: {}",
                    self.log_text()
                )
            });
        (summary.manifest.session, workspace)
    }

    fn screen_until(&self, session: &SessionId, marker: &str) -> String {
        let mut client = self.client(&ClientHello::local("kernel-matrix-reader"));
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = client.text(session).unwrap().text;
            if text.contains(marker) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "{marker} never appeared in the pane: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn leader(&self, session: &SessionId) -> u32 {
        let manifest = read_manifest(&manifest_path(self.home.path(), session)).unwrap();
        manifest.process.expect("the leader is recorded").pid
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn classification_of(&self, pid: u32, principal: &str) -> String {
        let process = format!("paneflow host: process {pid} (");
        let verdict = format!(") classified {principal} through");
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = self.log_text();
            if let Some(line) = text
                .lines()
                .find(|line| line.contains(&process) && line.contains(&verdict))
            {
                return line.to_string();
            }
            assert!(
                Instant::now() < deadline,
                "the host never logged process {pid} as {principal}: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

fn line_value<'a>(screen: &'a str, prefix: &str) -> Option<&'a str> {
    screen
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(str::trim)
}

fn kernel_peer_source() -> &'static str {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let mut descriptor: libc::c_int = -1;
    let mut length = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERPIDFD,
            (&raw mut descriptor).cast(),
            &raw mut length,
        )
    };
    if status == 0 {
        unsafe {
            libc::close(descriptor);
        }
        return "SO_PEERPIDFD";
    }
    let error = std::io::Error::last_os_error();
    assert_eq!(
        error.raw_os_error(),
        Some(libc::ENOPROTOOPT),
        "SO_PEERPIDFD failed for another reason than a kernel without it: {error}"
    );
    "SO_PEERCRED"
}

fn expected_peer_source() -> String {
    std::env::var(EXPECTED_SOURCE_ENV).unwrap_or_else(|_| kernel_peer_source().to_string())
}

fn accept_within(listener: &UnixListener) -> UnixStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    Instant::now() < deadline,
                    "the pane process never connected after the host died"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => panic!("accepting the pane process failed: {error}"),
        }
    }
}

fn manifest_names_leader(home: &Path, session: &SessionId, leader: u32) -> (bool, String) {
    let manifest = read_manifest(&manifest_path(home, session)).unwrap();
    let named = manifest.process.as_ref().map(|process| process.pid) == Some(leader);
    (named, manifest.lifecycle.label().to_string())
}

#[test]
fn the_host_binary_reads_each_caller_through_the_kernel_peer_source() {
    let host = HostProcess::start();
    let endpoint = host.endpoint.display().to_string();
    let (session, workspace) = host.pane(&["whoami", "host", &endpoint, "control"]);
    let screen = host.screen_until(&session, "whoami done");
    assert_eq!(
        line_value(&screen, "whoami principal="),
        Some("pane"),
        "{screen}"
    );
    assert_eq!(
        line_value(&screen, "whoami session="),
        Some(session.as_str())
    );
    assert_eq!(
        line_value(&screen, "whoami workspace="),
        Some(workspace.as_str())
    );
    let outside = host
        .client(&ClientHello::control("paneflow-cli"))
        .call("system.whoami", json!({}))
        .unwrap();
    assert_eq!(outside, json!({"principal": "external"}));

    let expected = format!("({})", expected_peer_source());
    let pane_line = host.classification_of(host.leader(&session), "pane");
    let outside_line = host.classification_of(std::process::id(), "external");
    println!("kernel-matrix: pane caller: {pane_line}");
    println!("kernel-matrix: outside caller: {outside_line}");
    assert!(pane_line.contains(&expected), "{pane_line}");
    assert!(outside_line.contains(&expected), "{outside_line}");
}

#[test]
fn a_pane_leader_that_outlives_a_killed_host_is_classified_from_its_manifest() {
    let mut host = HostProcess::start();
    let worker_endpoint = host.home.path().join("worker.sock");
    let listener = UnixListener::bind(&worker_endpoint).unwrap();
    let (session, workspace) = host.pane(&["outlive-host", &worker_endpoint.display().to_string()]);
    host.screen_until(&session, "outlive ready");
    let leader = host.leader(&session);
    host.kill();

    let stream = accept_within(&listener);
    let read = paneflow_peer::read_unix_peer(stream.as_fd());
    let identity = read
        .identity()
        .expect("the surviving pane process is identified");
    assert_eq!(identity.pid(), leader);
    let (named, lifecycle) = manifest_names_leader(host.home.path(), &session, leader);
    println!(
        "kernel-matrix: host killed; the manifest still names leader {leader}: {named} (lifecycle {lifecycle})"
    );
    let classifier = PeerClassifier::new(
        Some(host.endpoint.clone()),
        Some(host.home.path().to_path_buf()),
        "kernel-matrix-worker",
    );
    let (membership, by) = classifier.membership(&read);
    println!(
        "kernel-matrix: the worker's classifier answered {membership:?} through {}",
        by.label()
    );
    assert!(named, "the manifest no longer names the leader {leader}");
    assert_eq!(by, ClassifiedBy::Manifests);
    let expected = Membership::Pane {
        session: session.to_string(),
        workspace: Some(workspace.to_string()),
    };
    assert_eq!(membership, expected);
    assert_eq!(
        classifier.principal(&read, "worker"),
        Principal::Pane {
            session: session.to_string(),
            workspace: Some(workspace.to_string()),
        }
    );
}
