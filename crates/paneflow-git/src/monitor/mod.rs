mod carry;
mod dirty;
mod identity;
mod setting;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gix::bstr::{BString, ByteSlice};
use gix::dir::walk::EmissionMode;
use gix::worktree::IndexPersistedOrInMemory;
use gix::ObjectId;

pub use identity::Input;
pub use setting::{
    monitor_enabled, parse_monitor_setting, MonitorSetting, ACCEPTED_MONITOR_VALUES, MONITOR_ENV,
};

use crate::feed::{ChangeFeed, FeedOptions, FenceError, Ineligible, Loss};
use crate::snapshot::{
    assemble, compared_tree, full, head_of, prefix, record_dirwalk_entry, stat_mismatches,
    worktree_pass, Parts, StatDirty, Worktree, WorktreeWork,
};
use crate::sparse::{self, IndexFormat};
use crate::staged::{self, Staged};
use crate::submodule::{self, SubmoduleResult};
use crate::{open, threads, Error, Objects, Options, Snapshot, Submodules, Untracked};
use carry::{carry, Carried, Refused};
use dirty::{Pending, Plan, Planner, Unplannable};
use identity::{IndexStamp, Inputs};

pub const DIRTY_WALK_LIMIT: usize = 256;

pub const DIRTY_RECHECK_FLOOR: usize = 1024;

const STAGED_CACHE_LIMIT: usize = 4;

pub const FENCE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fenced,
    Converging,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    FirstProbe,
    Loss(Loss),
    Fence(FenceError),
    Input(Input),
    IndexFormat,
    IndexCarryLimit,
    SkipWorktree,
    DirtyLimit,
    WorktreeRoot,
    Coverage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeKind {
    Incremental,
    Baseline(Reason),
    Full(Ineligible),
    WorktreeDependentIndex,
    MonitorError(String),
    Disabled,
}

impl ProbeKind {
    pub fn fell_back(&self) -> bool {
        matches!(
            self,
            ProbeKind::Full(_) | ProbeKind::WorktreeDependentIndex | ProbeKind::MonitorError(_)
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeReport {
    pub kind: ProbeKind,
    pub rechecked_entries: usize,
    pub walked_paths: usize,
    pub files_hashed: usize,
    pub symlink_metadata_calls: usize,
    pub read_dir_calls: usize,
    pub index_read: bool,
    pub staged_recomputed: bool,
    pub submodules_opened: usize,
}

impl Default for ProbeReport {
    fn default() -> Self {
        Self {
            kind: ProbeKind::Baseline(Reason::FirstProbe),
            rechecked_entries: 0,
            walked_paths: 0,
            files_hashed: 0,
            symlink_metadata_calls: 0,
            read_dir_calls: 0,
            index_read: false,
            staged_recomputed: false,
            submodules_opened: 0,
        }
    }
}

type Waker = Arc<dyn Fn() + Send + Sync>;

enum FeedState {
    Idle,
    Live(Box<ChangeFeed>),
    Refused(Ineligible),
}

#[derive(Clone)]
struct Model {
    index: Arc<IndexPersistedOrInMemory>,
    index_stamp: IndexStamp,
    format: IndexFormat,
    inputs: Inputs,
    worktree: Worktree,
    staged: BTreeMap<Option<ObjectId>, Arc<Vec<Staged>>>,
    stat: Option<StatDirty>,
    submodules: BTreeMap<SubmodulesKey, BTreeMap<Vec<u8>, SubmoduleResult>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SubmodulesKey {
    Check,
    CheckAll,
}

impl SubmodulesKey {
    fn of(submodules: Submodules) -> Option<Self> {
        match submodules {
            Submodules::Ignore => None,
            Submodules::Check => Some(Self::Check),
            Submodules::CheckAll => Some(Self::CheckAll),
        }
    }
}

enum Step {
    Done(Snapshot, Box<gix::Repository>),
    Rebaseline(Reason),
}

enum Answer {
    Modeled(Snapshot, Box<gix::Repository>),
    Full,
}

pub struct Probed {
    snapshot: Snapshot,
    repo: Box<gix::Repository>,
}

impl std::fmt::Debug for Probed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Probed")
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

impl Probed {
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }

    pub fn into_snapshot(self) -> Snapshot {
        self.snapshot
    }

    pub fn read<T>(
        self,
        read: impl FnOnce(Snapshot, &Objects<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        read(self.snapshot, &Objects::new(&self.repo))
    }
}

pub fn worktree_root(path: &Path) -> Option<PathBuf> {
    let (location, _trust) = gix::discover::upwards(path).ok()?;
    let (_git_dir, work_dir) = location.into_repository_and_work_tree_directories();
    std::path::absolute(work_dir?).ok()
}

pub struct Monitor {
    path: PathBuf,
    feed_options: FeedOptions,
    feed: FeedState,
    waker: Option<Waker>,
    own_git_dirs: BTreeSet<PathBuf>,
    model: Option<Model>,
    pending: Pending,
    report: ProbeReport,
    enabled: bool,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("path", &self.path)
            .field("baselined", &self.model.is_some())
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl Monitor {
    pub fn new(path: &Path, feed_options: FeedOptions) -> Self {
        Self::configured(path, feed_options, monitor_enabled())
    }

    pub fn configured(path: &Path, feed_options: FeedOptions, enabled: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            feed_options,
            feed: FeedState::Idle,
            waker: None,
            own_git_dirs: BTreeSet::new(),
            model: None,
            pending: Pending::default(),
            report: ProbeReport::default(),
            enabled,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn last_probe(&self) -> &ProbeReport {
        &self.report
    }

    pub fn feed(&self) -> Option<&ChangeFeed> {
        match &self.feed {
            FeedState::Live(feed) => Some(feed),
            FeedState::Idle | FeedState::Refused(_) => None,
        }
    }

    pub fn ineligible(&self) -> Option<&Ineligible> {
        match &self.feed {
            FeedState::Refused(reason) => Some(reason),
            FeedState::Idle | FeedState::Live(_) => None,
        }
    }

    pub fn set_waker(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        let wake: Waker = Arc::new(wake);
        if let FeedState::Live(feed) = &mut self.feed {
            let installed = Arc::clone(&wake);
            feed.set_waker(move || installed());
        }
        self.waker = Some(wake);
    }

    pub fn snapshot(&mut self, options: &Options) -> Result<Snapshot, Error> {
        let path = self.path.clone();
        self.run(&path, options, None).map(Probed::into_snapshot)
    }

    pub fn fenced_snapshot(
        &mut self,
        options: &Options,
        deadline: Instant,
    ) -> Result<Snapshot, Error> {
        let path = self.path.clone();
        self.run(&path, options, Some(deadline))
            .map(Probed::into_snapshot)
    }

    pub fn probe(
        &mut self,
        cwd: &Path,
        options: &Options,
        freshness: Freshness,
    ) -> Result<Probed, Error> {
        let deadline = match freshness {
            Freshness::Fenced => Some(Instant::now() + FENCE_TIMEOUT),
            Freshness::Converging => None,
        };
        self.run(cwd, options, deadline)
    }

    fn run(
        &mut self,
        cwd: &Path,
        options: &Options,
        deadline: Option<Instant>,
    ) -> Result<Probed, Error> {
        if !self.enabled {
            self.report = ProbeReport {
                kind: ProbeKind::Disabled,
                ..ProbeReport::default()
            };
            return full_at(cwd, options);
        }
        let interrupted = options.interrupt.clone().unwrap_or_default();
        let result = self.probe_once(options, deadline);
        if interrupted.load(Ordering::Relaxed) {
            return Err(Error::Interrupted);
        }
        let modeled = result.and_then(|answer| match answer {
            Answer::Modeled(snapshot, repo) => {
                let prefix = prefix(cwd, &repo)?;
                Ok(Some(Probed {
                    snapshot: Snapshot { prefix, ..snapshot },
                    repo,
                }))
            }
            Answer::Full => Ok(None),
        });
        match modeled {
            Ok(Some(probed)) => Ok(probed),
            Ok(None) => full_at(cwd, options),
            Err(Error::Interrupted) => Err(Error::Interrupted),
            Err(error) => {
                log::warn!(
                    "the incremental git status of {} failed, this probe takes a full snapshot and the monitor restarts: {error}",
                    self.path.display()
                );
                self.reset();
                self.report = ProbeReport {
                    kind: ProbeKind::MonitorError(error.to_string()),
                    ..ProbeReport::default()
                };
                full_at(cwd, options)
            }
        }
    }

    fn reset(&mut self) {
        self.model = None;
        self.pending = Pending::default();
        self.feed = FeedState::Idle;
    }

    fn probe_once(
        &mut self,
        options: &Options,
        deadline: Option<Instant>,
    ) -> Result<Answer, Error> {
        self.start_feed();
        let feed = match &mut self.feed {
            FeedState::Live(feed) => feed,
            FeedState::Refused(reason) => {
                self.report = ProbeReport {
                    kind: ProbeKind::Full(reason.clone()),
                    ..ProbeReport::default()
                };
                return Ok(Answer::Full);
            }
            FeedState::Idle => return Ok(Answer::Full),
        };
        let fence_error = match deadline.map(|deadline| feed.fence(deadline)) {
            Some(Ok(batch)) => {
                self.pending.absorb(batch, &self.own_git_dirs);
                None
            }
            Some(Err(error)) => {
                self.pending.absorb(feed.drain(), &self.own_git_dirs);
                Some(error)
            }
            None => {
                self.pending.absorb(feed.drain(), &self.own_git_dirs);
                None
            }
        };
        let forced = self
            .model
            .is_none()
            .then_some(Reason::FirstProbe)
            .or(fence_error.map(Reason::Fence))
            .or(self.pending.loss.map(Reason::Loss));
        if let Some(reason) = forced {
            return self.baseline(options, reason);
        }
        match self.incremental(options)? {
            Step::Done(snapshot, repo) => Ok(Answer::Modeled(snapshot, repo)),
            Step::Rebaseline(reason) => self.baseline(options, reason),
        }
    }

    fn start_feed(&mut self) {
        if !matches!(self.feed, FeedState::Idle) {
            return;
        }
        self.model = None;
        self.feed = match ChangeFeed::start(&self.path, &self.feed_options) {
            Ok(mut feed) => {
                if let Some(wake) = &self.waker {
                    let wake = Arc::clone(wake);
                    feed.set_waker(move || wake());
                }
                FeedState::Live(Box::new(feed))
            }
            Err(reason) => FeedState::Refused(reason),
        };
    }

    fn baseline(&mut self, options: &Options, reason: Reason) -> Result<Answer, Error> {
        self.pending = Pending::default();
        self.model = None;
        let interrupt = options.interrupt.clone().unwrap_or_default();
        let lease = threads::PROBES.enter(threads::budget());
        let share = lease.share();
        let (repo, config) =
            open::discover_with_config(&self.path, options.threads.before_index(share))?;
        let probe_prefix = prefix(&self.path, &repo)?;
        let read = sparse::read_index(&repo)?;
        if read.depends_on_worktree {
            drop(lease);
            self.report = ProbeReport {
                kind: ProbeKind::WorktreeDependentIndex,
                ..ProbeReport::default()
            };
            return Ok(Answer::Full);
        }
        self.own_git_dirs = own_git_dirs(&repo);
        let index_stamp = IndexStamp::read(read.index.path());
        let index = Arc::new(read.index);
        let tree = compared_tree(&repo, options)?;
        let thread_limit = options.threads.for_entries(share, index.entries().len());
        let staged_repo = repo.clone();
        let (worktree, work, staged) = std::thread::scope(|scope| {
            let staged_index = Arc::clone(&index);
            let staged_interrupt = Arc::clone(&interrupt);
            let staged = scope.spawn(move || {
                staged::changes(&staged_repo, &staged_index, tree, &staged_interrupt)
            });
            let worktree = worktree_pass(&repo, &index, Untracked::Files, thread_limit, &interrupt);
            let staged = staged.join().unwrap_or_else(|_| {
                Err(Error::git(
                    "compare the index with its tree",
                    "the comparison thread panicked",
                ))
            });
            worktree.and_then(|(worktree, work)| staged.map(|staged| (worktree, work, staged)))
        })?;
        let stat = (options.stat_dirty && !worktree.refresh.is_empty())
            .then(|| stat_mismatches(&repo, &index, &worktree, None))
            .transpose()?;
        let mut model = Model {
            inputs: Inputs::read(&repo, config, &index, &worktree),
            index_stamp,
            format: read.format,
            index,
            worktree,
            staged: BTreeMap::from([(tree, Arc::new(staged))]),
            stat,
            submodules: BTreeMap::new(),
        };
        let submodules = evaluate_submodules(&repo, options, &interrupt, share, &model)?;
        if interrupt.load(Ordering::Relaxed) {
            return Err(Error::Interrupted);
        }
        let snapshot = finish(&mut model, &submodules, options, tree, probe_prefix);
        self.report = ProbeReport {
            kind: ProbeKind::Baseline(reason),
            rechecked_entries: work.entries_processed,
            walked_paths: 0,
            files_hashed: work.files_hashed,
            symlink_metadata_calls: work.symlink_metadata_calls,
            read_dir_calls: work.read_dir_calls,
            index_read: true,
            staged_recomputed: true,
            submodules_opened: snapshot.submodules_opened,
        };
        self.model = Some(model);
        Ok(Answer::Modeled(snapshot, Box::new(repo)))
    }

    fn incremental(&mut self, options: &Options) -> Result<Step, Error> {
        let Some(model) = self.model.as_ref() else {
            return Ok(Step::Rebaseline(Reason::FirstProbe));
        };
        let interrupt = options.interrupt.clone().unwrap_or_default();
        let lease = threads::PROBES.enter(threads::budget());
        let share = lease.share();
        let (repo, config) =
            open::discover_with_config(&self.path, options.threads.before_index(share))?;
        let probe_prefix = prefix(&self.path, &repo)?;
        if let Some(input) = model.inputs.changed(&config) {
            refresh_coverage(&mut self.feed);
            return Ok(Step::Rebaseline(Reason::Input(input)));
        }
        let index_stamp = IndexStamp::read(model.index.path());
        let mut index_read = false;
        let unchanged = index_stamp == model.index_stamp
            && (index_stamp.vouches_for_content() || !self.pending.index_written);
        let (index, carried, format) = if unchanged {
            (Arc::clone(&model.index), Carried::default(), model.format)
        } else {
            index_read = true;
            let read = sparse::read_index(&repo)?;
            if read.depends_on_worktree || read.format != model.format {
                return Ok(Step::Rebaseline(Reason::IndexFormat));
            }
            match carry(&model.index, &read.index) {
                Ok(carried) => (Arc::new(read.index), carried, read.format),
                Err(Refused::SkipWorktree) => return Ok(Step::Rebaseline(Reason::SkipWorktree)),
                Err(Refused::TooManyChanges) => {
                    return Ok(Step::Rebaseline(Reason::IndexCarryLimit))
                }
            }
        };
        if self.newly_tracked_under_ignored(&carried) {
            refresh_coverage(&mut self.feed);
            return Ok(Step::Rebaseline(Reason::Coverage));
        }
        let workdir = repo
            .workdir()
            .ok_or_else(|| Error::Bare {
                path: self.path.clone(),
            })?
            .to_path_buf();
        let ignore_case = repo
            .filesystem_options()
            .map_err(|error| Error::git("read the file system options", error))?
            .ignore_case;
        let planner = Planner {
            index: &index,
            worktree: &model.worktree,
            workdir: &workdir,
            ignore_case,
        };
        let plan = match planner.plan(&self.pending, &carried) {
            Ok(plan) => plan,
            Err(Unplannable::WorktreeRoot) => return Ok(Step::Rebaseline(Reason::WorktreeRoot)),
            Err(Unplannable::InputFile(input)) => {
                refresh_coverage(&mut self.feed);
                return Ok(Step::Rebaseline(Reason::Input(input)));
            }
        };
        let recheck_limit =
            (index.entries().len() * carry::CARRY_LIMIT_PERCENT / 100).max(DIRTY_RECHECK_FLOOR);
        if plan.walk.len() > DIRTY_WALK_LIMIT || plan.recheck.len() > recheck_limit {
            return Ok(Step::Rebaseline(Reason::DirtyLimit));
        }
        let tree = compared_tree(&repo, options)?;
        let mut next = model.clone();
        next.index = Arc::clone(&index);
        next.index_stamp = index_stamp;
        next.format = format;
        if index_read {
            next.staged.clear();
        }
        let work = recheck(&repo, &index, &plan, &mut next.worktree, &interrupt)?;
        let walked = walk_untracked(&repo, &index, &plan.walk, &interrupt, &mut next.worktree)?;
        let staged_recomputed = !next.staged.contains_key(&tree);
        if staged_recomputed {
            if next.staged.len() >= STAGED_CACHE_LIMIT {
                next.staged.clear();
            }
            let staged = staged::changes(&repo, &index, tree, &interrupt)?;
            next.staged.insert(tree, Arc::new(staged));
        }
        update_stat(&repo, &index, &plan, options, &mut next)?;
        forget_submodules(&plan, &mut next);
        let submodules = evaluate_submodules(&repo, options, &interrupt, share, &next)?;
        if interrupt.load(Ordering::Relaxed) {
            return Err(Error::Interrupted);
        }
        let snapshot = finish(&mut next, &submodules, options, tree, probe_prefix);
        self.report = ProbeReport {
            kind: ProbeKind::Incremental,
            rechecked_entries: work.entries_processed,
            walked_paths: plan.walk.len(),
            files_hashed: work.files_hashed,
            symlink_metadata_calls: work.symlink_metadata_calls,
            read_dir_calls: work.read_dir_calls + walked,
            index_read,
            staged_recomputed,
            submodules_opened: snapshot.submodules_opened,
        };
        self.model = Some(next);
        self.pending = Pending::default();
        Ok(Step::Done(snapshot, Box::new(repo)))
    }

    fn newly_tracked_under_ignored(&self, carried: &Carried) -> bool {
        let FeedState::Live(feed) = &self.feed else {
            return false;
        };
        let coverage = feed.coverage();
        carried.added_or_removed.iter().any(|path| {
            gix::path::from_byte_slice(path).is_ok_and(|relative| {
                coverage.is_ignored(relative) || coverage.ignored.contains(relative)
            })
        })
    }
}

fn full_at(cwd: &Path, options: &Options) -> Result<Probed, Error> {
    full(cwd, options).map(|(snapshot, repo)| Probed {
        snapshot,
        repo: Box::new(repo),
    })
}

fn refresh_coverage(feed: &mut FeedState) {
    if let FeedState::Live(feed) = feed {
        feed.invalidate_coverage();
        drop(feed.drain());
    }
}

fn own_git_dirs(repo: &gix::Repository) -> BTreeSet<PathBuf> {
    [repo.git_dir(), repo.common_dir()]
        .into_iter()
        .filter_map(|dir| std::fs::canonicalize(dir).ok())
        .collect()
}

fn sub_index(index: &gix::index::State, paths: &BTreeSet<Vec<u8>>) -> gix::index::State {
    let mut sub = gix::index::State::new(index.object_hash());
    sub.set_timestamp(index.timestamp());
    for path in paths {
        let Some(range) = index.entry_range(path.as_bstr()) else {
            continue;
        };
        for entry in &index.entries()[range] {
            sub.dangerously_push_entry(
                entry.stat,
                entry.id,
                entry.flags,
                entry.mode,
                entry.path(index),
            );
        }
    }
    sub.sort_entries();
    sub
}

fn recheck(
    repo: &gix::Repository,
    index: &gix::index::State,
    plan: &Plan,
    worktree: &mut Worktree,
    interrupt: &AtomicBool,
) -> Result<WorktreeWork, Error> {
    for path in &plan.forget {
        worktree.forget_tracked(path);
    }
    if plan.recheck.is_empty() {
        return Ok(WorktreeWork::default());
    }
    let sub = sub_index(index, &plan.recheck);
    let (partial, work) = worktree_pass(repo, &sub, Untracked::None, 1, interrupt)?;
    worktree.tracked.extend(partial.tracked);
    worktree.sides.extend(partial.sides);
    worktree.refresh.extend(partial.refresh);
    Ok(work)
}

struct UntrackedDelegate<'a> {
    worktree: &'a mut Worktree,
    interrupt: &'a AtomicBool,
}

impl gix::dir::walk::Delegate for UntrackedDelegate<'_> {
    fn emit(
        &mut self,
        entry: gix::dir::EntryRef<'_>,
        _collapsed_directory_status: Option<gix::dir::entry::Status>,
    ) -> gix::dir::walk::Action {
        if entry.disk_kind != Some(gix::dir::entry::Kind::Untrackable) {
            record_dirwalk_entry(self.worktree, &entry.to_owned());
        }
        if self.interrupt.load(Ordering::Relaxed) {
            std::ops::ControlFlow::Break(())
        } else {
            std::ops::ControlFlow::Continue(())
        }
    }
}

fn walk_untracked(
    repo: &gix::Repository,
    index: &gix::index::State,
    roots: &BTreeSet<Vec<u8>>,
    interrupt: &AtomicBool,
    worktree: &mut Worktree,
) -> Result<usize, Error> {
    if roots.is_empty() {
        return Ok(0);
    }
    for root in roots {
        forget_untracked(worktree, root);
    }
    let patterns: Vec<BString> = roots.iter().map(|root| literal_pathspec(root)).collect();
    let options = repo
        .dirwalk_options()
        .map_err(|error| Error::git("prepare the directory walk", error))?
        .emit_untracked(EmissionMode::Matching);
    let mut delegate = UntrackedDelegate {
        worktree,
        interrupt,
    };
    let outcome = repo
        .dirwalk(index, patterns, interrupt, options, &mut delegate)
        .map_err(|error| Error::git("walk the changed directories", error))?;
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    Ok(outcome.dirwalk.read_dir_calls as usize)
}

pub(crate) fn literal_pathspec(path: &[u8]) -> BString {
    let mut pattern = BString::from(":(top,literal)");
    pattern.extend_from_slice(path);
    pattern
}

fn forget_untracked(worktree: &mut Worktree, root: &[u8]) {
    let mut directory = root.to_vec();
    directory.push(b'/');
    worktree.untracked.remove(root);
    let below: Vec<Vec<u8>> = worktree
        .untracked
        .range(directory.clone()..)
        .take_while(|path| path.starts_with(&directory))
        .cloned()
        .collect();
    for path in below {
        worktree.untracked.remove(&path);
    }
}

fn update_stat(
    repo: &gix::Repository,
    index: &gix::index::State,
    plan: &Plan,
    options: &Options,
    model: &mut Model,
) -> Result<(), Error> {
    if let Some(stat) = model.stat.as_mut() {
        for path in plan.stat.iter().chain(&plan.forget) {
            stat.remove(path);
        }
        let mut refreshed = plan.stat.clone();
        refreshed.extend(plan.forget.iter().cloned());
        stat.extend(stat_mismatches(
            repo,
            index,
            &model.worktree,
            Some(&refreshed),
        )?);
        return Ok(());
    }
    if options.stat_dirty && !model.worktree.refresh.is_empty() {
        model.stat = Some(stat_mismatches(repo, index, &model.worktree, None)?);
    }
    Ok(())
}

fn forget_submodules(plan: &Plan, model: &mut Model) {
    for results in model.submodules.values_mut() {
        if plan.all_submodules {
            results.clear();
            continue;
        }
        for path in &plan.submodules {
            results.remove(path);
        }
    }
}

fn evaluate_submodules(
    repo: &gix::Repository,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
    model: &Model,
) -> Result<Vec<SubmoduleResult>, Error> {
    let Some(key) = SubmodulesKey::of(options.submodules) else {
        return Ok(Vec::new());
    };
    let cached = model.submodules.get(&key);
    submodule::evaluate(repo, options, interrupt, share, |path, _| {
        cached.and_then(|cached| cached.get(path)).cloned()
    })
}

fn finish(
    model: &mut Model,
    submodules: &[SubmoduleResult],
    options: &Options,
    tree: Option<ObjectId>,
    probe_prefix: Vec<u8>,
) -> Snapshot {
    if let Some(key) = SubmodulesKey::of(options.submodules) {
        model.submodules.insert(
            key,
            submodules
                .iter()
                .map(|result| (result.path.clone(), result.clone()))
                .collect(),
        );
    }
    let staged = model.staged.get(&tree).cloned().unwrap_or_default();
    let snapshot = assemble(
        Parts {
            head: head_of(tree),
            worktree: &model.worktree,
            staged: &staged,
            stat_dirty: model.stat.as_ref(),
            submodules,
            submodules_opened: submodules.iter().map(|result| result.opened).sum(),
        },
        options,
    );
    Snapshot {
        prefix: probe_prefix,
        ..snapshot
    }
}
