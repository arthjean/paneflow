#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, SessionId};
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

fn answer(output: &Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("a JSON result")
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
        let text = client.text(session).unwrap().text;
        if text.contains(marker) && text.contains('}') {
            return text;
        }
        assert!(
            Instant::now() < deadline,
            "the pane never printed {marker}: {text}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn whoami_answers_pane_from_a_pane_and_external_outside_on_the_host_and_the_worker() {
    let home = tempfile::tempdir().expect("a temporary home");
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the core serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let mut client =
        HostClient::connect(&core_endpoint, &ClientHello::local("caller-identity")).unwrap();

    for process in ["host", "worker"] {
        let outside = answer(&paneflow(
            home.path(),
            &["api", "call", "system.whoami", "--on", process],
        ));
        assert_eq!(
            outside,
            serde_json::json!({"principal": "external"}),
            "{process}"
        );

        let session = pane_running(
            &mut client,
            &["api", "call", "system.whoami", "--on", process],
        );
        let text = pane_text_with(&mut client, &session, "\"principal\"");
        assert!(
            text.contains("\"principal\": \"pane\""),
            "{process}: {text}"
        );
        assert!(
            text.contains(&format!("\"session\": \"{session}\"")),
            "{process}: {text}"
        );
    }

    drop(worker);
    let _ = core.request_shutdown(true);
    core_server.stop().unwrap();
}
