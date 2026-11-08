#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_git::feed::FeedOptions;
use paneflow_git::{snapshot, Monitor, Options, ProbeReport, Snapshot};

pub const FENCED: bool = cfg!(any(target_os = "linux", target_os = "macos"));

pub const CONVERGENCE_DEADLINE: Duration = Duration::from_secs(10);

const FENCE_DEADLINE: Duration = Duration::from_secs(5);

const CONVERGENCE_PAUSE: Duration = Duration::from_millis(25);

pub fn feed_options(cache: &Path) -> FeedOptions {
    FeedOptions {
        cache_dir: Some(cache.to_path_buf()),
        ..FeedOptions::default()
    }
}

pub fn monitor(root: &Path, cache: &Path) -> Monitor {
    Monitor::new(root, feed_options(cache))
}

#[derive(Debug, Default)]
pub struct Settled {
    pub snapshot: Option<Snapshot>,
    pub reports: Vec<ProbeReport>,
}

impl Settled {
    pub fn last(&self) -> &ProbeReport {
        self.reports.last().expect("at least one probe")
    }

    pub fn rechecked(&self) -> usize {
        self.reports
            .iter()
            .map(|report| report.rechecked_entries)
            .sum()
    }

    pub fn hashed(&self) -> usize {
        self.reports.iter().map(|report| report.files_hashed).sum()
    }

    pub fn walked(&self) -> usize {
        self.reports.iter().map(|report| report.walked_paths).sum()
    }

    pub fn incremental(&self) -> bool {
        self.reports
            .iter()
            .all(|report| report.kind == paneflow_git::ProbeKind::Incremental)
    }
}

pub fn settle(monitor: &mut Monitor, options: &Options) -> Result<Settled, String> {
    let root = monitor.path().to_path_buf();
    let mut settled = Settled::default();
    if FENCED {
        let probed = monitor
            .fenced_snapshot(options, Instant::now() + FENCE_DEADLINE)
            .map_err(|error| format!("fenced probe: {error}"))?;
        settled.reports.push(monitor.last_probe().clone());
        let full = snapshot(&root, options).map_err(|error| format!("full snapshot: {error}"))?;
        let differences = differences(&probed, &full);
        settled.snapshot = Some(probed);
        if differences.is_empty() {
            return Ok(settled);
        }
        return Err(differences.join("\n"));
    }
    let deadline = Instant::now() + CONVERGENCE_DEADLINE;
    loop {
        let probed = monitor
            .snapshot(options)
            .map_err(|error| format!("converging probe: {error}"))?;
        settled.reports.push(monitor.last_probe().clone());
        let full = snapshot(&root, options).map_err(|error| format!("full snapshot: {error}"))?;
        let differences = differences(&probed, &full);
        settled.snapshot = Some(probed);
        if differences.is_empty() {
            return Ok(settled);
        }
        if Instant::now() >= deadline {
            return Err(differences.join("\n"));
        }
        std::thread::sleep(CONVERGENCE_PAUSE);
    }
}

pub fn settled(monitor: &mut Monitor, options: &Options) -> Settled {
    settle(monitor, options).unwrap_or_else(|differences| {
        panic!(
            "the monitor of {} differs from a full snapshot:\n{differences}",
            monitor.path().display()
        )
    })
}

pub fn differences(probed: &Snapshot, full: &Snapshot) -> Vec<String> {
    let mut differences = Vec::new();
    if probed.head != full.head {
        differences.push(format!("head {:?} != {:?}", probed.head, full.head));
    }
    if probed.prefix != full.prefix {
        differences.push("prefix".to_string());
    }
    if probed.truncated != full.truncated {
        differences.push(format!(
            "truncated {} != {}",
            probed.truncated, full.truncated
        ));
    }
    let paths: BTreeSet<&Vec<u8>> = probed
        .entries
        .iter()
        .chain(&full.entries)
        .map(|entry| &entry.path)
        .collect();
    for path in paths {
        let left = probed.entries.iter().find(|entry| &entry.path == path);
        let right = full.entries.iter().find(|entry| &entry.path == path);
        if left != right {
            differences.push(format!(
                "{}: monitor {left:?} != full {right:?}",
                String::from_utf8_lossy(path)
            ));
        }
        if probed.sides.get(path) != full.sides.get(path) {
            differences.push(format!(
                "{} sides: monitor {:?} != full {:?}",
                String::from_utf8_lossy(path),
                probed.sides.get(path),
                full.sides.get(path)
            ));
        }
    }
    differences
}

pub fn paths(snapshot: &Snapshot) -> Vec<String> {
    snapshot
        .entries
        .iter()
        .map(|entry| String::from_utf8_lossy(&entry.path).into_owned())
        .collect()
}

pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).expect("canonical path")
}
