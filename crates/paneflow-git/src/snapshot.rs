use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gix::bstr::{BString, ByteSlice};
use gix::dir::entry::{Kind as DiskKind, Status as DirStatus};
use gix::index::entry::Mode;
use gix::status::index_worktree::Item as WorktreeItem;
use gix::status::plumbing::index_as_worktree::{Change, EntryStatus};
use gix::status::{Submodule, UntrackedFiles};
use gix::submodule::config::Ignore;

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
    let status = repo
        .status(gix::progress::Discard)
        .map_err(|error| Error::git("prepare the status", error))?;
    let dirwalk = repo
        .dirwalk_options()
        .map_err(|error| Error::git("prepare the directory walk", error))?;
    let index = sparse::index_for_status(repo)?;
    let thread_limit = options.threads.for_entries(share, index.entries().len());
    let staged_index = index.clone();
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
        .index_worktree_options_mut(|worktree| worktree.thread_limit = Some(thread_limit))
        .should_interrupt_owned(Arc::clone(interrupt));
    let staged_repo = repo.clone();
    let (outcome, staged) = std::thread::scope(|scope| {
        let staged = scope
            .spawn(move || staged::changes(&staged_repo, &staged_index, compared_tree, interrupt));
        let worktree = (|| {
            let mut items = status
                .into_index_worktree_iter(Vec::<BString>::new())
                .map_err(|error| Error::git("start the status", error))?;
            for item in items.by_ref() {
                let item = item.map_err(|error| Error::git("compute the status", error))?;
                record(&mut entries, &mut sides, item);
            }
            Ok(items.into_outcome())
        })();
        let staged = staged.join().unwrap_or_else(|_| {
            Err(Error::git(
                "compare the index with its tree",
                "the comparison thread panicked",
            ))
        });
        worktree.and_then(|outcome| staged.map(|staged| (outcome, staged)))
    })?;
    for change in staged {
        record_staged(&mut entries, &mut sides, change);
    }
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

fn record(entries: &mut Entries, sides: &mut SideMap, item: WorktreeItem) {
    match item {
        WorktreeItem::Modification {
            entry: index_entry,
            rela_path,
            status,
            ..
        } => {
            let path_sides = side(sides, &rela_path);
            if path_sides.index.is_none() {
                path_sides.index = Some(stored(index_entry.mode, &index_entry.id));
            }
            if let Some(mode) = worktree_mode(index_entry.mode, &status) {
                path_sides.worktree_mode = Some(mode);
            }
            record_worktree_status(entries, &rela_path, status);
        }
        WorktreeItem::DirectoryContents { entry: dir, .. } => {
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

fn record_staged(entries: &mut Entries, sides: &mut SideMap, change: staged::Staged) {
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
