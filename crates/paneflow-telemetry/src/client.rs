//! Consent-gated PostHog capture client.
//!
//! Network I/O is blocking and runtime-neutral. Call [`TelemetryClient::poll_flush`]
//! from a background task. Shutdown may call [`TelemetryClient::flush_blocking`]
//! with an explicit deadline. Failed batches are logged at DEBUG and dropped.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::event::TelemetryEvent;

/// A full queue triggers a flush on the next scheduler poll.
pub(crate) const BATCH_MAX: usize = 10;
/// Defense-in-depth bound on queued event count.
pub(crate) const QUEUE_MAX: usize = 1_000;
/// Hard bound on serialized property bytes retained in memory.
pub(crate) const QUEUE_MAX_BYTES: usize = 512 * 1024;
/// Maximum age of the oldest queued event before a scheduler poll flushes it.
pub(crate) const BATCH_MAX_AGE: Duration = Duration::from_secs(30);
/// Default transport deadline for background flushes.
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

const KILL_SWITCH_VARS: [&str; 3] = ["PANEFLOW_NO_TELEMETRY", "DO_NOT_TRACK", "NO_TELEMETRY"];

struct Event {
    name: &'static str,
    properties: Map<String, Value>,
}

struct Queue {
    events: VecDeque<Event>,
    queued_bytes: usize,
    dropped_events: usize,
    first_queued_at: Option<Instant>,
}

impl Queue {
    fn new() -> Self {
        Self {
            events: VecDeque::new(),
            queued_bytes: 0,
            dropped_events: 0,
            first_queued_at: None,
        }
    }

    fn take_events(&mut self) -> Vec<Event> {
        self.first_queued_at = None;
        self.queued_bytes = 0;
        self.events.drain(..).collect()
    }
}

enum RuntimeState {
    Active(Queue),
    Disabled,
}

struct Endpoint {
    api_key: String,
    host: String,
    distinct_id: String,
}

/// Explicit application consent state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TelemetryConsent {
    #[default]
    Unknown,
    Declined,
    Granted,
}

impl TelemetryConsent {
    pub const fn from_config(enabled: Option<bool>) -> Self {
        match enabled {
            None => Self::Unknown,
            Some(false) => Self::Declined,
            Some(true) => Self::Granted,
        }
    }
}

/// Thread-safe telemetry handle. Disabled and active states share the same
/// no-op capture surface, while construction and runtime state remain private.
pub struct TelemetryClient {
    endpoint: Option<Endpoint>,
    state: Mutex<RuntimeState>,
    /// A read guard spans each network request. Deactivation marks the state
    /// disabled first, then takes the write guard so it cannot return while an
    /// older handle can still begin or finish an HTTP request.
    send_gate: RwLock<()>,
}

impl Default for TelemetryClient {
    fn default() -> Self {
        Self::disabled()
    }
}

impl TelemetryClient {
    /// Build a disabled client with no endpoint or identifier state.
    pub fn disabled() -> Self {
        Self {
            endpoint: None,
            state: Mutex::new(RuntimeState::Disabled),
            send_gate: RwLock::new(()),
        }
    }

    /// Resolve consent and kill switches once, then lazily create the anonymous
    /// identifier only when capture is allowed. The returned boolean is the
    /// identifier factory's `is_first_run` value, or `false` when disabled.
    pub fn from_consent<F>(
        consent: TelemetryConsent,
        api_key: &str,
        host: &str,
        distinct_id: F,
    ) -> (Self, bool)
    where
        F: FnOnce() -> (String, bool),
    {
        Self::from_consent_with_kill_switch(
            consent,
            api_key,
            host,
            distinct_id,
            is_kill_switch_set(),
        )
    }

    fn from_consent_with_kill_switch<F>(
        consent: TelemetryConsent,
        api_key: &str,
        host: &str,
        distinct_id: F,
        kill_switch_set: bool,
    ) -> (Self, bool)
    where
        F: FnOnce() -> (String, bool),
    {
        if kill_switch_set || consent != TelemetryConsent::Granted {
            return (Self::disabled(), false);
        }
        if api_key.is_empty() {
            log::warn!(
                "paneflow: telemetry is opted-in but POSTHOG_API_KEY was empty at build time; \
                 PostHog will reject every batch. Provide POSTHOG_API_KEY at build time or set \
                 PANEFLOW_NO_TELEMETRY=1 to suppress this warning."
            );
        }
        let (distinct_id, is_first_run) = anonymous_distinct_id(distinct_id());
        (Self::active(api_key, host, distinct_id), is_first_run)
    }

    fn active(api_key: &str, host: &str, distinct_id: String) -> Self {
        Self {
            endpoint: Some(Endpoint {
                api_key: api_key.to_string(),
                host: host.trim_end_matches('/').to_string(),
                distinct_id,
            }),
            state: Mutex::new(RuntimeState::Active(Queue::new())),
            send_gate: RwLock::new(()),
        }
    }

    /// Queue one canonical event. Invalid or oversized payloads are dropped.
    pub fn capture(&self, event: TelemetryEvent) {
        let Some(encoded_len) = event.encoded_len_if_safe() else {
            log::debug!(
                "telemetry: rejected invalid or oversized {} event",
                event.name()
            );
            return;
        };
        let (name, properties) = event.into_parts();
        let mut state = self.lock_state();
        let RuntimeState::Active(queue) = &mut *state else {
            return;
        };
        let Some(next_bytes) = queue.queued_bytes.checked_add(encoded_len) else {
            queue.dropped_events = queue.dropped_events.saturating_add(1);
            return;
        };
        if queue.events.len() >= QUEUE_MAX || next_bytes > QUEUE_MAX_BYTES {
            queue.dropped_events = queue.dropped_events.saturating_add(1);
            return;
        }
        if queue.events.is_empty() {
            queue.first_queued_at = Some(Instant::now());
        }
        queue.queued_bytes = next_bytes;
        queue.events.push_back(Event { name, properties });
    }

    /// Scheduler hook. Call periodically from a background task.
    pub fn poll_flush(&self) {
        let Some(endpoint) = &self.endpoint else {
            return;
        };
        let _send_guard = self
            .send_gate
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let batch = {
            let mut state = self.lock_state();
            let RuntimeState::Active(queue) = &mut *state else {
                return;
            };
            if !should_flush(queue) {
                return;
            }
            log_dropped_events(queue);
            queue.take_events()
        };
        post_batch(endpoint, &batch, HTTP_TIMEOUT);
    }

    /// Drain pending events and perform one blocking POST bounded by `timeout`.
    /// A zero timeout drops the drained batch without starting a request.
    pub fn flush_blocking(&self, timeout: Duration) {
        let Some(endpoint) = &self.endpoint else {
            return;
        };
        let _send_guard = self
            .send_gate
            .read()
            .unwrap_or_else(PoisonError::into_inner);
        let batch = {
            let mut state = self.lock_state();
            let RuntimeState::Active(queue) = &mut *state else {
                return;
            };
            if queue.events.is_empty() {
                return;
            }
            log_dropped_events(queue);
            queue.take_events()
        };
        if timeout.is_zero() {
            return;
        }
        post_batch(endpoint, &batch, timeout.min(HTTP_TIMEOUT));
    }

    /// Permanently disable capture and discard queued events without waiting
    /// for a request that was already in flight.
    pub fn disable(&self) {
        let mut state = self.lock_state();
        *state = RuntimeState::Disabled;
    }

    /// Disable this handle, then wait for any request already in flight to
    /// finish before returning. Run this from a background task when a live
    /// request could make waiting visible to the caller.
    pub fn deactivate(&self) {
        self.disable();
        let _send_guard = self
            .send_gate
            .write()
            .unwrap_or_else(PoisonError::into_inner);
    }

    pub fn is_active(&self) -> bool {
        matches!(*self.lock_state(), RuntimeState::Active(_))
    }

    fn lock_state(&self) -> MutexGuard<'_, RuntimeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn is_kill_switch_set() -> bool {
    is_kill_switch_set_with(|key| std::env::var_os(key).is_some())
}

fn anonymous_distinct_id(candidate: (String, bool)) -> (String, bool) {
    let (candidate, is_first_run) = candidate;
    match Uuid::parse_str(&candidate) {
        Ok(id) if id.get_version_num() == 4 => (id.to_string(), is_first_run),
        _ => {
            log::debug!("telemetry: invalid distinct_id replaced with a session-scoped UUID v4");
            (Uuid::new_v4().to_string(), false)
        }
    }
}

fn is_kill_switch_set_with(mut is_present: impl FnMut(&str) -> bool) -> bool {
    KILL_SWITCH_VARS.iter().copied().any(&mut is_present)
}

fn should_flush(queue: &Queue) -> bool {
    queue.events.len() >= BATCH_MAX
        || queue
            .first_queued_at
            .is_some_and(|queued_at| queued_at.elapsed() >= BATCH_MAX_AGE)
}

fn log_dropped_events(queue: &mut Queue) {
    if queue.dropped_events == 0 {
        return;
    }
    log::debug!(
        "telemetry: dropped {} event(s) because the in-memory queue was full",
        queue.dropped_events
    );
    queue.dropped_events = 0;
}

fn build_batch_body(endpoint: &Endpoint, batch: &[Event]) -> Value {
    let events: Vec<Value> = batch
        .iter()
        .map(|event| {
            json!({
                "event": event.name,
                "distinct_id": endpoint.distinct_id,
                "properties": posthog_anonymous_properties(&event.properties),
            })
        })
        .collect();
    json!({
        "api_key": endpoint.api_key,
        "batch": events,
    })
}

fn posthog_anonymous_properties(properties: &Map<String, Value>) -> Value {
    let mut properties = properties.clone();
    properties.insert("$process_person_profile".to_string(), json!(false));
    properties.insert("$geoip_disable".to_string(), json!(true));
    Value::Object(properties)
}

fn post_batch(endpoint: &Endpoint, batch: &[Event], timeout: Duration) {
    if batch.is_empty() {
        return;
    }
    let body = build_batch_body(endpoint, batch);
    let url = format!("{}/batch", endpoint.host);
    let outcome = ureq::post(&url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .header("Content-Type", "application/json")
        .send_json(&body);

    match outcome {
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                log::debug!(
                    "telemetry: batch of {} event(s) rejected with HTTP {}; dropped",
                    batch.len(),
                    status.as_u16()
                );
            }
        }
        Err(error) => {
            log::debug!(
                "telemetry: batch of {} event(s) failed to flush ({error}); dropped",
                batch.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::TelemetryVersion;
    use std::cell::Cell;
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};

    fn active_client() -> TelemetryClient {
        active_client_at("http://127.0.0.1:1")
    }

    fn active_client_at(host: &str) -> TelemetryClient {
        TelemetryClient::active("phc", host, Uuid::new_v4().to_string())
    }

    fn queued(event: TelemetryEvent) -> Event {
        assert!(event.encoded_len_if_safe().is_some());
        let (name, properties) = event.into_parts();
        Event { name, properties }
    }

    fn version(value: &str) -> TelemetryVersion {
        TelemetryVersion::parse(value).unwrap()
    }

    #[test]
    fn consent_config_maps_to_explicit_states() {
        assert_eq!(
            TelemetryConsent::from_config(None),
            TelemetryConsent::Unknown
        );
        assert_eq!(
            TelemetryConsent::from_config(Some(false)),
            TelemetryConsent::Declined
        );
        assert_eq!(
            TelemetryConsent::from_config(Some(true)),
            TelemetryConsent::Granted
        );
    }

    #[test]
    fn disabled_consent_never_resolves_identifier() {
        for consent in [TelemetryConsent::Unknown, TelemetryConsent::Declined] {
            let called = Cell::new(false);
            let (client, first_run) = TelemetryClient::from_consent_with_kill_switch(
                consent,
                "phc",
                "http://h",
                || {
                    called.set(true);
                    ("id".to_string(), true)
                },
                false,
            );
            assert!(!client.is_active());
            assert!(!first_run);
            assert!(!called.get());
        }
    }

    #[test]
    fn kill_switch_never_resolves_identifier() {
        let called = Cell::new(false);
        let (client, first_run) = TelemetryClient::from_consent_with_kill_switch(
            TelemetryConsent::Granted,
            "phc",
            "http://h",
            || {
                called.set(true);
                (Uuid::new_v4().to_string(), true)
            },
            true,
        );
        assert!(!client.is_active());
        assert!(!first_run);
        assert!(!called.get());
    }

    #[test]
    fn granted_consent_builds_active_client_once() {
        let calls = Cell::new(0);
        let (client, first_run) = TelemetryClient::from_consent_with_kill_switch(
            TelemetryConsent::Granted,
            "phc",
            "http://h/",
            || {
                calls.set(calls.get() + 1);
                (Uuid::new_v4().to_string(), true)
            },
            false,
        );
        assert!(client.is_active());
        assert!(first_run);
        assert_eq!(calls.get(), 1);
        assert_eq!(client.endpoint.as_ref().unwrap().host, "http://h");
        assert!(
            Uuid::parse_str(&client.endpoint.as_ref().unwrap().distinct_id)
                .is_ok_and(|id| id.get_version_num() == 4)
        );
    }

    #[test]
    fn non_anonymous_identifier_is_replaced_and_not_first_run() {
        let (client, first_run) = TelemetryClient::from_consent_with_kill_switch(
            TelemetryConsent::Granted,
            "phc",
            "http://h",
            || ("arthur@example.com".to_string(), true),
            false,
        );
        let distinct_id = &client.endpoint.as_ref().unwrap().distinct_id;
        assert_ne!(distinct_id, "arthur@example.com");
        assert!(Uuid::parse_str(distinct_id).is_ok_and(|id| id.get_version_num() == 4));
        assert!(!first_run);
    }

    #[test]
    fn kill_switch_predicate_accepts_any_present_variable() {
        for expected in KILL_SWITCH_VARS {
            assert!(is_kill_switch_set_with(|key| key == expected));
        }
        assert!(!is_kill_switch_set_with(|_| false));
    }

    #[test]
    fn disabled_client_is_a_noop() {
        let client = TelemetryClient::disabled();
        client.capture(TelemetryEvent::telemetry_reenabled());
        client.poll_flush();
        client.flush_blocking(Duration::from_millis(1));
        assert!(!client.is_active());
    }

    #[test]
    fn capture_enqueues_event_and_tracks_bytes() {
        let client = active_client();
        client.capture(TelemetryEvent::update_available(
            version("0.8.1"),
            version("0.8.2"),
            crate::event::UpdateAssetFormat::Deb,
        ));
        let state = client.lock_state();
        let RuntimeState::Active(queue) = &*state else {
            panic!("expected active queue");
        };
        assert_eq!(queue.events.len(), 1);
        assert!(queue.queued_bytes > 0);
        assert_eq!(queue.events[0].name, "update_available");
    }

    #[test]
    fn capture_drops_new_events_when_count_bound_is_full() {
        let client = active_client();
        for _ in 0..QUEUE_MAX + 3 {
            client.capture(TelemetryEvent::telemetry_reenabled());
        }
        let state = client.lock_state();
        let RuntimeState::Active(queue) = &*state else {
            panic!("expected active queue");
        };
        assert_eq!(queue.events.len(), QUEUE_MAX);
        assert_eq!(queue.dropped_events, 3);
        assert!(queue.queued_bytes <= QUEUE_MAX_BYTES);
    }

    #[test]
    fn deactivate_clears_queue_and_ignores_future_capture() {
        let client = active_client();
        client.capture(TelemetryEvent::telemetry_reenabled());
        client.deactivate();
        client.capture(TelemetryEvent::telemetry_reenabled());
        assert!(!client.is_active());
        assert!(matches!(*client.lock_state(), RuntimeState::Disabled));
    }

    #[test]
    fn should_flush_uses_size_or_age_threshold() {
        let mut queue = Queue::new();
        assert!(!should_flush(&queue));
        queue
            .events
            .push_back(queued(TelemetryEvent::telemetry_reenabled()));
        queue.first_queued_at = Some(Instant::now() - BATCH_MAX_AGE);
        assert!(should_flush(&queue));

        let mut queue = Queue::new();
        for _ in 0..BATCH_MAX {
            queue
                .events
                .push_back(queued(TelemetryEvent::telemetry_reenabled()));
        }
        assert!(should_flush(&queue));
    }

    #[test]
    fn batch_body_forces_anonymous_processing_controls() {
        let endpoint = Endpoint {
            api_key: "phc_test".to_string(),
            host: "http://h".to_string(),
            distinct_id: "00000000-0000-4000-8000-000000000000".to_string(),
        };
        let body = build_batch_body(
            &endpoint,
            &[queued(TelemetryEvent::app_started(
                crate::event::OperatingSystem::Linux,
                crate::event::Architecture::X86_64,
                version("0.8.2"),
                crate::event::InstallMethod::Deb,
                true,
            ))],
        );
        assert_eq!(body["api_key"], "phc_test");
        assert_eq!(body["batch"][0]["event"], "app_started");
        assert_eq!(
            body["batch"][0]["distinct_id"],
            "00000000-0000-4000-8000-000000000000"
        );
        assert_eq!(
            body["batch"][0]["properties"]["$process_person_profile"],
            false
        );
        assert_eq!(body["batch"][0]["properties"]["$geoip_disable"], true);
    }

    #[test]
    fn processing_controls_override_caller_values() {
        let mut properties = Map::new();
        properties.insert("$process_person_profile".to_string(), json!(true));
        properties.insert("$geoip_disable".to_string(), json!(false));
        let properties = posthog_anonymous_properties(&properties);
        assert_eq!(properties["$process_person_profile"], false);
        assert_eq!(properties["$geoip_disable"], true);
    }

    #[test]
    fn poll_flush_drops_batch_on_unroutable_host() {
        let client = active_client();
        for _ in 0..BATCH_MAX {
            client.capture(TelemetryEvent::telemetry_reenabled());
        }
        client.poll_flush();
        let state = client.lock_state();
        let RuntimeState::Active(queue) = &*state else {
            panic!("expected active queue");
        };
        assert!(queue.events.is_empty());
        assert_eq!(queue.queued_bytes, 0);
    }

    #[test]
    fn zero_timeout_drops_without_starting_request() {
        let client = active_client();
        client.capture(TelemetryEvent::telemetry_reenabled());
        client.flush_blocking(Duration::ZERO);
        let state = client.lock_state();
        let RuntimeState::Active(queue) = &*state else {
            panic!("expected active queue");
        };
        assert!(queue.events.is_empty());
    }

    #[test]
    fn flush_blocking_respects_transport_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        let client = active_client_at(&host);
        client.capture(TelemetryEvent::telemetry_reenabled());
        let start = Instant::now();
        client.flush_blocking(Duration::from_millis(100));
        let elapsed = start.elapsed();

        accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(elapsed >= Duration::from_millis(75), "elapsed={elapsed:?}");
        assert!(elapsed < Duration::from_secs(1), "elapsed={elapsed:?}");
        release_tx.send(()).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn deactivate_waits_for_an_in_flight_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = format!("http://{}", listener.local_addr().unwrap());
        let (accepted_tx, accepted_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            accepted_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });

        let client = Arc::new(active_client_at(&host));
        client.capture(TelemetryEvent::telemetry_reenabled());
        let flushing = Arc::clone(&client);
        let flush = std::thread::spawn(move || {
            flushing.flush_blocking(Duration::from_secs(2));
        });
        accepted_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        client.disable();
        let deactivating = Arc::clone(&client);
        let (deactivated_tx, deactivated_rx) = mpsc::channel();
        let deactivate = std::thread::spawn(move || {
            deactivating.deactivate();
            deactivated_tx.send(()).unwrap();
        });
        assert!(deactivated_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err());
        assert!(!client.is_active());

        release_tx.send(()).unwrap();
        deactivated_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        flush.join().unwrap();
        deactivate.join().unwrap();
        server.join().unwrap();
    }
}
