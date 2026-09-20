#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, SessionSummary};
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use paneflow_serve::protocol::{METHOD_WORKER_HELLO, METHOD_WORKER_STATUS};
use serde_json::{Value, json};

fn core_endpoint(home: &std::path::Path) -> PathBuf {
    paneflow_home::host_endpoint_path(home)
}

fn shell_params() -> Value {
    #[cfg(windows)]
    let (shell, args) = ("cmd.exe", vec!["/Q", "/D"]);
    #[cfg(unix)]
    let (shell, args) = ("/bin/sh", vec!["-s"]);
    json!({
        "shell": shell,
        "args": args,
        "cwd": std::env::temp_dir().display().to_string(),
        "cols": 80,
        "rows": 24,
    })
}

fn delayed_marker_command(marker: &str) -> String {
    #[cfg(windows)]
    return format!("ping -n 3 127.0.0.1 >NUL && echo {marker}\r\n");
    #[cfg(unix)]
    return format!("sleep 1 && echo {marker}\n");
}

fn controller(endpoint: &std::path::Path) -> HostControl {
    let mut control =
        HostControl::connect(endpoint, "worker-lifecycle-test").expect("a Controller connects");
    control
        .request(
            METHOD_WORKER_HELLO,
            json!({"client": "worker-lifecycle-test"}),
        )
        .expect("the worker answers its identity");
    control
}

fn next_projected_event(follower: &mut HostControl) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let line = follower
            .read_stream_line(Duration::from_secs(5))
            .expect("the follow stream stays readable")
            .expect("the follow stream stays open");
        let frame: Value = serde_json::from_str(&line).expect("a JSON frame");
        if frame["type"] == "event" {
            return frame;
        }
    }
    panic!("no projected event arrived on the worker follow stream");
}

fn output_text(endpoint: &std::path::Path, session: &paneflow_config::schema::SessionId) -> String {
    let hello = ClientHello::local("worker-lifecycle-output");
    let mut client =
        HostClient::connect(endpoint, &hello).expect("the core accepts an output reader");
    let mut bytes = Vec::new();
    client
        .output(
            session,
            None,
            0,
            false,
            |_, chunk| {
                bytes.extend_from_slice(chunk);
                true
            },
            || false,
        )
        .expect("the output tail is readable");
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wait_for_output(
    endpoint: &std::path::Path,
    session: &paneflow_config::schema::SessionId,
    needle: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = output_text(endpoint, session);
        if text.contains(needle) || Instant::now() >= deadline {
            return text;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_worker_owns_the_home_reduces_for_controllers_and_rebuilds_after_a_restart() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(paneflow_home::HOME_ENV, home.path()) };

    let endpoint = core_endpoint(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), endpoint.clone())
        .expect("the core serves its endpoint");

    let hello = ClientHello::local("worker-lifecycle-test");
    let mut owner = HostClient::connect(&endpoint, &hello).expect("the core accepts a controller");
    let created: SessionSummary = serde_json::from_value(
        owner
            .call("session.create", shell_params())
            .expect("a session starts"),
    )
    .unwrap();
    let session = created.manifest.session.clone();
    let generation = created.manifest.generation;
    let child = created.manifest.process.expect("a child process identity");
    assert!(child.is_provably_live());

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let worker_endpoint = paneflow_home::serve_endpoint_path(home.path());

    assert!(
        matches!(
            paneflow_serve::open(home.path()),
            Err(paneflow_serve::WorkerError::AlreadyRunning(_))
        ),
        "a second worker on the same home is refused by the owner lock"
    );

    let mut control = controller(&worker_endpoint);
    let status = control
        .request(METHOD_WORKER_STATUS, json!({}))
        .expect("worker.status");
    assert_eq!(status["pid"].as_u64(), Some(u64::from(std::process::id())));
    assert_eq!(
        status["protocol"].as_u64(),
        Some(u64::from(paneflow_serve::WORKER_PROTOCOL_VERSION))
    );
    assert_eq!(status["home"], home.path().display().to_string());
    assert!(status["session_count"].as_u64().is_some());
    let advertised: Vec<String> = status["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .collect();
    assert_eq!(advertised, paneflow_serve::advertised_capabilities());
    assert!(advertised.contains(&"agent.follow".to_string()));

    let proxied = control
        .request("session.list", json!({}))
        .expect("the worker proxies session.list to the core");
    assert!(
        proxied["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|row| row["session"] == session.to_string()),
        "a Controller sees the core session through the worker: {proxied}"
    );

    let mut follower = controller(&worker_endpoint);
    let header = follower
        .request(METHOD_AGENT_FOLLOW, json!({}))
        .expect("agent.follow");
    assert_eq!(header["following"], true);
    assert!(
        header["capabilities"]
            .as_array()
            .is_some_and(|entries| !entries.is_empty()),
        "the bootstrap advertises capabilities so no Controller probes: {header}"
    );
    let known = header["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|entry| entry["session"] == session.to_string())
        .expect("the worker projects the live session");
    assert_eq!(known["activity_source"], "none");
    assert_eq!(known["status"], "idle");

    let late: SessionSummary = serde_json::from_value(
        owner
            .call("session.create", shell_params())
            .expect("a second session starts after the worker"),
    )
    .unwrap();
    let late_session = late.manifest.session.clone();
    let late_generation = late.manifest.generation;
    let late_child = late.manifest.process.expect("a second child identity");
    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": late_session,
                "kind": "ai.prompt_submit",
                "tool": "codex",
                "emitted_at_ms": 900,
                "runtime_generation": late_generation,
            }),
        )
        .expect("the late session hook reaches the core");
    let late_projected = next_projected_event(&mut follower);
    assert_eq!(late_projected["session"], late_session.to_string());
    assert_eq!(late_projected["agent"]["state"], "thinking");
    assert!(
        late_child.is_provably_live(),
        "discovering a late session never signals it"
    );

    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "emitted_at_ms": 1_000,
                "runtime_generation": generation,
                "hook_payload": {
                    "hook_event_name": "UserPromptSubmit",
                },
            }),
        )
        .expect("the core accepts the hook frame");

    let projected = next_projected_event(&mut follower);
    assert_eq!(projected["session"], session.to_string());
    assert_eq!(projected["kind"], "ai.prompt_submit");
    assert_eq!(
        projected["agent"]["state"], "thinking",
        "the worker reduces, the core did not: {projected}"
    );
    assert_eq!(projected["activity_source"], "hooks");
    assert_eq!(projected["status"], "busy");

    let scrollback_marker = "worker-restart-keeps-scrollback";
    owner
        .input(
            &session,
            generation,
            format!("echo {scrollback_marker}\r\n").as_bytes(),
        )
        .expect("the marker reaches the core-owned PTY");
    let before_restart = wait_for_output(&endpoint, &session, scrollback_marker);
    assert!(
        before_restart.contains(scrollback_marker),
        "the marker enters scrollback before the worker restart: {before_restart:?}"
    );

    let in_flight_marker = "worker-restart-keeps-running-command";
    owner
        .input(
            &session,
            generation,
            delayed_marker_command(in_flight_marker).as_bytes(),
        )
        .expect("the delayed command reaches the core-owned PTY");
    std::thread::sleep(Duration::from_millis(100));

    let fleet = control
        .request("fleet.list", json!({}))
        .expect("fleet.list is answered from the reduced state");
    let agent = fleet["agents"]
        .as_array()
        .expect("agents")
        .iter()
        .find(|agent| agent["session"] == session.to_string())
        .expect("the reduced session is listed");
    assert_eq!(agent["state"], "thinking");
    assert_eq!(agent["reduced_by"], "paneflow-serve");

    drop(control);
    drop(follower);
    worker.stop();

    assert!(
        child.is_provably_live(),
        "stopping the worker never signals a terminal"
    );

    let restarted_at = Instant::now();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home again");
    let rebuilt = worker
        .worker()
        .lock_state()
        .get(&session)
        .cloned()
        .expect("the session is rebuilt from its manifest");
    assert!(
        restarted_at.elapsed() < Duration::from_secs(2),
        "the rebuild stays inside the 2 s budget, took {:?}",
        restarted_at.elapsed()
    );
    assert_eq!(
        rebuilt.status(),
        "busy",
        "the seed on disk replays the turn that was open"
    );
    assert_eq!(
        rebuilt.activity_source,
        paneflow_serve::ActivitySource::Hooks
    );
    assert!(
        child.is_provably_live(),
        "a worker restart leaves every terminal untouched"
    );
    let after_restart = output_text(&endpoint, &session);
    assert!(
        after_restart.contains(scrollback_marker),
        "the core-owned scrollback survives the worker restart: {after_restart:?}"
    );
    let completed_command = wait_for_output(&endpoint, &session, in_flight_marker);
    assert!(
        completed_command.contains(in_flight_marker),
        "a command already running in the core-owned PTY survives the worker restart: {completed_command:?}"
    );

    let mut control = controller(&worker_endpoint);
    let after = control
        .request(METHOD_WORKER_STATUS, json!({}))
        .expect("worker.status after the restart");
    assert_eq!(after["session_count"], 2);
    drop(control);

    let sessions_before = owner
        .call("session.list", json!({}))
        .expect("the core lists its sessions before the replacement");
    let drained_at = Instant::now();
    assert!(
        paneflow_serve::stop_worker(home.path(), paneflow_serve::DRAIN_WAIT),
        "the running worker drains and stops inside its budget"
    );
    assert!(
        drained_at.elapsed() <= paneflow_serve::DRAIN_WAIT,
        "the drain never runs past its budget, took {:?}",
        drained_at.elapsed()
    );
    worker.stop();
    let replacement = paneflow_serve::open(home.path()).expect("the replacement takes the home");
    let sessions_after = owner
        .call("session.list", json!({}))
        .expect("the core lists its sessions after the replacement");
    assert_eq!(
        sessions_before, sessions_after,
        "the sessions list is identical before and after a worker replacement"
    );
    assert!(
        child.is_provably_live(),
        "a worker replacement never signals a terminal"
    );

    replacement.stop();
    let _ = owner.call(
        "session.stop",
        json!({"session": late_session, "generation": late_generation}),
    );
    let _ = owner.call(
        "session.stop",
        json!({"session": session, "generation": generation}),
    );
    drop(owner);
    let _ = core_server.stop();
    unsafe { std::env::remove_var(paneflow_home::HOME_ENV) };
}
