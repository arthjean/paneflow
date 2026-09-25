use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use interprocess::local_socket::{GenericFilePath, Listener, ListenerOptions, Stream, prelude::*};
use paneflow_config::schema::{SessionGeneration, SessionId, WorkspaceId};
use serde_json::{Value, json};

use crate::agent::AgentEvent;
use crate::control::{ConnectionAliases, ControlError};
use crate::host::{CreateSession, HostError, SessionHost};
use crate::manifest::now_ms;
use crate::protocol::{
    self, ClientHello, DATA_CHUNK_RAW_BYTES, ERR_BUSY, ERR_CHECKPOINT_TOO_LARGE, ERR_DEADLINE,
    ERR_ENGINE_REQUIRED, ERR_FRAME_TOO_LARGE, ERR_GENERATION_MISMATCH, ERR_HANDSHAKE_REQUIRED,
    ERR_INCOMPATIBLE, ERR_INTERNAL, ERR_INVALID_PARAMS, ERR_INVALID_REQUEST, ERR_LAUNCH_PENDING,
    ERR_METHOD_NOT_FOUND, ERR_NO_CONTROLLER, ERR_OUTPUT_EVICTED, ERR_OWNERSHIP_UNRESOLVED,
    ERR_PARSE, ERR_PROCESS_UNVERIFIED, ERR_SESSION_LIVE, ERR_SESSION_NOT_FOUND,
    ERR_SESSION_NOT_LIVE, ERR_SHUTTING_DOWN, ERR_SPAWN_FAILED, HOST_PROTOCOL_VERSION,
    METHOD_AGENT_EVENT, METHOD_AGENT_FOLLOW, METHOD_AGENT_SNAPSHOT, encode_data, error_envelope,
    result_envelope,
};
use crate::runtime::RuntimeError;
use paneflow_ipc_client::line_wire::{LineRead, Wire};

const CONTROL_CONNECTIONS_PER_PANE: usize = 2;
const PANES_A_HEAVY_WORKSPACE_ATTACHES: usize = 48;
const CONTROL_CALLS_IN_FLIGHT: usize = 8;

const MAX_CONNECTIONS: usize = 128;
pub const RESERVED_CONTROL_CONNECTIONS: usize = 8;
const MAX_STREAMING_CONNECTIONS: usize = MAX_CONNECTIONS - RESERVED_CONTROL_CONNECTIONS;

#[cfg(windows)]
const WINDOWS_PIPE_SDDL: &str = "D:P(A;;GA;;;OW)";

const _: () = assert!(
    MAX_CONNECTIONS
        >= PANES_A_HEAVY_WORKSPACE_ATTACHES * CONTROL_CONNECTIONS_PER_PANE
            + CONTROL_CALLS_IN_FLIGHT
);
const HANDSHAKE_DEADLINE: Duration = protocol::REQUEST_DEADLINE;
const CONTROL_IDLE_SLICE: Duration = Duration::from_secs(30);
pub const FOLLOW_KEEPALIVE: Duration = Duration::from_secs(2);

#[cfg(test)]
static DROP_CONNECTION_AFTER_INPUT: AtomicBool = AtomicBool::new(false);

pub struct ServerHandle {
    endpoint: PathBuf,
    shutdown: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl ServerHandle {
    pub fn spawn(host: Arc<SessionHost>, endpoint: PathBuf) -> io::Result<Self> {
        let listener = bind(&endpoint)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let flag = Arc::clone(&shutdown);
        let counter = Arc::clone(&active);
        let thread = std::thread::Builder::new()
            .name("paneflow-host-accept".into())
            .spawn(move || accept_loop(listener, host, flag, counter))?;
        Ok(Self {
            endpoint,
            shutdown,
            active,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    pub fn active_connections(&self) -> usize {
        self.active.load(Ordering::Acquire)
    }

    pub fn stop(mut self) -> io::Result<()> {
        self.shutdown.store(true, Ordering::Release);
        wake_accept_loop(&self.endpoint);
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| io::Error::other("the host accept thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shutdown.store(true, Ordering::Release);
            wake_accept_loop(&self.endpoint);
            let _ = thread.join();
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
        }
    }
}

pub fn serve(host: Arc<SessionHost>, endpoint: &Path, shutdown: Arc<AtomicBool>) -> io::Result<()> {
    let listener = bind(endpoint)?;
    let served = accept_loop(listener, host, shutdown, Arc::new(AtomicUsize::new(0)));
    #[cfg(unix)]
    {
        let _ = std::fs::remove_file(endpoint);
    }
    served
}

fn bind(endpoint: &Path) -> io::Result<Listener> {
    #[cfg(unix)]
    {
        if let Some(parent) = endpoint.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::symlink_metadata(endpoint) {
            Ok(metadata) => {
                use std::os::unix::fs::FileTypeExt;
                if !metadata.file_type().is_socket() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not a socket", endpoint.display()),
                    ));
                }
                ensure_unserved(endpoint)?;
                std::fs::remove_file(endpoint)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let name = endpoint.to_fs_name::<GenericFilePath>()?;
    #[cfg(windows)]
    let listener = {
        use interprocess::os::windows::{
            local_socket::ListenerOptionsExt, security_descriptor::SecurityDescriptor,
        };
        let sddl = widestring::U16CString::from_str(WINDOWS_PIPE_SDDL)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let descriptor = SecurityDescriptor::deserialize(sddl.as_ucstr())?;
        ListenerOptions::new()
            .name(name)
            .security_descriptor(descriptor)
            .create_sync()?
    };
    #[cfg(not(windows))]
    let listener = ListenerOptions::new().name(name).create_sync()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(endpoint, std::fs::Permissions::from_mode(0o600))?;
    }
    log::info!("paneflow-host: listening on {}", endpoint.display());
    Ok(listener)
}

#[cfg(unix)]
fn ensure_unserved(endpoint: &Path) -> io::Result<()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let target = endpoint.to_path_buf();
    std::thread::Builder::new()
        .name("paneflow-host-endpoint-probe".into())
        .spawn(move || {
            let _ = sender.send(std::os::unix::net::UnixStream::connect(target).map(drop));
        })?;
    match receiver.recv_timeout(HANDSHAKE_DEADLINE) {
        Err(_) | Ok(Ok(())) => Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!(
                "{} is served by another live process; it is left untouched",
                endpoint.display()
            ),
        )),
        Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => Ok(()),
        Ok(Err(error)) => Err(io::Error::new(
            error.kind(),
            format!(
                "{} could not be probed ({error}); it is left untouched",
                endpoint.display()
            ),
        )),
    }
}

fn accept_loop(
    listener: Listener,
    host: Arc<SessionHost>,
    shutdown: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
) -> io::Result<()> {
    for connection in listener.incoming() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("paneflow-host: accept failed: {error}");
                continue;
            }
        };
        let Some(guard) = ConnectionGuard::acquire(Arc::clone(&active)) else {
            reject_busy(stream);
            continue;
        };
        let host = Arc::clone(&host);
        let shutdown = Arc::clone(&shutdown);
        let spawned = std::thread::Builder::new()
            .name("paneflow-host-conn".into())
            .spawn(move || {
                let _guard = guard;
                match Wire::new(stream, protocol::MAX_CONTROL_FRAME_BYTES) {
                    Ok(wire) => handle_connection(wire, host, shutdown),
                    Err(error) => log::debug!("paneflow-host: connection setup failed: {error}"),
                }
            });
        if let Err(error) = spawned {
            log::warn!("paneflow-host: cannot start a connection thread: {error}");
        }
    }
    Ok(())
}

struct ConnectionGuard(Arc<AtomicUsize>);

impl ConnectionGuard {
    fn acquire(counter: Arc<AtomicUsize>) -> Option<Self> {
        loop {
            let current = counter.load(Ordering::Acquire);
            if current >= MAX_CONNECTIONS {
                return None;
            }
            if counter
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(Self(counter));
            }
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct StreamingGuard<'a>(&'a AtomicUsize);

impl<'a> StreamingGuard<'a> {
    fn acquire(host: &'a SessionHost) -> Option<Self> {
        let counter = host.streaming_connections();
        loop {
            let current = counter.load(Ordering::Acquire);
            if current >= MAX_STREAMING_CONNECTIONS {
                return None;
            }
            if counter
                .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Some(Self(counter));
            }
        }
    }
}

impl Drop for StreamingGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn refuse_streaming(wire: &mut Wire, id: &Value) -> Flow {
    let envelope = error_envelope(
        id,
        ERR_BUSY,
        format!(
            "host busy: {MAX_STREAMING_CONNECTIONS} streaming connections are active; {RESERVED_CONTROL_CONNECTIONS} slots stay reserved for control"
        ),
        None,
    );
    match wire.write_json(&envelope) {
        Ok(()) => Flow::Continue,
        Err(_) => Flow::Close,
    }
}

fn reject_busy(stream: Stream) {
    if let Ok(mut wire) = Wire::new(stream, protocol::MAX_CONTROL_FRAME_BYTES) {
        let _ = wire.write_json(&error_envelope(
            &Value::Null,
            ERR_BUSY,
            "host busy: too many concurrent connections",
            None,
        ));
    }
}

enum Flow {
    Continue,
    Close,
}

fn handle_connection(mut wire: Wire, host: Arc<SessionHost>, shutdown: Arc<AtomicBool>) {
    let mut greeted = false;
    let mut attaches = false;
    let mut aliases = ConnectionAliases::default();
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let slice = if greeted {
            CONTROL_IDLE_SLICE
        } else {
            HANDSHAKE_DEADLINE
        };
        let line = match wire.read_line(slice) {
            Ok(LineRead::Line(line)) => line,
            Ok(LineRead::Eof) => return,
            Ok(LineRead::Idle) => {
                if greeted {
                    continue;
                }
                return;
            }
            Ok(LineRead::TooLong) => {
                let _ = wire.write_json(&error_envelope(
                    &Value::Null,
                    ERR_FRAME_TOO_LARGE,
                    "request exceeds the 64 KiB control frame limit",
                    None,
                ));
                return;
            }
            Err(_) => return,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(error) => {
                if wire
                    .write_json(&error_envelope(
                        &Value::Null,
                        ERR_PARSE,
                        format!("parse error: {error}"),
                        None,
                    ))
                    .is_err()
                {
                    return;
                }
                continue;
            }
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let Some(method) = request.get("method").and_then(Value::as_str) else {
            if wire
                .write_json(&error_envelope(
                    &id,
                    ERR_INVALID_REQUEST,
                    "missing method",
                    None,
                ))
                .is_err()
            {
                return;
            }
            continue;
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        if !greeted {
            if method != "host.hello" {
                let _ = wire.write_json(&error_envelope(
                    &id,
                    ERR_HANDSHAKE_REQUIRED,
                    "host.hello must be the first request on a connection",
                    None,
                ));
                return;
            }
            match handshake(&host, &params) {
                Ok((identity, offers_engine)) => {
                    greeted = true;
                    attaches = offers_engine;
                    if wire.write_json(&result_envelope(&id, identity)).is_err() {
                        return;
                    }
                }
                Err(message) => {
                    let _ = wire.write_json(&error_envelope(&id, ERR_INCOMPATIBLE, message, None));
                    return;
                }
            }
            continue;
        }
        let flow = match method {
            "session.attach" | "session.output" if !attaches => {
                let written = wire.write_json(&error_envelope(
                    &id,
                    ERR_ENGINE_REQUIRED,
                    format!(
                        "{method} needs a client that declared a compatible terminal engine in host.hello"
                    ),
                    None,
                ));
                match written {
                    Ok(()) => Flow::Continue,
                    Err(_) => Flow::Close,
                }
            }
            "session.attach" => stream_attach(&mut wire, &host, &id, &params),
            "session.output" => stream_output(&mut wire, &host, &shutdown, &id, &params),
            METHOD_AGENT_FOLLOW => stream_agent_follow(&mut wire, &host, &shutdown, &id),
            "host.shutdown" => {
                let force = params
                    .get("force")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let envelope = match host.request_shutdown(force) {
                    Ok(report) => result_envelope(
                        &id,
                        json!({
                            "stopping": true,
                            "host_instance": host.instance(),
                            "ended_sessions": report.ended.len(),
                            "unresolved": report.unresolved,
                        }),
                    ),
                    Err(HostError::SessionsUnresolved { sessions }) => error_envelope(
                        &id,
                        ERR_OWNERSHIP_UNRESOLVED,
                        HostError::SessionsUnresolved {
                            sessions: sessions.clone(),
                        }
                        .to_string(),
                        Some(
                            json!({"unresolved": sessions, "owned_sessions": host.owned_sessions()}),
                        ),
                    ),
                    Err(HostError::Storage(reason)) => error_envelope(
                        &id,
                        protocol::ERR_DURABILITY,
                        reason,
                        Some(json!({"owned_sessions": host.owned_sessions()})),
                    ),
                    Err(error) => {
                        let live = host.live_sessions();
                        error_envelope(
                            &id,
                            ERR_SESSION_LIVE,
                            error.to_string(),
                            Some(json!({"live_sessions": live})),
                        )
                    }
                };
                let stopping = envelope.get("result").is_some();
                let written = wire.write_json(&envelope);
                if stopping {
                    shutdown.store(true, Ordering::Release);
                    host.wake_followers();
                    wake_accept_loop(Path::new(&host.identity().endpoint));
                    return;
                }
                match written {
                    Ok(()) => Flow::Continue,
                    Err(_) => Flow::Close,
                }
            }
            METHOD_AGENT_EVENT => {
                let ingested = AgentEvent::from_params(&params)
                    .map_err(DispatchError::Params)
                    .and_then(|mut event| {
                        event.received_at_ms = Some(now_ms());
                        host.ingest_agent_event(&event).map_err(DispatchError::Host)
                    });
                let envelope = match ingested {
                    Ok(outcome) => result_envelope(&id, outcome.ack),
                    Err(error) => error_to_envelope(&id, error),
                };
                let written = wire.write_json(&envelope);
                match written {
                    Ok(()) => Flow::Continue,
                    Err(_) => Flow::Close,
                }
            }
            _ => {
                let answered = crate::control::dispatch(
                    &host,
                    &mut aliases,
                    host.permissions(),
                    method,
                    &params,
                    now_ms(),
                );
                let envelope = match answered {
                    Some(Ok(result)) => result_envelope(&id, result),
                    Some(Err(error)) => control_error_to_envelope(&id, error),
                    None => match dispatch(&host, method, &params) {
                        Ok(result) => fit_control_frame(&id, result_envelope(&id, result)),
                        Err(error) => error_to_envelope(&id, error),
                    },
                };
                #[cfg(test)]
                if method == "session.input"
                    && DROP_CONNECTION_AFTER_INPUT.swap(false, Ordering::AcqRel)
                {
                    return;
                }
                match wire.write_json(&envelope) {
                    Ok(()) => Flow::Continue,
                    Err(_) => Flow::Close,
                }
            }
        };
        if matches!(flow, Flow::Close) {
            return;
        }
    }
}

fn wake_accept_loop(endpoint: &Path) {
    #[cfg(windows)]
    {
        use interprocess::ConnectWaitMode;
        use interprocess::os::windows::named_pipe::{DuplexPipeStream, pipe_mode};
        let _ = DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path_with_wait_mode(
            endpoint.as_os_str(),
            ConnectWaitMode::Timeout(Duration::from_millis(250)),
        );
    }
    #[cfg(not(windows))]
    if let Ok(name) = endpoint.to_fs_name::<GenericFilePath>() {
        let _ = Stream::connect(name);
    }
}

fn handshake(host: &SessionHost, params: &Value) -> Result<(Value, bool), String> {
    let hello: ClientHello = serde_json::from_value(params.clone())
        .map_err(|error| format!("host.hello params are invalid: {error}"))?;
    let identity = host.identity();
    protocol::check_compatibility(
        HOST_PROTOCOL_VERSION,
        &identity.engine,
        hello.protocol,
        hello.engine.as_ref(),
    )
    .map_err(|incompatibility| {
        format!(
            "client {} is incompatible with this host: {incompatibility}",
            hello.client
        )
    })?;
    if let Some(drift) = protocol::build_drift(&identity.version, hello.build.as_deref()) {
        log::info!(
            "paneflow-host: client {} was built from another Paneflow release ({drift}); protocol and engine decide compatibility",
            hello.client
        );
    }
    let offers_engine = hello.attaches();
    serde_json::to_value(identity)
        .map(|identity| (identity, offers_engine))
        .map_err(|error| error.to_string())
}

fn control_error_to_envelope(id: &Value, error: ControlError) -> Value {
    match error {
        ControlError::Params(message) => error_envelope(id, ERR_INVALID_PARAMS, message, None),
        ControlError::NoController(message) => error_envelope(id, ERR_NO_CONTROLLER, message, None),
        ControlError::Host(error) => error_to_envelope(id, DispatchError::Host(error)),
    }
}

#[derive(Debug)]
enum DispatchError {
    Params(String),
    MethodNotFound(String),
    Host(HostError),
}

impl From<HostError> for DispatchError {
    fn from(error: HostError) -> Self {
        Self::Host(error)
    }
}

fn error_to_envelope(id: &Value, error: DispatchError) -> Value {
    match error {
        DispatchError::Params(message) => error_envelope(id, ERR_INVALID_PARAMS, message, None),
        DispatchError::MethodNotFound(method) => error_envelope(
            id,
            ERR_METHOD_NOT_FOUND,
            format!("method not found: {method}"),
            None,
        ),
        DispatchError::Host(error) => {
            let code = match &error {
                HostError::SessionNotFound(_) => ERR_SESSION_NOT_FOUND,
                HostError::SessionExists { .. } | HostError::InvalidRequest(_) => {
                    ERR_INVALID_PARAMS
                }
                HostError::GenerationMismatch { .. } => ERR_GENERATION_MISMATCH,
                HostError::SessionNotLive(_) => ERR_SESSION_NOT_LIVE,
                HostError::SessionLive(_) | HostError::SessionsLive { .. } => ERR_SESSION_LIVE,
                HostError::SessionsUnresolved { .. } | HostError::OwnershipUnresolved { .. } => {
                    ERR_OWNERSHIP_UNRESOLVED
                }
                HostError::LaunchPending(_) => ERR_LAUNCH_PENDING,
                HostError::Busy(_) => ERR_BUSY,
                HostError::ShuttingDown => ERR_SHUTTING_DOWN,
                HostError::OwnerBusy(_) => ERR_INTERNAL,
                HostError::ProcessUnverified(_) => ERR_PROCESS_UNVERIFIED,
                HostError::SpawnFailed { .. } => ERR_SPAWN_FAILED,
                HostError::Runtime(RuntimeError::NotLive) => ERR_SESSION_NOT_LIVE,
                HostError::Runtime(RuntimeError::CheckpointTooLarge { .. }) => {
                    ERR_CHECKPOINT_TOO_LARGE
                }
                HostError::Runtime(RuntimeError::OutputEvicted { .. }) => ERR_OUTPUT_EVICTED,
                HostError::Runtime(RuntimeError::Deadline(_)) => ERR_DEADLINE,
                HostError::Durability(_) => protocol::ERR_DURABILITY,
                HostError::Runtime(_) | HostError::Storage(_) => ERR_INTERNAL,
            };
            error_envelope(id, code, error.to_string(), None)
        }
    }
}

fn param_session(params: &Value) -> Result<SessionId, DispatchError> {
    let raw = params
        .get("session")
        .and_then(Value::as_str)
        .ok_or_else(|| DispatchError::Params("missing session".to_string()))?;
    SessionId::parse(raw).map_err(|e| DispatchError::Params(e.to_string()))
}

fn param_generation(params: &Value) -> Result<Option<SessionGeneration>, DispatchError> {
    match params.get("generation") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|e| DispatchError::Params(format!("invalid generation: {e}"))),
    }
}

fn param_u64(params: &Value, key: &str) -> Result<Option<u64>, DispatchError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| DispatchError::Params(format!("{key} must be an unsigned integer"))),
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

const TEXT_FRAME_BUDGET_BYTES: usize = 48 * 1024;
const TEXT_FRAME_ENCODED_BUDGET_BYTES: usize = 56 * 1024;
const _: () = assert!(TEXT_FRAME_ENCODED_BUDGET_BYTES + 4096 <= protocol::MAX_CONTROL_FRAME_BYTES);

fn text_frame(text: &str, from: u64) -> (&str, Option<u64>) {
    let mut start = usize::try_from(from).unwrap_or(text.len()).min(text.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = (start + TEXT_FRAME_BUDGET_BYTES).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    loop {
        let chunk = &text[start..end];
        let encoded = serde_json::to_string(chunk).map_or(usize::MAX, |json| json.len());
        if encoded <= TEXT_FRAME_ENCODED_BUDGET_BYTES || chunk.is_empty() {
            let next = (end < text.len()).then_some(end as u64);
            return (chunk, next);
        }
        end = start + (end - start) / 2;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
    }
}

fn fit_control_frame(id: &Value, envelope: Value) -> Value {
    let encoded = serde_json::to_vec(&envelope).map_or(usize::MAX, |bytes| bytes.len());
    if encoded <= protocol::MAX_CONTROL_FRAME_BYTES {
        return envelope;
    }
    error_envelope(
        id,
        ERR_FRAME_TOO_LARGE,
        "reply exceeds the 64 KiB control frame limit",
        None,
    )
}

fn dispatch(host: &SessionHost, method: &str, params: &Value) -> Result<Value, DispatchError> {
    match method {
        "host.hello" => Ok(to_value(host.identity())),
        "host.status" => {
            let sessions = host.list(None);
            let live = sessions.iter().filter(|s| s.live).count();
            let resources = host.resource_report();
            Ok(json!({
                "identity": host.identity(),
                "sessions": sessions.len(),
                "live_sessions": live,
                "methods": protocol::METHODS,
                "helpers": {
                    "ai_hook_dir": host.helper_dir().map(|dir| dir.display().to_string()),
                },
                "resources": {
                    "persistence": resources.persistence,
                    "checkpoints": resources.checkpoints,
                    "live_runtimes": resources.live_runtimes,
                    "pending_launches": resources.pending_launches,
                    "connections": {
                        "streaming": host.streaming_connections().load(Ordering::Acquire),
                        "streaming_limit": MAX_STREAMING_CONNECTIONS,
                        "limit": MAX_CONNECTIONS,
                        "reserved_control": RESERVED_CONTROL_CONNECTIONS,
                    },
                    "sessions": resources.sessions,
                },
            }))
        }
        "session.list" => {
            let workspace = match params.get("workspace").and_then(Value::as_str) {
                Some(raw) => Some(
                    WorkspaceId::parse(raw).map_err(|e| DispatchError::Params(e.to_string()))?,
                ),
                None => None,
            };
            Ok(json!({
                "sessions": host.rows(
                    workspace.as_ref(),
                    crate::host::INACTIVE_ROWS_PER_WORKSPACE,
                )
            }))
        }
        "session.create" => {
            let request: CreateSession = serde_json::from_value(params.clone())
                .map_err(|e| DispatchError::Params(format!("invalid create request: {e}")))?;
            Ok(to_value(&host.create(request)?))
        }
        "session.inspect" => {
            let session = param_session(params)?;
            Ok(to_value(&host.inspect(&session)?))
        }
        "session.stop" => {
            let session = param_session(params)?;
            let generation = param_generation(params)?;
            Ok(to_value(&host.stop(&session, generation)?))
        }
        "session.restart" => {
            let session = param_session(params)?;
            let generation = param_generation(params)?;
            Ok(to_value(&host.restart(&session, generation)?))
        }
        "session.remove" => {
            let session = param_session(params)?;
            Ok(json!({"removed": to_value(&host.remove(&session)?)}))
        }
        "session.input" => {
            let session = param_session(params)?;
            let generation = param_generation(params)?;
            let data = params
                .get("data")
                .and_then(Value::as_str)
                .ok_or_else(|| DispatchError::Params("missing data".to_string()))?;
            let bytes = protocol::decode_data(data).map_err(DispatchError::Params)?;
            let accepted = host.input(&session, generation, bytes)?;
            Ok(json!({"accepted_bytes": accepted}))
        }
        "session.runtime.bind" => {
            let session = param_session(params)?;
            let generation = param_generation(params)?;
            let runtime_id = params
                .get("runtime_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty());
            Ok(host.bind_runtime(&session, generation, runtime_id)?)
        }
        "session.resize" => {
            let session = param_session(params)?;
            let generation = param_generation(params)?;
            let cols = param_u64(params, "cols")?
                .and_then(|c| u16::try_from(c).ok())
                .filter(|c| *c > 0)
                .ok_or_else(|| DispatchError::Params("cols must be 1..=65535".to_string()))?;
            let rows = param_u64(params, "rows")?
                .and_then(|r| u16::try_from(r).ok())
                .filter(|r| *r > 0)
                .ok_or_else(|| DispatchError::Params("rows must be 1..=65535".to_string()))?;
            host.resize(&session, generation, cols, rows)?;
            Ok(json!({"cols": cols, "rows": rows}))
        }
        "session.text" => {
            let session = param_session(params)?;
            let from = params["from"].as_u64().unwrap_or(0);
            let text = host.text(&session)?;
            let (chunk, next_offset) = text_frame(&text.text, from);
            Ok(json!({
                "session": session,
                "text": chunk,
                "live": text.live,
                "available": text.available,
                "complete": text.complete,
                "total_bytes": text.text.len(),
                "next_offset": next_offset,
            }))
        }
        METHOD_AGENT_SNAPSHOT => Ok(json!({"sessions": host.agent_snapshot()})),
        _ => Err(DispatchError::MethodNotFound(method.to_string())),
    }
}

fn stream_agent_follow(
    wire: &mut Wire,
    host: &SessionHost,
    shutdown: &AtomicBool,
    id: &Value,
) -> Flow {
    let subscription = host.subscribe_agents();
    let header = result_envelope(id, json!({"sessions": host.agent_snapshot()}));
    if wire.write_json(&header).is_err() {
        host.unsubscribe_agents(subscription.id);
        return Flow::Close;
    }
    let mut last_frame_at = std::time::Instant::now();
    loop {
        if shutdown.load(Ordering::Acquire) {
            host.unsubscribe_agents(subscription.id);
            let end = json!({"type": "end", "reason": "the local host is stopping"});
            return match wire.write_json(&end) {
                Ok(()) => Flow::Close,
                Err(_) => Flow::Close,
            };
        }
        match subscription.frames.recv_timeout(FOLLOW_KEEPALIVE) {
            Ok(frame) => {
                if wire.write_json(&frame).is_err() {
                    host.unsubscribe_agents(subscription.id);
                    return Flow::Close;
                }
                last_frame_at = std::time::Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if last_frame_at.elapsed() >= FOLLOW_KEEPALIVE {
                    if wire.write_json(&json!({"type": "keepalive"})).is_err() {
                        host.unsubscribe_agents(subscription.id);
                        return Flow::Close;
                    }
                    last_frame_at = std::time::Instant::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                host.unsubscribe_agents(subscription.id);
                let end = json!({"type": "end", "reason": "the agent stream was dropped"});
                let _ = wire.write_json(&end);
                return Flow::Close;
            }
        }
    }
}

fn stream_attach(wire: &mut Wire, host: &SessionHost, id: &Value, params: &Value) -> Flow {
    let Some(_streaming) = StreamingGuard::acquire(host) else {
        return refuse_streaming(wire, id);
    };
    let checkpoint = match param_session(params)
        .and_then(|session| Ok((session, param_generation(params)?)))
        .and_then(|(session, generation)| {
            host.checkpoint(&session, generation)
                .map(|checkpoint| (session, checkpoint))
                .map_err(DispatchError::from)
        }) {
        Ok(checkpoint) => checkpoint,
        Err(error) => {
            return match wire.write_json(&error_to_envelope(id, error)) {
                Ok(()) => Flow::Continue,
                Err(_) => Flow::Close,
            };
        }
    };
    let (session, checkpoint) = checkpoint;
    let chunks = checkpoint.snapshot.len().div_ceil(DATA_CHUNK_RAW_BYTES);
    let header = result_envelope(
        id,
        json!({
            "session": session,
            "generation": checkpoint.generation,
            "host_instance": host.instance(),
            "offset": checkpoint.offset,
            "cols": checkpoint.cols,
            "rows": checkpoint.rows,
            "bytes": checkpoint.snapshot.len(),
            "chunks": chunks,
        }),
    );
    if wire.write_json(&header).is_err() {
        return Flow::Close;
    }
    for (index, chunk) in checkpoint.snapshot.chunks(DATA_CHUNK_RAW_BYTES).enumerate() {
        let line = json!({
            "type": "chunk",
            "session": session,
            "generation": checkpoint.generation,
            "index": index,
            "data": encode_data(chunk),
        });
        if wire.write_json(&line).is_err() {
            return Flow::Close;
        }
    }
    let end = json!({
        "type": "end",
        "session": session,
        "generation": checkpoint.generation,
        "offset": checkpoint.offset,
    });
    match wire.write_json(&end) {
        Ok(()) => Flow::Continue,
        Err(_) => Flow::Close,
    }
}

fn stream_output(
    wire: &mut Wire,
    host: &SessionHost,
    shutdown: &AtomicBool,
    id: &Value,
    params: &Value,
) -> Flow {
    let parsed = param_session(params).and_then(|session| {
        Ok((
            session,
            param_generation(params)?,
            param_u64(params, "from")?.unwrap_or(0),
            params
                .get("follow")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        ))
    });
    let (session, generation, mut offset, follow) = match parsed {
        Ok(parsed) => parsed,
        Err(error) => {
            return match wire.write_json(&error_to_envelope(id, error)) {
                Ok(()) => Flow::Continue,
                Err(_) => Flow::Close,
            };
        }
    };
    let Some(_streaming) = StreamingGuard::acquire(host) else {
        return refuse_streaming(wire, id);
    };
    let generation = match generation {
        Some(generation) => generation,
        None => match host.inspect(&session) {
            Ok(summary) => summary.manifest.generation,
            Err(error) => {
                return match wire.write_json(&error_to_envelope(id, error.into())) {
                    Ok(()) => Flow::Continue,
                    Err(_) => Flow::Close,
                };
            }
        },
    };
    let header = result_envelope(
        id,
        json!({
            "session": session,
            "generation": generation,
            "host_instance": host.instance(),
            "from": offset,
            "follow": follow,
        }),
    );
    if wire.write_json(&header).is_err() {
        return Flow::Close;
    }
    let stream = match host.output_stream(&session, Some(generation)) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = wire.write_json(&error_to_envelope(&Value::Null, error.into()));
            return Flow::Close;
        }
    };
    let mut last_frame_at = std::time::Instant::now();
    loop {
        let read = match &stream {
            Ok(stream) => stream
                .read(offset, DATA_CHUNK_RAW_BYTES)
                .map_err(|evicted| RuntimeError::OutputEvicted {
                    requested: evicted.requested,
                    tail_start: evicted.tail_start,
                    tail_end: evicted.tail_end,
                })
                .map_err(HostError::from),
            Err(end) if offset == end.end_offset => Ok(end.clone()),
            Err(end) => Err(HostError::Runtime(RuntimeError::OutputEvicted {
                requested: offset,
                tail_start: end.end_offset,
                tail_end: end.end_offset,
            })),
        };
        let slice = match read {
            Ok(slice) => slice,
            Err(error) => {
                let _ = wire.write_json(&error_to_envelope(&Value::Null, error.into()));
                return Flow::Close;
            }
        };
        if !slice.data.is_empty() {
            let line = json!({
                "type": "output",
                "session": session,
                "generation": generation,
                "offset": slice.offset,
                "data": encode_data(&slice.data),
            });
            if wire.write_json(&line).is_err() {
                return Flow::Close;
            }
            last_frame_at = std::time::Instant::now();
            offset = slice.offset + slice.data.len() as u64;
            if slice.end_offset > offset {
                continue;
            }
        }
        let finished = !follow || !slice.live || shutdown.load(Ordering::Acquire);
        if finished && slice.end_offset <= offset {
            let end = json!({
                "type": "end",
                "session": session,
                "generation": generation,
                "next_offset": offset,
                "live": slice.live,
            });
            return match wire.write_json(&end) {
                Ok(()) => Flow::Continue,
                Err(_) => Flow::Close,
            };
        }
        let Ok(stream) = &stream else {
            continue;
        };
        let keepalive_at = last_frame_at + FOLLOW_KEEPALIVE;
        match stream.wait_past(offset, keepalive_at) {
            crate::stream::StreamWait::Published | crate::stream::StreamWait::Ended => {}
            crate::stream::StreamWait::Deadline => {
                if shutdown.load(Ordering::Acquire) {
                    continue;
                }
                let line = json!({
                    "type": "keepalive",
                    "session": session,
                    "generation": generation,
                    "offset": offset,
                });
                if wire.write_json(&line).is_err() {
                    return Flow::Close;
                }
                last_frame_at = std::time::Instant::now();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionLifecycle;
    use crate::client::{HostClient, HostClientError, OutputEnd};
    use crate::host::SessionSummary;
    use crate::protocol::ERR_OUTPUT_EVICTED;
    use crate::protocol::{ERR_HANDSHAKE_REQUIRED, ERR_SESSION_LIVE, MAX_CONTROL_FRAME_BYTES};

    use std::sync::atomic::AtomicU64;
    use std::time::Instant;

    static NEXT_ENDPOINT: AtomicU64 = AtomicU64::new(0);

    fn test_endpoint(home: &Path) -> PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            NEXT_ENDPOINT.fetch_add(1, Ordering::Relaxed)
        );
        #[cfg(windows)]
        {
            let _ = home;
            PathBuf::from(format!(r"\\.\pipe\paneflow-host-test-{unique}"))
        }
        #[cfg(unix)]
        {
            home.join(format!("h{unique}.sock"))
        }
    }

    fn shell_create_params() -> Value {
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q", "/D"]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<&str>::new());
        json!({
            "shell": shell,
            "args": args,
            "cwd": std::env::temp_dir().display().to_string(),
            "cols": 80,
            "rows": 24,
        })
    }

    fn start() -> (tempfile::TempDir, Arc<SessionHost>, ServerHandle) {
        let home = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(home.path());
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint).unwrap();
        (home, host, server)
    }

    #[test]
    fn a_text_frame_stops_at_its_budget_on_a_char_boundary_and_resumes_at_the_next_offset() {
        let text = "é".repeat(TEXT_FRAME_BUDGET_BYTES);
        let (first, next) = text_frame(&text, 0);
        assert!(first.len() <= TEXT_FRAME_BUDGET_BYTES);
        assert!(first.chars().all(|c| c == 'é'));
        let next = next.expect("more text follows");
        let (second, rest) = text_frame(&text, next);
        assert_eq!(first.len() + second.len(), text.len());
        assert_eq!(rest, None);
        let escapes = "\u{1b}".repeat(TEXT_FRAME_BUDGET_BYTES);
        let (chunk, _) = text_frame(&escapes, 0);
        assert!(serde_json::to_string(chunk).unwrap().len() <= TEXT_FRAME_ENCODED_BUDGET_BYTES);
        assert!(!chunk.is_empty());
        let (past_end, none) = text_frame("abc", 10);
        assert_eq!((past_end, none), ("", None));
    }

    #[test]
    fn an_oversized_reply_is_refused_as_a_frame_error_instead_of_closing_the_connection() {
        let id = json!(7);
        let oversized = result_envelope(&id, json!({"text": "x".repeat(MAX_CONTROL_FRAME_BYTES)}));
        let fitted = fit_control_frame(&id, oversized);
        assert_eq!(fitted["error"]["code"], ERR_FRAME_TOO_LARGE);
        assert_eq!(fitted["id"], id);
        let small = result_envelope(&id, json!({"text": "ok"}));
        assert_eq!(fit_control_frame(&id, small.clone()), small);
    }

    #[test]
    fn host_status_reports_queue_capacities_and_reserved_control_slots() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        let created = host
            .create(serde_json::from_value(shell_create_params()).unwrap())
            .unwrap();
        let session = created.manifest.session;
        let status = client.call("host.status", json!({})).unwrap();
        let resources = &status["resources"];
        assert_eq!(
            resources["persistence"]["budget_bytes"],
            crate::persistence::QUEUE_BUDGET_BYTES
        );
        assert_eq!(resources["checkpoints"]["max_concurrent"], 2);
        assert_eq!(resources["connections"]["limit"], MAX_CONNECTIONS);
        assert_eq!(
            resources["connections"]["reserved_control"],
            RESERVED_CONTROL_CONNECTIONS
        );
        assert_eq!(resources["live_runtimes"], 1);
        let entry = resources["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["session"] == json!(session))
            .unwrap()
            .clone();
        assert!(entry["runtime"]["tail_allocated_bytes"].is_u64());
        assert!(entry["runtime"]["tail_retained_bytes"].is_u64());
        assert_eq!(entry["runtime"]["inbox_budget_bytes"], 2 * 1024 * 1024);
        host.stop(&session, None).unwrap();
        server.stop().unwrap();
    }

    #[test]
    fn streaming_followers_leave_reserved_slots_for_control_requests() {
        let (_home, host, server) = start();
        let created = host
            .create(serde_json::from_value(shell_create_params()).unwrap())
            .unwrap();
        let session = created.manifest.session;
        let stop = Arc::new(AtomicBool::new(false));
        let mut followers = Vec::new();
        for index in 0..MAX_STREAMING_CONNECTIONS {
            let endpoint = server.endpoint().to_path_buf();
            let session = session.clone();
            let stop = Arc::clone(&stop);
            followers.push(std::thread::spawn(move || {
                let hello = ClientHello::local(format!("follower-{index}"));
                let mut client = HostClient::connect(&endpoint, &hello).unwrap();
                client
                    .output(
                        &session,
                        None,
                        0,
                        true,
                        |_, _| true,
                        || !stop.load(Ordering::Acquire),
                    )
                    .map(|_| ())
            }));
        }
        let registered = Instant::now() + Duration::from_secs(20);
        while host.streaming_connections().load(Ordering::Acquire) < MAX_STREAMING_CONNECTIONS {
            assert!(Instant::now() < registered, "every follower registers");
            std::thread::sleep(Duration::from_millis(20));
        }
        let hello = ClientHello::local("control");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let refused = control.output(&session, None, 0, true, |_, _| true, || true);
        assert_eq!(refused.err().and_then(|e| e.code()), Some(ERR_BUSY));
        let summary = control.inspect(&session).unwrap();
        assert!(summary.live, "control keeps working at the streaming limit");
        let status = control.call("host.status", json!({})).unwrap();
        assert_eq!(
            status["resources"]["connections"]["streaming"],
            MAX_STREAMING_CONNECTIONS
        );
        stop.store(true, Ordering::Release);
        for follower in followers {
            follower.join().unwrap().unwrap();
        }
        let drained = Instant::now() + Duration::from_secs(20);
        while host.streaming_connections().load(Ordering::Acquire) != 0 {
            assert!(Instant::now() < drained, "every streaming slot is released");
            std::thread::sleep(Duration::from_millis(20));
        }
        host.stop(&session, None).unwrap();
        server.stop().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn the_named_pipe_acl_has_no_world_or_authenticated_user_grant() {
        assert!(WINDOWS_PIPE_SDDL.contains(";;;OW)"));
        assert!(!WINDOWS_PIPE_SDDL.contains(";;;WD)"));
        assert!(!WINDOWS_PIPE_SDDL.contains(";;;AU)"));
        assert!(!WINDOWS_PIPE_SDDL.contains(";;;BU)"));
        assert!(!WINDOWS_PIPE_SDDL.contains(";;;SY)"));
        assert!(!WINDOWS_PIPE_SDDL.contains(";;;BA)"));
    }

    #[test]
    fn an_agent_frame_for_an_unknown_session_gets_the_session_not_found_code() {
        let (_home, _host, server) = start();
        let hello = ClientHello::control("agent-ingress-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        let error = client
            .call(
                "agent.event",
                json!({
                    "session": SessionId::new(),
                    "runtime_generation": 1,
                    "kind": "ai.stop",
                    "tool": "claude",
                    "hook_payload": {"hook_event_name": "Stop"}
                }),
            )
            .unwrap_err();
        assert_eq!(error.code(), Some(crate::protocol::ERR_SESSION_NOT_FOUND));
        server.stop().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_unix_socket_is_owner_read_write_only() {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(home.path());
        let listener = bind(&endpoint).unwrap();
        assert_eq!(
            std::fs::metadata(&endpoint).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn a_served_endpoint_is_never_taken_over_while_a_stale_one_is_reclaimed() {
        let (_home, _host, server) = start();
        let refused = bind(server.endpoint()).expect_err("a served endpoint is refused");
        assert_eq!(refused.kind(), io::ErrorKind::AddrInUse, "{refused}");
        let hello = ClientHello::local("paneflow-host-test");
        assert!(
            HostClient::connect(server.endpoint(), &hello).is_ok(),
            "the serving host keeps its endpoint"
        );

        let stale_home = tempfile::tempdir().unwrap();
        let stale = test_endpoint(stale_home.path());
        drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
        assert!(
            stale.exists(),
            "a listener that exits leaves its socket file"
        );
        drop(bind(&stale).expect("a socket nobody serves is reclaimed"));
    }

    #[test]
    fn an_unread_agent_ack_cannot_delay_notifications_or_reorder_parallel_connections() {
        let (_home, host, server) = start();
        let created = host
            .create(serde_json::from_value(shell_create_params()).unwrap())
            .unwrap();
        let session = created.manifest.session;
        let subscription = host.subscribe_agents();
        let mut first = paneflow_ipc_client::host_control::HostControl::connect(
            server.endpoint(),
            "delayed-ack",
        )
        .unwrap();
        let mut second =
            HostClient::connect(server.endpoint(), &ClientHello::control("next-ack")).unwrap();
        let params = json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.prompt_submit",
            "tool": "claude",
            "emitted_at_ms": 10,
            "hook_payload": {"hook_event_name": "UserPromptSubmit"},
        });
        first
            .write_request(METHOD_AGENT_EVENT, params.clone())
            .unwrap();
        let initial = subscription
            .frames
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(initial["revision"], 1);
        let mut next = params.clone();
        next["emitted_at_ms"] = json!(11);
        next["kind"] = json!("ai.stop");
        next["hook_payload"]["hook_event_name"] = json!("Stop");
        let ack = second.call(METHOD_AGENT_EVENT, next).unwrap();
        assert_eq!(ack["revision"], 2);
        assert_eq!(
            subscription
                .frames
                .recv_timeout(Duration::from_secs(5))
                .unwrap()["revision"],
            2
        );
        drop(first);
        let duplicate = second.call(METHOD_AGENT_EVENT, params).unwrap();
        assert_eq!(duplicate["duplicate"], true);
        assert_eq!(duplicate["revision"], 1);
        assert!(subscription.frames.try_recv().is_err());
        host.stop(&session, None).unwrap();
        server.stop().unwrap();
    }

    #[test]
    fn a_client_handshakes_creates_attaches_streams_and_stops_over_the_endpoint() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        assert_eq!(client.identity().host_instance, *host.instance());
        assert_eq!(client.identity().protocol, HOST_PROTOCOL_VERSION);

        let status = client.call("host.status", json!({})).unwrap();
        assert_eq!(status["live_sessions"], 0);
        assert!(
            status["methods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m == "session.attach")
        );

        let created = client
            .call("session.create", shell_create_params())
            .unwrap();
        let session = SessionId::parse(created["session"].as_str().unwrap()).unwrap();
        let generation: SessionGeneration =
            serde_json::from_value(created["generation"].clone()).unwrap();
        assert_eq!(generation, SessionGeneration::FIRST);
        assert_eq!(created["live"], true);
        assert_eq!(created["host_instance"], json!(host.instance()));

        let listed = client.call("session.list", json!({})).unwrap();
        assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);

        client
            .call(
                "session.input",
                json!({
                    "session": session,
                    "generation": generation,
                    "data": encode_data(b"echo HOST_WIRE_MARKER\r\n"),
                }),
            )
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut collected = Vec::new();
        let mut offset = 0;
        while Instant::now() < deadline {
            offset = client
                .output(
                    &session,
                    Some(generation),
                    offset,
                    false,
                    |_, bytes| {
                        collected.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .unwrap()
                .next_offset;
            if String::from_utf8_lossy(&collected).contains("HOST_WIRE_MARKER") {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(
            String::from_utf8_lossy(&collected).contains("HOST_WIRE_MARKER"),
            "streamed output carries the echoed marker"
        );

        let attachment = client.attach(&session, Some(generation)).unwrap();
        let checkpoint = &attachment.checkpoint;
        assert_eq!(attachment.host_instance, *host.instance());
        assert_eq!(checkpoint.generation, generation);
        assert!(checkpoint.offset >= offset.saturating_sub(64));
        assert!(!checkpoint.snapshot.is_empty());
        assert!(
            paneflow_terminal_ghostty::SnapshotDecoder::from_bytes(&checkpoint.snapshot).is_ok(),
            "the streamed checkpoint is a decodable native snapshot"
        );
        let resumed = client
            .output(
                &session,
                Some(generation),
                checkpoint.offset,
                false,
                |_, _| true,
                || true,
            )
            .unwrap();
        assert!(
            resumed.next_offset >= checkpoint.offset,
            "offsets continue from the checkpoint"
        );
        assert!(resumed.live && !resumed.stopped_by_client);

        let stale = client.call(
            "session.input",
            json!({"session": session, "generation": 2, "data": encode_data(b"x")}),
        );
        assert_eq!(
            stale.err().and_then(|e| e.code()),
            Some(ERR_GENERATION_MISMATCH)
        );
        let evicted = client.output(
            &session,
            Some(generation),
            u64::MAX / 2,
            false,
            |_, _| true,
            || true,
        );
        assert_eq!(
            evicted.err().and_then(|e| e.code()),
            Some(ERR_OUTPUT_EVICTED)
        );
        drop(client);

        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        let stopped = client
            .call(
                "session.stop",
                json!({"session": session, "generation": generation}),
            )
            .unwrap();
        assert_eq!(stopped["live"], false);
        assert_eq!(stopped["lifecycle"]["state"], "exited");
        let missing = client.call("session.inspect", json!({"session": SessionId::new()}));
        assert_eq!(
            missing.err().and_then(|e| e.code()),
            Some(ERR_SESSION_NOT_FOUND)
        );
        let unknown = client.call("session.explode", json!({}));
        assert_eq!(
            unknown.err().and_then(|e| e.code()),
            Some(ERR_METHOD_NOT_FOUND)
        );
        server.stop().unwrap();
    }

    #[test]
    fn a_follower_resumes_after_the_checkpoint_survives_idle_keepalives_and_sees_the_exit() {
        let (_home, _host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let created = control
            .create(&serde_json::from_value(shell_create_params()).unwrap())
            .unwrap();
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;

        control
            .input(&session, generation, b"echo FOLLOW_BEFORE\r\n")
            .unwrap();
        let mut before = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let attachment = control.attach(&session, Some(generation)).unwrap();
            before = attachment.checkpoint.snapshot.clone();
            let mut decoder =
                paneflow_terminal_ghostty::SnapshotDecoder::from_bytes(&before).unwrap();
            let terminal = decoder
                .decode(paneflow_terminal_ghostty::SnapshotRestore {
                    cell_width: 8,
                    cell_height: 16,
                    max_scrollback: 500,
                    appearance: paneflow_terminal_ghostty::TerminalAppearance::default(),
                })
                .unwrap();
            let text: String = terminal
                .snapshot()
                .unwrap()
                .cells
                .iter()
                .map(|cell| cell.character)
                .collect();
            if text.contains("FOLLOW_BEFORE") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!before.is_empty(), "the checkpoint captured the first echo");
        let attachment = control.attach(&session, Some(generation)).unwrap();
        let resume_from = attachment.checkpoint.offset;

        let endpoint = server.endpoint().to_path_buf();
        let follower_session = session.clone();
        let follower_hello = hello.clone();
        let follower = std::thread::spawn(move || {
            let mut client = HostClient::connect(&endpoint, &follower_hello).unwrap();
            let mut streamed = Vec::new();
            let mut offsets = Vec::new();
            let end = client
                .output(
                    &follower_session,
                    Some(generation),
                    resume_from,
                    true,
                    |offset, bytes| {
                        offsets.push(offset);
                        streamed.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .unwrap();
            (end, streamed, offsets)
        });

        std::thread::sleep(FOLLOW_KEEPALIVE + Duration::from_millis(500));
        control
            .input(&session, generation, b"echo FOLLOW_AFTER\r\n")
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let stopped = control.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);

        let (end, streamed, offsets) = follower.join().unwrap();
        assert!(!end.live, "the stream ends once the session exits");
        assert!(!end.stopped_by_client);
        assert_eq!(offsets.first().copied(), Some(resume_from));
        for pair in offsets.windows(2) {
            assert!(pair[0] < pair[1], "offsets are strictly increasing");
        }
        let text = String::from_utf8_lossy(&streamed);
        assert!(
            text.contains("FOLLOW_AFTER"),
            "bytes after the checkpoint reach the follower: {text:?}"
        );
        assert!(
            !text.contains("echo FOLLOW_BEFORE"),
            "bytes before the checkpoint are never replayed: {text:?}"
        );
        assert!(end.next_offset >= resume_from + streamed.len() as u64);
        server.stop().unwrap();
    }

    #[test]
    fn host_shutdown_refuses_while_a_session_lives_and_ends_the_server_afterwards() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        let created = client
            .call("session.create", shell_create_params())
            .unwrap();
        let session = SessionId::parse(created["session"].as_str().unwrap()).unwrap();

        let refused = client.call("host.shutdown", json!({})).unwrap_err();
        assert_eq!(refused.code(), Some(ERR_SESSION_LIVE));
        let HostClientError::Rpc {
            data: Some(data), ..
        } = refused
        else {
            panic!("the refusal lists the live sessions");
        };
        assert_eq!(data["live_sessions"].as_array().unwrap().len(), 1);
        assert_eq!(data["live_sessions"][0]["session"], json!(session));
        assert!(host.live_session_count() == 1, "nothing was stopped");

        let restarted = client.call("session.restart", json!({"session": session}));
        assert_eq!(
            restarted.err().and_then(|e| e.code()),
            Some(ERR_SESSION_LIVE)
        );
        client
            .call("session.stop", json!({"session": session}))
            .unwrap();
        let restarted = client
            .call("session.restart", json!({"session": session}))
            .unwrap();
        assert_eq!(restarted["generation"], 2);
        assert_eq!(restarted["live"], true);
        client
            .call("session.stop", json!({"session": session, "generation": 2}))
            .unwrap();

        let stopping = client.call("host.shutdown", json!({})).unwrap();
        assert_eq!(stopping["stopping"], true);
        assert_eq!(stopping["host_instance"], json!(host.instance()));
        let endpoint = server.endpoint().to_path_buf();
        server.stop().unwrap();
        assert!(
            HostClient::connect(&endpoint, &hello).is_err(),
            "the endpoint is gone once the server stopped"
        );
    }

    #[test]
    fn forced_host_shutdown_ends_live_sessions_and_stops_the_server() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        client
            .call("session.create", shell_create_params())
            .unwrap();
        assert_eq!(host.live_session_count(), 1);

        let stopping = client
            .call("host.shutdown", json!({"force": true}))
            .unwrap();
        assert_eq!(stopping["stopping"], true);
        assert_eq!(stopping["ended_sessions"], json!(1));
        assert_eq!(host.live_session_count(), 0);

        let endpoint = server.endpoint().to_path_buf();
        server.stop().unwrap();
        assert!(
            HostClient::connect(&endpoint, &hello).is_err(),
            "the endpoint is gone once the server stopped"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_second_shutdown_wakeup_is_bounded_when_an_accepted_pipe_outlives_its_listener() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = test_endpoint(home.path());
        let listener = bind(&endpoint).unwrap();
        let connected =
            Stream::connect(endpoint.as_path().to_fs_name::<GenericFilePath>().unwrap()).unwrap();
        let accepted = listener.accept().unwrap();
        drop(listener);
        let started = Instant::now();
        wake_accept_loop(&endpoint);
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(accepted);
        drop(connected);
    }

    #[test]
    fn a_client_from_another_release_attaches_when_protocol_and_engine_agree() {
        let (_home, _host, server) = start();
        let mut stale = ClientHello::local("paneflow-host-test");
        stale.build = Some("0.0.0-stale".to_string());
        let attached = HostClient::connect(server.endpoint(), &stale);
        assert!(
            attached.is_ok(),
            "the build identity is diagnostic only; protocol and engine decide: {:?}",
            attached.err()
        );

        let control = ClientHello::control("paneflow-host-test");
        assert!(
            HostClient::connect(server.endpoint(), &control).is_ok(),
            "control clients stay able to inspect and retire any host"
        );
        server.stop().unwrap();
    }

    #[test]
    fn session_remove_refuses_a_live_session_and_deletes_an_ended_manifest() {
        let (home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut client = HostClient::connect(server.endpoint(), &hello).unwrap();
        let created = client
            .call("session.create", shell_create_params())
            .unwrap();
        let session = SessionId::parse(created["session"].as_str().unwrap()).unwrap();
        let manifest = crate::manifest::manifest_path(home.path(), &session);
        assert!(manifest.is_file());

        let refused = client
            .call("session.remove", json!({"session": session}))
            .unwrap_err();
        assert_eq!(refused.code(), Some(ERR_SESSION_LIVE));
        assert!(manifest.is_file(), "a refusal never deletes the record");
        assert_eq!(host.live_session_count(), 1);

        client
            .call("session.stop", json!({"session": session}))
            .unwrap();
        let removed = client
            .call("session.remove", json!({"session": session}))
            .unwrap();
        assert_eq!(removed["removed"]["session"], json!(session));
        assert!(!manifest.exists());
        let listed = client.call("session.list", json!({})).unwrap();
        assert!(listed["sessions"].as_array().unwrap().is_empty());
        assert_eq!(
            client
                .call("session.remove", json!({"session": session}))
                .err()
                .and_then(|e| e.code()),
            Some(crate::protocol::ERR_SESSION_NOT_FOUND)
        );
        server.stop().unwrap();
    }

    #[test]
    fn an_incompatible_or_missing_handshake_is_refused_before_any_effect() {
        let (_home, host, server) = start();
        let mut hello = ClientHello::local("paneflow-host-test");
        if let Some(engine) = hello.engine.as_mut() {
            engine.source_sha = "f".repeat(40);
        }
        let Err(error) = HostClient::connect(server.endpoint(), &hello) else {
            panic!("a mismatched engine must be refused");
        };
        assert!(matches!(error, HostClientError::Incompatible(ref m) if m.contains("source_sha")));

        let mut old = ClientHello::local("paneflow-host-test");
        old.protocol = HOST_PROTOCOL_VERSION + 7;
        let Err(error) = HostClient::connect(server.endpoint(), &old) else {
            panic!("an older protocol must be refused");
        };
        assert!(matches!(error, HostClientError::Incompatible(_)));

        let mut wire = Wire::connect(server.endpoint(), protocol::MAX_CONTROL_FRAME_BYTES).unwrap();
        wire.write_json(&protocol::request(
            1,
            "session.create",
            shell_create_params(),
        ))
        .unwrap();
        let LineRead::Line(line) = wire.read_line(Duration::from_secs(5)).unwrap() else {
            panic!("expected a refusal line");
        };
        let refusal: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(refusal["error"]["code"], ERR_HANDSHAKE_REQUIRED);
        assert!(host.list(None).is_empty(), "no session was created");
        server.stop().unwrap();
    }

    #[test]
    fn an_oversized_control_frame_is_rejected_without_buffering_it() {
        let (_home, host, server) = start();
        let huge = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"host.hello\",\"params\":{{\"pad\":\"{}\"}}}}",
            "x".repeat(MAX_CONTROL_FRAME_BYTES + 1)
        );
        let mut wire = Wire::connect(server.endpoint(), protocol::MAX_CONTROL_FRAME_BYTES).unwrap();
        assert!(
            wire.write_line(huge.as_bytes()).is_err(),
            "the client side refuses to emit a frame above the limit"
        );
        drop(wire);

        let mut raw = Wire::connect(server.endpoint(), protocol::MAX_CONTROL_FRAME_BYTES).unwrap();
        let mut payload = huge.into_bytes();
        payload.push(b'\n');
        raw.write_raw(&payload).unwrap();
        let LineRead::Line(line) = raw.read_line(Duration::from_secs(5)).unwrap() else {
            panic!("expected a refusal line");
        };
        let refusal: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(refusal["error"]["code"], ERR_FRAME_TOO_LARGE);
        assert!(host.list(None).is_empty());
        server.stop().unwrap();
    }

    fn shell_line(windows: &str, unix: &str) -> Vec<u8> {
        if cfg!(windows) {
            format!("{windows}\r\n").into_bytes()
        } else {
            format!("{unix}\n").into_bytes()
        }
    }

    fn visible_text(raw: &str) -> String {
        let mut visible = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                visible.push(c);
                continue;
            }
            match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    let mut previous = '\0';
                    for c in chars.by_ref() {
                        if c == '\u{7}' || (previous == '\u{1b}' && c == '\\') {
                            break;
                        }
                        previous = c;
                    }
                }
                _ => {}
            }
        }
        visible
    }

    fn wait_until(budget: Duration, mut check: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + budget;
        loop {
            if check() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn create_shell(client: &mut HostClient) -> (SessionId, SessionGeneration) {
        let created = client
            .create(&serde_json::from_value(shell_create_params()).unwrap())
            .unwrap();
        (created.manifest.session, created.manifest.generation)
    }

    fn read_until_marker(
        client: &mut HostClient,
        session: &SessionId,
        generation: SessionGeneration,
        offset: &mut u64,
        marker: &str,
        budget: Duration,
    ) -> String {
        let mut collected = Vec::new();
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            *offset = client
                .output(
                    session,
                    Some(generation),
                    *offset,
                    false,
                    |_, bytes| {
                        collected.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .unwrap()
                .next_offset;
            if String::from_utf8_lossy(&collected).contains(marker) {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        String::from_utf8_lossy(&collected).into_owned()
    }

    fn wait_ended(host: &SessionHost, session: &SessionId, budget: Duration) -> SessionSummary {
        assert!(
            wait_until(budget, || host.inspect(session).is_ok_and(|summary| {
                !summary.live
                    && !summary.pending_launch
                    && !matches!(
                        summary.manifest.lifecycle,
                        SessionLifecycle::Starting | SessionLifecycle::Running
                    )
            })),
            "the session ends within {budget:?}"
        );
        host.inspect(session).unwrap()
    }

    fn spawn_follower(
        endpoint: PathBuf,
        hello: ClientHello,
        session: SessionId,
        generation: SessionGeneration,
        from: u64,
        pause_first_frame: Option<Duration>,
    ) -> JoinHandle<Result<(OutputEnd, String), HostClientError>> {
        std::thread::spawn(move || {
            let mut client = HostClient::connect(&endpoint, &hello)?;
            let mut streamed = Vec::new();
            let mut paused = pause_first_frame;
            let end = client.output(
                &session,
                Some(generation),
                from,
                true,
                |_, bytes| {
                    if let Some(pause) = paused.take() {
                        std::thread::sleep(pause);
                    }
                    streamed.extend_from_slice(bytes);
                    true
                },
                || true,
            )?;
            Ok((end, String::from_utf8_lossy(&streamed).into_owned()))
        })
    }

    #[test]
    fn an_output_flood_reaches_every_follower_and_a_paused_follower_never_stalls_control() {
        let (_home, _host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo READY-MARK", "echo READY-MARK"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "READY-MARK",
            Duration::from_secs(15),
        );
        let from = control
            .attach(&session, Some(generation))
            .unwrap()
            .checkpoint
            .offset;

        let followers: Vec<_> = [None, None, Some(Duration::from_millis(1500))]
            .into_iter()
            .map(|pause| {
                spawn_follower(
                    server.endpoint().to_path_buf(),
                    hello.clone(),
                    session.clone(),
                    generation,
                    from,
                    pause,
                )
            })
            .collect();
        std::thread::sleep(Duration::from_millis(200));

        control
            .input(
                &session,
                generation,
                &shell_line(
                    "for /L %i in (1,1,6000) do @echo FLOOD-LINE-%i",
                    "i=0; while [ $i -lt 6000 ]; do i=$((i+1)); echo FLOOD-LINE-$i; done",
                ),
            )
            .unwrap();
        let mut slowest = Duration::ZERO;
        for _ in 0..10 {
            let started = Instant::now();
            control.inspect(&session).unwrap();
            slowest = slowest.max(started.elapsed());
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            slowest < Duration::from_secs(2),
            "US-006: a paused follower must not hold the control path, slowest inspect {slowest:?}"
        );
        control
            .input(
                &session,
                generation,
                &shell_line("echo FLOOD-END", "echo FLOOD-END"),
            )
            .expect("the control path accepts input after the flood");
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "FLOOD-END",
            Duration::from_secs(30),
        );
        let stopped = control.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);

        for (index, follower) in followers.into_iter().enumerate() {
            match follower.join().unwrap() {
                Ok((end, text)) => {
                    assert!(!end.live, "follower {index} sees the end of the stream");
                    assert!(
                        text.contains("FLOOD-LINE-6000") && text.contains("FLOOD-END"),
                        "follower {index} received the whole flood"
                    );
                }
                Err(error) => assert_eq!(
                    error.code(),
                    Some(ERR_OUTPUT_EVICTED),
                    "follower {index} is told explicitly when the tail was evicted: {error}"
                ),
            }
        }
        server.stop().unwrap();
    }

    #[test]
    fn an_exit_without_output_ends_the_follower_and_leaves_an_empty_completed_text() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q", "/D", "/C", "exit 0"]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", vec!["-c", "exit 0"]);
        let created = control
            .create(
                &serde_json::from_value(json!({
                    "shell": shell,
                    "args": args,
                    "cwd": std::env::temp_dir().display().to_string(),
                    "cols": 80,
                    "rows": 24,
                }))
                .unwrap(),
            )
            .unwrap();
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;
        let follower = spawn_follower(
            server.endpoint().to_path_buf(),
            hello.clone(),
            session.clone(),
            generation,
            0,
            None,
        );
        let summary = wait_ended(&host, &session, Duration::from_secs(15));
        assert!(matches!(
            summary.manifest.lifecycle,
            SessionLifecycle::Exited { code: 0, .. }
        ));
        let (end, text) = follower.join().unwrap().unwrap();
        assert!(!end.live);
        assert!(!end.stopped_by_client);
        assert!(
            visible_text(&text).trim().is_empty(),
            "nothing was fabricated for a silent exit: {text:?}"
        );
        assert!(
            wait_until(Duration::from_secs(5), || matches!(
                host.output_stream(&session, None),
                Ok(Err(_))
            )),
            "NFR-04: the runtime is released within 5 s of the exit"
        );

        let final_text = control
            .call("session.text", json!({"session": session}))
            .unwrap();
        assert_eq!(final_text["live"], false);
        assert_eq!(final_text["text"], "");
        assert_eq!(final_text["complete"], true);
        server.stop().unwrap();
    }

    #[test]
    fn a_natural_exit_keeps_a_cold_record_releases_the_runtime_and_removal_drops_the_text() {
        let (home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo NATURAL_MA^RKER", "echo NATURAL_MA''RKER"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "NATURAL_MARKER",
            Duration::from_secs(15),
        );
        control
            .input(&session, generation, &shell_line("exit", "exit"))
            .unwrap();
        let summary = wait_ended(&host, &session, Duration::from_secs(15));
        assert!(
            matches!(summary.manifest.lifecycle, SessionLifecycle::Exited { .. }),
            "a natural exit is recorded as exited: {:?}",
            summary.manifest.lifecycle
        );
        assert!(
            wait_until(Duration::from_secs(5), || matches!(
                host.output_stream(&session, None),
                Ok(Err(_))
            )),
            "NFR-04: a completed session keeps no runtime, tail, or terminal"
        );
        let final_output = host
            .inspect(&session)
            .unwrap()
            .manifest
            .final_output
            .expect("a completed record carries its final output metadata");
        assert!(
            final_output.complete,
            "the PTY reached EOF before the drain budget"
        );
        assert!(final_output.text_available);
        assert!(final_output.text_bytes as usize <= crate::runtime::FINAL_TEXT_MAX_BYTES);
        let cold = crate::cold_text::path(home.path(), &session);
        assert!(
            wait_until(Duration::from_secs(5), || cold.is_file()),
            "the final text is kept as a cold file"
        );

        let final_text = control
            .call("session.text", json!({"session": session}))
            .unwrap();
        assert_eq!(final_text["live"], false, "{final_text}");
        assert_eq!(final_text["available"], true, "{final_text}");
        assert_eq!(final_text["complete"], true, "{final_text}");
        assert!(
            final_text["text"]
                .as_str()
                .unwrap()
                .contains("NATURAL_MARKER"),
            "the retained text is the final screen: {final_text}"
        );
        let refused = control.attach(&session, Some(generation));
        assert!(
            refused.is_err(),
            "opening an ended record never attaches or launches"
        );
        let ended = control
            .output(
                &session,
                Some(generation),
                final_output.offset,
                true,
                |_, _| true,
                || true,
            )
            .unwrap();
        assert!(
            !ended.live,
            "a follower of a completed record ends immediately"
        );

        control.remove(&session).unwrap();
        assert!(!cold.exists(), "removal releases the cold text");
        assert!(matches!(
            host.text(&session),
            Err(HostError::SessionNotFound(_))
        ));
        server.stop().unwrap();
    }

    #[test]
    fn a_stop_racing_a_natural_exit_settles_on_a_single_confirmed_exit() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let mut stopper = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo RACE-READY", "echo RACE-READY"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "RACE-READY",
            Duration::from_secs(15),
        );
        control
            .input(&session, generation, &shell_line("exit", "exit"))
            .unwrap();
        let stopped = stopper.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);
        assert!(matches!(
            stopped.manifest.lifecycle,
            SessionLifecycle::Exited { .. }
        ));
        let summary = wait_ended(&host, &session, Duration::from_secs(15));
        assert_eq!(summary.manifest.generation, generation);
        assert_eq!(summary.descendants_unresolved, 0);
        assert!(
            wait_until(Duration::from_secs(5), || host
                .inspect(&session)
                .unwrap()
                .manifest
                .final_output
                .is_some()),
            "exactly one completed record follows the racing stop"
        );
        assert!(
            wait_until(Duration::from_secs(5), || matches!(
                host.output_stream(&session, None),
                Ok(Err(_))
            )),
            "the runtime is released after the race"
        );
        server.stop().unwrap();
    }

    #[test]
    fn a_held_open_descendant_bounds_the_final_drain_and_marks_the_output_incomplete() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo HOLD-READY", "echo HOLD-READY"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "HOLD-READY",
            Duration::from_secs(15),
        );
        control
            .input(
                &session,
                generation,
                &shell_line("start /b ping -n 8 127.0.0.1 >nul", "sleep 7 &"),
            )
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let exit_sent_at = Instant::now();
        control
            .input(&session, generation, &shell_line("exit", "exit"))
            .unwrap();
        assert!(
            wait_until(Duration::from_secs(10), || host
                .inspect(&session)
                .unwrap()
                .manifest
                .final_output
                .is_some()),
            "US-007: the final drain is bounded even while a descendant holds the PTY"
        );
        let drained_after = exit_sent_at.elapsed();
        assert!(
            drained_after < Duration::from_secs(6),
            "the drain budget, not the descendant, decides when the record completes: {drained_after:?}"
        );
        let final_text = control
            .call("session.text", json!({"session": session}))
            .unwrap();
        assert_eq!(final_text["live"], false);
        if cfg!(target_os = "linux") {
            assert_eq!(
                final_text["complete"], false,
                "output cut by the drain budget is reported incomplete"
            );
        }
        let summary = host.inspect(&session).unwrap();
        assert!(
            !summary.live,
            "the session is no longer live once the shell exited"
        );
        assert!(
            wait_until(Duration::from_secs(20), || matches!(
                host.inspect(&session).unwrap().manifest.lifecycle,
                SessionLifecycle::Exited { .. }
            )),
            "the exit is confirmed once the descendant is gone"
        );
        assert!(
            wait_until(Duration::from_secs(5), || matches!(
                host.output_stream(&session, None),
                Ok(Err(_))
            )),
            "NFR-04: the runtime is released within 5 s of the confirmed exit"
        );
        server.stop().unwrap();
    }

    #[test]
    fn a_lost_data_directory_reports_unavailable_text_without_blocking_retirement() {
        let (home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo LOST-DIR", "echo LOST-DIR"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "LOST-DIR",
            Duration::from_secs(15),
        );
        let data_dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        std::fs::remove_dir_all(&data_dir).unwrap();
        control
            .input(&session, generation, &shell_line("exit", "exit"))
            .unwrap();
        wait_ended(&host, &session, Duration::from_secs(15));
        assert!(
            wait_until(Duration::from_secs(5), || matches!(
                host.output_stream(&session, None),
                Ok(Err(_))
            )),
            "US-009: a durability failure still retires the runtime"
        );
        assert!(
            wait_until(Duration::from_secs(5), || {
                let final_output = host
                    .inspect(&session)
                    .unwrap()
                    .manifest
                    .final_output
                    .expect("the completed record is still committed");
                !final_output.text_available
            }),
            "US-009: a lost data directory withdraws the final text"
        );
        let final_text = control
            .call("session.text", json!({"session": session}))
            .unwrap();
        assert_eq!(final_text["live"], false);
        assert_eq!(final_text["available"], false);
        assert_eq!(final_text["text"], "");
        server.stop().unwrap();
    }

    #[test]
    fn records_past_their_retention_release_their_cold_text_under_an_injected_clock() {
        let (home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let mut sessions = Vec::new();
        for index in 0..3 {
            let (session, generation) = create_shell(&mut control);
            let mut offset = 0;
            let marker = format!("CHURN-{index}");
            control
                .input(
                    &session,
                    generation,
                    &shell_line(&format!("echo {marker}"), &format!("echo {marker}")),
                )
                .unwrap();
            read_until_marker(
                &mut control,
                &session,
                generation,
                &mut offset,
                &marker,
                Duration::from_secs(15),
            );
            control
                .input(&session, generation, &shell_line("exit", "exit"))
                .unwrap();
            sessions.push(session);
        }
        for session in &sessions {
            wait_ended(&host, session, Duration::from_secs(15));
            assert!(
                wait_until(Duration::from_secs(5), || crate::cold_text::path(
                    home.path(),
                    session
                )
                .is_file()),
                "each churned session leaves a cold file"
            );
        }
        let now = now_ms();
        host.trim_terminated_records_at(now + crate::host::FINISHED_RECORD_MAX_AGE_MS / 2);
        assert_eq!(
            host.list(None).len(),
            3,
            "young records survive maintenance"
        );
        host.trim_terminated_records_at(now + crate::host::FINISHED_RECORD_MAX_AGE_MS + 60_000);
        assert!(host.list(None).is_empty(), "aged records are dropped");
        for session in &sessions {
            assert!(
                !crate::cold_text::path(home.path(), session).exists(),
                "the cold cache is released with the record"
            );
            assert!(matches!(
                host.text(session),
                Err(HostError::SessionNotFound(_))
            ));
        }
        assert_eq!(
            crate::cold_text::enforce_budget(home.path(), crate::cold_text::COLD_TEXT_BUDGET_BYTES),
            0
        );
        server.stop().unwrap();
    }

    fn idle_control_connection_delivers_the_first_key_exactly_once(idle: Duration) {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo IDLE-READY", "echo IDLE-READY"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "IDLE-READY",
            Duration::from_secs(15),
        );
        let from = control
            .attach(&session, Some(generation))
            .unwrap()
            .checkpoint
            .offset;
        let follower = spawn_follower(
            server.endpoint().to_path_buf(),
            hello.clone(),
            session.clone(),
            generation,
            from,
            None,
        );
        std::thread::sleep(idle);
        control
            .input(
                &session,
                generation,
                &shell_line("echo IDLE_RES^ULT", "echo IDLE_RES''ULT"),
            )
            .unwrap();
        let text = read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "IDLE_RESULT",
            Duration::from_secs(15),
        );
        assert_eq!(
            visible_text(&text).matches("IDLE_RESULT").count(),
            1,
            "US-010: the first key after {idle:?} idle is delivered exactly once: {text:?}"
        );
        let stopped = control.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);
        let (end, streamed) = follower.join().unwrap().unwrap();
        assert!(
            !end.live,
            "the follower outlives the idle period on keepalives"
        );
        assert_eq!(
            visible_text(&streamed).matches("IDLE_RESULT").count(),
            1,
            "the follower saw the single delivery: {streamed:?}"
        );
        wait_ended(&host, &session, Duration::from_secs(15));
        server.stop().unwrap();
    }

    #[test]
    fn repeated_disconnects_deliver_each_input_once_and_release_every_connection_thread() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo RECONNECT-READY", "echo RECONNECT-READY"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "RECONNECT-READY",
            Duration::from_secs(15),
        );
        let baseline = server.active_connections();
        let rounds = 12;
        for round in 0..rounds {
            let mut peer = HostClient::connect(server.endpoint(), &hello).unwrap();
            peer.input(
                &session,
                generation,
                &shell_line(
                    &format!("echo RC-{round}-MA^RK"),
                    &format!("echo RC-{round}-MA''RK"),
                ),
            )
            .unwrap();
            drop(peer);
        }
        let text = read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            &format!("RC-{}-MARK", rounds - 1),
            Duration::from_secs(30),
        );
        let visible = visible_text(&text);
        for round in 0..rounds {
            assert_eq!(
                visible.matches(&format!("RC-{round}-MARK")).count(),
                1,
                "US-010: input sent over a dropped control connection is delivered once: {visible:?}"
            );
        }
        assert!(
            wait_until(Duration::from_secs(5), || server.active_connections()
                <= baseline),
            "US-010: dropped control connections release their host threads, {} still open",
            server.active_connections()
        );
        let stopped = control.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);
        wait_ended(&host, &session, Duration::from_secs(15));
        server.stop().unwrap();
    }

    #[test]
    fn a_connection_lost_before_the_input_ack_reports_unknown_delivery_without_a_resend() {
        let (_home, host, server) = start();
        let hello = ClientHello::local("paneflow-host-test");
        let mut control = HostClient::connect(server.endpoint(), &hello).unwrap();
        let (session, generation) = create_shell(&mut control);
        let mut offset = 0;
        control
            .input(
                &session,
                generation,
                &shell_line("echo PASTE-READY", "echo PASTE-READY"),
            )
            .unwrap();
        read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "PASTE-READY",
            Duration::from_secs(15),
        );
        let mut pasting = HostClient::connect(server.endpoint(), &hello).unwrap();
        let paste = shell_line("echo PASTE-DELIV^ERED", "echo PASTE-DELIV''ERED");
        DROP_CONNECTION_AFTER_INPUT.store(true, Ordering::Release);
        let error = pasting
            .input(&session, generation, &paste)
            .expect_err("the peer vanished before acknowledging the paste");
        assert!(
            error.is_connection_loss(),
            "a missing acknowledgement is a connection loss, not a refusal: {error}"
        );
        assert!(
            !DROP_CONNECTION_AFTER_INPUT.load(Ordering::Acquire),
            "the fault fired exactly once"
        );
        let text = read_until_marker(
            &mut control,
            &session,
            generation,
            &mut offset,
            "PASTE-DELIVERED",
            Duration::from_secs(15),
        );
        assert_eq!(
            visible_text(&text).matches("PASTE-DELIVERED").count(),
            1,
            "US-010: the bytes the host read before the loss ran exactly once and were never resent: {text:?}"
        );
        drop(pasting);
        let stopped = control.stop(&session, Some(generation)).unwrap();
        assert!(!stopped.live);
        wait_ended(&host, &session, Duration::from_secs(15));
        server.stop().unwrap();
    }

    #[test]
    fn a_control_connection_idle_for_over_a_minute_still_delivers_the_first_key_once() {
        let idle = std::env::var("PANEFLOW_TEST_IDLE_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(61);
        idle_control_connection_delivers_the_first_key_exactly_once(Duration::from_secs(idle));
    }

    #[test]
    #[ignore = "five minute idle soak; run explicitly"]
    fn a_control_connection_idle_for_five_minutes_still_delivers_the_first_key_once() {
        idle_control_connection_delivers_the_first_key_exactly_once(Duration::from_secs(300));
    }

    #[test]
    #[ignore = "thirty minute idle soak; run explicitly"]
    fn a_control_connection_idle_for_thirty_minutes_still_delivers_the_first_key_once() {
        idle_control_connection_delivers_the_first_key_exactly_once(Duration::from_secs(1800));
    }

    #[test]
    fn a_headless_transport_reads_the_listed_surface_across_its_per_call_connections() {
        use paneflow_ipc_client::IpcTransport;
        use paneflow_ipc_client::host_control::HostTransport;

        let (_home, host, server) = start();
        let mut sessions = Vec::new();
        for marker in ["HEADLESS_FIRST", "HEADLESS_SECOND"] {
            let created = host
                .create(serde_json::from_value(shell_create_params()).unwrap())
                .unwrap();
            let (session, generation) = (created.manifest.session, created.manifest.generation);
            let command = format!("echo {marker}");
            host.input(&session, Some(generation), shell_line(&command, &command))
                .unwrap();
            sessions.push((session, generation, marker));
        }
        for (session, _, marker) in &sessions {
            assert!(
                wait_until(Duration::from_secs(20), || host
                    .text(session)
                    .is_ok_and(|text| visible_text(&text.text).contains(marker))),
                "{marker} reaches its own session"
            );
        }

        let transport = HostTransport::connect(server.endpoint(), "headless-test").unwrap();
        let listed = transport.call("surface.list", json!({})).unwrap();
        let (second, second_generation, _) = &sessions[1];
        let alias = listed["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|surface| surface["session"] == json!(second))
            .and_then(|surface| surface["surface_id"].as_u64())
            .expect("the second session is listed");

        let read = transport
            .call(
                "surface.read",
                json!({"surface_id": alias, "fenced": false}),
            )
            .expect("a listed surface reads on the next per-call connection");
        assert_eq!(read["session"], json!(second));
        let text = visible_text(read["text"].as_str().unwrap());
        assert!(text.contains("HEADLESS_SECOND"), "{text}");
        assert!(!text.contains("HEADLESS_FIRST"), "{text}");

        host.stop(second, Some(*second_generation)).unwrap();
        host.remove(second).unwrap();
        let vanished = transport
            .call("surface.read", json!({"surface_id": alias}))
            .expect_err("a session gone since surface.list is not read");
        assert!(vanished.contains("Surface not found"), "{vanished}");

        host.stop(&sessions[0].0, None).unwrap();
        server.stop().unwrap();
    }
}
