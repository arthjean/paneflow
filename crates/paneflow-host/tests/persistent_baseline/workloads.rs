use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, ProcessIdentity, SessionId};
use serde_json::{Value, json};

pub const HISTORY_LINES: u64 = 10_000;
pub const W02_REATTACHMENTS: usize = 10;
pub const W02_CONCURRENT: usize = 10;
pub const W02_CYCLES: usize = 100;
pub const W03_STREAMS: usize = 10;
pub const W03_RATE_BYTES_PER_SECOND: u64 = 1024 * 1024;
pub const W03_SINGLE_FLOOD_BYTES: u64 = 32 * 1024 * 1024;
pub const W05_BATCHES: usize = 10;
pub const W05_BATCH_SIZE: usize = 50;
pub const W05_FLOOD_BYTES: u64 = 64 * 1024;

pub const NFR08_IDLE_P95_MS: f64 = 30.0;
pub const NFR08_IDLE_P99_MS: f64 = 100.0;
pub const NFR08_LOADED_P95_MS: f64 = 50.0;
pub const NFR08_THROUGHPUT_RATIO: f64 = 0.9;
pub const NFR09_ATTACH_P95_MS: f64 = 1000.0;
pub const NFR09_CONCURRENT_TOTAL_MS: f64 = 5000.0;
pub const NFR04_RECLAIM_S: f64 = 5.0;
pub const NFR05_SLOPE_BYTES_PER_BATCH: f64 = 1024.0 * 1024.0;
pub const NFR05_FLOOR_BYTES: f64 = 16.0 * 1024.0 * 1024.0;
pub const NFR05_COUNTER_SLACK: u64 = 2;

pub struct Protocol {
    pub label: &'static str,
    pub stream_seconds: u64,
    pub echo_samples: usize,
    pub quiescence: Duration,
    pub worker_cycles: usize,
    pub acceptance_grade: bool,
}

pub fn protocol() -> Protocol {
    if std::env::var_os("PANEFLOW_BENCH_QUICK").is_some() {
        Protocol {
            label: "smoke",
            stream_seconds: 5,
            echo_samples: 200,
            quiescence: Duration::from_secs(5),
            worker_cycles: 2,
            acceptance_grade: false,
        }
    } else {
        Protocol {
            label: "full",
            stream_seconds: 60,
            echo_samples: 1000,
            quiescence: Duration::from_secs(60),
            worker_cycles: 10,
            acceptance_grade: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub id: String,
    pub workload: &'static str,
    pub metric: String,
    pub threshold: String,
    pub observed: Value,
    pub result: &'static str,
    pub reason: String,
}

impl Decision {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "workload": self.workload,
            "metric": self.metric,
            "threshold": self.threshold,
            "observed": self.observed,
            "result": self.result,
            "reason": self.reason,
        })
    }
}

pub fn decide(
    id: &str,
    workload: &'static str,
    metric: &str,
    threshold: &str,
    observed: Option<f64>,
    pass: impl Fn(f64) -> bool,
    unavailable_reason: &str,
) -> Decision {
    match observed {
        Some(value) if pass(value) => Decision {
            id: id.to_string(),
            workload,
            metric: metric.to_string(),
            threshold: threshold.to_string(),
            observed: json!(value),
            result: "pass",
            reason: String::new(),
        },
        Some(value) => Decision {
            id: id.to_string(),
            workload,
            metric: metric.to_string(),
            threshold: threshold.to_string(),
            observed: json!(value),
            result: "fail",
            reason: format!("{metric} observed {value:.3}; threshold {threshold}"),
        },
        None => Decision {
            id: id.to_string(),
            workload,
            metric: metric.to_string(),
            threshold: threshold.to_string(),
            observed: Value::Null,
            result: "unavailable",
            reason: unavailable_reason.to_string(),
        },
    }
}

pub fn seed_failure(decisions: &mut Vec<Decision>) {
    if std::env::var_os("PANEFLOW_BENCH_SEED_FAILURE").is_some() {
        decisions.push(Decision {
            id: "SEEDED".to_string(),
            workload: "harness",
            metric: "seeded known failure".to_string(),
            threshold: "never passes".to_string(),
            observed: json!("PANEFLOW_BENCH_SEED_FAILURE"),
            result: "fail",
            reason: "seeded failure proves the nonzero exit and artifact retention path"
                .to_string(),
        });
    }
}

pub fn prior_failures() -> Vec<Value> {
    let Some(path) = std::env::var_os("PANEFLOW_BENCH_PRIOR_RESULT") else {
        return Vec::new();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return vec![json!({"prior": Path::new(&path).display().to_string(), "unreadable": true})];
    };
    let Ok(prior) = serde_json::from_slice::<Value>(&bytes) else {
        return vec![json!({"prior": Path::new(&path).display().to_string(), "not_json": true})];
    };
    prior["thresholds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|decision| decision["result"] == "fail")
        .cloned()
        .chain(
            prior["prior_failures"]
                .as_array()
                .into_iter()
                .flatten()
                .cloned(),
        )
        .collect()
}

pub fn verdict(decisions: &[Decision], prior: &[Value]) -> Result<(), Vec<String>> {
    let mut failures: Vec<String> = decisions
        .iter()
        .filter(|decision| decision.result == "fail")
        .map(|decision| {
            format!(
                "{} ({}): {}",
                decision.id, decision.workload, decision.reason
            )
        })
        .collect();
    failures.extend(
        prior
            .iter()
            .map(|failure| format!("retained first failure: {failure}")),
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures)
    }
}

pub fn stats(samples: &[f64], minimum: usize) -> Value {
    let mut sorted: Vec<f64> = samples.iter().copied().filter(|v| v.is_finite()).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let percentile = |p: f64| -> Option<f64> {
        if sorted.is_empty() || sorted.len() < minimum {
            return None;
        }
        let rank = ((sorted.len() as f64 - 1.0) * p).round() as usize;
        sorted.get(rank).copied()
    };
    json!({
        "count": sorted.len(),
        "required_count": minimum,
        "insufficient": sorted.len() < minimum,
        "median": percentile(0.5),
        "p95": percentile(0.95),
        "p99": percentile(0.99),
        "max": sorted.last().copied(),
        "min": sorted.first().copied(),
        "raw": samples,
    })
}

pub fn percentile(samples: &[f64], p: f64, minimum: usize) -> Option<f64> {
    stats(samples, minimum)[match p {
        p if p >= 0.99 => "p99",
        p if p >= 0.95 => "p95",
        _ => "median",
    }]
    .as_f64()
}

pub struct FixtureLedger {
    pub owned: Mutex<Vec<(SessionId, ProcessIdentity)>>,
}

impl FixtureLedger {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            owned: Mutex::new(Vec::new()),
        })
    }

    pub fn record(&self, client: &mut HostClient, session: &SessionId) {
        if let Ok(summary) = client.inspect(session)
            && let Some(process) = summary.manifest.process
        {
            self.owned
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((session.clone(), process));
        }
    }

    pub fn survivors(&self) -> Vec<Value> {
        self.owned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, process)| process.is_provably_live())
            .map(|(session, process)| json!({"session": session, "pid": process.pid, "started_at": process.started_at}))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.owned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

pub fn fixture(mode: &[&str]) -> CreateSession {
    CreateSession {
        shell: Some(super::fixture_executable().display().to_string()),
        args: mode.iter().map(|arg| arg.to_string()).collect(),
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(80),
        rows: Some(24),
        ..CreateSession::default()
    }
}

fn wait_for_marker(client: &mut HostClient, session: &SessionId, marker: &str, deadline: Duration) {
    let until = Instant::now() + deadline;
    let mut offset = 0u64;
    let mut collected = Vec::new();
    loop {
        if let Ok(end) = client.output(
            session,
            None,
            offset,
            false,
            |_, bytes| {
                collected.extend_from_slice(bytes);
                true
            },
            || true,
        ) {
            offset = end.next_offset;
        }
        if String::from_utf8_lossy(&collected).contains(marker) {
            return;
        }
        assert!(
            Instant::now() < until,
            "marker {marker} did not appear within {deadline:?} ({} bytes read)",
            collected.len()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn timed_attach(endpoint: &Path, session: &SessionId) -> (f64, usize, u64) {
    let mut client = HostClient::connect(endpoint, &ClientHello::local("w02-attach")).unwrap();
    let started = Instant::now();
    let attachment = client.attach(session, None).unwrap();
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let bytes = attachment.checkpoint.snapshot.len();
    let hash = fnv1a64(&attachment.checkpoint.snapshot);
    (elapsed, bytes, hash)
}

fn staging_report(client: &mut HostClient) -> Value {
    client
        .call("host.status", json!({}))
        .map(|status| status["resources"].clone())
        .unwrap_or(Value::Null)
}

pub fn workload_history(
    client: &mut HostClient,
    endpoint: &Path,
    ledger: &FixtureLedger,
    decisions: &mut Vec<Decision>,
) -> Value {
    let lines = HISTORY_LINES.to_string();
    let created = client
        .create(&fixture(&["history", &lines]))
        .expect("the history fixture starts");
    let session = created.manifest.session;
    ledger.record(client, &session);
    wait_for_marker(
        client,
        &session,
        "fixture history done",
        Duration::from_secs(60),
    );
    std::thread::sleep(Duration::from_millis(500));

    let mut sequential = Vec::new();
    let mut bytes = Vec::new();
    let mut hashes = Vec::new();
    for _ in 0..W02_REATTACHMENTS {
        let (ms, size, hash) = timed_attach(endpoint, &session);
        sequential.push(ms);
        bytes.push(size);
        hashes.push(hash);
    }
    let concurrent_started = Instant::now();
    let workers: Vec<_> = (0..W02_CONCURRENT)
        .map(|_| {
            let endpoint = endpoint.to_path_buf();
            let session = session.clone();
            std::thread::spawn(move || timed_attach(&endpoint, &session))
        })
        .collect();
    let concurrent: Vec<(f64, usize, u64)> =
        workers.into_iter().map(|w| w.join().unwrap()).collect();
    let concurrent_total_ms = concurrent_started.elapsed().as_secs_f64() * 1000.0;
    let mut cycles = Vec::new();
    for _ in 0..W02_CYCLES {
        let (ms, _, hash) = timed_attach(endpoint, &session);
        cycles.push(ms);
        hashes.push(hash);
    }
    std::thread::sleep(Duration::from_millis(200));
    let resources = staging_report(client);
    let staged_after = resources["checkpoints"]["staged_bytes"].as_u64();
    let active_after = resources["checkpoints"]["active"].as_u64();
    let peak = resources["checkpoints"]["peak_staged_bytes"].as_u64();
    let equivalent = hashes.windows(2).all(|pair| pair[0] == pair[1]);
    let sequential_p95 = percentile(&sequential, 0.95, W02_REATTACHMENTS);
    decisions.push(decide(
        "NFR-09.attach_p95",
        "W02",
        "sequential reattachment p95 ms",
        &format!("<= {NFR09_ATTACH_P95_MS}"),
        sequential_p95,
        |v| v <= NFR09_ATTACH_P95_MS,
        "fewer than ten reattachment samples",
    ));
    decisions.push(decide(
        "NFR-09.concurrent_total",
        "W02",
        "ten simultaneous attachments total ms",
        &format!("<= {NFR09_CONCURRENT_TOTAL_MS}"),
        Some(concurrent_total_ms),
        |v| v <= NFR09_CONCURRENT_TOTAL_MS,
        "",
    ));
    decisions.push(decide(
        "NFR-06.checkpoint_release",
        "W02",
        "staged checkpoint bytes after 120 attachments",
        "== 0 with 0 active captures",
        staged_after
            .zip(active_after)
            .map(|(bytes, active)| (bytes + active) as f64),
        |v| v == 0.0,
        "host.status did not report checkpoint staging",
    ));
    decisions.push(decide(
        "W02.content_equivalence",
        "W02",
        "identical checkpoint bytes across attachments",
        "all 120 snapshot hashes equal",
        Some(if equivalent { 1.0 } else { 0.0 }),
        |v| v == 1.0,
        "",
    ));
    client.stop(&session, None).unwrap();
    json!({
        "status": "measured",
        "session": session,
        "history_lines": HISTORY_LINES,
        "grid": "80x24",
        "sequential_attach_ms": stats(&sequential, W02_REATTACHMENTS),
        "checkpoint_bytes": bytes,
        "concurrent_attach_ms": stats(&concurrent.iter().map(|c| c.0).collect::<Vec<_>>(), W02_CONCURRENT),
        "concurrent_total_ms": concurrent_total_ms,
        "attach_detach_cycles_ms": stats(&cycles, W02_CYCLES),
        "content_equivalent": equivalent,
        "checkpoint_staging_after": resources["checkpoints"],
        "peak_staged_bytes": peak,
        "continuation_and_effects": {
            "covered_by": [
                "paneflow-host server::tests::a_follower_resumes_after_the_checkpoint_survives_idle_keepalives_and_sees_the_exit",
                "paneflow-app terminal::ghostty_session::tests::repeated_attach_and_detach_with_filled_scrollback_retains_no_checkpoint_bytes",
            ],
            "note": "side-effect suppression on replay is asserted by the desktop test suite, not re-measured here",
        },
    })
}

struct EchoProbe {
    session: SessionId,
    generation: paneflow_host::SessionGeneration,
    received: Arc<Mutex<Vec<(u64, Instant)>>>,
    stop: Arc<AtomicBool>,
    finished: std::sync::mpsc::Receiver<Result<(), String>>,
}

impl EchoProbe {
    fn start(client: &mut HostClient, endpoint: &Path, ledger: &FixtureLedger) -> Self {
        let created = client.create(&fixture(&["echo"])).expect("echo fixture");
        let session = created.manifest.session;
        ledger.record(client, &session);
        wait_for_marker(client, &session, "fixture echo", Duration::from_secs(30));
        let mut follower = HostClient::connect(endpoint, &ClientHello::local("w03-echo")).unwrap();
        let attachment = follower.attach(&session, None).unwrap();
        let generation = attachment.checkpoint.generation;
        let offset = attachment.checkpoint.offset;
        drop(attachment);
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (tx, finished) = std::sync::mpsc::channel();
        let followed = session.clone();
        std::thread::Builder::new()
            .name("baseline-follower-echo".into())
            .spawn(move || {
                let mut pending = Vec::new();
                let result = follower
                    .output(
                        &followed,
                        Some(generation),
                        offset,
                        true,
                        |_, data| {
                            let now = Instant::now();
                            pending.extend_from_slice(data);
                            while let Some(start) = pending.windows(7).position(|w| w == b"PFECHO-")
                            {
                                let Some(end) = pending[start..].iter().position(|b| *b == b'\r')
                                else {
                                    break;
                                };
                                let number = std::str::from_utf8(&pending[start + 7..start + end])
                                    .ok()
                                    .and_then(|s| s.parse::<u64>().ok());
                                if let Some(number) = number {
                                    sink.lock().unwrap().push((number, now));
                                }
                                pending.drain(..start + end + 1);
                            }
                            true
                        },
                        || !stopping.load(Ordering::Acquire),
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            })
            .unwrap();
        Self {
            session,
            generation,
            received,
            stop,
            finished,
        }
    }

    fn measure(&self, client: &mut HostClient, samples: usize) -> Vec<f64> {
        let mut sent = Vec::with_capacity(samples);
        for number in 0..samples as u64 {
            let line = format!("PFECHO-{number}\r");
            let at = Instant::now();
            client
                .input(&self.session, self.generation, line.as_bytes())
                .expect("echo input accepted");
            sent.push((number, at));
            std::thread::sleep(Duration::from_millis(10));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.received.lock().unwrap().len() < samples && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let received = self.received.lock().unwrap();
        let latencies: Vec<f64> = sent
            .iter()
            .filter_map(|(number, at)| {
                received
                    .iter()
                    .find(|(seen, _)| seen == number)
                    .map(|(_, when)| when.saturating_duration_since(*at).as_secs_f64() * 1000.0)
            })
            .collect();
        drop(received);
        self.received.lock().unwrap().clear();
        latencies
    }

    fn finish(self, client: &mut HostClient) {
        self.stop.store(true, Ordering::Release);
        let _ = self.finished.recv_timeout(Duration::from_secs(5));
        let _ = client.stop(&self.session, Some(self.generation));
    }
}

struct StreamFollower {
    bytes: Arc<Mutex<u64>>,
    stop: Arc<AtomicBool>,
    finished: std::sync::mpsc::Receiver<Result<(), String>>,
    session: SessionId,
}

impl StreamFollower {
    fn start(endpoint: &Path, session: &SessionId) -> Self {
        let mut follower =
            HostClient::connect(endpoint, &ClientHello::local("w03-stream")).unwrap();
        let attachment = follower.attach(session, None).unwrap();
        let generation = attachment.checkpoint.generation;
        let offset = attachment.checkpoint.offset;
        drop(attachment);
        let bytes = Arc::new(Mutex::new(0u64));
        let counter = Arc::clone(&bytes);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let (tx, finished) = std::sync::mpsc::channel();
        let followed = session.clone();
        std::thread::Builder::new()
            .name("baseline-follower-stream".into())
            .spawn(move || {
                let result = follower
                    .output(
                        &followed,
                        Some(generation),
                        offset,
                        true,
                        |_, data| {
                            *counter.lock().unwrap() += data.len() as u64;
                            true
                        },
                        || !stopping.load(Ordering::Acquire),
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string());
                let _ = tx.send(result);
            })
            .unwrap();
        Self {
            bytes,
            stop,
            finished,
            session: session.clone(),
        }
    }

    fn received(&self) -> u64 {
        *self.bytes.lock().unwrap()
    }

    fn finish(self) -> Result<(), String> {
        self.stop.store(true, Ordering::Release);
        self.finished
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "stream follower watchdog".to_string())?
    }
}

pub fn workload_throughput(
    client: &mut HostClient,
    endpoint: &Path,
    ledger: &FixtureLedger,
    protocol: &Protocol,
    baseline_throughput: Option<f64>,
    decisions: &mut Vec<Decision>,
) -> Value {
    let flood_bytes = W03_SINGLE_FLOOD_BYTES.to_string();
    let single = client
        .create(&fixture(&["flood", &flood_bytes]))
        .expect("flood fixture");
    let single_id = single.manifest.session.clone();
    ledger.record(client, &single_id);
    let follower = StreamFollower::start(endpoint, &single_id);
    let started = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let summary = client.inspect(&single_id).unwrap();
        if !summary.live && !summary.manifest.lifecycle.is_running() {
            break;
        }
        assert!(Instant::now() < deadline, "single flood watchdog");
        std::thread::sleep(Duration::from_millis(50));
    }
    let single_elapsed = started.elapsed().as_secs_f64();
    let single_received = follower.received();
    let _ = follower.finish();
    let single_mib_per_s = (single_received as f64 / (1024.0 * 1024.0)) / single_elapsed;

    let echo = EchoProbe::start(client, endpoint, ledger);
    let idle_latencies = echo.measure(client, protocol.echo_samples);

    let seconds = protocol.stream_seconds.to_string();
    let rate = W03_RATE_BYTES_PER_SECOND.to_string();
    let mut streams = Vec::new();
    for _ in 0..W03_STREAMS {
        let created = client
            .create(&fixture(&["stream", &rate, &seconds]))
            .expect("stream fixture");
        ledger.record(client, &created.manifest.session);
        streams.push(StreamFollower::start(endpoint, &created.manifest.session));
    }
    let loaded_started = Instant::now();
    let loaded_latencies = echo.measure(client, protocol.echo_samples);
    let remaining = Duration::from_secs(protocol.stream_seconds)
        .saturating_sub(loaded_started.elapsed())
        + Duration::from_secs(2);
    std::thread::sleep(remaining);
    let mut per_stream = Vec::new();
    for stream in streams {
        let received = stream.received();
        let session = stream.session.clone();
        let ended = stream.finish();
        let _ = client.stop(&session, None);
        per_stream
            .push(json!({"session": session, "received_bytes": received, "follower": ended.err()}));
    }
    echo.finish(client);
    let received: Vec<f64> = per_stream
        .iter()
        .filter_map(|s| s["received_bytes"].as_u64())
        .map(|b| b as f64)
        .collect();
    let expected = (W03_RATE_BYTES_PER_SECOND * protocol.stream_seconds) as f64;
    let fairness = received
        .iter()
        .cloned()
        .fold(None, |acc: Option<(f64, f64)>, v| match acc {
            None => Some((v, v)),
            Some((lo, hi)) => Some((lo.min(v), hi.max(v))),
        })
        .map(|(lo, hi)| if hi > 0.0 { lo / hi } else { 0.0 });
    let idle_p95 = percentile(&idle_latencies, 0.95, protocol.echo_samples);
    let idle_p99 = percentile(&idle_latencies, 0.99, protocol.echo_samples);
    let loaded_p95 = percentile(&loaded_latencies, 0.95, protocol.echo_samples);
    decisions.push(decide(
        "NFR-08.idle_p95",
        "W03",
        "idle input-to-publication p95 ms",
        &format!("<= {NFR08_IDLE_P95_MS}"),
        idle_p95,
        |v| v <= NFR08_IDLE_P95_MS,
        "fewer than the required echo samples",
    ));
    decisions.push(decide(
        "NFR-08.idle_p99",
        "W03",
        "idle input-to-publication p99 ms",
        &format!("<= {NFR08_IDLE_P99_MS}"),
        idle_p99,
        |v| v <= NFR08_IDLE_P99_MS,
        "fewer than the required echo samples",
    ));
    decisions.push(decide(
        "NFR-08.loaded_p95",
        "W03",
        "focused echo p95 ms with ten 1 MiB/s streams",
        &format!("<= {NFR08_LOADED_P95_MS}"),
        loaded_p95,
        |v| v <= NFR08_LOADED_P95_MS,
        "fewer than the required echo samples",
    ));
    decisions.push(decide(
        "NFR-08.throughput_ratio",
        "W03",
        "single flood MiB/s relative to the matched baseline",
        &format!(">= {NFR08_THROUGHPUT_RATIO}"),
        baseline_throughput.map(|base| single_mib_per_s / base),
        |v| v >= NFR08_THROUGHPUT_RATIO,
        "no matched baseline throughput on this machine and topology",
    ));
    json!({
        "status": "measured",
        "protocol": protocol.label,
        "acceptance_grade": protocol.acceptance_grade,
        "single_flood": {"bytes": W03_SINGLE_FLOOD_BYTES, "received_bytes": single_received, "elapsed_s": single_elapsed, "mib_per_s": single_mib_per_s},
        "idle_echo_ms": stats(&idle_latencies, protocol.echo_samples),
        "loaded_echo_ms": stats(&loaded_latencies, protocol.echo_samples),
        "streams": {"count": W03_STREAMS, "rate_bytes_per_s": W03_RATE_BYTES_PER_SECOND, "seconds": protocol.stream_seconds, "expected_bytes_each": expected, "per_stream": per_stream, "fairness_min_over_max": fairness},
        "timestamp_boundary": "local monotonic clock from session.input acceptance to follower receipt of the published bytes; no display time",
        "graphics_long_line_corpus": {"status": "pending", "reason": "the bounded graphics and long-line corpus is exercised by scripts/bench-terminal, not by this host-path run"},
    })
}

pub fn workload_churn(
    client: &mut HostClient,
    endpoint: &Path,
    host_pid: u32,
    ledger: &FixtureLedger,
    protocol: &Protocol,
    decisions: &mut Vec<Decision>,
) -> Value {
    let flood = W05_FLOOD_BYTES.to_string();
    let mut batches = Vec::new();
    let mut reclaim_ms = Vec::new();
    for batch in 0..W05_BATCHES {
        let batch_started = Instant::now();
        let mut sessions = Vec::with_capacity(W05_BATCH_SIZE);
        for _ in 0..W05_BATCH_SIZE {
            let created = client
                .create(&fixture(&["flood", &flood]))
                .expect("churn fixture");
            ledger.record(client, &created.manifest.session);
            sessions.push(created.manifest.session);
        }
        for session in &sessions {
            let mut follower =
                HostClient::connect(endpoint, &ClientHello::local("w05-attach")).unwrap();
            let _ = follower.attach(session, None);
        }
        let exit_deadline = Instant::now() + Duration::from_secs(120);
        let exited_at;
        loop {
            let all_exited = sessions
                .iter()
                .all(|session| client.inspect(session).is_ok_and(|s| !s.live));
            if all_exited {
                exited_at = Some(Instant::now());
                break;
            }
            assert!(
                Instant::now() < exit_deadline,
                "churn batch {batch} exit watchdog"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let reclaim_deadline = Instant::now() + Duration::from_secs(NFR04_RECLAIM_S as u64 + 1);
        let mut reclaimed = None;
        let mut unreclaimed = Value::Null;
        loop {
            let status = client.call("host.status", json!({})).unwrap();
            let live = status["resources"]["live_runtimes"]
                .as_u64()
                .unwrap_or(u64::MAX);
            let held: Vec<Value> = status["resources"]["sessions"]
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| sessions.iter().any(|s| entry["session"] == json!(s)))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if live == 0 && held.is_empty() {
                reclaimed = Some(
                    exited_at
                        .map(|at| at.elapsed().as_secs_f64() * 1000.0)
                        .unwrap_or(0.0),
                );
                break;
            }
            if Instant::now() >= reclaim_deadline {
                let summaries: Vec<Value> = held
                    .iter()
                    .filter_map(|entry| {
                        serde_json::from_value::<SessionId>(entry["session"].clone()).ok()
                    })
                    .filter_map(|id| client.inspect(&id).ok())
                    .map(|summary| {
                        json!({
                            "session": summary.manifest.session,
                            "lifecycle": summary.manifest.lifecycle,
                            "live": summary.live,
                            "descendants_unresolved": summary.descendants_unresolved,
                            "durability_error": summary.durability_error,
                        })
                    })
                    .collect();
                unreclaimed = json!({
                    "live_runtimes": live,
                    "held_runtimes": held,
                    "summaries": summaries,
                });
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if let Some(ms) = reclaimed {
            reclaim_ms.push(ms);
        }
        let counters = super::process_counters(host_pid);
        batches.push(json!({
            "batch": batch,
            "sessions": W05_BATCH_SIZE,
            "elapsed_ms": batch_started.elapsed().as_secs_f64() * 1000.0,
            "runtime_reclaimed_ms": reclaimed,
            "unreclaimed": unreclaimed,
            "host_resident_bytes": super::resident_bytes(host_pid),
            "host_threads": counters.0,
            "host_handles": counters.1,
        }));
    }
    std::thread::sleep(protocol.quiescence);
    let final_counters = super::process_counters(host_pid);
    let final_rss = super::resident_bytes(host_pid);
    let warmed = batches.first();
    let warmed_rss = warmed.and_then(|b| b["host_resident_bytes"].as_u64());
    let warmed_threads = warmed.and_then(|b| b["host_threads"].as_u64());
    let warmed_handles = warmed.and_then(|b| b["host_handles"].as_u64());
    let tail: Vec<(f64, f64)> = batches
        .iter()
        .rev()
        .take(5)
        .rev()
        .filter_map(|b| {
            Some((
                b["batch"].as_u64()? as f64,
                b["host_resident_bytes"].as_u64()? as f64,
            ))
        })
        .collect();
    let slope = (tail.len() >= 2).then(|| {
        let n = tail.len() as f64;
        let mean_x = tail.iter().map(|(x, _)| x).sum::<f64>() / n;
        let mean_y = tail.iter().map(|(_, y)| y).sum::<f64>() / n;
        let cov: f64 = tail.iter().map(|(x, y)| (x - mean_x) * (y - mean_y)).sum();
        let var: f64 = tail.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
        if var == 0.0 { 0.0 } else { cov / var }
    });
    let all_reclaimed = reclaim_ms.len() == W05_BATCHES;
    decisions.push(decide(
        "NFR-04.runtime_release",
        "W05",
        "batches whose runtimes released within the reclaim budget",
        &format!("all {W05_BATCHES} within {NFR04_RECLAIM_S} s"),
        Some(reclaim_ms.len() as f64),
        |v| v == W05_BATCHES as f64,
        "",
    ));
    decisions.push(decide(
        "NFR-04.reclaim_max_ms",
        "W05",
        "slowest runtime release after confirmed exit, ms",
        &format!("<= {}", NFR04_RECLAIM_S * 1000.0),
        all_reclaimed.then(|| reclaim_ms.iter().cloned().fold(0.0, f64::max)),
        |v| v <= NFR04_RECLAIM_S * 1000.0,
        "a batch never reported zero live runtimes",
    ));
    let allowance = warmed_rss.map(|w| (w as f64 * 0.10).max(NFR05_FLOOR_BYTES));
    decisions.push(decide(
        "NFR-05.memory_after_churn",
        "W05",
        "host resident bytes after churn and quiescence minus the warmed baseline",
        "<= max(16 MiB, 10% of warmed)",
        final_rss.zip(warmed_rss).map(|(f, w)| f as f64 - w as f64),
        |v| allowance.is_some_and(|a| v <= a),
        "resident memory is unavailable on this platform sampler",
    ));
    decisions.push(decide(
        "NFR-05.memory_slope",
        "W05",
        "resident-memory slope over the last five batches, bytes per batch",
        &format!("<= {NFR05_SLOPE_BYTES_PER_BATCH}"),
        slope,
        |v| v <= NFR05_SLOPE_BYTES_PER_BATCH,
        "fewer than two resident samples",
    ));
    decisions.push(decide(
        "NFR-05.threads",
        "W05",
        "host threads after churn minus warmed baseline",
        &format!("<= {NFR05_COUNTER_SLACK}"),
        final_counters
            .0
            .zip(warmed_threads)
            .map(|(f, w)| f as f64 - w as f64),
        |v| v <= NFR05_COUNTER_SLACK as f64,
        "thread count is unavailable on this platform sampler",
    ));
    decisions.push(decide(
        "NFR-05.handles",
        "W05",
        "host handles or file descriptors after churn minus warmed baseline",
        &format!("<= {NFR05_COUNTER_SLACK}"),
        final_counters
            .1
            .zip(warmed_handles)
            .map(|(f, w)| f as f64 - w as f64),
        |v| v <= NFR05_COUNTER_SLACK as f64,
        "handle or descriptor count is unavailable on this platform sampler",
    ));
    json!({
        "status": "measured",
        "protocol": protocol.label,
        "acceptance_grade": protocol.acceptance_grade,
        "batches": batches,
        "quiescence_s": protocol.quiescence.as_secs_f64(),
        "final": {"host_resident_bytes": final_rss, "host_threads": final_counters.0, "host_handles": final_counters.1},
        "warmed_baseline": {"host_resident_bytes": warmed_rss, "host_threads": warmed_threads, "host_handles": warmed_handles},
        "slope_bytes_per_batch_last_5": slope,
        "runtime_reclaimed_ms": stats(&reclaim_ms, W05_BATCHES),
        "cold_records_retained": W05_BATCHES * W05_BATCH_SIZE,
        "cold_retention_with_injected_time": {"status": "automated_tests", "covered_by": ["paneflow-host server::tests::records_past_their_retention_release_their_cold_text_under_an_injected_clock"], "note": "retention aging cannot be injected over IPC; the unit suite drives trim_terminated_records_at with a clock"},
        "stale_callback_rejection": {"status": "automated_tests", "covered_by": ["paneflow-host host::tests::a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation", "paneflow-host host::tests::a_scan_waiting_to_persist_cannot_recreate_a_removed_record", "paneflow-host persistence::tests::a_revision_queued_before_removal_cannot_resurrect_the_record"]},
    })
}

#[allow(clippy::too_many_arguments)]
pub fn workload_worker_replacement(
    home: &Path,
    worker: &mut Option<super::WorkerProcess>,
    client: &mut HostClient,
    endpoint: &Path,
    open: &[SessionId],
    ledger: &FixtureLedger,
    protocol: &Protocol,
    decisions: &mut Vec<Decision>,
) -> Value {
    let Some(current) = worker.take() else {
        return json!({"status": "pending", "reason": "PANEFLOW_BENCH_CONTROLLER was not supplied; worker crash, restart, and replacement cycles need the existing worker"});
    };
    let replacement = std::env::var_os("PANEFLOW_BENCH_CONTROLLER_REPLACEMENT");
    let executable = std::env::var_os("PANEFLOW_BENCH_CONTROLLER").unwrap();
    let echo = EchoProbe::start(client, endpoint, ledger);
    let before: Vec<_> = open
        .iter()
        .chain(std::iter::once(&echo.session))
        .map(|s| client.inspect(s).unwrap().manifest)
        .collect();
    let mut cycles = Vec::new();
    let mut live = current;
    let mut unchanged = true;
    for cycle in 0..protocol.worker_cycles {
        let build_replacement = replacement.is_some() && cycle % 2 == 1;
        let crash = cycle % 2 == 0;
        let started = Instant::now();
        if crash {
            let _ = live.child.kill();
            let _ = live.child.wait();
            std::mem::forget(live);
        } else {
            drop(live);
        }
        let next = if build_replacement {
            replacement.clone().unwrap()
        } else {
            executable.clone()
        };
        live = super::WorkerProcess::start_enabled(home, next);
        let restart_ms = started.elapsed().as_secs_f64() * 1000.0;
        let status = paneflow_ipc_client::host_control::HostControl::connect_with_deadline(
            &live.endpoint,
            "persistent-bench",
            Duration::from_secs(5),
        )
        .and_then(|mut control| control.request("worker.status", json!({})));
        let projection = status
            .as_ref()
            .ok()
            .and_then(|s| s["session_count"].as_u64());
        let core_connected = status
            .as_ref()
            .ok()
            .and_then(|s| s["core_connected"].as_bool());
        let after: Vec<_> = before
            .iter()
            .map(|m| client.inspect(&m.session).unwrap().manifest)
            .collect();
        let same = before.iter().zip(&after).all(|(b, a)| {
            b.generation == a.generation && b.process == a.process && a.lifecycle.is_running()
        });
        unchanged &= same;
        let latencies = echo.measure(client, 20);
        cycles.push(json!({
            "cycle": cycle,
            "kind": if crash { "crash" } else { "deliberate restart" },
            "build_replacement": build_replacement,
            "restart_ms": restart_ms,
            "worker_status": status.as_ref().ok(),
            "worker_error": status.as_ref().err(),
            "projection_sessions": projection,
            "core_connected": core_connected,
            "identities_unchanged": same,
            "echo_after_ms": stats(&latencies, 20),
        }));
    }
    let identity_count = before.len();
    let after_total: Vec<_> = before
        .iter()
        .map(|m| client.inspect(&m.session).unwrap().manifest.generation)
        .collect();
    let generations_unchanged = before
        .iter()
        .zip(&after_total)
        .all(|(b, a)| b.generation == *a);
    echo.finish(client);
    *worker = Some(live);
    decisions.push(decide(
        "NFR-11.worker_cycles",
        "W04",
        "worker cycles with unchanged child identities and generations",
        &format!("all {} cycles", protocol.worker_cycles),
        Some(
            cycles
                .iter()
                .filter(|c| c["identities_unchanged"] == true)
                .count() as f64,
        ),
        |v| v == protocol.worker_cycles as f64,
        "",
    ));
    json!({
        "status": "measured",
        "protocol": protocol.label,
        "acceptance_grade": protocol.acceptance_grade,
        "cycles": cycles,
        "sessions_observed": identity_count,
        "identities_unchanged": unchanged,
        "generations_unchanged": generations_unchanged,
        "build_replacement": if replacement.is_some() { json!("PANEFLOW_BENCH_CONTROLLER_REPLACEMENT alternated on odd cycles") } else { json!({"status": "pending", "reason": "pass PANEFLOW_BENCH_CONTROLLER_REPLACEMENT with a second candidate build to alternate a build replacement"}) },
        "no_pty_restart": "generation and process identity equality prove the worker rebuilt its projection without a restart RPC",
        "reducer_certification": "not claimed; the Agent Runtime System PRD owns the reducer",
    })
}

pub fn automated_only(workload: &str, covered_by: &[&str], manual: &[&str]) -> Value {
    json!({
        "status": if manual.is_empty() { "automated_tests" } else { "manual" },
        "workload": workload,
        "covered_by": covered_by,
        "manual_cells": manual,
        "note": "automated cases run in cargo test on every shipping triple; manual cells are executed per the qualification runbook and are pending until recorded",
    })
}
