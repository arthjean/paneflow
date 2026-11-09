#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_git::feed::{
    check_file_system, Batch, ChangeFeed, Coverage, FeedOptions, Feeds, Ineligible, Loss, Origin,
    IDLE_EVICTION,
};
use support::{committed_repo, git, write};

const OUTCOME_DEADLINE: Duration = Duration::from_secs(30);

struct Fixture {
    tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn options(cache: &Path) -> FeedOptions {
    FeedOptions {
        cache_dir: Some(cache.to_path_buf()),
        ..FeedOptions::default()
    }
}

fn fixture() -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(
        &root,
        &[
            ("top.txt", "top\n"),
            ("a/b/f.txt", "f\n"),
            (".gitignore", "target/\nbuild/\n*.log\n"),
        ],
    );
    write(&root, "build/keep.txt", b"keep\n");
    git(&root, &["add", "-f", "build/keep.txt"]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "keep"]);
    write(&root, "build/tmp/x.txt", b"x\n");
    write(&root, "target/debug/app.o", b"o\n");
    write(&root, "u/v/w.txt", b"w\n");
    write(&root, "logs/only.log", b"log\n");
    std::fs::create_dir_all(root.join("e")).expect("empty dir");
    std::fs::create_dir_all(root.join("nested")).expect("nested dir");
    git(&root.join("nested"), &["init", "-q"]);
    write(&root, "nested/inner/file.txt", b"inner\n");
    let cache = tmp.path().join("cache");
    let root = std::fs::canonicalize(&root).expect("canonical root");
    Fixture { tmp, root, cache }
}

fn relative_set(root: &Path, paths: &BTreeSet<PathBuf>) -> BTreeSet<String> {
    paths
        .iter()
        .map(|path| {
            path.strip_prefix(root)
                .unwrap_or_else(|_| panic!("{} is outside {}", path.display(), root.display()))
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect()
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|item| item.to_string()).collect()
}

fn collect_until(feed: &mut ChangeFeed, mut done: impl FnMut(&Batch) -> bool) -> Batch {
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    let mut all = Batch::default();
    loop {
        let mut batch = feed.drain();
        all.loss = all.loss.or(batch.loss);
        all.dropped += batch.dropped;
        all.changes.append(&mut batch.changes);
        all.generation = batch.generation;
        all.sequence = batch.sequence;
        if done(&all) {
            return all;
        }
        assert!(
            Instant::now() < deadline,
            "the feed never reached the expected outcome: {all:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn reported(batch: &Batch, relative: &str) -> bool {
    batch.changes.iter().any(|change| {
        change.origin == Origin::Worktree
            && change.relative.to_string_lossy().replace('\\', "/") == relative
    })
}

fn settle(feed: &mut ChangeFeed) {
    let first = feed.drain();
    assert_eq!(
        first.loss,
        Some(Loss::WatchSetChanged),
        "a new feed asks for a baseline before anything else"
    );
}

#[test]
fn coverage_holds_every_directory_with_tracked_or_untracked_content_and_skips_ignored_ones() {
    let fixture = fixture();
    let coverage = Coverage::scan(&fixture.root, 1).expect("coverage");
    assert_eq!(coverage.worktree, fixture.root);
    assert_eq!(
        relative_set(&fixture.root, &coverage.directories),
        names(&["", "a", "a/b", "build", "e", "logs", "u", "u/v"]),
        "tracked and untracked directories are covered, ignored and nested repositories are not"
    );
    let ignored: BTreeSet<String> = coverage
        .ignored
        .iter()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(ignored, names(&["build/tmp", "target"]));
    assert_eq!(
        coverage.git_dirs,
        BTreeSet::from([std::fs::canonicalize(fixture.root.join(".git")).unwrap()])
    );
}

#[cfg(any(target_os = "macos", windows))]
#[test]
fn one_recursive_watch_covers_the_worktree_and_its_git_dir() {
    let fixture = fixture();
    let feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    let worktree_watches: Vec<_> = feed
        .watches()
        .iter()
        .filter(|watch| Some(watch.path.as_path()) != feed.fence_directory())
        .collect();
    assert_eq!(worktree_watches.len(), 1, "{worktree_watches:?}");
    assert_eq!(worktree_watches[0].path, fixture.root);
    assert!(worktree_watches[0].recursive);
}

#[test]
fn events_under_ignored_directories_are_dropped_and_tracked_edits_are_reported() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    write(&fixture.root, "target/debug/new.o", b"new\n");
    write(&fixture.root, "build/tmp/y.txt", b"y\n");
    write(&fixture.root, "a/b/f.txt", b"edited\n");
    let batch = collect_until(&mut feed, |batch| reported(batch, "a/b/f.txt"));
    assert_eq!(batch.loss, None, "{batch:?}");
    let leaked: Vec<_> = batch
        .changes
        .iter()
        .filter(|change| {
            change.relative.starts_with("target")
                || change.relative.starts_with(Path::new("build").join("tmp"))
        })
        .collect();
    assert!(
        leaked.is_empty(),
        "ignored events reached the probe: {leaked:?}"
    );
}

#[test]
fn git_dir_changes_are_reported_with_their_origin() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    write(&fixture.root, "a/b/f.txt", b"staged\n");
    git(&fixture.root, &["add", "a/b/f.txt"]);
    let batch = collect_until(&mut feed, |batch| {
        batch
            .changes
            .iter()
            .any(|change| change.origin == Origin::GitDir && change.relative == Path::new("index"))
    });
    assert_eq!(batch.loss, None);
}

#[test]
fn a_linked_worktree_covers_its_own_git_dir_and_the_common_dir() {
    let fixture = fixture();
    let linked = fixture.tmp.path().join("linked");
    git(
        &fixture.root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            linked.to_str().unwrap(),
        ],
    );
    let linked = std::fs::canonicalize(&linked).unwrap();
    let feed = ChangeFeed::start(&linked, &options(&fixture.cache)).expect("feed");
    let common = std::fs::canonicalize(fixture.root.join(".git")).unwrap();
    let own = std::fs::canonicalize(fixture.root.join(".git/worktrees/linked")).unwrap();
    assert_eq!(
        feed.coverage().git_dirs,
        BTreeSet::from([common.clone(), own.clone()])
    );
    let watched: BTreeSet<PathBuf> = feed
        .watches()
        .iter()
        .map(|watch| watch.path.clone())
        .collect();
    assert!(watched.contains(&linked), "{watched:?}");
    assert!(watched.contains(&common), "{watched:?}");
    if cfg!(target_os = "linux") {
        assert!(watched.contains(&own), "{watched:?}");
    }
}

#[test]
fn a_fence_certifies_every_write_that_completed_before_it() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    #[cfg(windows)]
    assert_eq!(
        feed.journal_status(),
        Ok(()),
        "the volume of the temporary directory keeps a change journal"
    );
    settle(&mut feed);
    for trial in 0..50 {
        let name = format!("u/fenced-{trial}.txt");
        write(&fixture.root, &name, b"fenced\n");
        let batch = feed
            .fence(Instant::now() + OUTCOME_DEADLINE)
            .expect("fence");
        assert_eq!(batch.loss, None, "trial {trial}: {batch:?}");
        assert!(reported(&batch, &name), "trial {trial}: {batch:?}");
        assert!(
            batch.changes.iter().all(|change| !change
                .root
                .starts_with(fixture.cache.join(paneflow_git::feed::FENCE_DIRECTORY_NAME))),
            "marker events never reach the probe"
        );
    }
}

#[cfg(windows)]
#[test]
fn windows_fences_through_the_change_journal_without_a_marker_directory() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    assert_eq!(feed.fence_directory(), None);
    settle(&mut feed);
    let name = "u/journal-delete.txt";
    write(&fixture.root, name, b"doomed\n");
    feed.fence(Instant::now() + OUTCOME_DEADLINE)
        .expect("fence");
    std::fs::remove_file(fixture.root.join(name)).expect("delete");
    let batch = feed
        .fence(Instant::now() + OUTCOME_DEADLINE)
        .expect("fence");
    assert!(
        batch.changes.iter().any(
            |change| change.kind == paneflow_git::feed::ChangeKind::Children
                && change.relative == std::path::Path::new("u")
        ),
        "a delete the journal cannot name marks its directory: {batch:?}"
    );
}

#[cfg(unix)]
#[test]
fn the_fence_marker_directory_is_private_and_lives_in_the_paneflow_cache() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = fixture();
    let feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    let marker =
        std::fs::canonicalize(fixture.cache.join(paneflow_git::feed::FENCE_DIRECTORY_NAME))
            .unwrap();
    assert_eq!(feed.fence_directory(), Some(marker.as_path()));
    let mode = std::fs::metadata(&marker).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
    assert!(!marker.starts_with(&fixture.root));
}

#[cfg(unix)]
#[test]
fn a_marker_directory_that_cannot_be_created_makes_the_feed_ineligible() {
    let fixture = fixture();
    std::fs::write(&fixture.cache, b"a file, not a directory").unwrap();
    let refused = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).unwrap_err();
    assert!(
        matches!(refused, Ineligible::MarkerDirectory { .. }),
        "{refused:?}"
    );
    let refused = ChangeFeed::start(&fixture.root, &FeedOptions::default()).unwrap_err();
    assert_eq!(refused, Ineligible::NoCacheDirectory);
}

#[cfg(windows)]
#[test]
fn windows_needs_no_marker_directory() {
    let fixture = fixture();
    std::fs::write(&fixture.cache, b"a file, not a directory").unwrap();
    ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    ChangeFeed::start(&fixture.root, &FeedOptions::default()).expect("feed");
}

#[cfg(windows)]
#[test]
fn a_wsl_path_seen_from_windows_gets_no_feed() {
    let Some(distribution) = std::fs::read_dir(r"\\wsl.localhost\")
        .ok()
        .and_then(|mut entries| entries.find_map(Result::ok))
    else {
        eprintln!("skipped: no WSL distribution reachable");
        return;
    };
    let refused = ChangeFeed::start(&distribution.path(), &FeedOptions::default()).unwrap_err();
    assert!(
        matches!(refused, Ineligible::WslNetworkPath { .. }),
        "{refused:?}"
    );
}

#[test]
fn the_local_temporary_directory_is_eligible() {
    let tmp = tempfile::tempdir().unwrap();
    let path = std::fs::canonicalize(tmp.path()).unwrap();
    check_file_system(&path).expect("the test machine's temporary directory is local");
}

#[test]
fn an_ineligible_worktree_is_refused_once_until_it_is_evicted() {
    support::isolate_process();
    let tmp = tempfile::tempdir().unwrap();
    let mut feeds = Feeds::new(options(&tmp.path().join("cache")));
    let t0 = Instant::now();
    let first = feeds.acquire(tmp.path(), t0).unwrap_err();
    assert!(matches!(first, Ineligible::Scan { .. }), "{first:?}");
    let second = feeds
        .acquire(tmp.path(), t0 + Duration::from_secs(1))
        .unwrap_err();
    assert_eq!(second, first);
    assert_eq!(
        feeds.starts(),
        1,
        "the refusal is remembered, not retried per probe"
    );
    assert_eq!(
        feeds.evict_idle(t0 + Duration::from_secs(1) + IDLE_EVICTION),
        1
    );
    let _ = feeds.acquire(tmp.path(), t0 + 2 * IDLE_EVICTION);
    assert_eq!(feeds.starts(), 2);
}

#[test]
fn feeds_drain_and_time_out_without_blocking_their_thread() {
    fn movable<T: Send + 'static>() {}
    movable::<ChangeFeed>();
    movable::<Feeds>();
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    let quiet = feed.drain();
    assert!(
        quiet.changes.is_empty() && quiet.loss.is_none(),
        "{quiet:?}"
    );
    let expired = feed.fence(Instant::now()).map(|_| ());
    if cfg!(windows) {
        assert_eq!(
            expired,
            Ok(()),
            "the journal fence reads up to its position without waiting"
        );
    } else {
        assert_eq!(expired, Err(paneflow_git::feed::FenceError::TimedOut));
    }
}

#[test]
fn renaming_a_watched_worktree_succeeds_and_reports_a_loss() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    let moved = fixture.root.with_file_name("moved");
    std::fs::rename(&fixture.root, &moved).expect("a watched worktree can be renamed");
    std::fs::create_dir(&fixture.root).unwrap();
    write(&fixture.root, "replacement.txt", b"new\n");
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    loop {
        let batch = feed.drain();
        if batch.loss.is_some() {
            return;
        }
        assert!(
            !reported(&batch, "replacement.txt"),
            "an event from the replacement directory arrived before any loss: {batch:?}"
        );
        assert!(
            Instant::now() < deadline,
            "renaming the worktree never surfaced as a loss"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn deleting_a_watched_worktree_succeeds_and_reports_a_loss() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    std::fs::remove_dir_all(&fixture.root).expect("a watched worktree can be deleted");
    collect_until(&mut feed, |batch| batch.loss.is_some());
    for drain in 0..3 {
        let batch = feed.drain();
        assert!(
            batch.loss.is_some(),
            "drain {drain} after the deletion trusted a feed that cannot watch: {batch:?}"
        );
    }
}

#[test]
fn an_event_storm_past_the_queue_bounds_surfaces_as_an_overflow() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    std::fs::create_dir(fixture.root.join("late")).unwrap();
    let storm = paneflow_git::feed::MAX_QUEUED_EVENTS + 2_000;
    for index in 0..storm {
        std::fs::write(
            fixture.root.join("u").join(format!("storm-{index:05}")),
            b"s",
        )
        .unwrap();
    }
    let batch = collect_until(&mut feed, |batch| batch.loss.is_some());
    assert_eq!(batch.loss, Some(Loss::Overflow), "{:?}", batch.loss);
    assert!(
        batch.changes.len() <= 2 * paneflow_git::feed::MAX_QUEUED_EVENTS,
        "the queue never holds more than its bound: {} changes",
        batch.changes.len()
    );
    write(&fixture.root, "late/inner.txt", b"after the overflow\n");
    collect_until(&mut feed, |batch| reported(batch, "late/inner.txt"));
}

#[test]
fn an_ignored_directory_replaced_by_a_file_is_still_reported() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    std::fs::remove_dir_all(fixture.root.join("target")).unwrap();
    write(&fixture.root, "target", b"now a file\n");
    let batch = collect_until(&mut feed, |batch| {
        batch.changes.iter().any(|change| {
            change.relative == Path::new("target")
                && change.kind != paneflow_git::feed::ChangeKind::Modify
        })
    });
    assert_eq!(batch.loss, None, "{batch:?}");
}

#[test]
fn production_feeds_keep_their_fence_marker_in_the_paneflow_cache() {
    let options = FeedOptions::for_paneflow_home();
    assert_eq!(options.cache_dir, paneflow_home::cache_dir());
    assert_eq!(options.max_events, paneflow_git::feed::MAX_QUEUED_EVENTS);
    assert_eq!(options.max_bytes, paneflow_git::feed::MAX_QUEUED_BYTES);
    assert_eq!(options.watch_limit, None);
}

fn reported_once_the_trigger_is_answered(feed: &mut ChangeFeed, trigger: &str, relative: &str) {
    collect_until(feed, |batch| {
        batch.loss.is_some() || reported(batch, trigger)
    });
    let worktree = feed.worktree().to_path_buf();
    write(&worktree, relative, b"written after the trigger\n");
    collect_until(feed, |batch| reported(batch, relative));
}

#[test]
fn writes_inside_a_directory_created_after_the_start_reach_the_probe() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    std::fs::create_dir_all(fixture.root.join("fresh/deeper")).unwrap();
    reported_once_the_trigger_is_answered(&mut feed, "fresh", "fresh/deeper/inner.txt");
}

#[test]
fn a_feed_keeps_reporting_after_a_watched_directory_is_removed() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    std::fs::remove_dir_all(fixture.root.join("a")).unwrap();
    reported_once_the_trigger_is_answered(&mut feed, "a", "top.txt");
}

#[test]
fn writes_inside_a_directory_the_ignore_rules_no_longer_exclude_reach_the_probe() {
    let fixture = fixture();
    let mut feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    settle(&mut feed);
    write(&fixture.root, ".gitignore", b"build/\n*.log\n");
    reported_once_the_trigger_is_answered(&mut feed, ".gitignore", "target/debug/new.o");
}
