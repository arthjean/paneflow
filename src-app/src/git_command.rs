use std::collections::BTreeSet;
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
    "log.showSignature=false",
];

const PROBE_ENV: &[(&str, &str)] = &[
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("LC_ALL", "C"),
    ("LANGUAGE", "C"),
];

const PROBE_DIFF_SUBCOMMANDS: &[&str] = &["diff", "show"];

const PROBE_DIFF_FLAGS: &[&str] = &["--no-ext-diff", "--no-textconv"];

const OPTIONS_WITH_VALUE: &[&str] = &["-c", "-C"];

const WORKTREE_READING_SUBCOMMANDS: &[&str] =
    &["status", "diff", "ls-files", "diff-files", "diff-index"];

const FILTER_DRIVER_KEYS: &str = r"^filter\..*\.(clean|smudge|process|required)$";

const REPOSITORY_CONFIG_SCOPES: &[&str] = &["local", "worktree"];

const NEUTRALIZED_FILTER_SETTINGS: &[(&str, &str)] = &[
    ("clean", ""),
    ("smudge", ""),
    ("process", ""),
    ("required", "false"),
];

const FILTER_QUERY_DEADLINE: Duration = Duration::from_secs(5);

const FILTER_QUERY_STDOUT_CAP: u64 = 64 * 1024;

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
    #[cfg(test)]
    crate::git_fixture::isolate(&mut command);
    if profile == GitProfile::Probe {
        command.envs(PROBE_ENV.iter().copied());
        for setting in PROBE_CONFIG {
            command.arg("-c").arg(setting);
        }
        let mut hooks_path = OsString::from("core.hooksPath=");
        hooks_path.push(empty_hooks_dir());
        command.arg("-c").arg(hooks_path);
    }
    let args: Vec<OsString> = args
        .into_iter()
        .map(|arg| arg.as_ref().to_os_string())
        .collect();
    let subcommand = subcommand_index(&args);
    for (index, arg) in args.iter().enumerate() {
        command.arg(arg);
        if profile == GitProfile::Probe
            && Some(index) == subcommand
            && PROBE_DIFF_SUBCOMMANDS.contains(&arg.to_str().unwrap_or_default())
        {
            command.args(PROBE_DIFF_FLAGS);
        }
    }
    command
}

fn subcommand_index<S: AsRef<OsStr>>(args: &[S]) -> Option<usize> {
    let mut option_value_pending = false;
    for (index, arg) in args.iter().enumerate() {
        if option_value_pending {
            option_value_pending = false;
            continue;
        }
        let text = arg.as_ref().to_str().unwrap_or_default();
        if OPTIONS_WITH_VALUE.contains(&text) {
            option_value_pending = true;
            continue;
        }
        if text.starts_with('-') {
            continue;
        }
        return Some(index);
    }
    None
}

fn is_probe(command: &Command) -> bool {
    command
        .get_envs()
        .any(|(key, value)| key == PROBE_ENV[0].0 && value == Some(OsStr::new(PROBE_ENV[0].1)))
}

fn reads_the_worktree(command: &Command) -> bool {
    let args: Vec<&OsStr> = command.get_args().collect();
    subcommand_index(&args).is_some_and(|index| {
        WORKTREE_READING_SUBCOMMANDS.contains(&args[index].to_str().unwrap_or_default())
    })
}

fn neutralize_repository_filters(command: &mut Command) -> Result<(), paneflow_process::ProcError> {
    if !is_probe(command) || !reads_the_worktree(command) {
        return Ok(());
    }
    let mut query = git(
        GitProfile::Probe,
        [
            "config",
            "--show-scope",
            "--name-only",
            "-z",
            "--get-regexp",
            FILTER_DRIVER_KEYS,
        ],
    );
    if let Some(dir) = command.get_current_dir() {
        query.current_dir(dir);
    }
    let output =
        paneflow_process::run_with_timeout(query, FILTER_QUERY_DEADLINE, FILTER_QUERY_STDOUT_CAP)?;
    let drivers = repository_filter_drivers(&output.stdout).ok_or_else(|| {
        paneflow_process::ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "git reported a filter driver whose name is not UTF-8",
        ))
    })?;
    let inherited = std::env::var("GIT_CONFIG_COUNT")
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0);
    let mut count = inherited;
    for driver in &drivers {
        for (setting, value) in NEUTRALIZED_FILTER_SETTINGS {
            command.env(
                format!("GIT_CONFIG_KEY_{count}"),
                format!("filter.{driver}.{setting}"),
            );
            command.env(format!("GIT_CONFIG_VALUE_{count}"), value);
            count += 1;
        }
    }
    if count > inherited {
        command.env("GIT_CONFIG_COUNT", count.to_string());
    }
    Ok(())
}

fn repository_filter_drivers(listing: &[u8]) -> Option<BTreeSet<String>> {
    let mut drivers = BTreeSet::new();
    let mut fields = listing.split(|byte| *byte == 0);
    while let (Some(scope), Some(key)) = (fields.next(), fields.next()) {
        if !REPOSITORY_CONFIG_SCOPES
            .iter()
            .any(|wanted| wanted.as_bytes() == scope)
        {
            continue;
        }
        let key = std::str::from_utf8(key).ok()?;
        if let Some((driver, _)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        {
            drivers.insert(driver.to_string());
        }
    }
    Some(drivers)
}

pub(crate) fn run(
    mut command: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<paneflow_process::BoundedOutput, paneflow_process::ProcError> {
    neutralize_repository_filters(&mut command)?;
    paneflow_process::run_with_timeout_keeping_stderr_tail(command, deadline, stdout_cap)
}

pub(crate) fn run_keeping_stdout_head(
    mut command: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<(paneflow_process::BoundedOutput, bool), paneflow_process::ProcError> {
    neutralize_repository_filters(&mut command)?;
    paneflow_process::run_with_timeout_keeping_stdout_head(command, deadline, stdout_cap)
}

pub(crate) fn empty_hooks_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = empty_hooks_dir_in(&empty_hooks_parent());
        if let Err(error) = std::fs::create_dir_all(&dir) {
            log::warn!(
                "git: cannot create the empty hooks dir {}: {error}",
                dir.display()
            );
        }
        dir
    })
}

fn empty_hooks_dir_in(cache: &Path) -> PathBuf {
    cache.join(EMPTY_HOOKS_DIR_NAME)
}

#[cfg(not(test))]
fn empty_hooks_parent() -> PathBuf {
    paneflow_home::cache_dir().unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
fn empty_hooks_parent() -> PathBuf {
    crate::git_fixture::private_temp_root().join("cache")
}

#[cfg(test)]
#[path = "git_command_tests.rs"]
mod tests;
