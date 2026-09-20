use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::time::Duration;

use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use serde_json::{Value, json};

const CLIENT_NAME: &str = "paneflow-serve";
const FRAME_QUEUE_SLOTS: usize = 1024;
const RECONNECT_DELAY: Duration = Duration::from_secs(1);
const STREAM_READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreFrame {
    Snapshot(Vec<Value>),
    Event(Box<Value>),
    Disconnected(String),
}

pub struct CoreLink {
    endpoint: PathBuf,
    frames: Receiver<CoreFrame>,
}

impl CoreLink {
    pub fn follow(home: &Path) -> std::io::Result<Self> {
        let endpoint = paneflow_home::host_endpoint_path(home);
        let (tx, frames) = sync_channel(FRAME_QUEUE_SLOTS);
        let followed = endpoint.clone();
        std::thread::Builder::new()
            .name("paneflow-serve-core".into())
            .spawn(move || {
                loop {
                    let reason = match follow_once(&followed, &tx) {
                        Ok(()) => "the core ended the agent stream".to_string(),
                        Err(reason) => reason,
                    };
                    if send(&tx, CoreFrame::Disconnected(reason)).is_err() {
                        return;
                    }
                    std::thread::sleep(RECONNECT_DELAY);
                }
            })?;
        Ok(Self { endpoint, frames })
    }

    pub fn endpoint(&self) -> &Path {
        &self.endpoint
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

fn follow_once(endpoint: &Path, tx: &SyncSender<CoreFrame>) -> Result<(), String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let header = control.request(METHOD_AGENT_FOLLOW, json!({}))?;
    let sessions = header["sessions"].as_array().cloned().unwrap_or_default();
    send(tx, CoreFrame::Snapshot(sessions))?;
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
        Err(TrySendError::Full(_)) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err("the worker stopped reading".to_string()),
    }
}
