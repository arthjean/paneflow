use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::DiffWorktree;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffScope {
    #[default]
    Project,
    MultiProject,
    Worktree,
}

impl DiffScope {
    pub fn label(self) -> &'static str {
        match self {
            DiffScope::Project => "Project",
            DiffScope::MultiProject => "Multi-project",
            DiffScope::Worktree => "Worktree",
        }
    }

    pub fn all() -> [DiffScope; 3] {
        [
            DiffScope::Project,
            DiffScope::MultiProject,
            DiffScope::Worktree,
        ]
    }

    pub fn as_persisted(self) -> &'static str {
        match self {
            DiffScope::Project => "project",
            DiffScope::MultiProject => "multi_project",
            DiffScope::Worktree => "worktree",
        }
    }

    pub fn from_persisted(s: &str) -> Option<DiffScope> {
        match s {
            "project" => Some(DiffScope::Project),
            "multi_project" => Some(DiffScope::MultiProject),
            "worktree" => Some(DiffScope::Worktree),
            _ => None,
        }
    }
}

pub struct RepoGroup {
    pub repo_root: PathBuf,
    pub repo_name: String,
    pub worktrees: Vec<DiffWorktree>,
}
