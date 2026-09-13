use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::bench_harness::{Direction, Metric, publish, refuse_debug_profile};
use crate::startup_trace::OUTPUT_PATH_ENV;

const SUITE: &str = "paneflow-startup-bench";
const EXE_ENV: &str = "PANEFLOW_BENCH_EXE";
const RUNS_ENV: &str = "PANEFLOW_BENCH_STARTUP_RUNS";
const DEFAULT_RUNS: usize = 10;
const RUN_TIMEOUT: Duration = Duration::from_secs(30);
const RESTORE_WORKSPACES: usize = 3;
const STEP_NOTE: &str =
    "wall-clock median across launches of the time between this mark and the previous one";
const TOTAL_NOTE: &str =
    "wall-clock median across launches from the first line of main to the first presented frame";

#[derive(Clone, Copy)]
enum Scenario {
    Welcome,
    Restore,
}

impl Scenario {
    const ALL: [Scenario; 2] = [Scenario::Welcome, Scenario::Restore];

    fn prefix(self) -> &'static str {
        match self {
            Scenario::Welcome => "welcome",
            Scenario::Restore => "restore3",
        }
    }

    fn session(self, cwd: &Path) -> paneflow_config::schema::SessionState {
        let workspaces = match self {
            Scenario::Welcome => Vec::new(),
            Scenario::Restore => (0..RESTORE_WORKSPACES)
                .map(|index| {
                    serde_json::json!({
                        "title": format!("bench-{index}"),
                        "cwd": cwd.to_string_lossy(),
                        "tabs": [{
                            "title": "",
                            "title_source": "preset",
                            "layout": {
                                "type": "pane",
                                "surfaces": [{
                                    "surface_type": "terminal",
                                    "name": "Terminal",
                                    "command": null,
                                    "cwd": cwd.to_string_lossy(),
                                    "env": null,
                                    "focus": true
                                }]
                            }
                        }]
                    })
                })
                .collect(),
        };
        let document = serde_json::json!({
            "version": paneflow_config::schema::SESSION_SCHEMA_VERSION,
            "active_workspace": 0,
            "workspaces": workspaces,
        });
        serde_json::from_value(document).expect("fixture session matches the session schema")
    }
}

struct Trace {
    profile: String,
    marks: Vec<(String, u64)>,
}

fn app_binary() -> PathBuf {
    if let Some(path) = std::env::var_os(EXE_ENV) {
        return PathBuf::from(path);
    }
    let test_binary = std::env::current_exe().expect("test binary path");
    let profile_dir = test_binary
        .parent()
        .and_then(Path::parent)
        .expect("test binaries live under <target>/<profile>/deps");
    profile_dir.join(format!("paneflow{}", std::env::consts::EXE_SUFFIX))
}

fn seed_home(home: &Path, scenario: Scenario, cwd: &Path) {
    std::fs::create_dir_all(home).expect("fixture home");
    let session = serde_json::to_string_pretty(&scenario.session(cwd)).expect("session json");
    std::fs::write(home.join("session.json"), session).expect("fixture session");
    std::fs::write(home.join("paneflow.json"), "{}\n").expect("fixture config");
    std::fs::write(
        home.join("window-state.json"),
        "{\"width\":1400.0,\"height\":900.0}\n",
    )
    .expect("fixture window state");
    std::fs::write(home.join("telemetry_id"), "startup-bench\n").expect("fixture telemetry id");
}

fn parse_trace(text: &str) -> Trace {
    let document: serde_json::Value = serde_json::from_str(text).expect("trace is JSON");
    let marks = document["marks"]
        .as_array()
        .expect("trace carries a marks array")
        .iter()
        .map(|mark| {
            (
                mark["name"].as_str().expect("mark name").to_owned(),
                mark["at_us"].as_u64().expect("mark offset"),
            )
        })
        .collect();
    Trace {
        profile: document["profile"].as_str().unwrap_or("unknown").to_owned(),
        marks,
    }
}

fn launch_to_first_frame(exe: &Path, home: &Path, trace_path: &Path) -> Trace {
    let _ = std::fs::remove_file(trace_path);
    let mut child = Command::new(exe)
        .env(paneflow_home::HOME_ENV, home)
        .env(OUTPUT_PATH_ENV, trace_path)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("failed to launch {}: {error}", exe.display()));
    let started = Instant::now();
    loop {
        match child.try_wait().expect("poll the launched app") {
            Some(status) => {
                assert!(status.success(), "paneflow exited with {status}");
                break;
            }
            None if started.elapsed() > RUN_TIMEOUT => {
                let _ = child.kill();
                panic!("paneflow did not present a first frame within {RUN_TIMEOUT:?}");
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    let text = std::fs::read_to_string(trace_path)
        .unwrap_or_else(|error| panic!("no trace at {}: {error}", trace_path.display()));
    parse_trace(&text)
}

fn percentile(sorted: &[u64], percentile: usize) -> u64 {
    let index = (sorted.len().saturating_sub(1) * percentile) / 100;
    sorted[index.min(sorted.len().saturating_sub(1))]
}

fn metric_from_us(name: &'static str, note: &'static str, samples_us: &mut [u64]) -> Metric {
    samples_us.sort_unstable();
    let mean_us = samples_us.iter().sum::<u64>() as f64 / samples_us.len().max(1) as f64;
    Metric {
        name,
        unit: "ns",
        direction: Direction::LowerIsBetter,
        value: percentile(samples_us, 50) as f64 * 1_000.0,
        p95: Some(percentile(samples_us, 95) as f64 * 1_000.0),
        mean: Some(mean_us * 1_000.0),
        alloc_bytes_per_iter: None,
        allocs_per_iter: None,
        iters: samples_us.len(),
        note,
        available: true,
    }
}

fn metrics_from_traces(prefix: &str, traces: &[Trace]) -> Vec<Metric> {
    let reference = &traces[0];
    for (index, trace) in traces.iter().enumerate() {
        let names = |t: &Trace| {
            t.marks
                .iter()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(trace),
            names(reference),
            "launch {index} produced a different mark sequence"
        );
    }
    let mut metrics = Vec::with_capacity(reference.marks.len() + 1);
    let mut totals: Vec<u64> = traces
        .iter()
        .map(|trace| trace.marks.last().map(|(_, at)| *at).unwrap_or(0))
        .collect();
    let total_name: &'static str = String::leak(format!("{prefix}_first_frame_total"));
    metrics.push(metric_from_us(total_name, TOTAL_NOTE, &mut totals));
    for (index, (name, _)) in reference.marks.iter().enumerate() {
        let mut steps: Vec<u64> = traces
            .iter()
            .map(|trace| {
                let at = trace.marks[index].1;
                let previous = if index == 0 {
                    0
                } else {
                    trace.marks[index - 1].1
                };
                at.saturating_sub(previous)
            })
            .collect();
        let metric_name: &'static str = String::leak(format!("{prefix}_step_{name}"));
        metrics.push(metric_from_us(metric_name, STEP_NOTE, &mut steps));
    }
    metrics
}

#[test]
#[ignore = "startup benchmark: run through scripts/bench-startup"]
fn startup_first_frame_benchmark() {
    refuse_debug_profile();
    let exe = app_binary();
    assert!(
        exe.is_file(),
        "no app binary at {}; build it first or set {EXE_ENV}",
        exe.display()
    );
    let runs = std::env::var(RUNS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|runs| *runs > 0)
        .unwrap_or(DEFAULT_RUNS);
    let fixture =
        std::env::temp_dir().join(format!("paneflow-startup-bench-{}", std::process::id()));
    let cwd = fixture.join("project");
    std::fs::create_dir_all(&cwd).expect("fixture project dir");
    println!("PANEFLOW_BENCH_NOTE app binary: {}", exe.display());
    println!("PANEFLOW_BENCH_NOTE fixture: {}", fixture.display());

    let mut metrics = Vec::new();
    for scenario in Scenario::ALL {
        let home = fixture.join(format!("home-{}", scenario.prefix()));
        seed_home(&home, scenario, &cwd);
        let warmup = launch_to_first_frame(&exe, &home, &fixture.join("warmup.json"));
        assert!(
            warmup.profile == "release" || std::env::var_os("PANEFLOW_BENCH_ALLOW_DEBUG").is_some(),
            "the app binary is a {} build; benchmark a release build (or set PANEFLOW_BENCH_ALLOW_DEBUG=1)",
            warmup.profile
        );
        let traces: Vec<Trace> = (0..runs)
            .map(|run| launch_to_first_frame(&exe, &home, &fixture.join(format!("run-{run}.json"))))
            .collect();
        metrics.extend(metrics_from_traces(scenario.prefix(), &traces));
    }
    let _ = std::fs::remove_dir_all(&fixture);

    println!(
        "PANEFLOW_BENCH_NOTE {runs} timed launches per scenario after one warm-up each; cpu share is not measured for a subprocess suite"
    );
    publish(SUITE, 0, &metrics, 0.0);
}

#[cfg(test)]
mod tests {
    use super::{Scenario, Trace, metrics_from_traces, parse_trace};

    fn trace(at_us: &[(&str, u64)]) -> Trace {
        Trace {
            profile: "release".to_owned(),
            marks: at_us
                .iter()
                .map(|(name, at)| ((*name).to_owned(), *at))
                .collect(),
        }
    }

    #[test]
    fn steps_are_medians_of_consecutive_mark_differences() {
        let traces = [
            trace(&[("a", 100), ("b", 400)]),
            trace(&[("a", 120), ("b", 220)]),
            trace(&[("a", 110), ("b", 310)]),
        ];
        let metrics = metrics_from_traces("welcome", &traces);
        assert_eq!(metrics[0].name, "welcome_first_frame_total");
        assert_eq!(metrics[0].value, 310_000.0);
        assert_eq!(metrics[1].name, "welcome_step_a");
        assert_eq!(metrics[1].value, 110_000.0);
        assert_eq!(metrics[2].name, "welcome_step_b");
        assert_eq!(metrics[2].value, 200_000.0);
        assert_eq!(metrics[2].iters, 3);
    }

    #[test]
    fn parse_reads_the_probe_document() {
        let text = r#"{"schema":1,"profile":"release","total_us":9,"marks":[{"name":"x","at_us":4,"step_us":4},{"name":"y","at_us":9,"step_us":5}]}"#;
        let parsed = parse_trace(text);
        assert_eq!(parsed.profile, "release");
        assert_eq!(parsed.marks, vec![("x".to_owned(), 4), ("y".to_owned(), 9)]);
    }

    #[test]
    #[should_panic(expected = "different mark sequence")]
    fn diverging_mark_sequences_are_rejected() {
        let traces = [trace(&[("a", 1), ("b", 2)]), trace(&[("a", 1), ("c", 2)])];
        metrics_from_traces("welcome", &traces);
    }

    #[test]
    fn fixture_sessions_match_the_schema() {
        let cwd = std::path::Path::new("bench-project");
        let welcome = Scenario::Welcome.session(cwd);
        assert!(welcome.workspaces.is_empty());
        let restore = Scenario::Restore.session(cwd);
        assert_eq!(restore.workspaces.len(), super::RESTORE_WORKSPACES);
        assert!(restore.workspaces.iter().all(|ws| ws.tabs.len() == 1));
        let text = serde_json::to_string(&restore).unwrap();
        let back: paneflow_config::schema::SessionState = serde_json::from_str(&text).unwrap();
        assert_eq!(back, restore);
    }
}
