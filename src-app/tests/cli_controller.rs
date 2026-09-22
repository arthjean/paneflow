#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use paneflow_host::protocol::{ClientHello, METHOD_AGENT_EVENT};
use paneflow_host::{HostClient, SessionSummary};
use serde_json::{Value, json};

const SIDEBAR_BUDGET: Duration = Duration::from_millis(500);
const RECONNECT_BUDGET: Duration = Duration::from_secs(3);
const LINE_WAIT: Duration = Duration::from_secs(10);

struct Follower {
    child: Child,
    lines: Receiver<String>,
}

impl Follower {
    fn spawn(home: &std::path::Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_paneflow"))
            .args(["sessions", "--follow", "--json"])
            .env(paneflow_home::HOME_ENV, home)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the paneflow CLI runs as a separate Controller process");
        let stdout = child.stdout.take().expect("the Controller pipes stdout");
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines }
    }

    fn next_json(&self, budget: Duration) -> Value {
        let deadline = Instant::now() + budget;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left.max(Duration::from_millis(1))) {
                Ok(line) => {
                    if let Ok(value) = serde_json::from_str::<Value>(line.trim())
                        && value.is_object()
                    {
                        return value;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("the CLI Controller printed no frame within {budget:?}")
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("the CLI Controller exited before printing a frame")
                }
            }
        }
    }

    fn next_of_type(&self, kind: &str, budget: Duration) -> Value {
        let deadline = Instant::now() + budget;
        loop {
            let value = self.next_json(deadline.saturating_duration_since(Instant::now()));
            if value["type"] == kind {
                return value;
            }
            assert!(
                Instant::now() < deadline,
                "no {kind} frame arrived within {budget:?}"
            );
        }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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

#[test]
fn the_cli_renders_the_same_session_projection_as_the_app_and_survives_a_worker_restart() {
    let home = tempfile::tempdir().expect("a temporary home");
    for (name, contents) in [
        ("paneflow.json", "{}"),
        ("session.json", "{}"),
        ("window-state.json", "{}"),
        ("telemetry_id", "7f03d6ba-1249-4a78-92dc-96f77e8d10a2"),
    ] {
        std::fs::write(home.path().join(name), contents).expect("isolated Controller state");
    }
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the core serves its endpoint");

    let hello = ClientHello::local("cli-controller-test");
    let mut owner =
        HostClient::connect(&core_endpoint, &hello).expect("the core accepts a controller");
    let created: SessionSummary = serde_json::from_value(
        owner
            .call("session.create", shell_params())
            .expect("a session starts"),
    )
    .expect("a session summary");
    let session = created.manifest.session.clone();
    let generation = created.manifest.generation;

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let follower = Follower::spawn(home.path());

    let bootstrap = follower.next_of_type("bootstrap", LINE_WAIT);
    let capabilities: Vec<String> = bootstrap["capabilities"]
        .as_array()
        .expect("the bootstrap carries capabilities")
        .iter()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .collect();
    assert_eq!(
        capabilities,
        paneflow_serve::advertised_capabilities(),
        "the CLI reads the advertised set instead of probing"
    );
    let known = bootstrap["sessions"]
        .as_array()
        .expect("sessions")
        .iter()
        .find(|row| row["session"] == session.to_string())
        .expect("the CLI lists the live session");
    assert_eq!(known["status"], "idle");
    assert_eq!(known["unread"], false);

    let submitted = Instant::now();
    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.prompt_submit",
                "tool": "claude",
                "emitted_at_ms": 1_000,
                "runtime_generation": generation,
                "hook_payload": {"hook_event_name": "UserPromptSubmit"},
            }),
        )
        .expect("the core accepts the opening hook");
    let busy = follower.next_of_type("event", LINE_WAIT);
    let latency = submitted.elapsed();
    assert_eq!(busy["session"], session.to_string());
    assert_eq!(busy["status"], "busy");
    assert_eq!(busy["activity_source"], "hooks");
    assert_eq!(busy["agent"]["state"], "thinking");
    assert_eq!(busy["runtime_id"], "com.anthropic.claude-code");
    assert_eq!(busy["unread"], false);
    assert!(
        latency < SIDEBAR_BUDGET,
        "the CLI Controller matched the sidebar in {latency:?}, over the {SIDEBAR_BUDGET:?} budget"
    );

    owner
        .call(
            METHOD_AGENT_EVENT,
            json!({
                "session": session,
                "kind": "ai.stop",
                "tool": "claude",
                "emitted_at_ms": 1_100,
                "runtime_generation": generation,
                "hook_payload": {"hook_event_name": "Stop", "last_result": "2 files changed"},
            }),
        )
        .expect("the core accepts the stop hook");
    let settled = follower.next_of_type("event", LINE_WAIT);
    assert_eq!(settled["status"], "idle");
    assert_eq!(settled["notify"]["kind"], "finished");
    assert_eq!(
        settled["unread"], true,
        "a finished turn reaches the CLI as unread, the flag the sidebar badges"
    );

    let acked = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .args(["sessions", "ack", &session.to_string()])
        .env(paneflow_home::HOME_ENV, home.path())
        .status()
        .expect("the CLI acknowledges the attention queue as a separate process");
    assert!(acked.success(), "paneflow sessions ack exited with {acked}");
    let lowered = follower.next_of_type("event", LINE_WAIT);
    assert_eq!(
        lowered["unread"], false,
        "the lowered row reaches every follower: {lowered}"
    );

    worker.stop();
    let restarted = Instant::now();
    let replacement =
        paneflow_serve::open(home.path()).expect("a replacement worker takes the home");
    let startup = restarted.elapsed();
    let resumed = follower.next_of_type("bootstrap", RECONNECT_BUDGET + LINE_WAIT);
    assert!(
        restarted.elapsed() < RECONNECT_BUDGET,
        "the CLI resumed in {:?} (worker startup {startup:?}), over the {RECONNECT_BUDGET:?} budget",
        restarted.elapsed()
    );
    assert_eq!(resumed["resumed"], true);
    assert!(
        resumed["sessions"]
            .as_array()
            .expect("sessions")
            .iter()
            .any(|row| row["session"] == session.to_string()),
        "the fresh bootstrap still carries the whole fleet"
    );
    assert!(
        !resumed["fresh"]
            .as_array()
            .expect("the resumed bootstrap names the rows that moved")
            .iter()
            .any(|row| row["session"] == session.to_string()),
        "a row whose projection did not move is not reprinted after a reconnect: {}",
        resumed["fresh"]
    );

    drop(follower);
    replacement.stop();
    let _ = core_server.stop();
    let _ = core;
}
