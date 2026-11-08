mod coverage;
mod eligibility;
mod registry;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use coverage::Coverage;
#[cfg(target_os = "linux")]
pub use eligibility::inotify_watches_in_use;
pub use eligibility::{check_file_system, Ineligible};
pub use registry::{Feeds, IDLE_EVICTION};

pub const MAX_QUEUED_EVENTS: usize = 4096;

pub const MAX_QUEUED_BYTES: usize = 4 * 1024 * 1024;

pub const FENCE_DIRECTORY_NAME: &str = "git-fence";

const RECURSIVE_ROOT: bool = cfg!(any(target_os = "macos", windows));

const FENCES: bool = cfg!(any(target_os = "linux", target_os = "macos"));

const GITIGNORE: &str = ".gitignore";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedOptions {
    pub cache_dir: Option<PathBuf>,
    pub max_events: usize,
    pub max_bytes: usize,
    pub thread_limit: usize,
    pub watch_limit: Option<usize>,
}

impl FeedOptions {
    pub fn for_paneflow_home() -> Self {
        Self {
            cache_dir: paneflow_home::cache_dir(),
            ..Self::default()
        }
    }
}

impl Default for FeedOptions {
    fn default() -> Self {
        Self {
            cache_dir: None,
            max_events: MAX_QUEUED_EVENTS,
            max_bytes: MAX_QUEUED_BYTES,
            thread_limit: crate::PLATFORM_CAP,
            watch_limit: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loss {
    WatchSetChanged,
    Rescan,
    Overflow,
    BackendError,
    BackendStopped,
}

impl From<gix_notify::Loss> for Loss {
    fn from(loss: gix_notify::Loss) -> Self {
        match loss {
            gix_notify::Loss::WatchSetChanged => Loss::WatchSetChanged,
            gix_notify::Loss::Rescan => Loss::Rescan,
            gix_notify::Loss::Overflow => Loss::Overflow,
            gix_notify::Loss::BackendError => Loss::BackendError,
            gix_notify::Loss::BackendStopped => Loss::BackendStopped,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Worktree,
    GitDir,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Any,
    Create,
    Modify,
    Remove,
    Rename,
}

impl From<gix_notify::EventKind> for ChangeKind {
    fn from(kind: gix_notify::EventKind) -> Self {
        match kind {
            gix_notify::EventKind::Any => ChangeKind::Any,
            gix_notify::EventKind::Create => ChangeKind::Create,
            gix_notify::EventKind::Modify => ChangeKind::Modify,
            gix_notify::EventKind::Remove => ChangeKind::Remove,
            gix_notify::EventKind::Rename => ChangeKind::Rename,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub origin: Origin,
    pub root: PathBuf,
    pub relative: PathBuf,
    pub kind: ChangeKind,
    pub directory: Option<bool>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub changes: Vec<Change>,
    pub loss: Option<Loss>,
    pub dropped: usize,
    pub generation: u64,
    pub sequence: u64,
}

impl Batch {
    fn absorb(&mut self, mut next: Batch) {
        self.loss = self.loss.or(next.loss);
        self.dropped += next.dropped;
        self.changes.append(&mut next.changes);
        self.generation = next.generation;
        self.sequence = next.sequence;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FenceError {
    #[error("this platform has no delivery fence")]
    Unsupported,
    #[error("the fence deadline passed")]
    TimedOut,
    #[error("watch coverage was lost while fencing")]
    CoverageLost,
    #[error("the watcher stopped")]
    Stopped,
    #[error("an earlier fence is still pending")]
    Busy,
}

impl From<gix_notify::SynchronizeError> for FenceError {
    fn from(error: gix_notify::SynchronizeError) -> Self {
        match error {
            gix_notify::SynchronizeError::TimedOut => FenceError::TimedOut,
            gix_notify::SynchronizeError::CoverageLost => FenceError::CoverageLost,
            gix_notify::SynchronizeError::Stopped => FenceError::Stopped,
            gix_notify::SynchronizeError::Busy => FenceError::Busy,
            _ => FenceError::Unsupported,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchedPath {
    pub path: PathBuf,
    pub recursive: bool,
}

pub struct ChangeFeed {
    watcher: gix_notify::Watcher,
    coverage: Coverage,
    watches: Vec<WatchedPath>,
    marker: Option<PathBuf>,
    thread_limit: usize,
    watch_limit: Option<usize>,
    stale: bool,
}

impl std::fmt::Debug for ChangeFeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChangeFeed")
            .field("worktree", &self.coverage.worktree)
            .field("watches", &self.watches.len())
            .finish_non_exhaustive()
    }
}

impl ChangeFeed {
    pub fn start(worktree: &Path, options: &FeedOptions) -> Result<Self, Ineligible> {
        let result = Self::try_start(worktree, options);
        if let Err(reason) = &result {
            log::warn!(
                "git change feed for {} is ineligible, probes take full statuses: {reason}",
                worktree.display()
            );
        }
        result
    }

    fn try_start(worktree: &Path, options: &FeedOptions) -> Result<Self, Ineligible> {
        let requested = std::fs::canonicalize(worktree).map_err(|error| Ineligible::Scan {
            cause: format!("resolve {}: {error}", worktree.display()),
        })?;
        check_file_system(&requested)?;
        let coverage =
            Coverage::scan(&requested, options.thread_limit).map_err(|error| Ineligible::Scan {
                cause: error.to_string(),
            })?;
        check_file_system(&coverage.worktree)?;
        for git_dir in &coverage.git_dirs {
            check_file_system(git_dir)?;
        }
        let marker = if FENCES {
            Some(fence_directory(options)?)
        } else {
            None
        };
        let watches = desired_watches(&coverage, marker.as_deref());
        check_watch_budget(watches.len(), options.watch_limit)?;
        let mut watcher = gix_notify::Watcher::new(gix_notify::Options {
            max_events: options.max_events,
            max_bytes: options.max_bytes,
        })
        .map_err(|error| Ineligible::Registration {
            cause: error.to_string(),
        })?;
        watcher
            .replace(watches.iter().map(|watch| gix_notify::Watch {
                path: watch.path.clone(),
                recursive: watch.recursive,
            }))
            .map_err(|error| Ineligible::Registration {
                cause: format!("{error:#}"),
            })?;
        Ok(Self {
            watcher,
            coverage,
            watches,
            marker,
            thread_limit: options.thread_limit,
            watch_limit: options.watch_limit,
            stale: false,
        })
    }

    pub fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    pub fn worktree(&self) -> &Path {
        &self.coverage.worktree
    }

    pub fn watches(&self) -> &[WatchedPath] {
        &self.watches
    }

    pub fn fence_directory(&self) -> Option<&Path> {
        self.marker.as_deref()
    }

    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.watcher.set_waker(wake);
    }

    pub fn invalidate_coverage(&mut self) {
        self.stale = true;
    }

    pub fn drain(&mut self) -> Batch {
        let mut batch = self.drain_native();
        if batch.loss.is_some_and(|loss| loss != Loss::WatchSetChanged) {
            self.stale = true;
        }
        if self.stale || self.coverage_is_outdated(&batch) {
            self.refresh_coverage(&mut batch);
        }
        batch
    }

    fn coverage_is_outdated(&self, batch: &Batch) -> bool {
        batch.changes.iter().any(|change| match change.origin {
            Origin::GitDir => change.relative == Path::new("info").join("exclude"),
            Origin::Worktree => {
                change.relative.file_name() == Some(OsStr::new(GITIGNORE))
                    || (!RECURSIVE_ROOT && self.is_unwatched_directory(change))
            }
        })
    }

    fn is_unwatched_directory(&self, change: &Change) -> bool {
        change.directory == Some(true)
            && matches!(change.kind, ChangeKind::Create | ChangeKind::Rename)
            && !self
                .coverage
                .directories
                .contains(&change.root.join(&change.relative))
    }

    fn refresh_coverage(&mut self, batch: &mut Batch) {
        match self.rescan() {
            Ok(false) => self.stale = false,
            Ok(true) => {
                self.stale = false;
                let next = self.drain_native();
                batch.absorb(next);
                batch.loss.get_or_insert(Loss::WatchSetChanged);
            }
            Err(reason) => {
                if !self.stale {
                    log::warn!(
                        "git change feed for {} lost its coverage, probes take full statuses until it recovers: {reason}",
                        self.coverage.worktree.display()
                    );
                }
                self.stale = true;
                batch.loss.get_or_insert(Loss::BackendStopped);
            }
        }
    }

    fn rescan(&mut self) -> Result<bool, Ineligible> {
        let coverage =
            Coverage::scan(&self.coverage.worktree, self.thread_limit).map_err(|error| {
                Ineligible::Scan {
                    cause: error.to_string(),
                }
            })?;
        if coverage.worktree != self.coverage.worktree {
            return Err(Ineligible::Scan {
                cause: format!(
                    "{} now resolves to the worktree {}",
                    self.coverage.worktree.display(),
                    coverage.worktree.display()
                ),
            });
        }
        check_file_system(&coverage.worktree)?;
        let watches = desired_watches(&coverage, self.marker.as_deref());
        let unignored = self
            .coverage
            .ignored
            .difference(&coverage.ignored)
            .next()
            .is_some();
        let rewatch = self.stale || watches != self.watches;
        if rewatch {
            check_watch_budget(
                watches.len().saturating_sub(self.watches.len()),
                self.watch_limit,
            )?;
            self.watcher
                .replace(watches.iter().map(|watch| gix_notify::Watch {
                    path: watch.path.clone(),
                    recursive: watch.recursive,
                }))
                .map_err(|error| Ineligible::Registration {
                    cause: format!("{error:#}"),
                })?;
            self.watches = watches;
        }
        self.coverage = coverage;
        Ok(rewatch || unignored)
    }

    fn drain_native(&mut self) -> Batch {
        let mut batch = Batch::default();
        loop {
            let drained = self.watcher.drain(gix_notify::Budget {
                max_events: usize::MAX,
                max_bytes: usize::MAX,
            });
            if let Some(loss) = drained.loss {
                batch.loss.get_or_insert(Loss::from(loss));
            }
            batch.generation = drained.generation;
            batch.sequence = drained.sequence;
            for event in drained.events {
                let directory = match event.path_kind {
                    gix_notify::PathKind::Directory => Some(true),
                    gix_notify::PathKind::File => Some(false),
                    gix_notify::PathKind::Any => None,
                };
                let kind = ChangeKind::from(event.kind);
                for path in event.paths {
                    match self.classify(&path, kind) {
                        Some((origin, root, relative)) => batch.changes.push(Change {
                            origin,
                            root,
                            relative,
                            kind,
                            directory,
                        }),
                        None => batch.dropped += 1,
                    }
                }
            }
            if !drained.more {
                return batch;
            }
        }
    }

    pub fn fence(&mut self, deadline: Instant) -> Result<Batch, FenceError> {
        let Some(marker) = self.marker.clone() else {
            return Err(FenceError::Unsupported);
        };
        let fence = self.watcher.synchronize_at(&marker, deadline)?;
        let mut batch = Batch::default();
        loop {
            let next = self.drain();
            batch.absorb(next);
            if batch.generation != fence.generation {
                batch.loss.get_or_insert(Loss::WatchSetChanged);
            }
            if batch.loss.is_some() || batch.sequence >= fence.sequence {
                return Ok(batch);
            }
            if Instant::now() >= deadline {
                return Err(FenceError::TimedOut);
            }
            std::thread::yield_now();
        }
    }

    fn classify(&self, path: &Path, kind: ChangeKind) -> Option<(Origin, PathBuf, PathBuf)> {
        if let Some(marker) = &self.marker {
            if path.starts_with(marker) {
                return None;
            }
        }
        let git_dir = self
            .coverage
            .git_dirs
            .iter()
            .filter(|git_dir| path.starts_with(git_dir))
            .max_by_key(|git_dir| git_dir.components().count());
        if let Some(git_dir) = git_dir {
            let relative = path.strip_prefix(git_dir).ok()?.to_path_buf();
            return Some((Origin::GitDir, git_dir.clone(), relative));
        }
        let relative = path.strip_prefix(&self.coverage.worktree).ok()?;
        if self.coverage.is_ignored(relative)
            || (kind == ChangeKind::Modify && self.coverage.ignored.contains(relative))
        {
            return None;
        }
        Some((
            Origin::Worktree,
            self.coverage.worktree.clone(),
            relative.to_path_buf(),
        ))
    }
}

fn desired_watches(coverage: &Coverage, marker: Option<&Path>) -> Vec<WatchedPath> {
    let mut watches = Vec::new();
    if RECURSIVE_ROOT {
        watches.push(WatchedPath {
            path: coverage.worktree.clone(),
            recursive: true,
        });
        for git_dir in &coverage.git_dirs {
            if !git_dir.starts_with(&coverage.worktree) {
                watches.push(WatchedPath {
                    path: git_dir.clone(),
                    recursive: true,
                });
            }
        }
    } else {
        for directory in coverage.directories.iter().chain(&coverage.git_dirs) {
            watches.push(WatchedPath {
                path: directory.clone(),
                recursive: false,
            });
        }
    }
    if let Some(marker) = marker {
        watches.push(WatchedPath {
            path: marker.to_path_buf(),
            recursive: false,
        });
    }
    watches.sort_by(|left, right| left.path.cmp(&right.path));
    watches.dedup_by(|right, left| left.path == right.path);
    watches
}

fn fence_directory(options: &FeedOptions) -> Result<PathBuf, Ineligible> {
    let cache = options
        .cache_dir
        .clone()
        .ok_or(Ineligible::NoCacheDirectory)?;
    let directory = cache.join(FENCE_DIRECTORY_NAME);
    let failed = |error: std::io::Error| Ineligible::MarkerDirectory {
        path: directory.clone(),
        cause: error.to_string(),
    };
    paneflow_home::create_private_dir_all(&directory).map_err(failed)?;
    if !std::fs::metadata(&directory).map_err(failed)?.is_dir() {
        return Err(failed(std::io::Error::other("not a directory")));
    }
    std::fs::canonicalize(&directory).map_err(failed)
}

#[cfg(target_os = "linux")]
fn check_watch_budget(needed: usize, watch_limit: Option<usize>) -> Result<(), Ineligible> {
    let limit = watch_limit.unwrap_or_else(eligibility::max_user_watches);
    eligibility::check_budget(needed, inotify_watches_in_use(), limit)
}

#[cfg(not(target_os = "linux"))]
fn check_watch_budget(_: usize, _: Option<usize>) -> Result<(), Ineligible> {
    Ok(())
}
