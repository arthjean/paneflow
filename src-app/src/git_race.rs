use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::git_fixture as fixture;

pub(crate) const RACED_FILE: &str = "raced.txt";

pub(crate) const STAT_DIRTY_PROBES: usize = 40;

pub(crate) const RACY_ROUNDS: usize = 5;

const BODY_LINES: usize = 480;

const BODY_LINE_WIDTH: usize = 1023;

const SEED_GENERATION: usize = 99_999_999;

const STAT_DIRTY_AGE: Duration = Duration::from_secs(3600);

const SECOND_START_MARGIN: Duration = Duration::from_millis(5);

const CLOSED_WINDOW_POLL: Duration = Duration::from_millis(1);

const WRITES_AHEAD_OF_EACH_PROBE: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scenario {
    StatDirty,
    RacyWindow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Raced {
    Tracked,
    Untracked,
}

#[derive(Debug)]
pub(crate) struct Outcome<T> {
    pub scenario: Scenario,
    pub writes: usize,
    pub probes: usize,
    pub write_failures: BTreeMap<Option<i32>, usize>,
    pub probe_errors: Vec<String>,
    pub answers: Vec<T>,
}

impl<T> Outcome<T> {
    pub(crate) fn failed_writes(&self) -> usize {
        self.write_failures.values().sum()
    }

    pub(crate) fn is_conclusive(&self) -> bool {
        self.writes > self.probes
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "{:?}: {} failed writes out of {} over {} probes, failures by raw OS error {:?} (1224 is a user-mapped section), {} probe errors {:?}",
            self.scenario,
            self.failed_writes(),
            self.writes,
            self.probes,
            self.write_failures,
            self.probe_errors.len(),
            self.probe_errors
        )
    }

    pub(crate) fn assert_conclusive(&self) {
        assert!(
            self.is_conclusive(),
            "inconclusive race: the writer made {} writes for {} probes, so it never overlapped a probe",
            self.writes,
            self.probes
        );
    }

    pub(crate) fn assert_no_failed_write(&self) {
        self.assert_conclusive();
        assert!(
            self.write_failures.is_empty() && self.probe_errors.is_empty(),
            "{}",
            self.summary()
        );
    }
}

pub(crate) fn same_size_body(generation: usize) -> String {
    format!(
        "{generation:08}\n{}",
        format!("{}\n", "x".repeat(BODY_LINE_WIDTH)).repeat(BODY_LINES)
    )
}

pub(crate) fn race<T, P>(scenario: Scenario, raced: Raced, probe: P) -> Outcome<T>
where
    T: Send + 'static,
    P: Fn(&Path) -> Result<T, String> + Send + 'static,
{
    let tmp = tempfile::tempdir().expect("race tempdir");
    let root = tmp.path().join("repo");
    let file = prepare(&root, scenario, raced);
    match scenario {
        Scenario::StatDirty => race_stat_dirty(scenario, &root, &file, probe),
        Scenario::RacyWindow => race_racy_window(scenario, &root, &file, probe),
    }
}

fn prepare(root: &Path, scenario: Scenario, raced: Raced) -> PathBuf {
    let file = root.join(RACED_FILE);
    match raced {
        Raced::Tracked => {
            fixture::committed_repo(root, &[(RACED_FILE, &same_size_body(SEED_GENERATION))]);
            if scenario == Scenario::StatDirty {
                set_modified(&file, SystemTime::now() - STAT_DIRTY_AGE);
                fixture::run(root, &["update-index", "--refresh"]);
            }
        }
        Raced::Untracked => {
            assert_eq!(
                scenario,
                Scenario::StatDirty,
                "an untracked file has no index entry to make racily clean"
            );
            fixture::committed_repo(root, &[("committed.txt", "committed\n")]);
            std::fs::write(&file, same_size_body(SEED_GENERATION)).expect("untracked file");
        }
    }
    file
}

fn set_modified(path: &Path, when: SystemTime) {
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("raced file")
        .set_modified(when)
        .expect("raced file mtime");
}

struct Writer {
    writes: usize,
    failures: BTreeMap<Option<i32>, usize>,
}

impl Writer {
    fn new() -> Self {
        Self {
            writes: 0,
            failures: BTreeMap::new(),
        }
    }

    fn rewrite(&mut self, file: &Path) {
        if let Err(error) = std::fs::write(file, same_size_body(self.writes)) {
            *self.failures.entry(error.raw_os_error()).or_default() += 1;
        }
        self.writes += 1;
    }
}

struct Probes<T> {
    errors: Vec<String>,
    answers: Vec<T>,
}

impl<T> Probes<T> {
    fn record(&mut self, result: Result<T, String>) {
        match result {
            Ok(answer) => self.answers.push(answer),
            Err(error) => self.errors.push(error),
        }
    }

    fn count(&self) -> usize {
        self.errors.len() + self.answers.len()
    }
}

fn outcome<T>(scenario: Scenario, writer: Writer, probes: Probes<T>) -> Outcome<T> {
    Outcome {
        scenario,
        writes: writer.writes,
        probes: probes.count(),
        write_failures: writer.failures,
        probe_errors: probes.errors,
        answers: probes.answers,
    }
}

fn race_stat_dirty<T, P>(scenario: Scenario, root: &Path, file: &Path, probe: P) -> Outcome<T>
where
    T: Send + 'static,
    P: Fn(&Path) -> Result<T, String> + Send + 'static,
{
    let finished = Arc::new(AtomicUsize::new(0));
    let written = Arc::new(AtomicUsize::new(0));
    let thread = {
        let root = root.to_path_buf();
        let finished = Arc::clone(&finished);
        let written = Arc::clone(&written);
        std::thread::spawn(move || {
            let mut probes = Probes {
                errors: Vec::new(),
                answers: Vec::new(),
            };
            for index in 0..STAT_DIRTY_PROBES {
                while written.load(Ordering::Acquire) < index + WRITES_AHEAD_OF_EACH_PROBE {
                    std::thread::yield_now();
                }
                probes.record(probe(&root));
                finished.fetch_add(1, Ordering::Release);
            }
            probes
        })
    };
    let mut writer = Writer::new();
    while finished.load(Ordering::Acquire) < STAT_DIRTY_PROBES && !thread.is_finished() {
        writer.rewrite(file);
        written.store(writer.writes, Ordering::Release);
    }
    let probes = thread.join().expect("probe thread");
    outcome(scenario, writer, probes)
}

fn race_racy_window<T, P>(scenario: Scenario, root: &Path, file: &Path, probe: P) -> Outcome<T>
where
    T: Send + 'static,
    P: Fn(&Path) -> Result<T, String> + Send + 'static,
{
    let window = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let thread = {
        let root = root.to_path_buf();
        let window = Arc::clone(&window);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut probes = Probes {
                errors: Vec::new(),
                answers: Vec::new(),
            };
            while !done.load(Ordering::Acquire) {
                if window.load(Ordering::Acquire) {
                    probes.record(probe(&root));
                } else {
                    std::thread::sleep(CLOSED_WINDOW_POLL);
                }
            }
            probes
        })
    };
    let mut writer = Writer::new();
    for _ in 0..RACY_ROUNDS {
        std::thread::sleep(until_next_second() + SECOND_START_MARGIN);
        writer.rewrite(file);
        fixture::run(root, &["add", "--", RACED_FILE]);
        let indexed_second = whole_seconds(SystemTime::now());
        window.store(true, Ordering::Release);
        while whole_seconds(SystemTime::now()) == indexed_second {
            writer.rewrite(file);
        }
        window.store(false, Ordering::Release);
    }
    done.store(true, Ordering::Release);
    let probes = thread.join().expect("probe thread");
    outcome(scenario, writer, probes)
}

fn whole_seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn until_next_second() -> Duration {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Duration::from_secs(1).saturating_sub(Duration::from_nanos(u64::from(since.subsec_nanos())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome_with(writes: usize, probes: usize) -> Outcome<()> {
        Outcome {
            scenario: Scenario::StatDirty,
            writes,
            probes,
            write_failures: BTreeMap::new(),
            probe_errors: Vec::new(),
            answers: vec![(); probes],
        }
    }

    #[test]
    fn a_writer_that_never_overlapped_a_probe_fails_as_inconclusive() {
        let fewer_writes = outcome_with(3, 40);
        let failure = std::panic::catch_unwind(|| fewer_writes.assert_no_failed_write())
            .expect_err("fewer writes than probes is inconclusive");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(message.contains("inconclusive race"), "{message}");
        assert!(!outcome_with(40, 40).is_conclusive());
        assert!(outcome_with(41, 40).is_conclusive());
    }

    #[test]
    fn a_failed_write_or_a_probe_error_fails_a_conclusive_race() {
        let mut failed_write = outcome_with(100, 40);
        failed_write.write_failures.insert(Some(1224), 3);
        let failure = std::panic::catch_unwind(|| failed_write.assert_no_failed_write())
            .expect_err("a failed write fails the race");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(message.contains("3 failed writes out of 100"), "{message}");

        let mut probe_error = outcome_with(100, 40);
        probe_error.probe_errors.push("no statuses".to_string());
        assert!(std::panic::catch_unwind(|| probe_error.assert_no_failed_write()).is_err());
    }

    #[test]
    fn a_probe_faster_than_the_writer_still_overlaps_it_in_the_stat_dirty_scenario() {
        let outcome = race(Scenario::StatDirty, Raced::Tracked, |_root: &Path| Ok(()));
        outcome.assert_no_failed_write();
        assert_eq!(outcome.probes, STAT_DIRTY_PROBES);
    }

    #[test]
    fn a_probe_that_panics_fails_the_race_instead_of_stalling_the_writer() {
        let failure = std::panic::catch_unwind(|| {
            race(
                Scenario::StatDirty,
                Raced::Tracked,
                |_root: &Path| -> Result<(), String> { panic!("the probe broke") },
            )
        })
        .expect_err("a panicking probe fails the race");
        let message = failure
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| failure.downcast_ref::<&str>().map(|text| text.to_string()))
            .unwrap_or_default();
        assert!(message.contains("probe thread"), "{message}");
    }

    fn git_probe(root: &Path, args: &[&str]) -> Result<String, String> {
        let out = fixture::output(root, args);
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "git {args:?} exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    #[test]
    fn every_rewrite_keeps_the_size_of_the_raced_file() {
        let seeded = same_size_body(SEED_GENERATION).len();
        assert!(seeded > 480 * 1024);
        assert_eq!(same_size_body(0).len(), seeded);
        assert_eq!(same_size_body(12_345_678).len(), seeded);
    }

    #[test]
    fn the_stat_dirty_scenario_leaves_the_raced_file_modified_against_a_refreshed_index() {
        let outcome = race(Scenario::StatDirty, Raced::Tracked, |root: &Path| {
            git_probe(root, &["diff", "--name-only"])
        });
        outcome.assert_conclusive();
        assert_eq!(outcome.probes, STAT_DIRTY_PROBES);
        assert!(!outcome.answers.is_empty(), "{}", outcome.summary());
        assert!(
            outcome
                .answers
                .iter()
                .all(|names| names.trim() == RACED_FILE),
            "{:?}",
            outcome.answers
        );
    }

    #[test]
    fn the_racy_scenario_probes_only_inside_its_index_write_seconds() {
        let outcome = race(Scenario::RacyWindow, Raced::Tracked, |root: &Path| {
            git_probe(root, &["ls-files", "--stage", "--", RACED_FILE])
        });
        outcome.assert_conclusive();
        assert!(outcome.probes > 0, "{}", outcome.summary());
        assert!(
            outcome
                .answers
                .iter()
                .all(|stage| stage.contains(RACED_FILE)),
            "{:?}",
            outcome.answers
        );
    }
}
