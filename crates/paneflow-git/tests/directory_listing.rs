#![cfg(windows)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::path::Path;

use gix::bstr::BString;
use gix::status::UntrackedFiles;
use support::{committed_repo, write};

const TRACKED: &[(&str, &str)] = &[
    ("README.md", "readme\n"),
    ("docs/guide.md", "guide\n"),
    ("src/lib.rs", "lib\n"),
    ("src/app/row.rs", "row\n"),
    ("src/app/deep/cell.rs", "cell\n"),
];

struct Reads {
    untracked: Vec<String>,
    read_dir_calls: u32,
    symlink_metadata_calls: usize,
}

fn status_reads(root: &Path) -> Reads {
    let repo = gix::open(root).expect("open the fixture");
    let dirwalk = repo.dirwalk_options().expect("dirwalk options");
    let mut items = repo
        .status(gix::progress::Discard)
        .expect("prepare the status")
        .index_worktree_options_mut(|worktree| {
            worktree.dirwalk_options = Some(dirwalk);
            worktree.thread_limit = Some(2);
        })
        .untracked_files(UntrackedFiles::Files)
        .into_index_worktree_iter(Vec::<BString>::new())
        .expect("start the status");
    let mut untracked = Vec::new();
    for item in items.by_ref() {
        let item = item.expect("status item");
        if let gix::status::index_worktree::Item::DirectoryContents { entry, .. } = item {
            untracked.push(entry.rela_path.to_string());
        }
    }
    untracked.sort();
    let outcome = items
        .into_outcome()
        .expect("a finished status")
        .index_worktree;
    Reads {
        untracked,
        read_dir_calls: outcome
            .dirwalk
            .expect("the directory walk ran")
            .read_dir_calls,
        symlink_metadata_calls: outcome.tracked_file_modification.symlink_metadata_calls,
    }
}

#[test]
fn a_windows_status_reads_each_tracked_directory_once_for_both_of_its_passes() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root, TRACKED);
    write(&root, "src/app/new.rs", b"new\n");
    write(&root, "notes/todo.md", b"todo\n");
    write(&root, "notes/later/idea.md", b"idea\n");

    let reads = status_reads(&root);

    assert_eq!(
        reads.untracked,
        ["notes/later/idea.md", "notes/todo.md", "src/app/new.rs"]
    );
    assert_eq!(
        reads.read_dir_calls, 2,
        "the walk reads only notes and notes/later, the directories the index does not hold"
    );
    assert_eq!(
        reads.symlink_metadata_calls, 0,
        "every tracked file is answered from the shared listing"
    );
}

#[test]
fn a_windows_status_sees_a_file_deleted_from_a_listed_directory() {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    committed_repo(&root, TRACKED);
    std::fs::remove_file(root.join("src/app/row.rs")).expect("delete a tracked file");

    let reads = status_reads(&root);

    assert!(reads.untracked.is_empty(), "{:?}", reads.untracked);
    assert_eq!(reads.read_dir_calls, 0);
    assert_eq!(
        reads.symlink_metadata_calls, 1,
        "a name the listing lacks falls back to one live lstat, which finds it gone"
    );
}
