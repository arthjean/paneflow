use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::{GenericFilePath, Listener, Stream, prelude::*};
use paneflow_host::agent::AgentBus;
use paneflow_host::server::{ConnectionGuard, bind_owner_only};
use paneflow_host::work_counters::{Counter, counters_value};
use paneflow_ipc_client::line_wire::{LineRead, Wire};
use serde_json::{Value, json};

use crate::protocol::{
    DEFAULT_ACTIVITY_LOG_LIMIT, ERR_BUSY, ERR_FRAME_TOO_LARGE, ERR_HANDSHAKE_REQUIRED,
    ERR_INVALID_REQUEST, ERR_METHOD_NOT_FOUND, ERR_PARSE, MAX_CONTROL_FRAME_BYTES,
    METHOD_AGENT_ACKNOWLEDGE, METHOD_AGENT_ACTIVITY_LOG, METHOD_AGENT_FOLLOW,
    METHOD_AGENT_SNAPSHOT, METHOD_HOST_HELLO, METHOD_USAGE_REFRESH, METHOD_USAGE_SNAPSHOT,
    METHOD_WORKER_HELLO, METHOD_WORKER_SHUTDOWN, METHOD_WORKER_STATUS, WorkerIdentity,
    error_envelope, result_envelope,
};
use crate::state::WorkerState;
use crate::usage::Usage;
use crate::usage::probe::{self, ProbeSlots};
use paneflow_agent_config::usage::accounts::UsageAgent;

const MAX_CONNECTIONS: usize = 128;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const FOLLOW_KEEPALIVE: Duration = Duration::from_secs(2);

pub struct Worker {
    pub identity: WorkerIdentity,
    pub home: PathBuf,
    pub core_endpoint: PathBuf,
    pub state: Mutex<WorkerState>,
    pub bus: AgentBus,
    pub sweeps: Counter,
    pub shutdown: Arc<AtomicBool>,
    pub(crate) shutdown_gate: Mutex<()>,
    pub(crate) shutdown_requested: Condvar,
    pub core_connected: AtomicBool,
    pub broadcast_snapshot: Mutex<Option<Vec<Value>>>,
    pub usage: Mutex<Usage>,
    pub probe_slots: ProbeSlots,
}

impl Worker {
    pub fn lock_usage(&self) -> std::sync::MutexGuard<'_, Usage> {
        self.usage.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn publish_usage(&self) {
        let frame = self.lock_usage().take_changed_frame();
        if let Some(frame) = frame {
            self.bus.broadcast(&frame);
        }
    }

    pub fn absorb_plan_limits(&self, frame: &Value) {
        let changed = self
            .lock_usage()
            .apply_plan_limits(frame, crate::state::now_ms());
        if changed {
            self.lock_usage().flush(Instant::now(), false);
            self.publish_usage();
        }
    }

    pub fn absorb_turn_signals(&self) {
        let signals = self.lock_state().drain_turn_signals();
        if signals.is_empty() {
            return;
        }
        let turn_ends = self.lock_usage().apply_turn_signals(&signals);
        let claimed =
            self.lock_usage()
                .claim_codex_reads(turn_ends, Duration::ZERO, Instant::now());
        self.read_codex(claimed);
        self.lock_usage().flush(Instant::now(), false);
        self.publish_usage();
    }

    pub fn sweep_usage(&self) {
        if self.core_connected.load(Ordering::Acquire) {
            let (running, in_turn) = {
                let state = self.lock_state();
                (state.running_sessions(), state.sessions_in_turn())
            };
            self.lock_usage()
                .expire_turns(&running, &in_turn, crate::state::now_ms());
        }
        let live_codex = self.running_codex_dirs();
        let claimed = self.lock_usage().claim_codex_reads(
            live_codex,
            crate::usage::codex::CODEX_POLL_INTERVAL,
            Instant::now(),
        );
        self.read_codex(claimed);
        self.lock_usage().flush(Instant::now(), false);
        self.publish_usage();
    }

    pub fn refresh_usage(self: &Arc<Self>, params: &Value) -> Value {
        let requested = probe::requested_accounts(params);
        let mut dirs: Vec<PathBuf> = paneflow_agent_config::codex_home().into_iter().collect();
        dirs.extend(self.running_codex_dirs());
        dirs.extend(
            requested
                .iter()
                .filter(|(agent, _)| *agent == UsageAgent::Codex)
                .map(|(_, dir)| dir.clone()),
        );
        let held = self.lock_usage().codex_dirs();
        dirs.extend(held);
        let claimed = self.lock_usage().claim_codex_reads(
            dirs,
            crate::usage::codex::CODEX_READ_FLOOR,
            Instant::now(),
        );
        self.read_codex(claimed);
        self.lock_usage().flush(Instant::now(), false);
        self.publish_usage();
        let probes = self
            .lock_usage()
            .claim_probes(requested, probe::PROBE_FLOOR, Instant::now());
        for (agent, dir) in probes {
            self.spawn_probe(agent, dir);
        }
        self.lock_usage().snapshot()
    }

    fn spawn_probe(self: &Arc<Self>, agent: UsageAgent, dir: PathBuf) {
        let worker = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("paneflow-usage-probe".to_string())
            .spawn(move || worker.run_probe(agent, &dir));
        if let Err(error) = spawned {
            log::warn!("paneflow-serve: cannot start a usage probe: {error}");
        }
    }

    fn run_probe(&self, agent: UsageAgent, dir: &std::path::Path) {
        let _slot = self.probe_slots.acquire();
        if self.shutdown.load(Ordering::Acquire) {
            return;
        }
        let user_home = paneflow_agent_config::home_dir();
        let now = crate::state::now_ms();
        match probe::probe_account(agent, dir, &self.home, user_home.as_deref(), now / 1000) {
            Ok(outcome) => {
                let changed =
                    self.lock_usage()
                        .record_probe(agent, dir, outcome, crate::state::now_ms());
                if changed {
                    self.lock_usage().flush(Instant::now(), false);
                    self.publish_usage();
                }
            }
            Err(error) => log::warn!(
                "paneflow-serve: the {} usage probe for {} kept the previous limits: {error}",
                agent.display_name(),
                dir.display()
            ),
        }
    }

    fn running_codex_dirs(&self) -> Vec<PathBuf> {
        let codex = UsageAgent::Codex.runtime_slug();
        self.lock_state()
            .running_accounts()
            .into_iter()
            .filter(|account| account.agent == codex && !account.config_dir.is_empty())
            .map(|account| PathBuf::from(account.config_dir))
            .collect()
    }

    fn read_codex(&self, dirs: Vec<PathBuf>) {
        if dirs.is_empty() {
            return;
        }
        let now = crate::state::now_ms();
        let readings = crate::usage::read_codex_accounts(dirs, now / 1000);
        let mut usage = self.lock_usage();
        for (dir, reading) in readings {
            usage.record_codex(&dir, reading, now);
        }
    }

    pub fn request_shutdown(&self) {
        let _gate = self
            .shutdown_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.shutdown.store(true, Ordering::Release);
        self.shutdown_requested.notify_all();
        self.bus.disconnect_all();
    }

    pub fn wait_for_shutdown(&self) {
        let mut gate = self
            .shutdown_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while !self.shutdown.load(Ordering::Acquire) {
            gate = self
                .shutdown_requested
                .wait(gate)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub fn lock_state(&self) -> std::sync::MutexGuard<'_, WorkerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn broadcast_snapshot_if_changed(&self, sessions: Vec<Value>) -> bool {
        let fingerprint = snapshot_fingerprint(&sessions);
        {
            let mut held = self
                .broadcast_snapshot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if held.as_ref() == Some(&fingerprint) {
                return false;
            }
            *held = Some(fingerprint);
        }
        self.bus
            .broadcast(&json!({"type": "snapshot", "sessions": sessions}));
        true
    }

    pub fn snapshot_frame(&self) -> Value {
        let sessions = self.lock_state().snapshot();
        let usage = self.lock_usage().frame();
        json!({
            "worker": self.identity,
            "capabilities": self.identity.capabilities,
            "core_connected": self.core_connected.load(Ordering::Acquire),
            "sessions": sessions,
            "usage": usage,
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
            "counters": self.work_counters(),
        })
    }

    pub fn work_counters(&self) -> Value {
        let broadcasts = self.bus.broadcasts();
        let mut counters = serde_json::Map::new();
        for (name, value) in [
            ("snapshot_broadcasts", broadcasts.snapshot.get()),
            ("projection_broadcasts", broadcasts.event.get()),
            ("sweeps", self.sweeps.get()),
        ] {
            counters.insert(name.to_string(), json!(value));
        }
        counters_value(counters)
    }

    pub fn publish(&self, projection: &crate::state::Projection, source: &Value) {
        let session = &projection.session;
        self.bus.broadcast(&json!({
            "type": "event",
            "session": session["session"],
            "title": session["title"],
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
            "foreground_runtime_id": session["foreground_runtime_id"],
            "unread": session["unread"],
            "declared_status": session["declared_status"],
            "program_status_capable": session["program_status_capable"],
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
}

pub struct ServerHandle {
    endpoint: PathBuf,
    worker: Arc<Worker>,
    thread: Option<JoinHandle<io::Result<()>>>,
}

impl ServerHandle {
    pub fn spawn(worker: Arc<Worker>, endpoint: PathBuf) -> io::Result<Self> {
        let listener = bind_owner_only(&endpoint, "paneflow-serve")?;
        let flag = Arc::clone(&worker.shutdown);
        let accepting = Arc::clone(&worker);
        let thread = std::thread::Builder::new()
            .name("paneflow-serve-accept".into())
            .spawn(move || accept_loop(listener, accepting, flag))?;
        Ok(Self {
            endpoint,
            worker,
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) -> io::Result<()> {
        self.worker.request_shutdown();
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
            self.worker.request_shutdown();
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

pub fn snapshot_fingerprint(sessions: &[Value]) -> Vec<Value> {
    sessions
        .iter()
        .map(|session| {
            let mut session = session.clone();
            if let Some(fields) = session.as_object_mut() {
                fields.remove("updated_at_ms");
            }
            session
        })
        .collect()
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
        let Some(guard) = ConnectionGuard::try_acquire(Arc::clone(&active), MAX_CONNECTIONS) else {
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
            Ok(LineRead::Eof) | Ok(LineRead::Idle) => return,
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
                worker.request_shutdown();
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
}

fn dispatch(worker: &Arc<Worker>, method: &str, params: &Value) -> Result<Value, DispatchError> {
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
        METHOD_USAGE_SNAPSHOT => Ok(worker.lock_usage().snapshot()),
        METHOD_USAGE_REFRESH => Ok(worker.refresh_usage(params)),
        _ => Err(DispatchError::MethodNotFound),
    }
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
        let until_keepalive = FOLLOW_KEEPALIVE.saturating_sub(last_frame_at.elapsed());
        match subscription.frames.recv_timeout(until_keepalive) {
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
                let reason = if shutdown.load(Ordering::Acquire) {
                    "the worker is stopping"
                } else {
                    "the stream was dropped"
                };
                let _ = wire.write_json(&json!({"type": "end", "reason": reason}));
                return Flow::Close;
            }
        }
    }
}
