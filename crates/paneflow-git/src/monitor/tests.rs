#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[cfg(not(feature = "oracle"))]
#[path = "../../tests/support/mod.rs"]
mod support;
#[cfg(feature = "oracle")]
use crate::support;

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use super::*;
use crate::feed::FeedOptions;
use crate::snapshot;

const WAKE_DEADLINE: Duration = Duration::from_secs(10);

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn fixture() -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    support::committed_repo(&root, &[("top.txt", "top\n"), ("src/lib.rs", "lib\n")]);
    let root = std::fs::canonicalize(&root).expect("canonical root");
    support::git(&root, &["status", "--porcelain"]);
    let cache = tmp.path().join("cache");
    Fixture {
        _tmp: tmp,
        root,
        cache,
    }
}

fn monitor(fixture: &Fixture, timing: Timing) -> Monitor {
    let options = FeedOptions {
        cache_dir: Some(fixture.cache.clone()),
        ..FeedOptions::default()
    };
    Monitor::configured(&fixture.root, options, true).with_timing(timing)
}

fn probe(monitor: &mut Monitor, visibility: Visibility) -> Snapshot {
    let root = monitor.path().to_path_buf();
    monitor
        .probe_as(
            &root,
            &Options::default(),
            Freshness::Converging,
            visibility,
        )
        .expect("probe")
        .into_snapshot()
}

fn full(fixture: &Fixture) -> Snapshot {
    snapshot(&fixture.root, &Options::default()).expect("full snapshot")
}

fn kind(monitor: &Monitor) -> ProbeKind {
    monitor.last_probe().kind.clone()
}

fn modified(snapshot: &Snapshot, path: &str) -> bool {
    snapshot.entries.iter().any(|entry| {
        entry.path == path.as_bytes()
            && entry.index_to_worktree == Some(crate::WorktreeChange::Modified)
    })
}

fn swallow_events_for(monitor: &mut Monitor, path: &str) {
    let deadline = Instant::now() + WAKE_DEADLINE;
    let wanted = std::path::Path::new(path);
    let mut seen = false;
    let mut quiet_drains = 0;
    loop {
        let FeedState::Live(feed) = &mut monitor.feed else {
            panic!("the monitor has a live feed");
        };
        let batch = feed.drain();
        seen |= batch.changes.iter().any(|change| change.relative == wanted);
        quiet_drains = if batch.changes.is_empty() {
            quiet_drains + 1
        } else {
            0
        };
        if seen && quiet_drains >= SWALLOW_QUIET_DRAINS {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the feed reported {path} and went quiet within {WAKE_DEADLINE:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

const SWALLOW_QUIET_DRAINS: usize = 50;

static LOGGED: Mutex<Vec<(log::Level, String)>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LOGGED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((record.level(), record.args().to_string()));
    }

    fn flush(&self) {}
}

fn capture_logs() {
    static CAPTURE: Capture = Capture;
    if log::set_logger(&CAPTURE).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

fn warnings_about(needle: &str) -> Vec<String> {
    LOGGED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter(|(level, line)| *level == log::Level::Warn && line.contains(needle))
        .map(|(_, line)| line.clone())
        .collect()
}

#[test]
fn a_shown_worktree_verifies_after_its_probe_budget_and_a_hidden_one_never_does() {
    let fixture = fixture();
    let mut monitor = monitor(
        &fixture,
        Timing {
            verify_after_probes: 2,
            ..Timing::default()
        },
    );
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::FirstProbe));
    for _ in 0..4 {
        probe(&mut monitor, Visibility::Hidden);
        assert_eq!(
            kind(&monitor),
            ProbeKind::Incremental,
            "hidden: no verification"
        );
    }
    let verified = probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::Verification));
    assert_eq!(verified, full(&fixture));
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(
        kind(&monitor),
        ProbeKind::Incremental,
        "a verification restarts the budget"
    );
}

#[test]
fn a_shown_worktree_verifies_five_minutes_after_its_last_full_snapshot() {
    let fixture = fixture();
    let mut monitor = monitor(&fixture, Timing::default());
    probe(&mut monitor, Visibility::Shown);
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
    let Some(at) = Instant::now().checked_sub(VERIFY_EVERY) else {
        return;
    };
    monitor.since_full = Some(SinceFull { at, probes: 0 });
    probe(&mut monitor, Visibility::Hidden);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::Verification));
}

#[test]
fn an_unreported_write_is_repaired_by_the_next_verification_and_logged_once_as_a_drift() {
    capture_logs();
    let fixture = fixture();
    let mut monitor = monitor(
        &fixture,
        Timing {
            verify_after_probes: 1,
            ..Timing::default()
        },
    );
    probe(&mut monitor, Visibility::Shown);
    support::write(&fixture.root, "top.txt", b"secret content\n");
    swallow_events_for(&mut monitor, "top.txt");
    let stale = probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
    assert!(
        !modified(&stale, "top.txt"),
        "the swallowed event left the model stale"
    );
    let repaired = probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::Drift(1)));
    assert!(modified(&repaired, "top.txt"));
    assert_eq!(repaired, full(&fixture));
    let warnings = warnings_about(&fixture.root.display().to_string());
    let drifts: Vec<&String> = warnings
        .iter()
        .filter(|line| line.contains("drifted"))
        .collect();
    assert_eq!(drifts.len(), 1, "{warnings:?}");
    assert!(
        drifts[0].contains("1 paths differ (full snapshot: 1 modified)"),
        "{drifts:?}"
    );
    assert!(!drifts[0].contains("secret"), "{drifts:?}");
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
}

#[test]
fn a_backend_loss_answers_full_and_restarts_the_feed_after_a_doubling_backoff() {
    for loss in [Loss::BackendError, Loss::BackendStopped] {
        let fixture = fixture();
        let mut monitor = monitor(&fixture, Timing::default());
        probe(&mut monitor, Visibility::Shown);
        support::write(&fixture.root, "top.txt", b"edited\n");
        monitor.pending.loss = Some(loss);
        let answered = probe(&mut monitor, Visibility::Shown);
        assert_eq!(kind(&monitor), ProbeKind::Stopped(loss));
        assert!(monitor.last_probe().kind.fell_back());
        assert_eq!(answered, full(&fixture));
        assert!(monitor.feed().is_none(), "the stopped feed is dropped");
        let (_, retry_at) = monitor.recovering().expect("backing off");
        assert!(retry_at > Instant::now());
        assert_eq!(monitor.recovery, Some(RECOVERY_FIRST));
        let waiting = probe(&mut monitor, Visibility::Shown);
        assert_eq!(kind(&monitor), ProbeKind::Recovering(loss));
        assert_eq!(waiting, full(&fixture));
        monitor.feed = FeedState::Backoff {
            retry_at: Instant::now(),
            loss,
        };
        probe(&mut monitor, Visibility::Shown);
        assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::Recovered));
        assert!(monitor.feed().is_some(), "the feed restarted");
        monitor.pending.loss = Some(loss);
        probe(&mut monitor, Visibility::Shown);
        assert_eq!(kind(&monitor), ProbeKind::Stopped(loss));
        assert_eq!(
            monitor.recovery,
            Some(RECOVERY_FIRST * 2),
            "the delay doubles"
        );
        monitor.feed = FeedState::Backoff {
            retry_at: Instant::now(),
            loss,
        };
        probe(&mut monitor, Visibility::Shown);
        probe(&mut monitor, Visibility::Shown);
        assert_eq!(kind(&monitor), ProbeKind::Incremental);
        assert_eq!(
            monitor.recovery, None,
            "a healthy incremental probe resets it"
        );
    }
}

#[test]
fn a_feed_that_cannot_restart_keeps_backing_off_up_to_five_minutes() {
    let fixture = fixture();
    let mut monitor = monitor(&fixture, Timing::default());
    probe(&mut monitor, Visibility::Shown);
    monitor.pending.loss = Some(Loss::BackendStopped);
    probe(&mut monitor, Visibility::Shown);
    monitor.feed_options.max_events = 0;
    let mut delays = Vec::new();
    for _ in 0..11 {
        monitor.feed = FeedState::Backoff {
            retry_at: Instant::now(),
            loss: Loss::BackendStopped,
        };
        let answered = probe(&mut monitor, Visibility::Shown);
        assert_eq!(answered, full(&fixture));
        assert_eq!(kind(&monitor), ProbeKind::Recovering(Loss::BackendStopped));
        delays.push(monitor.recovery.expect("backing off").as_secs());
    }
    assert_eq!(delays, [2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
    assert!(
        monitor.ineligible().is_none(),
        "a restart failure is retried"
    );
}

#[test]
fn more_than_a_minute_between_two_drains_rebaselines() {
    let fixture = fixture();
    let mut monitor = monitor(&fixture, Timing::default());
    probe(&mut monitor, Visibility::Shown);
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
    monitor.last_drain = SystemTime::now().checked_sub(DRAIN_GAP + Duration::from_secs(1));
    let answered = probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Baseline(Reason::DrainGap));
    assert_eq!(answered, full(&fixture));
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(kind(&monitor), ProbeKind::Incremental);
}

#[test]
fn repeated_losses_rebaseline_at_most_once_per_spacing_and_wake_the_consumer_after_it() {
    let fixture = fixture();
    let mut monitor = monitor(
        &fixture,
        Timing {
            rebaseline_spacing: Duration::from_secs(3600),
            ..Timing::default()
        },
    );
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    monitor.set_waker(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    probe(&mut monitor, Visibility::Shown);
    for _ in 0..3 {
        monitor.pending.loss = Some(Loss::Overflow);
        probe(&mut monitor, Visibility::Shown);
        assert_eq!(
            kind(&monitor),
            ProbeKind::Deferred(Reason::Loss(Loss::Overflow))
        );
        assert_eq!(
            monitor.pending.loss,
            Some(Loss::Overflow),
            "the loss stays due"
        );
    }
    let root = fixture.root.clone();
    let fenced = monitor
        .probe(&root, &Options::default(), Freshness::Fenced)
        .expect("fenced probe")
        .into_snapshot();
    assert!(
        matches!(kind(&monitor), ProbeKind::Baseline(_)),
        "a fenced probe never takes a deferred answer: {:?}",
        kind(&monitor)
    );
    assert_eq!(fenced, full(&fixture));

    monitor.timing.rebaseline_spacing = Duration::from_millis(50);
    monitor.wake_scheduled = Arc::new(AtomicBool::new(false));
    monitor.last_baseline = Some(Instant::now());
    monitor.pending.loss = Some(Loss::Rescan);
    probe(&mut monitor, Visibility::Shown);
    assert_eq!(
        kind(&monitor),
        ProbeKind::Deferred(Reason::Loss(Loss::Rescan))
    );
    let scheduled = Arc::clone(&monitor.wake_scheduled);
    assert!(scheduled.load(Ordering::SeqCst), "a wake is scheduled");
    let deadline = Instant::now() + WAKE_DEADLINE;
    while scheduled.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the deferred re-baseline wakes the consumers"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let answered = probe(&mut monitor, Visibility::Shown);
    assert_eq!(
        kind(&monitor),
        ProbeKind::Baseline(Reason::Loss(Loss::Rescan))
    );
    assert_eq!(answered, full(&fixture));
}

#[cfg(windows)]
mod journal {
    use super::*;
    use crate::feed::{FenceError, JournalError};

    fn fenced(monitor: &mut Monitor) -> Snapshot {
        let root = monitor.path().to_path_buf();
        monitor
            .probe(&root, &Options::default(), Freshness::Fenced)
            .expect("fenced probe")
            .into_snapshot()
    }

    fn live_feed(monitor: &mut Monitor) -> &mut ChangeFeed {
        match &mut monitor.feed {
            FeedState::Live(feed) => feed,
            FeedState::Idle | FeedState::Refused(_) | FeedState::Backoff { .. } => {
                panic!("the monitor has a live feed")
            }
        }
    }

    fn assert_rebaselines_then_recovers(
        fault: impl FnOnce(&mut ChangeFeed),
        expected: impl Fn(&JournalError) -> bool,
    ) {
        let fixture = fixture();
        let mut monitor = monitor(&fixture, Timing::default());
        fenced(&mut monitor);
        support::write(&fixture.root, "top.txt", b"edited\n");
        fault(live_feed(&mut monitor));
        let probed = fenced(&mut monitor);
        match kind(&monitor) {
            ProbeKind::Baseline(Reason::Fence(FenceError::Journal(error))) => {
                assert!(expected(&error), "{error:?}");
            }
            other => panic!("a faulted journal re-baselines the fenced probe: {other:?}"),
        }
        assert_eq!(probed.entries, full(&fixture).entries);
        assert!(modified(&probed, "top.txt"));
        support::write(&fixture.root, "src/lib.rs", b"edited too\n");
        let next = fenced(&mut monitor);
        assert_eq!(
            kind(&monitor),
            ProbeKind::Incremental,
            "the journal re-armed"
        );
        assert!(modified(&next, "src/lib.rs"));
    }

    #[test]
    fn a_file_deleted_and_recreated_rechecks_it_and_not_its_whole_directory() {
        let fixture = fixture();
        for index in 0..40 {
            support::write(
                &fixture.root,
                &format!("src/file{index}.txt"),
                format!("file {index}\n").as_bytes(),
            );
        }
        support::git(&fixture.root, &["add", "-A"]);
        support::git(
            &fixture.root,
            &["commit", "-q", "--no-verify", "-m", "files"],
        );
        support::git(&fixture.root, &["status", "--porcelain"]);
        let mut monitor = monitor(&fixture, Timing::default());
        fenced(&mut monitor);
        let path = fixture.root.join("src").join("file7.txt");
        std::fs::remove_file(&path).expect("delete");
        std::fs::write(&path, b"recreated\n").expect("recreate");
        let probed = fenced(&mut monitor);
        assert_eq!(kind(&monitor), ProbeKind::Incremental);
        assert!(
            monitor.last_probe().rechecked_entries <= 2,
            "{:?}",
            monitor.last_probe()
        );
        assert!(modified(&probed, "src/file7.txt"));
        assert_eq!(probed.entries, full(&fixture).entries);
    }

    #[test]
    fn a_fenced_probe_on_windows_answers_incrementally_from_the_journal() {
        let fixture = fixture();
        let mut monitor = monitor(&fixture, Timing::default());
        fenced(&mut monitor);
        support::write(&fixture.root, "src/lib.rs", b"edited\n");
        let probed = fenced(&mut monitor);
        assert_eq!(kind(&monitor), ProbeKind::Incremental);
        assert_eq!(probed.entries, full(&fixture).entries);
    }

    #[test]
    fn a_journal_wrap_rebaselines_the_fenced_probe_and_rearms() {
        assert_rebaselines_then_recovers(ChangeFeed::rewind_journal_for_test, |error| {
            *error == JournalError::Wrapped
        });
    }

    #[test]
    fn a_replaced_journal_rebaselines_the_fenced_probe_and_rearms() {
        assert_rebaselines_then_recovers(ChangeFeed::replace_journal_id_for_test, |error| {
            *error == JournalError::Replaced
        });
    }

    #[test]
    fn a_journal_read_error_rebaselines_the_fenced_probe_and_rearms() {
        assert_rebaselines_then_recovers(ChangeFeed::break_journal_reads_for_test, |error| {
            matches!(error, JournalError::Read(_))
        });
    }

    #[test]
    fn a_volume_without_a_journal_answers_every_fenced_probe_with_the_full_snapshot() {
        let fixture = fixture();
        let mut monitor = monitor(&fixture, Timing::default());
        fenced(&mut monitor);
        live_feed(&mut monitor).remove_journal_for_test();
        for edit in 0..2 {
            support::write(
                &fixture.root,
                "top.txt",
                format!("edit {edit}\n").as_bytes(),
            );
            let probed = fenced(&mut monitor);
            assert_eq!(
                kind(&monitor),
                ProbeKind::Baseline(Reason::Fence(FenceError::Journal(JournalError::Missing)))
            );
            assert_eq!(
                kind(&monitor).counter(),
                crate::engine::MonitorCounter::JournalMissing
            );
            assert_eq!(probed.entries, full(&fixture).entries);
        }
        let converging = probe(&mut monitor, Visibility::Shown);
        assert_eq!(
            kind(&monitor),
            ProbeKind::Incremental,
            "converging probes keep the feed"
        );
        assert_eq!(converging.entries, full(&fixture).entries);
    }
}
