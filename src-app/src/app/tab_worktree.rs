//! Git state for a checkout that is not a workspace root.
//!
//! A workspace carries the branch and diffstat of its own cwd
//! (`Workspace::git_branch` / `git_stats`). A tab bound to a worktree
//! (discussion #41) needs the same three values for a *different* directory,
//! and the sidebar reads them every frame - so they are cached per checkout
//! here and refreshed by the same off-thread probes that already feed the
//! workspace fields. Nothing in this module runs git; it only holds what the
//! bootstrap watcher and the 30 s poll bring back.

use std::collections::HashMap;

use crate::PaneFlowApp;
use crate::workspace::{GitDiffStats, worktree::WorktreeEntry};
use gpui::Context;

/// The three values a bound tab's row needs about its checkout.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CheckoutGit {
    /// Current branch, empty for a detached HEAD.
    pub branch: String,
    /// Whether the directory is inside a git repository at all.
    pub is_repo: bool,
    pub stats: GitDiffStats,
}

/// Per-checkout git state plus the worktree list of each repository, both
/// keyed by absolute path.
#[derive(Default)]
pub(crate) struct WorktreeStates {
    checkouts: HashMap<String, CheckoutGit>,
    /// `git worktree list` per repository root, for the tab menu's picker.
    listings: HashMap<String, Vec<WorktreeEntry>>,
    /// Local branches per repository root, most recent first. What the picker
    /// actually offers - the listing only says which of them already has a
    /// directory.
    branches: HashMap<String, Vec<String>>,
}

impl WorktreeStates {
    /// Store a probe result. Returns `true` when it changed something, so the
    /// caller only repaints on a real delta - the same contract
    /// [`PaneFlowApp::apply_git_state_for_cwd`] already honors.
    pub(crate) fn set_checkout(&mut self, cwd: &str, state: CheckoutGit) -> bool {
        match self.checkouts.get(cwd) {
            Some(current) if *current == state => false,
            _ => {
                self.checkouts.insert(cwd.to_string(), state);
                true
            }
        }
    }

    pub(crate) fn checkout(&self, cwd: &str) -> Option<&CheckoutGit> {
        self.checkouts.get(cwd)
    }

    pub(crate) fn set_listing(&mut self, repo_root: &str, entries: Vec<WorktreeEntry>) -> bool {
        match self.listings.get(repo_root) {
            Some(current) if *current == entries => false,
            _ => {
                self.listings.insert(repo_root.to_string(), entries);
                true
            }
        }
    }

    pub(crate) fn listing(&self, repo_root: &str) -> &[WorktreeEntry] {
        self.listings.get(repo_root).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn set_branches(&mut self, repo_root: &str, branches: Vec<String>) -> bool {
        match self.branches.get(repo_root) {
            Some(current) if *current == branches => false,
            _ => {
                self.branches.insert(repo_root.to_string(), branches);
                true
            }
        }
    }

    pub(crate) fn branches(&self, repo_root: &str) -> &[String] {
        self.branches.get(repo_root).map_or(&[], Vec::as_slice)
    }

    /// Drop every entry no longer named by `live`. Called after a workspace or
    /// tab closes so a torn-down worktree does not keep a row's worth of state
    /// alive for the rest of the session.
    pub(crate) fn retain_live(&mut self, live: &std::collections::HashSet<String>) {
        self.checkouts.retain(|cwd, _| live.contains(cwd));
        self.listings.retain(|root, _| live.contains(root));
        self.branches.retain(|root, _| live.contains(root));
    }
}

impl PaneFlowApp {
    /// Every checkout worth probing: each workspace root, plus the worktree of
    /// every bound tab. Deduplicated, so two tabs on one worktree cost one
    /// subprocess per tick rather than two.
    pub(crate) fn git_probe_cwds(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for ws in &self.workspaces {
            if !ws.cwd.is_empty() && seen.insert(ws.cwd.clone()) {
                out.push(ws.cwd.clone());
            }
            for cwd in ws.bound_tab_worktrees() {
                if seen.insert(cwd.clone()) {
                    out.push(cwd);
                }
            }
        }
        out
    }

    /// The git state a tab's row should show, or `None` for an unbound tab
    /// (which has no identity of its own to report) or one whose first probe
    /// has not landed yet.
    pub(crate) fn tab_checkout_git(&self, tab: &crate::workspace::Tab) -> Option<&CheckoutGit> {
        let path = tab.worktree.as_ref()?;
        self.worktree_states.checkout(&path.to_string_lossy())
    }

    /// The checkout the active tab works in: its worktree when bound, the
    /// workspace root otherwise.
    ///
    /// The git surfaces read this rather than `ws.cwd` so they follow the tab
    /// the user is looking at - the same move Zed makes when it recomputes its
    /// active repository from the focused item
    /// (`crates/workspace/src/workspace.rs`, `active_item_path_changed`),
    /// except that here the unit is the tab rather than the window.
    pub(crate) fn active_checkout(&self) -> Option<String> {
        let ws = self.active_workspace()?;
        ws.active_tab()
            .worktree
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| (!ws.cwd.is_empty()).then(|| ws.cwd.clone()))
    }

    /// The checkout a pane belongs to: the worktree of the tab holding it when
    /// that tab is bound, otherwise its workspace's root.
    pub(crate) fn checkout_for_pane(
        &self,
        pane: &gpui::Entity<crate::pane::Pane>,
    ) -> Option<String> {
        let ws = self
            .workspaces
            .iter()
            .find(|ws| ws.tab_for_pane(pane).is_some())?;
        ws.tab_for_pane(pane)
            .and_then(|tab| tab.worktree.as_ref())
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| (!ws.cwd.is_empty()).then(|| ws.cwd.clone()))
    }

    /// What to call a workspace's own checkout in a worktree picker.
    ///
    /// Its branch, taken from the worktree listing when it has arrived and
    /// from the workspace's own git state otherwise - the two agree, the
    /// listing is only more precise about a detached HEAD. "Project root" is
    /// the last resort, for a workspace that is not in a repository at all.
    pub(crate) fn workspace_checkout_label(&self, ws_idx: usize) -> String {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return "Project root".to_string();
        };
        let root = &ws.worktree_root;
        self.workspace_worktree_listing(ws_idx)
            .iter()
            .find(|entry| entry.path == *root)
            .map(|entry| {
                crate::workspace::worktree::checkout_label(entry.branch.as_deref(), root, root)
            })
            .filter(|label| !label.is_empty())
            .or_else(|| (!ws.git_branch.is_empty()).then(|| ws.git_branch.clone()))
            .unwrap_or_else(|| "Project root".to_string())
    }

    /// Bind `tab_idx` to `worktree`, or unbind it with `None`.
    ///
    /// The binding takes effect for panes opened *after* it: an existing pane
    /// keeps the shell it already has, because moving a live process between
    /// checkouts is not something Paneflow can do behind the user's back. What
    /// changes immediately is the row's identity and where the next pane lands.
    pub(crate) fn set_tab_worktree(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        worktree: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get_mut(ws_idx) else {
            return;
        };
        let ws_id = ws.id;
        let Some(tab) = ws.tab_mut(tab_idx) else {
            return;
        };
        if tab.worktree == worktree {
            return;
        }
        tab.worktree = worktree.clone();
        // Probe the new checkout now rather than waiting up to 30 s for the
        // poll: a row that names a branch only after half a minute reads as
        // broken.
        if let Some(path) = worktree {
            Self::spawn_initial_git_stats(ws_id, path.to_string_lossy().into_owned(), cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Refresh what the checkout picker offers for a workspace's repository -
    /// its local branches, and which of them already has a worktree - off the
    /// render thread. Called when a picker opens: both are plumbing reads, but
    /// they are still subprocesses.
    pub(crate) fn spawn_worktree_listing(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        let Some(repo_root) = self
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.clone())
        else {
            return;
        };
        let key = repo_root.to_string_lossy().into_owned();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe = repo_root.clone();
                let read = smol::unblock(move || {
                    (
                        crate::workspace::worktree::list_worktrees(&probe),
                        crate::workspace::worktree::list_branches(&probe),
                    )
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let mut changed = false;
                        if let Ok(entries) = read.0 {
                            changed |= app.worktree_states.set_listing(&key, entries);
                        }
                        if let Ok(branches) = read.1 {
                            changed |= app.worktree_states.set_branches(&key, branches);
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                });
            },
        )
        .detach();
    }

    /// The branches offered for a workspace's tabs, as last read.
    pub(crate) fn workspace_branches(&self, ws_idx: usize) -> &[String] {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.as_ref())
            .map_or(&[], |root| {
                self.worktree_states.branches(&root.to_string_lossy())
            })
    }

    /// Point a tab at a branch, making its worktree if the branch has none.
    ///
    /// This is the whole point of the picker: the user picks a branch, and
    /// whether that branch already has a directory is git's problem, not
    /// theirs. Selecting the branch the repository itself is on unbinds the
    /// tab instead of duplicating that checkout - git would refuse the second
    /// worktree anyway.
    ///
    /// The work runs off the render thread (a checkout can take seconds on a
    /// large repository) and re-resolves the tab by id when it lands, because
    /// indices do not survive an await.
    pub(crate) fn bind_tab_to_branch(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some(repo_root) = ws.repo_root.clone() else {
            return;
        };
        let Some(tab_id) = ws.tabs().get(tab_idx).map(|tab| tab.id) else {
            return;
        };
        let ws_id = ws.id;
        // A branch the listing already places needs no subprocess: bind now,
        // so the common case stays a single frame.
        if let Some(entry) = self
            .workspace_worktree_listing(ws_idx)
            .iter()
            .find(|entry| entry.branch.as_deref() == Some(branch.as_str()))
        {
            let path = (entry.path != repo_root).then(|| entry.path.clone());
            self.set_tab_worktree(ws_idx, tab_idx, path, cx);
            return;
        }

        self.branch_checkout_pending = Some(branch.clone());
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe = repo_root.clone();
                let name = branch.clone();
                let prepared = smol::unblock(move || {
                    crate::workspace::worktree::prepare_branch_checkout(&probe, &name)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.branch_checkout_pending = None;
                        match prepared {
                            Ok(path) => {
                                let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id)
                                else {
                                    cx.notify();
                                    return;
                                };
                                let path = (path != repo_root).then_some(path);
                                app.set_tab_worktree(ws_idx, tab_idx, path, cx);
                                app.spawn_worktree_listing(ws_idx, cx);
                            }
                            Err(message) => app.show_toast(message, cx),
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    /// Locate a tab by the ids that survive an await, unlike its indices.
    pub(crate) fn tab_position(&self, ws_id: u64, tab_id: u64) -> Option<(usize, usize)> {
        let ws_idx = self.workspaces.iter().position(|ws| ws.id == ws_id)?;
        let tab_idx = self.workspaces[ws_idx]
            .tabs()
            .iter()
            .position(|tab| tab.id == tab_id)?;
        Some((ws_idx, tab_idx))
    }

    /// The worktrees offered for a workspace's tabs: what `git worktree list`
    /// last reported for its repository.
    pub(crate) fn workspace_worktree_listing(&self, ws_idx: usize) -> &[WorktreeEntry] {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.as_ref())
            .map_or(&[], |root| {
                self.worktree_states.listing(&root.to_string_lossy())
            })
    }

    /// Forget the state of every checkout no longer open.
    pub(crate) fn prune_worktree_states(&mut self) {
        let live: std::collections::HashSet<String> = self
            .git_probe_cwds()
            .into_iter()
            .chain(
                self.workspaces
                    .iter()
                    .filter_map(|ws| ws.worktree_root.to_str().map(str::to_string)),
            )
            .collect();
        self.worktree_states.retain_live(&live);
    }
}

#[cfg(test)]
mod tests {
    use super::{CheckoutGit, WorktreeStates};
    use crate::workspace::GitDiffStats;

    fn state(branch: &str, insertions: usize) -> CheckoutGit {
        CheckoutGit {
            branch: branch.to_string(),
            is_repo: true,
            stats: GitDiffStats {
                files_changed: 1,
                insertions,
                deletions: 0,
            },
        }
    }

    #[test]
    fn a_repeated_probe_reports_no_change() {
        let mut states = WorktreeStates::default();
        assert!(states.set_checkout("/w/a", state("main", 3)));
        assert!(
            !states.set_checkout("/w/a", state("main", 3)),
            "an identical probe must not ask the rail to repaint"
        );
        assert!(states.set_checkout("/w/a", state("main", 4)));
        assert!(states.set_checkout("/w/a", state("feat/x", 4)));
    }

    #[test]
    fn checkouts_are_independent_and_prunable() {
        let mut states = WorktreeStates::default();
        states.set_checkout("/w/a", state("main", 1));
        states.set_checkout("/w/b", state("feat/x", 9));
        assert_eq!(
            states.checkout("/w/b").map(|s| s.branch.as_str()),
            Some("feat/x")
        );
        assert!(
            states.checkout("/w/missing").is_none(),
            "an unprobed checkout reports nothing rather than a stale neighbor"
        );

        let live = std::collections::HashSet::from(["/w/a".to_string()]);
        states.retain_live(&live);
        assert!(states.checkout("/w/a").is_some());
        assert!(
            states.checkout("/w/b").is_none(),
            "closing a tab must not leave its worktree state alive for the session"
        );
    }
}
