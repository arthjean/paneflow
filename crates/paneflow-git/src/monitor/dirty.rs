use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::index::entry::Mode;

use super::carry::Carried;
use super::identity::{input_of_name, Input};
use crate::feed::{Batch, ChangeKind, Loss, Origin};
use crate::snapshot::Worktree;

const DOT_GIT: &[u8] = b".git";

const INDEX_FILES: [&str; 2] = ["index", "index.lock"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Pending {
    pub(crate) paths: BTreeMap<Vec<u8>, bool>,
    pub(crate) children: BTreeSet<Vec<u8>>,
    pub(crate) loss: Option<Loss>,
    pub(crate) foreign_git_dirs: bool,
    pub(crate) index_written: bool,
}

impl Pending {
    pub(crate) fn absorb(&mut self, batch: Batch, own_git_dirs: &BTreeSet<PathBuf>) {
        if let Some(loss) = batch.loss {
            self.loss.get_or_insert(loss);
        }
        for change in batch.changes {
            match change.origin {
                Origin::GitDir => {
                    if !own_git_dirs.contains(&change.root) {
                        self.foreign_git_dirs = true;
                    } else if change.kind == ChangeKind::Children {
                        self.index_written |= change.relative.as_os_str().is_empty();
                    } else if INDEX_FILES
                        .iter()
                        .any(|name| change.relative == Path::new(name))
                    {
                        self.index_written = true;
                    }
                }
                Origin::Worktree if change.kind == ChangeKind::Children => {
                    self.children.insert(relative_bytes(&change.relative));
                }
                Origin::Worktree => {
                    let path = relative_bytes(&change.relative);
                    let modify_only = change.kind == ChangeKind::Modify;
                    self.paths
                        .entry(path)
                        .and_modify(|only| *only &= modify_only)
                        .or_insert(modify_only);
                }
            }
        }
    }
}

fn relative_bytes(relative: &Path) -> Vec<u8> {
    let mut bytes = Vec::new();
    for component in relative.components() {
        let name = gix::path::os_str_into_bstr(component.as_os_str())
            .map(|name| name.to_vec())
            .unwrap_or_else(|_| component.as_os_str().to_string_lossy().as_bytes().to_vec());
        if !bytes.is_empty() {
            bytes.push(b'/');
        }
        bytes.extend_from_slice(&name);
    }
    bytes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unplannable {
    WorktreeRoot,
    InputFile(Input),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) recheck: BTreeSet<Vec<u8>>,
    pub(crate) forget: BTreeSet<Vec<u8>>,
    pub(crate) walk: BTreeSet<Vec<u8>>,
    pub(crate) stat: BTreeSet<Vec<u8>>,
    pub(crate) submodules: BTreeSet<Vec<u8>>,
    pub(crate) all_submodules: bool,
}

pub(crate) struct Planner<'a> {
    pub(crate) index: &'a gix::index::State,
    pub(crate) worktree: &'a Worktree,
    pub(crate) workdir: &'a Path,
    pub(crate) ignore_case: bool,
    pub(crate) ignored: Option<&'a BTreeSet<PathBuf>>,
}

impl Planner<'_> {
    pub(crate) fn plan(&self, pending: &Pending, carried: &Carried) -> Result<Plan, Unplannable> {
        let mut plan = Plan {
            all_submodules: pending.foreign_git_dirs,
            stat: carried.stat_only.clone(),
            ..Plan::default()
        };
        for (path, modify_only) in &pending.paths {
            self.plan_event(path, *modify_only, &mut plan)?;
        }
        for directory in &pending.children {
            self.plan_children(directory, &mut plan)?;
        }
        for path in &carried.changed {
            plan.forget.insert(path.clone());
            self.recheck_exact(path, &mut plan);
            if carried.added_or_removed.contains(path) {
                self.plan_walk(path, &mut plan);
            }
        }
        plan.stat.extend(plan.recheck.iter().cloned());
        Ok(plan)
    }

    fn plan_event(
        &self,
        path: &[u8],
        modify_only: bool,
        plan: &mut Plan,
    ) -> Result<(), Unplannable> {
        if path.is_empty() {
            return Err(Unplannable::WorktreeRoot);
        }
        let path = match path
            .split_str("/")
            .position(|component| component == DOT_GIT)
        {
            Some(0) => return Err(Unplannable::WorktreeRoot),
            Some(position) => path
                .split_str("/")
                .take(position)
                .collect::<Vec<_>>()
                .join(&b'/'),
            None => path.to_vec(),
        };
        let name = path.rsplit_str("/").next().unwrap_or(&path);
        if let Some(input) = input_of_name(name) {
            return Err(Unplannable::InputFile(input));
        }
        if let Some(gitlink) = self.gitlink_at_or_above(&path) {
            plan.submodules.insert(gitlink.clone());
            if gitlink == path {
                self.recheck_exact(&path, plan);
                plan.forget.insert(path);
            }
            return Ok(());
        }
        let path = self.collapsed_ancestor(&path).unwrap_or(path);
        let on_disk = self.disk_kind(&path);
        if modify_only && on_disk == DiskKind::Directory {
            return Ok(());
        }
        let exact_file = self.recheck_under(&path, plan);
        let has_untracked_below = self.has_untracked_at_or_below(&path);
        if !exact_file || on_disk == DiskKind::Directory || has_untracked_below {
            self.plan_walk(&path, plan);
        }
        Ok(())
    }

    fn plan_children(&self, directory: &[u8], plan: &mut Plan) -> Result<(), Unplannable> {
        if !directory.is_empty() {
            if let Some(gitlink) = self.gitlink_at_or_above(directory) {
                plan.submodules.insert(gitlink);
                return Ok(());
            }
            let mut collapsed = directory.to_vec();
            collapsed.push(b'/');
            if self.collapsed_ancestor(directory).is_some()
                || self.worktree.untracked.contains(&collapsed)
            {
                self.plan_walk(directory, plan);
                return Ok(());
            }
        }
        let Some(listing) = self.listing(directory) else {
            if directory.is_empty() {
                return Err(Unplannable::WorktreeRoot);
            }
            return self.plan_event(directory, false, plan);
        };
        let prefix = child_prefix(directory);
        let entries = if prefix.is_empty() {
            self.index.entries()
        } else {
            self.index
                .prefixed_entries_range(prefix.as_bstr())
                .map_or(&[][..], |range| &self.index.entries()[range])
        };
        let mut known = BTreeSet::new();
        let mut tracked_directories = BTreeSet::new();
        for entry in entries {
            let path = entry.path(self.index).as_bytes();
            let rest = &path[prefix.len()..];
            match rest.find_byte(b'/') {
                None => {
                    let on_disk_as_directory = entry.mode == Mode::COMMIT;
                    if listing.get(rest) != Some(&on_disk_as_directory) {
                        plan.recheck.insert(path.to_vec());
                        plan.forget.insert(path.to_vec());
                        if on_disk_as_directory {
                            plan.submodules.insert(path.to_vec());
                        }
                    }
                    known.insert(rest.to_vec());
                }
                Some(end) => {
                    tracked_directories.insert(rest[..end].to_vec());
                }
            }
        }
        for name in tracked_directories {
            if listing.get(&name) != Some(&true) {
                self.plan_event(&child_path(directory, &name), false, plan)?;
            }
            known.insert(name);
        }
        let mut untracked = BTreeMap::new();
        for path in self
            .worktree
            .untracked
            .range(prefix.clone()..)
            .take_while(|path| path.starts_with(&prefix))
        {
            let rest = &path[prefix.len()..];
            let (name, is_directory) = match rest.find_byte(b'/') {
                None => (rest, false),
                Some(end) => (&rest[..end], true),
            };
            untracked.insert(name.to_vec(), is_directory);
        }
        for (name, is_directory) in untracked {
            if listing.get(&name) != Some(&is_directory) {
                self.plan_walk(&child_path(directory, &name), plan);
            }
            known.insert(name);
        }
        for name in listing.keys() {
            if known.contains(name) || (directory.is_empty() && name.as_slice() == DOT_GIT) {
                continue;
            }
            let path = child_path(directory, name);
            if self.is_covered_as_ignored(&path) {
                continue;
            }
            self.plan_walk(&path, plan);
        }
        Ok(())
    }

    fn listing(&self, directory: &[u8]) -> Option<BTreeMap<Vec<u8>, bool>> {
        let relative = gix::path::from_byte_slice(directory).ok()?;
        let entries = std::fs::read_dir(self.workdir.join(relative)).ok()?;
        let mut listing = BTreeMap::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Ok(name) = gix::path::os_str_into_bstr(&name) else {
                continue;
            };
            let is_directory = entry.file_type().is_ok_and(|kind| kind.is_dir());
            listing.insert(name.to_vec(), is_directory);
        }
        Some(listing)
    }

    fn is_covered_as_ignored(&self, path: &[u8]) -> bool {
        let (Some(ignored), Ok(relative)) = (self.ignored, gix::path::from_byte_slice(path)) else {
            return false;
        };
        relative
            .ancestors()
            .filter(|ancestor| !ancestor.as_os_str().is_empty())
            .any(|ancestor| ignored.contains(ancestor))
    }

    fn plan_walk(&self, path: &[u8], plan: &mut Plan) {
        if self.gitlink_at_or_above(path).is_some() {
            return;
        }
        let path = self
            .collapsed_ancestor(path)
            .unwrap_or_else(|| path.to_vec());
        plan.walk.insert(path);
    }

    fn recheck_exact(&self, path: &[u8], plan: &mut Plan) {
        if self.index.entry_range(path.as_bstr()).is_some() {
            plan.recheck.insert(path.to_vec());
        }
    }

    fn recheck_under(&self, path: &[u8], plan: &mut Plan) -> bool {
        let mut exact_file = false;
        if let Some(range) = self.index.entry_range(path.as_bstr()) {
            exact_file = self.index.entries()[range]
                .iter()
                .any(|entry| entry.mode != Mode::COMMIT);
            plan.recheck.insert(path.to_vec());
            plan.forget.insert(path.to_vec());
        }
        let mut directory = path.to_vec();
        directory.push(b'/');
        let mut found = false;
        if let Some(range) = self.index.prefixed_entries_range(directory.as_bstr()) {
            for entry in &self.index.entries()[range] {
                let entry_path = entry.path(self.index).as_bytes().to_vec();
                plan.forget.insert(entry_path.clone());
                plan.recheck.insert(entry_path);
                found = true;
            }
        }
        if !exact_file && !found && self.ignore_case {
            for entry in self.index.entries() {
                let entry_path = entry.path(self.index).as_bytes();
                if folds_to(entry_path, path) {
                    plan.forget.insert(entry_path.to_vec());
                    plan.recheck.insert(entry_path.to_vec());
                }
            }
        }
        exact_file
    }

    fn gitlink_at_or_above(&self, path: &[u8]) -> Option<Vec<u8>> {
        ancestors_and_self(path).find_map(|candidate| {
            self.index
                .entry_by_path(candidate.as_bstr())
                .filter(|entry| entry.mode == Mode::COMMIT)
                .map(|_| candidate.to_vec())
        })
    }

    fn collapsed_ancestor(&self, path: &[u8]) -> Option<Vec<u8>> {
        ancestors_and_self(path)
            .filter(|candidate| candidate.len() < path.len())
            .find(|candidate| {
                let mut collapsed = candidate.to_vec();
                collapsed.push(b'/');
                self.worktree.untracked.contains(&collapsed)
            })
            .map(<[u8]>::to_vec)
    }

    fn has_untracked_at_or_below(&self, path: &[u8]) -> bool {
        let mut directory = path.to_vec();
        directory.push(b'/');
        self.worktree.untracked.contains(path)
            || self
                .worktree
                .untracked
                .range(directory.clone()..)
                .next()
                .is_some_and(|next| next.starts_with(&directory))
    }

    fn disk_kind(&self, path: &[u8]) -> DiskKind {
        let Ok(relative) = gix::path::from_byte_slice(path) else {
            return DiskKind::Missing;
        };
        match std::fs::symlink_metadata(self.workdir.join(relative)) {
            Ok(metadata) if metadata.is_dir() => DiskKind::Directory,
            Ok(_) => DiskKind::Other,
            Err(_) => DiskKind::Missing,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DiskKind {
    Directory,
    Other,
    Missing,
}

fn child_prefix(directory: &[u8]) -> Vec<u8> {
    let mut prefix = directory.to_vec();
    if !prefix.is_empty() {
        prefix.push(b'/');
    }
    prefix
}

fn child_path(directory: &[u8], name: &[u8]) -> Vec<u8> {
    let mut path = child_prefix(directory);
    path.extend_from_slice(name);
    path
}

fn ancestors_and_self(path: &[u8]) -> impl Iterator<Item = &[u8]> {
    path.char_indices()
        .filter(|(_, _, character)| *character == '/')
        .map(move |(start, _, _)| &path[..start])
        .chain(std::iter::once(path))
}

fn folds_to(entry_path: &[u8], path: &[u8]) -> bool {
    entry_path.len() >= path.len()
        && entry_path[..path.len()].eq_ignore_ascii_case(path)
        && (entry_path.len() == path.len() || entry_path[path.len()] == b'/')
}
