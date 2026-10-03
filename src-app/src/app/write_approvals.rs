use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::{Duration, Instant};

use gpui::Context;
use paneflow_config::schema::SessionId;
use serde_json::Value;

use crate::PaneFlowApp;
use crate::terminal::view::conversation::WriteRequest;

const THREAD_NAME: &str = "paneflow-write-approvals";
const DECISION_THREAD_NAME: &str = "paneflow-write-decision";
const CLIENT_NAME: &str = "paneflow-desktop-approvals";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const SNAPSHOT_QUEUE_SLOTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingWrite {
    pub(crate) id: u64,
    pub(crate) source: String,
    pub(crate) target: SessionId,
    pub(crate) asked_at: Instant,
}

impl PendingWrite {
    pub(crate) fn request(&self) -> WriteRequest {
        WriteRequest {
            id: self.id,
            source: self.source.clone(),
        }
    }
}

#[derive(Default)]
pub(crate) struct WriteApprovalView {
    pub(crate) pending: Vec<PendingWrite>,
    snapshots: Option<Receiver<Vec<PendingWrite>>>,
}

pub(crate) fn pending_writes(snapshot: &Value) -> Vec<PendingWrite> {
    snapshot["pending"]
        .as_array()
        .map(|requests| {
            requests
                .iter()
                .filter_map(|request| {
                    let remaining = Duration::from_millis(request["expires_in_ms"].as_u64()?);
                    let waited =
                        paneflow_host::agent_write::APPROVAL_REQUEST_TTL.saturating_sub(remaining);
                    Some(PendingWrite {
                        id: request["id"].as_u64()?,
                        source: request["source"].as_str()?.to_string(),
                        target: SessionId::parse(request["target_session"].as_str()?).ok()?,
                        asked_at: Instant::now()
                            .checked_sub(waited)
                            .unwrap_or_else(Instant::now),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn publish(
    tx: &SyncSender<Vec<PendingWrite>>,
    wake: &crate::app::wake::AppWake,
    pending: Vec<PendingWrite>,
) -> bool {
    let sent = tx.try_send(pending);
    wake.notify();
    !matches!(sent, Err(TrySendError::Disconnected(_)))
}

fn spawn_follow_thread(wake: crate::app::wake::AppWake) -> Option<Receiver<Vec<PendingWrite>>> {
    let target = crate::terminal::host_link::host_endpoint()?;
    let (tx, rx) = sync_channel(SNAPSHOT_QUEUE_SLOTS);
    let spawned = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || {
            let hello = paneflow_host::protocol::ClientHello::local(CLIENT_NAME);
            let mut shown_any = false;
            loop {
                let mut desktop_gone = false;
                if let Ok(mut client) =
                    paneflow_host::client::HostClient::connect(&target.endpoint, &hello)
                {
                    let followed = client.follow_approvals(|snapshot| {
                        let pending = pending_writes(snapshot);
                        shown_any = !pending.is_empty();
                        desktop_gone = !publish(&tx, &wake, pending);
                        !desktop_gone
                    });
                    if let Err(error) = followed {
                        log::debug!("paneflow: the write approval stream ended: {error}");
                    }
                }
                if desktop_gone {
                    return;
                }
                if shown_any {
                    shown_any = false;
                    if !publish(&tx, &wake, Vec::new()) {
                        return;
                    }
                }
                std::thread::sleep(RECONNECT_DELAY);
            }
        });
    match spawned {
        Ok(_) => Some(rx),
        Err(error) => {
            log::warn!("paneflow: cannot start the write approval thread: {error}");
            None
        }
    }
}

pub(crate) fn decide(id: u64, allow: bool) {
    let Some(target) = crate::terminal::host_link::host_endpoint() else {
        return;
    };
    let spawned = std::thread::Builder::new()
        .name(DECISION_THREAD_NAME.into())
        .spawn(move || {
            let decided = crate::terminal::host_link::connect(&target.endpoint)
                .and_then(|mut client| client.decide_approval(id, allow));
            if let Err(error) = decided {
                log::warn!("paneflow: the host did not record write decision {id}: {error}");
            }
        });
    if let Err(error) = spawned {
        log::warn!("paneflow: cannot send write decision {id} to the host: {error}");
    }
}

impl PaneFlowApp {
    pub(crate) fn start_write_approval_stream(&mut self) {
        if self.write_approvals.snapshots.is_none() {
            self.write_approvals.snapshots = spawn_follow_thread(self.app_wake.clone());
        }
    }

    pub(crate) fn process_write_approval_frames(&mut self, cx: &mut Context<Self>) {
        let Some(snapshots) = self.write_approvals.snapshots.as_ref() else {
            return;
        };
        let mut latest = None;
        while let Ok(snapshot) = snapshots.try_recv() {
            latest = Some(snapshot);
        }
        let Some(pending) = latest else {
            return;
        };
        self.write_approvals.pending = pending;
        self.show_write_requests(cx);
        cx.notify();
    }

    pub(crate) fn show_write_requests(&mut self, cx: &mut Context<Self>) {
        let terminals: Vec<_> = crate::workspace::panes_across(&self.workspaces)
            .iter()
            .flat_map(|pane| pane.read(cx).terminals().cloned().collect::<Vec<_>>())
            .collect();
        for terminal in terminals {
            let session = terminal.read(cx).terminal.session_id.clone();
            let request = self
                .write_approvals
                .pending
                .iter()
                .find(|pending| pending.target == session)
                .map(PendingWrite::request);
            terminal.update(cx, |view, cx| view.show_write_request(request, cx));
        }
    }

    pub(crate) fn decide_write_request(&mut self, id: u64, allow: bool, cx: &mut Context<Self>) {
        self.write_approvals
            .pending
            .retain(|pending| pending.id != id);
        self.show_write_requests(cx);
        decide(id, allow);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_host_snapshot_names_each_request_its_source_and_its_target_pane() {
        let target = SessionId::new();
        let snapshot = json!({"type": "approvals", "pending": [
            {"id": 4, "source": "conductor", "source_session": SessionId::new(), "target_session": target, "expires_in_ms": 1000},
            {"id": 5, "source": "broken"}
        ]});
        let pending = pending_writes(&snapshot);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, 4);
        assert_eq!(pending[0].source, "conductor");
        assert_eq!(pending[0].target, target);
        assert!(pending[0].asked_at.elapsed() >= Duration::from_secs(119));
        assert_eq!(
            pending[0].request().message(),
            "conductor wants to write into this pane"
        );
        assert!(pending_writes(&json!({"type": "keepalive"})).is_empty());
    }
}
