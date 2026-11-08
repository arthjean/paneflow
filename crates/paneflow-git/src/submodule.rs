use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gix::submodule::config::Ignore;

use crate::snapshot::{collect, entry, gitlink, side, Entries, SideMap};
use crate::{open, Error, Options, SubmoduleChange, Submodules, Untracked, WorktreeChange};

pub(crate) fn record(
    repo: &gix::Repository,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
    entries: &mut Entries,
    sides: &mut SideMap,
) -> Result<usize, Error> {
    let Some(submodules) = repo
        .submodules()
        .map_err(|error| Error::git("read the submodule configuration", error))?
    else {
        return Ok(0);
    };
    let configured_ignore = match options.submodules {
        Submodules::CheckAll => Some(Ignore::None),
        Submodules::Check | Submodules::Ignore => superproject_ignore(repo)?,
    };
    let mut opened = 0;
    for submodule in submodules {
        let path = submodule
            .path()
            .map_err(|error| Error::git("read a submodule path", error))?
            .to_vec();
        let ignore = match configured_ignore {
            Some(ignore) => ignore,
            None => submodule
                .ignore()
                .map_err(|error| Error::git("read a submodule ignore setting", error))?
                .unwrap_or_default(),
        };
        if ignore == Ignore::All {
            continue;
        }
        let (change, submodule_opens, index_id) =
            status(&submodule, ignore, options, interrupt, share)?;
        opened += submodule_opens;
        let Some(index_id) = index_id.filter(|_| change.is_dirty()) else {
            continue;
        };
        let entry = entry(entries, &path);
        if entry.index_to_worktree.is_none() {
            entry.index_to_worktree = Some(WorktreeChange::Submodule(change));
            let path_sides = side(sides, &path);
            if path_sides.index.is_none() {
                path_sides.index = Some(gitlink(&index_id));
            }
        }
    }
    Ok(opened)
}

fn superproject_ignore(repo: &gix::Repository) -> Result<Option<Ignore>, Error> {
    use gix::config::tree::Diff;
    let config = repo.config_snapshot();
    let Some(value) = config.string(Diff::IGNORE_SUBMODULES) else {
        return Ok(None);
    };
    Diff::IGNORE_SUBMODULES
        .try_into_ignore(value)
        .map(Some)
        .map_err(|error| Error::git("read diff.ignoreSubmodules", error))
}

fn status(
    submodule: &gix::Submodule<'_>,
    ignore: Ignore,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
) -> Result<(SubmoduleChange, usize, Option<gix::ObjectId>), Error> {
    let unchanged = Ok((SubmoduleChange::default(), 0, None));
    let state = submodule
        .state()
        .map_err(|error| Error::git("read a submodule state", error))?;
    if !state.repository_exists || !state.worktree_checkout {
        return unchanged;
    }
    let Some(index_id) = submodule
        .index_id()
        .map_err(|error| Error::git("read a submodule index entry", error))?
    else {
        return unchanged;
    };
    let work_dir = submodule
        .work_dir()
        .map_err(|error| Error::git("locate a submodule worktree", error))?;
    let Ok(repo) = open::submodule(&work_dir, options.threads.before_index(share)) else {
        return Ok((SubmoduleChange::default(), 1, None));
    };
    let checked_out = repo.head_id().ok().map(gix::Id::detach);
    let mut change = SubmoduleChange {
        commit_changed: checked_out != Some(index_id),
        ..SubmoduleChange::default()
    };
    if ignore == Ignore::Dirty {
        return Ok((change, 1, Some(index_id)));
    }
    let content_options = Options {
        untracked: if ignore == Ignore::Untracked {
            Untracked::None
        } else {
            Untracked::Files
        },
        submodules: Submodules::Check,
        entry_cap: usize::MAX,
        interrupt: None,
        base: None,
        stat_dirty: false,
        ..options.clone()
    };
    let content = collect(&repo, &content_options, interrupt, share)?;
    change.modified_content = content.entries.iter().any(|entry| {
        entry.head_to_index.is_some()
            || entry.index_to_worktree.is_some()
            || entry.conflict.is_some()
    });
    change.untracked_content = content.entries.iter().any(|entry| entry.untracked);
    Ok((change, 1 + content.submodules_opened, Some(index_id)))
}
