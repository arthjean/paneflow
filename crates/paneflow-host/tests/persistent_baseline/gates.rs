use super::*;

pub(super) const ALLOCATION_TOLERANCE: f64 = 0.01;

pub(super) struct Scenario {
    pub(super) id: &'static str,
    pub(super) description: &'static str,
    pub(super) reproduce: &'static str,
}

pub(super) const HOST_IDLE: Scenario = Scenario {
    id: "host_worker_idle",
    description: "8 idle fixture sessions, host and worker without a desktop, 30 s window",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const HOST_ACTIVE: Scenario = Scenario {
    id: "host_worker_active",
    description: "8 `stream 16384 60` fixture sessions, host and worker without a desktop, 30 s window",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const HOOK_BURST: Scenario = Scenario {
    id: "hook_burst",
    description: "200 hooks in a burst on one session with 100 ms of injected durability latency, home on tmpfs so the disk adds none",
    reproduce: "TMPDIR=/dev/shm cargo test --profile gates --locked -p paneflow-host --lib a_hook_burst_with_slow_durability -- --nocapture",
};

pub(super) const DESKTOP_IDLE: Scenario = Scenario {
    id: "desktop_idle",
    description: "release desktop under Xvfb with Mesa lavapipe, 4 idle panes, 30 s window",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const DESKTOP_FOCUSED_IDLE: Scenario = Scenario {
    id: "desktop_focused_idle",
    description: "the same desktop once xdotool gives its window the X input focus, so the focused terminal blinks, 30 s window",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const DESKTOP_THINKING: Scenario = Scenario {
    id: "desktop_thinking",
    description: "the same focused desktop with one agent thinking (sidebar spinner running, cursor blinking), 30 s window",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const DESKTOP_DIFF_STAT: Scenario = Scenario {
    id: "desktop_diff_stat",
    description: "desktop on one workspace whose git repository and config do not change, 35 s window covering a 30 s git poll",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const DESKTOP_STARTUP: Scenario = Scenario {
    id: "desktop_startup",
    description: "startup suite, 10 timed launches each on a stale desktop IPC socket",
    reproduce: "scripts/bench-startup.sh",
};

pub(super) const TERMINAL_SUITE: Scenario = Scenario {
    id: "terminal_suite",
    description: "terminal_pipeline_benchmark against bench/baselines/linux-x86_64/terminal-alloc.json",
    reproduce: "scripts/perf-gates.sh",
};

pub(super) const EDITOR_SUITE: Scenario = Scenario {
    id: "editor_suite",
    description: "editor_pipeline_benchmark against bench/baselines/linux-x86_64/editor-alloc.json",
    reproduce: "scripts/perf-gates.sh",
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Limit {
    AtMost(f64),
    Below(f64),
    Exactly(f64),
    Within { baseline: f64, tolerance: f64 },
}

pub(super) struct Budget {
    pub(super) counter: &'static str,
    pub(super) limit: Limit,
    pub(super) unit: &'static str,
    pub(super) scenario: &'static Scenario,
    pub(super) margin: &'static str,
}

pub(super) const BUDGETS: &[Budget] = &[
    Budget {
        counter: "host.idle.process_listings",
        limit: Limit::Exactly(0.0),
        unit: "listings per window",
        scenario: &HOST_IDLE,
        margin: "no margin: a session that prints nothing lists no process (FR-05); measured 0",
    },
    Budget {
        counter: "host.idle.agent_bus_session_broadcasts",
        limit: Limit::Exactly(0.0),
        unit: "frames per window",
        scenario: &HOST_IDLE,
        margin: "no margin: an unchanged session is not rebroadcast; measured 0",
    },
    Budget {
        counter: "worker.idle.snapshot_broadcasts",
        limit: Limit::Exactly(0.0),
        unit: "snapshots per window",
        scenario: &HOST_IDLE,
        margin: "no margin: an unchanged snapshot is never broadcast (FR-06, US-008); measured 0",
    },
    Budget {
        counter: "host.idle.cpu_ms",
        limit: Limit::AtMost(30.0),
        unit: "ms of CPU per window",
        scenario: &HOST_IDLE,
        margin: "the PRD bound of 30 ms per 30 s; measured 0 to 20 ms over three runs, /proc/<pid>/stat counts 10 ms ticks",
    },
    Budget {
        counter: "worker.idle.cpu_ms",
        limit: Limit::AtMost(30.0),
        unit: "ms of CPU per window",
        scenario: &HOST_IDLE,
        margin: "the PRD bound of 30 ms per 30 s; measured 0 to 20 ms over three runs, /proc/<pid>/stat counts 10 ms ticks",
    },
    Budget {
        counter: "host.active.process_listings_per_s",
        limit: Limit::AtMost(2.0),
        unit: "listings per second",
        scenario: &HOST_ACTIVE,
        margin: "the PRD bound (FR-05); one shared listing per 500 ms measured 1.87 and 1.90 per second, so a second lister fails it",
    },
    Budget {
        counter: "host.active.foreground_observations_per_session",
        limit: Limit::AtMost(1.0),
        unit: "walks per session",
        scenario: &HOST_ACTIVE,
        margin: "the PRD bound of 1 per session and per foreground group change, with no group change in the window; measured 0",
    },
    Budget {
        counter: "host.active.agent_bus_session_broadcasts_per_s_per_session",
        limit: Limit::AtMost(2.0),
        unit: "frames per second per session",
        scenario: &HOST_ACTIVE,
        margin: "the PRD bound; one announcement per printing session per 500 ms scan tick, measured 1.97 (2.97 before the scan stopped announcing a session twice per tick)",
    },
    Budget {
        counter: "host.active.resident_mib",
        limit: Limit::AtMost(29.0),
        unit: "MiB",
        scenario: &HOST_ACTIVE,
        margin: "the larger measure after EP-002, 23.2 MiB (20.7 MiB on the other run, Fedora, release build), plus 25 %",
    },
    Budget {
        counter: "hooks.burst.p95_ms",
        limit: Limit::Below(350.0),
        unit: "ms",
        scenario: &HOOK_BURST,
        margin: "the paneflow-ai-hook response deadline (US-023); measured 201 ms in release, two batches of the 100 ms injected latency",
    },
    Budget {
        counter: "hooks.burst.lost",
        limit: Limit::Exactly(0.0),
        unit: "events",
        scenario: &HOOK_BURST,
        margin: "no margin: every acknowledged hook is durable (US-023)",
    },
    Budget {
        counter: "desktop.idle.root_renders",
        limit: Limit::AtMost(3.0),
        unit: "renders per window",
        scenario: &DESKTOP_IDLE,
        margin: "the PRD bound of 3 per 30 s; measured 0 in both runs after a 4 s settle",
    },
    Budget {
        counter: "desktop.idle.session_list_calls",
        limit: Limit::Exactly(0.0),
        unit: "calls per window",
        scenario: &DESKTOP_IDLE,
        margin: "no margin: an unchanged agent snapshot triggers no session.list (US-008); measured 0",
    },
    Budget {
        counter: "desktop.idle.host_agent_snapshots_applied",
        limit: Limit::Exactly(0.0),
        unit: "snapshots per window",
        scenario: &DESKTOP_IDLE,
        margin: "no margin: an unchanged snapshot is neither broadcast nor applied (US-008); measured 0",
    },
    Budget {
        counter: "desktop.focused_idle.root_renders_per_s",
        limit: Limit::AtMost(2.0),
        unit: "renders per second",
        scenario: &DESKTOP_FOCUSED_IDLE,
        margin: "the cursor blink alone, one toggle per 540 ms (1.85 per second); measured 1.85 on a real focused window",
    },
    Budget {
        counter: "desktop.thinking.root_renders_per_s",
        limit: Limit::AtMost(12.0),
        unit: "renders per second",
        scenario: &DESKTOP_THINKING,
        margin: "the PRD bound (FR-04); the spinner steps every 90 ms and the blink lands on its steps, so 11.1 per second; 13.0 when they drew separate frames",
    },
    Budget {
        counter: "desktop.diff_stat.probes",
        limit: Limit::AtMost(2.0),
        unit: "diff-stat probes per window",
        scenario: &DESKTOP_DIFF_STAT,
        margin: "one 30 s poll, two if the window straddles it; 15 when each probe's own reads of HEAD and index triggered the next",
    },
    Budget {
        counter: "desktop.diff_stat.git_spawns_per_probe",
        limit: Limit::Exactly(3.0),
        unit: "git processes per probe",
        scenario: &DESKTOP_DIFF_STAT,
        margin: "no margin: rev-parse, diff and ls-files, the filter query reused from its cache (US-009, FR-07)",
    },
    Budget {
        counter: "desktop.startup.stale_socket_ipc_server_started_p95_ms",
        limit: Limit::AtMost(5.0),
        unit: "ms",
        scenario: &DESKTOP_STARTUP,
        margin: "the PRD bound; it guards the removed 140 ms of retries (US-010), measured 0.10 to 0.80 ms over three runs",
    },
    Budget {
        counter: "terminal.gate_trickle_publishes",
        limit: Limit::Exactly(250.0),
        unit: "frames per 1000 chunks",
        scenario: &TERMINAL_SUITE,
        margin: "no margin: a simulated clock makes the count exact; measured 250",
    },
];

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Measurement {
    Value(f64),
    Missing(String),
    Unmeasured(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Outcome {
    Pass,
    Fail(String),
    Missing(String),
    Unmeasured(String),
}

#[derive(Clone, Debug)]
pub(super) struct Verdict {
    pub(super) counter: String,
    pub(super) measured: Measurement,
    pub(super) unit: String,
    pub(super) budget: String,
    pub(super) scenario: String,
    pub(super) reproduce: String,
    pub(super) outcome: Outcome,
}

fn number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn signed(value: f64) -> String {
    if value >= 0.0 {
        format!("+{}", number(value))
    } else {
        format!("-{}", number(-value))
    }
}

fn relative(delta: f64, reference: f64) -> String {
    if reference == 0.0 {
        String::new()
    } else {
        format!(" ({:+.2} %)", delta / reference * 100.0)
    }
}

impl Limit {
    fn describe(self, unit: &str) -> String {
        match self {
            Limit::AtMost(value) => format!("<= {} {unit}", number(value)),
            Limit::Below(value) => format!("< {} {unit}", number(value)),
            Limit::Exactly(value) => format!("= {} {unit}", number(value)),
            Limit::Within {
                baseline,
                tolerance,
            } => format!(
                "{} {unit} +/- {} %",
                number(baseline),
                number(tolerance * 100.0)
            ),
        }
    }

    fn judge(self, measured: f64) -> Outcome {
        let fail = |reference: f64| {
            let delta = measured - reference;
            Outcome::Fail(format!("{}{}", signed(delta), relative(delta, reference)))
        };
        match self {
            Limit::AtMost(limit) if measured <= limit => Outcome::Pass,
            Limit::AtMost(limit) => fail(limit),
            Limit::Below(limit) if measured < limit => Outcome::Pass,
            Limit::Below(limit) => fail(limit),
            Limit::Exactly(expected) if measured == expected => Outcome::Pass,
            Limit::Exactly(expected) => fail(expected),
            Limit::Within {
                baseline,
                tolerance,
            } => {
                let band = baseline.abs() * tolerance;
                if (measured - baseline).abs() <= band {
                    Outcome::Pass
                } else if measured > baseline {
                    fail(baseline)
                } else {
                    let delta = measured - baseline;
                    Outcome::Fail(format!(
                        "{}{}: below the baseline, refresh it in this PR with scripts/perf-gates.sh --refresh-alloc-baselines",
                        signed(delta),
                        relative(delta, baseline)
                    ))
                }
            }
        }
    }
}

fn verdict(
    counter: String,
    limit: Limit,
    unit: &str,
    scenario: &Scenario,
    measured: Measurement,
) -> Verdict {
    let outcome = match &measured {
        Measurement::Value(value) => limit.judge(*value),
        Measurement::Missing(reason) => Outcome::Missing(reason.clone()),
        Measurement::Unmeasured(reason) => Outcome::Unmeasured(reason.clone()),
    };
    Verdict {
        counter,
        measured,
        unit: unit.to_string(),
        budget: limit.describe(unit),
        scenario: format!("{}: {}", scenario.id, scenario.description),
        reproduce: scenario.reproduce.to_string(),
        outcome,
    }
}

pub(super) fn verify(
    budgets: &[Budget],
    measurements: &BTreeMap<String, Measurement>,
) -> Vec<Verdict> {
    budgets
        .iter()
        .map(|budget| {
            let measured = measurements
                .get(budget.counter)
                .cloned()
                .unwrap_or_else(|| {
                    Measurement::Missing(format!(
                        "scenario {} recorded no value",
                        budget.scenario.id
                    ))
                });
            verdict(
                budget.counter.to_string(),
                budget.limit,
                budget.unit,
                budget.scenario,
                measured,
            )
        })
        .collect()
}

const ALLOCATION_COLUMNS: [(&str, &str); 2] = [
    ("alloc_bytes_per_iter", "bytes per iteration"),
    ("allocs_per_iter", "allocations per iteration"),
];

fn platform_refusal(document: &Value, role: &str) -> Option<String> {
    let platform = format!(
        "{}-{}",
        document["os"].as_str().unwrap_or("unknown"),
        document["arch"].as_str().unwrap_or("unknown")
    );
    (platform != "linux-x86_64")
        .then(|| format!("{role} recorded on {platform}; only linux-x86_64 is comparable"))
}

pub(super) fn allocation_verdicts(
    suite: &str,
    scenario: &Scenario,
    current: Result<&Value, String>,
    baseline: Result<&Value, String>,
) -> Vec<Verdict> {
    let refusal = match (&current, &baseline) {
        (Err(reason), _) => Some(format!("no {suite} result: {reason}")),
        (_, Err(reason)) => Some(format!("no {suite} baseline: {reason}")),
        (Ok(current), Ok(baseline)) => platform_refusal(baseline, "the baseline")
            .or_else(|| platform_refusal(current, "the result")),
    };
    if let Some(reason) = refusal {
        return vec![verdict(
            format!("{suite}.allocations"),
            Limit::Within {
                baseline: 0.0,
                tolerance: ALLOCATION_TOLERANCE,
            },
            "per iteration",
            scenario,
            Measurement::Missing(reason),
        )];
    }
    let (Ok(current), Ok(baseline)) = (current, baseline) else {
        return Vec::new();
    };
    let metrics = |document: &Value| -> BTreeMap<String, Value> {
        document["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|metric| Some((metric["metric"].as_str()?.to_string(), metric.clone())))
            .collect()
    };
    let current = metrics(current);
    let baseline = metrics(baseline);
    let mut names: Vec<&String> = current.keys().chain(baseline.keys()).collect();
    names.sort();
    names.dedup();
    let mut verdicts = Vec::new();
    for name in names {
        for (column, unit) in ALLOCATION_COLUMNS {
            let before = baseline
                .get(name)
                .and_then(|metric| metric[column].as_f64());
            let now = current.get(name);
            if before.is_none() && now.is_none_or(|metric| metric[column].is_null()) {
                continue;
            }
            let counter = format!("{suite}.{name}.{column}");
            let measured = match now {
                None => Measurement::Missing(format!("{name} is no longer reported by the suite")),
                Some(metric) if metric["available"] == false => Measurement::Unmeasured(
                    metric["note"]
                        .as_str()
                        .unwrap_or("the suite reports it unavailable")
                        .to_string(),
                ),
                Some(metric) => match (metric[column].as_f64(), before) {
                    (None, _) => Measurement::Missing(format!("{name} reports no {column}")),
                    (Some(_), None) => Measurement::Missing(format!(
                        "{name} has no baseline value: refresh the baseline in this PR with scripts/perf-gates.sh --refresh-alloc-baselines"
                    )),
                    (Some(value), Some(_)) => Measurement::Value(value),
                },
            };
            verdicts.push(verdict(
                counter,
                Limit::Within {
                    baseline: before.unwrap_or(0.0),
                    tolerance: ALLOCATION_TOLERANCE,
                },
                unit,
                scenario,
                measured,
            ));
        }
    }
    verdicts
}

impl Verdict {
    pub(super) fn failed(&self) -> bool {
        matches!(self.outcome, Outcome::Fail(_) | Outcome::Missing(_))
    }

    fn measured_text(&self) -> String {
        match &self.measured {
            Measurement::Value(value) => format!("{} {}", number(*value), self.unit),
            Measurement::Missing(_) => "missing".to_string(),
            Measurement::Unmeasured(_) => "not measured".to_string(),
        }
    }

    fn excess_text(&self) -> String {
        match &self.outcome {
            Outcome::Pass => "within budget".to_string(),
            Outcome::Fail(excess) => excess.clone(),
            Outcome::Missing(reason) => format!("no measurement: {reason}"),
            Outcome::Unmeasured(reason) => format!("not measured, not judged: {reason}"),
        }
    }

    pub(super) fn line(&self) -> String {
        format!(
            "{} | {} | {} | {} | {}",
            self.counter,
            self.measured_text(),
            self.budget,
            self.excess_text(),
            self.scenario
        )
    }

    fn status(&self) -> &'static str {
        match self.outcome {
            Outcome::Pass => "pass",
            Outcome::Fail(_) => "FAIL",
            Outcome::Missing(_) => "FAIL (missing)",
            Outcome::Unmeasured(_) => "not measured",
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "counter": self.counter,
            "status": self.status(),
            "measured": match &self.measured {
                Measurement::Value(value) => json!(value),
                Measurement::Missing(reason) => json!({"missing": reason}),
                Measurement::Unmeasured(reason) => json!({"unmeasured": reason}),
            },
            "unit": self.unit,
            "budget": self.budget,
            "excess": self.excess_text(),
            "scenario": self.scenario,
            "reproduce": self.reproduce,
        })
    }
}

fn reproduce_commands(verdicts: &[Verdict]) -> Vec<&str> {
    let mut commands: Vec<&str> = verdicts
        .iter()
        .filter(|verdict| verdict.failed())
        .map(|verdict| verdict.reproduce.as_str())
        .collect();
    commands.sort_unstable();
    commands.dedup();
    commands
}

pub(super) fn failure_report(verdicts: &[Verdict]) -> Option<String> {
    let failed: Vec<&Verdict> = verdicts.iter().filter(|verdict| verdict.failed()).collect();
    if failed.is_empty() {
        return None;
    }
    let mut text = format!(
        "{} of {} performance budgets failed\ncounter | measured | budget | excess | scenario\n",
        failed.len(),
        verdicts.len()
    );
    for verdict in &failed {
        text.push_str(&verdict.line());
        text.push('\n');
    }
    for command in reproduce_commands(verdicts) {
        text.push_str(&format!("reproduce locally: {command}\n"));
    }
    Some(text)
}

fn cell(text: &str) -> String {
    text.replace('|', "\\|")
}

pub(super) fn markdown(verdicts: &[Verdict]) -> String {
    let failed = verdicts.iter().filter(|verdict| verdict.failed()).count();
    let unmeasured = verdicts
        .iter()
        .filter(|verdict| matches!(verdict.outcome, Outcome::Unmeasured(_)))
        .count();
    let mut text = format!(
        "## Performance gates\n\n{} budgets, {failed} failed, {unmeasured} not measured.\n\n| status | counter | measured | budget | excess | scenario |\n|---|---|---|---|---|---|\n",
        verdicts.len()
    );
    let mut ordered: Vec<&Verdict> = verdicts.iter().collect();
    ordered.sort_by_key(|verdict| !verdict.failed());
    for verdict in ordered {
        text.push_str(&format!(
            "| {} | `{}` | {} | {} | {} | {} |\n",
            verdict.status(),
            verdict.counter,
            cell(&verdict.measured_text()),
            cell(&verdict.budget),
            cell(&verdict.excess_text()),
            cell(&verdict.scenario)
        ));
    }
    let commands = reproduce_commands(verdicts);
    if !commands.is_empty() {
        text.push_str("\nReproduce locally:\n\n```bash\n");
        for command in commands {
            text.push_str(command);
            text.push('\n');
        }
        text.push_str("```\n");
    }
    text
}

pub(super) fn verdicts_json(verdicts: &[Verdict]) -> Value {
    json!({
        "total": verdicts.len(),
        "failed": verdicts.iter().filter(|verdict| verdict.failed()).count(),
        "unmeasured": verdicts.iter().filter(|verdict| matches!(verdict.outcome, Outcome::Unmeasured(_))).map(|verdict| verdict.counter.clone()).collect::<Vec<_>>(),
        "verdicts": verdicts.iter().map(Verdict::to_json).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing() -> BTreeMap<String, Measurement> {
        BUDGETS
            .iter()
            .map(|budget| {
                let value = match budget.limit {
                    Limit::AtMost(limit) | Limit::Exactly(limit) => limit,
                    Limit::Below(limit) => limit - 1.0,
                    Limit::Within { baseline, .. } => baseline,
                };
                (budget.counter.to_string(), Measurement::Value(value))
            })
            .collect()
    }

    #[test]
    fn the_budgets_span_host_worker_and_desktop_with_a_margin_each() {
        let count = |prefix: &str| {
            BUDGETS
                .iter()
                .filter(|budget| budget.counter.starts_with(prefix))
                .count()
        };
        assert!(BUDGETS.len() >= 15, "{} budgets", BUDGETS.len());
        assert!(count("host.") >= 1 && count("worker.") >= 1 && count("desktop.") >= 1);
        let mut names: Vec<&str> = BUDGETS.iter().map(|budget| budget.counter).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BUDGETS.len(), "budget names are unique");
        for budget in BUDGETS {
            assert!(
                !budget.margin.is_empty(),
                "{} has no margin",
                budget.counter
            );
            assert!(!budget.unit.is_empty(), "{} has no unit", budget.counter);
        }
        assert!(failure_report(&verify(BUDGETS, &passing())).is_none());
    }

    #[test]
    fn a_synthetic_overrun_fails_the_verifier_with_its_line() {
        let mut measurements = passing();
        measurements.insert(
            "worker.idle.snapshot_broadcasts".to_string(),
            Measurement::Value(15.0),
        );
        measurements.insert(
            "host.active.process_listings_per_s".to_string(),
            Measurement::Value(14.4),
        );
        let verdicts = verify(BUDGETS, &measurements);
        let report = failure_report(&verdicts).expect("an overrun fails the verifier");
        assert!(
            report.starts_with("2 of 20 performance budgets failed\ncounter | measured | budget | excess | scenario\n"),
            "{report}"
        );
        assert!(
            report.contains(
                "worker.idle.snapshot_broadcasts | 15 snapshots per window | = 0 snapshots per window | +15 | host_worker_idle: 8 idle fixture sessions, host and worker without a desktop, 30 s window\n"
            ),
            "{report}"
        );
        assert!(
            report.contains(
                "host.active.process_listings_per_s | 14.400 listings per second | <= 2 listings per second | +12.400 (+620.00 %) | host_worker_active:"
            ),
            "{report}"
        );
        assert!(
            report.ends_with("reproduce locally: scripts/perf-gates.sh\n"),
            "{report}"
        );
        let summary = markdown(&verdicts);
        assert!(
            summary.contains("20 budgets, 2 failed, 0 not measured."),
            "{summary}"
        );
        assert!(
            summary.contains("| FAIL | `worker.idle.snapshot_broadcasts` | 15 snapshots per window | = 0 snapshots per window | +15 |"),
            "{summary}"
        );
        assert_eq!(verdicts_json(&verdicts)["failed"], 2);
    }

    #[test]
    fn a_missing_or_pending_measurement_is_never_accepted() {
        let mut measurements = passing();
        measurements.remove("desktop.idle.root_renders");
        measurements.insert(
            "host.idle.process_listings".to_string(),
            Measurement::Missing("pending: counters unavailable: host 0.17.5".to_string()),
        );
        let verdicts = verify(BUDGETS, &measurements);
        let report = failure_report(&verdicts).expect("an absent measurement fails");
        assert!(
            report.contains("host.idle.process_listings | missing | = 0 listings per window | no measurement: pending: counters unavailable: host 0.17.5 | host_worker_idle:"),
            "{report}"
        );
        assert!(
            report.contains("desktop.idle.root_renders | missing | <= 3 renders per window | no measurement: scenario desktop_idle recorded no value |"),
            "{report}"
        );
    }

    fn suite(metrics: Value) -> Value {
        json!({"os": "linux", "arch": "x86_64", "metrics": metrics})
    }

    #[test]
    fn allocations_fail_one_percent_above_or_below_their_baseline() {
        let baseline = suite(json!([
            {"metric": "steady", "available": true, "alloc_bytes_per_iter": 1000.0, "allocs_per_iter": 10.0},
            {"metric": "grew", "available": true, "alloc_bytes_per_iter": 1000.0, "allocs_per_iter": 10.0},
            {"metric": "shrank", "available": true, "alloc_bytes_per_iter": 1000.0, "allocs_per_iter": 10.0},
            {"metric": "count", "available": true, "alloc_bytes_per_iter": null, "allocs_per_iter": null},
        ]));
        let current = suite(json!([
            {"metric": "steady", "available": true, "alloc_bytes_per_iter": 1009.0, "allocs_per_iter": 10.0},
            {"metric": "grew", "available": true, "alloc_bytes_per_iter": 1020.0, "allocs_per_iter": 10.0},
            {"metric": "shrank", "available": true, "alloc_bytes_per_iter": 1000.0, "allocs_per_iter": 9.0},
            {"metric": "count", "available": true, "alloc_bytes_per_iter": null, "allocs_per_iter": null},
        ]));
        let verdicts =
            allocation_verdicts("terminal", &TERMINAL_SUITE, Ok(&current), Ok(&baseline));
        assert_eq!(
            verdicts.len(),
            6,
            "count metrics carry no allocation column"
        );
        let failed: Vec<String> = verdicts
            .iter()
            .filter(|verdict| verdict.failed())
            .map(Verdict::line)
            .collect();
        assert_eq!(failed.len(), 2, "{failed:?}");
        assert!(failed[0].starts_with("terminal.grew.alloc_bytes_per_iter | 1020 bytes per iteration | 1000 bytes per iteration +/- 1 % | +20 (+2.00 %) |"), "{failed:?}");
        assert!(failed[1].starts_with("terminal.shrank.allocs_per_iter | 9 allocations per iteration | 10 allocations per iteration +/- 1 % | -1 (-10.00 %): below the baseline, refresh it in this PR"), "{failed:?}");
    }

    #[test]
    fn an_unavailable_metric_is_reported_unmeasured_neither_accepted_nor_a_regression() {
        let baseline = suite(json!([
            {"metric": "shape_cold_60_rows", "available": true, "alloc_bytes_per_iter": 2000.0, "allocs_per_iter": 10.0},
        ]));
        let current = suite(json!([
            {"metric": "shape_cold_60_rows", "available": false, "note": "no real shaper on this machine", "alloc_bytes_per_iter": null, "allocs_per_iter": null},
        ]));
        let verdicts = allocation_verdicts("editor", &EDITOR_SUITE, Ok(&current), Ok(&baseline));
        assert_eq!(verdicts.len(), 2);
        assert!(verdicts.iter().all(|verdict| !verdict.failed()));
        assert!(verdicts.iter().all(|verdict| verdict.outcome
            == Outcome::Unmeasured("no real shaper on this machine".to_string())));
        assert_eq!(
            verdicts_json(&verdicts)["unmeasured"],
            json!([
                "editor.shape_cold_60_rows.alloc_bytes_per_iter",
                "editor.shape_cold_60_rows.allocs_per_iter"
            ])
        );
    }

    #[test]
    fn an_allocation_baseline_from_another_platform_or_a_missing_result_fails() {
        let windows = json!({"os": "windows", "arch": "x86_64", "metrics": []});
        let linux = suite(json!([]));
        let refused = allocation_verdicts("terminal", &TERMINAL_SUITE, Ok(&linux), Ok(&windows));
        assert_eq!(refused.len(), 1);
        assert!(refused[0].failed());
        assert!(
            refused[0].line().contains(
                "the baseline recorded on windows-x86_64; only linux-x86_64 is comparable"
            ),
            "{}",
            refused[0].line()
        );
        let missing = allocation_verdicts(
            "editor",
            &EDITOR_SUITE,
            Err("the suite wrote no result".to_string()),
            Ok(&linux),
        );
        assert!(missing[0].failed());
        let new_metric = allocation_verdicts(
            "terminal",
            &TERMINAL_SUITE,
            Ok(&suite(
                json!([{"metric": "fresh", "available": true, "alloc_bytes_per_iter": 1.0, "allocs_per_iter": 1.0}]),
            )),
            Ok(&linux),
        );
        assert!(new_metric.iter().all(Verdict::failed));
        assert!(
            new_metric[0]
                .line()
                .contains("has no baseline value: refresh the baseline in this PR")
        );
    }

    fn workflow(name: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../.github/workflows")
                .join(name),
        )
        .unwrap()
    }

    fn job_block(workflow: &str, job: &str) -> String {
        let header = format!("  {job}:");
        let block: Vec<&str> = workflow
            .lines()
            .skip_while(|line| *line != header)
            .enumerate()
            .take_while(|(index, line)| {
                *index == 0 || !(line.starts_with("  ") && !line.starts_with("   "))
            })
            .map(|(_, line)| line)
            .collect();
        assert!(!block.is_empty(), "no {job} job");
        block.join("\n")
    }

    #[test]
    fn the_perf_gates_job_runs_on_pull_request_with_its_own_cache_inside_tests_pass() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
        for entry in std::fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                !text.contains("pull_request_target"),
                "{} triggers on pull_request_target",
                path.display()
            );
        }
        let run_tests = workflow("run_tests.yml");
        let job = job_block(&run_tests, "perf_gates");
        assert!(job.contains("prefix-key: \"perf-gates-\""), "{job}");
        assert!(job.contains("timeout-minutes: 30"), "{job}");
        assert!(
            job.contains("uses: ./.github/actions/fetch-libghostty"),
            "{job}"
        );
        assert!(job.contains("scripts/perf-gates.sh"), "{job}");
        assert!(job.contains("retention-days: 14"), "{job}");
        let release = workflow("release.yml");
        assert!(
            !release.contains("perf-gates-"),
            "release.yml shares the perf-gates- cache prefix"
        );
        let tests_pass = job_block(&run_tests, "tests_pass");
        assert!(
            tests_pass.contains("\n      - perf_gates\n"),
            "{tests_pass}"
        );
        assert!(
            tests_pass.contains("needs.perf_gates.result"),
            "{tests_pass}"
        );
    }
}
