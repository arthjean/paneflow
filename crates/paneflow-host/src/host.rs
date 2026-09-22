use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::agent::{AgentBus, AgentEvent, AgentSnapshotEntry, AgentSubscription};
use crate::bootstrap::{OwnerLock, OwnerLockError};
use crate::manifest::{
    HostedSessionRuntime, MANIFEST_SCHEMA_VERSION, ManifestError, SessionLaunch, SessionLifecycle,
    SessionManifest, now_ms, read_manifest, write_atomically, write_manifest,
};
use crate::protocol::{HOST_PROTOCOL_VERSION, HostIdentity, local_engine_identity};
use crate::runtime::{
    Checkpoint, LaunchCancel, LaunchWait, OutputSlice, RuntimeError, RuntimeNotice,
    RuntimeObserver, STARTUP_DEADLINE, SessionRuntime, SpawnSpec, StopReport,
};
use crate::session_input::SessionInput;

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
pub const MAX_PENDING_LAUNCHES: usize = 8;
const MAX_LAUNCH_ARGS: usize = 64;
const MAX_LAUNCH_ENV_ENTRIES: usize = 256;
const LATE_LAUNCH_WAIT: Duration = Duration::from_secs(3600);
pub const STOP_ACTION_BUDGET: Duration = Duration::from_secs(5);

pub type OperationId = u64;

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
}


struct PendingLaunch {
    operation: OperationId,
    generation: SessionGeneration,
    cancel: Option<LaunchCancel>,
    cancelled: bool,
    fallback_owner: Option<crate::runtime::LaunchHandle>,
}

struct SessionRecord {
    manifest: Arc<Mutex<SessionManifest>>,
    runtime: Option<Arc<SessionRuntime>>,
    launch: Option<PendingLaunch>,
    input: Arc<Mutex<SessionInput>>,
    escape_fence: bool,
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
            launch: None,
            input: Arc::new(Mutex::new(SessionInput::default())),
            escape_fence,
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

    fn set_escape_fence(&mut self, fenced: bool) {
        self.escape_fence = fenced;
        self.input
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
}

pub(crate) type FencedInputTarget = (
    SessionId,
    Arc<Mutex<SessionManifest>>,
    Arc<Mutex<SessionInput>>,
);

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

pub struct SessionHost {
    home: PathBuf,
    identity: HostIdentity,
    manifest_writer: Arc<Mutex<()>>,
    sessions: Mutex<BTreeMap<SessionId, SessionRecord>>,
    agent_bus: AgentBus,
    helper_dir: Option<PathBuf>,
    permissions: crate::control::ControlPermissions,
    submit_paste_delay: std::time::Duration,
    shutting_down: AtomicBool,
    #[cfg(test)]
    fail_launch_owner_spawn: AtomicBool,
    _owner: OwnerLock,
}

pub const INACTIVE_ROWS_PER_WORKSPACE: usize = 5;

const FINISHED_RECORD_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;

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
        let host = Arc::new(Self {
            home: home.to_path_buf(),
            identity,
            manifest_writer: Arc::new(Mutex::new(())),
            sessions: Mutex::new(BTreeMap::new()),
            agent_bus: AgentBus::new(),
            helper_dir,
            permissions,
            submit_paste_delay,
            shutting_down: AtomicBool::new(false),
            #[cfg(test)]
            fail_launch_owner_spawn: AtomicBool::new(false),
            _owner: owner,
        });
        host.adopt_previous_records();
        host.trim_terminated_records();
        host.write_instance_record()?;
        crate::viewport_scan::spawn(&host);
        crate::cancellation_scan::spawn(&host);
        Ok(host)
    }

    pub(crate) fn fenced_input_targets(&self) -> Vec<FencedInputTarget> {
        self.lock_sessions()
            .iter()
            .filter(|(_, record)| record.escape_fence && record.is_live())
            .map(|(session, record)| {
                (
                    session.clone(),
                    Arc::clone(&record.manifest),
                    Arc::clone(&record.input),
                )
            })
            .collect()
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

    pub(crate) fn live_scan_targets(
        &self,
    ) -> Vec<(SessionId, Arc<Mutex<SessionManifest>>, Arc<SessionRuntime>)> {
        self.reconcile_late_launches();
        self.lock_sessions()
            .iter()
            .filter(|(_, record)| record.is_live())
            .filter_map(|(session, record)| {
                Some((
                    session.clone(),
                    Arc::clone(&record.manifest),
                    record.runtime.clone()?,
                ))
            })
            .collect()
    }

    pub fn retire(&self) {
        let path = paneflow_home::host_instance_record_path_in(&self.home);
        let _guard = self.lock_writer();
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "paneflow-host: cannot remove the instance record {}: {error}",
                path.display()
            );
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
        let _guard = self.lock_writer();
        write_atomically(&path, &json)
            .map_err(|e| HostError::Storage(format!("cannot write the instance record: {e}")))
    }

    fn lock_writer(&self) -> std::sync::MutexGuard<'_, ()> {
        self.manifest_writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn adopt_previous_records(&self) {
        let paths = match crate::manifest::list_manifest_paths(&self.home) {
            Ok(paths) => paths,
            Err(error) => {
                log::warn!("paneflow-host: cannot list session manifests: {error}");
                return;
            }
        };
        let mut sessions = self.lock_sessions();
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
            if manifest.lifecycle.is_running() {
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
                let _guard = self.lock_writer();
                if let Err(error) = write_manifest(&self.home, &manifest) {
                    log::warn!(
                        "paneflow-host: cannot record the adopted session {}: {error}",
                        manifest.session
                    );
                }
            }
            sessions.insert(
                manifest.session.clone(),
                SessionRecord::fresh(Arc::new(Mutex::new(manifest))),
            );
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
        let durability_error = record
            .durability
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
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
        let now = now_ms();
        let dropped = {
            let mut sessions = self.lock_sessions();
            let dropped: Vec<SessionId> = sessions
                .iter()
                .filter(|(_, record)| !record.owns_process())
                .filter_map(|(id, record)| {
                    let manifest = record.manifest.lock().ok()?;
                    let max_age = record_max_age_ms(&manifest.lifecycle)?;
                    let stale = now.saturating_sub(manifest.updated_at_ms) > max_age;
                    stale.then(|| id.clone())
                })
                .collect();
            if dropped.is_empty() {
                return;
            }
            for id in &dropped {
                sessions.remove(id);
            }
            dropped
        };
        let _guard = self.lock_writer();
        for id in &dropped {
            let path = crate::manifest::manifest_path(&self.home, id);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    log::warn!("paneflow-host: cannot delete the manifest of {id}: {error}")
                }
            }
            crate::manifest::remove_session_data(&self.home, id);
        }
        log::info!(
            "paneflow-host: dropped {} session records past their retention",
            dropped.len()
        );
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
        let manifest = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(&event.session)
                .ok_or_else(|| HostError::SessionNotFound(event.session.clone()))?;
            Arc::clone(&record.manifest)
        };
        let generation = {
            let guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.generation
        };
        if let Some(requested) = event.generation
            && requested != generation
        {
            let reason = if requested < generation {
                "the event names a generation this session has left"
            } else {
                "the event names a generation this session has not reached"
            };
            log::warn!(
                "agent event rejected for session {}: runtime generation {} does not match current generation {}: {reason}",
                event.session,
                requested,
                generation
            );
            return Ok(json!({
                "accepted": false,
                "reason": reason,
                "session": event.session,
                "generation": generation,
            }));
        }
        let hook_event_name = event
            .payload
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| event.kind.wire_str())
            .to_string();
        crate::manifest::write_last_hook_event(
            &self.home,
            &event.session,
            &hook_event_name,
            event.tool_name.as_deref(),
            generation,
        )
        .map_err(|error| HostError::Storage(error.to_string()))?;
        let record = crate::manifest::HookRecord {
            hook_event_name,
            tool: event.tool.clone(),
            tool_name: event.tool_name.clone(),
            pid: event.pid,
            runtime_generation: generation,
            provider_session_id: payload_text(event, "session_id"),
            transcript_path: payload_text(event, "transcript_path"),
            emitted_at_ms: event.emitted_at_ms,
            received_at_ms: event.received_at_ms.unwrap_or_else(now_ms),
        };
        let stored = record.clone();
        self.update(&manifest, move |m| m.last_hook = Some(stored));
        self.agent_bus.broadcast(&event.to_frame(generation));
        Ok(json!({
            "accepted": true,
            "session": event.session,
            "generation": generation,
            "received_at_ms": event.received_at_ms,
            "last_hook": record,
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
            generation_started_at_ms: Some(created),
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            host_protocol_version: HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: created,
            updated_at_ms: created,
        }));
        let operation = self.next_operation();
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
                let snapshot = manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                self.persist_snapshot(&snapshot).map_err(|error| {
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
        let (manifest, next, spec) = {
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
            if record.is_live() || current.lifecycle == SessionLifecycle::Starting {
                return Err(HostError::SessionLive(session.clone()));
            }
            let manifest = Arc::clone(&record.manifest);
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let next = guard.generation.next();
            let resumed_cwd = guard
                .current_cwd
                .as_deref()
                .filter(|cwd| Path::new(cwd).is_dir())
                .unwrap_or(guard.cwd.as_str())
                .to_string();
            let cwd = resolve_cwd(Some(&resumed_cwd));
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
            (Arc::clone(&manifest), next, spec)
        };
        self.persist(&manifest)?;
        self.launch(session.clone(), manifest, spec, next)
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
                Some((guard.clone(), Arc::clone(&record.durability)))
            } else {
                None
            }
        };
        if let Some((snapshot, durability)) = snapshot {
            self.persist_with_durability(&snapshot, &durability);
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
            record.launch = None;
            record.set_escape_fence(fenced);
            cancelled
        };
        if cancelled {
            return self.stop(session, Some(runtime.generation()));
        }
        let summary = self.inspect(session)?;
        self.persist_snapshot(&summary.manifest).ok();
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
                let summary = self.summary_of(record);
                drop(sessions);
                self.persist_snapshot(&summary.manifest).ok();
                return self.inspect(session);
            };
            if !runtime.owns_process() && runtime.unverified().is_none() {
                let summary = self.summary_of(record);
                drop(sessions);
                self.persist_snapshot(&summary.manifest).ok();
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
        let snapshot = {
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if guard.generation == runtime.generation() {
                guard.lifecycle = lifecycle_after_stop(&report);
                guard.updated_at_ms = now_ms();
                Some(guard.clone())
            } else {
                log::debug!(
                    "paneflow-host: stop outcome of session {session} generation {} dropped; the session is at {}",
                    runtime.generation(),
                    guard.generation
                );
                None
            }
        };
        if let Some(snapshot) = snapshot {
            self.persist_with_durability(&snapshot, &durability);
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
        let removed = {
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
            sessions.remove(session);
            manifest
        };
        let _guard = self.lock_writer();
        let path = crate::manifest::manifest_path(&self.home, session);
        let outcome = match std::fs::remove_file(&path) {
            Ok(()) => Ok(removed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(removed),
            Err(error) => Err(HostError::Storage(format!(
                "cannot delete the manifest of {session}: {error}"
            ))),
        };
        crate::manifest::remove_session_data(&self.home, session);
        outcome
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
                let current = record
                    .manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .generation;
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
    ) -> Result<Checkpoint, HostError> {
        self.with_live_runtime(session, generation, SessionRuntime::checkpoint)
    }

    pub fn text(&self, session: &SessionId) -> Result<String, HostError> {
        self.with_live_runtime(session, None, SessionRuntime::text)
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
        self.with_live_runtime(session, generation, |runtime| runtime.output(from, max))
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
                .map(|record| Arc::clone(&record.input))
        };
        let observed = fenced.as_ref().map(|_| bytes.clone());
        let accepted =
            self.with_live_runtime(session, generation, |runtime| runtime.input(bytes))?;
        if let (Some(input), Some(observed)) = (fenced, observed) {
            input
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .observe(&observed, std::time::SystemTime::now());
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
        let manifest = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
                .ok_or_else(|| HostError::SessionNotFound(session.clone()))?;
            let current = record
                .manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .generation;
            if let Some(requested) = generation
                && requested != current
            {
                return Err(HostError::GenerationMismatch {
                    session: session.clone(),
                    current,
                    requested,
                });
            }
            Arc::clone(&record.manifest)
        };
        let binding = bound.map(|bound| bound.id.to_string());
        self.update(&manifest, |m| {
            m.runtime = with_launch_binding(m.runtime.take(), binding);
        });
        let fenced = bound.is_some_and(|bound| bound.lifecycle.escape_cancels_turn);
        if let Some(record) = self.lock_sessions().get_mut(session) {
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
        self.with_live_runtime(session, generation, |runtime| runtime.resize(cols, rows))?;
        let manifest = {
            let sessions = self.lock_sessions();
            sessions
                .get(session)
                .map(|record| Arc::clone(&record.manifest))
        };
        if let Some(manifest) = manifest {
            let changed = {
                let guard = manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                guard.launch.cols != cols || guard.launch.rows != rows
            };
            if changed {
                self.update(&manifest, |m| {
                    m.launch.cols = cols;
                    m.launch.rows = rows;
                });
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
            let failures: Vec<_> = self
                .list(None)
                .into_iter()
                .filter_map(|summary| self.persist_snapshot(&summary.manifest).err())
                .collect();
            if failures.is_empty() {
                Ok(ShutdownReport { ended, unresolved })
            } else {
                self.shutting_down.store(false, Ordering::Release);
                Err(HostError::Storage(failures.join("; ")))
            }
        } else {
            self.shutting_down.store(false, Ordering::Release);
            Err(HostError::SessionsUnresolved {
                sessions: remaining,
            })
        }
    }

    fn persist(&self, manifest: &Arc<Mutex<SessionManifest>>) -> Result<(), HostError> {
        let snapshot = manifest
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let _guard = self.lock_writer();
        write_manifest(&self.home, &snapshot)
            .map(|_| ())
            .map_err(|e| HostError::Storage(format!("cannot write the session manifest: {e}")))
    }

    pub(crate) fn update(
        &self,
        manifest: &Arc<Mutex<SessionManifest>>,
        apply: impl FnOnce(&mut SessionManifest),
    ) {
        let snapshot = {
            let mut guard = manifest
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            apply(&mut guard);
            guard.updated_at_ms = now_ms();
            guard.clone()
        };
        let _guard = self.lock_writer();
        if let Err(error) = write_manifest(&self.home, &snapshot) {
            log::warn!(
                "paneflow-host: cannot write the manifest of {}: {error}",
                snapshot.session
            );
        }
    }

    fn observer_for(&self, manifest: Arc<Mutex<SessionManifest>>) -> RuntimeObserver {
        let home = self.home.clone();
        let writer = Arc::clone(&self.manifest_writer);
        Arc::new(move |notice| {
            let snapshot = {
                let mut guard = manifest
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match notice {
                    RuntimeNotice::Title(title) => guard.title = Some(title),
                    RuntimeNotice::WorkingDirectory(cwd) => guard.current_cwd = Some(cwd),
                    RuntimeNotice::Exited(exit) => {
                        guard.lifecycle = SessionLifecycle::Exited {
                            code: exit.code,
                            signal: exit.signal,
                        };
                    }
                    RuntimeNotice::Lost => guard.lifecycle = SessionLifecycle::Lost,
                }
                guard.updated_at_ms = now_ms();
                guard.clone()
            };
            let _guard = writer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Err(error) = write_manifest(&home, &snapshot) {
                log::warn!(
                    "paneflow-host: cannot write the manifest of {}: {error}",
                    snapshot.session
                );
            }
        })
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
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
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
                generation_started_at_ms: None,
                screen_changed_at_ms: None,
                screen_activity: None,
                menu_prompt_active: false,
                runtime: None,
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
        host.update(
            &host.lock_sessions()[&live.manifest.session]
                .manifest
                .clone(),
            |m| m.updated_at_ms = 1,
        );

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
        assert!(
            read_manifest(&crate::manifest::manifest_path(home.path(), &session))
                .unwrap()
                .menu_prompt_active,
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
        let on_disk = read_manifest(&manifest_path).unwrap();
        assert!(matches!(on_disk.lifecycle, SessionLifecycle::Exited { .. }));
        assert_eq!(on_disk.session, session, "identity survives the exit");
        let again = host.stop(&session, None).unwrap();
        assert!(!again.live, "stopping an exited session is idempotent");
        assert!(matches!(
            host.input(&session, None, b"x".to_vec()),
            Err(HostError::Runtime(RuntimeError::NotLive))
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
        assert_eq!(response["accepted"], true);
        let seed_path = paneflow_home::host_session_data_dir_in(home.path(), session.as_str())
            .join("last-hook-event.json");
        let seed: Value = serde_json::from_slice(&std::fs::read(seed_path).unwrap()).unwrap();
        assert_eq!(
            seed,
            json!({
                "hook_event_name": "PermissionRequest",
                "tool_name": "AskUserQuestion",
                "runtime_generation": 1
            })
        );

        host.stop(&session, None).unwrap();
        host.restart(&session, None).unwrap();
        let rejected = host.ingest_agent_event(&accepted).unwrap();
        assert_eq!(rejected["accepted"], false);
        assert_eq!(rejected["generation"], 2);
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
    fn a_layout_reference_without_a_live_session_becomes_one_ordinary_shell() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("test-endpoint")).unwrap();
        let referenced = SessionId::new();
        let workspace = WorkspaceId::new();
        let cwd = std::env::temp_dir().display().to_string();

        let ensured = host
            .ensure(
                referenced.clone(),
                Some(workspace.clone()),
                Some(cwd.clone()),
            )
            .unwrap();
        assert_eq!(ensured.manifest.session, referenced);
        assert_eq!(ensured.manifest.workspace, Some(workspace));
        assert!(ensured.live);
        assert!(
            ensured.manifest.launch.args.is_empty(),
            "an adopted terminal is an ordinary shell, never a replayed agent command"
        );
        assert_eq!(ensured.manifest.launch.shell, default_shell());

        let again = host.ensure(referenced.clone(), None, None).unwrap();
        assert_eq!(
            again.manifest.process, ensured.manifest.process,
            "no second child"
        );
        assert_eq!(host.list(None).len(), 1);
        assert!(matches!(
            host.create(CreateSession {
                session: Some(referenced.clone()),
                ..shell_request(80, 24)
            }),
            Err(HostError::SessionExists { .. })
        ));
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
    fn shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds() {
        let home = tempfile::tempdir().unwrap();
        let host = SessionHost::open(home.path(), Path::new("shutdown-durability")).unwrap();
        let session = host.create(shell_request(80, 24)).unwrap().manifest.session;
        host.stop(&session, None).unwrap();
        let path = crate::manifest::manifest_path(home.path(), &session);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
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


}
