#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod monitor_support;
mod support;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use monitor_support::{canonical, monitor, paths, settled, FENCED};
use paneflow_git::monitor::Input;
use paneflow_git::{
    snapshot, Error, IndexChange, Monitor, Options, ProbeKind, Reason, Submodules, WorktreeChange,
};
use support::{backdate, commit_all, committed_repo, git, write};

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    outside: PathBuf,
}

fn fixture_with(files: usize) -> Fixture {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    let mut owned: Vec<(String, String)> = (0..files)
        .map(|index| {
            (
                format!("dir{}/file{index}.txt", index % 4),
                format!("content {index}\n"),
            )
        })
        .collect();
    owned.push(("top.txt".into(), "top\n".into()));
    owned.push((".gitignore".into(), "target/\n*.log\n".into()));
    let files: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    committed_repo(&root, &files);
    let outside = tmp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    let cache = tmp.path().join("cache");
    let root = canonical(&root);
    for entry in walk_files(&root) {
        backdate(&entry);
    }
    git(&root, &["status", "--porcelain"]);
    Fixture {
        _tmp: tmp,
        root,
        cache,
        outside,
    }
}

fn fixture() -> Fixture {
    fixture_with(40)
}

fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("read dir") {
            let entry = entry.expect("entry");
            if entry.file_name() == ".git" {
                continue;
            }
            let file_type = entry.file_type().expect("type");
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                found.push(entry.path());
            }
        }
    }
    found
}

fn baselined(fixture: &Fixture) -> Monitor {
    let mut monitor = monitor(&fixture.root, &fixture.cache);
    let first = settled(&mut monitor, &Options::default());
    assert!(
        matches!(
            first.reports[0].kind,
            ProbeKind::Baseline(Reason::FirstProbe)
        ),
        "{:?}",
        first.reports
    );
    monitor
}

fn change_of<'a>(
    snapshot: &'a paneflow_git::Snapshot,
    path: &str,
) -> Option<&'a paneflow_git::Entry> {
    snapshot
        .entries
        .iter()
        .find(|entry| entry.path == path.as_bytes())
}

#[test]
fn the_first_probe_watches_before_it_scans_and_a_write_during_the_scan_shows_next() {
    let fixture = fixture_with(2_000);
    let mut monitor = monitor(&fixture.root, &fixture.cache);
    let root = fixture.root.clone();
    let writer = std::thread::spawn(move || {
        for index in 0..20 {
            write(&root, &format!("dir1/during{index}.txt"), b"during\n");
        }
    });
    let first = monitor.snapshot(&Options::default()).expect("first probe");
    assert!(matches!(
        monitor.last_probe().kind,
        ProbeKind::Baseline(Reason::FirstProbe)
    ));
    assert!(
        monitor.feed().is_some(),
        "the feed exists once the baseline ran"
    );
    writer.join().expect("writer");
    let next = settled(&mut monitor, &Options::default());
    let listed = paths(next.snapshot.as_ref().expect("snapshot"));
    for index in 0..20 {
        assert!(
            listed.contains(&format!("dir1/during{index}.txt")),
            "write {index} shows after the baseline that may have missed it: first had {} entries",
            first.entries.len()
        );
    }
}

#[test]
fn an_input_changed_outside_the_worktree_is_caught_by_its_stat_without_any_event() {
    let fixture = fixture();
    let excludes = fixture.outside.join("excludes");
    std::fs::write(&excludes, "").expect("excludes file");
    git(
        &fixture.root,
        &[
            "config",
            "core.excludesFile",
            &support::forward_slashes(&excludes),
        ],
    );
    write(&fixture.root, "dir0/scratch.tmp", b"scratch\n");
    let mut monitor = baselined(&fixture);
    std::fs::write(&excludes, "*.tmp\n").expect("ignore tmp files");
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        after.last().kind,
        ProbeKind::Baseline(Reason::Input(Input::Excludes)),
        "{:?}",
        after.reports
    );
    assert!(change_of(after.snapshot.as_ref().unwrap(), "dir0/scratch.tmp").is_none());

    let attributes = fixture.outside.join("attributes");
    git(
        &fixture.root,
        &[
            "config",
            "core.attributesFile",
            &support::forward_slashes(&attributes),
        ],
    );
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        after.last().kind,
        ProbeKind::Baseline(Reason::Input(Input::Config))
    );
    std::fs::write(&attributes, "*.txt text eol=crlf\n").expect("attributes");
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        after.last().kind,
        ProbeKind::Baseline(Reason::Input(Input::Attributes))
    );
}

#[test]
fn the_monitor_keeps_no_repository_open_across_a_repack_and_prune() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir2/file2.txt", b"edited\n");
    settled(&mut monitor, &Options::default());
    git(&fixture.root, &["repack", "-a", "-d", "-q"]);
    git(&fixture.root, &["gc", "-q", "--prune=now"]);
    write(&fixture.root, "dir2/file6.txt", b"edited too\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
}

#[test]
fn an_interrupted_probe_leaves_the_model_and_its_dirty_set_for_the_next_probe() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir3/file3.txt", b"edited\n");
    if FENCED {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        monitor
            .fenced_snapshot(
                &Options {
                    interrupt: Some(Arc::new(AtomicBool::new(true))),
                    ..Options::default()
                },
                deadline,
            )
            .map(|_| ())
            .map_err(|error| assert!(matches!(error, Error::Interrupted), "{error}"))
            .expect_err("an interrupted probe fails");
    } else {
        let error = monitor
            .snapshot(&Options {
                interrupt: Some(Arc::new(AtomicBool::new(true))),
                ..Options::default()
            })
            .expect_err("an interrupted probe fails");
        assert!(matches!(error, Error::Interrupted), "{error}");
    }
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        change_of(after.snapshot.as_ref().unwrap(), "dir3/file3.txt")
            .and_then(|entry| entry.index_to_worktree),
        Some(WorktreeChange::Modified)
    );
    assert!(after.incremental(), "{:?}", after.reports);
}

#[test]
fn another_base_recomputes_only_the_staged_half() {
    let fixture = fixture();
    write(&fixture.root, "dir0/file0.txt", b"staged\n");
    git(&fixture.root, &["add", "dir0/file0.txt"]);
    git(
        &fixture.root,
        &["commit", "-q", "--no-verify", "-m", "second"],
    );
    let base = git(&fixture.root, &["rev-parse", "HEAD~1^{tree}"])
        .trim()
        .to_string();
    let mut monitor = baselined(&fixture);
    let with_base = settled(
        &mut monitor,
        &Options {
            base: Some(base),
            ..Options::default()
        },
    );
    let report = with_base.last();
    assert_eq!(report.kind, ProbeKind::Incremental);
    assert!(report.staged_recomputed);
    assert_eq!(report.rechecked_entries, 0);
    assert_eq!(report.files_hashed, 0);
    assert_eq!(
        change_of(with_base.snapshot.as_ref().unwrap(), "dir0/file0.txt")
            .and_then(|entry| entry.head_to_index),
        Some(IndexChange::Modified)
    );
    let back = settled(&mut monitor, &Options::default());
    assert!(!back.last().staged_recomputed, "the HEAD half is cached");
}

#[test]
fn one_tracked_edit_rechecks_one_entry() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir1/file5.txt", b"edited\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert_eq!(after.rechecked(), 1, "{:?}", after.reports);
    assert!(after.hashed() <= 1, "{:?}", after.reports);
    assert_eq!(after.walked(), 0, "a tracked edit walks no directory");
}

#[test]
fn deletions_and_kind_changes_of_tracked_files_match_the_full_snapshot() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    std::fs::remove_file(fixture.root.join("dir0/file4.txt")).expect("delete");
    settled(&mut monitor, &Options::default());
    std::fs::remove_file(fixture.root.join("dir0/file8.txt")).expect("remove");
    std::fs::create_dir_all(fixture.root.join("dir0/file8.txt")).expect("now a directory");
    write(&fixture.root, "dir0/file8.txt/inside.txt", b"inside\n");
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        change_of(after.snapshot.as_ref().unwrap(), "dir0/file8.txt")
            .and_then(|entry| entry.index_to_worktree),
        Some(WorktreeChange::Deleted)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::remove_file(fixture.root.join("dir1/file9.txt")).expect("remove");
        std::os::unix::fs::symlink("top.txt", fixture.root.join("dir1/file9.txt"))
            .expect("symlink");
        settled(&mut monitor, &Options::default());
        let path = fixture.root.join("dir2/file10.txt");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        settled(&mut monitor, &Options::default());
    }
}

#[test]
fn a_directory_renamed_above_tracked_entries_rechecks_both_endpoints() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    std::fs::rename(fixture.root.join("dir3"), fixture.root.join("moved3")).expect("rename dir");
    let after = settled(&mut monitor, &Options::default());
    let snapshot = after.snapshot.as_ref().unwrap();
    let deleted = snapshot
        .entries
        .iter()
        .filter(|entry| entry.index_to_worktree == Some(WorktreeChange::Deleted))
        .count();
    assert_eq!(deleted, 10, "every entry of dir3 is deleted");
    assert_eq!(
        snapshot
            .entries
            .iter()
            .filter(|entry| entry.untracked)
            .count(),
        10,
        "every file of moved3 is untracked"
    );
    std::fs::create_dir_all(fixture.root.join("fresh/deeper")).expect("mkdir");
    write(&fixture.root, "fresh/deeper/x.txt", b"x\n");
    settled(&mut monitor, &Options::default());
    std::fs::remove_dir_all(fixture.root.join("fresh")).expect("rmdir");
    settled(&mut monitor, &Options::default());
}

#[test]
fn a_case_only_rename_matches_the_full_snapshot() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    std::fs::rename(fixture.root.join("top.txt"), fixture.root.join("TOP.txt")).expect("rename");
    settled(&mut monitor, &Options::default());
    std::fs::rename(fixture.root.join("dir2"), fixture.root.join("DIR2")).expect("rename dir");
    settled(&mut monitor, &Options::default());
}

#[test]
fn an_untracked_path_goes_to_the_walk_and_never_to_the_tracked_pass() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir2/new.txt", b"new\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert_eq!(after.rechecked(), 0, "{:?}", after.reports);
    assert!(after.walked() >= 1);
    assert!(change_of(after.snapshot.as_ref().unwrap(), "dir2/new.txt")
        .is_some_and(|entry| entry.untracked));
}

#[test]
fn a_file_gone_between_its_event_and_its_stat_reads_as_deleted() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir1/file1.txt", b"edited then gone\n");
    std::fs::remove_file(fixture.root.join("dir1/file1.txt")).expect("delete");
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        change_of(after.snapshot.as_ref().unwrap(), "dir1/file1.txt")
            .and_then(|entry| entry.index_to_worktree),
        Some(WorktreeChange::Deleted)
    );
}

#[test]
fn a_dirty_path_inside_a_submodule_reprobes_that_submodule_only() {
    let fixture = fixture();
    let source = fixture.outside.join("sub-source");
    committed_repo(&source, &[("inner.txt", "inner\n")]);
    git(
        &fixture.root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &support::forward_slashes(&source),
            "sub",
        ],
    );
    commit_all(&fixture.root, "submodule");
    let options = Options {
        submodules: Submodules::Check,
        ..Options::default()
    };
    let mut monitor = monitor(&fixture.root, &fixture.cache);
    settled(&mut monitor, &options);
    write(&fixture.root, "dir0/file0.txt", b"unrelated\n");
    let unrelated = settled(&mut monitor, &options);
    assert!(unrelated.incremental());
    assert_eq!(
        unrelated
            .reports
            .iter()
            .map(|report| report.submodules_opened)
            .sum::<usize>(),
        0,
        "an edit outside the submodule reuses its status"
    );
    write(&fixture.root, "sub/inner.txt", b"inner edited\n");
    let inside = settled(&mut monitor, &options);
    assert!(inside
        .reports
        .iter()
        .any(|report| report.submodules_opened > 0));
    assert!(matches!(
        change_of(inside.snapshot.as_ref().unwrap(), "sub").and_then(|entry| entry.index_to_worktree),
        Some(WorktreeChange::Submodule(change)) if change.modified_content
    ));
}

#[test]
fn a_new_untracked_tree_is_walked_and_listed_file_by_file() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "a/b/c/f.txt", b"f\n");
    write(&fixture.root, "a/b/g.txt", b"g\n");
    let after = settled(&mut monitor, &Options::default());
    let listed = paths(after.snapshot.as_ref().unwrap());
    assert!(listed.contains(&"a/b/c/f.txt".to_string()), "{listed:?}");
    assert!(listed.contains(&"a/b/g.txt".to_string()), "{listed:?}");
    std::fs::remove_file(fixture.root.join("a/b/g.txt")).expect("delete untracked");
    settled(&mut monitor, &Options::default());
    std::fs::remove_dir_all(fixture.root.join("a")).expect("remove untracked tree");
    let gone = settled(&mut monitor, &Options::default());
    assert!(!paths(gone.snapshot.as_ref().unwrap())
        .iter()
        .any(|path| path.starts_with("a/")));
}

#[test]
fn more_untracked_entries_than_the_cap_truncate_like_the_full_snapshot() {
    let fixture = fixture();
    let options = Options {
        entry_cap: 3,
        ..Options::default()
    };
    let mut monitor = monitor(&fixture.root, &fixture.cache);
    settled(&mut monitor, &options);
    for index in 0..5 {
        write(&fixture.root, &format!("dir1/extra{index}.txt"), b"extra\n");
    }
    let after = settled(&mut monitor, &options);
    let snapshot = after.snapshot.as_ref().unwrap();
    assert!(snapshot.truncated);
    assert_eq!(snapshot.entries.len(), 3);
}

#[test]
fn an_ignore_rule_change_rebaselines() {
    let fixture = fixture();
    write(&fixture.root, "dir0/noise.log2", b"noise\n");
    let mut monitor = baselined(&fixture);
    write(&fixture.root, ".gitignore", b"target/\n*.log\n*.log2\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(
        after
            .reports
            .iter()
            .any(|report| report.kind == ProbeKind::Baseline(Reason::Input(Input::IgnoreFile))),
        "{:?}",
        after.reports
    );
    write(&fixture.root, ".git/info/exclude", b"top.txt\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(
        after
            .reports
            .iter()
            .any(|report| report.kind == ProbeKind::Baseline(Reason::Input(Input::Excludes))),
        "{:?}",
        after.reports
    );
}

#[test]
fn a_write_inside_an_ignored_directory_walks_nothing() {
    let fixture = fixture();
    write(
        &fixture.root,
        "target/debug/keep.o",
        b"object
",
    );
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "target/debug/app.o", b"object\n");
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert_eq!(after.walked(), 0, "{:?}", after.reports);
}

#[test]
fn git_status_refreshing_stat_data_reads_the_index_once_and_hashes_nothing() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(7_200);
    for index in [0, 4, 8] {
        let path = fixture.root.join(format!("dir0/file{index}.txt"));
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|file| file.set_modified(earlier))
            .expect("move the mtime two hours back");
    }
    let deadline = std::time::Instant::now() + monitor_support::CONVERGENCE_DEADLINE;
    let mut seen = 0;
    while seen < 3 && std::time::Instant::now() < deadline {
        seen += settled(&mut monitor, &Options::default()).rechecked();
    }
    assert!(seen >= 3, "the three mtime changes reach the monitor");
    git(&fixture.root, &["status", "--porcelain"]);
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert!(
        after.reports.iter().any(|report| report.index_read),
        "{:?}",
        after.reports
    );
    assert_eq!(after.hashed(), 0, "{:?}", after.reports);
    assert_eq!(after.rechecked(), 0, "{:?}", after.reports);
}

#[test]
fn git_add_of_three_files_rechecks_exactly_those_entries() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    for index in [1, 5, 9] {
        write(
            &fixture.root,
            &format!("dir1/file{index}.txt"),
            b"to stage\n",
        );
    }
    settled(&mut monitor, &Options::default());
    git(
        &fixture.root,
        &["add", "dir1/file1.txt", "dir1/file5.txt", "dir1/file9.txt"],
    );
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert_eq!(after.rechecked(), 3, "{:?}", after.reports);
    assert!(after.reports.iter().any(|report| report.staged_recomputed));
    let staged = after
        .snapshot
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .filter(|entry| entry.head_to_index == Some(IndexChange::Modified))
        .count();
    assert_eq!(staged, 3);
}

#[test]
fn a_skip_hash_index_rewritten_with_the_same_size_and_mtime_is_still_read() {
    let fixture = fixture();
    git(&fixture.root, &["config", "index.skipHash", "true"]);
    git(&fixture.root, &["rm", "-q", "--cached", "top.txt"]);
    git(&fixture.root, &["add", "top.txt"]);
    let index = fixture.root.join(".git").join("index");
    let bytes = std::fs::read(&index).expect("read the index");
    assert!(
        bytes[bytes.len() - 20..].iter().all(|byte| *byte == 0),
        "index.skipHash leaves a zero trailer"
    );
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir0/file0.txt", b"CONTENT 0\n");
    git(&fixture.root, &["add", "dir0/file0.txt"]);
    write(&fixture.root, "dir0/file0.txt", b"Content 0\n");
    settled(&mut monitor, &Options::default());
    let before = std::fs::metadata(&index).expect("index metadata");
    git(&fixture.root, &["add", "dir0/file0.txt"]);
    std::fs::File::options()
        .write(true)
        .open(&index)
        .and_then(|file| file.set_modified(before.modified()?))
        .expect("give the rewritten index the previous mtime");
    assert_eq!(
        std::fs::metadata(&index).expect("index metadata").len(),
        before.len(),
        "staging a same-size edit keeps the index size"
    );
    let after = settled(&mut monitor, &Options::default());
    assert_eq!(
        change_of(after.snapshot.as_ref().unwrap(), "dir0/file0.txt")
            .map(|entry| (entry.head_to_index, entry.index_to_worktree)),
        Some((Some(IndexChange::Modified), None))
    );
}

#[test]
fn git_commit_recomputes_the_staged_half_and_carries_worktree_verdicts() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir2/file2.txt", b"staged\n");
    write(&fixture.root, "dir3/file3.txt", b"unstaged\n");
    settled(&mut monitor, &Options::default());
    git(&fixture.root, &["add", "dir2/file2.txt"]);
    settled(&mut monitor, &Options::default());
    git(
        &fixture.root,
        &["commit", "-q", "--no-verify", "-m", "commit"],
    );
    let after = settled(&mut monitor, &Options::default());
    assert!(after.incremental(), "{:?}", after.reports);
    assert!(after.reports.iter().any(|report| report.staged_recomputed));
    let snapshot = after.snapshot.as_ref().unwrap();
    assert!(change_of(snapshot, "dir2/file2.txt").is_none());
    assert_eq!(
        change_of(snapshot, "dir3/file3.txt").and_then(|entry| entry.index_to_worktree),
        Some(WorktreeChange::Modified)
    );
}

#[test]
fn a_checkout_that_rewrites_most_of_the_index_rebaselines() {
    let fixture = fixture();
    git(&fixture.root, &["checkout", "-q", "-b", "other"]);
    for index in 0..10 {
        write(
            &fixture.root,
            &format!("dir{}/file{index}.txt", index % 4),
            b"other\n",
        );
    }
    commit_all(&fixture.root, "other");
    git(&fixture.root, &["checkout", "-q", "main"]);
    let mut monitor = baselined(&fixture);
    git(&fixture.root, &["checkout", "-q", "other"]);
    let after = settled(&mut monitor, &Options::default());
    assert!(
        after.reports.iter().any(|report| matches!(
            report.kind,
            ProbeKind::Baseline(Reason::IndexCarryLimit | Reason::Loss(_))
        )),
        "{:?}",
        after.reports
    );
}

#[test]
fn an_index_format_or_skip_worktree_change_rebaselines() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    git(&fixture.root, &["update-index", "--split-index"]);
    let after = settled(&mut monitor, &Options::default());
    assert!(
        after
            .reports
            .iter()
            .any(|report| report.kind == ProbeKind::Baseline(Reason::IndexFormat)),
        "{:?}",
        after.reports
    );
    git(&fixture.root, &["update-index", "--no-split-index"]);
    settled(&mut monitor, &Options::default());
    git(
        &fixture.root,
        &["update-index", "--skip-worktree", "dir0/file0.txt"],
    );
    let after = settled(&mut monitor, &Options::default());
    assert!(
        after.reports.iter().any(|report| matches!(
            report.kind,
            ProbeKind::Baseline(Reason::IndexFormat | Reason::SkipWorktree)
        )),
        "{:?}",
        after.reports
    );
}

#[test]
fn a_conflicted_merge_reports_the_conflict_kinds_of_the_full_snapshot() {
    let fixture = fixture();
    git(&fixture.root, &["checkout", "-q", "-b", "theirs"]);
    write(&fixture.root, "dir0/file0.txt", b"theirs\n");
    std::fs::remove_file(fixture.root.join("dir1/file1.txt")).expect("delete theirs");
    commit_all(&fixture.root, "theirs");
    git(&fixture.root, &["checkout", "-q", "main"]);
    write(&fixture.root, "dir0/file0.txt", b"ours\n");
    write(&fixture.root, "dir1/file1.txt", b"ours edit\n");
    commit_all(&fixture.root, "ours");
    let mut monitor = baselined(&fixture);
    let merge = support::output(&fixture.root, &["merge", "-q", "--no-edit", "theirs"]);
    assert!(!merge.status.success(), "the merge conflicts");
    let after = settled(&mut monitor, &Options::default());
    let snapshot = after.snapshot.as_ref().unwrap();
    assert!(change_of(snapshot, "dir0/file0.txt").is_some_and(|entry| entry.conflict.is_some()));
    assert!(change_of(snapshot, "dir1/file1.txt").is_some_and(|entry| entry.conflict.is_some()));
}

#[test]
fn a_limited_walk_reads_only_the_directories_of_the_dirty_paths() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    write(&fixture.root, "dir3/added.txt", b"added\n");
    let after = settled(&mut monitor, &Options::default());
    let reads: usize = after
        .reports
        .iter()
        .map(|report| report.read_dir_calls)
        .sum();
    assert!(
        (1..=2).contains(&reads),
        "the walk reads dir3 (and the root it starts from), not the four directories: {:?}",
        after.reports
    );
}

#[test]
fn paths_with_pathspec_magic_are_walked_literally() {
    let fixture = fixture();
    let mut monitor = baselined(&fixture);
    let names: &[&str] = if cfg!(windows) {
        &["dir0/[ab].txt", "!bang.txt", "dir1/a b.txt", "dir1/{x}.txt"]
    } else {
        &[
            "dir0/*.txt",
            "dir0/q?.txt",
            "dir0/[ab].txt",
            ":colon.txt",
            "!bang.txt",
        ]
    };
    write(&fixture.root, "dir0/a.txt", b"a\n");
    write(&fixture.root, "dir0/qx.txt", b"q\n");
    settled(&mut monitor, &Options::default());
    for name in names {
        write(&fixture.root, name, b"magic\n");
    }
    let after = settled(&mut monitor, &Options::default());
    let listed = paths(after.snapshot.as_ref().unwrap());
    for name in names {
        assert!(listed.contains(&name.to_string()), "{name} in {listed:?}");
    }
    for name in names {
        std::fs::remove_file(fixture.root.join(name)).expect("remove magic");
    }
    let gone = settled(&mut monitor, &Options::default());
    let listed = paths(gone.snapshot.as_ref().unwrap());
    assert!(
        listed.contains(&"dir0/a.txt".to_string()),
        "a literal walk of [ab] keeps a.txt"
    );
    assert!(
        listed.contains(&"dir0/qx.txt".to_string()),
        "a literal walk of q? keeps qx.txt"
    );
}

#[test]
fn an_ineligible_monitor_answers_with_full_snapshots() {
    let fixture = fixture();
    let mut monitor = Monitor::new(
        &fixture.root,
        paneflow_git::feed::FeedOptions {
            cache_dir: None,
            ..paneflow_git::feed::FeedOptions::default()
        },
    );
    let probed = monitor.snapshot(&Options::default()).expect("probe");
    let full = snapshot(&fixture.root, &Options::default()).expect("full");
    if FENCED {
        assert!(matches!(monitor.last_probe().kind, ProbeKind::Full(_)));
        assert!(monitor.feed().is_none());
    }
    assert!(monitor_support::differences(&probed, &full).is_empty());
}

#[test]
fn a_monitor_can_move_to_the_thread_that_owns_it() {
    fn owned_by_a_worker<T: Send + 'static>(_: &T) {}
    let fixture = fixture();
    let monitor = monitor(&fixture.root, &fixture.cache);
    owned_by_a_worker(&monitor);
    let handle = std::thread::spawn(move || {
        let mut monitor = monitor;
        monitor
            .snapshot(&Options::default())
            .map(|snapshot| snapshot.entries.len())
    });
    assert!(handle.join().expect("worker").is_ok());
}
