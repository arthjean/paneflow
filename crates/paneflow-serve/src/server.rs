use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::{GenericFilePath, Listener, Stream, prelude::*};
use paneflow_host::agent::AgentBus;
use paneflow_host::server::{ConnectionGuard, bind_owner_only, retire_listener};
use paneflow_host::work_counters::{Counter, counters_value};
use paneflow_ipc_client::line_wire::{LineRead, Wire};
use serde_json::{Value, json};

use crate::event_log::{Delivery, EventLog, Filter};
use crate::layout_authority::ConnectionId;
use crate::protocol::{
    DEFAULT_ACTIVITY_LOG_LIMIT, ERR_BUSY, ERR_EVICTED, ERR_FRAME_TOO_LARGE, ERR_HANDSHAKE_REQUIRED,
    ERR_INVALID_REQUEST, ERR_PARSE, MAX_CONTROL_FRAME_BYTES, METHOD_HOST_HELLO,
    METHOD_WORKER_HELLO, WorkerIdentity, error_envelope, result_envelope,
};
use crate::registry::{RegistryBuilder, ServeError, Served, registry};
use crate::state::WorkerState;
use crate::usage::Usage;
use crate::usage::probe::{self, ProbeSlots};
use paneflow_agent_config::usage::accounts::UsageAgent;
use paneflow_api::Principal;

const MAX_CONNECTIONS: usize = 128;
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const ACCEPT_WAKE_RETRY: Duration = Duration::from_millis(10);
const FOLLOW_KEEPALIVE: Duration = Duration::from_secs(2);
const IMPORT_PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

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
    pub events: EventLog,
    pub config: Arc<crate::config_runtime::ConfigRuntime>,
    pub prompts: crate::prompts::PromptBroker,
    pub agent_limit: crate::agent_asks::AgentRateLimit,
    pub action_runs: crate::action_runs::ActionRuns,
    pub integration_refreshes: std::sync::atomic::AtomicU64,
    pub layout: Arc<crate::layout_authority::LayoutAuthority>,
    pub classifier: paneflow_host::peer::PeerClassifier,
    pub principals: Mutex<std::collections::HashMap<ConnectionId, Principal>>,
    pub agent_callers:
        Mutex<std::collections::HashMap<ConnectionId, crate::agent_caller::CallerVerdict>>,
    pub policy: paneflow_host::policy::PolicySource,
    pub(crate) acting: crate::authority::ActingRuns,
    pub pane_grants: crate::pane_grants::PaneGrantStore,
    pub grant_profiles: crate::grant_methods::ProfileSource,
    pub pane_endpoints: paneflow_host::pane_endpoint::PaneEndpoints,
}

pub const WINDOW_LAYOUT_CLIENT: &str = "window";

impl Worker {
    fn lock_principals(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<ConnectionId, Principal>> {
        self.principals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn principal_of(&self, connection: ConnectionId) -> Principal {
        if connection == ConnectionId::INTERNAL {
            return Principal::Worker;
        }
        self.lock_principals()
            .get(&connection)
            .cloned()
            .unwrap_or_else(|| Principal::Unverified {
                reason: "the connection has not been classified".into(),
            })
    }

    fn lock_agent_callers(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::HashMap<ConnectionId, crate::agent_caller::CallerVerdict>,
    > {
        self.agent_callers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn classify_connection(&self, connection: ConnectionId, wire: &Wire, fixed: Option<Principal>) {
        let read = paneflow_host::peer::read_wire_peer(wire);
        let principal = match fixed {
            Some(pane) => pane,
            None => self.classifier.principal(&read, "worker"),
        };
        if let (Principal::Pane { session, .. }, Some(identity)) = (&principal, read.identity()) {
            let verdict = crate::agent_caller::classify_caller(
                &paneflow_peer::LiveLineage,
                &crate::agent_caller::LiveProcesses,
                (identity.pid(), identity.started_at()),
                crate::agent_caller::leader_of_session(&self.home, session),
            );
            log::debug!(
                "paneflow-serve: the caller {} in pane session {session} is {verdict:?}",
                identity.pid()
            );
            self.lock_agent_callers().insert(connection, verdict);
        }
        self.lock_principals().insert(connection, principal);
    }

    pub fn agent_caller(
        &self,
        connection: ConnectionId,
        principal: &Principal,
    ) -> Option<crate::agent_caller::CallerVerdict> {
        if connection == ConnectionId::INTERNAL || !principal.is_pane() {
            return None;
        }
        match self.lock_agent_callers().get(&connection) {
            Some(verdict) if verdict.is_agent() => Some(verdict.clone()),
            Some(_) => None,
            None => Some(crate::agent_caller::CallerVerdict::Agent {
                label: None,
                evidence: "the caller's parents were never read".into(),
            }),
        }
    }

    fn attached_as(&self, connection: ConnectionId, client: Option<&str>) {
        if client != Some(WINDOW_LAYOUT_CLIENT) {
            return;
        }
        let mut principals = self.lock_principals();
        if let Some(principal) = principals.get_mut(&connection)
            && *principal == Principal::External
        {
            *principal = Principal::Window;
        }
    }

    pub fn reconcile_pane_endpoints(self: &Arc<Self>) {
        let wanted = self.lock_state().sandboxed_panes();
        let ended = self
            .pane_endpoints
            .retain(|session| wanted.iter().any(|pane| pane.session == session));
        for session in ended {
            log::debug!("paneflow-serve: closed the endpoint of ended sandboxed pane {session}");
        }
        let weak = Arc::downgrade(self);
        let serve: paneflow_host::pane_endpoint::ServeStream =
            Arc::new(move |stream, principal| {
                if let Some(worker) = weak.upgrade() {
                    serve_pane_stream(worker, stream, principal);
                }
            });
        for pane in wanted {
            if self.pane_endpoints.is_open(&pane.session) {
                continue;
            }
            if let Err(error) = self.pane_endpoints.ensure(
                &self.home,
                paneflow_home::PaneServer::Worker,
                paneflow_host::pane_endpoint::PaneIdentity {
                    session: pane.session.clone(),
                    workspace: pane.workspace.clone(),
                },
                &serve,
            ) {
                log::debug!(
                    "paneflow-serve: cannot open the endpoint of sandboxed pane {}: {error}",
                    pane.session
                );
            }
        }
    }

    fn forget_connection(&self, connection: ConnectionId) {
        self.lock_principals().remove(&connection);
        self.lock_agent_callers().remove(&connection);
    }

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
                .claim_codex_reads(turn_ends.codex, Duration::ZERO, Instant::now());
        self.read_codex(claimed);
        self.read_transcripts(turn_ends.transcripts);
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

    pub fn import_usage(self: &Arc<Self>, params: &Value) -> Value {
        let accounts = probe::requested_accounts(params);
        let force = params
            .get("force")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.start_import(accounts, force);
        self.lock_usage().snapshot()
    }

    pub fn start_import(
        self: &Arc<Self>,
        accounts: Vec<(UsageAgent, PathBuf)>,
        force: bool,
    ) -> bool {
        if !self.lock_usage().begin_import(force) {
            return false;
        }
        self.publish_usage();
        let worker = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("paneflow-usage-import".to_string())
            .spawn(move || worker.run_import(&accounts));
        if let Err(error) = spawned {
            log::warn!("paneflow-serve: cannot start the activity import: {error}");
            self.lock_usage()
                .finish_import(None, crate::state::now_ms());
            self.publish_usage();
            return false;
        }
        true
    }

    fn run_import(&self, accounts: &[(UsageAgent, PathBuf)]) {
        let files = crate::usage::backfill::import_files(accounts);
        let now = crate::state::now_ms();
        let mut published = Instant::now();
        let outcome = crate::usage::backfill::run_import(
            files,
            &crate::usage::ledger::local_calendar(),
            now,
            &self.shutdown,
            |files| {
                if published.elapsed() >= IMPORT_PROGRESS_INTERVAL {
                    published = Instant::now();
                    self.lock_usage().import_progress(files);
                    self.publish_usage();
                }
            },
        );
        let interrupted = outcome.is_none();
        let booked = self
            .lock_usage()
            .finish_import(outcome, crate::state::now_ms());
        if interrupted {
            log::info!("paneflow-serve: the activity import stopped before it finished");
        } else {
            log::info!("paneflow-serve: imported {booked} days of past activity");
        }
        self.lock_usage().flush(Instant::now(), true);
        self.publish_usage();
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
        let rollouts: Vec<crate::usage::TranscriptRead> = readings
            .iter()
            .filter_map(|(dir, _)| crate::usage::codex_transcript(dir))
            .collect();
        {
            let mut usage = self.lock_usage();
            for (dir, reading) in readings {
                usage.record_codex(&dir, reading, now);
            }
        }
        self.read_transcripts(rollouts);
    }

    fn read_transcripts(&self, reads: Vec<crate::usage::TranscriptRead>) {
        for read in reads {
            let cursor = self.lock_usage().token_cursor(&read.path);
            let Some(pass) = crate::usage::read_transcript(&read, cursor, crate::state::now_ms())
            else {
                continue;
            };
            self.lock_usage().record_tokens(&read.account, pass);
        }
    }

    pub fn request_shutdown(&self) {
        self.layout.flush();
        let _gate = self
            .shutdown_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        self.shutdown.store(true, Ordering::Release);
        self.shutdown_requested.notify_all();
        self.bus.disconnect_all();
        self.events.close();
        self.prompts.close();
    }

    pub fn absorb_state_events(&self) {
        let mut state = self.lock_state();
        let events = state.drain_events();
        if events.is_empty() {
            return;
        }
        for event in &events {
            if event.kind == crate::state::TERMINAL_EXITED {
                self.prompts.cancel_asked_by(&event.session.to_string());
            }
        }
        self.events.append_all(events.into_iter().map(|event| {
            (
                event.kind.to_string(),
                paneflow_api::methods::events::EventIds {
                    session: Some(event.session.to_string()),
                    ..Default::default()
                },
                event.data,
            )
        }));
        drop(state);
    }

    pub fn identity_value(&self) -> Value {
        serde_json::to_value(&self.identity).unwrap_or_else(|_| json!({}))
    }

    pub fn append_layout_events(
        &self,
        events: Vec<paneflow_api::methods::events::PublishedEvent>,
    ) -> Vec<u64> {
        if events.is_empty() {
            return Vec::new();
        }
        self.events.append_all(
            events
                .into_iter()
                .map(|event| (event.kind, event.ids, event.data)),
        )
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
            "layout": self.layout.status(),
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
        let retired = endpoint.clone();
        let thread = std::thread::Builder::new()
            .name("paneflow-serve-accept".into())
            .spawn(move || {
                serve_connections(&listener, accepting, flag);
                retire_listener(listener, &retired);
                Ok(())
            })?;
        Ok(Self {
            endpoint,
            worker,
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) -> io::Result<()> {
        self.worker.request_shutdown();
        match self.thread.take() {
            Some(thread) => {
                wake_until_finished(&self.endpoint, &thread);
                thread
                    .join()
                    .map_err(|_| io::Error::other("the worker accept thread panicked"))?
            }
            None => Ok(()),
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.worker.request_shutdown();
            wake_until_finished(&self.endpoint, &thread);
            let _ = thread.join();
        }
    }
}

fn wake_until_finished(endpoint: &Path, thread: &JoinHandle<io::Result<()>>) {
    while !thread.is_finished() {
        if let Ok(name) = endpoint.to_fs_name::<GenericFilePath>() {
            let _ = Stream::connect(name);
        }
        std::thread::sleep(ACCEPT_WAKE_RETRY);
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

fn serve_connections(listener: &Listener, worker: Arc<Worker>, shutdown: Arc<AtomicBool>) {
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
                    Ok(mut wire) => {
                        wire.set_max_write(
                            paneflow_ipc_client::host_control::MAX_REPLY_FRAME_BYTES,
                        );
                        handle_connection(wire, worker, shutdown, None);
                    }
                    Err(error) => log::debug!("paneflow-serve: connection setup failed: {error}"),
                }
            });
        if let Err(error) = spawned {
            log::warn!("paneflow-serve: cannot start a connection thread: {error}");
        }
    }
}

pub(crate) enum Flow {
    Continue,
    Close,
}

pub(crate) fn write_error(wire: &mut Wire, id: &Value, error: &ServeError) -> Flow {
    match wire.write_json(&error_envelope(
        id,
        error.code,
        error.message.clone(),
        error.data.clone(),
    )) {
        Ok(()) => Flow::Continue,
        Err(_) => Flow::Close,
    }
}

struct Attachment<'a> {
    worker: &'a Worker,
    connection: crate::layout_authority::ConnectionId,
}

impl Drop for Attachment<'_> {
    fn drop(&mut self) {
        self.worker.layout.disconnect(self.connection);
        self.worker.forget_connection(self.connection);
    }
}

fn next_connection() -> crate::layout_authority::ConnectionId {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    crate::layout_authority::ConnectionId(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn serve_pane_stream(worker: Arc<Worker>, stream: Stream, principal: Principal) {
    match Wire::new(stream, MAX_CONTROL_FRAME_BYTES) {
        Ok(mut wire) => {
            wire.set_max_write(paneflow_ipc_client::host_control::MAX_REPLY_FRAME_BYTES);
            let shutdown = Arc::clone(&worker.shutdown);
            handle_connection(wire, worker, shutdown, Some(principal));
        }
        Err(error) => log::debug!("paneflow-serve: pane connection setup failed: {error}"),
    }
}

fn handle_connection(
    mut wire: Wire,
    worker: Arc<Worker>,
    shutdown: Arc<AtomicBool>,
    fixed: Option<Principal>,
) {
    let mut fixed = fixed;
    let mut greeted = false;
    let attachment = Attachment {
        worker: &worker,
        connection: next_connection(),
    };
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
            worker.classify_connection(attachment.connection, &wire, fixed.take());
            if wire
                .write_json(&result_envelope(&id, worker.identity_value()))
                .is_err()
            {
                return;
            }
            continue;
        }
        let method = if method == METHOD_HOST_HELLO {
            METHOD_WORKER_HELLO
        } else {
            method
        };
        if method.starts_with("layout.") {
            wire.set_max_frame(crate::layout_authority::MAX_LAYOUT_FRAME_BYTES);
        }
        let served = match registry() {
            Ok(registry) => registry.serve(
                &mut wire,
                &worker,
                &shutdown,
                &id,
                method,
                &params,
                attachment.connection,
            ),
            Err(error) => Served::Reply(Err(error)),
        };
        let flow = match served {
            Served::Reply(answer) => reply(&mut wire, &id, answer),
            Served::ReplyThenClose(answer, after) => {
                let _ = reply(&mut wire, &id, answer);
                after(&worker);
                Flow::Close
            }
            Served::Streamed(flow) => flow,
        };
        if matches!(flow, Flow::Close) {
            return;
        }
    }
}

fn reply(wire: &mut Wire, id: &Value, answer: Result<Value, ServeError>) -> Flow {
    match answer {
        Ok(result) => match wire.write_json(&result_envelope(id, result)) {
            Ok(()) => Flow::Continue,
            Err(_) => Flow::Close,
        },
        Err(error) => write_error(wire, id, &error),
    }
}

pub(crate) fn register_methods(builder: RegistryBuilder) -> RegistryBuilder {
    use paneflow_api::methods::{
        config, events, grant, layout, prompt, surface, system, worker, workspace,
    };

    builder
        .ready::<worker::Hello>(|worker, _| Ok(worker.identity_value()))
        .ready::<worker::Status>(|worker, _| Ok(worker.status_frame()))
        .closing::<worker::Shutdown>(
            |worker, _| Ok(json!({"stopping": true, "pid": worker.identity.pid})),
            Worker::request_shutdown,
        )
        .ready::<worker::Snapshot>(|worker, _| Ok(worker.snapshot_frame()))
        .stream::<worker::Follow>(|stream| {
            stream_follow(stream.wire, stream.worker, stream.shutdown, stream.id)
        })
        .ready::<worker::ActivityLogMethod>(|worker, call| {
            let limit = call
                .params
                .limit
                .filter(|limit| *limit > 0)
                .unwrap_or(DEFAULT_ACTIVITY_LOG_LIMIT);
            Ok(json!({"entries": worker.lock_state().activity_log().to_values(limit)}))
        })
        .ready::<worker::Acknowledge>(|worker, call| Ok(worker.acknowledge(call.raw)))
        .ready::<worker::UsageSnapshotMethod>(|worker, _| Ok(worker.lock_usage().snapshot()))
        .ready::<worker::UsageRefresh>(|worker, call| Ok(worker.refresh_usage(call.raw)))
        .ready::<worker::UsageImport>(|worker, call| Ok(worker.import_usage(call.raw)))
        .stream::<events::Follow>(stream_events)
        .ready::<config::Status>(|worker, _| to_value(worker.config.status()))
        .ready::<config::Reload>(|worker, _| to_value(worker.config.reload(worker)))
        .ready::<config::ListActions>(|worker, _| to_value(worker.config.actions()))
        .ready::<config::InvokeAction>(|worker, call| {
            worker
                .invoke_action(call.params, &call.authority.principal, call.connection)
                .and_then(to_value)
        })
        .ready::<prompt::ResolveAgentPrompt>(|worker, call| {
            to_value(worker.resolve_agent_prompt(
                call.params,
                &call.authority.principal,
                call.connection,
            ))
        })
        .ready::<config::WaitAction>(|worker, call| {
            worker
                .wait_action(call.params, &call.authority.principal)
                .and_then(to_value)
        })
        .ready::<config::ListKeymap>(|worker, _| to_value(worker.config.keymap()))
        .ready::<grant::Request>(|worker, call| {
            worker
                .request_grant(call.params, &call.authority.principal, call.connection)
                .and_then(to_value)
        })
        .ready::<grant::List>(|worker, call| {
            to_value(worker.list_grants(&call.authority.principal))
        })
        .ready::<grant::Revoke>(|worker, call| {
            worker
                .revoke_grant(call.params, &call.authority.principal)
                .and_then(to_value)
        })
        .ready::<grant::Delegate>(|worker, call| {
            worker
                .delegate_grant(call.params, &call.authority.principal)
                .and_then(to_value)
        })
        .ready::<prompt::Request>(|worker, call| {
            worker
                .request_prompt(call.params, &call.authority.principal, call.connection)
                .and_then(to_value)
        })
        .ready::<prompt::Wait>(|worker, call| {
            worker
                .wait_prompt(
                    call.params.id,
                    call.params.wait_ms,
                    &call.authority.principal,
                )
                .and_then(to_value)
        })
        .ready::<prompt::ReplyTo>(|worker, call| {
            worker
                .prompts
                .reply(call.params, &call.authority.principal)
                .and_then(to_value)
        })
        .stream::<prompt::Follow>(stream_prompts)
        .ready::<layout::ViewMethod>(|worker, call| {
            to_value(worker.layout.view(call.params.unless_revision))
        })
        .ready::<layout::Attach>(|worker, call| {
            worker.attached_as(call.connection, call.params.client.as_deref());
            to_value(worker.layout.attach(call.connection, call.params.client))
        })
        .ready::<layout::LeaseIds>(|worker, call| {
            worker.layout.lease(call.connection).and_then(to_value)
        })
        .ready::<layout::Apply>(|worker, call| {
            let outcome = worker.layout.apply(
                call.connection,
                serde_json::Value::Object(call.params.op.0),
                call.params.ids_from,
                crate::state::now_ms(),
                call.authority.may_spawn(),
            )?;
            worker.append_layout_events(outcome.events);
            match outcome.launch_error {
                Some(error) => Err(error),
                None => to_value(outcome.applied),
            }
        })
        .asynchronous::<surface::Split>(crate::structural::surface_split)
        .ready::<surface::Rename>(crate::structural::surface_rename)
        .ready::<surface::Focus>(crate::structural::surface_focus)
        .asynchronous::<workspace::Create>(crate::structural::workspace_create)
        .asynchronous::<workspace::Up>(crate::structural::workspace_up)
        .ready::<workspace::Select>(crate::structural::workspace_select)
        .ready::<workspace::Close>(crate::structural::workspace_close)
        .ready::<workspace::RestoreLayout>(crate::structural::workspace_restore_layout)
        .ready::<layout::Launch>(|worker, call| {
            let launched = worker.layout.launch(
                call.connection,
                call.params,
                call.authority.switch_open_for_variables(),
            )?;
            worker.append_layout_events(launched.events.clone());
            to_value(launched)
        })
        .ready::<layout::Sweep>(|worker, call| {
            to_value(worker.layout.sweep(call.connection, call.params)?)
        })
        .ready::<layout::Adopt>(|worker, call| {
            let (adopted, events) =
                worker
                    .layout
                    .adopt(call.connection, call.params.view, call.params.acknowledged)?;
            worker.append_layout_events(events);
            to_value(adopted)
        })
        .inline::<system::Describe>(|_, call| registry()?.describe(call.params.method.as_deref()))
        .inline::<system::WhoAmI>(|worker, call| {
            Ok(worker.principal_of(call.connection).whoami_value())
        })
}

fn to_value(value: impl serde::Serialize) -> Result<Value, ServeError> {
    serde_json::to_value(value).map_err(|error| ServeError {
        code: crate::protocol::ERR_INTERNAL,
        message: format!("the reply cannot be encoded: {error}"),
        data: None,
    })
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

fn stream_events(
    stream: crate::registry::Stream<'_, paneflow_api::methods::events::FollowParams>,
) -> Flow {
    let crate::registry::Stream {
        wire,
        worker,
        shutdown,
        id,
        params,
        ..
    } = stream;
    let filter = match Filter::new(params.types.as_deref(), params.workspace_id) {
        Ok(filter) => filter,
        Err(message) => return write_error(wire, id, &ServeError::invalid_params(message)),
    };
    let subscription = match worker
        .events
        .follow(params.since, params.epoch.as_deref(), filter)
    {
        Ok(subscription) => subscription,
        Err(evicted) => {
            return write_error(
                wire,
                id,
                &ServeError {
                    code: ERR_EVICTED,
                    message: format!(
                        "evicted: the events after seq {} are no longer in the log; resume from seq {} of epoch {}",
                        params.since.unwrap_or_default(),
                        evicted.seq,
                        evicted.epoch
                    ),
                    data: Some(json!({
                        "reason": "evicted",
                        "seq": evicted.seq,
                        "epoch": evicted.epoch,
                    })),
                },
            );
        }
    };
    let finish = |wire: &mut Wire, reason: &str| {
        worker.events.unfollow(subscription.id);
        let _ = wire.write_json(&json!({"type": "end", "reason": reason}));
        Flow::Close
    };
    let header = json!({
        "type": "following",
        "seq": subscription.seq,
        "epoch": subscription.epoch,
    });
    if wire.write_json(&result_envelope(id, header)).is_err() {
        worker.events.unfollow(subscription.id);
        return Flow::Close;
    }
    for logged in &subscription.replay {
        if write_event(wire, &logged.line).is_err() {
            worker.events.unfollow(subscription.id);
            return Flow::Close;
        }
    }
    let mut last_frame_at = Instant::now();
    loop {
        if shutdown.load(Ordering::Acquire) {
            return finish(wire, "the worker is stopping");
        }
        let until_keepalive = FOLLOW_KEEPALIVE.saturating_sub(last_frame_at.elapsed());
        match subscription.next(until_keepalive) {
            Delivery::Event(logged) => {
                if write_event(wire, &logged.line).is_err() {
                    worker.events.unfollow(subscription.id);
                    return Flow::Close;
                }
                last_frame_at = Instant::now();
            }
            Delivery::Quiet => {
                if last_frame_at.elapsed() >= FOLLOW_KEEPALIVE {
                    if wire.write_json(&json!({"type": "keepalive"})).is_err() {
                        worker.events.unfollow(subscription.id);
                        return Flow::Close;
                    }
                    last_frame_at = Instant::now();
                }
            }
            Delivery::Overflowed => {
                return finish(
                    wire,
                    "the follower fell more than 8 MiB behind the event log; follow again with since",
                );
            }
            Delivery::Closed => {
                let reason = if shutdown.load(Ordering::Acquire) {
                    "the worker is stopping"
                } else {
                    "the stream was dropped"
                };
                return finish(wire, reason);
            }
        }
    }
}

struct Following<'a> {
    worker: &'a Worker,
    id: u64,
}

impl Drop for Following<'_> {
    fn drop(&mut self) {
        self.worker.prompts.unfollow(self.id);
    }
}

fn stream_prompts(
    stream: crate::registry::Stream<'_, paneflow_api::methods::prompt::FollowParams>,
) -> Flow {
    use paneflow_api::methods::prompt::{FollowHeader, FollowingKind};
    let crate::registry::Stream {
        wire,
        worker,
        shutdown,
        id,
        params,
        principal,
    } = stream;
    if params.kinds.is_empty() {
        return write_error(
            wire,
            id,
            &ServeError::invalid_params("'kinds' must name at least one of input and pick"),
        );
    }
    let feed = worker
        .prompts
        .follow(params.kinds.clone(), principal.clone());
    let _following = Following {
        worker,
        id: feed.id,
    };
    let header = FollowHeader {
        kind: FollowingKind::Following,
        kinds: params.kinds,
        pending: feed.pending.len(),
    };
    let header = serde_json::to_value(header).unwrap_or(Value::Null);
    if wire.write_json(&result_envelope(id, header)).is_err() {
        return Flow::Close;
    }
    for frame in &feed.pending {
        if wire.write_json(frame).is_err() {
            return Flow::Close;
        }
    }
    let mut last_frame_at = Instant::now();
    loop {
        if shutdown.load(Ordering::Acquire) {
            let _ = wire.write_json(&json!({"type": "end", "reason": "the worker is stopping"}));
            return Flow::Close;
        }
        let until_keepalive = FOLLOW_KEEPALIVE.saturating_sub(last_frame_at.elapsed());
        match feed.frames.recv_timeout(until_keepalive) {
            Ok(frame) => {
                if wire.write_json(&frame).is_err() {
                    return Flow::Close;
                }
                last_frame_at = Instant::now();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                worker.prompts.sweep();
                if last_frame_at.elapsed() >= FOLLOW_KEEPALIVE {
                    if wire.write_json(&json!({"type": "keepalive"})).is_err() {
                        return Flow::Close;
                    }
                    last_frame_at = Instant::now();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let _ =
                    wire.write_json(&json!({"type": "end", "reason": "the worker is stopping"}));
                return Flow::Close;
            }
        }
    }
}

fn write_event(wire: &mut Wire, line: &str) -> io::Result<()> {
    wire.write_line_with_timeout(
        line.as_bytes(),
        paneflow_ipc_client::line_wire::WRITE_DEADLINE,
    )
}
