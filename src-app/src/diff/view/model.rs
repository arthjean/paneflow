use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DiffWorktree {
    pub path: PathBuf,
    pub branch: String,
    pub workspace_id: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ReviewSubject {
    pub repo_root: PathBuf,
    pub worktree: DiffWorktree,
}

impl ReviewSubject {
    pub fn repo_name(&self) -> String {
        self.repo_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_root.display().to_string())
    }

    pub fn branch_label(&self) -> String {
        crate::workspace::worktree::checkout_label(
            Some(&self.worktree.branch),
            &self.worktree.path,
            &self.repo_root,
        )
    }

    pub fn label(&self) -> String {
        let branch = self.branch_label();
        if branch.is_empty() {
            self.repo_name()
        } else {
            format!("{} · {branch}", self.repo_name())
        }
    }

    pub fn same_worktree(&self, other: &ReviewSubject) -> bool {
        self.repo_root == other.repo_root && self.worktree.path == other.worktree.path
    }
}

#[derive(Clone)]
pub struct FileEntry {
    pub path: String,
    pub change: super::super::git::FileChange,
    pub old_path: Option<String>,
    pub added: u32,
    pub removed: u32,
    pub is_binary: bool,
}

#[derive(Clone)]
pub enum FileListState {
    Loading,
    Loaded(Rc<Vec<FileEntry>>),
    Failed(String),
}
