use super::*;

use std::collections::BTreeSet;

pub(super) const AB_SCHEMA_VERSION: u64 = 2;
pub(super) const MAXIMUM_REGRESSION: f64 = 0.10;
pub(super) const MAXIMUM_AA_DRIFT: f64 = 0.05;
pub(super) const MINIMUM_SAMPLES: usize = 50;
pub(super) const MINIMUM_ROUNDS: usize = 10;
pub(super) const ROUND_ORDER: [&str; 4] = ["base", "head", "head", "base"];
pub(super) const SUITES: [&str; 2] = ["terminal", "active"];
const AB_SESSIONS: usize = 8;
const AB_WINDOW: Duration = Duration::from_secs(15);
const AB_CPU_SLICES: u32 = 5;
const HOST_ROLES: [&str; 10] = [
    "host.session",
    "host.pty_reader",
    "host.pty_writer",
    "host.viewport_scan",
    "host.ipc_connection",
    "host.ipc_accept",
    "host.launch_owner",
    "merged_truncated_names",
    "main",
    "other",
];
const RESOLUTIONS: [(&str, f64); 3] = [("ns", 100.0), ("ms/s", 1.0), ("ms", 0.01)];
const BATCHES: [(&str, usize); 2] = [("terminal", 10), ("active", 1)];

fn batch_size(suite: &str) -> usize {
    BATCHES
        .iter()
        .find(|(known, _)| *known == suite)
        .map_or(1, |(_, size)| *size)
}

fn batch_means(samples: &[f64], size: usize) -> Vec<f64> {
    samples
        .chunks_exact(size)
        .map(|batch| batch.iter().sum::<f64>() / size as f64)
        .collect()
}

fn resolution(unit: &str) -> Option<f64> {
    RESOLUTIONS
        .iter()
        .find(|(known, _)| *known == unit)
        .map(|(_, floor)| *floor)
}

fn active_metrics(scenario: &Value) -> Result<Value, String> {
    let slices = scenario["host_cpu_ms_per_s_by_slice"]
        .as_array()
        .ok_or("the active scenario recorded no CPU slices")?;
    let mut by_role: BTreeMap<&str, Vec<f64>> =
        HOST_ROLES.iter().map(|role| (*role, Vec::new())).collect();
    let mut totals: Vec<f64> = Vec::with_capacity(slices.len());
    for slice in slices {
        if let Some(pending) = slice.get("pending") {
            return Err(format!("host CPU was not measured: {pending}"));
        }
        let roles = slice.as_object().ok_or("a CPU slice is not an object")?;
        if let Some(unknown) = roles
            .keys()
            .find(|role| !HOST_ROLES.contains(&role.as_str()))
        {
            return Err(format!(
                "host thread role {unknown} is not tracked by the A/B"
            ));
        }
        for (role, samples) in &mut by_role {
            samples.push(roles.get(*role).and_then(Value::as_f64).unwrap_or(0.0));
        }
        totals.push(roles.values().filter_map(Value::as_f64).sum());
    }
    let echoes: Vec<f64> = scenario["echo_ms"]["raw"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .collect();
    if echoes.len() != active::ACTIVE_ECHO_SAMPLES {
        return Err(format!(
            "{} of {} echo round trips came back",
            echoes.len(),
            active::ACTIVE_ECHO_SAMPLES
        ));
    }
    let mut metrics = serde_json::Map::new();
    for (role, samples) in by_role {
        metrics.insert(
            format!("cpu.{role}"),
            json!({"unit": "ms/s", "samples": samples}),
        );
    }
    metrics.insert(
        "cpu.total".to_string(),
        json!({"unit": "ms/s", "samples": totals}),
    );
    metrics.insert(
        "echo_round_trip".to_string(),
        json!({"unit": "ms", "samples": echoes}),
    );
    Ok(Value::Object(metrics))
}

#[test]
#[ignore = "A/B active sampler; run through scripts/perf-ab.sh or .ps1"]
fn perf_ab_active_samples() {
    allow_breakaway_like_the_desktop_does();
    let out = std::env::var_os("PANEFLOW_AB_SAMPLES_OUT")
        .map(PathBuf::from)
        .expect("PANEFLOW_AB_SAMPLES_OUT names the sample file; run scripts/perf-ab.sh");
    let home = tempfile::tempdir().unwrap();
    seed_home(home.path());
    let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
    let adoption = bootstrap::ensure_host_running(home.path(), &host_executable(), "perf-ab")
        .expect("the detached host starts");
    let hello = ClientHello::local("perf-ab");
    let mut client = HostClient::connect(&endpoint, &hello).unwrap();
    let host_identity = paneflow_host::ProcessIdentity::capture(adoption.identity.pid);
    let ledger = FixtureLedger::new();
    let echo = workloads::EchoProbe::start(&mut client, &endpoint, &ledger);
    let plan = active::ActivePlan {
        streams: AB_SESSIONS,
        stream_args: &active::STREAM_ARGS,
        flood_args: Some(&active::FLOOD_ARGS),
        settle: SETTLE,
        window: AB_WINDOW,
        cpu_slices: AB_CPU_SLICES,
    };
    let processes = active::ActiveProcesses {
        host_pid: adoption.identity.pid,
        worker: None,
        desktop_home: None,
        echo: Some(&echo),
    };
    let scenario = active::run_active_scenario(&mut client, &ledger, &plan, &processes);
    echo.finish(&mut client);
    let mut decisions = Vec::new();
    let shutdown = shutdown_host(
        &mut decisions,
        client,
        &host_identity,
        home.path(),
        &endpoint,
        &hello,
        &ledger,
    );
    let scenario = scenario.unwrap_or_else(|reason| panic!("the active scenario failed: {reason}"));
    if let Err(failures) = verdict(&decisions, &[]) {
        panic!(
            "host shutdown checks failed:\n{}\nshutdown record: {shutdown}",
            failures.join("\n")
        );
    }
    let metrics = active_metrics(&scenario)
        .unwrap_or_else(|reason| panic!("the active scenario is incomplete: {reason}"));
    write_document(
        &out,
        &json!({
            "suite": "active",
            "host": {"version": adoption.identity.version, "build_id": adoption.identity.build_id},
            "sessions": AB_SESSIONS,
            "window_s": AB_WINDOW.as_secs_f64(),
            "cpu_slices": AB_CPU_SLICES,
            "metrics": metrics,
        }),
    );
}

#[derive(Default)]
struct Cohorts {
    unit: Option<String>,
    units_differ: bool,
    first: Vec<f64>,
    last: Vec<f64>,
    head: Vec<f64>,
}

#[derive(Default)]
struct Attempt {
    rounds: BTreeSet<u32>,
    metrics: BTreeMap<String, Cohorts>,
}

fn slot_file(name: &str) -> Option<(u32, usize, &str, &str)> {
    let mut parts = name.strip_suffix(".json")?.split('-');
    let round = parts.next()?.strip_prefix('r')?.parse().ok()?;
    let slot: usize = parts.next()?.parse().ok()?;
    let artifact = parts.next()?;
    let suite = parts.next()?;
    parts
        .next()
        .is_none()
        .then_some((round, slot, artifact, suite))
}

fn load_attempt(dir: &Path) -> Result<Attempt, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("{} is unreadable: {error}", dir.display()))?;
    let mut attempt = Attempt::default();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some((round, slot, artifact, suite)) = slot_file(&name) else {
            continue;
        };
        let expected = slot
            .checked_sub(1)
            .and_then(|index| ROUND_ORDER.get(index))
            .ok_or_else(|| format!("{name}: a round has slots 1 to 4"))?;
        if artifact != *expected {
            return Err(format!("{name}: slot {slot} belongs to {expected}"));
        }
        let document: Value = std::fs::read(entry.path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| format!("{name} is not a JSON sample file"))?;
        let metrics = document["metrics"]
            .as_object()
            .ok_or_else(|| format!("{name} carries no metrics object"))?;
        for (metric, entry) in metrics {
            let cohorts = attempt
                .metrics
                .entry(format!("{suite}.{metric}"))
                .or_default();
            let unit = entry["unit"].as_str().unwrap_or_default();
            match &cohorts.unit {
                None => cohorts.unit = Some(unit.to_string()),
                Some(known) if known != unit => cohorts.units_differ = true,
                Some(_) => {}
            }
            let raw: Vec<f64> = entry["samples"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_f64)
                .filter(|value| value.is_finite() && *value >= 0.0)
                .collect();
            let samples = batch_means(&raw, batch_size(suite));
            match slot {
                1 => cohorts.first.extend(samples),
                4 => cohorts.last.extend(samples),
                _ => cohorts.head.extend(samples),
            }
        }
        attempt.rounds.insert(round);
    }
    Ok(attempt)
}

struct Distribution {
    count: usize,
    p50: f64,
    p95: f64,
}

impl Distribution {
    fn of(samples: &[f64]) -> Option<Self> {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let rank = |fraction: f64| {
            let index = ((fraction * sorted.len() as f64).ceil() as usize).max(1) - 1;
            sorted.get(index).copied()
        };
        Some(Self {
            count: sorted.len(),
            p50: rank(0.50)?,
            p95: rank(0.95)?,
        })
    }

    fn summary(&self) -> Value {
        json!({"count": self.count, "p50": self.p50, "p95": self.p95})
    }
}

fn distribution_json(samples: &[f64]) -> Value {
    let mut value = Distribution::of(samples).map_or_else(
        || json!({"count": 0, "p50": null, "p95": null}),
        |distribution| distribution.summary(),
    );
    value["samples"] = json!(samples);
    value
}

fn change(control: f64, candidate: f64, floor: f64) -> f64 {
    candidate.max(floor) / control.max(floor) - 1.0
}

fn compare_metric(name: &str, cohorts: &Cohorts) -> Value {
    let unit = cohorts.unit.clone().unwrap_or_default();
    let base: Vec<f64> = cohorts.first.iter().chain(&cohorts.last).copied().collect();
    let mut row = json!({
        "metric": name,
        "unit": unit,
        "base": distribution_json(&base),
        "head": distribution_json(&cohorts.head),
    });
    let excluded = |mut row: Value, verdict: &str, reason: String| {
        row["verdict"] = json!(verdict);
        row["reason"] = json!(reason);
        row
    };
    if cohorts.units_differ {
        return excluded(
            row,
            "not_compared",
            "base and head report different units".to_string(),
        );
    }
    let Some(floor) = resolution(&unit) else {
        return excluded(
            row,
            "not_compared",
            format!("no measurement resolution is declared for unit {unit:?}"),
        );
    };
    row["resolution"] = json!(floor);
    if base.is_empty() || cohorts.head.is_empty() {
        let only = if base.is_empty() { "head" } else { "base" };
        return excluded(row, "not_compared", format!("only {only} measures it"));
    }
    let short: Vec<String> = [
        ("first base pass of each round", &cohorts.first),
        ("last base pass of each round", &cohorts.last),
        ("head", &cohorts.head),
    ]
    .iter()
    .filter(|(_, samples)| samples.len() < MINIMUM_SAMPLES)
    .map(|(cohort, samples)| format!("{cohort}: {} samples", samples.len()))
    .collect();
    let (Some(base_d), Some(head_d), Some(first), Some(last)) = (
        Distribution::of(&base),
        Distribution::of(&cohorts.head),
        Distribution::of(&cohorts.first),
        Distribution::of(&cohorts.last),
    ) else {
        return excluded(row, "insufficient", "a cohort has no sample".to_string());
    };
    if !short.is_empty() {
        return excluded(
            row,
            "insufficient",
            format!(
                "at least {MINIMUM_SAMPLES} samples per cohort are required: {}",
                short.join(", ")
            ),
        );
    }
    let p50_change = change(base_d.p50, head_d.p50, floor);
    let p95_change = change(base_d.p95, head_d.p95, floor);
    let aa_p50 = change(first.p50, last.p50, floor);
    let aa_p95 = change(first.p95, last.p95, floor);
    let p95_judged = aa_p95.abs() <= MAXIMUM_AA_DRIFT;
    let exceeds = |control: f64, candidate: f64| {
        candidate.max(floor) > control.max(floor) * (1.0 + MAXIMUM_REGRESSION)
    };
    let regression =
        exceeds(base_d.p50, head_d.p50) || (p95_judged && exceeds(base_d.p95, head_d.p95));
    row["p50_change"] = json!(p50_change);
    row["p95_change"] = json!(p95_change);
    row["aa"] = json!({
        "first": first.summary(),
        "last": last.summary(),
        "p50_change": aa_p50,
        "p95_change": aa_p95,
    });
    row["calibrated"] = json!(aa_p50.abs() <= MAXIMUM_AA_DRIFT);
    row["p95_judged"] = json!(p95_judged);
    row["verdict"] = json!(if regression { "regression" } else { "pass" });
    row
}

fn attempt_verdict(rounds: usize, metrics: &[Value]) -> (&'static str, Vec<String>) {
    let mut insufficient = Vec::new();
    if rounds < MINIMUM_ROUNDS {
        insufficient.push(format!(
            "{rounds} rounds were measured, at least {MINIMUM_ROUNDS} are required"
        ));
    }
    for suite in SUITES {
        let compared = metrics.iter().any(|row| {
            row["metric"]
                .as_str()
                .is_some_and(|name| name.starts_with(&format!("{suite}.")))
                && matches!(row["verdict"].as_str(), Some("pass" | "regression"))
        });
        if !compared {
            insufficient.push(format!("the {suite} suite compared no metric"));
        }
    }
    let named = |verdict: &str| -> Vec<String> {
        metrics
            .iter()
            .filter(|row| row["verdict"] == verdict)
            .map(|row| {
                format!(
                    "{}: {}",
                    row["metric"].as_str().unwrap_or_default(),
                    row["reason"].as_str().unwrap_or(verdict)
                )
            })
            .collect()
    };
    insufficient.extend(named("insufficient"));
    if !insufficient.is_empty() {
        return ("insufficient", insufficient);
    }
    let uncalibrated: Vec<String> = metrics
        .iter()
        .filter(|row| row["calibrated"] == false)
        .map(|row| {
            format!(
                "A/A {} p50 {} p95 {}",
                row["metric"].as_str().unwrap_or_default(),
                percent(row["aa"]["p50_change"].as_f64()),
                percent(row["aa"]["p95_change"].as_f64())
            )
        })
        .collect();
    if !uncalibrated.is_empty() {
        return ("uncalibrated", uncalibrated);
    }
    let regressions: Vec<String> = metrics
        .iter()
        .filter(|row| row["verdict"] == "regression")
        .map(|row| {
            format!(
                "{} p50 {} p95 {}",
                row["metric"].as_str().unwrap_or_default(),
                percent(row["p50_change"].as_f64()),
                percent(row["p95_change"].as_f64())
            )
        })
        .collect();
    if regressions.is_empty() {
        ("pass", Vec::new())
    } else {
        ("regression", regressions)
    }
}

fn compare_attempt(number: usize, dir: &Path) -> Value {
    let attempt = match load_attempt(dir) {
        Ok(attempt) => attempt,
        Err(reason) => {
            return json!({"attempt": number, "rounds": 0, "verdict": "insufficient", "reasons": [reason], "metrics": []});
        }
    };
    let metrics: Vec<Value> = attempt
        .metrics
        .iter()
        .map(|(name, cohorts)| compare_metric(name, cohorts))
        .collect();
    let (verdict, reasons) = attempt_verdict(attempt.rounds.len(), &metrics);
    json!({
        "attempt": number,
        "rounds": attempt.rounds.len(),
        "verdict": verdict,
        "reasons": reasons,
        "metrics": metrics,
    })
}

fn outcome(attempts: &[&str]) -> (&'static str, &'static str, &'static str) {
    if attempts.contains(&"insufficient") {
        return (
            "insufficient",
            "excluded",
            "a measurement was missing, so no verdict was reached",
        );
    }
    match attempts {
        ["pass"] | ["regression", "regression"] => (
            if attempts.len() == 1 {
                "pass"
            } else {
                "regression"
            },
            "calibrated",
            "calibrated, and any regression confirmed by a second execution",
        ),
        ["regression", "pass"] => (
            "unconfirmed_regression",
            "resets",
            "the second execution did not confirm the regression: a parasitic verdict",
        ),
        ["uncalibrated", "pass"] => (
            "pass",
            "uncalibrated",
            "the first execution was not calibrated; the run is never held against the promotion",
        ),
        ["uncalibrated", "regression"] => (
            "unconfirmed_regression",
            "uncalibrated",
            "the first execution was not calibrated and the second regression stands alone",
        ),
        ["uncalibrated", "uncalibrated"] | ["regression", "uncalibrated"] | ["uncalibrated"] => (
            "uncalibrated",
            "uncalibrated",
            "an A/A p50 drifted more than 5 %, so the run is rejected",
        ),
        _ => (
            "insufficient",
            "excluded",
            "the attempts do not follow the rerun rule",
        ),
    }
}

fn identity(side: &str) -> Value {
    let read =
        |suffix: &str| std::env::var(format!("PANEFLOW_AB_{}_{suffix}", side.to_uppercase())).ok();
    json!({"ref": read("REF"), "commit": read("SHA")})
}

fn result_document(attempts: Vec<Value>) -> Value {
    let verdicts: Vec<&str> = attempts
        .iter()
        .map(|attempt| attempt["verdict"].as_str().unwrap_or("insufficient"))
        .collect();
    let (verdict, effect, reason) = outcome(&verdicts);
    json!({
        "suite": "paneflow-perf-ab",
        "schema_version": AB_SCHEMA_VERSION,
        "stamp": stamp(),
        "base": identity("base"),
        "head": identity("head"),
        "machine": machine(),
        "toolchain": toolchain(),
        "rule": {
            "round_order": ROUND_ORDER,
            "minimum_rounds": MINIMUM_ROUNDS,
            "minimum_samples_per_cohort": MINIMUM_SAMPLES,
            "maximum_regression": MAXIMUM_REGRESSION,
            "maximum_aa_drift": MAXIMUM_AA_DRIFT,
            "aa_cohort": "the first base pass of every round against its last base pass",
            "calibration": "the run is uncalibrated when any metric's A/A p50 drifts more than maximum_aa_drift; a metric's p95 is judged only when its own A/A p95 drift is within maximum_aa_drift",
            "batch_size_by_suite": BATCHES.iter().map(|(suite, size)| (suite.to_string(), json!(size))).collect::<serde_json::Map<_, _>>(),
            "resolution_by_unit": RESOLUTIONS.iter().map(|(unit, floor)| (unit.to_string(), json!(floor))).collect::<serde_json::Map<_, _>>(),
            "rerun": "a regression or an uncalibrated first execution is measured once more; only two calibrated regressions make a regression verdict",
        },
        "verdict": verdict,
        "promotion": {"effect": effect, "reason": reason},
        "attempts": attempts,
    })
}

fn percent(value: Option<f64>) -> String {
    value.map_or("n/a".to_string(), |value| {
        format!("{:+.1} %", value * 100.0)
    })
}

fn quantity(value: &Value) -> String {
    value
        .as_f64()
        .map_or("n/a".to_string(), |value| format!("{value:.3}"))
}

fn short_commit(side: &Value) -> String {
    let commit = side["commit"].as_str().unwrap_or("unknown");
    format!(
        "`{}` ({})",
        side["ref"].as_str().unwrap_or("unknown"),
        &commit[..commit.len().min(12)]
    )
}

fn verdict_rank(row: &Value) -> u8 {
    match (row["verdict"].as_str(), row["calibrated"].as_bool()) {
        (Some("regression"), _) => 0,
        (_, Some(false)) => 1,
        (Some("insufficient"), _) => 2,
        _ if row["p95_judged"] == false => 3,
        (Some("not_compared"), _) => 4,
        _ => 5,
    }
}

fn markdown(result: &Value) -> String {
    let mut text = format!(
        "## Real-time A/B: {}\n\nBase {} against head {}, rounds ordered {}, rule: head p50, and p95 where its own A/A holds, at most +{:.0} % over base, A/A p50 drift at most {:.0} %.\n\nPromotion: **{}** ({}).\n\n",
        result["verdict"].as_str().unwrap_or("unknown"),
        short_commit(&result["base"]),
        short_commit(&result["head"]),
        ROUND_ORDER.join(" "),
        MAXIMUM_REGRESSION * 100.0,
        MAXIMUM_AA_DRIFT * 100.0,
        result["promotion"]["effect"].as_str().unwrap_or("unknown"),
        result["promotion"]["reason"].as_str().unwrap_or_default(),
    );
    for attempt in result["attempts"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "### Attempt {}: {} over {} rounds\n\n",
            attempt["attempt"],
            attempt["verdict"].as_str().unwrap_or("unknown"),
            attempt["rounds"]
        ));
        for reason in attempt["reasons"].as_array().into_iter().flatten() {
            text.push_str(&format!("- {}\n", reason.as_str().unwrap_or_default()));
        }
        text.push_str("\n| verdict | metric | unit | base p50 | head p50 | Δ p50 | base p95 | head p95 | Δ p95 | A/A Δ p50 | A/A Δ p95 |\n|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut rows: Vec<&Value> = attempt["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .collect();
        rows.sort_by_key(|row| verdict_rank(row));
        for row in rows {
            let verdict = match (row["verdict"].as_str(), row["calibrated"].as_bool()) {
                (Some(verdict), Some(false)) => format!("{verdict}, uncalibrated"),
                (Some(verdict), _) if row["p95_judged"] == false => {
                    format!("{verdict}, p95 not judged")
                }
                (Some(verdict), _) => verdict.to_string(),
                (None, _) => "unknown".to_string(),
            };
            text.push_str(&format!(
                "| {verdict} | `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                row["metric"].as_str().unwrap_or_default(),
                row["unit"].as_str().unwrap_or_default(),
                quantity(&row["base"]["p50"]),
                quantity(&row["head"]["p50"]),
                percent(row["p50_change"].as_f64()),
                quantity(&row["base"]["p95"]),
                quantity(&row["head"]["p95"]),
                percent(row["p95_change"].as_f64()),
                percent(row["aa"]["p50_change"].as_f64()),
                percent(row["aa"]["p95_change"].as_f64()),
            ));
        }
        text.push('\n');
    }
    text
}

#[test]
#[ignore = "A/B comparator; run through scripts/perf-ab.sh or .ps1"]
fn perf_ab_compare() {
    let dir = std::env::var_os("PANEFLOW_AB_DIR")
        .map(PathBuf::from)
        .expect("PANEFLOW_AB_DIR names the A/B output directory; run scripts/perf-ab.sh");
    let attempts: Vec<Value> = (1..)
        .map(|number| (number, dir.join(format!("attempt-{number}"))))
        .take_while(|(_, path)| path.is_dir())
        .map(|(number, path)| compare_attempt(number, &path))
        .collect();
    assert!(
        !attempts.is_empty(),
        "no attempt-1 directory under {}",
        dir.display()
    );
    let last_verdict = attempts
        .last()
        .and_then(|attempt| attempt["verdict"].as_str())
        .unwrap_or("insufficient")
        .to_string();
    let result = result_document(attempts);
    write_document(&dir.join("result.json"), &result);
    let summary = markdown(&result);
    std::fs::write(dir.join("summary.md"), &summary).unwrap();
    std::fs::write(dir.join("attempt-verdict"), last_verdict).unwrap();
    std::fs::write(
        dir.join("verdict"),
        result["verdict"].as_str().unwrap_or("insufficient"),
    )
    .unwrap();
    print!("{summary}");
    println!("report: {}", dir.join("result.json").display());
}

pub(super) const PROMOTION_RUNS: usize = 30;
pub(super) const PROMOTION_SPAN: Duration = Duration::from_secs(21 * 24 * 3600);
pub(super) const PROMOTION_CALIBRATED_SHARE: f64 = 0.90;
pub(super) const EXPIRED_MARKER: &str = "artifact-expired";
pub(super) const NO_ARTIFACT_MARKER: &str = "no-artifact";

#[derive(Debug, Clone, PartialEq)]
struct GateRun {
    run: String,
    stamp_s: u64,
    effect: String,
    verdict: String,
}

#[derive(Debug, PartialEq)]
enum RunRecord {
    Gate(GateRun),
    Skipped {
        run: String,
        reason: String,
    },
    Expired {
        run: String,
        completed_s: Option<u64>,
    },
    Unknown {
        run: String,
        reason: String,
    },
}

fn classify_run(run: &str, dir: &Path) -> RunRecord {
    let skipped = |reason: String| RunRecord::Skipped {
        run: run.to_string(),
        reason,
    };
    if let Ok(marker) = std::fs::read_to_string(dir.join(EXPIRED_MARKER)) {
        return RunRecord::Expired {
            run: run.to_string(),
            completed_s: marker.trim().parse().ok(),
        };
    }
    if dir.join(NO_ARTIFACT_MARKER).exists() {
        return skipped("the run stopped before the A/B uploaded anything".to_string());
    }
    let Ok(text) = std::fs::read_to_string(dir.join("result.json")) else {
        return skipped(
            "no result.json: the base or the head was unavailable, so no verdict was reached"
                .to_string(),
        );
    };
    let Ok(result) = serde_json::from_str::<Value>(&text) else {
        return RunRecord::Unknown {
            run: run.to_string(),
            reason: "result.json is not valid JSON".to_string(),
        };
    };
    let schema = result["schema_version"].as_u64().unwrap_or(0);
    if schema != AB_SCHEMA_VERSION {
        return skipped(format!(
            "measured under schema {schema}, not the current rule (schema {AB_SCHEMA_VERSION})"
        ));
    }
    let effect = result["promotion"]["effect"].as_str().unwrap_or("excluded");
    if !matches!(effect, "calibrated" | "uncalibrated" | "resets") {
        return skipped(format!(
            "promotion.effect is {effect}: a measurement was missing"
        ));
    }
    let Some(stamp_s) = result["stamp"]
        .as_str()
        .and_then(|stamp| stamp.strip_prefix("unix"))
        .and_then(|seconds| seconds.parse().ok())
    else {
        return RunRecord::Unknown {
            run: run.to_string(),
            reason: "its stamp carries no unix time".to_string(),
        };
    };
    RunRecord::Gate(GateRun {
        run: run.to_string(),
        stamp_s,
        effect: effect.to_string(),
        verdict: result["verdict"].as_str().unwrap_or_default().to_string(),
    })
}

fn rates(runs: &[&GateRun]) -> Value {
    let share = |count: usize| {
        if runs.is_empty() {
            Value::Null
        } else {
            json!(count as f64 / runs.len() as f64)
        }
    };
    let calibrated = runs.iter().filter(|run| run.effect == "calibrated").count();
    let resets = runs.iter().filter(|run| run.effect == "resets").count();
    let unconfirmed = runs
        .iter()
        .filter(|run| run.verdict == "unconfirmed_regression")
        .count();
    json!({
        "runs": runs.len(),
        "calibrated": calibrated,
        "calibrated_share": share(calibrated),
        "resets": resets,
        "unconfirmed_regressions": unconfirmed,
        "unconfirmed_regression_share": share(unconfirmed),
        "first_stamp_s": runs.first().map(|run| run.stamp_s),
        "last_stamp_s": runs.last().map(|run| run.stamp_s),
        "span_days": runs.first().zip(runs.last()).map(|(first, last)| (last.stamp_s - first.stamp_s) as f64 / 86_400.0),
    })
}

fn promotion_tally(records: &[RunRecord]) -> Value {
    let mut gate: Vec<&GateRun> = records
        .iter()
        .filter_map(|record| match record {
            RunRecord::Gate(run) => Some(run),
            _ => None,
        })
        .collect();
    gate.sort_by(|left, right| (left.stamp_s, &left.run).cmp(&(right.stamp_s, &right.run)));
    let before_latest_window = |completed_s: u64| {
        gate.iter().filter(|run| run.stamp_s > completed_s).count() >= PROMOTION_RUNS
    };
    let mut unknown = Vec::new();
    let mut skipped = Vec::new();
    for record in records {
        match record {
            RunRecord::Gate(_) => {}
            RunRecord::Skipped { run, reason } => skipped.push(json!({"run": run, "reason": reason})),
            RunRecord::Unknown { run, reason } => unknown.push(json!({"run": run, "reason": reason})),
            RunRecord::Expired { run, completed_s: Some(completed_s) } if before_latest_window(*completed_s) => skipped.push(json!({
                "run": run,
                "reason": format!("its artifact expired, but it finished before the latest {PROMOTION_RUNS} counted runs, so it cannot change the decision"),
            })),
            RunRecord::Expired { run, .. } => unknown.push(json!({
                "run": run,
                "reason": format!("its artifact expired before it was downloaded, and it may fall within the latest {PROMOTION_RUNS} counted runs"),
            })),
        }
    }
    let streak_start = gate
        .iter()
        .rposition(|run| run.effect == "resets")
        .map_or(0, |index| index + 1);
    let latest = &gate[gate.len().saturating_sub(PROMOTION_RUNS)..];
    let latest_rates = rates(latest);
    let span = latest
        .first()
        .zip(latest.last())
        .map_or(0, |(first, last)| last.stamp_s - first.stamp_s);
    let needed_calibrated = (PROMOTION_CALIBRATED_SHARE * PROMOTION_RUNS as f64).ceil() as u64;
    let calibrated = latest_rates["calibrated"].as_u64().unwrap_or(0);
    let resets = latest_rates["resets"].as_u64().unwrap_or(0);
    let mut quality_unmet = Vec::new();
    if calibrated < needed_calibrated {
        quality_unmet.push(format!(
            "{calibrated} calibrated, {needed_calibrated} needed"
        ));
    }
    if resets > 0 {
        quality_unmet.push(format!(
            "{resets} unconfirmed regression(s) reset the count"
        ));
    }
    let mut unmet = Vec::new();
    if latest.len() < PROMOTION_RUNS {
        unmet.push(format!("{} of {PROMOTION_RUNS} runs", latest.len()));
    }
    if span < PROMOTION_SPAN.as_secs() {
        unmet.push(format!(
            "spanning {:.1} of {} days",
            span as f64 / 86_400.0,
            PROMOTION_SPAN.as_secs() / 86_400
        ));
    }
    unmet.extend(quality_unmet.iter().cloned());
    let decision = if !unknown.is_empty() {
        "incomplete"
    } else if latest.len() < PROMOTION_RUNS {
        "pending"
    } else if !quality_unmet.is_empty() {
        "keep_consultative"
    } else if span < PROMOTION_SPAN.as_secs() {
        "pending"
    } else {
        "promote"
    };
    json!({
        "suite": "paneflow-perf-ab-promotion",
        "schema_version": AB_SCHEMA_VERSION,
        "rule": {
            "runs": PROMOTION_RUNS,
            "minimum_span_days": PROMOTION_SPAN.as_secs() / 86_400,
            "minimum_calibrated_share": PROMOTION_CALIBRATED_SHARE,
            "counted": "runs whose promotion.effect is calibrated, uncalibrated or resets, in stamp order; resets restarts the consecutive count",
            "skipped": "runs without result.json (base or head unavailable), with promotion.effect excluded, measured under an older schema, or expired after finishing before the latest counted runs",
        },
        "decision": decision,
        "unmet": unmet,
        "all_runs": rates(&gate),
        "latest_runs": latest_rates,
        "consecutive_since_last_reset": gate.len() - streak_start,
        "runs": gate.iter().map(|run| json!({"run": run.run, "stamp_s": run.stamp_s, "effect": run.effect, "verdict": run.verdict})).collect::<Vec<_>>(),
        "skipped": skipped,
        "unknown": unknown,
    })
}

fn promotion_markdown(tally: &Value) -> String {
    let share = |value: &Value| {
        value
            .as_f64()
            .map_or("n/a".to_string(), |share| format!("{:.0} %", share * 100.0))
    };
    let all = &tally["all_runs"];
    let latest = &tally["latest_runs"];
    let mut text = format!(
        "## Real-time A/B promotion\n\nDecision: **{}**.\n\n| Window | Runs | Calibrated | Unconfirmed regressions | Resets | Span |\n|---|---|---|---|---|---|\n",
        tally["decision"].as_str().unwrap_or_default()
    );
    for (name, window) in [("all counted runs", all), ("latest 30", latest)] {
        text.push_str(&format!(
            "| {name} | {} | {} | {} | {} | {:.1} days |\n",
            window["runs"],
            share(&window["calibrated_share"]),
            share(&window["unconfirmed_regression_share"]),
            window["resets"],
            window["span_days"].as_f64().unwrap_or(0.0),
        ));
    }
    text.push_str(&format!(
        "\nConsecutive runs since the last reset: {}.\n",
        tally["consecutive_since_last_reset"]
    ));
    let unmet: Vec<&str> = tally["unmet"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if !unmet.is_empty() {
        text.push_str(&format!("Unmet: {}.\n", unmet.join("; ")));
    }
    for (title, key) in [("Skipped", "skipped"), ("Unknown", "unknown")] {
        let entries = tally[key].as_array().cloned().unwrap_or_default();
        if !entries.is_empty() {
            text.push_str(&format!("\n{title}:\n\n"));
            for entry in entries {
                text.push_str(&format!(
                    "- {}: {}\n",
                    entry["run"].as_str().unwrap_or_default(),
                    entry["reason"].as_str().unwrap_or_default()
                ));
            }
        }
    }
    text
}

#[test]
#[ignore = "real-time promotion count; run through scripts/perf-ab-promotion.sh"]
fn perf_ab_promotion() {
    let dir = std::env::var_os("PANEFLOW_AB_PROMOTION_DIR")
        .map(PathBuf::from)
        .expect(
            "PANEFLOW_AB_PROMOTION_DIR names the downloaded runs; run scripts/perf-ab-promotion.sh",
        );
    let runs = dir.join("runs");
    let records: Vec<RunRecord> = std::fs::read_dir(&runs)
        .unwrap_or_else(|error| panic!("{}: {error}", runs.display()))
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .map(|entry| classify_run(&entry.file_name().to_string_lossy(), &entry.path()))
        .collect();
    let tally = promotion_tally(&records);
    write_document(&dir.join("promotion.json"), &tally);
    let summary = promotion_markdown(&tally);
    std::fs::write(dir.join("promotion.md"), &summary).unwrap();
    print!("{summary}");
    println!("report: {}", dir.join("promotion.json").display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cohorts(unit: &str, first: f64, last: f64, head: f64, count: usize) -> Cohorts {
        let spread = |center: f64| -> Vec<f64> {
            (0..count)
                .map(|index| center * (0.98 + 0.04 * index as f64 / count as f64))
                .collect()
        };
        Cohorts {
            unit: Some(unit.to_string()),
            units_differ: false,
            first: spread(first),
            last: spread(last),
            head: spread(head),
        }
    }

    fn rows(metrics: &[(&str, Cohorts)]) -> Vec<Value> {
        metrics
            .iter()
            .map(|(name, cohorts)| compare_metric(name, cohorts))
            .collect()
    }

    #[test]
    fn percentiles_use_the_nearest_rank_of_pf() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let distribution = Distribution::of(&samples).unwrap();
        assert_eq!(distribution.p50, 50.0);
        assert_eq!(distribution.p95, 95.0);
        assert!(Distribution::of(&[]).is_none());
    }

    #[test]
    fn a_head_more_than_ten_percent_slower_at_p50_or_p95_regresses() {
        let slower = compare_metric(
            "terminal.publish",
            &cohorts("ns", 1000.0, 1000.0, 1120.0, 60),
        );
        assert_eq!(slower["verdict"], "regression", "{slower}");
        assert_eq!(slower["calibrated"], true);
        let within = compare_metric(
            "terminal.publish",
            &cohorts("ns", 1000.0, 1000.0, 1080.0, 60),
        );
        assert_eq!(within["verdict"], "pass", "{within}");
        let mut tail = cohorts("ns", 1000.0, 1000.0, 1000.0, 100);
        for sample in tail.head.iter_mut().skip(90) {
            *sample *= 1.5;
        }
        let tail = compare_metric("terminal.publish", &tail);
        assert_eq!(
            tail["verdict"], "regression",
            "a p95 regression alone fails: {tail}"
        );
        let (verdict, reasons) = attempt_verdict(
            10,
            &rows(&[
                (
                    "terminal.publish",
                    cohorts("ns", 1000.0, 1000.0, 1120.0, 60),
                ),
                ("active.cpu.total", cohorts("ms/s", 50.0, 50.0, 50.0, 60)),
            ]),
        );
        assert_eq!(verdict, "regression");
        assert!(
            reasons[0].starts_with("terminal.publish p50 +12"),
            "{reasons:?}"
        );
    }

    #[test]
    fn an_aa_drift_above_five_percent_rejects_the_run_as_uncalibrated() {
        let metrics = rows(&[
            (
                "terminal.publish",
                cohorts("ns", 1000.0, 1060.0, 1500.0, 60),
            ),
            ("active.cpu.total", cohorts("ms/s", 50.0, 50.0, 50.0, 60)),
        ]);
        assert_eq!(metrics[0]["calibrated"], false);
        let (verdict, reasons) = attempt_verdict(10, &metrics);
        assert_eq!(
            verdict, "uncalibrated",
            "uncalibration outranks a regression"
        );
        assert!(
            reasons[0].contains("A/A terminal.publish p50 +6"),
            "{reasons:?}"
        );
        let calibrated = compare_metric(
            "terminal.publish",
            &cohorts("ns", 1000.0, 1040.0, 1000.0, 60),
        );
        assert_eq!(calibrated["calibrated"], true);
    }

    #[test]
    fn a_p95_whose_own_aa_drifts_is_not_judged_and_never_rejects_the_run() {
        let inflate_tail = |samples: &mut Vec<f64>| {
            for sample in samples.iter_mut().skip(90) {
                *sample *= 1.5;
            }
        };
        let mut noisy_tail = cohorts("ns", 1000.0, 1000.0, 1000.0, 100);
        inflate_tail(&mut noisy_tail.last);
        inflate_tail(&mut noisy_tail.head);
        let noisy_tail = compare_metric("terminal.layout", &noisy_tail);
        assert_eq!(noisy_tail["calibrated"], true, "{noisy_tail}");
        assert_eq!(noisy_tail["p95_judged"], false, "{noisy_tail}");
        assert_eq!(
            noisy_tail["verdict"], "pass",
            "a +50 % p95 is not judged when its own A/A p95 drifts: {noisy_tail}"
        );
        let mut slower = cohorts("ns", 1000.0, 1000.0, 1120.0, 100);
        inflate_tail(&mut slower.last);
        assert_eq!(
            compare_metric("terminal.layout", &slower)["verdict"],
            "regression",
            "the p50 is still judged"
        );
        let (verdict, reasons) = attempt_verdict(
            10,
            &[
                noisy_tail,
                compare_metric("active.cpu.total", &cohorts("ms/s", 50.0, 50.0, 50.0, 60)),
            ],
        );
        assert_eq!((verdict, reasons), ("pass", Vec::new()));
    }

    #[test]
    fn fewer_than_fifty_samples_or_ten_rounds_is_never_a_verdict() {
        let thin = compare_metric("active.echo_round_trip", &cohorts("ms", 1.0, 1.0, 9.0, 49));
        assert_eq!(thin["verdict"], "insufficient", "{thin}");
        let healthy = [
            (
                "terminal.publish",
                cohorts("ns", 1000.0, 1000.0, 1000.0, 60),
            ),
            ("active.cpu.total", cohorts("ms/s", 50.0, 50.0, 50.0, 60)),
        ];
        assert_eq!(attempt_verdict(10, &rows(&healthy)).0, "pass");
        let (verdict, reasons) = attempt_verdict(9, &rows(&healthy));
        assert_eq!(verdict, "insufficient");
        assert!(reasons[0].contains("9 rounds"), "{reasons:?}");
        let (verdict, reasons) = attempt_verdict(10, &rows(&healthy[..1]));
        assert_eq!(verdict, "insufficient");
        assert!(reasons.contains(&"the active suite compared no metric".to_string()));
    }

    #[test]
    fn values_below_the_resolution_compare_at_the_resolution() {
        let idle = compare_metric(
            "active.cpu.host.ipc_accept",
            &cohorts("ms/s", 0.0, 0.2, 0.9, 60),
        );
        assert_eq!(idle["verdict"], "pass", "{idle}");
        assert_eq!(idle["calibrated"], true);
        let woke = compare_metric(
            "active.cpu.host.ipc_accept",
            &cohorts("ms/s", 0.0, 0.0, 5.0, 60),
        );
        assert_eq!(woke["verdict"], "regression", "{woke}");
        assert_eq!(change(0.0, 0.0, 1.0), 0.0);
    }

    #[test]
    fn a_metric_measured_by_one_side_only_or_in_another_unit_is_not_compared() {
        let mut added = cohorts("ns", 1000.0, 1000.0, 1000.0, 60);
        added.first.clear();
        added.last.clear();
        let added = compare_metric("terminal.new_scenario", &added);
        assert_eq!(added["verdict"], "not_compared");
        assert_eq!(added["reason"], "only head measures it");
        let mut renamed = cohorts("ns", 1000.0, 1000.0, 1000.0, 60);
        renamed.units_differ = true;
        assert_eq!(
            compare_metric("terminal.x", &renamed)["verdict"],
            "not_compared"
        );
        let unknown = compare_metric("terminal.x", &cohorts("MiB/s", 1.0, 1.0, 1.0, 60));
        assert_eq!(unknown["verdict"], "not_compared");
    }

    #[test]
    fn only_two_calibrated_regressions_make_a_regression_verdict() {
        let effect = |attempts: &[&str]| {
            let (verdict, effect, _) = outcome(attempts);
            (verdict, effect)
        };
        assert_eq!(effect(&["pass"]), ("pass", "calibrated"));
        assert_eq!(
            effect(&["regression", "regression"]),
            ("regression", "calibrated")
        );
        assert_eq!(
            effect(&["regression", "pass"]),
            ("unconfirmed_regression", "resets")
        );
        assert_eq!(effect(&["uncalibrated", "pass"]), ("pass", "uncalibrated"));
        assert_eq!(
            effect(&["uncalibrated", "regression"]),
            ("unconfirmed_regression", "uncalibrated")
        );
        assert_eq!(
            effect(&["uncalibrated", "uncalibrated"]),
            ("uncalibrated", "uncalibrated")
        );
        assert_eq!(
            effect(&["regression", "uncalibrated"]),
            ("uncalibrated", "uncalibrated")
        );
        assert_eq!(
            effect(&["pass", "insufficient"]),
            ("insufficient", "excluded")
        );
        assert_eq!(effect(&["regression"]), ("insufficient", "excluded"));
    }

    fn sample_file(dir: &Path, name: &str, metric: &str, samples: &[f64]) {
        std::fs::write(
            dir.join(name),
            json!({"metrics": {metric: {"unit": "ns", "samples": samples}}}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn terminal_iterations_compare_as_means_of_ten_consecutive_iterations() {
        let mut iterations = vec![100.0; 25];
        iterations[3] = 1100.0;
        assert_eq!(
            batch_means(&iterations, batch_size("terminal")),
            [200.0, 100.0]
        );
        assert_eq!(batch_size("active"), 1);
        let dir = tempfile::tempdir().unwrap();
        sample_file(
            dir.path(),
            "r01-1-base-terminal.json",
            "publish",
            &iterations,
        );
        let attempt = load_attempt(dir.path()).unwrap();
        assert_eq!(attempt.metrics["terminal.publish"].first, [200.0, 100.0]);
    }

    #[test]
    fn sample_files_feed_the_cohort_of_their_slot() {
        let dir = tempfile::tempdir().unwrap();
        for (slot, side, value) in [
            (1, "base", 1.0),
            (2, "head", 2.0),
            (3, "head", 3.0),
            (4, "base", 4.0),
        ] {
            sample_file(
                dir.path(),
                &format!("r01-{slot}-{side}-active.json"),
                "publish",
                &[value],
            );
        }
        std::fs::write(dir.path().join("r01-1-base-active.log"), "ignored").unwrap();
        let attempt = load_attempt(dir.path()).unwrap();
        assert_eq!(attempt.rounds.len(), 1);
        let cohorts = &attempt.metrics["active.publish"];
        assert_eq!(cohorts.first, [1.0]);
        assert_eq!(cohorts.last, [4.0]);
        let mut head = cohorts.head.clone();
        head.sort_by(f64::total_cmp);
        assert_eq!(head, [2.0, 3.0]);
        sample_file(dir.path(), "r02-2-base-active.json", "publish", &[1.0]);
        let error = load_attempt(dir.path()).err().unwrap();
        assert_eq!(error, "r02-2-base-active.json: slot 2 belongs to head");
    }

    #[test]
    fn the_active_sampler_rejects_missing_echoes_and_unmeasured_cpu() {
        let echoes = vec![1.0; active::ACTIVE_ECHO_SAMPLES];
        let scenario = json!({
            "host_cpu_ms_per_s_by_slice": [{"host.session": 90.0, "other": 1.5}],
            "echo_ms": {"raw": echoes},
        });
        let metrics = active_metrics(&scenario).unwrap();
        assert_eq!(metrics["cpu.host.session"]["samples"], json!([90.0]));
        assert_eq!(metrics["cpu.host.ipc_accept"]["samples"], json!([0.0]));
        assert_eq!(metrics["cpu.total"]["samples"], json!([91.5]));
        let mut short = scenario.clone();
        short["echo_ms"]["raw"] = json!([1.0]);
        assert!(active_metrics(&short).unwrap_err().starts_with("1 of 200"));
        let mut pending = scenario.clone();
        pending["host_cpu_ms_per_s_by_slice"] = json!([{"pending": "no procfs"}]);
        assert!(active_metrics(&pending).unwrap_err().contains("no procfs"));
    }

    fn script_repository() -> (tempfile::TempDir, PathBuf) {
        let repository = tempfile::tempdir().unwrap();
        let root = repository.path().to_path_buf();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        git(&["init", "--quiet"]);
        git(&["config", "user.email", "ab@example.invalid"]);
        git(&["config", "user.name", "A/B fixture"]);
        git(&["config", "core.autocrlf", "false"]);
        let scripts = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts");
        std::fs::create_dir(root.join("scripts")).unwrap();
        for script in ["perf-ab.sh", "perf-ab.ps1"] {
            std::fs::copy(scripts.join(script), root.join("scripts").join(script)).unwrap();
        }
        std::fs::write(root.join("scripts/fetch-libghostty.sh"), "exit 0\n").unwrap();
        std::fs::write(
            root.join("scripts/fetch-libghostty.ps1"),
            "param([string[]]$Target)\nexit 0\n",
        )
        .unwrap();
        std::fs::write(root.join("broken"), "the base does not compile\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        std::fs::remove_file(root.join("broken")).unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        (repository, root)
    }

    #[test]
    fn an_unbuildable_base_is_reported_unavailable_with_its_own_exit_code() {
        let (_repository, root) = script_repository();
        let tools = tempfile::tempdir().unwrap();
        let scratch = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        let (cargo, mut command) = {
            let cargo = tools.path().join("cargo");
            std::fs::write(
                &cargo,
                "#!/bin/sh\nif [ -f broken ]; then\n  echo 'error[E0425]: cannot find value `x` in this scope' >&2\n  echo 'error: could not compile `paneflow-app`' >&2\n  exit 101\nfi\nexit 0\n",
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut command = Command::new("bash");
            command.arg(root.join("scripts/perf-ab.sh"));
            command.env("TMPDIR", scratch.path());
            (cargo, command)
        };
        #[cfg(windows)]
        let (cargo, mut command) = {
            let cargo = tools.path().join("cargo.cmd");
            std::fs::write(
                &cargo,
                "@echo off\r\nif exist broken (\r\n  echo error[E0425]: cannot find value `x` in this scope 1>&2\r\n  echo error: could not compile `paneflow-app` 1>&2\r\n  exit /b 101\r\n)\r\nexit /b 0\r\n",
            )
            .unwrap();
            let mut command = Command::new("pwsh");
            command
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-File"])
                .arg(root.join("scripts/perf-ab.ps1"));
            command
                .env("TEMP", scratch.path())
                .env("TMP", scratch.path());
            (cargo, command)
        };
        let result = command
            .args(["HEAD~1", "HEAD"])
            .current_dir(&root)
            .env("CARGO", &cargo)
            .env("PANEFLOW_PERF_AB_DIR", output.path())
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert_eq!(
            result.status.code(),
            Some(3),
            "stdout:\n{stdout}\nstderr:\n{stderr}"
        );
        let base = Command::new("git")
            .args(["rev-parse", "--short=12", "HEAD~1"])
            .current_dir(&root)
            .output()
            .unwrap();
        let base = String::from_utf8_lossy(&base.stdout).trim().to_string();
        assert!(
            stderr.contains(&format!("base {base} unavailable: the build failed")),
            "{stderr}"
        );
        assert!(stderr.contains("error[E0425]"), "{stderr}");
        let worktrees = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&worktrees.stdout)
                .matches("worktree ")
                .count(),
            1,
            "the detached worktrees are removed"
        );
        assert_eq!(
            std::fs::read_dir(scratch.path()).unwrap().count(),
            0,
            "the scratch build directory is removed"
        );
    }

    const DAY_S: u64 = 86_400;

    fn gate_runs(effects: &[&str], spacing_s: u64) -> Vec<RunRecord> {
        effects
            .iter()
            .enumerate()
            .map(|(index, effect)| {
                RunRecord::Gate(GateRun {
                    run: format!("{}", 1000 + index),
                    stamp_s: 1_790_000_000 + index as u64 * spacing_s,
                    effect: effect.to_string(),
                    verdict: if *effect == "resets" {
                        "unconfirmed_regression"
                    } else {
                        "pass"
                    }
                    .to_string(),
                })
            })
            .collect()
    }

    fn write_result(root: &Path, run: &str, schema: u64, effect: &str, stamp: &str) {
        write_document(
            &root.join(run).join("result.json"),
            &json!({"schema_version": schema, "stamp": stamp, "verdict": "pass", "promotion": {"effect": effect}}),
        );
    }

    #[test]
    fn runs_with_an_unavailable_base_or_an_excluded_effect_never_enter_the_count() {
        let root = tempfile::tempdir().unwrap();
        let runs = root.path();
        std::fs::create_dir_all(runs.join("base-unavailable")).unwrap();
        std::fs::write(runs.join("base-unavailable/build-base.log"), "error").unwrap();
        write_result(
            runs,
            "excluded",
            AB_SCHEMA_VERSION,
            "excluded",
            "unix1790000000",
        );
        write_result(runs, "former-rule", 1, "calibrated", "unix1790000000");
        std::fs::create_dir_all(runs.join("no-artifact")).unwrap();
        std::fs::write(runs.join("no-artifact").join(NO_ARTIFACT_MARKER), "").unwrap();
        write_result(
            runs,
            "counted-1",
            AB_SCHEMA_VERSION,
            "calibrated",
            "unix1790000000",
        );
        write_result(
            runs,
            "counted-2",
            AB_SCHEMA_VERSION,
            "uncalibrated",
            "unix1790086400",
        );
        let records: Vec<RunRecord> = [
            "base-unavailable",
            "excluded",
            "former-rule",
            "no-artifact",
            "counted-1",
            "counted-2",
        ]
        .iter()
        .map(|run| classify_run(run, &runs.join(run)))
        .collect();
        let tally = promotion_tally(&records);
        assert_eq!(tally["all_runs"]["runs"], 2);
        assert_eq!(tally["all_runs"]["calibrated"], 1);
        assert_eq!(tally["skipped"].as_array().unwrap().len(), 4);
        assert!(
            tally["skipped"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("base or the head was unavailable")
        );
        assert_eq!(tally["decision"], "pending");
    }

    #[test]
    fn an_expired_artifact_leaves_the_count_incomplete() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("old")).unwrap();
        std::fs::write(root.path().join("old").join(EXPIRED_MARKER), "").unwrap();
        let mut records = gate_runs(&["calibrated"; 30], DAY_S);
        records.push(classify_run("old", &root.path().join("old")));
        assert_eq!(promotion_tally(&records)["decision"], "incomplete");
    }

    #[test]
    fn an_expired_artifact_older_than_the_latest_thirty_runs_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        let mut records = gate_runs(&["calibrated"; 30], DAY_S);
        for (run, completed_s) in [("before", 1_789_999_999_u64), ("inside", 1_790_000_000)] {
            std::fs::create_dir_all(root.path().join(run)).unwrap();
            std::fs::write(
                root.path().join(run).join(EXPIRED_MARKER),
                format!("{completed_s}\n"),
            )
            .unwrap();
        }
        records.push(classify_run("before", &root.path().join("before")));
        let tally = promotion_tally(&records);
        assert_eq!(tally["decision"], "promote", "{tally}");
        assert_eq!(tally["skipped"][0]["run"], "before");
        records.push(classify_run("inside", &root.path().join("inside")));
        let tally = promotion_tally(&records);
        assert_eq!(tally["decision"], "incomplete", "{tally}");
        assert_eq!(tally["unknown"][0]["run"], "inside");
    }

    #[test]
    fn thirty_calibrated_runs_over_three_weeks_promote() {
        let tally = promotion_tally(&gate_runs(&["calibrated"; 30], DAY_S));
        assert_eq!(tally["decision"], "promote", "{tally}");
        assert_eq!(tally["consecutive_since_last_reset"], 30);
    }

    #[test]
    fn thirty_clean_runs_inside_three_weeks_keep_waiting() {
        let tally = promotion_tally(&gate_runs(&["calibrated"; 30], DAY_S / 2));
        assert_eq!(tally["decision"], "pending", "{tally}");
    }

    #[test]
    fn ninety_percent_calibrated_is_the_floor() {
        let mut effects = vec!["calibrated"; 27];
        effects.extend(["uncalibrated"; 3]);
        assert_eq!(
            promotion_tally(&gate_runs(&effects, DAY_S))["decision"],
            "promote"
        );
        effects[0] = "uncalibrated";
        let tally = promotion_tally(&gate_runs(&effects, DAY_S));
        assert_eq!(tally["decision"], "keep_consultative", "{tally}");
        assert_eq!(tally["all_runs"]["calibrated_share"], json!(26.0 / 30.0));
    }

    #[test]
    fn an_unconfirmed_regression_restarts_the_count_and_keeps_it_consultative_at_thirty() {
        let mut effects = vec!["calibrated"; 30];
        effects[20] = "resets";
        let tally = promotion_tally(&gate_runs(&effects, DAY_S));
        assert_eq!(tally["decision"], "keep_consultative", "{tally}");
        assert_eq!(tally["consecutive_since_last_reset"], 9);
        assert_eq!(tally["all_runs"]["unconfirmed_regressions"], 1);
        let mut longer = effects.clone();
        longer.extend(["calibrated"; 21]);
        assert_eq!(
            promotion_tally(&gate_runs(&longer, DAY_S))["decision"],
            "promote",
            "thirty consecutive clean runs after the reset"
        );
    }

    #[test]
    fn the_ab_workflow_runs_in_shadow_on_hot_paths_nightly_and_on_demand() {
        let workflow = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/perf-ab.yml"),
        )
        .unwrap()
        .replace("\r\n", "\n");
        for path in [
            "src-app/src/terminal/**",
            "src-app/src/app/**",
            "crates/paneflow-host/**",
            "crates/paneflow-serve/**",
            "Cargo.lock",
        ] {
            assert!(
                workflow.contains(&format!("      - \"{path}\"\n")),
                "{path}"
            );
        }
        for trigger in [
            "  pull_request:\n",
            "  schedule:\n",
            "  workflow_dispatch:\n",
        ] {
            assert!(workflow.contains(trigger), "{trigger}");
        }
        assert!(!workflow.contains("pull_request_target"));
        assert!(workflow.contains("permissions:\n  contents: read\n"));
        assert!(workflow.contains("prefix-key: \"perf-ab-\""));
        assert!(workflow.contains("timeout-minutes: 60"));
        assert_eq!(
            workflow.matches("PERF_AB_BLOCKING: \"false\"").count(),
            1,
            "the real-time verdict stays in shadow behind one workflow variable"
        );
        assert_eq!(
            workflow
                .matches("PERF_AB_INSTRUCTIONS_BLOCKING: \"true\"")
                .count(),
            1,
            "the instruction verdict blocks behind its own workflow variable"
        );
        assert!(workflow.contains("instructions=$(cat target/perf-ab/instructions-verdict"));
        assert!(
            workflow.find("Swatinem/rust-cache") < workflow.find("cargo install gungraun-runner"),
            "the cache restores the installed runner before the install step"
        );
        assert!(workflow.contains("scripts/perf-ab.sh"));
        assert!(
            workflow.contains("retention-days: 90"),
            "every result.json of the three-week promotion window outlives its count"
        );
        assert!(workflow.contains("extra-packages: valgrind"));
        assert!(workflow.contains("cargo install gungraun-runner --version \"$version\" --locked"));
        let release = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows/release.yml"),
        )
        .unwrap();
        assert!(!release.contains("perf-ab-"));
    }
}
