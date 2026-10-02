use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use serde_json::{Value, json};

const CLIENT_NAME: &str = "paneflow-serve";
const FRAME_QUEUE_SLOTS: usize = 1024;
const RECONNECT_DELAY: Duration = Duration::from_millis(250);
pub const REFUSED_RETRY: Duration = Duration::from_secs(30);
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreFrame {
    Snapshot(Vec<Value>),
    Session(Box<Value>),
    SessionRemoved(String),
    Event(Box<Value>),
    Cancellation(Box<Value>),
    Disconnected(String),
    Refused(String),
}

enum FollowEnd {
    Dropped(String),
    Refused(String),
}

pub struct CoreLink {
    frames: Receiver<CoreFrame>,
}

impl CoreLink {
    pub fn follow(home: &Path) -> std::io::Result<Self> {
        let endpoint = paneflow_home::host_endpoint_path(home);
        Self::spawn(move |tx| follow_once(&endpoint, tx))
    }

    fn spawn(
        mut follow: impl FnMut(&SyncSender<CoreFrame>) -> FollowEnd + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, frames) = sync_channel(FRAME_QUEUE_SLOTS);
        std::thread::Builder::new()
            .name("paneflow-serve-core".into())
            .spawn(move || {
                loop {
                    let (frame, delay) = match follow(&tx) {
                        FollowEnd::Dropped(reason) => {
                            (CoreFrame::Disconnected(reason), RECONNECT_DELAY)
                        }
                        FollowEnd::Refused(reason) => (CoreFrame::Refused(reason), REFUSED_RETRY),
                    };
                    if tx.send(frame).is_err() {
                        return;
                    }
                    std::thread::sleep(delay);
                }
            })?;
        Ok(Self { frames })
    }

    pub fn drain(&self, max: usize) -> Vec<CoreFrame> {
        let mut pending = Vec::new();
        while pending.len() < max {
            match self.frames.try_recv() {
                Ok(frame) => pending.push(frame),
                Err(_) => break,
            }
        }
        pending
    }

    pub fn wait(&self, timeout: Duration) -> Option<CoreFrame> {
        self.frames.recv_timeout(timeout).ok()
    }
}

pub fn call_core(endpoint: &Path, method: &str, params: &Value) -> Result<Value, String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    control.request(method, params.clone())
}

pub fn menu_prompt_active(
    endpoint: &Path,
    session: &paneflow_config::schema::SessionId,
    deadline: Duration,
) -> Option<bool> {
    let mut control = HostControl::connect_with_deadline(endpoint, CLIENT_NAME, deadline).ok()?;
    let snapshot = control
        .request_with_deadline(
            paneflow_ipc_client::host_control::METHOD_AGENT_SNAPSHOT,
            json!({}),
            deadline,
        )
        .ok()?;
    snapshot["sessions"]
        .as_array()?
        .iter()
        .find(|row| row["session"].as_str() == Some(session.as_str()))
        .map(|row| row["menu_prompt_active"].as_bool().unwrap_or(false))
}

fn follow_once(endpoint: &Path, tx: &SyncSender<CoreFrame>) -> FollowEnd {
    let mut control = match HostControl::connect(endpoint, CLIENT_NAME) {
        Ok(control) => control,
        Err(reason) => return FollowEnd::Dropped(reason),
    };
    let header = match control.request(METHOD_AGENT_FOLLOW, json!({})) {
        Ok(header) => header,
        Err(reason) => return FollowEnd::Refused(reason),
    };
    let sessions = header["sessions"].as_array().cloned().unwrap_or_default();
    match send(tx, CoreFrame::Snapshot(sessions)).and_then(|()| stream_frames(&mut control, tx)) {
        Ok(()) => FollowEnd::Dropped("the core ended the agent stream".to_string()),
        Err(reason) => FollowEnd::Dropped(reason),
    }
}

fn stream_frames(control: &mut HostControl, tx: &SyncSender<CoreFrame>) -> Result<(), String> {
    loop {
        let line = control
            .read_stream_line(STREAM_READ_TIMEOUT)
            .map_err(|error| error.to_string())?;
        let Some(line) = line else {
            return Err("the core closed the agent stream".to_string());
        };
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match value["type"].as_str() {
            Some("event") => send(tx, CoreFrame::Event(Box::new(value)))?,
            Some("session") => send(tx, CoreFrame::Session(Box::new(value["entry"].clone())))?,
            Some("session_removed") => {
                if let Some(session) = value["session"].as_str() {
                    send(tx, CoreFrame::SessionRemoved(session.to_string()))?;
                }
            }
            Some("cancellation") => send(tx, CoreFrame::Cancellation(Box::new(value)))?,
            Some("end") => {
                return Err(value["reason"]
                    .as_str()
                    .unwrap_or("the core ended the agent stream")
                    .to_string());
            }
            _ => {}
        }
    }
}

fn send(tx: &SyncSender<CoreFrame>, frame: CoreFrame) -> Result<(), String> {
    match tx.try_send(frame) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => {
            Err("the worker fell behind and needs a fresh snapshot".to_string())
        }
        Err(TrySendError::Disconnected(_)) => Err("the worker stopped reading".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refused_subscription_is_reported_and_retried_only_after_the_refusal_delay() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&attempts);
        let link = CoreLink::spawn(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            FollowEnd::Refused("unknown method agent.follow".to_string())
        })
        .unwrap();
        assert_eq!(
            link.wait(Duration::from_secs(5)),
            Some(CoreFrame::Refused(
                "unknown method agent.follow".to_string()
            ))
        );
        std::thread::sleep(RECONNECT_DELAY * 4);
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(REFUSED_RETRY, Duration::from_secs(30));
    }

    #[test]
    fn a_dropped_stream_resubscribes_within_a_second() {
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = std::sync::Arc::clone(&attempts);
        let link = CoreLink::spawn(move |_| {
            counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            FollowEnd::Dropped("the core closed the agent stream".to_string())
        })
        .unwrap();
        let started = std::time::Instant::now();
        while attempts.load(std::sync::atomic::Ordering::SeqCst) < 2 {
            assert!(link.wait(Duration::from_secs(1)).is_some());
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn a_full_worker_queue_reconnects_instead_of_silently_losing_an_accepted_event() {
        let (tx, rx) = sync_channel(1);
        send(&tx, CoreFrame::Snapshot(Vec::new())).unwrap();
        assert!(send(&tx, CoreFrame::Event(Box::new(json!({"revision": 2})))).is_err());
        assert!(matches!(rx.recv().unwrap(), CoreFrame::Snapshot(_)));
    }
}
