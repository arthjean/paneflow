#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{HostClient, SessionSummary};
use serde_json::{Value, json};

const SAMPLES: usize = 20;

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

fn projected_workspace(worker: &paneflow_serve::server::Worker, session: &SessionId) -> Value {
    worker.snapshot_frame()["sessions"]
        .as_array()
        .and_then(|sessions| {
            sessions
                .iter()
                .find(|entry| entry["session"] == session.to_string())
                .map(|entry| entry["workspace"].clone())
        })
        .unwrap_or(Value::Null)
}

fn wait_for(deadline: Duration, mut done: impl FnMut() -> bool) -> Option<Duration> {
    let started = Instant::now();
    while started.elapsed() < deadline {
        if done() {
            return Some(started.elapsed());
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    None
}

fn workspace_change_delays(
    owner: &mut HostClient,
    worker: &paneflow_serve::server::Worker,
    session: &SessionId,
    samples: usize,
) -> Vec<Duration> {
    (0..samples)
        .map(|_| {
            let workspace = WorkspaceId::new();
            let expected = json!(workspace);
            let changed = Instant::now();
            owner.set_workspace(session, &workspace).unwrap();
            wait_for(Duration::from_secs(5), || {
                projected_workspace(worker, session) == expected
            })
            .map(|_| changed.elapsed())
            .expect("the manifest change reaches the worker projection")
        })
        .collect()
}

fn p95(mut delays: Vec<Duration>) -> Duration {
    delays.sort();
    delays[((delays.len() as f64) * 0.95).ceil() as usize - 1]
}

fn start_core(
    home: &std::path::Path,
) -> (
    Arc<paneflow_host::SessionHost>,
    paneflow_host::ServerHandle,
    Instant,
) {
    let endpoint = paneflow_home::host_endpoint_path(home);
    let core = paneflow_host::SessionHost::open(home, &endpoint).expect("a core opens");
    let server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), endpoint)
        .expect("the core serves its endpoint");
    (core, server, Instant::now())
}

#[test]
fn host_manifest_changes_reach_the_worker_projection_by_push_and_survive_a_host_restart() {
    let home = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var(paneflow_home::HOME_ENV, home.path()) };
    let endpoint = paneflow_home::host_endpoint_path(home.path());
    let (core, core_server, _) = start_core(home.path());
    let hello = ClientHello::local("worker-push-test");
    let mut owner = HostClient::connect(&endpoint, &hello).unwrap();
    let created: SessionSummary =
        serde_json::from_value(owner.call("session.create", shell_params()).unwrap()).unwrap();
    let session = created.manifest.session.clone();

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    assert!(
        wait_for(Duration::from_secs(5), || {
            projected_workspace(worker.worker(), &session) == Value::Null
                && worker.worker().snapshot_frame()["core_connected"] == true
        })
        .is_some(),
        "the worker subscribes to the core"
    );

    let delays = workspace_change_delays(&mut owner, worker.worker(), &session, SAMPLES);
    let observed = p95(delays.clone());
    eprintln!("host to worker projection: {SAMPLES} samples, p95 {observed:?}, all {delays:?}");
    assert!(observed < Duration::from_millis(100), "p95 {observed:?}");

    owner
        .call("session.stop", json!({"session": session}))
        .unwrap();
    drop(owner);
    core_server.stop().unwrap();
    drop(core);
    assert!(
        wait_for(Duration::from_secs(5), || {
            worker.worker().snapshot_frame()["core_connected"] == false
        })
        .is_some(),
        "the worker notices the core went away"
    );

    let (core, core_server, restarted_at) = start_core(home.path());
    let mut owner = HostClient::connect(&endpoint, &hello).unwrap();
    let relaunched: SessionSummary =
        serde_json::from_value(owner.call("session.create", shell_params()).unwrap()).unwrap();
    let relaunched = relaunched.manifest.session;
    wait_for(Duration::from_secs(5), || {
        worker.worker().snapshot_frame()["sessions"]
            .as_array()
            .is_some_and(|sessions| {
                sessions
                    .iter()
                    .any(|entry| entry["session"] == relaunched.to_string())
            })
    })
    .expect("the worker takes a full snapshot of the restarted core");
    let resubscribed = restarted_at.elapsed();
    eprintln!("worker resubscribed {resubscribed:?} after the core restarted");
    assert!(resubscribed < Duration::from_secs(1), "{resubscribed:?}");
    let after_restart = p95(workspace_change_delays(
        &mut owner,
        worker.worker(),
        &relaunched,
        5,
    ));
    assert!(
        after_restart < Duration::from_millis(100),
        "the restarted core pushes again: {after_restart:?}"
    );

    owner
        .call("session.stop", json!({"session": relaunched}))
        .unwrap();
    drop(owner);
    worker.stop();
    core_server.stop().unwrap();
    drop(core);
}
