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

mod spawn_env;
mod staging;
mod types;

pub use spawn_env::*;
pub use staging::*;
pub use types::*;

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

type Durability = Arc<SessionPersistence>;

#[derive(Default)]
struct SeedLedger {
    removed: bool,
    receipts: VecDeque<(u64, u64)>,
    refusal_reported_for: Option<SessionGeneration>,
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

fn receipt_key(event: &AgentEvent) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    event.generation.hash(&mut hasher);
    event.message.hash(&mut hasher);
    event.summary.hash(&mut hasher);
    event.exit_code.hash(&mut hasher);
    event.kind.as_str().hash(&mut hasher);
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
                lifecycle: summary.manifest.lifecycle,
                process: summary.manifest.process,
                workspace: summary.manifest.workspace,
                title: summary.manifest.title,
                cwd: Some(summary.manifest.current_cwd.unwrap_or(summary.manifest.cwd)),
                last_hook: summary.manifest.last_hook,
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

    pub fn ingest_agent_event(&self, event: &AgentEvent) -> Result<Value, HostError> {
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
            .unwrap_or_else(|| event.kind.as_str())
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
                let level = if ledger.refusal_reported_for == Some(generation) {
                    log::Level::Debug
                } else {
                    ledger.refusal_reported_for = Some(generation);
                    log::Level::Warn
                };
                log::log!(
                    level,
                    "agent event rejected for session {}: runtime generation {:?} does not match current generation {}: {reason}",
                    event.session,
                    event.generation,
                    generation
                );
                return Ok(json!({
                        "accepted": false,
                        "reason": reason,
                        "revision": guard.hook_revision,
                }));
            }
            if guard.host_instance != self.identity.host_instance {
                return Ok(json!({
                        "accepted": false,
                        "reason": "the event belongs to a previous host instance",
                        "revision": guard.hook_revision,
                }));
            }
            if let Some((_, revision)) = ledger.receipts.iter().find(|(held, _)| *held == key) {
                let revision = *revision;
                drop(guard);
                let persistence_error = self
                    .persist(&manifest, &durability, WriteClass::Critical)
                    .err()
                    .map(|error| error.to_string());
                return Ok(json!({
                        "accepted": true,
                        "duplicate": true,
                        "durable": persistence_error.is_none(),
                        "persistence_error": persistence_error,
                        "revision": revision,
                }));
            }
            if !paneflow_ipc_client::agent::accepts_event(
                guard.last_hook.as_ref().and_then(|hook| hook.emitted_at_ms),
                event.emitted_at_ms,
            ) {
                return Ok(json!({
                        "accepted": false,
                        "reason": "an out-of-order event never replaces newer accepted state",
                        "revision": guard.hook_revision,
                }));
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
        let persistence_error = self
            .persist(&manifest, &durability, WriteClass::Critical)
            .err()
            .map(|error| error.to_string());
        if ledger.receipts.len() >= MAX_HOOK_RECEIPTS {
            ledger.receipts.pop_front();
        }
        ledger.receipts.push_back((key, revision));
        if let Some(frame) = record.event.as_ref() {
            self.publish_agent_frame(frame);
        }
        drop(ledger);
        Ok(json!({
                "accepted": true,
                "durable": persistence_error.is_none(),
                "persistence_error": persistence_error,
                "revision": revision,
        }))
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

    #[cfg(test)]
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
        let text_withdrawn = Arc::new(AtomicBool::new(false));
        if text_available {
            let home = self.home.clone();
            let host = self.weak_self();
            let text_session = session.clone();
            let text_manifest = Arc::clone(manifest);
            let text_durability = Arc::clone(durability);
            let withdrawn = Arc::clone(&text_withdrawn);
            let generation = record.generation;
            let text = record.text;
            self.persistence.spawn_exclusive(move || {
                if let Err(error) = crate::cold_text::write(&home, &text_session, &text) {
                    log::warn!(
                        "paneflow-host: final output of session {text_session} is not retained: {error}"
                    );
                    withdrawn.store(true, Ordering::Release);
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
                let mut final_output = final_output;
                final_output.text_available &= !text_withdrawn.load(Ordering::Acquire);
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

fn load_control_settings(home: &Path) -> (crate::control::ControlPermissions, std::time::Duration) {
    let config = paneflow_config::loader::load_config_from_path(&home.join("paneflow.json"));
    let permissions = crate::control::ControlPermissions::from_environment(
        config.ai_unrestricted_enabled(),
        config.ai_injection_fence_enabled(),
    );
    let delay = std::time::Duration::from_millis(config.resolved_submit_paste_delay_ms());
    (permissions, delay)
}

#[cfg(test)]
mod tests;
