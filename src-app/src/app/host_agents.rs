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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostAgentFrame {
    Snapshot(Vec<Value>),
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
    pub(crate) waiting_since_ms: Option<u64>,
    pub(crate) stale: bool,
    pub(crate) live: bool,
}

#[derive(Default)]
pub(crate) struct HostAgentView {
    rows: BTreeMap<SessionId, HostAgentRow>,
    frames: Option<Receiver<HostAgentFrame>>,
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
}

pub(crate) fn row_from_snapshot(entry: &Value) -> Option<HostAgentRow> {
    let session = SessionId::parse(entry.get("session")?.as_str()?).ok()?;
    let live = entry
        .get("live")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let agent = entry.get("agent");
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
        waiting_since_ms: agent
            .and_then(|agent| agent.get("waiting_since_ms"))
            .and_then(Value::as_u64),
        stale: agent
            .and_then(|agent| agent.get("stale"))
            .and_then(Value::as_bool)
            .unwrap_or_default(),
        live,
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

fn follow_once(endpoint: &std::path::Path, tx: &SyncSender<HostAgentFrame>) -> Result<(), String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let header = control.request(METHOD_AGENT_FOLLOW, json!({}))?;
    let sessions = header
        .get("sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    send(tx, HostAgentFrame::Snapshot(sessions))?;
    loop {
        let line = control
            .read_stream_line(STREAM_READ_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let Some(line) = line else {
            return Err("the local host closed the agent stream".to_string());
        };
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("event") => send(tx, HostAgentFrame::Event(Box::new(value)))?,
            Some("end") => {
                let reason = value
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("the local host ended the agent stream");
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
    let endpoint = paneflow_host::endpoint::host_endpoint_path_for_current_home()?;
    let (tx, rx) = sync_channel(FRAME_QUEUE_SLOTS);
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-agents".into())
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
            log::warn!("paneflow: cannot start the host agent stream thread: {error}");
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
                HostAgentFrame::Snapshot(entries) => self.apply_host_agent_snapshot(entries, cx),
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

    fn apply_host_agent_snapshot(&mut self, entries: Vec<Value>, cx: &mut Context<Self>) {
        self.host_agents.rows = entries
            .iter()
            .filter_map(row_from_snapshot)
            .map(|row| (row.session.clone(), row))
            .collect();
        self.host_agents.connected = true;
        self.host_agents.disconnect_reason = None;
        self.host_agents.bootstrapped = true;
        self.seed_attached_sessions_from_host(cx);
        self.refresh_owned_sessions(cx);
        cx.notify();
    }

    fn seed_attached_sessions_from_host(&mut self, cx: &mut Context<Self>) {
        let rows: Vec<HostAgentRow> = self
            .host_agents
            .rows
            .values()
            .filter(|row| row.state.is_some())
            .cloned()
            .collect();
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
        let (Some(tool), Some(state)) = (row.tool, row.state) else {
            return false;
        };
        let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == workspace_id) else {
            return false;
        };
        if ws
            .agent_sessions
            .values()
            .any(|session| session.surface_id == Some(surface_id))
        {
            return false;
        }
        let key = surface_id as u32;
        let mut session = AgentSession::new(tool, state);
        session.source = AgentStateSource::Hook;
        session.surface_id = Some(surface_id);
        session.message = row.message.clone();
        session.last_result = row.last_result.clone();
        session.waiting_since = (state == AgentState::WaitingForInput)
            .then(|| waiting_since_instant(row.waiting_since_ms));
        ws.agent_sessions.insert(key, session);
        if state == AgentState::Finished {
            ws.agent_completion_notification
                .record_finished(ws.muted, Some(surface_id));
        }
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
        if let Some(row) = row_from_snapshot(&json!({
            "session": session.to_string(),
            "live": true,
            "agent": frame.get("agent").cloned().unwrap_or(Value::Null),
        })) {
            match frame.get("agent") {
                Some(Value::Null) | None => {
                    self.host_agents.rows.remove(&session);
                }
                Some(_) => {
                    self.host_agents.rows.insert(session.clone(), row);
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
        let Some(params) = legacy_ai_params(frame, workspace_id, surface_id) else {
            cx.notify();
            return;
        };
        let result = self.handle_ipc(&kind, &params, None, cx);
        if result.get("error").is_none() && result.get("_jsonrpc_error").is_none() {
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

    pub(crate) fn host_agents_disconnect_reason(&self) -> Option<&str> {
        self.host_agents.disconnect_reason()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_row_reads_the_host_record_without_inferring_idle() {
        let session = SessionId::new();
        let entry = json!({
            "session": session.to_string(),
            "generation": 1,
            "live": false,
            "lifecycle": {"state": "lost"},
            "agent": {
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
        assert_eq!(row.waiting_since_ms, None);

        let bare = json!({"session": session.to_string(), "live": true});
        let row = row_from_snapshot(&bare).expect("row");
        assert_eq!(row.state, None, "no record is not an idle record");
        assert!(!row.stale);
        assert!(row.live);

        assert!(row_from_snapshot(&json!({"live": true})).is_none());
        assert!(row_from_snapshot(&json!({"session": "not-a-uuid"})).is_none());
    }

    #[test]
    fn a_host_event_is_replayed_to_the_controller_under_its_own_surface() {
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
            "agent": {
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
}
