#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod monitor_support;
mod support;

use std::path::Path;
use std::time::{Duration, Instant};

use monitor_support::{canonical, differences, feed_options, MARKER_FENCE};
use paneflow_git::{snapshot, Freshness, Monitor, Options, ProbeKind};

const GATE_OUT_ENV: &str = "PANEFLOW_GATE_GIT_MONITOR_OUT";

const DIRECTORIES: usize = 400;

const FILES_PER_DIRECTORY: usize = 50;

const EDITS: usize = 40;

const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

const DELIVERY_PAUSE: Duration = Duration::from_millis(2);

const STRAGGLER_PAUSE: Duration = Duration::from_millis(50);

fn generated_repo(root: &Path) {
    std::fs::create_dir_all(root).expect("root");
    for directory in 0..DIRECTORIES {
        let dir = root.join(format!("d{directory:03}"));
        std::fs::create_dir_all(&dir).expect("directory");
        for file in 0..FILES_PER_DIRECTORY {
            std::fs::write(
                dir.join(format!("f{file:02}.txt")),
                format!("{directory} {file}\n"),
            )
            .expect("file");
        }
    }
    support::init(root);
    support::git(root, &["add", "-A"]);
    support::git(root, &["commit", "-q", "-m", "generated"]);
    support::git(root, &["status", "--porcelain"]);
}

fn settle_stragglers(monitor: &mut Monitor, root: &Path, options: &Options) {
    if MARKER_FENCE {
        monitor
            .fenced_snapshot(options, Instant::now() + DELIVERY_DEADLINE)
            .expect("fenced settle");
    } else {
        std::thread::sleep(STRAGGLER_PAUSE);
        monitor
            .probe(root, options, Freshness::Converging)
            .expect("settle");
    }
}

fn percentile(values: &mut [f64], percent: usize) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = (values.len() * percent / 100).min(values.len().saturating_sub(1));
    values.get(index).copied().unwrap_or_default()
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

#[test]
fn a_one_edit_probe_of_a_twenty_thousand_entry_worktree_rechecks_one_entry() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    generated_repo(&root);
    let root = canonical(&root);
    let cache = tmp.path().join("cache");
    let options = Options::files_status();
    let mut monitor = Monitor::configured(&root, feed_options(&cache), true);
    monitor
        .probe(&root, &options, Freshness::Converging)
        .expect("baseline");
    assert!(matches!(monitor.last_probe().kind, ProbeKind::Baseline(_)));

    let mut walls = Vec::with_capacity(EDITS);
    let mut fulls = Vec::with_capacity(EDITS);
    let mut rechecked_max = 0;
    for edit in 0..EDITS {
        settle_stragglers(&mut monitor, &root, &options);
        let relative = format!(
            "d{:03}/f{:02}.txt",
            edit * 7 % DIRECTORIES,
            edit % FILES_PER_DIRECTORY
        );
        std::fs::write(root.join(&relative), format!("edit {edit}\n")).expect("edit");
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        let probed = loop {
            let started = Instant::now();
            let probed = monitor
                .probe(&root, &options, Freshness::Converging)
                .expect("probe")
                .into_snapshot();
            let wall = started.elapsed();
            let report = monitor.last_probe();
            assert_eq!(
                report.kind,
                ProbeKind::Incremental,
                "edit {edit}: {report:?}"
            );
            if report.rechecked_entries > 0 {
                rechecked_max = rechecked_max.max(report.rechecked_entries);
                walls.push(millis(wall));
                break probed;
            }
            assert!(
                Instant::now() < deadline,
                "edit {edit} of {relative} reached the monitor"
            );
            std::thread::sleep(DELIVERY_PAUSE);
        };
        let started = Instant::now();
        let full = snapshot(&root, &options).expect("full snapshot");
        fulls.push(millis(started.elapsed()));
        let differing = differences(&probed, &full);
        assert!(differing.is_empty(), "edit {edit}: {differing:?}");
    }
    if MARKER_FENCE {
        assert_eq!(rechecked_max, 1, "each one-edit probe re-checks one entry");
    } else {
        assert!(
            (1..=2).contains(&rechecked_max),
            "a converging one-edit probe re-checks its entry and at most one late straggler: {rechecked_max}"
        );
    }

    let entries = DIRECTORIES * FILES_PER_DIRECTORY;
    let one_edit_p50 = percentile(&mut walls, 50);
    let one_edit_p95 = percentile(&mut walls, 95);
    let full_p50 = percentile(&mut fulls, 50);
    eprintln!(
        "git monitor: {entries} entries, one-edit probe p50 {one_edit_p50:.3} ms p95 {one_edit_p95:.3} ms, full snapshot p50 {full_p50:.3} ms"
    );
    if let Some(out) = std::env::var_os(GATE_OUT_ENV) {
        let document = format!(
            "{{\n  \"entries\": {entries},\n  \"edits\": {EDITS},\n  \"one_edit_p50_ms\": {one_edit_p50},\n  \"one_edit_p95_ms\": {one_edit_p95},\n  \"full_p50_ms\": {full_p50},\n  \"rechecked_max\": {rechecked_max}\n}}\n"
        );
        std::fs::write(out, document).expect("gate output");
    }
}
