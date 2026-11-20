use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use paneflow_api::methods::events::{EventIds, PublishedEvent};
use paneflow_layout::{Event, EventScope, State};
use paneflow_serve::controller::{Controller, ControllerError};
use serde_json::{Map, json};

pub(crate) const MAX_UNSENT_LAYOUT_EVENTS: usize = 1024;
pub(crate) const LAYOUT_RESYNC: &str = "layout.resync";
const MAX_BATCH_BYTES: usize = 32 * 1024;
const RETRY_DELAY: Duration = Duration::from_millis(500);
const OLDER_WORKER_RETRY: Duration = Duration::from_secs(5);

pub(crate) fn published_events(
    events: &[Event],
    scopes: &[EventScope],
    revision: u64,
) -> Vec<PublishedEvent> {
    events
        .iter()
        .zip(scopes)
        .map(|(event, scope)| {
            let mut data = Map::new();
            if matches!(event, Event::LayoutChanged { .. }) {
                data.insert("revision".to_string(), json!(revision));
            }
            PublishedEvent {
                kind: event.kind().to_string(),
                ids: EventIds {
                    workspace_id: scope.workspace.map(|id| id.get()),
                    tab_id: scope.tab.map(|id| id.get()),
                    pane_id: scope.pane.map(|id| id.get()),
                    surface_id: scope.surface.map(|id| id.get()),
                    session: scope.session.as_ref().map(ToString::to_string),
                },
                data,
            }
        })
        .collect()
}

pub(crate) fn scoped_events(events: &[Event], before: &State, after: &State) -> Vec<EventScope> {
    events
        .iter()
        .map(|event| event.scope(before, after))
        .collect()
}

fn resync_event() -> PublishedEvent {
    PublishedEvent {
        kind: LAYOUT_RESYNC.to_string(),
        ids: EventIds::default(),
        data: Map::new(),
    }
}

#[derive(Default)]
struct Outbox {
    queue: VecDeque<PublishedEvent>,
    generation: u64,
    closed: bool,
}

impl Outbox {
    fn push(&mut self, event: PublishedEvent) {
        if self.queue.len() >= MAX_UNSENT_LAYOUT_EVENTS {
            self.queue.clear();
            self.queue.push_back(resync_event());
            self.generation += 1;
        }
        self.queue.push_back(event);
    }

    fn batch(&self) -> (u64, Vec<PublishedEvent>) {
        let mut size = 0usize;
        let mut batch = Vec::new();
        for event in &self.queue {
            let bytes = serde_json::to_vec(event).map_or(0, |encoded| encoded.len());
            if !batch.is_empty() && size + bytes > MAX_BATCH_BYTES {
                break;
            }
            size += bytes;
            batch.push(event.clone());
        }
        (self.generation, batch)
    }

    fn delivered(&mut self, generation: u64, count: usize) {
        if generation == self.generation {
            self.queue.drain(..count.min(self.queue.len()));
        }
    }
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
}

#[derive(Default)]
pub(crate) struct LayoutEventSink {
    shared: Arc<Shared>,
    started: bool,
}

impl LayoutEventSink {
    pub(crate) fn publish(&self, events: Vec<PublishedEvent>) {
        if events.is_empty() {
            return;
        }
        let mut outbox = self.shared.lock();
        for event in events {
            outbox.push(event);
        }
        drop(outbox);
        self.shared.ready.notify_all();
    }

    pub(crate) fn start(&mut self, endpoint: PathBuf) {
        if self.started {
            return;
        }
        let shared = Arc::clone(&self.shared);
        let spawned = std::thread::Builder::new()
            .name("paneflow-layout-events".into())
            .spawn(move || deliver(&shared, &endpoint));
        match spawned {
            Ok(_) => self.started = true,
            Err(error) => log::warn!("paneflow: cannot start the layout event thread: {error}"),
        }
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> Vec<PublishedEvent> {
        self.shared.lock().queue.iter().cloned().collect()
    }
}

impl Drop for LayoutEventSink {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.ready.notify_all();
    }
}

fn deliver(shared: &Shared, endpoint: &std::path::Path) {
    let mut controller: Option<Controller> = None;
    loop {
        let (generation, batch) = {
            let outbox = shared.lock();
            let outbox = shared
                .ready
                .wait_while(outbox, |outbox| outbox.queue.is_empty() && !outbox.closed)
                .unwrap_or_else(PoisonError::into_inner);
            if outbox.closed {
                return;
            }
            outbox.batch()
        };
        if controller.is_none() {
            match Controller::connect(endpoint) {
                Ok(connected) => controller = Some(connected),
                Err(error) => {
                    log::debug!("paneflow: layout events wait for the worker: {error}");
                    if !shared.pause(RETRY_DELAY) {
                        return;
                    }
                    continue;
                }
            }
        }
        let Some(connected) = controller.as_mut() else {
            continue;
        };
        match connected.publish_events(&batch) {
            Ok(_) => shared.lock().delivered(generation, batch.len()),
            Err(error) => {
                let delay = match error {
                    ControllerError::CapabilityNotAdvertised(_) => OLDER_WORKER_RETRY,
                    _ => RETRY_DELAY,
                };
                log::debug!("paneflow: the worker did not take layout events: {error}");
                controller = None;
                if !shared.pause(delay) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: &str, pane: u64) -> PublishedEvent {
        PublishedEvent {
            kind: kind.to_string(),
            ids: EventIds {
                pane_id: Some(pane),
                ..EventIds::default()
            },
            data: Map::new(),
        }
    }

    #[test]
    fn an_overflowing_outbox_replaces_the_lost_events_with_one_resync_and_keeps_order() {
        let mut outbox = Outbox::default();
        for pane in 0..MAX_UNSENT_LAYOUT_EVENTS as u64 {
            outbox.push(event("pane.created", pane));
        }
        assert_eq!(outbox.queue.len(), MAX_UNSENT_LAYOUT_EVENTS);
        outbox.push(event("pane.closed", 9_999));
        let kinds: Vec<&str> = outbox
            .queue
            .iter()
            .map(|event| event.kind.as_str())
            .collect();
        assert_eq!(kinds, [LAYOUT_RESYNC, "pane.closed"]);
        outbox.push(event("pane.focused", 1));
        assert_eq!(outbox.queue.len(), 3);
    }

    #[test]
    fn a_batch_in_flight_when_the_outbox_overflowed_does_not_drop_the_resync() {
        let mut outbox = Outbox::default();
        outbox.push(event("pane.created", 1));
        let (generation, batch) = outbox.batch();
        for pane in 0..MAX_UNSENT_LAYOUT_EVENTS as u64 {
            outbox.push(event("pane.created", pane));
        }
        outbox.delivered(generation, batch.len());
        assert_eq!(outbox.queue[0].kind, LAYOUT_RESYNC);
    }

    #[test]
    fn a_batch_stays_under_the_control_frame_limit_and_is_never_empty() {
        let mut outbox = Outbox::default();
        let mut wide = event("surface.renamed", 1);
        wide.data
            .insert("pad".to_string(), json!("x".repeat(MAX_BATCH_BYTES)));
        outbox.push(wide);
        for pane in 0..500 {
            outbox.push(event("pane.created", pane));
        }
        let (_, first) = outbox.batch();
        assert_eq!(first.len(), 1, "an oversized event still leaves alone");
        outbox.delivered(outbox.generation, 1);
        let (_, next) = outbox.batch();
        let size = serde_json::to_vec(&next).unwrap().len();
        assert!(size <= MAX_BATCH_BYTES + next.len(), "{size}");
        assert!(next.len() > 1);
    }

    fn followed(endpoint: &std::path::Path, count: usize) -> Vec<serde_json::Value> {
        let mut stream = paneflow_serve::controller::EventStream::open(
            endpoint,
            paneflow_serve::controller::EventRequest {
                since: Some(0),
                ..Default::default()
            },
        )
        .expect("the worker streams its log");
        let mut events = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while events.len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "only {} of {count} events arrived: {events:?}",
                events.len()
            );
            if let paneflow_serve::controller::EventFrame::Event(event) =
                stream.next(Duration::from_millis(200))
            {
                events.push(*event);
            }
        }
        events
    }

    #[test]
    fn events_published_while_the_worker_is_away_reach_it_in_order_once_it_starts() {
        let home = tempfile::tempdir().expect("a home");
        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        let mut sink = LayoutEventSink::default();
        sink.start(endpoint.clone());
        sink.publish(vec![
            event("pane.created", 1),
            event("pane.focused", 1),
            event("pane.closed", 1),
        ]);
        let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
        let kinds: Vec<String> = followed(&endpoint, 3)
            .iter()
            .map(|event| event["type"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(kinds, ["pane.created", "pane.focused", "pane.closed"]);
        drop(sink);
        worker.stop();
    }

    #[test]
    fn an_outbox_that_overflowed_while_the_worker_was_away_sends_one_resync_first() {
        let home = tempfile::tempdir().expect("a home");
        let endpoint = paneflow_home::serve_endpoint_path(home.path());
        let mut sink = LayoutEventSink::default();
        sink.start(endpoint.clone());
        sink.publish(
            (0..MAX_UNSENT_LAYOUT_EVENTS as u64 + 3)
                .map(|pane| event("pane.created", pane))
                .collect(),
        );
        let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
        let events = followed(&endpoint, 4);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| event["type"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            kinds,
            [
                LAYOUT_RESYNC,
                "pane.created",
                "pane.created",
                "pane.created"
            ]
        );
        let panes: Vec<u64> = events[1..]
            .iter()
            .filter_map(|event| event["pane_id"].as_u64())
            .collect();
        let last = MAX_UNSENT_LAYOUT_EVENTS as u64;
        assert_eq!(panes, [last, last + 1, last + 2]);
        drop(sink);
        worker.stop();
    }
}
