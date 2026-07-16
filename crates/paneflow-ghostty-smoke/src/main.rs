#[cfg(target_os = "linux")]
mod linux {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, bail};
    use paneflow_terminal_ghostty::{DisplayTerminal, WindowSize};
    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    const MARKER: &str = "PANEFLOW_GHOSTTY_PACKAGE_OK";
    const TIMEOUT: Duration = Duration::from_secs(8);

    pub(super) fn run() -> Result<()> {
        let size = WindowSize::new(80, 24, 8, 16)?;
        let mut terminal = DisplayTerminal::new(size, 1_000)?;
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 640,
                pixel_height: 384,
            })
            .context("open native PTY")?;

        let mut command = CommandBuilder::new("/bin/sh");
        command.args([
            "-c",
            "printf 'READY\\n'; IFS= read -r line; printf 'ECHO:%s\\n' \"$line\"; stty size",
        ]);
        let mut child = pair
            .slave
            .spawn_command(command)
            .context("spawn package smoke shell")?;
        let pid = child.process_id().context("smoke child has no PID")?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone PTY reader")?;
        let mut writer = pair.master.take_writer().context("take PTY writer")?;
        let (output_tx, output_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("paneflow-ghostty-package-smoke".into())
            .spawn(move || {
                let mut buffer = [0u8; 4096];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(read) if output_tx.send(buffer[..read].to_vec()).is_err() => break,
                        Ok(_) => {}
                    }
                }
            })
            .context("spawn PTY reader")?;

        let result = (|| -> Result<()> {
            pair.master
                .resize(PtySize {
                    rows: 41,
                    cols: 101,
                    pixel_width: 808,
                    pixel_height: 656,
                })
                .context("resize package smoke PTY")?;
            terminal.resize(WindowSize::new(101, 41, 8, 16)?)?;
            writer
                .write_all(format!("{MARKER}\r").as_bytes())
                .context("write PTY marker")?;
            writer.flush().context("flush PTY marker")?;

            let deadline = Instant::now() + TIMEOUT;
            let mut output = Vec::new();
            while Instant::now() < deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if let Ok(chunk) = output_rx.recv_timeout(remaining) {
                    terminal.feed(&chunk)?;
                    output.extend_from_slice(&chunk);
                    let text = String::from_utf8_lossy(&output);
                    if text.contains(MARKER) && text.contains("41 101") {
                        break;
                    }
                }
            }
            let output_text = String::from_utf8_lossy(&output);
            if !output_text.contains(MARKER) || !output_text.contains("41 101") {
                bail!("PTY marker or resize response missing from bounded smoke output")
            }

            let mut rendered = String::new();
            for cell in terminal.snapshot()?.cells.iter() {
                rendered.push(cell.character);
                if let Some(zerowidth) = cell.zerowidth.as_deref() {
                    rendered.extend(zerowidth.iter().copied());
                }
            }
            if !rendered.contains(MARKER) {
                bail!("libghostty snapshot did not render the static smoke marker")
            }

            let exit_deadline = Instant::now() + TIMEOUT;
            let status = loop {
                if let Some(status) = child.try_wait().context("query smoke child")? {
                    break status;
                }
                if Instant::now() >= exit_deadline {
                    bail!("package smoke child did not exit within the deadline")
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            if !status.success() {
                bail!("package smoke child exited with {}", status.exit_code())
            }

            // SAFETY: signal 0 performs a read-only process existence check.
            let process_exists = unsafe { libc::kill(pid as i32, 0) } == 0;
            if process_exists || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                bail!("package smoke child was not fully reaped")
            }
            Ok(())
        })();

        if result.is_err() && child.try_wait().ok().flatten().is_none() {
            let _ = child.kill();
            let _ = child.wait();
        }
        result
    }
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = linux::run() {
        eprintln!("libghostty package smoke failed: {error:#}");
        std::process::exit(1);
    }
    println!("libghostty package smoke passed");
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("libghostty package smoke is Linux-only");
    std::process::exit(2);
}
