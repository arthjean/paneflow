use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::protocol::{ClientHello, ERR_CHECKPOINT_TOO_LARGE, ERR_SESSION_NOT_FOUND};
use paneflow_host::{
    BootstrapError, Checkpoint, CreateSession, HostClient, HostClientError, SessionReconnection,
    SessionSummary,
};

use super::pty_session::SpawnParams;

pub(crate) const CLIENT_NAME: &str = "paneflow-desktop";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostEndpoint {
    pub(crate) home: PathBuf,
    pub(crate) endpoint: PathBuf,
}

pub(crate) fn host_endpoint() -> Option<HostEndpoint> {
    let home = paneflow_home::paneflow_home()?;
    let endpoint = paneflow_host::endpoint::host_endpoint_path(&home);
    Some(HostEndpoint { home, endpoint })
}

pub(crate) fn hello() -> ClientHello {
    ClientHello::local(CLIENT_NAME)
}

#[derive(Debug)]
pub(crate) enum HostLinkError {
    NoHome,
    Bootstrap(BootstrapError),
    Client(HostClientError),
}

impl std::fmt::Display for HostLinkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoHome => {
                formatter.write_str("no Paneflow state home could be resolved; set PANEFLOW_HOME")
            }
            Self::Bootstrap(error) => write!(formatter, "{error}"),
            Self::Client(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for HostLinkError {}

impl From<BootstrapError> for HostLinkError {
    fn from(error: BootstrapError) -> Self {
        Self::Bootstrap(error)
    }
}

impl From<HostClientError> for HostLinkError {
    fn from(error: HostClientError) -> Self {
        Self::Client(error)
    }
}

impl HostLinkError {
    pub(crate) fn user_message(&self) -> String {
        match self {
            Self::NoHome => self.to_string(),
            Self::Bootstrap(error) => format!("The local host could not be started: {error}"),
            Self::Client(HostClientError::Incompatible(message)) => {
                format!(
                    "The running local host is incompatible with this Paneflow build: {message}"
                )
            }
            Self::Client(HostClientError::Unreachable { .. }) => {
                "The local host is not reachable.".to_string()
            }
            Self::Client(error) => format!("The local host refused the request: {error}"),
        }
    }
}

pub(crate) fn connect(endpoint: &Path) -> Result<HostClient, HostClientError> {
    HostClient::connect(endpoint, &hello())
}

pub(crate) fn connect_or_start(target: &HostEndpoint) -> Result<HostClient, HostLinkError> {
    match connect(&target.endpoint) {
        Ok(client) => Ok(client),
        Err(HostClientError::Unreachable { .. }) => {
            let controller = std::env::current_exe().map_err(|source| {
                HostLinkError::Client(HostClientError::Unreachable {
                    endpoint: target.endpoint.display().to_string(),
                    source,
                })
            })?;
            paneflow_host::ensure_host_running(&target.home, &controller, CLIENT_NAME)?;
            Ok(connect(&target.endpoint)?)
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionIntent {
    Create,
    Reattach,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostLinkEndKind {
    Exited { code: i32, signal: Option<String> },
    Failed,
    Lost,
    HostReplaced,
    Missing,
    Restarted { generation: SessionGeneration },
    Incompatible,
    AttachRefused,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostLinkEnd {
    pub(crate) kind: HostLinkEndKind,
    pub(crate) detail: String,
}

impl HostLinkEnd {
    pub(crate) fn exited(code: i32, signal: Option<String>) -> Self {
        let detail = match &signal {
            Some(signal) => format!("The session exited with code {code} ({signal})."),
            None => format!("The session exited with code {code}."),
        };
        Self {
            kind: HostLinkEndKind::Exited { code, signal },
            detail,
        }
    }

    pub(crate) fn missing() -> Self {
        Self {
            kind: HostLinkEndKind::Missing,
            detail: "The local host has no record of this session.".to_string(),
        }
    }

    pub(crate) fn incompatible(message: String) -> Self {
        Self {
            kind: HostLinkEndKind::Incompatible,
            detail: format!(
                "The running local host is incompatible with this Paneflow build: {message}"
            ),
        }
    }

    pub(crate) fn attach_refused(message: String) -> Self {
        Self {
            kind: HostLinkEndKind::AttachRefused,
            detail: format!("The local host refused the attachment: {message}"),
        }
    }

    pub(crate) fn host_replaced() -> Self {
        Self {
            kind: HostLinkEndKind::HostReplaced,
            detail: "A new local host owns this state home; the process of this session did not survive."
                .to_string(),
        }
    }

    pub(crate) fn from_reconnection(
        reconnection: SessionReconnection,
        generation: SessionGeneration,
    ) -> Self {
        match reconnection {
            SessionReconnection::Live | SessionReconnection::Starting => Self {
                kind: HostLinkEndKind::Restarted { generation },
                detail: format!(
                    "This session was restarted elsewhere (generation {}).",
                    generation.get()
                ),
            },
            SessionReconnection::Exited { code, signal } => {
                Self::exited(i32::try_from(code).unwrap_or(-1), signal)
            }
            SessionReconnection::Failed { reason } => Self {
                kind: HostLinkEndKind::Failed,
                detail: format!("The session failed to start: {reason}"),
            },
            SessionReconnection::HostReplaced { .. } => Self::host_replaced(),
            SessionReconnection::Lost => Self {
                kind: HostLinkEndKind::Lost,
                detail: "The local host lost this session; its process did not survive."
                    .to_string(),
            },
        }
    }

    pub(crate) fn exit(&self) -> Option<(i32, Option<String>)> {
        match &self.kind {
            HostLinkEndKind::Exited { code, signal } => Some((*code, signal.clone())),
            _ => None,
        }
    }

    pub(crate) fn restartable(&self) -> bool {
        !matches!(self.kind, HostLinkEndKind::Incompatible)
    }

    pub(crate) fn action_hint(&self) -> &'static str {
        match self.kind {
            HostLinkEndKind::Incompatible => "Stop the running host or update it before reopening.",
            HostLinkEndKind::Restarted { .. } => "Press Enter to attach to the running session.",
            _ => "Press Enter to start a new shell in this pane.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostLinkState {
    Attaching,
    Attached,
    Reconnecting,
    Ended(HostLinkEnd),
    Unavailable(String),
}

impl HostLinkState {
    pub(crate) fn accepts_input(&self) -> bool {
        matches!(self, Self::Attaching | Self::Attached)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HostAttachment {
    pub(crate) endpoint: PathBuf,
    pub(crate) hello: ClientHello,
    pub(crate) session: SessionId,
    pub(crate) generation: SessionGeneration,
    pub(crate) host_instance: HostInstanceToken,
    pub(crate) checkpoint: Checkpoint,
}

#[derive(Debug, Clone)]
pub(crate) struct HostedSession {
    pub(crate) endpoint: PathBuf,
    pub(crate) generation: SessionGeneration,
}

#[derive(Debug, Clone)]
pub(crate) struct HostedAttachment {
    pub(crate) attachment: HostAttachment,
    pub(crate) pid: u32,
    pub(crate) cwd: String,
}

pub(super) struct AttachRequest {
    pub(super) intent: SessionIntent,
    pub(super) session: SessionId,
    pub(super) workspace: Option<WorkspaceId>,
    pub(super) params: SpawnParams,
}

pub(crate) enum ResolveOutcome {
    Attached(Box<HostedAttachment>),
    Ended(HostLinkEnd),
}

fn create_request(request: &AttachRequest) -> CreateSession {
    let params = &request.params;
    CreateSession {
        session: Some(request.session.clone()),
        workspace: request.workspace.clone(),
        cwd: Some(params.cwd.display().to_string()),
        shell: Some(params.shell.clone()),
        args: params.extra_args.clone(),
        env: params
            .env
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        cols: u16::try_from(params.cols).ok().filter(|c| *c > 0),
        rows: u16::try_from(params.rows).ok().filter(|r| *r > 0),
        title: None,
    }
}

fn inspect_optional(
    client: &mut HostClient,
    session: &SessionId,
) -> Result<Option<SessionSummary>, HostClientError> {
    match client.inspect(session) {
        Ok(summary) => Ok(Some(summary)),
        Err(error) if error.code() == Some(ERR_SESSION_NOT_FOUND) => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn resolve(request: AttachRequest) -> Result<ResolveOutcome, HostLinkError> {
    let target = host_endpoint().ok_or(HostLinkError::NoHome)?;
    let mut client = connect_or_start(&target)?;
    let owner = client.identity().host_instance.clone();
    let existing = inspect_optional(&mut client, &request.session)?;
    let live = match (request.intent, existing) {
        (_, Some(summary)) if summary.live && summary.owned => summary,
        (SessionIntent::Reattach, None) => {
            return Ok(ResolveOutcome::Ended(HostLinkEnd::missing()));
        }
        (SessionIntent::Reattach, Some(summary)) => {
            let generation = summary.manifest.generation;
            return Ok(ResolveOutcome::Ended(HostLinkEnd::from_reconnection(
                summary.reconnection(&owner),
                generation,
            )));
        }
        (SessionIntent::Create, None) | (SessionIntent::Resume, None) => {
            client.create(&create_request(&request))?
        }
        (SessionIntent::Create, Some(_)) | (SessionIntent::Resume, Some(_)) => {
            client.restart(&request.session, None)?
        }
    };
    let generation = live.manifest.generation;
    let attachment = match client.attach(&request.session, Some(generation)) {
        Ok(attachment) => attachment,
        Err(error) if error.code() == Some(ERR_CHECKPOINT_TOO_LARGE) => {
            return Ok(ResolveOutcome::Ended(HostLinkEnd::attach_refused(
                error.to_string(),
            )));
        }
        Err(error) => return Err(error.into()),
    };
    Ok(ResolveOutcome::Attached(Box::new(HostedAttachment {
        attachment: HostAttachment {
            endpoint: target.endpoint,
            hello: hello(),
            session: request.session,
            generation,
            host_instance: attachment.host_instance,
            checkpoint: attachment.checkpoint,
        },
        pid: live.manifest.process.map(|p| p.pid).unwrap_or(0),
        cwd: live
            .manifest
            .current_cwd
            .clone()
            .unwrap_or(live.manifest.cwd),
    })))
}

pub(crate) fn stop_session(
    endpoint: &Path,
    session: &SessionId,
    generation: SessionGeneration,
) -> Result<SessionSummary, HostClientError> {
    connect(endpoint)?.stop(session, Some(generation))
}

pub(crate) enum LiveSessionProbe {
    Sessions(Vec<PathBuf>),
    NoHost,
    Unknown(String),
}

pub(crate) fn live_session_cwds() -> LiveSessionProbe {
    match list_sessions(None) {
        Ok(sessions) => LiveSessionProbe::Sessions(
            sessions
                .into_iter()
                .filter(|summary| summary.live)
                .map(|summary| {
                    PathBuf::from(summary.manifest.current_cwd.unwrap_or(summary.manifest.cwd))
                })
                .collect(),
        ),
        Err(HostLinkError::NoHome) => LiveSessionProbe::NoHost,
        Err(HostLinkError::Client(HostClientError::Unreachable { .. })) => LiveSessionProbe::NoHost,
        Err(error) => LiveSessionProbe::Unknown(error.to_string()),
    }
}

pub(crate) fn list_sessions(
    workspace: Option<&WorkspaceId>,
) -> Result<Vec<SessionSummary>, HostLinkError> {
    let target = host_endpoint().ok_or(HostLinkError::NoHome)?;
    Ok(connect(&target.endpoint)?.list(workspace)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_states_carry_their_exit_and_restartability() {
        let exited = HostLinkEnd::exited(3, None);
        assert_eq!(exited.exit(), Some((3, None)));
        assert!(exited.restartable());
        assert!(exited.detail.contains("code 3"));

        let incompatible = HostLinkEnd::incompatible("source_sha".into());
        assert!(!incompatible.restartable());
        assert_eq!(incompatible.exit(), None);

        let restarted = HostLinkEnd::from_reconnection(
            SessionReconnection::Live,
            SessionGeneration::FIRST.next(),
        );
        assert!(matches!(
            restarted.kind,
            HostLinkEndKind::Restarted { generation } if generation == SessionGeneration::FIRST.next()
        ));
        assert!(restarted.action_hint().contains("attach"));

        let lost =
            HostLinkEnd::from_reconnection(SessionReconnection::Lost, SessionGeneration::FIRST);
        assert_eq!(lost.kind, HostLinkEndKind::Lost);
        assert!(!HostLinkState::Ended(lost).accepts_input());
        assert!(!HostLinkState::Reconnecting.accepts_input());
        assert!(HostLinkState::Attaching.accepts_input());
    }

    #[test]
    fn create_requests_carry_the_desktop_launch_parameters() {
        let session = SessionId::new();
        let workspace = WorkspaceId::new();
        let mut env = std::collections::HashMap::new();
        env.insert("PANEFLOW_SURFACE_ID".to_string(), "42".to_string());
        let request = AttachRequest {
            intent: SessionIntent::Create,
            session: session.clone(),
            workspace: Some(workspace.clone()),
            params: SpawnParams {
                shell: "/bin/zsh".to_string(),
                shell_quoting: super::super::types::ShellQuoting::default_for_platform(),
                extra_args: vec!["-l".to_string()],
                env,
                cwd: PathBuf::from("/tmp"),
                cols: 132,
                rows: 43,
                profile: paneflow_config::schema::TerminalSurfaceProfile::Normal,
            },
        };
        let created = create_request(&request);
        assert_eq!(created.session, Some(session));
        assert_eq!(created.workspace, Some(workspace));
        assert_eq!(created.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(created.args, vec!["-l".to_string()]);
        assert_eq!(
            created.env.get("PANEFLOW_SURFACE_ID").map(String::as_str),
            Some("42")
        );
        assert_eq!((created.cols, created.rows), (Some(132), Some(43)));
        assert_eq!(created.cwd.as_deref(), Some("/tmp"));
    }
}
