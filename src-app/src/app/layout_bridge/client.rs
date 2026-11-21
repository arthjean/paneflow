use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use paneflow_layout::{IdBlock, Snapshot};
use serde_json::{Value, json};

use crate::app::wake::AppWake;
use crate::terminal::host_link::{LaunchRoute, WorkerLaunchAnswer, WorkerLauncher};

pub(crate) const WINDOW_CLIENT: &str = "window";
const CONTROL_CLIENT: &str = "paneflow-window-layout";
const RETRY: Duration = Duration::from_millis(500);
const OLDER_WORKER_RETRY: Duration = Duration::from_secs(5);
const CALL_DEADLINE: Duration = Duration::from_secs(10);
const EVENT_SLICE: Duration = Duration::from_millis(500);
const PROBE_EVERY: Duration = Duration::from_millis(250);
const PROBE_READ: Duration = Duration::from_millis(1);
const KEEPALIVE: Duration = Duration::from_secs(2);
const LAUNCH_WAIT: Duration = Duration::from_secs(30);
const LAYOUT_EVENTS: [&str; 15] = [
    "workspace.created",
    "workspace.closed",
    "workspace.renamed",
    "workspace.selected",
    "tab.created",
    "tab.closed",
    "tab.renamed",
    "tab.selected",
    "pane.created",
    "pane.closed",
    "pane.focused",
    "surface.created",
    "surface.closed",
    "surface.renamed",
    "layout.changed",
];
pub(crate) const LEASE_REFILL_BELOW: u64 = paneflow_layout::LEASE_BLOCK / 2;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Request {
    Adopt {
        view: Value,
        acknowledged: u64,
        reconnect: bool,
    },
    Op {
        op: Value,
        ids_from: u64,
        expected: u64,
    },
    Lease,
    View {
        unless_revision: Option<u64>,
    },
    Launch {
        params: Value,
        reply: LaunchReply,
    },
    Sweep {
        params: Value,
        reply: SweepReply,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct SweepReply(std::sync::mpsc::SyncSender<Option<Value>>);

impl PartialEq for SweepReply {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LaunchReply(std::sync::mpsc::SyncSender<WorkerLaunchAnswer>);

impl PartialEq for LaunchReply {
    fn eq(&self, _: &Self) -> bool {
        false
    }
}

impl LaunchReply {
    fn answer(self, answer: WorkerLaunchAnswer) {
        let _ = self.0.send(answer);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Outgoing {
    pub(crate) generation: u64,
    pub(crate) seq: u64,
    pub(crate) request: Request,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Incoming {
    Unavailable,
    WithoutLayout,
    Attached {
        generation: u64,
        epoch: String,
    },
    Adopted {
        seq: u64,
        accepted: bool,
        revision: u64,
        view: Option<Box<Snapshot>>,
        reconnect: bool,
    },
    Acked {
        seq: u64,
        revision: u64,
        expected: u64,
    },
    Refused {
        seq: u64,
        reason: String,
        op: Value,
    },
    Leased {
        seq: u64,
        block: IdBlock,
    },
    Viewed {
        seq: u64,
        revision: u64,
        view: Option<Box<Snapshot>>,
    },
    Changed {
        revision: Option<u64>,
    },
    Disconnected,
}

#[derive(Default)]
struct Outbox {
    queue: VecDeque<Outgoing>,
    generation: u64,
    closed: bool,
    attached: bool,
}

#[derive(Default)]
struct Shared {
    outbox: Mutex<Outbox>,
    ready: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Outbox> {
        self.outbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn pause(&self, delay: Duration) -> bool {
        let outbox = self.lock();
        if outbox.closed {
            return false;
        }
        let (outbox, _) = self
            .ready
            .wait_timeout_while(outbox, delay, |outbox| !outbox.closed)
            .unwrap_or_else(PoisonError::into_inner);
        !outbox.closed
    }

    fn closed(&self) -> bool {
        self.lock().closed
    }
}

struct Inbox {
    tx: Sender<Incoming>,
    wake: Option<AppWake>,
}

impl Inbox {
    fn send(&self, incoming: Incoming) -> bool {
        let sent = self.tx.send(incoming).is_ok();
        if let Some(wake) = &self.wake {
            wake.notify();
        }
        sent
    }
}

pub(crate) struct LayoutLink {
    shared: Arc<Shared>,
    incoming: Receiver<Incoming>,
}

impl LayoutLink {
    pub(crate) fn start(endpoint: PathBuf, wake: Option<AppWake>) -> Option<Self> {
        let shared = Arc::new(Shared::default());
        let (tx, incoming) = channel();
        let requests = Inbox {
            tx: tx.clone(),
            wake: wake.clone(),
        };
        let events = Inbox { tx, wake };
        let request_shared = Arc::clone(&shared);
        let request_endpoint = endpoint.clone();
        let spawned = std::thread::Builder::new()
            .name("paneflow-layout-client".into())
            .spawn(move || serve(&request_shared, &request_endpoint, &requests));
        if let Err(error) = spawned {
            log::warn!("paneflow: cannot start the layout client thread: {error}");
            return None;
        }
        let event_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("paneflow-layout-changes".into())
            .spawn(move || follow_changes(&event_shared, &endpoint, &events));
        if let Err(error) = spawned {
            log::warn!("paneflow: cannot start the layout change thread: {error}");
        }
        Some(Self { shared, incoming })
    }

    pub(crate) fn send(&self, outgoing: Outgoing) {
        self.shared.lock().queue.push_back(outgoing);
        self.shared.ready.notify_all();
    }

    pub(crate) fn try_recv(&self) -> Option<Incoming> {
        self.incoming.try_recv().ok()
    }

    pub(crate) fn launch_route(&self) -> LaunchRoute {
        Arc::new(ClientLauncher {
            shared: Arc::clone(&self.shared),
        })
    }

    pub(crate) fn sweeper(&self) -> Sweeper {
        Sweeper {
            shared: Arc::clone(&self.shared),
        }
    }
}

pub(crate) struct Sweeper {
    shared: Arc<Shared>,
}

impl Sweeper {
    pub(crate) fn sweep(&self, params: Value) -> Option<Value> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        {
            let mut outbox = self.shared.lock();
            if outbox.closed || !outbox.attached {
                return None;
            }
            let generation = outbox.generation;
            outbox.queue.push_back(Outgoing {
                generation,
                seq: 0,
                request: Request::Sweep {
                    params,
                    reply: SweepReply(tx),
                },
            });
        }
        self.shared.ready.notify_all();
        rx.recv_timeout(LAUNCH_WAIT).ok().flatten()
    }
}

struct ClientLauncher {
    shared: Arc<Shared>,
}

impl WorkerLauncher for ClientLauncher {
    fn launch(&self, params: Value) -> WorkerLaunchAnswer {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        {
            let mut outbox = self.shared.lock();
            if outbox.closed || !outbox.attached {
                return WorkerLaunchAnswer::Unavailable;
            }
            let generation = outbox.generation;
            outbox.queue.push_back(Outgoing {
                generation,
                seq: 0,
                request: Request::Launch {
                    params,
                    reply: LaunchReply(tx),
                },
            });
        }
        self.shared.ready.notify_all();
        rx.recv_timeout(LAUNCH_WAIT)
            .unwrap_or(WorkerLaunchAnswer::Unknown)
    }
}

fn launch_answer(answer: Result<Answer, String>) -> (WorkerLaunchAnswer, bool) {
    match answer {
        Ok(Answer::Result(_)) => (WorkerLaunchAnswer::Launched, true),
        Ok(Answer::Error(error)) => {
            let reason = error["data"]["reason"].as_str().unwrap_or_default();
            let fatal = matches!(
                reason,
                paneflow_api::methods::layout::LAUNCH_FAILED
                    | paneflow_api::methods::layout::HOST_UNREACHABLE
            );
            let answer = if fatal {
                WorkerLaunchAnswer::Failed(
                    error["message"]
                        .as_str()
                        .unwrap_or("the worker could not start the terminal")
                        .to_string(),
                )
            } else {
                WorkerLaunchAnswer::Unavailable
            };
            (answer, true)
        }
        Err(error) => {
            log::debug!("paneflow: layout.launch failed: {error}");
            (WorkerLaunchAnswer::Unknown, false)
        }
    }
}

impl Drop for LayoutLink {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.ready.notify_all();
    }
}

enum Answer {
    Result(Value),
    Error(Value),
}

fn call(control: &mut HostControl, method: &str, params: Value) -> Result<Answer, String> {
    let id = control
        .write_request(method, params)
        .map_err(|error| format!("{method}: {error}"))?;
    let deadline = Instant::now() + CALL_DEADLINE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(format!("{method} was not answered in time"));
        }
        let line = control
            .read_stream_line(left)
            .map_err(|error| format!("{method}: {error}"))?
            .ok_or_else(|| format!("the worker closed the connection during {method}"))?;
        let Ok(envelope) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if envelope["id"].as_u64() != Some(id) {
            continue;
        }
        if envelope["error"].is_object() {
            return Ok(Answer::Error(envelope["error"].clone()));
        }
        return Ok(Answer::Result(envelope["result"].clone()));
    }
}

fn advertises_layout(identity: &Value) -> bool {
    identity["capabilities"]
        .as_array()
        .is_some_and(|capabilities| {
            capabilities.iter().any(|capability| {
                capability.as_str() == Some(paneflow_serve::protocol::CAPABILITY_LAYOUT_AUTHORITY)
            })
        })
}

fn serve(shared: &Shared, endpoint: &Path, inbox: &Inbox) {
    let mut was_connected = true;
    loop {
        if shared.closed() {
            return;
        }
        let mut control = match HostControl::open(endpoint, CONTROL_CLIENT, CALL_DEADLINE) {
            Ok(control) => control,
            Err(_) => {
                if was_connected {
                    was_connected = false;
                    if !inbox.send(Incoming::Unavailable) {
                        return;
                    }
                }
                if !shared.pause(RETRY) {
                    return;
                }
                continue;
            }
        };
        if !advertises_layout(control.identity()) {
            was_connected = false;
            if !inbox.send(Incoming::WithoutLayout) {
                return;
            }
            if !shared.pause(OLDER_WORKER_RETRY) {
                return;
            }
            continue;
        }
        control.set_max_frame(paneflow_serve::layout_authority::MAX_LAYOUT_FRAME_BYTES);
        let attached = match call(
            &mut control,
            "layout.attach",
            json!({"client": WINDOW_CLIENT}),
        ) {
            Ok(Answer::Result(attached)) => attached,
            Ok(Answer::Error(error)) => {
                log::warn!("paneflow: the worker refused layout.attach: {error}");
                if !shared.pause(OLDER_WORKER_RETRY) {
                    return;
                }
                continue;
            }
            Err(error) => {
                log::debug!("paneflow: layout.attach failed: {error}");
                if !shared.pause(RETRY) {
                    return;
                }
                continue;
            }
        };
        let generation = {
            let mut outbox = shared.lock();
            outbox.generation += 1;
            outbox.queue.clear();
            outbox.attached = true;
            outbox.generation
        };
        was_connected = true;
        let epoch = attached["epoch"].as_str().unwrap_or_default().to_string();
        if !inbox.send(Incoming::Attached { generation, epoch }) {
            return;
        }
        let open = drain(shared, &mut control, generation, inbox);
        {
            let mut outbox = shared.lock();
            outbox.attached = false;
            outbox.queue.clear();
        }
        if !open {
            return;
        }
        if !inbox.send(Incoming::Disconnected) {
            return;
        }
        if !shared.pause(RETRY) {
            return;
        }
    }
}

enum Next {
    Send(Outgoing),
    Probe,
    Closed,
}

fn next_outgoing(shared: &Shared, generation: u64) -> Next {
    let mut outbox = shared.lock();
    loop {
        if outbox.closed {
            return Next::Closed;
        }
        while let Some(front) = outbox.queue.pop_front() {
            if front.generation == generation {
                return Next::Send(front);
            }
        }
        let (next, waited) = shared
            .ready
            .wait_timeout(outbox, PROBE_EVERY)
            .unwrap_or_else(PoisonError::into_inner);
        outbox = next;
        if waited.timed_out() && outbox.queue.is_empty() && !outbox.closed {
            return Next::Probe;
        }
    }
}

fn still_connected(control: &mut HostControl) -> bool {
    match control.read_stream_line(PROBE_READ) {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(error) => error.kind() == std::io::ErrorKind::TimedOut,
    }
}

fn snapshot_of(value: &Value) -> Option<Box<Snapshot>> {
    serde_json::from_value::<Snapshot>(value.clone())
        .ok()
        .map(Box::new)
}

fn drain(shared: &Shared, control: &mut HostControl, generation: u64, inbox: &Inbox) -> bool {
    let mut answered = Instant::now();
    loop {
        let outgoing = match next_outgoing(shared, generation) {
            Next::Send(outgoing) => outgoing,
            Next::Probe => {
                if !still_connected(control) {
                    return true;
                }
                if answered.elapsed() >= KEEPALIVE {
                    if call(
                        control,
                        paneflow_serve::protocol::METHOD_WORKER_HELLO,
                        json!({}),
                    )
                    .is_err()
                    {
                        return true;
                    }
                    answered = Instant::now();
                }
                continue;
            }
            Next::Closed => return false,
        };
        answered = Instant::now();
        let seq = outgoing.seq;
        let incoming = match outgoing.request {
            Request::Adopt {
                view,
                acknowledged,
                reconnect,
            } => match call(
                control,
                "layout.adopt",
                json!({"view": view, "acknowledged": acknowledged}),
            ) {
                Ok(Answer::Result(adopted)) => Incoming::Adopted {
                    seq,
                    accepted: adopted["accepted"].as_bool().unwrap_or(false),
                    revision: adopted["revision"].as_u64().unwrap_or_default(),
                    view: snapshot_of(&adopted["view"]),
                    reconnect,
                },
                Ok(Answer::Error(error)) => {
                    log::warn!("paneflow: the worker refused layout.adopt: {error}");
                    return true;
                }
                Err(error) => {
                    log::debug!("paneflow: layout.adopt failed: {error}");
                    return true;
                }
            },
            Request::Op {
                op,
                ids_from,
                expected,
            } => match call(
                control,
                "layout.apply",
                json!({"op": op, "ids_from": ids_from}),
            ) {
                Ok(Answer::Result(applied)) => Incoming::Acked {
                    seq,
                    revision: applied["revision"].as_u64().unwrap_or_default(),
                    expected,
                },
                Ok(Answer::Error(error)) => Incoming::Refused {
                    seq,
                    reason: error["data"]["reason"]
                        .as_str()
                        .unwrap_or("invalid")
                        .to_string(),
                    op,
                },
                Err(error) => {
                    log::debug!("paneflow: layout.apply failed: {error}");
                    return true;
                }
            },
            Request::Lease => match call(control, "layout.lease_ids", json!({})) {
                Ok(Answer::Result(lease)) => Incoming::Leased {
                    seq,
                    block: IdBlock {
                        start: lease["start"].as_u64().unwrap_or_default(),
                        end: lease["end"].as_u64().unwrap_or_default(),
                    },
                },
                Ok(Answer::Error(error)) => {
                    log::warn!("paneflow: the worker refused an id lease: {error}");
                    return true;
                }
                Err(error) => {
                    log::debug!("paneflow: layout.lease_ids failed: {error}");
                    return true;
                }
            },
            Request::View { unless_revision } => match call(
                control,
                "layout.view",
                json!({"unless_revision": unless_revision}),
            ) {
                Ok(Answer::Result(view)) => {
                    let snapshot = snapshot_of(&view["view"]);
                    if snapshot.is_none() && !view["view"].is_null() {
                        return true;
                    }
                    Incoming::Viewed {
                        seq,
                        revision: view["revision"].as_u64().unwrap_or_default(),
                        view: snapshot,
                    }
                }
                Ok(Answer::Error(_)) | Err(_) => return true,
            },
            Request::Launch { params, reply } => {
                let (answer, connected) = launch_answer(call(control, "layout.launch", params));
                reply.answer(answer);
                if !connected {
                    return true;
                }
                continue;
            }
            Request::Sweep { params, reply } => {
                let (swept, connected) = match call(control, "layout.sweep", params) {
                    Ok(Answer::Result(swept)) => (Some(swept), true),
                    Ok(Answer::Error(error)) => {
                        log::debug!("paneflow: layout.sweep was refused: {error}");
                        (None, true)
                    }
                    Err(error) => {
                        log::debug!("paneflow: layout.sweep failed: {error}");
                        (None, false)
                    }
                };
                let _ = reply.0.send(swept);
                if !connected {
                    return true;
                }
                continue;
            }
        };
        if !inbox.send(incoming) {
            return false;
        }
    }
}

fn follow_changes(shared: &Shared, endpoint: &Path, inbox: &Inbox) {
    let request = paneflow_serve::controller::EventRequest {
        types: LAYOUT_EVENTS.iter().map(ToString::to_string).collect(),
        ..Default::default()
    };
    let mut stream = None;
    loop {
        if shared.closed() {
            return;
        }
        let Some(open) = stream.as_mut() else {
            match paneflow_serve::controller::EventStream::open(endpoint, request.clone()) {
                Ok(opened) => stream = Some(opened),
                Err(_) => {
                    if !shared.pause(RETRY) {
                        return;
                    }
                }
            }
            continue;
        };
        match open.next(EVENT_SLICE) {
            paneflow_serve::controller::EventFrame::Event(event) => {
                let revision = event["data"]["revision"].as_u64();
                if !inbox.send(Incoming::Changed { revision }) {
                    return;
                }
            }
            paneflow_serve::controller::EventFrame::Disconnected(_) => stream = None,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_worker_that_advertises_layout_authority_is_attached_to() {
        let current = json!({"capabilities": ["events.follow", "layout.authority"]});
        let older = json!({"capabilities": ["events.follow", "prompt.follow"]});
        assert!(advertises_layout(&current));
        assert!(!advertises_layout(&older));
        assert!(!advertises_layout(&json!({})));
    }
}
