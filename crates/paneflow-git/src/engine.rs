use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};

use crate::Error;

pub const ENGINE_ENV: &str = "PANEFLOW_GIT_ENGINE";

pub const ACCEPTED_ENGINE_VALUES: &[&str] = &["gix", "cli"];

pub const PINNING_FAILURE_COUNT: u32 = 2;

const REFTABLE_HEAD: &str = "ref: refs/heads/.invalid";

const SHA1_OBJECT_FORMAT: &str = "sha1";

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
}

impl Fallback {
    pub const ALL: [Fallback; 5] = [
        Fallback::Reftable,
        Fallback::ObjectFormat,
        Fallback::Error,
        Fallback::Forced,
        Fallback::EntryCap,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Fallback::Reftable => "reftable",
            Fallback::ObjectFormat => "object_format",
            Fallback::Error => "error",
            Fallback::Forced => "forced",
            Fallback::EntryCap => "entry_cap",
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
    pub pinned_directories: u64,
}

impl Counters {
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

#[derive(Default)]
struct Failures {
    consecutive: u32,
    pinned: bool,
}

pub struct Selector {
    preferred: Engine,
    failures: Mutex<HashMap<PathBuf, Failures>>,
    probes: [[AtomicU64; 2]; Probe::ALL.len()],
    fallbacks: [AtomicU64; Fallback::ALL.len()],
    pinned_directories: AtomicU64,
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
            failures: Mutex::new(HashMap::new()),
            probes: [const { [const { AtomicU64::new(0) }; 2] }; Probe::ALL.len()],
            fallbacks: [const { AtomicU64::new(0) }; Fallback::ALL.len()],
            pinned_directories: AtomicU64::new(0),
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
    ) -> T {
        let layout = locate(cwd);
        if let Some(reason) = self.fallback_before_gix(layout.as_ref()) {
            self.count_fallback(reason);
            return self.answer_with_cli(probe, cli);
        }
        match gix() {
            Ok(answer) => {
                if let Some(layout) = &layout {
                    self.record_success(&layout.git_dir);
                }
                self.count_probe(probe, Engine::Gix);
                answer
            }
            Err(error) => {
                if let Some(layout) = layout.as_ref().filter(|_| counts_toward_pinning(&error)) {
                    self.record_failure(&layout.git_dir, &error);
                }
                self.count_fallback(fallback_for(&error));
                self.answer_with_cli(probe, cli)
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
            pinned_directories: self.pinned_directories.load(Ordering::Relaxed),
        }
    }

    fn fallback_before_gix(&self, layout: Option<&Layout>) -> Option<Fallback> {
        if self.preferred == Engine::Cli {
            return Some(Fallback::Forced);
        }
        let layout = layout?;
        if uses_reftable(layout) {
            return Some(Fallback::Reftable);
        }
        if !uses_sha1_objects(layout) {
            return Some(Fallback::ObjectFormat);
        }
        self.is_pinned(&layout.git_dir).then_some(Fallback::Error)
    }

    fn answer_with_cli<T>(&self, probe: Probe, cli: impl FnOnce() -> T) -> T {
        self.count_probe(probe, Engine::Cli);
        cli()
    }

    fn count_probe(&self, probe: Probe, engine: Engine) {
        let engine_index = Engine::ALL
            .iter()
            .position(|known| *known == engine)
            .unwrap_or_default();
        self.probes[probe.index()][engine_index].fetch_add(1, Ordering::Relaxed);
    }

    fn count_fallback(&self, reason: Fallback) {
        self.fallbacks[reason.index()].fetch_add(1, Ordering::Relaxed);
    }

    fn is_pinned(&self, git_dir: &Path) -> bool {
        self.failures
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(git_dir)
            .is_some_and(|failures| failures.pinned)
    }

    fn record_success(&self, git_dir: &Path) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(entry) = failures.get_mut(git_dir).filter(|entry| !entry.pinned) {
            entry.consecutive = 0;
        }
    }

    fn record_failure(&self, git_dir: &Path, error: &Error) {
        let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
        let entry = failures.entry(git_dir.to_path_buf()).or_default();
        entry.consecutive += 1;
        if entry.consecutive >= PINNING_FAILURE_COUNT && !entry.pinned {
            entry.pinned = true;
            self.pinned_directories.fetch_add(1, Ordering::Relaxed);
            log::warn!(
                "the in-process git engine failed {} times in a row for {}, which now uses the git CLI until Paneflow restarts; last error: {error}",
                entry.consecutive,
                git_dir.display()
            );
        }
    }
}

fn counts_toward_pinning(error: &Error) -> bool {
    !matches!(error, Error::Interrupted | Error::TooManyEntries { .. })
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

fn uses_sha1_objects(layout: &Layout) -> bool {
    repository_extension(layout, "extensions.objectFormat")
        .is_none_or(|format| format == SHA1_OBJECT_FORMAT)
}
