use std::path::{Path, PathBuf};

use super::{Built, Fixture};
use crate::support::{committed_repo, forward_slashes, git, write};

pub fn matrix() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "git_worktree_add",
            build: git_worktree_add,
            must_include: &[
                (" M", "shared.txt"),
                ("A ", "staged_in_linked.txt"),
                ("??", "loose_in_linked.txt"),
            ],
        },
        Fixture {
            name: "paneflow_detached_checkout",
            build: paneflow_detached_checkout,
            must_include: &[
                (" D", "shared.txt"),
                ("M ", "other.txt"),
                ("??", "loose_in_linked.txt"),
            ],
        },
    ]
}

fn main_repository(root: &Path) {
    committed_repo(
        root,
        &[
            ("shared.txt", "shared\n"),
            ("other.txt", "other\n"),
            (".gitignore", ".env\n"),
        ],
    );
    write(root, "shared.txt", b"edited in the main worktree\n");
    write(root, "staged_in_main.txt", b"main\n");
    git(root, &["add", "staged_in_main.txt"]);
    write(root, "loose_in_main.txt", b"main\n");
    write(root, ".env", b"SECRET=main\n");
}

fn linked_path(root: &Path) -> PathBuf {
    root.parent().expect("fixture parent").join("linked")
}

fn git_worktree_add(root: &Path) -> Built {
    main_repository(root);
    let linked = linked_path(root);
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            &forward_slashes(&linked),
        ],
    );
    write(&linked, "shared.txt", b"edited in the linked worktree\n");
    write(&linked, "staged_in_linked.txt", b"linked\n");
    git(&linked, &["add", "staged_in_linked.txt"]);
    write(&linked, "loose_in_linked.txt", b"linked\n");
    Ok(linked)
}

fn paneflow_detached_checkout(root: &Path) -> Built {
    main_repository(root);
    write(root, ".worktreeinclude", b".env\n");
    let head = git(root, &["rev-parse", "HEAD"]).trim().to_string();
    let linked = linked_path(root);
    git(
        root,
        &[
            "worktree",
            "add",
            "--detach",
            &forward_slashes(&linked),
            &head,
        ],
    );
    let admin = git(&linked, &["rev-parse", "--absolute-git-dir"])
        .trim()
        .to_string();
    std::fs::write(
        Path::new(&admin).join("paneflow-owner"),
        format!(
            "owner=paneflow\nrepo_root={}\nbranch=HEAD-detached\n",
            root.display()
        ),
    )
    .expect("owner marker");
    std::fs::copy(root.join(".env"), linked.join(".env")).expect("copy include file");
    std::fs::remove_file(linked.join("shared.txt")).expect("remove");
    write(&linked, "other.txt", b"staged in the linked worktree\n");
    git(&linked, &["add", "other.txt"]);
    write(&linked, "loose_in_linked.txt", b"linked\n");
    Ok(linked)
}
