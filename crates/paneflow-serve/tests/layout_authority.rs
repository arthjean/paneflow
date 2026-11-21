#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_api::methods::layout::{ERR_LAYOUT_REFUSED, LEASE_SIZE};
use paneflow_config::schema::{
    LayoutNode, SESSION_SCHEMA_VERSION, SessionState, SurfaceDefinition, TabLayoutIds, TabSession,
    WorkspaceSession,
};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

mod common;

use common::open_starting_terminals;

const REPLY_WAIT: Duration = Duration::from_secs(30);
const SETTLE: Duration = Duration::from_secs(15);

struct Client {
    control: HostControl,
}

impl Client {
    fn connect(endpoint: &Path) -> Self {
        Self {
            control: HostControl::connect(endpoint, "layout-authority-test").expect("connects"),
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
}

fn terminal(name: &str) -> SurfaceDefinition {
    SurfaceDefinition {
        surface_type: Some("terminal".into()),
        name: Some(name.into()),
        cwd: Some("/work".into()),
        focus: Some(true),
        ..SurfaceDefinition::default()
    }
}

fn session_file(title: &str, revision: u64) -> SessionState {
    SessionState {
        version: SESSION_SCHEMA_VERSION,
        active_workspace: 0,
        workspaces: vec![WorkspaceSession {
            id: None,
            layout_id: Some(10),
            title: title.into(),
            cwd: "/work".into(),
            tabs: vec![TabSession {
                title: "shell".into(),
                layout: Some(LayoutNode::Pane {
                    surfaces: vec![terminal("a")],
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
        layout_revision: Some(revision),
    }
}

fn write_session(home: &Path, state: &SessionState) {
    std::fs::write(
        home.join("session.json"),
        serde_json::to_vec_pretty(state).expect("encodes"),
    )
    .expect("the session file is written");
}

fn read_session(home: &Path) -> Option<SessionState> {
    let bytes = std::fs::read(home.join("session.json")).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + SETTLE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn endpoint(home: &Path) -> PathBuf {
    paneflow_home::serve_endpoint_path(home)
}

fn rename_op(surface: u64, name: &str) -> Value {
    json!({"op": "rename_surface", "surface": surface, "name": name})
}

fn split_op(target: u64, pane: u64, surface: u64) -> Value {
    json!({
        "op": "split",
        "target": target,
        "direction": "vertical",
        "edge": "after",
        "pane": {
            "id": pane,
            "surfaces": [{"id": surface, "definition": {"surface_type": "terminal", "name": "b", "command": null, "cwd": "/work", "env": null, "focus": null, "scrollback": null}}],
            "active": 0
        },
        "focus": true
    })
}

fn surface_names(view: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for ws in view["view"]["state"]["workspaces"].as_array().unwrap() {
        for tab in ws["tabs"].as_array().unwrap() {
            collect_names(&tab["root"], &mut names);
        }
    }
    names
}

fn collect_names(node: &Value, names: &mut Vec<String>) {
    match node["kind"].as_str() {
        Some("pane") => {
            for surface in node["surfaces"].as_array().unwrap() {
                let definition = &surface["definition"];
                let name = definition["custom_name"]
                    .as_str()
                    .or(definition["name"].as_str())
                    .unwrap_or_default();
                names.push(name.to_string());
            }
        }
        Some("split") => {
            for child in node["children"].as_array().unwrap() {
                collect_names(&child["node"], names);
            }
        }
        _ => {}
    }
}

#[test]
fn the_worker_loads_the_session_file_and_serves_its_view_and_revision() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 7));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    let view = client.ok("layout.view", json!({}));
    assert_eq!(view["revision"], 7);
    assert_eq!(view["view"]["revision"], 7);
    assert_eq!(view["attached"], false);
    assert_eq!(view["view"]["state"]["workspaces"][0]["title"], "alpha");
    assert_eq!(view["view"]["state"]["workspaces"][0]["id"], 10);
    assert_eq!(surface_names(&view), ["a"]);
    worker.stop();
}

#[test]
fn an_attached_client_is_recorded_until_its_connection_closes() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let endpoint = endpoint(home.path());
    let mut window = Client::connect(&endpoint);
    let attached = window.ok("layout.attach", json!({"client": "window"}));
    assert_eq!(attached["revision"], 3);
    assert!(!attached["epoch"].as_str().unwrap().is_empty());
    assert_eq!(attached["view"]["state"]["workspaces"][0]["title"], "alpha");
    let mut observer = Client::connect(&endpoint);
    let status = observer.ok("worker.status", json!({}));
    assert_eq!(status["layout"]["attached"], json!(["window"]));
    assert_eq!(observer.ok("layout.view", json!({}))["attached"], true);
    drop(window);
    wait_for("the attachment ends with its connection", || {
        observer.ok("worker.status", json!({}))["layout"]["attached"] == json!([])
    });
    worker.stop();
}

#[test]
fn an_applied_op_returns_its_revision_and_events_and_a_refusal_is_typed() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    let applied = client.ok("layout.apply", json!({"op": rename_op(13, "build")}));
    assert_eq!(applied["revision"], 4);
    let kinds: Vec<&str> = applied["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert!(kinds.contains(&"surface.renamed"), "{kinds:?}");
    let refused = client
        .call("layout.apply", json!({"op": rename_op(999, "x")}))
        .expect_err("an unknown surface is refused");
    assert_eq!(refused["code"], ERR_LAYOUT_REFUSED);
    assert_eq!(refused["data"]["reason"], "not_found");
    let malformed = client
        .call("layout.apply", json!({"op": {"op": "no_such_op"}}))
        .expect_err("an unknown op is refused");
    assert_eq!(
        malformed["code"],
        paneflow_serve::protocol::ERR_INVALID_PARAMS
    );
    assert_eq!(client.ok("layout.view", json!({}))["revision"], 4);
    worker.stop();
}

#[test]
fn a_worker_nobody_attached_to_or_changed_never_writes_the_session_file() {
    let home = tempfile::tempdir().unwrap();
    let original = serde_json::to_vec_pretty(&session_file("alpha", 3)).unwrap();
    std::fs::write(home.path().join("session.json"), &original).unwrap();
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    client.ok("layout.view", json!({}));
    std::thread::sleep(Duration::from_millis(600));
    let status = client.ok("worker.status", json!({}));
    assert_eq!(status["layout"]["writes"], 0);
    worker.stop();
    assert_eq!(
        std::fs::read(home.path().join("session.json")).unwrap(),
        original
    );
}

#[test]
fn an_accepted_op_reaches_the_file_after_the_debounce_and_a_clean_stop_flushes_the_last_one() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    client.ok("layout.apply", json!({"op": rename_op(13, "first")}));
    wait_for("the debounced write", || {
        read_session(home.path()).is_some_and(|state| {
            state.layout_revision == Some(4)
                && state.workspaces[0].tabs[0]
                    .layout
                    .as_ref()
                    .is_some_and(|layout| format!("{layout:?}").contains("first"))
        })
    });
    client.ok("layout.apply", json!({"op": rename_op(13, "last")}));
    drop(client);
    worker.stop();
    let saved = read_session(home.path()).expect("the file reads back");
    assert_eq!(saved.layout_revision, Some(5));
    assert!(format!("{:?}", saved.workspaces[0].tabs[0].layout).contains("last"));
    let restarted = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    assert_eq!(client.ok("layout.view", json!({}))["revision"], 5);
    restarted.stop();
}

#[cfg(unix)]
#[test]
fn a_failed_write_keeps_the_model_in_memory_and_reports_the_failure() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).unwrap();
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    let attached = client.ok("layout.attach", json!({"client": "window"}));
    assert_eq!(attached["view"]["state"]["workspaces"], json!([]));
    let mut model = paneflow_layout::Model::new();
    let lease = client.ok("layout.lease_ids", json!({}));
    let ids = model.ids();
    ids.grant(paneflow_layout::IdBlock {
        start: lease["start"].as_u64().unwrap(),
        end: lease["end"].as_u64().unwrap(),
    });
    let pane = model.reserve_pane(vec![SurfaceDefinition::default()], 0);
    let workspace = paneflow_layout::WorkspaceId(model.reserve());
    let tab = paneflow_layout::TabId(model.reserve());
    let ids_from = model.next_id();
    let op = paneflow_layout::Op::CreateWorkspace {
        workspace: paneflow_layout::NewWorkspace {
            id: workspace,
            title: "kept".into(),
            cwd: "/work".into(),
            space: None,
            durable_id: None,
            custom_buttons: Vec::new(),
            managed_worktrees: Vec::new(),
            tabs: vec![paneflow_layout::NewTab {
                id: tab,
                title: String::new(),
                title_source: paneflow_config::schema::TabTitleSource::Preset,
                worktree: None,
                root: Some(paneflow_layout::NewNode::Pane(pane)),
            }],
            active_tab: 0,
        },
        select: true,
    };
    client.ok(
        "layout.apply",
        json!({"op": serde_json::to_value(&op).unwrap(), "ids_from": ids_from}),
    );
    wait_for("the write failure is reported", || {
        client.ok("worker.status", json!({}))["layout"]["write_error"].is_string()
    });
    let view = client.ok("layout.view", json!({}));
    assert_eq!(view["view"]["state"]["workspaces"][0]["title"], "kept");
    std::fs::set_permissions(home.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    wait_for(
        "the next attempt writes once the home is writable again",
        || read_session(home.path()).is_some(),
    );
    worker.stop();
}

#[test]
fn an_unreadable_session_file_starts_an_empty_model_and_is_kept_aside_before_the_first_write() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("session.json"), b"{ not json").unwrap();
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    let view = client.ok("layout.view", json!({}));
    assert_eq!(view["view"]["state"]["workspaces"], json!([]));
    assert_eq!(
        std::fs::read(home.path().join("session.json")).unwrap(),
        b"{ not json"
    );
    client.ok("layout.attach", json!({}));
    let empty = paneflow_layout::Model::new().snapshot();
    client.ok(
        "layout.adopt",
        json!({"view": serde_json::to_value(&empty).unwrap(), "acknowledged": 0}),
    );
    wait_for("the first write", || read_session(home.path()).is_some());
    let kept: Vec<PathBuf> = std::fs::read_dir(home.path())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().contains("unreadable"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(std::fs::read(&kept[0]).unwrap(), b"{ not json");
    worker.stop();

    let missing = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(missing.path()).unwrap();
    let mut client = Client::connect(&endpoint(missing.path()));
    assert_eq!(
        client.ok("layout.view", json!({}))["view"]["state"]["workspaces"],
        json!([])
    );
    worker.stop();
    assert!(!missing.path().join("session.json").exists());
}

#[cfg(unix)]
#[test]
fn a_window_of_an_earlier_build_owns_the_layout_and_the_worker_reloads_its_file_afterwards() {
    use interprocess::local_socket::{GenericFilePath, ListenerOptions, prelude::*};
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let window_socket = paneflow_home::ipc_endpoint_path(home.path());
    if let Some(parent) = window_socket.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let listener = ListenerOptions::new()
        .name(
            window_socket
                .clone()
                .to_fs_name::<GenericFilePath>()
                .unwrap(),
        )
        .create_sync()
        .expect("an earlier window listens on the home's IPC endpoint");
    let mut client = Client::connect(&endpoint(home.path()));
    let refused = client
        .call("layout.apply", json!({"op": rename_op(13, "cli")}))
        .expect_err("the earlier window owns the layout");
    assert_eq!(refused["code"], ERR_LAYOUT_REFUSED);
    assert_eq!(refused["data"]["reason"], "window_owns_layout");
    let unleased = client
        .call("layout.lease_ids", json!({}))
        .expect_err("the earlier window owns the layout and its file");
    assert_eq!(unleased["data"]["reason"], "window_owns_layout");
    assert_eq!(
        client.ok("worker.status", json!({}))["layout"]["writes"],
        0,
        "a lease wrote the session file an earlier window owns"
    );
    write_session(home.path(), &session_file("written-by-the-old-window", 3));
    drop(listener);
    let _ = std::fs::remove_file(&window_socket);
    client.ok("layout.apply", json!({"op": rename_op(13, "cli")}));
    let view = client.ok("layout.view", json!({}));
    assert_eq!(
        view["view"]["state"]["workspaces"][0]["title"],
        "written-by-the-old-window"
    );
    assert_eq!(surface_names(&view), ["cli"]);
    worker.stop();
}

#[test]
fn a_lease_is_a_contiguous_block_and_only_its_unused_ids_create_elements() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = open_starting_terminals(home.path());
    let endpoint = endpoint(home.path());
    let mut client = Client::connect(&endpoint);
    let first = client.ok("layout.lease_ids", json!({}));
    let start = first["start"].as_u64().unwrap();
    assert_eq!(first["end"].as_u64().unwrap() - start, LEASE_SIZE);
    assert!(start >= 20 + LEASE_SIZE, "the start skips a block: {start}");

    let outside = client
        .call(
            "layout.apply",
            json!({"op": split_op(12, start + LEASE_SIZE + 5, start + LEASE_SIZE + 6)}),
        )
        .expect_err("ids outside the lease are refused");
    assert_eq!(outside["data"]["reason"], "invalid");

    let mut other = Client::connect(&endpoint);
    let foreign = other
        .call(
            "layout.apply",
            json!({"op": split_op(12, start, start + 1)}),
        )
        .expect_err("another connection cannot use this lease");
    assert_eq!(foreign["data"]["reason"], "invalid");

    client.ok(
        "layout.apply",
        json!({"op": split_op(12, start, start + 1), "ids_from": start + 2}),
    );
    let reused = client
        .call(
            "layout.apply",
            json!({"op": split_op(12, start, start + 1), "ids_from": start + 4}),
        )
        .expect_err("a used id cannot create a second element");
    assert_eq!(reused["data"]["reason"], "invalid");

    let second = client.ok("layout.lease_ids", json!({}));
    let second_start = second["start"].as_u64().unwrap();
    assert!(second_start >= first["end"].as_u64().unwrap());
    client.ok(
        "layout.apply",
        json!({"op": split_op(start, second_start, second_start + 1), "ids_from": second_start + 2}),
    );
    client.ok(
        "layout.apply",
        json!({"op": split_op(12, start + 10, start + 11), "ids_from": start + 12}),
    );
    drop(client);
    drop(other);
    worker.stop();

    let persisted = read_session(home.path()).unwrap();
    let next = persisted.next_layout_id.unwrap();
    assert!(next >= second["end"].as_u64().unwrap(), "{next}");
    let restarted = open_starting_terminals(home.path());
    let mut client = Client::connect(&paneflow_home::serve_endpoint_path(home.path()));
    let after = client.ok("layout.lease_ids", json!({}));
    assert!(
        after["start"].as_u64().unwrap() >= next + LEASE_SIZE,
        "a restarted worker advances one full block past the persisted counter"
    );
    restarted.stop();
}

#[test]
fn adopt_takes_the_client_model_unless_the_worker_moved_past_the_acknowledged_revision() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let endpoint = endpoint(home.path());
    let mut window = Client::connect(&endpoint);
    let attached = window.ok("layout.attach", json!({"client": "window"}));
    let mut snapshot: paneflow_layout::Snapshot =
        serde_json::from_value(attached["view"].clone()).unwrap();
    snapshot.state.workspaces[0].title = "from-the-window".into();
    let adopted = window.ok(
        "layout.adopt",
        json!({"view": serde_json::to_value(&snapshot).unwrap(), "acknowledged": 3}),
    );
    assert_eq!(adopted["accepted"], true);
    let revision = adopted["revision"].as_u64().unwrap();
    assert!(revision > 3);

    let mut cli = Client::connect(&endpoint);
    cli.ok("layout.apply", json!({"op": rename_op(13, "moved")}));
    snapshot.state.workspaces[0].title = "stale".into();
    let refused = window.ok(
        "layout.adopt",
        json!({"view": serde_json::to_value(&snapshot).unwrap(), "acknowledged": revision}),
    );
    assert_eq!(refused["accepted"], false);
    assert_eq!(
        refused["view"]["state"]["workspaces"][0]["title"],
        "from-the-window"
    );
    worker.stop();
}

#[test]
fn a_session_file_rewritten_behind_the_worker_is_read_again_before_a_window_attaches() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    client.ok("layout.apply", json!({"op": rename_op(13, "moved")}));
    client.ok("layout.apply", json!({"op": rename_op(13, "again")}));
    assert_eq!(client.ok("layout.view", json!({}))["revision"], 5);
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_session(home.path(), &session_file("rewritten", 1));

    let mut window = Client::connect(&endpoint(home.path()));
    let attached = window.ok("layout.attach", json!({"client": "window"}));

    assert_eq!(attached["revision"], 1);
    assert_eq!(
        attached["view"]["state"]["workspaces"][0]["title"],
        "rewritten"
    );
    let view = attached["view"].clone();
    let adopted = window.ok("layout.adopt", json!({"view": view, "acknowledged": 1}));
    assert_eq!(adopted["accepted"], true, "{adopted}");
    worker.stop();
}

#[test]
fn a_session_file_rewritten_behind_an_unattached_worker_is_read_again_before_an_op_or_a_lease() {
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let mut client = Client::connect(&endpoint(home.path()));
    assert_eq!(
        client.ok("layout.view", json!({}))["view"]["state"]["workspaces"][0]["title"],
        "alpha"
    );

    write_session(home.path(), &session_file("written-by-the-old-window", 9));
    client.ok("layout.apply", json!({"op": rename_op(13, "cli")}));
    let view = client.ok("layout.view", json!({}));
    assert_eq!(
        view["view"]["state"]["workspaces"][0]["title"],
        "written-by-the-old-window"
    );
    assert_eq!(surface_names(&view), ["cli"]);
    wait_for("the op reaching the file", || {
        read_session(home.path()).is_some_and(|state| {
            state.workspaces[0].title == "written-by-the-old-window"
                && state.layout_revision > Some(9)
        })
    });

    write_session(home.path(), &session_file("written-again", 20));
    client.ok("layout.lease_ids", json!({}));
    assert_eq!(
        read_session(home.path()).unwrap().workspaces[0].title,
        "written-again",
        "a lease must not write the model the worker held before the file changed"
    );
    worker.stop();
}

#[test]
fn a_model_a_window_handed_over_is_not_replaced_by_the_file_after_an_earlier_refusal() {
    use interprocess::local_socket::{GenericFilePath, ListenerOptions, prelude::*};
    let home = tempfile::tempdir().unwrap();
    write_session(home.path(), &session_file("alpha", 3));
    let worker = paneflow_serve::open(home.path()).unwrap();
    let window_socket = paneflow_home::ipc_endpoint_path(home.path());
    if let Some(parent) = window_socket.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let listener = ListenerOptions::new()
        .name(
            window_socket
                .clone()
                .to_fs_name::<GenericFilePath>()
                .unwrap(),
        )
        .create_sync()
        .expect("a window listens before it attaches");
    let mut cli = Client::connect(&endpoint(home.path()));
    cli.call("layout.apply", json!({"op": rename_op(13, "cli")}))
        .expect_err("the window that listens owns the layout until it attaches");

    let mut window = Client::connect(&endpoint(home.path()));
    let attached = window.ok("layout.attach", json!({"client": "window"}));
    let mut view = attached["view"].clone();
    view["state"]["workspaces"][0]["title"] = json!("handed-over");
    let adopted = window.ok(
        "layout.adopt",
        json!({"view": view, "acknowledged": attached["revision"]}),
    );
    assert_eq!(adopted["accepted"], true, "{adopted}");
    window.ok("layout.apply", json!({"op": rename_op(13, "kept")}));

    let held = window.ok("layout.view", json!({}));
    assert_eq!(
        held["view"]["state"]["workspaces"][0]["title"], "handed-over",
        "the op after the handover reloaded session.json over the window's model"
    );
    assert_eq!(surface_names(&held), ["kept"]);
    drop(listener);
    worker.stop();
}
