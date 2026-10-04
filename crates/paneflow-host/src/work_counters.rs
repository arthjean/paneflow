use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value, json};

use crate::process::ProcessIdentity;

pub const PROCESS_IDENTITY_KEY: &str = "process_identity";

pub const HOST_COUNTERS: &[&str] = &[
    "process_listings",
    "foreground_observations",
    "agent_bus_session_broadcasts",
    "agent_bus_session_removed_broadcasts",
    "agent_bus_cancellation_broadcasts",
    "agent_bus_event_broadcasts",
    "agent_bus_snapshot_broadcasts",
];

pub const WORKER_COUNTERS: &[&str] = &["snapshot_broadcasts", "projection_broadcasts", "sweeps"];

pub const DESKTOP_COUNTERS: &[&str] = &[
    "root_renders",
    "host_agent_snapshots_applied",
    "session_list_calls",
    "git_spawns.total",
    "git_spawns.probe",
    "git_spawns.user_action",
    "git_spawns.by_subcommand.config",
    "git_spawns.by_subcommand.status",
    "git_spawns.by_subcommand.diff",
    "process_spawns",
];

#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

pub static PROCESS_LISTINGS: Counter = Counter::new();

pub static FOREGROUND_OBSERVATIONS: Counter = Counter::new();

pub fn process_identity() -> ProcessIdentity {
    static IDENTITY: OnceLock<ProcessIdentity> = OnceLock::new();
    *IDENTITY.get_or_init(|| ProcessIdentity::capture(std::process::id()))
}

pub fn counters_value(mut counters: Map<String, Value>) -> Value {
    counters.insert(PROCESS_IDENTITY_KEY.to_string(), json!(process_identity()));
    Value::Object(counters)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Measured(u64),
    Pending(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterSample {
    pub process: String,
    pub identity: Option<ProcessIdentity>,
    pub readings: BTreeMap<String, Reading>,
}

pub fn sample(process: &str, response: &Value, names: &[&str]) -> CounterSample {
    let Some(counters) = response.get("counters").filter(|value| value.is_object()) else {
        let reason = format!("counters unavailable: {process} reports no counters object");
        return CounterSample {
            process: process.to_string(),
            identity: None,
            readings: names
                .iter()
                .map(|name| (name.to_string(), Reading::Pending(reason.clone())))
                .collect(),
        };
    };
    let identity = counters
        .get(PROCESS_IDENTITY_KEY)
        .and_then(|identity| serde_json::from_value(identity.clone()).ok());
    let readings = names
        .iter()
        .map(|name| {
            let reading = name
                .split('.')
                .try_fold(counters, |value, key| value.get(key))
                .and_then(Value::as_u64)
                .map_or_else(
                    || Reading::Pending(format!("counter {name} absent from {process}")),
                    Reading::Measured,
                );
            (name.to_string(), reading)
        })
        .collect();
    CounterSample {
        process: process.to_string(),
        identity,
        readings,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Window {
    Deltas(BTreeMap<String, Reading>),
    Invalid(String),
}

pub fn window(before: &CounterSample, after: &CounterSample) -> Window {
    let measured = |sample: &CounterSample| {
        sample
            .readings
            .values()
            .any(|reading| matches!(reading, Reading::Measured(_)))
    };
    if (measured(before) || measured(after)) && before.identity != after.identity {
        return Window::Invalid(format!(
            "{} restarted during the measurement",
            after.process
        ));
    }
    let mut deltas = BTreeMap::new();
    for (name, end) in &after.readings {
        let start = before.readings.get(name);
        let delta = match (start, end) {
            (Some(Reading::Measured(start)), Reading::Measured(end)) => {
                let Some(delta) = end.checked_sub(*start) else {
                    return Window::Invalid(format!(
                        "{name} of {} went backwards from {start} to {end}",
                        after.process
                    ));
                };
                Reading::Measured(delta)
            }
            (Some(Reading::Pending(reason)), _) | (_, Reading::Pending(reason)) => {
                Reading::Pending(reason.clone())
            }
            (None, Reading::Measured(_)) => {
                Reading::Pending(format!("counter {name} absent from the first sample"))
            }
        };
        deltas.insert(name.clone(), delta);
    }
    Window::Deltas(deltas)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &["process_listings", "git_spawns.status"];

    fn response(pid: u32, started_at: u64, listings: u64, status: u64) -> Value {
        json!({"counters": {
            "process_identity": {"pid": pid, "started_at": started_at},
            "process_listings": listings,
            "git_spawns": {"status": status},
        }})
    }

    #[test]
    fn a_counter_moves_by_exactly_one_per_increment() {
        let counter = Counter::new();
        counter.increment();
        assert_eq!(counter.get(), 1);
        counter.increment();
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn a_status_without_counters_reports_every_counter_pending_never_zero() {
        let old_host = json!({"resources": {}});
        let sample = sample("host 0.17.5", &old_host, NAMES);
        assert_eq!(sample.identity, None);
        for name in NAMES {
            match &sample.readings[*name] {
                Reading::Pending(reason) => assert_eq!(
                    reason,
                    "counters unavailable: host 0.17.5 reports no counters object"
                ),
                Reading::Measured(value) => panic!("{name} read as {value}"),
            }
        }
        let Window::Deltas(deltas) = window(&sample, &sample) else {
            panic!("an all-pending window is not a restart");
        };
        assert!(
            deltas
                .values()
                .all(|delta| matches!(delta, Reading::Pending(_)))
        );
    }

    #[test]
    fn a_missing_counter_is_pending_and_its_neighbors_still_measure() {
        let partial = json!({"counters": {
            "process_identity": {"pid": 7, "started_at": 1},
            "process_listings": 4,
        }});
        let sample = sample("host", &partial, NAMES);
        assert_eq!(sample.readings["process_listings"], Reading::Measured(4));
        assert_eq!(
            sample.readings["git_spawns.status"],
            Reading::Pending("counter git_spawns.status absent from host".to_string())
        );
    }

    #[test]
    fn a_window_reports_nested_and_flat_deltas_for_one_process() {
        let before = sample("desktop", &response(7, 100, 3, 10), NAMES);
        let after = sample("desktop", &response(7, 100, 5, 13), NAMES);
        let Window::Deltas(deltas) = window(&before, &after) else {
            panic!("same process");
        };
        assert_eq!(deltas["process_listings"], Reading::Measured(2));
        assert_eq!(deltas["git_spawns.status"], Reading::Measured(3));
    }

    #[test]
    fn a_restart_between_two_samples_invalidates_the_window_instead_of_a_delta() {
        let before = sample("worker", &response(7, 100, 50, 50), NAMES);
        let new_pid = sample("worker", &response(8, 200, 2, 2), NAMES);
        assert_eq!(
            window(&before, &new_pid),
            Window::Invalid("worker restarted during the measurement".to_string())
        );
        let reused_pid = sample("worker", &response(7, 300, 60, 60), NAMES);
        assert_eq!(
            window(&before, &reused_pid),
            Window::Invalid("worker restarted during the measurement".to_string())
        );
        let upgraded = sample("worker", &json!({"pid": 7}), NAMES);
        assert!(matches!(window(&upgraded, &before), Window::Invalid(_)));
    }

    #[test]
    fn a_counter_that_goes_backwards_in_one_process_invalidates_the_window() {
        let before = sample("host", &response(7, 100, 9, 0), NAMES);
        let after = sample("host", &response(7, 100, 4, 0), NAMES);
        assert!(matches!(window(&before, &after), Window::Invalid(_)));
    }

    #[test]
    #[ignore = "release-only cost bound: cargo test --release -p paneflow-host --lib work_counters -- --ignored"]
    fn an_increment_costs_less_than_fifty_nanoseconds_in_release() {
        const INCREMENTS: u32 = 10_000_000;
        let counter = Counter::new();
        let started = std::time::Instant::now();
        for _ in 0..INCREMENTS {
            std::hint::black_box(&counter).increment();
        }
        let elapsed = started.elapsed();
        assert_eq!(counter.get(), u64::from(INCREMENTS));
        let per_increment = elapsed / INCREMENTS;
        assert!(
            per_increment < std::time::Duration::from_nanos(50),
            "{per_increment:?} per increment"
        );
    }

    #[test]
    fn the_counters_value_carries_this_process_identity() {
        let mut counters = Map::new();
        counters.insert(
            "process_listings".to_string(),
            json!(PROCESS_LISTINGS.get()),
        );
        let value = counters_value(counters);
        assert_eq!(
            value[PROCESS_IDENTITY_KEY]["pid"].as_u64(),
            Some(u64::from(std::process::id()))
        );
        let sample = sample("host", &json!({"counters": value}), &["process_listings"]);
        assert_eq!(sample.identity, Some(process_identity()));
    }
}
