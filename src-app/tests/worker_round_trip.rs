#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const ROUND_TRIPS: usize = 1_000;
const REPLACEMENTS: usize = 20;
const NOISE_TOLERANCE: f64 = 0.10;
const MAX_ATTEMPTS: usize = 5;
const FRAME_AT_60_HZ_US: f64 = 1_000_000.0 / 60.0;
const PROBE_WINDOW: Duration = Duration::from_millis(250);
const CLIENT: &str = "paneflow-worker-bench";

struct Binaries {
    current: PathBuf,
    next: PathBuf,
}

fn stage_binaries(dir: &Path) -> Binaries {
    let source = PathBuf::from(env!("CARGO_BIN_EXE_paneflow"));
    let name = source.file_name().expect("the binary has a file name");
    let current = dir.join("a").join(name);
    let next = dir.join("b").join(name);
    for target in [&current, &next] {
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("a staging dir");
        std::fs::copy(&source, target).expect("the binary is copied");
    }
    let mut bytes = std::fs::read(&next).expect("the copy reads back");
    bytes.push(0);
    std::fs::write(&next, bytes).expect("the second copy gets its own build id");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for target in [&current, &next] {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o755))
                .expect("the copy stays executable");
        }
    }
    Binaries { current, next }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn sorted(mut samples: Vec<f64>) -> Vec<f64> {
    samples.sort_by(f64::total_cmp);
    samples
}

fn spread(first: f64, second: f64) -> f64 {
    let base = first.min(second).max(f64::EPSILON);
    (first - second).abs() / base
}

fn round_trip_run(endpoint: &Path) -> Vec<f64> {
    let mut control = HostControl::connect(endpoint, CLIENT).expect("the worker answers");
    for _ in 0..50 {
        control.request("worker.hello", json!({})).expect("warm-up");
    }
    (0..ROUND_TRIPS)
        .map(|_| {
            let started = Instant::now();
            control
                .request("worker.hello", json!({}))
                .expect("a no-op request is answered");
            started.elapsed().as_secs_f64() * 1_000_000.0
        })
        .collect()
}

#[derive(Clone)]
struct Answer {
    at: Instant,
    build_id: String,
}

fn probe_until(endpoint: PathBuf, stop: Arc<AtomicBool>) -> std::thread::JoinHandle<Vec<Answer>> {
    std::thread::spawn(move || {
        let mut answers = Vec::new();
        while !stop.load(Ordering::Acquire) {
            if let Ok(control) = HostControl::probe_open(&endpoint, CLIENT, PROBE_WINDOW)
                && let Some(build_id) = control.identity()["build_id"].as_str()
            {
                answers.push(Answer {
                    at: Instant::now(),
                    build_id: build_id.to_string(),
                });
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        answers
    })
}

fn outage(answers: &[Answer], started: Instant, retiring: &str) -> Option<f64> {
    let last_old = answers
        .iter()
        .rfind(|answer| answer.build_id == retiring && answer.at <= started)?
        .at;
    let first_new = answers
        .iter()
        .find(|answer| answer.build_id != retiring && answer.at > last_old)?
        .at;
    Some(first_new.duration_since(last_old).as_secs_f64() * 1_000.0)
}

fn replacement_run(home: &Path, endpoint: &Path, binaries: &Binaries) -> Vec<f64> {
    let stop = Arc::new(AtomicBool::new(false));
    let probe = probe_until(endpoint.to_path_buf(), Arc::clone(&stop));
    std::thread::sleep(Duration::from_millis(100));
    let mut marks = Vec::new();
    for index in 0..REPLACEMENTS {
        let target = if index % 2 == 0 {
            &binaries.next
        } else {
            &binaries.current
        };
        let retiring = paneflow_serve::bootstrap::read_instance_record(home)
            .expect("a running worker records itself")
            .build_id;
        let started = Instant::now();
        let adoption = paneflow_serve::ensure_worker_running(home, target)
            .expect("the worker is replaced as an update replaces it");
        assert!(
            adoption.replaced.is_some(),
            "replacement {index} replaced nothing"
        );
        std::thread::sleep(Duration::from_millis(50));
        marks.push((started, retiring));
    }
    stop.store(true, Ordering::Release);
    let answers = probe.join().expect("the probe thread finishes");
    marks
        .iter()
        .map(|(started, retiring)| {
            outage(&answers, *started, retiring).expect("the probe saw both workers answer")
        })
        .collect()
}

fn stable<F: FnMut() -> Vec<f64>>(mut run: F, center: fn(&[f64]) -> f64) -> (Vec<f64>, Value) {
    let mut discarded = Vec::new();
    for attempt in 1..=MAX_ATTEMPTS {
        let first = sorted(run());
        let second = sorted(run());
        let (a, b) = (center(&first), center(&second));
        let difference = spread(a, b);
        if difference <= NOISE_TOLERANCE {
            let mut kept = first;
            kept.extend(second);
            return (
                sorted(kept),
                json!({"attempts": attempt, "discarded": discarded}),
            );
        }
        discarded.push(json!({"attempt": attempt, "first": a, "second": b, "spread": difference}));
    }
    panic!("every attempt differed by more than 10 %: {discarded:?}");
}

fn median(samples: &[f64]) -> f64 {
    percentile(samples, 0.5)
}

fn results_path(root: &Path) -> PathBuf {
    if let Some(out) = std::env::var_os("PANEFLOW_BENCH_OUT") {
        return PathBuf::from(out);
    }
    let stamp = std::env::var("PANEFLOW_BENCH_STAMP").unwrap_or_else(|_| "unstamped".into());
    let sha = std::env::var("PANEFLOW_BENCH_SHA").unwrap_or_else(|_| "unknown".into());
    root.join("bench")
        .join("results")
        .join(format!("worker-{stamp}-{sha}.json"))
}

#[test]
#[ignore = "measures the worker round trip and replacement; run with scripts/bench-worker.sh"]
fn worker_round_trip_benchmark() {
    let scratch = tempfile::tempdir().expect("a scratch dir");
    let home = scratch.path().join("home");
    std::fs::create_dir_all(&home).expect("a home");
    let binaries = stage_binaries(scratch.path());
    let endpoint = paneflow_home::serve_endpoint_path(&home);
    paneflow_serve::ensure_worker_running(&home, &binaries.current).expect("the worker starts");

    let (round_trips, round_trip_runs) = stable(|| round_trip_run(&endpoint), median);
    let (outages, replacement_runs) =
        stable(|| replacement_run(&home, &endpoint, &binaries), median);
    let _ = paneflow_serve::stop_worker(&home, paneflow_serve::DRAIN_WAIT);

    let round_trip_median = median(&round_trips);
    let round_trip_p95 = percentile(&round_trips, 0.95);
    let round_trip_max = round_trips.last().copied().unwrap_or_default();
    let outage_median = median(&outages);
    let outage_max = outages.last().copied().unwrap_or_default();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let result = json!({
        "schema": 1,
        "suite": "paneflow-worker-bench",
        "stamp": std::env::var("PANEFLOW_BENCH_STAMP").ok(),
        "git_sha": std::env::var("PANEFLOW_BENCH_SHA").ok(),
        "git_dirty": std::env::var("PANEFLOW_BENCH_DIRTY").ok(),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "debug_build": cfg!(debug_assertions),
        "round_trip": {
            "request": "worker.hello on an open connection",
            "requests_per_run": ROUND_TRIPS,
            "unit": "us",
            "median": round_trip_median,
            "p95": round_trip_p95,
            "max": round_trip_max,
            "runs": round_trip_runs,
        },
        "replacement": {
            "trigger": "ensure_worker_running with a binary of another build id, as an update does",
            "replacements_per_run": REPLACEMENTS,
            "unit": "ms",
            "median": outage_median,
            "max": outage_max,
            "samples": outages,
            "runs": replacement_runs,
        },
        "frame_at_60_hz_us": FRAME_AT_60_HZ_US,
        "p95_round_trip_within_one_frame": round_trip_p95 < FRAME_AT_60_HZ_US,
        "max_round_trip_within_one_frame": round_trip_max < FRAME_AT_60_HZ_US,
    });
    let path = results_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("the results dir");
    }
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&result).expect("json")),
    )
    .expect("the result is written");
    println!(
        "worker round trip: median {round_trip_median:.1} us, p95 {round_trip_p95:.1} us, max {round_trip_max:.1} us"
    );
    println!("worker replacement outage: median {outage_median:.1} ms, max {outage_max:.1} ms");
    println!("result: {}", path.display());
}
