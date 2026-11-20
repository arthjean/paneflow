#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use paneflow_api::methods::events::{EventIds, PublishedEvent};
use paneflow_serve::Controller;
use serde_json::Value;

const LINE_WAIT: Duration = Duration::from_secs(20);

struct Follower {
    child: Child,
    lines: Receiver<String>,
}

impl Follower {
    fn spawn(home: &std::path::Path, args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_paneflow"))
            .arg("events")
            .args(args)
            .env(paneflow_home::HOME_ENV, home)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the paneflow CLI starts");
        let stdout = child.stdout.take().expect("stdout is piped");
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

    fn line(&self) -> String {
        match self.lines.recv_timeout(LINE_WAIT) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => panic!("paneflow events printed nothing"),
            Err(RecvTimeoutError::Disconnected) => panic!("paneflow events exited"),
        }
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.line()).expect("one JSON object per line")
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn layout_event(kind: &str, pane: u64) -> PublishedEvent {
    PublishedEvent {
        kind: kind.to_string(),
        ids: EventIds {
            workspace_id: Some(1),
            pane_id: Some(pane),
            ..EventIds::default()
        },
        data: Default::default(),
    }
}

fn publish(endpoint: &std::path::Path, events: &[PublishedEvent]) -> Vec<u64> {
    Controller::connect(endpoint)
        .expect("the worker answers")
        .publish_events(events)
        .expect("the worker takes layout events")
}

#[test]
fn paneflow_events_prints_filtered_events_and_survives_a_worker_restart() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let first = publish(&endpoint, &[layout_event("pane.created", 1)]);

    let follower = Follower::spawn(
        home.path(),
        &[
            "--json",
            "--types",
            "terminal.*,pane.closed,pane.created",
            "--since",
            &(first[0] - 1).to_string(),
        ],
    );
    let replayed = follower.json();
    assert_eq!(replayed["seq"].as_u64(), Some(first[0]));
    assert_eq!(replayed["type"], "pane.created");

    let live = publish(
        &endpoint,
        &[
            layout_event("pane.focused", 1),
            layout_event("pane.closed", 1),
        ],
    );
    let closed = follower.json();
    assert_eq!(
        closed["type"], "pane.closed",
        "pane.focused is filtered out"
    );
    assert_eq!(closed["seq"].as_u64(), Some(live[1]));
    assert_eq!(closed["pane_id"], 1);

    worker.stop();
    let worker = paneflow_serve::open(home.path()).expect("a new worker takes the home");
    let restarted = follower.json();
    assert_eq!(restarted["type"], "restarted", "{restarted}");
    let after = publish(&endpoint, &[layout_event("pane.created", 2)]);
    let event = follower.json();
    assert_eq!(event["seq"].as_u64(), Some(after[0]));
    assert_eq!(event["pane_id"], 2);
    worker.stop();
}

#[test]
fn paneflow_events_prints_one_human_line_per_event() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let first = publish(&endpoint, &[layout_event("pane.created", 3)]);
    let follower = Follower::spawn(home.path(), &["--since", &(first[0] - 1).to_string()]);
    assert_eq!(
        follower.line(),
        format!("{} pane.created workspace_id=1 pane_id=3", first[0])
    );
    worker.stop();
}

#[test]
fn paneflow_events_with_no_worker_exits_with_the_no_instance_code() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .arg("events")
        .env(paneflow_home::HOME_ENV, home.path())
        .output()
        .expect("the paneflow CLI runs");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("worker is not reachable"), "{stderr}");
}
