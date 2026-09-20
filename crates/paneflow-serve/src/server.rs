use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::{GenericFilePath, Listener, ListenerOptions, Stream, prelude::*};
use paneflow_ipc_client::line_wire::{LineRead, Wire};
use serde_json::{Value, json};

use crate::protocol::{
    DEFAULT_ACTIVITY_LOG_LIMIT, ERR_BUSY, ERR_FRAME_TOO_LARGE, ERR_HANDSHAKE_REQUIRED,
    ERR_INTERNAL, ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE, MAX_CONTROL_FRAME_BYTES,
    METHOD_AGENT_ACKNOWLEDGE, METHOD_AGENT_ACTIVITY_LOG, METHOD_AGENT_FOLLOW,
    METHOD_AGENT_SNAPSHOT, METHOD_HOST_HELLO, METHOD_WORKER_HELLO, METHOD_WORKER_SHUTDOWN,
    METHOD_WORKER_STATUS, WorkerIdentity, error_envelope, result_envelope,
};
use crate::state::WorkerState;

const MAX_CONNECTIONS: usize = 128;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FOLLOW_POLL: Duration = Duration::from_millis(15);
const FOLLOW_KEEPALIVE: Duration = Duration::from_secs(2);
const SUBSCRIBER_QUEUE_SLOTS: usize = 256;

#[cfg(windows)]
const WINDOWS_PIPE_SDDL: &str = "D:P(A;;GA;;;OW)";

pub const PROXIED_PREFIXES: &[&str] = &["session.", "surface.", "host.", "system."];

pub const WORKER_ANSWERED: &[&str] = &[
    METHOD_WORKER_HELLO,
    METHOD_WORKER_STATUS,
    METHOD_WORKER_SHUTDOWN,
    METHOD_AGENT_SNAPSHOT,
    METHOD_AGENT_FOLLOW,
    METHOD_AGENT_ACTIVITY_LOG,
    METHOD_AGENT_ACKNOWLEDGE,
    "fleet.list",
    "surface.status",
];

pub fn is_proxied(method: &str) -> bool {
    if WORKER_ANSWERED.contains(&method) {
        return false;
    }
    PROXIED_PREFIXES
        .iter()
        .any(|prefix| method.starts_with(prefix))
}

pub struct Subscription {
    pub id: u64,
    pub frames: Receiver<Value>,
}

#[derive(Default)]
pub struct Bus {
    next_id: AtomicU64,
    subscribers: Mutex<Vec<(u64, SyncSender<Value>)>>,
}

impl Bus {
    pub fn subscribe(&self) -> Subscription {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, frames) = sync_channel(SUBSCRIBER_QUEUE_SLOTS);
        self.lock().push((id, tx));
        Subscription { id, frames }
    }

    pub fn unsubscribe(&self, id: u64) {
        self.lock().retain(|(held, _)| *held != id);
    }

    pub fn broadcast(&self, frame: &Value) {
        let mut subscribers = self.lock();
        subscribers.retain(|(id, tx)| match tx.try_send(frame.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                log::warn!("paneflow-serve: subscriber {id} fell behind and was dropped");
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(u64, SyncSender<Value>)>> {
        self.subscribers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

pub struct Worker {
    pub identity: WorkerIdentity,
    pub home: PathBuf,
    pub core_endpoint: PathBuf,
    pub state: Mutex<WorkerState>,
    pub bus: Bus,
    pub shutdown: Arc<AtomicBool>,
    pub core_connected: AtomicBool,
}

impl Worker {
    pub fn lock_state(&self) -> std::sync::MutexGuard<'_, WorkerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn snapshot_frame(&self) -> Value {
        json!({
            "worker": self.identity,
            "capabilities": self.identity.capabilities,
            "core_connected": self.core_connected.load(Ordering::Acquire),
            "sessions": self.lock_state().snapshot(),
        })
    }

    pub fn status_frame(&self) -> Value {
        json!({
            "pid": self.identity.pid,
            "protocol": self.identity.protocol,
            "home": self.identity.home,
            "endpoint": self.identity.endpoint,
            "version": self.identity.version,
            "session_count": self.lock_state().len(),
            "core_connected": self.core_connected.load(Ordering::Acquire),
            "capabilities": self.identity.capabilities,
        })
    }

    pub fn publish(&self, projection: &crate::state::Projection, source: &Value) {
        let session = &projection.session;
        self.bus.broadcast(&json!({
            "type": "event",
            "session": session["session"],
            "kind": source["kind"],
            "tool": source["tool"],
            "pid": source["pid"],
            "tool_name": source["tool_name"],
            "exit_code": source["exit_code"],
            "emitted_at_ms": source["emitted_at_ms"],
            "event_source": source["event_source"],
            "hook_payload": source["hook_payload"],
            "agent": session["activity"],
            "activity_source": session["activity_source"],
            "status": session["status"],
            "outcome": session["outcome"],
            "runtime_id": session["runtime_id"],
            "unread": session["unread"],
            "updated_at_ms": session["updated_at_ms"],
            "notify": projection
                .notification
                .as_ref()
                .map(crate::notifications::Notification::to_value),
        }));
    }

    fn acknowledge(&self, params: &Value) -> Value {
        let requested: Vec<paneflow_config::schema::SessionId> = params["sessions"]
            .as_array()
            .map(|entries| entries.iter().filter_map(Value::as_str).collect::<Vec<_>>())
            .or_else(|| params["session"].as_str().map(|one| vec![one]))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|raw| paneflow_config::schema::SessionId::parse(raw).ok())
            .collect();
        let projections = self.lock_state().acknowledge(&requested);
        let acknowledged: Vec<Value> = projections
            .iter()
            .map(|projection| projection.session["session"].clone())
            .collect();
        for projection in &projections {
            self.publish(projection, &json!({}));
        }
        json!({"acknowledged": acknowledged})
    }

    fn fleet_list(&self) -> Value {
        let now = crate::state::now_ms();
        let agents: Vec<Value> = self
            .lock_state()
            .entries()
            .enumerate()
            .filter_map(|(index, entry)| {
                let activity = entry.activity.as_ref()?;
                Some(json!({
                    "pid": activity.pid,
                    "tool": activity.tool,
                    "state": activity.state,
                    "hooked": true,
                    "stale": activity.stale,
                    "reason": Value::Null,
                    "surface_id": index as u64 + 1,
                    "surface_name": entry.title.clone().unwrap_or_else(|| entry.session.to_string()),
                    "session": entry.session,
                    "workspace": entry.workspace,
                    "active_tool_name": activity.active_tool_name,
                    "message": activity.message,
                    "last_result": activity.last_result,
                    "waiting_ms": activity
                        .waiting_since_ms
                        .map(|since| now.saturating_sub(since)),
                    "idle_ms": now.saturating_sub(activity.updated_at_ms),
                    "activity_source": entry.activity_source.wire_str(),
                    "reduced_by": "paneflow-serve",
                }))
            })
            .collect();
        json!({"agents": agents})
    }
}

pub struct ServerHandle {
    endpoint: PathBuf,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl ServerHandle {
    pub fn spawn(worker: Arc<Worker>, endpoint: PathBuf) -> io::Result<Self> {
        let listener = bind(&endpoint)?;
        let shutdown = Arc::clone(&worker.shutdown);
        let flag = Arc::clone(&shutdown);
        let thread = std::thread::Builder::new()
            .name("paneflow-serve-accept".into())
            .spawn(move || accept_loop(listener, worker, flag))?;
        Ok(Self {
            endpoint,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }

    pub fn stop(mut self) -> io::Result<()> {
        self.shutdown.store(true, Ordering::Release);
        if let Ok(name) = self.endpoint.as_path().to_fs_name::<GenericFilePath>() {
            let _ = Stream::connect(name);
        }
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| io::Error::other("the worker accept thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.shutdown.store(true, Ordering::Release);
            if let Ok(name) = self.endpoint.as_path().to_fs_name::<GenericFilePath>() {
                let _ = Stream::connect(name);
            }
            let _ = thread.join();
        }
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&self.endpoint);
        }
    }
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
    log::info!("paneflow-serve: listening on {}", endpoint.display());
    Ok(listener)
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

fn accept_loop(
    listener: Listener,
    worker: Arc<Worker>,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    for connection in listener.incoming() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let stream = match connection {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("paneflow-serve: accept failed: {error}");
                continue;
            }
        };
        let Some(guard) = ConnectionGuard::acquire(Arc::clone(&active)) else {
            if let Ok(mut wire) = Wire::new(stream, MAX_CONTROL_FRAME_BYTES) {
                let _ = wire.write_json(&error_envelope(
                    &Value::Null,
                    ERR_BUSY,
                    "worker busy: too many concurrent connections",
                    None,
                ));
            }
            continue;
        };
        let worker = Arc::clone(&worker);
        let shutdown = Arc::clone(&shutdown);
        let spawned = std::thread::Builder::new()
            .name("paneflow-serve-conn".into())
            .spawn(move || {
                let _guard = guard;
                match Wire::new(stream, MAX_CONTROL_FRAME_BYTES) {
                    Ok(wire) => handle_connection(wire, worker, shutdown),
                    Err(error) => log::debug!("paneflow-serve: connection setup failed: {error}"),
                }
            });
        if let Err(error) = spawned {
            log::warn!("paneflow-serve: cannot start a connection thread: {error}");
        }
    }
    Ok(())
}

enum Flow {
    Continue,
    Close,
}

fn handle_connection(mut wire: Wire, worker: Arc<Worker>, shutdown: Arc<AtomicBool>) {
    let mut greeted = false;
    loop {
        if shutdown.load(Ordering::Acquire) {
            return;
        }
        let line = match wire.read_line(IDLE_TIMEOUT) {
            Ok(LineRead::Line(line)) => line,
            Ok(LineRead::Eof) => return,
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
            let _ = wire.write_json(&error_envelope(
                &id,
                ERR_INVALID_REQUEST,
                "missing method",
                None,
            ));
            return;
        };
        let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
        if !greeted {
            if method != METHOD_WORKER_HELLO && method != METHOD_HOST_HELLO {
                let _ = wire.write_json(&error_envelope(
                    &id,
                    ERR_HANDSHAKE_REQUIRED,
                    "worker.hello must be the first request on a connection",
                    None,
                ));
                return;
            }
            greeted = true;
            let identity = serde_json::to_value(&worker.identity).unwrap_or_else(|_| json!({}));
            if wire.write_json(&result_envelope(&id, identity)).is_err() {
                return;
            }
            continue;
        }
        let flow = match method {
            METHOD_AGENT_FOLLOW => stream_follow(&mut wire, &worker, &shutdown, &id),
            METHOD_WORKER_SHUTDOWN => {
                let answered = wire.write_json(&result_envelope(
                    &id,
                    json!({"stopping": true, "pid": worker.identity.pid}),
                ));
                worker.shutdown.store(true, Ordering::Release);
                let _ = answered;
                Flow::Close
            }
            _ => {
                let envelope = match dispatch(&worker, method, &params) {
                    Ok(result) => result_envelope(&id, result),
                    Err(DispatchError::MethodNotFound) => error_envelope(
                        &id,
                        ERR_METHOD_NOT_FOUND,
                        format!("method not handled by the worker: {method}"),
                        None,
                    ),
                    Err(DispatchError::Core(message)) => {
                        error_envelope(&id, ERR_INTERNAL, message, None)
                    }
                };
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

enum DispatchError {
    MethodNotFound,
    Core(String),
}

fn dispatch(worker: &Worker, method: &str, params: &Value) -> Result<Value, DispatchError> {
    match method {
        METHOD_WORKER_HELLO | METHOD_HOST_HELLO => {
            Ok(serde_json::to_value(&worker.identity).unwrap_or_else(|_| json!({})))
        }
        METHOD_WORKER_STATUS => Ok(worker.status_frame()),
        METHOD_AGENT_SNAPSHOT => Ok(worker.snapshot_frame()),
        METHOD_AGENT_ACTIVITY_LOG => {
            let limit = params["limit"]
                .as_u64()
                .and_then(|limit| usize::try_from(limit).ok())
                .filter(|limit| *limit > 0)
                .unwrap_or(DEFAULT_ACTIVITY_LOG_LIMIT);
            Ok(json!({"entries": worker.lock_state().activity_log().to_values(limit)}))
        }
        METHOD_AGENT_ACKNOWLEDGE => Ok(worker.acknowledge(params)),
        "fleet.list" => Ok(worker.fleet_list()),
        "surface.status" => {
            let proxied = crate::core_link::call_core(&worker.core_endpoint, method, params)
                .map_err(DispatchError::Core)?;
            Ok(enrich_status(worker, proxied, params))
        }
        other if is_proxied(other) => {
            crate::core_link::call_core(&worker.core_endpoint, other, params)
                .map_err(DispatchError::Core)
        }
        _ => Err(DispatchError::MethodNotFound),
    }
}

fn enrich_status(worker: &Worker, mut proxied: Value, params: &Value) -> Value {
    let session = params["session"]
        .as_str()
        .and_then(|raw| paneflow_config::schema::SessionId::parse(raw).ok())
        .or_else(|| {
            proxied["session"]
                .as_str()
                .and_then(|raw| paneflow_config::schema::SessionId::parse(raw).ok())
        });
    let state = worker.lock_state();
    let entry = session.as_ref().and_then(|session| state.get(session));
    let Some(entry) = entry else {
        return proxied;
    };
    let Some(map) = proxied.as_object_mut() else {
        return proxied;
    };
    map.insert(
        "activity_source".to_string(),
        Value::from(entry.activity_source.wire_str()),
    );
    map.insert("reduced_by".to_string(), Value::from("paneflow-serve"));
    if let Some(activity) = entry.activity.as_ref() {
        let now = crate::state::now_ms();
        map.insert("state".to_string(), Value::from(activity.state.clone()));
        map.insert("stale".to_string(), Value::from(activity.stale));
        map.insert("tool".to_string(), Value::from(activity.tool.clone()));
        map.insert(
            "message".to_string(),
            activity.message.clone().map_or(Value::Null, Value::from),
        );
        map.insert(
            "last_result".to_string(),
            activity
                .last_result
                .clone()
                .map_or(Value::Null, Value::from),
        );
        map.insert(
            "waiting_ms".to_string(),
            activity
                .waiting_since_ms
                .map_or(Value::Null, |since| Value::from(now.saturating_sub(since))),
        );
        map.insert(
            "idle_ms".to_string(),
            Value::from(now.saturating_sub(activity.updated_at_ms)),
        );
    } else {
        map.insert("state".to_string(), Value::from("idle"));
    }
    proxied
}

fn stream_follow(wire: &mut Wire, worker: &Worker, shutdown: &AtomicBool, id: &Value) -> Flow {
    let subscription = worker.bus.subscribe();
    let mut header = worker.snapshot_frame();
    if let Some(map) = header.as_object_mut() {
        map.insert("following".to_string(), Value::Bool(true));
    }
    if wire.write_json(&result_envelope(id, header)).is_err() {
        worker.bus.unsubscribe(subscription.id);
        return Flow::Close;
    }
    let mut last_frame_at = Instant::now();
    loop {
        if shutdown.load(Ordering::Acquire) {
            worker.bus.unsubscribe(subscription.id);
            let _ = wire.write_json(&json!({"type": "end", "reason": "the worker is stopping"}));
            return Flow::Close;
        }
        match subscription.frames.recv_timeout(FOLLOW_POLL) {
            Ok(frame) => {
                if wire.write_json(&frame).is_err() {
                    worker.bus.unsubscribe(subscription.id);
                    return Flow::Close;
                }
                last_frame_at = Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if last_frame_at.elapsed() >= FOLLOW_KEEPALIVE {
                    if wire.write_json(&json!({"type": "keepalive"})).is_err() {
                        worker.bus.unsubscribe(subscription.id);
                        return Flow::Close;
                    }
                    last_frame_at = Instant::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                worker.bus.unsubscribe(subscription.id);
                let _ =
                    wire.write_json(&json!({"type": "end", "reason": "the stream was dropped"}));
                return Flow::Close;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_answers_its_own_projection_and_proxies_everything_else() {
        assert!(!is_proxied("fleet.list"));
        assert!(!is_proxied("surface.status"));
        assert!(!is_proxied(METHOD_AGENT_SNAPSHOT));
        assert!(!is_proxied(METHOD_AGENT_ACTIVITY_LOG));
        assert!(!is_proxied(METHOD_AGENT_FOLLOW));
        assert!(!is_proxied(METHOD_WORKER_STATUS));
        assert!(is_proxied("session.list"));
        assert!(is_proxied("session.create"));
        assert!(is_proxied("surface.read"));
        assert!(is_proxied("host.status"));
        assert!(!is_proxied("workspace.list"));
    }
}
