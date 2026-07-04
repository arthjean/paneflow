// Integration tests are compiled as a separate binary (not under
// `cfg(test)` of the library), so the workspace-wide
// `clippy::panic/unwrap/expect` denies apply to this file directly -
// `clippy.toml`'s `allow-*-in-tests` does NOT cover integration tests
// (see clippy issue #13981, called out in CLAUDE.md). Panic / unwrap /
// expect are idiomatic inside test bodies where a failure IS the
// signal, so allow them at file scope with a clear rationale.
#![allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result
)]

//! US-011 - end-to-end integration tests for `paneflow-ai-hook`.
//!
//! Each test spins up a mock IPC listener (Unix socket under a `TempDir`
//! on Linux/macOS, randomly-named Windows named pipe under
//! `\\.\pipe\paneflow-test-...`), invokes the `paneflow-ai-hook` binary
//! as a subprocess via `std::process::Command` with the expected env and
//! stdin, and asserts that exactly one correctly-shaped JSON-RPC frame
//! arrives on the listener.
//!
//! Cross-platform by design: the same test body runs on Linux, macOS,
//! and Windows (the only platform-specific piece is how the unique IPC
//! path is computed). CI matrix coverage is declared in US-012; the
//! tests themselves must compile and pass on all three OSes.
//!
//! Panic-safe cleanup: `TempDir` is dropped on unwind, which removes
//! the Unix socket file. The `MockServer` owns its `Listener` and
//! detached accept-thread `JoinHandle`; dropping `MockServer` closes the
//! listener, which on Windows releases the named pipe (kernel refcount)
//! and on Unix unlinks the path (via `TempDir`). This prevents leaked
//! pipe handles from accumulating on Windows CI runners, per the
//! explicit US-011 acceptance criterion.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

// Windows pipe-name nonce needs RandomState + OnceLock; gated here so
// the Unix build does not emit unused-import warnings.
#[cfg(windows)]
use std::hash::{BuildHasher, Hasher, RandomState};
#[cfg(windows)]
use std::sync::OnceLock;

use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};
use serde_json::{json, Value};

/// Path to the `paneflow-ai-hook` binary as produced by the surrounding
/// `cargo test` run. Cargo sets this env var for every integration test
/// that lives under `<crate>/tests/` and whose crate has a `[[bin]]`
/// target - see the Cargo book, "Environment Variables Cargo Sets for
/// Integration Tests". Using this avoids shelling back out to `cargo
/// run`, which would fork a second cargo instance, recompile the
/// workspace, and fight the outer lock.
const HOOK_BIN: &str = env!("CARGO_BIN_EXE_paneflow-ai-hook");

/// Wall-clock timeout for a single frame to arrive on the mock listener.
/// The hook binary is ~360 KB stripped (US-008 measurement); local
/// fork+exec + connect + writeln is well under 50 ms on a Linux laptop,
/// but CI runners with cold caches and virtualization overhead can
/// stretch to ~1 s. 5 s is the generous ceiling that keeps slow-CI
/// flakes out without masking real regressions.
const RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// Wall-clock timeout for the subprocess itself to exit. The hook always
/// exits 0; anything longer than `RECV_TIMEOUT + 2 s` means it is hung
/// on stdin or the socket write.
const EXIT_TIMEOUT: Duration = Duration::from_secs(7);

// ---------------------------------------------------------------------------
// Mock IPC server
// ---------------------------------------------------------------------------

/// Cross-platform keepalive for the IPC endpoint's *path*. The `Listener`
/// itself holds the kernel resource; this enum keeps the filesystem
/// backing alive on platforms that need one.
///
/// Each variant is constructed on exactly one OS; the other variant is
/// `dead_code` there. Per-variant allow-listing keeps the suppression
/// precise - if the whole enum were ever unused, the compiler should
/// still warn.
enum PathKeepalive {
    /// Unix: the `TempDir` owns the parent directory; dropping it
    /// removes both the socket file and the directory.
    #[allow(dead_code)]
    Unix(tempfile::TempDir),
    /// Windows: named pipes are refcount-managed by the kernel; when
    /// the listener drops, the pipe vanishes. No filesystem keepalive.
    #[allow(dead_code)]
    Windows,
}

/// Running mock server. Holds the receive channel for the first frame
/// delivered on the endpoint, plus the keepalive needed to keep the
/// endpoint reachable for the lifetime of the test.
struct MockServer {
    /// Path the client must `to_fs_name::<GenericFilePath>()`. On Unix
    /// this is a filesystem socket path; on Windows this is
    /// `\\.\pipe\paneflow-test-<uniq>`.
    socket_path: PathBuf,
    /// Channel that delivers the first newline-terminated line the
    /// listener accepts, or an error string if the accept itself
    /// failed. Only one result is captured; further frames (if any)
    /// are silently dropped after the accept thread exits. Carrying
    /// the accept-level error (as opposed to silent exit) is crucial
    /// on Windows CI, where a `create_sync`-succeeded-but-accept-
    /// failed case would otherwise only surface as a 5 s timeout
    /// with no useful diagnostic.
    rx: mpsc::Receiver<Result<Vec<u8>, String>>,
    _keepalive: PathKeepalive,
}

impl MockServer {
    /// Start a listener on a fresh per-test endpoint + launch an
    /// accept thread that reads ONE newline-terminated frame.
    fn start() -> Self {
        let (socket_path, keepalive) = unique_ipc_path();

        let name = socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .expect("US-011: to_fs_name must succeed on a fresh endpoint path");
        let listener = ListenerOptions::new()
            .name(name)
            .create_sync()
            .expect("US-011: ListenerOptions::create_sync must succeed on a fresh endpoint");

        let (tx, rx) = mpsc::channel::<Result<Vec<u8>, String>>();
        let socket_path_for_thread = socket_path.clone();
        std::thread::spawn(move || {
            // Accept blocks. The test's `recv_timeout` handles the case
            // where the subprocess never connects (e.g., the
            // "socket missing" scenario deliberately points the hook at
            // a *different* path - that is a different listener, not
            // this one; that test does not start a `MockServer` at all).
            let stream = match listener.accept() {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(Err(format!(
                        "listener.accept() on {} failed: {e}",
                        socket_path_for_thread.display()
                    )));
                    return;
                }
            };
            let mut reader = BufReader::new(stream);
            let mut line = Vec::new();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) if line.is_empty() => {
                    let _ = tx.send(Err(format!(
                        "read_until returned EOF before any bytes on {}",
                        socket_path_for_thread.display()
                    )));
                }
                _ => {
                    let _ = tx.send(Ok(line));
                }
            }
            // Accept-thread intentionally exits after one frame. The
            // hook writes exactly one frame per invocation, so this
            // matches the production behavior we're testing.
        });

        Self {
            socket_path,
            rx,
            _keepalive: keepalive,
        }
    }

    /// Block up to `RECV_TIMEOUT` for the first frame and return it as
    /// parsed JSON. Panics with a descriptive message on timeout or
    /// unparseable bytes so test failures point at the specific event.
    fn expect_frame(&self, scenario: &str) -> Value {
        let result = self.rx.recv_timeout(RECV_TIMEOUT).unwrap_or_else(|_| {
            panic!("US-011 [{scenario}]: no frame arrived within {RECV_TIMEOUT:?}")
        });
        let bytes = match result {
            Ok(bytes) => bytes,
            Err(accept_err) => panic!("US-011 [{scenario}]: {accept_err}"),
        };
        let trimmed = trim_trailing_newline(&bytes);
        serde_json::from_slice(trimmed).unwrap_or_else(|e| {
            panic!(
                "US-011 [{scenario}]: frame was not valid JSON: {e}; bytes={}",
                String::from_utf8_lossy(&bytes)
            )
        })
    }

    /// Non-panicking variant: returns `None` on timeout or on an
    /// accept-level failure. Used by the "no frame expected" scenarios
    /// (socket_missing, malformed_stdin) where silence IS the
    /// passing signal.
    fn try_recv(&self, timeout: Duration) -> Option<Value> {
        let bytes = self.rx.recv_timeout(timeout).ok()?.ok()?;
        let trimmed = trim_trailing_newline(&bytes);
        serde_json::from_slice(trimmed).ok()
    }
}

fn trim_trailing_newline(bytes: &[u8]) -> &[u8] {
    match bytes.last() {
        Some(b'\n') => &bytes[..bytes.len() - 1],
        _ => bytes,
    }
}

// ---------------------------------------------------------------------------
// Unique IPC path per test
// ---------------------------------------------------------------------------

/// Monotonic counter so concurrently-running tests never pick the same
/// path, even if two tests happen to hit the same nanosecond boundary.
static UNIQUE: AtomicU64 = AtomicU64::new(0);

#[cfg(unix)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    // `TempDir` chooses a unique directory under `$TMPDIR`. The socket
    // inherits the directory's permissions (user-only by default).
    let dir = tempfile::TempDir::new().expect("US-011: TempDir allocation failed");
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let path = dir.path().join(format!("paneflow-test-{n}.sock"));
    (path, PathKeepalive::Unix(dir))
}

#[cfg(windows)]
fn unique_ipc_path() -> (PathBuf, PathKeepalive) {
    // Named-pipe namespace is global (kernel-wide). Compose a name that
    // is unguessable by a same-UID adversary: the pid + counter + nanos
    // prefix alone is guessable within ~20 bits of nanos entropy at
    // millisecond timing resolution. The 64-bit `process_nonce()` is
    // seeded once per test process from `RandomState`, which uses OS
    // entropy, so the full name carries >=64 bits of attacker-
    // unpredictable randomness - well above the brute-force threshold
    // for a pipe-squatting false-positive.
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let nonce = process_nonce();
    let path = PathBuf::from(format!(
        r"\\.\pipe\paneflow-test-{pid}-{n}-{nanos}-{nonce:016x}"
    ));
    (path, PathKeepalive::Windows)
}

/// Process-lifetime random u64 sourced from OS entropy via `RandomState`.
/// Used as the high-entropy component of the Windows named-pipe name so
/// a same-UID attacker cannot guess the path from observable values
/// (pid + clock). Cached via `OnceLock` so every test in a run sees the
/// same nonce but different runs see different ones.
#[cfg(windows)]
fn process_nonce() -> u64 {
    static NONCE: OnceLock<u64> = OnceLock::new();
    *NONCE.get_or_init(|| {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(std::process::id() as u64);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.finish()
    })
}

// ---------------------------------------------------------------------------
// Hook subprocess helper
// ---------------------------------------------------------------------------

/// Minimal env var set every scenario builds on. Callers add the
/// socket path, workspace id, tool, and optional PID/log path on top.
struct HookEnv<'a> {
    socket_path: Option<&'a PathBuf>,
    workspace_id: u64,
    tool: &'a str,
    pid: Option<u32>,
    hook_log: Option<&'a PathBuf>,
}

/// Spawn `paneflow-ai-hook <event>` with the given env and stdin; wait
/// up to `EXIT_TIMEOUT` for it to exit; panic if the process hangs.
/// Returns the `ExitStatus` so callers can assert exit 0 per PRD C4.
fn run_hook(event: &str, env: &HookEnv<'_>, stdin_bytes: &[u8]) -> std::process::ExitStatus {
    let mut cmd = Command::new(HOOK_BIN);
    cmd.arg(event)
        // Strip inherited env so the host's `PANEFLOW_*` vars cannot
        // leak into the hook - crucial on developer machines that run
        // PaneFlow locally while tests execute.
        .env_clear()
        // Preserve OS basics that the hook needs to start at all.
        // Different OSes require different minima.
        .envs(minimal_os_env())
        .env("PANEFLOW_WORKSPACE_ID", env.workspace_id.to_string())
        .env("PANEFLOW_AI_TOOL", env.tool)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        // Inherit stderr so a Rust panic inside the hook (default
        // panic handler writes to stderr) surfaces in `cargo test`
        // output instead of being silently dropped. The hook writes
        // its normal diagnostics to `$PANEFLOW_HOOK_LOG`, not stderr,
        // so inheriting does not pollute test output on happy paths.
        .stderr(Stdio::inherit());

    if let Some(path) = env.socket_path {
        cmd.env("PANEFLOW_SOCKET_PATH", path);
    }
    if let Some(pid) = env.pid {
        cmd.env("PANEFLOW_AI_PID", pid.to_string());
    }
    if let Some(log) = env.hook_log {
        cmd.env("PANEFLOW_HOOK_LOG", log);
    }

    let mut child = cmd.spawn().expect("US-011: hook subprocess spawn failed");
    child
        .stdin
        .as_mut()
        .expect("US-011: hook stdin must be piped")
        .write_all(stdin_bytes)
        .expect("US-011: stdin write_all failed");
    // Drop stdin to send EOF so the hook's `read_stdin_json` returns.
    drop(child.stdin.take());

    // Poll-with-timeout: `Child::wait` has no timeout API in stable
    // std. Loop on `try_wait` with a short sleep until `EXIT_TIMEOUT`
    // elapses. 50 ms granularity keeps the test latency tight.
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status,
            Ok(None) => {
                if start.elapsed() > EXIT_TIMEOUT {
                    let _ = child.kill();
                    panic!("US-011: hook subprocess did not exit within {EXIT_TIMEOUT:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => panic!("US-011: Child::try_wait failed: {e}"),
        }
    }
}

/// The minimum env needed for the hook to link and execute on each
/// OS. `env_clear()` otherwise strips loader-search-path vars the
/// dynamic linker needs.
///
/// - Linux: dynamic glibc build; `PATH` is sufficient for the test
///   binary (there is no LD_LIBRARY_PATH requirement for a cargo-built
///   target in the standard layout).
/// - macOS: `DYLD_*` vars are SIP-dropped when spawning from a
///   system-integrity-protected parent shell. These forwards work in
///   unprotected terminals and dev-signed builds; in SIP-stripped
///   environments they are simply absent and the dynamic loader falls
///   back to the rpath baked into the Mach-O binary.
/// - Windows: `SystemRoot` + `PATH` are the minimum for `CreateProcess`
///   to resolve the dynamic `api-ms-win-*` forwarders; `ComSpec`,
///   `USERPROFILE`, `LOCALAPPDATA`, `windir`, and `ProgramData` are
///   additionally useful for CRT/NTDLL bootstrap and are safe to
///   forward (they carry no secret material).
fn minimal_os_env() -> Vec<(String, String)> {
    let mut kept = Vec::new();
    for key in [
        // Unix
        "PATH",
        "HOME",
        "TMPDIR",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
        // Windows core + CRT bootstrap
        "TMP",
        "TEMP",
        "SystemRoot",
        "SYSTEMROOT",
        "windir",
        "WINDIR",
        "ComSpec",
        "COMSPEC",
        "USERPROFILE",
        "LOCALAPPDATA",
        "ProgramData",
        "PROGRAMDATA",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
    ] {
        if let Ok(v) = std::env::var(key) {
            kept.push((key.to_string(), v));
        }
    }
    kept
}

// ---------------------------------------------------------------------------
// Envelope + params assertion helper
// ---------------------------------------------------------------------------

struct ExpectedEnvelope<'a> {
    method: &'a str,
    workspace_id: u64,
    tool: &'a str,
    /// Top-level `params.*` keys that must NOT be present on this
    /// method. Used to catch cross-event contamination (e.g., a
    /// regression that leaks `pid` onto every frame). Empty slice for
    /// "don't care".
    absent_keys: &'a [&'a str],
}

/// Assert the top-level JSON-RPC envelope and common `params` fields;
/// return a reference to `params` so per-scenario tests can further
/// assert method-specific fields (`tool_name`, `pid`,
/// `notification_type`, `hook_payload.*`).
fn assert_envelope<'a>(
    frame: &'a Value,
    expected: &ExpectedEnvelope<'_>,
    scenario: &str,
) -> &'a Value {
    assert_eq!(
        frame["jsonrpc"], "2.0",
        "US-011 [{scenario}]: jsonrpc must be \"2.0\""
    );
    assert_eq!(
        frame["method"], expected.method,
        "US-011 [{scenario}]: method mismatch"
    );
    assert!(
        frame.get("id").is_none(),
        "US-011 [{scenario}]: hook frame must be a JSON-RPC notification"
    );
    let params = &frame["params"];
    assert_eq!(
        params["workspace_id"].as_u64(),
        Some(expected.workspace_id),
        "US-011 [{scenario}]: params.workspace_id mismatch"
    );
    assert_eq!(
        params["tool"], expected.tool,
        "US-011 [{scenario}]: params.tool mismatch"
    );
    assert!(
        params.get("hook_payload").is_some(),
        "US-011 [{scenario}]: params.hook_payload must be present"
    );
    for key in expected.absent_keys {
        assert!(
            params.get(*key).is_none(),
            "US-011 [{scenario}]: params.{key} must NOT be present on this event"
        );
    }
    params
}

// ---------------------------------------------------------------------------
// Claude Code event scenarios (6)
// ---------------------------------------------------------------------------

#[test]
fn claude_user_prompt_submit_dispatches_ai_prompt_submit() {
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "abc",
        "transcript_path": "/tmp/t",
        "cwd": "/tmp",
        "permission_mode": "ask",
        "hook_event_name": "UserPromptSubmit",
        "prompt": "hello",
    });
    let status = run_hook(
        "UserPromptSubmit",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 42,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success(), "US-011: hook must exit 0 on happy path");

    let frame = server.expect_frame("claude_prompt_submit");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.prompt_submit",
            workspace_id: 42,
            tool: "claude",
            absent_keys: &[],
        },
        "claude_prompt_submit",
    );
    assert_eq!(params["hook_payload"]["session_id"], "abc");
    assert!(
        params["hook_payload"].get("prompt").is_none(),
        "prompt text must not be forwarded over lifecycle IPC"
    );
}

#[test]
fn claude_notification_dispatches_ai_notification() {
    // Only `permission_prompt` and `elicitation_dialog` count as "needs
    // input" - informational types like `idle_prompt` / `auth_success`
    // are now correctly dropped (otherwise the sidebar would stick on
    // "needs input" after every clean response - see field bug fixed in
    // build_frame's Notification arm).
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "abc",
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": "Allow Bash?",
    });
    let status = run_hook(
        "Notification",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 7,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("claude_notification");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.notification",
            workspace_id: 7,
            tool: "claude",
            absent_keys: &["notification_type"],
        },
        "claude_notification",
    );
    assert_eq!(
        params["hook_payload"]["notification_type"],
        "permission_prompt"
    );
}

#[test]
fn claude_notification_with_informational_type_is_dropped() {
    // Regression guard for the "Claude needs input" false-positive: the
    // hook MUST NOT emit a frame when notification_type is informational.
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "abc",
        "hook_event_name": "Notification",
        "notification_type": "idle_prompt",
        "message": "idle",
    });
    let status = run_hook(
        "Notification",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 7,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success(), "hook must still exit 0 when dropping");
    assert!(
        server.try_recv(Duration::from_millis(500)).is_none(),
        "hook must NOT send a frame for informational notification_type"
    );
}

#[test]
fn claude_stop_dispatches_ai_stop() {
    let server = MockServer::start();
    let stdin = json!({ "session_id": "abc", "hook_event_name": "Stop" });
    let status = run_hook(
        "Stop",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("claude_stop");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.stop",
            workspace_id: 1,
            tool: "claude",
            absent_keys: &["pid"],
        },
        "claude_stop",
    );
}

#[test]
fn claude_subagent_stop_also_dispatches_ai_stop() {
    // AC US-002: SubagentStop is mapped identically to Stop.
    let server = MockServer::start();
    let stdin = json!({ "session_id": "abc", "hook_event_name": "SubagentStop" });
    let status = run_hook(
        "SubagentStop",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("claude_subagent_stop");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.stop",
            workspace_id: 1,
            tool: "claude",
            absent_keys: &["pid"],
        },
        "claude_subagent_stop",
    );
}

#[test]
fn claude_pre_tool_use_dispatches_ai_tool_use_with_tool_name() {
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "abc",
        "hook_event_name": "PreToolUse",
        "tool_name": "Bash",
        "tool_input": { "command": "ls" },
    });
    let status = run_hook(
        "PreToolUse",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 3,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("claude_pre_tool_use");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.tool_use",
            workspace_id: 3,
            tool: "claude",
            absent_keys: &[],
        },
        "claude_pre_tool_use",
    );
    assert_eq!(
        params["tool_name"], "Bash",
        "US-011: tool_name must be mirrored at top level of params"
    );
}

#[test]
fn claude_post_tool_use_also_dispatches_ai_tool_use() {
    // AC US-002: PostToolUse is mapped identically to PreToolUse.
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "abc",
        "hook_event_name": "PostToolUse",
        "tool_name": "Edit",
    });
    let status = run_hook(
        "PostToolUse",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 3,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("claude_post_tool_use");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.tool_use",
            workspace_id: 3,
            tool: "claude",
            absent_keys: &[],
        },
        "claude_post_tool_use",
    );
}

// ---------------------------------------------------------------------------
// Codex event scenarios (6)
// ---------------------------------------------------------------------------

#[test]
fn codex_session_start_dispatches_ai_session_start_with_pid() {
    let server = MockServer::start();
    let stdin = json!({ "session_id": "s1", "hook_event_name": "SessionStart" });
    let status = run_hook(
        "SessionStart",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 5,
            tool: "codex",
            pid: Some(4242),
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_session_start");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.session_start",
            workspace_id: 5,
            tool: "codex",
            absent_keys: &[],
        },
        "codex_session_start",
    );
    assert_eq!(
        params["pid"].as_u64(),
        Some(4242),
        "US-011: SessionStart must carry pid at top level of params"
    );
}

#[test]
fn codex_user_prompt_submit_dispatches_ai_prompt_submit() {
    let server = MockServer::start();
    let stdin =
        json!({ "session_id": "s1", "hook_event_name": "UserPromptSubmit", "prompt": "hi" });
    let status = run_hook(
        "UserPromptSubmit",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 9,
            tool: "codex",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_prompt_submit");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.prompt_submit",
            workspace_id: 9,
            tool: "codex",
            absent_keys: &[],
        },
        "codex_prompt_submit",
    );
}

#[test]
fn codex_notification_dispatches_ai_notification() {
    // Same whitelist as Claude - only permission_prompt /
    // elicitation_dialog get forwarded; everything else is dropped to
    // avoid false-positive "needs input" badges.
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "s1",
        "hook_event_name": "Notification",
        "notification_type": "elicitation_dialog",
        "message": "hi",
    });
    let status = run_hook(
        "Notification",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 9,
            tool: "codex",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_notification");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.notification",
            workspace_id: 9,
            tool: "codex",
            absent_keys: &["notification_type"],
        },
        "codex_notification",
    );
}

#[test]
fn codex_pre_tool_use_dispatches_ai_tool_use() {
    let server = MockServer::start();
    let stdin = json!({
        "session_id": "s1",
        "hook_event_name": "PreToolUse",
        "tool_name": "shell",
    });
    let status = run_hook(
        "PreToolUse",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 9,
            tool: "codex",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_pre_tool_use");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.tool_use",
            workspace_id: 9,
            tool: "codex",
            absent_keys: &[],
        },
        "codex_pre_tool_use",
    );
    assert_eq!(params["tool_name"], "shell");
}

#[test]
fn codex_stop_dispatches_ai_stop() {
    let server = MockServer::start();
    let stdin = json!({ "session_id": "s1", "hook_event_name": "Stop" });
    let status = run_hook(
        "Stop",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 9,
            tool: "codex",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_stop");
    assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.stop",
            workspace_id: 9,
            tool: "codex",
            absent_keys: &["pid"],
        },
        "codex_stop",
    );
}

#[test]
fn codex_permission_request_maps_to_notification_with_permission_prompt() {
    // AC US-003: PermissionRequest → ai.notification + notification_type
    // = "permission_prompt".
    let server = MockServer::start();
    let stdin = json!({ "session_id": "s1", "hook_event_name": "PermissionRequest" });
    let status = run_hook(
        "PermissionRequest",
        &HookEnv {
            socket_path: Some(&server.socket_path),
            workspace_id: 9,
            tool: "codex",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(status.success());

    let frame = server.expect_frame("codex_permission_request");
    let params = assert_envelope(
        &frame,
        &ExpectedEnvelope {
            method: "ai.notification",
            workspace_id: 9,
            tool: "codex",
            absent_keys: &[],
        },
        "codex_permission_request",
    );
    assert_eq!(
        params["notification_type"], "permission_prompt",
        "US-011: PermissionRequest must carry notification_type=permission_prompt"
    );
}

// ---------------------------------------------------------------------------
// Unhappy-path scenarios
// ---------------------------------------------------------------------------

#[test]
fn socket_missing_hook_exits_0_and_no_frame_arrives() {
    // Point the hook at an absolute path with no listener. The hook
    // must exit 0 (PRD C4) and no frame must arrive on any of our
    // helpers (we prove absence via timeout on a freshly-started
    // listener that the hook is NOT pointed at).
    let decoy = MockServer::start();
    let missing_dir = tempfile::TempDir::new().unwrap();
    let missing_path = missing_dir.path().join("does-not-exist.sock");

    let stdin = json!({ "session_id": "x", "hook_event_name": "Stop" });
    let status = run_hook(
        "Stop",
        &HookEnv {
            socket_path: Some(&missing_path),
            workspace_id: 1,
            tool: "claude",
            pid: None,
            hook_log: None,
        },
        stdin.to_string().as_bytes(),
    );
    assert!(
        status.success(),
        "US-011: hook must exit 0 even when socket is unreachable (PRD C4)"
    );

    // 250 ms is enough: if the hook had (wrongly) reached our decoy
    // server, the frame would be queued within milliseconds. This
    // keeps the negative-assertion test fast.
    assert!(
        decoy.try_recv(Duration::from_millis(250)).is_none(),
        "US-011: no frame must land on an unrelated listener when socket is missing"
    );
}

#[test]
fn malformed_stdin_logs_diagnostic_and_exits_0() {
    // AC US-002: when stdin is invalid JSON, the hook exits 0 but
    // writes one diagnostic line to $PANEFLOW_HOOK_LOG if set.
    let server = MockServer::start();
    let log_dir = tempfile::TempDir::new().unwrap();
    let log_path = log_dir.path().join("hook.log");

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
    assert!(
        status.success(),
        "US-011: hook must exit 0 on malformed stdin (PRD C4)"
    );

    assert!(
        server.try_recv(Duration::from_millis(250)).is_none(),
        "US-011: no frame must be sent when stdin is not parseable JSON"
    );

    let log = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("US-011: expected $PANEFLOW_HOOK_LOG at {log_path:?}: {e}"));
    assert!(
        log.contains("paneflow-ai-hook:") && log.contains("invalid stdin JSON"),
        "US-011: diagnostic log must mention the parse failure; got: {log:?}"
    );
}

#[test]
fn oversized_stdin_is_rejected_and_drops_frame() {
    // US-020: the ai-hook's only untrusted input is stdin (it is the hook
    // *producer*, not a JSONL file parser - that surface lives in the
    // sibling session loaders, already bounded by EP-003). The stdin read is
    // capped at MAX_STDIN_BYTES (16 MiB; `crates/paneflow-ai-hook/src/main.rs`
    // const) via the `take(MAX + 1)` idiom, so a payload exceeding the cap is
    // rejected with a diagnostic and no frame is ever sent - a chatty/hostile
    // hook cannot drive unbounded allocation. This regression guard pins that
    // bound (it was the one coverage gap in the harness).
    const MAX_STDIN_BYTES: usize = 16 * 1024 * 1024;

    let server = MockServer::start();
    let log_dir = tempfile::TempDir::new().unwrap();
    let log_path = log_dir.path().join("hook.log");

    // One byte over the cap. Content need not be JSON: the size guard fires
    // before the parse step, so this exercises the bound, not the parser.
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
    assert!(
        status.success(),
        "US-020: hook must still exit 0 on oversized stdin (PRD C4)"
    );

    assert!(
        server.try_recv(Duration::from_millis(250)).is_none(),
        "US-020: no frame must be sent when stdin exceeds the byte cap"
    );

    let log = std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| panic!("US-020: expected $PANEFLOW_HOOK_LOG at {log_path:?}: {e}"));
    assert!(
        log.contains("paneflow-ai-hook:") && log.contains("stdin exceeds"),
        "US-020: diagnostic log must mention the oversize rejection; got: {log:?}"
    );
}
