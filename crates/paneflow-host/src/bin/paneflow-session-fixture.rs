use std::io::{Read, Write};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

const USAGE: &str = "paneflow-session-fixture <idle|declare|echo|flood <bytes>|stream <bytes-per-second> <seconds>|history <lines>|delayed-exit <ms> <code>|blocked-stdin|descendants <count> <parent-ms>|descendants-orphan <count> <parent-ms>|split-sequences|whoami <host|worker|window> <endpoint> [engine|desktop|control|worker]|call <host|worker|window> <endpoint> <engine|desktop|control|worker> <method> <params>|setsid-whoami <args>|orphan-whoami <args>|escape-whoami <args>|outlive-host <socket>>";

const FLOOD_CHUNK: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ\r\n";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    let outcome = match mode {
        "idle" => idle(),
        "declare" => declare(),
        "echo" => echo(),
        "flood" => flood(arg_u64(&args, 1).unwrap_or(1 << 20)),
        "stream" => stream(
            arg_u64(&args, 1).unwrap_or(1 << 20),
            arg_u64(&args, 2).unwrap_or(60),
        ),
        "history" => history(arg_u64(&args, 1).unwrap_or(10_000)),
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
        "whoami" => whoami(&args[1..]),
        "call" => call(&args[1..]),
        "setsid-whoami" => spawn_whoami(&args[1..], Detach::Session),
        "orphan-whoami" => spawn_whoami(&args[1..], Detach::Orphan),
        "escape-whoami" => spawn_whoami(&args[1..], Detach::SessionAndOrphan),
        "orphan-relay" => orphan_relay(&args[1..]),
        "outlive-host" => outlive_host(&args[1..]),
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

fn silence_terminal_echo() {
    #[cfg(unix)]
    unsafe {
        let mut termios: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut termios) == 0 {
            termios.c_lflag &= !libc::ECHO;
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &termios);
        }
    }
}

fn declare() -> Result<(), u8> {
    silence_terminal_echo();
    announce("fixture idle, declaring each stdin line as an OSC 7501 state")?;
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lines() {
        let line = line.map_err(|_| 3)?;
        let state = line.trim();
        if state.is_empty() {
            continue;
        }
        write!(stdout, "\x1b]7501;state={state}\x1b\\").map_err(|_| 3)?;
        stdout.flush().map_err(|_| 3)?;
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn echo() -> Result<(), u8> {
    silence_terminal_echo();
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

fn stream(bytes_per_second: u64, seconds: u64) -> Result<(), u8> {
    announce("fixture stream")?;
    let mut stdout = std::io::stdout().lock();
    let started = std::time::Instant::now();
    let total = Duration::from_secs(seconds);
    let mut written = 0u64;
    while started.elapsed() < total {
        let due = (started.elapsed().as_secs_f64() * bytes_per_second as f64) as u64;
        if written >= due {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        let take = usize::try_from(due - written)
            .unwrap_or(FLOOD_CHUNK.len())
            .min(FLOOD_CHUNK.len());
        stdout.write_all(&FLOOD_CHUNK[..take]).map_err(|_| 3)?;
        written += take as u64;
        if written % (64 * 1024) < FLOOD_CHUNK.len() as u64 {
            stdout.flush().map_err(|_| 3)?;
        }
    }
    stdout.flush().map_err(|_| 3)?;
    announce(&format!("fixture stream done {written}"))
}

fn history(lines: u64) -> Result<(), u8> {
    let palette = ["31", "32", "33", "34", "35", "36"];
    let words = [
        "alpha", "beta", "gamma", "délta", "epsilon", "ζeta", "eta", "theta",
    ];
    {
        let mut stdout = std::io::stdout().lock();
        for index in 0..lines {
            let color = palette[(index % palette.len() as u64) as usize];
            let word = words[(index % words.len() as u64) as usize];
            let line = format!(
                "\x1b[{color}m{index:06}\x1b[0m {word} \x1b[1m{:08x}\x1b[22m ok\r\n",
                index.wrapping_mul(0x9e37_79b9)
            );
            stdout.write_all(line.as_bytes()).map_err(|_| 3)?;
        }
        stdout.flush().map_err(|_| 3)?;
    }
    announce("fixture history done")?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
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

fn host_hello(kind: Option<&str>) -> paneflow_host::protocol::ClientHello {
    use paneflow_host::protocol::ClientHello;
    match kind {
        Some("engine") => ClientHello::local("fixture-whoami"),
        Some("desktop") => ClientHello::control("paneflow-desktop"),
        Some("worker") => ClientHello::control("paneflow-serve-fixture"),
        _ => ClientHello::control("fixture-whoami"),
    }
}

#[cfg(unix)]
const ORPHAN_OF_ENV: &str = "PANEFLOW_FIXTURE_ORPHAN_OF";

const SPAWNED_WHOAMI_ENV: &str = "PANEFLOW_FIXTURE_SPAWNED_WHOAMI";

#[cfg(unix)]
fn await_orphaning() {
    let Some(relay) = std::env::var(ORPHAN_OF_ENV)
        .ok()
        .and_then(|raw| raw.parse::<i32>().ok())
    else {
        return;
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while unsafe { libc::getppid() } == relay && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(not(unix))]
fn await_orphaning() {}

#[cfg(unix)]
fn outlive_host(args: &[String]) -> Result<(), u8> {
    let socket = std::path::PathBuf::from(args.first().ok_or(2_u8)?);
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let parent = unsafe { libc::getppid() };
    announce(&format!("outlive ready parent={parent}"))?;
    while unsafe { libc::getppid() } == parent {
        std::thread::sleep(Duration::from_millis(20));
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let mut stream = loop {
        match std::os::unix::net::UnixStream::connect(&socket) {
            Ok(stream) => break stream,
            Err(_) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return Err(3),
        }
    };
    let mut closed = [0_u8; 1];
    let _ = stream.read(&mut closed);
    Ok(())
}

#[cfg(not(unix))]
fn outlive_host(_: &[String]) -> Result<(), u8> {
    Err(2)
}

fn whoami(args: &[String]) -> Result<(), u8> {
    use paneflow_ipc_client::IpcTransport;
    use serde_json::json;
    await_orphaning();
    let server = args.first().map(String::as_str).unwrap_or_default();
    let endpoint = std::path::PathBuf::from(args.get(1).ok_or(2_u8)?);
    let mut classified = None;
    let answer: Result<serde_json::Value, String> = match server {
        "host" => {
            let hello = host_hello(args.get(2).map(String::as_str));
            match paneflow_host::HostClient::connect(&endpoint, &hello) {
                Ok(mut client) => {
                    let answer = client
                        .call("system.whoami", json!({}))
                        .map_err(|error| error.to_string());
                    let pid = std::process::id();
                    let started_at = paneflow_host::process::process_start_time(pid);
                    classified = Some(client.call(
                        "peer.classify",
                        json!({"pid": pid, "started_at": started_at.unwrap_or_default()}),
                    ));
                    answer
                }
                Err(error) => Err(error.to_string()),
            }
        }
        "worker" => paneflow_ipc_client::host_control::HostControl::open(
            &endpoint,
            "fixture-whoami",
            Duration::from_secs(10),
        )
        .map_err(|error| format!("{error:?}"))
        .and_then(|mut control| control.request("system.whoami", json!({}))),
        "window" => paneflow_ipc_client::IpcClient::new(endpoint).call("system.whoami", json!({})),
        _ => return Err(2),
    };
    match answer {
        Ok(reply) => {
            for key in ["principal", "session", "workspace"] {
                if let Some(value) = reply[key].as_str() {
                    announce(&format!("whoami {key}={value}"))?;
                }
            }
        }
        Err(error) => announce(&format!("whoami error={error}"))?,
    }
    match classified {
        Some(Ok(reply)) => announce(&format!("classify in_pane={}", reply["in_pane"]))?,
        Some(Err(error)) => announce(&format!(
            "classify refused code={}",
            error.code().unwrap_or_default()
        ))?,
        None => {}
    }
    announce("whoami done")?;
    if std::env::var_os(SPAWNED_WHOAMI_ENV).is_some() {
        return Ok(());
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

fn call(args: &[String]) -> Result<(), u8> {
    use paneflow_ipc_client::IpcTransport;
    let server = args.first().map(String::as_str).unwrap_or_default();
    let endpoint = std::path::PathBuf::from(args.get(1).ok_or(2_u8)?);
    let method = args.get(3).ok_or(2_u8)?;
    let params: serde_json::Value = args
        .get(4)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let answer: Result<serde_json::Value, String> = match server {
        "host" => {
            let hello = host_hello(args.get(2).map(String::as_str));
            match paneflow_host::HostClient::connect(&endpoint, &hello) {
                Ok(mut client) => match client.call(method, params) {
                    Ok(reply) => Ok(reply),
                    Err(paneflow_host::HostClientError::Rpc {
                        code,
                        message,
                        data,
                    }) => {
                        let reason = data
                            .as_ref()
                            .and_then(|data| data["reason"].as_str())
                            .unwrap_or("none")
                            .to_string();
                        announce(&format!("call code={code}"))?;
                        announce(&format!("call reason={reason}"))?;
                        Err(message)
                    }
                    Err(error) => Err(error.to_string()),
                },
                Err(error) => Err(error.to_string()),
            }
        }
        "worker" => paneflow_ipc_client::host_control::HostControl::open(
            &endpoint,
            "fixture-call",
            Duration::from_secs(10),
        )
        .map_err(|error| format!("{error:?}"))
        .and_then(|mut control| control.request(method, params)),
        "window" => paneflow_ipc_client::IpcClient::new(endpoint).call(method, params),
        _ => return Err(2),
    };
    match answer {
        Ok(reply) => announce(&format!("call ok={reply}"))?,
        Err(message) => announce(&format!("call error={message}"))?,
    }
    announce("call done")?;
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Detach {
    Session,
    Orphan,
    SessionAndOrphan,
}

#[cfg(unix)]
fn whoami_command(args: &[String], new_session: bool) -> Result<Command, u8> {
    use std::os::unix::process::CommandExt;
    let executable = std::env::current_exe().map_err(|_| 3)?;
    let mut command = Command::new(executable);
    command
        .arg("whoami")
        .args(args)
        .env(SPAWNED_WHOAMI_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if new_session {
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    Ok(command)
}

#[cfg(unix)]
fn spawn_whoami(args: &[String], detach: Detach) -> Result<(), u8> {
    let child = match detach {
        Detach::Session => whoami_command(args, true)?.spawn(),
        Detach::Orphan | Detach::SessionAndOrphan => {
            let executable = std::env::current_exe().map_err(|_| 3)?;
            let mode = if detach == Detach::Orphan {
                "keep-session"
            } else {
                "new-session"
            };
            Command::new(executable)
                .arg("orphan-relay")
                .arg(mode)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
        }
    };
    let mut child = child.map_err(|_| 3)?;
    if detach != Detach::Session {
        let _ = child.wait();
    }
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(unix)]
fn orphan_relay(args: &[String]) -> Result<(), u8> {
    let new_session = args.first().map(String::as_str) == Some("new-session");
    whoami_command(&args[1..], new_session)?
        .env(ORPHAN_OF_ENV, std::process::id().to_string())
        .spawn()
        .map(drop)
        .map_err(|_| 3)
}

#[cfg(not(unix))]
fn spawn_whoami(_args: &[String], _detach: Detach) -> Result<(), u8> {
    Err(2)
}

#[cfg(not(unix))]
fn orphan_relay(_args: &[String]) -> Result<(), u8> {
    Err(2)
}
