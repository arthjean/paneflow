//! Minimal PostHog capture client (US-012).
//!
//! Design goals - in order of priority:
//! 1. **Never block the main thread.** Network I/O goes through an
//!    `ActiveClient::post_batch` call that the caller schedules on a
//!    background thread (`cx.background_spawn` in US-013, or a plain
//!    `std::thread::spawn` at shutdown).
//! 2. **Runtime-neutral.** We use blocking `ureq`, not `reqwest`/tokio -
//!    the PaneFlow desktop already ships `ureq` (self-update) and runs on
//!    GPUI's `smol` executor. Adding tokio would fragment the async
//!    surface and bloat the binary by several MB (see PRD Research
//!    Findings §Validated Technical Choices).
//! 3. **Drop, don't retry.** A failed flush logs at DEBUG and discards
//!    the batch. v1 tolerates data loss over complexity or startup
//!    latency (PRD AC #4).
//! 4. **Type-erased disabled state.** The `Null` variant gives us a
//!    single call-site signature (`client.capture(...)`) whether the
//!    user has opted in, opted out, or not yet been asked (PRD AC #6).
//!
//! Non-goals for v1 (may ship later):
//! - Retry queue / persistence across launches.
//! - Client-side timestamps (PostHog server-stamps on receipt; good
//!   enough for a ~30s batching window).
//! - Event de-duplication.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Flush threshold: a full batch of events triggers an immediate post.
pub(crate) const BATCH_MAX: usize = 10;
/// Hard queue bound. Capture is best effort, so once this many events are
/// buffered we drop new events instead of retaining unbounded memory.
pub(crate) const QUEUE_MAX: usize = 1_000;

/// Flush threshold: if the oldest queued event has been waiting this
/// long, post the batch even if it's under `BATCH_MAX`. Trades a small
/// amount of latency for steady data flow during idle sessions.
pub(crate) const BATCH_MAX_AGE: Duration = Duration::from_secs(30);

/// Per-request transport timeout. Applies to connect + read + write as a
/// single global budget (ureq `timeout_global`). A DNS failure surfaces
/// within this budget (PRD unhappy-path AC).
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// A single queued capture. We intentionally omit a client-side
/// `timestamp` - PostHog server-stamps on receipt, which is accurate
/// enough for a 30-second flush window and saves us a date-formatting
/// crate.
struct Event {
    event: String,
    properties: Value,
}

/// In-memory queue state. Guarded by a plain `Mutex` - contention is
/// negligible (tens of events per session at most) and the critical
/// sections are `Vec::push` / `mem::take`.
struct Queue {
    events: Vec<Event>,
    dropped_events: usize,
    /// Wall-clock timestamp of the moment the currently-buffered batch
    /// started - reset to `None` every time the queue drains. Used to
    /// trigger the age-based flush in `should_flush`.
    first_queued_at: Option<Instant>,
}

/// Consent state supplied by the application layer.
///
/// The telemetry crate deliberately does not depend on the global Paneflow
/// config schema. Callers adapt their own config into this tiny shape.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TelemetryConsent {
    enabled: Option<bool>,
}

impl TelemetryConsent {
    pub fn new(enabled: Option<bool>) -> Self {
        Self { enabled }
    }

    pub fn enabled(self) -> Option<bool> {
        self.enabled
    }
}

/// Live PostHog client. Cheap to construct; all methods are `&self` so
/// it can be shared across threads via `Arc<TelemetryClient>`.
pub struct ActiveClient {
    api_key: String,
    host: String,
    distinct_id: String,
    enabled: AtomicBool,
    queue: Mutex<Queue>,
}

/// Type-erased dispatch between opted-in (Active) and disabled (Null)
/// states. Callers always write `client.capture(...)` regardless.
pub enum TelemetryClient {
    Active(ActiveClient),
    Null,
}

impl TelemetryClient {
    /// Unconditional Active constructor. The caller has already decided
    /// telemetry is on - no consent checks happen here. Use
    /// [`TelemetryClient::from_consent`] for the gated factory.
    pub fn new(api_key: &str, host: &str, distinct_id: &str) -> Self {
        Self::Active(ActiveClient {
            api_key: api_key.to_string(),
            host: host.trim_end_matches('/').to_string(),
            distinct_id: distinct_id.to_string(),
            enabled: AtomicBool::new(true),
            queue: Mutex::new(Queue {
                events: Vec::new(),
                dropped_events: 0,
                first_queued_at: None,
            }),
        })
    }

    /// Consent-aware factory. Returns `Null` if any of the gates fail:
    /// - A kill-switch env var is set - any of `PANEFLOW_NO_TELEMETRY`,
    ///   `DO_NOT_TRACK`, or `NO_TELEMETRY`. Project-specific plus the two
    ///   de-facto community standards (`DO_NOT_TRACK` - .NET SDK / GitHub
    ///   CLI / Homebrew precedent; `NO_TELEMETRY` - the `no-telemetry`
    ///   universal opt-out). Unconditional; checked before consent state.
    /// - `consent.enabled` is `None` (user never answered).
    /// - `consent.enabled` is `Some(false)` (user declined).
    ///
    /// Only `Some(true)` with no env kill-switch returns Active.
    ///
    /// A WARN log is emitted once when the caller builds an Active client
    /// with an empty `api_key` - PostHog would otherwise 401 every batch
    /// silently, which only surfaces in the server dashboard.
    pub fn from_consent(
        consent: TelemetryConsent,
        api_key: &str,
        host: &str,
        distinct_id: &str,
    ) -> Self {
        if !Self::consent_allows_capture(consent) {
            return Self::Null;
        }
        if api_key.is_empty() {
            log::warn!(
                "paneflow: telemetry is opted-in but POSTHOG_API_KEY was empty at build time - \
                 PostHog will reject every batch with HTTP 401 and events will be silently \
                 dropped. Provide POSTHOG_API_KEY at build time or set PANEFLOW_NO_TELEMETRY=1 \
                 to suppress this warning."
            );
        }
        Self::new(api_key, host, distinct_id)
    }

    pub fn consent_allows_capture(consent: TelemetryConsent) -> bool {
        !is_kill_switch_set() && consent.enabled() == Some(true)
    }

    /// Queue one event. `Null` variant no-ops; no allocation.
    pub fn capture(&self, event: &str, properties: Value) {
        if let Self::Active(c) = self {
            c.capture(event, properties);
        }
    }

    /// Scheduler hook. Call periodically from a background task; only
    /// triggers an HTTP POST when the queue meets the size or age
    /// threshold. Cheap when there is nothing to do.
    pub fn poll_flush(&self) {
        if let Self::Active(c) = self {
            c.poll_flush();
        }
    }

    /// Shutdown hook. Drains any pending events and waits up to
    /// `timeout` for the HTTP POST to complete. On timeout the batch is
    /// dropped (its worker thread is detached) and shutdown continues -
    /// never block process exit on telemetry.
    pub fn flush_blocking(&self, timeout: Duration) {
        if let Self::Active(c) = self {
            c.flush_blocking(timeout);
        }
    }

    /// Permanently disable this handle and discard any queued events.
    ///
    /// Config reconciliation swaps `Arc<TelemetryClient>` handles, but background
    /// flushers may still own an older `Arc`. Deactivation makes those stale
    /// handles inert after opt-out.
    pub fn deactivate(&self) {
        if let Self::Active(c) = self {
            c.deactivate();
        }
    }

    /// Lightweight introspection for US-014 (settings toggle) - callers
    /// need to know whether to swap the client handle when consent changes.
    ///
    /// Currently only exercised by the unit-test suite; the reconcile path
    /// in `app::ipc_handler::reconcile_telemetry` rebuilds via
    /// [`TelemetryClient::from_consent`] unconditionally. Kept public for
    /// future callers (settings UI, IPC "is telemetry active?" probe).
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active(c) if c.enabled.load(Ordering::Relaxed))
    }
}

impl ActiveClient {
    fn capture(&self, event: &str, properties: Value) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let Ok(mut q) = self.queue.lock() else {
            // Lock poisoning means a previous holder panicked. Silently
            // drop the event - telemetry must never surface errors.
            return;
        };
        if q.events.len() >= QUEUE_MAX {
            q.dropped_events = q.dropped_events.saturating_add(1);
            return;
        }
        if q.events.is_empty() {
            q.first_queued_at = Some(Instant::now());
        }
        q.events.push(Event {
            event: event.to_string(),
            properties,
        });
    }

    fn poll_flush(&self) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let batch = {
            let Ok(mut q) = self.queue.lock() else {
                return;
            };
            if !should_flush(&q) {
                return;
            }
            if q.dropped_events > 0 {
                log::debug!(
                    "telemetry: dropped {} event(s) because the in-memory queue was full",
                    q.dropped_events
                );
                q.dropped_events = 0;
            }
            q.first_queued_at = None;
            std::mem::take(&mut q.events)
        };
        if batch.is_empty() {
            return;
        }
        post_batch(&self.api_key, &self.host, &self.distinct_id, &batch);
    }

    fn flush_blocking(&self, timeout: Duration) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let batch = {
            let Ok(mut q) = self.queue.lock() else {
                return;
            };
            if q.events.is_empty() {
                return;
            }
            if q.dropped_events > 0 {
                log::debug!(
                    "telemetry: dropped {} event(s) because the in-memory queue was full",
                    q.dropped_events
                );
                q.dropped_events = 0;
            }
            q.first_queued_at = None;
            std::mem::take(&mut q.events)
        };

        // Move the POST into a worker thread so we can enforce `timeout`
        // without relying on tokio-style cancellation. If the deadline
        // elapses the handle is dropped (detached) and the process
        // continues shutdown; the OS reaps the thread on exit.
        let api_key = self.api_key.clone();
        let host = self.host.clone();
        let distinct_id = self.distinct_id.clone();
        let handle = std::thread::spawn(move || {
            post_batch(&api_key, &host, &distinct_id, &batch);
        });

        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if handle.is_finished() {
                // Join to collect the thread and propagate nothing
                // (post_batch itself swallows all errors).
                let _ = handle.join();
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // Timed out - detach. The thread may still be in flight; it's
        // bounded by `HTTP_TIMEOUT` and will terminate without external
        // intervention. No value in `join`-ing here since the caller
        // asked us to honor the deadline.
    }

    fn deactivate(&self) {
        self.enabled.store(false, Ordering::Relaxed);
        if let Ok(mut q) = self.queue.lock() {
            q.events.clear();
            q.dropped_events = 0;
            q.first_queued_at = None;
        }
    }
}

/// Env-var kill-switch predicate. Any one of the three disables telemetry
/// unconditionally; checked before consent state in
/// [`TelemetryClient::from_consent`]. Kept as a free function (not a method)
/// so both the factory and the test module can call it without wiring.
fn is_kill_switch_set() -> bool {
    std::env::var("PANEFLOW_NO_TELEMETRY").is_ok()
        || std::env::var("DO_NOT_TRACK").is_ok()
        || std::env::var("NO_TELEMETRY").is_ok()
}

/// Trigger predicate. Split from `poll_flush` so the test module can
/// drive it directly without touching network state.
fn should_flush(q: &Queue) -> bool {
    if q.events.len() >= BATCH_MAX {
        return true;
    }
    match q.first_queued_at {
        Some(t) => t.elapsed() >= BATCH_MAX_AGE,
        None => false,
    }
}

/// Build the PostHog `/batch` body shape.
///
/// ```json
/// {
///   "api_key": "phc_...",
///   "batch": [
///     { "event": "...", "distinct_id": "...", "properties": {...} }
///   ]
/// }
/// ```
fn build_batch_body(api_key: &str, distinct_id: &str, batch: &[Event]) -> Value {
    let events: Vec<Value> = batch
        .iter()
        .map(|e| {
            let properties = posthog_anonymous_properties(&e.properties);
            json!({
                "event": e.event,
                "distinct_id": distinct_id,
                "properties": properties,
            })
        })
        .collect();
    json!({
        "api_key": api_key,
        "batch": events,
    })
}

fn posthog_anonymous_properties(properties: &Value) -> Value {
    match properties {
        Value::Object(map) => {
            let mut map = map.clone();
            map.entry("$process_person_profile".to_string())
                .or_insert_with(|| json!(false));
            Value::Object(map)
        }
        other => json!({
            "value": other,
            "$process_person_profile": false,
        }),
    }
}

/// POST the batch. Swallows every failure - transport errors and
/// non-2xx statuses both log at DEBUG and drop the batch silently. The
/// client must never surface errors to users or to the caller.
fn post_batch(api_key: &str, host: &str, distinct_id: &str, batch: &[Event]) {
    if batch.is_empty() {
        return;
    }
    let body = build_batch_body(api_key, distinct_id, batch);
    let url = format!("{host}/batch");

    let outcome = ureq::post(&url)
        .config()
        .timeout_global(Some(HTTP_TIMEOUT))
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
        Err(e) => {
            log::debug!(
                "telemetry: batch of {} event(s) failed to flush ({e}); dropped",
                batch.len()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Env vars are process-global. Any test that mutates the kill-switch
    // vars must serialize on this lock - otherwise a parallel `Null`-factory
    // test bleeds into the `Active`-factory test and the latter sees the
    // kill switch. The guard snapshots all three kill-switch vars so any
    // test that leaks one leaves no cross-test residue.
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    const KILL_SWITCH_VARS: [&str; 3] = ["PANEFLOW_NO_TELEMETRY", "DO_NOT_TRACK", "NO_TELEMETRY"];

    struct EnvGuard {
        prior: [(&'static str, Option<String>); 3],
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let prior = KILL_SWITCH_VARS.map(|k| (k, std::env::var(k).ok()));
            // Clear the snapshot so each test starts from a known state;
            // Drop restores the original values.
            for (k, _) in &prior {
                // SAFETY: ENV_LOCK held via `lock`.
                unsafe { std::env::remove_var(k) };
            }
            Self { prior, _lock: lock }
        }

        /// Sets `PANEFLOW_NO_TELEMETRY` specifically - preserved for the
        /// pre-existing test that only exercises that variable.
        fn set(&self, value: Option<&str>) {
            self.set_var("PANEFLOW_NO_TELEMETRY", value);
        }

        fn set_var(&self, key: &str, value: Option<&str>) {
            // SAFETY: serialized via ENV_LOCK held in `_lock` for the
            // lifetime of the guard - no other test or production thread
            // mutates this variable during the critical section.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: ENV_LOCK still held via `_lock`.
            unsafe {
                for (k, prior) in &self.prior {
                    match prior {
                        Some(v) => std::env::set_var(k, v),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }
    }

    fn consent(enabled: Option<bool>) -> TelemetryConsent {
        TelemetryConsent::new(enabled)
    }

    fn active(c: &TelemetryClient) -> &ActiveClient {
        match c {
            TelemetryClient::Active(a) => a,
            TelemetryClient::Null => panic!("expected Active variant"),
        }
    }

    #[test]
    fn new_builds_active_variant() {
        let c = TelemetryClient::new("phc_test", "http://localhost", "abc");
        assert!(c.is_active());
    }

    #[test]
    fn host_trailing_slash_is_trimmed() {
        let c = TelemetryClient::new("phc_test", "http://localhost/", "abc");
        assert_eq!(active(&c).host, "http://localhost");
    }

    #[test]
    fn null_client_capture_and_flush_noop() {
        let c = TelemetryClient::Null;
        c.capture("anything", json!({"foo": "bar"}));
        c.poll_flush();
        c.flush_blocking(Duration::from_millis(100));
        assert!(!c.is_active());
    }

    #[test]
    fn from_consent_null_when_unanswered() {
        let g = EnvGuard::take();
        g.set(None);
        let c = TelemetryClient::from_consent(consent(None), "phc", "http://h", "id");
        assert!(!c.is_active());
    }

    #[test]
    fn from_consent_null_when_opted_out() {
        let g = EnvGuard::take();
        g.set(None);
        let c = TelemetryClient::from_consent(consent(Some(false)), "phc", "http://h", "id");
        assert!(!c.is_active());
    }

    #[test]
    fn from_consent_active_when_opted_in() {
        let g = EnvGuard::take();
        g.set(None);
        let c = TelemetryClient::from_consent(consent(Some(true)), "phc", "http://h", "id");
        assert!(c.is_active());
    }

    #[test]
    fn from_consent_env_kill_switch_overrides_opt_in() {
        let g = EnvGuard::take();
        g.set(Some("1"));
        // Even with explicit consent, the env var wins.
        let c = TelemetryClient::from_consent(consent(Some(true)), "phc", "http://h", "id");
        assert!(!c.is_active());
    }

    #[test]
    fn from_consent_do_not_track_env_kills_opt_in() {
        // De-facto cross-tool standard (.NET SDK, GitHub CLI, Homebrew).
        let g = EnvGuard::take();
        g.set_var("DO_NOT_TRACK", Some("1"));
        let c = TelemetryClient::from_consent(consent(Some(true)), "phc", "http://h", "id");
        assert!(!c.is_active());
    }

    #[test]
    fn from_consent_no_telemetry_env_kills_opt_in() {
        // De-facto universal opt-out standard (`no-telemetry` project).
        let g = EnvGuard::take();
        g.set_var("NO_TELEMETRY", Some("1"));
        let c = TelemetryClient::from_consent(consent(Some(true)), "phc", "http://h", "id");
        assert!(!c.is_active());
    }

    #[test]
    fn is_kill_switch_set_picks_up_any_of_three_vars() {
        let g = EnvGuard::take();
        assert!(
            !is_kill_switch_set(),
            "clean env must not report kill switch"
        );
        for var in KILL_SWITCH_VARS {
            g.set_var(var, Some("1"));
            assert!(is_kill_switch_set(), "{var} should trigger kill switch");
            g.set_var(var, None);
            assert!(
                !is_kill_switch_set(),
                "clearing {var} must lift the kill switch"
            );
        }
    }

    #[test]
    fn capture_enqueues_event() {
        let c = TelemetryClient::new("phc", "http://h", "id");
        c.capture("hello", json!({"x": 1}));
        c.capture("world", json!({"x": 2}));
        let a = active(&c);
        let q = a.queue.lock().unwrap();
        assert_eq!(q.events.len(), 2);
        assert_eq!(q.events[0].event, "hello");
        assert_eq!(q.dropped_events, 0);
        assert!(q.first_queued_at.is_some());
    }

    #[test]
    fn capture_drops_new_events_when_queue_is_full() {
        let c = TelemetryClient::new("phc", "http://h", "id");
        for i in 0..QUEUE_MAX + 3 {
            c.capture("hello", json!({"i": i}));
        }
        let a = active(&c);
        let q = a.queue.lock().unwrap();
        assert_eq!(q.events.len(), QUEUE_MAX);
        assert_eq!(q.dropped_events, 3);
    }

    #[test]
    fn deactivate_clears_queue_and_ignores_future_capture() {
        let c = TelemetryClient::new("phc", "http://h", "id");
        c.capture("hello", json!({"x": 1}));
        c.deactivate();
        assert!(!c.is_active());
        c.capture("world", json!({"x": 2}));
        let a = active(&c);
        let q = a.queue.lock().unwrap();
        assert!(q.events.is_empty());
        assert_eq!(q.dropped_events, 0);
    }

    #[test]
    fn should_flush_false_for_empty_queue() {
        let q = Queue {
            events: Vec::new(),
            dropped_events: 0,
            first_queued_at: None,
        };
        assert!(!should_flush(&q));
    }

    #[test]
    fn should_flush_false_for_small_recent_batch() {
        let q = Queue {
            events: vec![Event {
                event: "e".into(),
                properties: json!({}),
            }],
            dropped_events: 0,
            first_queued_at: Some(Instant::now()),
        };
        assert!(!should_flush(&q));
    }

    #[test]
    fn should_flush_true_on_size_threshold() {
        let events: Vec<Event> = (0..BATCH_MAX)
            .map(|i| Event {
                event: format!("e{i}"),
                properties: json!({}),
            })
            .collect();
        let q = Queue {
            events,
            dropped_events: 0,
            first_queued_at: Some(Instant::now()),
        };
        assert!(should_flush(&q));
    }

    #[test]
    fn should_flush_true_on_age_threshold() {
        let q = Queue {
            events: vec![Event {
                event: "e".into(),
                properties: json!({}),
            }],
            dropped_events: 0,
            first_queued_at: Some(Instant::now() - BATCH_MAX_AGE - Duration::from_millis(1)),
        };
        assert!(should_flush(&q));
    }

    #[test]
    fn batch_body_shape_matches_posthog_contract() {
        let events = vec![
            Event {
                event: "app_started".into(),
                properties: json!({"os": "linux"}),
            },
            Event {
                event: "app_exited".into(),
                properties: json!({"session_duration_seconds": 42}),
            },
        ];
        let body = build_batch_body("phc_test", "dist-123", &events);
        assert_eq!(body["api_key"], "phc_test");
        let batch = body["batch"].as_array().unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0]["event"], "app_started");
        assert_eq!(batch[0]["distinct_id"], "dist-123");
        assert_eq!(batch[0]["properties"]["os"], "linux");
        assert_eq!(batch[0]["properties"]["$process_person_profile"], false);
        // No client-side timestamp - server stamps on receipt.
        assert!(batch[0].get("timestamp").is_none());
    }

    #[test]
    fn non_object_properties_are_wrapped_before_posthog_batching() {
        let events = vec![Event {
            event: "odd".into(),
            properties: json!("raw"),
        }];
        let body = build_batch_body("phc_test", "dist-123", &events);
        assert_eq!(body["batch"][0]["properties"]["value"], "raw");
        assert_eq!(
            body["batch"][0]["properties"]["$process_person_profile"],
            false
        );
    }

    // An unroutable endpoint (port 1, reserved) forces `ureq` into its
    // connection-refused / timeout path without relying on external
    // network state. The test proves two contracts simultaneously:
    // 1. `poll_flush` returns without panicking on transport failure.
    // 2. The queue is drained regardless - v1 drops, does not retry.
    #[test]
    fn poll_flush_drops_batch_on_unroutable_host() {
        let c = TelemetryClient::new("phc", "http://127.0.0.1:1", "id");
        for i in 0..BATCH_MAX {
            c.capture("e", json!({"i": i}));
        }
        // Size threshold reached → poll_flush drains + posts + fails silently.
        c.poll_flush();
        let a = active(&c);
        let q = a.queue.lock().unwrap();
        assert!(
            q.events.is_empty(),
            "queue must be cleared even on failed post"
        );
        assert!(q.first_queued_at.is_none());
    }

    #[test]
    fn flush_blocking_respects_timeout_on_unroutable_host() {
        let c = TelemetryClient::new("phc", "http://127.0.0.1:1", "id");
        c.capture("e", json!({}));
        let start = Instant::now();
        // Very short timeout - ensures we're testing the deadline path,
        // not waiting for ureq's HTTP_TIMEOUT (5s).
        c.flush_blocking(Duration::from_millis(100));
        let elapsed = start.elapsed();
        // Allow some jitter; the point is we're not waiting seconds.
        assert!(
            elapsed < Duration::from_millis(500),
            "flush_blocking should honor timeout, elapsed={elapsed:?}"
        );
    }
}
