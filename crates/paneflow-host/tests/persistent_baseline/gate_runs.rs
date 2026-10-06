use super::*;

use gates::{Measurement, Verdict};

pub(super) const GATE_SESSIONS: usize = 8;
pub(super) const GATE_WINDOW: Duration = Duration::from_secs(30);
pub(super) const DESKTOP_PANES: usize = 4;
pub(super) const DIFF_STAT_WINDOW: Duration = Duration::from_secs(35);
const MIB: f64 = 1024.0 * 1024.0;

type Values = BTreeMap<String, Measurement>;

fn record_all(values: &mut Values, names: &[&str], reason: &str) {
    for name in names {
        values.insert(name.to_string(), Measurement::Missing(reason.to_string()));
    }
}

pub(super) fn counter(work_counters: &Value, name: &str) -> Measurement {
    if let Some(reason) = work_counters["invalid"].as_str() {
        return Measurement::Missing(reason.to_string());
    }
    if let Some(reason) = work_counters["pending"].as_str() {
        return Measurement::Missing(format!("pending: {reason}"));
    }
    match &work_counters[name] {
        Value::Number(value) => value.as_f64().map_or_else(
            || Measurement::Missing(format!("{name} is not a number")),
            Measurement::Value,
        ),
        other => Measurement::Missing(other["pending"].as_str().map_or_else(
            || format!("{name} was not reported"),
            |reason| format!("pending: {reason}"),
        )),
    }
}

fn per(measurement: Measurement, divisor: f64) -> Measurement {
    match measurement {
        Measurement::Value(value) => Measurement::Value(value / divisor),
        other => other,
    }
}

fn open(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    mode: &[&str],
    count: usize,
) -> Result<Vec<SessionId>, String> {
    (0..count)
        .map(|_| {
            let created = client
                .create(&workloads::fixture(mode))
                .map_err(|error| format!("the {} fixture did not start: {error}", mode[0]))?;
            ledger.record(client, &created.manifest.session);
            Ok(created.manifest.session)
        })
        .collect()
}

fn dead_sessions(client: &mut HostClient, sessions: &[SessionId]) -> Vec<String> {
    sessions
        .iter()
        .filter(|session| !client.inspect(session).is_ok_and(|summary| summary.live))
        .map(|session| session.to_string())
        .collect()
}

fn stop(client: &mut HostClient, sessions: &[SessionId]) {
    for session in sessions {
        let _ = client.stop(session, None);
    }
}

fn cpu_ms(before: Option<u64>, after: Option<u64>) -> Measurement {
    match (before, after) {
        (Some(before), Some(after)) => {
            Measurement::Value(after.saturating_sub(before) as f64 / 1_000_000.0)
        }
        _ => Measurement::Missing(
            "the process CPU time is read from /proc/<pid>/stat, which this platform lacks"
                .to_string(),
        ),
    }
}

const HOST_IDLE_NAMES: [&str; 5] = [
    "host.idle.process_listings",
    "host.idle.agent_bus_session_broadcasts",
    "worker.idle.snapshot_broadcasts",
    "host.idle.cpu_ms",
    "worker.idle.cpu_ms",
];

pub(super) fn host_worker_idle(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    host_pid: u32,
    worker: Option<&WorkerProcess>,
    values: &mut Values,
) -> Value {
    let sessions = match open(client, ledger, &["idle"], GATE_SESSIONS) {
        Ok(sessions) => sessions,
        Err(reason) => {
            record_all(values, &HOST_IDLE_NAMES, &reason);
            return json!({"failed": reason});
        }
    };
    std::thread::sleep(SETTLE);
    let worker_pid = worker.map(|worker| worker.child.id());
    let host_before = active::host_counters(client);
    let worker_before = worker.map(active::worker_counters);
    let host_cpu_before = process_cpu_ns(host_pid);
    let worker_cpu_before = worker_pid.and_then(process_cpu_ns);
    let started = Instant::now();
    std::thread::sleep(GATE_WINDOW);
    let host_cpu_after = process_cpu_ns(host_pid);
    let worker_cpu_after = worker_pid.and_then(process_cpu_ns);
    let host_after = active::host_counters(client);
    let worker_after = worker.map(active::worker_counters);
    let window = started.elapsed();
    let dead = dead_sessions(client, &sessions);
    stop(client, &sessions);
    if !dead.is_empty() {
        let reason = format!(
            "idle fixture session {} died during the window",
            dead.join(", ")
        );
        record_all(values, &HOST_IDLE_NAMES, &reason);
        return json!({"failed": reason});
    }
    let host_window = active::window_json(&host_before, &host_after);
    let worker_window = match (&worker_before, &worker_after) {
        (Some(before), Some(after)) => active::window_json(before, after),
        _ => {
            json!({"pending": "PANEFLOW_BENCH_CONTROLLER was not supplied; the worker is unmeasured"})
        }
    };
    values.insert(
        "host.idle.process_listings".to_string(),
        counter(&host_window, "process_listings"),
    );
    values.insert(
        "host.idle.agent_bus_session_broadcasts".to_string(),
        counter(&host_window, "agent_bus_session_broadcasts"),
    );
    values.insert(
        "worker.idle.snapshot_broadcasts".to_string(),
        counter(&worker_window, "snapshot_broadcasts"),
    );
    values.insert(
        "host.idle.cpu_ms".to_string(),
        cpu_ms(host_cpu_before, host_cpu_after),
    );
    values.insert(
        "worker.idle.cpu_ms".to_string(),
        match worker_pid {
            Some(_) => cpu_ms(worker_cpu_before, worker_cpu_after),
            None => Measurement::Missing(
                "PANEFLOW_BENCH_CONTROLLER was not supplied; the worker is unmeasured".to_string(),
            ),
        },
    );
    json!({
        "sessions": GATE_SESSIONS,
        "fixture": "idle",
        "settle_s": SETTLE.as_secs_f64(),
        "window_s": window.as_secs_f64(),
        "host": {"work_counters": host_window, "cpu_ns": [host_cpu_before, host_cpu_after]},
        "worker": {"work_counters": worker_window, "cpu_ns": [worker_cpu_before, worker_cpu_after]},
    })
}

const HOST_ACTIVE_NAMES: [&str; 4] = [
    "host.active.process_listings_per_s",
    "host.active.foreground_observations_per_session",
    "host.active.agent_bus_session_broadcasts_per_s_per_session",
    "host.active.resident_mib",
];

pub(super) fn host_worker_active(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    host_pid: u32,
    worker: Option<&WorkerProcess>,
    values: &mut Values,
) -> Value {
    let plan = active::ActivePlan {
        streams: GATE_SESSIONS,
        stream_args: &active::STREAM_ARGS,
        flood_args: None,
        settle: SETTLE,
        window: GATE_WINDOW,
        cpu_slices: 1,
    };
    let processes = active::ActiveProcesses {
        host_pid,
        worker,
        desktop_home: None,
        echo: None,
    };
    let scenario = match active::run_active_scenario(client, ledger, &plan, &processes) {
        Ok(scenario) => scenario,
        Err(reason) => {
            record_all(values, &HOST_ACTIVE_NAMES, &reason);
            return json!({"failed": reason});
        }
    };
    let window_s = scenario["window_s"].as_f64().unwrap_or(f64::NAN);
    let sessions = GATE_SESSIONS as f64;
    let host = &scenario["host"]["work_counters"];
    values.insert(
        "host.active.process_listings_per_s".to_string(),
        per(counter(host, "process_listings"), window_s),
    );
    values.insert(
        "host.active.foreground_observations_per_session".to_string(),
        per(counter(host, "foreground_observations"), sessions),
    );
    values.insert(
        "host.active.agent_bus_session_broadcasts_per_s_per_session".to_string(),
        per(
            counter(host, "agent_bus_session_broadcasts"),
            window_s * sessions,
        ),
    );
    values.insert(
        "host.active.resident_mib".to_string(),
        scenario["host"]["resident_bytes"].as_f64().map_or_else(
            || Measurement::Missing("the host resident memory was not readable".to_string()),
            |bytes| Measurement::Value(bytes / MIB),
        ),
    );
    scenario
}

const DESKTOP_NAMES: [&str; 5] = [
    "desktop.idle.root_renders",
    "desktop.idle.session_list_calls",
    "desktop.idle.host_agent_snapshots_applied",
    "desktop.focused_idle.root_renders_per_s",
    "desktop.thinking.root_renders_per_s",
];

fn focused_idle_renders(
    focus: &Result<(), String>,
    renders: Measurement,
    window_s: f64,
) -> Measurement {
    match (focus, renders) {
        (Err(reason), _) => Measurement::Missing(reason.clone()),
        (Ok(()), Measurement::Value(count)) if count <= 3.0 => Measurement::Missing(format!(
            "the focused terminal never blinked: {count} root renders in the window"
        )),
        (Ok(()), renders) => per(renders, window_s),
    }
}

fn focused_thinking_renders(
    focus: &Result<(), String>,
    submitted: &Value,
    renders: Measurement,
    window_s: f64,
) -> Measurement {
    match (focus, &renders, submitted.get("error")) {
        (Err(reason), _, _) => Measurement::Missing(format!(
            "the thinking window was never focused, so the cursor blink is not measured: {reason}"
        )),
        (Ok(()), _, Some(error)) => Measurement::Missing(format!(
            "the simulated prompt was not accepted, so no agent thinks: {error}"
        )),
        (Ok(()), Measurement::Value(count), None) if *count <= 3.0 => Measurement::Missing(
            format!("the agent never started thinking: {count} root renders in the window"),
        ),
        _ => per(renders, window_s),
    }
}

fn desktop_window(desktop: &DesktopProcess, window: Duration) -> (Value, f64) {
    let before = active::desktop_counters(desktop);
    let started = Instant::now();
    std::thread::sleep(window);
    let after = active::desktop_counters(desktop);
    (
        active::window_json(&before, &after),
        started.elapsed().as_secs_f64(),
    )
}

pub(super) fn desktop_idle_and_thinking(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    home: &Path,
    endpoint: &Path,
    values: &mut Values,
) -> Value {
    let sessions = match open(client, ledger, &["idle"], DESKTOP_PANES) {
        Ok(sessions) => sessions,
        Err(reason) => {
            record_all(values, &DESKTOP_NAMES, &reason);
            return json!({"failed": reason});
        }
    };
    let detail = match headless::start_blinking_desktop(home, &sessions) {
        Err(error) => {
            let reason = format!(
                "the desktop did not start under the virtual display (display server or Vulkan adapter): {error}"
            );
            record_all(values, &DESKTOP_NAMES, &reason);
            json!({"failed": reason})
        }
        Ok(desktop) => {
            std::thread::sleep(SETTLE);
            let (idle, idle_s) = desktop_window(&desktop, GATE_WINDOW);
            for name in [
                "root_renders",
                "session_list_calls",
                "host_agent_snapshots_applied",
            ] {
                values.insert(format!("desktop.idle.{name}"), counter(&idle, name));
            }
            let focus = headless::focus_window(&desktop);
            std::thread::sleep(SETTLE);
            let (focused, focused_s) = desktop_window(&desktop, GATE_WINDOW);
            values.insert(
                "desktop.focused_idle.root_renders_per_s".to_string(),
                focused_idle_renders(&focus, counter(&focused, "root_renders"), focused_s),
            );
            let submitted = headless::submit_prompt(endpoint, &sessions[0]);
            std::thread::sleep(SETTLE);
            let (thinking, thinking_s) = desktop_window(&desktop, GATE_WINDOW);
            values.insert(
                "desktop.thinking.root_renders_per_s".to_string(),
                focused_thinking_renders(
                    &focus,
                    &submitted,
                    counter(&thinking, "root_renders"),
                    thinking_s,
                ),
            );
            json!({
                "panes": DESKTOP_PANES,
                "settle_s": SETTLE.as_secs_f64(),
                "idle": {"window_s": idle_s, "work_counters": idle},
                "focused_idle": {"window_s": focused_s, "work_counters": focused, "focus": focus.err()},
                "thinking": {"window_s": thinking_s, "work_counters": thinking, "prompt_submit": submitted},
            })
        }
    };
    stop(client, &sessions);
    detail
}

fn committed_repository(root: &Path) -> Result<(), String> {
    let io = |error: std::io::Error| format!("the fixture repository: {error}");
    std::fs::create_dir_all(root).map_err(io)?;
    let global = root.with_extension("gitconfig");
    std::fs::write(&global, "").map_err(io)?;
    let git = |args: &[&str]| -> Result<(), String> {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=perf-gates",
                "-c",
                "user.email=perf-gates@localhost",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .map_err(|error| format!("git {}: {error}", args.join(" ")))?;
        output.status.success().then_some(()).ok_or_else(|| {
            format!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
    };
    git(&["init", "-q"])?;
    std::fs::write(root.join("tracked.txt"), "one\n").map_err(io)?;
    git(&["add", "tracked.txt"])?;
    git(&["commit", "-q", "-m", "fixture"])?;
    std::fs::write(root.join("tracked.txt"), "one\ntwo\n").map_err(io)?;
    std::fs::File::options()
        .write(true)
        .open(root.join(".git").join("config"))
        .and_then(|config| config.set_modified(SystemTime::now() - Duration::from_secs(60)))
        .map_err(io)
}

pub(super) fn desktop_diff_stat(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    home: &Path,
    values: &mut Values,
) -> Value {
    let name = "desktop.diff_stat.git_spawns_per_probe";
    let probes_name = "desktop.diff_stat.probes";
    let names = [name, probes_name];
    let repository = home.join("diff-stat-repository");
    if let Err(reason) = committed_repository(&repository) {
        record_all(values, &names, &reason);
        return json!({"failed": reason});
    }
    let sessions = match open(client, ledger, &["idle"], 1) {
        Ok(sessions) => sessions,
        Err(reason) => {
            record_all(values, &names, &reason);
            return json!({"failed": reason});
        }
    };
    let detail = match headless::start_desktop_in(home, &repository, &sessions) {
        Err(error) => {
            let reason = format!(
                "the desktop did not start under the virtual display (display server or Vulkan adapter): {error}"
            );
            record_all(values, &names, &reason);
            json!({"failed": reason})
        }
        Ok(desktop) => {
            std::thread::sleep(SETTLE);
            let (window, window_s) = desktop_window(&desktop, DIFF_STAT_WINDOW);
            let probes = counter(&window, "git_spawns.by_subcommand.diff");
            let spawns = counter(&window, "git_spawns.total");
            values.insert(probes_name.to_string(), probes.clone());
            let measurement = match (probes, spawns) {
                (Measurement::Value(0.0), _) => Measurement::Missing(format!(
                    "no diff-stat probe ran during the {window_s:.0} s window"
                )),
                (Measurement::Value(probes), spawns) => per(spawns, probes),
                (missing, _) => missing,
            };
            values.insert(name.to_string(), measurement);
            json!({"window_s": window_s, "work_counters": window, "repository": repository.display().to_string()})
        }
    };
    stop(client, &sessions);
    detail
}

fn read_json(path: &Path) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))
}

pub(super) fn hook_burst(inputs: &Path, values: &mut Values) -> Value {
    let path = inputs.join("hook-burst.json");
    match read_json(&path) {
        Ok(document) => {
            for (name, field) in [
                ("hooks.burst.p95_ms", "p95_ms"),
                ("hooks.burst.lost", "lost"),
            ] {
                values.insert(
                    name.to_string(),
                    document[field].as_f64().map_or_else(
                        || Measurement::Missing(format!("{} carries no {field}", path.display())),
                        Measurement::Value,
                    ),
                );
            }
            document
        }
        Err(error) => {
            let reason = format!(
                "the hook burst test wrote no result, so it failed or did not run: {error}"
            );
            record_all(values, &["hooks.burst.p95_ms", "hooks.burst.lost"], &reason);
            json!({"failed": reason})
        }
    }
}

fn suite_metric<'a>(document: &'a Value, name: &str) -> Option<&'a Value> {
    document["metrics"]
        .as_array()?
        .iter()
        .find(|metric| metric["metric"] == name)
}

pub(super) fn startup(inputs: &Path, values: &mut Values) -> Value {
    let name = "desktop.startup.stale_socket_ipc_server_started_p95_ms";
    let metric = "stale_socket_step_ipc_server_started";
    let path = inputs.join("startup.json");
    let measurement = match read_json(&path) {
        Err(error) => Measurement::Missing(format!(
            "the startup suite wrote no result, so it failed or did not run: {error}"
        )),
        Ok(document) => match suite_metric(&document, metric) {
            None => Measurement::Missing(format!("the startup suite reported no {metric}")),
            Some(found) if found["available"] == false => {
                Measurement::Missing(format!("{metric} is unavailable: {}", found["note"]))
            }
            Some(found) => found["p95"].as_f64().map_or_else(
                || Measurement::Missing(format!("{metric} carries no p95")),
                |ns| Measurement::Value(ns / 1_000_000.0),
            ),
        },
    };
    values.insert(name.to_string(), measurement.clone());
    json!({"result": path.display().to_string(), "measurement": format!("{measurement:?}")})
}

pub(super) fn trickle(inputs: &Path, values: &mut Values) -> Value {
    let name = "terminal.gate_trickle_publishes";
    let path = inputs.join("terminal.json");
    let measurement = match read_json(&path) {
        Err(error) => Measurement::Missing(format!(
            "the terminal suite wrote no result, so it failed or did not run: {error}"
        )),
        Ok(document) => suite_metric(&document, "gate_trickle_publishes")
            .and_then(|metric| metric["value"].as_f64())
            .map_or_else(
                || {
                    Measurement::Missing(
                        "the terminal suite reported no gate_trickle_publishes".to_string(),
                    )
                },
                Measurement::Value,
            ),
    };
    values.insert(name.to_string(), measurement.clone());
    json!({"result": path.display().to_string(), "measurement": format!("{measurement:?}")})
}

pub(super) fn allocation_baseline(suite: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../bench/baselines/linux-x86_64")
        .join(format!("{suite}-alloc.json"))
}

pub(super) fn allocations(inputs: &Path) -> Vec<Verdict> {
    [
        ("terminal", &gates::TERMINAL_SUITE),
        ("editor", &gates::EDITOR_SUITE),
    ]
    .into_iter()
    .flat_map(|(suite, scenario)| {
        let current = read_json(&inputs.join(format!("{suite}.json")));
        let baseline = read_json(&allocation_baseline(suite));
        gates::allocation_verdicts(
            suite,
            scenario,
            current.as_ref().map_err(Clone::clone),
            baseline.as_ref().map_err(Clone::clone),
        )
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_without_focus_or_blink_never_satisfies_the_focused_budget() {
        let unfocused = Err("no visible X11 window belongs to the desktop pid 7".to_string());
        assert_eq!(
            focused_idle_renders(&unfocused, Measurement::Value(0.0), 30.0),
            Measurement::Missing("no visible X11 window belongs to the desktop pid 7".to_string())
        );
        assert!(matches!(
            focused_idle_renders(&Ok(()), Measurement::Value(2.0), 30.0),
            Measurement::Missing(reason) if reason.contains("never blinked")
        ));
        assert_eq!(
            focused_idle_renders(&Ok(()), Measurement::Value(56.0), 30.0),
            Measurement::Value(56.0 / 30.0)
        );
    }

    #[test]
    fn a_thinking_window_that_was_never_focused_never_satisfies_its_budget() {
        let unfocused =
            Err("the desktop window 42 did not take the X input focus within 5 s".to_string());
        let accepted = json!({"accepted": true});
        assert!(matches!(
            focused_thinking_renders(&unfocused, &accepted, Measurement::Value(333.0), 30.0),
            Measurement::Missing(reason) if reason.contains("never focused") && reason.contains("window 42")
        ));
        assert!(matches!(
            focused_thinking_renders(&Ok(()), &json!({"error": "refused"}), Measurement::Value(333.0), 30.0),
            Measurement::Missing(reason) if reason.contains("refused")
        ));
        assert!(matches!(
            focused_thinking_renders(&Ok(()), &accepted, Measurement::Value(3.0), 30.0),
            Measurement::Missing(reason) if reason.contains("never started thinking")
        ));
        assert_eq!(
            focused_thinking_renders(&Ok(()), &accepted, Measurement::Value(333.0), 30.0),
            Measurement::Value(333.0 / 30.0)
        );
    }

    #[test]
    fn a_pending_counter_or_an_invalidated_window_reads_as_missing() {
        assert_eq!(
            counter(&json!({"process_listings": 4}), "process_listings"),
            Measurement::Value(4.0)
        );
        assert_eq!(
            counter(
                &json!({"process_listings": {"pending": "counters unavailable: host 0.17.5"}}),
                "process_listings"
            ),
            Measurement::Missing("pending: counters unavailable: host 0.17.5".to_string())
        );
        assert_eq!(
            counter(
                &json!({"invalid": "worker restarted during the measurement"}),
                "snapshot_broadcasts"
            ),
            Measurement::Missing("worker restarted during the measurement".to_string())
        );
        assert_eq!(
            counter(&json!({"pending": "no worker"}), "snapshot_broadcasts"),
            Measurement::Missing("pending: no worker".to_string())
        );
        assert_eq!(
            counter(&json!({}), "root_renders"),
            Measurement::Missing("root_renders was not reported".to_string())
        );
        assert_eq!(per(Measurement::Value(60.0), 30.0), Measurement::Value(2.0));
    }

    #[test]
    fn a_suite_that_wrote_no_result_leaves_its_budgets_missing() {
        let empty = tempfile::tempdir().unwrap();
        let mut values = Values::new();
        hook_burst(empty.path(), &mut values);
        startup(empty.path(), &mut values);
        trickle(empty.path(), &mut values);
        assert_eq!(values.len(), 4);
        assert!(
            values
                .values()
                .all(|value| matches!(value, Measurement::Missing(reason) if reason.contains("wrote no result")))
        );
        assert!(allocations(empty.path()).iter().all(Verdict::failed));
    }

    #[test]
    fn the_committed_allocation_baselines_are_linux_suite_results() {
        for suite in ["terminal", "editor"] {
            let baseline = read_json(&allocation_baseline(suite)).unwrap();
            assert_eq!(baseline["os"], "linux", "{suite}");
            assert_eq!(baseline["arch"], "x86_64", "{suite}");
            assert_eq!(baseline["profile"], "release", "{suite}");
            let verdicts = gates::allocation_verdicts(
                suite,
                &gates::TERMINAL_SUITE,
                Ok(&baseline),
                Ok(&baseline),
            );
            assert!(
                verdicts.len() > 10,
                "{suite}: {} gated columns",
                verdicts.len()
            );
            assert!(verdicts.iter().all(|verdict| !verdict.failed()), "{suite}");
        }
    }
}
