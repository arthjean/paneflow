use std::cmp::Ordering;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::index::entry::{Flags, Mode, Stage};
use gix::index::extension::Tree as CacheTree;
use gix::objs::tree::EntryKind;
use gix::ObjectId;

use crate::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Staged {
    pub(crate) path: BString,
    pub(crate) head: Option<(Mode, ObjectId)>,
    pub(crate) index: Option<(Mode, ObjectId)>,
}

pub(crate) fn changes(
    repo: &gix::Repository,
    index: &gix::index::State,
    tree: Option<ObjectId>,
    interrupt: &AtomicBool,
) -> Result<Vec<Staged>, Error> {
    let mut differ = Differ {
        repo,
        index,
        interrupt,
        changes: Vec::new(),
    };
    differ.directory(
        BString::default(),
        tree,
        index.tree(),
        0..index.entries().len(),
    )?;
    Ok(differ.changes)
}

pub(crate) fn subtree_unchanged(node: Option<&CacheTree>, tree: Option<ObjectId>) -> bool {
    matches!((node, tree), (Some(node), Some(tree)) if node.num_entries.is_some() && node.id == tree)
}

struct TreeEntry {
    name: BString,
    kind: EntryKind,
    id: ObjectId,
}

struct Group<'a> {
    name: &'a BStr,
    is_directory: bool,
    range: Range<usize>,
}

struct Differ<'a> {
    repo: &'a gix::Repository,
    index: &'a gix::index::State,
    interrupt: &'a AtomicBool,
    changes: Vec<Staged>,
}

impl<'a> Differ<'a> {
    fn directory(
        &mut self,
        prefix: BString,
        tree: Option<ObjectId>,
        node: Option<&CacheTree>,
        range: Range<usize>,
    ) -> Result<(), Error> {
        if subtree_unchanged(node, tree) {
            return Ok(());
        }
        if self.interrupt.load(AtomicOrdering::Relaxed) {
            return Err(Error::Interrupted);
        }
        let tree_entries = match tree {
            Some(tree) => self.tree_entries(tree)?,
            None => Vec::new(),
        };
        let mut position = range.start;
        let mut tree_position = 0;
        loop {
            let group =
                (position < range.end).then(|| self.group(prefix.as_bstr(), position, range.end));
            match (group, tree_entries.get(tree_position)) {
                (None, None) => break,
                (Some(group), None) => {
                    position = group.range.end;
                    self.added(group.range);
                }
                (None, Some(entry)) => {
                    tree_position += 1;
                    self.deleted(prefix.as_bstr(), entry)?;
                }
                (Some(group), Some(entry)) => {
                    match key_order(group.name, group.is_directory, entry) {
                        Ordering::Less => {
                            position = group.range.end;
                            self.added(group.range);
                        }
                        Ordering::Greater => {
                            tree_position += 1;
                            self.deleted(prefix.as_bstr(), entry)?;
                        }
                        Ordering::Equal => {
                            position = group.range.end;
                            tree_position += 1;
                            if group.is_directory {
                                let child = node.and_then(|node| {
                                    node.children.iter().find(|child| {
                                        child.name.as_slice() == group.name.as_bytes()
                                    })
                                });
                                let child_prefix = directory_prefix(prefix.as_bstr(), group.name);
                                self.directory(child_prefix, Some(entry.id), child, group.range)?;
                            } else {
                                self.compared(group.range, entry);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn group(&self, prefix: &BStr, start: usize, end: usize) -> Group<'a> {
        let entries = &self.index.entries()[start..end];
        let path = entries[0].path(self.index);
        let relative = &path[prefix.len()..];
        match relative.find_byte(b'/') {
            Some(separator) => {
                let directory = &path[..prefix.len() + separator + 1];
                let len =
                    entries.partition_point(|entry| entry.path(self.index).starts_with(directory));
                Group {
                    name: relative[..separator].as_bstr(),
                    is_directory: true,
                    range: start..start + len,
                }
            }
            None => {
                let len = entries.partition_point(|entry| entry.path(self.index) == path);
                Group {
                    name: relative.as_bstr(),
                    is_directory: false,
                    range: start..start + len,
                }
            }
        }
    }

    fn added(&mut self, range: Range<usize>) {
        for entry in &self.index.entries()[range] {
            if unstaged_placeholder(entry) {
                continue;
            }
            self.changes.push(Staged {
                path: entry.path(self.index).to_owned(),
                head: None,
                index: Some((entry.mode, entry.id)),
            });
        }
    }

    fn compared(&mut self, range: Range<usize>, tree_entry: &TreeEntry) {
        let entries = &self.index.entries()[range];
        if entries.iter().any(unstaged_placeholder) {
            return;
        }
        let Some(entry) = entries.first() else {
            return;
        };
        let head = (index_mode(tree_entry.kind), tree_entry.id);
        if (entry.mode, entry.id) != head {
            self.changes.push(Staged {
                path: entry.path(self.index).to_owned(),
                head: Some(head),
                index: Some((entry.mode, entry.id)),
            });
        }
    }

    fn deleted(&mut self, prefix: &BStr, entry: &TreeEntry) -> Result<(), Error> {
        if entry.kind != EntryKind::Tree {
            let mut path = prefix.to_owned();
            path.extend_from_slice(&entry.name);
            self.changes.push(Staged {
                path,
                head: Some((index_mode(entry.kind), entry.id)),
                index: None,
            });
            return Ok(());
        }
        if self.interrupt.load(AtomicOrdering::Relaxed) {
            return Err(Error::Interrupted);
        }
        let child_prefix = directory_prefix(prefix, entry.name.as_bstr());
        for child in self.tree_entries(entry.id)? {
            self.deleted(child_prefix.as_bstr(), &child)?;
        }
        Ok(())
    }

    fn tree_entries(&self, tree: ObjectId) -> Result<Vec<TreeEntry>, Error> {
        let tree = self
            .repo
            .find_tree(tree)
            .map_err(|error| Error::git("read a tree of the compared commit", error))?;
        let decoded = tree
            .decode()
            .map_err(|error| Error::git("decode a tree of the compared commit", error))?;
        Ok(decoded
            .entries
            .iter()
            .map(|entry| TreeEntry {
                name: entry.filename.to_owned(),
                kind: entry.mode.kind(),
                id: entry.oid.to_owned(),
            })
            .collect())
    }
}

fn key_order(name: &BStr, is_directory: bool, entry: &TreeEntry) -> Ordering {
    let index_key = name.iter().copied().chain(is_directory.then_some(b'/'));
    let tree_key = entry
        .name
        .iter()
        .copied()
        .chain((entry.kind == EntryKind::Tree).then_some(b'/'));
    index_key.cmp(tree_key)
}

fn directory_prefix(prefix: &BStr, name: &BStr) -> BString {
    let mut path = BString::from(Vec::with_capacity(prefix.len() + name.len() + 1));
    path.extend_from_slice(prefix);
    path.extend_from_slice(name);
    path.push(b'/');
    path
}

fn unstaged_placeholder(entry: &gix::index::Entry) -> bool {
    entry.flags.contains(Flags::INTENT_TO_ADD) || entry.stage() != Stage::Unconflicted
}

fn index_mode(kind: EntryKind) -> Mode {
    match kind {
        EntryKind::Tree => Mode::DIR,
        EntryKind::Blob => Mode::FILE,
        EntryKind::BlobExecutable => Mode::FILE_EXECUTABLE,
        EntryKind::Link => Mode::SYMLINK,
        EntryKind::Commit => Mode::COMMIT,
    }
}
