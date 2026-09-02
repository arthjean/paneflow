#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result
)]

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions, Stream};
use paneflow_ai_hook::MAX_STDIN_BYTES;
use serde_json::{json, Value};

const HOOK_BIN: &str = env!("CARGO_BIN_EXE_paneflow-ai-hook");
const RECV_TIMEOUT: Duration = Duration::from_secs(5);
const EXIT_TIMEOUT: Duration = Duration::from_secs(7);

#[cfg(unix)]
type PathKeepalive = tempfile::TempDir;

#[cfg(windows)]
struct PathKeepalive;

struct MockServer {
    socket_path: PathBuf,
    rx: mpsc::Receiver<Result<Stream, String>>,
    accept_thread: Option<JoinHandle<()>>,
    _keepalive: PathKeepalive,
}

impl MockServer {
    fn start() -> Self {
        let (socket_path, keepalive) = unique_ipc_path();
        let name = socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("test IPC name");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("test IPC listener");
        let (tx, rx) = mpsc::channel();
        let display_path = socket_path.clone();
        let accept_thread = std::thread::Builder::new()
            .name("ai-hook-test-listener".into())
            .spawn(move || match listener.accept() {
                Ok(stream) => {
                    let _ = tx.send(Ok(stream));
                }
                Err(error) => {
                    let _ = tx.send(Err(format!(
                        "accept on {} failed: {error}",
                        display_path.display()
                    )));
                }
            })
            .expect("test listener thread");

        Self {
            socket_path,
            rx,
            accept_thread: Some(accept_thread),
            _keepalive: keepalive,
        }
    }

    fn expect_frame(&self, scenario: &str) -> Value {
        let result = self
            .rx
            .recv_timeout(RECV_TIMEOUT)
            .unwrap_or_else(|_| panic!("[{scenario}]: no frame arrived within {RECV_TIMEOUT:?}"));
        let stream = result.unwrap_or_else(|error| panic!("[{scenario}]: {error}"));
        let bytes = read_frame(stream, RECV_TIMEOUT)
            .unwrap_or_else(|error| panic!("[{scenario}]: {error}"));
        serde_json::from_slice(trim_trailing_newline(&bytes)).unwrap_or_else(|error| {
            panic!(
                "[{scenario}]: invalid frame JSON ({error}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }

    fn try_recv(&self, timeout: Duration) -> Option<Value> {
        let stream = self.rx.recv_timeout(timeout).ok()?.ok()?;
        let bytes = read_frame(stream, timeout).ok()?;
        serde_json::from_slice(trim_trailing_newline(&bytes)).ok()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let Some(thread) = self.accept_thread.take() else {
            return;
        };
        if !thread.is_finished() {
            if let Ok(name) = self.socket_path.as_path().to_fs_name::<GenericFilePath>() {
                let _ = Stream::connect(name);
            }
        }
        let _ = thread.join();
    }
}

fn read_frame(stream: Stream, timeout: Duration) -> Result<Vec<u8>, String> {
    stream
        .set_nonblocking(true)
        .map_err(|error| format!("failed to bound frame read: {error}"))?;
    let deadline = Instant::now() + timeout;
    let mut reader = BufReader::new(stream);
    let mut bytes = Vec::new();

    loop {
        match reader.read_until(b'\n', &mut bytes) {
            Ok(0) if bytes.is_empty() => return Err("connection closed without a frame".into()),
            Ok(_) => return Ok(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!("frame read exceeded {timeout:?}"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(format!("frame read failed: {error}")),
        }
    }
}

fn trim_trailing_newline(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

static UNIQUE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    let directory = tempfile::TempDir::new().expect("test temp directory");
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = directory
        .path()
        .join(format!("paneflow-test-{sequence}.sock"));
    (path, directory)
}

#[cfg(windows)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    (
        PathBuf::from(format!(r"\\.\pipe\paneflow-test-{pid}-{sequence}")),
        PathKeepalive,
    )
}

struct HookEnv<'a> {
    socket_path: Option<&'a Path>,
    workspace_id: u64,
    tool: &'a str,
    pid: Option<u32>,
    hook_log: Option<&'a Path>,
}

fn run_hook(event: &str, hook_env: &HookEnv<'_>, stdin_bytes: &[u8]) -> std::process::ExitStatus {
    let mut command = Command::new(HOOK_BIN);
    command
        .arg(event)
        .env_clear()
        .envs(non_paneflow_environment())
        .env("PANEFLOW_WORKSPACE_ID", hook_env.workspace_id.to_string())
        .env("PANEFLOW_AI_TOOL", hook_env.tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());

    if let Some(path) = hook_env.socket_path {
        command.env("PANEFLOW_SOCKET_PATH", path);
    }
    if let Some(pid) = hook_env.pid {
        command.env("PANEFLOW_AI_PID", pid.to_string());
    }
    if let Some(log) = hook_env.hook_log {
        command.env("PANEFLOW_HOOK_LOG", log);
    }

    let mut child = command.spawn().expect("hook subprocess");
    child
        .stdin
        .as_mut()
        .expect("piped stdin")
        .write_all(stdin_bytes)
        .expect("write hook stdin");
    drop(child.stdin.take());

    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) if started.elapsed() <= EXIT_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                panic!("hook subprocess exceeded {EXIT_TIMEOUT:?}");
            }
            Err(error) => panic!("hook subprocess wait failed: {error}"),
        }
    }
}

fn non_paneflow_environment() -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| !is_paneflow_environment_key(key))
        .collect()
}

fn is_paneflow_environment_key(key: &OsStr) -> bool {
    const PREFIX: &[u8] = b"PANEFLOW_";

    key.to_string_lossy()
        .as_bytes()
        .get(..PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(PREFIX))
}

struct SuccessCase {
    name: &'static str,
    event: &'static str,
    workspace_id: u64,
    tool: &'static str,
    pid: Option<u32>,
    payload: Value,
    method: &'static str,
    expected_tool_name: Option<&'static str>,
}

fn assert_envelope<'a>(frame: &'a Value, case: &SuccessCase) -> &'a Value {
    assert_eq!(frame["jsonrpc"], "2.0", "case={}", case.name);
    assert_eq!(frame["method"], case.method, "case={}", case.name);
    assert!(frame.get("id").is_none(), "case={}", case.name);
    let params = &frame["params"];
    assert_eq!(
        params["workspace_id"], case.workspace_id,
        "case={}",
        case.name
    );
    assert_eq!(params["tool"], case.tool, "case={}", case.name);
    assert!(params.get("hook_payload").is_some(), "case={}", case.name);
    assert_eq!(
        params.get("tool_name").and_then(Value::as_str),
        case.expected_tool_name,
        "case={}",
        case.name
    );
    params
}

#[test]
fn supported_events_dispatch_through_the_process_boundary() {
    let cases = vec![
        SuccessCase {
            name: "claude_prompt",
            event: "UserPromptSubmit",
            workspace_id: 42,
            tool: "claude",
            pid: None,
            payload: json!({"session_id": "abc", "prompt": "hello"}),
            method: "ai.prompt_submit",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "claude_notification",
            event: "Notification",
            workspace_id: 7,
            tool: "claude",
            pid: None,
            payload: json!({
                "session_id": "abc",
                "notification_type": "permission_prompt",
                "message": "Allow Bash?"
            }),
            method: "ai.notification",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "claude_stop",
            event: "Stop",
            workspace_id: 1,
            tool: "claude",
            pid: None,
            payload: json!({"session_id": "abc"}),
            method: "ai.stop",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "claude_subagent_stop",
            event: "SubagentStop",
            workspace_id: 1,
            tool: "claude",
            pid: None,
            payload: json!({"session_id": "sub"}),
            method: "ai.stop",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "claude_pre_tool",
            event: "PreToolUse",
            workspace_id: 3,
            tool: "claude",
            pid: None,
            payload: json!({"tool_name": "Bash", "tool_input": {"command": "ls"}}),
            method: "ai.tool_use",
            expected_tool_name: Some("Bash"),
        },
        SuccessCase {
            name: "claude_post_tool",
            event: "PostToolUse",
            workspace_id: 3,
            tool: "claude",
            pid: None,
            payload: json!({"tool_name": "Edit"}),
            method: "ai.tool_use",
            expected_tool_name: Some("Edit"),
        },
        SuccessCase {
            name: "codex_session_start",
            event: "SessionStart",
            workspace_id: 5,
            tool: "codex",
            pid: Some(4242),
            payload: json!({"session_id": "s1"}),
            method: "ai.session_start",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "codex_prompt",
            event: "UserPromptSubmit",
            workspace_id: 9,
            tool: "codex",
            pid: None,
            payload: json!({"session_id": "s1", "prompt": "hi"}),
            method: "ai.prompt_submit",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "codex_notification",
            event: "Notification",
            workspace_id: 9,
            tool: "codex",
            pid: None,
            payload: json!({
                "notification_type": "elicitation_dialog",
                "message": "Choose"
            }),
            method: "ai.notification",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "codex_tool",
            event: "PreToolUse",
            workspace_id: 9,
            tool: "codex",
            pid: None,
            payload: json!({"tool_name": "shell"}),
            method: "ai.tool_use",
            expected_tool_name: Some("shell"),
        },
        SuccessCase {
            name: "codex_stop",
            event: "Stop",
            workspace_id: 9,
            tool: "codex",
            pid: None,
            payload: json!({"session_id": "s1"}),
            method: "ai.stop",
            expected_tool_name: None,
        },
        SuccessCase {
            name: "codex_permission",
            event: "PermissionRequest",
            workspace_id: 9,
            tool: "codex",
            pid: None,
            payload: json!({"message": "Approve shell?"}),
            method: "ai.notification",
            expected_tool_name: None,
        },
    ];

    for case in cases {
        let server = MockServer::start();
        let status = run_hook(
            case.event,
            &HookEnv {
                socket_path: Some(&server.socket_path),
                workspace_id: case.workspace_id,
                tool: case.tool,
                pid: case.pid,
                hook_log: None,
            },
            case.payload.to_string().as_bytes(),
        );
        assert!(status.success(), "case={}", case.name);
        let frame = server.expect_frame(case.name);
        let params = assert_envelope(&frame, &case);
        if case.name == "claude_prompt" {
            assert_eq!(params["hook_payload"]["prompt"], "hello");
        }
        if case.name == "codex_session_start" {
            assert_eq!(params["pid"], 4242);
        }
        if case.name == "codex_permission" {
            assert!(params.get("notification_type").is_none());
        }
    }
}

#[test]
fn informational_notification_is_dropped_with_one_accurate_diagnostic() {
    let server = MockServer::start();
    let log_directory = tempfile::TempDir::new().expect("log directory");
    let log_path = log_directory.path().join("hook.log");
    let payload = json!({"notification_type": "idle_prompt", "message": "idle"});
    let status = run_hook(
        "Notification",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 7,
            tool: "claude",
            pid: None,
            hook_log: Some(&log_path),
        },
        payload.to_string().as_bytes(),
    );

    assert!(status.success());
    assert!(server.try_recv(Duration::from_millis(500)).is_none());
    let log = std::fs::read_to_string(log_path).expect("hook log");
    assert_eq!(log.lines().count(), 1);
    assert!(log.contains("dropping notification_type=Some(\"idle_prompt\")"));
    assert!(!log.contains("unhandled hook event"));
}

#[test]
fn malformed_tool_is_rejected_instead_of_becoming_claude() {
    let server = MockServer::start();
    let log_directory = tempfile::TempDir::new().expect("log directory");
    let log_path = log_directory.path().join("hook.log");
    let status = run_hook(
        "Stop",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 1,
            tool: "tool/../etc",
            pid: None,
            hook_log: Some(&log_path),
        },
        json!({}).to_string().as_bytes(),
    );

    assert!(status.success());
    assert!(server.try_recv(Duration::from_millis(250)).is_none());
    let log = std::fs::read_to_string(log_path).expect("hook log");
    assert!(log.contains("PANEFLOW_AI_TOOL"));
}

#[test]
fn inherited_paneflow_environment_is_filtered_case_insensitively() {
    assert!(is_paneflow_environment_key(OsStr::new("PANEFLOW_AI_TOOL")));
    assert!(is_paneflow_environment_key(OsStr::new("paneflow_ai_tool")));
    assert!(!is_paneflow_environment_key(OsStr::new("PATH")));
}

#[test]
fn frame_read_is_bounded_when_peer_stays_open_without_newline() {
    let server = MockServer::start();
    let name = server
        .socket_path
        .as_path()
        .to_fs_name::<GenericFilePath>()
        .expect("test IPC name");
    let mut client = Stream::connect(name).expect("connect partial-frame client");
    client.write_all(b"{").expect("write partial frame");

    let started = Instant::now();
    assert!(server.try_recv(Duration::from_millis(50)).is_none());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "partial frame must not block the test harness"
    );
}

#[test]
fn missing_socket_still_exits_successfully() {
    let (missing_path, _keepalive) = unique_ipc_path();
    let status = run_hook(
        "Stop",
        &HookEnv {
            socket_path: Some(&missing_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        json!({}).to_string().as_bytes(),
    );
    assert!(status.success());
}

#[test]
fn malformed_stdin_logs_and_sends_no_frame() {
    let server = MockServer::start();
    let log_directory = tempfile::TempDir::new().expect("log directory");
    let log_path = log_directory.path().join("hook.log");
    let status = run_hook(
        "UserPromptSubmit",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: Some(&log_path),
        },
        b"not-valid-json",
    );

    assert!(status.success());
    assert!(server.try_recv(Duration::from_millis(250)).is_none());
    assert!(std::fs::read_to_string(log_path)
        .expect("hook log")
        .contains("invalid stdin JSON"));
}

#[test]
fn oversized_stdin_logs_and_sends_no_frame() {
    let server = MockServer::start();
    let log_directory = tempfile::TempDir::new().expect("log directory");
    let log_path = log_directory.path().join("hook.log");
    let oversized = vec![b'x'; MAX_STDIN_BYTES + 1];
    let status = run_hook(
        "UserPromptSubmit",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: Some(&log_path),
        },
        &oversized,
    );

    assert!(status.success());
    assert!(server.try_recv(Duration::from_millis(250)).is_none());
    assert!(std::fs::read_to_string(log_path)
        .expect("hook log")
        .contains("stdin exceeds"));
}
