#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

use std::path::Path;
use std::time::Duration;

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

mod common;

const REPLY_WAIT: Duration = Duration::from_secs(30);

struct Client {
    control: HostControl,
}

impl Client {
    fn connect(home: &Path) -> Self {
        Self {
            control: HostControl::connect(
                &paneflow_home::serve_endpoint_path(home),
                "worker-structural-test",
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

fn surface(view: &Value, id: u64) -> Value {
    surfaces(view)
        .into_iter()
        .find(|known| known["id"].as_u64() == Some(id))
        .unwrap_or_else(|| panic!("surface {id} is in the view"))
}

#[test]
fn the_structural_methods_build_a_layout_with_running_terminals_and_no_window() {
    let home = tempfile::tempdir().unwrap();
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let worker = common::open_starting_terminals(home.path());
    let mut cli = Client::connect(home.path());

    let opened = cli.ok(
        "workspace.up",
        json!({"name": "api", "panes": [
            {"cwd": a.path(), "label": "server"},
            {"cwd": b.path(), "label": "server", "focus": true}
        ]}),
    );
    assert_eq!(opened["title"], "api");
    assert_eq!(opened["panes"], 2);
    assert_eq!(opened["labels"], json!(["server", "server-2"]));
    let ids: Vec<u64> = opened["surface_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect();
    assert_eq!(ids.len(), 2);
    let index = opened["index"].as_u64().unwrap();

    let view = cli.ok("layout.view", json!({}));
    for id in &ids {
        assert!(
            surface(&view, *id)["definition"]["session"].is_string(),
            "every pane of workspace.up has its terminal"
        );
    }
    assert_eq!(
        view["view"]["state"]["active_workspace"].as_u64(),
        Some(index),
        "workspace.up selects the new workspace"
    );
    let canonical_a =
        paneflow_launch::cwd::strip_verbatim_prefix(std::fs::canonicalize(a.path()).unwrap());
    assert_eq!(
        surface(&view, ids[0])["definition"]["cwd"].as_str(),
        canonical_a.to_str()
    );

    let split = cli.ok(
        "surface.split",
        json!({"surface_id": ids[0], "direction": "horizontal", "label": "logs"}),
    );
    assert_eq!(split["split"], true);
    assert_eq!(split["direction"], "horizontal");
    assert_eq!(split["panes"], 3);
    let logs = split["surface_id"].as_u64().unwrap();
    let view = cli.ok("layout.view", json!({}));
    let made = surface(&view, logs);
    assert!(made["definition"]["session"].is_string());
    assert_eq!(made["definition"]["custom_name"], "logs");
    assert_eq!(
        made["definition"]["cwd"].as_str(),
        canonical_a.to_str(),
        "a split with no cwd starts in the workspace directory"
    );

    let renamed = cli.ok(
        "surface.rename",
        json!({"surface_id": logs, "name": "  tail\u{7}  "}),
    );
    assert_eq!(renamed, json!({"renamed": true, "name": "tail"}));
    let view = cli.ok("layout.view", json!({}));
    assert_eq!(surface(&view, logs)["definition"]["custom_name"], "tail");

    let created = cli.ok(
        "workspace.create",
        json!({"name": "notes", "cwd": b.path()}),
    );
    assert_eq!(created["title"], "notes");
    assert_eq!(created["panes"], 1);
    let notes = created["index"].as_u64().unwrap();
    let view = cli.ok("layout.view", json!({}));
    assert_eq!(
        view["view"]["state"]["active_workspace"].as_u64(),
        Some(index),
        "workspace.create does not select the new workspace"
    );

    let focused = cli.ok("surface.focus", json!({"surface_id": logs}));
    assert_eq!(
        focused,
        json!({"focused": true, "surface_id": logs, "workspace": index, "scope": "workspace"})
    );
    let selected = cli.ok("workspace.select", json!({"index": notes}));
    assert_eq!(selected, json!({"selected": notes}));
    let view = cli.ok("layout.view", json!({}));
    assert_eq!(
        view["view"]["state"]["active_workspace"].as_u64(),
        Some(notes)
    );
    let out_of_bounds = cli
        .call("workspace.select", json!({"index": 40}))
        .expect_err("no workspace 40");
    assert!(
        out_of_bounds["message"]
            .as_str()
            .unwrap()
            .contains("index 40 is out of bounds"),
        "{out_of_bounds}"
    );

    let restored = cli.ok(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{"type": "terminal"}]},
            {"type": "pane", "surfaces": [{"type": "terminal"}]}
        ]}}),
    );
    assert_eq!(restored, json!({"restored": true, "panes": 2}));

    let closed = cli.ok("workspace.close", json!({"index": notes}));
    assert_eq!(closed, json!({"closed": notes}));
    let view = cli.ok("layout.view", json!({}));
    assert_eq!(
        view["view"]["state"]["workspaces"]
            .as_array()
            .unwrap()
            .len() as u64,
        notes
    );
    worker.stop();
}

#[test]
fn a_surface_name_of_a_restored_layout_names_the_pane_it_opens() {
    let home = tempfile::tempdir().unwrap();
    let worker = common::open_starting_terminals(home.path());
    let mut cli = Client::connect(home.path());
    cli.ok("workspace.up", json!({"name": "api", "panes": [{}]}));
    let before: Vec<u64> = surfaces(&cli.ok("layout.view", json!({})))
        .iter()
        .map(|known| known["id"].as_u64().unwrap())
        .collect();

    let restored = cli.ok(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{"type": "terminal", "name": "kept-pane"}]},
            {"type": "pane", "surfaces": [{"type": "terminal", "name": "  rl-logs  "}]},
            {"type": "pane", "surfaces": [{"type": "terminal", "name": "title", "custom_name": "chosen"}]},
            {"type": "pane", "surfaces": [{"type": "terminal"}]}
        ]}}),
    );
    assert_eq!(restored["restored"], true, "{restored}");

    let view = cli.ok("layout.view", json!({}));
    let reused = surface(&view, before[0]);
    assert_eq!(reused["definition"]["custom_name"], Value::Null, "{reused}");
    let opened: Vec<Value> = surfaces(&view)
        .into_iter()
        .filter(|known| !before.contains(&known["id"].as_u64().unwrap()))
        .map(|known| known["definition"]["custom_name"].clone())
        .collect();
    assert_eq!(opened, [json!("rl-logs"), json!("chosen"), Value::Null]);
    worker.stop();
}

#[test]
fn a_structural_method_that_carries_a_command_needs_orchestration_with_no_window() {
    if std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_some()
        || std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_some()
    {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let worker = common::open_starting_terminals(home.path());
    let mut cli = Client::connect(home.path());
    for (method, params) in [
        ("workspace.up", json!({"panes": [{"command": "make"}]})),
        ("workspace.up", json!({"panes": [{"env": {"A": "1"}}]})),
        (
            "surface.split",
            json!({"direction": "vertical", "prompt": "review"}),
        ),
        (
            "workspace.create",
            json!({"layout": {"type": "pane", "surfaces": [{"type": "terminal", "command": "make"}]}}),
        ),
    ] {
        let refused = cli.call(method, params).expect_err("gated");
        assert_eq!(refused["code"], -32601, "{method}: {refused}");
        assert!(
            refused["message"]
                .as_str()
                .unwrap()
                .contains("PANEFLOW_IPC_ORCHESTRATION=1"),
            "{refused}"
        );
    }
    worker.stop();
}

#[test]
fn a_split_with_an_unknown_surface_or_a_missing_cwd_is_refused_like_the_window_does() {
    let home = tempfile::tempdir().unwrap();
    let worker = common::open_starting_terminals(home.path());
    let mut cli = Client::connect(home.path());
    let cwd = tempfile::tempdir().unwrap();
    cli.ok("workspace.up", json!({"panes": [{"cwd": cwd.path()}]}));
    let missing = cli
        .call(
            "surface.split",
            json!({"direction": "vertical", "surface_id": 999_999}),
        )
        .expect_err("no such surface");
    assert!(
        missing["message"]
            .as_str()
            .unwrap()
            .contains("surface_id 999999 not found"),
        "{missing}"
    );
    let bad_cwd = cli
        .call(
            "surface.split",
            json!({"direction": "vertical", "cwd": home.path().join("absent")}),
        )
        .expect_err("a cwd that does not exist");
    assert!(
        bad_cwd["message"]
            .as_str()
            .unwrap()
            .contains("cwd does not exist"),
        "{bad_cwd}"
    );
    worker.stop();
}

#[cfg(unix)]
#[test]
fn a_command_of_workspace_up_is_typed_into_its_terminal_once_the_shell_settles() {
    unsafe { std::env::set_var("PANEFLOW_IPC_ORCHESTRATION", "1") };
    let home = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host opens");
    let core_server =
        paneflow_host::ServerHandle::spawn(std::sync::Arc::clone(&core), core_endpoint.clone())
            .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker opens");
    let mut cli = Client::connect(home.path());

    let opened = cli.ok(
        "workspace.up",
        json!({"panes": [{"cwd": cwd.path(), "command": "echo typed-$PANEFLOW_SURFACE_ID"}]}),
    );
    let surface_id = opened["surface_ids"][0].as_u64().unwrap();
    let view = cli.ok("layout.view", json!({}));
    let made = surface(&view, surface_id);
    assert_eq!(
        made["definition"]["command"], "echo typed-$PANEFLOW_SURFACE_ID",
        "the command is kept so an attaching window declares the agent"
    );
    let session: paneflow_config::schema::SessionId = made["definition"]["session"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let mut host = paneflow_host::HostClient::connect(
        &core_endpoint,
        &paneflow_host::protocol::ClientHello::control("worker-structural-test"),
    )
    .unwrap();
    let expected = format!("typed-{surface_id}");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let text = host
            .text(&session)
            .map(|reply| reply.text)
            .unwrap_or_default();
        if text.lines().any(|line| line.trim() == expected) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the command never ran: {text:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let generation = host.inspect(&session).unwrap().manifest.generation;
    let _ = host.stop(&session, Some(generation));
    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}

#[cfg(unix)]
#[test]
fn a_command_of_a_layout_is_typed_into_its_terminal_by_create_and_restore_layout() {
    unsafe { std::env::set_var("PANEFLOW_IPC_ORCHESTRATION", "1") };
    let home = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host opens");
    let core_server =
        paneflow_host::ServerHandle::spawn(std::sync::Arc::clone(&core), core_endpoint.clone())
            .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker opens");
    let mut cli = Client::connect(home.path());

    cli.ok(
        "workspace.create",
        json!({"name": "made", "layout": {"type": "pane", "surfaces": [
            {"type": "terminal", "command": "echo created-by-layout"}
        ]}}),
    );
    cli.ok(
        "workspace.restore_layout",
        json!({"layout": {"type": "split", "direction": "vertical", "children": [
            {"type": "pane", "surfaces": [{"type": "terminal"}]},
            {"type": "pane", "surfaces": [{"type": "terminal", "command": "echo restored-by-layout"}]}
        ]}}),
    );
    let view = cli.ok("layout.view", json!({}));
    let session_running = |command: &str| -> paneflow_config::schema::SessionId {
        surfaces(&view)
            .into_iter()
            .find(|known| known["definition"]["command"] == command)
            .unwrap_or_else(|| panic!("a surface carries {command}"))["definition"]["session"]
            .as_str()
            .unwrap_or_else(|| panic!("the surface of {command} has its session"))
            .parse()
            .unwrap()
    };
    let mut host = paneflow_host::HostClient::connect(
        &core_endpoint,
        &paneflow_host::protocol::ClientHello::control("worker-structural-test"),
    )
    .unwrap();
    for (command, expected) in [
        ("echo created-by-layout", "created-by-layout"),
        ("echo restored-by-layout", "restored-by-layout"),
    ] {
        let session = session_running(command);
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            let text = host
                .text(&session)
                .map(|reply| reply.text)
                .unwrap_or_default();
            if text.lines().any(|line| line.trim() == expected) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "{command} never ran: {text:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let generation = host.inspect(&session).unwrap().manifest.generation;
        let _ = host.stop(&session, Some(generation));
    }
    drop(host);
    worker.stop();
    core_server.stop().unwrap();
}

fn worktree_of(repo: &Path, name: &str, marked: bool) -> std::path::PathBuf {
    let admin = repo.join(".git").join("worktrees").join(name);
    std::fs::create_dir_all(&admin).unwrap();
    let path = repo.parent().unwrap().join(format!("wt-{name}"));
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(path.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
    if marked {
        std::fs::write(admin.join("paneflow-owner"), "owner=paneflow\n").unwrap();
    }
    path
}

#[test]
fn workspace_up_with_no_window_records_only_the_worktrees_paneflow_owns() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let owned = worktree_of(&repo, "feat", true);
    let foreign = worktree_of(&repo, "other", false);
    let worker = common::open_starting_terminals(home.path());
    let mut cli = Client::connect(home.path());
    let opened = cli.ok(
        "workspace.up",
        json!({"name": "wt", "panes": [
            {"cwd": owned, "managed_worktree": {"path": owned, "repo_root": repo, "branch": "feat", "teardown": "auto"}},
            {"cwd": foreign, "managed_worktree": {"path": foreign, "repo_root": repo, "branch": "other", "teardown": "auto"}}
        ]}),
    );
    let index = opened["index"].as_u64().unwrap() as usize;
    let view = cli.ok("layout.view", json!({}));
    let ws = &view["view"]["state"]["workspaces"][index];
    let recorded = ws["extras"]["managed_worktrees"].as_array().unwrap();
    assert_eq!(recorded.len(), 1, "{ws}");
    assert_eq!(recorded[0]["path"].as_str(), owned.to_str());
    assert_eq!(recorded[0]["branch"], "feat");
    assert_eq!(recorded[0]["teardown"], "auto");
    let tabs = ws["tabs"].as_array().unwrap();
    assert!(
        tabs.iter()
            .any(|tab| tab["worktree"].as_str() == owned.to_str()),
        "the owned worktree has its tab: {ws}"
    );
    assert!(
        tabs.iter()
            .all(|tab| tab["worktree"].as_str() != foreign.to_str()),
        "a foreign directory is not bound as a worktree: {ws}"
    );
    worker.stop();
}
