//! JSON-RPC socket server for AI agent control.
//!
//! Listens on `<runtime_dir>/paneflow/paneflow.sock` on Unix and
//! `\\.\pipe\paneflow` on Windows (US-009). Each connection reads
//! newline-delimited JSON-RPC requests and writes responses.
//!
//! Cross-platform IPC is handled by the `interprocess` crate's
//! `local_socket` module, which dispatches to Unix domain sockets on
//! POSIX and named pipes on Windows transparently. The wire protocol
//! (newline-delimited JSON-RPC 2.0) is byte-identical across platforms.
//!
//! ## Trust model - local-only, owner-UID enforcement (US-010)
//!
//! The IPC server is **strictly local**: it has no network surface,
//! no port binding, no remote identity. Trust derives entirely from
//! filesystem and kernel-credential boundaries:
//!
//! - **Socket file mode 0600** (Unix): set immediately after bind in
//!   `bind_socket`. Non-owner processes on the same machine cannot
//!   `connect()` past the kernel filesystem check.
//! - **Peer-UID enforcement** (Unix): every accepted connection runs
//!   `getsockopt(SO_PEERCRED)` (Linux) / `LOCAL_PEERCRED` (macOS) and
//!   compares the peer's UID to the server's. A mismatch returns a
//!   JSON-RPC `-32001 permission denied` error envelope and closes
//!   the stream BEFORE any method dispatches. Defence-in-depth - if a
//!   privileged third party bypasses the file-mode check
//!   (e.g. CAP_DAC_OVERRIDE, mode-fixing automation), the kernel
//!   credential check still rejects them.
//! - **Windows** uses Named Pipes with an explicit SDDL security
//!   descriptor for LocalSystem, Administrators, and the object owner.
//!   We do not rely on the platform default DACL.
//!
//! No HMAC tokens, no TLS - both would add complexity without
//! meaningful gain on a local-only socket. If the IPC ever grows a
//! network surface, that decision must be revisited.
//!
//! ## Per-method blast radius (US-012 cli-hardening-followup-2026-Q3)
//!
//! The trust model above gates *who* can connect (same-UID only). It
//! does NOT gate *what* an authorised client can do. The methods
//! below carry different blast radii once connected:
//!
//! - `system.*`: read-only health checks. Safe.
//! - `workspace.list` / `workspace.current` / `workspace.select` /
//!   `workspace.close`: navigation + workspace lifecycle. Visible
//!   side effects on the UI; no file/system mutation.
//! - `workspace.create`: spawns a PTY at `cwd`. `cwd` is
//!   canonicalised (US-014) and rejected if not a directory.
//! - `surface.split`: layout mutation, bounded by `MAX_PANES`. Bare layout
//!   splits are navigation-level; spawn fields are gated like `workspace.up`.
//! - `workspace.up`: multi-pane creation. Navigation-only pane specs are
//!   allowed for same-UID clients, but `command`, `prompt`, `context`, and
//!   non-empty `env` are orchestration primitives gated behind
//!   `PANEFLOW_IPC_ORCHESTRATION=1`. `PANEFLOW_IPC_SCRIPTING=1` also enables
//!   them as a broader legacy opt-in.
//! - **`surface.send_text` / `surface.send_keystroke`: same-UID RCE
//!   primitive when enabled.** A connected client can inject
//!   arbitrary bytes (including `\n`) into any visible PTY,
//!   effectively running any shell command in the user's
//!   privileges. These are gated behind the
//!   `PANEFLOW_IPC_SCRIPTING=1` opt-in env var; when unset (the
//!   default), the handlers return JSON-RPC error
//!   `-32601 Method not enabled`. The intended consumer is the
//!   trusted same-UID `paneflow-ai-hook` binary; the wrapper
//!   installer can set the env var on the user's behalf with a
//!   visible prompt. `surface.send_keystroke` additionally
//!   rejects CRLF bytes regardless of the opt-in (CRLF injection
//!   bypass guard).
//! - `ai.*`: lifecycle telemetry from the AI hook. Read-only on
//!   the host UI side; safe.
//!
//! ## Methods
//!
//! - `system.ping` / `system.capabilities` / `system.identify` - stateless
//!   health checks handled directly on the socket thread.
//! - `workspace.list` / `workspace.current` / `workspace.select` /
//!   `workspace.close` - workspace navigation.
//! - `workspace.create` - accepts `name` (string, default "Terminal"),
//!   `cwd` (string path, optional) and `layout` (optional `LayoutNode`
//!   JSON, US-001). When `layout` is present, the new workspace's pane
//!   tree is built from the layout in a single round-trip; when absent,
//!   behavior is unchanged (a single default pane). A malformed `layout`
//!   payload returns the JSON-RPC `-32602 Invalid params` error envelope
//!   and leaves no orphan workspace behind.
//! - `workspace.restore_layout` - apply a `LayoutNode` to the active
//!   workspace (used by session restore).
//! - `surface.list` / `surface.send_text` / `surface.send_keystroke` /
//!   `surface.split` - pane operations.
//! - `ai.session_start` / `ai.prompt_submit` / `ai.tool_use` /
//!   `ai.notification` / `ai.stop` / `ai.exit` / `ai.session_end` - AI
//!   hook lifecycle (`ai.exit` carries the wrapped agent binary's real
//!   exit status, EP-004 US-010).
//!
//! Handlers may return a structured JSON-RPC error by emitting the
//! `_jsonrpc_error` sentinel (see `app::ipc_handler::JsonRpcError`); the
//! dispatcher promotes it to a proper `error` envelope. Legacy
//! application errors returned as `{"error": "string"}` are also promoted
//! so clients never treat failures as successful `result` payloads.

use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use interprocess::TryClone;
use interprocess::local_socket::{GenericFilePath, Listener, ListenerOptions, Stream, prelude::*};
// `ListenerNonblockingMode` is only referenced by the Unix-only clobber-
// detection accept loop; gating the import keeps the Windows build warning-free.
#[cfg(unix)]
use interprocess::local_socket::ListenerNonblockingMode;
#[cfg(windows)]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// IPC request type - sent from socket thread to GPUI thread
// ---------------------------------------------------------------------------

pub struct IpcRequest {
    pub method: String,
    pub params: Value,
    pub _id: Value,
    pub response_tx: mpsc::Sender<Value>,
    /// U-053: set by the socket thread when it gives up waiting (the 5 s
    /// dispatch timeout fired and the client already got an error). The GPUI
    /// consumer checks this before running the handler so a slow non-idempotent
    /// mutation (workspace.create, surface.split) can't execute after the
    /// client gave up - otherwise a client retry would create duplicate
    /// workspaces/panes.
    pub cancelled: Arc<AtomicBool>,
    /// Set by the GPUI consumer just before `handle_ipc` runs. The socket
    /// thread cancels only queued work; once a handler starts, it waits for the
    /// real result so retrying clients do not duplicate mutations.
    pub started: Arc<AtomicBool>,
    /// EP-003 US-010 (agent-control-plane): the socket peer's PID, captured
    /// from `SO_PEERCRED` once per connection (None on macOS/Windows, where
    /// the local-socket peer PID is not exposed). Used only to trace writes
    /// granted by AI free-access mode; never an authorization input.
    pub caller_pid: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpcState {
    Online,
    Disabled,
}

const IPC_STATE_ONLINE: u8 = 0;
const IPC_STATE_DISABLED: u8 = 1;

/// US-022: hard cap on the bytes a single newline-delimited request may
// US-013: JSON-RPC framing ceiling, centralized (see `crate::limits`). Still
// accessible as `super::MAX_REQUEST_LEN` from the tests submodule via this use.
use crate::limits::MAX_REQUEST_LEN;

/// US-022: ceiling on concurrently-served IPC connections. The accept loop
/// spawns one blocking thread per connection; without a cap a same-UID peer
/// opening connections in a loop fans out unbounded OS threads. Beyond this,
/// new connections are refused with backpressure (`-32000`) and closed.
const MAX_CONCURRENT_CONNECTIONS: usize = 16;

/// EP-004 US-010: bounded queue from the socket handler threads to the GPUI
/// thread. Once 256 requests are pending, new GPUI-bound requests fail fast
/// with an overload error instead of growing memory without a cap.
pub(crate) const IPC_REQUEST_QUEUE_CAPACITY: usize = 256;

/// EP-004 US-011: maximum live IPC handlers the GPUI thread runs in one tick.
/// Remaining queued requests stay pending for the next scheduled tick.
pub(crate) const IPC_DRAIN_MAX_PER_TICK: usize = 64;

/// Cancelled requests do not spend live handler budget, but draining them is
/// still bounded so a backlog of timed-out requests cannot monopolize a tick.
pub(crate) const IPC_DRAIN_MAX_DEQUEUES_PER_TICK: usize = IPC_DRAIN_MAX_PER_TICK * 2;

/// US-022: idle read deadline per connection. A peer that opens a connection
/// and then sends nothing (or stops mid-stream) otherwise pins its handler
/// thread forever. Enforced at the OS level via `set_recv_timeout`. Generous
/// enough never to cut a real request (clients send immediately on connect
/// and use one connection per request).
const IPC_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for server-side writes. A peer that connects and stops draining
/// must not pin a handler thread while Paneflow tries to write a reply,
/// overload rejection, heartbeat, or event frame.
const IPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub(crate) struct IpcStatus {
    state: Arc<AtomicU8>,
}

impl IpcStatus {
    fn online() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(IPC_STATE_ONLINE)),
        }
    }

    pub(crate) fn state(&self) -> IpcState {
        match self.state.load(Ordering::Acquire) {
            IPC_STATE_DISABLED => IpcState::Disabled,
            _ => IpcState::Online,
        }
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.state() == IpcState::Disabled
    }

    fn disable(&self) {
        self.state.store(IPC_STATE_DISABLED, Ordering::Release);
    }
}

// ---------------------------------------------------------------------------
// Socket server
// ---------------------------------------------------------------------------

/// Start the IPC server on a dedicated OS thread.
/// Returns the receiver for IPC requests to be polled by the GPUI thread.
///
/// On Unix, the server monitors the socket file on disk and automatically
/// re-binds when another instance (e.g. `cargo run`) clobbers it. Without
/// this, the listener becomes orphaned (wrong inode) and all new connections
/// get `ECONNREFUSED`, silently disabling AI hook integration. Named pipes
/// on Windows have different lifecycle semantics (the second process to
/// claim the pipe name fails at creation, not silently), so the clobber
/// detection is Unix-only.
pub fn start_server() -> (
    mpsc::Receiver<IpcRequest>,
    IpcStatus,
    Arc<crate::ipc_events::EventBus>,
) {
    // US-012 (cli-hardening-followup-2026-Q3): one-time boot-time
    // warn-log when scripting is enabled. The per-call gate in
    // `surface.send_text` / `surface.send_keystroke` stays the
    // enforcement boundary; this log surfaces the active-RCE-primitive
    // posture in `paneflow-debug.log` so the operator notices when
    // PANEFLOW_IPC_SCRIPTING was inherited from a launcher script or
    // sourced .env file without their realising.
    let scripting_enabled = std::env::var("PANEFLOW_IPC_SCRIPTING").as_deref() == Ok("1");
    let orchestration_enabled =
        scripting_enabled || std::env::var("PANEFLOW_IPC_ORCHESTRATION").as_deref() == Ok("1");
    if scripting_enabled {
        tracing::warn!(
            "ipc.scripting_enabled is ON; any same-UID process can inject keystrokes into agent panes"
        );
    }
    if orchestration_enabled {
        tracing::warn!(
            "ipc.orchestration_enabled is ON; any same-UID process can create panes with commands, prompts, context, or env"
        );
    }

    let (tx, rx) = mpsc::sync_channel(IPC_REQUEST_QUEUE_CAPACITY);
    let status = IpcStatus::online();
    let thread_status = status.clone();

    // EP-002 (agent-control-plane): the outbound event bus. One handle stays in
    // start_server to be returned to the GPUI app (it broadcasts); a clone moves
    // into the IPC thread so each accepted connection can register a subscriber.
    let event_bus = crate::ipc_events::EventBus::new();
    let thread_event_bus = Arc::clone(&event_bus);

    // Singleton guard: probe the socket BEFORE the IPC thread spawns and
    // before `bind_socket` reclaims any stale socket. If
    // another live Paneflow instance is already listening, two parallel
    // processes will otherwise enter an endless mutual clobber loop -
    // each detects the other's rebind at the next 5 s health check, drops
    // its listener, and re-creates the file, perpetuating the cycle.
    // During every micro-window between drop and re-create, the AI shim's
    // `connect()` fails, an IPC message is silently lost, and a session's
    // `Thinking` / `Done` / `session_start` status stays stale forever.
    //
    // Escape hatch: `PANEFLOW_ALLOW_MULTIPLE=1` skips the guard for the
    // rare case of intentional side-by-side debug instances. Tests do
    // not call `start_server`, so they are unaffected.
    if std::env::var_os("PANEFLOW_ALLOW_MULTIPLE").is_none()
        && let Some(socket_spec) = socket_path_spec()
        && let Some(info) = detect_existing_instance(socket_spec.path())
    {
        eprintln!(
            "paneflow: another Paneflow instance is already running on {}.\n\
             Existing instance: {}\n\
             Close the open window first, or set PANEFLOW_ALLOW_MULTIPLE=1 to override.",
            socket_spec.path().display(),
            info
        );
        log::error!(
            "singleton guard: refusing to start; existing instance on {} ({})",
            socket_spec.path().display(),
            info
        );
        std::process::exit(1);
    }

    // US-005 (cli-hardening-followup-2026-Q3): the IPC thread spawn
    // is fallible (RLIMIT_NPROC exhaustion on a low-ulimit container,
    // EAGAIN on a fork-bombed host). The previous `.expect()` panicked
    // the GPUI main thread on that error, killing every active agent.
    // Mirror the runtime spawn pattern at `runtime.rs:1022-1034`:
    // log + return the `rx` early with no live producer; the consumer
    // is now responsible for tolerating a never-firing channel
    // (it does -- the GPUI poll path checks `try_recv` non-blocking).
    let spawn_result = std::thread::Builder::new()
        .name("paneflow-ipc".into())
        .spawn(move || {
            let Some(socket_spec) = socket_path_spec() else {
                thread_status.disable();
                log::warn!(
                    "paneflow: could not resolve a usable IPC socket path - IPC server disabled. \
                     See earlier runtime_paths warnings for the specific cause."
                );
                return;
            };
            let socket_path = socket_spec.path().to_path_buf();

            // Only Unix needs the containing directory to exist - the
            // Windows named-pipe path lives in the kernel namespace, not
            // the filesystem.
            #[cfg(unix)]
            if !prepare_socket_parent(&socket_spec) {
                thread_status.disable();
                return;
            }

            let listener = match bind_socket(&socket_path) {
                Some(l) => l,
                None => {
                    thread_status.disable();
                    return;
                }
            };

            #[cfg(unix)]
            let mut our_ino = socket_inode(&socket_path).unwrap_or(0);
            #[cfg(unix)]
            let mut last_health_check = std::time::Instant::now();
            #[cfg(unix)]
            let mut listener = listener;
            #[cfg(not(unix))]
            let listener = listener;

            // Non-blocking accept is UNIX-ONLY: it lets the loop periodically
            // re-verify the socket inode (clobber detection) without starving
            // connections. On Windows there is no inode/clobber check, and
            // `interprocess`'s non-blocking named-pipe accept mismanages pipe
            // instances - the accepted `Stream` does not correspond to the
            // client that actually wrote, so the handler's read blocks forever
            // (the frame is "sent OK" by the hook but never delivered to the
            // server read), and that blocked read aborts the whole process at
            // shutdown (STATUS_STACK_BUFFER_OVERRUN, observed in the field).
            // BLOCKING accept on Windows matches the reliable MockServer used
            // by the ai-hook integration suite: accept() waits in the kernel on
            // a single pending instance, so every client's data is delivered to
            // exactly the handler that accepted it. Stream I/O is blocking on
            // both platforms so `handle_connection` can use plain `BufRead`.
            #[cfg(unix)]
            listener
                .set_nonblocking(ListenerNonblockingMode::Accept)
                .ok();

            // US-022: bound the number of concurrently-served connections so a
            // peer opening sockets in a loop can't fan out unbounded threads.
            // Only this (single) accept thread increments; handler threads
            // decrement via the RAII guard below, so the load is exact.
            let active_connections = Arc::new(AtomicUsize::new(0));

            // Decrement the live-connection count on any handler exit path
            // (return, EOF, panic-unwind). Hoisted out of the spawn closure so
            // it can be constructed BEFORE the spawn and moved in: if the spawn
            // itself fails, the closure (and this guard) is dropped, running the
            // decrement and restoring the slot the `fetch_add` below claimed.
            struct ActiveGuard(Arc<AtomicUsize>);
            impl Drop for ActiveGuard {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::AcqRel);
                }
            }

            loop {
                match listener.accept() {
                    Ok(stream) => {
                        if active_connections.load(Ordering::Acquire) >= MAX_CONCURRENT_CONNECTIONS
                        {
                            reject_overloaded(stream);
                            continue;
                        }
                        active_connections.fetch_add(1, Ordering::AcqRel);
                        let guard = ActiveGuard(Arc::clone(&active_connections));
                        let tx = tx.clone();
                        let bus = Arc::clone(&thread_event_bus);
                        // EP-001 US-005 parity: use the fallible `Builder::spawn`,
                        // never the panicking `thread::spawn`. Under
                        // RLIMIT_NPROC / EAGAIN the latter panics and unwinds
                        // this accept thread, silently killing the IPC server
                        // (AI-hook status + MCP bridge go dark while the status
                        // flag still reads Online). On the `Err` path the moved
                        // `guard` and `stream` are dropped here -- the count is
                        // restored and the connection closed -- and the loop
                        // keeps accepting.
                        if let Err(e) = std::thread::Builder::new()
                            .name("paneflow-ipc-conn".into())
                            .spawn(move || {
                                let _guard = guard;
                                handle_connection(stream, tx, bus);
                            })
                        {
                            log::warn!(
                                "IPC: handler thread spawn failed ({e}); dropping this \
                                 connection. Check `ulimit -u` / container thread limits."
                            );
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No pending connection - brief sleep to avoid busy-spin
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        thread_status.disable();
                        log::error!("IPC accept error: {e}");
                        break;
                    }
                }

                // Every 5 seconds, verify our socket file hasn't been
                // clobbered (Unix inode check). Skipped on Windows: named
                // pipes don't have inodes and a concurrent `CreateNamedPipe`
                // fails loudly rather than silently orphaning us.
                #[cfg(unix)]
                if last_health_check.elapsed() >= Duration::from_secs(5) {
                    last_health_check = std::time::Instant::now();
                    let current_ino = socket_inode(&socket_path).unwrap_or(0);
                    if current_ino != our_ino {
                        log::warn!(
                            "IPC socket clobbered (inode {} → {}), re-binding",
                            our_ino,
                            current_ino
                        );
                        drop(listener);
                        match bind_socket(&socket_path) {
                            Some(l) => {
                                l.set_nonblocking(ListenerNonblockingMode::Accept).ok();
                                listener = l;
                                our_ino = socket_inode(&socket_path).unwrap_or(0);
                            }
                            None => {
                                thread_status.disable();
                                return;
                            }
                        }
                    }
                }
            }

            // interprocess' auto name reclamation unlinks the socket file
            // on `Listener::drop` for Unix; this explicit remove is a
            // belt-and-braces no-op there and never runs on Windows
            // (nothing to remove in the named-pipe namespace).
            #[cfg(unix)]
            let _ = remove_socket_file_if_socket(&socket_path, "shutdown cleanup");
        });
    if let Err(e) = spawn_result {
        status.disable();
        tracing::error!(
            "IPC disabled: paneflow-ipc thread spawn failed: {e}. \
             Check `ulimit -u` / container thread limits. \
             External clients (paneflow-ai-hook) will not connect."
        );
        // `tx` was moved into the closure regardless of spawn outcome,
        // so on error the closure (and its captured `tx`) is dropped
        // here. The receiver `rx` then sees `Err(Disconnected)` on
        // every subsequent `try_recv`. The consumer at
        // `app/ipc_handler.rs` uses a non-blocking bounded drain, so both
        // `Empty` and `Disconnected` resolve to "no IPC work this tick" --
        // the app runs normally, only external IPC clients can't reach it.
    }

    (rx, status, event_bus)
}

/// Bind a new listener at the given path/pipe name.
fn bind_socket(socket_path: &std::path::Path) -> Option<Listener> {
    // Unix: remove any stale socket file from a crashed prior run. The
    // interprocess crate's name reclamation handles graceful shutdown;
    // this pre-clean covers `kill -9` / SIGKILL / crash paths.
    // Windows: no-op; the kernel pipe namespace does not retain stale
    // entries after the owning process exits.
    #[cfg(unix)]
    if !remove_socket_file_if_socket(socket_path, "stale IPC socket cleanup") {
        return None;
    }

    let name = match socket_path.to_fs_name::<GenericFilePath>() {
        Ok(n) => n,
        Err(e) => {
            log::error!(
                "Failed to build IPC socket name for {}: {e}",
                socket_path.display()
            );
            return None;
        }
    };

    #[cfg(windows)]
    let listener_result = match windows_named_pipe_security_descriptor() {
        Ok(sd) => ListenerOptions::new()
            .name(name)
            .security_descriptor(sd)
            .create_sync(),
        Err(e) => {
            log::error!(
                "Failed to build IPC named-pipe security descriptor for {}: {e}",
                socket_path.display()
            );
            return None;
        }
    };
    #[cfg(not(windows))]
    let listener_result = ListenerOptions::new().name(name).create_sync();

    let listener = match listener_result {
        Ok(l) => l,
        Err(e) => {
            log::error!(
                "Failed to bind IPC socket at {}: {e}",
                socket_path.display()
            );
            return None;
        }
    };

    // chmod 0o600 - Unix only. Windows named pipes use the explicit SDDL above.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // U-031: the 0600 mode is the PRIMARY trust boundary (peer-UID is
        // defence-in-depth). If chmod fails, the socket keeps its umask-derived
        // creation mode - possibly group/world-connectable - so fail closed:
        // remove the socket and refuse to serve rather than expose it.
        if let Err(e) =
            std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
        {
            log::error!(
                "IPC server: failed to chmod socket {} to 0600 ({e}); refusing to serve",
                socket_path.display()
            );
            let _ = std::fs::remove_file(socket_path);
            return None;
        }
    }
    log::info!("IPC server listening on {}", socket_path.display());
    Some(listener)
}

#[cfg(windows)]
fn windows_named_pipe_security_descriptor() -> std::io::Result<SecurityDescriptor> {
    let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;OW)")
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    SecurityDescriptor::deserialize(sddl.as_ucstr())
}

#[cfg(unix)]
fn prepare_socket_parent(socket_spec: &crate::runtime_paths::IpcSocketPath) -> bool {
    let Some(parent) = socket_spec.path().parent() else {
        return true;
    };

    if socket_spec.owned_parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!(
                "IPC server: failed to create socket parent {} ({e}); refusing to serve",
                parent.display()
            );
            return false;
        }
        // Lock the socket's containing dir to the owner. Under
        // $XDG_RUNTIME_DIR this already holds, but the fallback chain
        // ($TMPDIR / ~/.cache/run) can land in a world-traversable
        // /tmp - 0700 stops other local users from reaching the socket
        // at all (defense-in-depth atop the socket's own 0600 +
        // SO_PEERCRED).
        use std::os::unix::fs::PermissionsExt as _;
        if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)) {
            log::error!(
                "IPC server: failed to chmod owned socket parent {} to 0700 ({e}); refusing to serve",
                parent.display()
            );
            return false;
        }
        return true;
    }

    if parent.is_dir() && unowned_socket_parent_is_safe(parent) {
        true
    } else {
        log::error!(
            "IPC server: PANEFLOW_SOCKET_PATH parent {} is missing, not a directory, or group/world writable without sticky bit; refusing to serve",
            parent.display()
        );
        false
    }
}

#[cfg(unix)]
fn unowned_socket_parent_is_safe(parent: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    let Ok(metadata) = std::fs::metadata(parent) else {
        return false;
    };
    if !metadata.is_dir() {
        return false;
    }
    let mode = metadata.permissions().mode();
    let writable_by_group_or_other = mode & 0o022 != 0;
    let sticky = mode & 0o1000 != 0;
    !writable_by_group_or_other || sticky
}

#[cfg(unix)]
fn remove_socket_file_if_socket(path: &std::path::Path, context: &str) -> bool {
    use std::os::unix::fs::FileTypeExt as _;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return true,
        Err(e) => {
            log::error!(
                "IPC server: failed to inspect {} before {context} ({e}); refusing to serve",
                path.display()
            );
            return false;
        }
    };

    if !metadata.file_type().is_socket() {
        log::error!(
            "IPC server: refusing to remove non-socket path {} during {context}",
            path.display()
        );
        return false;
    }

    if let Err(e) = std::fs::remove_file(path) {
        log::error!(
            "IPC server: failed to remove stale socket {} during {context} ({e}); refusing to serve",
            path.display()
        );
        return false;
    }
    true
}

/// Get the inode number of a filesystem path (0 if the file doesn't exist).
/// Unix-only: used by the clobber-detection health check.
#[cfg(unix)]
fn socket_inode(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.ino())
}

/// Probe `socket_path` to determine whether another live Paneflow instance
/// is already serving on it.
///
/// Returns `Some(identity_string)` if a `system.identify` round-trip
/// succeeds and the response advertises `"PaneFlow"` - the caller must
/// refuse to start. Returns `None` for any other outcome (missing file,
/// stale socket from a SIGKILL'd prior run, non-Paneflow listener, parse
/// failure, timeout) - the caller can safely proceed to `bind_socket`'s
/// existing remove-then-rebind path.
///
/// Resilient to the rebind race window: the legacy `bind_socket` recreates
/// the socket on every 5 s clobber-detection tick, and during the few-ms
/// window between `drop(listener)` and `create_sync()` a `connect()` would
/// spuriously return `ECONNREFUSED`. We retry up to 3 times with a short
/// inter-attempt sleep to cross that window deterministically.
///
/// Once this guard is universally deployed, the rebind loop never starts
/// (the second instance exits before bind), so the multi-attempt is
/// belt-and-braces for the transition period and for SIGKILL recovery
/// races where the OS hasn't yet released the file.
fn detect_existing_instance(socket_path: &std::path::Path) -> Option<String> {
    // Fast bail-out: no socket file at all = definitely no instance.
    // Avoids the connect overhead in the common cold-start case.
    #[cfg(unix)]
    if !socket_path.exists() {
        return None;
    }

    let name = socket_path.to_fs_name::<GenericFilePath>().ok()?;

    for attempt in 0..3 {
        if attempt > 0 {
            // Cross the legacy rebind window. The bind_socket recreate
            // path is bounded by `remove_file` + `create_sync` + chmod -
            // typically well under 10 ms; 70 ms is a comfortable margin.
            std::thread::sleep(Duration::from_millis(70));
        }

        let Ok(mut stream) = Stream::connect(name.clone()) else {
            continue;
        };

        // US-022: bound the probe at the OS level (`set_recv_timeout`, same
        // mechanism as the bridge client) instead of a scratch thread that
        // leaked on every timeout. 300 ms is generous for a stateless
        // socket-thread handler; a live but unresponsive process within that
        // budget is functionally indistinguishable from "no peer" and we
        // proceed to bind. A hostile squatter on the path can neither stall us
        // (the deadline) nor feed us an unbounded line (the `take` cap).
        if stream
            .set_recv_timeout(Some(Duration::from_millis(300)))
            .is_err()
        {
            continue;
        }

        // Stateless ping handled directly on the peer's socket thread
        // (see `handle_connection`), so a live instance responds in
        // microseconds without any GPUI round-trip.
        if stream
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system.identify\"}\n")
            .is_err()
        {
            continue;
        }
        let _ = stream.flush();

        let mut line = String::new();
        if BufReader::new(stream)
            .take(MAX_REQUEST_LEN)
            .read_line(&mut line)
            .is_err()
        {
            continue;
        }

        // The `system.identify` result includes `"name":"PaneFlow"` (see
        // `handle_connection`). Match on the literal so a non-Paneflow
        // listener squatting on the same path doesn't pin us to exit -
        // we'd rather clobber an unknown squatter than refuse to start.
        if line.contains("\"PaneFlow\"") {
            return Some(line.trim().to_string());
        }
    }

    None
}

/// Outcome of one capped request read (US-022).
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// Clean end of stream.
    Eof,
    /// The line reached `MAX_REQUEST_LEN` without a newline - oversized.
    TooLong,
    /// A complete (or trailing) line was read into the buffer.
    Got,
}

/// Read one newline-delimited request into `line`, capped at
/// [`MAX_REQUEST_LEN`]. `Take` is rebuilt per call so the limit is per-line;
/// a line that hits the cap without a terminating newline is reported as
/// [`LineRead::TooLong`] rather than allocated unboundedly (the DoS the cap
/// exists to stop). Pure framing logic, unit-tested below.
#[cfg(any(test, not(windows)))]
fn read_capped_line(reader: &mut impl BufRead, line: &mut String) -> std::io::Result<LineRead> {
    line.clear();
    // `by_ref()` reborrows so `Take` owns a `&mut reader`, not `reader` itself
    // (the cap is per-call, and the caller keeps the reader for the next line).
    let n = reader.by_ref().take(MAX_REQUEST_LEN).read_line(line)?;
    if n == 0 {
        return Ok(LineRead::Eof);
    }
    if n as u64 >= MAX_REQUEST_LEN && !line.ends_with('\n') {
        return Ok(LineRead::TooLong);
    }
    Ok(LineRead::Got)
}

#[cfg(not(windows))]
fn read_request_line(reader: &mut impl BufRead, line: &mut String) -> std::io::Result<LineRead> {
    read_capped_line(reader, line)
}

/// Windows named-pipe read path that avoids interprocess's `ReadFileEx`
/// `CannotUnwind` guard. A peer can connect and close before sending a frame;
/// interprocess then aborts the process instead of returning `BrokenPipe`.
#[cfg(windows)]
fn read_request_line(stream: &mut Stream, line: &mut String) -> std::io::Result<LineRead> {
    let mut bytes = Vec::new();
    let mut scratch = [0u8; 4096];

    loop {
        let remaining = MAX_REQUEST_LEN as usize - bytes.len();
        if remaining == 0 {
            break;
        }

        let read_len = remaining.min(scratch.len());
        let n = match pipe_read_some(stream, &mut scratch[..read_len]) {
            Ok(n) => n,
            Err(e) if closed_pipe_error(&e) => return Ok(LineRead::Eof),
            Err(e) => return Err(e),
        };
        if n == 0 {
            if bytes.is_empty() {
                return Ok(LineRead::Eof);
            }
            break;
        }

        let chunk = &scratch[..n];
        if let Some(newline) = chunk.iter().position(|b| *b == b'\n') {
            bytes.extend_from_slice(&chunk[..=newline]);
            break;
        }
        bytes.extend_from_slice(chunk);
    }

    line.clear();
    line.push_str(
        std::str::from_utf8(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
    );

    if line.is_empty() {
        Ok(LineRead::Eof)
    } else if bytes.len() as u64 >= MAX_REQUEST_LEN && !line.ends_with('\n') {
        Ok(LineRead::TooLong)
    } else {
        Ok(LineRead::Got)
    }
}

/// US-022 backpressure: refuse a connection once the concurrency cap is hit.
/// Writes one JSON-RPC error envelope and drops the stream (closing it) so the
/// peer gets a structured rejection rather than a silent hang.
fn reject_overloaded(mut stream: Stream) {
    let envelope = json!({
        "jsonrpc": "2.0",
        "error": {"code": -32000, "message": "server busy: too many concurrent connections"},
        "id": Value::Null,
    });
    // Abort-safe write (CP-4): on Windows a peer that already closed must not
    // trip interprocess's overlapped-write abort; `write_envelope` routes
    // through our managed WriteFile. `stream` is dropped right after either way.
    let _ = write_envelope(&mut stream, &envelope);
}

/// EP-003 US-010 (agent-control-plane): the connected peer's PID, for tracing
/// writes granted by AI free-access mode. `SO_PEERCRED` exposes it on Linux;
/// macOS `LOCAL_PEERCRED` and Windows named pipes do not, so this returns None
/// there. Best-effort and advisory only - never an authorization input (the
/// peer-UID check in `auth::check_peer` is the security boundary).
#[cfg(unix)]
fn peer_pid(stream: &Stream) -> Option<i64> {
    stream
        .peer_creds()
        .ok()
        .and_then(|c| c.pid())
        .map(|p| p as i64)
}

#[cfg(not(unix))]
fn peer_pid(_stream: &Stream) -> Option<i64> {
    None
}

fn handle_connection(
    stream: Stream,
    request_tx: mpsc::SyncSender<IpcRequest>,
    event_bus: Arc<crate::ipc_events::EventBus>,
) {
    // EP-006 US-013: `event_bus` now feeds `serve_subscription` on BOTH
    // platforms (the Windows event stream is no longer stubbed), so the former
    // `#[cfg(not(unix))] let _ = &event_bus;` unused-variable shim is gone.

    // `Stream::try_clone` is provided by `interprocess::TryClone` and
    // works on both Unix domain sockets and Windows named pipes. One
    // handle reads, the other writes, so request/response flow does not
    // fight over a single mutable cursor.
    let Ok(writer_stream) = stream.try_clone() else {
        return;
    };

    // US-010: peer-UID enforcement happens BEFORE we wrap `stream` in
    // a BufReader, because the cleanest way to query peer credentials
    // on `interprocess::local_socket::Stream` is the trait method
    // `Stream::peer_creds()` (brought in by `prelude::*`), and that
    // method needs the bare stream - once wrapped in BufReader, the
    // method is no longer reachable through `get_ref()` (BufReader
    // only re-exports `Read`-shaped methods). The check is
    // `#[cfg(unix)]`-only; Windows pipe ACLs cover the same surface
    // (see module doc) and SDDL hardening is deferred per PRD §10.
    // On a peer-cred query failure we fall back to perms-0600 only
    // with a warn log (AC6) - the kernel filesystem check still
    // gates non-owner connects, so the residual exposure is bounded.
    let mut writer = writer_stream;

    #[cfg(unix)]
    {
        match auth::check_peer(&stream) {
            auth::AuthOutcome::Allow => {}
            auth::AuthOutcome::Deny {
                server_uid,
                peer_uid,
            } => {
                let envelope = json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32001,
                        "message": "permission denied: peer UID mismatch"
                    },
                    "id": Value::Null,
                });
                let _ = writeln!(&mut writer, "{}", envelope);
                let _ = writer.flush();
                log::warn!(
                    "IPC: rejecting connection (peer UID {}, server UID {})",
                    peer_uid,
                    server_uid
                );
                return;
            }
            auth::AuthOutcome::DegradedFallback => {
                // AC6: peer-cred query unavailable, perms-0600 stays
                // as the line of defence. Warn-log emitted inside
                // check_peer so the fallback isn't silent.
            }
        }
    }

    // EP-003 US-010: capture the peer PID once, while `stream` is still the
    // bare socket (peer_creds is unreachable through the BufReader wrapper
    // below). Threaded into each IpcRequest for the free-access write trace.
    let caller_pid = peer_pid(&stream);

    // US-022 / EP-004: drop a peer that opens a connection and then goes mute,
    // so it can't pin this handler thread forever. Unix sockets use the OS
    // receive timeout here; Windows named pipes are bounded inside
    // `pipe_read_some` because `set_recv_timeout` is unsupported there.
    let _ = stream.set_recv_timeout(Some(IPC_IDLE_TIMEOUT));

    #[cfg(windows)]
    let mut reader = stream;
    #[cfg(not(windows))]
    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    loop {
        match read_request_line(&mut reader, &mut line) {
            Ok(LineRead::Eof) => break,
            Ok(LineRead::TooLong) => {
                // US-022: oversized request → structured rejection + close,
                // never an unbounded allocation.
                let envelope = json!({
                    "jsonrpc": "2.0",
                    "error": {"code": -32600, "message": "request exceeds maximum length"},
                    "id": Value::Null,
                });
                // Abort-safe write (CP-4): see `write_envelope`.
                let _ = write_envelope(&mut writer, &envelope);
                break;
            }
            Ok(LineRead::Got) => {}
            // Idle timeout (WouldBlock on Unix, TimedOut on Windows) or any
            // other read error → drop the connection.
            Err(_) => break,
        }

        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let mut suppress_reply = false;
        let response = match serde_json::from_str::<Value>(line) {
            Ok(req) => {
                let id = req.get("id").cloned();
                let response_id = id.clone().unwrap_or(Value::Null);
                suppress_reply = id.is_none();
                match req.get("method").and_then(|m| m.as_str()) {
                    Some(method) => {
                        let method = method.to_string();
                        let params = req.get("params").cloned().unwrap_or(json!({}));

                        if method == "events.subscribe" {
                            serve_subscription(&mut writer, &params, &event_bus);
                            return;
                        }

                        if method.starts_with("ai.") {
                            crate::ai_hooks::hook_diag(&format!(
                                "ipc server received {method} (tool={:?} pid={:?} ws={:?})",
                                params.get("tool"),
                                params.get("pid"),
                                params.get("workspace_id"),
                            ));
                        }

                        match method.as_str() {
                            "system.ping" => {
                                json!({"jsonrpc": "2.0", "result": {"pong": true}, "id": response_id})
                            }
                            "system.capabilities" => {
                                json!({"jsonrpc": "2.0", "result": {
                                    "scripting": std::env::var("PANEFLOW_IPC_SCRIPTING")
                                        .is_ok_and(|v| v == "1"),
                                    "orchestration": std::env::var("PANEFLOW_IPC_ORCHESTRATION")
                                        .is_ok_and(|v| v == "1")
                                        || std::env::var("PANEFLOW_IPC_SCRIPTING")
                                            .is_ok_and(|v| v == "1"),
                                    "methods": [
                                        "system.ping", "system.capabilities", "system.identify",
                                        "workspace.list", "workspace.create", "workspace.select",
                                        "workspace.close", "workspace.current",
                                        "workspace.restore_layout", "workspace.up",
                                        "surface.list", "surface.read", "surface.search", "surface.rename",
                                        "surface.send_text", "surface.send_keystroke", "surface.split",
                                        "surface.focus", "surface.status",
                                        "fleet.list",
                                        "events.subscribe",
                                        "ai.session_start",
                                        "ai.prompt_submit",
                                        "ai.tool_use",
                                        "ai.notification",
                                        "ai.stop",
                                        "ai.exit",
                                        "ai.session_end"
                                    ]
                                }, "id": response_id})
                            }
                            "system.identify" => {
                                json!({"jsonrpc": "2.0", "result": {
                                    "name": "PaneFlow",
                                    "version": env!("CARGO_PKG_VERSION"),
                                    "protocol": "jsonrpc-2.0"
                                }, "id": response_id})
                            }
                            _ => dispatch_to_gpui(
                                &request_tx,
                                method,
                                params,
                                response_id,
                                caller_pid,
                            ),
                        }
                    }
                    None => {
                        suppress_reply = false;
                        json!({"jsonrpc": "2.0", "error": {"code": -32600, "message": "Invalid Request"}, "id": response_id})
                    }
                }
            }
            Err(e) => {
                json!({"jsonrpc": "2.0", "error": {"code": -32700, "message": format!("Parse error: {e}")}, "id": null})
            }
        };

        // JSON-RPC notifications do not receive replies. Requests with an `id`,
        // including `ai.*`, reply normally.
        if !suppress_reply && !write_envelope(&mut writer, &response) {
            break;
        }

        // Windows: serve exactly ONE request per connection. Every paneflow
        // client closes after a single exchange - `paneflow-ai-hook` is
        // fire-and-forget (writes one frame, closes immediately without reading
        // the reply), and `paneflow-ipc-client` does one request/response then
        // drops the stream. Looping back to a SECOND read on a now peer-closed
        // Windows named pipe also aborts the ENTIRE process inside `interprocess`
        // (STATUS_STACK_BUFFER_OVERRUN 0xC0000409 - the read `__fastfail`s
        // instead of returning EOF; confirmed with a live debugger). Unix has no
        // such abort (EOF is returned cleanly), so it keeps the multi-request loop.
        #[cfg(windows)]
        break;
    }
}

/// EP-002 / EP-006 (agent-control-plane): serve a persistent `events.subscribe`
/// stream. Registers a subscriber, writes a `subscribed` ack, then writes each
/// pushed event line until the client disconnects. A 30 s idle tick emits a
/// heartbeat (US-007) so a dead client is detected even when no events flow, and
/// any backlog shed under backpressure (US-004) is reported as a `dropped`
/// marker. Returns when a push fails (client gone) or the bus shuts down; the
/// `Subscription` drops here, unsubscribing (RAII).
///
/// Cross-platform (US-013). On Windows, every push goes through [`push_frame`] /
/// [`push_line`], which probe the named pipe's liveness with `PeekNamedPipe`
/// BEFORE writing: a write to an already-disconnected named pipe does NOT return
/// an error from `interprocess` - it aborts the whole process via the overlapped
/// `WriteFileEx` `CannotUnwind` guard (the same abort the request/response path
/// dodges with `suppress_reply`), and no `catch_unwind` can stop a `process::
/// abort`. The probe turns a `watch` client's Ctrl-C into a clean RAII eviction.
/// Runtime behaviour is smoke-tested on Windows (US-014); the build host is Linux.
fn serve_subscription(writer: &mut Stream, params: &Value, bus: &Arc<crate::ipc_events::EventBus>) {
    use std::sync::mpsc::RecvTimeoutError;

    const HEARTBEAT: Duration = Duration::from_secs(30);

    let filter = match crate::ipc_events::EventFilter::from_params(params) {
        Ok(f) => f,
        Err(msg) => {
            let err = json!({
                "jsonrpc": "2.0",
                "error": {"code": -32602, "message": msg},
                "id": Value::Null,
            });
            // Guarded like every other push: on Windows the subscribe request's
            // pipe may already be abandoned by the time we reply.
            push_frame(writer, &err);
            return;
        }
    };
    let sub = bus.subscribe(filter);
    let ack = json!({"type": "subscribed", "id": sub.id});
    if !push_frame(writer, &ack) {
        return;
    }

    loop {
        // Report any events shed under backpressure since the last write.
        let dropped = sub.take_dropped();
        if dropped > 0 {
            let marker = json!({"type": "dropped", "count": dropped});
            if !push_frame(writer, &marker) {
                break;
            }
        }
        match sub.rx.recv_timeout(HEARTBEAT) {
            Ok(line) => {
                // `line` already carries its trailing newline.
                if !push_line(writer, line.as_bytes()) {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let hb = json!({"type": "heartbeat"});
                if !push_frame(writer, &hb) {
                    break;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Write one JSON value as a newline-terminated frame to a subscription stream,
/// guarded by [`subscriber_connected`]. Returns `false` when the peer is gone or
/// the write fails - the caller breaks and the `Subscription` drops (RAII).
fn push_frame(writer: &mut Stream, value: &Value) -> bool {
    if !subscriber_connected(writer) {
        return false;
    }
    write_envelope(writer, value)
}

/// Like [`push_frame`] but for bytes that ALREADY carry their trailing newline
/// (a pushed event line, written verbatim). Same Windows liveness guard.
fn push_line(writer: &mut Stream, line: &[u8]) -> bool {
    if !subscriber_connected(writer) {
        return false;
    }
    push_bytes(writer, line)
}

/// Serialize a JSON-RPC value as a newline-terminated frame and send it
/// abort-safely. `true` on success. Shared by the subscription push path and the
/// request/response + rejection writes, so every server-side write to the pipe
/// goes through the same Windows-safe path.
fn write_envelope(writer: &mut Stream, value: &Value) -> bool {
    let mut frame = value.to_string();
    frame.push('\n');
    push_bytes(writer, frame.as_bytes())
}

/// Send raw bytes to the peer, `true` on success.
///
/// The `subscriber_connected` probe narrows but cannot close the disconnect
/// window: the peer can still vanish between the probe and this write, and the
/// rejection / reply writes have no probe at all. On Windows that previously
/// reached interprocess's overlapped `WriteFileEx`, which turns a closed-pipe
/// failure into a `CannotUnwind` -> `process::abort` (STATUS_STACK_BUFFER_OVERRUN
/// 0xC0000409) that no `catch_unwind` can stop. [`pipe_write_all`] keeps the
/// failure a returned `io::Error`. Unix writes to a closed socket return
/// `BrokenPipe` cleanly, so the normal `Write` path is already safe there.
fn push_bytes(writer: &mut Stream, buf: &[u8]) -> bool {
    #[cfg(windows)]
    {
        pipe_write_all(writer, buf, IPC_WRITE_TIMEOUT).is_ok()
    }
    #[cfg(not(windows))]
    {
        if let Err(e) = writer.set_send_timeout(Some(IPC_WRITE_TIMEOUT))
            && e.kind() != std::io::ErrorKind::Unsupported
        {
            return false;
        }
        writer.write_all(buf).is_ok() && writer.flush().is_ok()
    }
}

/// EP-006 follow-up (CP-4): write `buf` to the named pipe with our OWN overlapped
/// `WriteFile`, bypassing interprocess's abort-on-closed-pipe `Write` impl.
///
/// interprocess opens the server pipe with `FILE_FLAG_OVERLAPPED`
/// (`listener/create_instance.rs`) and sends via an alertable `WriteFileEx` +
/// completion routine; once the peer has closed, that FFI completion path panics
/// and interprocess's `CannotUnwind` guard aborts the whole process. We issue a
/// plain overlapped `WriteFile` (NO completion routine, so no Rust runs in an
/// APC) and reap it with `GetOverlappedResult`; a dead pipe then surfaces as
/// `ERROR_NO_DATA` / `ERROR_BROKEN_PIPE`, a normal `io::Error`. A NULL
/// `lpOverlapped` is not an option - MSDN documents it as potentially corrupting
/// on an overlapped handle - hence the per-call manual-reset event.
#[cfg(windows)]
fn pipe_write_all(writer: &Stream, buf: &[u8], timeout: Duration) -> std::io::Result<()> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let Stream::NamedPipe(np) = writer;
    let handle: HANDLE = np.as_handle().as_raw_handle() as _;

    // Manual-reset, initially-unsignaled event backing the OVERLAPPED.
    // SAFETY: default attributes/name; the returned handle is checked below.
    let event: HANDLE = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // Close the event on every return path.
    struct EventGuard(HANDLE);
    impl Drop for EventGuard {
        fn drop(&mut self) {
            // SAFETY: a live event handle we created and have not yet closed.
            unsafe { CloseHandle(self.0) };
        }
    }
    let _event_guard = EventGuard(event);

    let mut remaining = buf;
    while !remaining.is_empty() {
        // SAFETY: a zeroed OVERLAPPED is valid; we attach the event we created.
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = event;
        let want = remaining.len().min(u32::MAX as usize) as u32;
        // SAFETY: `handle` is a valid overlapped pipe handle borrowed from `np`
        // for this call; `ov` carries the event; ptr/len describe `remaining`.
        // No completion routine, so no Rust runs in an FFI callback.
        let started = unsafe {
            WriteFile(
                handle,
                remaining.as_ptr(),
                want,
                std::ptr::null_mut(),
                &mut ov,
            )
        };
        if started == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(err);
            }
            let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            let reap_pending_write = |ov: &OVERLAPPED| {
                let _ = unsafe { CancelIoEx(handle, ov) };
                let mut cancelled_transferred: u32 = 0;
                let _ = unsafe { GetOverlappedResult(handle, ov, &mut cancelled_transferred, 1) };
            };
            match unsafe { WaitForSingleObject(event, timeout_ms) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    reap_pending_write(&ov);
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "named-pipe write timed out",
                    ));
                }
                WAIT_FAILED => {
                    let err = std::io::Error::last_os_error();
                    reap_pending_write(&ov);
                    return Err(err);
                }
                other => {
                    reap_pending_write(&ov);
                    return Err(std::io::Error::other(format!(
                        "unexpected WaitForSingleObject result {other}"
                    )));
                }
            }
        }
        let mut transferred: u32 = 0;
        // SAFETY: same handle + overlapped as the WriteFile above; `transferred`
        // is a valid out-pointer; bWait = TRUE blocks until the write settles.
        let reaped = unsafe { GetOverlappedResult(handle, &ov, &mut transferred, 1) };
        if reaped == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if transferred == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "named-pipe write transferred 0 bytes",
            ));
        }
        remaining = &remaining[transferred as usize..];
    }
    Ok(())
}

#[cfg(windows)]
fn pipe_read_some(reader: &Stream, buf: &mut [u8]) -> std::io::Result<usize> {
    pipe_read_some_with_timeout(reader, buf, IPC_IDLE_TIMEOUT)
}

#[cfg(windows)]
fn pipe_read_some_with_timeout(
    reader: &Stream,
    buf: &mut [u8],
    timeout: Duration,
) -> std::io::Result<usize> {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, HANDLE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    let Stream::NamedPipe(np) = reader;
    let handle: HANDLE = np.as_handle().as_raw_handle() as _;

    let event: HANDLE = unsafe { CreateEventW(std::ptr::null(), 1, 0, std::ptr::null()) };
    if event.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    struct EventGuard(HANDLE);
    impl Drop for EventGuard {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }
    let _event_guard = EventGuard(event);

    let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
    ov.hEvent = event;
    let want = buf.len().min(u32::MAX as usize) as u32;
    let started = unsafe {
        ReadFile(
            handle,
            buf.as_mut_ptr(),
            want,
            std::ptr::null_mut(),
            &mut ov,
        )
    };
    let pending = if started == 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
            return Err(err);
        }
        true
    } else {
        false
    };

    if pending {
        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        let reap_pending_read = |ov: &OVERLAPPED| {
            let _ = unsafe { CancelIoEx(handle, ov) };
            let mut cancelled_transferred: u32 = 0;
            let _ = unsafe { GetOverlappedResult(handle, ov, &mut cancelled_transferred, 1) };
        };
        match unsafe { WaitForSingleObject(event, timeout_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                // The OVERLAPPED lives on this stack frame, so after marking
                // the read for cancellation we still reap completion before
                // returning. Named-pipe cancellation completes as an ordinary
                // overlapped result (often ERROR_OPERATION_ABORTED).
                reap_pending_read(&ov);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "named-pipe read timed out",
                ));
            }
            WAIT_FAILED => {
                let err = std::io::Error::last_os_error();
                reap_pending_read(&ov);
                return Err(err);
            }
            other => {
                reap_pending_read(&ov);
                return Err(std::io::Error::other(format!(
                    "unexpected WaitForSingleObject result {other}"
                )));
            }
        }
    }

    let mut transferred: u32 = 0;
    let reaped = unsafe { GetOverlappedResult(handle, &ov, &mut transferred, 1) };
    if reaped == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(transferred as usize)
}

#[cfg(windows)]
fn closed_pipe_error(err: &std::io::Error) -> bool {
    use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_PIPE_NOT_CONNECTED};

    matches!(
        err.raw_os_error(),
        Some(code) if code == ERROR_BROKEN_PIPE as i32 || code == ERROR_PIPE_NOT_CONNECTED as i32
    )
}

/// EP-006 US-013: Unix path - a no-op `true`. A write to a closed Unix socket
/// returns `Err(BrokenPipe)` cleanly (Rust ignores SIGPIPE), which the caller
/// already handles, so no pre-probe is needed.
#[cfg(not(windows))]
fn subscriber_connected(_writer: &Stream) -> bool {
    true
}

/// EP-006 US-013: Windows path - probe the named pipe's liveness BEFORE a push so
/// a disconnected subscriber is dropped cleanly instead of aborting the process.
///
/// `PeekNamedPipe` with a zero-length buffer and all-null out-params is the
/// documented "just check the pipe" call: it consumes no data and issues no
/// overlapped I/O, so it can never trip interprocess's `CannotUnwind` abort
/// (unlike the overlapped `WriteFileEx` a write would issue). A broken/closed
/// pipe returns 0 (e.g. `ERROR_BROKEN_PIPE`); any failure is treated as "gone".
///
/// A vanishingly small race remains - the peer can close between this probe and
/// the subsequent write - which is the residual the Windows runtime smoke
/// (US-014) validates; in practice a `watch` client's events are seconds apart
/// and the 30 s heartbeat is the dominant write, so the window is negligible.
#[cfg(windows)]
fn subscriber_connected(writer: &Stream) -> bool {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    // interprocess 2.4's generic local_socket `Stream` forwards `Read`/`Write`
    // but NOT `AsHandle`; on Windows its sole (public, non-`#[non_exhaustive]`)
    // variant wraps the platform named-pipe stream that DOES expose the OS
    // handle, so match the variant to reach it. The match is exhaustive on
    // Windows (one variant); a future interprocess variant would fail to compile
    // here, forcing a review rather than silently mis-probing.
    let Stream::NamedPipe(np) = writer;
    let handle = np.as_handle().as_raw_handle();
    // SAFETY: `handle` is a valid, open named-pipe handle borrowed from `np` for
    // the duration of this call. Every optional out-param is null and the buffer
    // length is 0 - the documented connection-status-only form of PeekNamedPipe,
    // which consumes nothing and issues no overlapped I/O.
    let connected = unsafe {
        PeekNamedPipe(
            handle as _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    connected != 0
}

fn dispatch_to_gpui(
    request_tx: &mpsc::SyncSender<IpcRequest>,
    method: String,
    params: Value,
    id: Value,
    caller_pid: Option<i64>,
) -> Value {
    let (resp_tx, resp_rx) = mpsc::channel();
    // U-053: shared cancel flag - set if we time out below so the GPUI
    // consumer skips a request the client already gave up on.
    let cancelled = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicBool::new(false));
    let ipc_req = IpcRequest {
        method: method.clone(),
        params,
        _id: id.clone(),
        response_tx: resp_tx,
        cancelled: Arc::clone(&cancelled),
        started: Arc::clone(&started),
        caller_pid,
    };

    match request_tx.try_send(ipc_req) {
        Ok(()) => {}
        Err(mpsc::TrySendError::Full(_)) => {
            return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "Paneflow is busy; retry shortly"}, "id": id});
        }
        Err(mpsc::TrySendError::Disconnected(_)) => {
            return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "App shutting down"}, "id": id});
        }
    }

    await_or_cancel(&resp_rx, &cancelled, &started, Duration::from_secs(5), id)
}

/// Wait for the GPUI handler's response. If the request is still queued after
/// `timeout`, set `cancelled` so the GPUI consumer skips it. Once the handler
/// has started, wait for the real response instead of telling the client to
/// retry a mutation that may still complete.
fn await_or_cancel(
    resp_rx: &mpsc::Receiver<Value>,
    cancelled: &AtomicBool,
    started: &AtomicBool,
    timeout: Duration,
    id: Value,
) -> Value {
    let queued_at = Instant::now();
    loop {
        let wait_for = if started.load(Ordering::Acquire) {
            Duration::from_millis(50)
        } else {
            let Some(remaining) = timeout.checked_sub(queued_at.elapsed()) else {
                cancelled.store(true, Ordering::SeqCst);
                return json!({"jsonrpc": "2.0", "error": {"code": -32002, "message": "Request dispatch timeout"}, "id": id});
            };
            remaining.min(Duration::from_millis(50))
        };

        match resp_rx.recv_timeout(wait_for) {
            Ok(result) => return crate::app::ipc_handler::promote_response(result, id),
            Err(mpsc::RecvTimeoutError::Timeout) if started.load(Ordering::Acquire) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if queued_at.elapsed() >= timeout {
                    cancelled.store(true, Ordering::SeqCst);
                    return json!({"jsonrpc": "2.0", "error": {"code": -32002, "message": "Request dispatch timeout"}, "id": id});
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "App shutting down"}, "id": id});
            }
        }
    }
}

fn socket_path_spec() -> Option<crate::runtime_paths::IpcSocketPath> {
    crate::runtime_paths::socket_path_spec()
}

// ---------------------------------------------------------------------------
// US-010: peer-UID enforcement
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod auth {
    //! Peer-UID enforcement on the IPC server.
    //!
    //! Splits cleanly so each layer is testable in isolation:
    //!
    //! - [`authorize`]: pure policy decision - given a server UID and a
    //!   peer UID, allow or deny. No I/O, exhaustively unit-tested
    //!   (matching pair → allow, mismatched pair → deny).
    //! - [`server_uid`]: thin wrapper over `getuid(2)`.
    //! - [`check_peer`]: glue that runs `Stream::peer_creds()` (provided
    //!   by interprocess 2.4 - `getsockopt(SO_PEERCRED)` on Linux,
    //!   `LOCAL_PEERCRED` on macOS, `xucred` on the BSDs) and feeds
    //!   the result into `authorize`.
    //!
    //! [`check_peer`] returns an [`AuthOutcome`] the caller turns into
    //! the JSON-RPC envelope (or just keeps serving on
    //! `DegradedFallback`). The split keeps the policy fully covered
    //! by deterministic tests; the live-syscall integration is
    //! exercised by paneflow itself on every connection.

    use super::Stream;
    use interprocess::local_socket::prelude::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum AuthOutcome {
        /// Peer UID matches server UID - proceed to dispatch.
        Allow,
        /// Peer UID query succeeded and the value did NOT match the
        /// server's UID. Caller emits the JSON-RPC EPERM envelope.
        Deny { server_uid: u32, peer_uid: u32 },
        /// Peer UID could not be queried (very old kernel / exotic
        /// Unix without an `euid` field in `peer_creds()`). AC6:
        /// fall back to the perms-0600 file-mode line of defence and
        /// continue serving. The warn log fires inside [`check_peer`]
        /// so the fallback isn't silent.
        DegradedFallback,
    }

    /// Pure-function policy. Equality of effective UIDs is the
    /// allowlist.
    pub(super) fn authorize(server_uid: u32, peer_uid: u32) -> AuthOutcome {
        if server_uid == peer_uid {
            AuthOutcome::Allow
        } else {
            AuthOutcome::Deny {
                server_uid,
                peer_uid,
            }
        }
    }

    /// Resolve the running process's effective UID via `geteuid(2)`.
    ///
    /// `peer_creds().euid()` returns the peer's *effective* UID; we
    /// must compare against ours symmetrically. Calling `getuid()`
    /// (real UID) here would diverge from `geteuid()` under any
    /// privilege-separation wrapper (`sudo`, setuid, polkit-helped
    /// child) and either falsely accept or falsely reject a peer that
    /// shares one but not the other.
    pub(super) fn server_uid() -> u32 {
        // libc::uid_t is u32 on every supported target; the cast is a
        // no-op there but stays explicit for cross-target clarity.
        unsafe { libc::geteuid() as u32 }
    }

    /// Run the peer-credential query against the connected stream and
    /// translate the outcome. Defers the kernel-call mechanics to
    /// `interprocess::local_socket::Stream::peer_creds()` (`SO_PEERCRED`
    /// on Linux, `LOCAL_PEERCRED` on macOS, `xucred` on the BSDs);
    /// upstream owns those per-OS quirks so paneflow doesn't
    /// duplicate `getsockopt` boilerplate per target.
    pub(super) fn check_peer(stream: &Stream) -> AuthOutcome {
        let server = server_uid();
        match stream.peer_creds() {
            Ok(creds) => match creds.euid() {
                Some(peer) => authorize(server, peer),
                None => {
                    // `peer_creds()` succeeded but the platform doesn't
                    // expose an effective UID (NetBSD ucred lacks
                    // euid, for example). Same fallback as the Err
                    // branch - perms-0600 stays as the line of
                    // defence.
                    log::warn!(
                        "IPC: peer-cred query returned no euid on this OS; \
                         falling back to perms-0600 only"
                    );
                    AuthOutcome::DegradedFallback
                }
            },
            Err(e) => {
                log::warn!("IPC: peer-cred query failed ({e}); falling back to perms-0600 only");
                AuthOutcome::DegradedFallback
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn authorize_accepts_matching_uid() {
            assert_eq!(authorize(1000, 1000), AuthOutcome::Allow);
            assert_eq!(authorize(0, 0), AuthOutcome::Allow);
        }

        #[test]
        fn authorize_rejects_mismatched_uid() {
            assert_eq!(
                authorize(1000, 1001),
                AuthOutcome::Deny {
                    server_uid: 1000,
                    peer_uid: 1001,
                }
            );
            assert_eq!(
                authorize(1000, 0),
                AuthOutcome::Deny {
                    server_uid: 1000,
                    peer_uid: 0,
                }
            );
        }

        /// `geteuid(2)` must return the same value on two successive
        /// calls - the kernel doesn't change a process's effective UID
        /// without an explicit `setuid(2)` / `seteuid(2)` call. Stable
        /// across calls is the property the auth path actually relies
        /// on (we capture the server euid once and compare every
        /// incoming peer euid against it).
        #[test]
        fn server_uid_is_stable() {
            let a = server_uid();
            let b = server_uid();
            assert_eq!(a, b, "geteuid must be stable across calls");
        }

        /// Symmetric to `authorize_accepts_matching_uid` - root running
        /// the server is an explicit policy choice, not an accidental
        /// bypass: any non-root peer is denied even when the server is
        /// uid 0. The matching-UID accept at `(0, 0)` is the only
        /// root-to-root path; that case is intentional (a privileged
        /// IPC client speaking to a privileged paneflow run by the
        /// same operator).
        #[test]
        fn authorize_root_server_rejects_non_root_peer() {
            assert!(matches!(
                authorize(0, 1000),
                AuthOutcome::Deny {
                    server_uid: 0,
                    peer_uid: 1000
                }
            ));
        }
    }
}

#[cfg(test)]
mod framing_tests {
    use super::{LineRead, MAX_REQUEST_LEN, read_capped_line};
    use std::io::Cursor;

    #[test]
    fn capped_line_rejects_oversized_unterminated() {
        // US-022 negative test: a line that reaches the cap without a newline
        // is reported TooLong, never accumulated past the bound.
        let huge = vec![b'x'; MAX_REQUEST_LEN as usize + 64];
        let mut cur = Cursor::new(huge);
        let mut line = String::new();
        assert_eq!(
            read_capped_line(&mut cur, &mut line).unwrap(),
            LineRead::TooLong
        );
        assert!(line.len() as u64 <= MAX_REQUEST_LEN, "buffer stays bounded");
    }

    #[test]
    fn capped_line_accepts_normal_then_eof() {
        let mut cur = Cursor::new(b"{\"jsonrpc\":\"2.0\"}\n".to_vec());
        let mut line = String::new();
        assert_eq!(
            read_capped_line(&mut cur, &mut line).unwrap(),
            LineRead::Got
        );
        assert_eq!(line, "{\"jsonrpc\":\"2.0\"}\n");
        assert_eq!(
            read_capped_line(&mut cur, &mut line).unwrap(),
            LineRead::Eof
        );
    }

    #[test]
    fn capped_line_accepts_exactly_at_cap_with_newline() {
        // Boundary: a line of exactly MAX_REQUEST_LEN bytes whose final byte
        // is the newline is accepted (not a truncation).
        let mut body = vec![b'a'; MAX_REQUEST_LEN as usize - 1];
        body.push(b'\n');
        let mut cur = Cursor::new(body);
        let mut line = String::new();
        assert_eq!(
            read_capped_line(&mut cur, &mut line).unwrap(),
            LineRead::Got
        );
    }

    #[cfg(unix)]
    #[test]
    fn bind_socket_refuses_to_remove_regular_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("paneflow.sock");
        std::fs::write(&path, b"do not delete").expect("write guard file");

        assert!(
            super::bind_socket(&path).is_none(),
            "regular files at the socket path must not be reclaimed"
        );
        assert_eq!(
            std::fs::read(&path).expect("regular file still exists"),
            b"do not delete"
        );
    }

    #[cfg(unix)]
    #[test]
    fn unowned_socket_parent_rejects_world_writable_without_sticky_bit() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777))
            .expect("chmod tempdir");
        assert!(!super::unowned_socket_parent_is_safe(dir.path()));

        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o1777))
            .expect("chmod sticky tempdir");
        assert!(super::unowned_socket_parent_is_safe(dir.path()));
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{IpcRequest, await_or_cancel, dispatch_to_gpui};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    fn test_ipc_request() -> IpcRequest {
        let (response_tx, _response_rx) = mpsc::channel();
        IpcRequest {
            method: "surface.read".to_string(),
            params: json!({}),
            _id: json!(1),
            response_tx,
            cancelled: Arc::new(AtomicBool::new(false)),
            started: Arc::new(AtomicBool::new(false)),
            caller_pid: None,
        }
    }

    #[test]
    fn dispatch_to_gpui_returns_overload_when_request_queue_full() {
        let (tx, _rx) = mpsc::sync_channel(1);
        tx.try_send(test_ipc_request()).unwrap();

        let resp = dispatch_to_gpui(
            &tx,
            "surface.read".to_string(),
            json!({ "surface_id": 1 }),
            json!("req-overload"),
            None,
        );

        assert_eq!(resp["error"]["code"], -32000);
        assert_eq!(resp["error"]["message"], "Paneflow is busy; retry shortly");
        assert_eq!(resp["id"], "req-overload");
    }

    #[test]
    fn dispatch_to_gpui_returns_shutdown_when_receiver_dropped() {
        let (tx, rx) = mpsc::sync_channel(1);
        drop(rx);

        let resp = dispatch_to_gpui(
            &tx,
            "surface.read".to_string(),
            json!({ "surface_id": 1 }),
            json!("req-closed"),
            None,
        );

        assert_eq!(resp["error"]["code"], -32000);
        assert_eq!(resp["error"]["message"], "App shutting down");
        assert_eq!(resp["id"], "req-closed");
    }

    #[test]
    fn await_or_cancel_sets_flag_and_errors_on_timeout() {
        // U-053: when the GPUI handler is not started within the deadline,
        // await_or_cancel must (a) return a -32002 timeout envelope to the
        // client AND (b) set the shared cancel flag so the GPUI consumer skips
        // the not-yet-run handler - preventing a duplicate non-idempotent
        // mutation on the client's retry. _tx is kept alive so we exercise the
        // Timeout path (not Disconnected); a short deadline keeps the test fast.
        let (_tx, rx) = mpsc::channel::<serde_json::Value>();
        let cancelled = AtomicBool::new(false);
        let started = AtomicBool::new(false);
        let resp = await_or_cancel(
            &rx,
            &cancelled,
            &started,
            Duration::from_millis(20),
            json!(7),
        );

        assert!(
            cancelled.load(Ordering::Acquire),
            "timeout must set the cancel flag so the GPUI side skips the request"
        );
        assert_eq!(resp["error"]["code"], -32002);
        assert_eq!(resp["id"], 7);
    }

    #[test]
    fn await_or_cancel_waits_for_started_handler_instead_of_cancelling() {
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(true));
        let send_started = Arc::clone(&started);
        std::thread::spawn(move || {
            assert!(send_started.load(Ordering::Acquire));
            std::thread::sleep(Duration::from_millis(40));
            tx.send(json!({"status": "ok"})).unwrap();
        });

        let resp = await_or_cancel(
            &rx,
            &cancelled,
            &started,
            Duration::from_millis(5),
            json!(9),
        );

        assert!(
            !cancelled.load(Ordering::Acquire),
            "started handlers must not be cancelled behind the client"
        );
        assert_eq!(resp["result"]["status"], "ok");
        assert_eq!(resp["id"], 9);
    }

    #[test]
    fn await_or_cancel_passes_through_response_without_cancelling() {
        // The happy path: a response arrives before the deadline → no cancel,
        // result promoted under `result` (no `_jsonrpc_error` sentinel here).
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        tx.send(json!({"status": "ok"})).unwrap();
        let cancelled = AtomicBool::new(false);
        let started = AtomicBool::new(false);
        let resp = await_or_cancel(&rx, &cancelled, &started, Duration::from_secs(5), json!(3));

        assert!(
            !cancelled.load(Ordering::Acquire),
            "a timely response must not set the cancel flag"
        );
        assert_eq!(resp["result"]["status"], "ok");
        assert_eq!(resp["id"], 3);
    }
}

/// EP-006 US-013 / US-014 (runbook scenario CP-4): the headline Windows
/// invariant, as an automated regression test instead of a manual-only smoke.
///
/// A subscriber that disconnects (a `paneflow watch` Ctrl-C) MUST be evicted by
/// the `PeekNamedPipe` liveness probe BEFORE the next push, so the server never
/// issues an overlapped write to a closed named pipe - interprocess converts
/// that into a `CannotUnwind` -> `process::abort` (STATUS_STACK_BUFFER_OVERRUN
/// 0xC0000409) that no `catch_unwind` can stop. These tests drive the real
/// named-pipe push path (`push_frame` / `push_line` / `subscriber_connected`)
/// that the Linux build host can only compile-check, so the eviction has a
/// deterministic guard, not just the `docs/WINDOWS-SMOKE-TEST.md` checklist.
///
/// Windows-only: on Unix `subscriber_connected` is a no-op `true` (a write to a
/// closed socket returns `BrokenPipe` cleanly, no probe needed).
#[cfg(all(test, windows))]
mod windows_pipe_tests {
    use super::{
        LineRead, pipe_read_some_with_timeout, pipe_write_all, push_frame, push_line,
        read_request_line, subscriber_connected,
    };
    use interprocess::local_socket::{
        GenericFilePath, Listener, ListenerOptions, Stream, prelude::*,
    };
    use serde_json::json;
    use std::io::Read;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{Duration, Instant};

    /// A process-unique pipe path so parallel test threads never collide and a
    /// stray prior instance can't shadow this one. Distinct `paneflow-test-`
    /// prefix keeps it clear of a live `\\.\pipe\paneflow{,-dev}` server.
    fn unique_pipe_path() -> std::path::PathBuf {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        std::path::PathBuf::from(format!(
            r"\\.\pipe\paneflow-test-{}-{}",
            std::process::id(),
            seq
        ))
    }

    #[test]
    fn named_pipe_security_descriptor_deserializes() {
        super::windows_named_pipe_security_descriptor().expect("IPC named-pipe SDDL must be valid");
    }

    /// Bind a listener and return a live (server, client) named-pipe pair plus
    /// the listener (kept alive by the caller). Mirrors `bind_socket`'s name +
    /// `ListenerOptions` setup so the test drives the same construction
    /// production does. The client connects on its own thread to dodge the
    /// single-thread connect/accept deadlock.
    fn connected_pair() -> (Stream, Stream, Listener) {
        let path = unique_pipe_path();
        let listener = {
            // `to_fs_name` consumes its receiver, so call it on a borrowed
            // `&Path` (like `bind_socket` does) to leave `path` owned and
            // movable into the client thread below.
            let name = path
                .as_path()
                .to_fs_name::<GenericFilePath>()
                .expect("build pipe name");
            ListenerOptions::new()
                .name(name)
                .create_sync()
                .expect("bind test listener")
        };
        let client_thread = std::thread::spawn(move || {
            let name = path
                .as_path()
                .to_fs_name::<GenericFilePath>()
                .expect("build client pipe name");
            Stream::connect(name).expect("client connect")
        });
        let server = listener.accept().expect("accept client");
        let client = client_thread.join().expect("join client thread");
        (server, client, listener)
    }

    /// Poll the liveness probe until it reports the peer gone, bounded by
    /// `timeout`. A closed named pipe surfaces as `ERROR_BROKEN_PIPE` from
    /// `PeekNamedPipe` (probe returns 0), but the OS can take a beat to settle
    /// after the peer's handle closes, so we poll rather than assume instant.
    fn wait_until_disconnected(server: &Stream, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if !subscriber_connected(server) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn live_subscriber_reads_as_connected_and_receives_push() {
        let (mut server, client, _listener) = connected_pair();
        assert!(
            subscriber_connected(&server),
            "a live named-pipe peer must probe as connected"
        );
        assert!(
            push_frame(&mut server, &json!({"type": "subscribed", "id": 1})),
            "push to a live subscriber succeeds"
        );

        // End-to-end: the pushed line actually reaches the client. Read on a
        // worker thread with a channel deadline - Windows named pipes reject
        // `set_recv_timeout` (see the `handle_connection` note), so a channel
        // recv timeout is the only hang-proof bound on a blocking read.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut client = client;
            let mut buf = [0u8; 256];
            let n = client.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
        });
        let line = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("client receives the pushed frame within 2s");
        assert!(
            line.contains("subscribed"),
            "client received the frame verbatim, got: {line:?}"
        );
    }

    #[test]
    fn disconnected_subscriber_is_evicted_without_process_abort() {
        let (mut server, client, _listener) = connected_pair();
        assert!(subscriber_connected(&server), "connected before drop");

        // Simulate `paneflow watch` Ctrl-C: the subscriber closes its end.
        drop(client);

        assert!(
            wait_until_disconnected(&server, Duration::from_secs(2)),
            "PeekNamedPipe must report the closed peer (the CP-4 eviction gate)"
        );

        // The EP-006 headline invariant: a push to the now-dead subscriber is
        // refused by the guard and returns false - it must NOT reach an
        // overlapped write to the closed pipe, which aborts the whole process.
        // Reaching these asserts at all proves no abort happened; `false` proves
        // the guard short-circuited the dangerous write. (Safe by construction:
        // both helpers re-probe internally, and we only get here after the probe
        // already read `false`, so neither ever attempts the write.)
        assert!(
            !push_frame(&mut server, &json!({"type": "heartbeat"})),
            "push_frame evicts a disconnected subscriber instead of writing"
        );
        assert!(
            !push_line(&mut server, b"{\"type\":\"ai.stop\"}\n"),
            "push_line evicts a disconnected subscriber instead of writing"
        );
    }

    #[test]
    fn raw_write_to_closed_pipe_returns_err_without_process_abort() {
        let (server, client, _listener) = connected_pair();
        // Peer closes (a `paneflow watch` Ctrl-C) and the close settles.
        drop(client);
        assert!(
            wait_until_disconnected(&server, Duration::from_secs(2)),
            "peer close must settle before the write"
        );

        // Hit the raw overlapped write DIRECTLY, bypassing the
        // `subscriber_connected` fast-path the eviction test relies on. This is
        // the residual-race write (probe passed, peer then vanished): before
        // CP-4's managed WriteFile it aborted the whole process
        // (STATUS_STACK_BUFFER_OVERRUN 0xC0000409); now it must return a plain
        // `Err`. Reaching the assert at all proves the process did not abort.
        let r = pipe_write_all(
            &server,
            b"{\"type\":\"heartbeat\"}\n",
            Duration::from_secs(1),
        );
        assert!(
            r.is_err(),
            "overlapped write to a closed pipe returns Err, never aborts"
        );
    }

    #[test]
    fn raw_read_from_closed_pipe_returns_eof_without_process_abort() {
        let (mut server, client, _listener) = connected_pair();
        // A peer can connect and close before sending a JSON-RPC frame. Before
        // the managed ReadFile path, interprocess's ReadFileEx guard could
        // abort the whole process on that immediate ERROR_BROKEN_PIPE.
        drop(client);
        assert!(
            wait_until_disconnected(&server, Duration::from_secs(2)),
            "peer close must settle before the read"
        );

        let mut line = String::new();
        assert_eq!(
            read_request_line(&mut server, &mut line).expect("closed pipe maps to EOF"),
            LineRead::Eof
        );
        assert!(line.is_empty());
    }

    #[test]
    fn muted_named_pipe_read_times_out_without_pinning_handler() {
        let (server, _client, _listener) = connected_pair();
        let mut buf = [0u8; 16];
        let started = Instant::now();

        let err = pipe_read_some_with_timeout(&server, &mut buf, Duration::from_millis(50))
            .expect_err("mute peer should hit the explicit read timeout");

        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout path must release promptly instead of pinning the handler slot"
        );
    }
}
