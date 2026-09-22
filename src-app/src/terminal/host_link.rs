use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use paneflow_config::schema::{HostInstanceToken, SessionGeneration, SessionId, WorkspaceId};
use paneflow_host::protocol::{
    ClientHello, ERR_CHECKPOINT_TOO_LARGE, ERR_GENERATION_MISMATCH, ERR_SESSION_NOT_FOUND,
    ERR_SESSION_NOT_LIVE,
};
use paneflow_host::{
    BootstrapError, Checkpoint, CreateSession, HostClient, HostClientError, SessionReconnection,
    SessionRow, SessionSummary,
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
        Ok(client) => {
            if paneflow_home::home_fingerprint(Path::new(&client.identity().home))
                != paneflow_home::home_fingerprint(&target.home)
            {
                return Err(BootstrapError::EndpointFaulted(
                    target.endpoint.clone(),
                    "the endpoint belongs to another state home".into(),
                )
                .into());
            }
            Ok(client)
        }
        Err(error) if error.is_endpoint_absent() => {
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
    Restart { expected: Option<SessionGeneration> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostLinkEndKind {
    Exited { code: i32, signal: Option<String> },
    Failed,
    Lost,
    HostReplaced,
    Missing,
    Starting,
    Unverified,
    Restarted { generation: SessionGeneration },
    Incompatible,
    AttachRefused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostLinkAction {
    NewTerminal,
    Restart,
    Attach,
    Retry,
}

impl HostLinkAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NewTerminal => "New terminal",
            Self::Restart => "Restart",
            Self::Attach => "Attach",
            Self::Retry => "Retry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostLinkEnd {
    pub(crate) kind: HostLinkEndKind,
    pub(crate) detail: String,
    observed_generation: Option<SessionGeneration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FinalText {
    pub(crate) text: String,
    pub(crate) available: bool,
    pub(crate) complete: bool,
}

pub(crate) static RETAINED_CHECKPOINT_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn retained_checkpoint_bytes() -> usize {
    RETAINED_CHECKPOINT_BYTES.load(std::sync::atomic::Ordering::Acquire)
}

#[derive(Debug)]
pub(crate) struct CheckpointPayload {
    bytes: Vec<u8>,
}

impl CheckpointPayload {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        RETAINED_CHECKPOINT_BYTES.fetch_add(bytes.len(), std::sync::atomic::Ordering::AcqRel);
        Self { bytes }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for CheckpointPayload {
    fn drop(&mut self) {
        RETAINED_CHECKPOINT_BYTES.fetch_sub(self.bytes.len(), std::sync::atomic::Ordering::AcqRel);
    }
}

impl HostLinkEnd {
    pub(crate) fn note_final_output(&mut self, final_text: Option<&FinalText>) {
        if !matches!(self.kind, HostLinkEndKind::Exited { .. }) {
            return;
        }
        match final_text {
            Some(text) if !text.available => {
                self.detail = format!("{}. Final output is no longer available.", self.detail);
            }
            Some(text) if !text.complete => {
                self.detail = format!("{}. Some final output could not be collected.", self.detail);
            }
            _ => {}
        }
    }

    pub(crate) fn exited(code: i32, signal: Option<String>) -> Self {
        let detail = match &signal {
            Some(signal) => format!("Session ended ({signal})"),
            None => "Session ended".to_string(),
        };
        Self {
            kind: HostLinkEndKind::Exited { code, signal },
            detail,
            observed_generation: None,
        }
    }

    pub(crate) fn missing() -> Self {
        Self {
            kind: HostLinkEndKind::Missing,
            observed_generation: None,
            detail: "Session ended: the local host has no record of it".to_string(),
        }
    }

    pub(crate) fn incompatible(message: String) -> Self {
        Self {
            kind: HostLinkEndKind::Incompatible,
            observed_generation: None,
            detail: format!(
                "The running local host is incompatible with this build and keeps its sessions: {message}"
            ),
        }
    }

    pub(crate) fn starting() -> Self {
        Self {
            kind: HostLinkEndKind::Starting,
            observed_generation: None,
            detail: "The local host is still checking the process of this session".to_string(),
        }
    }

    pub(crate) fn unverified(reason: String) -> Self {
        Self {
            kind: HostLinkEndKind::Unverified,
            observed_generation: None,
            detail: format!(
                "The local host still owns this process but could not verify it: {reason}"
            ),
        }
    }

    pub(crate) fn attach_refused(message: String) -> Self {
        Self {
            kind: HostLinkEndKind::AttachRefused,
            observed_generation: None,
            detail: format!("The local host refused the attachment: {message}"),
        }
    }

    pub(crate) fn host_replaced() -> Self {
        Self {
            kind: HostLinkEndKind::HostReplaced,
            observed_generation: None,
            detail: "A new local host owns this state home; the process of this session did not survive."
                .to_string(),
        }
    }

    pub(crate) fn from_reconnection(
        reconnection: SessionReconnection,
        generation: SessionGeneration,
    ) -> Self {
        let mut end = match reconnection {
            SessionReconnection::Live => Self {
                kind: HostLinkEndKind::Restarted { generation },
                observed_generation: None,
                detail: "This session was restarted elsewhere".to_string(),
            },
            SessionReconnection::Starting => Self::starting(),
            SessionReconnection::Unverified { reason } => Self::unverified(reason),
            SessionReconnection::Exited { code, signal } => {
                Self::exited(i32::try_from(code).unwrap_or(-1), signal)
            }
            SessionReconnection::Failed { reason } => Self {
                kind: HostLinkEndKind::Failed,
                observed_generation: None,
                detail: format!("The session failed to start: {reason}"),
            },
            SessionReconnection::HostReplaced { .. } => Self::host_replaced(),
            SessionReconnection::Lost => Self {
                kind: HostLinkEndKind::Lost,
                observed_generation: None,
                detail: "Session lost: its process did not survive".to_string(),
            },
        };
        end.observed_generation = Some(generation);
        end
    }

    pub(crate) fn exit(&self) -> Option<(i32, Option<String>)> {
        match &self.kind {
            HostLinkEndKind::Exited { code, signal } => Some((*code, signal.clone())),
            _ => None,
        }
    }

    pub(crate) fn restartable(&self) -> bool {
        matches!(
            self.kind,
            HostLinkEndKind::Exited { .. }
                | HostLinkEndKind::Failed
                | HostLinkEndKind::Lost
                | HostLinkEndKind::HostReplaced
        )
    }

    pub(crate) fn action(&self) -> HostLinkAction {
        match self.kind {
            HostLinkEndKind::Missing => HostLinkAction::NewTerminal,
            HostLinkEndKind::Exited { .. }
            | HostLinkEndKind::Failed
            | HostLinkEndKind::Lost
            | HostLinkEndKind::HostReplaced => HostLinkAction::Restart,
            HostLinkEndKind::Restarted { .. } => HostLinkAction::Attach,
            HostLinkEndKind::Starting
            | HostLinkEndKind::Unverified
            | HostLinkEndKind::Incompatible
            | HostLinkEndKind::AttachRefused => HostLinkAction::Retry,
        }
    }

    pub(crate) fn action_label(&self) -> Option<&'static str> {
        Some(self.action().label())
    }

    pub(crate) fn secondary_action_label(&self) -> Option<&'static str> {
        matches!(self.kind, HostLinkEndKind::Incompatible).then_some("Stop host and restart")
    }

    pub(crate) fn intent(&self, observed: Option<SessionGeneration>) -> SessionIntent {
        match self.action() {
            HostLinkAction::NewTerminal => SessionIntent::Create,
            HostLinkAction::Restart => match self.observed_generation.or(observed) {
                Some(expected) => SessionIntent::Restart {
                    expected: Some(expected),
                },
                None => SessionIntent::Reattach,
            },
            HostLinkAction::Attach | HostLinkAction::Retry => SessionIntent::Reattach,
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
    pub(crate) offset: u64,
}

impl HostAttachment {
    pub(crate) fn from_checkpoint(
        endpoint: PathBuf,
        hello: ClientHello,
        session: SessionId,
        host_instance: HostInstanceToken,
        checkpoint: Checkpoint,
    ) -> (Self, CheckpointPayload) {
        let attachment = Self {
            endpoint,
            hello,
            session,
            generation: checkpoint.generation,
            host_instance,
            offset: checkpoint.offset,
        };
        (attachment, CheckpointPayload::new(checkpoint.snapshot))
    }
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
    Attached(Box<HostedAttachment>, CheckpointPayload),
    Ended(HostLinkEnd, Option<FinalText>),
}

fn final_text_of(client: &mut HostClient, session: &SessionId) -> Option<FinalText> {
    let reply = client.text(session).ok()?;
    Some(FinalText {
        text: reply.text,
        available: reply.available,
        complete: reply.complete,
    })
}

fn ended_with_final_text(
    client: &mut HostClient,
    session: &SessionId,
    mut end: HostLinkEnd,
) -> ResolveOutcome {
    let final_text = matches!(end.kind, HostLinkEndKind::Exited { .. })
        .then(|| final_text_of(client, session))
        .flatten();
    end.note_final_output(final_text.as_ref());
    ResolveOutcome::Ended(end, final_text)
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

fn end_of(client: &HostClient, summary: &SessionSummary) -> HostLinkEnd {
    let owner = client.identity().host_instance.clone();
    HostLinkEnd::from_reconnection(summary.reconnection(&owner), summary.manifest.generation)
}

pub(super) fn resolve(request: AttachRequest) -> Result<ResolveOutcome, HostLinkError> {
    let target = host_endpoint().ok_or(HostLinkError::NoHome)?;
    resolve_at(request, target)
}

fn resolve_at(
    request: AttachRequest,
    target: HostEndpoint,
) -> Result<ResolveOutcome, HostLinkError> {
    let mut client = match connect_or_start(&target) {
        Ok(client) => client,
        Err(HostLinkError::Client(HostClientError::Incompatible(message)))
        | Err(HostLinkError::Bootstrap(BootstrapError::Incompatible(_, message))) => {
            return Ok(ResolveOutcome::Ended(
                HostLinkEnd::incompatible(message),
                None,
            ));
        }
        Err(error) => return Err(error),
    };
    let existing = inspect_optional(&mut client, &request.session)?;
    let live = match (request.intent, existing) {
        (_, Some(summary)) if summary.live && summary.owned => summary,
        (_, Some(summary)) if summary.owned && summary.pending_launch => {
            return Ok(ResolveOutcome::Ended(HostLinkEnd::starting(), None));
        }
        (SessionIntent::Create, None) => client.create(&create_request(&request))?,
        (SessionIntent::Create, Some(summary)) => {
            let end = end_of(&client, &summary);
            return Ok(ended_with_final_text(&mut client, &request.session, end));
        }
        (SessionIntent::Reattach, None) => {
            return Ok(ResolveOutcome::Ended(HostLinkEnd::missing(), None));
        }
        (SessionIntent::Reattach, Some(summary)) => {
            let end = end_of(&client, &summary);
            return Ok(ended_with_final_text(&mut client, &request.session, end));
        }
        (SessionIntent::Restart { .. }, None) => {
            return Ok(ResolveOutcome::Ended(HostLinkEnd::missing(), None));
        }
        (SessionIntent::Restart { expected }, Some(summary)) => {
            let end = end_of(&client, &summary);
            if !end.restartable() || expected.is_none() {
                return Ok(ended_with_final_text(&mut client, &request.session, end));
            }
            match client.restart(&request.session, expected) {
                Ok(restarted) => restarted,
                Err(error) if error.code() == Some(ERR_GENERATION_MISMATCH) => {
                    match inspect_optional(&mut client, &request.session)? {
                        Some(current) if current.live && current.owned => current,
                        Some(current) => {
                            let end = end_of(&client, &current);
                            return Ok(ended_with_final_text(&mut client, &request.session, end));
                        }
                        None => return Ok(ResolveOutcome::Ended(HostLinkEnd::missing(), None)),
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    };
    let generation = live.manifest.generation;
    let attachment = match client.attach(&request.session, Some(generation)) {
        Ok(attachment) => attachment,
        Err(error) if error.code() == Some(ERR_CHECKPOINT_TOO_LARGE) => {
            return Ok(ResolveOutcome::Ended(
                HostLinkEnd::attach_refused(error.to_string()),
                None,
            ));
        }
        Err(error) => return Err(error.into()),
    };
    let (attachment, snapshot) = HostAttachment::from_checkpoint(
        target.endpoint,
        hello(),
        request.session,
        attachment.host_instance,
        attachment.checkpoint,
    );
    Ok(ResolveOutcome::Attached(
        Box::new(HostedAttachment {
            attachment,
            pid: live.manifest.process.map(|p| p.pid).unwrap_or(0),
            cwd: live
                .manifest
                .current_cwd
                .clone()
                .unwrap_or(live.manifest.cwd),
        }),
        snapshot,
    ))
}

pub(crate) fn stop_session(
    endpoint: &Path,
    session: &SessionId,
    generation: SessionGeneration,
) -> Result<SessionSummary, HostClientError> {
    HostClient::connect(endpoint, &ClientHello::control(CLIENT_NAME))?
        .stop(session, Some(generation))
}

pub(crate) fn shutdown_host(endpoint: &Path) -> Result<(), HostClientError> {
    HostClient::connect(endpoint, &ClientHello::control(CLIENT_NAME))?
        .call("host.shutdown", serde_json::json!({}))?;
    Ok(())
}

pub(crate) fn stop_incompatible_host() -> Result<(), String> {
    let target = host_endpoint().ok_or_else(|| HostLinkError::NoHome.to_string())?;
    let controller = std::env::current_exe().map_err(|error| error.to_string())?;
    paneflow_host::bootstrap::stop_for_replacement(&controller, &target.home, &target.endpoint)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ShutdownOutcome {
    Acknowledged,
    NoHost,
    Unsaved(String),
    Refused(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct StopAllOutcome {
    pub(crate) stopped: usize,
    pub(crate) unresolved: Vec<(SessionId, String)>,
    pub(crate) unsaved: Vec<(SessionId, String)>,
    pub(crate) shutdown: Option<ShutdownOutcome>,
}

impl StopAllOutcome {
    pub(crate) fn confirmed(&self) -> bool {
        self.unresolved.is_empty()
            && self.unsaved.is_empty()
            && !matches!(
                self.shutdown,
                Some(ShutdownOutcome::Refused(_) | ShutdownOutcome::Unsaved(_))
            )
    }

    pub(crate) fn durability_only(&self) -> bool {
        self.unresolved.is_empty()
            && (!self.unsaved.is_empty()
                || matches!(self.shutdown, Some(ShutdownOutcome::Unsaved(_))))
            && !matches!(self.shutdown, Some(ShutdownOutcome::Refused(_)))
    }

    pub(crate) fn user_message(&self) -> String {
        let mut parts = Vec::new();
        if !self.unresolved.is_empty() {
            parts.push(format!(
                "{} could not be confirmed stopped: {}",
                count_sessions(self.unresolved.len()),
                self.unresolved
                    .iter()
                    .map(|(session, reason)| format!("{session}: {reason}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if !self.unsaved.is_empty() {
            parts.push(format!(
                "{} stopped but its final state could not be saved: {}",
                count_sessions(self.unsaved.len()),
                self.unsaved
                    .iter()
                    .map(|(session, reason)| format!("{session}: {reason}"))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if let Some(ShutdownOutcome::Refused(reason)) = &self.shutdown {
            parts.push(format!("the local host refused to shut down: {reason}"));
        }
        if let Some(ShutdownOutcome::Unsaved(reason)) = &self.shutdown {
            parts.push(format!(
                "the local host still owns unsaved final state: {reason}"
            ));
        }
        parts.join(". ")
    }
}

fn count_sessions(n: usize) -> String {
    if n == 1 {
        "1 session".to_string()
    } else {
        format!("{n} sessions")
    }
}

pub(crate) fn stop_sessions_and_shutdown(
    targets: Vec<(PathBuf, SessionId, SessionGeneration)>,
    host: Option<PathBuf>,
) -> StopAllOutcome {
    use std::time::Instant;
    let deadline = Instant::now() + paneflow_host::host::STOP_ACTION_BUDGET;
    let mut last = StopAllOutcome {
        unresolved: targets
            .iter()
            .map(|(_, session, _)| (session.clone(), "stop acknowledgement pending".into()))
            .collect(),
        shutdown: host
            .as_ref()
            .map(|_| ShutdownOutcome::Refused("shutdown acknowledgement pending".into())),
        ..StopAllOutcome::default()
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("paneflow-stop-all".into())
        .spawn(move || {
            let outcome = stop_sessions_until(targets, host, deadline, &sender);
            let _ = sender.send((true, outcome));
        });
    if let Err(error) = spawned {
        last.shutdown = Some(ShutdownOutcome::Refused(error.to_string()));
        return last;
    }
    while let Ok((complete, outcome)) =
        receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        last = outcome;
        if complete {
            return last;
        }
    }
    last.shutdown = Some(ShutdownOutcome::Refused(
        "the five-second stop deadline expired; pending operations remain owned by the host".into(),
    ));
    last
}

fn stop_sessions_until(
    mut targets: Vec<(PathBuf, SessionId, SessionGeneration)>,
    host: Option<PathBuf>,
    deadline: std::time::Instant,
    progress: &std::sync::mpsc::Sender<(bool, StopAllOutcome)>,
) -> StopAllOutcome {
    let mut outcome = StopAllOutcome::default();
    if let Some(endpoint) = host.as_ref() {
        match HostClient::connect(endpoint, &ClientHello::control(CLIENT_NAME))
            .and_then(|mut client| client.list(None))
        {
            Ok(sessions) => {
                for row in sessions {
                    if row.owned
                        && !targets
                            .iter()
                            .any(|(_, session, _)| *session == row.session)
                    {
                        targets.push((endpoint.clone(), row.session, row.generation));
                    }
                }
            }
            Err(error) if error.is_endpoint_absent() => {}
            Err(error) => {
                outcome.shutdown = Some(ShutdownOutcome::Refused(error.to_string()));
                return outcome;
            }
        }
    }
    outcome.unresolved = targets
        .iter()
        .map(|(_, session, _)| (session.clone(), "stop acknowledgement pending".into()))
        .collect();
    outcome.shutdown = host
        .as_ref()
        .map(|_| ShutdownOutcome::Refused("shutdown acknowledgement pending".into()));
    let _ = progress.send((false, outcome.clone()));
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut pending = 0;
    for (endpoint, session, generation) in targets {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let sender = sender.clone();
        let spawned = std::thread::Builder::new()
            .name("paneflow-stop-session".into())
            .spawn(move || {
                let result = stop_session(&endpoint, &session, generation);
                let _ = sender.send((session, result));
            });
        if spawned.is_ok() {
            pending += 1;
        }
    }
    drop(sender);
    while pending > 0 {
        let Ok((session, result)) =
            receiver.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
        else {
            break;
        };
        pending -= 1;
        outcome.unresolved.retain(|(id, _)| *id != session);
        match result {
            Ok(summary) if summary.owns_process() => {
                log::warn!(
                    "paneflow: hosted session {session} is still owned by the host after the stop"
                );
                outcome.unresolved.push((
                    session,
                    format!(
                        "{} descendant process(es) are unresolved",
                        summary.descendants_unresolved
                    ),
                ));
            }
            Ok(summary) => {
                log::info!(
                    "paneflow: hosted session {session} stopped at quit ({})",
                    summary.manifest.lifecycle.label()
                );
                outcome.stopped += 1;
                if let Some(reason) = summary.durability_error {
                    outcome.unsaved.push((session, reason));
                }
            }
            Err(error)
                if matches!(
                    error.code(),
                    Some(ERR_SESSION_NOT_FOUND) | Some(ERR_SESSION_NOT_LIVE)
                ) =>
            {
                log::info!("paneflow: hosted session {session} was already gone at quit");
                outcome.stopped += 1;
            }
            Err(error) => {
                log::warn!("paneflow: hosted session {session} stop at quit failed: {error}");
                outcome.unresolved.push((session, error.to_string()));
            }
        }
        let _ = progress.send((false, outcome.clone()));
    }
    if let Some(endpoint) = host
        && outcome.unresolved.is_empty()
        && std::time::Instant::now() < deadline
    {
        outcome.shutdown = Some(match shutdown_host(&endpoint) {
            Ok(()) => {
                log::info!("paneflow: host shutdown acknowledged at quit");
                ShutdownOutcome::Acknowledged
            }
            Err(error) if error.is_endpoint_absent() => ShutdownOutcome::NoHost,
            Err(error) if error.code() == Some(paneflow_host::protocol::ERR_DURABILITY) => {
                ShutdownOutcome::Unsaved(error.to_string())
            }
            Err(error) => {
                log::warn!("paneflow: host shutdown at quit refused: {error}");
                ShutdownOutcome::Refused(error.to_string())
            }
        });
    }
    outcome
}

pub(crate) fn host_is_serving() -> Option<usize> {
    let target = host_endpoint()?;
    let mut client = match HostClient::connect(&target.endpoint, &ClientHello::control(CLIENT_NAME))
    {
        Ok(client) => client,
        Err(error) if error.is_endpoint_absent() => return Some(0),
        Err(_) => return None,
    };
    let listed = client.list(None).ok()?;
    Some(
        listed
            .iter()
            .filter(|summary| summary.live || summary.lifecycle.holds_ownership())
            .count(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveSession {
    pub(crate) session: SessionId,
    pub(crate) title: String,
    pub(crate) cwd: PathBuf,
}

pub(crate) enum LiveSessionProbe {
    Sessions(Vec<LiveSession>),
    NoHost,
    Unknown(String),
}

fn live_session_row(row: SessionRow) -> LiveSession {
    let title = row
        .title
        .clone()
        .filter(|title| !title.trim().is_empty())
        .or_else(|| row.agent.clone())
        .unwrap_or_else(|| row.session.to_string());
    LiveSession {
        session: row.session,
        title,
        cwd: PathBuf::from(row.cwd),
    }
}

pub(crate) fn live_sessions() -> LiveSessionProbe {
    match list_sessions(None) {
        Ok(sessions) => LiveSessionProbe::Sessions(
            sessions
                .into_iter()
                .filter(|row| row.live)
                .map(live_session_row)
                .collect(),
        ),
        Err(HostLinkError::NoHome) => LiveSessionProbe::NoHost,
        Err(HostLinkError::Client(error)) if error.is_endpoint_absent() => LiveSessionProbe::NoHost,
        Err(error) => LiveSessionProbe::Unknown(error.to_string()),
    }
}

pub(crate) fn list_sessions(
    workspace: Option<&WorkspaceId>,
) -> Result<Vec<SessionRow>, HostLinkError> {
    let target = host_endpoint().ok_or(HostLinkError::NoHome)?;
    let listed = connect(&target.endpoint)?
        .call("session.list", serde_json::json!({"workspace": workspace}))?;
    Ok(parse_session_rows(&listed))
}

pub(crate) fn parse_session_rows(listed: &serde_json::Value) -> Vec<SessionRow> {
    listed["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|row| match serde_json::from_value(row.clone()) {
            Ok(summary) => Some(summary),
            Err(error) => {
                log::warn!(
                    "paneflow: skipping a session record this build cannot read ({}): {error}",
                    row["session"].as_str().unwrap_or("unknown id")
                );
                None
            }
        })
        .collect()
}

pub(crate) fn remove_session(endpoint: &Path, session: &SessionId) -> Result<(), HostClientError> {
    connect(endpoint)?.remove(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn end_states_carry_their_exit_and_restartability() {
        let exited = HostLinkEnd::exited(3, None);
        assert_eq!(exited.exit(), Some((3, None)));
        assert!(exited.restartable());
        assert_eq!(exited.detail, "Session ended");
        assert_eq!(exited.action_label(), Some("Restart"));
        assert_eq!(
            exited.intent(Some(SessionGeneration::FIRST)),
            SessionIntent::Restart {
                expected: Some(SessionGeneration::FIRST)
            }
        );

        let incompatible = HostLinkEnd::incompatible("source_sha".into());
        assert!(!incompatible.restartable());
        assert_eq!(incompatible.exit(), None);
        assert_eq!(incompatible.action_label(), Some("Retry"));
        assert_eq!(
            incompatible.secondary_action_label(),
            Some("Stop host and restart")
        );
        assert_eq!(incompatible.intent(None), SessionIntent::Reattach);

        let missing = HostLinkEnd::missing();
        assert_eq!(missing.action_label(), Some("New terminal"));
        assert_eq!(missing.intent(None), SessionIntent::Create);
        assert!(!missing.restartable());

        let unverified = HostLinkEnd::from_reconnection(
            SessionReconnection::Unverified {
                reason: "wait failed".into(),
            },
            SessionGeneration::FIRST,
        );
        assert_eq!(unverified.kind, HostLinkEndKind::Unverified);
        assert_eq!(unverified.action_label(), Some("Retry"));
        assert!(!unverified.restartable());
        let starting =
            HostLinkEnd::from_reconnection(SessionReconnection::Starting, SessionGeneration::FIRST);
        assert_eq!(starting.kind, HostLinkEndKind::Starting);
        assert_eq!(starting.intent(None), SessionIntent::Reattach);

        let restarted = HostLinkEnd::from_reconnection(
            SessionReconnection::Live,
            SessionGeneration::FIRST.next(),
        );
        assert!(matches!(
            restarted.kind,
            HostLinkEndKind::Restarted { generation } if generation == SessionGeneration::FIRST.next()
        ));
        assert_eq!(restarted.action_label(), Some("Attach"));
        assert_eq!(restarted.intent(None), SessionIntent::Reattach);

        let lost =
            HostLinkEnd::from_reconnection(SessionReconnection::Lost, SessionGeneration::FIRST);
        assert_eq!(lost.kind, HostLinkEndKind::Lost);
        assert!(!HostLinkState::Ended(lost).accepts_input());
        assert!(!HostLinkState::Reconnecting.accepts_input());
        assert!(HostLinkState::Attaching.accepts_input());
    }

    #[test]
    fn a_restored_end_keeps_its_observed_generation_for_explicit_restart() {
        let generation = SessionGeneration::FIRST.next();
        let end = HostLinkEnd::from_reconnection(
            SessionReconnection::Exited {
                code: 0,
                signal: None,
            },
            generation,
        );
        assert_eq!(
            end.intent(None),
            SessionIntent::Restart {
                expected: Some(generation)
            }
        );
        assert_eq!(
            end.intent(Some(SessionGeneration::FIRST)),
            SessionIntent::Restart {
                expected: Some(generation)
            }
        );
        assert_eq!(
            HostLinkEnd::exited(0, None).intent(None),
            SessionIntent::Reattach
        );
    }

    #[test]
    fn a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures() {
        let confirmed = StopAllOutcome {
            stopped: 2,
            shutdown: Some(ShutdownOutcome::Acknowledged),
            ..StopAllOutcome::default()
        };
        assert!(confirmed.confirmed());
        assert!(!confirmed.durability_only());

        let unsaved = StopAllOutcome {
            stopped: 1,
            unsaved: vec![(SessionId::new(), "disk full".into())],
            shutdown: Some(ShutdownOutcome::Acknowledged),
            ..StopAllOutcome::default()
        };
        assert!(!unsaved.confirmed());
        assert!(unsaved.durability_only());
        assert!(
            unsaved
                .user_message()
                .contains("final state could not be saved")
        );

        let unresolved = StopAllOutcome {
            unresolved: vec![(SessionId::new(), "wait handle lost".into())],
            unsaved: vec![(SessionId::new(), "disk full".into())],
            shutdown: Some(ShutdownOutcome::Refused("2 unresolved".into())),
            ..StopAllOutcome::default()
        };
        assert!(!unresolved.confirmed());
        assert!(!unresolved.durability_only());
        let message = unresolved.user_message();
        assert!(message.contains("could not be confirmed stopped"));
        assert!(message.contains("refused to shut down"));
    }

    #[test]
    fn every_session_stop_goes_through_the_single_host_call_site() {
        use std::path::{Path, PathBuf};

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut violations = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let rel = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == "terminal/host_link.rs" {
                    continue;
                }
                let text = std::fs::read_to_string(&path).unwrap();
                for (i, line) in text.lines().enumerate() {
                    if line.contains("\"session.stop\"") || line.contains("client.stop(") {
                        violations.push(format!("{rel}:{}", i + 1));
                    }
                }
            }
        }
        assert!(
            violations.is_empty(),
            "the quit dialog and every close path stop sessions through host_link::stop_session; found {violations:?}"
        );
    }

    #[test]
    fn an_unreadable_record_is_skipped_and_the_rest_of_the_listing_survives() {
        let good = serde_json::json!({
            "session": SessionId::new(),
            "generation": 1,
            "cwd": "/repo",
            "shell": "/bin/zsh",
            "lifecycle": {"state": "running"},
            "reconnection": {"state": "live"},
            "live": true,
            "owned": true,
            "updated_at_ms": 2,
        });
        let mut broken = good.clone();
        broken["lifecycle"] = serde_json::json!({"state": "teleported"});
        let listed = serde_json::json!({"sessions": [broken, good]});
        let rows = parse_session_rows(&listed);
        assert_eq!(rows.len(), 1, "one unreadable record never hides the rest");
        assert_eq!(rows[0].cwd, "/repo");
        assert!(parse_session_rows(&serde_json::json!({})).is_empty());
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
    #[test]
    fn restoration_and_stale_restart_do_not_launch_through_host_ipc() {
        use std::sync::Arc;
        let home = tempfile::tempdir().unwrap();
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        let failed = SessionId::new();
        assert!(
            host.create(CreateSession {
                session: Some(failed.clone()),
                shell: Some(home.path().join("missing-shell").display().to_string()),
                ..CreateSession::default()
            })
            .is_err()
        );
        let server =
            paneflow_host::ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).unwrap();
        let target = HostEndpoint {
            home: home.path().to_path_buf(),
            endpoint,
        };
        for (session, intent) in [
            (SessionId::new(), SessionIntent::Reattach),
            (failed.clone(), SessionIntent::Reattach),
            (failed.clone(), SessionIntent::Restart { expected: None }),
            (
                failed.clone(),
                SessionIntent::Restart {
                    expected: Some(SessionGeneration::FIRST.next()),
                },
            ),
        ] {
            let outcome = resolve_at(
                AttachRequest {
                    intent,
                    session,
                    workspace: None,
                    params: SpawnParams {
                        shell: "missing-shell".into(),
                        shell_quoting: super::super::types::ShellQuoting::default_for_platform(),
                        extra_args: Vec::new(),
                        env: Default::default(),
                        cwd: home.path().to_path_buf(),
                        cols: 80,
                        rows: 24,
                        profile: paneflow_config::schema::TerminalSurfaceProfile::Normal,
                    },
                },
                target.clone(),
            )
            .unwrap();
            assert!(matches!(outcome, ResolveOutcome::Ended(..)));
            assert_eq!(host.list(None).len(), 1);
            assert_eq!(
                host.inspect(&failed).unwrap().manifest.generation,
                SessionGeneration::FIRST
            );
        }
        server.stop().unwrap();
    }

    #[test]
    fn every_retained_end_state_restores_without_creating_a_process() {
        use paneflow_host::manifest::{SessionLifecycle, write_manifest};
        let home = tempfile::tempdir().unwrap();
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        let failed = SessionId::new();
        assert!(
            host.create(CreateSession {
                session: Some(failed.clone()),
                shell: Some(home.path().join("absent").display().to_string()),
                ..CreateSession::default()
            })
            .is_err()
        );
        let template = host.inspect(&failed).unwrap().manifest;
        drop(host);
        let mut records = vec![failed];
        for lifecycle in [
            SessionLifecycle::Exited {
                code: 7,
                signal: None,
            },
            SessionLifecycle::Lost,
            SessionLifecycle::Running,
            SessionLifecycle::Unverified {
                reason: "observation failed".into(),
            },
        ] {
            let mut manifest = template.clone();
            manifest.session = SessionId::new();
            manifest.lifecycle = lifecycle;
            manifest.host_instance = HostInstanceToken::new();
            records.push(manifest.session.clone());
            write_manifest(home.path(), &manifest).unwrap();
        }
        let host = paneflow_host::SessionHost::open(home.path(), &endpoint).unwrap();
        let server =
            paneflow_host::ServerHandle::spawn(std::sync::Arc::clone(&host), endpoint.clone())
                .unwrap();
        for session in &records {
            for _ in 0..2 {
                let result = resolve_at(
                    AttachRequest {
                        intent: SessionIntent::Reattach,
                        session: session.clone(),
                        workspace: None,
                        params: SpawnParams {
                            shell: "must-not-launch".into(),
                            shell_quoting: super::super::types::ShellQuoting::default_for_platform(
                            ),
                            extra_args: Vec::new(),
                            env: Default::default(),
                            cwd: home.path().into(),
                            cols: 80,
                            rows: 24,
                            profile: paneflow_config::schema::TerminalSurfaceProfile::Normal,
                        },
                    },
                    HostEndpoint {
                        home: home.path().into(),
                        endpoint: endpoint.clone(),
                    },
                )
                .unwrap();
                assert!(matches!(result, ResolveOutcome::Ended(..)));
                assert_eq!(
                    host.inspect(session).unwrap().manifest.generation,
                    SessionGeneration::FIRST
                );
                assert_eq!(host.list(None).len(), records.len());
                assert_eq!(host.live_session_count(), 0);
            }
        }
        server.stop().unwrap();
    }
}
