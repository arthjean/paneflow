#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use paneflow_git::{
    snapshot, Conflict, Entry, Error, Head, IndexChange, Options, SubmoduleChange, Submodules,
    Untracked, WorktreeChange,
};
use support::{
    backdate, commit_all, committed_repo, forward_slashes, git, init, marker_program, porcelain,
    write,
};

const RACE_FILE: &str = "agent-edit.bin";
const RACE_FILE_BYTES: usize = 480 * 1024;
const RACE_SNAPSHOTS: usize = 200;

fn repo() -> (tempfile::TempDir, PathBuf) {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    (tmp, root)
}

fn entries_by_path(root: &Path, options: &Options) -> BTreeMap<String, Entry> {
    snapshot(root, options)
        .expect("snapshot")
        .entries
        .into_iter()
        .map(|entry| (String::from_utf8(entry.path.clone()).expect("utf-8"), entry))
        .collect()
}

fn tracked(head_to_index: Option<IndexChange>, index_to_worktree: Option<WorktreeChange>) -> Entry {
    Entry {
        head_to_index,
        index_to_worktree,
        ..Entry::default()
    }
}

fn untracked() -> Entry {
    Entry {
        untracked: true,
        ..Entry::default()
    }
}

fn without_paths(entries: BTreeMap<String, Entry>) -> Vec<(String, Entry)> {
    entries
        .into_iter()
        .map(|(path, entry)| {
            assert_eq!(entry.path, path.as_bytes());
            (
                path,
                Entry {
                    path: Vec::new(),
                    ..entry
                },
            )
        })
        .collect()
}

#[test]
fn a_snapshot_lists_each_kind_of_change_sorted_by_path_bytes() {
    let (_tmp, root) = repo();
    committed_repo(
        &root,
        &[
            ("a.txt", "a\n"),
            ("b.txt", "b\n"),
            ("c.txt", "c\n"),
            ("d.txt", "d\n"),
            ("e.txt", "e\n"),
            (".gitignore", "*.log\n"),
        ],
    );
    write(&root, "a.txt", b"a staged\n");
    git(&root, &["add", "a.txt"]);
    write(&root, "b.txt", b"b in the worktree\n");
    std::fs::remove_file(root.join("c.txt")).expect("delete c");
    git(&root, &["rm", "-q", "d.txt"]);
    write(&root, "e.txt", b"e staged\n");
    git(&root, &["add", "e.txt"]);
    write(&root, "e.txt", b"e staged then edited\n");
    write(&root, "new.txt", b"new\n");
    git(&root, &["add", "new.txt"]);
    write(&root, "ita.txt", b"intent\n");
    git(&root, &["add", "-N", "ita.txt"]);
    write(&root, "nested/z.txt", b"z\n");
    write(&root, "nested/deep/y.txt", b"y\n");
    write(&root, "debug.log", b"ignored\n");

    let snapshot = snapshot(&root, &Options::default()).expect("snapshot");
    let tree = git(&root, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(snapshot.head, Head::Tree(tree.trim().to_string()));
    assert!(!snapshot.truncated);
    let paths: Vec<&[u8]> = snapshot
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted, "entries are sorted by path bytes");

    let entries = entries_by_path(&root, &Options::default());
    assert_eq!(
        without_paths(entries),
        vec![
            ("a.txt".into(), tracked(Some(IndexChange::Modified), None)),
            (
                "b.txt".into(),
                tracked(None, Some(WorktreeChange::Modified))
            ),
            ("c.txt".into(), tracked(None, Some(WorktreeChange::Deleted))),
            ("d.txt".into(), tracked(Some(IndexChange::Deleted), None)),
            (
                "e.txt".into(),
                tracked(Some(IndexChange::Modified), Some(WorktreeChange::Modified))
            ),
            (
                "ita.txt".into(),
                tracked(None, Some(WorktreeChange::IntentToAdd))
            ),
            ("nested/deep/y.txt".into(), untracked()),
            ("nested/z.txt".into(), untracked()),
            ("new.txt".into(), tracked(Some(IndexChange::Added), None)),
        ]
    );
}

#[test]
fn a_staged_rename_is_a_deletion_and_an_addition() {
    let (_tmp, root) = repo();
    committed_repo(&root, &[("old.txt", "same content for the rename\n")]);
    git(&root, &["mv", "old.txt", "renamed.txt"]);
    let entries = without_paths(entries_by_path(&root, &Options::default()));
    assert_eq!(
        entries,
        vec![
            ("old.txt".into(), tracked(Some(IndexChange::Deleted), None)),
            (
                "renamed.txt".into(),
                tracked(Some(IndexChange::Added), None)
            ),
        ]
    );
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_symlink_is_a_type_change_on_both_halves() {
    let (_tmp, root) = repo();
    committed_repo(&root, &[("link", "plain file\n"), ("other", "o\n")]);
    std::fs::remove_file(root.join("link")).expect("remove");
    std::os::unix::fs::symlink("other", root.join("link")).expect("symlink");
    let worktree = without_paths(entries_by_path(&root, &Options::default()));
    assert_eq!(
        worktree,
        vec![(
            "link".into(),
            tracked(None, Some(WorktreeChange::TypeChange))
        )]
    );
    git(&root, &["add", "link"]);
    let index = without_paths(entries_by_path(&root, &Options::default()));
    assert_eq!(
        index,
        vec![("link".into(), tracked(Some(IndexChange::TypeChange), None))]
    );
}

#[test]
fn a_merge_conflict_reports_its_kind() {
    let (_tmp, root) = repo();
    committed_repo(&root, &[("both.txt", "base\n"), ("gone.txt", "base\n")]);
    git(&root, &["switch", "-q", "-c", "side"]);
    write(&root, "both.txt", b"side\n");
    git(&root, &["rm", "-q", "gone.txt"]);
    commit_all(&root, "side");
    git(&root, &["switch", "-q", "main"]);
    write(&root, "both.txt", b"main\n");
    write(&root, "gone.txt", b"main\n");
    commit_all(&root, "main");
    let merge = support::output(&root, &["merge", "-q", "side"]);
    assert!(!merge.status.success(), "the merge conflicts");

    let entries = entries_by_path(&root, &Options::default());
    let conflicts: Vec<(&str, Option<Conflict>)> = entries
        .iter()
        .map(|(path, entry)| (path.as_str(), entry.conflict))
        .collect();
    assert_eq!(
        conflicts,
        vec![
            ("both.txt", Some(Conflict::BothModified)),
            ("gone.txt", Some(Conflict::DeletedByThem)),
        ]
    );
}

#[test]
fn an_unborn_head_compares_the_index_with_nothing() {
    let (_tmp, root) = repo();
    init(&root);
    write(&root, "staged.txt", b"staged\n");
    git(&root, &["add", "staged.txt"]);
    write(&root, "loose.txt", b"loose\n");
    let snapshot = snapshot(&root, &Options::default()).expect("snapshot");
    assert_eq!(snapshot.head, Head::Unborn);
    let entries: BTreeMap<String, Entry> = snapshot
        .entries
        .into_iter()
        .map(|entry| (String::from_utf8(entry.path.clone()).expect("utf-8"), entry))
        .collect();
    assert_eq!(
        without_paths(entries),
        vec![
            ("loose.txt".into(), untracked()),
            ("staged.txt".into(), tracked(Some(IndexChange::Added), None)),
        ]
    );
}

#[test]
fn a_base_other_than_head_is_compared_even_when_the_index_matches_head() {
    let (_tmp, root) = repo();
    committed_repo(
        &root,
        &[("kept.txt", "kept\n"), ("changed.txt", "before\n")],
    );
    let base = git(&root, &["rev-parse", "HEAD"]).trim().to_string();
    write(&root, "changed.txt", b"after\n");
    write(&root, "added.txt", b"added\n");
    commit_all(&root, "second");
    let options = Options {
        base: Some(base.clone()),
        ..Options::default()
    };

    let snapshot = snapshot(&root, &options).expect("snapshot");

    let base_tree = git(&root, &["rev-parse", &format!("{base}^{{tree}}")]);
    assert_eq!(snapshot.head, Head::Tree(base_tree.trim().to_string()));
    assert_eq!(
        without_paths(entries_by_path(&root, &options)),
        vec![
            ("added.txt".into(), tracked(Some(IndexChange::Added), None)),
            (
                "changed.txt".into(),
                tracked(Some(IndexChange::Modified), None)
            ),
        ]
    );
}

#[test]
fn a_stale_cache_tree_is_trusted_exactly_as_far_as_git_trusts_it() {
    let (tmp, root) = repo();
    committed_repo(&root, &[("kept.txt", "kept\n"), ("src/lib.rs", "lib\n")]);
    let blob = tmp.path().join("staged.txt");
    std::fs::write(&blob, b"lib staged\n").expect("write the staged blob");
    let staged = git(&root, &["hash-object", "-w", &forward_slashes(&blob)]);
    backdate(&root.join("src/lib.rs"));
    git(&root, &["update-index", "--refresh"]);
    let repository = gix::open(&root).expect("open the fixture");
    let mut index = repository.open_index().expect("read the index");
    index
        .entry_mut_by_path_and_stage("src/lib.rs".into(), gix::index::entry::Stage::Unconflicted)
        .expect("the tracked entry")
        .id = gix::ObjectId::from_hex(staged.trim().as_bytes()).expect("a blob id");
    index
        .write(gix::index::write::Options::default())
        .expect("rewrite the index entry without invalidating its cache-tree");

    assert_eq!(
        porcelain(&root),
        "",
        "git skips the staged diff of a tree its cache-tree marks as unchanged"
    );
    assert_eq!(
        without_paths(entries_by_path(&root, &Options::default())),
        Vec::new()
    );
}

#[test]
fn untracked_files_are_sorted_one_by_one_before_the_cap() {
    let (_tmp, root) = repo();
    committed_repo(&root, &[("kept.txt", "k\n")]);
    for name in ["e.txt", "dir/c.txt", "a.txt", "dir/b.txt", "d.txt"] {
        write(&root, name, b"u\n");
    }
    let capped = snapshot(
        &root,
        &Options {
            entry_cap: 2,
            ..Options::default()
        },
    )
    .expect("snapshot");
    assert!(capped.truncated);
    let paths: Vec<&[u8]> = capped
        .entries
        .iter()
        .map(|entry| entry.path.as_slice())
        .collect();
    assert_eq!(paths, vec![b"a.txt".as_slice(), b"d.txt".as_slice()]);

    let none = snapshot(
        &root,
        &Options {
            untracked: Untracked::None,
            ..Options::default()
        },
    )
    .expect("snapshot");
    assert_eq!(none.entries, Vec::new());
    assert!(!none.truncated);
}

fn git_dir_listing(git_dir: &Path) -> BTreeMap<PathBuf, Option<(u64, std::time::SystemTime)>> {
    let mut listing = BTreeMap::new();
    let mut pending = vec![git_dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("read_dir") {
            let path = entry.expect("entry").path();
            let metadata = std::fs::symlink_metadata(&path).expect("metadata");
            if metadata.is_dir() {
                pending.push(path.clone());
                listing.insert(path, None);
            } else {
                listing.insert(
                    path,
                    Some((metadata.len(), metadata.modified().expect("mtime"))),
                );
            }
        }
    }
    listing
}

#[test]
fn a_snapshot_leaves_the_index_and_everything_under_git_untouched() {
    let (_tmp, root) = repo();
    committed_repo(
        &root,
        &[("stat-dirty.txt", "same content\n"), ("other.txt", "o\n")],
    );
    backdate(&root.join("stat-dirty.txt"));
    write(&root, "untracked.txt", b"u\n");
    let git_dir = root.join(".git");
    let index = git_dir.join("index");
    let index_bytes = std::fs::read(&index).expect("index");
    let before = git_dir_listing(&git_dir);
    for _ in 0..3 {
        snapshot(&root, &Options::default()).expect("snapshot");
    }
    assert_eq!(std::fs::read(&index).expect("index"), index_bytes);
    assert_eq!(git_dir_listing(&git_dir), before);
}

#[test]
fn no_repository_outlives_a_snapshot() {
    let (tmp, root) = repo();
    committed_repo(&root, &[("file.txt", "f\n")]);
    write(&root, "file.txt", b"changed\n");
    snapshot(&root, &Options::default()).expect("snapshot");
    std::fs::remove_dir_all(&root).expect("the repository can be deleted right after a snapshot");
    drop(tmp);
}

#[test]
fn a_path_outside_any_repository_is_a_distinct_error() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let error = snapshot(tmp.path(), &Options::default()).expect_err("no repository");
    assert!(matches!(error, Error::NotARepository { .. }), "{error:?}");
}

#[test]
fn a_bare_repository_is_a_distinct_error() {
    let (_tmp, root) = repo();
    std::fs::create_dir_all(&root).expect("dir");
    git(&root, &["init", "-q", "--bare"]);
    let error = snapshot(&root, &Options::default()).expect_err("bare");
    assert!(matches!(error, Error::Bare { .. }), "{error:?}");
}

#[test]
fn a_raised_interrupt_returns_an_interrupted_error_instead_of_a_partial_status() {
    let (_tmp, root) = repo();
    committed_repo(&root, &[("file.txt", "f\n")]);
    for index in 0..50 {
        write(&root, &format!("untracked/{index}.txt"), b"u\n");
    }
    let interrupt = Arc::new(AtomicBool::new(true));
    let error = snapshot(
        &root,
        &Options {
            interrupt: Some(Arc::clone(&interrupt)),
            ..Options::default()
        },
    )
    .expect_err("interrupted");
    assert!(matches!(error, Error::Interrupted), "{error:?}");
}

fn add_submodule(root: &Path, sub_source: &Path, path: &str) {
    git(
        root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &support::forward_slashes(sub_source),
            path,
        ],
    );
    commit_all(root, "add submodule");
}

#[test]
fn a_submodule_reports_new_commits_modified_and_untracked_content() {
    let (tmp, root) = repo();
    let source = tmp.path().join("sub-source");
    committed_repo(&source, &[("inner.txt", "inner\n")]);
    committed_repo(&root, &[("top.txt", "top\n")]);
    add_submodule(&root, &source, "sub");
    let sub = root.join("sub");
    assert_eq!(entries_by_path(&root, &Options::default()), BTreeMap::new());

    write(&sub, "inner.txt", b"inner edited\n");
    write(&sub, "loose.txt", b"loose\n");
    let entries = without_paths(entries_by_path(&root, &Options::default()));
    assert_eq!(
        entries,
        vec![(
            "sub".into(),
            tracked(
                None,
                Some(WorktreeChange::Submodule(SubmoduleChange {
                    commit_changed: false,
                    modified_content: true,
                    untracked_content: true,
                }))
            )
        )]
    );

    git(&sub, &["config", "core.autocrlf", "false"]);
    commit_all(&sub, "new commit");
    let entries = without_paths(entries_by_path(&root, &Options::default()));
    assert_eq!(
        entries,
        vec![(
            "sub".into(),
            tracked(
                None,
                Some(WorktreeChange::Submodule(SubmoduleChange {
                    commit_changed: true,
                    modified_content: false,
                    untracked_content: false,
                }))
            )
        )]
    );

    let ignored = snapshot(
        &root,
        &Options {
            submodules: Submodules::Ignore,
            ..Options::default()
        },
    )
    .expect("snapshot");
    assert_eq!(ignored.entries, Vec::new());
}

#[test]
fn a_submodule_runs_no_filter_driver_declared_in_its_own_config() {
    let (tmp, root) = repo();
    let tools = tmp.path().join("tools");
    std::fs::create_dir_all(&tools).expect("tools");
    let marker = tmp.path().join("marker-ran");
    let program = marker_program(&tools, &marker);
    let source = tmp.path().join("sub-source");
    committed_repo(&source, &[("inner.txt", "inner\n")]);
    committed_repo(&root, &[("top.txt", "top\n")]);
    add_submodule(&root, &source, "sub");
    let sub = root.join("sub");
    git(&sub, &["config", "filter.marker.clean", &program]);
    git(&sub, &["config", "filter.marker.process", &program]);
    git(&sub, &["config", "filter.marker.required", "true"]);
    let sub_git_dir = git(&sub, &["rev-parse", "--absolute-git-dir"]);
    write(
        Path::new(sub_git_dir.trim()),
        "info/attributes",
        b"* filter=marker\n",
    );
    backdate(&sub.join("inner.txt"));

    let result = snapshot(&root, &Options::default());
    assert!(!marker.exists(), "a submodule-configured program ran");
    assert_eq!(result.expect("snapshot").entries, Vec::new());
}

#[test]
fn a_constant_size_truncating_writer_never_fails_and_no_snapshot_fails() {
    let (_tmp, root) = repo();
    init(&root);
    let first = vec![b'a'; RACE_FILE_BYTES];
    write(&root, RACE_FILE, &first);
    commit_all(&root, "init");
    let path = root.join(RACE_FILE);
    let stop = Arc::new(AtomicBool::new(false));
    let completed_writes = Arc::new(AtomicUsize::new(0));
    let writer = {
        let stop = Arc::clone(&stop);
        let completed_writes = Arc::clone(&completed_writes);
        let path = path.clone();
        std::thread::spawn(move || {
            let contents = [vec![b'b'; RACE_FILE_BYTES], vec![b'c'; RACE_FILE_BYTES]];
            let mut writes = 0usize;
            let mut failures = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                match std::fs::write(&path, &contents[writes % 2]) {
                    Ok(()) => {}
                    Err(error) => failures.push(error.raw_os_error()),
                }
                writes += 1;
                completed_writes.store(writes, Ordering::Relaxed);
            }
            (writes, failures)
        })
    };
    let mut snapshots = 0usize;
    let mut probe_errors = Vec::new();
    while snapshots < RACE_SNAPSHOTS || completed_writes.load(Ordering::Relaxed) < RACE_SNAPSHOTS {
        if let Err(error) = snapshot(&root, &Options::default()) {
            probe_errors.push(error.to_string());
        }
        snapshots += 1;
    }
    stop.store(true, Ordering::Relaxed);
    let (writes, write_failures) = writer.join().expect("writer");
    assert!(writes >= RACE_SNAPSHOTS && snapshots >= RACE_SNAPSHOTS);
    assert_eq!(write_failures, Vec::new(), "failed writes out of {writes}");
    assert_eq!(probe_errors, Vec::<String>::new(), "failed snapshots");
}
