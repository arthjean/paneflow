#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Probe {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
}

impl Probe {
    fn spawn(script: &str, cwd: &std::path::Path) -> Self {
        let pair = paneflow_host::pty::open(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("the host must open the native Linux PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", script]);
        command.cwd(cwd);
        let child = pair
            .slave
            .spawn_command(command)
            .expect("the host PTY must spawn /bin/sh");
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("the host PTY must clone its reader");
        let writer = pair
            .master
            .take_writer()
            .expect("the host PTY must expose one writer");
        let (tx, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if tx.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self {
            child,
            master: pair.master,
            writer,
            output,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        self.writer
            .write_all(bytes)
            .expect("PTY write must succeed");
        self.writer.flush().expect("PTY flush must succeed");
    }

    fn read_until(&self, expected: &str) {
        let deadline = Instant::now() + TIMEOUT;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Ok(chunk) = self.output.recv_timeout(remaining) {
                output.extend_from_slice(&chunk);
                if String::from_utf8_lossy(&output).contains(expected) {
                    return;
                }
            }
        }
        panic!(
            "timed out waiting for {expected:?}; output={:?}",
            String::from_utf8_lossy(&output)
        );
    }

    fn wait_for_exit(&mut self) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().expect("child status query") {
                return status;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        self.child.kill().expect("timed-out child must be killable");
        self.child.wait().expect("killed child must be reaped")
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn wait_for_process_group_exit(pgid: i32) {
    let deadline = Instant::now() + TIMEOUT;
    while Instant::now() < deadline {
        let result = unsafe { libc::kill(-pgid, 0) };
        if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("process group {pgid} still exists after teardown timeout");
}

#[test]
fn the_host_pty_delivers_ctrl_c_and_supports_group_shutdown() {
    let cwd = tempfile::tempdir().expect("temporary cwd");
    let mut interrupt = Probe::spawn(
        "trap 'printf INTERRUPTED\\n; exit 130' INT; printf READY\\n; while :; do read _; done",
        cwd.path(),
    );
    interrupt.read_until("READY");
    interrupt.write(&[0x03]);
    interrupt.read_until("INTERRUPTED");
    assert_eq!(interrupt.wait_for_exit().exit_code(), 130);

    let mut grouped = Probe::spawn(
        r#"sleep 30 & child=$!; trap 'kill "$child" 2>/dev/null || :; wait "$child" 2>/dev/null || :; exit 0' HUP TERM; wait "$child""#,
        cwd.path(),
    );
    let pgid = grouped
        .master
        .process_group_leader()
        .expect("the host PTY must expose the foreground process group");
    assert!(pgid > 0);
    let signal_result = unsafe { libc::kill(-pgid, libc::SIGHUP) };
    assert_eq!(signal_result, 0, "process-group SIGHUP must be delivered");
    let status = grouped.wait_for_exit();
    assert!(status.success() || status.signal().is_some());
    wait_for_process_group_exit(pgid);
}
