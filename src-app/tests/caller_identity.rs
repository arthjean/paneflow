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

fn worker_call(
    control: &mut paneflow_ipc_client::host_control::HostControl,
    method: &str,
    params: Value,
) -> Result<Value, Value> {
    let id = control
        .write_request(method, params)
        .expect("the request is written");
    loop {
        let line = control
            .read_stream_line(Duration::from_secs(30))
            .expect("the worker answers")
            .expect("the connection stays open");
        let envelope: Value = serde_json::from_str(&line).expect("a JSON frame");
        if envelope["id"].as_u64() != Some(id) {
            continue;
        }
        if envelope["error"].is_object() {
            return Err(envelope["error"].clone());
        }
        return Ok(envelope["result"].clone());
    }
}

fn attach_as_window_then_whoami_and_sweep(home: &Path) -> (Value, Result<Value, Value>) {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut control = loop {
        match paneflow_ipc_client::host_control::HostControl::connect(&endpoint, "window") {
            Ok(control) => break control,
            Err(error) => {
                assert!(
                    Instant::now() < deadline,
                    "the worker never answered: {error}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    worker_call(
        &mut control,
        "layout.attach",
        serde_json::json!({"client": "window"}),
    )
    .expect("layout.attach answers");
    let whoami = worker_call(&mut control, "system.whoami", serde_json::json!({}))
        .expect("system.whoami answers");
    let sweep = worker_call(
        &mut control,
        "layout.sweep",
        serde_json::json!({"live": false}),
    );
    (whoami, sweep)
}

struct DetachedWorker<'a> {
    home: &'a Path,
}

impl Drop for DetachedWorker<'_> {
    fn drop(&mut self) {
        let _ = paneflow(self.home, &["serve", "stop"]);
        let _ = paneflow(self.home, &["host", "stop", "--force"]);
    }
}

#[test]
fn a_process_that_is_not_paneflow_attaching_as_the_window_stays_external_on_the_worker() {
    let home = tempfile::tempdir().expect("a temporary home");
    let started = paneflow(home.path(), &["serve", "start"]);
    let _worker = DetachedWorker { home: home.path() };
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let (whoami, sweep) = attach_as_window_then_whoami_and_sweep(home.path());
    assert_eq!(whoami, serde_json::json!({"principal": "external"}));
    let refused = sweep.expect_err("layout.sweep belongs to the attached window");
    assert_eq!(refused["data"]["principal"], "external", "{refused}");
}

#[test]
fn a_worker_in_this_process_lets_this_executable_attach_as_the_window() {
    let home = tempfile::tempdir().expect("a temporary home");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let (whoami, sweep) = attach_as_window_then_whoami_and_sweep(home.path());
    assert_eq!(whoami, serde_json::json!({"principal": "window"}));
    if let Err(error) = &sweep {
        assert_ne!(error["data"]["reason"], "principal_refused", "{error}");
    }
    drop(worker);
}
