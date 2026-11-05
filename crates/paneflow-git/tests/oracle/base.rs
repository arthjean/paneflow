use std::path::Path;

use super::{git_with_stdin, hash_blob, symlink_file, Built, Fixture, Skip};
use crate::support::{backdate, commit_all, committed_repo, git, init, output, write};

pub const MAY_SKIP: &[&str] = &["type_change"];

pub fn matrix() -> Vec<Fixture> {
    vec![
        Fixture {
            name: "autocrlf_false",
            build: autocrlf_false,
            must_include: &[
                (" M", "lf_to_crlf.txt"),
                (" M", "crlf_to_lf.txt"),
                (" M", "normalized.txt"),
            ],
        },
        Fixture {
            name: "autocrlf_true",
            build: autocrlf_true,
            must_include: &[("A ", "staged_crlf.txt")],
        },
        Fixture {
            name: "autocrlf_input",
            build: autocrlf_input,
            must_include: &[("A ", "staged_crlf.txt")],
        },
        Fixture {
            name: "attributes",
            build: attributes,
            must_include: &[
                (" M", "edited.raw"),
                (" M", "edited.bin"),
                (" M", "late.text"),
            ],
        },
        Fixture {
            name: "staged_changes",
            build: staged_changes,
            must_include: &[
                ("A ", "added.txt"),
                ("M ", "modified.txt"),
                ("D ", "deleted.txt"),
                ("MM", "staged_then_modified.txt"),
                (" D", "worktree_deleted.txt"),
                ("D ", "rename_source.txt"),
                ("A ", "rename_target.txt"),
                ("D ", "uncached.txt"),
                ("??", "uncached.txt"),
            ],
        },
        Fixture {
            name: "untracked_and_ignored",
            build: untracked_and_ignored,
            must_include: &[
                ("??", "top.txt"),
                ("??", "nested/deeper/leaf.txt"),
                ("??", "nested/sibling.txt"),
                ("??", "inner-repo/"),
            ],
        },
        Fixture {
            name: "unborn_head",
            build: unborn_head,
            must_include: &[
                ("A ", "staged.txt"),
                ("AM", "staged_then_modified.txt"),
                (" A", "intent.txt"),
                ("??", "loose.txt"),
            ],
        },
        Fixture {
            name: "intent_to_add",
            build: intent_to_add,
            must_include: &[(" A", "intent.txt"), (" A", "intent_edited.txt")],
        },
        Fixture {
            name: "type_change",
            build: type_change,
            must_include: &[(" T", "worktree_link"), ("T ", "staged_link")],
        },
        Fixture {
            name: "conflict_kinds",
            build: conflict_kinds,
            must_include: &[
                ("DD", "both_deleted.txt"),
                ("AU", "added_by_us.txt"),
                ("UD", "deleted_by_them.txt"),
                ("UA", "added_by_them.txt"),
                ("DU", "deleted_by_us.txt"),
                ("AA", "both_added.txt"),
                ("UU", "both_modified.txt"),
            ],
        },
        Fixture {
            name: "merge_conflict",
            build: merge_conflict,
            must_include: &[("UU", "both.txt"), ("UD", "gone.txt")],
        },
    ]
}

const LINE_ENDING_FILES: &[(&str, &[u8])] = &[
    ("lf.txt", b"one\ntwo\n"),
    ("crlf.txt", b"one\r\ntwo\r\n"),
    ("mixed.txt", b"one\r\ntwo\n"),
    ("lf_to_crlf.txt", b"three\nfour\n"),
    ("crlf_to_lf.txt", b"three\r\nfour\r\n"),
];

fn line_endings(root: &Path, autocrlf: &str) -> Built {
    init(root);
    for (name, content) in LINE_ENDING_FILES {
        write(root, name, content);
    }
    commit_all(root, "line endings");
    write(root, "normalized.txt", b"five\r\nsix\r\n");
    git(root, &["-c", "core.autocrlf=true", "add", "normalized.txt"]);
    git(root, &["commit", "-q", "--no-verify", "-m", "normalized"]);
    git(root, &["config", "core.autocrlf", autocrlf]);
    write(root, "lf_to_crlf.txt", b"three\r\nfour\r\n");
    write(root, "crlf_to_lf.txt", b"three\nfour\n");
    for name in ["lf.txt", "crlf.txt", "mixed.txt", "normalized.txt"] {
        backdate(&root.join(name));
    }
    write(root, "staged_crlf.txt", b"staged\r\nlines\r\n");
    git(root, &["add", "staged_crlf.txt"]);
    write(root, "untracked_crlf.txt", b"loose\r\n");
    Ok(root.to_path_buf())
}

fn autocrlf_false(root: &Path) -> Built {
    line_endings(root, "false")
}

fn autocrlf_true(root: &Path) -> Built {
    line_endings(root, "true")
}

fn autocrlf_input(root: &Path) -> Built {
    line_endings(root, "input")
}

fn attributes(root: &Path) -> Built {
    init(root);
    write(root, "late.text", b"late\r\nlines\r\n");
    commit_all(root, "before attributes");
    write(
        root,
        ".gitattributes",
        b"*.crlf eol=crlf\n*.text text\n*.raw -text\n*.bin binary\n",
    );
    write(root, "kept.crlf", b"kept\nlines\n");
    write(root, "edited.crlf", b"edited\nlines\n");
    write(root, "kept.text", b"kept\nlines\n");
    write(root, "edited.text", b"edited\nlines\n");
    write(root, "kept.raw", b"kept\nlines\n");
    write(root, "edited.raw", b"edited\nlines\n");
    write(root, "kept.bin", b"\0\x01kept\r\n");
    write(root, "edited.bin", b"\0\x01edited\r\n");
    git(
        root,
        &[
            "add",
            ".gitattributes",
            "*.crlf",
            "*.raw",
            "*.bin",
            "kept.text",
            "edited.text",
        ],
    );
    git(root, &["commit", "-q", "--no-verify", "-m", "attributes"]);
    write(root, "edited.crlf", b"edited\r\nlines\r\n");
    write(root, "edited.text", b"edited\r\nlines\r\n");
    write(root, "edited.raw", b"edited\r\nlines\r\n");
    write(root, "edited.bin", b"\0\x01edited\n");
    for name in [
        "late.text",
        "kept.crlf",
        "kept.text",
        "kept.raw",
        "kept.bin",
    ] {
        backdate(&root.join(name));
    }
    Ok(root.to_path_buf())
}

fn staged_changes(root: &Path) -> Built {
    committed_repo(
        root,
        &[
            ("modified.txt", "before\n"),
            ("deleted.txt", "gone\n"),
            ("staged_then_modified.txt", "before\n"),
            ("worktree_deleted.txt", "here\n"),
            ("rename_source.txt", "moving content\n"),
            ("uncached.txt", "kept on disk\n"),
            ("untouched.txt", "same\n"),
        ],
    );
    write(root, "added.txt", b"new\n");
    git(root, &["add", "added.txt"]);
    write(root, "modified.txt", b"after\n");
    git(root, &["add", "modified.txt"]);
    git(root, &["rm", "-q", "deleted.txt"]);
    write(root, "staged_then_modified.txt", b"staged\n");
    git(root, &["add", "staged_then_modified.txt"]);
    write(root, "staged_then_modified.txt", b"staged then edited\n");
    std::fs::remove_file(root.join("worktree_deleted.txt")).expect("remove");
    git(root, &["mv", "rename_source.txt", "rename_target.txt"]);
    git(root, &["rm", "-q", "--cached", "uncached.txt"]);
    backdate(&root.join("untouched.txt"));
    Ok(root.to_path_buf())
}

fn untracked_and_ignored(root: &Path) -> Built {
    committed_repo(
        root,
        &[
            ("tracked.txt", "tracked\n"),
            (".gitignore", "*.log\nbuild/\n"),
        ],
    );
    write(root, "top.txt", b"top\n");
    write(root, "nested/deeper/leaf.txt", b"leaf\n");
    write(root, "nested/sibling.txt", b"sibling\n");
    write(root, "debug.log", b"ignored\n");
    write(root, "nested/trace.log", b"ignored\n");
    write(root, "build/out.o", b"ignored\n");
    std::fs::create_dir_all(root.join("empty_dir")).expect("empty dir");
    let inner = root.join("inner-repo");
    committed_repo(&inner, &[("inner.txt", "inner\n")]);
    Ok(root.to_path_buf())
}

fn unborn_head(root: &Path) -> Built {
    init(root);
    write(root, "staged.txt", b"staged\n");
    write(root, "staged_then_modified.txt", b"staged\n");
    git(root, &["add", "staged.txt", "staged_then_modified.txt"]);
    write(root, "staged_then_modified.txt", b"edited\n");
    write(root, "intent.txt", b"intent\n");
    git(root, &["add", "-N", "intent.txt"]);
    write(root, "loose.txt", b"loose\n");
    Ok(root.to_path_buf())
}

fn intent_to_add(root: &Path) -> Built {
    committed_repo(root, &[("base.txt", "base\n")]);
    write(root, "intent.txt", b"intent\n");
    write(root, "intent_edited.txt", b"intent\n");
    git(root, &["add", "-N", "intent.txt", "intent_edited.txt"]);
    write(root, "intent_edited.txt", b"edited after the intent\n");
    Ok(root.to_path_buf())
}

fn type_change(root: &Path) -> Built {
    committed_repo(
        root,
        &[
            ("target.txt", "target\n"),
            ("worktree_link", "plain file\n"),
            ("staged_link", "plain file\n"),
        ],
    );
    for name in ["worktree_link", "staged_link"] {
        let link = root.join(name);
        std::fs::remove_file(&link).expect("remove");
        if let Err(error) = symlink_file("target.txt", &link) {
            return Err(Skip(format!(
                "this platform cannot create a symbolic link ({error})"
            )));
        }
    }
    git(root, &["config", "core.symlinks", "true"]);
    git(root, &["add", "staged_link"]);
    Ok(root.to_path_buf())
}

const CONFLICT_STAGES: &[(&str, &[u8])] = &[
    ("both_deleted.txt", &[1]),
    ("added_by_us.txt", &[2]),
    ("deleted_by_them.txt", &[1, 2]),
    ("added_by_them.txt", &[3]),
    ("deleted_by_us.txt", &[1, 3]),
    ("both_added.txt", &[2, 3]),
    ("both_modified.txt", &[1, 2, 3]),
];

fn conflict_kinds(root: &Path) -> Built {
    committed_repo(root, &[("base.txt", "base\n")]);
    let blobs = [
        hash_blob(root, b"base\n"),
        hash_blob(root, b"ours\n"),
        hash_blob(root, b"theirs\n"),
    ];
    let mut index_info = String::new();
    for (path, stages) in CONFLICT_STAGES {
        for stage in *stages {
            let blob = &blobs[usize::from(*stage) - 1];
            index_info.push_str(&format!("100644 {blob} {stage}\t{path}\n"));
        }
        if stages.iter().any(|stage| *stage >= 2) {
            write(
                root,
                path,
                b"<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n",
            );
        }
    }
    git_with_stdin(
        root,
        &["update-index", "--index-info"],
        index_info.as_bytes(),
    );
    Ok(root.to_path_buf())
}

fn merge_conflict(root: &Path) -> Built {
    committed_repo(
        root,
        &[
            ("both.txt", "base\n"),
            ("gone.txt", "base\n"),
            ("calm.txt", "calm\n"),
        ],
    );
    git(root, &["switch", "-q", "-c", "side"]);
    write(root, "both.txt", b"side\n");
    git(root, &["rm", "-q", "gone.txt"]);
    commit_all(root, "side");
    git(root, &["switch", "-q", "main"]);
    write(root, "both.txt", b"main\n");
    write(root, "gone.txt", b"main\n");
    commit_all(root, "main");
    let merge = output(root, &["merge", "-q", "side"]);
    assert!(!merge.status.success(), "the merge conflicts");
    Ok(root.to_path_buf())
}
