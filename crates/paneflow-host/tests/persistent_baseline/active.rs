use super::*;

use paneflow_host::work_counters::{
    CounterSample, DESKTOP_COUNTERS, HOST_COUNTERS, Reading, WORKER_COUNTERS, Window,
};

pub(super) const ACTIVE_SCENARIOS: [usize; 3] = [1, 4, 8];
pub(super) const ACTIVE_WINDOW: Duration = Duration::from_secs(30);
pub(super) const STREAM_ARGS: [&str; 3] = ["stream", "16384", "60"];
pub(super) const FLOOD_ARGS: [&str; 2] = ["flood", "8388608"];
pub(super) const ACTIVE_ECHO_SAMPLES: usize = 200;

pub(super) struct ActivePlan<'a> {
    pub(super) streams: usize,
    pub(super) stream_args: &'a [&'a str],
    pub(super) flood_args: Option<&'a [&'a str]>,
    pub(super) settle: Duration,
    pub(super) window: Duration,
}

pub(super) struct ActiveProcesses<'a> {
    pub(super) host_pid: u32,
    pub(super) worker: Option<&'a WorkerProcess>,
    pub(super) desktop_home: Option<&'a Path>,
    pub(super) echo: Option<&'a workloads::EchoProbe>,
}

fn unavailable(process: &str, names: &[&str], reason: String) -> CounterSample {
    CounterSample {
        process: process.to_string(),
        identity: None,
        readings: names
            .iter()
            .map(|name| (name.to_string(), Reading::Pending(reason.clone())))
            .collect(),
    }
}

pub(super) fn host_counters(client: &mut HostClient) -> CounterSample {
    match client.call("host.status", json!({})) {
        Ok(status) => paneflow_host::work_counters::sample("host", &status, HOST_COUNTERS),
        Err(error) => unavailable(
            "host",
            HOST_COUNTERS,
            format!("host.status failed: {error}"),
        ),
    }
}

pub(super) fn worker_counters(worker: &WorkerProcess) -> CounterSample {
    let status = HostControl::connect_with_deadline(
        &worker.endpoint,
        "persistent-bench",
        Duration::from_secs(2),
    )
    .map_err(|error| error.to_string())
    .and_then(|mut control| {
        control
            .request("worker.status", json!({}))
            .map_err(|error| error.to_string())
    });
    match status {
        Ok(status) => paneflow_host::work_counters::sample("worker", &status, WORKER_COUNTERS),
        Err(error) => unavailable(
            "worker",
            WORKER_COUNTERS,
            format!("worker.status failed: {error}"),
        ),
    }
}

pub(super) fn desktop_counters(desktop: &DesktopProcess) -> CounterSample {
    match IpcClient::new(desktop.endpoint.clone()).call("system.counters", json!({})) {
        Ok(result) => paneflow_host::work_counters::sample("desktop", &result, DESKTOP_COUNTERS),
        Err(error) => unavailable(
            "desktop",
            DESKTOP_COUNTERS,
            format!("system.counters failed: {error:?}"),
        ),
    }
}

pub(super) fn reading_json(reading: &Reading) -> Value {
    match reading {
        Reading::Measured(value) => json!(value),
        Reading::Pending(reason) => json!({"pending": reason}),
    }
}

pub(super) fn window_json(before: &CounterSample, after: &CounterSample) -> Value {
    match paneflow_host::work_counters::window(before, after) {
        Window::Deltas(deltas) => Value::Object(
            deltas
                .iter()
                .map(|(name, delta)| (name.clone(), reading_json(delta)))
                .collect(),
        ),
        Window::Invalid(reason) => json!({"invalid": reason}),
    }
}

fn process_window(
    pid: u32,
    cpu: (&Attribution, &Attribution),
    counters: (&CounterSample, &CounterSample),
    window: Duration,
) -> Value {
    let mut sample = process_sample(pid, cpu.0, cpu.1, window);
    sample["work_counters"] = window_json(counters.0, counters.1);
    sample
}

pub(super) fn run_active_scenario(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    plan: &ActivePlan<'_>,
    processes: &ActiveProcesses<'_>,
) -> Result<Value, String> {
    if plan.streams == 0 {
        return Err("no active session was opened".to_string());
    }
    let mut streams = Vec::with_capacity(plan.streams);
    for _ in 0..plan.streams {
        let created = client
            .create(&workloads::fixture(plan.stream_args))
            .map_err(|error| format!("the stream fixture did not start: {error}"))?;
        ledger.record(client, &created.manifest.session);
        streams.push(created.manifest);
    }
    let flood = match plan.flood_args {
        Some(args) => {
            let created = client
                .create(&workloads::fixture(args))
                .map_err(|error| format!("the flood fixture did not start: {error}"))?;
            ledger.record(client, &created.manifest.session);
            Some(created.manifest.session)
        }
        None => None,
    };
    let mut sessions: Vec<SessionId> = streams.iter().map(|s| s.session.clone()).collect();
    sessions.extend(flood.iter().cloned());
    let desktop = processes
        .desktop_home
        .map(|home| DesktopProcess::start_listing(home, &sessions));
    std::thread::sleep(plan.settle);

    let worker_pid = processes.worker.map(|worker| worker.child.id());
    let desktop_pid = desktop.as_ref().map(|desktop| desktop.child.id());
    let host_counters_before = host_counters(client);
    let worker_counters_before = processes.worker.map(worker_counters);
    let desktop_counters_before = desktop.as_ref().map(desktop_counters);
    let host_cpu_before = thread_cpu(processes.host_pid);
    let worker_cpu_before = worker_pid.map(thread_cpu);
    let desktop_cpu_before = desktop_pid.map(thread_cpu);
    let started = Instant::now();
    std::thread::sleep(plan.window);
    let host_cpu_after = thread_cpu(processes.host_pid);
    let worker_cpu_after = worker_pid.map(thread_cpu);
    let desktop_cpu_after = desktop_pid.map(thread_cpu);
    let host_counters_after = host_counters(client);
    let worker_counters_after = processes.worker.map(worker_counters);
    let desktop_counters_after = desktop.as_ref().map(desktop_counters);
    let window = started.elapsed();
    let echo = match processes.echo {
        Some(probe) => workloads::stats(
            &probe.measure(client, ACTIVE_ECHO_SAMPLES),
            ACTIVE_ECHO_SAMPLES,
        ),
        None => json!({"pending": "no echo probe was opened"}),
    };

    let dead: Vec<String> = streams
        .iter()
        .filter(|manifest| {
            !client.inspect(&manifest.session).is_ok_and(|summary| {
                summary.live && summary.manifest.generation == manifest.generation
            })
        })
        .map(|manifest| manifest.session.to_string())
        .collect();
    let flood_live = flood
        .as_ref()
        .map(|session| client.inspect(session).is_ok_and(|summary| summary.live));
    drop(desktop);
    for session in &sessions {
        let _ = client.stop(session, None);
    }
    if !dead.is_empty() {
        return Err(format!(
            "fixture session {} died during the {:.1} s window; no average is published over fewer than {} sessions",
            dead.join(", "),
            window.as_secs_f64(),
            plan.streams
        ));
    }

    let worker = match (
        worker_pid,
        &worker_cpu_before,
        &worker_cpu_after,
        &worker_counters_before,
        &worker_counters_after,
    ) {
        (Some(pid), Some(cpu_before), Some(cpu_after), Some(before), Some(after)) => {
            process_window(pid, (cpu_before, cpu_after), (before, after), window)
        }
        _ => {
            json!({"pending": "PANEFLOW_BENCH_CONTROLLER was not supplied; the worker is unmeasured"})
        }
    };
    let desktop = match (
        desktop_pid,
        &desktop_cpu_before,
        &desktop_cpu_after,
        &desktop_counters_before,
        &desktop_counters_after,
    ) {
        (Some(pid), Some(cpu_before), Some(cpu_after), Some(before), Some(after)) => {
            process_window(pid, (cpu_before, cpu_after), (before, after), window)
        }
        _ => {
            json!({"pending": "PANEFLOW_BENCH_DESKTOP was not supplied; the desktop is unmeasured"})
        }
    };
    Ok(json!({
        "sessions": plan.streams,
        "stream_sessions": streams.iter().map(|manifest| manifest.session.to_string()).collect::<Vec<_>>(),
        "flood_session": flood,
        "flood_live_at_window_end": flood_live,
        "settle_s": plan.settle.as_secs_f64(),
        "window_s": window.as_secs_f64(),
        "host": process_window(
            processes.host_pid,
            (&host_cpu_before, &host_cpu_after),
            (&host_counters_before, &host_counters_after),
            window,
        ),
        "echo_ms": echo,
        "worker": worker,
        "desktop": desktop,
    }))
}

pub(super) fn schema_refusal(baseline: &Value, document: &Value) -> Option<String> {
    (baseline["schema_version"] != document["schema_version"]).then(|| {
        format!(
            "baseline schema {} differs from candidate schema {}; comparison refused, record a new baseline",
            baseline["schema_version"], document["schema_version"]
        )
    })
}

fn counter_verdict(candidate: &Value, baseline: Option<&Value>) -> String {
    match (candidate.as_u64(), baseline.and_then(Value::as_u64)) {
        (None, _) => format!(
            "pending: {}",
            candidate["pending"].as_str().unwrap_or("not measured")
        ),
        (Some(_), None) => "no baseline".to_string(),
        (Some(now), Some(before)) if now < before => "lower".to_string(),
        (Some(now), Some(before)) if now > before => "higher".to_string(),
        (Some(_), Some(_)) => "same".to_string(),
    }
}

pub(super) fn compare_active(document: &Value, baseline: Option<&Value>) -> String {
    let mut text = String::new();
    let refusal = baseline.and_then(|baseline| schema_refusal(baseline, document));
    match (baseline, &refusal) {
        (None, _) => text.push_str("no active baseline configured; counters are reported only\n"),
        (Some(_), Some(refusal)) => {
            text.push_str(refusal);
            text.push('\n');
        }
        (Some(_), None) => {}
    }
    let base = baseline.filter(|_| refusal.is_none());
    text.push_str(&format!(
        "{:<64} {:>12} {:>12} {}\n",
        "counter delta over the window", "candidate", "baseline", "verdict"
    ));
    for scenario in document["scenarios"].as_array().into_iter().flatten() {
        let sessions = &scenario["sessions"];
        if let Some(failed) = scenario["failed"].as_str() {
            text.push_str(&format!("{sessions} sessions: failed: {failed}\n"));
            continue;
        }
        let base_scenario = base.and_then(|base| {
            base["scenarios"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|candidate| candidate["sessions"] == *sessions)
        });
        let echo_p95 = &scenario["echo_ms"]["p95"];
        let base_echo_p95 = base_scenario.map(|b| &b["echo_ms"]["p95"]);
        text.push_str(&format!(
            "{:<64} {:>12} {:>12} {}\n",
            format!("{sessions} sessions host echo round trip p95 ms"),
            echo_p95
                .as_f64()
                .map_or("pending".to_string(), |v| format!("{v:.3}")),
            base_echo_p95
                .and_then(Value::as_f64)
                .map_or("n/a".to_string(), |v| format!("{v:.3}")),
            match (echo_p95.as_f64(), base_echo_p95.and_then(Value::as_f64)) {
                (None, _) => "pending".to_string(),
                (Some(_), None) => "no baseline".to_string(),
                (Some(now), Some(before)) => format!("{:+.1} %", (now / before - 1.0) * 100.0),
            },
        ));
        for process in ["host", "worker", "desktop"] {
            if let Some(invalid) = scenario[process]["work_counters"]["invalid"].as_str() {
                text.push_str(&format!(
                    "{sessions} sessions {process}: invalid: {invalid}\n"
                ));
                continue;
            }
            let Some(counters) = scenario[process]["work_counters"].as_object() else {
                continue;
            };
            for (name, value) in counters {
                let before = base_scenario.map(|b| &b[process]["work_counters"][name]);
                text.push_str(&format!(
                    "{:<64} {:>12} {:>12} {}\n",
                    format!("{sessions} sessions {process} {name}"),
                    value
                        .as_u64()
                        .map_or("pending".to_string(), |v| v.to_string()),
                    before
                        .and_then(Value::as_u64)
                        .map_or("n/a".to_string(), |v| v.to_string()),
                    counter_verdict(value, before),
                ));
            }
        }
    }
    text
}
