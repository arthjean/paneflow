#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;

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

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn with_nothing_running_every_api_verb_fails_like_the_existing_verbs() {
    let home = tempfile::tempdir().expect("a temporary home");
    let existing = paneflow(home.path(), &["ls"]);
    assert!(!existing.status.success());
    for args in [
        &["api", "list"][..],
        &["api", "describe", "surface.read"],
        &["api", "call", "surface.list"],
    ] {
        let output = paneflow(home.path(), args);
        assert_eq!(output.status.code(), existing.status.code(), "{args:?}");
        assert_eq!(stderr(&output), stderr(&existing), "{args:?}");
        assert!(output.stdout.is_empty(), "{args:?}");
    }
}

#[test]
fn with_a_host_and_a_worker_the_api_verbs_route_each_method_to_its_owner() {
    let home = tempfile::tempdir().expect("a temporary home");
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the core serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");

    let listed = paneflow(home.path(), &["api", "list"]);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let lines = stdout(&listed);
    let owner_of = |name: &str| {
        lines
            .lines()
            .find(|line| line.split_whitespace().next() == Some(name))
            .and_then(|line| line.split_whitespace().nth(1))
            .map(str::to_string)
    };
    assert_eq!(owner_of("pane.write").as_deref(), Some("host"), "{lines}");
    assert_eq!(
        owner_of("worker.status").as_deref(),
        Some("worker"),
        "{lines}"
    );
    assert_eq!(
        owner_of("surface.list").as_deref(),
        Some("window"),
        "{lines}"
    );
    assert_eq!(
        owner_of("surface.focus").as_deref(),
        Some("worker"),
        "the worker serves the structural methods with no window: {lines}"
    );
    assert_eq!(
        owner_of("client.dispatch_action"),
        None,
        "no window serves it: {lines}"
    );

    let as_json = paneflow(home.path(), &["api", "list", "--json"]);
    let rows: Value = serde_json::from_slice(&as_json.stdout).expect("JSON descriptors");
    assert!(
        rows.as_array()
            .expect("an array")
            .iter()
            .all(|row| row["params"]["$schema"].is_string())
    );

    let described = paneflow(home.path(), &["api", "describe", "surface.read"]);
    assert!(described.status.success(), "{}", stderr(&described));
    let text = stdout(&described);
    for expected in [
        "Parameters:",
        "lines",
        "optional",
        "200",
        "Result fields:",
        "total_lines",
    ] {
        assert!(text.contains(expected), "{expected} missing from\n{text}");
    }
    assert!(text.contains("paneflow api call surface.read"), "{text}");

    let surfaces = paneflow(home.path(), &["api", "call", "surface.list"]);
    assert!(surfaces.status.success(), "{}", stderr(&surfaces));
    let surfaces: Value = serde_json::from_slice(&surfaces.stdout).expect("JSON result");
    assert!(surfaces["surfaces"].is_array(), "{surfaces}");

    let status = paneflow(home.path(), &["api", "call", "worker.status"]);
    assert!(status.status.success(), "{}", stderr(&status));
    let status: Value = serde_json::from_slice(&status.stdout).expect("JSON result");
    assert_eq!(status["pid"], std::process::id());

    let unknown = paneflow(home.path(), &["api", "call", "surface.teleport"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert_eq!(
        stderr(&unknown).trim(),
        "paneflow: Method not found: surface.teleport"
    );

    let mistyped = paneflow(home.path(), &["api", "call", "surface.read", "lines=many"]);
    assert_eq!(mistyped.status.code(), Some(1));
    assert!(
        stderr(&mistyped).contains("'lines'"),
        "{}",
        stderr(&mistyped)
    );

    let refused = paneflow(
        home.path(),
        &[
            "api",
            "call",
            "surface.read",
            "--args",
            "{\"lines\": \"5\"}",
        ],
    );
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(
        stderr(&refused).trim(),
        "paneflow: 'lines' must be a non-negative integer"
    );

    worker.stop();
    let gone = paneflow(home.path(), &["api", "call", "usage.snapshot"]);
    assert_eq!(gone.status.code(), Some(1));
    let message = stderr(&gone);
    assert!(message.contains("usage.snapshot"), "{message}");
    assert!(message.contains("Paneflow worker"), "{message}");

    let _ = core_server.stop();
    drop(core);
}
