use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crate::git_command::{GitProfile, git};

const FIXTURE_DEADLINE: Duration = Duration::from_secs(60);

const FIXTURE_STDOUT_CAP: u64 = 16 * 1024 * 1024;

const FIXTURE_CONFIG: &[&str] = &[
    "commit.gpgsign=false",
    "tag.gpgsign=false",
    "init.defaultBranch=main",
];

const FIXTURE_IDENTITY: &[(&str, &str)] = &[
    ("GIT_AUTHOR_NAME", "Paneflow"),
    ("GIT_AUTHOR_EMAIL", "paneflow@example.com"),
    ("GIT_COMMITTER_NAME", "Paneflow"),
    ("GIT_COMMITTER_EMAIL", "paneflow@example.com"),
];

const EMPTY_GLOBAL_CONFIG_NAME: &str = "paneflow-tests-empty-gitconfig";

pub(crate) fn isolate(command: &mut Command) {
    command
        .env("GIT_CONFIG_GLOBAL", empty_global_config())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS");
}

pub(crate) fn command(cwd: &Path, args: &[&str]) -> Command {
    let mut fixture_args: Vec<&str> = Vec::with_capacity(FIXTURE_CONFIG.len() * 2 + args.len());
    for setting in FIXTURE_CONFIG {
        fixture_args.push("-c");
        fixture_args.push(setting);
    }
    fixture_args.extend_from_slice(args);
    let mut command = git(GitProfile::UserAction, fixture_args);
    isolate(&mut command);
    command
        .envs(FIXTURE_IDENTITY.iter().copied())
        .current_dir(cwd);
    command
}

pub(crate) fn output(cwd: &Path, args: &[&str]) -> paneflow_process::BoundedOutput {
    paneflow_process::run_with_timeout(command(cwd, args), FIXTURE_DEADLINE, FIXTURE_STDOUT_CAP)
        .unwrap_or_else(|error| panic!("git {args:?} in {} failed: {error}", cwd.display()))
}

pub(crate) fn git_succeeds(cwd: &Path, args: &[&str]) -> bool {
    output(cwd, args).status.success()
}

pub(crate) fn run(cwd: &Path, args: &[&str]) -> String {
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

pub(crate) fn init(root: &Path) {
    init_with(root, &[]);
}

pub(crate) fn init_with(root: &Path, extra_args: &[&str]) {
    std::fs::create_dir_all(root)
        .unwrap_or_else(|error| panic!("create {}: {error}", root.display()));
    let mut args = vec!["init", "-q"];
    args.extend_from_slice(extra_args);
    run(root, &args);
    run(root, &["config", "core.autocrlf", "false"]);
}

pub(crate) fn commit_all(root: &Path, message: &str) {
    run(root, &["add", "-A"]);
    run(root, &["commit", "-q", "--no-verify", "-m", message]);
}

pub(crate) fn committed_repo(root: &Path, files: &[(&str, &str)]) {
    init(root);
    for (name, content) in files {
        let path = root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|error| panic!("create {}: {error}", parent.display()));
        }
        std::fs::write(&path, content)
            .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    }
    commit_all(root, "init");
}

fn empty_global_config() -> &'static Path {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        let path = std::env::temp_dir().join(EMPTY_GLOBAL_CONFIG_NAME);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
        let metadata = file
            .metadata()
            .unwrap_or_else(|error| panic!("stat {}: {error}", path.display()));
        assert!(
            metadata.is_file() && metadata.len() == 0,
            "{} must be an empty regular file",
            path.display()
        );
        path
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fixture_repository_ignores_the_user_global_and_system_git_config() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        committed_repo(&root, &[("a.txt", "a\n")]);
        let command = command(&root, &["status"]);
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert!(envs.contains(&(
            "GIT_CONFIG_GLOBAL".to_string(),
            Some(empty_global_config().display().to_string())
        )));
        assert!(envs.contains(&("GIT_CONFIG_NOSYSTEM".to_string(), Some("1".to_string()))));
        for inherited in ["GIT_DIR", "GIT_INDEX_FILE"] {
            assert!(envs.contains(&(inherited.to_string(), None)), "{inherited}");
        }
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for setting in ["commit.gpgsign=false", "tag.gpgsign=false"] {
            assert!(args.iter().any(|arg| arg == setting), "{setting}");
        }
        assert_eq!(
            run(&root, &["config", "--global", "--list"]).trim(),
            "",
            "the fixture reads an empty global config"
        );
        assert_eq!(
            run(&root, &["log", "--format=%an <%ae>"]).trim(),
            "Paneflow <paneflow@example.com>"
        );
    }

    #[test]
    fn a_signing_user_config_never_reaches_a_fixture_commit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        init(&root);
        run(&root, &["config", "commit.gpgsign", "true"]);
        run(&root, &["config", "tag.gpgsign", "true"]);
        run(
            &root,
            &["config", "gpg.program", "/nonexistent/paneflow-gpg"],
        );
        std::fs::write(root.join("a.txt"), "a\n").expect("file");
        commit_all(&root, "unsigned");
        run(&root, &["tag", "-a", "v1", "-m", "unsigned tag"]);
        assert!(!run(&root, &["cat-file", "commit", "HEAD"]).contains("gpgsig"));
    }

    #[test]
    #[should_panic(expected = "exited with")]
    fn a_failing_fixture_step_fails_the_test_instead_of_skipping_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        init(tmp.path());
        commit_all(tmp.path(), "nothing to commit");
    }

    #[test]
    fn the_builder_isolates_every_git_spawned_by_code_under_test() {
        let command = git(GitProfile::Probe, ["status"]);
        let global = command
            .get_envs()
            .find(|(key, _)| *key == "GIT_CONFIG_GLOBAL")
            .and_then(|(_, value)| value)
            .map(|value| value.to_os_string());
        assert_eq!(global.as_deref(), Some(empty_global_config().as_os_str()));
    }
}
