#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const START_WAIT: Duration = Duration::from_secs(30);

fn spawn_worker(home: &Path) -> std::process::Child {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .args(["serve", "run", "--home"])
        .arg(home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("PANEFLOW_") {
            command.env_remove(name);
        }
    }
    command.spawn().expect("the worker binary starts")
}

fn connect(home: &Path) -> HostControl {
    let endpoint = paneflow_home::serve_endpoint_path(home);
    let deadline = Instant::now() + START_WAIT;
    loop {
        if let Ok(control) = HostControl::connect(&endpoint, "worker-layout-test") {
            return control;
        }
        assert!(Instant::now() < deadline, "the worker never answered");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn lease(control: &mut HostControl) -> (u64, u64) {
    let lease: Value = control
        .request("layout.lease_ids", json!({}))
        .expect("a lease is granted");
    (
        lease["start"].as_u64().unwrap(),
        lease["end"].as_u64().unwrap(),
    )
}

#[test]
fn a_killed_worker_frees_its_owner_lock_and_its_successor_never_grants_a_leased_id_again() {
    let home = tempfile::tempdir().expect("a home");
    let mut child = spawn_worker(home.path());
    let mut control = connect(home.path());
    assert!(paneflow_serve::layout_authority::owner_lock_held(
        home.path()
    ));
    let (_, first_end) = lease(&mut control);
    child
        .kill()
        .expect("the worker is killed without a shutdown");
    child.wait().expect("the killed worker is reaped");
    drop(control);
    assert!(
        !paneflow_serve::layout_authority::owner_lock_held(home.path()),
        "the owner lock of a killed worker is free"
    );
    let successor = paneflow_serve::open(home.path()).expect("a new worker takes the home");
    let mut control = connect(home.path());
    let (second_start, _) = lease(&mut control);
    assert!(
        second_start >= first_end,
        "the successor granted {second_start}, inside the block that ended at {first_end}"
    );
    drop(control);
    successor.stop();
}
