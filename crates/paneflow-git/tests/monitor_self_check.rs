#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod monitor_support;
mod support;

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use monitor_support::{canonical, feed_options};
use paneflow_git::{Freshness, Monitor, Options, ProbeKind};
use support::committed_repo;

const QUIET_DEADLINE: Duration = Duration::from_secs(10);

const QUIET_CHECKS: usize = 100;

const QUIET_PAUSE: Duration = Duration::from_millis(10);

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn fixture() -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(
        &root,
        &[
            ("top.txt", "top\n"),
            ("src/lib.rs", "lib\n"),
            ("src/deep/inner.rs", "inner\n"),
        ],
    );
    let root = canonical(&root);
    support::git(&root, &["status", "--porcelain"]);
    let cache = tmp.path().join("cache");
    Fixture {
        _tmp: tmp,
        root,
        cache,
    }
}

fn wait_for_quiet(wakes: &AtomicUsize) -> usize {
    let deadline = Instant::now() + QUIET_DEADLINE;
    let mut last = wakes.load(Ordering::SeqCst);
    let mut quiet = 0;
    while quiet < QUIET_CHECKS {
        assert!(
            Instant::now() < deadline,
            "the feed went quiet within {QUIET_DEADLINE:?}"
        );
        std::thread::sleep(QUIET_PAUSE);
        let now = wakes.load(Ordering::SeqCst);
        quiet = if now == last { quiet + 1 } else { 0 };
        last = now;
    }
    last
}

#[test]
fn an_idle_worktree_probed_a_hundred_times_wakes_no_consumer_and_rereads_nothing() {
    let fixture = fixture();
    let mut monitor = Monitor::configured(&fixture.root, feed_options(&fixture.cache), true);
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    monitor.set_waker(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let root = fixture.root.clone();
    monitor
        .probe(&root, &Options::files_status(), Freshness::Converging)
        .expect("baseline");
    let before = wait_for_quiet(&wakes);
    for probe in 0..100 {
        monitor
            .probe(&root, &Options::files_status(), Freshness::Converging)
            .expect("idle probe");
        let report = monitor.last_probe();
        assert_eq!(
            report.kind,
            ProbeKind::Incremental,
            "probe {probe}: {report:?}"
        );
        assert_eq!(
            (
                report.rechecked_entries,
                report.walked_paths,
                report.files_hashed,
                report.index_read
            ),
            (0, 0, 0, false),
            "probe {probe} found nothing to re-read: {report:?}"
        );
    }
    let after = wait_for_quiet(&wakes);
    assert_eq!(
        after, before,
        "a hundred probes of an idle worktree woke its consumers"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_memory_mapped_write_that_inotify_never_reports_is_repaired_by_the_next_verification() {
    use paneflow_git::{Reason, Timing, WorktreeChange};

    let fixture = fixture();
    let mut monitor = Monitor::configured(&fixture.root, feed_options(&fixture.cache), true)
        .with_timing(Timing {
            verify_after_probes: 2,
            ..Timing::default()
        });
    let fenced = |monitor: &mut Monitor| {
        monitor
            .fenced_snapshot(&Options::default(), Instant::now() + QUIET_DEADLINE)
            .expect("fenced probe")
    };
    fenced(&mut monitor);
    let target = fixture.root.join("top.txt");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&target)
        .expect("open for mapping");
    let len = file.metadata().expect("length").len() as usize;
    let mapped = map_shared(&file, len);
    drop(file);
    let before = fenced(&mut monitor);
    assert_eq!(monitor.last_probe().kind, ProbeKind::Incremental);
    assert!(before.entries.is_empty(), "{:?}", before.entries);
    let modified_before = std::fs::metadata(&target).expect("stat").modified().ok();
    write_through_mapping(mapped, len, b"TOP\n");
    let stale = fenced(&mut monitor);
    assert_eq!(monitor.last_probe().kind, ProbeKind::Incremental);
    assert!(
        stale.entries.is_empty(),
        "inotify reported the mapped write, so this check proves nothing: {:?}",
        stale.entries
    );
    assert_ne!(
        std::fs::metadata(&target).expect("stat").modified().ok(),
        modified_before,
        "the mapped write moved the file's mtime, so a full status sees it"
    );
    let repaired = monitor
        .snapshot(&Options::default())
        .expect("verifying probe");
    assert_eq!(
        monitor.last_probe().kind,
        ProbeKind::Baseline(Reason::Drift(1))
    );
    assert!(repaired.entries.iter().any(|entry| entry.path == b"top.txt"
        && entry.index_to_worktree == Some(WorktreeChange::Modified)));
    assert_eq!(
        repaired,
        paneflow_git::snapshot(&fixture.root, &Options::default()).expect("full")
    );
    unmap(mapped, len);
}

#[cfg(target_os = "linux")]
fn map_shared(file: &std::fs::File, len: usize) -> usize {
    use std::os::fd::AsRawFd;
    let address = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            file.as_raw_fd(),
            0,
        )
    };
    assert_ne!(address, libc::MAP_FAILED, "mmap");
    address as usize
}

#[cfg(target_os = "linux")]
fn write_through_mapping(address: usize, len: usize, content: &[u8]) {
    assert_eq!(content.len(), len, "a same-size rewrite");
    let pointer = address as *mut u8;
    unsafe {
        std::ptr::copy_nonoverlapping(content.as_ptr(), pointer, len);
        assert_eq!(libc::msync(pointer.cast(), len, libc::MS_SYNC), 0, "msync");
    }
}

#[cfg(target_os = "linux")]
fn unmap(address: usize, len: usize) {
    let pointer = address as *mut libc::c_void;
    assert_eq!(unsafe { libc::munmap(pointer, len) }, 0, "munmap");
}
