use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::projection::DirState;
use crate::app::files_tree::{FileNode, compare_entries, compare_nodes};

const PER_PATH_EXPANSION_LIMIT: usize = 64;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ListingDiff {
    pub removed: Vec<usize>,
    pub inserted: Vec<usize>,
}

impl ListingDiff {
    pub(super) fn between(old: &[FileNode], new: &[FileNode]) -> Self {
        let mut diff = Self::default();
        let (mut i, mut j) = (0, 0);
        while i < old.len() && j < new.len() {
            match compare_nodes(&old[i], &new[j]) {
                Ordering::Less => {
                    diff.removed.push(i);
                    i += 1;
                }
                Ordering::Greater => {
                    diff.inserted.push(j);
                    j += 1;
                }
                Ordering::Equal => {
                    if old[i] != new[j] {
                        diff.removed.push(i);
                        diff.inserted.push(j);
                    }
                    i += 1;
                    j += 1;
                }
            }
        }
        diff.removed.extend(i..old.len());
        diff.inserted.extend(j..new.len());
        diff
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.inserted.is_empty()
    }

    fn fits(&self, old_len: usize, new_len: usize) -> bool {
        self.removed.len() <= old_len
            && old_len - self.removed.len() + self.inserted.len() == new_len
            && self.removed.windows(2).all(|pair| pair[0] < pair[1])
            && self.inserted.windows(2).all(|pair| pair[0] < pair[1])
            && self.removed.last().is_none_or(|last| *last < old_len)
            && self.inserted.last().is_none_or(|last| *last < new_len)
    }
}

pub(super) enum DirChange {
    Listing {
        dir: PathBuf,
        nodes: Arc<[FileNode]>,
        diff: Option<ListingDiff>,
        unreadable: bool,
    },
    Dropped(PathBuf),
}

#[derive(Default)]
struct VisibleCounts {
    values: Vec<usize>,
    sums: Vec<usize>,
    total: usize,
}

impl VisibleCounts {
    fn new(values: Vec<usize>) -> Self {
        let len = values.len();
        let mut sums = vec![0; len + 1];
        for index in 1..=len {
            sums[index] += values[index - 1];
            let next = index + index.isolate_lowest_one();
            if next <= len {
                sums[next] += sums[index];
            }
        }
        let total = values.iter().sum();
        Self {
            values,
            sums,
            total,
        }
    }

    fn add(&mut self, position: usize, delta: isize) {
        let Some(value) = self.values.get_mut(position) else {
            return;
        };
        *value = value.wrapping_add_signed(delta);
        self.total = self.total.wrapping_add_signed(delta);
        let mut index = position + 1;
        while index < self.sums.len() {
            self.sums[index] = self.sums[index].wrapping_add_signed(delta);
            index += index.isolate_lowest_one();
        }
    }

    fn prefix(&self, position: usize) -> usize {
        let mut index = position.min(self.values.len());
        let mut sum = 0;
        while index > 0 {
            sum += self.sums[index];
            index &= index - 1;
        }
        sum
    }

    fn select(&self, row: usize) -> Option<(usize, usize)> {
        if row >= self.total {
            return None;
        }
        let len = self.values.len();
        let mut position = 0;
        let mut remaining = row;
        let mut step = if len == 0 {
            0
        } else {
            1 << (usize::BITS - 1 - len.leading_zeros())
        };
        while step > 0 {
            let next = position + step;
            if next <= len && self.sums[next] <= remaining {
                position = next;
                remaining -= self.sums[next];
            }
            step >>= 1;
        }
        Some((position, remaining))
    }
}

struct DirListing {
    nodes: Arc<[FileNode]>,
    counts: VisibleCounts,
    unreadable: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RowRef<'a> {
    pub node: &'a FileNode,
    pub depth: usize,
    pub expanded: bool,
    pub state: DirState,
}

#[derive(Default)]
pub(super) struct TreeModel {
    root: PathBuf,
    expanded: HashSet<PathBuf>,
    dirs: HashMap<PathBuf, DirListing>,
    #[cfg(test)]
    pub(super) counted_entries: usize,
}

fn position(nodes: &[FileNode], path: &Path, is_dir: bool) -> Option<usize> {
    nodes
        .binary_search_by(|node| compare_entries(node.is_dir, &node.path, is_dir, path))
        .ok()
}

fn position_any(nodes: &[FileNode], path: &Path) -> Option<usize> {
    position(nodes, path, true).or_else(|| position(nodes, path, false))
}

fn carried_counts(
    old: &[usize],
    diff: &ListingDiff,
    nodes: &[FileNode],
    count: impl Fn(&FileNode) -> usize,
) -> Vec<usize> {
    let mut values = Vec::with_capacity(nodes.len());
    let mut removed = diff.removed.iter().copied().peekable();
    let mut inserted = diff.inserted.iter().copied().peekable();
    let mut old_position = 0;
    for (new_position, node) in nodes.iter().enumerate() {
        if inserted.next_if_eq(&new_position).is_some() {
            values.push(count(node));
            continue;
        }
        while removed.next_if_eq(&old_position).is_some() {
            old_position += 1;
        }
        values.push(
            old.get(old_position)
                .copied()
                .unwrap_or_else(|| count(node)),
        );
        old_position += 1;
    }
    values
}

impl TreeModel {
    pub(super) fn new(root: PathBuf, expanded: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut expanded: HashSet<PathBuf> = expanded
            .into_iter()
            .filter(|path| path.starts_with(&root))
            .collect();
        expanded.insert(root.clone());
        Self {
            root,
            expanded,
            dirs: HashMap::new(),
            #[cfg(test)]
            counted_entries: 0,
        }
    }

    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn expanded(&self) -> &HashSet<PathBuf> {
        &self.expanded
    }

    pub(super) fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    pub(super) fn root_listing_ready(&self) -> bool {
        self.dirs.contains_key(&self.root)
    }

    pub(super) fn root_unreadable(&self) -> bool {
        self.dirs.get(&self.root).is_some_and(|dir| dir.unreadable)
    }

    #[cfg(test)]
    pub(super) fn listing(&self, dir: &Path) -> Option<&[FileNode]> {
        self.dirs.get(dir).map(|dir| &*dir.nodes)
    }

    #[cfg(test)]
    pub(super) fn listing_ptr(&self, dir: &Path) -> Option<*const FileNode> {
        self.dirs.get(dir).map(|dir| dir.nodes.as_ptr())
    }

    #[cfg(test)]
    pub(super) fn visible_rows_of(&self, dir: &Path) -> Option<usize> {
        self.dirs.get(dir).map(|dir| dir.counts.total)
    }

    pub(super) fn row_count(&self) -> usize {
        self.dirs.get(&self.root).map_or(0, |dir| dir.counts.total)
    }

    fn node_rows(&self, node: &FileNode) -> usize {
        if node.is_dir && self.expanded.contains(&node.path) {
            1 + self.dirs.get(&node.path).map_or(0, |dir| dir.counts.total)
        } else {
            1
        }
    }

    pub(super) fn apply(&mut self, changes: Vec<DirChange>) {
        for change in changes {
            match change {
                DirChange::Listing {
                    dir,
                    nodes,
                    diff,
                    unreadable,
                } => self.install(dir, nodes, diff, unreadable),
                DirChange::Dropped(dir) => self.drop_dir(&dir),
            }
        }
    }

    fn install(
        &mut self,
        dir: PathBuf,
        nodes: Arc<[FileNode]>,
        diff: Option<ListingDiff>,
        unreadable: bool,
    ) {
        if !dir.starts_with(&self.root) {
            return;
        }
        let previous = self.dirs.remove(&dir);
        let old_total = previous.as_ref().map_or(0, |listing| listing.counts.total);
        let (values, counted) = match (previous, diff) {
            (Some(previous), Some(diff)) if diff.fits(previous.nodes.len(), nodes.len()) => (
                carried_counts(&previous.counts.values, &diff, &nodes, |node| {
                    self.node_rows(node)
                }),
                diff.inserted.len(),
            ),
            _ => (
                nodes.iter().map(|node| self.node_rows(node)).collect(),
                nodes.len(),
            ),
        };
        #[cfg(test)]
        {
            self.counted_entries += counted;
        }
        #[cfg(not(test))]
        let _ = counted;
        let counts = VisibleCounts::new(values);
        let delta = counts.total as isize - old_total as isize;
        self.dirs.insert(
            dir.clone(),
            DirListing {
                nodes,
                counts,
                unreadable,
            },
        );
        self.total_changed(&dir, delta);
    }

    fn drop_dir(&mut self, dir: &Path) {
        if dir == self.root {
            return;
        }
        if let Some(listing) = self.dirs.remove(dir) {
            self.total_changed(dir, -(listing.counts.total as isize));
        }
    }

    fn total_changed(&mut self, dir: &Path, delta: isize) {
        if dir != self.root && self.expanded.contains(dir) {
            self.row_count_changed(dir, delta);
        }
    }

    fn row_count_changed(&mut self, dir: &Path, delta: isize) {
        let mut child = dir;
        if delta == 0 {
            return;
        }
        loop {
            let Some(parent) = child.parent() else {
                return;
            };
            if !parent.starts_with(&self.root) {
                return;
            }
            let Some(listing) = self.dirs.get_mut(parent) else {
                return;
            };
            let Some(position) = position(&listing.nodes, child, true) else {
                return;
            };
            listing.counts.add(position, delta);
            if parent == self.root || !self.expanded.contains(parent) {
                return;
            }
            child = parent;
        }
    }

    pub(super) fn set_expanded(&mut self, path: &Path, expanded: bool) -> bool {
        if path == self.root || !path.starts_with(&self.root) {
            return false;
        }
        let changed = if expanded {
            self.expanded.insert(path.to_path_buf())
        } else {
            self.expanded.remove(path)
        };
        if changed {
            let subtree = self.dirs.get(path).map_or(0, |dir| dir.counts.total) as isize;
            self.row_count_changed(path, if expanded { subtree } else { -subtree });
        }
        changed
    }

    pub(super) fn replace_expanded(&mut self, expanded: HashSet<PathBuf>) {
        let mut expanded: HashSet<PathBuf> = expanded
            .into_iter()
            .filter(|path| path.starts_with(&self.root))
            .collect();
        expanded.insert(self.root.clone());
        if expanded == self.expanded {
            return;
        }
        let collapsed: Vec<PathBuf> = self.expanded.difference(&expanded).cloned().collect();
        let opened: Vec<PathBuf> = expanded.difference(&self.expanded).cloned().collect();
        if collapsed.len() + opened.len() <= PER_PATH_EXPANSION_LIMIT {
            for path in collapsed {
                self.set_expanded(&path, false);
            }
            for path in opened {
                self.set_expanded(&path, true);
            }
            return;
        }
        self.expanded = expanded;
        self.recount();
    }

    fn recount(&mut self) {
        let mut order: Vec<PathBuf> = self.dirs.keys().cloned().collect();
        order.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
        for dir in order {
            let Some(listing) = self.dirs.get(&dir) else {
                continue;
            };
            let values = listing
                .nodes
                .iter()
                .map(|node| self.node_rows(node))
                .collect();
            if let Some(listing) = self.dirs.get_mut(&dir) {
                listing.counts = VisibleCounts::new(values);
            }
        }
    }

    fn state(&self, node: &FileNode, expanded: bool) -> DirState {
        if !node.is_dir {
            return DirState::Ready;
        }
        match self.dirs.get(&node.path) {
            Some(listing) if listing.unreadable => DirState::Failed,
            None if expanded => DirState::Loading,
            _ => DirState::Ready,
        }
    }

    pub(super) fn rows(&self, range: Range<usize>) -> Vec<RowRef<'_>> {
        let end = range.end.min(self.row_count());
        if range.start >= end {
            return Vec::new();
        }
        let Some(mut listing) = self.dirs.get(&self.root) else {
            return Vec::new();
        };
        let mut stack: Vec<(&DirListing, usize)> = Vec::new();
        let mut row = range.start;
        loop {
            let Some((position, offset)) = listing.counts.select(row) else {
                return Vec::new();
            };
            stack.push((listing, position));
            if offset == 0 {
                break;
            }
            let Some(child) = self.dirs.get(&listing.nodes[position].path) else {
                return Vec::new();
            };
            listing = child;
            row = offset - 1;
        }
        let wanted = end - range.start;
        let mut out = Vec::with_capacity(wanted);
        while out.len() < wanted {
            let Some(&(listing, position)) = stack.last() else {
                break;
            };
            let node = &listing.nodes[position];
            let expanded = node.is_dir && self.expanded.contains(&node.path);
            out.push(RowRef {
                node,
                depth: stack.len() - 1,
                expanded,
                state: self.state(node, expanded),
            });
            if expanded
                && let Some(child) = self.dirs.get(&node.path)
                && !child.nodes.is_empty()
            {
                stack.push((child, 0));
                continue;
            }
            while let Some((listing, position)) = stack.last_mut() {
                *position += 1;
                if *position < listing.nodes.len() {
                    break;
                }
                stack.pop();
            }
        }
        out
    }

    pub(super) fn row(&self, index: usize) -> Option<RowRef<'_>> {
        self.rows(index..index + 1).pop()
    }

    pub(super) fn index(&self, path: &Path) -> Option<usize> {
        let relative = path.strip_prefix(&self.root).ok()?;
        let parts: Vec<_> = relative.components().collect();
        let mut dir = self.root.clone();
        let mut index = 0;
        for (level, part) in parts.iter().enumerate() {
            if level > 0 && !self.expanded.contains(&dir) {
                return None;
            }
            let listing = self.dirs.get(&dir)?;
            let child = dir.join(part);
            let position = position_any(&listing.nodes, &child)?;
            index += listing.counts.prefix(position);
            if level + 1 == parts.len() {
                return Some(index);
            }
            if !listing.nodes[position].is_dir {
                return None;
            }
            index += 1;
            dir = child;
        }
        None
    }

    pub(super) fn reconcile_selection(
        &self,
        selected: Option<&Path>,
        previous_index: usize,
    ) -> Option<PathBuf> {
        if let Some(path) = selected {
            for candidate in path.ancestors() {
                if self.index(candidate).is_some() {
                    return Some(candidate.to_path_buf());
                }
            }
        }
        let count = self.row_count();
        (count > 0)
            .then(|| self.row(previous_index.min(count - 1)))
            .flatten()
            .map(|row| row.node.path.clone())
    }
}

#[cfg(test)]
mod tests;
