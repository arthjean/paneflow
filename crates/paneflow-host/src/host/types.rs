use super::*;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionText {
    pub text: String,
    pub live: bool,
    pub available: bool,
    pub complete: bool,
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
    pub(super) fn of(summary: SessionSummary, owner: &HostInstanceToken) -> Self {
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
