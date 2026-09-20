#![cfg(windows)]
#![allow(clippy::expect_used, clippy::panic)]

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

fn run(executable: &Path, home: &Path, verb: &str) -> std::process::Output {
    Command::new(executable)
        .args(["serve", verb])
        .env(paneflow_home::HOME_ENV, home)
        .output()
        .expect("the Paneflow CLI runs")
}

fn wait_until_removable(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match std::fs::remove_file(path) {
            Ok(()) => return,
            Err(_) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(error) => panic!("the stopped worker still locks {}: {error}", path.display()),
        }
    }
}

#[test]
fn a_detached_worker_never_locks_the_controller_executable() {
    let scratch = tempfile::tempdir().expect("scratch directory");
    let home = scratch.path().join("home");
    let controller = scratch.path().join("paneflow-controller.exe");
    let stopper = scratch.path().join("paneflow-stopper.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_paneflow"), &controller).expect("controller copy");
    std::fs::copy(env!("CARGO_BIN_EXE_paneflow"), &stopper).expect("stopper copy");

    let started = run(&controller, &home, "start");
    assert!(
        started.status.success(),
        "worker start failed: {}",
        String::from_utf8_lossy(&started.stderr)
    );

    let removable = std::fs::remove_file(&controller);
    let stopped = run(&stopper, &home, "stop");
    assert!(
        stopped.status.success(),
        "worker stop failed: {}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    if removable.is_err() {
        wait_until_removable(&controller);
    }
    assert!(
        removable.is_ok(),
        "the detached worker locked the controller executable: {removable:?}"
    );
}
