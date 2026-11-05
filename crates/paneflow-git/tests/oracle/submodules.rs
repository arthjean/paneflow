use std::path::{Path, PathBuf};

use super::{Built, Fixture, Probe};
use crate::support::{commit_all, committed_repo, forward_slashes, git, write};

pub const MUST_FAIL: &[&str] = &["submodule_dangling_gitfile"];

pub fn matrix(probe: Probe) -> Vec<Fixture> {
    let clean = probe == Probe::Clean;
    vec![
        Fixture {
            name: "submodule_clean",
            build: submodule_clean,
            must_include: &[],
        },
        Fixture {
            name: "submodule_new_commit",
            build: submodule_new_commit,
            must_include: &[(" M", "sub")],
        },
        Fixture {
            name: "submodule_modified_content",
            build: submodule_modified_content,
            must_include: &[(" M", "sub")],
        },
        Fixture {
            name: "submodule_untracked_content",
            build: submodule_untracked_content,
            must_include: &[(" M", "sub")],
        },
        Fixture {
            name: "submodule_not_initialized",
            build: submodule_not_initialized,
            must_include: &[],
        },
        Fixture {
            name: "submodule_deinitialized",
            build: submodule_deinitialized,
            must_include: &[],
        },
        Fixture {
            name: "submodule_ignore_dirty",
            build: submodule_ignore_dirty,
            must_include: if clean {
                &[(" M", "moved"), (" M", "edited")]
            } else {
                &[(" M", "moved")]
            },
        },
        Fixture {
            name: "submodule_ignore_all",
            build: submodule_ignore_all,
            must_include: if clean {
                &[(" M", "moved"), (" M", "edited")]
            } else {
                &[]
            },
        },
        Fixture {
            name: "submodule_without_git_dir",
            build: submodule_without_git_dir,
            must_include: &[],
        },
        Fixture {
            name: "submodule_dangling_gitfile",
            build: submodule_dangling_gitfile,
            must_include: &[],
        },
        Fixture {
            name: "submodule_staged_and_changed",
            build: submodule_staged_and_changed,
            must_include: &[("MM", "sub"), ("A ", "added_sub")],
        },
    ]
}

fn source(root: &Path, name: &str) -> PathBuf {
    let source = root
        .parent()
        .expect("fixture parent")
        .join(format!("source-{name}"));
    if !source.exists() {
        committed_repo(&source, &[("inner.txt", "inner\n")]);
    }
    source
}

pub fn add_submodule(root: &Path, path: &str) -> PathBuf {
    let source = source(root, path);
    git(
        root,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            &forward_slashes(&source),
            path,
        ],
    );
    root.join(path)
}

pub fn superproject(root: &Path, submodules: &[&str]) -> Vec<PathBuf> {
    committed_repo(root, &[("top.txt", "top\n")]);
    let paths = submodules
        .iter()
        .map(|path| add_submodule(root, path))
        .collect();
    commit_all(root, "add submodules");
    paths
}

fn new_commit(sub: &Path) {
    write(sub, "inner.txt", b"inner moved\n");
    commit_all(sub, "move the submodule");
}

fn submodule_clean(root: &Path) -> Built {
    superproject(root, &["sub"]);
    Ok(root.to_path_buf())
}

fn submodule_new_commit(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    new_commit(&subs[0]);
    Ok(root.to_path_buf())
}

fn submodule_modified_content(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    write(&subs[0], "inner.txt", b"inner edited\n");
    Ok(root.to_path_buf())
}

fn submodule_untracked_content(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    write(&subs[0], "loose.txt", b"loose\n");
    Ok(root.to_path_buf())
}

fn submodule_not_initialized(root: &Path) -> Built {
    let origin = root.parent().expect("fixture parent").join("origin");
    superproject(&origin, &["sub"]);
    git(
        origin.parent().expect("fixture parent"),
        &[
            "clone",
            "-q",
            &forward_slashes(&origin),
            &forward_slashes(root),
        ],
    );
    Ok(root.to_path_buf())
}

fn submodule_deinitialized(root: &Path) -> Built {
    superproject(root, &["sub"]);
    git(root, &["submodule", "deinit", "-q", "-f", "sub"]);
    Ok(root.to_path_buf())
}

fn dirty_pair(root: &Path) {
    let subs = superproject(root, &["moved", "edited"]);
    new_commit(&subs[0]);
    write(&subs[1], "inner.txt", b"inner edited\n");
    write(&subs[1], "loose.txt", b"loose\n");
}

fn submodule_ignore_dirty(root: &Path) -> Built {
    dirty_pair(root);
    git(root, &["config", "submodule.moved.ignore", "dirty"]);
    git(root, &["config", "submodule.edited.ignore", "dirty"]);
    Ok(root.to_path_buf())
}

fn submodule_ignore_all(root: &Path) -> Built {
    dirty_pair(root);
    git(root, &["config", "submodule.moved.ignore", "all"]);
    git(root, &["config", "submodule.edited.ignore", "all"]);
    Ok(root.to_path_buf())
}

fn submodule_without_git_dir(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    write(&subs[0], "inner.txt", b"inner edited\n");
    std::fs::remove_file(subs[0].join(".git")).expect("remove the gitfile");
    Ok(root.to_path_buf())
}

fn submodule_dangling_gitfile(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    write(&subs[0], "inner.txt", b"inner edited\n");
    let modules = root.join(".git").join("modules");
    std::fs::rename(modules.join("sub"), modules.join("sub-gone")).expect("hide the git dir");
    Ok(root.to_path_buf())
}

fn submodule_staged_and_changed(root: &Path) -> Built {
    let subs = superproject(root, &["sub"]);
    new_commit(&subs[0]);
    git(root, &["add", "sub"]);
    write(&subs[0], "loose.txt", b"loose\n");
    add_submodule(root, "added_sub");
    Ok(root.to_path_buf())
}
