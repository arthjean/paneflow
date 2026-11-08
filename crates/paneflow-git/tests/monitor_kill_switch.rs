#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use paneflow_git::feed::{FeedOptions, FENCE_DIRECTORY_NAME};
use paneflow_git::monitor::{monitor_enabled, MONITOR_ENV};
use paneflow_git::{Freshness, Monitor, Options, ProbeKind};

#[test]
fn paneflow_git_monitor_off_makes_every_new_monitor_take_full_snapshots() {
    support::isolate_process();
    std::env::set_var(MONITOR_ENV, "off");
    assert!(!monitor_enabled());
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    support::committed_repo(&root, &[("top.txt", "top\n")]);
    let cache = tmp.path().join("cache");
    let mut monitor = Monitor::new(
        &root,
        FeedOptions {
            cache_dir: Some(cache.clone()),
            ..FeedOptions::default()
        },
    );
    assert!(!monitor.is_enabled());
    for freshness in [Freshness::Converging, Freshness::Fenced] {
        monitor
            .probe(&root, &Options::default(), freshness)
            .expect("probe");
        assert_eq!(monitor.last_probe().kind, ProbeKind::Disabled);
        assert!(monitor.feed().is_none());
    }
    assert!(!cache.join(FENCE_DIRECTORY_NAME).exists());
}
