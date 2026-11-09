use super::*;
use crate::git_fixture as fixture;
use paneflow_git::engine::Engine;

const WAKE_DEADLINE: Duration = Duration::from_secs(10);

struct Repo {
    _temp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
}

fn repo() -> Repo {
    fixture::isolate_process();
    let temp = tempfile::tempdir().expect("temporary directory");
    let root = temp.path().join("repo");
    std::fs::create_dir_all(root.join("src").join("deep")).expect("directories");
    fixture::init(&root);
    std::fs::write(root.join("top.txt"), "top\n").expect("top");
    std::fs::write(root.join("src").join("deep").join("inner.rs"), "inner\n").expect("inner");
    fixture::commit_all(&root, "initial");
    let cache = temp.path().join("cache");
    Repo {
        _temp: temp,
        root,
        cache,
    }
}

fn registry(repo: &Repo, idle: Duration) -> GitMonitors {
    GitMonitors::new(
        FeedOptions {
            cache_dir: Some(repo.cache.clone()),
            ..FeedOptions::default()
        },
        true,
        idle,
    )
}

fn probe(monitors: &GitMonitors, selector: &Selector, cwd: &Path, keep: Keep) -> Snapshot {
    monitors
        .with_snapshot(
            selector,
            cwd,
            &Options::files_status(),
            Freshness::Converging,
            keep,
            |snapshot, _| Ok(snapshot),
        )
        .expect("probe")
}

fn modified(snapshot: &Snapshot, path: &str) -> bool {
    snapshot.entries.iter().any(|entry| {
        entry.path == path.as_bytes()
            && entry.index_to_worktree == Some(paneflow_git::WorktreeChange::Modified)
    })
}

#[test]
fn consumers_probing_one_worktree_from_any_directory_share_one_monitor_and_one_feed() {
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION);
    let selector = Selector::new(Engine::Gix);
    std::fs::write(repo.root.join("top.txt"), "edited\n").expect("edit");
    let from_root = probe(&monitors, &selector, &repo.root, Keep::Alive);
    let subdirectory = repo.root.join("src");
    let from_subdirectory = probe(&monitors, &selector, &subdirectory, Keep::Alive);
    let hidden = probe(&monitors, &selector, &repo.root, Keep::IfLive);
    assert_eq!(monitors.monitors(), 1);
    assert_eq!(monitors.live_feeds(), 1);
    assert!(modified(&from_root, "top.txt"));
    assert_eq!(
        from_subdirectory,
        paneflow_git::snapshot(&subdirectory, &Options::files_status()).expect("full")
    );
    assert_eq!(from_subdirectory.prefix, b"src/".to_vec());
    assert_eq!(hidden, from_root);
    assert_eq!(selector.counters().fallbacks(Fallback::Monitor), 0);
}

#[test]
fn a_hidden_probe_without_a_live_monitor_takes_the_full_snapshot_and_starts_no_feed() {
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION);
    let selector = Selector::new(Engine::Gix);
    let hidden = probe(&monitors, &selector, &repo.root, Keep::IfLive);
    assert_eq!(monitors.monitors(), 0);
    assert_eq!(
        hidden,
        paneflow_git::snapshot(&repo.root, &Options::files_status()).expect("full")
    );
}

#[test]
fn an_evicted_monitor_is_not_restarted_by_a_hidden_probe() {
    let repo = repo();
    let monitors = registry(&repo, Duration::ZERO);
    let selector = Selector::new(Engine::Gix);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    assert_eq!(monitors.evict_idle(), 1);
    assert_eq!(monitors.monitors(), 0);
    probe(&monitors, &selector, &repo.root, Keep::IfLive);
    assert_eq!(monitors.monitors(), 0, "the hidden poll starts no feed");
}

#[test]
fn an_ineligible_feed_answers_with_the_full_snapshot_and_counts_a_monitor_fallback() {
    let repo = repo();
    let monitors = GitMonitors::new(
        FeedOptions {
            cache_dir: Some(repo.cache.clone()),
            max_events: 0,
            ..FeedOptions::default()
        },
        true,
        IDLE_EVICTION,
    );
    let selector = Selector::new(Engine::Gix);
    std::fs::write(repo.root.join("top.txt"), "edited\n").expect("edit");
    let answered = probe(&monitors, &selector, &repo.root, Keep::Alive);
    assert_eq!(
        answered,
        paneflow_git::snapshot(&repo.root, &Options::files_status()).expect("full")
    );
    assert!(modified(&answered, "top.txt"));
    assert_eq!(monitors.live_feeds(), 0);
    assert_eq!(selector.counters().fallbacks(Fallback::Monitor), 1);
}

#[test]
fn the_kill_switch_keeps_every_probe_off_the_monitors() {
    let repo = repo();
    let monitors = GitMonitors::new(
        FeedOptions {
            cache_dir: Some(repo.cache.clone()),
            ..FeedOptions::default()
        },
        false,
        IDLE_EVICTION,
    );
    let selector = Selector::new(Engine::Gix);
    let answered = probe(&monitors, &selector, &repo.root, Keep::Alive);
    assert_eq!(monitors.monitors(), 0);
    assert_eq!(
        selector
            .counters()
            .monitor(paneflow_git::engine::MonitorCounter::Disabled),
        1,
        "the kill switch is counted"
    );
    assert!(monitors.subscribe(&repo.root, Arc::new(|| {})).is_none());
    assert_eq!(
        answered,
        paneflow_git::snapshot(&repo.root, &Options::files_status()).expect("full")
    );
}

#[test]
fn a_subscriber_is_woken_by_a_write_after_its_probe_and_the_next_probe_shows_it() {
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION);
    let selector = Selector::new(Engine::Gix);
    let (woken, wakes) = std::sync::mpsc::channel();
    let subscription = monitors
        .subscribe(
            &repo.root.join("src"),
            Arc::new(move || {
                let _ = woken.send(());
            }),
        )
        .expect("a subscription");
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    while wakes.try_recv().is_ok() {}
    std::fs::write(
        repo.root.join("src").join("deep").join("inner.rs"),
        "edited\n",
    )
    .expect("edit");
    let deadline = Instant::now() + WAKE_DEADLINE;
    loop {
        wakes
            .recv_timeout(WAKE_DEADLINE)
            .expect("the monitor wakes its subscribers");
        if modified(
            &probe(&monitors, &selector, &repo.root, Keep::Alive),
            "src/deep/inner.rs",
        ) {
            break;
        }
        assert!(Instant::now() < deadline, "the edit never showed");
    }
    drop(subscription);
}

#[test]
fn releasing_a_worktree_drops_its_monitor_and_feed() {
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION);
    let selector = Selector::new(Engine::Gix);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    assert_eq!(monitors.monitors(), 1);
    assert!(monitors.release(&repo.root.join("src")));
    assert_eq!(monitors.monitors(), 0);
}

#[test]
fn objects_read_through_a_monitor_see_the_worktree_as_the_probe_spelled_it() {
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION);
    let selector = Selector::new(Engine::Gix);
    let read = |monitors: &GitMonitors| {
        monitors
            .with_snapshot(
                &selector,
                &repo.root.join("src"),
                &Options::files_status(),
                Freshness::Converging,
                Keep::Alive,
                |_, objects| Ok(objects.workdir().map(Path::to_path_buf)),
            )
            .expect("read")
    };
    let full = paneflow_git::with_snapshot(&repo.root, &Options::files_status(), |_, objects| {
        Ok(objects.workdir().map(Path::to_path_buf))
    })
    .expect("full");
    assert_eq!(read(&monitors), full, "the first probe");
    assert_eq!(read(&monitors), full, "an incremental probe");
}

#[test]
fn a_shown_worktree_verifies_through_the_registry_and_a_hidden_probe_never_does() {
    use paneflow_git::engine::MonitorCounter;
    let repo = repo();
    let monitors = registry(&repo, IDLE_EVICTION).with_timing(Timing {
        verify_after_probes: 1,
        ..Timing::default()
    });
    let selector = Selector::new(Engine::Gix);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    for _ in 0..3 {
        probe(&monitors, &selector, &repo.root, Keep::IfLive);
    }
    let counters = selector.counters();
    assert_eq!(counters.monitor(MonitorCounter::FirstProbe), 1);
    assert_eq!(counters.monitor(MonitorCounter::Incremental), 4);
    assert_eq!(counters.monitor(MonitorCounter::Verification), 0);
    let verified = probe(&monitors, &selector, &repo.root, Keep::Alive);
    assert_eq!(selector.counters().monitor(MonitorCounter::Verification), 1);
    assert_eq!(
        verified,
        paneflow_git::snapshot(&repo.root, &Options::files_status()).expect("full")
    );
}

#[test]
fn an_ineligible_feed_records_its_reason_on_the_engine_counters() {
    use paneflow_git::engine::MonitorCounter;
    let repo = repo();
    let monitors = GitMonitors::new(
        FeedOptions {
            cache_dir: Some(repo.cache.clone()),
            max_events: 0,
            ..FeedOptions::default()
        },
        true,
        IDLE_EVICTION,
    );
    let selector = Selector::new(Engine::Gix);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    probe(&monitors, &selector, &repo.root, Keep::Alive);
    let counters = selector.counters();
    assert_eq!(counters.monitor(MonitorCounter::IneligibleRegistration), 2);
    assert_eq!(counters.fallbacks(Fallback::Monitor), 2);
}
