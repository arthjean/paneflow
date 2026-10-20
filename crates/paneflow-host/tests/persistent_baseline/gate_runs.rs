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

const BLINK_ALONE_RENDERS_PER_S: f64 = 2.0;

fn focused_thinking_renders(
    focus: &Result<(), String>,
    thinking: &Value,
    renders: Measurement,
    window_s: f64,
) -> Measurement {
    match (focus, thinking.get("error")) {
        (Err(reason), _) => Measurement::Missing(format!(
            "the thinking window was never focused, so the cursor blink is not measured: {reason}"
        )),
        (Ok(()), Some(error)) => Measurement::Missing(format!(
            "no spinner drawn: the agent never declared its turn: {error}"
        )),
        (Ok(()), None) => match per(renders, window_s) {
            Measurement::Value(rate) if rate <= BLINK_ALONE_RENDERS_PER_S => {
                Measurement::Missing(format!(
                    "no spinner drawn: {rate:.3} root renders per second, no more than the cursor blink alone ({BLINK_ALONE_RENDERS_PER_S})"
                ))
            }
            measured => measured,
        },
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
    let sessions = match open(client, ledger, &headless::DECLARING_FIXTURE, DESKTOP_PANES) {
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
            let declared =
                headless::start_thinking(client, endpoint, &sessions[0], SessionGeneration::FIRST);
            std::thread::sleep(SETTLE);
            let (thinking, thinking_s) = desktop_window(&desktop, GATE_WINDOW);
            values.insert(
                "desktop.thinking.root_renders_per_s".to_string(),
                focused_thinking_renders(
                    &focus,
                    &declared,
                    counter(&thinking, "root_renders"),
                    thinking_s,
                ),
            );
            json!({
                "panes": DESKTOP_PANES,
                "settle_s": SETTLE.as_secs_f64(),
                "idle": {"window_s": idle_s, "work_counters": idle},
                "focused_idle": {"window_s": focused_s, "work_counters": focused, "focus": focus.err()},
                "thinking": {"window_s": thinking_s, "work_counters": thinking, "start_thinking": declared},
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
            let focus = headless::focus_window(&desktop);
            std::thread::sleep(SETTLE);
            let (window, window_s) = desktop_window(&desktop, DIFF_STAT_WINDOW);
            let probes = counter(&window, "git_spawns.by_subcommand.diff");
            let spawns = counter(&window, "git_spawns.total");
            let (probes, measurement) = match (&focus, probes, spawns) {
                (Err(reason), _, _) => {
                    let reason = format!(
                        "the window was never focused, and the git poll only runs while a window is active: {reason}"
                    );
                    (
                        Measurement::Missing(reason.clone()),
                        Measurement::Missing(reason),
                    )
                }
                (Ok(()), Measurement::Value(0.0), _) => (
                    Measurement::Value(0.0),
                    Measurement::Missing(format!(
                        "no diff-stat probe ran during the {window_s:.0} s window"
                    )),
                ),
                (Ok(()), Measurement::Value(probes), spawns) => {
                    (Measurement::Value(probes), per(spawns, probes))
                }
                (Ok(()), missing, _) => (missing.clone(), missing),
            };
            values.insert(probes_name.to_string(), probes);
            values.insert(name.to_string(), measurement);
            json!({"window_s": window_s, "work_counters": window, "focus": focus.err(), "repository": repository.display().to_string()})
        }
    };
    stop(client, &sessions);
    detail
}

pub(super) const TUI_WORKSPACES: usize = 30;
const TUI_SETTLE: Duration = Duration::from_secs(10);
const TUI_INACTIVE_WINDOW: Duration = Duration::from_secs(60);
const TUI_FIXTURE_SECONDS: u64 = 300;
const TUI_FIXTURE_MARKER: &str = "tui fixture line";

pub(super) const TUI_NAMES: [&str; 4] = [
    "desktop.tui_30_workspaces.root_renders_per_s",
    "desktop.tui_30_workspaces.git_spawns",
    "desktop.tui_30_workspaces.git_spawns_per_min",
    "desktop.tui_30_workspaces_inactive.git_spawns_per_window",
];

fn tui_fixture(cwd: &Path, seconds: u64) -> CreateSession {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/fixtures");
    #[cfg(windows)]
    let (shell, script) = (
        "powershell",
        vec!["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"],
    );
    #[cfg(windows)]
    let script_path = fixtures.join("tui-agent.ps1");
    #[cfg(not(windows))]
    let (shell, script) = ("sh", Vec::<&str>::new());
    #[cfg(not(windows))]
    let script_path = fixtures.join("tui-agent.sh");
    CreateSession {
        shell: Some(shell.to_string()),
        args: script
            .into_iter()
            .map(str::to_string)
            .chain([script_path.display().to_string(), seconds.to_string()])
            .collect(),
        cwd: Some(cwd.display().to_string()),
        cols: Some(80),
        rows: Some(24),
        ..CreateSession::default()
    }
}

fn sum(first: Measurement, second: Measurement) -> Measurement {
    match (first, second) {
        (Measurement::Value(first), Measurement::Value(second)) => {
            Measurement::Value(first + second)
        }
        (Measurement::Value(_), missing) | (missing, _) => missing,
    }
}

pub(super) struct TuiWindows<'a> {
    pub(super) started: &'a Result<u64, String>,
    pub(super) focus: &'a Result<(), String>,
    pub(super) inactive: &'a Value,
    pub(super) first: &'a Value,
    pub(super) first_s: f64,
    pub(super) second: &'a Value,
    pub(super) second_s: f64,
}

pub(super) fn tui_measurements(windows: &TuiWindows) -> Vec<(&'static str, Measurement)> {
    let all_missing = |reason: String| -> Vec<(&'static str, Measurement)> {
        TUI_NAMES
            .iter()
            .map(|name| (*name, Measurement::Missing(reason.clone())))
            .collect()
    };
    if let Err(reason) = windows.started {
        return all_missing(format!(
            "the TUI fixture never drew in the active pane: {reason}"
        ));
    }
    if let Err(reason) = windows.focus {
        return all_missing(format!(
            "the window was never focused, so the active workspace is not measured: {reason}"
        ));
    }
    let renders = per(counter(windows.first, "root_renders"), windows.first_s);
    if let Measurement::Value(rate) = renders
        && rate <= BLINK_ALONE_RENDERS_PER_S
    {
        return all_missing(format!(
            "the TUI pane received no output: {rate:.3} root renders per second, no more than the cursor blink alone"
        ));
    }
    let spawns = counter(windows.first, "git_spawns.total");
    let minute = per(
        sum(spawns.clone(), counter(windows.second, "git_spawns.total")),
        (windows.first_s + windows.second_s) / 60.0,
    );
    vec![
        (TUI_NAMES[0], renders),
        (TUI_NAMES[1], spawns),
        (TUI_NAMES[2], minute),
        (TUI_NAMES[3], counter(windows.inactive, "git_spawns.total")),
    ]
}

fn seed_tui_repositories(root: &Path) -> Result<Vec<PathBuf>, String> {
    let repositories: Vec<PathBuf> = (0..TUI_WORKSPACES)
        .map(|index| root.join(format!("repository-{index:02}")))
        .collect();
    repositories
        .iter()
        .try_for_each(|repository| committed_repository(repository))
        .map_err(|reason| {
            format!("the {TUI_WORKSPACES} workspace repositories were not seeded: {reason}")
        })?;
    Ok(repositories)
}

pub(super) fn desktop_tui_30_workspaces(
    client: &mut HostClient,
    ledger: &FixtureLedger,
    home: &Path,
    values: &mut Values,
) -> Value {
    let repositories = match seed_tui_repositories(&home.join("tui-workspaces")) {
        Ok(repositories) => repositories,
        Err(reason) => {
            record_all(values, &TUI_NAMES, &reason);
            return json!({"failed": reason});
        }
    };
    let mut sessions = Vec::with_capacity(TUI_WORKSPACES);
    for (index, repository) in repositories.iter().enumerate() {
        let request = if index == 0 {
            tui_fixture(repository, TUI_FIXTURE_SECONDS)
        } else {
            CreateSession {
                cwd: Some(repository.display().to_string()),
                ..workloads::fixture(&["idle"])
            }
        };
        match client.create(&request) {
            Ok(created) => {
                ledger.record(client, &created.manifest.session);
                sessions.push(created.manifest.session);
            }
            Err(error) => {
                stop(client, &sessions);
                let reason = format!("the session of workspace {index} did not start: {error}");
                record_all(values, &TUI_NAMES, &reason);
                return json!({"failed": reason});
            }
        }
    }
    let workspaces: Vec<(PathBuf, SessionId)> = repositories
        .into_iter()
        .zip(sessions.iter().cloned())
        .collect();
    let detail = match headless::start_desktop_with_workspaces(home, &workspaces) {
        Err(error) => {
            let reason = format!(
                "the desktop did not start under the virtual display (display server or Vulkan adapter): {error}"
            );
            record_all(values, &TUI_NAMES, &reason);
            json!({"failed": reason})
        }
        Ok(desktop) => {
            std::thread::sleep(TUI_SETTLE);
            let started = headless::surface_showing(&desktop, TUI_FIXTURE_MARKER);
            let (inactive, inactive_s) = desktop_window(&desktop, TUI_INACTIVE_WINDOW);
            let focus = headless::focus_window(&desktop);
            std::thread::sleep(SETTLE);
            let (first, first_s) = desktop_window(&desktop, GATE_WINDOW);
            let (second, second_s) = desktop_window(&desktop, GATE_WINDOW);
            let windows = TuiWindows {
                started: &started,
                focus: &focus,
                inactive: &inactive,
                first: &first,
                first_s,
                second: &second,
                second_s,
            };
            for (name, measurement) in tui_measurements(&windows) {
                values.insert(name.to_string(), measurement);
            }
            json!({
                "workspaces": TUI_WORKSPACES,
                "settle_s": TUI_SETTLE.as_secs_f64(),
                "fixture": {"surface_id": started.as_ref().ok(), "failed": started.as_ref().err()},
                "inactive": {"window_s": inactive_s, "work_counters": inactive},
                "focused": {"focus": focus.err()},
                "active_first": {"window_s": first_s, "work_counters": first},
                "active_second": {"window_s": second_s, "work_counters": second},
            })
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
        let declared = json!({"declared": "OSC 7501 state=working"});
        assert!(matches!(
            focused_thinking_renders(&unfocused, &declared, Measurement::Value(333.0), 30.0),
            Measurement::Missing(reason) if reason.contains("never focused") && reason.contains("window 42")
        ));
        assert!(matches!(
            focused_thinking_renders(&Ok(()), &json!({"error": "refused"}), Measurement::Value(333.0), 30.0),
            Measurement::Missing(reason) if reason.starts_with("no spinner drawn") && reason.contains("refused")
        ));
        assert!(matches!(
            focused_thinking_renders(&Ok(()), &declared, Measurement::Value(3.0), 30.0),
            Measurement::Missing(reason) if reason.starts_with("no spinner drawn")
        ));
        assert_eq!(
            focused_thinking_renders(&Ok(()), &declared, Measurement::Value(333.0), 30.0),
            Measurement::Value(333.0 / 30.0)
        );
    }

    fn thinking_verdict(measured: Measurement) -> gates::Verdict {
        let budget = gates::BUDGETS
            .iter()
            .find(|budget| budget.counter == "desktop.thinking.root_renders_per_s")
            .expect("the thinking budget exists");
        let values = BTreeMap::from([(budget.counter.to_string(), measured)]);
        gates::verify(std::slice::from_ref(budget), &values).remove(0)
    }

    #[test]
    fn the_blink_alone_never_satisfies_the_thinking_budget() {
        let declared = json!({"declared": "OSC 7501 state=working"});
        let blink_alone =
            focused_thinking_renders(&Ok(()), &declared, Measurement::Value(56.0), 30.0);
        assert!(
            matches!(
                &blink_alone,
                Measurement::Missing(reason) if reason.starts_with("no spinner drawn: 1.867 root renders per second")
            ),
            "{blink_alone:?}"
        );
        assert!(thinking_verdict(blink_alone).failed());
        assert!(thinking_verdict(Measurement::Value(1.866)).failed());
        assert!(thinking_verdict(Measurement::Value(9.9)).failed());
        assert!(thinking_verdict(Measurement::Value(12.1)).failed());
        assert!(!thinking_verdict(Measurement::Value(11.1)).failed());
        assert!(!thinking_verdict(Measurement::Value(10.0)).failed());
        assert!(!thinking_verdict(Measurement::Value(12.0)).failed());
    }

    fn tui_window(root_renders: u64, git_spawns: u64) -> Value {
        json!({"root_renders": root_renders, "git_spawns.total": git_spawns})
    }

    #[test]
    fn a_tui_scenario_without_a_drawing_fixture_leaves_every_budget_missing() {
        let inactive = tui_window(750, 0);
        let busy = tui_window(750, 9);
        let measure = |started: Result<u64, String>, first: &Value| {
            tui_measurements(&TuiWindows {
                started: &started,
                focus: &Ok(()),
                inactive: &inactive,
                first,
                first_s: 30.0,
                second: &busy,
                second_s: 30.0,
            })
        };
        let never_started = measure(
            Err("none of the 30 surfaces shows \"tui fixture line\"".to_string()),
            &busy,
        );
        assert_eq!(never_started.len(), TUI_NAMES.len());
        assert!(never_started.iter().all(|(_, measurement)| matches!(
            measurement,
            Measurement::Missing(reason) if reason.contains("never drew")
        )));
        let silent = measure(Ok(7), &tui_window(56, 0));
        assert!(silent.iter().all(|(_, measurement)| matches!(
            measurement,
            Measurement::Missing(reason) if reason.contains("received no output")
        )));
        let measured: BTreeMap<_, _> = measure(Ok(7), &busy).into_iter().collect();
        assert_eq!(measured[TUI_NAMES[0]], Measurement::Value(25.0));
        assert_eq!(measured[TUI_NAMES[1]], Measurement::Value(9.0));
        assert_eq!(measured[TUI_NAMES[2]], Measurement::Value(18.0));
        assert_eq!(measured[TUI_NAMES[3]], Measurement::Value(0.0));
    }

    #[test]
    fn a_home_that_cannot_seed_its_repositories_leaves_the_git_budgets_missing() {
        let home = tempfile::tempdir().unwrap();
        let blocked = home.path().join("tui-workspaces");
        std::fs::write(&blocked, "a file where the repositories should go").unwrap();
        let reason = seed_tui_repositories(&blocked).unwrap_err();
        assert!(reason.contains("were not seeded"), "{reason}");
        let mut values = Values::new();
        record_all(&mut values, &TUI_NAMES, &reason);
        let verdicts = gates::verify(gates::BUDGETS, &values);
        let git_budgets: Vec<&gates::Verdict> = verdicts
            .iter()
            .filter(|verdict| {
                verdict.counter.contains("tui_30_workspaces")
                    && verdict.counter.contains("git_spawns")
            })
            .collect();
        assert_eq!(git_budgets.len(), 2);
        for verdict in git_budgets {
            assert!(verdict.failed(), "{}", verdict.line());
            assert!(
                verdict.line().contains("were not seeded"),
                "{}",
                verdict.line()
            );
        }
    }

    #[test]
    fn an_unfocused_tui_scenario_measures_no_active_workspace() {
        let window = tui_window(750, 9);
        let measured = tui_measurements(&TuiWindows {
            started: &Ok(7),
            focus: &Err("no visible X11 window belongs to the desktop pid 7".to_string()),
            inactive: &window,
            first: &window,
            first_s: 30.0,
            second: &window,
            second_s: 30.0,
        });
        assert!(measured.iter().all(|(_, measurement)| matches!(
            measurement,
            Measurement::Missing(reason) if reason.contains("never focused")
        )));
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
