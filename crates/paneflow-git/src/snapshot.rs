use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gix::bstr::{BString, ByteSlice};
use gix::dir::entry::{Kind as DiskKind, Status as DirStatus};
use gix::index::entry::Mode;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
use gix::status::tree_index::TrackRenames;
use gix::status::{Item, Submodule, UntrackedFiles};
use gix::submodule::config::Ignore;

use crate::{
    open, sparse, submodule, threads, Conflict, Entry, Error, Head, IndexChange, Objects, Options,
    Sides, Snapshot, Stored, Submodules, Untracked, WorktreeChange,
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
    let result = open::discover(path, options.threads.before_index(share)).and_then(|repo| {
        let prefix = prefix(path, &repo)?;
        let snapshot = collect(&repo, options, &interrupt, share)?;
        read(Snapshot { prefix, ..snapshot }, &Objects::new(&repo))
    });
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    result
}

pub(crate) type Entries = BTreeMap<Vec<u8>, Entry>;

#[derive(Default)]
pub(crate) struct Recorded {
    sides: Sides,
    staged: bool,
}

pub(crate) type SideMap = BTreeMap<Vec<u8>, Recorded>;

fn prefix(path: &Path, repo: &gix::Repository) -> Result<Vec<u8>, Error> {
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
    let base_tree = options
        .base
        .as_deref()
        .map(|base| base_tree(repo, base))
        .transpose()?;
    let compared_tree = match base_tree {
        Some(tree) => Some(tree),
        None => head_tree(repo)?,
    };
    let head = compared_tree.map_or(Head::Unborn, |tree| Head::Tree(tree.to_string()));
    let stat_options = repo
        .stat_options()
        .map_err(|error| Error::git("read the stat options", error))?;
    let mut entries = Entries::new();
    let mut sides = SideMap::new();
    let mut status = repo
        .status(gix::progress::Discard)
        .map_err(|error| Error::git("prepare the status", error))?;
    if let Some(tree) = base_tree {
        status = status.head_tree(tree);
    }
    let dirwalk = repo
        .dirwalk_options()
        .map_err(|error| Error::git("prepare the directory walk", error))?;
    let index = sparse::index_for_status(repo)?;
    let thread_limit = options.threads.for_entries(share, index.entries().len());
    let nothing_staged = compared_tree.is_some_and(|tree| index_matches_tree(&index, tree));
    let status = status
        .index(index)
        .index_worktree_options_mut(|worktree| worktree.dirwalk_options = Some(dirwalk))
        .untracked_files(match options.untracked {
            Untracked::Files => UntrackedFiles::Files,
            Untracked::None => UntrackedFiles::None,
        })
        .index_worktree_submodules(Submodule::Given {
            ignore: Ignore::All,
            check_dirty: false,
        })
        .index_worktree_rewrites(None)
        .tree_index_track_renames(TrackRenames::Disabled)
        .index_worktree_options_mut(|worktree| worktree.thread_limit = Some(thread_limit))
        .should_interrupt_owned(Arc::clone(interrupt));
    let outcome = if nothing_staged {
        let mut items = status
            .into_index_worktree_iter(Vec::<BString>::new())
            .map_err(|error| Error::git("start the status", error))?;
        record_all(
            items.by_ref().map(|item| item.map(Item::IndexWorktree)),
            &mut entries,
            &mut sides,
        )?;
        items.into_outcome()
    } else {
        let mut items = status
            .into_iter(Vec::<BString>::new())
            .map_err(|error| Error::git("start the status", error))?;
        record_all(items.by_ref(), &mut entries, &mut sides)?;
        items.into_outcome()
    };
    if interrupt.load(Ordering::Relaxed) {
        return Err(Error::Interrupted);
    }
    if options.stat_dirty {
        if let Some(outcome) = outcome.filter(|outcome| outcome.has_changes()) {
            record_stat_dirty(
                repo,
                &outcome.worktree_index,
                stat_options,
                &mut entries,
                &mut sides,
            );
        }
    }
    let submodules_opened = match options.submodules {
        Submodules::Ignore => 0,
        Submodules::Check | Submodules::CheckAll => {
            submodule::record(repo, options, interrupt, share, &mut entries, &mut sides)?
        }
    };
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
    Ok(Snapshot {
        head,
        prefix: Vec::new(),
        entries,
        sides,
        truncated,
        submodules_opened,
    })
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

fn record_tree_index_sides(sides: &mut SideMap, change: &gix::diff::index::Change) {
    use gix::diff::index::ChangeRef;
    let (head, index) = match change {
        ChangeRef::Addition { entry_mode, id, .. } => (None, Some(stored(*entry_mode, id))),
        ChangeRef::Deletion { entry_mode, id, .. } => (Some(stored(*entry_mode, id)), None),
        ChangeRef::Modification {
            previous_entry_mode,
            previous_id,
            entry_mode,
            id,
            ..
        } => (
            Some(stored(*previous_entry_mode, previous_id)),
            Some(stored(*entry_mode, id)),
        ),
        ChangeRef::Rewrite { entry_mode, id, .. } => (None, Some(stored(*entry_mode, id))),
    };
    let recorded = sides.entry(change.location().to_vec()).or_default();
    recorded.staged = true;
    recorded.sides.head = head;
    recorded.sides.index = index;
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

fn index_matches_tree(index: &gix::index::State, tree: gix::ObjectId) -> bool {
    index
        .tree()
        .is_some_and(|root| root.num_entries.is_some() && root.id == tree)
}

fn record_all<E: std::fmt::Display>(
    items: impl Iterator<Item = Result<Item, E>>,
    entries: &mut Entries,
    sides: &mut SideMap,
) -> Result<(), Error> {
    for item in items {
        let item = item.map_err(|error| Error::git("compute the status", error))?;
        record(entries, sides, item);
    }
    Ok(())
}

pub(crate) fn entry<'a>(entries: &'a mut Entries, path: &[u8]) -> &'a mut Entry {
    entries.entry(path.to_vec()).or_insert_with(|| Entry {
        path: path.to_vec(),
        ..Entry::default()
    })
}

fn record_stat_dirty(
    repo: &gix::Repository,
    index: &gix::index::State,
    stat_options: gix::index::entry::stat::Options,
    entries: &mut Entries,
    sides: &mut SideMap,
) {
    use gix::index::entry::{Flags, Stage};
    let Some(workdir) = repo.workdir() else {
        return;
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
        if entries
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
        entry(entries, path).stat_dirty = true;
        let path_sides = side(sides, path);
        if path_sides.index.is_none() {
            path_sides.index = Some(stored(index_entry.mode, &index_entry.id));
        }
    }
}

fn record(entries: &mut Entries, sides: &mut SideMap, item: Item) {
    match item {
        Item::TreeIndex(change) => {
            let path = change.location().to_vec();
            let kind = tree_index_change(&change);
            entry(entries, &path).head_to_index = Some(kind);
            record_tree_index_sides(sides, &change);
        }
        Item::IndexWorktree(WorktreeItem::Modification {
            entry: index_entry,
            rela_path,
            status,
            ..
        }) => {
            let path_sides = side(sides, &rela_path);
            if path_sides.index.is_none() {
                path_sides.index = Some(stored(index_entry.mode, &index_entry.id));
            }
            if let Some(mode) = worktree_mode(index_entry.mode, &status) {
                path_sides.worktree_mode = Some(mode);
            }
            record_worktree_status(entries, &rela_path, status);
        }
        Item::IndexWorktree(WorktreeItem::DirectoryContents { entry: dir, .. }) => {
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
            entry(entries, &path).untracked = true;
        }
        Item::IndexWorktree(WorktreeItem::Rewrite { .. }) => {}
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

fn tree_index_change(change: &gix::diff::index::Change) -> IndexChange {
    use gix::diff::index::ChangeRef;
    match change {
        ChangeRef::Addition { .. } => IndexChange::Added,
        ChangeRef::Deletion { .. } => IndexChange::Deleted,
        ChangeRef::Modification {
            previous_entry_mode,
            entry_mode,
            ..
        } => {
            if object_kind(*previous_entry_mode) == object_kind(*entry_mode) {
                IndexChange::Modified
            } else {
                IndexChange::TypeChange
            }
        }
        ChangeRef::Rewrite { .. } => IndexChange::Added,
    }
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
