#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::{Read, Write};
    use std::time::{Duration, Instant};

    use paneflow_terminal_ghostty::{
        BackendEvent, DisplayTerminal, TerminalAppearance, WindowSize,
    };
    use portable_pty::{CommandBuilder, PtySize};

    if std::env::args().any(|arg| arg == "--cursor-child") {
        let mut output = std::io::stdout();
        output.write_all(b"\x1b[2J\x1b[H\x1b[5;3H\x1b[?25h")?;
        output.flush()?;
        std::thread::sleep(Duration::from_millis(150));
        output.write_all(b"\x1b[?2026h\x1b[4;1H\x1b[K")?;
        output.flush()?;
        std::thread::sleep(Duration::from_millis(100));
        output.write_all(b"\x1b[5;3H\x1b[?25h\x1b[?2026l")?;
        output.flush()?;
        std::thread::sleep(Duration::from_millis(150));
        return Ok(());
    }

    let mut terminal = DisplayTerminal::new(
        WindowSize::new(40, 10, 8, 16)?,
        100,
        TerminalAppearance::default(),
    )?;
    let pair = paneflow_host::pty::open(PtySize {
        rows: 10,
        cols: 40,
        pixel_width: 320,
        pixel_height: 160,
    })?;
    let mut command = CommandBuilder::new(std::env::current_exe()?);
    command.arg("--cursor-child");
    let mut child = pair.slave.spawn_command(command)?;
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader()?;
    let mut writer = pair.master.take_writer()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            let length = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(length) => length,
            };
            if tx.send(buffer[..length].to_vec()).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_intermediate_cursor = false;
    let mut final_cursor = None;
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut synchronized = false;
        while Instant::now() < deadline && final_cursor.is_none() {
            let bytes = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))?;
            for byte in bytes {
                terminal.feed(&[byte])?;
                for event in terminal.drain_events() {
                    if let BackendEvent::WritePty(reply) = event {
                        writer.write_all(&reply)?;
                        writer.flush()?;
                    }
                }
                let active = terminal.synchronized_output()?;
                let cursor = terminal.snapshot()?.cursor;
                saw_intermediate_cursor |=
                    active && cursor.point.line == 3 && cursor.point.column == 0;
                if synchronized && !active {
                    final_cursor = Some(cursor);
                }
                synchronized = active;
            }
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result?;
    assert!(saw_intermediate_cursor, "child redraw was not observed");
    let cursor = final_cursor.ok_or("child never completed its synchronized redraw")?;
    assert!(cursor.visible);
    assert_eq!(
        (cursor.point.line, cursor.point.column),
        (4, 2),
        "ConPTY must restore the cursor before forwarding the synchronized-output end marker"
    );
    println!("ConPTY synchronized cursor redraw passed");
    Ok(())
}

#[cfg(not(windows))]
fn main() {}
