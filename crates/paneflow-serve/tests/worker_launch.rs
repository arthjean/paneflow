#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paneflow_api::methods::layout::{HOST_UNREACHABLE, LAUNCH_FAILED, NOT_ATTACHED};
use paneflow_config::schema::{
    LayoutNode, SESSION_SCHEMA_VERSION, SessionId, SessionState, SurfaceDefinition, TabLayoutIds,
    TabSession, WorkspaceSession,
};
use paneflow_host::CreateSession;
use paneflow_ipc_client::host_control::HostControl;
use paneflow_serve::launcher::{LaunchError, SessionLink, SessionStarter};
use serde_json::{Value, json};

const REPLY_WAIT: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(20);

struct Client {
    control: HostControl,
}

impl Client {
    fn connect(home: &Path) -> Self {
        Self {
            control: HostControl::connect(
                &paneflow_home::serve_endpoint_path(home),
                "worker-launch-test",
            )
            .expect("connects"),
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, Value> {
        let id = self
            .control
            .write_request(method, params)
            .expect("the request is written");
        loop {
            let line = self
                .control
                .read_stream_line(REPLY_WAIT)
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

    fn ok(&mut self, method: &str, params: Value) -> Value {
        self.call(method, params)
            .unwrap_or_else(|error| panic!("{method} failed: {error}"))
    }

    fn lease(&mut self) -> u64 {
        self.ok("layout.lease_ids", json!({}))["start"]
            .as_u64()
            .unwrap()
    }
}

#[derive(Default)]
struct Recorded {
    created: Vec<CreateSession>,
    refuse: Option<String>,
}

#[derive(Clone, Default)]
struct FakeStarter(Arc<Mutex<Recorded>>);

impl FakeStarter {
    fn refusing(message: &str) -> Self {
        let starter = Self::default();
        starter.0.lock().unwrap().refuse = Some(message.to_string());
        starter
    }

    fn created(&self) -> Vec<CreateSession> {
        self.0.lock().unwrap().created.clone()
    }
}

struct FakeLink(Arc<Mutex<Recorded>>);

impl SessionStarter for FakeStarter {
    fn connect(&self) -> Result<Box<dyn SessionLink>, LaunchError> {
        Ok(Box::new(FakeLink(Arc::clone(&self.0))))
    }
}

impl SessionLink for FakeLink {
    fn exists(&mut self, session: &SessionId) -> Result<bool, LaunchError> {
        let recorded = self.0.lock().unwrap();
        Ok(recorded
            .created
            .iter()
            .any(|request| request.session.as_ref() == Some(session)))
    }

    fn create(&mut self, request: &CreateSession) -> Result<(), LaunchError> {
        let mut recorded = self.0.lock().unwrap();
        if let Some(message) = &recorded.refuse {
            return Err(LaunchError::Refused(message.clone()));
        }
        recorded.created.push(request.clone());
        Ok(())
    }
}

fn session_file(cwd: &Path) -> SessionState {
    SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            id: None,
            layout_id: Some(10),
            title: "alpha".into(),
            cwd: cwd.display().to_string(),
            tabs: vec![TabSession {
                title: "shell".into(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![SurfaceDefinition {
                        surface_type: Some("terminal".into()),
                        name: Some("a".into()),
                        focus: Some(true),
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
    }
}

fn prepare_home(home: &Path, cwd: &Path) {
    std::fs::write(
        home.join("session.json"),
        serde_json::to_vec_pretty(&session_file(cwd)).unwrap(),
    )
    .unwrap();
}

fn split_op(pane: u64, surface: u64, definition: Value) -> Value {
    json!({
        "op": "split",
        "target": 12,
        "direction": "vertical",
        "edge": "after",
        "pane": {
            "id": pane,
            "surfaces": [{"id": surface, "definition": definition}],
            "active": 0
        },
        "focus": true
    })
}

fn terminal_definition() -> Value {
    json!({"surface_type": "terminal", "name": "b"})
}

fn surfaces(view: &Value) -> Vec<Value> {
    fn walk(node: &Value, found: &mut Vec<Value>) {
        match node["kind"].as_str() {
            Some("pane") => found.extend(node["surfaces"].as_array().unwrap().iter().cloned()),
            Some("split") => {
                for child in node["children"].as_array().unwrap() {
                    walk(&child["node"], found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for ws in view["view"]["state"]["workspaces"].as_array().unwrap() {
        for tab in ws["tabs"].as_array().unwrap() {
            walk(&tab["root"], &mut found);
        }
    }
    found
}

fn session_of(view: &Value, surface: u64) -> Option<String> {
    surfaces(view)
        .into_iter()
        .find(|known| known["id"].as_u64() == Some(surface))
        .and_then(|known| known["definition"]["session"].as_str().map(str::to_owned))
}

fn open_worker(home: &Path, starter: &FakeStarter) -> paneflow_serve::worker::RunningWorker {
    paneflow_serve::open_with(
        home,
        paneflow_serve::Launching {
            support: paneflow_serve::launcher::LaunchSupport::default(),
            starter: Some(Arc::new(starter.clone())),
        },
    )
    .expect("the worker opens")
}

#[test]
fn an_op_from_a_client_with_no_window_starts_its_terminal_and_binds_the_session() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let starter = FakeStarter::default();
    let worker = open_worker(home.path(), &starter);
    let mut cli = Client::connect(home.path());
    let start = cli.lease();

    let applied = cli.ok(
        "layout.apply",
        json!({"op": split_op(start, start + 1, terminal_definition()), "ids_from": start + 2}),
    );

    let created = starter.created();
    assert_eq!(created.len(), 1, "one session for the one new terminal");
    let request = &created[0];
    assert_eq!(
        request.env.get("PANEFLOW_SURFACE_ID").map(String::as_str),
        Some((start + 1).to_string().as_str())
    );
    assert_eq!(request.cwd.as_deref(), Some(cwd.path().to_str().unwrap()));
    assert_eq!((request.cols, request.rows), (Some(120), Some(40)));
    let view = cli.ok("layout.view", json!({}));
    assert_eq!(
        session_of(&view, start + 1),
        request.session.as_ref().map(ToString::to_string),
        "the worker applied bind_session with the session it created"
    );
    assert_eq!(view["revision"], applied["revision"]);
    worker.stop();
}

#[test]
fn a_refused_create_keeps_the_surface_without_a_session_and_reports_the_host_message() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let starter = FakeStarter::refusing("the pty pool is exhausted");
    let worker = open_worker(home.path(), &starter);
    let mut cli = Client::connect(home.path());
    let start = cli.lease();

    let error = cli
        .call(
            "layout.apply",
            json!({"op": split_op(start, start + 1, terminal_definition()), "ids_from": start + 2}),
        )
        .expect_err("the caller learns the launch failed");
    assert_eq!(error["data"]["reason"], LAUNCH_FAILED);
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("the pty pool is exhausted"),
        "{error}"
    );
    let view = cli.ok("layout.view", json!({}));
    assert!(
        surfaces(&view)
            .iter()
            .any(|known| known["id"].as_u64() == Some(start + 1)),
        "the surface stays in the layout"
    );
    assert_eq!(session_of(&view, start + 1), None);
    worker.stop();
}

#[test]
fn a_creating_op_with_the_host_unreachable_is_refused_and_leaves_the_model_unchanged() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let worker = paneflow_serve::open(home.path()).expect("the worker opens");
    let mut cli = Client::connect(home.path());
    let start = cli.lease();
    let before = cli.ok("layout.view", json!({}));

    let error = cli
        .call(
            "layout.apply",
            json!({"op": split_op(start, start + 1, terminal_definition()), "ids_from": start + 2}),
        )
        .expect_err("no host, no terminal");
    assert_eq!(error["data"]["reason"], HOST_UNREACHABLE, "{error}");
    let after = cli.ok("layout.view", json!({}));
    assert_eq!(after["revision"], before["revision"]);
    assert_eq!(surfaces(&after).len(), 1);

    let renamed = cli.call(
        "layout.apply",
        json!({"op": {"op": "rename_surface", "surface": 13, "name": "kept"}}),
    );
    assert!(renamed.is_ok(), "an op that starts nothing needs no host");
    worker.stop();
}

#[test]
fn an_attached_window_creates_no_session_and_launches_through_the_worker() {
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let starter = FakeStarter::default();
    let worker = open_worker(home.path(), &starter);
    let mut window = Client::connect(home.path());
    window.ok("layout.attach", json!({"client": "window"}));
    let start = window.lease();

    window.ok(
        "layout.apply",
        json!({"op": split_op(start, start + 1, terminal_definition()), "ids_from": start + 2}),
    );
    assert!(
        starter.created().is_empty(),
        "the window's own op leaves the start to the window's launch"
    );
    let session = SessionId::new();
    window.ok(
        "layout.apply",
        json!({"op": {"op": "bind_session", "surface": start + 1, "session": session}}),
    );

    let mut other = Client::connect(home.path());
    let foreign = other
        .call(
            "layout.launch",
            json!({"surface_id": start + 1, "session": session}),
        )
        .expect_err("only the attached client launches");
    assert_eq!(foreign["data"]["reason"], NOT_ATTACHED);

    let launched = window.ok(
        "layout.launch",
        json!({"surface_id": start + 1, "session": session, "cwd": cwd.path(), "cols": 90, "rows": 30}),
    );
    assert!(launched["notices"].as_array().unwrap().is_empty());
    let created = starter.created();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].session.as_ref(), Some(&session));
    assert_eq!((created[0].cols, created[0].rows), (Some(90), Some(30)));

    window.ok(
        "layout.launch",
        json!({"surface_id": start + 1, "session": session}),
    );
    assert_eq!(
        starter.created().len(),
        1,
        "a repeated launch of a known session creates nothing"
    );
    if std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_none()
        && std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_none()
    {
        let gated = window
            .call(
                "layout.launch",
                json!({"surface_id": start + 1, "session": SessionId::new(), "env": {"BASH_ENV": "/tmp/x"}}),
            )
            .expect_err("variables from an attached client still need the orchestration gate");
        assert_eq!(gated["code"], -32601, "{gated}");
        assert_eq!(starter.created().len(), 1);
    }
    let rebound = window
        .call(
            "layout.launch",
            json!({"surface_id": start + 1, "session": SessionId::new()}),
        )
        .expect_err("a surface bound to another session is not relaunched here");
    assert_eq!(rebound["data"]["reason"], "not_found");
    worker.stop();
}

#[test]
fn a_command_in_a_new_surface_from_a_client_with_no_window_needs_orchestration() {
    if std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_some()
        || std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_some()
    {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let starter = FakeStarter::default();
    let worker = open_worker(home.path(), &starter);
    let mut cli = Client::connect(home.path());
    let start = cli.lease();

    let error = cli
        .call(
            "layout.apply",
            json!({
                "op": split_op(start, start + 1, json!({"surface_type": "terminal", "command": "make test"})),
                "ids_from": start + 2
            }),
        )
        .expect_err("a command needs the orchestration gate");
    assert_eq!(error["code"], -32601, "{error}");
    assert!(starter.created().is_empty());
    worker.stop();
}

#[cfg(unix)]
#[test]
fn a_terminal_started_with_no_window_reads_its_own_surface_id() {
    use std::os::unix::fs::PermissionsExt;

    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    prepare_home(home.path(), cwd.path());
    let shell = home.path().join("report-surface.sh");
    std::fs::write(
        &shell,
        "#!/bin/sh\nprintf 'surface=%s\\n' \"$PANEFLOW_SURFACE_ID\"\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": shell, "shell_integration": false})).unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker opens");
    let mut cli = Client::connect(home.path());
    let start = cli.lease();

    cli.ok(
        "layout.apply",
        json!({"op": split_op(start, start + 1, terminal_definition()), "ids_from": start + 2}),
    );
    let view = cli.ok("layout.view", json!({}));
    let session: SessionId = session_of(&view, start + 1)
        .expect("the new surface is bound")
        .parse()
        .unwrap();
    let mut host = paneflow_host::HostClient::connect(
        &core_endpoint,
        &paneflow_host::protocol::ClientHello::control("worker-launch-test"),
    )
    .unwrap();
    let expected = format!("surface={}", start + 1);
    let deadline = Instant::now() + SETTLE;
    loop {
        let text = host
            .text(&session)
            .map(|reply| reply.text)
            .unwrap_or_default();
        if text.contains(&expected) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the terminal never printed {expected}: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let summary = host.inspect(&session).unwrap();
    let _ = host.stop(&session, Some(summary.manifest.generation));
    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}
