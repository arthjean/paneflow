#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize};
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

const TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn ctrl_c_interrupts_a_session_spawned_by_a_host_that_ignores_ctrl_c() {
    assert_ne!(unsafe { SetConsoleCtrlHandler(None, 1) }, 0);

    let pair = paneflow_host::pty::open(PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    })
    .expect("the host must open ConPTY");
    let mut command = CommandBuilder::new("ping.exe");
    command.args(["-n", "60", "127.0.0.1"]);
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("the host PTY must spawn ping");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().unwrap();
    let writer = Arc::new(Mutex::new(pair.master.take_writer().unwrap()));
    let output = Arc::new(Mutex::new(String::new()));
    std::thread::spawn({
        let writer = Arc::clone(&writer);
        let output = Arc::clone(&output);
        move || {
            let mut buffer = [0u8; 4096];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
                if chunk.contains("\x1b[6n") {
                    let _ = writer.lock().unwrap().write_all(b"\x1b[1;1R");
                }
                output.lock().unwrap().push_str(&chunk);
            }
        }
    });

    let deadline = Instant::now() + TIMEOUT;
    while output.lock().unwrap().matches("127.0.0.1").count() < 2 {
        assert!(
            Instant::now() < deadline,
            "ping never answered: {:?}",
            output.lock().unwrap()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    writer.lock().unwrap().write_all(b"\x03").unwrap();

    let deadline = Instant::now() + TIMEOUT;
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!(
                "Ctrl+C did not interrupt the session: {:?}",
                output.lock().unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
