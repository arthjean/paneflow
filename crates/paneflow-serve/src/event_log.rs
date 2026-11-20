use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use paneflow_api::methods::events::{Event, EventIds};
use serde_json::{Map, Value};

pub const RING_CAPACITY: usize = 1024;

pub const MAX_FOLLOWER_BACKLOG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Logged {
    pub seq: u64,
    pub kind: String,
    pub workspace_id: Option<u64>,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TypePattern {
    Exact(String),
    Family(String),
}

impl TypePattern {
    fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if let Some(family) = raw.strip_suffix(".*") {
            if family.is_empty() || family.contains('*') {
                return Err(format!(
                    "'{raw}' is not an event type or a family such as terminal.*"
                ));
            }
            return Ok(Self::Family(format!("{family}.")));
        }
        if raw.is_empty() || raw.contains('*') {
            return Err(format!(
                "'{raw}' is not an event type or a family such as terminal.*"
            ));
        }
        Ok(Self::Exact(raw.to_string()))
    }

    fn matches(&self, kind: &str) -> bool {
        match self {
            Self::Exact(name) => name == kind,
            Self::Family(prefix) => kind.starts_with(prefix.as_str()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    types: Vec<TypePattern>,
    workspace_id: Option<u64>,
}

impl Filter {
    pub fn new(types: Option<&[String]>, workspace_id: Option<u64>) -> Result<Self, String> {
        let types = types
            .unwrap_or_default()
            .iter()
            .flat_map(|entry| entry.split(','))
            .filter(|entry| !entry.trim().is_empty())
            .map(TypePattern::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            types,
            workspace_id,
        })
    }

    pub fn matches(&self, logged: &Logged) -> bool {
        let kind_matches = self.types.is_empty()
            || self
                .types
                .iter()
                .any(|pattern| pattern.matches(&logged.kind));
        let workspace_matches = self
            .workspace_id
            .is_none_or(|workspace| logged.workspace_id == Some(workspace));
        kind_matches && workspace_matches
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evicted {
    pub seq: u64,
    pub epoch: String,
}

pub enum Delivery {
    Event(Arc<Logged>),
    Quiet,
    Overflowed,
    Closed,
}

pub struct Subscription {
    pub id: u64,
    pub seq: u64,
    pub epoch: String,
    pub replay: Vec<Arc<Logged>>,
    live: Receiver<Arc<Logged>>,
    backlog: Arc<AtomicUsize>,
    overflowed: Arc<AtomicBool>,
}

impl Subscription {
    pub fn next(&self, timeout: Duration) -> Delivery {
        if self.overflowed.load(Ordering::Acquire) {
            return Delivery::Overflowed;
        }
        match self.live.recv_timeout(timeout) {
            Ok(_) if self.overflowed.load(Ordering::Acquire) => Delivery::Overflowed,
            Ok(logged) => {
                self.backlog.fetch_sub(logged.line.len(), Ordering::AcqRel);
                Delivery::Event(logged)
            }
            Err(RecvTimeoutError::Timeout) => Delivery::Quiet,
            Err(RecvTimeoutError::Disconnected) if self.overflowed.load(Ordering::Acquire) => {
                Delivery::Overflowed
            }
            Err(RecvTimeoutError::Disconnected) => Delivery::Closed,
        }
    }
}

struct Follower {
    id: u64,
    filter: Filter,
    tx: Sender<Arc<Logged>>,
    backlog: Arc<AtomicUsize>,
    overflowed: Arc<AtomicBool>,
}

#[derive(Default)]
struct Inner {
    last_seq: u64,
    ring: VecDeque<Arc<Logged>>,
    followers: Vec<Follower>,
    next_follower: u64,
    closed: bool,
}

pub struct EventLog {
    epoch: String,
    inner: Mutex<Inner>,
}

pub fn fresh_epoch(pid: u32, started_at_ms: u64) -> String {
    static STARTS: AtomicU64 = AtomicU64::new(0);
    let start = STARTS.fetch_add(1, Ordering::Relaxed);
    format!("{pid:x}-{started_at_ms:x}-{start:x}")
}

impl EventLog {
    pub fn new(epoch: impl Into<String>) -> Self {
        Self {
            epoch: epoch.into(),
            inner: Mutex::new(Inner::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn epoch(&self) -> &str {
        &self.epoch
    }

    pub fn last_seq(&self) -> u64 {
        self.lock().last_seq
    }

    pub fn ring_len(&self) -> usize {
        self.lock().ring.len()
    }

    pub fn follower_count(&self) -> usize {
        self.lock().followers.len()
    }

    pub fn append(&self, kind: &str, ids: EventIds, data: Map<String, Value>) -> u64 {
        let mut inner = self.lock();
        append_locked(&mut inner, kind, ids, data)
    }

    pub fn append_all(
        &self,
        events: impl IntoIterator<Item = (String, EventIds, Map<String, Value>)>,
    ) -> Vec<u64> {
        let mut inner = self.lock();
        events
            .into_iter()
            .map(|(kind, ids, data)| append_locked(&mut inner, &kind, ids, data))
            .collect()
    }

    pub fn follow(
        &self,
        since: Option<u64>,
        epoch: Option<&str>,
        filter: Filter,
    ) -> Result<Subscription, Evicted> {
        let mut inner = self.lock();
        let evicted = || Evicted {
            seq: inner.last_seq,
            epoch: self.epoch.clone(),
        };
        if epoch.is_some_and(|epoch| epoch != self.epoch) {
            return Err(evicted());
        }
        let replay = match since {
            None => Vec::new(),
            Some(since) => {
                let first = inner
                    .ring
                    .front()
                    .map_or(inner.last_seq.saturating_add(1), |logged| logged.seq);
                if since > inner.last_seq || since.saturating_add(1) < first {
                    return Err(evicted());
                }
                inner
                    .ring
                    .iter()
                    .filter(|logged| logged.seq > since && filter.matches(logged))
                    .cloned()
                    .collect()
            }
        };
        let (tx, live) = channel();
        let backlog = Arc::new(AtomicUsize::new(0));
        let overflowed = Arc::new(AtomicBool::new(false));
        inner.next_follower += 1;
        let id = inner.next_follower;
        if !inner.closed {
            inner.followers.push(Follower {
                id,
                filter,
                tx,
                backlog: Arc::clone(&backlog),
                overflowed: Arc::clone(&overflowed),
            });
        }
        Ok(Subscription {
            id,
            seq: inner.last_seq,
            epoch: self.epoch.clone(),
            replay,
            live,
            backlog,
            overflowed,
        })
    }

    pub fn unfollow(&self, id: u64) {
        self.lock().followers.retain(|follower| follower.id != id);
    }

    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        inner.followers.clear();
    }
}

fn append_locked(inner: &mut Inner, kind: &str, ids: EventIds, data: Map<String, Value>) -> u64 {
    inner.last_seq += 1;
    let seq = inner.last_seq;
    let workspace_id = ids.workspace_id;
    let event = Event {
        seq,
        ts: crate::state::now_ms(),
        kind: kind.to_string(),
        ids,
        data,
    };
    let line = serde_json::to_string(&event).unwrap_or_else(|error| {
        log::warn!("paneflow-serve: an event of type {kind} did not serialize: {error}");
        format!(
            "{{\"seq\":{seq},\"ts\":{},\"type\":\"{kind}\",\"data\":{{}}}}",
            event.ts
        )
    });
    let logged = Arc::new(Logged {
        seq,
        kind: kind.to_string(),
        workspace_id,
        line,
    });
    if inner.ring.len() >= RING_CAPACITY {
        inner.ring.pop_front();
    }
    inner.ring.push_back(Arc::clone(&logged));
    let size = logged.line.len();
    inner.followers.retain(|follower| {
        if !follower.filter.matches(&logged) {
            return true;
        }
        let held = follower.backlog.load(Ordering::Acquire);
        if held.saturating_add(size) > MAX_FOLLOWER_BACKLOG_BYTES {
            follower.overflowed.store(true, Ordering::Release);
            return false;
        }
        follower.backlog.fetch_add(size, Ordering::AcqRel);
        follower.tx.send(Arc::clone(&logged)).is_ok()
    });
    seq
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn session(id: &str) -> EventIds {
        EventIds {
            session: Some(id.to_string()),
            ..EventIds::default()
        }
    }

    fn data(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    fn drain(subscription: &Subscription) -> Vec<u64> {
        let mut seqs = Vec::new();
        while let Delivery::Event(logged) = subscription.next(Duration::from_millis(1)) {
            seqs.push(logged.seq);
        }
        seqs
    }

    #[test]
    fn an_appended_event_carries_the_next_seq_a_ts_its_type_ids_and_data() {
        let log = EventLog::new("e");
        assert_eq!(log.append("terminal.bell", session("s"), Map::new()), 1);
        let seq = log.append(
            "pane.created",
            EventIds {
                workspace_id: Some(1),
                tab_id: Some(2),
                pane_id: Some(3),
                ..EventIds::default()
            },
            data(json!({"origin": "split"})),
        );
        assert_eq!(seq, 2);
        let subscription = log.follow(Some(1), Some("e"), Filter::default()).unwrap();
        let event: Value = serde_json::from_str(&subscription.replay[0].line).unwrap();
        assert_eq!(event["seq"], 2);
        assert!(event["ts"].as_u64().is_some_and(|ts| ts > 0));
        assert_eq!(event["type"], "pane.created");
        assert_eq!(event["workspace_id"], 1);
        assert_eq!(event["tab_id"], 2);
        assert_eq!(event["pane_id"], 3);
        assert!(event.get("surface_id").is_none());
        assert_eq!(event["data"]["origin"], "split");
    }

    #[test]
    fn a_follower_with_since_replays_exactly_the_missed_events_then_receives_live_ones() {
        let log = EventLog::new("e");
        for _ in 0..5 {
            log.append("terminal.bell", session("s"), Map::new());
        }
        let subscription = log.follow(Some(2), Some("e"), Filter::default()).unwrap();
        assert_eq!(subscription.seq, 5);
        let replayed: Vec<u64> = subscription
            .replay
            .iter()
            .map(|logged| logged.seq)
            .collect();
        assert_eq!(replayed, vec![3, 4, 5]);
        log.append("terminal.bell", session("s"), Map::new());
        assert_eq!(drain(&subscription), vec![6]);
    }

    #[test]
    fn a_since_older_than_the_ring_or_another_epoch_is_evicted_with_the_current_position() {
        let log = EventLog::new("e");
        for _ in 0..(RING_CAPACITY + 10) {
            log.append("terminal.bell", session("s"), Map::new());
        }
        assert_eq!(log.ring_len(), RING_CAPACITY);
        assert_eq!(log.last_seq(), (RING_CAPACITY + 10) as u64);
        let expected = Evicted {
            seq: (RING_CAPACITY + 10) as u64,
            epoch: "e".to_string(),
        };
        assert_eq!(
            log.follow(Some(5), Some("e"), Filter::default()).err(),
            Some(expected.clone())
        );
        assert_eq!(
            log.follow(Some(20), Some("other"), Filter::default()).err(),
            Some(expected.clone())
        );
        assert_eq!(
            log.follow(Some(10_000), None, Filter::default()).err(),
            Some(expected.clone())
        );
        assert_eq!(
            log.follow(None, Some("other"), Filter::default()).err(),
            Some(expected),
            "an epoch from another worker start is refused even with no since"
        );
        let oldest_kept = log
            .follow(Some(10), Some("e"), Filter::default())
            .expect("the events after seq 10 are all still in the ring");
        assert_eq!(oldest_kept.replay.len(), RING_CAPACITY);
        assert_eq!(oldest_kept.replay[0].seq, 11);
    }

    #[test]
    fn a_type_and_workspace_filter_sends_only_matching_events_with_their_log_seq() {
        let log = EventLog::new("e");
        let filter = Filter::new(
            Some(&["terminal.*".to_string(), "pane.closed".to_string()]),
            None,
        )
        .unwrap();
        let typed = log.follow(None, None, filter).unwrap();
        let in_workspace = log
            .follow(None, None, Filter::new(None, Some(7)).unwrap())
            .unwrap();
        log.append("terminal.bell", session("s"), Map::new());
        log.append(
            "pane.created",
            EventIds {
                workspace_id: Some(7),
                ..EventIds::default()
            },
            Map::new(),
        );
        log.append(
            "pane.closed",
            EventIds {
                workspace_id: Some(8),
                ..EventIds::default()
            },
            Map::new(),
        );
        assert_eq!(drain(&typed), vec![1, 3]);
        assert_eq!(drain(&in_workspace), vec![2]);
        assert!(Filter::new(Some(&["*".to_string()]), None).is_err());
        assert!(Filter::new(Some(&["terminal.*,pane.closed".to_string()]), None).is_ok());
    }

    #[test]
    fn a_follower_more_than_eight_mebibytes_behind_is_dropped_and_others_keep_receiving() {
        let log = EventLog::new("e");
        let stalled = log.follow(None, None, Filter::default()).unwrap();
        let reading = log.follow(None, None, Filter::default()).unwrap();
        let payload = "x".repeat(32 * 1024);
        let mut received = 0usize;
        let total = MAX_FOLLOWER_BACKLOG_BYTES / payload.len() + 8;
        for _ in 0..total {
            log.append(
                "layout.changed",
                EventIds::default(),
                data(json!({"pad": payload})),
            );
            received += drain(&reading).len();
        }
        assert_eq!(received, total);
        assert_eq!(log.follower_count(), 1);
        assert!(matches!(
            stalled.next(Duration::from_millis(1)),
            Delivery::Overflowed
        ));
    }

    #[test]
    fn a_closed_log_ends_every_stream() {
        let log = EventLog::new("e");
        let subscription = log.follow(None, None, Filter::default()).unwrap();
        log.close();
        assert!(matches!(
            subscription.next(Duration::from_millis(1)),
            Delivery::Closed
        ));
    }
}
