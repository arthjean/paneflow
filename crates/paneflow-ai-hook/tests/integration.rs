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

const WATCHDOG_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(unix)]
type PathKeepalive = tempfile::TempDir;

#[cfg(windows)]
struct PathKeepalive;

struct MockHost {
    endpoint: PathBuf,
    event_rx: mpsc::Receiver<Value>,
    thread: Option<std::thread::JoinHandle<()>>,
    _keepalive: Option<PathKeepalive>,
}

#[derive(Default)]
struct Order {
    sets: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    requires: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

struct Recorder {
    endpoint: PathBuf,
    seen: mpsc::Receiver<String>,
    thread: std::thread::JoinHandle<()>,
}

const RECORDER_STOP: &str = "recorder.stop";

impl Recorder {
    fn start(endpoint: PathBuf) -> Self {
        let name = endpoint
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("IPC name");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("IPC listener");
        let (seen_tx, seen) = mpsc::channel();
        let thread = std::thread::spawn(move || loop {
            let stream = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream);
            let first = read_json_line(&mut reader);
            let method = first["method"].as_str().unwrap_or_default().to_string();
            if method == RECORDER_STOP {
                return;
            }
            seen_tx.send(method).expect("record");
        });
        Self {
            endpoint,
            seen,
            thread,
        }
    }

    fn contacts(self) -> Vec<String> {
        let name = self
            .endpoint
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("IPC name");
        let mut stream = Stream::connect(name).expect("connect to the recorder");
        stream
            .write_all(format!("{{\"method\":\"{RECORDER_STOP}\"}}\n").as_bytes())
            .expect("stop the recorder");
        self.thread.join().expect("recorder thread");
        self.seen.try_iter().collect()
    }
}

impl MockHost {
    fn start() -> Self {
        Self::replying(json!({"accepted": true}))
    }

    fn replying(reply: Value) -> Self {
        let (endpoint, keepalive) = unique_ipc_path();
        Self::at(
            endpoint,
            Some(keepalive),
            "agent.event",
            reply,
            Order::default(),
        )
    }

    fn at(
        endpoint: PathBuf,
        keepalive: Option<PathKeepalive>,
        method: &'static str,
        reply: Value,
        order: Order,
    ) -> Self {
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
            assert_eq!(event["method"], method);
            let ordered = order
                .requires
                .is_none_or(|flag| flag.load(Ordering::SeqCst));
            if let Some(flag) = order.sets {
                flag.store(true, Ordering::SeqCst);
            }
            event_tx
                .send(json!({"request": event.clone(), "ordered": ordered}))
                .expect("send event");
            write_result(&mut writer, &event, reply);
        });
        Self {
            endpoint,
            event_rx,
            thread: Some(thread),
            _keepalive: keepalive,
        }
    }

    fn event(&self) -> Value {
        self.request()["request"].clone()
    }

    fn request(&self) -> Value {
        self.event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("reporter event")
    }
}

impl Drop for MockHost {
    fn drop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + WATCHDOG_TIMEOUT;
        while !thread.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if std::thread::panicking() {
            return;
        }
        assert!(
            thread.is_finished(),
            "mock host thread still running after {WATCHDOG_TIMEOUT:?}"
        );
        thread.join().expect("host thread");
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

#[cfg(unix)]
fn paired_ipc_paths() -> (PathBuf, PathBuf, PathKeepalive) {
    let directory = tempfile::TempDir::new().expect("temp directory");
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    (
        directory
            .path()
            .join(format!("paneflow-host-{sequence}.sock")),
        directory
            .path()
            .join(format!("paneflow-serve-{sequence}.sock")),
        directory,
    )
}

#[cfg(windows)]
fn paired_ipc_paths() -> (PathBuf, PathBuf, PathKeepalive) {
    let sequence = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let suffix = format!("{}-{sequence}", std::process::id());
    (
        PathBuf::from(format!(r"\\.\pipe\paneflow-host-{suffix}")),
        PathBuf::from(format!(r"\\.\pipe\paneflow-serve-{suffix}")),
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
    args: &[&str],
    hook_env: &HookEnv<'_>,
    stdin_bytes: &[u8],
) -> (std::process::ExitStatus, Duration, Vec<u8>) {
    let mut command = Command::new(executable);
    command
        .args(args)
        .env_clear()
        .envs(non_paneflow_environment())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if !hook_env.tool.is_empty() {
        command.env("PANEFLOW_AI_TOOL", hook_env.tool);
    }
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
    match child.stdin.as_mut().expect("stdin").write_all(stdin_bytes) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(error) => panic!("stdin payload: {error:?}"),
    }
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
                let elapsed = started.elapsed();
                assert!(
                    elapsed < EXIT_TIMEOUT,
                    "reporter took {elapsed:?}, past {EXIT_TIMEOUT:?}"
                );
                return (status, elapsed, stdout);
            }
            Ok(None) if started.elapsed() < WATCHDOG_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(None) => {
                let _ = child.kill();
                panic!("reporter still running after the {WATCHDOG_TIMEOUT:?} watchdog");
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
            &[event],
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
fn a_hook_that_names_no_tool_reaches_the_host_unattributed() {
    let host = MockHost::start();
    let (status, _, _) = run_reporter(
        Path::new(HOOK_BIN),
        &["UserPromptSubmit"],
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: None,
            tool: "",
            generation: Some(3),
            hook_log: None,
        },
        json!({"session_id": "provider-session"})
            .to_string()
            .as_bytes(),
    );
    assert!(status.success());
    let frame = host.event();
    assert!(
        frame["params"].get("tool").is_none(),
        "the reporter never guesses Claude: {frame}"
    );
    assert_eq!(frame["params"]["kind"], "ai.prompt_submit");
}

#[test]
fn a_host_drop_of_an_unattributed_hook_is_logged_with_its_reason() {
    let reason = "the event names no tool and the pane runs no recognized agent runtime";
    let host = MockHost::replying(json!({"accepted": false, "reason": reason}));
    let directory = tempfile::TempDir::new().expect("temp directory");
    let log = directory.path().join("hook.log");
    let (status, _, _) = run_reporter(
        Path::new(HOOK_BIN),
        &["Stop"],
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: None,
            tool: "",
            generation: Some(3),
            hook_log: Some(&log),
        },
        json!({"session_id": "provider-session"})
            .to_string()
            .as_bytes(),
    );
    assert!(status.success());
    assert!(host.event()["params"].get("tool").is_none());
    let logged = std::fs::read_to_string(&log).expect("hook log");
    assert!(logged.contains(reason), "{logged}");
    assert!(!logged.to_ascii_lowercase().contains("claude"), "{logged}");
}

#[test]
fn a_stop_failure_forwards_the_provider_error() {
    let host = MockHost::start();
    let (status, _, _) = run_reporter(
        Path::new(HOOK_BIN),
        &["StopFailure"],
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: None,
            tool: "claude",
            generation: Some(3),
            hook_log: None,
        },
        json!({
            "hook_event_name": "StopFailure",
            "error": "rate_limit",
            "error_details": "429 Too Many Requests",
        })
        .to_string()
        .as_bytes(),
    );
    assert!(status.success());
    let frame = host.event();
    assert_eq!(frame["params"]["hook_payload"]["error"], "rate_limit");
    assert_eq!(
        frame["params"]["hook_payload"]["hook_event_name"],
        "StopFailure"
    );
}

#[test]
fn stdin_preserves_utf8_and_escaped_unicode_prompts() {
    for payload in [
        "{\"prompt\":\"Bonjour, 世界\"}",
        r#"{"prompt":"Bonjour, \u4e16\u754c"}"#,
    ] {
        let host = MockHost::start();
        let (status, _, stdout) = run_reporter(
            Path::new(HOOK_BIN),
            &["UserPromptSubmit"],
            &HookEnv {
                endpoint: Some(&host.endpoint),
                session: Some(SESSION_ID),
                session_dir: None,
                tool: "claude",
                generation: Some(7),
                hook_log: None,
            },
            payload.as_bytes(),
        );
        assert!(status.success());
        assert!(stdout.is_empty());
        assert_eq!(
            host.event()["params"]["hook_payload"]["prompt"],
            "Bonjour, 世界"
        );
    }
}

#[test]
fn malformed_stdin_is_rejected_before_delivery_or_fallback() {
    for payload in [
        b"{\"prompt\":\"\xff\"}".as_slice(),
        br#"{"prompt":"\ud800"}"#,
        b"{]",
    ] {
        let (endpoint, _keepalive) = unique_ipc_path();
        let directory = tempfile::tempdir().expect("session directory");
        let log = directory.path().join("hook.log");
        let (status, _, stdout) = run_reporter(
            Path::new(HOOK_BIN),
            &["UserPromptSubmit"],
            &HookEnv {
                endpoint: Some(&endpoint),
                session: Some(SESSION_ID),
                session_dir: Some(directory.path()),
                tool: "claude",
                generation: Some(7),
                hook_log: Some(&log),
            },
            payload,
        );
        assert!(status.success());
        assert!(stdout.is_empty());
        assert_eq!(
            std::fs::read_to_string(log).expect("diagnostic"),
            "paneflow-ai-hook: UserPromptSubmit: invalid stdin JSON\n"
        );
        assert!(!directory.path().join("last-hook-event.json").exists());
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
            &["Stop"],
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
        &["PermissionRequest"],
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

#[test]
fn unreachable_host_seed_keeps_the_notification_type() {
    let (endpoint, _keepalive) = unique_ipc_path();
    let directory = tempfile::tempdir().expect("session directory");
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["Notification"],
        &HookEnv {
            endpoint: Some(&endpoint),
            session: Some(SESSION_ID),
            session_dir: Some(directory.path()),
            tool: "claude",
            generation: Some(4),
            hook_log: None,
        },
        br#"{"notification_type":"permission_prompt","message":"Approve edit?"}"#,
    );
    assert!(status.success());
    assert!(stdout.is_empty());
    let seed: Value = serde_json::from_slice(
        &std::fs::read(directory.path().join("last-hook-event.json")).expect("seed"),
    )
    .expect("seed JSON");
    assert_eq!(
        seed,
        json!({
            "hook_event_name": "Notification",
            "notification_type": "permission_prompt",
            "runtime_generation": 4
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
        &["SessionStart"],
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

fn background_markers(session_dir: &Path, generation: u64) -> Vec<String> {
    let directory = session_dir
        .join("background-hooks")
        .join(generation.to_string());
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| Some(entry.ok()?.file_name().to_string_lossy().into_owned()))
        .collect();
    names.sort();
    names
}

#[test]
fn subagent_events_keep_a_background_marker_per_child_under_their_generation() {
    let directory = tempfile::tempdir().expect("session directory");
    let log = directory.path().join("hook.log");
    let children = [
        ("explorer-1", "SubagentStart"),
        ("writer_2", "SubagentStart"),
    ];
    for (agent_id, event) in children {
        let host = MockHost::start();
        let (status, _, _) = run_reporter(
            Path::new(HOOK_BIN),
            &[event],
            &HookEnv {
                endpoint: Some(&host.endpoint),
                session: Some(SESSION_ID),
                session_dir: Some(directory.path()),
                tool: "claude",
                generation: Some(5),
                hook_log: Some(&log),
            },
            json!({"session_id": "provider-session", "agent_id": agent_id})
                .to_string()
                .as_bytes(),
        );
        assert!(status.success(), "agent_id={agent_id}");
        assert_eq!(host.event()["params"]["kind"], "ai.session_start");
    }
    assert_eq!(
        background_markers(directory.path(), 5),
        vec!["explorer-1.json".to_string(), "writer_2.json".to_string()]
    );
    let marker: Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .path()
                .join("background-hooks")
                .join("5")
                .join("explorer-1.json"),
        )
        .expect("marker bytes"),
    )
    .expect("marker JSON");
    assert_eq!(
        marker,
        json!({"activity_id": "explorer-1", "runtime_generation": 5})
    );

    let host = MockHost::start();
    let (status, _, _) = run_reporter(
        Path::new(HOOK_BIN),
        &["SubagentStop"],
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: Some(directory.path()),
            tool: "claude",
            generation: Some(5),
            hook_log: Some(&log),
        },
        json!({"session_id": "provider-session", "agent_id": "explorer-1"})
            .to_string()
            .as_bytes(),
    );
    assert!(status.success());
    assert_eq!(host.event()["params"]["kind"], "ai.session_start");
    assert_eq!(
        background_markers(directory.path(), 5),
        vec!["writer_2.json".to_string()],
        "a stopping child removes only its own marker"
    );
}

#[test]
fn an_unusable_background_identity_is_still_forwarded_as_a_latch() {
    let directory = tempfile::tempdir().expect("session directory");
    let log = directory.path().join("hook.log");
    let host = MockHost::start();
    let (status, _, _) = run_reporter(
        Path::new(HOOK_BIN),
        &["SubagentStart"],
        &HookEnv {
            endpoint: Some(&host.endpoint),
            session: Some(SESSION_ID),
            session_dir: Some(directory.path()),
            tool: "claude",
            generation: Some(1),
            hook_log: Some(&log),
        },
        json!({"session_id": "provider-session", "agent_id": "a".repeat(161)})
            .to_string()
            .as_bytes(),
    );
    assert!(status.success());
    assert_eq!(
        host.event()["params"]["hook_payload"]["hook_event_name"],
        "HookSeen",
        "the event still latches the session as hook owned"
    );
    assert!(background_markers(directory.path(), 1).is_empty());
    let recorded = std::fs::read_to_string(&log).expect("hook log");
    assert!(
        recorded.contains("unusable background agent id"),
        "{recorded}"
    );
}

fn status_line_input() -> Value {
    json!({
        "session_id": "b08fea1d-0871-4dd1-acbb-f05e3b9d348b",
        "transcript_path": "/home/u/.claude/projects/p/b08fea1d.jsonl",
        "cost": {"total_cost_usd": 0.8, "total_api_duration_ms": 50_442},
        "rate_limits": {
            "five_hour": {"used_percentage": 13, "resets_at": 1_793_492_400},
            "seven_day": {"used_percentage": 59, "resets_at": 1_793_527_200}
        }
    })
}

fn run_status_line(
    endpoint: Option<&Path>,
    config_dir: &Path,
    stdin_bytes: &[u8],
) -> (std::process::ExitStatus, Duration, Vec<u8>) {
    let mut command = Command::new(HOOK_BIN);
    command
        .arg("StatusLine")
        .env_clear()
        .envs(non_paneflow_environment())
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env("PANEFLOW_SESSION_ID", SESSION_ID)
        .env("PANEFLOW_RUNTIME_GENERATION", "4")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    if let Some(endpoint) = endpoint {
        command.env("PANEFLOW_HOST_ENDPOINT", endpoint);
    }
    let started = Instant::now();
    let mut child = command.spawn().expect("relay process");
    let _ = child.stdin.as_mut().expect("stdin").write_all(stdin_bytes);
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("relay output");
    (output.status, started.elapsed(), output.stdout)
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() * 95 / 100 - 1]
}

#[test]
fn the_status_line_relay_forwards_rate_limits_to_the_host() {
    let host = MockHost::start();
    let config_dir = std::env::temp_dir().join("claude-work");
    let (status, _, stdout) = run_status_line(
        Some(&host.endpoint),
        &config_dir,
        status_line_input().to_string().as_bytes(),
    );
    assert!(status.success());
    assert!(
        stdout.is_empty(),
        "the relay prints nothing without a chained command"
    );
    let frame = host.event();
    assert_eq!(frame["method"], "agent.event");
    assert_eq!(frame["params"]["session"], SESSION_ID);
    assert_eq!(frame["params"]["kind"], "status_line");
    assert_eq!(frame["params"]["runtime_generation"], 4);
    let payload = &frame["params"]["hook_payload"];
    assert_eq!(
        payload["config_dir"],
        config_dir.display().to_string(),
        "{frame}"
    );
    assert_eq!(payload["rate_limits"], status_line_input()["rate_limits"]);
    assert_eq!(
        payload["session_id"],
        "b08fea1d-0871-4dd1-acbb-f05e3b9d348b"
    );
    assert!(payload["received_at_ms"].as_u64().is_some_and(|ms| ms > 0));
    assert!(
        !frame.to_string().contains("transcript"),
        "the transcript path never leaves the relay: {frame}"
    );
}

#[test]
fn the_status_line_relay_outside_paneflow_or_with_bad_input_exits_zero_silently() {
    let config_dir = std::env::temp_dir();
    let (status, elapsed, stdout) = run_status_line(
        None,
        &config_dir,
        status_line_input().to_string().as_bytes(),
    );
    assert!(status.success() && stdout.is_empty());
    assert!(elapsed < EXIT_TIMEOUT);
    let (endpoint, _keepalive) = unique_ipc_path();
    for input in [&b"not json"[..], b"", b"[1]"] {
        let (status, _, stdout) = run_status_line(Some(&endpoint), &config_dir, input);
        assert!(status.success() && stdout.is_empty());
    }
}

#[test]
fn the_status_line_relay_exits_zero_when_the_host_is_down_or_silent() {
    let config_dir = std::env::temp_dir();
    let input = status_line_input().to_string();
    let (absent, _keepalive) = unique_ipc_path();
    let down: Vec<Duration> = (0..100)
        .map(|_| {
            let (status, elapsed, _) =
                run_status_line(Some(&absent), &config_dir, input.as_bytes());
            assert!(status.success());
            elapsed
        })
        .collect();

    let (silent, _silent_keepalive) = unique_ipc_path();
    let name = silent
        .as_path()
        .to_fs_name::<GenericFilePath>()
        .expect("IPC name");
    let listener = ListenerOptions::new()
        .name(name)
        .create_sync()
        .expect("IPC listener");
    let held = std::thread::spawn(move || {
        let mut streams = Vec::new();
        for _ in 0..100 {
            match listener.accept() {
                Ok(stream) => streams.push(stream),
                Err(_) => break,
            }
        }
        streams.len()
    });
    let slow: Vec<Duration> = (0..100)
        .map(|_| {
            let (status, elapsed, _) =
                run_status_line(Some(&silent), &config_dir, input.as_bytes());
            assert!(status.success());
            elapsed
        })
        .collect();
    assert_eq!(held.join().expect("silent host"), 100);
    let (down, slow) = (p95(down), p95(slow));
    eprintln!("status line relay p95: host down {down:?}, host silent {slow:?}");
    if let Some(out) = std::env::var_os("PANEFLOW_GATE_STATUS_LINE_OUT") {
        std::fs::write(
            out,
            serde_json::to_vec_pretty(&json!({
                "runs": 100,
                "host_down_p95_ms": down.as_secs_f64() * 1000.0,
                "host_silent_p95_ms": slow.as_secs_f64() * 1000.0,
            }))
            .expect("status line measurement"),
        )
        .expect("status line measurement file");
    }
}

fn answering_env<'a>(endpoint: &'a Path) -> HookEnv<'a> {
    HookEnv {
        endpoint: Some(endpoint),
        session: Some(SESSION_ID),
        session_dir: None,
        tool: "claude",
        generation: None,
        hook_log: None,
    }
}

const QUESTION_INPUT: &[u8] = br#"{"hook_event_name":"PreToolUse","tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which color?","options":[{"label":"Red"},{"label":"Blue"}],"multiSelect":false}]}}"#;

fn answered_output() -> Value {
    json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "allow",
        "updatedInput": {
            "questions": [{"question": "Which color?", "options": [{"label": "Red"}, {"label": "Blue"}], "multiSelect": false}],
            "answers": {"Which color?": "Blue"}
        }
    }})
}

#[test]
fn an_answered_question_reports_to_the_host_first_then_prints_the_workers_output() {
    let (host_path, serve_path, keepalive) = paired_ipc_paths();
    let reported = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let host = MockHost::at(
        host_path.clone(),
        Some(keepalive),
        "agent.event",
        json!({"accepted": true}),
        Order {
            sets: Some(reported.clone()),
            requires: None,
        },
    );
    let worker = MockHost::at(
        serve_path,
        None,
        "agent_prompt.resolve",
        json!({"status": "answered", "output": answered_output()}),
        Order {
            sets: None,
            requires: Some(reported),
        },
    );
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["PreToolUse", "answer"],
        &answering_env(&host_path),
        QUESTION_INPUT,
    );
    assert!(status.success());
    assert_eq!(host.event()["method"], "agent.event");
    let resolved = worker.request();
    assert_eq!(
        resolved["ordered"], true,
        "the host hears the event before the worker is asked"
    );
    let params = &resolved["request"]["params"];
    assert_eq!(params["event"], "ask_user_question");
    assert_eq!(params["agent"], "claude");
    assert_eq!(params["session"], SESSION_ID);
    assert_eq!(
        params["payload"],
        serde_json::from_slice::<Value>(QUESTION_INPUT).unwrap()
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&stdout).expect("stdout is the hook JSON"),
        answered_output()
    );
}

#[test]
fn a_declined_answer_or_a_hook_without_the_answer_argument_writes_nothing() {
    let (host_path, serve_path, keepalive) = paired_ipc_paths();
    let _host = MockHost::at(
        host_path.clone(),
        Some(keepalive),
        "agent.event",
        json!({"accepted": true}),
        Order::default(),
    );
    let worker = MockHost::at(
        serve_path,
        None,
        "agent_prompt.resolve",
        json!({"status": "declined", "reason": "terminal"}),
        Order::default(),
    );
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["PreToolUse", "answer"],
        &answering_env(&host_path),
        QUESTION_INPUT,
    );
    assert!(status.success());
    assert!(stdout.is_empty());
    assert_eq!(
        worker.request()["request"]["params"]["event"],
        "ask_user_question"
    );

    let (host_path, serve_path, keepalive) = paired_ipc_paths();
    let _host = MockHost::at(
        host_path.clone(),
        Some(keepalive),
        "agent.event",
        json!({"accepted": true}),
        Order::default(),
    );
    let recorder = Recorder::start(serve_path);
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["PreToolUse"],
        &answering_env(&host_path),
        QUESTION_INPUT,
    );
    assert!(status.success());
    assert!(stdout.is_empty());
    assert!(
        recorder.contacts().is_empty(),
        "without the answer argument, which only agent_prompts paneflow writes, the worker is never asked"
    );
}

#[test]
fn an_unreachable_worker_or_a_question_permission_writes_nothing_within_the_hook_bound() {
    let (host_path, _serve_path, keepalive) = paired_ipc_paths();
    let _host = MockHost::at(
        host_path.clone(),
        Some(keepalive),
        "agent.event",
        json!({"accepted": true}),
        Order::default(),
    );
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["PermissionRequest", "answer"],
        &answering_env(&host_path),
        br#"{"hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"ls"}}"#,
    );
    assert!(status.success());
    assert!(stdout.is_empty());

    let (host_path, serve_path, keepalive) = paired_ipc_paths();
    let _host = MockHost::at(
        host_path.clone(),
        Some(keepalive),
        "agent.event",
        json!({"accepted": true}),
        Order::default(),
    );
    let recorder = Recorder::start(serve_path);
    let (status, _, stdout) = run_reporter(
        Path::new(HOOK_BIN),
        &["PermissionRequest", "answer"],
        &answering_env(&host_path),
        br#"{"hook_event_name":"PermissionRequest","tool_name":"AskUserQuestion","tool_input":{}}"#,
    );
    assert!(status.success());
    assert!(stdout.is_empty());
    assert!(
        recorder.contacts().is_empty(),
        "the permission Claude Code raises for a declined question is never asked"
    );
}
