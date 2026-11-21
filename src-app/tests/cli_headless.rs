#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const SETTLE: Duration = Duration::from_secs(30);

fn paneflow(home: &Path, args: &[&str]) -> (i32, Value, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .args(args)
        .env(paneflow_home::HOME_ENV, home)
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_SURFACE_ID")
        .env_remove("PANEFLOW_IPC_ORCHESTRATION")
        .env_remove("PANEFLOW_IPC_SCRIPTING")
        .output()
        .expect("the paneflow CLI runs");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let value = serde_json::from_str(stdout.trim()).unwrap_or(Value::Null);
    (output.status.code().unwrap_or(-1), value, stderr)
}

fn ok(home: &Path, args: &[&str]) -> Value {
    let (code, value, stderr) = paneflow(home, args);
    assert_eq!(code, 0, "paneflow {args:?} failed: {stderr}");
    value
}

#[cfg(unix)]
#[test]
fn the_layout_verbs_work_with_a_worker_and_a_host_and_no_window() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint.clone())
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let spec = home.path().join("dev.toml");
    std::fs::write(
        &spec,
        format!(
            "name = \"dev\"\n\n[[panes]]\ncwd = \"{}\"\nname = \"server\"\n",
            work.path().display()
        ),
    )
    .unwrap();

    let up = ok(home.path(), &["up", spec.to_str().unwrap()]);
    assert_eq!(up["title"], "dev", "{up}");
    let server = up["surface_ids"][0].as_u64().unwrap();

    let created = ok(
        home.path(),
        &[
            "new",
            "--name",
            "notes",
            "--cwd",
            work.path().to_str().unwrap(),
        ],
    );
    assert_eq!(created["title"], "notes", "{created}");
    assert_eq!(created["panes"], 1);
    let notes = created["index"].as_u64().unwrap();

    let split = ok(
        home.path(),
        &["split", "vertical", "--target", &server.to_string()],
    );
    assert_eq!(split["split"], true, "{split}");
    assert_eq!(split["direction"], "vertical");
    let logs = split["surface_id"].as_u64().unwrap();

    let selected = ok(home.path(), &["select", &notes.to_string()]);
    assert_eq!(selected, json!({"selected": notes}));

    let focused = ok(home.path(), &["focus", &logs.to_string()]);
    assert_eq!(focused["focused"], true, "{focused}");
    assert_eq!(focused["surface_id"], logs);

    let listed = ok(home.path(), &["ls"]);
    let rows = listed["surfaces"].as_array().unwrap();
    let ids: Vec<u64> = rows
        .iter()
        .map(|row| row["surface_id"].as_u64().unwrap())
        .collect();
    assert!(ids.contains(&server) && ids.contains(&logs), "{listed}");
    assert_eq!(ids.len(), 3, "{listed}");
    let named = rows.iter().find(|row| row["surface_id"] == server).unwrap();
    assert_eq!(named["name"], "server");

    let deadline = Instant::now() + SETTLE;
    loop {
        let status = ok(home.path(), &["status", &server.to_string(), "--json"]);
        if status["session"].is_string() || status["state"].is_string() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no status for {server}: {status}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let (code, _, stderr) = paneflow(home.path(), &["read", &server.to_string()]);
    assert_eq!(code, 0, "the host serves read by the layout id: {stderr}");
    let (code, _, stderr) = paneflow(home.path(), &["search", &server.to_string(), "anything"]);
    assert_eq!(code, 0, "the host serves search by the layout id: {stderr}");

    let (code, _, stderr) = paneflow(
        home.path(),
        &["api", "call", "client.dispatch_action", "name=toggle_zoom"],
    );
    assert_ne!(code, 0, "a window action needs a window");
    assert!(!stderr.is_empty(), "the refusal says why");

    worker.stop();
    core_server.stop().unwrap();
}

#[test]
fn a_layout_verb_that_carries_a_command_is_refused_with_no_window_as_with_one() {
    let home = tempfile::tempdir().unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let spec = home.path().join("agent.toml");
    std::fs::write(
        &spec,
        "name = \"agent\"\n\n[[panes]]\ncommand = \"make test\"\n",
    )
    .unwrap();
    let (code, _, stderr) = paneflow(home.path(), &["up", spec.to_str().unwrap()]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("orchestration"),
        "the existing refusal names orchestration: {stderr}"
    );
    worker.stop();
    core_server.stop().unwrap();
}

#[cfg(unix)]
#[test]
fn a_layout_verb_starts_the_worker_when_none_runs() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({"default_shell": "/bin/sh", "shell_integration": false}))
            .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the host serves its endpoint");
    let worker_endpoint = paneflow_home::serve_endpoint_path(home.path());
    assert!(
        paneflow_ipc_client::host_control::HostControl::connect(&worker_endpoint, "probe").is_err(),
        "no worker runs yet"
    );

    let (code, created, stderr) = paneflow(
        home.path(),
        &[
            "new",
            "--name",
            "boot",
            "--cwd",
            work.path().to_str().unwrap(),
        ],
    );
    let stopped = paneflow(home.path(), &["serve", "stop"]);
    assert_eq!(code, 0, "the CLI started the worker: {stderr}");
    assert_eq!(created["title"], "boot", "{created}");
    assert_eq!(stopped.0, 0, "the started worker stops: {}", stopped.2);
    core_server.stop().unwrap();
}

#[cfg(unix)]
#[test]
fn up_with_a_worktree_has_the_worker_create_it_with_no_window() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let repo = work.path().join("repo");
    paneflow_git::fixture::committed_repo(&repo, &[("README.md", "init\n")]);
    let worktrees = work.path().join("worktrees");
    std::fs::write(
        home.path().join("paneflow.json"),
        serde_json::to_vec(&json!({
            "default_shell": "/bin/sh",
            "shell_integration": false,
            "worktrees": {"dir": worktrees}
        }))
        .unwrap(),
    )
    .unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the host serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("a worker");
    let spec = home.path().join("feature.toml");
    std::fs::write(
        &spec,
        format!(
            "name = \"feature\"\n\n[[panes]]\ncwd = \"{}\"\nworktree = \"feat/headless\"\n",
            repo.display()
        ),
    )
    .unwrap();

    let up = ok(home.path(), &["up", spec.to_str().unwrap()]);

    assert_eq!(up["title"], "feature", "{up}");
    let listed = paneflow_git::fixture::run(&repo, &["worktree", "list", "--porcelain"]);
    assert!(listed.contains("feat/headless"), "{listed}");
    let view = paneflow_ipc_client::host_control::HostControl::connect(
        &paneflow_home::serve_endpoint_path(home.path()),
        "cli-headless-test",
    )
    .unwrap()
    .request_with_deadline("layout.view", json!({}), SETTLE)
    .unwrap();
    let ws = &view["view"]["state"]["workspaces"][up["index"].as_u64().unwrap() as usize];
    let path = ws["extras"]["managed_worktrees"][0]["path"]
        .as_str()
        .map(std::path::PathBuf::from)
        .expect("the worker recorded the worktree");
    assert!(path.starts_with(&worktrees), "{}", path.display());
    assert!(path.join("README.md").is_file());

    worker.stop();
    core_server.stop().unwrap();
}
