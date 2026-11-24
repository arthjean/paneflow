#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_api::grants::{GrantHolder, GrantOrigin, GrantScope, Right};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, SessionId};
use paneflow_serve::pane_grants::NewGrant;
use serde_json::Value;

fn paneflow(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .args(args)
        .env(paneflow_home::HOME_ENV, home)
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        command.env_remove(key);
    }
    command.output().expect("the paneflow CLI runs")
}

fn listed(home: &Path) -> Value {
    let output = paneflow(home, &["grants", "ls", "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("a JSON grant set")
}

fn pane_running(client: &mut HostClient, args: &[&str]) -> SessionId {
    let request = CreateSession {
        workspace: Some(paneflow_config::schema::WorkspaceId::new()),
        shell: Some(env!("CARGO_BIN_EXE_paneflow").to_string()),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(200),
        rows: Some(40),
        ..CreateSession::default()
    };
    client.create(&request).unwrap().manifest.session
}

fn pane_text_with(client: &mut HostClient, session: &SessionId, marker: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let text = client
            .text(session)
            .map(|reply| reply.text)
            .unwrap_or_default();
        if text.contains(marker) {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "the pane never printed {marker}: {text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_running(worker: &paneflow_serve::worker::RunningWorker, session: &SessionId) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while !worker
        .worker()
        .lock_state()
        .running_sessions()
        .contains(session.as_str())
    {
        assert!(Instant::now() < deadline, "the worker never saw {session}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn grants_ls_lists_and_grants_revoke_revokes_a_grant_with_its_delegations() {
    let home = tempfile::tempdir().expect("a temporary home");
    std::fs::write(
        home.path().join("paneflow.json"),
        r#"{"shell_integration": false, "ipc": {}}"#,
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the core serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let mut client =
        HostClient::connect(&core_endpoint, &ClientHello::local("cli-grants")).unwrap();
    let holder = pane_running(&mut client, &["events"]);
    wait_running(&worker, &holder);
    let grant = |delegated_from: Option<String>| NewGrant {
        holder: GrantHolder {
            session: holder.to_string(),
            generation: 1,
            workspace: None,
        },
        rights: vec![Right::Write],
        scope: GrantScope::Workspace,
        expires_at_ms: None,
        delegated_from,
        origin: GrantOrigin::Prompt,
    };
    let store = &worker.worker().pane_grants;
    let now = paneflow_host::manifest::now_ms();
    let (parent, _) = store.create(grant(None), now);
    let (child, change) = store.create(grant(Some(parent.id.clone())), now);
    worker.worker().publish_grant_change(change);

    let set = listed(home.path());
    let ids: Vec<&str> = set["grants"]
        .as_array()
        .unwrap()
        .iter()
        .map(|grant| grant["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec![parent.id.as_str(), child.id.as_str()], "{set}");
    let human = paneflow(home.path(), &["grants", "ls"]);
    let printed = String::from_utf8_lossy(&human.stdout);
    assert!(printed.contains(&parent.id), "{printed}");
    assert!(printed.contains("write"), "{printed}");

    let stranger = pane_running(&mut client, &["grants", "revoke", &parent.id]);
    let text = pane_text_with(&mut client, &stranger, "was not delegated");
    assert!(
        text.contains("was not delegated by the calling pane"),
        "{text}"
    );
    assert_eq!(listed(home.path())["grants"].as_array().unwrap().len(), 2);

    let revoked = paneflow(home.path(), &["grants", "revoke", &parent.id]);
    assert!(
        revoked.status.success(),
        "{}",
        String::from_utf8_lossy(&revoked.stderr)
    );
    let printed = String::from_utf8_lossy(&revoked.stdout);
    assert!(
        printed.contains(&format!("revoked {}, {}", parent.id, child.id)),
        "{printed}"
    );
    assert_eq!(listed(home.path())["grants"], serde_json::json!([]));

    worker.stop();
    let _ = core.request_shutdown(true);
    core_server.stop().unwrap();
}
