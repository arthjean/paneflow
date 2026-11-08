#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod monitor_support;
mod support;

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use monitor_support::{canonical, differences, feed_options, CONVERGENCE_DEADLINE, FENCED};
use paneflow_git::feed::FENCE_DIRECTORY_NAME;
use paneflow_git::{
    snapshot, worktree_root, Freshness, Monitor, Options, ProbeKind, Reason, WorktreeChange,
};
use support::{committed_repo, git, write};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    other: PathBuf,
}

fn fixture() -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(
        &root,
        &[
            ("top.txt", "top\n"),
            ("src/lib.rs", "lib\n"),
            ("src/deep/inner.rs", "inner\n"),
        ],
    );
    let other = tmp.path().join("other");
    committed_repo(&other, &[("other.txt", "other\n")]);
    write(&other, "other.txt", b"changed elsewhere\n");
    let cache = tmp.path().join("cache");
    let root = canonical(&root);
    let other = canonical(&other);
    git(&root, &["status", "--porcelain"]);
    Fixture {
        _tmp: tmp,
        root,
        cache,
        other,
    }
}

fn baselined(fixture: &Fixture) -> Monitor {
    let mut monitor = Monitor::configured(&fixture.root, feed_options(&fixture.cache), true);
    monitor
        .probe(&fixture.root, &Options::default(), Freshness::Converging)
        .expect("baseline");
    assert_eq!(
        monitor.last_probe().kind,
        ProbeKind::Baseline(Reason::FirstProbe)
    );
    monitor
}

fn modified(snapshot: &paneflow_git::Snapshot, path: &str) -> bool {
    snapshot.entries.iter().any(|entry| {
        entry.path == path.as_bytes() && entry.index_to_worktree == Some(WorktreeChange::Modified)
    })
}

#[test]
fn a_fenced_probe_sees_a_write_that_completed_before_it_started() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(
        &fixture.root,
        "src/deep/inner.rs",
        b"edited before the probe\n",
    );
    let probed = monitor
        .probe(&fixture.root, &Options::default(), Freshness::Fenced)
        .expect("fenced probe")
        .into_snapshot();
    let full = snapshot(&fixture.root, &Options::default()).expect("full");
    assert!(differences(&probed, &full).is_empty());
    assert!(modified(&probed, "src/deep/inner.rs"), "{probed:?}");
    if FENCED {
        assert_eq!(monitor.last_probe().kind, ProbeKind::Incremental);
        assert_eq!(monitor.last_probe().rechecked_entries, 1);
    } else {
        assert_eq!(
            monitor.last_probe().kind,
            ProbeKind::Baseline(Reason::Fence(paneflow_git::feed::FenceError::Unsupported)),
            "a platform without a fence answers a fenced probe with the full snapshot"
        );
    }
}

#[test]
fn a_fence_past_its_deadline_takes_the_full_snapshot() {
    if !FENCED {
        return;
    }
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "top.txt", b"edited\n");
    let probed = monitor
        .fenced_snapshot(&Options::default(), Instant::now())
        .expect("fenced probe");
    assert!(
        matches!(
            monitor.last_probe().kind,
            ProbeKind::Baseline(Reason::Fence(_))
        ),
        "{:?}",
        monitor.last_probe().kind
    );
    let full = snapshot(&fixture.root, &Options::default()).expect("full");
    assert!(differences(&probed, &full).is_empty());
    assert!(modified(&probed, "top.txt"));
}

#[test]
fn a_fence_whose_marker_directory_vanished_takes_the_full_snapshot() {
    if !FENCED {
        return;
    }
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    std::fs::remove_dir_all(fixture.cache.join(FENCE_DIRECTORY_NAME)).expect("remove marker");
    write(&fixture.root, "top.txt", b"edited\n");
    let probed = monitor
        .probe(&fixture.root, &Options::default(), Freshness::Fenced)
        .expect("fenced probe")
        .into_snapshot();
    assert!(
        matches!(
            monitor.last_probe().kind,
            ProbeKind::Baseline(Reason::Fence(_) | Reason::Loss(_))
        ),
        "{:?}",
        monitor.last_probe().kind
    );
    let full = snapshot(&fixture.root, &Options::default()).expect("full");
    assert!(differences(&probed, &full).is_empty());
}

#[test]
fn a_converging_probe_answers_without_a_fence_and_a_later_write_wakes_the_consumer() {
    let fixture = fixture();
    let mut monitor = Monitor::configured(&fixture.root, feed_options(&fixture.cache), true);
    let (woken, wakes) = mpsc::channel();
    monitor.set_waker(move || {
        let _ = woken.send(());
    });
    monitor
        .probe(&fixture.root, &Options::default(), Freshness::Converging)
        .expect("baseline");
    while wakes.try_recv().is_ok() {}
    write(&fixture.root, "src/lib.rs", b"edited after the probe\n");
    wakes
        .recv_timeout(CONVERGENCE_DEADLINE)
        .expect("the feed wakes the consumer for a write after its probe");
    let deadline = Instant::now() + CONVERGENCE_DEADLINE;
    loop {
        let probed = monitor
            .probe(&fixture.root, &Options::default(), Freshness::Converging)
            .expect("converging probe")
            .into_snapshot();
        assert_eq!(monitor.last_probe().kind, ProbeKind::Incremental);
        if modified(&probed, "src/lib.rs") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the converging probe never showed the write"
        );
        wakes
            .recv_timeout(CONVERGENCE_DEADLINE)
            .expect("another wake");
    }
}

#[test]
fn a_probe_from_a_subdirectory_reads_objects_with_that_directory_as_its_prefix() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    let cwd = fixture.root.join("src");
    let probed = monitor
        .probe(&cwd, &Options::default(), Freshness::Converging)
        .expect("probe");
    assert_eq!(monitor.last_probe().kind, ProbeKind::Incremental);
    let full = snapshot(&cwd, &Options::default()).expect("full");
    assert_eq!(probed.snapshot().prefix, b"src/".to_vec());
    assert!(differences(probed.snapshot(), &full).is_empty());
    let workdir = probed
        .read(|_, objects| Ok(objects.workdir().map(canonical)))
        .expect("read");
    assert_eq!(workdir, Some(fixture.root.clone()));
    assert_eq!(
        worktree_root(&cwd).map(|root| canonical(&root)),
        Some(fixture.root.clone())
    );
}

#[test]
fn a_monitor_error_answers_with_the_full_snapshot_and_restarts_the_monitor() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    let probed = monitor
        .probe(&fixture.other, &Options::default(), Freshness::Converging)
        .expect("the full snapshot answers")
        .into_snapshot();
    assert!(
        matches!(monitor.last_probe().kind, ProbeKind::MonitorError(_)),
        "{:?}",
        monitor.last_probe().kind
    );
    assert!(monitor.last_probe().kind.fell_back());
    assert!(
        monitor.feed().is_none(),
        "the failed monitor dropped its feed"
    );
    let full = snapshot(&fixture.other, &Options::default()).expect("full");
    assert!(differences(&probed, &full).is_empty());
    assert!(modified(&probed, "other.txt"));
    monitor
        .probe(&fixture.root, &Options::default(), Freshness::Converging)
        .expect("restart");
    assert_eq!(
        monitor.last_probe().kind,
        ProbeKind::Baseline(Reason::FirstProbe)
    );
}

#[test]
fn a_disabled_monitor_takes_full_snapshots_and_starts_no_feed() {
    let fixture = fixture();
    let mut monitor = Monitor::configured(&fixture.root, feed_options(&fixture.cache), false);
    write(&fixture.root, "top.txt", b"edited\n");
    for freshness in [Freshness::Converging, Freshness::Fenced] {
        let probed = monitor
            .probe(&fixture.root, &Options::default(), freshness)
            .expect("probe")
            .into_snapshot();
        assert_eq!(monitor.last_probe().kind, ProbeKind::Disabled);
        assert!(monitor.feed().is_none());
        let full = snapshot(&fixture.root, &Options::default()).expect("full");
        assert!(differences(&probed, &full).is_empty());
    }
    assert!(
        !fixture.cache.join(FENCE_DIRECTORY_NAME).exists(),
        "no feed ever created its fence directory"
    );
}

#[test]
fn the_monitor_setting_accepts_on_and_off_and_warns_on_anything_else() {
    use paneflow_git::monitor::parse_monitor_setting;
    assert!(parse_monitor_setting(None).enabled);
    assert!(parse_monitor_setting(Some("on")).enabled);
    assert!(!parse_monitor_setting(Some("off")).enabled);
    assert!(!parse_monitor_setting(Some(" OFF ")).enabled);
    let unknown = parse_monitor_setting(Some("disabled"));
    assert!(unknown.enabled);
    assert!(unknown
        .warning
        .expect("a warning")
        .contains("PANEFLOW_GIT_MONITOR"));
}
