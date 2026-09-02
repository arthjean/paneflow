#![cfg_attr(not(unix), allow(dead_code))]

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};

use paneflow_ipc_client::ai_hook::{
    METHOD_EXIT, METHOD_NOTIFICATION, METHOD_PROMPT_SUBMIT, METHOD_SESSION_END,
    METHOD_SESSION_START, METHOD_STOP, METHOD_TOOL_USE,
};
use serde_json::Value;

const SUBSCRIBER_QUEUE_CAP: usize = 1024;

pub const KNOWN_EVENT_TYPES: &[&str] = &[
    METHOD_SESSION_START,
    METHOD_PROMPT_SUBMIT,
    METHOD_TOOL_USE,
    METHOD_NOTIFICATION,
    METHOD_STOP,
    METHOD_EXIT,
    METHOD_SESSION_END,
    "surface_changed",
];

#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct EventFilter {
    pub surfaces: Option<HashSet<u64>>,
    pub types: Option<HashSet<String>>,
}

impl EventFilter {
    pub fn from_params(params: &Value) -> Result<Self, String> {
        let Some(obj) = params.as_object() else {
            return Err("events.subscribe params must be an object".to_string());
        };

        let surfaces = match obj.get("surfaces") {
            Some(v) => {
                let Some(arr) = v.as_array() else {
                    return Err("events.subscribe surfaces must be an array of numbers".to_string());
                };
                let mut set = HashSet::new();
                for item in arr {
                    let Some(surface_id) = item.as_u64() else {
                        return Err(
                            "events.subscribe surfaces must be an array of numbers".to_string()
                        );
                    };
                    set.insert(surface_id);
                }
                Some(set)
            }
            None => None,
        };

        let types = match obj.get("types") {
            Some(v) => {
                let Some(arr) = v.as_array() else {
                    return Err("events.subscribe types must be an array of strings".to_string());
                };
                let mut set = HashSet::new();
                for item in arr {
                    let Some(type_) = item.as_str() else {
                        return Err(
                            "events.subscribe types must be an array of strings".to_string()
                        );
                    };
                    if !KNOWN_EVENT_TYPES.contains(&type_) {
                        return Err(format!("unknown event type: {type_}"));
                    }
                    set.insert(type_.to_string());
                }
                Some(set)
            }
            None => None,
        };

        Ok(Self { surfaces, types })
    }

    pub fn matches(&self, type_: &str, surface_id: Option<u64>) -> bool {
        if let Some(types) = &self.types
            && !types.contains(type_)
        {
            return false;
        }
        if let Some(surfaces) = &self.surfaces {
            return surface_id.is_some_and(|sid| surfaces.contains(&sid));
        }
        true
    }
}

struct Subscriber {
    id: u64,
    filter: EventFilter,
    tx: SyncSender<String>,
    dropped: Arc<AtomicU64>,
}

pub struct EventBus {
    subscribers: Mutex<Vec<Subscriber>>,
    next_id: AtomicU64,
}

pub struct Subscription {
    pub id: u64,
    pub rx: Receiver<String>,
    dropped: Arc<AtomicU64>,
    bus: Arc<EventBus>,
}

impl Subscription {
    pub fn take_dropped(&self) -> u64 {
        self.dropped.swap(0, Ordering::Relaxed)
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        self.bus.unsubscribe(self.id);
    }
}

impl EventBus {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            subscribers: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn subscribe(self: &Arc<Self>, filter: EventFilter) -> Subscription {
        let (tx, rx) = sync_channel::<String>(SUBSCRIBER_QUEUE_CAP);
        let dropped = Arc::new(AtomicU64::new(0));
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.push(Subscriber {
                id,
                filter,
                tx,
                dropped: Arc::clone(&dropped),
            });
        }
        Subscription {
            id,
            rx,
            dropped,
            bus: Arc::clone(self),
        }
    }

    fn unsubscribe(&self, id: u64) {
        if let Ok(mut subs) = self.subscribers.lock() {
            subs.retain(|s| s.id != id);
        }
    }

    pub fn has_subscribers(&self) -> bool {
        self.subscribers
            .lock()
            .map(|s| !s.is_empty())
            .unwrap_or(false)
    }

    pub fn broadcast(&self, type_: &str, surface_id: Option<u64>, event: &Value) {
        let Ok(subs) = self.subscribers.lock() else {
            return;
        };
        if subs.is_empty() {
            return;
        }
        let mut line = match serde_json::to_string(event) {
            Ok(s) => s,
            Err(_) => return,
        };
        line.push('\n');
        for sub in subs.iter() {
            if !sub.filter.matches(type_, surface_id) {
                continue;
            }
            if let Err(TrySendError::Full(_)) = sub.tx.try_send(line.clone()) {
                sub.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_params_rejects_unknown_type() {
        let err = EventFilter::from_params(&json!({"types": ["ai.stop", "bogus"]})).unwrap_err();
        assert!(err.contains("bogus"), "got: {err}");
    }

    #[test]
    fn from_params_rejects_non_object_params() {
        let err = EventFilter::from_params(&json!(["ai.stop"])).unwrap_err();
        assert!(err.contains("object"), "got: {err}");
    }

    #[test]
    fn from_params_rejects_non_array_filters() {
        let err = EventFilter::from_params(&json!({"types": "ai.stop"})).unwrap_err();
        assert!(err.contains("types"), "got: {err}");

        let err = EventFilter::from_params(&json!({"surfaces": "1"})).unwrap_err();
        assert!(err.contains("surfaces"), "got: {err}");
    }

    #[test]
    fn from_params_rejects_mixed_filter_arrays() {
        let err = EventFilter::from_params(&json!({"types": ["ai.stop", 4]})).unwrap_err();
        assert!(err.contains("types"), "got: {err}");

        let err = EventFilter::from_params(&json!({"surfaces": [1, "bad"]})).unwrap_err();
        assert!(err.contains("surfaces"), "got: {err}");
    }

    #[test]
    fn from_params_accepts_known_types_and_surfaces() {
        let f = EventFilter::from_params(&json!({"types":["ai.stop"],"surfaces":[7]})).unwrap();
        assert!(f.types.unwrap().contains("ai.stop"));
        assert!(f.surfaces.unwrap().contains(&7));
    }

    #[test]
    fn empty_filter_matches_everything() {
        let f = EventFilter::default();
        assert!(f.matches("ai.stop", Some(1)));
        assert!(f.matches("surface_changed", None));
    }

    #[test]
    fn type_filter_excludes_other_types() {
        let f = EventFilter::from_params(&json!({"types":["ai.notification"]})).unwrap();
        assert!(f.matches("ai.notification", Some(1)));
        assert!(!f.matches("ai.stop", Some(1)));
    }

    #[test]
    fn surface_filter_excludes_unscoped_and_other_surfaces() {
        let f = EventFilter::from_params(&json!({"surfaces":[42]})).unwrap();
        assert!(f.matches("ai.stop", Some(42)));
        assert!(!f.matches("ai.stop", Some(7)));
        assert!(
            !f.matches("ai.stop", None),
            "a surface-scoped subscriber skips unscoped events"
        );
    }

    #[test]
    fn broadcast_delivers_to_matching_and_filters_others() {
        let bus = EventBus::new();
        let sub = bus.subscribe(EventFilter::from_params(&json!({"types":["ai.stop"]})).unwrap());
        bus.broadcast("ai.stop", Some(1), &json!({"type":"ai.stop"}));
        assert!(sub.rx.try_recv().is_ok(), "matching event delivered");
        bus.broadcast("ai.tool_use", Some(1), &json!({"type":"ai.tool_use"}));
        assert!(sub.rx.try_recv().is_err(), "non-matching type filtered out");
    }

    #[test]
    fn broadcast_drops_newest_when_subscriber_queue_full() {
        let bus = EventBus::new();
        let sub = bus.subscribe(EventFilter::default());
        for _ in 0..SUBSCRIBER_QUEUE_CAP + 5 {
            bus.broadcast("ai.stop", Some(1), &json!({"type":"ai.stop"}));
        }
        assert_eq!(sub.take_dropped(), 5, "5 events past the cap were dropped");
        assert_eq!(sub.take_dropped(), 0, "counter resets after a read");
    }

    #[test]
    fn unsubscribe_on_drop_removes_from_registry() {
        let bus = EventBus::new();
        {
            let _sub = bus.subscribe(EventFilter::default());
            assert!(bus.has_subscribers());
        }
        assert!(!bus.has_subscribers(), "drop unsubscribed");
    }
}
