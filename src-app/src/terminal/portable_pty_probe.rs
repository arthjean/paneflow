use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Probe {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    master: Box<dyn portable_pty::MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
}

impl Probe {
    fn spawn(script: &str, cwd: &std::path::Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("portable-pty must open the native Linux PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", script]);
        command.cwd(cwd);
        command.env("PANEFLOW_PTY_PROBE", "environment-ok");
        let child = pair
            .slave
            .spawn_command(command)
            .expect("portable-pty must spawn /bin/sh");
        drop(pair.slave);

        let mut reader = pair
            .master
            .try_clone_reader()
            .expect("portable-pty must clone its PTY reader");
        let writer = pair
            .master
            .take_writer()
            .expect("portable-pty must expose one PTY writer");
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

    fn read_until(&self, expected: &str) -> String {
        let deadline = Instant::now() + TIMEOUT;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if let Ok(chunk) = self.output.recv_timeout(remaining) {
                output.extend_from_slice(&chunk);
                let text = String::from_utf8_lossy(&output);
                if text.contains(expected) {
                    return text.into_owned();
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
fn portable_pty_covers_spawn_io_resize_pid_exit_and_reap() {
    let cwd = tempfile::tempdir().expect("temporary cwd");
    let script = r#"
printf 'READY:%s:%s\n' "$PWD" "$PANEFLOW_PTY_PROBE"
while IFS= read -r line; do
  case "$line" in
    echo:*) printf 'ECHO:%s\n' "${line#echo:}" ;;
    size) stty size ;;
    exit:*) exit "${line#exit:}" ;;
  esac
done
"#;
    let mut probe = Probe::spawn(script, cwd.path());
    let pid = probe.child.process_id().expect("Linux child PID");
    let ready = probe.read_until("environment-ok");
    assert!(ready.contains(&cwd.path().to_string_lossy().to_string()));

    probe.write(b"echo:round-trip\r");
    assert!(
        probe
            .read_until("ECHO:round-trip")
            .contains("ECHO:round-trip")
    );

    probe
        .master
        .resize(PtySize {
            rows: 41,
            cols: 101,
            pixel_width: 808,
            pixel_height: 656,
        })
        .expect("PTY resize must succeed");
    assert_eq!(
        probe.master.get_size().expect("PTY size"),
        PtySize {
            rows: 41,
            cols: 101,
            pixel_width: 808,
            pixel_height: 656,
        }
    );
    probe.write(b"size\r");
    assert!(probe.read_until("41 101").contains("41 101"));

    probe.write(b"exit:7\r");
    let status = probe.wait_for_exit();
    assert_eq!(status.exit_code(), 7);
    assert!(probe.child.try_wait().expect("post-wait status").is_some());
    let process_is_gone = unsafe { libc::kill(pid as i32, 0) } == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    assert!(process_is_gone, "waited child must not remain as a zombie");
}

#[test]
fn portable_pty_delivers_ctrl_c_and_supports_group_shutdown() {
    let cwd = tempfile::tempdir().expect("temporary cwd");
    let mut interrupt = Probe::spawn(
        "trap 'printf INTERRUPTED\\n; exit 130' INT; printf READY\\n; while :; do read _; done",
        cwd.path(),
    );
    interrupt.read_until("READY");
    interrupt.write(&[0x03]);
    interrupt.read_until("INTERRUPTED");
    assert_eq!(interrupt.wait_for_exit().exit_code(), 130);

    let mut grouped = Probe::spawn("trap 'exit 0' HUP TERM; sleep 30 & wait", cwd.path());
    let pgid = grouped
        .master
        .process_group_leader()
        .expect("portable-pty must expose the foreground process group");
    assert!(pgid > 0);
    let signal_result = unsafe { libc::kill(-pgid, libc::SIGHUP) };
    assert_eq!(signal_result, 0, "process-group SIGHUP must be delivered");
    let status = grouped.wait_for_exit();
    assert!(status.success() || status.signal().is_some());
    wait_for_process_group_exit(pgid);
}
