use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::Error;

pub const ENGINE_ENV: &str = "PANEFLOW_GIT_ENGINE";

pub const ACCEPTED_ENGINE_VALUES: &[&str] = &["gix", "cli"];

const REFTABLE_HEAD: &str = "ref: refs/heads/.invalid";

const SUPPORTED_OBJECT_FORMATS: &[&str] = &["sha1", "sha256"];

const REFTABLE_REF_STORAGE: &str = "reftable";

const HEAD_READ_CAP: u64 = 512;

const CONFIG_READ_CAP: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Engine {
    Gix,
    Cli,
}

impl Engine {
    pub const ALL: [Engine; 2] = [Engine::Gix, Engine::Cli];

    pub fn name(self) -> &'static str {
        match self {
            Engine::Gix => "gix",
            Engine::Cli => "cli",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Probe {
    FilesStatus,
    DiffStats,
    DockListing,
    IsClean,
}

impl Probe {
    pub const ALL: [Probe; 4] = [
        Probe::FilesStatus,
        Probe::DiffStats,
        Probe::DockListing,
        Probe::IsClean,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Probe::FilesStatus => "files_status",
            Probe::DiffStats => "diff_stats",
            Probe::DockListing => "dock_listing",
            Probe::IsClean => "is_clean",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Fallback {
    Reftable,
    ObjectFormat,
    Error,
    Forced,
    EntryCap,
    Monitor,
}

impl Fallback {
    pub const ALL: [Fallback; 6] = [
        Fallback::Reftable,
        Fallback::ObjectFormat,
        Fallback::Error,
        Fallback::Forced,
        Fallback::EntryCap,
        Fallback::Monitor,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Fallback::Reftable => "reftable",
            Fallback::ObjectFormat => "object_format",
            Fallback::Error => "error",
            Fallback::Forced => "forced",
            Fallback::EntryCap => "entry_cap",
            Fallback::Monitor => "monitor",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MonitorCounter {
    Incremental,
    Deferred,
    FirstProbe,
    Recovered,
    LossWatchSetChanged,
    LossRescan,
    LossOverflow,
    LossBackendError,
    LossBackendStopped,
    Recovering,
    Fence,
    JournalMissing,
    JournalReplaced,
    JournalWrapped,
    JournalRead,
    JournalVolumes,
    JournalOverflow,
    DrainGap,
    Input,
    IndexFormat,
    IndexCarryLimit,
    SkipWorktree,
    DirtyLimit,
    WorktreeRoot,
    Coverage,
    Verification,
    Drift,
    IneligibleNetworkFileSystem,
    IneligibleFuse,
    IneligibleWslDrive,
    IneligibleWslNetworkPath,
    IneligibleUnsupportedFileSystem,
    IneligibleUnidentified,
    IneligibleNoCacheDirectory,
    IneligibleMarkerDirectory,
    IneligibleWatchBudget,
    IneligibleScan,
    IneligibleRegistration,
    WorktreeDependentIndex,
    MonitorError,
    Disabled,
}

impl MonitorCounter {
    pub const ALL: [MonitorCounter; 41] = [
        MonitorCounter::Incremental,
        MonitorCounter::Deferred,
        MonitorCounter::FirstProbe,
        MonitorCounter::Recovered,
        MonitorCounter::LossWatchSetChanged,
        MonitorCounter::LossRescan,
        MonitorCounter::LossOverflow,
        MonitorCounter::LossBackendError,
        MonitorCounter::LossBackendStopped,
        MonitorCounter::Recovering,
        MonitorCounter::Fence,
        MonitorCounter::JournalMissing,
        MonitorCounter::JournalReplaced,
        MonitorCounter::JournalWrapped,
        MonitorCounter::JournalRead,
        MonitorCounter::JournalVolumes,
        MonitorCounter::JournalOverflow,
        MonitorCounter::DrainGap,
        MonitorCounter::Input,
        MonitorCounter::IndexFormat,
        MonitorCounter::IndexCarryLimit,
        MonitorCounter::SkipWorktree,
        MonitorCounter::DirtyLimit,
        MonitorCounter::WorktreeRoot,
        MonitorCounter::Coverage,
        MonitorCounter::Verification,
        MonitorCounter::Drift,
        MonitorCounter::IneligibleNetworkFileSystem,
        MonitorCounter::IneligibleFuse,
        MonitorCounter::IneligibleWslDrive,
        MonitorCounter::IneligibleWslNetworkPath,
        MonitorCounter::IneligibleUnsupportedFileSystem,
        MonitorCounter::IneligibleUnidentified,
        MonitorCounter::IneligibleNoCacheDirectory,
        MonitorCounter::IneligibleMarkerDirectory,
        MonitorCounter::IneligibleWatchBudget,
        MonitorCounter::IneligibleScan,
        MonitorCounter::IneligibleRegistration,
        MonitorCounter::WorktreeDependentIndex,
        MonitorCounter::MonitorError,
        MonitorCounter::Disabled,
    ];

    pub fn name(self) -> &'static str {
        match self {
            MonitorCounter::Incremental => "incremental",
            MonitorCounter::Deferred => "deferred",
            MonitorCounter::FirstProbe => "first_probe",
            MonitorCounter::Recovered => "recovered",
            MonitorCounter::LossWatchSetChanged => "loss_watch_set_changed",
            MonitorCounter::LossRescan => "loss_rescan",
            MonitorCounter::LossOverflow => "loss_overflow",
            MonitorCounter::LossBackendError => "loss_backend_error",
            MonitorCounter::LossBackendStopped => "loss_backend_stopped",
            MonitorCounter::Recovering => "recovering",
            MonitorCounter::Fence => "fence",
            MonitorCounter::JournalMissing => "journal_missing",
            MonitorCounter::JournalReplaced => "journal_replaced",
            MonitorCounter::JournalWrapped => "journal_wrapped",
            MonitorCounter::JournalRead => "journal_read",
            MonitorCounter::JournalVolumes => "journal_volumes",
            MonitorCounter::JournalOverflow => "journal_overflow",
            MonitorCounter::DrainGap => "drain_gap",
            MonitorCounter::Input => "input",
            MonitorCounter::IndexFormat => "index_format",
            MonitorCounter::IndexCarryLimit => "index_carry_limit",
            MonitorCounter::SkipWorktree => "skip_worktree",
            MonitorCounter::DirtyLimit => "dirty_limit",
            MonitorCounter::WorktreeRoot => "worktree_root",
            MonitorCounter::Coverage => "coverage",
            MonitorCounter::Verification => "verification",
            MonitorCounter::Drift => "drift",
            MonitorCounter::IneligibleNetworkFileSystem => "ineligible_network_file_system",
            MonitorCounter::IneligibleFuse => "ineligible_fuse",
            MonitorCounter::IneligibleWslDrive => "ineligible_wsl_drive",
            MonitorCounter::IneligibleWslNetworkPath => "ineligible_wsl_network_path",
            MonitorCounter::IneligibleUnsupportedFileSystem => "ineligible_unsupported_file_system",
            MonitorCounter::IneligibleUnidentified => "ineligible_unidentified",
            MonitorCounter::IneligibleNoCacheDirectory => "ineligible_no_cache_directory",
            MonitorCounter::IneligibleMarkerDirectory => "ineligible_marker_directory",
            MonitorCounter::IneligibleWatchBudget => "ineligible_watch_budget",
            MonitorCounter::IneligibleScan => "ineligible_scan",
            MonitorCounter::IneligibleRegistration => "ineligible_registration",
            MonitorCounter::WorktreeDependentIndex => "worktree_dependent_index",
            MonitorCounter::MonitorError => "monitor_error",
            MonitorCounter::Disabled => "disabled",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineSetting {
    pub preferred: Engine,
    pub warning: Option<String>,
}

pub fn parse_engine_setting(value: Option<&str>) -> EngineSetting {
    let Some(raw) = value else {
        return EngineSetting {
            preferred: Engine::Gix,
            warning: None,
        };
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "gix" => EngineSetting {
            preferred: Engine::Gix,
            warning: None,
        },
        "cli" => EngineSetting {
            preferred: Engine::Cli,
            warning: None,
        },
        _ => EngineSetting {
            preferred: Engine::Gix,
            warning: Some(format!(
                "{ENGINE_ENV}={raw:?} is not recognized, using gix; accepted values: {}",
                ACCEPTED_ENGINE_VALUES.join(", ")
            )),
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub probes: Vec<(Probe, Engine, u64)>,
    pub fallbacks: Vec<(Fallback, u64)>,
    pub monitor: Vec<(MonitorCounter, u64)>,
    pub unavailable_directories: u64,
}

impl Counters {
    pub fn monitor(&self, counter: MonitorCounter) -> u64 {
        self.monitor
            .iter()
            .find(|(c, _)| *c == counter)
            .map_or(0, |(_, count)| *count)
    }

    pub fn probes(&self, probe: Probe, engine: Engine) -> u64 {
        self.probes
            .iter()
            .find(|(p, e, _)| *p == probe && *e == engine)
            .map_or(0, |(_, _, count)| *count)
    }

    pub fn fallbacks(&self, reason: Fallback) -> u64 {
        self.fallbacks
            .iter()
            .find(|(r, _)| *r == reason)
            .map_or(0, |(_, count)| *count)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: Fallback,
    pub detail: String,
    repository_fault: bool,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "the git status is unavailable: {}", self.detail)
    }
}

impl Unavailable {
    fn from_error(error: &Error) -> Self {
        Self {
            reason: fallback_for(error),
            detail: error.to_string(),
            repository_fault: !matches!(error, Error::Interrupted | Error::TooManyEntries { .. }),
        }
    }

    fn refused(reason: Fallback, detail: String) -> Self {
        Self {
            reason,
            detail,
            repository_fault: true,
        }
    }
}

pub struct Selector {
    preferred: Engine,
    unavailable_directories: Mutex<HashMap<PathBuf, Fallback>>,
    probes: [[AtomicU64; 2]; Probe::ALL.len()],
    fallbacks: [AtomicU64; Fallback::ALL.len()],
    monitor: [AtomicU64; MonitorCounter::ALL.len()],
}

#[derive(Debug)]
struct Layout {
    git_dir: PathBuf,
    common_dir: PathBuf,
}

impl Selector {
    pub fn new(preferred: Engine) -> Self {
        Self {
            preferred,
            unavailable_directories: Mutex::new(HashMap::new()),
            probes: [const { [const { AtomicU64::new(0) }; 2] }; Probe::ALL.len()],
            fallbacks: [const { AtomicU64::new(0) }; Fallback::ALL.len()],
            monitor: [const { AtomicU64::new(0) }; MonitorCounter::ALL.len()],
        }
    }

    pub fn global() -> &'static Selector {
        static GLOBAL: OnceLock<Selector> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            let value = std::env::var(ENGINE_ENV).ok();
            let setting = parse_engine_setting(value.as_deref());
            if let Some(warning) = &setting.warning {
                log::warn!("{warning}");
            }
            Selector::new(setting.preferred)
        })
    }

    pub fn preferred(&self) -> Engine {
        self.preferred
    }

    pub fn run<T>(
        &self,
        probe: Probe,
        cwd: &Path,
        gix: impl FnOnce() -> Result<T, Error>,
        cli: impl FnOnce() -> T,
        unavailable: impl FnOnce(&Unavailable) -> T,
    ) -> T {
        if self.preferred == Engine::Cli {
            self.count_fallback(Fallback::Forced);
            self.count_probe(probe, Engine::Cli);
            return cli();
        }
        let layout = locate(cwd);
        if let Some(refused) = layout.as_ref().and_then(refuse_before_gix) {
            return self.answer_unavailable(probe, layout.as_ref(), &refused, unavailable);
        }
        match gix() {
            Ok(answer) => {
                if let Some(layout) = &layout {
                    self.record_answer(probe, &layout.git_dir);
                }
                self.count_probe(probe, Engine::Gix);
                answer
            }
            Err(error) => {
                let refused = Unavailable::from_error(&error);
                self.answer_unavailable(probe, layout.as_ref(), &refused, unavailable)
            }
        }
    }

    pub fn counters(&self) -> Counters {
        let mut probes = Vec::with_capacity(Probe::ALL.len() * Engine::ALL.len());
        for probe in Probe::ALL {
            for (engine_index, engine) in Engine::ALL.into_iter().enumerate() {
                probes.push((
                    probe,
                    engine,
                    self.probes[probe.index()][engine_index].load(Ordering::Relaxed),
                ));
            }
        }
        Counters {
            probes,
            fallbacks: Fallback::ALL
                .into_iter()
                .map(|reason| {
                    (
                        reason,
                        self.fallbacks[reason.index()].load(Ordering::Relaxed),
                    )
                })
                .collect(),
            monitor: MonitorCounter::ALL
                .into_iter()
                .map(|counter| {
                    (
                        counter,
                        self.monitor[counter.index()].load(Ordering::Relaxed),
                    )
                })
                .collect(),
            unavailable_directories: self.directories().len() as u64,
        }
    }

    fn answer_unavailable<T>(
        &self,
        probe: Probe,
        layout: Option<&Layout>,
        refused: &Unavailable,
        unavailable: impl FnOnce(&Unavailable) -> T,
    ) -> T {
        self.count_fallback(refused.reason);
        match layout.filter(|_| refused.repository_fault) {
            Some(layout) => self.record_unavailable(probe, &layout.git_dir, refused),
            None => log::debug!("{} answered without gix: {refused}", probe.name()),
        }
        unavailable(refused)
    }

    fn count_probe(&self, probe: Probe, engine: Engine) {
        let engine_index = Engine::ALL
            .iter()
            .position(|known| *known == engine)
            .unwrap_or_default();
        self.probes[probe.index()][engine_index].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_fallback(&self, reason: Fallback) {
        self.count_fallback(reason);
    }

    pub fn record_monitor(&self, counter: MonitorCounter) {
        self.monitor[counter.index()].fetch_add(1, Ordering::Relaxed);
    }

    fn count_fallback(&self, reason: Fallback) {
        self.fallbacks[reason.index()].fetch_add(1, Ordering::Relaxed);
    }

    fn directories(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Fallback>> {
        self.unavailable_directories
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn record_answer(&self, probe: Probe, git_dir: &Path) {
        if let Some(reason) = self.directories().remove(git_dir) {
            log::info!(
                "the in-process git engine answers {} for {} again, after {}",
                probe.name(),
                git_dir.display(),
                reason.name()
            );
        }
    }

    fn record_unavailable(&self, probe: Probe, git_dir: &Path, refused: &Unavailable) {
        let previous = self
            .directories()
            .insert(git_dir.to_path_buf(), refused.reason);
        if previous == Some(refused.reason) {
            log::debug!("{} for {}: {refused}", probe.name(), git_dir.display());
        } else {
            log::warn!(
                "the in-process git engine cannot answer {} for {}, which shows no git status until it can: {}",
                probe.name(),
                git_dir.display(),
                refused.detail
            );
        }
    }
}

fn refuse_before_gix(layout: &Layout) -> Option<Unavailable> {
    if uses_reftable(layout) {
        return Some(Unavailable::refused(
            Fallback::Reftable,
            "the repository stores its references in reftable, which the in-process git engine cannot read yet".to_string(),
        ));
    }
    unsupported_object_format(layout).map(|format| {
        Unavailable::refused(
            Fallback::ObjectFormat,
            format!(
                "the repository uses the {format} object format, which the in-process git engine cannot read"
            ),
        )
    })
}

fn fallback_for(error: &Error) -> Fallback {
    match error {
        Error::TooManyEntries { .. } => Fallback::EntryCap,
        _ => Fallback::Error,
    }
}

fn locate(cwd: &Path) -> Option<Layout> {
    let (path, _trust) = gix::discover::upwards(cwd).ok()?;
    let (git_dir, _work_dir) = path.into_repository_and_work_tree_directories();
    let git_dir = std::path::absolute(&git_dir).unwrap_or(git_dir);
    let common_dir = read_capped(&git_dir.join("commondir"), HEAD_READ_CAP)
        .map(|relative| git_dir.join(relative.trim()))
        .unwrap_or_else(|| git_dir.clone());
    Some(Layout {
        git_dir,
        common_dir,
    })
}

fn read_capped(path: &Path, cap: u64) -> Option<String> {
    use std::io::Read;
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(cap)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn repository_extension(layout: &Layout, key: &str) -> Option<String> {
    let path = layout.common_dir.join("config");
    let bytes = read_capped(&path, CONFIG_READ_CAP)?;
    let config = gix::config::File::from_bytes_no_includes(
        bytes.as_bytes(),
        gix::config::file::Metadata::from(gix::config::Source::Local),
        gix::config::file::init::Options::default(),
    )
    .ok()?;
    config
        .string(key)
        .map(|value| value.to_string().trim().to_ascii_lowercase())
}

fn uses_reftable(layout: &Layout) -> bool {
    let marked = read_capped(&layout.git_dir.join("HEAD"), HEAD_READ_CAP)
        .is_some_and(|head| head.trim() == REFTABLE_HEAD);
    marked
        || repository_extension(layout, "extensions.refStorage").as_deref()
            == Some(REFTABLE_REF_STORAGE)
}

fn unsupported_object_format(layout: &Layout) -> Option<String> {
    repository_extension(layout, "extensions.objectFormat")
        .filter(|format| !SUPPORTED_OBJECT_FORMATS.contains(&format.as_str()))
}
