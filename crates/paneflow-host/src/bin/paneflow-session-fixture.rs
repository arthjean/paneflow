use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const USAGE: &str = "paneflow-session-fixture <idle|echo|flood <bytes>|delayed-exit <ms> <code>|blocked-stdin|descendants <count> <parent-ms>|descendants-orphan <count> <parent-ms>|split-sequences>";

const FLOOD_CHUNK: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let outcome = match mode {
        "idle" => idle(),
        "echo" => echo(),
        "flood" => flood(arg_u64(&args, 1).unwrap_or(1 << 20)),
        "delayed-exit" => delayed_exit(
            arg_u64(&args, 1).unwrap_or(500),
            arg_u64(&args, 2).unwrap_or(0),
        ),
        "blocked-stdin" => blocked_stdin(),
        "descendants" => descendants(
            arg_u64(&args, 1).unwrap_or(1),
            arg_u64(&args, 2).unwrap_or(30_000),
            false,
        ),
        "descendants-orphan" => descendants(
            arg_u64(&args, 1).unwrap_or(1),
            arg_u64(&args, 2).unwrap_or(500),
            true,
        ),
        "descendant-sleep" => descendant_sleep(),
        "split-sequences" => split_sequences(),
        _ => {
            eprintln!("{USAGE}");
            Err(2)
        }
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => ExitCode::from(code),
    }
}

fn arg_u64(args: &[String], index: usize) -> Option<u64> {
    args.get(index).and_then(|raw| raw.parse().ok())
}

fn announce(line: &str) -> Result<(), u8> {
    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(line.as_bytes())
        .and_then(|()| stdout.write_all(b"\r\n"))
        .and_then(|()| stdout.flush())
        .map_err(|_| 3)
}

fn idle() -> Result<(), u8> {
    announce("fixture idle")?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn echo() -> Result<(), u8> {
    announce("fixture echo")?;
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut buffer = [0u8; 4096];
    loop {
        let read = stdin.read(&mut buffer).map_err(|_| 3)?;
        if read == 0 {
            return Ok(());
        }
        if buffer[..read].contains(&0x04) {
            return Ok(());
        }
        stdout.write_all(&buffer[..read]).map_err(|_| 3)?;
        stdout.flush().map_err(|_| 3)?;
    }
}

fn flood(bytes: u64) -> Result<(), u8> {
    let mut stdout = std::io::stdout().lock();
    let mut written = 0u64;
    while written < bytes {
        let take = usize::try_from(bytes - written)
            .unwrap_or(FLOOD_CHUNK.len())
            .min(FLOOD_CHUNK.len());
        stdout.write_all(&FLOOD_CHUNK[..take]).map_err(|_| 3)?;
        written += take as u64;
    }
    stdout.flush().map_err(|_| 3)?;
    announce("fixture flood done")
}

fn delayed_exit(milliseconds: u64, code: u64) -> Result<(), u8> {
    announce("fixture delayed exit")?;
    std::thread::sleep(Duration::from_millis(milliseconds));
    let code = u8::try_from(code).unwrap_or(1);
    if code == 0 { Ok(()) } else { Err(code) }
}

fn blocked_stdin() -> Result<(), u8> {
    announce("fixture blocked stdin")?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn descendant_sleep() -> Result<(), u8> {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    idle()
}

fn descendants(count: u64, parent_milliseconds: u64, leave_alive: bool) -> Result<(), u8> {
    let executable = std::env::current_exe().map_err(|_| 3)?;
    let mut children = Vec::new();
    for _ in 0..count {
        let child = Command::new(&executable)
            .arg("descendant-sleep")
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|_| 3)?;
        children.push(child);
    }
    let pids: Vec<String> = children
        .iter()
        .map(|child| child.id().to_string())
        .collect();
    announce(&format!("fixture descendants {}", pids.join(",")))?;
    std::thread::sleep(Duration::from_millis(parent_milliseconds));
    if !leave_alive {
        for child in &mut children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    Ok(())
}

fn split_sequences() -> Result<(), u8> {
    let mut stdout = std::io::stdout().lock();
    let pieces: [&[u8]; 6] = [
        b"\x1b[31mred",
        b"\x1b",
        b"[0m plain ",
        "é".as_bytes().get(..1).unwrap_or(b""),
        "é".as_bytes().get(1..).unwrap_or(b""),
        b" done\r\n",
    ];
    for piece in pieces {
        stdout.write_all(piece).map_err(|_| 3)?;
        stdout.flush().map_err(|_| 3)?;
        std::thread::sleep(Duration::from_millis(40));
    }
    announce("fixture split sequences done")
}
