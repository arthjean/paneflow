use std::collections::{BTreeSet, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

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

const FILTER_QUERY_KEYS: &str = r"^(filter\..*\.(clean|smudge|process|required)|include\.path|includeif\..*\.path|core\.repositoryformatversion|extensions\.worktreeconfig)$";

const REPOSITORY_CONFIG_SCOPES: &[&str] = &["local", "worktree"];

const UNCACHEABLE_INCLUDE_CONDITIONS: &[&str] = &["onbranch:", "hasconfig:"];

const FILTER_CACHE_CAPACITY: usize = 64;

const RACY_CONFIG_WINDOW: Duration = Duration::from_secs(2);

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

pub(crate) fn record_spawn(command: &Command) {
    let args: Vec<&OsStr> = command.get_args().collect();
    let subcommand = subcommand_index(&args).and_then(|index| args[index].to_str());
    crate::work_counters::record_git_spawn(is_probe(command), subcommand);
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
    let cwd = command.get_current_dir().map(Path::to_path_buf);
    let drivers = match cwd.as_deref().and_then(cached_filter_drivers) {
        Some(drivers) => drivers,
        None => query_filter_drivers(cwd.as_deref())?,
    };
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

fn query_filter_drivers(
    cwd: Option<&Path>,
) -> Result<BTreeSet<String>, paneflow_process::ProcError> {
    let started = SystemTime::now();
    let mut query = git(
        GitProfile::Probe,
        [
            "config",
            "--show-scope",
            "--show-origin",
            "-z",
            "--get-regexp",
            FILTER_QUERY_KEYS,
        ],
    );
    if let Some(dir) = cwd {
        query.current_dir(dir);
    }
    record_spawn(&query);
    let output =
        paneflow_process::run_with_timeout(query, FILTER_QUERY_DEADLINE, FILTER_QUERY_STDOUT_CAP)?;
    let base = cwd.and_then(git_working_base);
    let listing = repository_filter_listing(&output.stdout, base.as_deref()).ok_or_else(|| {
        paneflow_process::ProcError::Spawn(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "git reported a filter driver whose name is not UTF-8",
        ))
    })?;
    let answered =
        output.status.success() || (output.status.code() == Some(1) && output.stdout.is_empty());
    if answered && let (Some(cwd), Some(watched)) = (cwd, listing.watched) {
        remember_filter_drivers(cwd, watched, &listing.drivers, started);
    }
    Ok(listing.drivers)
}

struct FilterListing {
    drivers: BTreeSet<String>,
    watched: Option<BTreeSet<PathBuf>>,
}

fn git_working_base(cwd: &Path) -> Option<PathBuf> {
    let top = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    if cwd.starts_with(top.join(".git")) {
        Some(cwd.to_path_buf())
    } else {
        Some(top.to_path_buf())
    }
}

fn repository_filter_listing(listing: &[u8], base: Option<&Path>) -> Option<FilterListing> {
    let mut drivers = BTreeSet::new();
    let mut watched = base.map(|_| BTreeSet::new());
    let fields: Vec<&[u8]> = listing.split(|byte| *byte == 0).collect();
    for [scope, origin, entry] in fields.as_chunks::<3>().0 {
        if !REPOSITORY_CONFIG_SCOPES
            .iter()
            .any(|wanted| wanted.as_bytes() == *scope)
        {
            continue;
        }
        let (key, value) = match entry.iter().position(|byte| *byte == b'\n') {
            Some(split) => (&entry[..split], Some(&entry[split + 1..])),
            None => (*entry, None),
        };
        let key = std::str::from_utf8(key).ok()?;
        if let Some((driver, _)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        {
            drivers.insert(driver.to_string());
        }
        let Some(files) = watched.as_mut() else {
            continue;
        };
        let origin_file = std::str::from_utf8(origin)
            .ok()
            .and_then(|origin| origin.strip_prefix("file:"))
            .zip(base)
            .map(|(origin, base)| base.join(origin))
            .filter(|origin_file| origin_file.is_file());
        let Some(origin_file) = origin_file else {
            watched = None;
            continue;
        };
        match include_target(key, value, &origin_file) {
            IncludeTarget::NotAnInclude => {}
            IncludeTarget::File(target) => {
                files.insert(target);
            }
            IncludeTarget::Uncacheable => {
                watched = None;
                continue;
            }
        }
        if key == "extensions.worktreeconfig" {
            watched = None;
            continue;
        }
        if let Some(files) = watched.as_mut() {
            files.insert(origin_file);
        }
    }
    Some(FilterListing { drivers, watched })
}

enum IncludeTarget {
    NotAnInclude,
    File(PathBuf),
    Uncacheable,
}

fn include_target(key: &str, value: Option<&[u8]>, origin_file: &Path) -> IncludeTarget {
    let condition = match key {
        "include.path" => None,
        _ => match key
            .strip_prefix("includeif.")
            .and_then(|rest| rest.strip_suffix(".path"))
        {
            Some(condition) => Some(condition),
            None => return IncludeTarget::NotAnInclude,
        },
    };
    if condition.is_some_and(|condition| {
        UNCACHEABLE_INCLUDE_CONDITIONS
            .iter()
            .any(|prefix| condition.starts_with(prefix))
    }) {
        return IncludeTarget::Uncacheable;
    }
    let Some(path) = value.and_then(|value| std::str::from_utf8(value).ok()) else {
        return IncludeTarget::Uncacheable;
    };
    if let Some(home_relative) = path.strip_prefix("~/") {
        return dirs::home_dir().map_or(IncludeTarget::Uncacheable, |home| {
            IncludeTarget::File(home.join(home_relative))
        });
    }
    if path.starts_with('~') || path.starts_with("%(") {
        return IncludeTarget::Uncacheable;
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return IncludeTarget::File(path.to_path_buf());
    }
    origin_file
        .parent()
        .map_or(IncludeTarget::Uncacheable, |dir| {
            IncludeTarget::File(dir.join(path))
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
}

fn file_stamp(path: &Path) -> Option<FileStamp> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        inode: std::os::unix::fs::MetadataExt::ino(&metadata),
    })
}

struct CachedFilters {
    local_config: PathBuf,
    watched: Vec<(PathBuf, Option<FileStamp>)>,
    drivers: BTreeSet<String>,
}

fn filter_cache() -> &'static Mutex<HashMap<PathBuf, CachedFilters>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, CachedFilters>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn local_config_path(cwd: &Path) -> Option<PathBuf> {
    let git_dir = crate::workspace::git::find_git_dir(cwd.to_str()?)?;
    Some(crate::workspace::git::resolve_main_git_dir(&git_dir)?.join("config"))
}

fn cached_filter_drivers(cwd: &Path) -> Option<BTreeSet<String>> {
    let local_config = local_config_path(cwd)?;
    let cache = filter_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let held = cache.get(cwd)?;
    (held.local_config == local_config
        && held
            .watched
            .iter()
            .all(|(path, stamp)| file_stamp(path) == *stamp))
    .then(|| held.drivers.clone())
}

fn remember_filter_drivers(
    cwd: &Path,
    mut watched: BTreeSet<PathBuf>,
    drivers: &BTreeSet<String>,
    queried_at: SystemTime,
) {
    let Some(local_config) = local_config_path(cwd) else {
        return;
    };
    watched.insert(local_config.clone());
    let racy_after = queried_at
        .checked_sub(RACY_CONFIG_WINDOW)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut stamps = Vec::with_capacity(watched.len());
    for path in watched {
        let stamp = file_stamp(&path);
        if stamp
            .as_ref()
            .is_some_and(|stamp| stamp.modified.is_none_or(|modified| modified >= racy_after))
        {
            return;
        }
        stamps.push((path, stamp));
    }
    let mut cache = filter_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if cache.len() >= FILTER_CACHE_CAPACITY && !cache.contains_key(cwd) {
        cache.clear();
    }
    cache.insert(
        cwd.to_path_buf(),
        CachedFilters {
            local_config,
            watched: stamps,
            drivers: drivers.clone(),
        },
    );
}

pub(crate) fn run(
    mut command: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<paneflow_process::BoundedOutput, paneflow_process::ProcError> {
    neutralize_repository_filters(&mut command)?;
    record_spawn(&command);
    paneflow_process::run_with_timeout_keeping_stderr_tail(command, deadline, stdout_cap)
}

pub(crate) fn run_keeping_stdout_head(
    mut command: Command,
    deadline: Duration,
    stdout_cap: u64,
) -> Result<(paneflow_process::BoundedOutput, bool), paneflow_process::ProcError> {
    neutralize_repository_filters(&mut command)?;
    record_spawn(&command);
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
