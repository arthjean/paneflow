use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use gpui::Context;
use paneflow_config::schema::SessionId;
use paneflow_ipc_client::agent::AgentState;
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use paneflow_serve::protocol::METHOD_WORKER_HELLO;
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
    Hooks,
    Screen,
    None,
}

impl ActivitySource {
    pub(crate) fn parse(raw: Option<&str>) -> Self {
        match raw {
            Some("hooks") => Self::Hooks,
            Some("screen") => Self::Screen,
            _ => Self::None,
        }
    }

    pub(crate) fn is_hooks(self) -> bool {
        matches!(self, Self::Hooks)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostAgentFrame {
    Snapshot {
        sessions: Vec<Value>,
        capabilities: Vec<String>,
    },
    Event(Box<Value>),
    Disconnected(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HostAgentRow {
    pub(crate) session: SessionId,
    pub(crate) tool: Option<TerminalAgent>,
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
    pub(crate) restart_recommended: bool,
}

#[derive(Default)]
pub(crate) struct HostAgentView {
    rows: BTreeMap<SessionId, HostAgentRow>,
    frames: Option<Receiver<HostAgentFrame>>,
    capabilities: Vec<String>,
    connected: bool,
    bootstrapped: bool,
    disconnect_reason: Option<String>,
}

impl HostAgentView {
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
        restart_recommended: entry.get("restart_recommended").is_some_and(|value| {
            value.get("token").and_then(Value::as_str) == Some(paneflow_serve::RESTART_RECOMMENDED)
        }),
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
    session.tool = tool;
    session.state = state;
    session.source = if row.activity_source.is_hooks() {
        AgentStateSource::Hook
    } else {
        AgentStateSource::Terminal
    };
    session.active_tool_name = row.active_tool_name.clone();
    session.message = row.message.clone();
    session.last_result = row.last_result.clone();
    session.last_event_at_ms = row.last_event_at_ms;
    session.surface_id = Some(surface_id);
    session.waiting_since =
        (state == AgentState::WaitingForInput).then(|| waiting_since_instant(row.waiting_since_ms));
    session.last_activity = std::time::Instant::now();
    Some(session)
}

fn follow_once(endpoint: &std::path::Path, tx: &SyncSender<HostAgentFrame>) -> Result<(), String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let identity = control.request(METHOD_WORKER_HELLO, json!({"client": CLIENT_NAME}))?;
    let capabilities: Vec<String> = identity["capabilities"]
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

fn send(tx: &SyncSender<HostAgentFrame>, frame: HostAgentFrame) -> Result<(), String> {
    match tx.try_send(frame) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err("the desktop stopped reading".to_string()),
    }
}

pub(crate) fn spawn_follow_thread() -> Option<Receiver<HostAgentFrame>> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let (tx, rx) = sync_channel(FRAME_QUEUE_SLOTS);
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
        self.host_agents.frames = spawn_follow_thread();
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
        for frame in pending {
            match frame {
                HostAgentFrame::Snapshot {
                    sessions,
                    capabilities,
                } => self.apply_host_agent_snapshot(sessions, capabilities, cx),
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
        self.host_agents.rows = entries
            .iter()
            .filter_map(row_from_snapshot)
            .map(|row| (row.session.clone(), row))
            .collect();
        self.host_agents.capabilities = capabilities;
        self.host_agents.connected = true;
        self.host_agents.disconnect_reason = None;
        self.host_agents.bootstrapped = true;
        self.seed_attached_sessions_from_host(cx);
        self.refresh_owned_sessions(cx);
        cx.notify();
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
            self.agent_sessions_changed(cx);
        }
    }

    fn seed_session_surface(
        &mut self,
        row: &HostAgentRow,
        workspace_id: u64,
        surface_id: u64,
    ) -> bool {
        let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) else {
            return false;
        };
        let held_key = ws
            .agent_sessions
            .iter()
            .find(|(_, session)| session.surface_id == Some(surface_id))
            .map(|(key, _)| *key);
        let previous = held_key.and_then(|key| ws.agent_sessions.get(&key));
        let Some(session) = projected_session(row, surface_id, previous) else {
            if let Some(key) = held_key {
                ws.agent_sessions.remove(&key);
                return true;
            }
            return false;
        };
        let key = held_key.or(row.pid).unwrap_or(surface_id as u32);
        if let Some(old_key) = held_key
            && old_key != key
        {
            ws.agent_sessions.remove(&old_key);
        }
        ws.agent_sessions.insert(key, session);
        true
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
            self.agent_sessions_changed(cx);
        }
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
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "activity": frame.get("agent").cloned().unwrap_or(Value::Null),
            "activity_source": frame.get("activity_source").cloned().unwrap_or(Value::Null),
        }));
        if let Some(row) = row.as_ref() {
            match frame.get("agent") {
                Some(Value::Null) | None => {
                    self.host_agents.rows.remove(&session);
                }
                Some(_) => {
                    self.host_agents.rows.insert(session.clone(), row.clone());
                }
            }
        }
        self.host_agents.connected = true;
        let Some(kind) = frame.get("kind").and_then(Value::as_str).map(str::to_owned) else {
            cx.notify();
            return;
        };
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
        if let Some(params) = legacy_ai_params(frame, workspace_id, surface_id) {
            self.apply_projected_agent_metadata(&kind, &params, workspace_id, surface_id, cx);
        }
        let next_state = row.as_ref().and_then(|row| row.state);
        let hook_finished = next_state == Some(AgentState::Finished)
            && previous_state != Some(AgentState::Finished)
            && row
                .as_ref()
                .is_some_and(|row| row.activity_source.is_hooks())
            && frame.get("event_source").and_then(Value::as_str) != Some("interrupt");
        if hook_finished {
            let visible = self.surfaces_under_user_eye(workspace_id, cx);
            let notify = self.cached_config.clone();
            if let Some(workspace) = self
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == workspace_id)
            {
                let seen = crate::app::agent_status::completion_was_seen(
                    visible.as_ref(),
                    Some(surface_id),
                ) || workspace.muted;
                workspace
                    .agent_completion_notification
                    .record_finished(seen, Some(surface_id));
                if let Some(row) = row.as_ref()
                    && let Some(tool) = row.tool
                {
                    crate::app::ipc_handler::fire_turn_end_notification(
                        tool,
                        &workspace.title,
                        row.last_result.as_deref(),
                        &notify,
                        seen,
                        cx.background_executor().clone(),
                    );
                }
            }
        }
        if next_state == Some(AgentState::WaitingForInput)
            && previous_state != Some(AgentState::WaitingForInput)
            && let Some(row) = row.as_ref()
            && let Some(tool) = row.tool
            && let Some(workspace) = self
                .workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
        {
            crate::app::ipc_handler::fire_attention_notification(
                tool,
                &workspace.title,
                row.message.as_deref(),
                &self.cached_config,
                self.session_is_seen(workspace_id, row.pid.unwrap_or(surface_id as u32), cx)
                    || workspace.muted,
                cx.background_executor().clone(),
            );
        }
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
                &workspace.title,
                exit_code,
                &self.cached_config,
                self.session_is_seen(workspace_id, row.pid.unwrap_or(surface_id as u32), cx)
                    || workspace.muted,
                cx.background_executor().clone(),
            );
        }
        self.sync_attention(cx);
        self.agent_sessions_changed(cx);
        if let Some(params) = legacy_ai_params(frame, workspace_id, surface_id) {
            self.broadcast_ai_frame(&kind, &params);
        }
        cx.notify();
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

    #[test]
    fn a_snapshot_row_reads_the_worker_projection_without_inferring_idle() {
        let session = SessionId::new();
        let entry = json!({
            "session": session.to_string(),
            "generation": 1,
            "live": false,
            "lifecycle": {"state": "lost"},
            "status": "busy",
            "activity_source": "hooks",
            "activity": {
                "tool": "claude",
                "state": "thinking",
                "source": "hook",
                "stale": true,
                "updated_at_ms": 1,
            },
        });
        let row = row_from_snapshot(&entry).expect("row");
        assert_eq!(row.session, session);
        assert_eq!(row.tool, Some(TerminalAgent::ClaudeCode));
        assert_eq!(row.state, Some(AgentState::Thinking));
        assert!(row.stale);
        assert!(!row.live);
        assert_eq!(row.activity_source, ActivitySource::Hooks);
        assert!(!row.restart_recommended);
        assert_eq!(row.waiting_since_ms, None);

        let bare = json!({"session": session.to_string(), "live": true});
        let row = row_from_snapshot(&bare).expect("row");
        assert_eq!(row.state, None, "no record is not an idle record");
        assert!(!row.stale);
        assert!(row.live);
        assert_eq!(row.activity_source, ActivitySource::None);

        assert!(row_from_snapshot(&json!({"live": true})).is_none());
        assert!(row_from_snapshot(&json!({"session": "not-a-uuid"})).is_none());
    }

    #[test]
    fn a_screen_sourced_row_is_named_as_such_and_never_as_a_hook() {
        let session = SessionId::new();
        let row = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "activity_source": "screen",
            "activity": {"tool": "codex", "state": "thinking", "source": "terminal", "updated_at_ms": 1},
        }))
        .expect("row");
        assert_eq!(row.activity_source, ActivitySource::Screen);
        assert!(!row.activity_source.is_hooks());
        assert_eq!(
            row.state,
            Some(AgentState::Thinking),
            "a screen verdict still animates the spinner"
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
            "activity_source": "hooks",
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
    fn a_screen_verdict_reaches_the_controller_labelled_as_a_screen_verdict() {
        let frame = json!({
            "type": "event",
            "session": SessionId::new().to_string(),
            "kind": "ai.stop",
            "tool": "codex",
            "activity_source": "screen",
            "hook_payload": {},
        });
        let params = legacy_ai_params(&frame, 7, 11).expect("params");
        assert_eq!(params["activity_source"], "screen");
        assert!(
            !crate::app::ipc_handler::frame_is_hook_sourced(&params),
            "a screen verdict never counts as a completion"
        );
        let hooked = legacy_ai_params(
            &json!({"kind": "ai.stop", "tool": "claude", "activity_source": "hooks"}),
            7,
            11,
        )
        .expect("params");
        assert!(crate::app::ipc_handler::frame_is_hook_sourced(&hooked));
        let silent =
            legacy_ai_params(&json!({"kind": "ai.stop", "tool": "claude"}), 7, 11).expect("params");
        assert!(
            crate::app::ipc_handler::frame_is_hook_sourced(&silent),
            "a frame with no source is a direct hook client, as before the worker"
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
            "activity_source": "hooks",
            "activity": {
                "tool": "claude",
                "state": "waiting_for_input",
                "source": "hook",
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
}
