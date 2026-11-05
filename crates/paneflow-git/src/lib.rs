mod open;
mod snapshot;
mod submodule;

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub use snapshot::snapshot;

pub const DEFAULT_THREAD_LIMIT: usize = 2;

pub const DEFAULT_ENTRY_CAP: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Untracked {
    Files,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submodules {
    Ignore,
    Check,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub thread_limit: usize,
    pub untracked: Untracked,
    pub submodules: Submodules,
    pub entry_cap: usize,
    pub interrupt: Option<Arc<AtomicBool>>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            thread_limit: DEFAULT_THREAD_LIMIT,
            untracked: Untracked::Files,
            submodules: Submodules::Check,
            entry_cap: DEFAULT_ENTRY_CAP,
            interrupt: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IndexChange {
    Added,
    Modified,
    Deleted,
    TypeChange,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct SubmoduleChange {
    pub commit_changed: bool,
    pub modified_content: bool,
    pub untracked_content: bool,
}

impl SubmoduleChange {
    pub fn is_dirty(self) -> bool {
        self.commit_changed || self.modified_content || self.untracked_content
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorktreeChange {
    Modified,
    Deleted,
    TypeChange,
    IntentToAdd,
    Submodule(SubmoduleChange),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Conflict {
    BothDeleted,
    AddedByUs,
    DeletedByThem,
    AddedByThem,
    DeletedByUs,
    BothAdded,
    BothModified,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub path: Vec<u8>,
    pub head_to_index: Option<IndexChange>,
    pub index_to_worktree: Option<WorktreeChange>,
    pub conflict: Option<Conflict>,
    pub untracked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Unborn,
    Tree(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub head: Head,
    pub entries: Vec<Entry>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{} is not inside a git repository", path.display())]
    NotARepository { path: PathBuf },
    #[error("{} is a bare repository without a worktree", path.display())]
    Bare { path: PathBuf },
    #[error("{} is not owned by the current user and is not a safe directory", path.display())]
    Untrusted { path: PathBuf },
    #[error("the status was interrupted")]
    Interrupted,
    #[error("{context}: {message}")]
    Git {
        context: &'static str,
        message: String,
    },
}

impl Error {
    fn git(context: &'static str, error: impl std::fmt::Display) -> Self {
        Self::Git {
            context,
            message: error.to_string(),
        }
    }
}
