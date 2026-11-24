#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use paneflow_config::schema::{
    LayoutNode, SESSION_SCHEMA_VERSION, SessionState, SurfaceDefinition, TabLayoutIds, TabSession,
    WorkspaceSession,
};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const PATIENCE: Duration = Duration::from_secs(30);

static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Recorder;

impl log::Log for Recorder {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        RECORDS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(format!("{} {}", record.target(), record.args()));
    }

    fn flush(&self) {}
}

fn call(home: &Path, method: &str, params: Value) -> Result<Value, String> {
    HostControl::connect(&paneflow_home::serve_endpoint_path(home), "token-test")
        .unwrap()
        .request_with_deadline(method, params, PATIENCE)
}

fn next_requested(follower: &mut HostControl) -> Value {
    loop {
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" {
            return frame;
        }
    }
}

fn seed_session(home: &Path) {
    let state = SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            id: None,
            layout_id: Some(10),
            title: "alpha".into(),
            cwd: "/work".into(),
            tabs: vec![TabSession {
                title: "shell".into(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition {
                        surface_type: Some("terminal".into()),
                        name: Some("a".into()),
                        cwd: Some("/work".into()),
                        ..SurfaceDefinition::default()
                    }],
                }),
                layout_ids: Some(TabLayoutIds {
                    tab: 11,
                    panes: vec![12],
                    surfaces: vec![vec![13]],
                }),
                ..TabSession::default()
            }],
            active_tab: 0,
            legacy_layout: None,
            custom_buttons: Vec::new(),
            expanded_paths: Vec::new(),
            managed_worktrees: Vec::new(),
            sidebar_collapsed: false,
            muted: false,
            space: None,
        }],
        detached_panes: Vec::new(),
        active_space: None,
        next_layout_id: Some(20),
        layout_revision: Some(1),
    };
    std::fs::write(
        home.join("session.json"),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
}

fn files_under(root: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn a_token_reaches_only_its_requested_frame_never_a_log_a_file_or_another_reply() {
    log::set_logger(&Recorder).unwrap();
    log::set_max_level(log::LevelFilter::Trace);
    let home = tempfile::tempdir().unwrap();
    seed_session(home.path());
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut follower =
        HostControl::connect(&paneflow_home::serve_endpoint_path(home.path()), "window").unwrap();
    follower
        .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
        .unwrap();
    let asked = json!({"kind": "input", "title": "Deploy?", "origin": {"kind": "agent", "agent": "claude", "session": "s1"}});
    let mut replies = vec![call(home.path(), "prompt.request", asked).unwrap()];
    let shown = next_requested(&mut follower);
    let token = shown["token"].as_str().unwrap().to_string();
    let id = shown["id"].clone();
    replies.push(call(home.path(), "prompt.wait", json!({"id": id, "wait_ms": 0})).unwrap());
    let refused = call(
        home.path(),
        "prompt.reply",
        json!({"id": id, "answer": "no", "token": "f".repeat(32)}),
    )
    .unwrap_err();
    replies.push(Value::String(refused));
    replies.push(
        call(
            home.path(),
            "prompt.reply",
            json!({"id": id, "answer": "yes", "token": token}),
        )
        .unwrap(),
    );
    replies.push(call(home.path(), "prompt.wait", json!({"id": id, "wait_ms": 0})).unwrap());
    call(
        home.path(),
        "layout.apply",
        json!({"op": {"op": "rename_surface", "surface": 13, "name": "after-the-prompt"}}),
    )
    .unwrap();
    let saved = home.path().join("session.json");
    let deadline = std::time::Instant::now() + PATIENCE;
    while !std::fs::read_to_string(&saved)
        .unwrap_or_default()
        .contains("after-the-prompt")
    {
        assert!(
            std::time::Instant::now() < deadline,
            "the worker never rewrote session.json"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    for (method, params) in [
        ("worker.status", json!({})),
        ("worker.snapshot", json!({})),
        ("worker.activity_log", json!({})),
        ("config.status", json!({})),
        ("layout.view", json!({})),
    ] {
        if let Ok(reply) = call(home.path(), method, params) {
            replies.push(reply);
        }
    }
    worker.stop();
    let files = files_under(home.path());
    for reply in &replies {
        assert!(
            !reply.to_string().contains(&token),
            "a reply carries the token: {reply}"
        );
    }
    for path in files {
        let bytes = std::fs::read(&path).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(&token),
            "{} holds the token",
            path.display()
        );
    }
    let records = RECORDS.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(!records.is_empty(), "the worker logged while it ran");
    for record in records.iter() {
        assert!(
            !record.contains(&token),
            "a log line holds the token: {record}"
        );
    }
}
