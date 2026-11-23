#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::{CreateSession, HostError};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::json;

const SETTLE: Duration = Duration::from_secs(60);

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + SETTLE;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn stand_in_helper(dir: &Path) -> PathBuf {
    let helper = dir.join("paneflow-host");
    std::fs::write(
        &helper,
        "#!/bin/sh\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    helper
}

#[test]
fn the_worker_serves_a_sandboxed_pane_on_its_own_endpoint_while_the_pane_lives() {
    let home = tempfile::Builder::new().prefix("pfw").tempdir().unwrap();
    std::fs::write(
        home.path().join("paneflow.json"),
        r#"{"shell_integration": false}"#,
    )
    .unwrap();
    let tools = tempfile::tempdir().unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).unwrap();
    core.use_sandbox_helper(stand_in_helper(tools.path()));
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint).unwrap();
    let worker = paneflow_serve::open(home.path()).unwrap();
    let workspace = paneflow_config::schema::WorkspaceId::new();
    let created = core.create(CreateSession {
        workspace: Some(workspace.clone()),
        cwd: Some(tools.path().display().to_string()),
        shell: Some("/bin/sh".into()),
        args: vec!["-c".into(), "sleep 60".into()],
        sandbox: true,
        ..CreateSession::default()
    });
    let session = match created {
        Ok(summary) => summary.manifest.session,
        Err(HostError::SpawnFailed { reason, .. }) => {
            assert!(
                std::env::var_os("PANEFLOW_SANDBOX_REQUIRED")
                    .is_none_or(|required| required != "1"),
                "PANEFLOW_SANDBOX_REQUIRED=1 and the sandbox did not start: {reason}"
            );
            assert!(reason.contains("bwrap"), "{reason}");
            worker.stop();
            let _ = core.request_shutdown(true);
            core_server.stop().unwrap();
            return;
        }
        Err(other) => panic!("unexpected launch error: {other}"),
    };
    let endpoint = paneflow_home::pane_endpoint_path(
        home.path(),
        session.as_str(),
        paneflow_home::PaneServer::Worker,
    );
    wait_until("the worker never opened the pane endpoint", || {
        endpoint.exists()
    });
    let mut pane = HostControl::connect(&endpoint, "sandboxed-pane").unwrap();
    assert_eq!(
        pane.request("system.whoami", json!({})).unwrap(),
        json!({"principal": "pane", "session": session.as_str(), "workspace": workspace.to_string()})
    );
    pane.request("layout.attach", json!({"client": "window"}))
        .unwrap_or_default();
    assert_eq!(
        pane.request("system.whoami", json!({})).unwrap()["principal"],
        "pane",
        "attaching as the window does not lift a pane endpoint's principal"
    );
    for (method, params) in [
        (
            "layout.apply",
            json!({"op": {"op": "focus_pane", "pane": 5}}),
        ),
        ("action.invoke", json!({"name": "anything"})),
    ] {
        let refused = pane.request(method, params).unwrap_err();
        assert!(
            refused.contains("not available to a sandboxed pane"),
            "{method}: {refused}"
        );
    }
    let child = paneflow_serve::structural::inherit_sandbox(
        worker.worker(),
        &paneflow_api::Principal::Pane {
            session: session.to_string(),
            workspace: Some(workspace.to_string()),
        },
        None,
    )
    .unwrap();
    assert_eq!(
        child
            .get(paneflow_config::schema::SANDBOX_REQUEST_ENV)
            .map(String::as_str),
        Some(paneflow_config::schema::SANDBOX_REQUEST_PANEFLOW),
        "a pane a sandboxed pane splits is sandboxed too"
    );
    assert_eq!(
        paneflow_serve::structural::inherit_sandbox(
            worker.worker(),
            &paneflow_api::Principal::External,
            None
        ),
        None
    );
    let shared = paneflow_home::serve_endpoint_path(home.path());
    let mut outside = HostControl::connect(&shared, "outside").unwrap();
    assert_eq!(
        outside.request("system.whoami", json!({})).unwrap(),
        json!({"principal": "external"})
    );
    core.stop(&session, None).unwrap();
    wait_until("the worker kept the endpoint of an ended pane", || {
        !endpoint.exists() && !worker.worker().pane_endpoints.is_open(session.as_str())
    });
    worker.stop();
    let _ = core.request_shutdown(true);
    core_server.stop().unwrap();
}
