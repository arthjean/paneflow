use super::*;

pub(super) fn stamp() -> String {
    std::env::var("PANEFLOW_BENCH_STAMP").unwrap_or_else(|_| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("unix{seconds}")
    })
}

pub(super) fn output_path() -> PathBuf {
    std::env::var_os("PANEFLOW_BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bench/results")
                .join(format!("persistent-{}-local.json", stamp()))
        })
}

fn baseline_document() -> Option<Value> {
    let path = std::env::var_os("PANEFLOW_BENCH_BASELINE")?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

fn baseline_matches(baseline: &Value, document: &Value) -> bool {
    baseline["schema_version"] == document["schema_version"]
        && baseline["topology"] == document["topology"]
        && baseline["machine"] == document["machine"]
        && baseline["toolchain"]["profile"] == document["toolchain"]["profile"]
}

pub(super) fn baseline_throughput(document: &Value) -> Option<f64> {
    let baseline = baseline_document()?;
    baseline_matches(&baseline, document)
        .then(|| baseline["workloads"]["W03"]["single_flood"]["mib_per_s"].as_f64())
        .flatten()
}

pub(super) fn compare(document: &Value, decisions: &[Decision]) -> String {
    let mut text = String::new();
    let baseline = baseline_document();
    let comparable = baseline
        .as_ref()
        .is_some_and(|baseline| baseline_matches(baseline, document));
    match (&baseline, comparable) {
        (None, _) => text.push_str("no comparable baseline configured; thresholds only\n"),
        (Some(_), false) => text.push_str(
            "baseline topology, schema, machine, or profile differs; no performance comparison is valid\n",
        ),
        (Some(_), true) => {}
    }
    let base = baseline.filter(|_| comparable);
    let metric = |doc: &Value, path: &[&str]| -> Option<f64> {
        let mut cursor = doc;
        for key in path {
            cursor = &cursor[*key];
        }
        cursor.as_f64()
    };
    text.push_str(&format!(
        "{:<44} {:>14} {:>14} {:>22} {:>8}\n",
        "metric", "candidate", "baseline", "threshold", "result"
    ));
    for scenario in document["scenarios"].as_array().into_iter().flatten() {
        let sessions = scenario["sessions"].as_u64().unwrap_or(0);
        let base_scenario = base.as_ref().and_then(|b| {
            b["scenarios"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|candidate| candidate["sessions"].as_u64() == Some(sessions))
                .cloned()
        });
        for (label, path) in [
            ("host cpu %", ["host", "cpu", "total_cpu_percent"]),
            ("host rss bytes", ["host", "resident_bytes", ""]),
        ] {
            let path: Vec<&str> = path.iter().copied().filter(|p| !p.is_empty()).collect();
            let now = metric(scenario, &path);
            let before = base_scenario.as_ref().and_then(|b| metric(b, &path));
            text.push_str(&format!(
                "{:<44} {:>14} {:>14} {:>22} {:>8}\n",
                format!("W01 {sessions} sessions {label}"),
                now.map_or("pending".to_string(), |v| format!("{v:.3}")),
                before.map_or("n/a".to_string(), |v| format!("{v:.3}")),
                "informational",
                "-"
            ));
        }
    }
    let workload_rows: [(&str, &[&str]); 6] = [
        (
            "W02 attach p95 ms",
            &["workloads", "W02", "sequential_attach_ms", "p95"],
        ),
        (
            "W02 concurrent total ms",
            &["workloads", "W02", "concurrent_total_ms"],
        ),
        (
            "W03 single flood MiB/s",
            &["workloads", "W03", "single_flood", "mib_per_s"],
        ),
        (
            "W03 idle echo p95 ms",
            &["workloads", "W03", "idle_echo_ms", "p95"],
        ),
        (
            "W03 loaded echo p95 ms",
            &["workloads", "W03", "loaded_echo_ms", "p95"],
        ),
        (
            "W05 final host rss bytes",
            &["workloads", "W05", "final", "host_resident_bytes"],
        ),
    ];
    for (label, path) in workload_rows {
        let now = metric(document, path);
        let before = base.as_ref().and_then(|b| metric(b, path));
        let decision = decisions.iter().find(|d| {
            label.contains(d.workload)
                && d.metric
                    .split(' ')
                    .next()
                    .is_some_and(|w| label.to_lowercase().contains(&w.to_lowercase()))
        });
        text.push_str(&format!(
            "{:<44} {:>14} {:>14} {:>22} {:>8}\n",
            label,
            now.map_or("pending".to_string(), |v| format!("{v:.3}")),
            before.map_or("n/a".to_string(), |v| format!("{v:.3}")),
            decision.map_or("informational".to_string(), |d| d.threshold.clone()),
            decision.map_or("-", |d| d.result)
        ));
    }
    text.push_str("\nthreshold decisions:\n");
    for decision in decisions {
        text.push_str(&format!(
            "  {:<28} {:<8} {} {}\n",
            decision.id,
            decision.result,
            decision.metric,
            if decision.reason.is_empty() {
                String::new()
            } else {
                format!("({})", decision.reason)
            }
        ));
    }
    text
}

pub(super) fn write_document(path: &Path, document: &Value) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, serde_json::to_vec_pretty(document).unwrap()).unwrap();
}
