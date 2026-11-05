use std::path::Path;

use super::{Built, Fixture};
use crate::support::{backdate, committed_repo, forward_slashes, git, write};

pub const SKIP_HASH_TRAILER: [u8; 20] = [0; 20];

pub fn matrix() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "sparse_cone",
            build: sparse_cone,
            must_include: &[(" M", "inside/edited.txt"), ("??", "outside/new.txt")],
        },
        Fixture {
            name: "sparse_cone_sparse_index",
            build: sparse_cone_sparse_index,
            must_include: &[(" M", "inside/edited.txt"), ("??", "outside/new.txt")],
        },
        Fixture {
            name: "sparse_expecting_files_outside",
            build: sparse_expecting_files_outside,
            must_include: &[(" M", "inside/edited.txt"), ("??", "outside/new.txt")],
        },
        Fixture {
            name: "sparse_non_cone",
            build: sparse_non_cone,
            must_include: &[(" M", "inside/edited.txt"), ("??", "outside/new.txt")],
        },
        Fixture {
            name: "split_index",
            build: split_index,
            must_include: CHANGES,
        },
        Fixture {
            name: "untracked_cache",
            build: untracked_cache,
            must_include: CHANGES,
        },
        Fixture {
            name: "fsmonitor_extension",
            build: fsmonitor_extension,
            must_include: CHANGES,
        },
        Fixture {
            name: "skip_hash",
            build: skip_hash,
            must_include: CHANGES,
        },
        Fixture {
            name: "index_version_4",
            build: index_version_4,
            must_include: CHANGES,
        },
    ]
}

const CHANGES: &[(&str, &str)] = &[
    ("M ", "staged.txt"),
    (" M", "edited.txt"),
    (" D", "removed.txt"),
    ("A ", "dir/added.txt"),
    ("??", "dir/loose.txt"),
];

const SPARSE_FILES: &[(&str, &str)] = &[
    ("top.txt", "top\n"),
    ("inside/edited.txt", "inside\n"),
    ("inside/kept.txt", "inside\n"),
    ("outside/hidden.txt", "outside\n"),
    ("outside/deeper/hidden.txt", "outside\n"),
];

fn sparse(root: &Path, init: &[&str], set: &[&str]) -> Built {
    committed_repo(root, SPARSE_FILES);
    git(root, init);
    git(root, set);
    assert!(
        !root.join("outside/hidden.txt").exists(),
        "the sparse checkout removed the files outside its patterns"
    );
    write(root, "inside/edited.txt", b"inside edited\n");
    write(root, "outside/new.txt", b"new outside\n");
    write(
        root,
        "outside/hidden.txt",
        b"outside reappeared and edited\n",
    );
    Ok(root.to_path_buf())
}

fn sparse_cone(root: &Path) -> Built {
    sparse(
        root,
        &["sparse-checkout", "init", "--cone"],
        &["sparse-checkout", "set", "inside"],
    )
}

fn sparse_cone_sparse_index(root: &Path) -> Built {
    sparse(
        root,
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
        &["sparse-checkout", "set", "inside"],
    )
}

fn sparse_expecting_files_outside(root: &Path) -> Built {
    let built = sparse(
        root,
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
        &["sparse-checkout", "set", "inside"],
    )?;
    git(
        root,
        &["config", "sparse.expectFilesOutsideOfPatterns", "true"],
    );
    Ok(built)
}

fn sparse_non_cone(root: &Path) -> Built {
    sparse(
        root,
        &["sparse-checkout", "init", "--no-cone"],
        &["sparse-checkout", "set", "--no-cone", "/*.txt", "/inside/"],
    )
}

fn base_changes(root: &Path) {
    committed_repo(
        root,
        &[
            ("staged.txt", "before\n"),
            ("edited.txt", "before\n"),
            ("removed.txt", "here\n"),
            ("untouched.txt", "same\n"),
            ("dir/kept.txt", "kept\n"),
        ],
    );
}

fn apply_changes(root: &Path) {
    write(root, "staged.txt", b"staged\n");
    git(root, &["add", "staged.txt"]);
    write(root, "dir/added.txt", b"added\n");
    git(root, &["add", "dir/added.txt"]);
    write(root, "edited.txt", b"edited\n");
    std::fs::remove_file(root.join("removed.txt")).expect("remove");
    write(root, "dir/loose.txt", b"loose\n");
    backdate(&root.join("untouched.txt"));
}

fn split_index(root: &Path) -> Built {
    base_changes(root);
    git(root, &["update-index", "--split-index"]);
    apply_changes(root);
    let git_dir = root.join(".git");
    let shared = std::fs::read_dir(&git_dir)
        .expect("git dir")
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("sharedindex.")
        });
    assert!(shared, "the split index wrote a shared index");
    Ok(root.to_path_buf())
}

fn untracked_cache(root: &Path) -> Built {
    base_changes(root);
    git(root, &["config", "core.untrackedCache", "true"]);
    git(root, &["update-index", "--untracked-cache"]);
    write(root, "dir/cached.txt", b"cached\n");
    git(root, &["status", "--porcelain"]);
    std::fs::remove_file(root.join("dir/cached.txt")).expect("remove");
    apply_changes(root);
    git(root, &["status", "--porcelain"]);
    assert!(
        index_has_extension(root, b"UNTR"),
        "the index carries an untracked cache"
    );
    Ok(root.to_path_buf())
}

fn fsmonitor_extension(root: &Path) -> Built {
    base_changes(root);
    let hook = root
        .parent()
        .expect("fixture parent")
        .join("fsmonitor-hook.sh");
    std::fs::write(&hook, "#!/bin/sh\nprintf 'token-1\\0/\\0'\n").expect("fsmonitor hook");
    let fsmonitor = format!("core.fsmonitor=sh '{}'", forward_slashes(&hook));
    git(root, &["-c", &fsmonitor, "update-index", "--fsmonitor"]);
    apply_changes(root);
    git(root, &["-c", &fsmonitor, "status", "--porcelain"]);
    assert!(
        index_has_extension(root, b"FSMN"),
        "the index carries an fsmonitor extension"
    );
    Ok(root.to_path_buf())
}

fn skip_hash(root: &Path) -> Built {
    base_changes(root);
    git(root, &["config", "index.skipHash", "true"]);
    apply_changes(root);
    let index = std::fs::read(root.join(".git").join("index")).expect("index");
    assert_eq!(
        index[index.len() - SKIP_HASH_TRAILER.len()..],
        SKIP_HASH_TRAILER,
        "the index was written without a trailing hash"
    );
    Ok(root.to_path_buf())
}

fn index_version_4(root: &Path) -> Built {
    base_changes(root);
    git(root, &["update-index", "--index-version", "4"]);
    apply_changes(root);
    let index = std::fs::read(root.join(".git").join("index")).expect("index");
    assert_eq!(index[4..8], [0, 0, 0, 4], "the index is version 4");
    Ok(root.to_path_buf())
}

pub fn index_has_extension(root: &Path, signature: &[u8; 4]) -> bool {
    let index = std::fs::read(root.join(".git").join("index")).expect("index");
    index.windows(4).any(|window| window == signature)
}
