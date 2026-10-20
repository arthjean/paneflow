use super::*;
use notify::Watcher;

const GIT_EVENT_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

const GIT_EVENT_DEBOUNCE_CAP: std::time::Duration = std::time::Duration::from_secs(2);

const SHOWN_GIT_POLL: std::time::Duration = std::time::Duration::from_secs(30);

const HIDDEN_GIT_PROBE_AGE: std::time::Duration = std::time::Duration::from_secs(300);

const HIDDEN_GIT_POLL_ROUNDS: usize = 10;

fn plan_git_poll(
    window_active: bool,
    shown: &[String],
    hidden: &[String],
    probed_at: &std::collections::HashMap<String, std::time::Instant>,
    now: std::time::Instant,
) -> Vec<String> {
    if !window_active {
        return Vec::new();
    }
    let mut due: Vec<(Option<std::time::Instant>, &String)> = hidden
        .iter()
        .map(|cwd| (probed_at.get(cwd).copied(), cwd))
        .filter(|(probed, _)| {
            probed.is_none_or(|probed| now.duration_since(probed) >= HIDDEN_GIT_PROBE_AGE)
        })
        .collect();
    due.sort_by_key(|(probed, _)| *probed);
    let quota = hidden.len().div_ceil(HIDDEN_GIT_POLL_ROUNDS);
    shown
        .iter()
        .cloned()
        .chain(due.into_iter().take(quota).map(|(_, cwd)| cwd.clone()))
        .collect()
}

fn workspace_git_cwds(workspace: &crate::workspace::Workspace) -> Vec<String> {
    let mut cwds: Vec<String> = Vec::new();
    for cwd in std::iter::once(workspace.cwd.clone())
        .chain(workspace.bound_tab_worktrees())
        .filter(|cwd| !cwd.is_empty())
    {
        if !cwds.contains(&cwd) {
            cwds.push(cwd);
        }
    }
    cwds
}

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
    fn shown_workspace_indices(&self, cx: &App) -> Vec<usize> {
        self.workspaces
            .iter()
            .enumerate()
            .filter(|(idx, workspace)| {
                *idx == self.active_idx
                    || workspace
                        .collect_panes()
                        .iter()
                        .any(|pane| pane.read(cx).detached.is_some())
            })
            .map(|(idx, _)| idx)
            .collect()
    }

    fn due_git_poll_cwds(&mut self, now: std::time::Instant, cx: &App) -> Vec<String> {
        let live: std::collections::HashSet<String> = self.git_probe_cwds().into_iter().collect();
        self.git_probed_at.retain(|cwd, _| live.contains(cwd));
        let mut shown: Vec<String> = Vec::new();
        for idx in self.shown_workspace_indices(cx) {
            for cwd in workspace_git_cwds(&self.workspaces[idx]) {
                if !shown.contains(&cwd) {
                    shown.push(cwd);
                }
            }
        }
        let hidden: Vec<String> = self
            .git_probe_cwds()
            .into_iter()
            .filter(|cwd| !shown.contains(cwd))
            .collect();
        let due = plan_git_poll(
            crate::agents::notifications::window_active(),
            &shown,
            &hidden,
            &self.git_probed_at,
            now,
        );
        for cwd in &due {
            self.git_probed_at.insert(cwd.clone(), now);
        }
        due
    }

    pub(crate) fn probe_workspace_git_now(&mut self, ws_idx: usize, cx: &mut Context<Self>) {
        if !crate::agents::notifications::window_active() {
            return;
        }
        let Some(workspace) = self.workspaces.get(ws_idx) else {
            return;
        };
        let cwds = workspace_git_cwds(workspace);
        if cwds.is_empty() {
            return;
        }
        let now = std::time::Instant::now();
        for cwd in &cwds {
            self.git_probed_at.insert(cwd.clone(), now);
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let results = smol::unblock(move || Self::probe_git_state(cwds)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.apply_git_probe(&results, cx);
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn probe_pane_workspace_git_now(
        &mut self,
        pane: &gpui::Entity<crate::pane::Pane>,
        cx: &mut Context<Self>,
    ) {
        if let Some(ws_idx) = self
            .workspaces
            .iter()
            .position(|workspace| workspace.tab_index_containing_pane(pane).is_some())
        {
            self.probe_workspace_git_now(ws_idx, cx);
        }
    }

    pub(crate) fn probe_workspace_git_if_shown_changed(
        &mut self,
        shown_before: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        if self.active_workspace().map(|workspace| workspace.id) != shown_before {
            self.probe_workspace_git_now(self.active_idx, cx);
        }
    }

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
                    smol::Timer::after(SHOWN_GIT_POLL).await;

                    let cwds = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.due_git_poll_cwds(std::time::Instant::now(), cx)
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
        let live: std::collections::HashSet<String> = self.git_probe_cwds().into_iter().collect();
        let mut changed = false;
        let mut refreshed_diff = false;
        for (cwd, branch, is_repo, stats) in results {
            if !live.contains(cwd) {
                continue;
            }
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

    fn cwds(prefix: &str, count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("/{prefix}/{index:02}"))
            .collect()
    }

    #[test]
    fn no_active_window_plans_no_periodic_probe() {
        let shown = cwds("shown", 1);
        let hidden = cwds("hidden", 29);
        assert!(
            plan_git_poll(
                false,
                &shown,
                &hidden,
                &std::collections::HashMap::new(),
                Instant::now()
            )
            .is_empty()
        );
    }

    #[test]
    fn a_poll_probes_the_shown_workspace_and_the_oldest_tenth_of_the_hidden_ones() {
        let start = Instant::now();
        let now = start + Duration::from_secs(600);
        let shown = cwds("shown", 1);
        let hidden = cwds("hidden", 29);
        let mut probed_at = std::collections::HashMap::new();
        probed_at.insert(hidden[0].clone(), now - Duration::from_secs(60));
        probed_at.insert(hidden[1].clone(), start);
        probed_at.insert(shown[0].clone(), now - Duration::from_secs(1));
        let planned = plan_git_poll(true, &shown, &hidden, &probed_at, now);
        assert_eq!(planned.len(), 4, "{planned:?}");
        assert_eq!(planned[0], shown[0]);
        assert!(!planned.contains(&hidden[0]), "probed a minute ago");
        assert!(
            !planned.contains(&hidden[1]),
            "never-probed workspaces go first"
        );
        assert_eq!(&planned[1..], &hidden[2..5]);
    }

    #[test]
    fn hidden_workspaces_cycle_every_five_minutes_within_thirty_probes_a_minute() {
        let start = Instant::now();
        let shown = cwds("shown", 1);
        let hidden = cwds("hidden", 32);
        let mut probed_at = std::collections::HashMap::new();
        let mut probes: Vec<(u64, String)> = Vec::new();
        for tick in 1..=40u64 {
            let now = start + SHOWN_GIT_POLL * tick as u32;
            for cwd in plan_git_poll(true, &shown, &hidden, &probed_at, now) {
                probed_at.insert(cwd.clone(), now);
                probes.push((tick, cwd));
            }
        }
        for tick in 2..=40u64 {
            let minute = probes
                .iter()
                .filter(|(at, _)| *at == tick || *at == tick - 1)
                .count();
            assert!(
                minute * 3 <= 30,
                "{minute} probes in the minute ending at tick {tick}"
            );
        }
        for cwd in &hidden {
            let ticks: Vec<u64> = probes
                .iter()
                .filter(|(_, probed)| probed == cwd)
                .map(|(tick, _)| *tick)
                .collect();
            assert!(
                ticks.first().is_some_and(|first| *first <= 10),
                "{cwd}: {ticks:?}"
            );
            assert!(
                ticks.windows(2).all(|pair| pair[1] - pair[0] >= 10),
                "{cwd} probed twice within 5 min: {ticks:?}"
            );
        }
        assert_eq!(
            probes.iter().filter(|(_, cwd)| *cwd == shown[0]).count(),
            40
        );
    }

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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_probe_leaves_its_git_dir_unmarked_while_commit_and_switch_mark_it() {
        use crate::git_fixture as fixture;
        use notify::Watcher as _;

        fn marked_dirs(
            events: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
        ) -> Vec<std::path::PathBuf> {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut dirs = Vec::new();
            while Instant::now() < deadline {
                match events.recv_timeout(Duration::from_millis(400)) {
                    Ok(Ok(event)) => dirs.extend(git_dirs_changed_by(&event)),
                    Ok(Err(error)) => panic!("git watcher error: {error}"),
                    Err(_) => break,
                }
            }
            dirs
        }

        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("repo");
        fixture::committed_repo(&root, &[("a.txt", "1\n")]);
        let git_dir = root.join(".git");
        let (tx, events) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(tx).expect("git watcher");
        watcher
            .watch(&git_dir, notify::RecursiveMode::NonRecursive)
            .expect("watch the git dir");
        marked_dirs(&events);

        std::fs::write(root.join("a.txt"), "2\n").expect("edit the work tree");
        let probed = PaneFlowApp::probe_git_state(vec![root.to_string_lossy().into_owned()]);
        assert!(
            probed.iter().all(|(_, _, is_repo, _)| *is_repo),
            "{probed:?}"
        );
        assert_eq!(marked_dirs(&events), Vec::<std::path::PathBuf>::new());

        fixture::run(&root, &["add", "a.txt"]);
        marked_dirs(&events);
        fixture::run(&root, &["commit", "-q", "--no-verify", "-m", "two"]);
        assert!(marked_dirs(&events).contains(&git_dir));

        fixture::run(&root, &["switch", "-q", "-c", "topic"]);
        assert!(marked_dirs(&events).contains(&git_dir));
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
