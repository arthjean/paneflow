use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use gpui::{App, AppContext, BackgroundExecutor, Context, Entity, Focusable};
use paneflow_config::schema::{LayoutNode, PaneFlowConfig, TabTitleSource, TerminalSurfaceProfile};
use paneflow_ipc_client::ai_hook::{
    AiToolName, LifecycleEventSource, METHOD_EXIT, METHOD_NOTIFICATION, METHOD_PROMPT_SUBMIT,
    METHOD_SESSION_END, METHOD_SESSION_START, METHOD_STOP, METHOD_TOOL_USE, SessionPid, SurfaceId,
};

use crate::agent_launcher::TerminalAgent;
use crate::agents::notifications::{self as desktop_notifications, DesktopNotification};
use crate::ai_types::AgentSession;
pub(crate) use crate::app::notifications::{
    fire_agent_exit_notification, fire_worker_notification,
};
use crate::app::notifications::{
    fire_attention_notification, fire_turn_end_notification, sanitize_notification_message,
};
use crate::layout::LayoutTree;
use crate::layout::{MAX_PANES, SplitDirection};
use crate::pane::Pane;
use crate::terminal::TerminalView;
use crate::workspace::{MAX_WORKSPACES, Tab, Workspace, next_workspace_id};
use crate::{PaneFlowApp, ai_types};

mod agent_frames;
mod gates;
mod jsonrpc;
mod surface_methods;
mod transcript;
mod workspace_methods;

use gates::*;
pub(crate) use jsonrpc::*;
pub(crate) use surface_methods::*;
use transcript::*;
pub(crate) use workspace_methods::*;

#[cfg(test)]
pub(crate) use agent_frames::frame_is_hook_sourced;

fn drain_ipc_requests_for_tick(
    rx: &std::sync::mpsc::Receiver<crate::ipc::IpcRequest>,
) -> Vec<crate::ipc::IpcRequest> {
    let mut ready = Vec::with_capacity(crate::ipc::IPC_DRAIN_MAX_PER_TICK);
    let mut dequeued = 0usize;

    while ready.len() < crate::ipc::IPC_DRAIN_MAX_PER_TICK
        && dequeued < crate::ipc::IPC_DRAIN_MAX_DEQUEUES_PER_TICK
    {
        let Ok(req) = rx.try_recv() else {
            break;
        };
        dequeued += 1;

        if req.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            continue;
        }

        ready.push(req);
    }

    ready
}

impl PaneFlowApp {
    pub(crate) fn process_automation_tick(&mut self, cx: &mut Context<Self>) {
        self.process_host_agent_frames(cx);
        self.process_ipc_requests(cx);
        self.broadcast_surface_changes(cx);
        self.process_config_changes(cx);
        self.process_update_check(cx);
    }

    pub(crate) fn process_ipc_requests(&mut self, cx: &mut Context<Self>) {
        for req in drain_ipc_requests_for_tick(&self.ipc_rx) {
            if req.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                continue;
            }
            req.started
                .store(true, std::sync::atomic::Ordering::Release);
            let result = self.handle_ipc(&req.method, &req.params, req.caller_pid, cx);
            if req.method.starts_with("ai.")
                && result.get("error").is_none()
                && result.get("_jsonrpc_error").is_none()
            {
                self.broadcast_ai_frame(&req.method, &req.params);
            }
            let _ = req.response_tx.send(result);
        }
    }

    pub(crate) fn broadcast_surface_changes(&mut self, cx: &mut Context<Self>) {
        if !self.event_bus.has_subscribers() {
            return;
        }
        let current = self.collect_surface_generations(cx);
        let mut seen: HashSet<u64> = HashSet::with_capacity(current.len());
        for (sid, generation) in &current {
            seen.insert(*sid);
            if self.last_broadcast_gen.get(sid).copied() != Some(*generation) {
                self.last_broadcast_gen.insert(*sid, *generation);
                let event = serde_json::json!({
                    "type": "surface_changed",
                    "surface_id": sid,
                    "output_generation": generation,
                    "ts": crate::ipc_events::now_ms(),
                });
                self.event_bus
                    .broadcast("surface_changed", Some(*sid), &event);
            }
        }
        self.last_broadcast_gen.retain(|k, _| seen.contains(k));
    }

    pub(crate) fn handle_ipc(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        caller_pid: Option<i64>,
        cx: &mut Context<Self>,
    ) -> serde_json::Value {
        match method {
            m if m.starts_with("workspace.") => self.handle_workspace_method(method, params, cx),
            m if m.starts_with("surface.") || m == "fleet.list" => {
                self.handle_surface_method(method, params, caller_pid, cx)
            }
            m if m.starts_with("ai.") => self.handle_agent_frame(method, params, cx),
            _ => JsonRpcError::method_not_found(format!("Method not found: {method}")).into_value(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, mpsc};

    fn test_ipc_request(method: &str, cancelled: bool) -> crate::ipc::IpcRequest {
        let (response_tx, _response_rx) = mpsc::channel();
        crate::ipc::IpcRequest {
            method: method.to_string(),
            params: serde_json::json!({}),
            response_tx,
            cancelled: Arc::new(AtomicBool::new(cancelled)),
            started: Arc::new(AtomicBool::new(false)),
            caller_pid: None,
        }
    }

    #[test]
    fn ipc_drain_caps_live_requests_per_tick() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..=crate::ipc::IPC_DRAIN_MAX_PER_TICK {
            tx.send(test_ipc_request("surface.read", false))
                .expect("queue test request");
        }

        let ready = drain_ipc_requests_for_tick(&rx);

        assert_eq!(ready.len(), crate::ipc::IPC_DRAIN_MAX_PER_TICK);
        assert!(
            rx.try_recv().is_ok(),
            "requests beyond the per-tick budget stay pending"
        );
    }

    #[test]
    fn ipc_drain_skips_cancelled_without_spending_live_budget() {
        let (tx, rx) = mpsc::channel();
        tx.send(test_ipc_request("surface.split", true))
            .expect("queue cancelled request");
        for _ in 0..crate::ipc::IPC_DRAIN_MAX_PER_TICK {
            tx.send(test_ipc_request("surface.read", false))
                .expect("queue live request");
        }

        let ready = drain_ipc_requests_for_tick(&rx);

        assert_eq!(ready.len(), crate::ipc::IPC_DRAIN_MAX_PER_TICK);
        assert!(
            rx.try_recv().is_err(),
            "cancelled request did not consume live handler budget"
        );
    }

    #[test]
    fn ipc_drain_caps_cancelled_dequeues_per_tick() {
        let (tx, rx) = mpsc::channel();
        for _ in 0..=crate::ipc::IPC_DRAIN_MAX_DEQUEUES_PER_TICK {
            tx.send(test_ipc_request("surface.split", true))
                .expect("queue cancelled request");
        }

        let ready = drain_ipc_requests_for_tick(&rx);

        assert!(ready.is_empty());
        assert!(
            rx.try_recv().is_ok(),
            "cancelled backlog drain is also bounded per tick"
        );
    }
}
