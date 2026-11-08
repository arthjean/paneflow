extern crate self as paneflow_git;

pub mod engine;
pub mod feed;
pub mod monitor;
mod objects;
mod open;
#[cfg(feature = "oracle")]
#[path = "../tests/oracle/mod.rs"]
pub mod oracle;
pub mod porcelain;
pub mod renames;
mod snapshot;
mod sparse;
mod staged;
mod submodule;
#[cfg(feature = "oracle")]
#[path = "../tests/support/mod.rs"]
pub mod support;
mod threads;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub use monitor::{
    worktree_root, Freshness, Monitor, ProbeKind, ProbeReport, Probed, Reason, FENCE_TIMEOUT,
};
pub use objects::{Header, Objects};
pub use snapshot::{snapshot, with_snapshot};
pub use threads::{Threads, ENTRIES_PER_THREAD, PLATFORM_CAP};

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
    CheckAll,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub threads: Threads,
    pub untracked: Untracked,
    pub submodules: Submodules,
    pub entry_cap: usize,
    pub interrupt: Option<Arc<AtomicBool>>,
    pub base: Option<String>,
    pub stat_dirty: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            threads: Threads::Scheduled,
            untracked: Untracked::Files,
            submodules: Submodules::Check,
            entry_cap: DEFAULT_ENTRY_CAP,
            interrupt: None,
            base: None,
            stat_dirty: false,
        }
    }
}

impl Options {
    pub fn files_status() -> Self {
        Self::default()
    }

    pub fn diff_stats() -> Self {
        Self::default()
    }

    pub fn dock_listing() -> Self {
        Self {
            stat_dirty: true,
            ..Self::default()
        }
    }

    pub fn is_clean() -> Self {
        Self {
            submodules: Submodules::CheckAll,
            ..Self::default()
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
    pub stat_dirty: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Head {
    Unborn,
    Tree(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    pub mode: u32,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sides {
    pub head: Option<Stored>,
    pub index: Option<Stored>,
    pub worktree_mode: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub head: Head,
    pub prefix: Vec<u8>,
    pub entries: Vec<Entry>,
    pub sides: BTreeMap<Vec<u8>, Sides>,
    pub truncated: bool,
    pub submodules_opened: usize,
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
    #[error("the status listed more than {cap} entries")]
    TooManyEntries { cap: usize },
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
