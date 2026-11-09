#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

const FIXTURE_CONFIG: &[&str] = &[
    "commit.gpgsign=false",
    "tag.gpgsign=false",
    "init.defaultBranch=main",
    "maintenance.auto=false",
    "gc.auto=0",
];

const FIXTURE_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Paneflow"),
    ("GIT_AUTHOR_EMAIL", "paneflow@example.com"),
    ("GIT_COMMITTER_NAME", "Paneflow"),
    ("GIT_COMMITTER_EMAIL", "paneflow@example.com"),
];

const INHERITED_REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
];

static GLOBAL_CONFIG: OnceLock<PathBuf> = OnceLock::new();

const DEFAULT_HASH_ENV: &str = "GIT_DEFAULT_HASH";

pub const SHA256: &str = "sha256";

thread_local! {
    static OBJECT_FORMAT: Cell<Option<&'static str>> = const { Cell::new(None) };
}

pub fn with_object_format<T>(format: &'static str, build: impl FnOnce() -> T) -> T {
    let previous = OBJECT_FORMAT.replace(Some(format));
    let built = build();
    OBJECT_FORMAT.set(previous);
    built
}

pub fn isolate_process() -> &'static Path {
    isolate_process_with("")
}

pub fn isolate_process_with(global_config: &str) -> &'static Path {
    GLOBAL_CONFIG.get_or_init(|| {
        let dir = tempfile::Builder::new()
            .prefix("paneflow-git-global")
            .tempdir()
            .expect("global config dir")
            .keep();
        let path = dir.join("gitconfig");
        std::fs::write(&path, global_config).expect("global config");
        std::env::set_var("GIT_CONFIG_GLOBAL", &path);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        for key in INHERITED_REPOSITORY_ENV {
            std::env::remove_var(key);
        }
        path
    })
}

pub fn command(cwd: &Path, args: &[&str]) -> Command {
    let global = isolate_process();
    let mut command = Command::new("git");
    for setting in FIXTURE_CONFIG {
        command.arg("-c").arg(setting);
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .envs(FIXTURE_IDENTITY.iter().copied());
    for key in INHERITED_REPOSITORY_ENV {
        command.env_remove(key);
    }
    match OBJECT_FORMAT.get() {
        Some(format) => command.env(DEFAULT_HASH_ENV, format),
        None => command.env_remove(DEFAULT_HASH_ENV),
    };
    command
}

pub fn output(cwd: &Path, args: &[&str]) -> Output {
    command(cwd, args)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?} in {}: {error}", cwd.display()))
}

pub fn git(cwd: &Path, args: &[&str]) -> String {
    let out = output(cwd, args);
    assert!(
        out.status.success(),
        "git {args:?} in {} exited with {}: {}",
        cwd.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn init(root: &Path) {
    std::fs::create_dir_all(root).expect("repository dir");
    git(root, &["init", "-q"]);
    git(root, &["config", "core.autocrlf", "false"]);
}

pub fn write(root: &Path, name: &str, content: &[u8]) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent dir");
    }
    std::fs::write(&path, content).expect("write file");
}

pub fn commit_all(root: &Path, message: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "--no-verify", "-m", message]);
}

pub fn committed_repo(root: &Path, files: &[(&str, &str)]) {
    init(root);
    for (name, content) in files {
        write(root, name, content.as_bytes());
    }
    commit_all(root, "init");
}

pub fn backdate(path: &Path) {
    let earlier = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(earlier))
        .unwrap_or_else(|error| panic!("backdate {}: {error}", path.display()));
}

pub fn forward_slashes(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

pub fn marker_program(dir: &Path, marker: &Path) -> String {
    let script = dir.join("marker.sh");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho ran >> '{}'\ncat\n",
            forward_slashes(marker)
        ),
    )
    .expect("marker script");
    format!("sh '{}'", forward_slashes(&script))
}

pub fn porcelain(cwd: &Path) -> String {
    let args = [
        "-c",
        "core.fsmonitor=false",
        "status",
        "--porcelain=v1",
        "--untracked-files=all",
        "--no-renames",
    ];
    let out = command(cwd, &args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .unwrap_or_else(|error| panic!("git status in {}: {error}", cwd.display()));
    assert!(
        out.status.success(),
        "git status in {} exited with {}: {}",
        cwd.display(),
        out.status,
        String::from_utf8_lossy(&out.stderr).trim()
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}
