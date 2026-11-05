use std::path::Path;

use gix::bstr::{BStr, BString};
use gix::index::entry::{Flags, Stat};
use gix::worktree::IndexPersistedOrInMemory;

use crate::Error;

const EXPECT_FILES_OUTSIDE_OF_PATTERNS: &str = "sparse.expectFilesOutsideOfPatterns";

pub(crate) fn index_for_status(repo: &gix::Repository) -> Result<IndexPersistedOrInMemory, Error> {
    let index = repo
        .index_or_empty()
        .map_err(|error| Error::git("read the index", error))?;
    if !index.is_sparse() && !has_skip_worktree(&index) {
        return Ok(IndexPersistedOrInMemory::Persisted(index));
    }
    let mut state = if index.is_sparse() {
        expand(repo, &index)?
    } else {
        gix::index::State::clone(&index)
    };
    if let (true, Some(workdir)) = (clears_present_files(repo), repo.workdir()) {
        clear_present_skip_worktree(&mut state, workdir);
    }
    mark_skip_worktree_as_tracked(&mut state);
    Ok(IndexPersistedOrInMemory::InMemory(
        gix::index::File::from_state(state, index.path()),
    ))
}

fn clears_present_files(repo: &gix::Repository) -> bool {
    let config = repo.config_snapshot();
    config
        .boolean(gix::config::tree::Core::SPARSE_CHECKOUT)
        .unwrap_or(false)
        && !config
            .boolean(EXPECT_FILES_OUTSIDE_OF_PATTERNS)
            .unwrap_or(false)
}

fn present_in(workdir: &Path, path: &BStr) -> bool {
    gix::path::from_bstr(path)
        .is_ok_and(|relative| std::fs::symlink_metadata(workdir.join(relative)).is_ok())
}

fn has_skip_worktree(state: &gix::index::State) -> bool {
    state
        .entries()
        .iter()
        .any(|entry| entry.flags.contains(Flags::SKIP_WORKTREE))
}

fn mark_skip_worktree_as_tracked(state: &mut gix::index::State) {
    for entry in state.entries_mut() {
        if entry.flags.contains(Flags::SKIP_WORKTREE) {
            entry.flags.insert(Flags::UPTODATE);
        }
    }
}

fn clear_present_skip_worktree(state: &mut gix::index::State, workdir: &Path) {
    for (entry, path) in state.entries_mut_with_paths() {
        if entry.flags.contains(Flags::SKIP_WORKTREE) && present_in(workdir, path) {
            entry.flags.remove(Flags::SKIP_WORKTREE);
        }
    }
}

fn expand(repo: &gix::Repository, sparse: &gix::index::File) -> Result<gix::index::State, Error> {
    let mut expanded = gix::index::State::new(sparse.object_hash());
    expanded.set_timestamp(sparse.timestamp());
    for entry in sparse.entries() {
        let path = entry.path(sparse);
        if !entry.mode.is_sparse() {
            expanded.dangerously_push_entry(entry.stat, entry.id, entry.flags, entry.mode, path);
            continue;
        }
        let subtree = repo
            .index_from_tree(&entry.id)
            .map_err(|error| Error::git("expand a sparse directory of the index", error))?;
        let mut prefix = BString::from(path.to_vec());
        if !prefix.ends_with(b"/") {
            prefix.push(b'/');
        }
        for inner in subtree.entries() {
            let mut full = prefix.clone();
            full.extend_from_slice(inner.path(&subtree));
            expanded.dangerously_push_entry(
                Stat::default(),
                inner.id,
                inner.flags | Flags::SKIP_WORKTREE,
                inner.mode,
                full.as_ref(),
            );
        }
    }
    expanded.sort_entries();
    Ok(expanded)
}
