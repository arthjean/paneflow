use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use gpui::Context;
use paneflow_config::schema::SessionId;
use paneflow_ipc_client::agent::AgentState;
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use serde_json::{Value, json};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::ai_types::{AgentSession, AgentStateSource};

const CLIENT_NAME: &str = "paneflow-desktop";
const FRAME_QUEUE_SLOTS: usize = 512;
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);
const DRAIN_MAX_PER_TICK: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivitySource {
    Declared,
    None,
}

impl ActivitySource {
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("declared") => Self::Declared,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostAgentFrame {
    Snapshot {
        sessions: Vec<Value>,
        capabilities: Vec<String>,
        initial: bool,
    },
    Event(Box<Value>),
    Disconnected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostAgentRow {
    pub(crate) session: SessionId,
    pub(crate) tool: Option<TerminalAgent>,
    pub(crate) runtime: Option<TerminalAgent>,
    pub(crate) state: Option<AgentState>,
    pub(crate) message: Option<String>,
    pub(crate) last_result: Option<String>,
    pub(crate) active_tool_name: Option<String>,
    pub(crate) pid: Option<u32>,
    pub(crate) waiting_since_ms: Option<u64>,
    pub(crate) last_event_at_ms: Option<u64>,
    pub(crate) stale: bool,
    pub(crate) live: bool,
    pub(crate) activity_source: ActivitySource,
    pub(crate) hooked: bool,
    pub(crate) restart_recommended: bool,
    pub(crate) unread: bool,
    pub(crate) state_seq: u64,
    pub(crate) provider_session_id: Option<String>,
    pub(crate) declared_status: Option<paneflow_host::program_status::DeclaredStatus>,
    pub(crate) program_status_capable: bool,
}

#[derive(Default)]
pub(crate) struct HostAgentView {
    rows: BTreeMap<SessionId, HostAgentRow>,
    frames: Option<Receiver<HostAgentFrame>>,
    capabilities: Vec<String>,
    connected: bool,
    bootstrapped: bool,
    disconnect_reason: Option<String>,
    applied_snapshot: Option<(Vec<Value>, Vec<String>)>,
    pub(crate) declared_watch: crate::app::declared_status::DeclaredWatch,
}

impl HostAgentView {
    fn accept_snapshot(
        &mut self,
        sessions: &[Value],
        capabilities: &[String],
        initial: bool,
    ) -> bool {
        let fingerprint = (
            paneflow_serve::server::snapshot_fingerprint(sessions),
            capabilities.to_vec(),
        );
        let redundant = !initial
            && self.connected
            && self.bootstrapped
            && self.applied_snapshot.as_ref() == Some(&fingerprint);
        if !redundant {
            self.applied_snapshot = Some(fingerprint);
        }
        !redundant
    }

    fn replace_snapshot(&mut self, entries: &[Value], capabilities: Vec<String>) -> bool {
        let rows: BTreeMap<SessionId, HostAgentRow> = entries
            .iter()
            .filter_map(row_from_snapshot)
            .map(|row| (row.session.clone(), row))
            .collect();
        let changed = self.rows != rows
            || self.capabilities != capabilities
            || !self.connected
            || !self.bootstrapped
            || self.disconnect_reason.is_some();
        self.rows = rows;
        self.capabilities = capabilities;
        self.connected = true;
        self.disconnect_reason = None;
        self.bootstrapped = true;
        changed
    }

    pub(crate) fn live_session_with_provider_id(
        &self,
        tool: TerminalAgent,
        provider_session_id: &str,
        except: &SessionId,
    ) -> Option<SessionId> {
        self.rows
            .values()
            .find(|row| {
                row.live
                    && row.tool == Some(tool)
                    && &row.session != except
                    && row.provider_session_id.as_deref() == Some(provider_session_id)
            })
            .map(|row| row.session.clone())
    }

    pub(crate) fn row(&self, session: &SessionId) -> Option<&HostAgentRow> {
        self.rows.get(session)
    }

    pub(crate) fn connected(&self) -> bool {
        self.connected
    }

    pub(crate) fn disconnect_reason(&self) -> Option<&str> {
        self.disconnect_reason.as_deref()
    }

    pub(crate) fn advertises(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|held| held == capability)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerNotificationKind {
    Finished,
    NeedsInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerNotification {
    pub(crate) kind: WorkerNotificationKind,
    pub(crate) runtime_label: String,
    pub(crate) body: Option<String>,
}

impl WorkerNotification {
    pub(crate) fn from_frame(frame: &Value) -> Option<Self> {
        let notify = frame.get("notify")?;
        let kind = match notify.get("kind").and_then(Value::as_str)? {
            "finished" => WorkerNotificationKind::Finished,
            "needs_input" => WorkerNotificationKind::NeedsInput,
            _ => return None,
        };
        Some(Self {
            kind,
            runtime_label: notify
                .get("runtime_label")
                .and_then(Value::as_str)
                .unwrap_or("Agent")
                .to_owned(),
            body: notify
                .get("body")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

fn event_keeps_row(frame: &Value) -> bool {
    [frame.get("agent"), frame.get("declared_status")]
        .into_iter()
        .any(|field| field.is_some_and(|value| !value.is_null()))
}

pub(crate) fn host_observed_agent(row: Option<&HostAgentRow>) -> Option<TerminalAgent> {
    row.filter(|row| row.live).and_then(|row| row.runtime)
}

fn row_from_event(session: &SessionId, frame: &Value) -> Option<HostAgentRow> {
    row_from_snapshot(&json!({
        "session": session.to_string(),
        "live": true,
        "activity": frame.get("agent").cloned().unwrap_or(Value::Null),
        "activity_source": frame.get("activity_source").cloned().unwrap_or(Value::Null),
        "foreground_runtime_id": frame.get("foreground_runtime_id").cloned().unwrap_or(Value::Null),
        "unread": frame.get("unread").cloned().unwrap_or(Value::Null),
        "declared_status": frame.get("declared_status").cloned().unwrap_or(Value::Null),
        "program_status_capable": frame.get("program_status_capable").cloned().unwrap_or(Value::Null),
    }))
}

fn declaration_survives_observation(
    observed: Option<TerminalAgent>,
    declared_until: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    observed.is_none() && declared_until.is_some_and(|until| now < until)
}

pub(crate) fn row_from_snapshot(entry: &Value) -> Option<HostAgentRow> {
    let session = SessionId::parse(entry.get("session")?.as_str()?).ok()?;
    let live = entry
        .get("live")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let agent = entry.get("activity").or_else(|| entry.get("agent"));
    Some(HostAgentRow {
        session,
        tool: agent
            .and_then(|agent| agent.get("tool"))
            .and_then(Value::as_str)
            .and_then(TerminalAgent::from_binary),
        runtime: entry
            .get("foreground_runtime_id")
            .and_then(Value::as_str)
            .and_then(TerminalAgent::from_runtime_id),
        state: agent
            .and_then(|agent| agent.get("state"))
            .and_then(Value::as_str)
            .and_then(AgentState::parse),
        message: agent
            .and_then(|agent| agent.get("message"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        last_result: agent
            .and_then(|agent| agent.get("last_result"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        active_tool_name: agent
            .and_then(|agent| agent.get("active_tool_name"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        pid: agent
            .and_then(|agent| agent.get("pid"))
            .and_then(Value::as_u64)
            .and_then(|pid| u32::try_from(pid).ok()),
        waiting_since_ms: agent
            .and_then(|agent| agent.get("waiting_since_ms"))
            .and_then(Value::as_u64),
        last_event_at_ms: agent
            .and_then(|agent| agent.get("last_event_at_ms"))
            .and_then(Value::as_u64),
        stale: agent
            .and_then(|agent| agent.get("stale"))
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        live,
        activity_source: ActivitySource::parse(
            entry.get("activity_source").and_then(Value::as_str),
        ),
        hooked: entry
            .get("hook_revision")
            .and_then(Value::as_u64)
            .is_some_and(|revision| revision > 0),
        restart_recommended: entry.get("restart_recommended").is_some_and(|value| {
            value.get("token").and_then(Value::as_str) == Some(paneflow_serve::RESTART_RECOMMENDED)
        }),
        state_seq: entry
            .get("state_seq")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        unread: entry
            .get("unread")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        provider_session_id: agent
            .and_then(|agent| agent.get("provider_session_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        declared_status: entry
            .get("declared_status")
            .cloned()
            .and_then(|declared| serde_json::from_value(declared).ok()),
        program_status_capable: entry
            .get("program_status_capable")
            .and_then(Value::as_bool)
            .unwrap_or_default(),
    })
}

pub(crate) fn legacy_ai_params(frame: &Value, workspace_id: u64, surface_id: u64) -> Option<Value> {
    let kind = frame.get("kind")?.as_str()?;
    let mut params = json!({
        "workspace_id": workspace_id,
        "surface_id": surface_id,
        "tool": frame.get("tool").cloned().unwrap_or(Value::Null),
        "hook_payload": frame.get("hook_payload").cloned().unwrap_or_else(|| json!({})),
    });
    let map = params.as_object_mut()?;
    if map.get("tool").is_some_and(Value::is_null) {
        map.remove("tool");
    }
    for key in [
        "pid",
        "tool_name",
        "exit_code",
        "emitted_at_ms",
        "event_source",
        "activity_source",
    ] {
        match frame.get(key) {
            Some(value) if !value.is_null() => {
                map.insert(key.to_string(), value.clone());
            }
            _ => {}
        }
    }
    let _ = kind;
    Some(params)
}

pub(crate) fn waiting_since_instant(waiting_since_ms: Option<u64>) -> std::time::Instant {
    let now = std::time::Instant::now();
    let Some(since_ms) = waiting_since_ms else {
        return now;
    };
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(since_ms);
    now.checked_sub(Duration::from_millis(now_ms.saturating_sub(since_ms)))
        .unwrap_or(now)
}

fn projected_session(
    row: &HostAgentRow,
    surface_id: u64,
    previous: Option<&AgentSession>,
) -> Option<AgentSession> {
    let (tool, state) = (row.tool?, row.state?);
    let mut session = previous
        .cloned()
        .unwrap_or_else(|| AgentSession::new(tool, state));
    if session.tool != tool || (session.program_status_capable && !row.program_status_capable) {
        session.started_at = std::time::Instant::now();
    }
    session.tool = tool;
    session.state = state;
    session.source = if row.hooked {
        AgentStateSource::Hook
    } else {
        AgentStateSource::Terminal
    };
    session.active_tool_name = row.active_tool_name.clone();
    session.message = row.message.clone();
    session.last_result = row.last_result.clone();
    session.last_event_at_ms = row.last_event_at_ms;
    session.state_seq = row.state_seq;
    session.program_status_capable = row.program_status_capable;
    session.surface_id = Some(surface_id);
    session.waiting_since =
        (state == AgentState::WaitingForInput).then(|| waiting_since_instant(row.waiting_since_ms));
    session.last_activity = std::time::Instant::now();
    Some(session)
}

pub(crate) fn seed_projected_session(
    sessions: &mut std::collections::HashMap<u32, AgentSession>,
    row: &HostAgentRow,
    surface_id: u64,
    start_time: impl Fn(u32) -> Option<u64>,
) -> bool {
    let held_key = sessions
        .iter()
        .find(|(_, session)| session.surface_id == Some(surface_id))
        .map(|(key, _)| *key);
    let previous = held_key.and_then(|key| sessions.get(&key));
    let Some(mut session) = projected_session(row, surface_id, previous) else {
        return held_key.and_then(|key| sessions.remove(&key)).is_some();
    };
    let key = row
        .pid
        .filter(|pid| *pid <= i32::MAX as u32)
        .filter(|pid| {
            sessions
                .get(pid)
                .is_none_or(|held| held.surface_id == Some(surface_id))
        })
        .unwrap_or_else(|| crate::ai_types::surface_session_key(surface_id));
    if held_key != Some(key) || session.proc_start.is_none() {
        session.proc_start = (key <= i32::MAX as u32).then(|| start_time(key)).flatten();
    }
    if let Some(old_key) = held_key.filter(|old_key| *old_key != key) {
        sessions.remove(&old_key);
    }
    sessions.insert(key, session);
    true
}

struct FrameSink {
    tx: SyncSender<HostAgentFrame>,
    wake: crate::app::wake::AppWake,
}

fn follow_once(endpoint: &std::path::Path, tx: &FrameSink) -> Result<(), String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let capabilities: Vec<String> = control.identity()["capabilities"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    if !capabilities
        .iter()
        .any(|capability| capability == "agent.follow")
    {
        return Err(
            "the worker does not advertise the required agent.follow capability".to_string(),
        );
    }
    let header = control.request(METHOD_AGENT_FOLLOW, json!({}))?;
    let sessions = header
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    send(
        tx,
        HostAgentFrame::Snapshot {
            sessions,
            capabilities: capabilities.clone(),
            initial: true,
        },
    )?;
    loop {
        let line = control
            .read_stream_line(STREAM_READ_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let Some(line) = line else {
            return Err("the worker closed the agent stream".to_string());
        };
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("event") => send(tx, HostAgentFrame::Event(Box::new(value)))?,
            Some("snapshot") => {
                let sessions = value
                    .get("sessions")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                send(
                    tx,
                    HostAgentFrame::Snapshot {
                        sessions,
                        capabilities: capabilities.clone(),
                        initial: false,
                    },
                )?;
            }
            Some("end") => {
                let reason = value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("the worker ended the agent stream");
                return Err(reason.to_string());
            }
            _ => {}
        }
    }
}

fn send(sink: &FrameSink, frame: HostAgentFrame) -> Result<(), String> {
    let sent = sink.tx.try_send(frame);
    sink.wake.notify();
    match sent {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err("the desktop stopped reading".to_string()),
    }
}

pub(crate) fn acknowledge_worker_unread(
    sessions: Vec<SessionId>,
    executor: gpui::BackgroundExecutor,
) {
    if sessions.is_empty() {
        return;
    }
    let Some(endpoint) = paneflow_home::serve_endpoint_path_for_current_home() else {
        return;
    };
    executor
        .spawn(async move {
            smol::unblock(move || {
                let ids: Vec<String> = sessions.iter().map(SessionId::to_string).collect();
                match paneflow_serve::Controller::connect(&endpoint) {
                    Ok(mut controller) => {
                        if let Err(error) = controller.acknowledge(&ids) {
                            log::debug!(
                                "paneflow: the worker refused an unread acknowledgement: {error}"
                            );
                        }
                    }
                    Err(error) => log::debug!(
                        "paneflow: cannot reach the worker to acknowledge unread sessions: {error}"
                    ),
                }
            })
            .await;
        })
        .detach();
}

pub(crate) fn spawn_follow_thread(
    wake: crate::app::wake::AppWake,
) -> Option<Receiver<HostAgentFrame>> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let (tx, rx) = sync_channel(FRAME_QUEUE_SLOTS);
    let tx = FrameSink { tx, wake };
    let spawned = std::thread::Builder::new()
        .name("paneflow-worker-agents".into())
        .spawn(move || {
            loop {
                let reason = match follow_once(&endpoint, &tx) {
                    Ok(()) => "the agent stream ended".to_string(),
                    Err(reason) => reason,
                };
                if send(&tx, HostAgentFrame::Disconnected(reason)).is_err() {
                    return;
                }
                std::thread::sleep(RECONNECT_DELAY);
            }
        });
    match spawned {
        Ok(_) => Some(rx),
        Err(error) => {
            log::warn!("paneflow: cannot start the worker agent stream thread: {error}");
            None
        }
    }
}

impl PaneFlowApp {
    pub(crate) fn start_host_agent_stream(&mut self) {
        if self.host_agents.frames.is_some() {
            return;
        }
        self.host_agents.frames = spawn_follow_thread(self.app_wake.clone());
    }

    pub(crate) fn process_host_agent_frames(&mut self, cx: &mut Context<Self>) {
        let mut pending = Vec::new();
        if let Some(frames) = self.host_agents.frames.as_ref() {
            while pending.len() < DRAIN_MAX_PER_TICK {
                let Ok(frame) = frames.try_recv() else {
                    break;
                };
                pending.push(frame);
            }
        }
        if pending.len() == DRAIN_MAX_PER_TICK {
            self.app_wake.notify();
        }
        for frame in pending {
            match frame {
                HostAgentFrame::Snapshot {
                    sessions,
                    capabilities,
                    initial,
                } => {
                    if self
                        .host_agents
                        .accept_snapshot(&sessions, &capabilities, initial)
                    {
                        self.apply_host_agent_snapshot(sessions, capabilities, cx);
                    }
                }
                HostAgentFrame::Event(frame) => self.apply_host_agent_event(&frame, cx),
                HostAgentFrame::Disconnected(reason) => {
                    self.host_agents.connected = false;
                    self.host_agents.disconnect_reason = Some(reason);
                    for row in self.host_agents.rows.values_mut() {
                        row.stale = true;
                    }
                    cx.notify();
                }
            }
        }
    }

    fn apply_host_agent_snapshot(
        &mut self,
        entries: Vec<Value>,
        capabilities: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        crate::work_counters::count(&crate::work_counters::HOST_AGENT_SNAPSHOTS_APPLIED);
        let changed = self.host_agents.replace_snapshot(&entries, capabilities);
        self.apply_host_observed_agents(cx);
        self.seed_attached_sessions_from_host(cx);
        self.refresh_declared_status(cx);
        self.refresh_owned_sessions(cx);
        if changed {
            cx.notify();
        }
    }

    fn apply_host_observed_agents(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let mut agentless: Vec<(u64, u32)> = Vec::new();
        let mut changed = false;
        for ws_idx in 0..self.workspaces.len() {
            let mut detected = std::collections::HashSet::new();
            for pane in self.workspaces[ws_idx].collect_panes() {
                let terminals: Vec<gpui::Entity<crate::terminal::TerminalView>> =
                    pane.read(cx).terminals().cloned().collect();
                let mut pane_changed = false;
                for tv in terminals {
                    let surface_id = tv.entity_id().as_u64();
                    let observed =
                        host_observed_agent(self.host_agents.row(&tv.read(cx).terminal.session_id));
                    if let Some(agent) = observed {
                        detected.insert(agent.binary().to_string());
                    }
                    tv.update(cx, |view, cx| {
                        if let Some(agent) = observed {
                            view.record_observed_conversation(agent, cx);
                        }
                        let t = &mut view.terminal;
                        if declaration_survives_observation(observed, t.agent_declared_until, now) {
                            return;
                        }
                        t.agent_declared_until = None;
                        if t.detected_agent != observed || !t.agent_confirmed {
                            if observed.is_none() && t.detected_agent.is_some() {
                                agentless.push((surface_id, t.child_pid));
                            }
                            t.detected_agent = observed;
                            t.agent_confirmed = true;
                            pane_changed = true;
                        }
                    });
                }
                if pane_changed {
                    pane.update(cx, |_, cx| cx.notify());
                    changed = true;
                }
            }
            let ws = &mut self.workspaces[ws_idx];
            if ws.detected_agents != detected {
                ws.detected_agents = detected;
                changed = true;
            }
        }
        self.reap_sessions_without_agent(&agentless, cx);
        if changed {
            cx.notify();
        }
    }

    fn seed_attached_sessions_from_host(&mut self, cx: &mut Context<Self>) {
        let rows: Vec<HostAgentRow> = self.host_agents.rows.values().cloned().collect();
        let mut seeded = 0usize;
        for row in rows {
            let Some((workspace_id, surface_id)) = self.surface_for_session(&row.session, cx)
            else {
                continue;
            };
            if self.seed_session_surface(&row, workspace_id, surface_id) {
                seeded += 1;
            }
        }
        if seeded > 0 {
            self.sync_attention(cx);
        }
    }

    fn seed_session_surface(
        &mut self,
        row: &HostAgentRow,
        workspace_id: u64,
        surface_id: u64,
    ) -> bool {
        self.workspaces
            .iter_mut()
            .find(|ws| ws.id == workspace_id)
            .is_some_and(|ws| {
                seed_projected_session(
                    &mut ws.agent_sessions,
                    row,
                    surface_id,
                    paneflow_host::process::process_start_time,
                )
            })
    }

    pub(crate) fn seed_surface_from_host(
        &mut self,
        session: &SessionId,
        workspace_id: u64,
        surface_id: u64,
        cx: &mut Context<Self>,
    ) {
        let Some(row) = self.host_agents.row(session).cloned() else {
            return;
        };
        if self.seed_session_surface(&row, workspace_id, surface_id) {
            self.sync_attention(cx);
        }
        self.refresh_declared_status(cx);
    }

    pub(crate) fn surface_for_session(
        &self,
        session: &SessionId,
        cx: &gpui::App,
    ) -> Option<(u64, u64)> {
        self.workspaces.iter().find_map(|ws| {
            ws.collect_panes().iter().find_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .find(|terminal| &terminal.read(cx).terminal.session_id == session)
                    .map(|terminal| (ws.id, terminal.entity_id().as_u64()))
            })
        })
    }

    fn apply_host_agent_event(&mut self, frame: &Value, cx: &mut Context<Self>) {
        let Some(session) = frame
            .get("session")
            .and_then(Value::as_str)
            .and_then(|raw| SessionId::parse(raw).ok())
        else {
            return;
        };
        let row = row_from_event(&session, frame);
        if let Some(row) = row.as_ref() {
            if event_keeps_row(frame) {
                self.host_agents.rows.insert(session.clone(), row.clone());
            } else {
                self.host_agents.rows.remove(&session);
            }
        }
        self.host_agents.connected = true;
        self.apply_host_observed_agents(cx);
        let Some((workspace_id, surface_id)) = self.surface_for_session(&session, cx) else {
            cx.notify();
            return;
        };
        let previous_state = self
            .workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| {
                workspace
                    .agent_sessions
                    .values()
                    .find(|session| session.surface_id == Some(surface_id))
            })
            .map(|session| session.state);
        if let Some(row) = row.as_ref() {
            self.seed_session_surface(row, workspace_id, surface_id);
        }
        self.refresh_declared_status(cx);
        if let Some(decision) = WorkerNotification::from_frame(frame) {
            self.deliver_worker_notification(&decision, &session, workspace_id, surface_id, cx);
        }
        let Some(kind) = frame.get("kind").and_then(Value::as_str).map(str::to_owned) else {
            self.sync_attention(cx);
            cx.notify();
            return;
        };
        if let Some(params) = legacy_ai_params(frame, workspace_id, surface_id) {
            self.apply_projected_agent_metadata(&kind, &params, workspace_id, surface_id, cx);
        }
        let next_state = row.as_ref().and_then(|row| row.state);
        if next_state == Some(AgentState::Errored)
            && previous_state != Some(AgentState::Errored)
            && let Some(exit_code) = frame
                .get("exit_code")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok())
            && let Some(row) = row.as_ref()
            && let Some(tool) = row.tool
            && let Some(workspace) = self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
        {
            crate::app::ipc_handler::fire_agent_exit_notification(
                tool,
                workspace,
                exit_code,
                &self.cached_config,
                crate::app::agent_status::completion_was_seen(
                    self.surfaces_under_user_eye(workspace_id, cx).as_ref(),
                    Some(surface_id),
                ) || workspace.muted,
                cx.background_executor().clone(),
            );
        }
        self.sync_attention(cx);
        if let Some(params) = legacy_ai_params(frame, workspace_id, surface_id) {
            self.broadcast_ai_frame(&kind, &params);
        }
        cx.notify();
    }

    pub(crate) fn unread_sessions_for_surfaces(
        &self,
        surfaces: &std::collections::HashSet<u64>,
        cx: &gpui::App,
    ) -> Vec<SessionId> {
        self.host_agents
            .rows
            .values()
            .filter(|row| row.unread)
            .filter(|row| {
                self.surface_for_session(&row.session, cx)
                    .is_some_and(|(_, surface)| surfaces.contains(&surface))
            })
            .map(|row| row.session.clone())
            .collect()
    }

    fn deliver_worker_notification(
        &mut self,
        decision: &WorkerNotification,
        session: &SessionId,
        workspace_id: u64,
        surface_id: u64,
        cx: &mut Context<Self>,
    ) {
        let visible = self.surfaces_under_user_eye(workspace_id, cx);
        let pane_title = self.surface_pane_title(surface_id, cx);
        let config = self.cached_config.clone();
        let executor = cx.background_executor().clone();
        let Some(workspace) = self
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.id == workspace_id)
        else {
            return;
        };
        let seen =
            crate::app::agent_status::completion_was_seen(visible.as_ref(), Some(surface_id))
                || workspace.muted;
        let space = crate::app::spaces::declared_space_label(&config, workspace);
        let notification = match decision.kind {
            WorkerNotificationKind::Finished => {
                workspace
                    .agent_completion_notification
                    .record_finished(seen, Some(surface_id));
                if seen {
                    acknowledge_worker_unread(vec![session.clone()], executor.clone());
                }
                crate::agents::notifications::DesktopNotification::turn_finished_for(
                    &decision.runtime_label,
                    &workspace.title,
                    decision.body.as_deref(),
                    space,
                )
            }
            WorkerNotificationKind::NeedsInput => {
                crate::agents::notifications::DesktopNotification::needs_input_for(
                    &decision.runtime_label,
                    &workspace.title,
                    pane_title.as_deref(),
                    decision.body.as_deref(),
                    space,
                )
            }
        };
        crate::app::ipc_handler::fire_worker_notification(
            notification,
            &config,
            seen,
            Some(surface_id),
            executor,
        );
    }

    fn surface_pane_title(&self, surface_id: u64, cx: &gpui::App) -> Option<String> {
        self.workspaces.iter().find_map(|ws| {
            ws.collect_panes().iter().find_map(|pane| {
                pane.read(cx)
                    .terminals()
                    .find(|terminal| terminal.entity_id().as_u64() == surface_id)
                    .map(|terminal| crate::pane::Pane::terminal_surface_title(terminal, cx))
            })
        })
    }

    pub(crate) fn host_agent_row(&self, session: &SessionId) -> Option<&HostAgentRow> {
        self.host_agents.row(session)
    }

    pub(crate) fn host_agents_are_stale(&self) -> bool {
        self.host_agents.bootstrapped && !self.host_agents.connected()
    }

    pub(crate) fn host_agents_are_settled(&self) -> bool {
        self.host_agents.bootstrapped && self.host_agents.connected()
    }

    pub(crate) fn host_agents_disconnect_reason(&self) -> Option<&str> {
        self.host_agents.disconnect_reason()
    }

    pub(crate) fn worker_advertises(&self, capability: &str) -> bool {
        self.host_agents.advertises(capability)
    }

    pub(crate) fn worker_is_reconnecting(&self) -> bool {
        !self.host_agents.connected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamped_entry(session: &SessionId, status: &str, updated_at_ms: u64) -> Value {
        json!({
            "session": session.to_string(),
            "live": true,
            "status": status,
            "updated_at_ms": updated_at_ms,
        })
    }

    #[test]
    fn an_identical_snapshot_is_not_applied_again_but_a_reconnection_header_is() {
        let session = SessionId::new();
        let capabilities = vec!["agent.follow".to_string()];
        let mut view = HostAgentView::default();
        let first = vec![stamped_entry(&session, "busy", 1)];
        assert!(view.accept_snapshot(&first, &capabilities, true));
        view.connected = true;
        view.bootstrapped = true;

        let restamped = vec![stamped_entry(&session, "busy", 2)];
        assert!(
            !view.accept_snapshot(&restamped, &capabilities, false),
            "an identical snapshot neither refreshes the session list nor renders"
        );

        let changed = vec![stamped_entry(&session, "idle", 3)];
        assert!(view.accept_snapshot(&changed, &capabilities, false));
        assert!(!view.accept_snapshot(&changed, &capabilities, false));
        assert!(view.accept_snapshot(&changed, &["other".to_string()], false));

        assert!(
            view.accept_snapshot(&changed, &["other".to_string()], true),
            "the header of a resumed follow is applied even when identical"
        );

        view.connected = false;
        assert!(
            view.accept_snapshot(&changed, &["other".to_string()], false),
            "a snapshot after a disconnection is applied"
        );
    }

    #[test]
    fn a_snapshot_that_changes_only_what_no_row_reads_changes_nothing_shown() {
        let session = SessionId::new();
        let capabilities = vec!["agent.follow".to_string()];
        let entry = |title: &str, status: &str| json!({"session": session.to_string(), "live": true, "title": title, "status": status});
        let mut view = HostAgentView::default();
        assert!(view.replace_snapshot(&[entry("a", "busy")], capabilities.clone()));
        assert!(!view.replace_snapshot(&[entry("b", "busy")], capabilities.clone()));
        assert!(view.replace_snapshot(&[], capabilities.clone()));
        assert!(view.replace_snapshot(&[entry("b", "busy")], vec![]));
        view.connected = false;
        view.disconnect_reason = Some("worker restarted".to_string());
        assert!(view.replace_snapshot(&[entry("b", "busy")], vec![]));
        assert!(view.connected && view.disconnect_reason.is_none());
    }

    #[test]
    fn a_snapshot_row_reads_the_worker_projection_without_inferring_idle() {
        let session = SessionId::new();
        let entry = json!({
            "session": session.to_string(),
            "generation": 1,
            "live": false,
            "lifecycle": {"state": "lost"},
            "status": "busy",
            "activity_source": "declared",
            "hook_revision": 3,
            "unread": true,
            "activity": {
                "tool": "claude",
                "state": "thinking",
                "source": "hook",
                "stale": true,
                "updated_at_ms": 1,
            },
        });
        let row = row_from_snapshot(&entry).expect("row");
        assert!(
            row.unread,
            "the attention queue is read off the worker projection, never kept privately"
        );
        assert_eq!(row.session, session);
        assert_eq!(row.tool, Some(TerminalAgent::ClaudeCode));
        assert_eq!(row.state, Some(AgentState::Thinking));
        assert!(row.stale);
        assert!(!row.live);
        assert_eq!(row.activity_source, ActivitySource::Declared);
        assert!(row.hooked);
        assert!(!row.restart_recommended);
        assert_eq!(row.waiting_since_ms, None);

        let bare = json!({"session": session.to_string(), "live": true});
        let row = row_from_snapshot(&bare).expect("row");
        assert!(!row.unread);
        assert_eq!(row.state, None, "no record is not an idle record");
        assert!(!row.stale);
        assert!(row.live);
        assert_eq!(row.activity_source, ActivitySource::None);
        assert!(!row.hooked);

        assert!(row_from_snapshot(&json!({"live": true})).is_none());
        assert!(row_from_snapshot(&json!({"session": "not-a-uuid"})).is_none());
    }

    #[test]
    fn a_declaration_survives_only_absent_observation_before_its_deadline() {
        let now = std::time::Instant::now();
        let future = now.checked_add(std::time::Duration::from_secs(5));
        let past = now.checked_sub(std::time::Duration::from_secs(5));

        assert!(declaration_survives_observation(None, future, now));
        assert!(!declaration_survives_observation(None, past, now));
        assert!(!declaration_survives_observation(None, None, now));
        assert!(!declaration_survives_observation(
            Some(TerminalAgent::ClaudeCode),
            future,
            now
        ));
    }

    #[test]
    fn a_hosted_codex_pane_yields_one_sidebar_row_from_the_host_observation() {
        let row = HostAgentRow {
            live: true,
            runtime: Some(TerminalAgent::Codex),
            ..projected_row(Some("codex"), Some(4242))
        };
        let observed = host_observed_agent(Some(&row));
        assert_eq!(observed, Some(TerminalAgent::Codex));
        assert_eq!(
            host_observed_agent(Some(&HostAgentRow {
                live: false,
                ..row.clone()
            })),
            None
        );

        let session = projected_session(&row, 9, None).expect("the host row projects a session");
        let detected: std::collections::HashSet<String> = observed
            .map(|agent| agent.binary().to_string())
            .into_iter()
            .collect();
        let status = crate::ai_types::workspace_agent_status([&session], &detected);
        assert_eq!(status.active_labels, vec!["Codex".to_string()]);
        assert!(status.unhooked.is_empty(), "{:?}", status.unhooked);
        assert_eq!(
            crate::workspace::PaneScan::default(),
            crate::workspace::PaneScan {
                ports: Vec::new(),
                foreground_command: None,
            },
            "the port scan carries no agent evidence that could add a second row"
        );
    }

    #[test]
    fn an_agent_without_hooks_is_observed_from_the_host_runtime_before_any_activity() {
        let row = row_from_snapshot(&json!({
            "session": SessionId::new().to_string(),
            "live": true,
            "activity": null,
            "runtime_id": "com.sourcegraph.amp",
            "foreground_runtime_id": "com.sourcegraph.amp",
        }))
        .expect("row");
        assert_eq!(row.tool, None);
        assert_eq!(host_observed_agent(Some(&row)), Some(TerminalAgent::Amp));
        assert_eq!(
            host_observed_agent(Some(&HostAgentRow {
                runtime: None,
                ..row
            })),
            None
        );
    }

    #[test]
    fn a_claude_session_that_returned_to_the_shell_is_no_longer_observed() {
        let row = row_from_snapshot(&json!({
            "session": SessionId::new().to_string(),
            "live": true,
            "activity": {"tool": "claude", "state": "finished", "source": "hook", "updated_at_ms": 1},
            "runtime_id": "com.anthropic.claude-code",
            "foreground_runtime_id": null,
        }))
        .expect("row");
        assert_eq!(row.tool, Some(TerminalAgent::ClaudeCode));
        assert_eq!(host_observed_agent(Some(&row)), None);
    }

    #[test]
    fn a_worker_event_observes_the_agent_while_it_runs_and_releases_it_once_it_exits() {
        let session = SessionId::new();
        let frame = |foreground: Value| {
            json!({
                "type": "event",
                "session": session.to_string(),
                "agent": {"tool": "claude", "state": "finished", "source": "hook", "updated_at_ms": 1},
                "activity_source": "none",
                "status": "idle",
                "runtime_id": "com.anthropic.claude-code",
                "foreground_runtime_id": foreground,
            })
        };

        let running = row_from_event(&session, &frame(json!("com.anthropic.claude-code")));
        assert_eq!(
            host_observed_agent(running.as_ref()),
            Some(TerminalAgent::ClaudeCode)
        );

        let exited = row_from_event(&session, &frame(Value::Null));
        assert_eq!(host_observed_agent(exited.as_ref()), None);
    }

    #[test]
    fn a_blocked_declaration_from_an_agent_without_hooks_waits_for_input() {
        let home = tempfile::tempdir().expect("home");
        let session = SessionId::new();
        let mut worker = paneflow_serve::state::WorkerState::new(home.path());
        let row = |declared: Option<&str>| {
            let mut row = json!({
                "session": session.to_string(),
                "generation": 1,
                "generation_started_at_ms": 1_000,
                "live": true,
                "lifecycle": paneflow_host::manifest::SessionLifecycle::Running,
                "host_protocol_version": paneflow_host::HOST_PROTOCOL_VERSION,
                "host_build_id": "test-build",
                "observed_runtime": paneflow_host::runtime_observer::RuntimeObservation {
                    id: "com.sourcegraph.amp".to_string(),
                    pid: 40,
                    pid_started_at: Some(7),
                    process_group: 40,
                    process_name: "amp".to_string(),
                    argv: None,
                },
            });
            if let Some(state) = declared {
                row["declared_status"] = json!({"state": state, "message": "Approve?"});
            }
            row
        };
        worker.apply_core_snapshot(&[row(None)]);
        let projection = worker
            .apply_core_snapshot(&[row(Some("blocked"))])
            .into_iter()
            .next()
            .expect("the declaration is announced");
        assert_eq!(projection.session["status"], "attention");
        assert_eq!(projection.session["activity_source"], "declared");
        let host_row = row_from_snapshot(&projection.session).expect("row");
        let session = projected_session(&host_row, 9, None).expect("agent session");
        assert_eq!(session.state, AgentState::WaitingForInput);
        assert_eq!(
            session.tool,
            TerminalAgent::from_binary("amp").expect("amp")
        );
        assert_eq!(session.source, AgentStateSource::Terminal);
        assert!(session.state_seq > 0);
        assert!(session.waiting_since.is_some());
    }

    #[cfg(unix)]
    const QUERY_THEN_WORKING_COMMAND: &[u8] =
        b"printf '\\033]7501;?\\033\\\\\\033]7501;state=working\\033\\\\'; sleep 30\r\n";

    #[cfg(windows)]
    const QUERY_THEN_WORKING_COMMAND: &[u8] = b"powershell -NoProfile -Command \"[Console]::Write([char]27 + ']7501;?' + [char]7 + [char]27 + ']7501;state=working' + [char]7); Start-Sleep 30\"\r\n";

    #[test]
    fn a_support_query_then_working_reaches_the_agent_session_as_capable_and_thinking() {
        let home = tempfile::tempdir().expect("home");
        let host = paneflow_host::host::SessionHost::open(
            home.path(),
            std::path::Path::new("program-status-capability"),
        )
        .expect("session host");
        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<String>::new());
        let session = host
            .create(paneflow_host::host::CreateSession {
                shell: Some(shell.to_string()),
                args,
                cwd: Some(home.path().display().to_string()),
                cols: Some(120),
                rows: Some(24),
                ..Default::default()
            })
            .expect("hosted session")
            .manifest
            .session;
        let host_entry = |host: &paneflow_host::host::SessionHost| {
            host.agent_snapshot()
                .into_iter()
                .find(|entry| entry.session == session)
                .expect("snapshot entry")
        };
        assert!(!host_entry(&host).program_status_capable);

        host.input(&session, None, QUERY_THEN_WORKING_COMMAND.to_vec())
            .expect("input");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let entry = loop {
            let entry = host_entry(&host);
            let working = entry
                .declared_status
                .as_ref()
                .is_some_and(|declared| declared.state == "working");
            if entry.program_status_capable && working {
                break entry;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the host never published the capability: {entry:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let capable_manifest = host
            .inspect(&session)
            .expect("manifest")
            .manifest
            .program_status_capable;
        let _ = host.stop(&session, None);
        assert!(capable_manifest, "the manifest carries the capability");

        let mut core_row = serde_json::to_value(&entry).expect("snapshot entry json");
        core_row["observed_runtime"] = json!(paneflow_host::runtime_observer::RuntimeObservation {
            id: "com.sourcegraph.amp".to_string(),
            pid: 40,
            pid_started_at: Some(7),
            process_group: 40,
            process_name: "amp".to_string(),
            argv: None,
        });
        let mut worker = paneflow_serve::state::WorkerState::new(home.path());
        worker.apply_core_snapshot(&[core_row]);
        let follow_header = worker.snapshot();
        assert_eq!(follow_header[0]["program_status_capable"], true);

        let row = row_from_snapshot(&follow_header[0]).expect("row");
        assert!(row.program_status_capable);
        let agent = projected_session(&row, 9, None).expect("agent session");
        assert!(agent.program_status_capable);
        assert_eq!(agent.state, AgentState::Thinking);
    }

    #[test]
    fn a_row_from_a_host_without_the_capability_field_is_not_capable() {
        let row = row_from_snapshot(&json!({
            "session": SessionId::new().to_string(),
            "live": true,
            "activity": {"tool": "claude", "state": "idle"},
        }))
        .expect("row");
        assert!(!row.program_status_capable);
        assert!(
            !projected_session(&row, 3, None).is_some_and(|session| session.program_status_capable)
        );
        let event_row = row_from_snapshot(&json!({
            "session": SessionId::new().to_string(),
            "live": true,
            "program_status_capable": true,
        }))
        .expect("event row");
        assert!(event_row.program_status_capable);
    }

    #[test]
    fn the_no_status_grace_restarts_only_with_a_new_runtime_generation() {
        let row = |capable: bool| {
            row_from_snapshot(&json!({
                "session": SessionId::new().to_string(),
                "live": true,
                "activity": {"tool": "claude", "state": "finished"},
                "program_status_capable": capable,
            }))
            .expect("row")
        };
        let mut held = projected_session(&row(true), 3, None).expect("session");
        let long_ago = std::time::Instant::now() - Duration::from_secs(60);
        held.started_at = long_ago;

        let same = projected_session(&row(true), 3, Some(&held)).expect("session");
        assert_eq!(same.started_at, long_ago, "a later record keeps the start");

        let restarted = projected_session(&row(false), 3, Some(&held)).expect("session");
        assert!(
            restarted.started_at > long_ago,
            "a capability reset marks a new agent process"
        );

        held.program_status_capable = false;
        let still_silent = projected_session(&row(false), 3, Some(&held)).expect("session");
        assert_eq!(still_silent.started_at, long_ago);
    }

    #[test]
    fn a_program_without_an_agent_reaches_the_desktop_with_its_declared_status() {
        let home = tempfile::tempdir().expect("home");
        let session = SessionId::new();
        let mut worker = paneflow_serve::state::WorkerState::new(home.path());
        let host_row = json!({
            "session": session.to_string(),
            "generation": 1,
            "live": true,
            "lifecycle": paneflow_host::manifest::SessionLifecycle::Running,
            "declared_status": paneflow_host::program_status::DeclaredStatus {
                state: "blocked".to_string(),
                kind: Some("permission".to_string()),
                progress: None,
                app: "terraform".to_string(),
                title: String::new(),
                message: "Apply the plan?".to_string(),
            },
        });
        worker.apply_core_snapshot(&[host_row]);

        let follow_header = worker.snapshot();
        let row = row_from_snapshot(&follow_header[0]).expect("row");

        assert_eq!(
            row.declared_status
                .as_ref()
                .map(|status| status.state.as_str()),
            Some("blocked")
        );
        assert_eq!(host_observed_agent(Some(&row)), None);
        assert!(
            projected_session(&row, 5, None).is_none(),
            "the surface never becomes an agent session"
        );
        assert_eq!(
            crate::app::declared_status::declared_chip(row.declared_status.as_ref()),
            crate::app::declared_status::DeclaredChip::Show {
                label: "blocked: permission · Apply the plan?".into(),
                error: false,
            }
        );
        assert_eq!(
            crate::app::declared_status::declared_attention(row.declared_status.as_ref()),
            Some(crate::app::attention::AttentionLevel::Input)
        );
    }

    #[test]
    fn a_malformed_declared_status_is_dropped_without_losing_the_row() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "declared_status": "blocked",
        }))
        .expect("row");
        assert_eq!(row.declared_status, None);
    }

    #[test]
    fn an_event_keeps_the_row_of_a_program_that_still_declares_a_status() {
        let declared = json!({"state": "error", "app": "cargo"});
        assert!(event_keeps_row(
            &json!({"agent": null, "declared_status": declared})
        ));
        assert!(event_keeps_row(
            &json!({"agent": {"tool": "claude"}, "declared_status": null})
        ));
        assert!(!event_keeps_row(
            &json!({"agent": null, "declared_status": null})
        ));
        assert!(!event_keeps_row(&json!({})));
    }

    #[test]
    fn a_declared_row_without_hooks_is_named_as_terminal_sourced() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "activity_source": "declared",
            "activity": {"tool": "codex", "state": "thinking", "source": "terminal", "updated_at_ms": 1},
        }))
        .expect("row");
        assert_eq!(row.activity_source, ActivitySource::Declared);
        assert!(!row.hooked);
        assert_eq!(
            row.state,
            Some(AgentState::Thinking),
            "a declared working state animates the spinner"
        );
        let projected = projected_session(&row, 17, None).expect("projected session");
        assert_eq!(projected.state, AgentState::Thinking);
        assert_eq!(projected.source, AgentStateSource::Terminal);
        assert_eq!(projected.surface_id, Some(17));
    }

    #[test]
    fn a_worker_projection_overwrites_the_controller_state_instead_of_reducing_again() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "activity_source": "declared",
            "hook_revision": 1,
            "activity": {
                "tool": "claude",
                "state": "waiting_for_input",
                "source": "hook",
                "active_tool_name": "AskUserQuestion",
                "message": "Pick one",
                "waiting_since_ms": 1,
                "last_event_at_ms": 2,
                "updated_at_ms": 3,
            },
        }))
        .expect("row");
        let mut local = AgentSession::new(TerminalAgent::ClaudeCode, AgentState::Thinking);
        local.source = AgentStateSource::Terminal;
        let projected = projected_session(&row, 19, Some(&local)).expect("projected session");
        assert_eq!(projected.state, AgentState::WaitingForInput);
        assert_eq!(projected.source, AgentStateSource::Hook);
        assert_eq!(
            projected.active_tool_name.as_deref(),
            Some("AskUserQuestion")
        );
        assert_eq!(projected.message.as_deref(), Some("Pick one"));
        assert_eq!(projected.last_event_at_ms, Some(2));
    }

    #[test]
    fn a_session_on_an_older_core_carries_the_restart_recommendation() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "restart_recommended": {
                "token": paneflow_serve::RESTART_RECOMMENDED,
                "required_protocol": 1,
                "core_protocol": 0,
            },
        }))
        .expect("row");
        assert!(row.restart_recommended);
    }

    #[test]
    fn a_worker_event_is_replayed_to_the_controller_under_its_own_surface() {
        let frame = json!({
            "type": "event",
            "session": SessionId::new().to_string(),
            "kind": "ai.exit",
            "tool": "codex",
            "pid": 42,
            "exit_code": 1,
            "tool_name": Value::Null,
            "emitted_at_ms": 99,
            "hook_payload": {"summary": "boom"},
        });
        let params = legacy_ai_params(&frame, 7, 11).expect("params");

        assert_eq!(params["workspace_id"], 7);
        assert_eq!(params["surface_id"], 11);
        assert_eq!(params["tool"], "codex");
        assert_eq!(params["pid"], 42);
        assert_eq!(params["exit_code"], 1);
        assert_eq!(params["emitted_at_ms"], 99);
        assert_eq!(params["hook_payload"]["summary"], "boom");
        assert!(
            params.get("tool_name").is_none(),
            "a null field is absent, never a forged value"
        );
        assert!(legacy_ai_params(&json!({}), 7, 11).is_none());
    }

    #[test]
    fn a_declared_verdict_reaches_the_controller_labelled_as_declared() {
        let frame = json!({
            "type": "event",
            "session": SessionId::new().to_string(),
            "kind": "ai.stop",
            "tool": "codex",
            "activity_source": "declared",
            "hook_payload": {},
        });
        let params = legacy_ai_params(&frame, 7, 11).expect("params");
        assert_eq!(params["activity_source"], "declared");
        let undeclared = legacy_ai_params(
            &json!({"kind": "ai.stop", "tool": "claude", "activity_source": "none"}),
            7,
            11,
        )
        .expect("params");
        assert_eq!(undeclared["activity_source"], "none");
        let silent =
            legacy_ai_params(&json!({"kind": "ai.stop", "tool": "claude"}), 7, 11).expect("params");
        assert!(
            silent["activity_source"].is_null(),
            "a frame with no source is never labelled as a hook"
        );
    }

    #[test]
    fn an_interrupt_marker_survives_the_replay_to_the_controller() {
        let frame = json!({
            "type": "event",
            "session": SessionId::new().to_string(),
            "kind": "ai.stop",
            "tool": "claude",
            "event_source": "interrupt",
            "hook_payload": {},
        });
        let params = legacy_ai_params(&frame, 7, 11).expect("params");
        assert_eq!(params["event_source"], "interrupt");
    }

    #[test]
    fn a_host_waiting_stamp_is_carried_back_in_time_not_restarted() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "activity_source": "declared",
            "activity": {
                "tool": "claude",
                "state": "waiting_for_input",
                "source": "terminal",
                "waiting_since_ms": 1_000,
                "updated_at_ms": 1_000,
            },
        }))
        .expect("row");
        assert_eq!(row.waiting_since_ms, Some(1_000));

        let before = std::time::Instant::now();
        let restored = waiting_since_instant(row.waiting_since_ms);
        assert!(
            restored <= before,
            "a wait that started in the past is not restarted on reopen"
        );
        assert!(waiting_since_instant(None) >= before);
    }

    #[test]
    fn a_controller_reads_the_advertised_set_instead_of_probing_for_a_feature() {
        let mut view = HostAgentView::default();
        assert!(!view.advertises("agent.follow"));
        view.capabilities = paneflow_serve::advertised_capabilities();
        assert!(view.advertises("agent.follow"));
        assert!(view.advertises("restart.recommendation"));
        assert!(!view.advertises("screen.scan"));
    }

    #[test]
    fn the_controller_delivers_the_workers_decision_and_never_infers_one_of_its_own() {
        let finished = WorkerNotification::from_frame(&json!({
            "status": "idle",
            "activity_source": "declared",
            "notify": {"kind": "finished", "runtime_label": "Claude Code", "body": "2 files changed"},
        }))
        .expect("a worker completion reaches the desktop");
        assert_eq!(finished.kind, WorkerNotificationKind::Finished);
        assert_eq!(finished.runtime_label, "Claude Code");
        assert_eq!(finished.body.as_deref(), Some("2 files changed"));

        let asking = WorkerNotification::from_frame(&json!({
            "notify": {"kind": "needs_input", "runtime_label": "Codex", "body": null},
        }))
        .expect("a worker attention edge reaches the desktop");
        assert_eq!(asking.kind, WorkerNotificationKind::NeedsInput);
        assert_eq!(asking.body, None);

        assert_eq!(
            WorkerNotification::from_frame(&json!({
                "status": "idle",
                "activity_source": "declared",
                "notify": Value::Null,
            })),
            None,
            "a settle without a notification carries no decision, so the desktop stays quiet"
        );
        assert_eq!(
            WorkerNotification::from_frame(&json!({"status": "idle", "outcome": "expired"})),
            None
        );
        assert_eq!(
            WorkerNotification::from_frame(&json!({"notify": {"kind": "alert"}})),
            None
        );
    }

    fn projected_row(tool: Option<&str>, pid: Option<u32>) -> HostAgentRow {
        let activity = tool.map(|tool| {
            json!({"tool": tool, "state": "thinking", "source": "hook", "pid": pid, "updated_at_ms": 1})
        });
        row_from_snapshot(&json!({
            "session": SessionId::new().to_string(),
            "live": true,
            "activity": activity,
        }))
        .expect("row")
    }

    fn surfaces_of(sessions: &std::collections::HashMap<u32, AgentSession>) -> Vec<(u32, u64)> {
        let mut held: Vec<(u32, u64)> = sessions
            .iter()
            .filter_map(|(key, session)| session.surface_id.map(|surface| (*key, surface)))
            .collect();
        held.sort_unstable();
        held
    }

    #[test]
    fn two_claude_sessions_keep_two_rows_and_closing_pane_a_leaves_b_on_screen() {
        let mut sessions = std::collections::HashMap::new();
        let start = |pid: u32| Some(u64::from(pid) * 10);
        assert!(seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(101)),
            1,
            start
        ));
        assert!(seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(102)),
            2,
            start
        ));
        assert_eq!(surfaces_of(&sessions), vec![(101, 1), (102, 2)]);
        assert_eq!(sessions[&101].proc_start, Some(1010));

        assert!(seed_projected_session(
            &mut sessions,
            &projected_row(None, None),
            1,
            start
        ));
        assert_eq!(surfaces_of(&sessions), vec![(102, 2)]);
        assert_eq!(sessions[&102].tool, TerminalAgent::ClaudeCode);
    }

    #[test]
    fn a_recycled_pid_on_another_surface_never_replaces_the_row_that_held_it() {
        let mut sessions = std::collections::HashMap::new();
        let probed = std::cell::RefCell::new(Vec::new());
        let start = |pid: u32| {
            probed.borrow_mut().push(pid);
            Some(7)
        };
        seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(4242)),
            1,
            start,
        );
        seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(4242)),
            2,
            start,
        );

        let band = crate::ai_types::surface_session_key(2);
        assert_eq!(surfaces_of(&sessions), vec![(4242, 1), (band, 2)]);
        assert_eq!(
            sessions[&band].proc_start, None,
            "a surface key is never probed"
        );
        assert_eq!(*probed.borrow(), vec![4242]);

        seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(5151)),
            1,
            start,
        );
        assert_eq!(surfaces_of(&sessions), vec![(5151, 1), (band, 2)]);
        assert_eq!(
            *probed.borrow(),
            vec![4242, 5151],
            "a new agent on the surface pins its own start time"
        );
        seed_projected_session(
            &mut sessions,
            &projected_row(Some("claude"), Some(5151)),
            1,
            start,
        );
        assert_eq!(
            probed.borrow().len(),
            2,
            "an unchanged agent is not probed again"
        );
    }
}
