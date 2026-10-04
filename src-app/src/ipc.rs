use std::io::{BufRead, BufReader, Read, Write};
use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicUsize, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

use interprocess::TryClone;
#[cfg(unix)]
use interprocess::local_socket::ListenerNonblockingMode;
use interprocess::local_socket::{GenericFilePath, Listener, ListenerOptions, Stream, prelude::*};
#[cfg(windows)]
use interprocess::os::windows::{
    local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
};
use paneflow_host::server::ConnectionGuard;
use serde_json::{Value, json};

pub struct IpcRequest {
    pub method: String,
    pub params: Value,
    pub response_tx: mpsc::Sender<Value>,
    pub state: Arc<RequestState>,
    pub caller_pid: Option<i64>,
}

const REQUEST_QUEUED: u8 = 0;
const REQUEST_STARTED: u8 = 1;
const REQUEST_CANCELLED: u8 = 2;

#[derive(Debug, Default)]
pub struct RequestState(AtomicU8);

impl RequestState {
    fn transition(&self, to: u8) -> bool {
        self.0
            .compare_exchange(REQUEST_QUEUED, to, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub fn try_start(&self) -> bool {
        self.transition(REQUEST_STARTED)
    }

    pub fn try_cancel(&self) -> bool {
        self.transition(REQUEST_CANCELLED)
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire) == REQUEST_CANCELLED
    }

    fn is_started(&self) -> bool {
        self.0.load(Ordering::Acquire) == REQUEST_STARTED
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpcState {
    Online,
    Disabled,
}

const IPC_STATE_ONLINE: u8 = 0;
const IPC_STATE_DISABLED: u8 = 1;

use crate::limits::MAX_REQUEST_LEN;

const MAX_REQUEST_CONNECTIONS: usize = 16;

const MAX_SUBSCRIPTION_CONNECTIONS: usize = 16;

const MAX_CONCURRENT_CONNECTIONS: usize = MAX_REQUEST_CONNECTIONS + MAX_SUBSCRIPTION_CONNECTIONS;

pub(crate) const IPC_REQUEST_QUEUE_CAPACITY: usize = 256;

pub(crate) const IPC_DRAIN_MAX_PER_TICK: usize = 64;

pub(crate) const IPC_DRAIN_MAX_DEQUEUES_PER_TICK: usize = IPC_DRAIN_MAX_PER_TICK * 2;

const IPC_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

const IPC_WRITE_TIMEOUT: Duration = Duration::from_secs(5);

const IPC_DISPATCH_TIMEOUT: Duration = Duration::from_secs(5);

const IPC_STARTED_REPLY_TIMEOUT: Duration = Duration::from_secs(30);

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

pub fn start_server() -> (
    smol::channel::Receiver<IpcRequest>,
    IpcStatus,
    Arc<crate::ipc_events::EventBus>,
) {
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
            "ipc.orchestration_enabled is ON; any same-UID process can create panes with commands, prompts, or env"
        );
    }

    let (tx, rx) = smol::channel::bounded(IPC_REQUEST_QUEUE_CAPACITY);
    let status = IpcStatus::online();
    let thread_status = status.clone();

    let event_bus = crate::ipc_events::EventBus::new();
    let thread_event_bus = Arc::clone(&event_bus);

    if !multiple_instances_allowed(std::env::var("PANEFLOW_ALLOW_MULTIPLE").ok().as_deref())
        && let Some(socket_spec) = socket_path_spec()
        && let Some(info) = singleton_refusal(
            claim_instance(&socket_spec),
            detect_existing_instance(&socket_spec.path),
        )
    {
        eprintln!(
            "paneflow: another Paneflow instance is already running on {}.\n\
             Existing instance: {}\n\
             Close the open window first, or set PANEFLOW_ALLOW_MULTIPLE=1 to override.",
            socket_spec.path.display(),
            info
        );
        log::error!(
            "singleton guard: refusing to start; existing instance on {} ({})",
            socket_spec.path.display(),
            info
        );
        std::process::exit(1);
    }

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
            let socket_path = socket_spec.path.clone();

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
            remember_own_socket(&socket_path, our_ino);
            #[cfg(unix)]
            let mut last_health_check = std::time::Instant::now();
            #[cfg(unix)]
            let mut listener = listener;
            #[cfg(not(unix))]
            let listener = listener;

            #[cfg(unix)]
            listener
                .set_nonblocking(ListenerNonblockingMode::Accept)
                .ok();

            let active_connections = Arc::new(AtomicUsize::new(0));
            let active_subscriptions = Arc::new(AtomicUsize::new(0));

            loop {
                match listener.accept() {
                    Ok(stream) => {
                        let stream = match blocking_connection(stream) {
                            Ok(stream) => stream,
                            Err(e) => {
                                log::warn!(
                                    "paneflow: dropping an IPC connection that cannot be made blocking: {e}"
                                );
                                continue;
                            }
                        };
                        let Some(guard) = ConnectionGuard::try_acquire(
                            Arc::clone(&active_connections),
                            MAX_CONCURRENT_CONNECTIONS,
                        ) else {
                            reject_overloaded(stream);
                            continue;
                        };
                        let tx = tx.clone();
                        let bus = Arc::clone(&thread_event_bus);
                        let subscriptions = Arc::clone(&active_subscriptions);
                        if let Err(e) = std::thread::Builder::new()
                            .name("paneflow-ipc-conn".into())
                            .spawn(move || {
                                let _guard = guard;
                                handle_connection(stream, tx, bus, subscriptions);
                            })
                        {
                            log::warn!(
                                "IPC: handler thread spawn failed ({e}); dropping this \
                                 connection. Check `ulimit -u` / container thread limits."
                            );
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => {
                        thread_status.disable();
                        log::error!("IPC accept error: {e}");
                        break;
                    }
                }

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
                                remember_own_socket(&socket_path, our_ino);
                            }
                            None => {
                                thread_status.disable();
                                return;
                            }
                        }
                    }
                }
            }

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
    }

    (rx, status, event_bus)
}

fn multiple_instances_allowed(value: Option<&str>) -> bool {
    value == Some("1")
}

fn request_params(request: &Value) -> Result<Value, &'static str> {
    match request.get("params") {
        None | Some(Value::Null) => Ok(json!({})),
        Some(params @ Value::Object(_)) => Ok(params.clone()),
        Some(_) => Err("'params' must be an object"),
    }
}

fn subscription_filter(
    params: &Value,
    id: &Value,
) -> Result<crate::ipc_events::EventFilter, Value> {
    crate::ipc_events::EventFilter::from_params(params).map_err(|message| {
        json!({
            "jsonrpc": "2.0",
            "error": {"code": -32602, "message": message},
            "id": id,
        })
    })
}

fn bind_socket(socket_path: &std::path::Path) -> Option<Listener> {
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

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
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
fn prepare_socket_parent(socket_spec: &paneflow_home::IpcEndpoint) -> bool {
    let Some(parent) = socket_spec.path.parent() else {
        return true;
    };

    if socket_spec.owned_parent {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::error!(
                "IPC server: failed to create socket parent {} ({e}); refusing to serve",
                parent.display()
            );
            return false;
        }
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

#[cfg(unix)]
fn socket_inode(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.ino())
}

const IDENTIFY_ATTEMPTS: usize = if cfg!(windows) { 3 } else { 1 };

const IDENTIFY_REPLY_TIMEOUT: Duration = Duration::from_millis(300);

const INSTANCE_STARTING: &str = "another instance holds the startup lock and is still starting";

#[derive(Debug)]
#[cfg_attr(
    not(unix),
    allow(
        dead_code,
        reason = "only the Unix instance lock claims or finds a held lock"
    )
)]
enum InstanceClaim {
    Claimed,
    HeldElsewhere,
    Unavailable,
}

fn singleton_refusal(claim: InstanceClaim, existing: Option<String>) -> Option<String> {
    match (existing, claim) {
        (Some(info), _) => Some(info),
        (None, InstanceClaim::HeldElsewhere) => Some(INSTANCE_STARTING.to_string()),
        (None, InstanceClaim::Claimed | InstanceClaim::Unavailable) => None,
    }
}

#[cfg(unix)]
fn claim_instance(socket_spec: &paneflow_home::IpcEndpoint) -> InstanceClaim {
    static HELD: std::sync::OnceLock<std::fs::File> = std::sync::OnceLock::new();
    if !prepare_socket_parent(socket_spec) {
        return InstanceClaim::Unavailable;
    }
    match try_claim_instance_lock(&instance_lock_path(&socket_spec.path)) {
        Ok(Some(lock)) => {
            let _ = HELD.set(lock);
            InstanceClaim::Claimed
        }
        Ok(None) => InstanceClaim::HeldElsewhere,
        Err(error) => {
            log::warn!(
                "singleton guard: cannot take the instance lock next to {} ({error})",
                socket_spec.path.display()
            );
            InstanceClaim::Unavailable
        }
    }
}

#[cfg(not(unix))]
fn claim_instance(_socket_spec: &paneflow_home::IpcEndpoint) -> InstanceClaim {
    InstanceClaim::Unavailable
}

#[cfg(unix)]
fn instance_lock_path(socket_path: &std::path::Path) -> std::path::PathBuf {
    let mut path = socket_path.as_os_str().to_os_string();
    path.push(".lock");
    std::path::PathBuf::from(path)
}

#[cfg(unix)]
fn try_claim_instance_lock(path: &std::path::Path) -> std::io::Result<Option<std::fs::File>> {
    use std::os::unix::fs::OpenOptionsExt as _;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(unix)]
fn own_socket() -> &'static std::sync::Mutex<Option<(std::path::PathBuf, u64)>> {
    static OWN: std::sync::OnceLock<std::sync::Mutex<Option<(std::path::PathBuf, u64)>>> =
        std::sync::OnceLock::new();
    OWN.get_or_init(Default::default)
}

#[cfg(unix)]
fn remember_own_socket(path: &std::path::Path, inode: u64) {
    *own_socket()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((path.to_path_buf(), inode));
}

pub(crate) fn release_own_socket() {
    #[cfg(unix)]
    {
        let held = own_socket()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some((path, inode)) = held
            && socket_inode(&path) == Some(inode)
        {
            let _ = remove_socket_file_if_socket(&path, "quit cleanup");
        }
    }
}

fn detect_existing_instance(socket_path: &std::path::Path) -> Option<String> {
    #[cfg(unix)]
    if !socket_path.exists() {
        return None;
    }

    let name = socket_path.to_fs_name::<GenericFilePath>().ok()?;

    for attempt in 0..IDENTIFY_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(70));
        }

        let mut stream = match Stream::connect(name.clone()) {
            Ok(stream) => stream,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    || (cfg!(unix) && error.kind() == std::io::ErrorKind::ConnectionRefused) =>
            {
                return None;
            }
            Err(_) => continue,
        };

        if stream
            .set_recv_timeout(Some(IDENTIFY_REPLY_TIMEOUT))
            .is_err()
        {
            continue;
        }

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

        if line.contains("\"PaneFlow\"") {
            return Some(line.trim().to_string());
        }
    }

    None
}

#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    Eof,
    TooLong,
    Got,
}

#[cfg(any(test, not(windows)))]
fn read_capped_line(reader: &mut impl BufRead, line: &mut String) -> std::io::Result<LineRead> {
    line.clear();
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

fn write_overloaded_error(writer: &mut Stream, message: &str) {
    let envelope = json!({
        "jsonrpc": "2.0",
        "error": {"code": -32000, "message": message},
        "id": Value::Null,
    });
    let _ = write_envelope(writer, &envelope);
}

fn reject_overloaded(mut stream: Stream) {
    write_overloaded_error(&mut stream, "server busy: too many concurrent connections");
}

fn blocking_connection(stream: Stream) -> std::io::Result<Stream> {
    stream.set_nonblocking(false)?;
    Ok(stream)
}

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
    request_tx: smol::channel::Sender<IpcRequest>,
    event_bus: Arc<crate::ipc_events::EventBus>,
    active_subscriptions: Arc<AtomicUsize>,
) {
    let Ok(writer_stream) = stream.try_clone() else {
        return;
    };

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
            auth::AuthOutcome::DegradedFallback => {}
        }
    }

    let caller_pid = peer_pid(&stream);

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
                let envelope = json!({
                    "jsonrpc": "2.0",
                    "error": {"code": -32600, "message": "request exceeds maximum length"},
                    "id": Value::Null,
                });
                let _ = write_envelope(&mut writer, &envelope);
                break;
            }
            Ok(LineRead::Got) => {}
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
                        let params = match request_params(&req) {
                            Ok(params) => params,
                            Err(message) => {
                                let envelope = json!({
                                    "jsonrpc": "2.0",
                                    "error": {"code": -32602, "message": message},
                                    "id": response_id,
                                });
                                if !suppress_reply && !write_envelope(&mut writer, &envelope) {
                                    break;
                                }
                                #[cfg(windows)]
                                break;
                                #[cfg(not(windows))]
                                continue;
                            }
                        };

                        if method == "events.subscribe" {
                            let Some(_subscription_guard) = ConnectionGuard::try_acquire(
                                Arc::clone(&active_subscriptions),
                                MAX_SUBSCRIPTION_CONNECTIONS,
                            ) else {
                                write_overloaded_error(
                                    &mut writer,
                                    "server busy: too many event subscriptions",
                                );
                                return;
                            };
                            serve_subscription(&mut writer, &params, &response_id, &event_bus);
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
                            "system.identify" => {
                                json!({"jsonrpc": "2.0", "result": {
                                    "name": "PaneFlow",
                                    "version": env!("CARGO_PKG_VERSION"),
                                    "protocol": "jsonrpc-2.0"
                                }, "id": response_id})
                            }
                            "system.counters" => {
                                json!({"jsonrpc": "2.0", "result": crate::work_counters::counters(), "id": response_id})
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

        if !suppress_reply && !write_envelope(&mut writer, &response) {
            break;
        }

        #[cfg(windows)]
        break;
    }
}

fn serve_subscription(
    writer: &mut Stream,
    params: &Value,
    id: &Value,
    bus: &Arc<crate::ipc_events::EventBus>,
) {
    use std::sync::mpsc::RecvTimeoutError;

    const HEARTBEAT: Duration = Duration::from_secs(30);

    let filter = match subscription_filter(params, id) {
        Ok(filter) => filter,
        Err(envelope) => {
            push_frame(writer, &envelope);
            return;
        }
    };
    let sub = bus.subscribe(filter);
    let ack = json!({"type": "subscribed", "id": sub.id});
    if !push_frame(writer, &ack) {
        return;
    }

    loop {
        let dropped = sub.take_dropped();
        if dropped > 0 {
            let marker = json!({"type": "dropped", "count": dropped});
            if !push_frame(writer, &marker) {
                break;
            }
        }
        match sub.rx.recv_timeout(HEARTBEAT) {
            Ok(line) => {
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

fn push_frame(writer: &mut Stream, value: &Value) -> bool {
    if !subscriber_connected(writer) {
        return false;
    }
    write_envelope(writer, value)
}

fn push_line(writer: &mut Stream, line: &[u8]) -> bool {
    if !subscriber_connected(writer) {
        return false;
    }
    push_bytes(writer, line)
}

fn write_envelope(writer: &mut Stream, value: &Value) -> bool {
    let mut frame = value.to_string();
    frame.push('\n');
    push_bytes(writer, frame.as_bytes())
}

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

    let mut remaining = buf;
    while !remaining.is_empty() {
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = event;
        let want = remaining.len().min(u32::MAX as usize) as u32;
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

#[cfg(not(windows))]
fn subscriber_connected(_writer: &Stream) -> bool {
    true
}

#[cfg(windows)]
fn subscriber_connected(writer: &Stream) -> bool {
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use windows_sys::Win32::System::Pipes::PeekNamedPipe;

    let Stream::NamedPipe(np) = writer;
    let handle = np.as_handle().as_raw_handle();
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
    request_tx: &smol::channel::Sender<IpcRequest>,
    method: String,
    params: Value,
    id: Value,
    caller_pid: Option<i64>,
) -> Value {
    let (resp_tx, resp_rx) = mpsc::channel();
    let state = Arc::new(RequestState::default());
    let ipc_req = IpcRequest {
        method: method.clone(),
        params,
        response_tx: resp_tx,
        state: Arc::clone(&state),
        caller_pid,
    };

    match request_tx.try_send(ipc_req) {
        Ok(()) => {}
        Err(smol::channel::TrySendError::Full(_)) => {
            return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "Paneflow is busy; retry shortly"}, "id": id});
        }
        Err(smol::channel::TrySendError::Closed(_)) => {
            return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "App shutting down"}, "id": id});
        }
    }

    await_or_cancel(
        &resp_rx,
        &state,
        IPC_DISPATCH_TIMEOUT,
        IPC_STARTED_REPLY_TIMEOUT,
        id,
    )
}

fn await_or_cancel(
    resp_rx: &mpsc::Receiver<Value>,
    state: &RequestState,
    timeout: Duration,
    started_reply_timeout: Duration,
    id: Value,
) -> Value {
    let queued_at = Instant::now();
    let mut started_at: Option<Instant> = None;
    loop {
        if started_at.is_none() && state.is_started() {
            started_at = Some(Instant::now());
        }
        let deadline = match started_at {
            Some(started) => started + started_reply_timeout,
            None => queued_at + timeout,
        };
        let wait_for = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        match resp_rx.recv_timeout(wait_for) {
            Ok(result) => return crate::app::ipc_handler::promote_response(result, id),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() < deadline {
                    continue;
                }
                if started_at.is_some() {
                    return json!({"jsonrpc": "2.0", "error": {"code": -32002, "message": "Request started but did not answer in time"}, "id": id});
                }
                if state.try_cancel() {
                    return json!({"jsonrpc": "2.0", "error": {"code": -32002, "message": "Request dispatch timeout"}, "id": id});
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return json!({"jsonrpc": "2.0", "error": {"code": -32000, "message": "App shutting down"}, "id": id});
            }
        }
    }
}

fn socket_path_spec() -> Option<paneflow_home::IpcEndpoint> {
    crate::runtime_paths::socket_path_spec()
}

#[cfg(unix)]
mod auth {
    use super::Stream;
    use interprocess::local_socket::prelude::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum AuthOutcome {
        Allow,
        Deny { server_uid: u32, peer_uid: u32 },
        DegradedFallback,
    }

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

    pub(super) fn server_uid() -> u32 {
        unsafe { libc::geteuid() as u32 }
    }

    pub(super) fn check_peer(stream: &Stream) -> AuthOutcome {
        let server = server_uid();
        match stream.peer_creds() {
            Ok(creds) => match creds.euid() {
                Some(peer) => authorize(server, peer),
                None => {
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

        #[test]
        fn server_uid_is_stable() {
            let a = server_uid();
            let b = server_uid();
            assert_eq!(a, b, "geteuid must be stable across calls");
        }

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
mod connection_limit_tests {
    use super::{ConnectionGuard, MAX_SUBSCRIPTION_CONNECTIONS};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn subscription_slots_are_capped_and_released() {
        let counter = Arc::new(AtomicUsize::new(0));
        let mut guards = Vec::new();
        for _ in 0..MAX_SUBSCRIPTION_CONNECTIONS {
            guards.push(
                ConnectionGuard::try_acquire(Arc::clone(&counter), MAX_SUBSCRIPTION_CONNECTIONS)
                    .expect("slot"),
            );
        }
        assert!(
            ConnectionGuard::try_acquire(Arc::clone(&counter), MAX_SUBSCRIPTION_CONNECTIONS)
                .is_none()
        );
        drop(guards.pop());
        assert_eq!(
            counter.load(Ordering::Acquire),
            MAX_SUBSCRIPTION_CONNECTIONS - 1
        );
        assert!(
            ConnectionGuard::try_acquire(Arc::clone(&counter), MAX_SUBSCRIPTION_CONNECTIONS)
                .is_some()
        );
    }
}

#[cfg(test)]
mod request_contract_tests {
    use super::{json, multiple_instances_allowed, request_params, subscription_filter};

    #[test]
    fn only_the_documented_value_allows_multiple_instances() {
        assert!(multiple_instances_allowed(Some("1")));
        for value in [None, Some(""), Some("0"), Some("true"), Some("yes")] {
            assert!(!multiple_instances_allowed(value), "{value:?}");
        }
    }

    #[test]
    fn request_params_must_be_an_object_when_present() {
        assert_eq!(request_params(&json!({"method": "m"})), Ok(json!({})));
        assert_eq!(request_params(&json!({"params": null})), Ok(json!({})));
        assert_eq!(
            request_params(&json!({"params": {"index": 1}})),
            Ok(json!({"index": 1}))
        );
        assert!(request_params(&json!({"params": [1]})).is_err());
        assert!(request_params(&json!({"params": "1"})).is_err());
    }

    #[test]
    fn a_subscription_param_error_echoes_the_request_id() {
        let error = subscription_filter(&json!({"types": ["bogus"]}), &json!(42))
            .expect_err("an unknown event type is refused");
        assert_eq!(error["id"], 42);
        assert_eq!(error["error"]["code"], -32602);
    }
}

#[cfg(test)]
mod framing_tests {
    use super::{LineRead, MAX_REQUEST_LEN, read_capped_line};
    use std::io::Cursor;

    #[test]
    fn capped_line_rejects_oversized_unterminated() {
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
    fn an_accepted_connection_delivers_a_reply_larger_than_the_socket_buffer() {
        use interprocess::local_socket::prelude::*;
        use interprocess::local_socket::{GenericFilePath, ListenerNonblockingMode, Stream};
        use std::io::{Read, Write};
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("paneflow.sock");
        let listener = super::bind_socket(&path).expect("bind");
        listener
            .set_nonblocking(ListenerNonblockingMode::Accept)
            .expect("nonblocking accept");
        let mut client =
            Stream::connect(path.to_fs_name::<GenericFilePath>().expect("name")).expect("connect");
        let accepted = loop {
            match listener.accept() {
                Ok(stream) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept: {error}"),
            }
        };
        let mut accepted = super::blocking_connection(accepted).expect("blocking stream");
        let reply = vec![b'x'; 256 * 1024];
        let expected = reply.len();
        let writer = std::thread::spawn(move || accepted.write_all(&reply));
        std::thread::sleep(Duration::from_millis(200));
        let mut received = Vec::new();
        client.read_to_end(&mut received).expect("read the reply");
        writer
            .join()
            .expect("writer thread")
            .expect("the whole reply is written");
        assert_eq!(received.len(), expected);
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
mod singleton_guard_tests {
    use super::{InstanceClaim, detect_existing_instance, singleton_refusal};
    use std::time::{Duration, Instant};

    #[test]
    fn a_live_answer_refuses_first_and_a_held_lock_refuses_only_without_one() {
        let live = Some("{\"app\":\"PaneFlow\"}".to_string());
        for claim in [
            InstanceClaim::Claimed,
            InstanceClaim::HeldElsewhere,
            InstanceClaim::Unavailable,
        ] {
            assert_eq!(singleton_refusal(claim, live.clone()), live);
        }
        assert_eq!(
            singleton_refusal(InstanceClaim::HeldElsewhere, None).as_deref(),
            Some(super::INSTANCE_STARTING)
        );
        assert_eq!(singleton_refusal(InstanceClaim::Claimed, None), None);
        assert_eq!(
            singleton_refusal(InstanceClaim::Unavailable, None),
            None,
            "without a lock, as on Windows, only a live answer refuses the launch"
        );
    }

    #[cfg(unix)]
    fn socket_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("pf-ipc")
            .tempdir_in("/tmp")
            .expect("short socket dir")
    }

    #[cfg(unix)]
    #[test]
    fn a_stale_socket_is_reported_at_once_without_retry_sleeps() {
        let dir = socket_dir();
        let path = dir.path().join("stale.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).expect("bind"));
        assert!(
            path.exists(),
            "the closed listener leaves its socket file behind"
        );
        let mut samples: Vec<Duration> = (0..20)
            .map(|_| {
                let started = Instant::now();
                assert!(detect_existing_instance(&path).is_none());
                started.elapsed()
            })
            .collect();
        samples.sort();
        let p95 = samples[samples.len() * 95 / 100 - 1];
        assert!(
            p95 <= Duration::from_millis(5),
            "p95 {p95:?} on a stale socket"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_live_paneflow_answer_refuses_the_second_instance() {
        use std::io::{BufRead, Write};
        let dir = socket_dir();
        let path = dir.path().join("live.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = std::io::BufReader::new(stream.try_clone().expect("clone"));
            let mut request = String::new();
            reader.read_line(&mut request).expect("request");
            let mut stream = stream;
            stream
                .write_all(b"{\"jsonrpc\":\"2.0\",\"result\":{\"app\":\"PaneFlow\"},\"id\":1}\n")
                .expect("reply");
        });
        let info = detect_existing_instance(&path).expect("a live instance answers");
        assert!(info.contains("\"PaneFlow\""));
        assert_eq!(
            singleton_refusal(InstanceClaim::Claimed, Some(info.clone())),
            Some(info)
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_listener_that_never_answers_is_settled_within_350_ms() {
        let dir = socket_dir();
        let path = dir.path().join("silent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        let held = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            std::thread::sleep(Duration::from_secs(1));
            drop(stream);
        });
        let started = Instant::now();
        assert!(detect_existing_instance(&path).is_none());
        assert!(
            started.elapsed() <= Duration::from_millis(350),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(singleton_refusal(InstanceClaim::Claimed, None), None);
        held.join().expect("silent listener");
    }

    #[cfg(unix)]
    #[test]
    fn two_launches_racing_for_the_instance_lock_let_exactly_one_continue() {
        let dir = socket_dir();
        let lock = super::instance_lock_path(&dir.path().join("race.sock"));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let launches: Vec<_> = (0..2)
            .map(|_| {
                let lock = lock.clone();
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    super::try_claim_instance_lock(&lock).expect("lock file")
                })
            })
            .collect();
        let claims: Vec<_> = launches
            .into_iter()
            .map(|launch| launch.join().expect("launch"))
            .collect();
        assert_eq!(claims.iter().filter(|claim| claim.is_some()).count(), 1);
        assert_eq!(
            singleton_refusal(InstanceClaim::HeldElsewhere, None).as_deref(),
            Some(super::INSTANCE_STARTING)
        );
        drop(claims);
        assert!(
            super::try_claim_instance_lock(&lock)
                .expect("lock file")
                .is_some(),
            "an exited instance never leaves a stale lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn quitting_removes_only_the_socket_this_instance_bound() {
        let dir = socket_dir();
        let path = dir.path().join("own.sock");
        let listener = super::bind_socket(&path).expect("bind");
        super::remember_own_socket(&path, super::socket_inode(&path).expect("inode"));
        super::release_own_socket();
        assert!(!path.exists(), "a normal quit removes the desktop socket");
        drop(listener);

        let ours = std::os::unix::net::UnixListener::bind(&path).expect("bind");
        super::remember_own_socket(&path, super::socket_inode(&path).expect("inode"));
        std::fs::remove_file(&path).expect("unlink");
        let _successor = std::os::unix::net::UnixListener::bind(&path).expect("successor");
        drop(ours);
        super::release_own_socket();
        assert!(path.exists(), "a successor's socket survives this quit");
    }

    #[test]
    fn absent_endpoint_is_reported_without_retry_sleeps() {
        let path = if cfg!(windows) {
            std::path::PathBuf::from(format!(r"\\.\pipe\paneflow-absent-{}", std::process::id()))
        } else {
            std::env::temp_dir().join(format!("paneflow-absent-{}.sock", std::process::id()))
        };
        let started = Instant::now();
        assert!(detect_existing_instance(&path).is_none());
        assert!(
            started.elapsed() < Duration::from_millis(60),
            "a missing endpoint must not pay the inter-attempt sleeps"
        );
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::{IpcRequest, RequestState, await_or_cancel, dispatch_to_gpui};
    use serde_json::json;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    const STARTED_REPLY: Duration = Duration::from_secs(5);

    fn test_ipc_request() -> IpcRequest {
        let (response_tx, _response_rx) = mpsc::channel();
        IpcRequest {
            method: "surface.read".to_string(),
            params: json!({}),
            response_tx,
            state: Arc::new(RequestState::default()),
            caller_pid: None,
        }
    }

    #[test]
    fn dispatch_to_gpui_returns_overload_when_request_queue_full() {
        let (tx, _rx) = smol::channel::bounded(1);
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
        let (tx, rx) = smol::channel::bounded::<IpcRequest>(1);
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
    fn a_timed_out_request_can_never_start_afterwards() {
        let (_tx, rx) = mpsc::channel::<serde_json::Value>();
        let state = RequestState::default();

        let resp = await_or_cancel(
            &rx,
            &state,
            Duration::from_millis(20),
            STARTED_REPLY,
            json!(7),
        );

        assert_eq!(resp["error"]["code"], -32002);
        assert_eq!(resp["id"], 7);
        assert!(state.is_cancelled());
        assert!(
            !state.try_start(),
            "the GPUI side must not run a request the client was told timed out"
        );
    }

    #[test]
    fn a_request_racing_its_deadline_either_runs_or_times_out_never_both() {
        for _ in 0..200 {
            let (tx, rx) = mpsc::channel::<serde_json::Value>();
            let state = Arc::new(RequestState::default());
            let gpui_state = Arc::clone(&state);
            let gpui = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1));
                if gpui_state.try_start() {
                    tx.send(json!({"mutated": true})).unwrap();
                    true
                } else {
                    false
                }
            });

            let resp = await_or_cancel(
                &rx,
                &state,
                Duration::from_millis(1),
                STARTED_REPLY,
                json!(1),
            );
            let ran = gpui.join().unwrap();

            if ran {
                assert_eq!(
                    resp["result"]["mutated"], true,
                    "a started request is awaited"
                );
            } else {
                assert_eq!(resp["error"]["code"], -32002);
            }
        }
    }

    #[test]
    fn await_or_cancel_waits_for_started_handler_instead_of_cancelling() {
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        let state = Arc::new(RequestState::default());
        assert!(state.try_start());
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            tx.send(json!({"status": "ok"})).unwrap();
        });

        let resp = await_or_cancel(
            &rx,
            &state,
            Duration::from_millis(5),
            STARTED_REPLY,
            json!(9),
        );

        assert!(
            !state.is_cancelled(),
            "started handlers must not be cancelled behind the client"
        );
        assert_eq!(resp["result"]["status"], "ok");
        assert_eq!(resp["id"], 9);
    }

    #[test]
    fn the_wait_after_start_is_bounded() {
        let (_tx, rx) = mpsc::channel::<serde_json::Value>();
        let state = RequestState::default();
        assert!(state.try_start());
        let began = std::time::Instant::now();

        let resp = await_or_cancel(
            &rx,
            &state,
            Duration::from_millis(5),
            Duration::from_millis(80),
            json!(4),
        );

        assert_eq!(resp["error"]["code"], -32002);
        assert!(began.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn await_or_cancel_passes_through_response_without_cancelling() {
        let (tx, rx) = mpsc::channel::<serde_json::Value>();
        tx.send(json!({"status": "ok"})).unwrap();
        let state = RequestState::default();
        let resp = await_or_cancel(&rx, &state, Duration::from_secs(5), STARTED_REPLY, json!(3));

        assert!(
            !state.is_cancelled(),
            "a timely response must not set the cancel flag"
        );
        assert_eq!(resp["result"]["status"], "ok");
        assert_eq!(resp["id"], 3);
    }
}

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

    fn connected_pair() -> (Stream, Stream, Listener) {
        let path = unique_pipe_path();
        let listener = {
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

        drop(client);

        assert!(
            wait_until_disconnected(&server, Duration::from_secs(2)),
            "PeekNamedPipe must report the closed peer (the CP-4 eviction gate)"
        );

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
        drop(client);
        assert!(
            wait_until_disconnected(&server, Duration::from_secs(2)),
            "peer close must settle before the write"
        );

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

#[cfg(test)]
mod counters_tests {
    use super::*;

    #[test]
    fn system_counters_answers_on_the_connection_thread_without_the_gpui_queue() {
        let dir = tempfile::tempdir().expect("tempdir");
        #[cfg(unix)]
        let path = dir.path().join("counters.sock");
        #[cfg(windows)]
        let path = std::path::PathBuf::from(format!(
            r"\\.\pipe\paneflow-counters-test-{}-{}",
            std::process::id(),
            dir.path()
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("x")
        ));
        let listener = bind_socket(&path).expect("the test socket binds");
        let (request_tx, _never_drained) = smol::channel::bounded(1);
        let server = std::thread::spawn(move || {
            let stream = listener.accept().expect("the client connects");
            handle_connection(
                stream,
                request_tx,
                crate::ipc_events::EventBus::new(),
                Arc::new(AtomicUsize::new(0)),
            );
        });
        let name = path
            .to_fs_name::<GenericFilePath>()
            .expect("the socket name");
        let mut client = Stream::connect(name).expect("the client connects");
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"system.counters\"}\n")
            .expect("the request is written");
        let mut line = String::new();
        BufReader::new(&mut client)
            .read_line(&mut line)
            .expect("the reply arrives");
        let reply: Value = serde_json::from_str(&line).expect("a JSON reply");
        let sample = paneflow_host::work_counters::sample(
            "desktop",
            &reply["result"],
            &["root_renders", "session_list_calls", "git_spawns.total"],
        );
        assert_eq!(
            sample.identity.map(|identity| identity.pid),
            Some(std::process::id())
        );
        assert!(
            sample.readings.values().all(|reading| matches!(
                reading,
                paneflow_host::work_counters::Reading::Measured(_)
            )),
            "{reply}"
        );
        drop(client);
        server.join().expect("the connection thread ends");
    }
}
