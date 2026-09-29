use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitProfile {
    Probe,
    UserAction,
}

const INHERITED_REPOSITORY_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
];

const PROBE_CONFIG: &[&str] = &[
    "core.fsmonitor=false",
    "safe.bareRepository=explicit",
    "diff.external=",
    "diff.autoRefreshIndex=false",
];

const PROBE_ENV: &[(&str, &str)] = &[
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("LC_ALL", "C"),
    ("LANGUAGE", "C"),
];

const PROBE_DIFF_SUBCOMMANDS: &[&str] = &["diff", "show"];

const PROBE_DIFF_FLAGS: &[&str] = &["--no-ext-diff", "--no-textconv"];

const OPTIONS_WITH_VALUE: &[&str] = &["-c", "-C"];

const EMPTY_HOOKS_DIR_NAME: &str = "git-empty-hooks";

pub(crate) fn git<I, S>(profile: GitProfile, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new("git");
    for key in INHERITED_REPOSITORY_ENV {
        command.env_remove(key);
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    if profile == GitProfile::Probe {
        command.envs(PROBE_ENV.iter().copied());
        for setting in PROBE_CONFIG {
            command.arg("-c").arg(setting);
        }
        let mut hooks_path = OsString::from("core.hooksPath=");
        hooks_path.push(empty_hooks_dir());
        command.arg("-c").arg(hooks_path);
    }
    let mut subcommand_seen = false;
    let mut option_value_pending = false;
    for arg in args {
        let arg = arg.as_ref();
        command.arg(arg);
        if subcommand_seen {
            continue;
        }
        if option_value_pending {
            option_value_pending = false;
            continue;
        }
        let text = arg.to_str().unwrap_or_default();
        if OPTIONS_WITH_VALUE.contains(&text) {
            option_value_pending = true;
            continue;
        }
        if text.starts_with('-') {
            continue;
        }
        subcommand_seen = true;
        if profile == GitProfile::Probe && PROBE_DIFF_SUBCOMMANDS.contains(&text) {
            command.args(PROBE_DIFF_FLAGS);
        }
    }
    command
}

pub(crate) fn run(
    command: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<paneflow_process::BoundedOutput, paneflow_process::ProcError> {
    paneflow_process::run_with_timeout_keeping_stderr_tail(command, deadline, stdout_cap)
}

pub(crate) fn empty_hooks_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = paneflow_home::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(EMPTY_HOOKS_DIR_NAME);
        if let Err(error) = std::fs::create_dir_all(&dir) {
            log::warn!(
                "git: cannot create the empty hooks dir {}: {error}",
                dir.display()
            );
        }
        dir
    })
}

#[cfg(test)]
#[path = "git_command_tests.rs"]
mod tests;
