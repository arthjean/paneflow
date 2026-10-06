use super::*;
use notify::Watcher;

const GIT_EVENT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

const GIT_EVENT_DEBOUNCE_CAP: std::time::Duration = std::time::Duration::from_secs(2);

fn git_refresh_due(
    burst_started: std::time::Instant,
    last_event: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.duration_since(last_event) >= GIT_EVENT_DEBOUNCE
        || now.duration_since(burst_started) >= GIT_EVENT_DEBOUNCE_CAP
}

fn git_dirs_changed_by(event: &notify::Event) -> Vec<std::path::PathBuf> {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter(|path| {
            matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("HEAD" | "index")
            )
        })
        .filter_map(|path| path.parent().map(std::path::Path::to_path_buf))
        .collect()
}

impl PaneFlowApp {
    pub(in crate::app) fn rebind_git_dir(
        &mut self,
        ws_idx: usize,
        git_dir: Option<std::path::PathBuf>,
    ) {
        if self.workspaces[ws_idx].git_dir == git_dir {
            return;
        }
        if let Some(old) = self.workspaces[ws_idx].git_dir.take() {
            self.unwatch_git_dir(&old);
        }
        if let Some(dir) = &git_dir {
            let current = self.git_watch_counts.get(dir).copied().unwrap_or(0);
            if current == 0
                && let Some(ref mut watcher) = self.git_watcher
                && let Err(e) = watcher.watch(dir, notify::RecursiveMode::NonRecursive)
            {
                log::warn!("git watcher: failed to watch {}: {e}", dir.display());
            } else {
                *self.git_watch_counts.entry(dir.clone()).or_insert(0) += 1;
            }
        }
        self.workspaces[ws_idx].git_dir = git_dir;
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
                let mut last_event = std::time::Instant::now();
                let mut burst_started: Option<std::time::Instant> = None;
                let mut pending_git_dirs = std::collections::HashSet::<std::path::PathBuf>::new();

                loop {
                    smol::Timer::after(std::time::Duration::from_millis(200)).await;

                    let new_dirs = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                            let mut dirs = Vec::new();
                            while let Ok(event) = app.git_event_rx.try_recv() {
                                if let Ok(ref ev) = event {
                                    dirs.extend(git_dirs_changed_by(ev));
                                }
                            }
                            dirs
                        })
                    });

                    match new_dirs {
                        Ok(dirs) if !dirs.is_empty() => {
                            pending_git_dirs.extend(dirs);
                            last_event = std::time::Instant::now();
                            burst_started.get_or_insert(last_event);
                        }
                        Ok(_) => {}
                        Err(_) => break,
                    }

                    if burst_started.is_some_and(|started| {
                        git_refresh_due(started, last_event, std::time::Instant::now())
                    }) {
                        burst_started = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_quiet_repository_refreshes_after_the_debounce() {
        let start = Instant::now();
        assert!(!git_refresh_due(
            start,
            start,
            start + Duration::from_millis(100)
        ));
        assert!(git_refresh_due(start, start, start + GIT_EVENT_DEBOUNCE));
    }

    fn event(kind: notify::EventKind, file: &str) -> notify::Event {
        notify::Event::new(kind).add_path(std::path::PathBuf::from("/repo/.git").join(file))
    }

    #[test]
    fn a_probe_reading_head_and_index_does_not_retrigger_itself() {
        use notify::event::{AccessKind, AccessMode};
        for kind in [
            AccessKind::Open(AccessMode::Any),
            AccessKind::Read,
            AccessKind::Close(AccessMode::Read),
        ] {
            for file in ["HEAD", "index"] {
                assert!(
                    git_dirs_changed_by(&event(notify::EventKind::Access(kind), file)).is_empty()
                );
            }
        }
    }

    #[test]
    fn a_rewritten_head_or_index_marks_its_git_dir() {
        use notify::event::{CreateKind, ModifyKind, RenameMode};
        let renamed = notify::EventKind::Modify(ModifyKind::Name(RenameMode::To));
        assert_eq!(
            git_dirs_changed_by(&event(renamed, "index")),
            vec![std::path::PathBuf::from("/repo/.git")]
        );
        assert_eq!(
            git_dirs_changed_by(&event(notify::EventKind::Create(CreateKind::File), "HEAD")),
            vec![std::path::PathBuf::from("/repo/.git")]
        );
        assert!(git_dirs_changed_by(&event(renamed, "index.lock")).is_empty());
    }

    #[test]
    fn an_index_rewritten_nonstop_still_refreshes_within_two_seconds() {
        let start = Instant::now();
        let now = start + GIT_EVENT_DEBOUNCE_CAP;
        let last_event = now - Duration::from_millis(10);
        assert!(git_refresh_due(start, last_event, now));
        assert!(!git_refresh_due(
            start,
            last_event,
            now - Duration::from_millis(20)
        ));
    }
}
