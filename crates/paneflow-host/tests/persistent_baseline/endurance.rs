use super::*;

pub(super) const ENDURANCE_RETAINED_IDLE: usize = 9;
pub(super) const ENDURANCE_BURST_SIZE: usize = 10;

pub(super) struct EndurancePlan {
    pub(super) duration: Duration,
    pub(super) idle: Duration,
    required: Duration,
    pub(super) worker_cycles: usize,
    pub(super) desktop_cycles: usize,
    pub(super) burst_interval: Duration,
    pub(super) sample_interval: Duration,
}

impl EndurancePlan {
    pub(super) fn from_env() -> Self {
        let env_u64 = |name: &str, default: u64| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(default)
        };
        Self {
            duration: Duration::from_secs(60 * env_u64("PANEFLOW_BENCH_ENDURANCE_MINUTES", 480)),
            idle: Duration::from_secs(60 * env_u64("PANEFLOW_BENCH_IDLE_MINUTES", 30)),
            required: Duration::from_secs(
                60 * env_u64("PANEFLOW_BENCH_ENDURANCE_REQUIRED_MINUTES", 480),
            ),
            worker_cycles: env_u64("PANEFLOW_BENCH_WORKER_CYCLES", 100) as usize,
            desktop_cycles: env_u64("PANEFLOW_BENCH_DESKTOP_CYCLES", 100) as usize,
            burst_interval: Duration::from_secs(60 * env_u64("PANEFLOW_BENCH_BURST_MINUTES", 5)),
            sample_interval: Duration::from_secs(env_u64("PANEFLOW_BENCH_SAMPLE_SECONDS", 60)),
        }
    }

    pub(super) fn acceptance_grade(&self, desktop: bool) -> bool {
        self.duration >= self.required
            && self.idle >= Duration::from_secs(30 * 60)
            && self.worker_cycles >= 100
            && desktop
            && self.desktop_cycles >= 100
    }

    pub(super) fn to_json(&self, desktop: bool) -> Value {
        json!({
            "duration_s": self.duration.as_secs(),
            "required_duration_s": self.required.as_secs(),
            "idle_s": self.idle.as_secs(),
            "worker_cycles": self.worker_cycles,
            "desktop_cycles": self.desktop_cycles,
            "burst_interval_s": self.burst_interval.as_secs(),
            "burst_size": ENDURANCE_BURST_SIZE,
            "sample_interval_s": self.sample_interval.as_secs(),
            "retained_sessions": ENDURANCE_RETAINED_IDLE + 1,
            "acceptance_grade": self.acceptance_grade(desktop),
            "label": if self.acceptance_grade(desktop) { "endurance" } else { "rehearsal" },
        })
    }
}

pub(super) fn retained_identities(
    client: &mut HostClient,
    sessions: &[SessionId],
) -> Vec<paneflow_host::SessionManifest> {
    sessions
        .iter()
        .map(|session| client.inspect(session).unwrap().manifest)
        .collect()
}

pub(super) fn identities_unchanged(
    client: &mut HostClient,
    before: &[paneflow_host::SessionManifest],
) -> bool {
    before.iter().all(|earlier| {
        client.inspect(&earlier.session).is_ok_and(|now| {
            now.live
                && now.manifest.generation == earlier.generation
                && now.manifest.process == earlier.process
        })
    })
}

pub(super) fn endurance_burst(
    client: &mut HostClient,
    endpoint: &Path,
    ledger: &FixtureLedger,
    retained: usize,
    index: usize,
) -> Value {
    let started = Instant::now();
    let flood = workloads::W05_FLOOD_BYTES.to_string();
    let mut sessions = Vec::with_capacity(ENDURANCE_BURST_SIZE);
    for _ in 0..ENDURANCE_BURST_SIZE {
        let created = client
            .create(&workloads::fixture(&["flood", &flood]))
            .expect("burst fixture");
        ledger.record(client, &created.manifest.session);
        sessions.push(created.manifest.session);
    }
    for session in &sessions {
        let mut follower = HostClient::connect(endpoint, &ClientHello::local("w08-burst")).unwrap();
        let _ = follower.attach(session, None);
    }
    let exit_deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if sessions
            .iter()
            .all(|session| client.inspect(session).is_ok_and(|summary| !summary.live))
        {
            break;
        }
        assert!(
            Instant::now() < exit_deadline,
            "endurance burst {index} exit watchdog"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let exited_at = Instant::now();
    let reclaim_deadline = exited_at + Duration::from_secs(workloads::NFR04_RECLAIM_S as u64 + 1);
    let mut reclaimed_ms = None;
    let mut unreclaimed = Value::Null;
    loop {
        let status = client.call("host.status", json!({})).unwrap();
        let live = status["resources"]["live_runtimes"]
            .as_u64()
            .unwrap_or(u64::MAX);
        let held = status["resources"]["sessions"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter(|entry| sessions.iter().any(|s| entry["session"] == json!(s)))
                    .count()
            })
            .unwrap_or(usize::MAX);
        if live == retained as u64 && held == 0 {
            reclaimed_ms = Some(exited_at.elapsed().as_secs_f64() * 1000.0);
            break;
        }
        if Instant::now() >= reclaim_deadline {
            unreclaimed = json!({"live_runtimes": live, "held_burst_runtimes": held});
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    for session in &sessions {
        let _ = client.remove(session);
    }
    json!({
        "burst": index,
        "sessions": ENDURANCE_BURST_SIZE,
        "elapsed_ms": started.elapsed().as_secs_f64() * 1000.0,
        "runtime_reclaimed_ms": reclaimed_ms,
        "unreclaimed": unreclaimed,
    })
}

pub(super) fn endurance_sample(
    client: &mut HostClient,
    host_pid: u32,
    worker_pid: Option<u32>,
    retained: &[SessionId],
    elapsed: Duration,
    phase: &str,
) -> Value {
    let status = client
        .call("host.status", json!({}))
        .map(|status| status["resources"].clone())
        .unwrap_or(Value::Null);
    let summaries: Vec<_> = retained
        .iter()
        .filter_map(|session| client.inspect(session).ok())
        .collect();
    let unresolved: usize = summaries
        .iter()
        .map(|summary| summary.descendants_unresolved)
        .sum();
    let durability_errors = summaries
        .iter()
        .filter(|summary| summary.durability_error.is_some())
        .count();
    let counters = process_counters(host_pid);
    json!({
        "elapsed_s": elapsed.as_secs(),
        "phase": phase,
        "host_resident_bytes": resident_bytes(host_pid),
        "host_threads": counters.0,
        "host_handles": counters.1,
        "worker_resident_bytes": worker_pid.and_then(resident_bytes),
        "live_runtimes": status["live_runtimes"],
        "pending_launches": status["pending_launches"],
        "persistence": status["persistence"],
        "checkpoints": status["checkpoints"],
        "retained_live": summaries.iter().filter(|summary| summary.live).count(),
        "retained_descendants_unresolved": unresolved,
        "retained_durability_errors": durability_errors,
    })
}

pub(super) fn compare_endurance(decisions: &[Decision]) -> String {
    let mut text = String::from("decision | observed | threshold | result\n");
    for decision in decisions {
        text.push_str(&format!(
            "{} | {} | {} | {}{}\n",
            decision.id,
            decision.observed,
            decision.threshold,
            decision.result,
            if decision.reason.is_empty() {
                String::new()
            } else {
                format!(" ({})", decision.reason)
            }
        ));
    }
    text
}
