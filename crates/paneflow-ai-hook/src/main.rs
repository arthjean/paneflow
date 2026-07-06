#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::unwrap_in_result,
        clippy::panic
    )
)]
//! PaneFlow AI-hook callback binary.
//!
//! Invoked by Claude Code / Codex CLI hooks from inside a PaneFlow PTY. Reads
//! the hook JSON from stdin, builds a JSON-RPC 2.0 frame, and writes it to
//! PaneFlow's IPC socket/pipe. Exits 0 on every path (silent fail) so a
//! PaneFlow outage never breaks the user's AI CLI session.
//!
//! US-001 scope: crate scaffolding + blocking JSON-RPC client `send_frame`.
//! US-002 scope: Claude Code hook event → `ai.*` mapping + env/stdin plumbing.
//! US-003 scope: Codex hook event mapping (`SessionStart`, `PermissionRequest`),
//! tool-identity lookup via `$PANEFLOW_AI_TOOL`, and session-start PID capture
//! via `$PANEFLOW_AI_PID` (set by the shim in US-004) with `hook_payload.pid`
//! fallback.

use std::env;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;
#[cfg(windows)]
use std::time::Instant;

#[cfg(windows)]
use interprocess::local_socket::ConnectOptions;
use interprocess::local_socket::{prelude::*, GenericFilePath, Stream};

/// U-027: write deadline for the one-shot hook frame. The hook is invoked
/// synchronously by the shim, so a stalled same-UID socket peer must not block
/// it indefinitely. 500 ms is ample for a local socket write.
const HOOK_IPC_TIMEOUT: Duration = Duration::from_millis(500);

/// Mirrors the server's single-frame read cap. Stdin may be larger because the
/// hook compacts payloads before emitting IPC.
const MAX_HOOK_FRAME_BYTES: usize = 256 * 1024;
const MAX_HOOK_TEXT_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// JSON-RPC method constants
// ---------------------------------------------------------------------------
//
// Mirrors the method strings matched by the server at
// `src-app/src/app/ipc_handler.rs` (lines 347/386/412/441/479/530) and advertised
// by `system.capabilities` at `src-app/src/ipc.rs:242-248`.

const METHOD_SESSION_START: &str = "ai.session_start";
const METHOD_SESSION_END: &str = "ai.session_end";
const METHOD_PROMPT_SUBMIT: &str = "ai.prompt_submit";
const METHOD_NOTIFICATION: &str = "ai.notification";
const METHOD_STOP: &str = "ai.stop";
const METHOD_TOOL_USE: &str = "ai.tool_use";
/// EP-004 US-010: shim-synthesized frame carrying the wrapped agent
/// binary's REAL exit status (the shell's ChildExit never sees it).
const METHOD_EXIT: &str = "ai.exit";
const LIFECYCLE_EVENT_SOURCE_ENV: &str = "PANEFLOW_AI_EVENT_SOURCE";
const LIFECYCLE_EVENT_SOURCE_INTERRUPT: &str = "interrupt";
const PARAM_EVENT_SOURCE: &str = "event_source";

// Tool identity: the agent's BINARY name (`claude`, `codex`, `gemini`,
// `cursor-agent`, …), set by the shim from its own argv[0] stem and
// forwarded verbatim in the `tool` field of every IPC frame. The server
// resolves it via `TerminalAgent::from_binary` and REJECTS unknown strings
// (the historical 2-variant enum silently retyped everything as Claude).
// `ipc.rs` validates the field as alphanumeric + hyphens ≤ 64 chars -
// [`detect_tool_from`] enforces the same shape on this side so a hostile
// env value degrades to the legacy default instead of a rejected frame.
const TOOL_DEFAULT: &str = "claude";

// ---------------------------------------------------------------------------
// JSON-RPC client
// ---------------------------------------------------------------------------

/// Backoffs between delivery attempts (total attempts = 1 + len). A single
/// attempt was enough to drop a lifecycle frame whenever the PaneFlow main
/// thread was busy past the 500 ms timeout at the exact moment the frame
/// arrived (large render, config reload) - and a dropped `ai.stop` left the
/// sidebar spinner on "thinking…" until the 300 s Stalled sweep. Three
/// bounded attempts make that loss practically impossible while keeping the
/// worst case under ~2 s, well inside what Claude Code / Codex tolerate for
/// a hook (and still honoring PRD C4: the final failure stays silent).
const SEND_BACKOFF: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(300)];

/// [`send_frame`] with retry: re-attempts on ANY send error (connect refused
/// from a full backlog, write timeout from a busy main thread, …) - the
/// distinction isn't observable from this side of the socket, and the budget
/// is bounded either way. Returns the LAST error when every attempt fails.
fn send_frame_with_retry(socket_path: &Path, frame: &serde_json::Value) -> std::io::Result<()> {
    let mut result = send_frame(socket_path, frame);
    for backoff in SEND_BACKOFF {
        if result.is_ok() {
            return result;
        }
        std::thread::sleep(backoff);
        result = send_frame(socket_path, frame);
    }
    result
}

/// Open a blocking local-socket connection to `socket_path`, write `frame`
/// serialized as JSON + a single `\n` terminator, then close the stream.
///
/// Mirrors the server framing at `src-app/src/ipc.rs` (newline-delimited
/// JSON-RPC 2.0 read via `BufReader::lines`). Uses `GenericFilePath` on both
/// Unix (domain socket path) and Windows (`\\.\pipe\<name>` pipe path);
/// `interprocess` dispatches to the correct platform primitive internally.
///
/// # Errors
///
/// Returns any `std::io::Error` from name resolution, `Stream::connect`, or
/// the write/flush calls. `dispatch` translates these into a silent exit 0
/// so a missing or stale socket never aborts the user's Claude Code / Codex
/// session (PRD constraint C4).
pub fn send_frame(socket_path: &Path, frame: &serde_json::Value) -> std::io::Result<()> {
    let mut payload = serde_json::to_vec(frame)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    payload.push(b'\n');
    if payload.len() > MAX_HOOK_FRAME_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "paneflow hook IPC frame exceeds the server size cap",
        ));
    }

    let mut stream = connect_frame_stream(socket_path)?;
    // U-027: bound the write. The shim invokes this hook synchronously on
    // post-exit cleanup and the SIGINT path, blocking on its exit; a same-UID
    // squatter that accepts the connection but never drains would otherwise
    // wedge `write_all`/`flush` forever. 500 ms is ample for a local socket
    // write - beyond it `dispatch` turns the error into a silent exit 0, which
    // is the PRD's "fail silent, never break the session" contract (a bounded
    // failure is strictly better than an unbounded hang).
    //
    // Windows named pipes reject set_send_timeout in interprocess, so Windows
    // uses a nonblocking stream and an explicit deadline below.
    #[cfg(not(windows))]
    {
        if let Err(e) = stream.set_send_timeout(Some(HOOK_IPC_TIMEOUT)) {
            if e.kind() != std::io::ErrorKind::Unsupported {
                return Err(e);
            }
        }
    }

    #[cfg(windows)]
    {
        write_all_with_deadline(&mut stream, &payload, HOOK_IPC_TIMEOUT)?;
        stream.flush()?;
        Ok(())
    }

    #[cfg(not(windows))]
    {
        stream.write_all(&payload)?;
        stream.flush()?;
        Ok(())
    }
}

fn connect_frame_stream(socket_path: &Path) -> std::io::Result<Stream> {
    let name = socket_path.to_fs_name::<GenericFilePath>()?;
    #[cfg(windows)]
    {
        ConnectOptions::new()
            .name(name)
            .nonblocking_stream(true)
            .connect_sync()
    }
    #[cfg(not(windows))]
    {
        Stream::connect(name)
    }
}

#[cfg(windows)]
fn wait_for_io(deadline: Instant) -> std::io::Result<()> {
    let now = Instant::now();
    if now >= deadline {
        return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "paneflow hook IPC write timed out",
        ));
    }
    std::thread::sleep((deadline - now).min(Duration::from_millis(5)));
    Ok(())
}

#[cfg(windows)]
fn write_all_with_deadline(
    stream: &mut Stream,
    mut payload: &[u8],
    timeout: Duration,
) -> std::io::Result<()> {
    let deadline = Instant::now() + timeout;
    while !payload.is_empty() {
        match stream.write(payload) {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "paneflow hook IPC write made no progress",
                ));
            }
            Ok(n) => payload = &payload[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => wait_for_io(deadline)?,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Hook event → frame mapping
// ---------------------------------------------------------------------------

/// Build a JSON-RPC 2.0 frame for the given hook event. Returns `None` for
/// unknown event names, or for `SessionStart` when no PID is available (env
/// missing + no `hook_payload.pid`), so the caller can log + skip gracefully.
///
/// Mapping:
/// - Claude Code (US-002):
///   - `UserPromptSubmit` → `ai.prompt_submit`
///   - `Notification`     → `ai.notification`
///   - `Stop` | `SubagentStop` → `ai.stop`
///   - `PreToolUse` | `PostToolUse` → `ai.tool_use` (with top-level `tool_name`
///     mirrored from `hook_payload.tool_name` per `ipc_handler.rs:417-420`)
/// - Codex (US-003, shared arms above plus):
///   - `SessionStart` → `ai.session_start` with required top-level `pid`
///     (nonzero u32; server validates at `ipc_handler.rs:351-358`). PID
///     comes from the `pid` parameter (usually `$PANEFLOW_AI_PID`) or
///     falls back to `hook_payload.pid` before giving up.
///   - `PermissionRequest` → `ai.notification` with additional top-level
///     `notification_type: "permission_prompt"`. The server currently
///     ignores `notification_type` (`ipc_handler.rs:441-478` does not read
///     it), so this is forward-looking metadata that doesn't break today.
#[cfg(test)]
fn build_frame(
    event: &str,
    workspace_id: u64,
    tool: &str,
    hook_payload: serde_json::Value,
    pid: Option<u32>,
    surface_id: Option<u64>,
) -> Option<serde_json::Value> {
    build_frame_with_event_source(
        event,
        workspace_id,
        tool,
        hook_payload,
        pid,
        surface_id,
        None,
    )
}

fn build_frame_with_event_source(
    event: &str,
    workspace_id: u64,
    tool: &str,
    hook_payload: serde_json::Value,
    pid: Option<u32>,
    surface_id: Option<u64>,
    event_source: Option<&str>,
) -> Option<serde_json::Value> {
    let mut params = serde_json::Map::new();
    params.insert("workspace_id".into(), serde_json::Value::from(workspace_id));
    params.insert("tool".into(), serde_json::Value::String(tool.to_owned()));
    if let Some(sid) = surface_id {
        params.insert("surface_id".into(), serde_json::Value::from(sid));
    }

    // Multi-session refactor: stamp the emitting AI binary's PID on EVERY
    // lifecycle frame, not just SessionStart. The server keys
    // `Workspace::agent_sessions` by PID, so without this stamp two
    // concurrent Claude Codes (or any pair of agents) in the same
    // workspace collapse into one row. Falls back to
    // `hook_payload.pid` for non-shim invocations.
    let session_pid = pid.or_else(|| {
        hook_payload
            .get("pid")
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .filter(|&p| p > 0)
    });
    if let Some(p) = session_pid {
        params.insert("pid".into(), serde_json::Value::from(p));
    }

    let method = match event {
        "SessionStart" => {
            // SessionStart REQUIRES a PID (the server validates it at
            // ipc_handler.rs:351-358). The shared stamp above is
            // optional; here we hard-require it and bail if missing.
            session_pid?;
            if let Some(sid) = hook_payload.get("session_id").and_then(|v| v.as_str()) {
                params.insert(
                    "session_id".into(),
                    serde_json::Value::String(sid.to_owned()),
                );
            }
            METHOD_SESSION_START
        }
        "UserPromptSubmit" => METHOD_PROMPT_SUBMIT,
        "Notification" => {
            // Claude Code 2.x fires `Notification` for many event types,
            // most of which are informational (auth_success, idle_prompt,
            // "skills not included" banner, update_available, etc.) and
            // do NOT correspond to "user input required". Only forward
            // the ones that genuinely block on a human.
            //
            // Whitelist (vs blacklist) is the safer policy here: the
            // `notification_type` enum is not stable across Claude Code
            // releases, and any unknown type falsely flagged as
            // `WaitingForInput` would visibly stick on the sidebar
            // (last-write-wins over a preceding `Stop` → Finished).
            //
            // Server-side handler at `ipc_handler.rs:441-478` sets
            // `WaitingForInput` for every frame received here, so the
            // filter MUST live at this layer.
            let notif_type = hook_payload
                .get("notification_type")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match notif_type {
                "permission_prompt" | "elicitation_dialog" => METHOD_NOTIFICATION,
                // Surface unknown types to `$PANEFLOW_HOOK_LOG` so the whitelist
                // can be widened from real telemetry when Anthropic ships a new
                // permission-style type. Stays out of stderr per the hook
                // contract - only the opt-in log path receives it.
                other => {
                    diagnose(&format!(
                        "Notification: dropping notification_type={other:?}"
                    ));
                    return None;
                }
            }
        }
        "Stop" | "SubagentStop" => METHOD_STOP,
        // Exit is shim-synthesized like SessionEnd: `paneflow-shim` fires it
        // after the wrapped agent binary terminates, with the agent's raw
        // exit code (shell `128+signum` convention for signal deaths) in
        // `hook_payload.exit_code` - synthesized by `dispatch` from
        // `$PANEFLOW_AI_EXIT_CODE`. Hard-require the code: a frame without
        // it is useless to the server's `Errored` classifier
        // (`ipc_handler.rs` `ai.exit` arm), so bail instead of degrading.
        "Exit" => {
            let code = hook_payload
                .get("exit_code")
                .and_then(serde_json::Value::as_i64)?;
            params.insert("exit_code".into(), serde_json::Value::from(code));
            METHOD_EXIT
        }
        // SessionEnd is invoked by `paneflow-shim` after the real AI binary
        // exits, NOT by claude/codex themselves - neither tool fires a
        // session-end hook event. The shim runs `paneflow-ai-hook SessionEnd`
        // with empty stdin (no `hook_payload.*` fields are required by the
        // server at `ipc_handler.rs:530-559` beyond `workspace_id` + `tool`,
        // both already in `params`). Without this signal the sidebar loader
        // sticks indefinitely whenever the user quits during a `Thinking`
        // turn (Ctrl+C, /exit mid-stream) - `ai.stop` never fires in that
        // case so the 5s auto-reset never arms.
        "SessionEnd" => METHOD_SESSION_END,
        "PreToolUse" | "PostToolUse" => {
            if let Some(tool_name) = hook_payload.get("tool_name").and_then(|v| v.as_str()) {
                params.insert(
                    "tool_name".into(),
                    serde_json::Value::String(tool_name.to_owned()),
                );
            }
            METHOD_TOOL_USE
        }
        "PermissionRequest" => {
            params.insert(
                "notification_type".into(),
                serde_json::Value::String("permission_prompt".to_owned()),
            );
            METHOD_NOTIFICATION
        }
        _ => return None,
    };

    if matches!(event, "Stop" | "Exit" | "SessionEnd")
        && event_source == Some(LIFECYCLE_EVENT_SOURCE_INTERRUPT)
    {
        params.insert(
            PARAM_EVENT_SOURCE.into(),
            serde_json::Value::String(LIFECYCLE_EVENT_SOURCE_INTERRUPT.to_owned()),
        );
    }

    params.insert(
        "hook_payload".into(),
        compact_hook_payload(event, &hook_payload),
    );

    Some(serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": serde_json::Value::Object(params),
    }))
}

fn compact_hook_payload(event: &str, payload: &serde_json::Value) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    copy_string_field(payload, &mut out, "session_id", 256);
    copy_u64_field(payload, &mut out, "pid");

    match event {
        "SessionStart" => {
            copy_string_field(payload, &mut out, "cwd", 2048);
        }
        "UserPromptSubmit" => {}
        "Notification" => {
            copy_string_field(payload, &mut out, "notification_type", 128);
            copy_string_field(payload, &mut out, "message", MAX_HOOK_TEXT_BYTES);
        }
        "PermissionRequest" => {
            copy_string_field(payload, &mut out, "message", MAX_HOOK_TEXT_BYTES);
        }
        "PreToolUse" | "PostToolUse" => {
            copy_string_field(payload, &mut out, "tool_name", 128);
        }
        "Stop" | "SubagentStop" | "SessionEnd" => {
            copy_string_field(payload, &mut out, "summary", MAX_HOOK_TEXT_BYTES);
            copy_string_field(payload, &mut out, "last_result", MAX_HOOK_TEXT_BYTES);
            copy_string_field(payload, &mut out, "transcript_path", 2048);
        }
        "Exit" => {
            copy_i64_field(payload, &mut out, "exit_code");
            copy_string_field(payload, &mut out, "summary", MAX_HOOK_TEXT_BYTES);
        }
        _ => {}
    }

    serde_json::Value::Object(out)
}

fn copy_string_field(
    source: &serde_json::Value,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    max_bytes: usize,
) {
    let Some(value) = source.get(key).and_then(|v| v.as_str()) else {
        return;
    };
    target.insert(
        key.to_string(),
        serde_json::Value::String(truncate_utf8(value, max_bytes)),
    );
}

fn copy_u64_field(
    source: &serde_json::Value,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    if let Some(value) = source.get(key).and_then(|v| v.as_u64()) {
        target.insert(key.to_string(), serde_json::Value::from(value));
    }
}

fn copy_i64_field(
    source: &serde_json::Value,
    target: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
) {
    if let Some(value) = source.get(key).and_then(|v| v.as_i64()) {
        target.insert(key.to_string(), serde_json::Value::from(value));
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let marker = "...[truncated]";
    let keep = max_bytes.saturating_sub(marker.len());
    let mut boundary = keep.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let mut out = value[..boundary].to_string();
    out.push_str(marker);
    out
}

// ---------------------------------------------------------------------------
// Env-driven tool + PID detection (US-003)
// ---------------------------------------------------------------------------

/// Resolve the AI tool identity from `$PANEFLOW_AI_TOOL`, which the shim
/// sets to one of the 16 `TerminalAgent` binary names based on its own
/// argv[0] stem. Forwarded verbatim - the server is the single authority
/// on the name→agent mapping. Missing or malformed values fall back to
/// `"claude"` (preserves US-002 behavior when the shim is not deployed).
fn detect_tool() -> String {
    detect_tool_from(env::var("PANEFLOW_AI_TOOL").ok().as_deref())
}

/// Testable inner. Keeps the tests out of `env::set_var`, which is Send-unsafe
/// under Cargo's parallel test runner. Mirrors the server-side wire shape
/// (alphanumeric + hyphens, ≤ 64 chars) so a hostile env value can't smuggle
/// arbitrary bytes into the frame.
fn detect_tool_from(raw: Option<&str>) -> String {
    match raw {
        Some(s)
            if !s.is_empty()
                && s.len() <= 64
                && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') =>
        {
            s.to_owned()
        }
        _ => TOOL_DEFAULT.to_owned(),
    }
}

/// Read the AI binary PID from `$PANEFLOW_AI_PID` (set by the US-004 shim
/// after spawning the real Claude/Codex process). Returns `None` if unset,
/// non-numeric, out of `u32` range, or zero - the server rejects `pid == 0`
/// at `ipc_handler.rs:353`.
fn read_ai_pid() -> Option<u32> {
    read_ai_pid_from(env::var("PANEFLOW_AI_PID").ok().as_deref())
}

fn read_ai_pid_from(raw: Option<&str>) -> Option<u32> {
    raw?.parse::<u32>().ok().filter(|&p| p > 0)
}

/// EP-004 US-010: read the wrapped agent binary's raw exit code from
/// `$PANEFLOW_AI_EXIT_CODE` (set by the shim's `notify_exit`). `None` if
/// unset or non-numeric - the caller bails rather than send a degraded
/// frame. Negative values are legitimate (Windows NTSTATUS codes, e.g.
/// `STATUS_CONTROL_C_EXIT` = -1073741510).
fn read_exit_code() -> Option<i32> {
    read_exit_code_from(env::var("PANEFLOW_AI_EXIT_CODE").ok().as_deref())
}

/// Testable inner - same `env::set_var`-avoidance rationale as
/// [`read_ai_pid_from`].
fn read_exit_code_from(raw: Option<&str>) -> Option<i32> {
    raw?.parse::<i32>().ok()
}

fn read_lifecycle_event_source() -> Option<String> {
    read_lifecycle_event_source_from(env::var(LIFECYCLE_EVENT_SOURCE_ENV).ok().as_deref())
}

fn read_lifecycle_event_source_from(raw: Option<&str>) -> Option<String> {
    match raw {
        Some(LIFECYCLE_EVENT_SOURCE_INTERRUPT) => Some(LIFECYCLE_EVENT_SOURCE_INTERRUPT.to_owned()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Diagnostic logging (opt-in)
// ---------------------------------------------------------------------------

/// Append a single diagnostic line to `$PANEFLOW_HOOK_LOG` if set. Silent
/// no-op otherwise - we must never write to stderr in the hook hot path
/// because Claude Code surfaces stderr in its UI.
///
/// Note on symlink follow: `OpenOptions::append(true).create(true)` follows
/// symlinks on Unix. A malicious `$PANEFLOW_HOOK_LOG` symlink can cause
/// log lines to be appended to the symlink target. Severity is LOW because
/// the hook runs as the user, `append` never truncates, and the log content
/// is a single `paneflow-ai-hook: <msg>` line containing no payload data.
fn diagnose(msg: &str) {
    diagnose_to(
        msg,
        env::var_os("PANEFLOW_HOOK_LOG").as_deref().map(Path::new),
    );
}

/// Testable inner: accepts an explicit log path (or `None` to no-op). Keeps
/// the tests out of the `env::set_var`/`remove_var` Send-unsafe path that
/// would otherwise race under Cargo's default parallel test runner.
fn diagnose_to(msg: &str, log_path: Option<&Path>) {
    let Some(log_path) = log_path else {
        return;
    };
    // One atomic append (whole line incl. newline): app/shim/ai-hook write the
    // same file concurrently, and a per-argument `writeln!` tears under that.
    let line = format!("paneflow-ai-hook: {msg}\n");
    let _ = OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

// ---------------------------------------------------------------------------
// Dispatch orchestrator
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    dispatch();
    ExitCode::SUCCESS
}

/// Read env + argv + stdin, build the frame, and send it. Every failure path
/// returns early with an opt-in diagnostic and never propagates an error -
/// PRD constraint C4 mandates that the user's Claude Code session never
/// breaks because of a PaneFlow outage.
fn dispatch() {
    let Some(event) = env::args().nth(1) else {
        diagnose("missing argv[1] hook event name");
        return;
    };

    let Some(socket_path) = read_socket_path() else {
        return;
    };

    let Some(workspace_id) = read_workspace_id() else {
        return;
    };

    // SessionEnd and Exit are the events the shim invokes itself
    // (post-`run_real`) with `Stdio::null()`, so empty stdin is the EXPECTED
    // case - not an error. Skip the stdin read entirely: SessionEnd uses an
    // empty payload `{}`; Exit synthesizes its payload from
    // `$PANEFLOW_AI_EXIT_CODE` (EP-004 US-010). Every other event requires
    // real stdin (the AI tool feeds JSON via the hook contract);
    // empty/malformed there still bails via `read_stdin_json` as before.
    let hook_payload = if event == "SessionEnd" {
        serde_json::json!({})
    } else if event == "Exit" {
        let Some(code) = read_exit_code() else {
            diagnose("Exit: missing or invalid PANEFLOW_AI_EXIT_CODE");
            return;
        };
        serde_json::json!({ "exit_code": code })
    } else {
        let Some(payload) = read_stdin_json(&event) else {
            return;
        };
        payload
    };

    let tool = detect_tool();
    let tool = tool.as_str();
    let pid = read_ai_pid();
    let event_source = read_lifecycle_event_source();
    // US-016 - best-effort: a missing or malformed surface_id leaves the
    // server falling back to workspace-only routing.
    let surface_id = read_surface_id();

    let Some(frame) = build_frame_with_event_source(
        &event,
        workspace_id,
        tool,
        hook_payload,
        pid,
        surface_id,
        event_source.as_deref(),
    ) else {
        // `build_frame` returns `None` in exactly two cases: an unknown event
        // name, or `SessionStart` with no PID resolvable. Distinguish them
        // so a developer reading `$PANEFLOW_HOOK_LOG` knows whether to fix
        // their event name or check their env / stdin.
        let reason = if event == "SessionStart" {
            "missing pid (set $PANEFLOW_AI_PID or include pid in hook JSON)"
        } else if event == "Exit" {
            "missing exit_code in synthesized payload"
        } else {
            "unhandled hook event"
        };
        diagnose(&format!("{event}: {reason}"));
        return;
    };

    if send_frame_with_retry(&socket_path, &frame).is_err() {
        diagnose(&format!("{event}: send_frame failed after retries"));
    }
}

/// Read `PANEFLOW_SOCKET_PATH` and verify it's an absolute path. The PTY
/// injects an absolute path (`runtime_paths.rs:75-83`), so a non-absolute
/// value means either the env was overwritten or the binary is being
/// invoked outside a PaneFlow PTY - either way, do nothing.
fn read_socket_path() -> Option<PathBuf> {
    let raw = env::var_os("PANEFLOW_SOCKET_PATH")?;
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        diagnose("PANEFLOW_SOCKET_PATH is not absolute");
        return None;
    }
    Some(path)
}

fn read_workspace_id() -> Option<u64> {
    let raw = env::var("PANEFLOW_WORKSPACE_ID").ok()?;
    match raw.parse::<u64>() {
        Ok(id) => Some(id),
        Err(_) => {
            diagnose("PANEFLOW_WORKSPACE_ID is not u64");
            None
        }
    }
}

/// Surface (TerminalView) id propagated by `pty_session.rs` as
/// `$PANEFLOW_SURFACE_ID`. Optional: a missing or unparseable value
/// degrades cleanly.
fn read_surface_id() -> Option<u64> {
    env::var("PANEFLOW_SURFACE_ID").ok()?.parse::<u64>().ok()
}

/// Hard cap on the stdin read. Claude Code / Codex hook payloads are tiny
/// JSON objects (session metadata + prompt text); the largest observed is
/// well under 1 MB. 16 MB is the safety ceiling that bounds memory use
/// without constraining legitimate payloads.
const MAX_STDIN_BYTES: u64 = 16 * 1024 * 1024;

/// Read stdin to EOF (or `MAX_STDIN_BYTES`, whichever comes first) and parse
/// as JSON. Returns `None` on empty, oversized, or invalid input (with a
/// diagnostic); never sends a degraded frame. Reads raw bytes (not a
/// `String`) so we skip the stdlib's UTF-8 validation machinery -
/// `serde_json::from_slice` does its own validation.
fn read_stdin_json(event: &str) -> Option<serde_json::Value> {
    let mut buf = Vec::new();
    // US-026: read ONE byte past the cap so a payload of *exactly*
    // MAX_STDIN_BYTES at true EOF is accepted, while a genuine overflow
    // (> MAX_STDIN_BYTES) is still rejected. `take(MAX_STDIN_BYTES)` made the
    // two indistinguishable (both stop at N bytes), so a legitimate
    // exactly-N-byte frame was wrongly dropped.
    if std::io::stdin()
        .take(MAX_STDIN_BYTES + 1)
        .read_to_end(&mut buf)
        .is_err()
    {
        diagnose(&format!("{event}: stdin read error"));
        return None;
    }
    if buf.len() as u64 > MAX_STDIN_BYTES {
        diagnose(&format!("{event}: stdin exceeds {MAX_STDIN_BYTES} bytes"));
        return None;
    }
    if buf.iter().all(u8::is_ascii_whitespace) {
        diagnose(&format!("{event}: empty stdin"));
        return None;
    }
    match serde_json::from_slice(&buf) {
        Ok(v) => Some(v),
        Err(_) => {
            diagnose(&format!("{event}: invalid stdin JSON"));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// Unix-only: the `send_frame` round-trip test uses a filesystem path inside
// `tempfile::TempDir`, which is not a valid Windows named-pipe path
// (`\\.\pipe\...`). Windows coverage is scoped to US-011. The pure-function
// `build_frame` and `diagnose` tests are cfg'd separately below so Windows
// still exercises the mapping table.
#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;

    use std::io::{BufRead, BufReader};

    use interprocess::local_socket::{Listener, ListenerOptions};
    use serde_json::json;
    use tempfile::TempDir;

    /// AC US-001: spin up a `Listener` on a `tempfile::TempDir` path, call
    /// `send_frame`, verify the received bytes match the sent frame.
    #[test]
    fn send_frame_delivers_newline_terminated_json() {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("test.sock");

        // `ToFsName::to_fs_name` takes `self` by value; route through `&Path`
        // so `socket_path` remains usable for the subsequent `send_frame` call.
        let name = socket_path
            .as_path()
            .to_fs_name::<GenericFilePath>()
            .unwrap();
        let listener: Listener = ListenerOptions::new().name(name).create_sync().unwrap();

        let server = std::thread::spawn(move || {
            // `accept` blocks until the kernel delivers a queued connection.
            // `Stream::connect` can land before `accept` is entered - the OS
            // queues pending connections up to the listen backlog - so no
            // settle sleep is needed.
            let stream = listener.accept().expect("listener accept");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).expect("read_line");
            line
        });

        let frame = json!({
            "jsonrpc": "2.0",
            "method": "ai.prompt_submit",
            "params": {
                "workspace_id": 1u64,
                "tool": "claude",
                "hook_payload": { "session_id": "abc", "prompt": "hi" },
            },
            "id": 1,
        });

        send_frame(&socket_path, &frame).expect("send_frame");

        let received = server.join().expect("server thread join");

        // `read_line` keeps the delimiter; `BufReader::lines()` (what the real
        // server uses) strips it. Assert both shapes: the line ends with `\n`,
        // and the JSON body parses back to the same `Value`.
        assert!(
            received.ends_with('\n'),
            "frame must be newline-terminated, got: {received:?}"
        );

        let body = received.trim_end_matches('\n');
        let expected = serde_json::to_string(&frame).unwrap();
        assert_eq!(body, expected, "serialized bytes must match exactly");

        let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(parsed, frame, "round-tripped JSON must equal original");
    }

    #[test]
    fn send_frame_returns_err_when_socket_missing() {
        let dir = TempDir::new().unwrap();
        let socket_path = dir.path().join("does-not-exist.sock");

        let frame = json!({ "jsonrpc": "2.0", "method": "ai.stop", "id": 2 });

        let result = send_frame(&socket_path, &frame);
        assert!(
            result.is_err(),
            "send_frame must return Err when the socket path has no listener"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::{json, Value};

    /// Assert frame envelope shape + return `params` for per-event assertions.
    /// Hook frames are JSON-RPC notifications, so they intentionally omit `id`.
    fn assert_envelope<'a>(frame: &'a Value, expected_method: &str) -> &'a Value {
        assert_eq!(frame["jsonrpc"], "2.0");
        assert_eq!(frame["method"], expected_method);
        assert!(
            frame.get("id").is_none(),
            "hook frames must be JSON-RPC notifications without id"
        );
        &frame["params"]
    }

    // ---------- Claude Code (US-002) ----------

    #[test]
    fn user_prompt_submit_maps_to_ai_prompt_submit() {
        let payload = json!({ "session_id": "s1", "prompt": "hi" });
        let frame = build_frame(
            "UserPromptSubmit",
            42,
            "claude",
            payload.clone(),
            None,
            None,
        )
        .unwrap();

        let params = assert_envelope(&frame, "ai.prompt_submit");
        assert_eq!(params["workspace_id"], 42);
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["hook_payload"], json!({"session_id": "s1"}));
        assert!(params.get("tool_name").is_none());
    }

    #[test]
    fn notification_maps_to_ai_notification() {
        let payload = json!({
            "message": "Allow Bash?",
            "notification_type": "permission_prompt",
        });
        let frame = build_frame("Notification", 7, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.notification");
        assert_eq!(params["workspace_id"], 7);
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["hook_payload"], payload);
        assert!(
            params.get("notification_type").is_none(),
            "Claude's Notification event must not inject top-level \
             notification_type - that is only for Codex's PermissionRequest"
        );
    }

    #[test]
    fn stop_maps_to_ai_stop() {
        let payload = json!({ "session_id": "s1" });
        let frame = build_frame("Stop", 1, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.stop");
        assert_eq!(params["workspace_id"], 1);
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn interrupt_source_is_lifted_for_synthetic_lifecycle_events() {
        let payload = json!({ "session_id": "s1" });
        let frame = build_frame_with_event_source(
            "Stop",
            1,
            "claude",
            payload.clone(),
            None,
            None,
            Some("interrupt"),
        )
        .unwrap();
        let params = assert_envelope(&frame, "ai.stop");
        assert_eq!(params["event_source"], "interrupt");

        let frame = build_frame_with_event_source(
            "UserPromptSubmit",
            1,
            "claude",
            payload,
            None,
            None,
            Some("interrupt"),
        )
        .unwrap();
        let params = assert_envelope(&frame, "ai.prompt_submit");
        assert!(params.get("event_source").is_none());
    }

    #[test]
    fn subagent_stop_maps_to_ai_stop() {
        let payload = json!({ "session_id": "sub" });
        let frame = build_frame("SubagentStop", 1, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.stop");
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["hook_payload"], payload);
    }

    // ---------- Exit (EP-004 US-010, shim-synthesized) ----------

    #[test]
    fn exit_maps_to_ai_exit_with_top_level_exit_code() {
        let payload = json!({ "exit_code": 1 });
        let frame = build_frame("Exit", 9, "claude", payload.clone(), Some(4242), None).unwrap();

        let params = assert_envelope(&frame, "ai.exit");
        assert_eq!(params["workspace_id"], 9);
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["exit_code"], 1, "code must be lifted to top level");
        assert_eq!(params["pid"], 4242, "session routing needs the shim PID");
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn exit_preserves_negative_windows_ntstatus_codes() {
        // STATUS_CONTROL_C_EXIT (0xC000013A) survives the i64 round-trip.
        let payload = json!({ "exit_code": -1_073_741_510_i64 });
        let frame = build_frame("Exit", 9, "codex", payload, None, None).unwrap();
        let params = assert_envelope(&frame, "ai.exit");
        assert_eq!(params["exit_code"], -1_073_741_510_i64);
    }

    #[test]
    fn exit_without_exit_code_bails() {
        assert!(
            build_frame("Exit", 9, "claude", json!({}), None, None).is_none(),
            "an ai.exit frame without exit_code is useless to the classifier"
        );
    }

    #[test]
    fn read_exit_code_from_parses_i32_including_negative() {
        assert_eq!(read_exit_code_from(Some("0")), Some(0));
        assert_eq!(read_exit_code_from(Some("130")), Some(130));
        assert_eq!(
            read_exit_code_from(Some("-1073741510")),
            Some(-1_073_741_510)
        );
        assert_eq!(read_exit_code_from(Some("abc")), None);
        assert_eq!(read_exit_code_from(Some("")), None);
        assert_eq!(read_exit_code_from(None), None);
    }

    #[test]
    fn lifecycle_event_source_only_accepts_interrupt_marker() {
        assert_eq!(
            read_lifecycle_event_source_from(Some("interrupt")).as_deref(),
            Some("interrupt")
        );
        assert!(read_lifecycle_event_source_from(Some("")).is_none());
        assert!(read_lifecycle_event_source_from(Some("other")).is_none());
        assert!(read_lifecycle_event_source_from(None).is_none());
    }

    #[test]
    fn pre_tool_use_maps_to_ai_tool_use_with_tool_name() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "ls" },
        });
        let frame = build_frame("PreToolUse", 3, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.tool_use");
        assert_eq!(params["workspace_id"], 3);
        assert_eq!(params["tool"], "claude");
        assert_eq!(
            params["tool_name"], "Bash",
            "tool_name must be lifted to top-level params from hook_payload"
        );
        assert_eq!(params["hook_payload"], json!({"tool_name": "Bash"}));
    }

    #[test]
    fn post_tool_use_maps_to_ai_tool_use_with_tool_name() {
        let payload = json!({ "tool_name": "Edit" });
        let frame = build_frame("PostToolUse", 3, "claude", payload, None, None).unwrap();

        let params = assert_envelope(&frame, "ai.tool_use");
        assert_eq!(params["tool_name"], "Edit");
    }

    #[test]
    fn pre_tool_use_without_tool_name_omits_top_level_field() {
        // Degraded stdin: PreToolUse hook fired without a tool_name. The frame
        // still dispatches so the server can mark the workspace as tool-busy,
        // but with `tool_name` absent from top-level params.
        let payload = json!({ "tool_input": { "command": "ls" } });
        let frame = build_frame("PreToolUse", 3, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.tool_use");
        assert!(
            params.get("tool_name").is_none(),
            "tool_name must be absent when hook_payload does not provide it"
        );
        assert_eq!(params["hook_payload"], json!({}));
    }

    #[test]
    fn unknown_event_returns_none() {
        let payload = json!({});
        assert!(build_frame("Bogus", 1, "claude", payload.clone(), None, None).is_none());
        assert!(build_frame("", 1, "claude", payload, None, None).is_none());
    }

    #[test]
    fn session_end_maps_to_ai_session_end_with_no_pid_required() {
        // SessionEnd is invoked by paneflow-shim after the real AI binary
        // exits. Unlike SessionStart, no `pid` is required (server only
        // needs workspace_id + tool to clear the loader state).
        let payload = json!({});
        let frame = build_frame("SessionEnd", 7, "codex", payload.clone(), None, None).unwrap();
        let params = assert_envelope(&frame, "ai.session_end");
        assert_eq!(params["workspace_id"], 7);
        assert_eq!(params["tool"], "codex");
        assert!(params.get("pid").is_none(), "session_end carries no pid");
    }

    // ---------- Codex (US-003) ----------

    #[test]
    fn codex_session_start_with_env_pid_maps_to_ai_session_start() {
        let payload = json!({ "session_id": "codex-1", "cwd": "/work" });
        let frame = build_frame(
            "SessionStart",
            5,
            "codex",
            payload.clone(),
            Some(4242),
            None,
        )
        .unwrap();

        let params = assert_envelope(&frame, "ai.session_start");
        assert_eq!(params["workspace_id"], 5);
        assert_eq!(params["tool"], "codex");
        assert_eq!(
            params["pid"], 4242,
            "pid must be lifted to top-level params from the env/shim value"
        );
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn codex_session_start_falls_back_to_stdin_pid() {
        // Env-PID absent, but Codex itself put the pid in the hook JSON. The
        // hook binary must honor it so the frame still dispatches even when
        // invoked outside the US-004 shim.
        let payload = json!({ "session_id": "codex-2", "pid": 7777u64 });
        let frame = build_frame("SessionStart", 9, "codex", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.session_start");
        assert_eq!(params["workspace_id"], 9);
        assert_eq!(params["tool"], "codex");
        assert_eq!(params["pid"], 7777);
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn codex_session_start_without_any_pid_returns_none() {
        // Neither env-PID nor hook_payload.pid - skip the frame (server
        // rejects pid == 0 / missing with `ipc_handler.rs:353`).
        let payload = json!({ "session_id": "codex-3" });
        assert!(
            build_frame("SessionStart", 9, "codex", payload, None, None).is_none(),
            "SessionStart must return None when no pid is resolvable"
        );
    }

    #[test]
    fn codex_session_start_stdin_pid_zero_is_rejected() {
        // Server requires pid > 0. A zero in hook_payload must be treated as
        // absent so the frame isn't built with an invalid pid.
        let payload = json!({ "pid": 0u64 });
        assert!(
            build_frame("SessionStart", 1, "codex", payload, None, None).is_none(),
            "pid == 0 must not satisfy SessionStart's pid requirement"
        );
    }

    #[test]
    fn codex_user_prompt_submit_carries_tool_codex() {
        let payload = json!({ "prompt": "run tests" });
        let frame =
            build_frame("UserPromptSubmit", 2, "codex", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.prompt_submit");
        assert_eq!(params["tool"], "codex");
        assert_eq!(params["hook_payload"], json!({}));
    }

    #[test]
    fn notification_without_input_required_type_is_dropped() {
        // Claude Code fires `Notification` for many informational events
        // (auth_success, idle_prompt, "skills not included" banner, etc.)
        // without a `notification_type` we recognize. These must NOT
        // produce an `ai.notification` frame - otherwise the sidebar
        // shows "needs input" after a clean turn ends, since
        // `WaitingForInput` overwrites the preceding `Stop → Finished`.
        let payload = json!({ "message": "Indexing workspace…" });
        assert!(
            build_frame("Notification", 2, "codex", payload.clone(), None, None).is_none(),
            "Notification without permission_prompt/elicitation_dialog must be dropped"
        );

        let payload_with_unknown = json!({
            "message": "Auth refreshed",
            "notification_type": "auth_success",
        });
        assert!(
            build_frame(
                "Notification",
                2,
                "claude",
                payload_with_unknown,
                None,
                None
            )
            .is_none(),
            "auth_success and other informational types must be dropped"
        );
    }

    #[test]
    fn notification_with_elicitation_dialog_is_forwarded() {
        let payload = json!({
            "message": "What language?",
            "notification_type": "elicitation_dialog",
        });
        let frame = build_frame("Notification", 4, "claude", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.notification");
        assert_eq!(params["tool"], "claude");
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn codex_stop_carries_tool_codex() {
        let payload = json!({ "session_id": "codex-stop" });
        let frame = build_frame("Stop", 2, "codex", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.stop");
        assert_eq!(params["workspace_id"], 2);
        assert_eq!(params["tool"], "codex");
        assert_eq!(params["hook_payload"], payload);
    }

    #[test]
    fn codex_pre_tool_use_carries_tool_codex_and_tool_name() {
        let payload = json!({ "tool_name": "shell", "command": "ls" });
        let frame = build_frame("PreToolUse", 2, "codex", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.tool_use");
        assert_eq!(params["tool"], "codex");
        assert_eq!(params["tool_name"], "shell");
    }

    #[test]
    fn codex_permission_request_maps_to_ai_notification_with_type() {
        let payload = json!({ "message": "Approve shell command?" });
        let frame =
            build_frame("PermissionRequest", 2, "codex", payload.clone(), None, None).unwrap();

        let params = assert_envelope(&frame, "ai.notification");
        assert_eq!(params["tool"], "codex");
        assert_eq!(
            params["notification_type"], "permission_prompt",
            "PermissionRequest must carry top-level notification_type so the \
             server can later distinguish it from generic notifications"
        );
        assert_eq!(params["hook_payload"], payload);
    }

    // ---------- Env-lookup helpers (US-003) ----------

    #[test]
    fn detect_tool_from_forwards_wellformed_names_verbatim() {
        // Any wire-shaped binary name passes through - the SERVER owns the
        // name→agent mapping (TerminalAgent::from_binary) and rejects
        // unknowns; this side must not collapse future agents to claude.
        assert_eq!(detect_tool_from(Some("codex")), "codex");
        assert_eq!(detect_tool_from(Some("claude")), "claude");
        assert_eq!(detect_tool_from(Some("gemini")), "gemini");
        assert_eq!(detect_tool_from(Some("cursor-agent")), "cursor-agent");
        assert_eq!(detect_tool_from(Some("kiro-cli")), "kiro-cli");
    }

    #[test]
    fn detect_tool_from_defaults_on_missing_or_malformed() {
        // Missing env (legacy shim) and wire-shape violations (empty,
        // whitespace, separator smuggling, over-length) all degrade to the
        // historical claude default instead of emitting a rejectable frame.
        assert_eq!(detect_tool_from(None), TOOL_DEFAULT);
        assert_eq!(detect_tool_from(Some("")), TOOL_DEFAULT);
        assert_eq!(detect_tool_from(Some("a b")), TOOL_DEFAULT);
        assert_eq!(detect_tool_from(Some("tool/../etc")), TOOL_DEFAULT);
        assert_eq!(detect_tool_from(Some(&"x".repeat(65))), TOOL_DEFAULT);
    }

    #[test]
    fn read_ai_pid_from_parses_positive_u32() {
        assert_eq!(read_ai_pid_from(Some("1")), Some(1));
        assert_eq!(read_ai_pid_from(Some("4294967295")), Some(u32::MAX));
    }

    #[test]
    fn read_ai_pid_from_rejects_zero_negative_and_nonnumeric() {
        assert_eq!(
            read_ai_pid_from(Some("0")),
            None,
            "pid == 0 is rejected server-side"
        );
        assert_eq!(read_ai_pid_from(Some("-1")), None);
        assert_eq!(read_ai_pid_from(Some("abc")), None);
        assert_eq!(read_ai_pid_from(Some("")), None);
        assert_eq!(read_ai_pid_from(Some("4294967296")), None, "overflows u32");
        assert_eq!(read_ai_pid_from(None), None);
    }

    #[test]
    fn compact_hook_payload_drops_prompt_and_caps_text() {
        let payload = json!({
            "session_id": "s1",
            "prompt": "x".repeat(10_000),
            "message": "y".repeat(10_000),
        });
        let prompt = compact_hook_payload("UserPromptSubmit", &payload);
        assert_eq!(prompt, json!({"session_id": "s1"}));

        let notification = compact_hook_payload("Notification", &payload);
        assert!(notification["message"].as_str().unwrap().len() <= MAX_HOOK_TEXT_BYTES);
        assert!(notification.get("prompt").is_none());
    }

    #[test]
    fn send_frame_rejects_oversized_frame_before_connecting() {
        let frame = json!({
            "jsonrpc": "2.0",
            "method": "ai.notification",
            "params": { "message": "x".repeat(MAX_HOOK_FRAME_BYTES) },
        });
        #[cfg(not(windows))]
        let path = Path::new("/tmp/paneflow-oversized-hook-test.sock");
        #[cfg(windows)]
        let path = Path::new(r"\\.\pipe\paneflow-oversized-hook-test");
        let err = send_frame(path, &frame).expect_err("oversized frame");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    // `diagnose` tests call the testable inner `diagnose_to` directly with an
    // explicit `Option<&Path>` - this avoids `env::set_var`/`remove_var`,
    // which is Send-unsafe on Linux/glibc and would race under Cargo's
    // default parallel test runner.

    #[test]
    fn diagnose_to_is_noop_when_log_path_is_none() {
        // If this panics or writes somewhere unexpected the test framework
        // will surface it; a successful no-op is observed by the absence of
        // any created file in the current directory and no panic.
        diagnose_to("this should vanish", None);
    }

    #[test]
    fn diagnose_to_appends_lines_when_log_path_set() {
        let dir = tempfile::TempDir::new().unwrap();
        let log_path = dir.path().join("hook.log");

        diagnose_to("first line", Some(&log_path));
        diagnose_to("second line", Some(&log_path));

        let contents = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("paneflow-ai-hook: first line"));
        assert!(contents.contains("paneflow-ai-hook: second line"));
        assert_eq!(contents.matches('\n').count(), 2);
    }

    /// Locks in that the Windows named-pipe path produced by
    /// `src-app/src/runtime_paths.rs:82` (`\\.\pipe\paneflow`) is recognised
    /// as absolute by `Path::is_absolute`. If this ever regresses, the hook
    /// binary's `read_socket_path` guard would silently reject every frame
    /// on Windows - a HIGH-severity regression the Phase 7 audit flagged.
    #[test]
    #[cfg(windows)]
    fn windows_named_pipe_path_is_absolute() {
        use std::path::PathBuf;
        let p = PathBuf::from(r"\\.\pipe\paneflow");
        assert!(
            p.is_absolute(),
            "Rust stdlib must treat device-namespace paths as absolute; \
             if this fails, read_socket_path() would silently no-op on Windows"
        );
    }
}
