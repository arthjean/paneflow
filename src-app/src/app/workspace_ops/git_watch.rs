use super::*;
use notify::Watcher;

impl PaneFlowApp {
    pub(in crate::app) fn watch_git_dir(&mut self, ws: &Workspace) {
        if let Some(ref git_dir) = ws.git_dir {
            let current = self.git_watch_counts.get(git_dir).copied().unwrap_or(0);
            if current == 0
                && let Some(ref mut watcher) = self.git_watcher
                && let Err(e) = watcher.watch(git_dir, notify::RecursiveMode::NonRecursive)
            {
                log::warn!("git watcher: failed to watch {}: {e}", git_dir.display());
                return;
            }
            *self.git_watch_counts.entry(git_dir.clone()).or_insert(0) += 1;
        }
    }

    pub(in crate::app) fn unwatch_git_dir(&mut self, git_dir: &std::path::Path) {
        if let Some(count) = self.git_watch_counts.get_mut(git_dir) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.git_watch_counts.remove(git_dir);
                if let Some(ref mut watcher) = self.git_watcher {
                    let _ = watcher.unwatch(git_dir);
                }
            }
        }
    }

    pub(in crate::app) fn start_git_watcher(
        workspaces: &[crate::workspace::Workspace],
    ) -> (
        Option<notify::RecommendedWatcher>,
        std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
        std::collections::HashMap<std::path::PathBuf, usize>,
    ) {
        let (git_event_tx, git_event_rx) = std::sync::mpsc::channel();
        let mut git_watcher = match notify::recommended_watcher(git_event_tx) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("git file watcher unavailable: {e}. Falling back to polling.");
                None
            }
        };
        let mut git_watch_counts = std::collections::HashMap::new();
        if let Some(ref mut watcher) = git_watcher {
            for ws in workspaces {
                if let Some(ref git_dir) = ws.git_dir {
                    if let Err(e) = watcher.watch(git_dir, notify::RecursiveMode::NonRecursive) {
                        log::warn!("git watcher: failed to watch {}: {e}", git_dir.display());
                    } else {
                        *git_watch_counts.entry(git_dir.clone()).or_insert(0) += 1;
                    }
                }
            }
        }
        (git_watcher, git_event_rx, git_watch_counts)
    }

    pub(in crate::app) fn spawn_git_event_refresh(cx: &mut Context<Self>) {
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let debounce = std::time::Duration::from_millis(300);
                let mut last_event = std::time::Instant::now() - debounce;
                let mut pending = false;
                let mut pending_git_dirs = std::collections::HashSet::<std::path::PathBuf>::new();

                loop {
                    smol::Timer::after(std::time::Duration::from_millis(200)).await;

                    let new_dirs = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                            let mut dirs = Vec::new();
                            while let Ok(event) = app.git_event_rx.try_recv() {
                                if let Ok(ref ev) = event {
                                    for p in &ev.paths {
                                        if matches!(
                                            p.file_name().and_then(|n| n.to_str()),
                                            Some("HEAD" | "index")
                                        ) && let Some(parent) = p.parent()
                                        {
                                            dirs.push(parent.to_path_buf());
                                        }
                                    }
                                }
                            }
                            dirs
                        })
                    });

                    match new_dirs {
                        Ok(dirs) if !dirs.is_empty() => {
                            pending_git_dirs.extend(dirs);
                            last_event = std::time::Instant::now();
                            pending = true;
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }

                    if pending && last_event.elapsed() >= debounce {
                        pending = false;
                        let affected_dirs = std::mem::take(&mut pending_git_dirs);
                        log::debug!(
                            "git watcher: debounced event fired for {} dir(s)",
                            affected_dirs.len()
                        );

                        let cwds = cx.update(|cx| {
                            this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                                app.workspaces
                                    .iter()
                                    .filter(|ws| {
                                        ws.git_dir
                                            .as_ref()
                                            .is_some_and(|gd| affected_dirs.contains(gd))
                                    })
                                    .flat_map(|ws| {
                                        std::iter::once(ws.cwd.clone())
                                            .chain(ws.bound_tab_worktrees())
                                    })
                                    .filter(|cwd| !cwd.is_empty())
                                    .collect::<std::collections::BTreeSet<String>>()
                                    .into_iter()
                                    .collect::<Vec<String>>()
                            })
                        });

                        let cwds = match cwds {
                            Ok(c) => c,
                            Err(_) => break,
                        };

                        if cwds.is_empty() {
                            continue;
                        }

                        let results = smol::unblock(move || Self::probe_git_state(cwds)).await;

                        let apply = cx.update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.apply_git_probe(&results, cx);
                            })
                        });
                        if apply.is_err() {
                            break;
                        }
                    }
                }
            },
        )
        .detach();
    }

    pub(in crate::app) fn spawn_git_poll(cx: &mut Context<Self>) {
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(30)).await;

                    let cwds = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                            app.git_probe_cwds()
                        })
                    });
                    let cwds = match cwds {
                        Ok(c) => c,
                        Err(_) => break,
                    };

                    let results = smol::unblock(move || Self::probe_git_state(cwds)).await;

                    let apply = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.apply_git_probe(&results, cx);
                            app.refresh_pull_requests(cx);
                            app.enforce_worktree_limit(cx);
                        })
                    });
                    if apply.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();
    }

    fn probe_git_state(
        cwds: Vec<String>,
    ) -> Vec<(String, String, bool, crate::workspace::GitDiffStats)> {
        cwds.into_iter()
            .map(|cwd| {
                let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                (cwd, branch, is_repo, stats)
            })
            .collect()
    }

    fn apply_git_probe(
        &mut self,
        results: &[(String, String, bool, crate::workspace::GitDiffStats)],
        cx: &mut Context<Self>,
    ) {
        let mut changed = false;
        let mut refreshed_diff = false;
        for (cwd, branch, is_repo, stats) in results {
            if self.apply_git_state_for_cwd(cwd, branch.clone(), *is_repo, stats.clone()) {
                changed = true;
                refreshed_diff |= self.refresh_diff_dock_if_open_for_cwd(cwd, cx);
            }
        }
        if changed && !refreshed_diff {
            cx.notify();
        }
    }
}
