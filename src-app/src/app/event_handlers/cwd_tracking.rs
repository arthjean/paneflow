use super::*;

use crate::app::tab_worktree::CheckoutGit;

#[derive(Default)]
pub(crate) struct CwdProbeSequence {
    next: u64,
    latest: std::collections::HashMap<gpui::EntityId, u64>,
}

impl CwdProbeSequence {
    pub(crate) fn begin(&mut self, pane: gpui::EntityId) -> u64 {
        self.next += 1;
        self.latest.insert(pane, self.next);
        self.next
    }

    pub(crate) fn finish(&mut self, pane: gpui::EntityId, seq: u64) -> bool {
        if self.latest.get(&pane) != Some(&seq) {
            return false;
        }
        self.latest.remove(&pane);
        true
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum CwdGit {
    Foreign,
    Workspace {
        git_dir: Option<std::path::PathBuf>,
        git: CheckoutGit,
    },
    Worktree {
        checkout: std::path::PathBuf,
        git: CheckoutGit,
    },
}

pub(crate) struct WorkspaceCheckout {
    pub cwd: String,
    pub repo_root: Option<std::path::PathBuf>,
    pub worktree_root: std::path::PathBuf,
}

fn checkout_git(cwd: &str) -> CheckoutGit {
    let (branch, is_repo) = crate::workspace::detect_branch(cwd);
    CheckoutGit {
        branch,
        is_repo,
        stats: crate::workspace::GitDiffStats::from_cwd(cwd),
    }
}

pub(crate) fn probe_cwd_git(pane_cwd: &str, workspace: &WorkspaceCheckout) -> CwdGit {
    let Some(git_dir) = crate::workspace::find_git_dir(pane_cwd) else {
        return CwdGit::Foreign;
    };
    let (repo_root, is_worktree) = crate::workspace::resolve_repo_root(&git_dir);
    if repo_root.is_none() || repo_root != workspace.repo_root {
        return CwdGit::Foreign;
    }
    let checkout = crate::workspace::resolve_worktree_root(
        pane_cwd,
        Some(&git_dir),
        repo_root.as_deref(),
        is_worktree,
    );
    if checkout != workspace.worktree_root {
        let git = checkout_git(&checkout.to_string_lossy());
        return CwdGit::Worktree { checkout, git };
    }
    CwdGit::Workspace {
        git_dir: crate::workspace::find_git_dir(&workspace.cwd),
        git: checkout_git(&workspace.cwd),
    }
}

impl PaneFlowApp {
    pub(in crate::app) fn handle_cwd_change(
        &mut self,
        terminal: &Entity<TerminalView>,
        new_cwd: &str,
        cx: &mut Context<Self>,
    ) {
        let located = self.workspaces.iter().enumerate().find_map(|(ws_idx, ws)| {
            ws.tabs()
                .iter()
                .position(|tab| {
                    tab.root.as_ref().is_some_and(|root| {
                        root.any_leaf(&mut |pane| {
                            pane.read(cx)
                                .active_terminal_opt()
                                .is_some_and(|t| *t == *terminal)
                        })
                    })
                })
                .map(|tab_idx| (ws_idx, tab_idx))
        });
        let Some((ws_idx, tab_idx)) = located else {
            return;
        };

        let ws = &self.workspaces[ws_idx];
        let ws_id = ws.id;
        let tab_id = ws.tabs()[tab_idx].id;
        let workspace = WorkspaceCheckout {
            cwd: ws.cwd.clone(),
            repo_root: ws.repo_root.clone(),
            worktree_root: ws.worktree_root.clone(),
        };
        let pane = terminal.entity_id();
        let seq = self.cwd_probes.begin(pane);
        let new_cwd = new_cwd.to_string();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let probed = smol::unblock({
                    let new_cwd = new_cwd.clone();
                    move || {
                        let probed = probe_cwd_git(&new_cwd, &workspace);
                        (workspace, probed)
                    }
                })
                .await;

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        if !app.cwd_probes.finish(pane, seq) {
                            return;
                        }
                        let (workspace, probed) = probed;
                        log::debug!("pane cwd changed to {new_cwd}: {probed:?}");
                        app.apply_cwd_git(ws_id, tab_id, &workspace.cwd, probed, cx);
                    })
                });
            },
        )
        .detach();
    }

    fn apply_cwd_git(
        &mut self,
        ws_id: u64,
        tab_id: u64,
        ws_cwd: &str,
        probed: CwdGit,
        cx: &mut Context<Self>,
    ) {
        let Some((ws_idx, tab_idx)) = self.tab_position(ws_id, tab_id) else {
            return;
        };
        match probed {
            CwdGit::Foreign => {}
            CwdGit::Worktree { checkout, git } => {
                self.set_tab_worktree(ws_idx, tab_idx, Some(checkout.clone()), cx);
                if self
                    .worktree_states
                    .set_checkout(&checkout.to_string_lossy(), git)
                {
                    cx.notify();
                }
            }
            CwdGit::Workspace { git_dir, git } => {
                self.set_tab_worktree(ws_idx, tab_idx, None, cx);
                if self.workspaces[ws_idx].active_tab_idx() != tab_idx {
                    return;
                }
                if self.workspaces[ws_idx].git_dir != git_dir {
                    if let Some(old) = self.workspaces[ws_idx].git_dir.take() {
                        self.unwatch_git_dir(&old);
                    }
                    self.workspaces[ws_idx].git_dir = git_dir;
                    let ws = &self.workspaces[ws_idx];
                    if let Some(dir) = ws.git_dir.clone() {
                        let count = self.git_watch_counts.entry(dir.clone()).or_insert(0);
                        *count += 1;
                        if *count == 1
                            && let Some(ref mut watcher) = self.git_watcher
                            && let Err(e) = watcher.watch(&dir, notify::RecursiveMode::NonRecursive)
                        {
                            log::warn!("git watcher: failed to watch {}: {e}", dir.display());
                        }
                    }
                }
                let changed =
                    self.apply_git_state_for_cwd(ws_cwd, git.branch, git.is_repo, git.stats);
                let refreshed_diff = changed && self.refresh_diff_dock_if_open_for_cwd(ws_cwd, cx);
                if changed && !refreshed_diff {
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn spawn_initial_git_stats(ws_id: u64, cwd: String, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let cwd_for_apply = cwd.clone();
                let (branch, is_repo, stats) = smol::unblock(move || {
                    let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                    let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                    (branch, is_repo, stats)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        if app.workspaces.iter().any(|ws| ws.id == ws_id) {
                            let changed =
                                app.apply_git_state_for_cwd(&cwd_for_apply, branch, is_repo, stats);
                            let refreshed_diff = changed
                                && app.refresh_diff_dock_if_open_for_cwd(&cwd_for_apply, cx);
                            if changed && !refreshed_diff {
                                cx.notify();
                            }
                            if changed {
                                app.refresh_pull_requests(cx);
                            }
                        }
                    })
                });
            },
        )
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_git(cwd: &std::path::Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    fn committed_repo(root: &std::path::Path) -> bool {
        std::fs::create_dir_all(root.join("src")).unwrap();
        if !test_git(root, &["init", "-q", "-b", "main"]) {
            return false;
        }
        assert!(test_git(root, &["config", "core.autocrlf", "false"]));
        std::fs::write(root.join("src").join("lib.rs"), "one\n").unwrap();
        assert!(test_git(root, &["add", "."]));
        test_git(
            root,
            &[
                "-c",
                "user.email=paneflow@example.com",
                "-c",
                "user.name=Paneflow",
                "commit",
                "-q",
                "-m",
                "init",
            ],
        )
    }

    fn workspace_at(cwd: &std::path::Path) -> WorkspaceCheckout {
        let cwd = cwd.to_string_lossy().into_owned();
        let git_dir = crate::workspace::find_git_dir(&cwd);
        let (repo_root, is_worktree) = git_dir
            .as_deref()
            .map_or((None, false), crate::workspace::resolve_repo_root);
        let worktree_root = crate::workspace::resolve_worktree_root(
            &cwd,
            git_dir.as_deref(),
            repo_root.as_deref(),
            is_worktree,
        );
        WorkspaceCheckout {
            cwd,
            repo_root,
            worktree_root,
        }
    }

    fn cwd(path: &std::path::Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn only_the_latest_probe_of_a_pane_may_apply() {
        let mut probes = CwdProbeSequence::default();
        let pane = gpui::EntityId::from(7u64);
        let other = gpui::EntityId::from(8u64);
        let slow = probes.begin(pane);
        let fast = probes.begin(pane);
        let elsewhere = probes.begin(other);
        assert!(probes.finish(pane, fast));
        assert!(
            !probes.finish(pane, slow),
            "a stale probe never binds the tab"
        );
        assert!(
            probes.finish(other, elsewhere),
            "panes do not race each other"
        );
    }

    #[test]
    fn a_trip_out_of_the_repository_and_back_restores_the_workspace_state() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let foreign = tmp.path().join("foreign");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        if !committed_repo(&repo) || !committed_repo(&foreign) {
            return;
        }
        std::fs::write(repo.join("src").join("lib.rs"), "one\ntwo\n").unwrap();
        std::fs::write(foreign.join("scratch.txt"), "a\nb\nc\n").unwrap();
        let workspace = workspace_at(&repo);

        assert_eq!(probe_cwd_git(&cwd(&outside), &workspace), CwdGit::Foreign);
        assert_eq!(
            probe_cwd_git(&cwd(&foreign.join("src")), &workspace),
            CwdGit::Foreign,
            "another repository's stats never land under the workspace cwd"
        );

        let started = std::time::Instant::now();
        let back = probe_cwd_git(&cwd(&repo.join("src")), &workspace);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        let CwdGit::Workspace { git_dir, git } = back else {
            panic!("back in the workspace checkout, got {back:?}");
        };
        assert_eq!(git_dir, crate::workspace::find_git_dir(&workspace.cwd));
        assert_eq!(git.branch, "main");
        assert!(git.is_repo);
        assert_eq!(git.stats.files_changed, 1);
        assert_eq!(git.stats.insertions, 1);
    }

    #[test]
    fn a_pane_in_a_linked_worktree_binds_it_and_the_main_checkout_unbinds() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        if !committed_repo(&repo) {
            return;
        }
        let linked = tmp.path().join("linked");
        assert!(test_git(
            &repo,
            &["worktree", "add", "-q", "-b", "feat/x", &cwd(&linked)]
        ));
        let workspace = workspace_at(&repo);

        let CwdGit::Worktree { checkout, git } =
            probe_cwd_git(&cwd(&linked.join("src")), &workspace)
        else {
            panic!("a linked worktree of the workspace repository binds the tab");
        };
        assert_eq!(checkout, workspace_at(&linked).worktree_root);
        assert_eq!(git.branch, "feat/x");

        assert!(matches!(
            probe_cwd_git(&cwd(&repo), &workspace),
            CwdGit::Workspace { .. }
        ));
    }
}
