use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use serde::{Deserialize, Serialize};

use crate::bootstrap::{OwnerLock, OwnerLockError};
use crate::manifest::{
    AgentSummary, MANIFEST_SCHEMA_VERSION, ManifestError, SessionLaunch, SessionLifecycle,
    SessionManifest, now_ms, read_manifest, write_atomically, write_manifest,
};
use crate::protocol::{HOST_PROTOCOL_VERSION, HostIdentity, local_engine_identity};
use crate::runtime::{
    Checkpoint, OutputSlice, RuntimeError, RuntimeNotice, RuntimeObserver, SessionRuntime,
    SpawnSpec,
};

pub const DEFAULT_COLS: u16 = 80;
pub const DEFAULT_ROWS: u16 = 24;
pub const DEFAULT_SCROLLBACK_LINES: usize = 10_000;
const MAX_LAUNCH_ARGS: usize = 64;
const MAX_LAUNCH_ENV_ENTRIES: usize = 256;

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
}

impl SessionSummary {
    pub fn reconnection(&self, current_owner: &HostInstanceToken) -> SessionReconnection {
        if self.live && self.owned {
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
            SessionLifecycle::Lost | SessionLifecycle::Starting | SessionLifecycle::Running
                if &self.manifest.host_instance != current_owner =>
            {
                SessionReconnection::HostReplaced {
                    previous_owner: self.manifest.host_instance.clone(),
                    current_owner: current_owner.clone(),
                }
            }
            SessionLifecycle::Lost | SessionLifecycle::Starting | SessionLifecycle::Running => {
                SessionReconnection::Lost
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSnapshotEntry {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub live: bool,
    pub lifecycle: SessionLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSummary>,
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
    #[error("session {0} is still running; stop it before restarting it")]
    SessionLive(SessionId),
    #[error("{count} live session(s) remain; stop them before stopping the host")]
    SessionsLive { count: usize },
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

struct SessionRecord {
    manifest: Arc<Mutex<SessionManifest>>,
    runtime: Option<Arc<SessionRuntime>>,
}

impl SessionRecord {
    fn is_live(&self) -> bool {
        self.runtime.as_deref().is_some_and(SessionRuntime::is_live)
    }
}

pub struct SessionHost {
    home: PathBuf,
    identity: HostIdentity,
    manifest_writer: Arc<Mutex<()>>,
    sessions: Mutex<BTreeMap<SessionId, SessionRecord>>,
    _owner: OwnerLock,
}

impl SessionHost {
    pub fn open(home: &Path, endpoint: &Path) -> Result<Arc<Self>, HostError> {
        std::fs::create_dir_all(paneflow_home::host_sessions_dir_in(home))
            .map_err(|e| HostError::Storage(format!("cannot create the host directory: {e}")))?;
        let owner = OwnerLock::acquire(home).map_err(|error| match error {
            OwnerLockError::Held(_) => HostError::OwnerBusy(error.to_string()),
            OwnerLockError::Io(io) => HostError::Storage(io.to_string()),
        })?;
        let identity = HostIdentity {
            name: "paneflow-host".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            protocol: HOST_PROTOCOL_VERSION,
            host_instance: HostInstanceToken::new(),
            engine: local_engine_identity(),
            pid: std::process::id(),
            home: home.display().to_string(),
            endpoint: endpoint.display().to_string(),
            started_at_ms: now_ms(),
        };
        let host = Arc::new(Self {
            home: home.to_path_buf(),
            identity,
            manifest_writer: Arc::new(Mutex::new(())),
            sessions: Mutex::new(BTreeMap::new()),
            _owner: owner,
        });
        host.adopt_previous_records();
        host.write_instance_record()?;
        Ok(host)
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
            if manifest.lifecycle.is_running() {
                manifest.lifecycle = SessionLifecycle::Lost;
                manifest.updated_at_ms = now_ms();
                let _guard = self.lock_writer();
                if let Err(error) = write_manifest(&self.home, &manifest) {
                    log::warn!(
                        "paneflow-host: cannot record the lost session {}: {error}",
                        manifest.session
                    );
                }
            }
            sessions.insert(
                manifest.session.clone(),
                SessionRecord {
                    manifest: Arc::new(Mutex::new(manifest)),
                    runtime: None,
                },
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
        SessionSummary {
            manifest,
            live,
            owned,
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
        self.list(None)
            .into_iter()
            .map(|summary| AgentSnapshotEntry {
                session: summary.manifest.session,
                generation: summary.manifest.generation,
                live: summary.live,
                lifecycle: summary.manifest.lifecycle,
                agent: summary.manifest.agent,
            })
            .collect()
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
        let env = launch_env(
            &session,
            request.workspace.as_ref(),
            &self.home,
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
            agent: None,
            created_at_ms: created,
            updated_at_ms: created,
        }));
        {
            let mut sessions = self.lock_sessions();
            if sessions.contains_key(&session) {
                return Err(HostError::SessionExists { session });
            }
            sessions.insert(
                session.clone(),
                SessionRecord {
                    manifest: Arc::clone(&manifest),
                    runtime: None,
                },
            );
        }
        if let Err(error) = self.persist(&manifest) {
            self.lock_sessions().remove(&session);
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
        self.launch(session, manifest, spec, SessionGeneration::FIRST)
    }

    pub fn restart(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<SessionSummary, HostError> {
        let manifest = {
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
            if record.is_live() {
                return Err(HostError::SessionLive(session.clone()));
            }
            Arc::clone(&record.manifest)
        };
        let (next, spec) = {
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
            guard.agent = None;
            guard.launch.args.clear();
            guard.cwd = cwd.display().to_string();
            guard.updated_at_ms = now_ms();
            let env = launch_env(
                &guard.session,
                guard.workspace.as_ref(),
                &self.home,
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
            (next, spec)
        };
        self.persist(&manifest)?;
        self.launch(session.clone(), manifest, spec, next)
    }

    fn launch(
        &self,
        session: SessionId,
        manifest: Arc<Mutex<SessionManifest>>,
        spec: SpawnSpec,
        generation: SessionGeneration,
    ) -> Result<SessionSummary, HostError> {
        let observer = self.observer_for(Arc::clone(&manifest));
        let spawned = SessionRuntime::spawn(spec, generation, observer);
        let runtime = match spawned {
            Ok(runtime) => runtime,
            Err(error) => {
                self.update(&manifest, |m| {
                    m.lifecycle = SessionLifecycle::Failed {
                        reason: error.0.clone(),
                    };
                });
                return Err(HostError::SpawnFailed {
                    session,
                    reason: error.0,
                });
            }
        };
        let process = runtime.process();
        self.update(&manifest, |m| {
            m.lifecycle = if runtime.is_live() {
                SessionLifecycle::Running
            } else {
                match runtime.exit() {
                    Some(exit) => SessionLifecycle::Exited {
                        code: exit.code,
                        signal: exit.signal,
                    },
                    None => SessionLifecycle::Running,
                }
            };
            m.process = Some(process);
        });
        let runtime = Arc::new(runtime);
        let mut sessions = self.lock_sessions();
        let record = sessions.entry(session).or_insert_with(|| SessionRecord {
            manifest: Arc::clone(&manifest),
            runtime: None,
        });
        record.runtime = Some(runtime);
        Ok(self.summary_of(record))
    }

    pub fn ensure(
        &self,
        session: SessionId,
        workspace: Option<WorkspaceId>,
        cwd: Option<String>,
    ) -> Result<SessionSummary, HostError> {
        if let Ok(existing) = self.inspect(&session) {
            return Ok(existing);
        }
        self.create(CreateSession {
            session: Some(session),
            workspace,
            cwd,
            ..CreateSession::default()
        })
    }

    pub fn stop(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
    ) -> Result<SessionSummary, HostError> {
        let (manifest, runtime) = {
            let sessions = self.lock_sessions();
            let record = sessions
                .get(session)
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
            if !record.is_live() {
                if current.lifecycle.is_running() || current.lifecycle == SessionLifecycle::Lost {
                    return Err(HostError::ProcessUnverified(session.clone()));
                }
                return Ok(self.summary_of(record));
            }
            let runtime = record
                .runtime
                .clone()
                .ok_or_else(|| HostError::ProcessUnverified(session.clone()))?;
            (manifest, runtime)
        };
        let exit = runtime.stop()?;
        if let Some(exit) = exit {
            self.update(&manifest, |m| {
                m.lifecycle = SessionLifecycle::Exited {
                    code: exit.code,
                    signal: exit.signal.clone(),
                };
            });
        }
        self.inspect(session)
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
        self.with_live_runtime(session, generation, |runtime| runtime.input(bytes))
    }

    pub fn resize(
        &self,
        session: &SessionId,
        generation: Option<SessionGeneration>,
        cols: u16,
        rows: u16,
    ) -> Result<(), HostError> {
        self.with_live_runtime(session, generation, |runtime| runtime.resize(cols, rows))
    }

    pub fn live_sessions(&self) -> Vec<SessionSummary> {
        self.list(None).into_iter().filter(|s| s.live).collect()
    }

    pub fn live_session_count(&self) -> usize {
        self.live_sessions().len()
    }

    pub fn request_shutdown(&self) -> Result<(), HostError> {
        let live = self.live_sessions();
        if live.is_empty() {
            Ok(())
        } else {
            Err(HostError::SessionsLive { count: live.len() })
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

    fn update(
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

fn launch_env(
    session: &SessionId,
    workspace: Option<&WorkspaceId>,
    home: &Path,
    user: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const PROTECTED: &[&str] = &[
        "TERM",
        "COLORTERM",
        "TERM_PROGRAM",
        "TERM_PROGRAM_VERSION",
        "SHLVL",
        "PANEFLOW_SESSION_ID",
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
    if let Some(workspace) = workspace
        && !env.contains_key("PANEFLOW_WORKSPACE_ID")
    {
        env.insert("PANEFLOW_WORKSPACE_ID".to_string(), workspace.to_string());
    }
    env.insert("PANEFLOW_HOME".to_string(), home.display().to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

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
        assert_eq!(on_disk, created.manifest);
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
            host.request_shutdown(),
            Err(HostError::SessionsLive { count: 1 })
        ));
        host.stop(&session, None).unwrap();
        assert_eq!(host.request_shutdown(), Ok(()));
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
        let env = launch_env(
            &session,
            Some(&workspace),
            Path::new("/home/x/.paneflow"),
            &user,
        );
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("yes"));
        assert_eq!(env.get("PANEFLOW_SESSION_ID"), Some(&session.to_string()));
        assert_eq!(
            env.get("PANEFLOW_WORKSPACE_ID"),
            Some(&workspace.to_string())
        );
        assert_eq!(env.get("TERM").map(String::as_str), Some("xterm-256color"));
        for key in ["CLAUDECODE", "LD_PRELOAD", "TMUX", "BAD=NAME"] {
            assert!(!env.contains_key(key), "{key} must not reach the child");
        }

        let legacy = BTreeMap::from([("PANEFLOW_WORKSPACE_ID".to_string(), "7".to_string())]);
        let env = launch_env(
            &session,
            Some(&workspace),
            Path::new("/home/x/.paneflow"),
            &legacy,
        );
        assert_eq!(
            env.get("PANEFLOW_WORKSPACE_ID").map(String::as_str),
            Some("7"),
            "a caller-provided workspace marker keeps the existing hook routing"
        );
    }
}
