//! Multi-worktree diff open handlers + worktree collection helpers,
//! extracted from `event_handlers.rs` (US-055 code-motion).

use gpui::{AppContext, Context, Window};

use crate::PaneFlowApp;

fn push_unique_worktree(
    out: &mut Vec<crate::diff::DiffWorktree>,
    seen: &mut std::collections::HashSet<String>,
    path: std::path::PathBuf,
    branch: String,
    workspace_id: Option<u64>,
) {
    if seen.insert(norm_path(&path)) {
        out.push(crate::diff::DiffWorktree {
            path,
            branch,
            workspace_id,
        });
    }
}

impl PaneFlowApp {
    /// US-003 (prd-multi-worktree-diff) - action handler: open the
    /// multi-worktree diff view for the *active* workspace's repo. A no-op
    /// when the active workspace has no resolved `repo_root` (not a git repo).
    pub(crate) fn handle_open_multi_diff(
        &mut self,
        _: &crate::app::actions::OpenMultiDiff,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo_root) = self
            .workspaces
            .get(self.active_idx)
            .and_then(|ws| ws.repo_root.clone())
        else {
            return;
        };
        self.open_multi_diff_for_repo(repo_root, window, cx);
    }

    /// Open a `DiffView` tab seeded with every sibling worktree sharing
    /// `repo_root`. The tab is hosted in the active workspace's focused pane
    /// (falling back to its first leaf); the diff content itself is
    /// repo-scoped, independent of which pane hosts it. Ephemeral - not
    /// persisted to the session. EP-002+ fills the seeded worktrees with
    /// real diff columns.
    /// Gather the sibling-worktree seed for a repo: one [`crate::diff::DiffWorktree`]
    /// per open workspace whose `repo_root` matches. US-005 of
    /// prd-git-diff-mode-2026-Q3.md extracted this from `open_multi_diff_for_repo`
    /// so the dedicated Diff mode (`rebuild_diff_view`) and the legacy tab path
    /// share one source of truth. Pure in-memory read; git metadata was resolved
    /// when the workspace was created, so this is safe to call on the main
    /// thread.
    pub(crate) fn collect_diff_worktrees(
        &self,
        repo_root: &std::path::Path,
    ) -> Vec<crate::diff::DiffWorktree> {
        let mut seen = std::collections::HashSet::new();
        let mut worktrees = Vec::new();
        for ws in self
            .workspaces
            .iter()
            .filter(|ws| ws.repo_root.as_deref() == Some(repo_root))
        {
            push_unique_worktree(
                &mut worktrees,
                &mut seen,
                ws.worktree_root.clone(),
                ws.git_branch.clone(),
                Some(ws.id),
            );
        }
        worktrees
    }

    /// US-011: the active workspace as a single-element worktree seed (Project
    /// scope). Empty when there is no active workspace. Pure in-memory read.
    pub(crate) fn collect_project_worktrees(&self) -> Vec<crate::diff::DiffWorktree> {
        self.workspaces
            .get(self.active_idx)
            .map(|ws| {
                vec![crate::diff::DiffWorktree {
                    path: ws.worktree_root.clone(),
                    branch: ws.git_branch.clone(),
                    workspace_id: Some(ws.id),
                }]
            })
            .unwrap_or_default()
    }

    /// US-014: every open workspace grouped by canonicalized `repo_root`
    /// (Multi-project scope). `BTreeMap` keying gives stable repo ordering;
    /// workspaces with no resolved repo are skipped. Pure in-memory read.
    pub(crate) fn collect_multiproject_groups(&self) -> Vec<crate::diff::RepoGroup> {
        use std::collections::BTreeMap;
        let mut map: BTreeMap<
            std::path::PathBuf,
            (crate::diff::RepoGroup, std::collections::HashSet<String>),
        > = BTreeMap::new();
        for ws in &self.workspaces {
            let Some(root) = ws.repo_root.clone() else {
                continue;
            };
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string());
            let (group, seen) = map.entry(root.clone()).or_insert_with(|| {
                (
                    crate::diff::RepoGroup {
                        repo_root: root.clone(),
                        repo_name: name,
                        worktrees: Vec::new(),
                    },
                    std::collections::HashSet::new(),
                )
            });
            push_unique_worktree(
                &mut group.worktrees,
                seen,
                ws.worktree_root.clone(),
                ws.git_branch.clone(),
                Some(ws.id),
            );
        }
        map.into_values().map(|(group, _)| group).collect()
    }

    pub(crate) fn open_multi_diff_for_repo(
        &mut self,
        repo_root: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Gather sibling worktrees across all workspaces sharing this repo.
        let worktrees = self.collect_diff_worktrees(&repo_root);

        // Resolve a host pane from the active workspace (focused, else first leaf).
        let target_pane = {
            let Some(ws) = self.workspaces.get(self.active_idx) else {
                return;
            };
            let Some(root) = ws.root.as_ref() else {
                return;
            };
            root.focused_pane(window, cx)
                .or_else(|| root.collect_leaves().into_iter().next())
        };
        let Some(target_pane) = target_pane else {
            return;
        };

        let diff = cx.new(|cx| crate::diff::DiffView::new(repo_root, worktrees, cx));
        target_pane.update(cx, |pane, cx| {
            pane.add_diff_tab(diff, cx);
            cx.notify();
        });
        cx.notify();
    }
}

/// US-013: normalize a worktree path for dedup so the same checkout only gets
/// one diff column even when several workspaces/panes point at it.
fn norm_path(p: &std::path::Path) -> String {
    let resolved = normalize_lexically(p);
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}

fn normalize_lexically(path: &std::path::Path) -> std::path::PathBuf {
    let mut out = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !out.pop() {
                    out.push(component.as_os_str());
                }
            }
            _ => out.push(component.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_unique_worktree_dedups_equivalent_paths() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();

        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        push_unique_worktree(&mut out, &mut seen, repo.clone(), "main".into(), Some(1));
        push_unique_worktree(&mut out, &mut seen, repo.join("."), "main".into(), Some(2));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].workspace_id, Some(1));
    }
}
