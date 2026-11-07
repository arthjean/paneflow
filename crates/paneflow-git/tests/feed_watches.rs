#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use paneflow_git::feed::{ChangeFeed, FeedOptions, Ineligible};
use support::{committed_repo, git, write};

struct Fixture {
    _tmp: tempfile::TempDir,
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
    Fixture {
        _tmp: tmp,
        root,
        cache,
    }
}

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn linux_registers_one_non_recursive_watch_per_covered_directory() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = fixture();
    let before = paneflow_git::feed::inotify_watches_in_use();
    let feed = ChangeFeed::start(&fixture.root, &options(&fixture.cache)).expect("feed");
    let marker =
        std::fs::canonicalize(fixture.cache.join(paneflow_git::feed::FENCE_DIRECTORY_NAME))
            .unwrap();
    let mut expected: BTreeSet<PathBuf> = feed.coverage().directories.clone();
    expected.extend(feed.coverage().git_dirs.iter().cloned());
    expected.insert(marker);
    let watched: BTreeSet<PathBuf> = feed
        .watches()
        .iter()
        .map(|watch| {
            assert!(
                !watch.recursive,
                "{} is watched recursively",
                watch.path.display()
            );
            watch.path.clone()
        })
        .collect();
    assert_eq!(watched, expected);
    assert!(
        !watched
            .iter()
            .any(|path| path.starts_with(fixture.root.join("target"))
                || path.starts_with(fixture.root.join("build/tmp"))),
        "no watch inside an ignored directory without tracked entries: {watched:?}"
    );
    assert_eq!(
        paneflow_git::feed::inotify_watches_in_use() - before,
        feed.watches().len(),
        "the kernel holds exactly the watches the feed declares"
    );
}

#[test]
fn a_registration_failure_leaves_no_watch_installed() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root reads every directory");
        return;
    }
    let fixture = fixture();
    write(&fixture.root, "locked/f.txt", b"f\n");
    git(&fixture.root, &["add", "locked/f.txt"]);
    git(
        &fixture.root,
        &["commit", "-q", "--no-verify", "-m", "locked"],
    );
    let locked = fixture.root.join("locked");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o300)).unwrap();
    let before = paneflow_git::feed::inotify_watches_in_use();
    let refused = ChangeFeed::start(&fixture.root, &options(&fixture.cache));
    let after = paneflow_git::feed::inotify_watches_in_use();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let refused = refused.unwrap_err();
    assert!(
        matches!(&refused, Ineligible::Registration { cause } if cause.contains("locked")),
        "{refused:?}"
    );
    assert_eq!(after, before, "no partial watch set stays installed");
}

#[test]
fn a_feed_past_half_of_max_user_watches_is_refused_before_registering() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = fixture();
    let before = paneflow_git::feed::inotify_watches_in_use();
    let refused = ChangeFeed::start(
        &fixture.root,
        &FeedOptions {
            watch_limit: Some(2 * before + 8),
            ..options(&fixture.cache)
        },
    )
    .unwrap_err();
    assert!(
        matches!(refused, Ineligible::WatchBudget { limit, .. } if limit == 2 * before + 8),
        "{refused:?}"
    );
    assert_eq!(paneflow_git::feed::inotify_watches_in_use(), before);
}

#[test]
fn a_windows_drive_seen_from_wsl_gets_no_feed() {
    let _serial = SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return;
    };
    if !mountinfo
        .lines()
        .any(|line| line.contains(" /mnt/c ") && line.contains("drvfs"))
    {
        eprintln!("skipped: no WSL drive mounted at /mnt/c");
        return;
    }
    let before = paneflow_git::feed::inotify_watches_in_use();
    let refused = ChangeFeed::start(Path::new("/mnt/c"), &FeedOptions::default()).unwrap_err();
    assert!(
        matches!(refused, Ineligible::WslDrive { .. }),
        "{refused:?}"
    );
    assert_eq!(paneflow_git::feed::inotify_watches_in_use(), before);
}
