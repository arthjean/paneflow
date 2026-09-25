use super::*;

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
        let is_active_tab = self.workspaces[ws_idx].active_tab_idx() == tab_idx;

        if self.workspaces[ws_idx].cwd == new_cwd {
            return;
        }

        let ws_id = self.workspaces[ws_idx].id;
        let tab_id = self.workspaces[ws_idx].tabs()[tab_idx].id;
        let ws_repo_root = self.workspaces[ws_idx].repo_root.clone();
        let ws_worktree_root = self.workspaces[ws_idx].worktree_root.clone();

        let new_cwd_owned = new_cwd.to_string();

        cx.spawn({
            let new_cwd = new_cwd_owned.clone();
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (git_dir, branch, is_repo, stats, checkout) = smol::unblock({
                    let cwd = new_cwd.clone();
                    move || {
                        let git_dir = crate::workspace::find_git_dir(&cwd);
                        let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                        let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                        let checkout = git_dir.as_deref().map(|dir| {
                            let (repo_root, is_worktree) = crate::workspace::resolve_repo_root(dir);
                            let root = crate::workspace::resolve_worktree_root(
                                &cwd,
                                Some(dir),
                                repo_root.as_deref(),
                                is_worktree,
                            );
                            (repo_root, root)
                        });
                        (git_dir, branch, is_repo, stats, checkout)
                    }
                })
                .await;

                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let Some(ws_idx) = app.workspaces.iter().position(|ws| ws.id == ws_id)
                        else {
                            return;
                        };
                        if let Some((repo_root, checkout)) = checkout
                            && repo_root.is_some()
                            && repo_root == ws_repo_root
                            && checkout != ws_worktree_root
                        {
                            if let Some((ws_idx, tab_idx)) = app.tab_position(ws_id, tab_id) {
                                app.set_tab_worktree(ws_idx, tab_idx, Some(checkout.clone()), cx);
                            }
                            let key = checkout.to_string_lossy().into_owned();
                            if app.worktree_states.set_checkout(
                                &key,
                                crate::app::tab_worktree::CheckoutGit {
                                    branch,
                                    is_repo,
                                    stats,
                                },
                            ) {
                                cx.notify();
                            }
                            return;
                        }
                        if !is_active_tab {
                            return;
                        }
                        let old_git_dir = app.workspaces[ws_idx].git_dir.clone();
                        if let Some(ref dir) = old_git_dir {
                            app.unwatch_git_dir(dir);
                        }
                        let tracked_cwd = {
                            let ws = &mut app.workspaces[ws_idx];
                            ws.git_dir = git_dir.clone();
                            ws.cwd.clone()
                        };
                        if let Some(ref dir) = git_dir {
                            let count = app.git_watch_counts.entry(dir.clone()).or_insert(0);
                            *count += 1;
                            if *count == 1
                                && let Some(ref mut watcher) = app.git_watcher
                                && let Err(e) =
                                    watcher.watch(dir, notify::RecursiveMode::NonRecursive)
                            {
                                log::warn!("git watcher: failed to watch {}: {e}", dir.display());
                            }
                        }
                        let changed =
                            app.apply_git_state_for_cwd(&tracked_cwd, branch, is_repo, stats);
                        let refreshed_diff =
                            changed && app.refresh_diff_dock_if_open_for_cwd(&tracked_cwd, cx);
                        log::debug!("workspace CWD changed to: {new_cwd}");
                        if changed && !refreshed_diff {
                            cx.notify();
                        }
                    })
                });
            }
        })
        .detach();
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
