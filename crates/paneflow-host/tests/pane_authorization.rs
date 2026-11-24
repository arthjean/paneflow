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

fn endpoint_in(home: &Path) -> PathBuf {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
    );
    #[cfg(windows)]
    {
        let _ = home;
        PathBuf::from(format!(r"\\.\pipe\paneflow-authorization-test-{unique}"))
    }
    #[cfg(unix)]
    {
        home.join(format!("a{unique}.sock"))
    }
}

struct Host {
    home: tempfile::TempDir,
    host: Arc<SessionHost>,
    server: Option<ServerHandle>,
    endpoint: PathBuf,
}

impl Host {
    fn start(config: Value) -> Self {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("paneflow.json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        let endpoint = endpoint_in(home.path());
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).unwrap();
        Self {
            home,
            host,
            server: Some(server),
            endpoint,
        }
    }

    fn client(&self, hello: &ClientHello) -> HostClient {
        HostClient::connect(&self.endpoint, hello).unwrap()
    }

    fn session(&self, workspace: &WorkspaceId, args: Vec<String>) -> SessionId {
        let request = CreateSession {
            workspace: Some(workspace.clone()),
            shell: Some(env!("CARGO_BIN_EXE_paneflow-session-fixture").to_string()),
            args,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(240),
            rows: Some(40),
            ..CreateSession::default()
        };
        self.client(&ClientHello::local("authorization-test"))
            .create(&request)
            .unwrap()
            .manifest
            .session
    }

    fn idle(&self, workspace: &WorkspaceId) -> SessionId {
        let session = self.session(workspace, vec!["idle".into()]);
        self.screen_until(&session, "fixture idle");
        session
    }

    fn call_from_pane(
        &self,
        workspace: &WorkspaceId,
        hello: &str,
        method: &str,
        params: Value,
    ) -> (SessionId, String) {
        let args = [
            "call",
            "host",
            &self.endpoint.display().to_string(),
            hello,
            method,
            &params.to_string(),
        ]
        .map(str::to_string)
        .to_vec();
        let session = self.session(workspace, args);
        let screen = self.screen_until(&session, "call done");
        (session, screen)
    }

    fn screen_until(&self, session: &SessionId, marker: &str) -> String {
        let mut client = self.client(&ClientHello::local("authorization-reader"));
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

fn line(screen: &str, prefix: &str) -> Option<String> {
    let joined: String = screen
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("");
    let start = joined.find(prefix)? + prefix.len();
    let rest = &joined[start..];
    let end = rest.find("call ").unwrap_or(rest.len());
    Some(rest[..end].trim().to_string())
}

fn closed_gates() -> Value {
    json!({"ipc": {"scripting": false, "orchestration": false}})
}

fn live(host: &Host, session: &SessionId) -> bool {
    host.client(&ClientHello::control("authorization-probe"))
        .inspect(session)
        .is_ok_and(|summary| summary.live)
}

#[test]
fn a_pane_that_claims_an_engine_cannot_decide_a_write_approval() {
    let host = Host::start(closed_gates());
    let (_, screen) = host.call_from_pane(
        &WorkspaceId::new(),
        "engine",
        "approval.decide",
        json!({"id": 1, "decision": "allow"}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("principal_refused"),
        "{screen}"
    );
}

#[test]
fn a_pane_that_claims_an_engine_is_refused_input_as_a_control_client_is() {
    let host = Host::start(closed_gates());
    let workspace = WorkspaceId::new();
    let target = host.idle(&workspace);
    let (_, screen) = host.call_from_pane(
        &workspace,
        "engine",
        "session.input",
        json!({"session": target.as_str(), "data": "eA=="}),
    );
    assert_eq!(
        line(&screen, "call code=").as_deref(),
        Some("-32601"),
        "{screen}"
    );
    assert_eq!(
        line(&screen, "call error=").as_deref(),
        Some("session.input disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable"),
        "{screen}"
    );
}

#[test]
fn a_pane_writing_as_another_source_session_is_refused_as_a_source_mismatch() {
    let host = Host::start(json!({"ipc": {"scripting": true}}));
    let workspace = WorkspaceId::new();
    let target = host.idle(&workspace);
    let other = host.idle(&workspace);
    let (_, screen) = host.call_from_pane(
        &workspace,
        "control",
        "pane.write",
        json!({"source_session": other.as_str(), "session": target.as_str(), "text": "hi"}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("source_mismatch"),
        "{screen}"
    );
}

#[test]
fn a_pane_stops_a_session_of_its_workspace_and_is_refused_one_of_another() {
    let host = Host::start(closed_gates());
    let workspace = WorkspaceId::new();
    let elsewhere = WorkspaceId::new();
    let foreign = host.idle(&elsewhere);
    let (_, screen) = host.call_from_pane(
        &workspace,
        "control",
        "session.stop",
        json!({"session": foreign.as_str()}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("scope_refused"),
        "{screen}"
    );
    let message = line(&screen, "call error=").unwrap_or_default();
    assert!(message.contains(elsewhere.as_str()), "{screen}");
    assert!(live(&host, &foreign), "the foreign session still runs");

    let neighbour = host.idle(&workspace);
    let (_, screen) = host.call_from_pane(
        &workspace,
        "control",
        "session.stop",
        json!({"session": neighbour.as_str()}),
    );
    assert!(line(&screen, "call ok=").is_some(), "{screen}");
    let deadline = Instant::now() + Duration::from_secs(30);
    while live(&host, &neighbour) {
        assert!(Instant::now() < deadline, "the neighbour never stopped");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_pane_cannot_shut_the_host_down() {
    let host = Host::start(json!({"ipc": {"scripting": true, "orchestration": true}}));
    let (_, screen) = host.call_from_pane(
        &WorkspaceId::new(),
        "engine",
        "host.shutdown",
        json!({"force": true}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("principal_refused"),
        "{screen}"
    );
    assert!(
        host.client(&ClientHello::control("still-there"))
            .call("host.status", json!({}))
            .is_ok()
    );
}

#[test]
fn the_window_and_the_worker_reach_any_workspace_as_before() {
    let host = Host::start(closed_gates());
    let target = host.idle(&WorkspaceId::new());
    let mut worker = host.client(&ClientHello::control("paneflow-serve-authorization"));
    let mut window = host.client(&ClientHello::local("paneflow-desktop"));
    let generation = worker.inspect(&target).unwrap().manifest.generation;
    window
        .call(
            "session.input",
            json!({"session": target.as_str(), "data": "eA==", "generation": generation}),
        )
        .unwrap();
    worker.stop(&target, Some(generation)).unwrap();
    let _ = host.home.path();
}

#[test]
fn a_claimed_scope_session_confines_a_session_write_to_the_claimed_workspace() {
    let host = Host::start(closed_gates());
    let here = WorkspaceId::new();
    let caller = host.idle(&here);
    let foreign = host.idle(&WorkspaceId::new());
    let mut worker = host.client(&ClientHello::control("paneflow-serve-forward"));
    let refused = worker
        .call(
            "session.stop",
            json!({"session": foreign.as_str(), "scope_session": caller.as_str()}),
        )
        .unwrap_err();
    let paneflow_host::HostClientError::Rpc { data, .. } = refused else {
        panic!("an RPC refusal: {refused}");
    };
    assert_eq!(data.unwrap()["reason"], "scope_refused");
    assert!(live(&host, &foreign));
    let neighbour = host.idle(&here);
    worker
        .call(
            "session.stop",
            json!({"session": neighbour.as_str(), "scope_session": caller.as_str()}),
        )
        .unwrap();
}

#[test]
fn a_pane_cannot_move_a_foreign_session_into_its_workspace_nor_resize_it() {
    let host = Host::start(closed_gates());
    let workspace = WorkspaceId::new();
    let elsewhere = WorkspaceId::new();
    let foreign = host.idle(&elsewhere);
    let (_, screen) = host.call_from_pane(
        &workspace,
        "control",
        "session.workspace",
        json!({"session": foreign.as_str(), "workspace": workspace.as_str()}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("principal_refused"),
        "{screen}"
    );
    let summary = host
        .client(&ClientHello::control("authorization-probe"))
        .inspect(&foreign)
        .unwrap();
    assert_eq!(summary.manifest.workspace.as_ref(), Some(&elsewhere));

    let (_, screen) = host.call_from_pane(
        &workspace,
        "control",
        "session.resize",
        json!({"session": foreign.as_str(), "cols": 10, "rows": 5}),
    );
    assert_eq!(
        line(&screen, "call reason=").as_deref(),
        Some("scope_refused"),
        "{screen}"
    );
}
