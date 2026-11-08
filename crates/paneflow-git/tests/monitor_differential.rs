#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod monitor_support;
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use monitor_support::{canonical, monitor, settle};
use paneflow_git::{Options, ProbeKind, Submodules, Untracked};
use support::{commit_all, committed_repo, git, write};

const STEPS: usize = 12;

const OPERATIONS: &[Operation] = &[
    Operation::Edit,
    Operation::SameSizeRewrite,
    Operation::Delete,
    Operation::CreateFile,
    Operation::CreateNestedDirectory,
    Operation::RenameFile,
    Operation::RenameDirectory,
    Operation::CaseOnlyRename,
    Operation::Chmod,
    Operation::SymlinkSwap,
    Operation::GitignoreEdit,
    Operation::GitAdd,
    Operation::GitRmCached,
    Operation::GitCommit,
    Operation::GitCheckout,
    Operation::SubmoduleEdit,
    Operation::IgnoredWrite,
];

#[derive(Clone, Copy, Debug)]
enum Operation {
    Edit,
    SameSizeRewrite,
    Delete,
    CreateFile,
    CreateNestedDirectory,
    RenameFile,
    RenameDirectory,
    CaseOnlyRename,
    Chmod,
    SymlinkSwap,
    GitignoreEdit,
    GitAdd,
    GitRmCached,
    GitCommit,
    GitCheckout,
    SubmoduleEdit,
    IgnoredWrite,
}

struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        (!items.is_empty()).then(|| &items[self.below(items.len())])
    }
}

struct Scenario {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    cache: PathBuf,
    sub: PathBuf,
    base: String,
    counter: usize,
}

fn scenario() -> Scenario {
    support::isolate_process();
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("repo");
    let mut files: Vec<(String, String)> = Vec::new();
    for directory in ["src", "src/app", "docs", "lib/core", "lib/util"] {
        for index in 0..8 {
            files.push((
                format!("{directory}/file{index}.txt"),
                format!("{directory} {index}\n"),
            ));
        }
    }
    files.push((".gitignore".into(), "target/\nbuild/\n*.log\n".into()));
    files.push(("Readme.md".into(), "readme\n".into()));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, content)| (name.as_str(), content.as_str()))
        .collect();
    committed_repo(&root, &borrowed);
    write(&root, "build/keep.txt", b"keep\n");
    git(&root, &["add", "-f", "build/keep.txt"]);
    git(&root, &["commit", "-q", "--no-verify", "-m", "keep"]);
    let source = tmp.path().join("sub-source");
    committed_repo(&source, &[("inner.txt", "inner\n")]);
    git(
        &root,
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
    commit_all(&root, "submodule");
    git(&root, &["branch", "other"]);
    git(&root, &["checkout", "-q", "other"]);
    write(&root, "src/file0.txt", b"other branch\n");
    write(&root, "other/only.txt", b"only on other\n");
    std::fs::remove_file(root.join("docs/file1.txt")).expect("remove on other");
    commit_all(&root, "other");
    git(&root, &["checkout", "-q", "main"]);
    let base = git(&root, &["rev-parse", "HEAD~1^{tree}"])
        .trim()
        .to_string();
    let cache = tmp.path().join("cache");
    let root = canonical(&root);
    let sub = root.join("sub");
    Scenario {
        _tmp: tmp,
        root,
        cache,
        sub,
        base,
        counter: 0,
    }
}

fn tracked_files(root: &Path) -> Vec<String> {
    git(root, &["ls-files", "-z"])
        .split('\0')
        .filter(|path| !path.is_empty() && *path != "sub" && *path != ".gitmodules")
        .filter(|path| root.join(path).is_file())
        .map(str::to_string)
        .collect()
}

fn untracked_files(root: &Path) -> Vec<String> {
    git(root, &["ls-files", "-z", "--others", "--exclude-standard"])
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect()
}

fn directories(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for directory in [
        "src", "src/app", "docs", "lib", "lib/core", "lib/util", "made",
    ] {
        if root.join(directory).is_dir() {
            found.push(directory.to_string());
        }
    }
    found
}

impl Scenario {
    fn fresh(&mut self, stem: &str) -> String {
        self.counter += 1;
        format!("{stem}{}", self.counter)
    }

    fn apply(&mut self, operation: Operation, random: &mut Random) -> String {
        let root = self.root.clone();
        let tracked = tracked_files(&root);
        match operation {
            Operation::Edit => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "edit: nothing tracked".into();
                };
                let mut content = std::fs::read(root.join(&path)).unwrap_or_default();
                content.extend_from_slice(b"edited\n");
                std::fs::write(root.join(&path), content).expect("edit");
                format!("edit {path}")
            }
            Operation::SameSizeRewrite => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "rewrite: nothing tracked".into();
                };
                git(&root, &["add", "-f", "--", &path]);
                let content = std::fs::read(root.join(&path)).unwrap_or_default();
                let rewritten: Vec<u8> = content
                    .iter()
                    .map(|byte| {
                        if byte.is_ascii_lowercase() {
                            byte.to_ascii_uppercase()
                        } else if byte.is_ascii_uppercase() {
                            byte.to_ascii_lowercase()
                        } else {
                            *byte
                        }
                    })
                    .collect();
                std::fs::write(root.join(&path), rewritten).expect("same-size rewrite");
                format!("same-size rewrite of {path} right after git add")
            }
            Operation::Delete => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "delete: nothing tracked".into();
                };
                std::fs::remove_file(root.join(&path)).expect("delete");
                format!("delete {path}")
            }
            Operation::CreateFile => {
                let directories = directories(&root);
                let directory = random.pick(&directories).cloned().unwrap_or_default();
                let name = self.fresh("new");
                let path = if directory.is_empty() {
                    format!("{name}.txt")
                } else {
                    format!("{directory}/{name}.txt")
                };
                write(&root, &path, b"created\n");
                format!("create {path}")
            }
            Operation::CreateNestedDirectory => {
                let name = self.fresh("tree");
                let path = format!("made/{name}/b/c/f.txt");
                write(&root, &path, b"nested\n");
                format!("create nested {path}")
            }
            Operation::RenameFile => {
                let mut candidates = tracked.clone();
                candidates.extend(untracked_files(&root));
                let Some(path) = random.pick(&candidates).cloned() else {
                    return "rename: nothing".into();
                };
                let target = format!("{}.moved", self.fresh(&path));
                std::fs::rename(root.join(&path), root.join(&target)).expect("rename file");
                format!("rename {path} to {target}")
            }
            Operation::RenameDirectory => {
                let directories = directories(&root);
                let Some(directory) = random.pick(&directories).cloned() else {
                    return "rename directory: none".into();
                };
                let target = self.fresh("renamed");
                std::fs::rename(root.join(&directory), root.join(&target))
                    .expect("rename directory");
                format!("rename directory {directory} to {target}")
            }
            Operation::CaseOnlyRename => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "case rename: nothing".into();
                };
                let (parent, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
                let flipped: String = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_lowercase() {
                            c.to_ascii_uppercase()
                        } else {
                            c.to_ascii_lowercase()
                        }
                    })
                    .collect();
                let target = if parent.is_empty() {
                    flipped
                } else {
                    format!("{parent}/{flipped}")
                };
                std::fs::rename(root.join(&path), root.join(&target)).expect("case-only rename");
                format!("case-only rename {path} to {target}")
            }
            Operation::Chmod => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "chmod: nothing".into();
                };
                toggle_executable(&root.join(&path));
                format!("chmod {path}")
            }
            Operation::SymlinkSwap => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "symlink: nothing".into();
                };
                let full = root.join(&path);
                std::fs::remove_file(&full).expect("remove before symlink");
                if symlink("Readme.md", &full).is_err() {
                    std::fs::write(&full, b"no symlink privilege\n").expect("fallback write");
                    return format!("replace {path} by a file (no symlink privilege)");
                }
                format!("replace {path} by a symlink")
            }
            Operation::GitignoreEdit => {
                let pattern = if random.below(2) == 0 {
                    "*.moved\n"
                } else {
                    "made/\n"
                };
                let path = root.join(".gitignore");
                let mut content = std::fs::read(&path).unwrap_or_default();
                content.extend_from_slice(pattern.as_bytes());
                std::fs::write(&path, content).expect("gitignore edit");
                format!("append {} to .gitignore", pattern.trim())
            }
            Operation::GitAdd => {
                git(&root, &["add", "-A"]);
                "git add -A".into()
            }
            Operation::GitRmCached => {
                let Some(path) = random.pick(&tracked).cloned() else {
                    return "rm --cached: nothing".into();
                };
                git(&root, &["rm", "-q", "--cached", "--", &path]);
                format!("git rm --cached {path}")
            }
            Operation::GitCommit => {
                git(
                    &root,
                    &["commit", "-q", "--allow-empty", "--no-verify", "-m", "step"],
                );
                "git commit".into()
            }
            Operation::GitCheckout => {
                let current = git(&root, &["branch", "--show-current"]);
                let target = if current.trim() == "other" {
                    "main"
                } else {
                    "other"
                };
                git(&root, &["checkout", "-q", "-f", target]);
                format!("git checkout -f {target}")
            }
            Operation::SubmoduleEdit => {
                if !self.sub.join(".git").exists() {
                    return "submodule edit: no checkout".into();
                }
                if random.below(2) == 0 {
                    write(&self.sub, "inner.txt", b"inner edited\n");
                    "edit sub/inner.txt".into()
                } else {
                    let name = self.fresh("loose");
                    write(&self.sub, &format!("{name}.txt"), b"loose\n");
                    format!("create sub/{name}.txt")
                }
            }
            Operation::IgnoredWrite => {
                let name = self.fresh("artifact");
                let path = if random.below(2) == 0 {
                    format!("target/debug/{name}.o")
                } else {
                    format!("build/tmp/{name}.o")
                };
                write(&root, &path, b"ignored\n");
                format!("write ignored {path}")
            }
        }
    }
}

#[cfg(unix)]
fn toggle_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() ^ 0o111;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}

#[cfg(not(unix))]
fn toggle_executable(path: &Path) {
    let Ok(metadata) = std::fs::metadata(path) else {
        return;
    };
    let mut permissions = metadata.permissions();
    permissions.set_readonly(!permissions.readonly());
    std::fs::set_permissions(path, permissions).expect("toggle read-only");
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn symlink(target: &str, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

fn options_for(step: usize, base: &str) -> Options {
    match step % 5 {
        0 => Options::files_status(),
        1 => Options::dock_listing(),
        2 => Options::is_clean(),
        3 => Options {
            untracked: Untracked::None,
            submodules: Submodules::Ignore,
            ..Options::default()
        },
        _ => Options {
            base: Some(base.to_string()),
            ..Options::default()
        },
    }
}

fn run_seeds(seeds: std::ops::Range<u64>) {
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for seed in seeds.clone() {
        let mut scenario = scenario();
        let mut random = Random::new(seed);
        let mut monitor = monitor(&scenario.root, &scenario.cache);
        if let Err(differences) = settle(&mut monitor, &Options::default()) {
            panic!("seed {seed}, baseline: {differences}");
        }
        for step in 0..STEPS {
            let operation = *random.pick(OPERATIONS).expect("operations");
            let described = scenario.apply(operation, &mut random);
            let options = options_for(step, &scenario.base);
            match settle(&mut monitor, &options) {
                Ok(settled) => {
                    for report in &settled.reports {
                        let kind = match &report.kind {
                            ProbeKind::Baseline(reason) => format!("baseline {reason:?}"),
                            other => format!("{other:?}"),
                        };
                        *kinds.entry(kind).or_default() += 1;
                    }
                }
                Err(differences) => panic!(
                    "seed {seed}, step {step}, operation {operation:?} ({described}), options {options:?}, last probe {:?}:\n{differences}",
                    monitor.last_probe()
                ),
            }
        }
    }
    let incremental = kinds.get("Incremental").copied().unwrap_or_default();
    let total: usize = kinds.values().sum();
    eprintln!("seeds {seeds:?}: probe kinds {kinds:?}");
    assert!(
        incremental * 3 >= total,
        "at least a third of the probes take the incremental path, got {kinds:?}"
    );
}

#[test]
fn seeds_0_to_5_keep_the_monitor_equal_to_a_full_snapshot() {
    run_seeds(0..6);
}

#[test]
fn seeds_6_to_11_keep_the_monitor_equal_to_a_full_snapshot() {
    run_seeds(6..12);
}

#[test]
fn seeds_12_to_17_keep_the_monitor_equal_to_a_full_snapshot() {
    run_seeds(12..18);
}

#[test]
fn seeds_18_to_23_keep_the_monitor_equal_to_a_full_snapshot() {
    run_seeds(18..24);
}

#[test]
fn an_injected_overflow_rebaselines_and_still_equals_a_full_snapshot() {
    let mut scenario = scenario();
    let mut monitor = paneflow_git::Monitor::new(
        &scenario.root,
        paneflow_git::feed::FeedOptions {
            max_events: 16,
            ..monitor_support::feed_options(&scenario.cache)
        },
    );
    monitor_support::settled(&mut monitor, &Options::default());
    for index in 0..200 {
        write(&scenario.root, &format!("src/flood{index}.txt"), b"flood\n");
    }
    scenario.counter += 1;
    let settled = monitor_support::settled(&mut monitor, &Options::default());
    assert!(
        settled.reports.iter().any(|report| matches!(
            report.kind,
            paneflow_git::ProbeKind::Baseline(
                paneflow_git::Reason::Loss(_)
                    | paneflow_git::Reason::Fence(paneflow_git::feed::FenceError::CoverageLost)
            )
        )),
        "an overflow re-baselines, through the drain or through the fence it breaks: {:?}",
        settled.reports
    );
}

#[test]
fn a_replaced_root_rebaselines_and_still_equals_a_full_snapshot() {
    let scenario = scenario();
    let mut monitor = monitor(&scenario.root, &scenario.cache);
    monitor_support::settled(&mut monitor, &Options::default());
    let aside = scenario.root.with_file_name("repo-aside");
    std::fs::rename(&scenario.root, &aside).expect("move the root away");
    copy_tree(&aside, &scenario.root);
    write(&scenario.root, "src/after.txt", b"after\n");
    let settled = monitor_support::settled(&mut monitor, &Options::default());
    assert!(
        settled
            .reports
            .iter()
            .any(|report| matches!(report.kind, paneflow_git::ProbeKind::Baseline(_))),
        "a replaced root re-baselines: {:?}",
        settled.reports
    );
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("copy root");
    for entry in std::fs::read_dir(from).expect("read copy source") {
        let entry = entry.expect("copy entry");
        let target = to.join(entry.file_name());
        let file_type = entry.file_type().expect("copy type");
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target);
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

#[test]
fn a_racy_same_size_rewrite_gets_the_verdict_of_a_full_snapshot() {
    let scenario = scenario();
    let mut monitor = monitor(&scenario.root, &scenario.cache);
    monitor_support::settled(&mut monitor, &Options::default());
    for _ in 0..5 {
        write(&scenario.root, "src/file3.txt", b"src 3\n");
        git(&scenario.root, &["add", "src/file3.txt"]);
        write(&scenario.root, "src/file3.txt", b"SRC 3\n");
        monitor_support::settled(&mut monitor, &Options::default());
        write(&scenario.root, "src/file3.txt", b"src 3\n");
        monitor_support::settled(&mut monitor, &Options::default());
    }
}
