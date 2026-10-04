#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;
use std::time::Duration;

fn trivial_command() -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd.exe");
        command.args(["/C", "exit 0"]);
        command
    }
    #[cfg(unix)]
    {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 0"]);
        command
    }
}

#[test]
fn each_supervised_or_detached_spawn_moves_the_spawn_count_by_exactly_one() {
    let before = paneflow_process::spawn_count();
    let output =
        paneflow_process::run_with_timeout(trivial_command(), Duration::from_secs(10), 1024)
            .expect("the trivial command runs");
    assert!(output.status.success());
    assert_eq!(paneflow_process::spawn_count() - before, 1);
    paneflow_process::spawn_detached(&mut trivial_command()).expect("the detached spawn starts");
    assert_eq!(paneflow_process::spawn_count() - before, 2);
}
