use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use gix::submodule::config::Ignore;

use crate::snapshot::{collect, entry, gitlink, side, Entries, SideMap};
use crate::{open, Error, Options, SubmoduleChange, Submodules, Untracked, WorktreeChange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SubmoduleResult {
    pub(crate) path: Vec<u8>,
    pub(crate) ignore: Ignore,
    pub(crate) gitlink: Option<gix::ObjectId>,
    pub(crate) index_id: Option<gix::ObjectId>,
    pub(crate) change: SubmoduleChange,
    pub(crate) opened: usize,
}

impl SubmoduleResult {
    fn dirty_gitlink(&self) -> Option<gix::ObjectId> {
        self.index_id.filter(|_| self.change.is_dirty())
    }
}

pub(crate) fn evaluate(
    repo: &gix::Repository,
    options: &Options,
    interrupt: &Arc<AtomicBool>,
    share: usize,
    mut reuse: impl FnMut(&[u8], Ignore) -> Option<SubmoduleResult>,
) -> Result<Vec<SubmoduleResult>, Error> {
    let Some(submodules) = repo
        .submodules()
        .map_err(|error| Error::git("read the submodule configuration", error))?
    else {
        return Ok(Vec::new());
    };
    let configured_ignore = match options.submodules {
        Submodules::CheckAll => Some(Ignore::None),
        Submodules::Check | Submodules::Ignore => superproject_ignore(repo)?,
    };
    let mut results = Vec::new();
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
        let gitlink = submodule
            .index_id()
            .map_err(|error| Error::git("read a submodule index entry", error))?;
        if let Some(reused) = reuse(&path, ignore)
            .filter(|reused| reused.gitlink == gitlink && reused.ignore == ignore)
        {
            results.push(SubmoduleResult {
                opened: 0,
                ..reused
            });
            continue;
        }
        let (change, opened, index_id) = status(&submodule, ignore, options, interrupt, share)?;
        results.push(SubmoduleResult {
            path,
            ignore,
            gitlink,
            index_id,
            change,
            opened,
        });
    }
    Ok(results)
}

pub(crate) fn apply(results: &[SubmoduleResult], entries: &mut Entries, sides: &mut SideMap) {
    for result in results {
        let Some(index_id) = result.dirty_gitlink() else {
            continue;
        };
        let entry = entry(entries, &result.path);
        if entry.index_to_worktree.is_none() {
            entry.index_to_worktree = Some(WorktreeChange::Submodule(result.change));
            let path_sides = side(sides, &result.path);
            if path_sides.index.is_none() {
                path_sides.index = Some(gitlink(&index_id));
            }
        }
    }
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
