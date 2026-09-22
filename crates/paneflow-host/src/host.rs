use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::{AgentBus, AgentEvent, AgentSnapshotEntry, AgentSubscription};
use crate::bootstrap::{OwnerLock, OwnerLockError};
use crate::manifest::{
    HostedSessionRuntime, MANIFEST_SCHEMA_VERSION, ManifestError, SessionLaunch, SessionLifecycle,
    SessionManifest, now_ms, read_manifest, write_atomically,
};
use crate::persistence::{
    CRITICAL_DEADLINE, ManifestRevision, PersistError, Persistence, QueueReport,
    SessionPersistence, WriteClass,
};
use crate::protocol::{HOST_PROTOCOL_VERSION, HostIdentity, local_engine_identity};
use crate::runtime::{
    Checkpoint, CompletedRecord, LaunchCancel, LaunchWait, OutputSlice, RuntimeError,
    RuntimeNotice, RuntimeObserver, STARTUP_DEADLINE, SessionRuntime, SpawnSpec, StopReport,
};
use crate::session_input::SessionInput;

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub const MAX_PENDING_LAUNCHES: usize = 8;
const MAX_LAUNCH_ARGS: usize = 64;
const MAX_LAUNCH_ENV_ENTRIES: usize = 256;
const MAX_HOOK_RECEIPTS: usize = 16;
const LATE_LAUNCH_WAIT: Duration = Duration::from_secs(3600);
pub const STOP_ACTION_BUDGET: Duration = Duration::from_secs(5);
pub const MAX_CONCURRENT_CHECKPOINTS: usize = 2;
pub const CHECKPOINT_STAGING_BUDGET_BYTES: usize = 256 * 1024 * 1024;
pub const CHECKPOINT_ADMISSION_DEADLINE: Duration = Duration::from_secs(5);
pub const MAX_TERMINAL_CELLS: u32 = 4_194_304;

pub type OperationId = u64;

#[derive(Default)]
struct StagingState {
    active: usize,
    bytes: usize,
    peak_bytes: usize,
    refused: u64,
}

#[derive(Default)]
struct CheckpointStaging {
    state: Mutex<StagingState>,
    released: std::sync::Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StagingReport {
    pub active: usize,
    pub max_concurrent: usize,
    pub staged_bytes: usize,
    pub peak_staged_bytes: usize,
    pub budget_bytes: usize,
    pub refused: u64,
}

impl CheckpointStaging {
    fn lock(&self) -> std::sync::MutexGuard<'_, StagingState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn admit(self: &Arc<Self>, bytes: usize, deadline: Instant) -> Result<StagingLease, HostError> {
        if bytes > CHECKPOINT_STAGING_BUDGET_BYTES {
            self.lock().refused += 1;
            return Err(HostError::Busy(format!(
                "a {bytes} byte checkpoint exceeds the {CHECKPOINT_STAGING_BUDGET_BYTES} byte staging budget"
            )));
        }
        let mut state = self.lock();
        loop {
            let fits = state.active < MAX_CONCURRENT_CHECKPOINTS
                && state.bytes.saturating_add(bytes) <= CHECKPOINT_STAGING_BUDGET_BYTES;
            if fits {
                state.active += 1;
                state.bytes += bytes;
                state.peak_bytes = state.peak_bytes.max(state.bytes);
                return Ok(StagingLease {
                    staging: Arc::clone(self),
                    bytes,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.refused += 1;
                return Err(HostError::Busy(format!(
                    "checkpoint staging is at capacity ({} of {MAX_CONCURRENT_CHECKPOINTS} captures, {} of {CHECKPOINT_STAGING_BUDGET_BYTES} bytes); retry when an attachment finishes",
                    state.active, state.bytes
                )));
            }
            let (guard, _) = self
                .released
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = guard;
        }
    }

    fn report(&self) -> StagingReport {
        let state = self.lock();
        StagingReport {
            active: state.active,
            max_concurrent: MAX_CONCURRENT_CHECKPOINTS,
            staged_bytes: state.bytes,
            peak_staged_bytes: state.peak_bytes,
            budget_bytes: CHECKPOINT_STAGING_BUDGET_BYTES,
            refused: state.refused,
        }
    }
}

pub struct StagingLease {
    staging: Arc<CheckpointStaging>,
    bytes: usize,
}

impl StagingLease {
    fn adjust(&mut self, actual: usize) {
        let mut state = self.staging.lock();
        state.bytes = state
            .bytes
            .saturating_sub(self.bytes)
            .saturating_add(actual);
        state.peak_bytes = state.peak_bytes.max(state.bytes);
        self.bytes = actual;
    }
}

impl Drop for StagingLease {
    fn drop(&mut self) {
        let mut state = self.staging.lock();
        state.active = state.active.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
        drop(state);
        self.staging.released.notify_all();
    }
}

pub struct StagedCheckpoint {
    checkpoint: Checkpoint,
    _lease: StagingLease,
}

impl StagedCheckpoint {
    pub fn into_inner(self) -> Checkpoint {
        self.checkpoint
    }
}

impl std::ops::Deref for StagedCheckpoint {
    type Target = Checkpoint;

    fn deref(&self) -> &Checkpoint {
        &self.checkpoint
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionResources {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub runtime: crate::runtime::RuntimeResources,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceReport {
    pub persistence: QueueReport,
    pub checkpoints: StagingReport,
    pub live_runtimes: usize,
    pub pending_launches: usize,
    pub sessions: Vec<SessionResources>,
}

pub fn validate_dimensions(cols: u16, rows: u16) -> Result<(), HostError> {
    if cols == 0 || rows == 0 {
        return Err(HostError::InvalidRequest(
            "terminal dimensions must be non-zero".to_string(),
        ));
    }
    let cells = u32::from(cols)
        .checked_mul(u32::from(rows))
        .ok_or_else(|| HostError::InvalidRequest("terminal dimensions overflow".to_string()))?;
    if cells > MAX_TERMINAL_CELLS {
        return Err(HostError::InvalidRequest(format!(
            "a {cols}x{rows} terminal exceeds the {MAX_TERMINAL_CELLS} cell limit"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateSession {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSummary {
    #[serde(flatten)]
    pub manifest: SessionManifest,
    pub live: bool,
    pub owned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending_launch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch_operation: Option<OperationId>,
    #[serde(default, skip_serializing_if = "is_zero_usize")]
    pub descendants_unresolved: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub durability_error: Option<String>,
}

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SessionReconnection {
    Live,
    Starting,
    Exited {
        code: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<String>,
    },
    Failed {
        reason: String,
    },
    HostReplaced {
        previous_owner: HostInstanceToken,
        current_owner: HostInstanceToken,
    },
    Lost,
    Unverified {
        reason: String,
    },
}

impl SessionSummary {
    pub fn reconnection(&self, current_owner: &HostInstanceToken) -> SessionReconnection {
        if self.owned
            && let SessionLifecycle::Unverified { reason } = &self.manifest.lifecycle
        {
            return SessionReconnection::Unverified {
                reason: reason.clone(),
            };
        }
        if self.owned && (self.pending_launch || self.live) {
            return match self.manifest.lifecycle {
                SessionLifecycle::Starting => SessionReconnection::Starting,
                _ => SessionReconnection::Live,
            };
        }
        match &self.manifest.lifecycle {
            SessionLifecycle::Exited { code, signal } => SessionReconnection::Exited {
                code: *code,
                signal: signal.clone(),
            },
            SessionLifecycle::Failed { reason } => SessionReconnection::Failed {
                reason: reason.clone(),
            },
            SessionLifecycle::Lost
            | SessionLifecycle::Starting
            | SessionLifecycle::Running
            | SessionLifecycle::Unverified { .. }
                if &self.manifest.host_instance != current_owner =>
            {
                SessionReconnection::HostReplaced {
                    previous_owner: self.manifest.host_instance.clone(),
                    current_owner: current_owner.clone(),
                }
            }
            SessionLifecycle::Unverified { reason } => SessionReconnection::Unverified {
                reason: reason.clone(),
            },
            SessionLifecycle::Lost | SessionLifecycle::Starting | SessionLifecycle::Running => {
                SessionReconnection::Lost
            }
        }
    }

    pub fn owns_process(&self) -> bool {
        self.pending_launch
            || self.live
            || self.descendants_unresolved > 0
            || matches!(self.manifest.lifecycle, SessionLifecycle::Unverified { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedSession {
    pub session: SessionId,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShutdownReport {
    pub ended: Vec<SessionSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved: Vec<UnresolvedSession>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("session {0} is not known to this host")]
    SessionNotFound(SessionId),
    #[error("session {session} already exists")]
    SessionExists { session: SessionId },
    #[error("session {session} is at generation {current}, request named {requested}")]
    GenerationMismatch {
        session: SessionId,
        current: SessionGeneration,
        requested: SessionGeneration,
    },
    #[error("session {0} is not running")]
    SessionNotLive(SessionId),
    #[error("session {0} is still running; stop it first")]
    SessionLive(SessionId),
    #[error("{count} live session(s) remain; stop them before stopping the host")]
    SessionsLive { count: usize },
    #[error("{} session(s) could not be confirmed stopped; the host keeps owning them", .sessions.len())]
    SessionsUnresolved { sessions: Vec<UnresolvedSession> },
    #[error("session {0} is still starting; its process is being checked")]
    LaunchPending(SessionId),
    #[error("session {session} has unresolved process ownership: {reason}")]
    OwnershipUnresolved { session: SessionId, reason: String },
    #[error("the host is at capacity: {0}")]
    Busy(String),
    #[error("the host is shutting down and accepts no new process")]
    ShuttingDown,
    #[error("{0}")]
    OwnerBusy(String),
    #[error("session {0} has no owned process handle in this host instance; nothing was signaled")]
    ProcessUnverified(SessionId),
    #[error("session {session} could not start: {reason}")]
    SpawnFailed { session: SessionId, reason: String },
    #[error("invalid request: {0}")]
    InvalidRequest(String),
    #[error("{0}")]
    Runtime(#[from] RuntimeError),
    #[error("host storage error: {0}")]
    Storage(String),
    #[error("persistence failed: {0}")]
    Durability(#[from] PersistError),
}

type Durability = Arc<SessionPersistence>;

#[derive(Default)]
struct SeedLedger {
    removed: bool,
    receipts: VecDeque<(u64, u64)>,
}

struct PendingLaunch {
    operation: OperationId,
    generation: SessionGeneration,
    cancel: Option<LaunchCancel>,
    cancelled: bool,
    fallback_owner: Option<crate::runtime::LaunchHandle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompletedRuntime {
    generation: SessionGeneration,
    final_offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionText {
    pub text: String,
    pub live: bool,
    pub available: bool,
    pub complete: bool,
}

struct SessionRecord {
    manifest: Arc<Mutex<SessionManifest>>,
    runtime: Option<Arc<SessionRuntime>>,
    completed: Option<CompletedRuntime>,
    launch: Option<PendingLaunch>,
    input: Arc<Mutex<SessionInput>>,
    escape_fence: bool,
    seed: Arc<Mutex<SeedLedger>>,
    durability: Durability,
}

impl SessionRecord {
    fn fresh(manifest: Arc<Mutex<SessionManifest>>) -> Self {
        let escape_fence = escape_fence_of(
            manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .runtime
                .as_ref(),
        );
        Self {
            manifest,
            runtime: None,
            completed: None,
            launch: None,
            input: Arc::new(Mutex::new(SessionInput::default())),
            escape_fence,
            seed: Arc::new(Mutex::new(SeedLedger::default())),
            durability: Arc::new(SessionPersistence::default()),
        }
    }

    fn is_live(&self) -> bool {
        self.runtime.as_deref().is_some_and(SessionRuntime::is_live)
    }

    fn owns_process(&self) -> bool {
        self.launch.is_some()
            || self
                .runtime
                .as_deref()
                .is_some_and(|runtime| runtime.owns_process() || runtime.unverified().is_some())
            || self.lifecycle_holds_ownership()
    }

    fn lifecycle_holds_ownership(&self) -> bool {
        matches!(
            self.manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .lifecycle,
            SessionLifecycle::Unverified { .. }
        )
    }

    fn generation(&self) -> SessionGeneration {
        self.manifest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation
    }

    fn set_escape_fence(&mut self, fenced: bool) {
        self.escape_fence = fenced;
        self.input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

pub(crate) type FencedInputTarget = (SessionId, Arc<Mutex<SessionInput>>);

pub(crate) struct ScanTarget {
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
    pub(crate) manifest: Arc<Mutex<SessionManifest>>,
    pub(crate) runtime: Arc<SessionRuntime>,
}

pub fn launch_binding_for_command(
    command: &str,
) -> Option<&'static paneflow_agent_config::Runtime> {
    let alias = Path::new(command)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(command);
    paneflow_agent_config::runtime_by_command_alias(alias)
}

fn escape_fence_of(runtime: Option<&HostedSessionRuntime>) -> bool {
    runtime
        .and_then(|runtime| runtime.launch_binding.as_deref())
        .and_then(paneflow_agent_config::runtime_by_id)
        .is_some_and(|runtime| runtime.lifecycle.escape_cancels_turn)
}

fn with_launch_binding(
    held: Option<HostedSessionRuntime>,
    binding: Option<String>,
) -> Option<HostedSessionRuntime> {
    match (held, binding) {
        (None, None) => None,
        (None, launch_binding) => Some(HostedSessionRuntime {
            current_observation: None,
            launch_binding,
        }),
        (Some(held), launch_binding) => Some(HostedSessionRuntime {
            launch_binding,
            ..held
        }),
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Barrier {
    RestartPersist,
    LaunchCommit,
    StopCommit,
    ManifestPersist,
}

#[cfg(test)]
type BarrierHook = Arc<dyn Fn(Barrier) + Send + Sync>;

pub struct SessionHost {
    home: PathBuf,
    identity: HostIdentity,
    persistence: Persistence,
    staging: Arc<CheckpointStaging>,
    streaming_connections: AtomicUsize,
    sessions: Mutex<BTreeMap<SessionId, SessionRecord>>,
    agent_bus: AgentBus,
    helper_dir: Option<PathBuf>,
    permissions: crate::control::ControlPermissions,
    submit_paste_delay: std::time::Duration,
    next_operation: AtomicU64,
    shutting_down: AtomicBool,
    #[cfg(test)]
    barrier: Mutex<Option<BarrierHook>>,
    #[cfg(test)]
    fail_launch_owner_spawn: AtomicBool,
    weak: Weak<Self>,
    _owner: OwnerLock,
}

pub const INACTIVE_ROWS_PER_WORKSPACE: usize = 5;

pub(crate) const FINISHED_RECORD_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;

const INTERRUPTED_RECORD_MAX_AGE_MS: u64 = 30 * 24 * 60 * 60 * 1000;

fn record_max_age_ms(lifecycle: &SessionLifecycle) -> Option<u64> {
    match lifecycle {
        SessionLifecycle::Starting
        | SessionLifecycle::Running
        | SessionLifecycle::Unverified { .. } => None,
        SessionLifecycle::Lost => Some(INTERRUPTED_RECORD_MAX_AGE_MS),
        SessionLifecycle::Exited { .. } | SessionLifecycle::Failed { .. } => {
            Some(FINISHED_RECORD_MAX_AGE_MS)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRow {
    pub session: SessionId,
    pub generation: SessionGeneration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<WorkspaceId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub shell: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub lifecycle: SessionLifecycle,
    pub reconnection: SessionReconnection,
    pub live: bool,
    pub owned: bool,
    #[serde(default)]
    pub host_protocol_version: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub host_build_id: String,
    pub updated_at_ms: u64,
}

impl SessionRow {
    fn of(summary: SessionSummary, owner: &HostInstanceToken) -> Self {
        let reconnection = summary.reconnection(owner);
        let SessionSummary {
            manifest,
            live,
            owned,
            ..
        } = summary;
        Self {
            session: manifest.session,
            generation: manifest.generation,
            workspace: manifest.workspace,
            title: manifest.title,
            cwd: manifest.current_cwd.unwrap_or(manifest.cwd),
            agent: manifest.last_hook.map(|hook| hook.tool),
            shell: manifest.launch.shell,
            pid: manifest.process.map(|process| process.pid),
            lifecycle: manifest.lifecycle,
            reconnection,
            live,
            owned,
            host_protocol_version: manifest.host_protocol_version,
            host_build_id: manifest.host_build_id,
            updated_at_ms: manifest.updated_at_ms,
        }
    }
}

pub fn inactive_window(
    mut summaries: Vec<SessionSummary>,
    inactive_per_workspace: usize,
) -> Vec<SessionSummary> {
    summaries.sort_unstable_by_key(|summary| std::cmp::Reverse(summary.manifest.updated_at_ms));
    let mut seen: HashMap<Option<WorkspaceId>, usize> = HashMap::new();
    summaries.retain(|summary| {
        if summary.owns_process() {
            return true;
        }
        let counted = seen.entry(summary.manifest.workspace.clone()).or_default();
        let keep = *counted < inactive_per_workspace;
        *counted += 1;
        keep
    });
    summaries
}

fn without_launch_environment(mut summary: SessionSummary) -> SessionSummary {
    summary.manifest.launch.env = BTreeMap::new();
    summary
}

pub struct IngestOutcome {
    pub ack: Value,
    pub frame: Option<Value>,
}

fn receipt_key(event: &AgentEvent) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.generation.hash(&mut hasher);
    event.message.hash(&mut hasher);
    event.summary.hash(&mut hasher);
    event.exit_code.hash(&mut hasher);
    event.kind.wire_str().hash(&mut hasher);
    event.tool.hash(&mut hasher);
    event.tool_name.hash(&mut hasher);
    event.pid.hash(&mut hasher);
    event.emitted_at_ms.hash(&mut hasher);
    event
        .event_source
        .map(|source| source.as_str())
        .hash(&mut hasher);
    event.payload.to_string().hash(&mut hasher);
    hasher.finish()
}

impl SessionHost {
    pub fn open(home: &Path, endpoint: &Path) -> Result<Arc<Self>, HostError> {
        std::fs::create_dir_all(paneflow_home::host_sessions_dir_in(home))
            .map_err(|e| HostError::Storage(format!("cannot create the host directory: {e}")))?;
        std::fs::create_dir_all(paneflow_home::host_session_data_root_in(home)).map_err(|e| {
            HostError::Storage(format!(
                "cannot create the host session data directory: {e}"
            ))
        })?;
        let owner = OwnerLock::acquire(home).map_err(|error| match error {
            OwnerLockError::Held(_) => HostError::OwnerBusy(error.to_string()),
            OwnerLockError::Io(io) => HostError::Storage(io.to_string()),
        })?;
        let identity = HostIdentity {
            name: "paneflow-host".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: HOST_PROTOCOL_VERSION,
            build_id: crate::protocol::host_build_id(),
            host_instance: HostInstanceToken::new(),
            engine: local_engine_identity(),
            pid: std::process::id(),
            home: home.display().to_string(),
            endpoint: endpoint.display().to_string(),
            started_at_ms: now_ms(),
        };
        let helper_dir = match crate::helpers::current_hook_dir(home) {
            Ok(dir) => Some(dir),
            Err(error) => {
                log::warn!(
                    "paneflow-host: {error}; agent hooks will not be reachable from hosted sessions"
                );
                None
            }
        };
        let (permissions, submit_paste_delay) = load_control_settings(home);
        let persistence = Persistence::start(home).map_err(|error| {
            HostError::Storage(format!("cannot start the persistence service: {error}"))
        })?;
        let host = Arc::new_cyclic(|weak| Self {
            home: home.to_path_buf(),
            identity,
            persistence,
            staging: Arc::new(CheckpointStaging::default()),
            streaming_connections: AtomicUsize::new(0),
            sessions: Mutex::new(BTreeMap::new()),
            agent_bus: AgentBus::new(),
            helper_dir,
            permissions,
            submit_paste_delay,
            next_operation: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
            #[cfg(test)]
            barrier: Mutex::new(None),
            #[cfg(test)]
            fail_launch_owner_spawn: AtomicBool::new(false),
            weak: weak.clone(),
            _owner: owner,
        });
        host.adopt_previous_records();
        host.trim_terminated_records();
        host.write_instance_record()?;
        crate::viewport_scan::spawn(&host);
        crate::cancellation_scan::spawn(&host);
        crate::maintenance::spawn(&host);
        Ok(host)
    }

    pub fn run_maintenance(&self) {
        self.trim_terminated_records();
        let evicted =
            crate::cold_text::enforce_budget(&self.home, crate::cold_text::COLD_TEXT_BUDGET_BYTES);
        if evicted > 0 {
            log::info!(
                "paneflow-host: evicted {evicted} final output file(s) past the cold budget"
            );
        }
    }

    pub fn wake_followers(&self) {
        let streams: Vec<_> = self
            .lock_sessions()
            .values()
            .filter_map(|record| record.runtime.as_deref().map(SessionRuntime::stream))
            .collect();
        for stream in streams {
            stream.wake();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_barrier(&self, hook: BarrierHook) {
        *self
            .barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
    }

    #[cfg(test)]
    fn barrier(&self, point: Barrier) {
        let hook = self
            .barrier
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(point);
        }
    }

    #[cfg(not(test))]
    fn barrier(&self, _point: ()) {}

    fn next_operation(&self) -> OperationId {
        self.next_operation.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn fenced_input_targets(&self) -> Vec<FencedInputTarget> {
        self.lock_sessions()
            .iter()
            .filter(|(_, record)| record.escape_fence && record.is_live())
            .map(|(session, record)| (session.clone(), Arc::clone(&record.input)))
            .collect()
    }

    pub(crate) fn commit_marker<T: Send + 'static>(
        &self,
        session: &SessionId,
        generation: SessionGeneration,
        write: impl FnOnce(&Path) -> T + Send + 'static,
    ) -> Option<T> {
        let (manifest, seed) = {
            let sessions = self.lock_sessions();
            let record = sessions.get(session)?;
            (Arc::clone(&record.manifest), Arc::clone(&record.seed))
        };
        let directory = self.session_data_dir(session);
        let named = session.clone();
        let written = self.persistence.run_exclusive(CRITICAL_DEADLINE, move || {
            let ledger = seed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if ledger.removed {
                return None;
            }
            let current = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .generation;
            if current != generation {
                log::debug!(
                    "paneflow-host: marker for session {named} generation {generation} dropped; the session is at {current}"
                );
                return None;
            }
            Some(write(&directory))
        });
        match written {
            Ok(marker) => marker,
            Err(error) => {
                log::warn!("paneflow-host: marker of session {session} not recorded: {error}");
                None
            }
        }
    }

    pub fn persistence_report(&self) -> QueueReport {
        self.persistence.report()
    }

    pub(crate) fn streaming_connections(&self) -> &AtomicUsize {
        &self.streaming_connections
    }

    pub fn resource_report(&self) -> ResourceReport {
        let sessions = self.lock_sessions();
        let live_runtimes = sessions.values().filter(|record| record.is_live()).count();
        let pending_launches = sessions
            .values()
            .filter(|record| record.launch.is_some())
            .count();
        let per_session = sessions
            .iter()
            .filter_map(|(session, record)| {
                let runtime = record.runtime.as_deref()?;
                Some(SessionResources {
                    session: session.clone(),
                    generation: runtime.generation(),
                    runtime: runtime.resources(),
                })
            })
            .collect();
        drop(sessions);
        ResourceReport {
            persistence: self.persistence.report(),
            checkpoints: self.staging.report(),
            live_runtimes,
            pending_launches,
            sessions: per_session,
        }
    }

    pub(crate) fn announce_cancellation(
        &self,
        session: &SessionId,
        generation: SessionGeneration,
        marker: &crate::hook_assets::Cancellation,
    ) {
        self.agent_bus.broadcast(&json!({
            "type": "cancellation",
            "session": session,
            "generation": generation,
            "runtime_generation": marker.runtime_generation,
            "cancelled_at": marker.cancelled_at,
            "submitted_at": marker.submitted_at,
        }));
    }

    pub fn session_data_dir(&self, session: &SessionId) -> PathBuf {
        paneflow_home::host_session_data_dir_in(&self.home, session.as_str())
    }

    pub(crate) fn live_scan_targets(&self) -> Vec<ScanTarget> {
        self.reconcile_late_launches();
        self.lock_sessions()
            .iter()
            .filter(|(_, record)| record.is_live())
            .filter_map(|(session, record)| {
                Some(ScanTarget {
                    session: session.clone(),
                    generation: record.generation(),
                    manifest: Arc::clone(&record.manifest),
                    runtime: record.runtime.clone()?,
                })
            })
            .collect()
    }

    pub fn retire(&self) {
        let path = paneflow_home::host_instance_record_path_in(&self.home);
        let removed = self.persistence.run_exclusive(CRITICAL_DEADLINE, move || {
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!(
                    "cannot remove the instance record {}: {error}",
                    path.display()
                )),
            }
        });
        match removed {
            Ok(Ok(())) => {}
            Ok(Err(error)) => log::warn!("paneflow-host: {error}"),
            Err(error) => log::warn!("paneflow-host: instance record not retired: {error}"),
        }
    }

    pub fn identity(&self) -> &HostIdentity {
        &self.identity
    }

    pub fn instance(&self) -> &HostInstanceToken {
        &self.identity.host_instance
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn helper_dir(&self) -> Option<&Path> {
        self.helper_dir.as_deref()
    }

    pub fn permissions(&self) -> crate::control::ControlPermissions {
        self.permissions
    }

    pub fn submit_paste_delay(&self) -> std::time::Duration {
        self.submit_paste_delay
    }

    pub fn subscribe_agents(&self) -> AgentSubscription {
        self.agent_bus.subscribe()
    }

    pub fn unsubscribe_agents(&self, id: u64) {
        self.agent_bus.unsubscribe(id);
    }

    pub fn publish_agent_frame(&self, frame: &Value) {
        self.agent_bus.broadcast(frame);
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    pub fn pending_launches(&self) -> usize {
        self.lock_sessions()
            .values()
            .filter(|record| record.launch.is_some())
            .count()
    }

    fn write_instance_record(&self) -> Result<(), HostError> {
        let path = paneflow_home::host_instance_record_path_in(&self.home);
        let json = serde_json::to_vec_pretty(&self.identity)
            .map_err(|e| HostError::Storage(e.to_string()))?;
        self.persistence
            .run_exclusive(CRITICAL_DEADLINE, move || {
                write_atomically(&path, &json).map_err(|e| e.to_string())
            })?
            .map_err(|e| HostError::Storage(format!("cannot write the instance record: {e}")))
    }

    fn adopt_previous_records(&self) {
        let paths = match crate::manifest::list_manifest_paths(&self.home) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!("paneflow-host: cannot list session manifests: {error}");
                return;
            }
        };
        let mut adopted = Vec::new();
        let mut rewrites = Vec::new();
        for path in paths {
            let mut manifest = match read_manifest(&path) {
                Ok(manifest) => manifest,
                Err(ManifestError::Io(error)) => {
                    log::warn!(
                        "paneflow-host: skipping unreadable manifest {}: {error}",
                        path.display()
                    );
                    continue;
                }
                Err(error) => {
                    log::warn!(
                        "paneflow-host: leaving unsupported manifest {} untouched: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            let adopted_at = now_ms();
            let mut rewrite = false;
            if manifest.lifecycle.holds_ownership() {
                manifest.lifecycle = SessionLifecycle::Lost;
                manifest.updated_at_ms = adopted_at;
                rewrite = true;
            }
            if manifest.host_protocol_version != HOST_PROTOCOL_VERSION {
                manifest.host_protocol_version = HOST_PROTOCOL_VERSION;
                rewrite = true;
            }
            let build_id = crate::protocol::host_build_id();
            if manifest.host_build_id != build_id {
                manifest.host_build_id = build_id;
                rewrite = true;
            }
            if rewrite {
                rewrites.push(manifest.clone());
            }
            adopted.push(manifest);
        }
        {
            let mut sessions = self.lock_sessions();
            for manifest in adopted {
                sessions.insert(
                    manifest.session.clone(),
                    SessionRecord::fresh(Arc::new(Mutex::new(manifest))),
                );
            }
        }
        for manifest in rewrites {
            if let Err(error) = self.persist_record(&manifest.session, WriteClass::Critical) {
                log::warn!(
                    "paneflow-host: cannot record the adopted session {}: {error}",
                    manifest.session
                );
            }
        }
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, SessionRecord>> {
        self.sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn summary_of(&self, record: &SessionRecord) -> SessionSummary {
        let manifest = record
            .manifest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let live = record.is_live();
        let owned = manifest.host_instance == self.identity.host_instance;
        let durability_error = record.durability.error();
        SessionSummary {
            manifest,
            live,
            owned,
            pending_launch: record.launch.is_some(),
            launch_operation: record.launch.as_ref().map(|launch| launch.operation),
            descendants_unresolved: record
                .runtime
                .as_deref()
                .map_or(0, SessionRuntime::descendants_unresolved),
            durability_error,
        }
    }

    fn trim_terminated_records(&self) {
        self.trim_terminated_records_at(now_ms());
    }

    pub fn trim_terminated_records_at(&self, now: u64) {
        let dropped = {
            let mut sessions = self.lock_sessions();
            let dropped: Vec<(SessionId, Arc<Mutex<SeedLedger>>, Durability)> = sessions
                .iter()
                .filter(|(_, record)| !record.owns_process())
                .filter_map(|(id, record)| {
                    let manifest = record.manifest.lock().ok()?;
                    let max_age = record_max_age_ms(&manifest.lifecycle)?;
                    let stale = now.saturating_sub(manifest.updated_at_ms) > max_age;
                    stale.then(|| {
                        (
                            id.clone(),
                            Arc::clone(&record.seed),
                            Arc::clone(&record.durability),
                        )
                    })
                })
                .collect();
            if dropped.is_empty() {
                return;
            }
            for (id, _, _) in &dropped {
                sessions.remove(id);
            }
            dropped
        };
        for (id, seed, durability) in &dropped {
            self.delete_record_files(id, seed, durability);
        }
        log::info!(
            "paneflow-host: dropped {} session records past their retention",
            dropped.len()
        );
    }

    fn delete_record_files(
        &self,
        session: &SessionId,
        seed: &Arc<Mutex<SeedLedger>>,
        durability: &Durability,
    ) {
        seed.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .removed = true;
        if let Err(error) = self
            .persistence
            .remove_and_wait(session, durability, CRITICAL_DEADLINE)
        {
            log::warn!("paneflow-host: cannot delete the record of {session}: {error}");
        }
    }

    pub fn list(&self, workspace: Option<&WorkspaceId>) -> Vec<SessionSummary> {
        let sessions = self.lock_sessions();
        sessions
            .values()
            .map(|record| self.summary_of(record))
            .filter(|summary| {
                workspace.is_none_or(|wanted| summary.manifest.workspace.as_ref() == Some(wanted))
            })
            .map(without_launch_environment)
            .collect()
    }

    pub fn rows(
        &self,
        workspace: Option<&WorkspaceId>,
        inactive_per_workspace: usize,
    ) -> Vec<SessionRow> {
        let owner = self.instance().clone();
        inactive_window(self.list(workspace), inactive_per_workspace)
            .into_iter()
            .map(|summary| SessionRow::of(summary, &owner))
            .collect()
    }

    pub fn inspect(&self, session: &SessionId) -> Result<SessionSummary, HostError> {
        let sessions = self.lock_sessions();
        sessions
            .get(session)
            .map(|record| self.summary_of(record))
            .ok_or_else(|| HostError::SessionNotFound(session.clone()))
    }

    pub fn agent_snapshot(&self) -> Vec<AgentSnapshotEntry> {
        let output_changes: BTreeMap<SessionId, u64> = {
            let sessions = self.lock_sessions();
            sessions
                .iter()
                .filter_map(|(session, record)| {
                    record.runtime.as_deref().and_then(|runtime| {
                        runtime
                            .output_changed_at_ms()
                            .map(|changed_at| (session.clone(), changed_at))
                    })
                })
                .collect()
        };
        self.list(None)
            .into_iter()
            .map(|summary| AgentSnapshotEntry {
                output_changed_at_ms: output_changes.get(&summary.manifest.session).copied(),
                session: summary.manifest.session,
                generation: summary.manifest.generation,
                launch_shell: summary.manifest.launch.shell.clone(),
                live: summary.live,
                lifecycle: summary.manifest.lifecycle,
                process: summary.manifest.process,
                workspace: summary.manifest.workspace,
                title: summary.manifest.title,
                cwd: Some(summary.manifest.current_cwd.unwrap_or(summary.manifest.cwd)),
                last_hook: summary.manifest.last_hook,
                hook_revision: summary.manifest.hook_revision,
                generation_started_at_ms: summary.manifest.generation_started_at_ms,
                screen_changed_at_ms: summary.manifest.screen_changed_at_ms,
                screen_activity: summary.manifest.screen_activity,
                menu_prompt_active: summary.manifest.menu_prompt_active,
                observed_runtime: summary
                    .manifest
                    .runtime
                    .and_then(|runtime| runtime.current_observation),
                host_protocol_version: summary.manifest.host_protocol_version,
                host_build_id: summary.manifest.host_build_id,
            })
            .collect()
    }

    fn launch_env(
        &self,
        session: &SessionId,
        generation: SessionGeneration,
        workspace: Option<&WorkspaceId>,
        user: &BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        launch_env(
            session,
            generation,
            workspace,
            &self.home,
            Path::new(&self.identity.endpoint),
            self.helper_dir.as_deref(),
            user,
        )
    }

    pub fn ingest_agent_event(&self, event: &AgentEvent) -> Result<IngestOutcome, HostError> {
        let (manifest, seed, durability) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(&event.session)
                .ok_or_else(|| HostError::SessionNotFound(event.session.clone()))?;
            (
                Arc::clone(&record.manifest),
                Arc::clone(&record.seed),
                Arc::clone(&record.durability),
            )
        };
        let mut ledger = seed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ledger.removed {
            return Err(HostError::SessionNotFound(event.session.clone()));
        }
        let key = receipt_key(event);
        let hook_event_name = event
            .payload
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| event.kind.wire_str())
            .to_string();
        let (snapshot, record) = {
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let generation = guard.generation;
            if event.generation != Some(generation) {
                let reason = match event.generation {
                    None => "the event lacks its captured runtime generation",
                    Some(requested) if requested < generation => {
                        "the event names a generation this session has left"
                    }
                    Some(_) => "the event names a generation this session has not reached",
                };
                log::warn!(
                    "agent event rejected for session {}: runtime generation {:?} does not match current generation {}: {reason}",
                    event.session,
                    event.generation,
                    generation
                );
                return Ok(IngestOutcome {
                    ack: json!({
                        "accepted": false,
                        "reason": reason,
                        "session": event.session,
                        "generation": generation,
                        "revision": guard.hook_revision,
                    }),
                    frame: None,
                });
            }
            if guard.host_instance != self.identity.host_instance {
                return Ok(IngestOutcome {
                    ack: json!({
                        "accepted": false,
                        "reason": "the event belongs to a previous host instance",
                        "session": event.session,
                        "generation": generation,
                        "revision": guard.hook_revision,
                    }),
                    frame: None,
                });
            }
            if let Some((_, revision)) = ledger.receipts.iter().find(|(held, _)| *held == key) {
                let revision = *revision;
                let snapshot = guard.clone();
                drop(guard);
                let persistence_error = self
                    .persist(&manifest, &durability, WriteClass::Critical)
                    .err()
                    .map(|error| error.to_string());
                return Ok(IngestOutcome {
                    ack: json!({
                        "accepted": true,
                        "duplicate": true,
                        "durable": persistence_error.is_none(),
                        "persistence_error": persistence_error,
                        "session": event.session,
                        "generation": generation,
                        "revision": revision,
                        "last_hook": snapshot.last_hook,
                    }),
                    frame: None,
                });
            }
            if !paneflow_ipc_client::agent::accepts_event(
                guard.last_hook.as_ref().and_then(|hook| hook.emitted_at_ms),
                event.emitted_at_ms,
            ) {
                return Ok(IngestOutcome {
                    ack: json!({
                        "accepted": false,
                        "reason": "an out-of-order event never replaces newer accepted state",
                        "session": event.session,
                        "generation": generation,
                        "revision": guard.hook_revision,
                    }),
                    frame: None,
                });
            }
            let received_at_ms = event.received_at_ms.unwrap_or_else(now_ms);
            let revision = guard.hook_revision.saturating_add(1);
            let mut accepted_frame = event.to_frame(generation);
            accepted_frame["revision"] = json!(revision);
            accepted_frame["received_at_ms"] = json!(received_at_ms);
            let activity_event = match event.kind {
                crate::agent::AgentEventKind::SessionStart
                | crate::agent::AgentEventKind::ToolUse
                | crate::agent::AgentEventKind::SessionEnd => guard
                    .last_hook
                    .as_ref()
                    .and_then(|hook| hook.activity_event.clone()),
                _ => Some(accepted_frame.clone()),
            };
            let record = crate::manifest::HookRecord {
                event: Some(accepted_frame),
                activity_event,
                hook_event_name: hook_event_name.clone(),
                tool: event.tool.clone(),
                tool_name: event.tool_name.clone(),
                pid: event.pid,
                runtime_generation: generation,
                provider_session_id: payload_text(event, "session_id"),
                transcript_path: payload_text(event, "transcript_path"),
                emitted_at_ms: event.emitted_at_ms,
                received_at_ms,
            };
            let mut snapshot = guard.clone();
            snapshot.last_hook = Some(record.clone());
            snapshot.hook_revision = revision;
            snapshot.updated_at_ms = now_ms();
            let encoded = serde_json::to_vec_pretty(&snapshot)
                .map_err(|error| HostError::InvalidRequest(error.to_string()))?;
            if encoded.len() as u64 > crate::manifest::MAX_MANIFEST_BYTES {
                return Err(HostError::InvalidRequest(
                    "the event exceeds the recoverable session manifest size limit".to_string(),
                ));
            }
            *guard = snapshot.clone();
            (snapshot, record)
        };
        let revision = snapshot.hook_revision;
        let generation = snapshot.generation;
        let persistence_error = self
            .persist(&manifest, &durability, WriteClass::Critical)
            .err()
            .map(|error| error.to_string());
        if ledger.receipts.len() >= MAX_HOOK_RECEIPTS {
            ledger.receipts.pop_front();
        }
        ledger.receipts.push_back((key, revision));
        let frame = record.event.clone();
        if let Some(frame) = frame.as_ref() {
            self.publish_agent_frame(frame);
        }
        drop(ledger);
        Ok(IngestOutcome {
            ack: json!({
                "accepted": true,
                "durable": persistence_error.is_none(),
                "persistence_error": persistence_error,
                "session": event.session,
                "generation": generation,
                "revision": revision,
                "received_at_ms": event.received_at_ms,
                "last_hook": record,
            }),
            frame,
        })
    }

    pub fn create(&self, request: CreateSession) -> Result<SessionSummary, HostError> {
        if request.args.len() > MAX_LAUNCH_ARGS {
            return Err(HostError::InvalidRequest(format!(
                "at most {MAX_LAUNCH_ARGS} launch arguments are accepted"
            )));
        }
        if request.env.len() > MAX_LAUNCH_ENV_ENTRIES {
            return Err(HostError::InvalidRequest(format!(
                "at most {MAX_LAUNCH_ENV_ENTRIES} environment entries are accepted"
            )));
        }
        if self.is_shutting_down() {
            return Err(HostError::ShuttingDown);
        }
        let session = request.session.clone().unwrap_or_default();
        let cwd = resolve_cwd(request.cwd.as_deref());
        let shell = match request
            .shell
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(shell) => shell.to_string(),
            None => default_shell(),
        };
        let cols = request.cols.filter(|c| *c > 0).unwrap_or(DEFAULT_COLS);
        let rows = request.rows.filter(|r| *r > 0).unwrap_or(DEFAULT_ROWS);
        validate_dimensions(cols, rows)?;
        let env = self.launch_env(
            &session,
            SessionGeneration::FIRST,
            request.workspace.as_ref(),
            &request.env,
        );
        let launch = SessionLaunch {
            shell: shell.clone(),
            args: request.args.clone(),
            env: request
                .env
                .iter()
                .filter(|(key, _)| crate::env::is_valid_env_name(key))
                .filter(|(key, _)| !crate::env::is_forbidden_child_env_key(key))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            cols,
            rows,
        };
        let created = now_ms();
        let manifest = Arc::new(Mutex::new(SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session: session.clone(),
            workspace: request.workspace.clone(),
            generation: SessionGeneration::FIRST,
            host_instance: self.identity.host_instance.clone(),
            cwd: cwd.display().to_string(),
            launch,
            lifecycle: SessionLifecycle::Starting,
            process: None,
            title: request.title.clone(),
            current_cwd: None,
            last_hook: None,
            hook_revision: 0,
            generation_started_at_ms: Some(created),
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            final_output: None,
            host_protocol_version: HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: created,
            updated_at_ms: created,
        }));
        let operation = self.next_operation();
        let durability;
        {
            let mut sessions = self.lock_sessions();
            if sessions.contains_key(&session) {
                return Err(HostError::SessionExists { session });
            }
            self.admit_launch(&sessions)?;
            let mut record = SessionRecord::fresh(Arc::clone(&manifest));
            record.launch = Some(PendingLaunch {
                operation,
                generation: SessionGeneration::FIRST,
                cancel: None,
                cancelled: false,
                fallback_owner: None,
            });
            self.persistence.reserve_final(&record.durability);
            durability = Arc::clone(&record.durability);
            sessions.insert(session.clone(), record);
        }
        let session_dir = self.session_data_dir(&session);
        let prepared = std::fs::create_dir_all(&session_dir)
            .map_err(|error| {
                HostError::Storage(format!(
                    "cannot create session data directory {}: {error}",
                    session_dir.display()
                ))
            })
            .and_then(|()| {
                self.persist(&manifest, &durability, WriteClass::Critical)
                    .map_err(|error| {
                        HostError::Storage(format!("cannot write the session manifest: {error}"))
                    })
            });
        if let Err(error) = prepared {
            let mut sessions = self.lock_sessions();
            if sessions
                .get(&session)
                .and_then(|record| record.launch.as_ref())
                .is_some_and(|launch| launch.operation == operation)
            {
                sessions.remove(&session);
            }
            drop(sessions);
            self.persistence.remove(&session, &durability);
            return Err(error);
        }

        let spec = SpawnSpec {
            shell,
            args: request.args,
            cwd,
            env,
            cols,
            rows,
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
        };
        let created = self.launch(session, manifest, spec, SessionGeneration::FIRST, operation);
        if created.is_ok() {
            self.trim_terminated_records();
        }
        created
    }

    fn admit_launch(&self, sessions: &BTreeMap<SessionId, SessionRecord>) -> Result<(), HostError> {
        if self.is_shutting_down() {
            return Err(HostError::ShuttingDown);
        }
        let pending = sessions
            .values()
            .filter(|record| record.launch.is_some())
            .count();
        if pending >= MAX_PENDING_LAUNCHES {
            return Err(HostError::Busy(format!(
                "{pending} launches are still unresolved; retry when one finishes"
            )));
        }
        Ok(())
    }

    pub fn restart(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<SessionSummary, HostError> {
        if self.is_shutting_down() {
            return Err(HostError::ShuttingDown);
        }
        let prior = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let current = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(requested) = generation
                && requested != current.generation
            {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current: current.generation,
                    requested,
                });
            }
            self.refuse_replacement(session, record, &current)?;
            current
        };
        let resumed_cwd = prior
            .current_cwd
            .as_deref()
            .filter(|cwd| Path::new(cwd).is_dir())
            .unwrap_or(prior.cwd.as_str())
            .to_string();
        let cwd = resolve_cwd(Some(&resumed_cwd));
        let next = prior.generation.next();
        let operation = self.next_operation();
        let seed = {
            let sessions = self.lock_sessions();
            Arc::clone(
                &sessions
                    .get(session)
                    .ok_or_else(|| HostError::SessionNotFound(session.clone()))?
                    .seed,
            )
        };
        let mut ledger = seed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if ledger.removed {
            return Err(HostError::SessionNotFound(session.clone()));
        }
        let (manifest, spec, durability) = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let current = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if current.generation != prior.generation {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current: current.generation,
                    requested: prior.generation,
                });
            }
            self.refuse_replacement(session, record, &current)?;
            self.admit_launch(&sessions)?;
            let record = sessions
                .get_mut(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let manifest = Arc::clone(&record.manifest);
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.generation = next;
            guard.host_instance = self.identity.host_instance.clone();
            guard.lifecycle = SessionLifecycle::Starting;
            guard.process = None;
            guard.current_cwd = None;
            guard.last_hook = None;
            guard.generation_started_at_ms = Some(now_ms());
            guard.screen_changed_at_ms = None;
            guard.screen_activity = None;
            guard.menu_prompt_active = false;
            guard.runtime = None;
            guard.host_protocol_version = HOST_PROTOCOL_VERSION;
            guard.host_build_id = crate::protocol::host_build_id();
            guard.launch.args.clear();
            guard.cwd = cwd.display().to_string();
            guard.updated_at_ms = now_ms();
            let env = self.launch_env(
                &guard.session,
                next,
                guard.workspace.as_ref(),
                &guard.launch.env,
            );
            let spec = SpawnSpec {
                shell: guard.launch.shell.clone(),
                args: Vec::new(),
                cwd,
                env,
                cols: guard.launch.cols,
                rows: guard.launch.rows,
                scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            };
            drop(guard);
            record.launch = Some(PendingLaunch {
                operation,
                generation: next,
                cancel: None,
                cancelled: false,
                fallback_owner: None,
            });
            record.set_escape_fence(false);
            record.durability.clear_error();
            self.persistence.reserve_final(&record.durability);
            (manifest, spec, Arc::clone(&record.durability))
        };
        #[cfg(test)]
        self.barrier(Barrier::RestartPersist);
        #[cfg(not(test))]
        self.barrier(());
        if let Err(error) = self.persist(&manifest, &durability, WriteClass::Critical) {
            let mut sessions = self.lock_sessions();
            if let Some(record) = sessions.get_mut(session)
                && record
                    .launch
                    .as_ref()
                    .is_some_and(|launch| launch.operation == operation)
            {
                record.launch = None;
                let fenced = escape_fence_of(prior.runtime.as_ref());
                *record
                    .manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = prior;
                record.set_escape_fence(fenced);
                drop(sessions);
                self.persist_final(&manifest, &durability);
            }
            return Err(HostError::Storage(format!(
                "cannot write the restart of {session}; the previous state is kept: {error}"
            )));
        }
        ledger.receipts.clear();
        drop(ledger);
        self.launch(session.clone(), manifest, spec, next, operation)
    }

    fn refuse_replacement(
        &self,
        session: &SessionId,
        record: &SessionRecord,
        current: &SessionManifest,
    ) -> Result<(), HostError> {
        if record.launch.is_some() {
            return Err(HostError::LaunchPending(session.clone()));
        }
        if record.is_live() || current.lifecycle == SessionLifecycle::Starting {
            return Err(HostError::SessionLive(session.clone()));
        }
        if let Some(reason) = record
            .runtime
            .as_deref()
            .and_then(SessionRuntime::unverified)
        {
            return Err(HostError::OwnershipUnresolved {
                session: session.clone(),
                reason,
            });
        }
        if let Some(runtime) = record.runtime.as_deref()
            && runtime.descendants_unresolved() > 0
        {
            return Err(HostError::OwnershipUnresolved {
                session: session.clone(),
                reason: format!(
                    "{} descendant process(es) of the previous generation are unresolved",
                    runtime.descendants_unresolved()
                ),
            });
        }
        if let SessionLifecycle::Unverified { reason } = &current.lifecycle
            && current.host_instance == self.identity.host_instance
        {
            return Err(HostError::OwnershipUnresolved {
                session: session.clone(),
                reason: reason.clone(),
            });
        }
        Ok(())
    }

    fn launch(
        &self,
        session: SessionId,
        manifest: Arc<Mutex<SessionManifest>>,
        spec: SpawnSpec,
        generation: SessionGeneration,
        operation: OperationId,
    ) -> Result<SessionSummary, HostError> {
        let durability = {
            let sessions = self.lock_sessions();
            sessions
                .get(&session)
                .map(|record| Arc::clone(&record.durability))
                .unwrap_or_default()
        };
        let observer = self.observer_for(Arc::clone(&manifest), durability, generation);
        let handle = match SessionRuntime::launch(spec, generation, observer) {
            Ok(handle) => handle,
            Err(error) => return Err(self.fail_launch(&session, operation, error.0)),
        };
        {
            let mut sessions = self.lock_sessions();
            match sessions.get_mut(&session).and_then(|record| {
                record
                    .launch
                    .as_mut()
                    .filter(|launch| launch.operation == operation)
            }) {
                Some(launch) => {
                    launch.cancel = Some(handle.canceller());
                    if launch.cancelled {
                        handle.cancel();
                    }
                }
                None => {
                    handle.cancel();
                    drop(sessions);
                    self.own_late_launch(session.clone(), operation, handle);
                    return Err(HostError::SessionNotFound(session));
                }
            }
        }
        match handle.wait(STARTUP_DEADLINE) {
            LaunchWait::Ready(runtime) | LaunchWait::Recovery(runtime) => {
                self.commit_launch(&session, operation, runtime)
            }
            LaunchWait::Failed(reason) => Err(self.fail_launch(&session, operation, reason)),
            LaunchWait::Pending(handle) => {
                log::warn!(
                    "paneflow-host: session {session} did not start within {STARTUP_DEADLINE:?}; the launch stays owned until it resolves"
                );
                self.own_late_launch(session.clone(), operation, handle);
                Err(HostError::LaunchPending(session))
            }
        }
    }

    fn own_late_launch(
        &self,
        session: SessionId,
        operation: OperationId,
        handle: crate::runtime::LaunchHandle,
    ) {
        let host = self.weak_self();
        let named = session.clone();
        let retained = Arc::new(Mutex::new(Some(handle)));
        let task_retained = Arc::clone(&retained);
        let task = move || {
            let Some(mut handle) = task_retained
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
            else {
                return;
            };
            let outcome = loop {
                match handle.wait(LATE_LAUNCH_WAIT) {
                    LaunchWait::Pending(again) => handle = again,
                    other => break other,
                }
            };
            let Some(host) = host.upgrade() else {
                if let LaunchWait::Ready(runtime) | LaunchWait::Recovery(runtime) = outcome {
                    let _ = runtime.stop();
                }
                return;
            };
            host.settle_late_launch(&session, operation, outcome);
        };
        #[cfg(test)]
        let fail_spawn = self.fail_launch_owner_spawn.swap(false, Ordering::AcqRel);
        #[cfg(not(test))]
        let fail_spawn = false;
        let spawned = if fail_spawn {
            Err(std::io::Error::other(
                "injected launch owner thread creation failure",
            ))
        } else {
            std::thread::Builder::new()
                .name("paneflow-host-launch-owner".into())
                .spawn(task)
        };
        if let Err(error) = spawned {
            log::error!(
                "paneflow-host: cannot start the late launch owner of {named}: {error}; the existing scan retains recovery"
            );
            let handle = retained
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(launch) = self
                .lock_sessions()
                .get_mut(&named)
                .and_then(|record| record.launch.as_mut())
                .filter(|launch| launch.operation == operation)
            {
                launch.fallback_owner = handle;
            }
        }
    }

    fn reconcile_late_launches(&self) {
        let pending: Vec<_> = self
            .lock_sessions()
            .iter_mut()
            .filter_map(|(session, record)| {
                let launch = record.launch.as_mut()?;
                Some((
                    session.clone(),
                    launch.operation,
                    launch.fallback_owner.take()?,
                ))
            })
            .collect();
        for (session, operation, handle) in pending {
            match handle.wait(std::time::Duration::ZERO) {
                LaunchWait::Pending(handle) => {
                    if let Some(launch) = self
                        .lock_sessions()
                        .get_mut(&session)
                        .and_then(|record| record.launch.as_mut())
                        .filter(|launch| launch.operation == operation)
                    {
                        launch.fallback_owner = Some(handle);
                    }
                }
                outcome => self.settle_late_launch(&session, operation, outcome),
            }
        }
    }

    fn settle_late_launch(&self, session: &SessionId, operation: OperationId, outcome: LaunchWait) {
        match outcome {
            LaunchWait::Ready(runtime) | LaunchWait::Recovery(runtime) => {
                if let Err(error) = self.commit_launch(session, operation, runtime) {
                    log::warn!(
                        "paneflow-host: late launch of session {session} settled with {error}"
                    );
                }
            }
            LaunchWait::Failed(reason) => {
                let error = self.fail_launch(session, operation, reason);
                log::warn!("paneflow-host: late launch of session {session}: {error}");
            }
            LaunchWait::Pending(_) => {}
        }
    }

    fn weak_self(&self) -> Weak<Self> {
        self.weak.clone()
    }

    fn fail_launch(
        &self,
        session: &SessionId,
        operation: OperationId,
        reason: String,
    ) -> HostError {
        let snapshot = {
            let mut sessions = self.lock_sessions();
            let Some(record) = sessions.get_mut(session) else {
                return HostError::SpawnFailed {
                    session: session.clone(),
                    reason,
                };
            };
            let owns = record
                .launch
                .as_ref()
                .is_some_and(|launch| launch.operation == operation);
            if !owns {
                return HostError::SpawnFailed {
                    session: session.clone(),
                    reason,
                };
            }
            let launch_generation = record.launch.take().map(|launch| launch.generation);
            let mut guard = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if Some(guard.generation) == launch_generation {
                guard.lifecycle = SessionLifecycle::Failed {
                    reason: reason.clone(),
                };
                guard.updated_at_ms = now_ms();
                Some((Arc::clone(&record.manifest), Arc::clone(&record.durability)))
            } else {
                None
            }
        };
        if let Some((manifest, durability)) = snapshot {
            self.persist_final(&manifest, &durability);
        }
        HostError::SpawnFailed {
            session: session.clone(),
            reason,
        }
    }

    fn commit_launch(
        &self,
        session: &SessionId,
        operation: OperationId,
        runtime: SessionRuntime,
    ) -> Result<SessionSummary, HostError> {
        #[cfg(test)]
        self.barrier(Barrier::LaunchCommit);
        #[cfg(not(test))]
        self.barrier(());
        let process = runtime.process();
        let runtime = Arc::new(runtime);
        let cancelled = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let launch = record
                .launch
                .as_ref()
                .filter(|launch| launch.operation == operation)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let cancelled = launch.cancelled;
            let mut guard = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let binding =
                launch_binding_for_command(&guard.launch.shell).map(|bound| bound.id.to_string());
            let fenced = binding
                .as_deref()
                .and_then(paneflow_agent_config::runtime_by_id)
                .is_some_and(|bound| bound.lifecycle.escape_cancels_turn);
            guard.runtime = with_launch_binding(guard.runtime.take(), binding);
            guard.process = Some(process);
            guard.lifecycle = if cancelled {
                SessionLifecycle::Unverified {
                    reason: "the cancelled launch is being stopped".into(),
                }
            } else if let Some(reason) = runtime.unverified() {
                SessionLifecycle::Unverified { reason }
            } else if let Some(exit) = runtime.exit() {
                SessionLifecycle::Exited {
                    code: exit.code,
                    signal: exit.signal,
                }
            } else {
                SessionLifecycle::Running
            };
            guard.updated_at_ms = now_ms();
            drop(guard);
            record.runtime = Some(Arc::clone(&runtime));
            record.completed = None;
            record.launch = None;
            record.set_escape_fence(fenced);
            cancelled
        };
        if cancelled {
            return self.stop(session, Some(runtime.generation()));
        }
        if let Err(error) = self.persist_record(session, WriteClass::Critical) {
            log::warn!("paneflow-host: launch of {session} is not durable yet: {error}");
        }
        self.inspect(session)
    }

    pub fn stop(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<SessionSummary, HostError> {
        let deadline = Instant::now() + STOP_ACTION_BUDGET;
        let operation = self.next_operation();
        let host = self.weak_self().upgrade().ok_or(HostError::ShuttingDown)?;
        let named = session.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("paneflow-host-stop".into())
            .spawn(move || {
                let _ = sender.send(host.stop_until(&named, generation, deadline));
            })
            .map_err(|error| HostError::OwnershipUnresolved {
                session: session.clone(),
                reason: format!("cannot start stop operation: {error}"),
            })?;
        receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| {
                Err(HostError::OwnershipUnresolved {
                    session: session.clone(),
                    reason: format!("stop operation {operation} exceeded its deadline; the host retains it for recovery"),
                })
            })
    }

    fn stop_until(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        deadline: Instant,
    ) -> Result<SessionSummary, HostError> {
        let (manifest, runtime, durability) = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get_mut(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let manifest = Arc::clone(&record.manifest);
            let current = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(requested) = generation
                && requested != current.generation
            {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current: current.generation,
                    requested,
                });
            }
            if let Some(launch) = record.launch.as_mut() {
                launch.cancelled = true;
                if let Some(cancel) = launch.cancel.as_ref() {
                    cancel.cancel();
                }
                return Err(HostError::LaunchPending(session.clone()));
            }
            let Some(runtime) = record.runtime.clone() else {
                if current.lifecycle.holds_ownership()
                    || current.lifecycle == SessionLifecycle::Lost
                {
                    return Err(HostError::ProcessUnverified(session.clone()));
                }
                let (manifest, durability) =
                    (Arc::clone(&record.manifest), Arc::clone(&record.durability));
                drop(sessions);
                self.persist_final(&manifest, &durability);
                return self.inspect(session);
            };
            if !runtime.owns_process() && runtime.unverified().is_none() {
                let (manifest, durability) =
                    (Arc::clone(&record.manifest), Arc::clone(&record.durability));
                drop(sessions);
                self.persist_final(&manifest, &durability);
                return self.inspect(session);
            }
            (manifest, runtime, Arc::clone(&record.durability))
        };
        let report = match runtime.stop_until(deadline) {
            Ok(report) => report,
            Err(error) => StopReport {
                exit: runtime.exit(),
                descendants_unresolved: runtime.descendants_unresolved(),
                unverified: Some(format!("the stop could not be confirmed: {error}")),
            },
        };
        #[cfg(test)]
        self.barrier(Barrier::StopCommit);
        #[cfg(not(test))]
        self.barrier(());
        let settled = {
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.generation == runtime.generation() {
                guard.lifecycle = lifecycle_after_stop(&report);
                guard.updated_at_ms = now_ms();
                true
            } else {
                log::debug!(
                    "paneflow-host: stop outcome of session {session} generation {} dropped; the session is at {}",
                    runtime.generation(),
                    guard.generation
                );
                false
            }
        };
        if settled {
            self.persist_final(&manifest, &durability);
        }
        if let Some(reason) = report.unverified {
            return Err(HostError::OwnershipUnresolved {
                session: session.clone(),
                reason,
            });
        }
        self.inspect(session)
    }

    pub fn remove(&self, session: &SessionId) -> Result<SessionManifest, HostError> {
        let (removed, seed, durability) = {
            let mut sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let manifest = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if record.launch.is_some() {
                return Err(HostError::LaunchPending(session.clone()));
            }
            if record.is_live() || manifest.lifecycle.is_running() {
                return Err(HostError::SessionLive(session.clone()));
            }
            if record.owns_process() && manifest.host_instance == self.identity.host_instance {
                let reason = record
                    .runtime
                    .as_deref()
                    .and_then(SessionRuntime::unverified)
                    .or_else(|| match &manifest.lifecycle {
                        SessionLifecycle::Unverified { reason } => Some(reason.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "descendant processes are unresolved".to_string());
                return Err(HostError::OwnershipUnresolved {
                    session: session.clone(),
                    reason,
                });
            }
            let seed = Arc::clone(&record.seed);
            let durability = Arc::clone(&record.durability);
            sessions.remove(session);
            (manifest, seed, durability)
        };
        seed.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .removed = true;
        self.persistence
            .remove_and_wait(session, &durability, CRITICAL_DEADLINE)?;
        Ok(removed)
    }

    fn with_live_runtime<T>(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        op: impl FnOnce(&SessionRuntime) -> Result<T, RuntimeError>,
    ) -> Result<T, HostError> {
        let runtime = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            if let Some(requested) = generation {
                let current = record.generation();
                if requested != current {
                    return Err(HostError::GenerationMismatch {
                        session: session.clone(),
                        current,
                        requested,
                    });
                }
            }
            if record.launch.is_some() {
                return Err(HostError::LaunchPending(session.clone()));
            }
            record
                .runtime
                .clone()
                .ok_or_else(|| HostError::SessionNotLive(session.clone()))?
        };
        op(&runtime).map_err(HostError::from)
    }

    pub fn checkpoint(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<StagedCheckpoint, HostError> {
        let estimate =
            self.with_live_runtime(session, generation, SessionRuntime::checkpoint_size)?;
        let mut lease = self
            .staging
            .admit(estimate, Instant::now() + CHECKPOINT_ADMISSION_DEADLINE)?;
        let checkpoint = self.with_live_runtime(session, generation, SessionRuntime::checkpoint)?;
        lease.adjust(checkpoint.snapshot.len());
        Ok(StagedCheckpoint {
            checkpoint,
            _lease: lease,
        })
    }

    pub fn text(&self, session: &SessionId) -> Result<SessionText, HostError> {
        let (runtime, final_output) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            if record.launch.is_some() {
                return Err(HostError::LaunchPending(session.clone()));
            }
            let live = record.runtime.clone().filter(|runtime| !runtime.retired());
            let final_output = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .final_output
                .clone();
            (live, final_output)
        };
        if let Some(runtime) = runtime {
            let text = runtime.text()?;
            return Ok(SessionText {
                text,
                live: true,
                available: true,
                complete: false,
            });
        }
        let Some(final_output) = final_output else {
            return Err(HostError::SessionNotLive(session.clone()));
        };
        let text = final_output
            .text_available
            .then(|| crate::cold_text::read(&self.home, session))
            .flatten();
        Ok(SessionText {
            available: text.is_some(),
            text: text.unwrap_or_default(),
            live: false,
            complete: final_output.complete,
        })
    }

    pub fn output_stream(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<Result<Arc<crate::stream::OutputStream>, OutputSlice>, HostError> {
        let sessions = self.lock_sessions();
        let record = sessions
            .get(session)
            .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
        if let Some(requested) = generation {
            let current = record.generation();
            if requested != current {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current,
                    requested,
                });
            }
        }
        if record.launch.is_some() {
            return Err(HostError::LaunchPending(session.clone()));
        }
        if let Some(runtime) = record.runtime.as_deref() {
            return Ok(Ok(runtime.stream()));
        }
        let completed = record
            .completed
            .ok_or_else(|| HostError::SessionNotLive(session.clone()))?;
        Ok(Err(OutputSlice {
            offset: completed.final_offset,
            data: Vec::new(),
            end_offset: completed.final_offset,
            live: false,
        }))
    }

    pub fn bracketed_paste_enabled(&self, session: &SessionId) -> Result<bool, HostError> {
        self.with_live_runtime(session, None, SessionRuntime::bracketed_paste_enabled)
    }

    pub fn output(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        from: u64,
        max: usize,
    ) -> Result<OutputSlice, HostError> {
        match self.output_stream(session, generation)? {
            Ok(stream) => stream
                .read(from, max)
                .map_err(|evicted| RuntimeError::OutputEvicted {
                    requested: evicted.requested,
                    tail_start: evicted.tail_start,
                    tail_end: evicted.tail_end,
                })
                .map_err(HostError::from),
            Err(end) if from == end.end_offset => Ok(end),
            Err(end) => Err(HostError::Runtime(RuntimeError::OutputEvicted {
                requested: from,
                tail_start: end.end_offset,
                tail_end: end.end_offset,
            })),
        }
    }

    pub fn input(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        bytes: Vec<u8>,
    ) -> Result<usize, HostError> {
        let fenced = {
            let sessions = self.lock_sessions();
            sessions
                .get(session)
                .filter(|record| record.escape_fence)
                .map(|record| (Arc::clone(&record.input), record.generation()))
        };
        let observed = fenced.as_ref().map(|_| bytes.clone());
        let accepted =
            self.with_live_runtime(session, generation, |runtime| runtime.input(bytes))?;
        if let (Some((input, captured)), Some(observed)) = (fenced, observed) {
            input
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .observe(&observed, std::time::SystemTime::now(), captured);
        }
        Ok(accepted)
    }

    pub fn bind_runtime(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        runtime_id: Option<&str>,
    ) -> Result<Value, HostError> {
        let bound = match runtime_id {
            None => None,
            Some(id) => Some(paneflow_agent_config::runtime_by_id(id).ok_or_else(|| {
                HostError::InvalidRequest(format!("{id} is not a catalog runtime"))
            })?),
        };
        let (manifest, durability, current) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let current = record.generation();
            if let Some(requested) = generation
                && requested != current
            {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current,
                    requested,
                });
            }
            (
                Arc::clone(&record.manifest),
                Arc::clone(&record.durability),
                current,
            )
        };
        let binding = bound.map(|bound| bound.id.to_string());
        self.commit(
            &manifest,
            &durability,
            Some(current),
            WriteClass::Metadata,
            |m| {
                m.runtime = with_launch_binding(m.runtime.take(), binding);
            },
        );
        let fenced = bound.is_some_and(|bound| bound.lifecycle.escape_cancels_turn);
        if let Some(record) = self.lock_sessions().get_mut(session)
            && record.generation() == current
            && Arc::ptr_eq(&record.manifest, &manifest)
        {
            record.set_escape_fence(fenced);
        }
        Ok(json!({
            "session": session,
            "launch_binding": bound.map(|bound| bound.id),
            "escape_cancels_turn": fenced,
        }))
    }

    pub fn resize(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        cols: u16,
        rows: u16,
    ) -> Result<(), HostError> {
        let target = {
            let sessions = self.lock_sessions();
            sessions.get(session).map(|record| {
                (
                    Arc::clone(&record.manifest),
                    Arc::clone(&record.durability),
                    record.generation(),
                )
            })
        };
        validate_dimensions(cols, rows)?;
        let expected = target.as_ref().map(|(_, _, current)| *current);
        self.with_live_runtime(session, generation.or(expected), |runtime| {
            runtime.resize(cols, rows)
        })?;
        if let Some((manifest, durability, current)) = target {
            let changed = {
                let guard = manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.launch.cols != cols || guard.launch.rows != rows
            };
            if changed {
                self.commit(
                    &manifest,
                    &durability,
                    Some(current),
                    WriteClass::Metadata,
                    |m| {
                        m.launch.cols = cols;
                        m.launch.rows = rows;
                    },
                );
            }
        }
        Ok(())
    }

    pub fn live_sessions(&self) -> Vec<SessionSummary> {
        self.list(None).into_iter().filter(|s| s.live).collect()
    }

    pub fn owned_sessions(&self) -> Vec<SessionSummary> {
        self.list(None)
            .into_iter()
            .filter(|s| s.owned && s.owns_process())
            .collect()
    }

    pub fn live_session_count(&self) -> usize {
        self.live_sessions().len()
    }

    pub fn request_shutdown(&self, force: bool) -> Result<ShutdownReport, HostError> {
        let deadline = Instant::now() + STOP_ACTION_BUDGET;
        let operation = self.next_operation();
        let host = self.weak_self().upgrade().ok_or(HostError::ShuttingDown)?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(0);
        std::thread::Builder::new()
            .name("paneflow-host-shutdown".into())
            .spawn(move || {
                let result = host.shutdown_until(force, deadline);
                if sender.send(result).is_err() {
                    host.shutting_down.store(false, Ordering::Release);
                }
            })
            .map_err(|error| HostError::Storage(format!("cannot start shutdown: {error}")))?;
        receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| {
                let sessions = self.owned_sessions().into_iter().map(|summary| UnresolvedSession {
                    session: summary.manifest.session,
                    reason: format!("shutdown operation {operation} exceeded its deadline; generation {} remains owned (launch operation: {:?})", summary.manifest.generation, summary.launch_operation),
                }).collect::<Vec<_>>();
                if sessions.is_empty() {
                    Err(HostError::Storage("shutdown persistence deadline expired; final state remains pending".into()))
                } else {
                    Err(HostError::SessionsUnresolved { sessions })
                }
            })
    }

    fn shutdown_until(&self, force: bool, deadline: Instant) -> Result<ShutdownReport, HostError> {
        let owned = {
            let sessions = self.lock_sessions();
            if self.is_shutting_down() {
                return Err(HostError::ShuttingDown);
            }
            let owned: Vec<_> = sessions
                .values()
                .filter(|record| record.owns_process())
                .map(|record| self.summary_of(record))
                .filter(|summary| summary.owned)
                .collect();
            if !force && !owned.is_empty() {
                return Err(HostError::SessionsLive { count: owned.len() });
            }
            self.shutting_down.store(true, Ordering::Release);
            owned
        };
        let mut ended = Vec::new();
        let mut unresolved = Vec::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut pending = Vec::new();
        for summary in owned {
            let session = summary.manifest.session;
            let host = self.weak_self().upgrade().ok_or(HostError::ShuttingDown)?;
            let sender = sender.clone();
            let named = session.clone();
            let generation = summary.manifest.generation;
            let spawned = std::thread::Builder::new()
                .name("paneflow-host-shutdown-stop".into())
                .spawn(move || {
                    let result = host.stop_until(&named, Some(generation), deadline);
                    let _ = sender.send((named, result));
                });
            if let Err(error) = spawned {
                unresolved.push(UnresolvedSession {
                    session,
                    reason: error.to_string(),
                });
            } else {
                pending.push(session);
            }
        }
        drop(sender);
        while !pending.is_empty() {
            let Ok((session, result)) =
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                break;
            };
            pending.retain(|id| *id != session);
            match result {
                Ok(stopped) if stopped.descendants_unresolved > 0 => {
                    unresolved.push(UnresolvedSession {
                        session,
                        reason: format!(
                            "{} descendant process(es) could not be confirmed terminated",
                            stopped.descendants_unresolved
                        ),
                    });
                }
                Ok(stopped) => ended.push(stopped),
                Err(error) => unresolved.push(UnresolvedSession {
                    session,
                    reason: error.to_string(),
                }),
            }
        }
        unresolved.extend(pending.into_iter().map(|session| UnresolvedSession {
            session,
            reason: "shutdown deadline expired; the stop operation remains owned".into(),
        }));
        if !unresolved.is_empty() {
            self.shutting_down.store(false, Ordering::Release);
            return Err(HostError::SessionsUnresolved {
                sessions: unresolved,
            });
        }
        let remaining: Vec<UnresolvedSession> = self
            .owned_sessions()
            .into_iter()
            .map(|summary| UnresolvedSession {
                reason: unresolved
                    .iter()
                    .find(|entry| entry.session == summary.manifest.session)
                    .map(|entry| entry.reason.clone())
                    .unwrap_or_else(|| "still owned after the stop".to_string()),
                session: summary.manifest.session,
            })
            .collect();
        if remaining.is_empty() {
            let budget = deadline.saturating_duration_since(Instant::now());
            match self.persistence.drain(budget) {
                Ok(()) => Ok(ShutdownReport { ended, unresolved }),
                Err(failures) => {
                    self.shutting_down.store(false, Ordering::Release);
                    Err(HostError::Storage(failures.join("; ")))
                }
            }
        } else {
            self.shutting_down.store(false, Ordering::Release);
            Err(HostError::SessionsUnresolved {
                sessions: remaining,
            })
        }
    }

    fn persist(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        class: WriteClass,
    ) -> Result<(), PersistError> {
        #[cfg(test)]
        self.barrier(Barrier::ManifestPersist);
        let (snapshot, revision) = {
            let guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (guard.clone(), durability.next_revision())
        };
        let record = ManifestRevision::encode(&snapshot, revision, durability, class)?;
        match class {
            WriteClass::Critical => self.persistence.submit_and_wait(record, CRITICAL_DEADLINE),
            WriteClass::Metadata | WriteClass::Final => self.persistence.submit(record),
        }
    }

    fn persist_final(&self, manifest: &Arc<Mutex<SessionManifest>>, durability: &Durability) {
        if let Err(error) = self.persist(manifest, durability, WriteClass::Final) {
            log::warn!("paneflow-host: {error}");
        }
    }

    fn persist_record(&self, session: &SessionId, class: WriteClass) -> Result<(), PersistError> {
        let target = {
            let sessions = self.lock_sessions();
            sessions
                .get(session)
                .map(|record| (Arc::clone(&record.manifest), Arc::clone(&record.durability)))
        };
        match target {
            Some((manifest, durability)) => self.persist(&manifest, &durability, class),
            None => Ok(()),
        }
    }

    pub(crate) fn commit(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        expected_generation: Option<SessionGeneration>,
        class: WriteClass,
        apply: impl FnOnce(&mut SessionManifest),
    ) -> bool {
        let changed = {
            let sessions = self.lock_sessions();
            if !sessions
                .values()
                .any(|record| Arc::ptr_eq(&record.manifest, manifest))
            {
                return false;
            }
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(expected) = expected_generation
                && guard.generation != expected
            {
                log::debug!(
                    "paneflow-host: update for session {} generation {expected} dropped; the session is at {}",
                    guard.session,
                    guard.generation
                );
                return false;
            }
            let before = guard.clone();
            apply(&mut guard);
            let changed = *guard != before;
            if changed {
                guard.updated_at_ms = now_ms();
            }
            changed
        };
        if changed {
            match class {
                WriteClass::Final => self.persist_final(manifest, durability),
                WriteClass::Metadata => {
                    if let Err(error) = self.persist(manifest, durability, WriteClass::Metadata) {
                        log::debug!("paneflow-host: metadata revision deferred: {error}");
                    }
                }
                WriteClass::Critical => {
                    if let Err(error) = self.persist(manifest, durability, WriteClass::Critical) {
                        log::warn!("paneflow-host: {error}");
                    }
                }
            }
        }
        true
    }

    pub(crate) fn commit_scan(
        &self,
        target: &ScanTarget,
        apply: impl FnOnce(&mut SessionManifest),
    ) -> bool {
        let durability = {
            let sessions = self.lock_sessions();
            sessions
                .get(&target.session)
                .map(|record| Arc::clone(&record.durability))
        };
        let Some(durability) = durability else {
            return false;
        };
        self.commit(
            &target.manifest,
            &durability,
            Some(target.generation),
            WriteClass::Metadata,
            apply,
        )
    }

    fn observer_for(
        &self,
        manifest: Arc<Mutex<SessionManifest>>,
        durability: Durability,
        generation: SessionGeneration,
    ) -> RuntimeObserver {
        let host = self.weak_self();
        Arc::new(move |notice| {
            let Some(host) = host.upgrade() else {
                return;
            };
            let session = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .session
                .clone();
            let lifecycle_settled = matches!(notice, RuntimeNotice::Exited(_));
            if let RuntimeNotice::Completed(record) = notice {
                host.complete_session(&session, &manifest, &durability, record);
                return;
            }
            let class = if lifecycle_settled || matches!(notice, RuntimeNotice::Unverified(_)) {
                WriteClass::Final
            } else {
                WriteClass::Metadata
            };
            host.commit(
                &manifest,
                &durability,
                Some(generation),
                class,
                |guard| match notice {
                    RuntimeNotice::Title(title) => guard.title = Some(title),
                    RuntimeNotice::WorkingDirectory(cwd) => guard.current_cwd = Some(cwd),
                    RuntimeNotice::Exited(exit) => {
                        guard.lifecycle = SessionLifecycle::Exited {
                            code: exit.code,
                            signal: exit.signal,
                        };
                    }
                    RuntimeNotice::Unverified(reason) => {
                        guard.lifecycle = SessionLifecycle::Unverified { reason };
                    }
                    RuntimeNotice::Completed(_) => {}
                },
            );
            if lifecycle_settled {
                host.release_retired_runtime(&session, generation);
            }
        })
    }

    fn complete_session(
        &self,
        session: &SessionId,
        manifest: &Arc<Mutex<SessionManifest>>,
        durability: &Durability,
        record: CompletedRecord,
    ) {
        let owned = {
            let sessions = self.lock_sessions();
            sessions
                .get(session)
                .is_some_and(|held| Arc::ptr_eq(&held.manifest, manifest))
                && manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .generation
                    == record.generation
        };
        if !owned {
            log::debug!(
                "paneflow-host: final output of session {session} generation {} dropped; the record moved on",
                record.generation
            );
            return;
        }
        let text_bytes = record.text.len() as u64;
        let text_available = !record.text.is_empty();
        if text_available {
            let home = self.home.clone();
            let host = self.weak_self();
            let text_session = session.clone();
            let text_manifest = Arc::clone(manifest);
            let text_durability = Arc::clone(durability);
            let generation = record.generation;
            let text = record.text;
            self.persistence.spawn_exclusive(move || {
                if let Err(error) = crate::cold_text::write(&home, &text_session, &text) {
                    log::warn!(
                        "paneflow-host: final output of session {text_session} is not retained: {error}"
                    );
                    if let Some(host) = host.upgrade() {
                        host.commit(
                            &text_manifest,
                            &text_durability,
                            Some(generation),
                            WriteClass::Final,
                            |guard| {
                                if let Some(final_output) = guard.final_output.as_mut() {
                                    final_output.text_available = false;
                                }
                            },
                        );
                    }
                }
                Ok(())
            });
        }
        let final_output = crate::manifest::FinalOutput {
            offset: record.final_offset,
            complete: record.complete,
            text_bytes,
            text_available,
        };
        {
            let mut sessions = self.lock_sessions();
            if let Some(held) = sessions
                .get_mut(session)
                .filter(|held| Arc::ptr_eq(&held.manifest, manifest))
            {
                held.completed = Some(CompletedRuntime {
                    generation: record.generation,
                    final_offset: record.final_offset,
                });
            }
        }
        self.commit(
            manifest,
            durability,
            Some(record.generation),
            WriteClass::Final,
            |guard| {
                guard.final_output = Some(final_output);
            },
        );
        self.release_retired_runtime(session, record.generation);
    }

    fn release_retired_runtime(&self, session: &SessionId, generation: SessionGeneration) {
        let mut sessions = self.lock_sessions();
        let Some(record) = sessions.get_mut(session) else {
            return;
        };
        let releasable = record.runtime.as_deref().is_some_and(|runtime| {
            runtime.generation() == generation
                && runtime.retired()
                && runtime.exit().is_some()
                && !runtime.owns_process()
                && runtime.unverified().is_none()
        }) && record
            .completed
            .is_some_and(|completed| completed.generation == generation);
        if releasable {
            record.runtime = None;
        }
    }
}

fn lifecycle_after_stop(report: &StopReport) -> SessionLifecycle {
    if let Some(reason) = &report.unverified {
        return SessionLifecycle::Unverified {
            reason: reason.clone(),
        };
    }
    if report.descendants_unresolved > 0 {
        return SessionLifecycle::Unverified {
            reason: format!(
                "{} descendant process(es) remain unresolved",
                report.descendants_unresolved
            ),
        };
    }
    match &report.exit {
        Some(exit) => SessionLifecycle::Exited {
            code: exit.code,
            signal: exit.signal.clone(),
        },
        None => SessionLifecycle::Unverified {
            reason: "the stop returned neither an exit nor a failure".into(),
        },
    }
}

fn payload_text(event: &AgentEvent, key: &str) -> Option<String> {
    event
        .payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| {
            text.chars()
                .take(crate::agent::MAX_AGENT_TEXT_BYTES)
                .collect()
        })
}

fn resolve_cwd(requested: Option<&str>) -> PathBuf {
    if let Some(raw) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        let path = PathBuf::from(raw);
        if path.is_dir() {
            return path;
        }
        log::warn!(
            "paneflow-host: requested cwd {raw:?} is not a directory; using the home directory"
        );
    }
    dirs_home().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(windows)]
    let raw = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let raw = std::env::var_os("HOME");
    raw.map(PathBuf::from).filter(|p| p.is_dir())
}

pub fn default_shell() -> String {
    let configured = paneflow_config::loader::load_config().default_shell;
    if let Some(shell) = configured
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(resolved) = usable_executable(shell) {
            return resolved;
        }
        log::warn!("paneflow-host: configured default_shell {shell:?} is not usable; falling back");
    }
    platform_default_shell()
}

fn usable_executable(candidate: &str) -> Option<String> {
    let path = PathBuf::from(candidate);
    if path.is_absolute() {
        return path.is_file().then(|| candidate.to_string());
    }
    which::which(candidate)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(unix)]
fn platform_default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .and_then(|s| usable_executable(&s))
        .unwrap_or_else(|| "/bin/sh".to_string())
}

#[cfg(windows)]
fn platform_default_shell() -> String {
    ["pwsh.exe", "powershell.exe"]
        .iter()
        .find_map(|name| usable_executable(name))
        .or_else(|| {
            std::env::var("COMSPEC")
                .ok()
                .filter(|s| PathBuf::from(s).is_file())
        })
        .unwrap_or_else(|| "cmd.exe".to_string())
}

fn load_control_settings(home: &Path) -> (crate::control::ControlPermissions, std::time::Duration) {
    let config = paneflow_config::loader::load_config_from_path(&home.join("paneflow.json"));
    let permissions = crate::control::ControlPermissions::from_environment(
        config.ai_unrestricted_enabled(),
        config.ai_injection_fence_enabled(),
    );
    let delay = std::time::Duration::from_millis(config.resolved_submit_paste_delay_ms());
    (permissions, delay)
}

pub fn launch_env(
    session: &SessionId,
    generation: SessionGeneration,
    workspace: Option<&WorkspaceId>,
    home: &Path,
    endpoint: &Path,
    helper_dir: Option<&Path>,
    user: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const PROTECTED: &[&str] = &[
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "SHLVL",
        "PANEFLOW_SESSION_ID",
        "PANEFLOW_SESSION_DIR",
        "PANEFLOW_WORKSPACE_UUID",
        "PANEFLOW_HOST_ENDPOINT",
        "PANEFLOW_RUNTIME_GENERATION",
        "PANEFLOW_HOME",
    ];
    let mut env = BTreeMap::new();
    for (key, value) in user {
        #[cfg(windows)]
        let key = key.to_uppercase();
        #[cfg(not(windows))]
        let key = key.clone();
        if !crate::env::is_valid_env_name(&key)
            || crate::env::is_forbidden_child_env_key(&key)
            || crate::env::is_inherited_host_terminal_env_key(&key)
            || PROTECTED.contains(&key.as_str())
        {
            continue;
        }
        env.insert(key, value.clone());
    }
    env.insert("TERM".to_string(), "xterm-256color".to_string());
    env.insert("COLORTERM".to_string(), "truecolor".to_string());
    env.insert("TERM_PROGRAM".to_string(), "ghostty".to_string());
    env.insert(
        "TERM_PROGRAM_VERSION".to_string(),
        paneflow_terminal_ghostty::GHOSTTY_APP_VERSION.to_string(),
    );
    env.insert("SHLVL".to_string(), "0".to_string());
    if std::env::var("LANG").map_or(true, |v| v.is_empty()) {
        env.insert("LANG".to_string(), "en_US.UTF-8".to_string());
    }
    env.insert("PANEFLOW_SESSION_ID".to_string(), session.to_string());
    env.insert(
        "PANEFLOW_SESSION_DIR".to_string(),
        paneflow_home::host_session_data_dir_in(home, session.as_str())
            .display()
            .to_string(),
    );
    if let Some(workspace) = workspace {
        env.insert("PANEFLOW_WORKSPACE_UUID".to_string(), workspace.to_string());
    }
    env.insert(
        "PANEFLOW_HOST_ENDPOINT".to_string(),
        endpoint.display().to_string(),
    );
    env.insert(
        "PANEFLOW_RUNTIME_GENERATION".to_string(),
        generation.to_string(),
    );
    env.insert("PANEFLOW_HOME".to_string(), home.display().to_string());
    if let Some(helper_dir) = helper_dir
        && !env.contains_key("PANEFLOW_BIN_DIR")
    {
        env.insert(
            "PANEFLOW_BIN_DIR".to_string(),
            helper_dir.display().to_string(),
        );
        let inherited = std::env::var("PATH").ok();
        let existing = env.get("PATH").map(String::as_str).or(inherited.as_deref());
        if let Some(path) = crate::helpers::prepend_to_path(existing, helper_dir) {
            env.insert("PATH".to_string(), path);
        }
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::time::{Duration, Instant};

    #[test]
    fn shutdown_deadline_is_shared_by_stalled_stops_and_keeps_inspection_responsive() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("shared-stop-deadline")).unwrap();
        let mut sessions = Vec::new();
        for _ in 0..3 {
            sessions.push(host.create(shell_request(80, 24)).unwrap().manifest.session);
        }
        let release = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        let blocked = Arc::clone(&release);
        let entered = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&entered);
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::StopCommit {
                count.fetch_add(1, Ordering::SeqCst);
                let (lock, wake) = &*blocked;
                let guard = lock.lock().unwrap();
                drop(wake.wait_while(guard, |released| !*released).unwrap());
            }
        }));
        let running = Arc::clone(&host);
        let started = Instant::now();
        let stopping = std::thread::spawn(move || running.request_shutdown(true));
        assert!(wait_until(Duration::from_secs(3), || entered
            .load(Ordering::SeqCst)
            == 3));
        let inspect_started = Instant::now();
        for session in &sessions {
            assert!(host.inspect(session).is_ok());
        }
        assert!(inspect_started.elapsed() < Duration::from_secs(1));
        let result = stopping.join().unwrap();
        let elapsed = started.elapsed();
        let (lock, wake) = &*release;
        *lock.lock().unwrap() = true;
        wake.notify_all();
        host.set_barrier(Arc::new(|_| {}));
        assert!(
            result.is_err(),
            "a pending commit cannot acknowledge shutdown"
        );
        assert!(
            elapsed < STOP_ACTION_BUDGET + Duration::from_secs(1),
            "{elapsed:?}"
        );
        assert!(wait_until(Duration::from_secs(3), || !host.is_shutting_down()));
        for session in sessions {
            host.stop(&session, None).unwrap();
        }
    }

    fn shell_request(cols: u16, rows: u16) -> CreateSession {
        #[cfg(windows)]
        let (shell, args) = (
            Some("cmd.exe".to_string()),
            vec!["/Q".to_string(), "/D".to_string()],
        );
        #[cfg(unix)]
        let (shell, args) = (Some("/bin/sh".to_string()), Vec::new());
        CreateSession {
            shell,
            args,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(cols),
            rows: Some(rows),
            ..CreateSession::default()
        }
    }

    fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    }

    #[test]
    fn checkpoint_staging_admits_two_captures_and_the_third_waits_for_a_release() {
        let staging = Arc::new(CheckpointStaging::default());
        let soon = || Instant::now() + Duration::from_millis(100);
        let first = staging.admit(1024, soon()).unwrap();
        let second = staging.admit(2048, soon()).unwrap();
        let refused = staging.admit(1, soon());
        assert!(matches!(refused, Err(HostError::Busy(_))));
        assert_eq!(staging.report().active, MAX_CONCURRENT_CHECKPOINTS);
        assert_eq!(staging.report().staged_bytes, 3072);
        assert_eq!(staging.report().refused, 1);
        drop(first);
        let third = staging.admit(4096, soon()).unwrap();
        assert_eq!(staging.report().staged_bytes, 6144);
        let oversized = staging.admit(CHECKPOINT_STAGING_BUDGET_BYTES + 1, soon());
        assert!(matches!(oversized, Err(HostError::Busy(_))));
        let over_budget = staging.admit(CHECKPOINT_STAGING_BUDGET_BYTES - 6144 + 1, soon());
        assert!(matches!(over_budget, Err(HostError::Busy(_))));
        drop(second);
        drop(third);
        let report = staging.report();
        assert_eq!((report.active, report.staged_bytes), (0, 0));
        assert_eq!(report.peak_staged_bytes, 6144);
    }

    #[test]
    fn a_staged_checkpoint_releases_its_admission_when_dropped() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("staging")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let staged = host.checkpoint(&session, None).unwrap();
        let held = host.resource_report().checkpoints;
        assert_eq!(held.active, 1);
        assert_eq!(held.staged_bytes, staged.snapshot.len());
        assert!(held.staged_bytes > 0);
        drop(staged);
        let released = host.resource_report().checkpoints;
        assert_eq!((released.active, released.staged_bytes), (0, 0));
        let resources = host.resource_report();
        let runtime = resources
            .sessions
            .iter()
            .find(|entry| entry.session == session)
            .unwrap()
            .runtime;
        assert_eq!(
            runtime.tail_budget_bytes,
            crate::protocol::MAX_OUTPUT_TAIL_BYTES
        );
        assert!(runtime.tail_allocated_bytes >= runtime.tail_retained_bytes);
        assert!(runtime.tail_allocated_bytes <= runtime.tail_budget_bytes);
        assert_eq!(
            runtime.input_budget_bytes,
            crate::runtime::MAX_INPUT_QUEUE_BYTES
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn oversized_terminal_dimensions_are_refused_before_reaching_the_engine() {
        assert!(validate_dimensions(80, 24).is_ok());
        assert!(validate_dimensions(4096, 1024).is_ok());
        assert!(matches!(
            validate_dimensions(0, 24),
            Err(HostError::InvalidRequest(_))
        ));
        assert!(matches!(
            validate_dimensions(u16::MAX, u16::MAX),
            Err(HostError::InvalidRequest(_))
        ));
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("dimensions")).unwrap();
        let mut request = shell_request(80, 24);
        request.cols = Some(u16::MAX);
        request.rows = Some(u16::MAX);
        assert!(matches!(
            host.create(request),
            Err(HostError::InvalidRequest(_))
        ));
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        assert!(matches!(
            host.resize(&session, None, u16::MAX, u16::MAX),
            Err(HostError::InvalidRequest(_))
        ));
        assert!(host.inspect(&session).unwrap().live);
        host.stop(&session, None).unwrap();
    }

    fn exited_manifest(home: &Path, workspace: &WorkspaceId, updated_at_ms: u64) -> SessionId {
        finished_manifest(
            home,
            workspace,
            SessionLifecycle::Exited {
                code: 0,
                signal: None,
            },
            updated_at_ms,
        )
    }

    fn finished_manifest(
        home: &Path,
        workspace: &WorkspaceId,
        lifecycle: SessionLifecycle,
        updated_at_ms: u64,
    ) -> SessionId {
        let session = SessionId::new();
        let cwd = home
            .join("worktrees")
            .join("paneflow-a1b2c3d4")
            .join("feat-a-reasonably-long-branch-name")
            .join("crates")
            .join("paneflow-host");
        let manifest = SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session: session.clone(),
            workspace: Some(workspace.clone()),
            generation: SessionGeneration::FIRST,
            host_instance: HostInstanceToken::new(),
            cwd: cwd.display().to_string(),
            launch: SessionLaunch {
                shell: "sh".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cols: 80,
                rows: 24,
            },
            lifecycle,
            process: None,
            title: Some("claude \u{00b7} feat/a-reasonably-long-branch-name".to_string()),
            current_cwd: Some(cwd.display().to_string()),
            last_hook: None,
            hook_revision: 0,
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            final_output: None,
            host_protocol_version: HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: updated_at_ms,
            updated_at_ms,
        };
        crate::manifest::write_manifest(home, &manifest).unwrap();
        session
    }

    fn summary_at(
        workspace: Option<&WorkspaceId>,
        live: bool,
        updated_at_ms: u64,
    ) -> SessionSummary {
        SessionSummary {
            manifest: SessionManifest {
                schema: MANIFEST_SCHEMA_VERSION,
                session: SessionId::new(),
                workspace: workspace.cloned(),
                generation: SessionGeneration::FIRST,
                host_instance: HostInstanceToken::new(),
                cwd: "/tmp".to_string(),
                launch: SessionLaunch {
                    shell: "sh".to_string(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                },
                lifecycle: if live {
                    SessionLifecycle::Running
                } else {
                    SessionLifecycle::Exited {
                        code: 0,
                        signal: None,
                    }
                },
                process: None,
                title: None,
                current_cwd: None,
                last_hook: None,
                hook_revision: 0,
                generation_started_at_ms: None,
                screen_changed_at_ms: None,
                screen_activity: None,
                menu_prompt_active: false,
                runtime: None,
                final_output: None,
                host_protocol_version: HOST_PROTOCOL_VERSION,
                host_build_id: crate::protocol::host_build_id(),
                created_at_ms: updated_at_ms,
                updated_at_ms,
            },
            live,
            owned: true,
            pending_launch: false,
            launch_operation: None,
            descendants_unresolved: 0,
            durability_error: None,
        }
    }

    #[test]
    fn a_listing_never_drops_a_running_session_however_many_are_open() {
        let workspace = WorkspaceId::new();
        let summaries: Vec<SessionSummary> = (0..40)
            .map(|index| summary_at(Some(&workspace), true, index))
            .collect();

        let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

        assert_eq!(
            windowed.len(),
            40,
            "every running session is listed whatever the window allows"
        );
    }

    #[test]
    fn the_inactive_window_counts_each_workspace_on_its_own() {
        let first = WorkspaceId::new();
        let second = WorkspaceId::new();
        let mut summaries = Vec::new();
        for index in 0..20u64 {
            summaries.push(summary_at(Some(&first), false, index));
            summaries.push(summary_at(Some(&second), false, index));
            summaries.push(summary_at(None, false, index));
        }

        let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

        for wanted in [Some(&first), Some(&second), None] {
            let kept = windowed
                .iter()
                .filter(|summary| summary.manifest.workspace.as_ref() == wanted)
                .count();
            assert_eq!(
                kept, INACTIVE_ROWS_PER_WORKSPACE,
                "each workspace keeps its own preview of finished sessions"
            );
        }
    }

    #[test]
    fn the_inactive_window_keeps_the_most_recent_of_a_workspace() {
        let workspace = WorkspaceId::new();
        let mut summaries: Vec<SessionSummary> = (0..20u64)
            .map(|index| summary_at(Some(&workspace), false, index))
            .collect();
        let newest = summaries[19].manifest.session.clone();
        let oldest = summaries[0].manifest.session.clone();
        summaries.rotate_left(7);

        let windowed = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE);

        assert!(
            windowed.iter().any(|s| s.manifest.session == newest),
            "the freshest finished session is previewed"
        );
        assert!(
            !windowed.iter().any(|s| s.manifest.session == oldest),
            "the stalest finished session falls outside the window"
        );
    }

    #[test]
    fn a_heavy_day_of_agents_still_fits_one_frame() {
        const OPEN_TERMINALS: usize = 50;
        const WORKSPACES: usize = 8;

        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let owner = host.instance().clone();

        let mut summaries = Vec::new();
        for _ in 0..OPEN_TERMINALS {
            summaries.push(summary_at(Some(&WorkspaceId::new()), true, now_ms()));
        }
        for _ in 0..WORKSPACES {
            let workspace = WorkspaceId::new();
            for index in 0..40u64 {
                summaries.push(summary_at(Some(&workspace), false, index));
            }
        }

        let rows: Vec<SessionRow> = inactive_window(summaries, INACTIVE_ROWS_PER_WORKSPACE)
            .into_iter()
            .map(|summary| SessionRow::of(summary, &owner))
            .collect();
        assert_eq!(
            rows.len(),
            OPEN_TERMINALS + WORKSPACES * INACTIVE_ROWS_PER_WORKSPACE
        );

        let frame = serde_json::to_vec(&json!({"sessions": rows})).unwrap();
        assert!(
            frame.len() * 2 < crate::protocol::MAX_CONTROL_FRAME_BYTES,
            "{OPEN_TERMINALS} agents plus a preview per workspace must fit a frame twice over, got {} bytes",
            frame.len()
        );
    }

    #[test]
    fn a_session_finished_yesterday_is_forgotten_and_a_fresh_one_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let workspace = WorkspaceId::new();
        let now = now_ms();
        let yesterday = exited_manifest(
            home.path(),
            &workspace,
            now - FINISHED_RECORD_MAX_AGE_MS - 60_000,
        );
        let recent = exited_manifest(home.path(), &workspace, now - 60_000);
        let stale_data = paneflow_home::host_session_data_dir_in(home.path(), yesterday.as_str());
        std::fs::create_dir_all(&stale_data).unwrap();
        std::fs::write(stale_data.join("last-hook-event.json"), b"{}").unwrap();

        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let listed = host.list(None);

        assert!(
            listed.iter().any(|s| s.manifest.session == recent),
            "a session that ended an hour ago is still there after a restart"
        );
        assert!(
            !listed.iter().any(|s| s.manifest.session == yesterday),
            "a session that ended more than a day ago is forgotten"
        );
        assert!(
            !crate::manifest::manifest_path(home.path(), &yesterday).exists(),
            "a forgotten record leaves no file behind"
        );
        assert!(
            !stale_data.exists(),
            "the retention sweep takes the forgotten session's hook seed with it"
        );
    }

    #[test]
    fn a_session_interrupted_before_a_week_away_is_still_there_on_return() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let workspace = WorkspaceId::new();
        let week = 7 * 24 * 60 * 60 * 1000;
        let now = now_ms();
        let interrupted =
            finished_manifest(home.path(), &workspace, SessionLifecycle::Lost, now - week);
        let finished = exited_manifest(home.path(), &workspace, now - week);

        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let listed = host.list(None);

        assert!(
            listed.iter().any(|s| s.manifest.session == interrupted),
            "a session left running before a week away is waiting on return"
        );
        assert!(
            !listed.iter().any(|s| s.manifest.session == finished),
            "a session that ended on its own that week is gone"
        );
    }

    #[test]
    fn a_session_the_machine_rebooted_under_is_kept_for_a_month() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let workspace = WorkspaceId::new();
        let now = now_ms();
        let kept = finished_manifest(
            home.path(),
            &workspace,
            SessionLifecycle::Lost,
            now - INTERRUPTED_RECORD_MAX_AGE_MS + 60_000,
        );
        let dropped = finished_manifest(
            home.path(),
            &workspace,
            SessionLifecycle::Lost,
            now - INTERRUPTED_RECORD_MAX_AGE_MS - 60_000,
        );

        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let listed = host.list(None);

        assert!(
            listed.iter().any(|s| s.manifest.session == kept),
            "an interrupted session is kept for a month"
        );
        assert!(
            !listed.iter().any(|s| s.manifest.session == dropped),
            "an interrupted session older than a month is finally forgotten"
        );
    }

    #[test]
    fn a_live_session_is_never_forgotten_however_old_it_is() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let live = host.create(shell_request(80, 24)).unwrap();
        let (manifest, durability) = {
            let sessions = host.lock_sessions();
            let record = &sessions[&live.manifest.session];
            (Arc::clone(&record.manifest), Arc::clone(&record.durability))
        };
        host.commit(&manifest, &durability, None, WriteClass::Metadata, |m| {
            m.updated_at_ms = 1
        });

        host.trim_terminated_records();

        assert!(
            host.list(None)
                .iter()
                .any(|s| s.manifest.session == live.manifest.session),
            "an agent that has been running for days is never forgotten"
        );

        let _ = host.stop(&live.manifest.session, None);
    }

    #[test]
    fn a_listing_leaves_the_launch_environment_out_so_many_sessions_still_fit_a_frame() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();

        let mut request = shell_request(80, 24);
        request.env = (0..40)
            .map(|i| (format!("LAB_VAR_{i:02}"), "x".repeat(256)))
            .collect();
        let created = host.create(request).unwrap();

        let listed = host.list(None);
        assert_eq!(listed.len(), 1);
        assert!(
            listed[0].manifest.launch.env.is_empty(),
            "a listing must not carry every session's environment"
        );

        let inspected = host.inspect(&created.manifest.session).unwrap();
        assert_eq!(
            inspected.manifest.launch.env.len(),
            40,
            "inspecting one session still answers with its environment"
        );

        let frame = serde_json::to_vec(&json!({"sessions": host.list(None)})).unwrap();
        assert!(
            frame.len() * 32 < crate::protocol::MAX_CONTROL_FRAME_BYTES,
            "one listed session must leave room for many more, got {} bytes",
            frame.len()
        );

        let _ = host.stop(&created.manifest.session, None);
    }

    #[test]
    fn the_viewport_scan_stamps_the_screen_and_flags_an_agent_drawn_menu() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("host.sock");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let created = host.create(shell_request(100, 24)).unwrap();
        let session = created.manifest.session.clone();
        assert!(created.manifest.screen_changed_at_ms.is_none());
        assert!(!created.manifest.menu_prompt_active);

        assert!(
            wait_until(Duration::from_secs(15), || {
                host.inspect(&session)
                    .is_ok_and(|summary| summary.manifest.screen_changed_at_ms.is_some())
            }),
            "the scan stamps the first painted screen"
        );
        let stamped = host.inspect(&session).unwrap().manifest;
        assert!(!stamped.menu_prompt_active);
        assert!(
            stamped.runtime.is_none(),
            "a plain shell is never mistaken for an agent runtime"
        );

        let manifest = Arc::clone(&host.lock_sessions()[&session].manifest);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let once = AtomicBool::new(false);
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::ManifestPersist
                && manifest.lock().unwrap().menu_prompt_active
                && !once.swap(true, Ordering::SeqCst)
            {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(15))
                    .unwrap();
            }
        }));
        host.input(
            &session,
            Some(SessionGeneration::FIRST),
            b"echo Enter to select - up/down to navigate - Esc to cancel
"
            .to_vec(),
        )
        .unwrap();
        assert!(
            wait_until(Duration::from_secs(15), || {
                host.inspect(&session)
                    .is_ok_and(|summary| summary.manifest.menu_prompt_active)
            }),
            "an agent-drawn menu footer reaches the manifest without any hook"
        );
        let asking = host.inspect(&session).unwrap().manifest;
        assert!(asking.screen_changed_at_ms >= stamped.screen_changed_at_ms);
        entered_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        let path = crate::manifest::manifest_path(home.path(), &session);
        let persisted_while_paused = read_manifest(&path).unwrap().menu_prompt_active;
        release_tx.send(()).unwrap();
        host.set_barrier(Arc::new(|_| {}));
        assert!(
            !persisted_while_paused,
            "inspection observes the viewport edge before its persistence completes"
        );
        assert!(wait_until(Duration::from_secs(15), || {
            read_manifest(&path).is_ok_and(|manifest| manifest.menu_prompt_active)
        }));
        assert!(
            read_manifest(&path).unwrap().menu_prompt_active,
            "the edge is persisted, not only held in memory"
        );

        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_bare_escape_in_a_bound_claude_pane_fences_the_turn_and_the_next_enter_resumes_it() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("host.sock");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let created = host.create(shell_request(100, 24)).unwrap();
        let session = created.manifest.session.clone();
        let directory = host.session_data_dir(&session);
        let subscription = host.subscribe_agents();

        host.input(&session, None, b"\x1b".to_vec()).unwrap();
        std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 2);
        assert_eq!(
            crate::hook_assets::read_cancellation(&directory),
            None,
            "an unbound pane never fences a turn"
        );

        host.bind_runtime(&session, None, Some("com.anthropic.claude-code"))
            .unwrap();
        assert_eq!(
            host.inspect(&session)
                .unwrap()
                .manifest
                .runtime
                .and_then(|runtime| runtime.launch_binding)
                .as_deref(),
            Some("com.anthropic.claude-code")
        );

        host.input(&session, None, b"\x1b".to_vec()).unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || {
                crate::hook_assets::read_cancellation(&directory).is_some()
            }),
            "a bare escape settles into a cancellation marker"
        );
        let fenced = crate::hook_assets::read_cancellation(&directory).unwrap();
        assert_eq!(fenced.runtime_generation, SessionGeneration::FIRST.get());
        assert_eq!(fenced.submitted_at, None);

        let announced = wait_until(Duration::from_secs(5), || {
            matches!(
                subscription.frames.try_recv(),
                Ok(frame) if frame["type"] == "cancellation"
                    && frame["session"] == session.to_string()
            )
        });
        assert!(announced, "the fence is announced on the agent bus");

        host.input(&session, None, b"retry\r".to_vec()).unwrap();
        assert!(
            wait_until(Duration::from_secs(5), || {
                crate::hook_assets::read_cancellation(&directory)
                    .is_some_and(|marker| marker.submitted_at.is_some())
            }),
            "the next Enter records the resumption in the same marker"
        );

        host.bind_runtime(&session, None, None).unwrap();
        host.input(&session, None, b"\x1b".to_vec()).unwrap();
        std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 2);
        let unbound = crate::hook_assets::read_cancellation(&directory).unwrap();
        assert_eq!(
            unbound.cancelled_at, fenced.cancelled_at,
            "unbinding the runtime retires the fence"
        );

        assert!(
            host.bind_runtime(&session, None, Some("com.example.nope"))
                .is_err(),
            "only a catalog runtime can be bound"
        );

        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_codex_pane_never_fences_an_escape_because_its_interrupt_hook_settles_the_turn() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("host.sock");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        let session = created.manifest.session.clone();
        host.bind_runtime(&session, None, Some("com.openai.codex"))
            .unwrap();

        host.input(&session, None, b"\x1b".to_vec()).unwrap();
        std::thread::sleep(crate::session_input::ESCAPE_SETTLE * 3);
        assert_eq!(
            crate::hook_assets::read_cancellation(&host.session_data_dir(&session)),
            None
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn the_host_assigns_durable_ids_persists_manifests_and_stops_owned_processes() {
        let home = tempfile::tempdir().unwrap();
        let endpoint = PathBuf::from("test-endpoint");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        let record = paneflow_home::host_instance_record_path_in(home.path());
        let identity: HostIdentity =
            serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap();
        assert_eq!(&identity.host_instance, host.instance());
        assert_eq!(identity.protocol, HOST_PROTOCOL_VERSION);
        assert!(host.list(None).is_empty());

        let workspace = WorkspaceId::new();
        let mut request = shell_request(80, 24);
        request.workspace = Some(workspace.clone());
        request.title = Some("shell".to_string());
        let created = host.create(request).unwrap();
        let session = created.manifest.session.clone();
        assert!(created.live);
        assert!(created.owned);
        assert_eq!(created.manifest.lifecycle, SessionLifecycle::Running);
        assert_eq!(created.manifest.generation, SessionGeneration::FIRST);
        assert_eq!(&created.manifest.host_instance, host.instance());
        let process = created.manifest.process.expect("process identity recorded");
        assert!(process.pid != 0);
        assert!(
            process.started_at.is_some(),
            "kernel start time is captured"
        );
        assert_ne!(
            session.to_string(),
            process.pid.to_string(),
            "the durable id is not the pid"
        );
        assert_eq!(
            created.manifest.launch.env.get("PANEFLOW_SESSION_ID"),
            None,
            "host-set variables are not stored as user launch metadata"
        );

        let manifest_path = crate::manifest::manifest_path(home.path(), &session);
        let on_disk = read_manifest(&manifest_path).unwrap();
        assert_eq!(
            SessionManifest {
                title: None,
                updated_at_ms: 0,
                ..on_disk
            },
            SessionManifest {
                title: None,
                updated_at_ms: 0,
                ..created.manifest.clone()
            },
            "the manifest on disk carries the same durable identity as the one in memory"
        );
        assert_eq!(host.list(Some(&workspace)).len(), 1);
        assert!(host.list(Some(&WorkspaceId::new())).is_empty());

        host.input(
            &session,
            Some(SessionGeneration::FIRST),
            b"echo HOST_ROUNDTRIP\r\n".to_vec(),
        )
        .unwrap();
        assert!(wait_until(Duration::from_secs(15), || {
            let slice = host.output(&session, None, 0, 1 << 20).unwrap();
            String::from_utf8_lossy(&slice.data).contains("HOST_ROUNDTRIP")
        }));
        let checkpoint = host
            .checkpoint(&session, Some(SessionGeneration::FIRST))
            .unwrap();
        assert!(checkpoint.offset > 0);
        assert!(matches!(
            host.checkpoint(&session, Some(SessionGeneration::FIRST.next())),
            Err(HostError::GenerationMismatch { .. })
        ));
        assert!(matches!(
            host.inspect(&SessionId::new()),
            Err(HostError::SessionNotFound(_))
        ));

        let stopped = host.stop(&session, Some(SessionGeneration::FIRST)).unwrap();
        assert!(!stopped.live);
        assert!(matches!(
            stopped.manifest.lifecycle,
            SessionLifecycle::Exited { .. }
        ));
        assert!(
            !process.is_provably_live(),
            "the owned process tree is gone"
        );
        assert!(
            wait_until(Duration::from_secs(5), || {
                read_manifest(&manifest_path).is_ok_and(|on_disk| {
                    matches!(on_disk.lifecycle, SessionLifecycle::Exited { .. })
                })
            }),
            "the final lifecycle revision reaches disk without blocking the stop"
        );
        let on_disk = read_manifest(&manifest_path).unwrap();
        assert_eq!(on_disk.session, session, "identity survives the exit");
        let again = host.stop(&session, None).unwrap();
        assert!(!again.live, "stopping an exited session is idempotent");
        assert!(matches!(
            host.input(&session, None, b"x".to_vec()),
            Err(HostError::Runtime(RuntimeError::NotLive)) | Err(HostError::SessionNotLive(_))
        ));
    }

    #[test]
    fn agent_ingress_rejects_old_generations_and_persists_the_accepted_seed() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("agent-ingress")).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        let session = created.manifest.session.clone();
        let accepted = crate::agent::AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.notification",
            "tool": "claude",
            "tool_name": "AskUserQuestion",
            "hook_payload": {
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "message": "Choose one"
            }
        }))
        .unwrap();
        let response = host.ingest_agent_event(&accepted).unwrap();
        assert_eq!(response.ack["accepted"], true);
        assert_eq!(response.ack["durable"], true);
        assert_eq!(response.ack["revision"], 1);
        assert!(response.frame.is_some(), "a fresh event is broadcast");
        let mut unfenced = accepted.clone();
        unfenced.generation = None;
        let rejected = host.ingest_agent_event(&unfenced).unwrap();
        assert_eq!(rejected.ack["accepted"], false);
        assert_eq!(rejected.ack["revision"], 1);
        assert!(rejected.frame.is_none());
        let seed_path = paneflow_home::host_session_data_dir_in(home.path(), session.as_str())
            .join("last-hook-event.json");
        let seed: Value = serde_json::from_slice(&std::fs::read(seed_path).unwrap()).unwrap();
        assert_eq!(
            seed,
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "runtime_generation": 1,
                "revision": 1
            })
        );

        host.stop(&session, None).unwrap();
        host.restart(&session, None).unwrap();
        let rejected = host.ingest_agent_event(&accepted).unwrap();
        assert_eq!(rejected.ack["accepted"], false);
        assert_eq!(rejected.ack["generation"], 2);
        assert!(
            rejected.frame.is_none(),
            "a rejected event is never broadcast"
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn forgetting_a_session_takes_its_hook_seed_directory_with_it() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("agent-seed-cleanup")).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        let session = created.manifest.session.clone();
        let event = crate::agent::AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.stop",
            "tool": "claude",
            "hook_payload": {"hook_event_name": "Stop"}
        }))
        .unwrap();
        host.ingest_agent_event(&event).unwrap();
        let session_dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        assert!(session_dir.join("last-hook-event.json").is_file());

        host.stop(&session, None).unwrap();
        host.remove(&session).unwrap();
        assert!(
            !session_dir.exists(),
            "a forgotten session leaves no hook seed behind at {}",
            session_dir.display()
        );
    }

    #[test]
    fn a_requested_session_id_is_created_once_and_never_silently_replaced() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("test-endpoint")).unwrap();
        let referenced = SessionId::new();
        let created = host
            .create(CreateSession {
                session: Some(referenced.clone()),
                ..shell_request(80, 24)
            })
            .unwrap();
        assert_eq!(created.manifest.session, referenced);
        assert!(created.live);
        assert!(matches!(
            host.create(CreateSession {
                session: Some(referenced.clone()),
                ..shell_request(80, 24)
            }),
            Err(HostError::SessionExists { .. })
        ));
        assert_eq!(host.list(None).len(), 1);
        host.stop(&referenced, None).unwrap();
    }

    #[test]
    fn a_new_host_instance_marks_inherited_running_records_lost_and_never_signals_them() {
        let home = tempfile::tempdir().unwrap();
        let session = {
            let host = SessionHost::open(home.path(), Path::new("first")).unwrap();
            let created = host.create(shell_request(80, 24)).unwrap();
            created.manifest.session
        };
        let manifest_path = crate::manifest::manifest_path(home.path(), &session);
        let before = read_manifest(&manifest_path).unwrap();
        assert_eq!(before.lifecycle, SessionLifecycle::Running);

        let host = SessionHost::open(home.path(), Path::new("second")).unwrap();
        let adopted = host.inspect(&session).unwrap();
        assert!(!adopted.live);
        assert!(!adopted.owned);
        assert_eq!(adopted.manifest.lifecycle, SessionLifecycle::Lost);
        assert_ne!(&adopted.manifest.host_instance, host.instance());
        let subscription = host.subscribe_agents();
        let event = AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": adopted.manifest.generation,
            "kind": "ai.prompt_submit",
            "tool": "claude",
        }))
        .unwrap();
        let rejected = host.ingest_agent_event(&event).unwrap();
        assert_eq!(rejected.ack["accepted"], false);
        assert_eq!(
            rejected.ack["reason"],
            "the event belongs to a previous host instance"
        );
        assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 0);
        assert!(read_manifest(&manifest_path).unwrap().last_hook.is_none());
        assert!(subscription.frames.try_recv().is_err());
        assert_eq!(
            read_manifest(&manifest_path).unwrap().lifecycle,
            SessionLifecycle::Lost
        );
        assert!(matches!(
            host.stop(&session, None),
            Err(HostError::ProcessUnverified(_))
        ));
        assert!(matches!(
            host.checkpoint(&session, None),
            Err(HostError::SessionNotLive(_))
        ));
        assert!(manifest_path.exists(), "the record stays for inspection");
    }

    #[test]
    fn an_explicit_restart_starts_a_new_generation_as_an_ordinary_shell() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("restart")).unwrap();
        #[cfg_attr(windows, allow(unused_mut))]
        let mut request = shell_request(80, 24);
        #[cfg(unix)]
        {
            request.args = vec!["-s".to_string()];
        }
        assert!(!request.args.is_empty());
        let created = host.create(request).unwrap();
        let session = created.manifest.session.clone();
        assert!(matches!(
            host.restart(&session, None),
            Err(HostError::SessionLive(_))
        ));
        let stopped = host.stop(&session, None).unwrap();
        assert!(!stopped.live);
        assert_eq!(
            stopped.reconnection(host.instance()),
            match stopped.manifest.lifecycle.clone() {
                SessionLifecycle::Exited { code, signal } =>
                    SessionReconnection::Exited { code, signal },
                other => panic!("unexpected lifecycle {other:?}"),
            }
        );

        assert!(matches!(
            host.restart(&session, Some(SessionGeneration::FIRST.next())),
            Err(HostError::GenerationMismatch { .. })
        ));
        let restarted = host
            .restart(&session, Some(SessionGeneration::FIRST))
            .unwrap();
        assert_eq!(restarted.manifest.session, session);
        assert_eq!(
            restarted.manifest.generation,
            SessionGeneration::FIRST.next()
        );
        assert!(restarted.live);
        assert!(
            restarted.manifest.launch.args.is_empty(),
            "a restart never replays the recorded command"
        );
        assert_ne!(restarted.manifest.process, stopped.manifest.process);
        assert_eq!(
            restarted.reconnection(host.instance()),
            SessionReconnection::Live
        );
        let on_disk =
            read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
        assert_eq!(on_disk.generation, SessionGeneration::FIRST.next());
        assert!(matches!(
            host.request_shutdown(false),
            Err(HostError::SessionsLive { count: 1 })
        ));
        host.stop(&session, None).unwrap();
        assert_eq!(
            host.request_shutdown(false),
            Ok(ShutdownReport {
                ended: Vec::new(),
                unresolved: Vec::new(),
            })
        );
    }

    #[test]
    fn a_record_owned_by_a_previous_host_reconnects_as_host_replaced() {
        let home = tempfile::tempdir().unwrap();
        let session = {
            let host = SessionHost::open(home.path(), Path::new("first")).unwrap();
            host.create(shell_request(80, 24)).unwrap().manifest.session
        };
        let host = SessionHost::open(home.path(), Path::new("second")).unwrap();
        let adopted = host.inspect(&session).unwrap();
        assert!(matches!(
            adopted.reconnection(host.instance()),
            SessionReconnection::HostReplaced { ref current_owner, ref previous_owner }
                if current_owner == host.instance() && previous_owner == &adopted.manifest.host_instance
        ));
        let restarted = host.restart(&session, None).unwrap();
        assert_eq!(
            restarted.manifest.generation,
            SessionGeneration::FIRST.next()
        );
        assert_eq!(&restarted.manifest.host_instance, host.instance());
        assert!(restarted.owned && restarted.live);
        host.stop(&session, None).unwrap();
        host.retire();
        assert!(!paneflow_home::host_instance_record_path_in(home.path()).exists());
    }

    #[test]
    fn two_hosts_cannot_own_one_home_at_the_same_time() {
        let home = tempfile::tempdir().unwrap();
        let first = SessionHost::open(home.path(), Path::new("first")).unwrap();
        assert!(matches!(
            SessionHost::open(home.path(), Path::new("second")),
            Err(HostError::OwnerBusy(_))
        ));
        drop(first);
        SessionHost::open(home.path(), Path::new("third")).unwrap();
    }

    #[test]
    fn unsupported_manifests_are_left_in_place() {
        let home = tempfile::tempdir().unwrap();
        let dir = paneflow_home::host_sessions_dir_in(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        let garbage = dir.join(format!("{}.json", SessionId::new()));
        std::fs::write(&garbage, b"{\"schema\": 99}").unwrap();
        let host = SessionHost::open(home.path(), Path::new("endpoint")).unwrap();
        assert!(host.list(None).is_empty());
        assert_eq!(std::fs::read(&garbage).unwrap(), b"{\"schema\": 99}");
    }

    #[test]
    fn launch_env_identifies_the_durable_session_and_drops_forbidden_keys() {
        let session = SessionId::new();
        let workspace = WorkspaceId::new();
        let user = BTreeMap::from([
            ("KEEP_ME".to_string(), "yes".to_string()),
            ("CLAUDECODE".to_string(), "1".to_string()),
            ("LD_PRELOAD".to_string(), "x".to_string()),
            ("TMUX".to_string(), "x".to_string()),
            ("PANEFLOW_SESSION_ID".to_string(), "forged".to_string()),
            ("BAD=NAME".to_string(), "x".to_string()),
        ]);
        let helper_dir = std::env::temp_dir().join("paneflow-helpers");
        let env = launch_env(
            &session,
            SessionGeneration::FIRST,
            Some(&workspace),
            Path::new("/home/x/.paneflow"),
            Path::new("/run/paneflow-host.sock"),
            Some(&helper_dir),
            &user,
        );
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("yes"));
        assert_eq!(env.get("PANEFLOW_SESSION_ID"), Some(&session.to_string()));
        assert_eq!(
            env.get("PANEFLOW_SESSION_DIR").map(PathBuf::from),
            Some(
                Path::new("/home/x/.paneflow")
                    .join("host")
                    .join("session-data")
                    .join(session.as_str())
            )
        );
        assert_eq!(
            env.get("PANEFLOW_RUNTIME_GENERATION").map(String::as_str),
            Some("1")
        );
        assert!(
            !env.contains_key("PANEFLOW_WORKSPACE_ID"),
            "the legacy integer marker is never forged from a UUID; the MCP bridge parses it as u64"
        );
        assert_eq!(
            env.get("PANEFLOW_WORKSPACE_UUID"),
            Some(&workspace.to_string()),
            "hooks address the durable workspace, not a GPUI surface"
        );
        assert_eq!(
            env.get("PANEFLOW_HOST_ENDPOINT").map(String::as_str),
            Some("/run/paneflow-host.sock")
        );
        assert_eq!(
            env.get("PANEFLOW_BIN_DIR").map(PathBuf::from),
            Some(helper_dir.clone())
        );
        assert_eq!(
            std::env::split_paths(env.get("PATH").expect("PATH"))
                .next()
                .as_deref(),
            Some(helper_dir.as_path()),
            "the host-local helper directory leads the child PATH"
        );
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        for key in ["CLAUDECODE", "LD_PRELOAD", "TMUX", "BAD=NAME"] {
            assert!(!env.contains_key(key), "{key} must not reach the child");
        }

        let legacy = BTreeMap::from([
            ("PANEFLOW_WORKSPACE_ID".to_string(), "7".to_string()),
            ("PANEFLOW_WORKSPACE_UUID".to_string(), "forged".to_string()),
            (
                "PANEFLOW_HOST_ENDPOINT".to_string(),
                "/tmp/forged.sock".to_string(),
            ),
        ]);
        let env = launch_env(
            &session,
            SessionGeneration::FIRST,
            Some(&workspace),
            Path::new("/home/x/.paneflow"),
            Path::new("/run/paneflow-host.sock"),
            None,
            &legacy,
        );
        assert_eq!(
            env.get("PANEFLOW_WORKSPACE_ID").map(String::as_str),
            Some("7"),
            "a caller-provided workspace marker keeps the existing hook routing"
        );
        assert_eq!(
            env.get("PANEFLOW_WORKSPACE_UUID"),
            Some(&workspace.to_string()),
            "a forged durable workspace never reaches the child"
        );
        assert_eq!(
            env.get("PANEFLOW_HOST_ENDPOINT").map(String::as_str),
            Some("/run/paneflow-host.sock")
        );
        assert!(
            !env.contains_key("PANEFLOW_BIN_DIR"),
            "a missing helper directory is reported, never invented"
        );
    }

    fn unverified_record(host: &SessionHost, reason: &str) -> SessionId {
        let session = SessionId::new();
        let manifest = SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session: session.clone(),
            workspace: None,
            generation: SessionGeneration::FIRST,
            host_instance: host.instance().clone(),
            cwd: std::env::temp_dir().display().to_string(),
            launch: SessionLaunch {
                shell: "sh".to_string(),
                args: Vec::new(),
                env: BTreeMap::new(),
                cols: 80,
                rows: 24,
            },
            lifecycle: SessionLifecycle::Unverified {
                reason: reason.to_string(),
            },
            process: None,
            title: None,
            current_cwd: None,
            last_hook: None,
            hook_revision: 0,
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            final_output: None,
            host_protocol_version: HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        };
        host.lock_sessions().insert(
            session.clone(),
            SessionRecord::fresh(Arc::new(Mutex::new(manifest))),
        );
        session
    }

    #[test]
    fn concurrent_restarts_of_one_generation_commit_at_most_one_new_generation() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("race")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        host.stop(&session, None).unwrap();
        host.set_barrier(Arc::new(|point| {
            if point == Barrier::RestartPersist {
                std::thread::sleep(Duration::from_millis(300));
            }
        }));
        let outcomes: Vec<Result<SessionSummary, HostError>> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    let host = &host;
                    let session = &session;
                    scope.spawn(move || host.restart(session, Some(SessionGeneration::FIRST)))
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect()
        });
        let committed = outcomes.iter().filter(|outcome| outcome.is_ok()).count();
        assert_eq!(committed, 1, "exactly one restart commits: {outcomes:?}");
        assert!(
            outcomes.iter().any(|outcome| matches!(
                outcome,
                Err(HostError::GenerationMismatch { .. }) | Err(HostError::LaunchPending(_))
            )),
            "the loser is refused with a typed error: {outcomes:?}"
        );
        let settled = host.inspect(&session).unwrap();
        assert_eq!(settled.manifest.generation, SessionGeneration::FIRST.next());
        assert!(settled.live);
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_stop_of_generation_one_never_writes_exited_into_generation_two() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("stale-stop")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let weak = Arc::downgrade(&host);
        let racing = session.clone();
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::StopCommit
                && counter.fetch_add(1, Ordering::SeqCst) == 0
                && let Some(host) = weak.upgrade()
            {
                host.restart(&racing, Some(SessionGeneration::FIRST))
                    .expect("the process of generation one is gone, so the restart is admitted");
            }
        }));
        let after_stop = host.stop(&session, Some(SessionGeneration::FIRST)).unwrap();
        assert_eq!(
            after_stop.manifest.generation,
            SessionGeneration::FIRST.next()
        );
        assert_eq!(
            after_stop.manifest.lifecycle,
            SessionLifecycle::Running,
            "the stale stop outcome is dropped at commit instead of overwriting the new generation"
        );
        assert!(after_stop.live);
        let on_disk =
            read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
        assert_eq!(on_disk.lifecycle, SessionLifecycle::Running);
        host.set_barrier(Arc::new(|_| {}));
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_restart_whose_persist_fails_restores_the_prior_record_without_a_stranded_start() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("persist-failure")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let prior = host.stop(&session, None).unwrap();
        host.persistence.drain(Duration::from_secs(5)).unwrap();
        let sessions_dir = paneflow_home::host_sessions_dir_in(home.path());
        let blocked = sessions_dir.clone();
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::RestartPersist && blocked.is_dir() {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let replaced = std::fs::remove_dir_all(&blocked)
                        .and_then(|()| std::fs::write(&blocked, b"not a directory"));
                    match replaced {
                        Ok(()) => break,
                        Err(_) if Instant::now() < deadline => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(error) => panic!("cannot replace the sessions directory: {error}"),
                    }
                }
            }
        }));
        let failed = host.restart(&session, Some(SessionGeneration::FIRST));
        assert!(
            matches!(failed, Err(HostError::Storage(_))),
            "a persist failure is a typed error: {failed:?}"
        );
        let restored = host.inspect(&session).unwrap();
        assert_eq!(restored.manifest.generation, SessionGeneration::FIRST);
        assert_eq!(restored.manifest.lifecycle, prior.manifest.lifecycle);
        assert!(!restored.pending_launch, "no launch stays registered");
        assert!(!restored.live);
        host.set_barrier(Arc::new(|_| {}));
        std::fs::remove_file(&sessions_dir).unwrap();
        std::fs::create_dir_all(&sessions_dir).unwrap();
        let restarted = host
            .restart(&session, Some(SessionGeneration::FIRST))
            .unwrap();
        assert_eq!(
            restarted.manifest.generation,
            SessionGeneration::FIRST.next()
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_create_whose_persist_times_out_leaves_no_manifest_behind() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("persist-timeout")).unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        host.persistence.spawn_exclusive(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(30));
            Ok(())
        });
        let mut request = shell_request(80, 24);
        let session = SessionId::new();
        request.session = Some(session.clone());
        let failed = host.create(request);
        assert!(
            matches!(failed, Err(HostError::Storage(_))),
            "a stalled writer is a typed storage error: {failed:?}"
        );
        assert!(matches!(
            host.inspect(&session),
            Err(HostError::SessionNotFound(_))
        ));
        release_tx.send(()).unwrap();
        host.persistence.drain(Duration::from_secs(5)).unwrap();
        assert!(
            !crate::manifest::manifest_path(home.path(), &session).exists(),
            "the revision queued before the failure is discarded, not written later"
        );
        assert!(!host.session_data_dir(&session).exists());
    }

    #[test]
    fn a_restart_whose_persist_times_out_keeps_the_prior_record_on_disk() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("restart-timeout")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let prior = host.stop(&session, None).unwrap();
        host.persistence.drain(Duration::from_secs(5)).unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        host.persistence.spawn_exclusive(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(30));
            Ok(())
        });
        let failed = host.restart(&session, Some(SessionGeneration::FIRST));
        assert!(matches!(failed, Err(HostError::Storage(_))), "{failed:?}");
        release_tx.send(()).unwrap();
        host.persistence.drain(Duration::from_secs(5)).unwrap();
        let on_disk =
            read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
        assert_eq!(on_disk.generation, SessionGeneration::FIRST);
        assert_eq!(on_disk.lifecycle, prior.manifest.lifecycle);
        let restored = host.inspect(&session).unwrap();
        assert_eq!(restored.manifest.generation, SessionGeneration::FIRST);
        assert!(!restored.pending_launch);
    }

    #[test]
    fn a_stop_during_the_launch_terminates_the_child_instead_of_publishing_it() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("late-child")).unwrap();
        let weak = Arc::downgrade(&host);
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        let id = SessionId::new();
        let racing = id.clone();
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::LaunchCommit
                && counter.fetch_add(1, Ordering::SeqCst) == 0
                && let Some(host) = weak.upgrade()
            {
                assert!(
                    matches!(host.stop(&racing, None), Err(HostError::LaunchPending(_))),
                    "a stop during the launch is refused as pending, never advertised as done"
                );
                assert!(matches!(
                    host.remove(&racing),
                    Err(HostError::LaunchPending(_))
                ));
                assert!(matches!(
                    host.restart(&racing, None),
                    Err(HostError::LaunchPending(_))
                ));
            }
        }));
        let created = host
            .create(CreateSession {
                session: Some(id.clone()),
                ..shell_request(80, 24)
            })
            .unwrap();
        assert!(
            !created.live,
            "the cancelled launch never publishes a live session"
        );
        assert!(!created.pending_launch);
        assert!(matches!(
            created.manifest.lifecycle,
            SessionLifecycle::Exited { .. }
        ));
        let process = created
            .manifest
            .process
            .expect("the late child identity is recorded");
        assert!(!process.is_provably_live(), "the late child was terminated");
        host.set_barrier(Arc::new(|_| {}));
    }

    #[test]
    fn launch_owner_thread_failure_is_reconciled_by_the_existing_scan() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("launch-owner-failure")).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        let session = created.manifest.session.clone();
        host.stop(&session, Some(created.manifest.generation))
            .unwrap();
        let mut spec = SpawnSpec {
            shell: created.manifest.launch.shell.clone(),
            args: Vec::new(),
            cwd: std::env::temp_dir(),
            env: BTreeMap::new(),
            cols: 80,
            rows: 24,
            scrollback_lines: 500,
        };
        spec.env
            .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), "100".into());
        let handle =
            SessionRuntime::launch(spec, created.manifest.generation, Arc::new(|_| {})).unwrap();
        {
            let mut sessions = host.lock_sessions();
            let record = sessions.get_mut(&session).unwrap();
            record.runtime = None;
            record.manifest.lock().unwrap().lifecycle = SessionLifecycle::Starting;
            record.launch = Some(PendingLaunch {
                operation: 99,
                generation: created.manifest.generation,
                cancel: Some(handle.canceller()),
                cancelled: false,
                fallback_owner: None,
            });
        }
        host.fail_launch_owner_spawn.store(true, Ordering::Release);
        host.own_late_launch(session.clone(), 99, handle);
        assert!(host.inspect(&session).unwrap().pending_launch);
        assert!(wait_until(Duration::from_secs(5), || {
            let summary = host.inspect(&session).unwrap();
            !summary.pending_launch && summary.live
        }));
        let summary = host.inspect(&session).unwrap();
        let identity = summary.manifest.process.unwrap();
        assert!(identity.is_provably_live());
        assert!(
            !host
                .stop(&session, Some(summary.manifest.generation))
                .unwrap()
                .owns_process()
        );
        assert!(!identity.is_provably_live());
    }

    #[test]
    fn admission_stops_at_eight_unresolved_launches() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("admission")).unwrap();
        {
            let mut sessions = host.lock_sessions();
            for index in 0..MAX_PENDING_LAUNCHES {
                let session = SessionId::new();
                let mut record = SessionRecord::fresh(Arc::new(Mutex::new(SessionManifest {
                    schema: MANIFEST_SCHEMA_VERSION,
                    session: session.clone(),
                    workspace: None,
                    generation: SessionGeneration::FIRST,
                    host_instance: host.instance().clone(),
                    cwd: std::env::temp_dir().display().to_string(),
                    launch: SessionLaunch {
                        shell: "sh".to_string(),
                        args: Vec::new(),
                        env: BTreeMap::new(),
                        cols: 80,
                        rows: 24,
                    },
                    lifecycle: SessionLifecycle::Starting,
                    process: None,
                    title: None,
                    current_cwd: None,
                    last_hook: None,
                    hook_revision: 0,
                    generation_started_at_ms: None,
                    screen_changed_at_ms: None,
                    screen_activity: None,
                    menu_prompt_active: false,
                    runtime: None,
                    final_output: None,
                    host_protocol_version: HOST_PROTOCOL_VERSION,
                    host_build_id: crate::protocol::host_build_id(),
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                })));
                record.launch = Some(PendingLaunch {
                    operation: index as u64 + 1,
                    generation: SessionGeneration::FIRST,
                    cancel: None,
                    cancelled: false,
                    fallback_owner: None,
                });
                sessions.insert(session, record);
            }
        }
        assert_eq!(host.pending_launches(), MAX_PENDING_LAUNCHES);
        assert!(matches!(
            host.create(shell_request(80, 24)),
            Err(HostError::Busy(_))
        ));
        let summaries = host.list(None);
        assert!(
            summaries.iter().all(|summary| summary.pending_launch
                && summary.reconnection(host.instance()) == SessionReconnection::Starting),
            "a pending launch is reported as starting, never as a verified process"
        );
        host.lock_sessions().clear();
    }

    #[test]
    fn a_panicked_runtime_keeps_ownership_until_a_stop_confirms_the_exit() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("panic")).unwrap();
        let created = host.create(shell_request(80, 24)).unwrap();
        let session = created.manifest.session.clone();
        let process = created.manifest.process.unwrap();
        let runtime = host.lock_sessions()[&session].runtime.clone().unwrap();
        runtime.inject_panic();
        assert!(
            wait_until(Duration::from_secs(5), || {
                matches!(
                    host.inspect(&session).unwrap().manifest.lifecycle,
                    SessionLifecycle::Unverified { .. }
                )
            }),
            "the panic surfaces as an unverified record"
        );
        let unverified = host.inspect(&session).unwrap();
        assert!(unverified.owns_process());
        assert!(matches!(
            unverified.reconnection(host.instance()),
            SessionReconnection::Unverified { .. }
        ));
        assert!(process.is_provably_live(), "no signal was fabricated");
        assert!(matches!(
            host.remove(&session),
            Err(HostError::OwnershipUnresolved { .. })
        ));
        assert!(matches!(
            host.restart(&session, None),
            Err(HostError::OwnershipUnresolved { .. })
        ));
        assert!(matches!(
            host.checkpoint(&session, None),
            Err(HostError::Runtime(RuntimeError::Unverified(_)))
        ));
        let stopped = host.stop(&session, None).unwrap();
        assert!(matches!(
            stopped.manifest.lifecycle,
            SessionLifecycle::Exited { .. }
        ));
        assert!(!process.is_provably_live());
        host.remove(&session).unwrap();
    }

    #[test]
    fn a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("shutdown")).unwrap();
        let session = unverified_record(&host, "the wait handle was lost");
        let refused = host.request_shutdown(true);
        assert!(
            matches!(
                &refused,
                Err(HostError::SessionsUnresolved { sessions })
                    if sessions.len() == 1 && sessions[0].session == session
            ),
            "the shutdown names the unresolved session: {refused:?}"
        );
        assert!(!host.is_shutting_down());
        assert!(host.create(shell_request(80, 24)).is_ok());
        for summary in host.live_sessions() {
            host.stop(&summary.manifest.session, None).unwrap();
        }
        host.lock_sessions().remove(&session);
        assert!(host.request_shutdown(false).is_ok());
        assert!(host.is_shutting_down());
        assert!(matches!(
            host.create(shell_request(80, 24)),
            Err(HostError::ShuttingDown)
        ));
    }

    #[test]
    fn a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("scan-restart")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let target = host
            .live_scan_targets()
            .into_iter()
            .find(|target| target.session == session)
            .unwrap();
        host.stop(&session, None).unwrap();
        let weak = Arc::downgrade(&host);
        let named = session.clone();
        let once = AtomicBool::new(false);
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::ManifestPersist && !once.swap(true, Ordering::SeqCst) {
                weak.upgrade()
                    .unwrap()
                    .restart(&named, Some(SessionGeneration::FIRST))
                    .unwrap();
            }
        }));
        assert!(host.commit_scan(&target, |manifest| manifest.title = Some("old scan".into())));
        host.set_barrier(Arc::new(|_| {}));
        let stored = read_manifest(&crate::manifest::manifest_path(home.path(), &session)).unwrap();
        assert_eq!(stored.generation, SessionGeneration::FIRST.next());
        assert_eq!(stored.lifecycle, SessionLifecycle::Running);
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_scan_waiting_to_persist_cannot_recreate_a_removed_record() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("scan-remove")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let target = host
            .live_scan_targets()
            .into_iter()
            .find(|target| target.session == session)
            .unwrap();
        host.stop(&session, None).unwrap();
        let weak = Arc::downgrade(&host);
        let named = session.clone();
        let once = AtomicBool::new(false);
        host.set_barrier(Arc::new(move |point| {
            if point == Barrier::ManifestPersist && !once.swap(true, Ordering::SeqCst) {
                weak.upgrade().unwrap().remove(&named).unwrap();
            }
        }));
        assert!(host.commit_scan(&target, |manifest| manifest.title = Some("old scan".into())));
        host.set_barrier(Arc::new(|_| {}));
        assert!(!crate::manifest::manifest_path(home.path(), &session).exists());
        assert!(!host.session_data_dir(&session).exists());
        assert!(!host.commit_scan(&target, |_| panic!("removed state cannot be updated")));
    }

    #[test]
    fn shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("shutdown-durability")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        host.stop(&session, None).unwrap();
        let path = crate::manifest::manifest_path(home.path(), &session);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let replaced = std::fs::remove_file(&path).and_then(|()| std::fs::create_dir(&path));
            match replaced {
                Ok(()) => break,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("cannot replace the manifest with a directory: {error}"),
            }
        }
        assert!(matches!(
            host.request_shutdown(false),
            Err(HostError::Storage(_))
        ));
        assert!(!host.is_shutting_down());
        let summary = host.inspect(&session).unwrap();
        assert!(!summary.owns_process());
        assert!(summary.durability_error.is_some());
        std::fs::remove_dir(&path).unwrap();
        assert!(host.request_shutdown(false).is_ok());
        assert!(host.is_shutting_down());
        assert!(host.inspect(&session).unwrap().durability_error.is_none());
        let stored = read_manifest(&path).unwrap();
        assert!(matches!(stored.lifecycle, SessionLifecycle::Exited { .. }));
    }

    #[test]
    fn a_duplicate_hook_retries_failed_seed_persistence_without_another_notification() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("seed-retry")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let path = host
            .session_data_dir(&session)
            .join(crate::manifest::LAST_HOOK_EVENT_FILE);
        std::fs::create_dir(&path).unwrap();
        let event = AgentEvent::from_params(&json!({
            "session": session, "runtime_generation": 1,
            "kind": "ai.stop", "tool": "claude", "emitted_at_ms": 42,
        }))
        .unwrap();
        let first = host.ingest_agent_event(&event).unwrap();
        assert_eq!(first.ack["durable"], false);
        let retry = host.ingest_agent_event(&event).unwrap();
        assert_eq!(retry.ack["durable"], false);
        assert!(retry.frame.is_none());
        std::fs::remove_dir(&path).unwrap();
        let recovered = host.ingest_agent_event(&event).unwrap();
        assert_eq!(recovered.ack["durable"], true);
        assert_eq!(recovered.ack["revision"], first.ack["revision"]);
        assert!(recovered.frame.is_none());
        let seed: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(seed["revision"], first.ack["revision"]);
        assert!(host.inspect(&session).unwrap().durability_error.is_none());
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_retried_agent_event_after_an_ambiguous_ack_is_acknowledged_but_not_notified_twice() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("receipts")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let event = crate::agent::AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.stop",
            "tool": "claude",
            "emitted_at_ms": 1_700_000_000_000u64,
            "hook_payload": {"hook_event_name": "Stop"}
        }))
        .unwrap();
        let first = host.ingest_agent_event(&event).unwrap();
        assert_eq!(first.ack["revision"], 1);
        assert!(first.frame.is_some());
        let retried = host.ingest_agent_event(&event).unwrap();
        assert_eq!(retried.ack["accepted"], true);
        assert_eq!(retried.ack["duplicate"], true);
        assert_eq!(retried.ack["revision"], 1);
        assert_eq!(retried.ack["durable"], true);
        assert!(retried.frame.is_none(), "a retry never notifies twice");
        assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 1);
        let next = crate::agent::AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.stop",
            "tool": "claude",
            "emitted_at_ms": 1_700_000_000_001u64,
            "hook_payload": {"hook_event_name": "Stop"}
        }))
        .unwrap();
        let advanced = host.ingest_agent_event(&next).unwrap();
        assert_eq!(advanced.ack["revision"], 2);
        assert!(advanced.frame.is_some());
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn parallel_hook_commits_publish_in_revision_order_before_returning_acknowledgements() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("parallel-hook-order")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let subscription = host.subscribe_agents();
        let start = Arc::new(std::sync::Barrier::new(3));
        let mut threads = Vec::new();
        for message in ["first", "second"] {
            let host = Arc::clone(&host);
            let session = session.clone();
            let start = Arc::clone(&start);
            threads.push(std::thread::spawn(move || {
                let event = AgentEvent::from_params(&json!({
                    "session": session, "runtime_generation": 1,
                    "kind": "ai.notification", "tool": "claude", "message": message,
                    "hook_payload": {"hook_event_name": "PermissionRequest"},
                }))
                .unwrap();
                start.wait();
                host.ingest_agent_event(&event).unwrap()
            }));
        }
        start.wait();
        let first = subscription
            .frames
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let second = subscription
            .frames
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(first["revision"], 1);
        assert_eq!(second["revision"], 2);
        for thread in threads {
            assert_eq!(thread.join().unwrap().ack["accepted"], true);
        }
        assert_eq!(
            host.agent_snapshot()[0]
                .last_hook
                .as_ref()
                .unwrap()
                .event
                .as_ref(),
            Some(&second)
        );
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn accepted_hook_frames_fit_the_reload_limit_and_oversized_events_never_commit() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("hook-size-limit")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let mut event = AgentEvent::from_params(&json!({
            "session": session, "runtime_generation": 1,
            "kind": "ai.prompt_submit", "tool": "claude",
            "hook_payload": {"hook_event_name": "UserPromptSubmit", "padding": "x".repeat(28 * 1024)},
        })).unwrap();
        let accepted = host.ingest_agent_event(&event).unwrap();
        assert_eq!(accepted.ack["durable"], true);
        let path = crate::manifest::manifest_path(home.path(), &session);
        let size = std::fs::metadata(&path).unwrap().len();
        assert!(size > 48 * 1024 && size <= crate::manifest::MAX_MANIFEST_BYTES);
        let stored = read_manifest(&path).unwrap();
        assert_eq!(stored.hook_revision, 1);
        event.payload["padding"] = json!("x".repeat(40 * 1024));
        assert!(matches!(
            host.ingest_agent_event(&event),
            Err(HostError::InvalidRequest(_))
        ));
        assert_eq!(host.inspect(&session).unwrap().manifest.hook_revision, 1);
        assert_eq!(read_manifest(&path).unwrap().hook_revision, 1);
        let mut oversized = stored;
        oversized.title = Some("x".repeat(crate::manifest::MAX_MANIFEST_BYTES as usize));
        assert_eq!(
            crate::manifest::write_manifest(home.path(), &oversized)
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
        assert_eq!(read_manifest(&path).unwrap().hook_revision, 1);
        host.stop(&session, None).unwrap();
    }

    #[test]
    fn a_delayed_seed_or_marker_write_never_recreates_a_removed_session_directory() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("seed-barrier")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        let directory = host.session_data_dir(&session);
        assert!(directory.is_dir());
        let event = crate::agent::AgentEvent::from_params(&json!({
            "session": session,
            "runtime_generation": 1,
            "kind": "ai.stop",
            "tool": "claude",
            "hook_payload": {"hook_event_name": "Stop"}
        }))
        .unwrap();
        host.stop(&session, None).unwrap();
        host.remove(&session).unwrap();
        assert!(!directory.exists());
        assert!(matches!(
            host.ingest_agent_event(&event),
            Err(HostError::SessionNotFound(_))
        ));
        assert!(
            host.commit_marker(&session, SessionGeneration::FIRST, |dir| {
                crate::hook_assets::record_cancellation(dir, 1, std::time::SystemTime::now())
            })
            .is_none(),
            "a marker for a removed session is dropped at the barrier"
        );
        let late_seed = crate::manifest::write_last_hook_event(
            home.path(),
            &session,
            "Stop",
            None,
            SessionGeneration::FIRST,
            1,
        );
        assert!(
            late_seed.is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "a late seed write is refused instead of recreating the directory"
        );
        assert!(
            crate::hook_assets::record_cancellation(&directory, 1, std::time::SystemTime::now())
                .unwrap()
                .is_none()
        );
        assert!(
            !directory.exists(),
            "nothing recreated {}",
            directory.display()
        );
    }

    #[test]
    fn a_marker_captured_under_generation_one_is_dropped_once_generation_two_runs() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("marker-generation")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        host.stop(&session, None).unwrap();
        host.restart(&session, None).unwrap();
        let stale = host.commit_marker(&session, SessionGeneration::FIRST, |_| ());
        assert!(
            stale.is_none(),
            "the captured generation is re-checked at commit"
        );
        let current = host.commit_marker(&session, SessionGeneration::FIRST.next(), |_| ());
        assert!(current.is_some());
        host.stop(&session, None).unwrap();
    }
}
