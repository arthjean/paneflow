#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use paneflow_config::schema::WorkspaceId;
use paneflow_host::protocol::ClientHello;
use paneflow_host::server::ServerHandle;
use paneflow_host::{CreateSession, HostClient, SessionHost, SessionId};
use serde_json::{Value, json};

static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(0);

fn fixture_executable() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-session-fixture"))
}

fn endpoint_in(home: &Path) -> PathBuf {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
    );
    #[cfg(windows)]
    {
        let _ = home;
        PathBuf::from(format!(r"\\.\pipe\paneflow-identity-test-{unique}"))
    }
    #[cfg(unix)]
    {
        home.join(format!("i{unique}.sock"))
    }
}

struct Host {
    _home: tempfile::TempDir,
    host: Arc<SessionHost>,
    server: Option<ServerHandle>,
    endpoint: PathBuf,
}

impl Host {
    fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let endpoint = endpoint_in(home.path());
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).unwrap();
        Self {
            _home: home,
            host,
            server: Some(server),
            endpoint,
        }
    }

    fn client(&self, hello: &ClientHello) -> HostClient {
        HostClient::connect(&self.endpoint, hello).unwrap()
    }

    fn pane(&self, mode: &str, extra: &[&str]) -> (SessionId, WorkspaceId) {
        let workspace = WorkspaceId::new();
        let mut args = vec![mode.to_string()];
        if mode != "idle" {
            args.extend(["host".to_string(), self.endpoint.display().to_string()]);
        }
        args.extend(extra.iter().map(|arg| arg.to_string()));
        let request = CreateSession {
            workspace: Some(workspace.clone()),
            shell: Some(fixture_executable().display().to_string()),
            args,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(200),
            rows: Some(40),
            ..CreateSession::default()
        };
        let summary = self
            .client(&ClientHello::local("caller-identity-test"))
            .create(&request)
            .unwrap();
        (summary.manifest.session, workspace)
    }

    fn screen_until(&self, session: &SessionId, marker: &str) -> String {
        let mut client = self.client(&ClientHello::local("caller-identity-reader"));
        let deadline = Instant::now() + Duration::from_secs(30);
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
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.host.request_shutdown(true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

fn line_value<'a>(screen: &'a str, prefix: &str) -> Option<&'a str> {
    screen
        .lines()
        .find_map(|line| line.trim().strip_prefix(prefix))
        .map(str::trim)
}

fn whoami(client: &mut HostClient) -> Value {
    client.call("system.whoami", json!({})).unwrap()
}

#[test]
fn a_pane_process_whose_hello_claims_an_engine_is_classified_as_its_pane() {
    let host = Host::start();
    let (session, workspace) = host.pane("whoami", &["engine"]);
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
}

#[test]
fn a_pane_caller_of_peer_classify_is_refused_as_principal_refused() {
    let host = Host::start();
    let (session, _) = host.pane("whoami", &["control"]);
    let screen = host.screen_until(&session, "whoami done");
    assert_eq!(
        line_value(&screen, "classify refused code="),
        Some(
            paneflow_api::principal::ERR_PRINCIPAL_REFUSED
                .to_string()
                .as_str()
        ),
        "{screen}"
    );
}

#[test]
fn a_caller_outside_every_pane_is_named_by_its_hello() {
    let host = Host::start();
    let _idle = host.pane("idle", &[]);
    let cases = [
        (ClientHello::local("paneflow-desktop"), "window"),
        (ClientHello::control("paneflow-serve-launch"), "worker"),
        (ClientHello::local("paneflow-serve-launch"), "worker"),
        (ClientHello::control("paneflow-cli"), "external"),
    ];
    for (hello, expected) in cases {
        let answer = whoami(&mut host.client(&hello));
        assert_eq!(answer, json!({"principal": expected}), "{}", hello.client);
    }
}

#[test]
fn peer_classify_names_the_pane_of_a_leader_and_flags_a_stale_start_time() {
    let host = Host::start();
    let (session, workspace) = host.pane("idle", &[]);
    host.screen_until(&session, "fixture idle");
    let mut client = host.client(&ClientHello::control("paneflow-serve-classify"));
    let summary = client.inspect(&session).unwrap();
    let process = summary.manifest.process.expect("the leader is recorded");
    let started_at = process.started_at.expect("its start time is recorded");
    #[cfg(unix)]
    assert_eq!(summary.manifest.process_session, Some(process.pid));
    let answer = client
        .call(
            "peer.classify",
            json!({"pid": process.pid, "started_at": started_at}),
        )
        .unwrap();
    assert_eq!(
        answer,
        json!({"in_pane": true, "session": session.as_str(), "workspace": workspace.as_str()})
    );
    let stale = client
        .call(
            "peer.classify",
            json!({"pid": process.pid, "started_at": started_at + 1}),
        )
        .unwrap();
    assert_eq!(stale, json!({"in_pane": false, "stale": true}));
    let me = std::process::id();
    let outside = client
        .call(
            "peer.classify",
            json!({"pid": me, "started_at": paneflow_host::process::process_start_time(me)}),
        )
        .unwrap();
    assert_eq!(outside, json!({"in_pane": false}));
}

#[cfg(unix)]
#[test]
fn a_pane_process_that_called_setsid_but_kept_its_parent_is_still_its_pane() {
    let host = Host::start();
    let (session, _) = host.pane("setsid-whoami", &["engine"]);
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
}

#[cfg(unix)]
#[test]
fn a_pane_process_that_double_forked_without_setsid_is_still_its_pane() {
    let host = Host::start();
    let (session, _) = host.pane("orphan-whoami", &["engine"]);
    let screen = host.screen_until(&session, "whoami done");
    assert_eq!(
        line_value(&screen, "whoami principal="),
        Some("pane"),
        "{screen}"
    );
}

#[cfg(unix)]
#[test]
fn a_pane_process_that_calls_setsid_and_double_forks_escapes_layer_one() {
    let host = Host::start();
    let (session, _) = host.pane("escape-whoami", &["control"]);
    let screen = host.screen_until(&session, "whoami done");
    assert_eq!(
        line_value(&screen, "whoami principal="),
        Some("external"),
        "{screen}"
    );
}
