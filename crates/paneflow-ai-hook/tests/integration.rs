#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result
)]

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions, Stream};
use interprocess::TryClone;
use serde_json::{json, Value};

const HOOK_BIN: &str = env!("CARGO_BIN_EXE_paneflow-ai-hook");
const SESSION_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const EXIT_TIMEOUT: Duration = Duration::from_millis(800);

#[cfg(unix)]
type PathKeepalive = tempfile::TempDir;

#[cfg(windows)]
struct PathKeepalive;

struct MockHost {
    endpoint: PathBuf,
    event_rx: mpsc::Receiver<Value>,
    thread: Option<std::thread::JoinHandle<()>>,
    _keepalive: PathKeepalive,
}

impl MockHost {
    fn start() -> Self {
        let (endpoint, keepalive) = unique_ipc_path();
        let name = endpoint
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("IPC name");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("IPC listener");
        let (event_tx, event_rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let stream = listener.accept().expect("accept reporter");
            let mut writer = stream.try_clone().expect("clone stream");
            let mut reader = BufReader::new(stream);
            let hello = read_json_line(&mut reader);
            assert_eq!(hello["method"], "host.hello");
            write_result(&mut writer, &hello, json!({"protocol": 1}));
            let event = read_json_line(&mut reader);
            assert_eq!(event["method"], "agent.event");
            event_tx.send(event.clone()).expect("send event");
            write_result(&mut writer, &event, json!({"accepted": true}));
        });
        Self {
            endpoint,
            event_rx,
            thread: Some(thread),
            _keepalive: keepalive,
        }
    }

    fn event(&self) -> Value {
        self.event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reporter event")
    }
}

impl Drop for MockHost {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            thread.join().expect("host thread");
        }
    }
}

fn read_json_line(reader: &mut BufReader<Stream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read frame");
    serde_json::from_str(line.trim()).expect("frame JSON")
}

fn write_result(writer: &mut Stream, request: &Value, result: Value) {
    let mut response = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": request["id"],
        "result": result
    }))
    .expect("response JSON");
    response.push(b'\n');
    writer.write_all(&response).expect("write response");
    writer.flush().expect("flush response");
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    let directory = tempfile::TempDir::new().expect("temp directory");
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = directory.path().join(format!("hook-{sequence}.sock"));
    (path, directory)
}

#[cfg(windows)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    (
        PathBuf::from(format!(
            r"\\.\pipe\paneflow-ai-hook-{}-{sequence}",
            std::process::id()
        )),
        PathKeepalive,
    )
}

struct HookEnv<'a> {
    endpoint: Option<&'a Path>,
    session: Option<&'a str>,
    session_dir: Option<&'a Path>,
    tool: &'a str,
    generation: Option<u64>,
    hook_log: Option<&'a Path>,
}

fn run_reporter(
    executable: &Path,
    event: &str,
    hook_env: &HookEnv<'_>,
    stdin_bytes: &[u8],
) -> (std::process::ExitStatus, Duration, Vec<u8>) {
    let mut command = Command::new(executable);
    command
        .arg(event)
        .env_clear()
        .envs(non_paneflow_environment())
        .env("PANEFLOW_AI_TOOL", hook_env.tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(endpoint) = hook_env.endpoint {
        command.env("PANEFLOW_HOST_ENDPOINT", endpoint);
    }
    if let Some(session) = hook_env.session {
        command.env("PANEFLOW_SESSION_ID", session);
    }
    if let Some(session_dir) = hook_env.session_dir {
        command.env("PANEFLOW_SESSION_DIR", session_dir);
    }
    if let Some(generation) = hook_env.generation {
        command.env("PANEFLOW_RUNTIME_GENERATION", generation.to_string());
    }
    if let Some(log) = hook_env.hook_log {
        command.env("PANEFLOW_HOOK_LOG", log);
    }
    let started = Instant::now();
    let mut child = command.spawn().expect("reporter process");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(stdin_bytes)
        .expect("stdin payload");
    drop(child.stdin.take());
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut stdout = Vec::new();
                child
                    .stdout
                    .take()
                    .expect("stdout")
                    .read_to_end(&mut stdout)
                    .expect("read stdout");
                return (status, started.elapsed(), stdout);
            }
            Ok(None) if started.elapsed() < EXIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(None) => {
                let _ = child.kill();
                panic!("reporter exceeded {EXIT_TIMEOUT:?}");
            }
            Err(error) => panic!("reporter wait failed: {error}"),
        }
    }
}

fn non_paneflow_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| !is_paneflow_environment_key(key))
        .collect()
}

fn is_paneflow_environment_key(key: &OsStr) -> bool {
    key.to_string_lossy()
        .to_ascii_uppercase()
        .starts_with("PANEFLOW_")
}

#[test]
fn hand_typed_claude_events_use_the_host_protocol() {
    let cases = [
        ("SessionStart", "ai.session_start", "HookSeen"),
        ("SubagentStart", "ai.session_start", "HookSeen"),
        ("SubagentStop", "ai.session_start", "HookSeen"),
        ("UserPromptSubmit", "ai.prompt_submit", "UserPromptSubmit"),
        ("PermissionRequest", "ai.notification", "PermissionRequest"),
        ("Stop", "ai.stop", "Stop"),
        ("StopFailure", "ai.stop", "StopFailure"),
        ("Interrupt", "ai.stop", "Interrupt"),
    ];
    for (event, kind, payload_event) in cases {
        let host = MockHost::start();
        let payload = json!({
            "session_id": "provider-session",
            "transcript_path": "C:\\temp\\transcript.jsonl",
            "tool_name": "AskUserQuestion"
        });
        let (status, elapsed, stdout) = run_reporter(
            Path::new(HOOK_BIN),
            event,
            &HookEnv {
                endpoint: Some(&host.endpoint),
                session: Some(SESSION_ID),
                session_dir: None,
                tool: "claude",
                generation: Some(7),
                hook_log: None,
            },
            payload.to_string().as_bytes(),
        );
        assert!(status.success(), "event={event}");
        assert!(elapsed < Duration::from_millis(500), "event={event}");
        assert!(stdout.is_empty(), "event={event}");
        let frame = host.event();
        assert_eq!(frame["params"]["session"], SESSION_ID, "event={event}");
        assert_eq!(frame["params"]["kind"], kind, "event={event}");
        assert_eq!(frame["params"]["runtime_generation"], 7, "event={event}");
        assert_eq!(
            frame["params"]["hook_payload"]["hook_event_name"], payload_event,
            "event={event}"
        );
    }
}

#[test]
fn missing_session_is_an_immediate_no_op_without_files_or_network() {
    let directory = tempfile::tempdir().expect("temp directory");
    let log = directory.path().join("hook.log");
    let mut best = EXIT_TIMEOUT;
    for _ in 0..3 {
        let (status, elapsed, stdout) = run_reporter(
            Path::new(HOOK_BIN),
            "Stop",
            &HookEnv {
                endpoint: None,
                session: None,
                session_dir: Some(directory.path()),
                tool: "claude",
                generation: Some(1),
                hook_log: Some(&log),
            },
            b"{}",
        );
        assert!(status.success());
        assert!(stdout.is_empty());
        best = best.min(elapsed);
    }
    assert!(best < Duration::from_millis(50), "best={best:?}");
    assert!(!log.exists());
    assert!(!directory.path().join("last-hook-event.json").exists());
}

#[test]
fn unreachable_host_is_bounded_and_writes_the_generation_seed() {
    let (endpoint, _keepalive) = unique_ipc_path();
    let directory = tempfile::tempdir().expect("session directory");
    let (status, elapsed, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        "PermissionRequest",
        &HookEnv {
            endpoint: Some(&endpoint),
            session: Some(SESSION_ID),
            session_dir: Some(directory.path()),
            tool: "claude",
            generation: Some(9),
            hook_log: None,
        },
        br#"{"tool_name":"AskUserQuestion"}"#,
    );
    assert!(status.success());
    assert!(elapsed < EXIT_TIMEOUT, "elapsed={elapsed:?}");
    assert!(stdout.is_empty());
    let seed: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("last-hook-event.json")).expect("seed"),
    )
    .expect("seed JSON");
    assert_eq!(
        seed,
        json!({
            "hook_event_name": "PermissionRequest",
            "runtime_generation": 9,
            "tool_name": "AskUserQuestion"
        })
    );
}

#[cfg(unix)]
#[test]
fn unix_reporter_script_passes_the_same_event_contract() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempfile::tempdir().expect("reporter directory");
    let binary = directory.path().join("paneflow-ai-hook");
    std::fs::copy(HOOK_BIN, &binary).expect("copy reporter");
    let script = directory.path().join("paneflow-ai-hook.sh");
    std::fs::write(
        &script,
        include_bytes!("../../../runtimes/claude-code/assets/hooks/lifecycle.sh"),
    )
    .expect("write script");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
        .expect("script permissions");
    let host = MockHost::start();
    let (status, _, stdout) = run_reporter(
        &script,
        "SessionStart",
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: None,
            tool: "claude",
            generation: Some(2),
            hook_log: None,
        },
        br#"{"session_id":"provider-session"}"#,
    );
    assert!(status.success());
    assert!(stdout.is_empty());
    let frame = host.event();
    assert_eq!(frame["params"]["kind"], "ai.session_start");
    assert_eq!(frame["params"]["runtime_generation"], 2);
}
