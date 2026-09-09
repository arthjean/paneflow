use std::collections::HashMap;
use std::path::PathBuf;

use crate::PaneFlowApp;
use crate::workspace::{
    GitDiffStats,
    worktree::{Snapshot, WorktreeEntry},
};
use gpui::Context;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct CheckoutGit {
    pub branch: String,
    pub is_repo: bool,
    pub stats: GitDiffStats,
}

#[derive(Default)]
pub(crate) struct WorktreeStates {
    checkouts: HashMap<String, CheckoutGit>,
    listings: HashMap<String, Vec<WorktreeEntry>>,
    branches: HashMap<String, Vec<String>>,
    snapshots: HashMap<String, Vec<Snapshot>>,
}

impl WorktreeStates {
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

    pub(crate) fn set_snapshots(&mut self, repo_root: &str, snapshots: Vec<Snapshot>) -> bool {
        match self.snapshots.get(repo_root) {
            Some(current) if *current == snapshots => false,
            _ => {
                self.snapshots.insert(repo_root.to_string(), snapshots);
                true
            }
        }
    }

    pub(crate) fn snapshots(&self, repo_root: &str) -> &[Snapshot] {
        self.snapshots.get(repo_root).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn retain_live(&mut self, live: &std::collections::HashSet<String>) {
        self.checkouts.retain(|cwd, _| live.contains(cwd));
        self.listings.retain(|root, _| live.contains(root));
        self.branches.retain(|root, _| live.contains(root));
        self.snapshots.retain(|root, _| live.contains(root));
    }
}

impl PaneFlowApp {
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

    pub(crate) fn tab_checkout_git(&self, tab: &crate::workspace::Tab) -> Option<&CheckoutGit> {
        let path = tab.worktree.as_ref()?;
        self.worktree_states.checkout(&path.to_string_lossy())
    }

    pub(crate) fn active_checkout(&self) -> Option<String> {
        let ws = self.active_workspace()?;
        ws.active_tab()
            .worktree
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| (!ws.cwd.is_empty()).then(|| ws.cwd.clone()))
    }

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
        if let Some(path) = worktree {
            Self::spawn_initial_git_stats(ws_id, path.to_string_lossy().into_owned(), cx);
        }
        self.save_session(cx);
        cx.notify();
    }

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
                    let listing = crate::workspace::worktree::list_worktrees(&probe);
                    if let Ok(entries) = &listing {
                        crate::workspace::worktree::migrate_owner_markers(entries);
                    }
                    (
                        listing,
                        crate::workspace::worktree::list_branches(&probe),
                        crate::workspace::worktree::list_snapshots(&probe),
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
                        if let Ok(snapshots) = read.2 {
                            changed |= app.worktree_states.set_snapshots(&key, snapshots);
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

    pub(crate) fn workspace_branches(&self, ws_idx: usize) -> &[String] {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.as_ref())
            .map_or(&[], |root| {
                self.worktree_states.branches(&root.to_string_lossy())
            })
    }

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
                            Ok(prepared) => {
                                if prepared.created {
                                    app.adopt_created_worktree(
                                        ws_id,
                                        &repo_root,
                                        &prepared.path,
                                        &branch,
                                    );
                                    app.enforce_worktree_limit(cx);
                                }
                                let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id)
                                else {
                                    cx.notify();
                                    return;
                                };
                                let path = (prepared.path != repo_root).then_some(prepared.path);
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

    fn root_checkout_has_busy_agent(&self, ws_idx: usize, cx: &gpui::App) -> bool {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return false;
        };
        let busy_surfaces: std::collections::HashSet<u64> = ws
            .agent_sessions
            .values()
            .filter(|session| crate::app::broadcast::state_blocks_delivery(&session.state))
            .filter_map(|session| session.surface_id)
            .collect();
        if busy_surfaces.is_empty() {
            return false;
        }
        ws.tabs()
            .iter()
            .filter(|tab| tab.worktree.is_none())
            .any(|tab| !tab.surface_ids(cx).is_disjoint(&busy_surfaces))
    }

    pub(crate) fn switch_checkout_for_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        branch: String,
        base: Option<String>,
        launch_preset: Option<usize>,
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
        if self.root_checkout_has_busy_agent(ws_idx, cx) {
            self.pane_palette_branch_failed(
                tab_id,
                "An agent is working in this checkout; create a worktree instead".to_string(),
                cx,
            );
            return;
        }
        self.branch_checkout_pending = Some(if branch.trim().is_empty() {
            base.clone().unwrap_or_else(|| "HEAD".to_string())
        } else {
            branch.clone()
        });
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe = repo_root.clone();
                let switched = smol::unblock(move || {
                    crate::workspace::worktree::switch_checkout(&probe, &branch, base.as_deref())
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.branch_checkout_pending = None;
                        match switched {
                            Ok(()) => {
                                Self::spawn_initial_git_stats(
                                    ws_id,
                                    repo_root.to_string_lossy().into_owned(),
                                    cx,
                                );
                                if let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id) {
                                    app.set_tab_worktree(ws_idx, tab_idx, None, cx);
                                    app.spawn_worktree_listing(ws_idx, cx);
                                    app.pane_palette_branch_created(tab_id, launch_preset, cx);
                                }
                            }
                            Err(message) => app.pane_palette_branch_failed(tab_id, message, cx),
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn create_branch_for_tab(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        branch: String,
        base: Option<String>,
        launch_preset: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some(repo_root) = ws.repo_root.clone() else {
            self.show_toast("No git repository for this workspace", cx);
            return;
        };
        let Some(tab_id) = ws.tabs().get(tab_idx).map(|tab| tab.id) else {
            return;
        };
        let ws_id = ws.id;
        let detached = branch.trim().is_empty();
        self.branch_checkout_pending = Some(if detached {
            base.clone().unwrap_or_else(|| "HEAD".to_string())
        } else {
            branch.clone()
        });
        cx.notify();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probe = repo_root.clone();
                let name = branch.clone();
                let created = smol::unblock(move || {
                    if detached {
                        crate::workspace::worktree::create_detached_checkout(
                            &probe,
                            base.as_deref(),
                        )
                        .map(|path| {
                            crate::workspace::worktree::PreparedCheckout {
                                path,
                                created: true,
                            }
                        })
                    } else {
                        crate::workspace::worktree::create_branch_checkout(
                            &probe,
                            &name,
                            base.as_deref(),
                        )
                    }
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.branch_checkout_pending = None;
                        match created {
                            Ok(prepared) => {
                                let path = prepared.path;
                                let record = if detached {
                                    path.file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default()
                                } else {
                                    branch.clone()
                                };
                                if prepared.created {
                                    app.adopt_created_worktree(ws_id, &repo_root, &path, &record);
                                    app.enforce_worktree_limit(cx);
                                }
                                if let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id) {
                                    app.set_tab_worktree(ws_idx, tab_idx, Some(path), cx);
                                    app.spawn_worktree_listing(ws_idx, cx);
                                    app.pane_palette_branch_created(tab_id, launch_preset, cx);
                                }
                            }
                            Err(message) => app.pane_palette_branch_failed(tab_id, message, cx),
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    fn notify_snapshot_kept(&mut self, snapshot: Option<Snapshot>, cx: &mut Context<Self>) {
        if let Some(snapshot) = snapshot {
            self.show_toast(
                format!(
                    "Uncommitted changes of {} were saved as a snapshot - restore it from \
                     Settings > Worktrees",
                    snapshot.label()
                ),
                cx,
            );
        }
    }

    pub(crate) fn worktree_snapshots(&self) -> Vec<(u64, Snapshot)> {
        let mut seen = std::collections::HashSet::new();
        self.workspaces
            .iter()
            .filter_map(|ws| ws.repo_root.as_ref().map(|root| (ws.id, root)))
            .filter(|(_, root)| seen.insert(root.to_path_buf()))
            .flat_map(|(ws_id, root)| {
                self.worktree_states
                    .snapshots(&root.to_string_lossy())
                    .iter()
                    .map(move |snapshot| (ws_id, snapshot.clone()))
            })
            .collect()
    }

    pub(crate) fn restore_worktree_snapshot(
        &mut self,
        ws_id: u64,
        snapshot: Snapshot,
        cx: &mut Context<Self>,
    ) {
        let Some((ws_idx, repo_root)) = self
            .workspaces
            .iter()
            .position(|ws| ws.id == ws_id)
            .and_then(|idx| Some((idx, self.workspaces[idx].repo_root.clone()?)))
        else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe) = (repo_root.clone(), snapshot.clone());
                let restored = smol::unblock(move || {
                    crate::workspace::worktree::restore_snapshot(&probe_root, &probe)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        match restored {
                            Ok(path) => {
                                app.adopt_created_worktree(
                                    ws_id,
                                    &repo_root,
                                    &path,
                                    &snapshot.label(),
                                );
                                app.show_toast(
                                    format!("Restored {} to {}", snapshot.label(), path.display()),
                                    cx,
                                );
                                app.save_session(cx);
                            }
                            Err(message) => app.show_toast(message, cx),
                        }
                        app.spawn_worktree_listing(ws_idx, cx);
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn delete_worktree_snapshot(
        &mut self,
        ws_id: u64,
        snapshot: Snapshot,
        cx: &mut Context<Self>,
    ) {
        let Some((ws_idx, repo_root)) = self
            .workspaces
            .iter()
            .position(|ws| ws.id == ws_id)
            .and_then(|idx| Some((idx, self.workspaces[idx].repo_root.clone()?)))
        else {
            return;
        };
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe) = (repo_root, snapshot);
                let deleted = smol::unblock(move || {
                    crate::workspace::worktree::delete_snapshot(&probe_root, &probe)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        if let Err(message) = deleted {
                            app.show_toast(message, cx);
                        }
                        app.spawn_worktree_listing(ws_idx, cx);
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn managed_worktrees_snapshot(
        &self,
    ) -> Vec<(u64, crate::workspace::worktree::ManagedWorktree)> {
        self.workspaces
            .iter()
            .flat_map(|ws| {
                ws.managed_worktrees
                    .iter()
                    .map(move |wt| (ws.id, wt.clone()))
            })
            .collect()
    }

    fn bound_worktree_paths(&self) -> std::collections::HashSet<std::path::PathBuf> {
        self.workspaces
            .iter()
            .flat_map(|ws| {
                ws.tabs()
                    .iter()
                    .filter_map(|tab| tab.worktree.clone())
                    .chain(std::iter::once(ws.worktree_root.clone()))
            })
            .collect()
    }

    pub(crate) fn enforce_worktree_limit(&mut self, cx: &mut Context<Self>) {
        if !self.cached_config.worktrees.auto_remove_enabled() {
            return;
        }
        let keep = self.cached_config.worktrees.keep_limit() as usize;
        let candidates: Vec<_> = self
            .managed_worktrees_snapshot()
            .into_iter()
            .map(|(_, wt)| wt)
            .collect();
        if candidates.len() <= keep {
            return;
        }
        let bound = self.bound_worktree_paths();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let removed = smol::unblock(move || {
                    crate::workspace::worktree::trim_to_limit(candidates, keep, &bound)
                })
                .await;
                if removed.is_empty() {
                    return;
                }
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.forget_managed_paths(&removed, cx);
                    })
                });
            },
        )
        .detach();
    }

    fn forget_managed_paths(&mut self, removed: &[std::path::PathBuf], cx: &mut Context<Self>) {
        let mut touched = Vec::new();
        for (ws_idx, ws) in self.workspaces.iter_mut().enumerate() {
            let before = ws.managed_worktrees.len();
            ws.managed_worktrees
                .retain(|wt| !removed.contains(&wt.path));
            if ws.managed_worktrees.len() != before {
                touched.push(ws_idx);
            }
        }
        for ws_idx in touched {
            self.spawn_worktree_listing(ws_idx, cx);
        }
        self.prune_worktree_states();
        self.save_session(cx);
        cx.notify();
    }

    pub(crate) fn remove_managed_worktree(
        &mut self,
        ws_id: u64,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.iter().find(|ws| ws.id == ws_id) else {
            return;
        };
        let Some(repo_root) = ws
            .managed_worktrees
            .iter()
            .find(|wt| wt.path == path)
            .map(|wt| wt.repo_root.clone())
        else {
            return;
        };
        if self.workspaces.iter().any(|ws| ws.worktree_root == path) {
            self.show_toast(
                format!("{} is open as a workspace - close it first", path.display()),
                cx,
            );
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe_path) = (repo_root.clone(), path.clone());
                let removed =
                    smol::unblock(move || remove_checkout(&probe_root, &probe_path)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        match removed {
                            Ok(snapshot) => {
                                app.forget_removed_worktree(ws_id, &repo_root, &path, cx);
                                app.forget_managed_paths(std::slice::from_ref(&path), cx);
                                app.notify_snapshot_kept(snapshot, cx);
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

    pub(crate) fn tab_detached_checkout(&self, ws_idx: usize, tab_idx: usize) -> Option<PathBuf> {
        let ws = self.workspaces.get(ws_idx)?;
        let path = ws.tabs().get(tab_idx)?.worktree.clone()?;
        let entry = self
            .workspace_worktree_listing(ws_idx)
            .iter()
            .find(|entry| entry.path == path)?;
        entry.branch.is_none().then_some(path)
    }

    pub(crate) fn create_branch_here(
        &mut self,
        ws_idx: usize,
        path: PathBuf,
        branch: String,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some(repo_root) = ws.repo_root.clone() else {
            return;
        };
        let ws_id = ws.id;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe_path, name) = (repo_root.clone(), path.clone(), branch);
                let result = smol::unblock(move || {
                    crate::workspace::worktree::create_branch_here(&probe_root, &probe_path, &name)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.branch_prompt_finished(result.clone(), cx);
                        if result.is_ok()
                            && let Some(ws_idx) =
                                app.workspaces.iter().position(|ws| ws.id == ws_id)
                        {
                            app.spawn_worktree_listing(ws_idx, cx);
                            Self::spawn_initial_git_stats(
                                ws_id,
                                path.to_string_lossy().into_owned(),
                                cx,
                            );
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    fn adopt_created_worktree(
        &mut self,
        ws_id: u64,
        repo_root: &std::path::Path,
        path: &std::path::Path,
        branch: &str,
    ) {
        let Some(ws) = self.workspaces.iter_mut().find(|ws| ws.id == ws_id) else {
            return;
        };
        if ws.managed_worktrees.iter().any(|wt| wt.path == path) {
            return;
        }
        ws.managed_worktrees
            .push(crate::workspace::worktree::ManagedWorktree {
                path: path.to_path_buf(),
                repo_root: repo_root.to_path_buf(),
                branch: branch.to_string(),
                teardown: Default::default(),
            });
    }

    pub(crate) fn tab_position(&self, ws_id: u64, tab_id: u64) -> Option<(usize, usize)> {
        let ws_idx = self.workspaces.iter().position(|ws| ws.id == ws_id)?;
        let tab_idx = self.workspaces[ws_idx]
            .tabs()
            .iter()
            .position(|tab| tab.id == tab_id)?;
        Some((ws_idx, tab_idx))
    }

    pub(crate) fn workspace_worktree_listing(&self, ws_idx: usize) -> &[WorktreeEntry] {
        self.workspaces
            .get(ws_idx)
            .and_then(|ws| ws.repo_root.as_ref())
            .map_or(&[], |root| {
                self.worktree_states.listing(&root.to_string_lossy())
            })
    }

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

    pub(crate) fn remove_tab_worktree(
        &mut self,
        ws_idx: usize,
        tab_idx: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let Some(repo_root) = ws.repo_root.clone() else {
            return;
        };
        let Some(path) = ws.tabs().get(tab_idx).and_then(|tab| tab.worktree.clone()) else {
            return;
        };
        let ws_id = ws.id;
        if self.workspaces.iter().any(|ws| ws.worktree_root == path) {
            self.show_toast(
                format!("{} is open as a workspace - close it first", path.display()),
                cx,
            );
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (probe_root, probe_path) = (repo_root.clone(), path.clone());
                let removed =
                    smol::unblock(move || remove_checkout(&probe_root, &probe_path)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        match removed {
                            Ok(snapshot) => {
                                app.forget_removed_worktree(ws_id, &repo_root, &path, cx);
                                app.notify_snapshot_kept(snapshot, cx);
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

    fn forget_removed_worktree(
        &mut self,
        ws_id: u64,
        repo_root: &std::path::Path,
        path: &std::path::Path,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) else {
            return;
        };
        let orphaned: Vec<usize> = self.workspaces[ws_idx]
            .tabs()
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.worktree.as_deref() == Some(path))
            .map(|(idx, _)| idx)
            .collect();
        for tab_idx in orphaned {
            self.set_tab_worktree(ws_idx, tab_idx, None, cx);
        }
        self.prune_worktree_states();
        self.spawn_worktree_listing(ws_idx, cx);
        self.review_forget_worktree(repo_root, path, cx);
    }
}

fn remove_checkout(
    repo_root: &std::path::Path,
    path: &std::path::Path,
) -> Result<Option<Snapshot>, String> {
    crate::workspace::worktree::snapshot_and_remove(repo_root, path)
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
