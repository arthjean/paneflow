use std::path::Path;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::index::entry::{Flags, Stat};
use gix::worktree::IndexPersistedOrInMemory;

use crate::Error;

const EXPECT_FILES_OUTSIDE_OF_PATTERNS: &str = "sparse.expectFilesOutsideOfPatterns";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct IndexFormat {
    pub(crate) sparse: bool,
    pub(crate) split: bool,
    pub(crate) skip_worktree: bool,
    pub(crate) version: u8,
}

pub(crate) struct ReadIndex {
    pub(crate) index: IndexPersistedOrInMemory,
    pub(crate) format: IndexFormat,
    pub(crate) depends_on_worktree: bool,
}

pub(crate) fn index_for_status(repo: &gix::Repository) -> Result<IndexPersistedOrInMemory, Error> {
    read_index(repo).map(|read| read.index)
}

pub(crate) fn read_index(repo: &gix::Repository) -> Result<ReadIndex, Error> {
    let index = repo
        .index_or_empty()
        .map_err(|error| Error::git("read the index", error))?;
    let format = IndexFormat {
        sparse: index.is_sparse(),
        split: header_entries(index.path()).is_some_and(|count| count != index.entries().len()),
        skip_worktree: has_skip_worktree(&index),
        version: index.version() as u8,
    };
    let depends_on_worktree = format.skip_worktree && clears_present_files(repo);
    Ok(ReadIndex {
        index: expanded(repo, index)?,
        format,
        depends_on_worktree,
    })
}

fn header_entries(path: &Path) -> Option<usize> {
    use std::io::Read;
    let mut header = [0u8; 12];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .ok()?;
    let count: [u8; 4] = header[8..12].try_into().ok()?;
    usize::try_from(u32::from_be_bytes(count)).ok()
}

fn expanded(
    repo: &gix::Repository,
    index: gix::worktree::Index,
) -> Result<IndexPersistedOrInMemory, Error> {
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
    let mut missing_dir: Option<BString> = None;
    for (entry, path) in state.entries_mut_with_paths() {
        if !entry.flags.contains(Flags::SKIP_WORKTREE) {
            continue;
        }
        if missing_dir
            .as_ref()
            .is_some_and(|dir| path.starts_with(dir.as_slice()))
        {
            continue;
        }
        if present_in(workdir, path) {
            entry.flags.remove(Flags::SKIP_WORKTREE);
        } else {
            missing_dir = shallowest_missing_parent(workdir, path);
        }
    }
}

fn shallowest_missing_parent(workdir: &Path, path: &BStr) -> Option<BString> {
    let mut missing = None;
    let mut end = path.len();
    while let Some(slash) = path[..end].rfind_byte(b'/') {
        if present_in(workdir, path[..slash].as_bstr()) {
            break;
        }
        missing = Some(BString::from(&path[..=slash]));
        end = slash;
    }
    missing
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
