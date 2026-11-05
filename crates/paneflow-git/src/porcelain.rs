use crate::{Conflict, Entry, IndexChange, Snapshot, WorktreeChange};

pub const FILES_STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v1",
    "--untracked-files=all",
    "--no-renames",
    "-z",
    "--",
    ".",
];

pub const CLEAN_STATUS_ARGS: &[&str] = &[
    "status",
    "--porcelain=v1",
    "-z",
    "--untracked-files=all",
    "--ignore-submodules=none",
];

pub const UNTRACKED_CODE: [u8; 2] = *b"??";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Record {
    pub code: [u8; 2],
    pub path: Vec<u8>,
}

pub fn records(snapshot: &Snapshot) -> Vec<Record> {
    let mut records = Vec::with_capacity(snapshot.entries.len());
    for entry in &snapshot.entries {
        entry_records(entry, &mut records);
    }
    records
}

fn entry_records(entry: &Entry, records: &mut Vec<Record>) {
    if let Some(conflict) = entry.conflict {
        records.push(Record {
            code: conflict_code(conflict),
            path: entry.path.clone(),
        });
        return;
    }
    let code = [
        entry.head_to_index.map_or(b' ', index_code),
        entry.index_to_worktree.map_or(b' ', worktree_code),
    ];
    if code != *b"  " {
        records.push(Record {
            code,
            path: entry.path.clone(),
        });
    }
    if entry.untracked {
        records.push(Record {
            code: UNTRACKED_CODE,
            path: entry.path.clone(),
        });
    }
}

fn index_code(change: IndexChange) -> u8 {
    match change {
        IndexChange::Added => b'A',
        IndexChange::Modified => b'M',
        IndexChange::Deleted => b'D',
        IndexChange::TypeChange => b'T',
    }
}

fn worktree_code(change: WorktreeChange) -> u8 {
    match change {
        WorktreeChange::Modified => b'M',
        WorktreeChange::Deleted => b'D',
        WorktreeChange::TypeChange => b'T',
        WorktreeChange::IntentToAdd => b'A',
        WorktreeChange::Submodule(submodule) if submodule.is_dirty() => b'M',
        WorktreeChange::Submodule(_) => b' ',
    }
}

fn conflict_code(conflict: Conflict) -> [u8; 2] {
    match conflict {
        Conflict::BothDeleted => *b"DD",
        Conflict::AddedByUs => *b"AU",
        Conflict::DeletedByThem => *b"UD",
        Conflict::AddedByThem => *b"UA",
        Conflict::DeletedByUs => *b"DU",
        Conflict::BothAdded => *b"AA",
        Conflict::BothModified => *b"UU",
    }
}
