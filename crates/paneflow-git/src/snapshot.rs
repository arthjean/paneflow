use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gix::bstr::{BString, ByteSlice};
use gix::dir::entry::{Kind as DiskKind, Status as DirStatus};
use gix::dir::walk::EmissionMode;
use gix::index::entry::Mode;
use gix::status::index_worktree::{BuiltinSubmoduleStatus, Item as WorktreeItem};
use gix::status::plumbing::index_as_worktree::traits::FastEq;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
use gix::status::plumbing::index_as_worktree_with_renames::{Entry as VisitedEntry, VisitEntry};
use gix::status::Submodule;
use gix::submodule::config::Ignore;

use crate::submodule::SubmoduleResult;
use crate::{
    open, sparse, staged, submodule, threads, Conflict, Entry, Error, Head, IndexChange, Objects,
    Options, Sides, Snapshot, Stored, Submodules, Untracked, WorktreeChange,
};

pub fn snapshot(path: &Path, options: &Options) -> Result<Snapshot, Error> {
    with_snapshot(path, options, |snapshot, _| Ok(snapshot))
}

pub fn with_snapshot<T>(
    path: &Path,
    options: &Options,
    read: impl FnOnce(Snapshot, &Objects<'_>) -> Result<T, Error>,
) -> Result<T, Error> {
    let interrupt = options.interrupt.clone().unwrap_or_default();
    let lease = threads::PROBES.enter(threads::budget());
    let share = lease.share();
    let result = discover_and_collect(path, options, &interrupt, share)
        .and_then(|(snapshot, repo)| read(snapshot, &Objects::new(&repo)));
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    result
}

pub(crate) fn full(path: &Path, options: &Options) -> Result<(Snapshot, gix::Repository), Error> {
    let interrupt = options.interrupt.clone().unwrap_or_default();
    let lease = threads::PROBES.enter(threads::budget());
    let result = discover_and_collect(path, options, &interrupt, lease.share());
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    result
}

fn discover_and_collect(
    path: &Path,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
) -> Result<(Snapshot, gix::Repository), Error> {
    let repo = open::discover(path, options.threads.before_index(share))?;
    let prefix = prefix(path, &repo)?;
    let snapshot = collect(&repo, options, interrupt, share)?;
    Ok((Snapshot { prefix, ..snapshot }, repo))
}

pub(crate) type Entries = BTreeMap<Vec<u8>, Entry>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Recorded {
    sides: Sides,
    staged: bool,
}

pub(crate) type SideMap = BTreeMap<Vec<u8>, Recorded>;

pub(crate) type StatDirty = BTreeMap<Vec<u8>, Stored>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Worktree {
    pub(crate) tracked: Entries,
    pub(crate) sides: SideMap,
    pub(crate) untracked: BTreeSet<Vec<u8>>,
    pub(crate) refresh: BTreeSet<Vec<u8>>,
}

impl Worktree {
    pub(crate) fn forget_tracked(&mut self, path: &[u8]) {
        self.tracked.remove(path);
        self.sides.remove(path);
        self.refresh.remove(path);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct WorktreeWork {
    pub(crate) entries_processed: usize,
    pub(crate) symlink_metadata_calls: usize,
    pub(crate) files_hashed: usize,
    pub(crate) read_dir_calls: usize,
}

pub(crate) struct Parts<'a> {
    pub(crate) head: Head,
    pub(crate) worktree: &'a Worktree,
    pub(crate) staged: &'a [staged::Staged],
    pub(crate) stat_dirty: Option<&'a StatDirty>,
    pub(crate) submodules: &'a [SubmoduleResult],
    pub(crate) submodules_opened: usize,
}

pub(crate) fn prefix(path: &Path, repo: &gix::Repository) -> Result<Vec<u8>, Error> {
    let workdir = repo.workdir().ok_or_else(|| Error::Bare {
        path: path.to_path_buf(),
    })?;
    let directory = std::fs::canonicalize(path)
        .map_err(|error| Error::git("resolve the probed directory", error))?;
    let workdir = std::fs::canonicalize(workdir)
        .map_err(|error| Error::git("resolve the worktree directory", error))?;
    let relative = directory.strip_prefix(&workdir).map_err(|_| {
        Error::git(
            "locate the probed directory in the worktree",
            format!("{} is outside {}", directory.display(), workdir.display()),
        )
    })?;
    let mut prefix = Vec::new();
    for component in relative.components() {
        let name = gix::path::os_str_into_bstr(component.as_os_str())
            .map_err(|error| Error::git("encode the probed directory", error))?;
        prefix.extend_from_slice(name);
        prefix.push(b'/');
    }
    Ok(prefix)
}

pub(crate) fn collect(
    repo: &gix::Repository,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
) -> Result<Snapshot, Error> {
    let compared_tree = compared_tree(repo, options)?;
    let index = sparse::index_for_status(repo)?;
    let thread_limit = options.threads.for_entries(share, index.entries().len());
    let staged_repo = repo.clone();
    let (worktree, staged) = std::thread::scope(|scope| {
        let index = &index;
        let staged =
            scope.spawn(move || staged::changes(&staged_repo, index, compared_tree, interrupt));
        let worktree = (|| {
            let (worktree, _) =
                worktree_pass(repo, index, options.untracked, thread_limit, interrupt)?;
            Ok(worktree)
        })();
        let staged = staged.join().unwrap_or_else(|_| {
            Err(Error::git(
                "compare the index with its tree",
                "the comparison thread panicked",
            ))
        });
        worktree.and_then(|worktree| staged.map(|staged| (worktree, staged)))
    })?;
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    let stat_dirty = (options.stat_dirty && !worktree.refresh.is_empty())
        .then(|| stat_mismatches(repo, &index, &worktree, None))
        .transpose()?;
    let submodules = match options.submodules {
        Submodules::Ignore => Vec::new(),
        Submodules::Check | Submodules::CheckAll => {
            submodule::evaluate(repo, options, interrupt, share, |_, _| None)?
        }
    };
    let submodules_opened = submodules.iter().map(|result| result.opened).sum();
    Ok(assemble(
        Parts {
            head: head_of(compared_tree),
            worktree: &worktree,
            staged: &staged,
            stat_dirty: stat_dirty.as_ref(),
            submodules: &submodules,
            submodules_opened,
        },
        options,
    ))
}

pub(crate) fn head_of(compared_tree: Option<gix::ObjectId>) -> Head {
    compared_tree.map_or(Head::Unborn, |tree| Head::Tree(tree.to_string()))
}

pub(crate) fn compared_tree(
    repo: &gix::Repository,
    options: &Options,
) -> Result<Option<gix::ObjectId>, Error> {
    match options.base.as_deref() {
        Some(base) => base_tree(repo, base).map(Some),
        None => head_tree(repo),
    }
}

pub(crate) fn assemble(parts: Parts<'_>, options: &Options) -> Snapshot {
    let mut entries = parts.worktree.tracked.clone();
    let mut sides = parts.worktree.sides.clone();
    if options.untracked == Untracked::Files {
        for path in &parts.worktree.untracked {
            entry(&mut entries, path).untracked = true;
        }
    }
    for change in parts.staged {
        record_staged(&mut entries, &mut sides, change);
    }
    if options.stat_dirty && !parts.worktree.refresh.is_empty() {
        if let Some(stat_dirty) = parts.stat_dirty {
            for (path, stored) in stat_dirty {
                if entries.get(path).is_some_and(|entry| {
                    entry.index_to_worktree.is_some() || entry.conflict.is_some()
                }) {
                    continue;
                }
                entry(&mut entries, path).stat_dirty = true;
                let path_sides = side(&mut sides, path);
                if path_sides.index.is_none() {
                    path_sides.index = Some(stored.clone());
                }
            }
        }
    }
    if options.submodules != Submodules::Ignore {
        submodule::apply(parts.submodules, &mut entries, &mut sides);
    }
    for entry in entries.values_mut() {
        if entry.index_to_worktree == Some(WorktreeChange::IntentToAdd) {
            entry.head_to_index = None;
        }
        if entry.conflict.is_some() {
            entry.untracked = false;
        }
    }
    let truncated = entries.len() > options.entry_cap;
    let entries: Vec<Entry> = entries.into_values().take(options.entry_cap).collect();
    let sides = kept_sides(&entries, sides);
    Snapshot {
        head: parts.head,
        prefix: Vec::new(),
        entries,
        sides,
        truncated,
        submodules_opened: parts.submodules_opened,
    }
}

pub(crate) fn worktree_pass(
    repo: &gix::Repository,
    index: &gix::index::State,
    untracked: Untracked,
    thread_limit: usize,
    interrupt: &AtomicBool,
) -> Result<(Worktree, WorktreeWork), Error> {
    let dirwalk_options = match untracked {
        Untracked::Files => Some(
            repo.dirwalk_options()
                .map_err(|error| Error::git("prepare the directory walk", error))?
                .emit_untracked(EmissionMode::Matching),
        ),
        Untracked::None => None,
    };
    let submodule = BuiltinSubmoduleStatus::new(
        repo.clone().into_sync(),
        Submodule::Given {
            ignore: Ignore::All,
            check_dirty: false,
        },
    )
    .map_err(|error| Error::git("prepare the status", error))?;
    let mut visit = Visit::default();
    let outcome = repo
        .index_worktree_status(
            index,
            Vec::<BString>::new(),
            &mut visit,
            FastEq,
            submodule,
            &mut gix::progress::Discard,
            interrupt,
            gix::status::index_worktree::Options {
                sorting: None,
                dirwalk_options,
                rewrites: None,
                thread_limit: Some(thread_limit),
            },
        )
        .map_err(|error| Error::git("compute the status", error))?;
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    let tracked = &outcome.tracked_file_modification;
    let work = WorktreeWork {
        entries_processed: tracked.entries_processed,
        symlink_metadata_calls: tracked.symlink_metadata_calls,
        files_hashed: tracked.worktree_files_read,
        read_dir_calls: outcome
            .dirwalk
            .map_or(0, |walk| walk.read_dir_calls as usize),
    };
    Ok((visit.worktree, work))
}

#[derive(Default)]
struct Visit {
    worktree: Worktree,
}

impl<'index> VisitEntry<'index> for Visit {
    type ContentChange = ();
    type SubmoduleStatus = gix::submodule::Status;

    fn visit_entry(&mut self, entry: VisitedEntry<'index, (), gix::submodule::Status>) {
        record(&mut self.worktree, WorktreeItem::from(entry));
    }
}

pub(crate) fn record_dirwalk_entry(worktree: &mut Worktree, dir: &gix::dir::Entry) {
    if dir.status != DirStatus::Untracked {
        return;
    }
    let mut path = dir.rela_path.to_vec();
    if matches!(
        dir.disk_kind,
        Some(DiskKind::Directory | DiskKind::Repository)
    ) {
        path.push(b'/');
    }
    worktree.untracked.insert(path);
}

fn kept_sides(entries: &[Entry], mut recorded: SideMap) -> BTreeMap<Vec<u8>, Sides> {
    let mut kept = BTreeMap::new();
    for entry in entries {
        let Some(Recorded { mut sides, staged }) = recorded.remove(&entry.path) else {
            continue;
        };
        if !staged && entry.index_to_worktree != Some(WorktreeChange::IntentToAdd) {
            sides.head = sides.index.clone();
        }
        kept.insert(entry.path.clone(), sides);
    }
    kept
}

pub(crate) fn side<'a>(sides: &'a mut SideMap, path: &[u8]) -> &'a mut Sides {
    &mut sides.entry(path.to_vec()).or_default().sides
}

fn stored(mode: Mode, id: &gix::oid) -> Stored {
    Stored {
        mode: mode.bits(),
        id: id.to_string(),
    }
}

pub(crate) fn gitlink(id: &gix::oid) -> Stored {
    stored(Mode::COMMIT, id)
}

fn worktree_mode(
    index_mode: Mode,
    status: &EntryStatus<(), gix::submodule::Status>,
) -> Option<u32> {
    match status {
        EntryStatus::Change(Change::Type { worktree_mode }) => Some(worktree_mode.bits()),
        EntryStatus::Change(Change::Modification {
            executable_bit_changed: true,
            ..
        }) => Some(match index_mode {
            Mode::FILE => Mode::FILE_EXECUTABLE.bits(),
            Mode::FILE_EXECUTABLE => Mode::FILE.bits(),
            other => other.bits(),
        }),
        EntryStatus::Change(Change::Modification { .. }) | EntryStatus::IntentToAdd => {
            Some(index_mode.bits())
        }
        _ => None,
    }
}

fn base_tree(repo: &gix::Repository, base: &str) -> Result<gix::ObjectId, Error> {
    let id = gix::ObjectId::from_hex(base.as_bytes())
        .map_err(|error| Error::git("parse the base object id", error))?;
    if id == gix::ObjectId::empty_tree(id.kind()) {
        return Ok(id);
    }
    let tree = repo
        .find_object(id)
        .map_err(|error| Error::git("read the base object", error))?
        .peel_to_tree()
        .map_err(|error| Error::git("read the base tree", error))?;
    Ok(tree.id)
}

fn head_tree(repo: &gix::Repository) -> Result<Option<gix::ObjectId>, Error> {
    let head = repo
        .head()
        .map_err(|error| Error::git("read HEAD", error))?;
    if head.is_unborn() {
        return Ok(None);
    }
    let tree = repo
        .head_tree_id()
        .map_err(|error| Error::git("read the HEAD tree", error))?;
    Ok(Some(tree.detach()))
}

pub(crate) fn entry<'a>(entries: &'a mut Entries, path: &[u8]) -> &'a mut Entry {
    entries.entry(path.to_vec()).or_insert_with(|| Entry {
        path: path.to_vec(),
        ..Entry::default()
    })
}

pub(crate) fn stat_mismatches(
    repo: &gix::Repository,
    index: &gix::index::State,
    worktree: &Worktree,
    only: Option<&BTreeSet<Vec<u8>>>,
) -> Result<StatDirty, Error> {
    use gix::index::entry::{Flags, Stage};
    let stat_options = repo
        .stat_options()
        .map_err(|error| Error::git("read the stat options", error))?;
    let mut mismatches = StatDirty::new();
    let Some(workdir) = repo.workdir() else {
        return Ok(mismatches);
    };
    for index_entry in index.entries() {
        if index_entry.stage() != Stage::Unconflicted
            || index_entry.mode == Mode::COMMIT
            || index_entry
                .flags
                .intersects(Flags::INTENT_TO_ADD | Flags::SKIP_WORKTREE)
        {
            continue;
        }
        let path: &[u8] = index_entry.path(index).as_ref();
        if only.is_some_and(|only| !only.contains(path)) {
            continue;
        }
        if worktree
            .tracked
            .get(path)
            .is_some_and(|entry| entry.index_to_worktree.is_some() || entry.conflict.is_some())
        {
            continue;
        }
        let Ok(relative) = gix::path::from_bstr(path.as_bstr()) else {
            continue;
        };
        let Ok(metadata) = gix::index::fs::Metadata::from_path_no_follow(&workdir.join(relative))
        else {
            continue;
        };
        let Ok(stat) = gix::index::entry::Stat::from_fs(&metadata) else {
            continue;
        };
        if stat.matches(&index_entry.stat, stat_options) {
            continue;
        }
        mismatches.insert(path.to_vec(), stored(index_entry.mode, &index_entry.id));
    }
    Ok(mismatches)
}

fn record(worktree: &mut Worktree, item: WorktreeItem) {
    match item {
        WorktreeItem::Modification {
            entry: index_entry,
            rela_path,
            status,
            ..
        } => {
            if matches!(
                status,
                EntryStatus::NeedsUpdate(_)
                    | EntryStatus::Change(Change::Modification {
                        set_entry_stat_size_zero: true,
                        ..
                    })
            ) {
                worktree.refresh.insert(rela_path.to_vec());
            }
            if matches!(status, EntryStatus::NeedsUpdate(_)) {
                return;
            }
            let path_sides = side(&mut worktree.sides, &rela_path);
            if path_sides.index.is_none() {
                path_sides.index = Some(stored(index_entry.mode, &index_entry.id));
            }
            if let Some(mode) = worktree_mode(index_entry.mode, &status) {
                path_sides.worktree_mode = Some(mode);
            }
            record_worktree_status(&mut worktree.tracked, &rela_path, status);
        }
        WorktreeItem::DirectoryContents { entry: dir, .. } => {
            record_dirwalk_entry(worktree, &dir);
        }
        WorktreeItem::Rewrite { .. } => {}
    }
}

fn record_worktree_status(
    entries: &mut Entries,
    path: &[u8],
    status: EntryStatus<(), gix::submodule::Status>,
) {
    let change = match status {
        EntryStatus::Conflict { summary, .. } => {
            entry(entries, path).conflict = Some(conflict(summary));
            return;
        }
        EntryStatus::NeedsUpdate(_) => return,
        EntryStatus::IntentToAdd => WorktreeChange::IntentToAdd,
        EntryStatus::Change(Change::Removed) => WorktreeChange::Deleted,
        EntryStatus::Change(Change::Type { .. }) => WorktreeChange::TypeChange,
        EntryStatus::Change(Change::Modification { .. }) => WorktreeChange::Modified,
        EntryStatus::Change(Change::SubmoduleModification(_)) => return,
    };
    entry(entries, path).index_to_worktree = Some(change);
}

fn record_staged(entries: &mut Entries, sides: &mut SideMap, change: &staged::Staged) {
    let kind = match (change.head, change.index) {
        (None, _) => IndexChange::Added,
        (_, None) => IndexChange::Deleted,
        (Some((head, _)), Some((index, _))) if object_kind(head) == object_kind(index) => {
            IndexChange::Modified
        }
        (Some(_), Some(_)) => IndexChange::TypeChange,
    };
    entry(entries, &change.path).head_to_index = Some(kind);
    let recorded = sides.entry(change.path.to_vec()).or_default();
    recorded.staged = true;
    recorded.sides.head = change.head.map(|(mode, id)| stored(mode, &id));
    recorded.sides.index = change.index.map(|(mode, id)| stored(mode, &id));
}

#[derive(PartialEq, Eq)]
enum ObjectKind {
    Blob,
    Link,
    Commit,
    Tree,
}

fn object_kind(mode: Mode) -> ObjectKind {
    if mode == Mode::SYMLINK {
        ObjectKind::Link
    } else if mode == Mode::COMMIT {
        ObjectKind::Commit
    } else if mode == Mode::DIR {
        ObjectKind::Tree
    } else {
        ObjectKind::Blob
    }
}

fn conflict(summary: gix::status::plumbing::index_as_worktree::Conflict) -> Conflict {
    use gix::status::plumbing::index_as_worktree::Conflict as Gix;
    match summary {
        Gix::BothDeleted => Conflict::BothDeleted,
        Gix::AddedByUs => Conflict::AddedByUs,
        Gix::DeletedByThem => Conflict::DeletedByThem,
        Gix::AddedByThem => Conflict::AddedByThem,
        Gix::DeletedByUs => Conflict::DeletedByUs,
        Gix::BothAdded => Conflict::BothAdded,
        Gix::BothModified => Conflict::BothModified,
    }
}
