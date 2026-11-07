#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use paneflow_git::feed::{FeedOptions, Feeds, Loss};
use support::committed_repo;

const CHILD_WORKTREE_ENV: &str = "PANEFLOW_FEED_EXIT_CHILD_WORKTREE";
const CHILD_CACHE_ENV: &str = "PANEFLOW_FEED_EXIT_CHILD_CACHE";
const CHILD_TEST: &str = "a_feed_held_at_exit_leaves_nothing_behind_the_process";

#[test]
fn a_feed_held_at_exit_leaves_nothing_behind_the_process() {
    if let (Some(worktree), Some(cache)) = (
        std::env::var_os(CHILD_WORKTREE_ENV),
        std::env::var_os(CHILD_CACHE_ENV),
    ) {
        let mut feeds = Feeds::new(FeedOptions {
            cache_dir: Some(PathBuf::from(cache)),
            ..FeedOptions::default()
        });
        let feed = feeds
            .acquire(Path::new(&worktree), Instant::now())
            .expect("feed");
        assert_eq!(feed.drain().loss, Some(Loss::WatchSetChanged));
        std::fs::write(Path::new(&worktree).join("a/ready.txt"), b"ready\n").unwrap();
        std::process::exit(0);
    }
    support::isolate_process();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("repo");
    committed_repo(&root, &[("a/f.txt", "f\n")]);
    let root = std::fs::canonicalize(root).unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([CHILD_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD_WORKTREE_ENV, &root)
        .env(CHILD_CACHE_ENV, tmp.path().join("cache"))
        .status()
        .expect("child test process");
    assert!(status.success(), "child exited with {status}");
    assert!(
        root.join("a/ready.txt").exists(),
        "the child held a live feed when it exited"
    );
    std::fs::remove_dir_all(&root).expect("the worktree can be deleted right after the exit");
}
