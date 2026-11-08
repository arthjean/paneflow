#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use paneflow_git::{snapshot, IndexChange, Options};
use support::{commit_all, committed_repo, forward_slashes, git, write};

const DIRECTORIES: &[&str] = &[
    "",
    "src",
    "src/app",
    "src/app/deep",
    "docs",
    "docs/guide",
    "assets",
];
const SCENARIOS: u64 = 24;
const OPERATIONS: usize = 7;

struct Rng(u64);

impl Rng {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % bound.max(1) as u64).unwrap_or(0)
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

fn seeded(root: &Path) {
    let mut files = Vec::new();
    for directory in DIRECTORIES {
        for file in 0..3 {
            let path = if directory.is_empty() {
                format!("f{file}.txt")
            } else {
                format!("{directory}/f{file}.txt")
            };
            files.push((path.clone(), format!("{path} initial\n")));
        }
    }
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    committed_repo(root, &files);
}

fn tracked(root: &Path) -> Vec<String> {
    git(root, &["ls-files", "-z"])
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .collect()
}

fn blob(root: &Path, scratch: &Path, content: &str) -> String {
    let path = scratch.join("blob.txt");
    std::fs::write(&path, content).expect("write a blob source");
    git(root, &["hash-object", "-w", &forward_slashes(&path)])
        .trim()
        .to_owned()
}

fn operate(root: &Path, scratch: &Path, rng: &mut Rng, step: usize) {
    let files = tracked(root);
    let pick = files[rng.below(files.len())].clone();
    match rng.below(11) {
        0 | 1 => {
            write(root, &pick, format!("{pick} staged {step}\n").as_bytes());
            git(root, &["add", "--", &pick]);
        }
        2 => {
            git(root, &["rm", "-q", "-f", "--", &pick]);
        }
        3 => {
            let directory = DIRECTORIES[rng.below(DIRECTORIES.len())];
            let path = if directory.is_empty() {
                format!("new{step}.txt")
            } else {
                format!("{directory}/new{step}.txt")
            };
            write(root, &path, b"new\n");
            git(root, &["add", "--", &path]);
        }
        4 => {
            let path = format!("fresh{step}/inner/leaf.txt");
            write(root, &path, b"leaf\n");
            git(root, &["add", "--", &path]);
        }
        5 => {
            git(root, &["rm", "-q", "-f", "--cached", "--", &pick]);
        }
        6 => {
            git(root, &["rm", "-q", "-f", "--", &pick]);
            let path = format!("{pick}/inside.txt");
            write(root, &path, b"inside\n");
            git(root, &["add", "--", &path]);
        }
        7 => {
            git(root, &["update-index", "--chmod=+x", "--", &pick]);
        }
        8 => {
            let target = blob(root, scratch, "elsewhere");
            git(
                root,
                &[
                    "update-index",
                    "--cacheinfo",
                    &format!("120000,{target},{pick}"),
                ],
            );
        }
        9 => {
            write(root, &pick, format!("{pick} unstaged {step}\n").as_bytes());
        }
        _ => {
            let directory = DIRECTORIES[1 + rng.below(DIRECTORIES.len() - 1)];
            if files
                .iter()
                .any(|file| file.starts_with(&format!("{directory}/")))
            {
                git(root, &["rm", "-q", "-r", "-f", "--", directory]);
            }
        }
    }
}

fn strip_cache_tree(root: &Path) {
    let repository = gix::open(root).expect("open the fixture");
    let mut index = repository.open_index().expect("read the index");
    index
        .write(gix::index::write::Options {
            extensions: gix::index::write::Extensions::Given {
                tree_cache: false,
                end_of_index_entry: false,
            },
            ..Default::default()
        })
        .expect("rewrite the index without its cache-tree");
    let reread = repository.open_index().expect("read the index again");
    assert!(reread.tree().is_none(), "the cache-tree is gone");
}

fn expected(root: &Path, base: Option<&str>) -> BTreeMap<String, IndexChange> {
    let mut args = vec!["diff", "--cached", "--no-renames", "--name-status", "-z"];
    args.extend(base);
    let output = git(root, &args);
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    let mut changes = BTreeMap::new();
    while let (Some(status), Some(path)) = (fields.next(), fields.next()) {
        let change = match status {
            "A" => IndexChange::Added,
            "M" => IndexChange::Modified,
            "D" => IndexChange::Deleted,
            "T" => IndexChange::TypeChange,
            other => panic!("unexpected staged status {other:?} for {path}"),
        };
        changes.insert(path.to_owned(), change);
    }
    changes
}

fn actual(root: &Path, base: Option<&str>) -> BTreeMap<String, IndexChange> {
    let options = Options {
        base: base.map(str::to_owned),
        ..Options::default()
    };
    snapshot(root, &options)
        .expect("snapshot")
        .entries
        .into_iter()
        .filter_map(|entry| {
            entry
                .head_to_index
                .map(|change| (String::from_utf8(entry.path).expect("utf-8"), change))
        })
        .collect()
}

#[test]
fn random_staged_changes_match_git_diff_cached_with_and_without_a_cache_tree() {
    support::isolate_process();
    for scenario in 0..SCENARIOS {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        seeded(&root);
        let mut rng =
            Rng(0x9e37_79b9_7f4a_7c15 ^ (scenario + 1).wrapping_mul(0x2545_f491_4f6c_dd1d));
        let committed_midway = rng.chance(40);
        for step in 0..OPERATIONS {
            if committed_midway && step == OPERATIONS / 2 {
                commit_all(&root, "midway");
            }
            operate(&root, tmp.path(), &mut rng, step);
        }
        if rng.chance(30) {
            strip_cache_tree(&root);
        }
        let base_owned = (committed_midway && rng.chance(50))
            .then(|| git(&root, &["rev-parse", "HEAD~1"]).trim().to_owned());
        let base = base_owned.as_deref();

        assert_eq!(
            actual(&root, base),
            expected(&root, base),
            "scenario {scenario} (base {base:?}) staged {:?}",
            git(&root, &["status", "--porcelain=v1", "-uno"])
        );
    }
}

#[test]
fn one_staged_file_deep_in_the_tree_is_found_through_its_invalidated_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    seeded(&root);
    write(&root, "src/app/deep/f1.txt", b"changed\n");
    git(&root, &["add", "--", "src/app/deep/f1.txt"]);
    let repository = gix::open(&root).expect("open the fixture");
    let index = repository.open_index().expect("read the index");
    let root_tree = index.tree().expect("git keeps a cache-tree");
    assert!(
        root_tree.num_entries.is_none(),
        "git add invalidates the path"
    );
    let docs = root_tree
        .children
        .iter()
        .find(|child| child.name.as_slice() == b"docs")
        .expect("the docs subtree");
    assert!(
        docs.num_entries.is_some(),
        "an untouched subtree stays valid"
    );

    assert_eq!(
        actual(&root, None),
        BTreeMap::from([("src/app/deep/f1.txt".to_owned(), IndexChange::Modified)])
    );
}
