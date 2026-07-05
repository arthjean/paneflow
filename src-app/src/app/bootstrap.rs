//! `PaneFlowApp::new()` - the application constructor.
//!
//! Wires the title bar, IPC server, config watcher, git-dir watcher, update
//! checker, and all background tickers (50 ms IPC poll, 30 s git fallback,
//! 30 s stale-PID sweep). Restores a saved session or creates a fresh
//! single-workspace state.
//!
//! Extracted from `main.rs` per US-027 of the src-app refactor PRD - pure
//! code-motion, behaviour unchanged.

use gpui::{AppContext, Context};
use notify::Watcher;

use crate::launch_cwd;
use crate::pane::Pane;
use crate::telemetry;
use crate::terminal::TerminalView;
use crate::terminal::blink::{BlinkPhase, BlinkPhaseGlobal, CURSOR_BLINK_INTERVAL};
use crate::window_chrome::title_bar;
use crate::workspace::{Workspace, next_workspace_id};
use crate::{PaneFlowApp, ipc, keybindings, update};

impl PaneFlowApp {
    fn default_workspace(cx: &mut Context<Self>) -> Workspace {
        let ws_id = next_workspace_id();
        let cwd = launch_cwd::implicit_launch_cwd();
        let terminal_cwd = cwd.clone();
        let terminal = cx.new(|cx| TerminalView::with_cwd(ws_id, Some(terminal_cwd), None, cx));
        cx.subscribe(&terminal, Self::handle_terminal_event)
            .detach();
        let pane = cx.new(|cx| Pane::new(terminal, ws_id, cx));
        cx.subscribe(&pane, Self::handle_pane_event).detach();
        let dir_name = launch_cwd::title_for_cwd_or(&cwd, "Terminal 1");
        let ws = Workspace::with_cwd_and_id(ws_id, dir_name, cwd, pane);
        Self::spawn_initial_git_stats(ws_id, ws.cwd.clone(), cx);
        ws
    }

    pub(crate) fn spawn_telemetry_flusher(
        telemetry: std::sync::Arc<telemetry::client::TelemetryClient>,
        cx: &mut Context<Self>,
    ) {
        cx.background_spawn(async move {
            loop {
                smol::Timer::after(std::time::Duration::from_secs(5)).await;
                let client = std::sync::Arc::clone(&telemetry);
                if !client.is_active() {
                    break;
                }
                smol::unblock(move || client.poll_flush()).await;
            }
        })
        .detach();
    }

    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        let title_bar = cx.new(title_bar::TitleBar::new);
        cx.subscribe(&title_bar, Self::handle_title_bar_event)
            .detach();
        let (ipc_rx, ipc_status, event_bus) = ipc::start_server();

        // US-006 - install the shared cursor-blink phase as a GPUI global
        // before any `TerminalView` is constructed. Each `TerminalView`
        // reads the global in `with_cwd` and observes the entity, so all
        // visible cursors blink in phase. One bootstrap-spawned loop
        // toggles `phase.visible` every 530 ms - replaces N per-terminal
        // `smol::Timer` loops with a single ticker for the whole app.
        let blink_phase = cx.new(|_| BlinkPhase::default());
        cx.set_global(BlinkPhaseGlobal(blink_phase.clone()));
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(CURSOR_BLINK_INTERVAL).await;
                    // Read the entity fresh from the global on every tick
                    // to keep this loop consistent with the existing
                    // git-watcher / IPC-poll patterns in this file: all of
                    // them go through `this.update(cx, |app, cx| ...)` and
                    // pull whatever they need from `cx`/`app` inside the
                    // closure rather than capturing it. Capturing the
                    // entity once would also be safe (the App owns the
                    // strong ref via the global; clones at app teardown
                    // are dropped together) - consistency wins.
                    let result = cx.update(|cx| {
                        this.update(cx, |_app: &mut Self, cx: &mut Context<Self>| {
                            let phase = cx.global::<BlinkPhaseGlobal>().0.clone();
                            phase.update(cx, |p, cx| {
                                p.visible = !p.visible;
                                cx.notify();
                            });
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

        // ConfigWatcher: background thread detects file changes (300ms debounce),
        // stores parsed config in a shared slot for the 50ms poll loop to pick up.
        // Note: `start()` moves the OS watcher into a background thread, so the
        // `ConfigWatcher` struct itself can be safely dropped after starting.
        let pending_config = std::sync::Arc::new(std::sync::Mutex::new(
            None::<paneflow_config::schema::PaneFlowConfig>,
        ));
        let pending_config_writer = std::sync::Arc::clone(&pending_config);
        if let Some(Err(e)) = paneflow_config::watcher::ConfigWatcher::new(std::sync::Arc::new(
            move |cfg: paneflow_config::schema::PaneFlowConfig| {
                *pending_config_writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(cfg);
            },
        ))
        .map(|config_watcher| config_watcher.start())
        {
            log::warn!("config watcher failed to start: {e}; config hot-reload disabled");
        }

        // US-006: dedicated theme watcher. Mirrors `ConfigWatcher` shape but
        // signals via an `Arc<AtomicBool>` rather than carrying a payload -
        // theme invalidation is a tristate "did the file change" question,
        // and the actual `TerminalTheme` is recomputed lazily by
        // `active_theme()` on the next render. The 50 ms poll loop drains
        // this flag and calls `cx.notify()` to schedule the repaint. On
        // init failure the historical 500 ms polling fallback inside
        // `active_theme()` keeps the UI responsive (AC #3).
        let theme_changed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let theme_changed_writer = std::sync::Arc::clone(&theme_changed);
        match crate::theme::ThemeWatcher::new(std::sync::Arc::new(move || {
            theme_changed_writer.store(true, std::sync::atomic::Ordering::Release);
        })) {
            Some(watcher) => {
                if let Err(e) = watcher.start() {
                    log::warn!(
                        "theme watcher failed to start: {e}; falling back to 500 ms polling"
                    );
                }
            }
            None => {
                log::warn!("theme watcher: no config dir resolved; falling back to 500 ms polling");
            }
        }

        // Background update check is deferred until after the telemetry
        // client is constructed below - `spawn_check` now takes the
        // client by Arc so it can emit `update_check_started` /
        // `update_available` (US-007), and the client doesn't exist
        // this early in bootstrap.

        // Restore session or create a single default workspace. The
        // tuple's second component carries forensic context when
        // `session.json` was unparseable (US-006); we hold onto it and
        // emit the `session_corrupted` PostHog event after the
        // telemetry client is constructed below - load_session itself
        // runs too early in bootstrap to call `self.telemetry`.
        let (saved_session, session_corruption) = Self::load_session();

        // US-009 (prd-agents-view.md): pull the Agents-view bits out of
        // the saved session BEFORE the workspaces match consumes it.
        // The mode + project list are applied to the struct literal
        // below; a no-agents-installed fallback runs afterwards so the
        // UI never opens onto a blank Agents view if discovery returns
        // empty (e.g. user uninstalled `bunx` between launches).
        let restored_mode = saved_session.as_ref().map(|s| s.mode).unwrap_or_default();
        // US-015 (prd-git-diff-mode-2026-Q3.md): restore the diff scope (an
        // unknown / absent value falls back to the default, Project).
        let restored_diff_scope = saved_session
            .as_ref()
            .and_then(|s| s.diff_scope.as_deref())
            .and_then(crate::diff::DiffScope::from_persisted)
            .unwrap_or_default();
        let (restored_projects, restored_chats): (
            Vec<crate::project::Project>,
            Vec<crate::project::Thread>,
        ) = saved_session
            .as_ref()
            .map(|s| {
                let mut remaining_threads = crate::project::MAX_RESTORED_TOTAL_THREADS;
                if s.projects.len() > crate::project::MAX_RESTORED_PROJECTS {
                    log::warn!(
                        "session restore: {} project(s) exceeds cap {}, restoring the first {}",
                        s.projects.len(),
                        crate::project::MAX_RESTORED_PROJECTS,
                        crate::project::MAX_RESTORED_PROJECTS
                    );
                }
                let projects = s
                    .projects
                    .iter()
                    .take(crate::project::MAX_RESTORED_PROJECTS)
                    .map(|project| {
                        crate::project::project_from_session_with_budget(
                            project,
                            &mut remaining_threads,
                        )
                    })
                    .collect();

                if s.chats.len() > crate::project::MAX_RESTORED_CHATS {
                    log::warn!(
                        "session restore: {} chat(s) exceeds cap {}, restoring the first {}",
                        s.chats.len(),
                        crate::project::MAX_RESTORED_CHATS,
                        crate::project::MAX_RESTORED_CHATS
                    );
                }
                let chats = s
                    .chats
                    .iter()
                    .take(crate::project::MAX_RESTORED_CHATS)
                    .filter_map(|chat| {
                        crate::project::thread_from_session_with_budget(
                            chat,
                            &mut remaining_threads,
                        )
                    })
                    .collect();
                (projects, chats)
            })
            .unwrap_or_else(|| (Vec::new(), Vec::new()));
        // Bump the in-memory ID counters past anything the session
        // restored so a freshly-created project/thread/chat can never
        // collide with a restored ID (US-007's `bump_id_counters_to`
        // is idempotent and a no-op when the counters already lead).
        // US-002: chats share the `next_thread_id` counter, so they MUST
        // be folded into the bump or the next chat ID collides.
        crate::project::bump_id_counters_to(&restored_projects, &restored_chats);
        let restored_agents_target = saved_session
            .as_ref()
            .and_then(|s| s.agents_target.as_ref())
            .and_then(|target| {
                crate::project::agents_target_from_session(
                    target,
                    &restored_projects,
                    &restored_chats,
                )
            });
        let restored_active_project = match restored_agents_target {
            Some(crate::project::AgentsTarget::Thread { project_idx, .. }) => project_idx,
            _ => saved_session
                .as_ref()
                .map(|s| {
                    // Clamp to a valid index: a session.json hand-edit (or
                    // a future migration that drops projects) shouldn't
                    // leave `active_project_idx` pointing past the end.
                    if restored_projects.is_empty() {
                        0
                    } else {
                        s.active_project.min(restored_projects.len() - 1)
                    }
                })
                .unwrap_or(0),
        };

        let (workspaces, active_idx) = match saved_session {
            Some(session) => {
                log::info!(
                    "restoring session: {} workspace(s), {} project(s), mode={:?}",
                    session.workspaces.len(),
                    session.projects.len(),
                    session.mode
                );
                let (workspaces, active_idx) = Self::restore_workspaces(&session, cx);
                if workspaces.is_empty() {
                    log::warn!(
                        "session restore: session contained no restorable workspaces; creating default workspace"
                    );
                    (vec![Self::default_workspace(cx)], 0)
                } else {
                    (workspaces, active_idx)
                }
            }
            None => (vec![Self::default_workspace(cx)], 0),
        };

        // Setup notify file watcher for .git directories
        let (git_event_tx, git_event_rx) = std::sync::mpsc::channel();
        let mut git_watcher = match notify::recommended_watcher(git_event_tx) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("git file watcher unavailable: {e}. Falling back to polling.");
                None
            }
        };
        let mut git_watch_counts = std::collections::HashMap::new();
        // Watch all workspaces' .git directories
        if let Some(ref mut watcher) = git_watcher {
            for ws in &workspaces {
                if let Some(ref git_dir) = ws.git_dir {
                    if let Err(e) = watcher.watch(git_dir, notify::RecursiveMode::NonRecursive) {
                        log::warn!("git watcher: failed to watch {}: {e}", git_dir.display());
                    } else {
                        *git_watch_counts.entry(git_dir.clone()).or_insert(0) += 1;
                    }
                }
            }
        }

        // Poll git watcher events with 300ms debounce.
        // Filter: only HEAD and index matter. NonRecursive mode limits events to
        // top-level entries of .git/ so no subdirectory false positives.
        // On debounce fire, run git probes off main thread and apply results.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let debounce = std::time::Duration::from_millis(300);
                let mut last_event = std::time::Instant::now() - debounce;
                let mut pending = false;
                let mut pending_git_dirs = std::collections::HashSet::<std::path::PathBuf>::new();

                loop {
                    smol::Timer::after(std::time::Duration::from_millis(200)).await;

                    // Drain events from the watcher channel, collect affected .git dirs
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
                        Err(_) => break, // app shutting down
                    }

                    // Debounce: fire after 300ms of quiet
                    if pending && last_event.elapsed() >= debounce {
                        pending = false;
                        let affected_dirs = std::mem::take(&mut pending_git_dirs);
                        log::debug!(
                            "git watcher: debounced event fired for {} dir(s)",
                            affected_dirs.len()
                        );

                        // Collect CWDs of affected workspaces (main thread)
                        let cwds = cx.update(|cx| {
                            this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                                app.workspaces
                                    .iter()
                                    .filter(|ws| {
                                        ws.git_dir
                                            .as_ref()
                                            .is_some_and(|gd| affected_dirs.contains(gd))
                                    })
                                    .map(|ws| ws.cwd.clone())
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

                        // Run git probes off main thread
                        let results = smol::unblock(move || {
                            cwds.into_iter()
                                .map(|cwd| {
                                    let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                                    let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                                    (cwd, branch, is_repo, stats)
                                })
                                .collect::<Vec<_>>()
                        })
                        .await;

                        // Apply results to matching workspaces (main thread)
                        let apply = cx.update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                let mut changed = false;
                                let mut refreshed_diff = false;
                                for (cwd, branch, is_repo, stats) in &results {
                                    if app.apply_git_state_for_cwd(
                                        cwd,
                                        branch.clone(),
                                        *is_repo,
                                        stats.clone(),
                                    ) {
                                        changed = true;
                                        refreshed_diff |=
                                            app.refresh_agents_diff_if_open_for_cwd(cwd, cx);
                                    }
                                }
                                if changed && !refreshed_diff {
                                    cx.notify();
                                }
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

        // Files-sidebar watcher drain loop (EP-002 US-005). Mirrors the git
        // loop above: poll the per-open watch channel, coalesce affected parent
        // dirs, debounce ~100ms with a 500ms hard-flush ceiling (so a
        // continuous stream like `git checkout` still flushes), then re-read
        // only the affected cached directories. A notify overflow/`Rescan`
        // signal forces a root re-read (US-006 AC3).
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let debounce = std::time::Duration::from_millis(100);
                let ceiling = std::time::Duration::from_millis(500);
                const FILES_EVENT_DRAIN_MAX_PER_TICK: usize = 512;
                let mut first_pending: Option<std::time::Instant> = None;
                let mut last_event = std::time::Instant::now();
                let mut pending_dirs = std::collections::HashSet::<std::path::PathBuf>::new();
                let mut need_rescan = false;

                loop {
                    smol::Timer::after(std::time::Duration::from_millis(50)).await;

                    // Drain the watch channel → affected parent dirs + rescan flag.
                    let drained = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                            let mut dirs = Vec::new();
                            let mut rescan = false;
                            let mut watcher_failed = false;
                            let mut drained_events = 0usize;
                            if let Some(rx) = &app.files_event_rx {
                                for _ in 0..FILES_EVENT_DRAIN_MAX_PER_TICK {
                                    match rx.try_recv() {
                                        Ok(Ok(ev)) => {
                                            drained_events += 1;
                                            if ev.need_rescan() {
                                                rescan = true;
                                            }
                                            for p in &ev.paths {
                                                if let Some(parent) = p.parent() {
                                                    dirs.push(parent.to_path_buf());
                                                }
                                            }
                                        }
                                        Ok(Err(err)) => {
                                            drained_events += 1;
                                            log::warn!(
                                                "files watcher error: {err}; falling back to on-expand reads"
                                            );
                                            rescan = true;
                                            watcher_failed = true;
                                            break;
                                        }
                                        Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                            log::warn!(
                                                "files watcher disconnected; falling back to on-expand reads"
                                            );
                                            watcher_failed = true;
                                            break;
                                        }
                                    }
                                }
                                if drained_events == FILES_EVENT_DRAIN_MAX_PER_TICK {
                                    tracing::debug!(
                                        target: "paneflow_app::files_sidebar",
                                        "files watcher drain capped at {FILES_EVENT_DRAIN_MAX_PER_TICK} events for this tick"
                                    );
                                }
                            }
                            if watcher_failed {
                                app.files_watcher = None;
                                app.files_event_rx = None;
                            }
                            (dirs, rescan)
                        })
                    });

                    let (dirs, rescan) = match drained {
                        Ok(d) => d,
                        Err(_) => break, // app shutting down
                    };

                    if !dirs.is_empty() || rescan {
                        if first_pending.is_none() {
                            first_pending = Some(std::time::Instant::now());
                        }
                        last_event = std::time::Instant::now();
                        pending_dirs.extend(dirs);
                        need_rescan |= rescan;
                    }

                    // Fire after a quiet debounce window OR once the hard
                    // ceiling elapses under a continuous event stream.
                    let should_fire = first_pending.is_some_and(|start| {
                        last_event.elapsed() >= debounce || start.elapsed() >= ceiling
                    });
                    if should_fire {
                        first_pending = None;
                        let affected: Vec<std::path::PathBuf> =
                            std::mem::take(&mut pending_dirs).into_iter().collect();
                        let rescan = std::mem::replace(&mut need_rescan, false);
                        let applied = cx.update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.refresh_files_dirs(affected, rescan, cx);
                            })
                        });
                        if applied.is_err() {
                            break;
                        }
                    }
                }
            },
        )
        .detach();

        // Poll automation channels every 50 ms.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_millis(50)).await;
                    let result = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.process_automation_tick(cx);
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

        // Config hot-reload is now driven by ConfigWatcher (notify crate, 300ms debounce).
        // Changes are picked up in the 50ms IPC poll loop below via process_config_changes().

        // Fallback: poll git metadata for all workspaces every 30s.
        // Primary detection is event-driven (US-003 notify watcher above).
        // This timer catches edge cases where file system events are missed.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(30)).await;

                    // Phase 1: collect CWDs from workspaces + agents
                    // projects (cheap, main thread). Dedup so a cwd
                    // shared by a workspace and a project only fires
                    // one subprocess per tick.
                    let cwds = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, _cx: &mut Context<Self>| {
                            let mut seen = std::collections::HashSet::new();
                            let mut out = Vec::new();
                            for ws in &app.workspaces {
                                if seen.insert(ws.cwd.clone()) {
                                    out.push(ws.cwd.clone());
                                }
                            }
                            for p in &app.projects {
                                if seen.insert(p.cwd.clone()) {
                                    out.push(p.cwd.clone());
                                }
                            }
                            out
                        })
                    });
                    let cwds = match cwds {
                        Ok(c) => c,
                        Err(_) => break,
                    };

                    // Phase 2: run git probes off main thread
                    let results = smol::unblock(move || {
                        cwds.into_iter()
                            .map(|cwd| {
                                let (branch, is_repo) = crate::workspace::detect_branch(&cwd);
                                let stats = crate::workspace::GitDiffStats::from_cwd(&cwd);
                                (cwd, branch, is_repo, stats)
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;

                    // Phase 3: apply results (cheap, main thread)
                    let apply = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            let mut changed = false;
                            let mut refreshed_diff = false;
                            for (cwd, branch, is_repo, stats) in &results {
                                if app.apply_git_state_for_cwd(
                                    cwd,
                                    branch.clone(),
                                    *is_repo,
                                    stats.clone(),
                                ) {
                                    changed = true;
                                    refreshed_diff |=
                                        app.refresh_agents_diff_if_open_for_cwd(cwd, cx);
                                }
                            }
                            if changed && !refreshed_diff {
                                cx.notify();
                            }
                        })
                    });
                    if apply.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

        // Stale PID sweep: every 30s, probe registered AI agent PIDs with
        // kill(pid, 0) to detect crashed processes and clean up sidebar state.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(30)).await;
                    if cx
                        .update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.sweep_stale_pids(cx);
                            })
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            },
        )
        .detach();

        // Port scanning and CWD detection are now event-driven:
        // - TerminalEvent::ActivityBurst → schedule_port_scan()
        // - TerminalEvent::CwdChanged → handle_cwd_change()
        // See handle_terminal_event() for the push-based implementation.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(5)).await;
                    if cx
                        .update(|cx| {
                            this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                app.schedule_active_port_rescans(cx);
                            })
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            },
        )
        .detach();

        // US-008 - classify the install source once, then hand off to the
        // install-method hygiene migrations. Migrations are Linux-only and
        // the module itself is gated behind `#[cfg(target_os = "linux")]`,
        // so the call site needs the matching gate. On macOS / Windows the
        // tar.gz → rpm/deb crossover doesn't exist, so the helper isn't
        // compiled in at all.
        let install_method = update::install_method::detect();
        #[cfg(target_os = "linux")]
        update::migrations::run_startup_migrations(&install_method);

        // Compile-time env vars: the PostHog project key is injected by the
        // release pipeline; the host defaults to EU Cloud so a build that
        // omits the override still honours the PRD's EU-residency constraint.
        let posthog_api_key = option_env!("POSTHOG_API_KEY").unwrap_or("");
        let posthog_host = option_env!("POSTHOG_HOST").unwrap_or("https://eu.i.posthog.com");
        let telemetry_config_snapshot = paneflow_config::loader::load_config();
        let telemetry_enabled_last = telemetry_config_snapshot
            .telemetry
            .as_ref()
            .and_then(|t| t.enabled);
        let telemetry_consent = telemetry::client::TelemetryConsent::new(telemetry_enabled_last);
        // US-010/US-012 - resolve the anonymous telemetry_id only after the
        // consent and kill-switch gates pass. Opt-out, unanswered consent, and
        // env kill-switches must not create persistent telemetry state.
        let (telemetry_distinct_id, is_first_run_for_telemetry) =
            if telemetry::client::TelemetryClient::consent_allows_capture(telemetry_consent) {
                telemetry::id::telemetry_id_with_first_run()
            } else {
                (String::new(), false)
            };
        let telemetry = std::sync::Arc::new(telemetry::client::TelemetryClient::from_consent(
            telemetry_consent,
            posthog_api_key,
            posthog_host,
            &telemetry_distinct_id,
        ));
        // EP-003 US-008 (agent-control-plane): one-shot boot warn when AI
        // free-access mode is enabled, mirroring the PANEFLOW_IPC_SCRIPTING
        // boot warn in `ipc::start_server()`. Reuses the snapshot already
        // loaded for telemetry so the file is not re-read. The fence is
        // independent and defaults ON, so it does not warn.
        if telemetry_config_snapshot.ai_unrestricted_enabled() {
            tracing::warn!(
                "ai.unrestricted is ON; same-UID callers may auto-submit prompts to agent panes without PANEFLOW_IPC_SCRIPTING (toggle in Settings -> AI Agent)"
            );
        }
        // US-007: now that the telemetry client exists, fire off the
        // background update check. The detached worker emits
        // `update_check_started` immediately and `update_available`
        // only when both the version is greater AND an asset matched.
        let pending_update = update::checker::spawn_check(
            std::sync::Arc::clone(&telemetry),
            update::checker::UpdateCheckTrigger::Auto,
        );
        // Background flusher: every 5 s the client inspects its queue and
        // posts when the size or age threshold is met. Re-spawned when the
        // telemetry client is swapped by config reconciliation.
        Self::spawn_telemetry_flusher(std::sync::Arc::clone(&telemetry), cx);

        // US-009 - coexistence detection + one-time advisory toast. Runs
        // strictly after the US-008 icon migration so a same-session
        // upgrade→cleanup→toast chain stays in order. Detection is always
        // logged (AC: "helper is still called for logging") so duplicate
        // installs remain visible in debug transcripts even after the
        // marker has muted the toast.
        #[cfg(target_os = "linux")]
        if let Some(report) = update::migrations::detect_coexistent_install(&install_method) {
            log::info!(
                "paneflow: coexistent install detected - running from {} (this install); other install at {} (installed via {})",
                report.running_path.display(),
                report.other_path.display(),
                report.other_method_label,
            );
            if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
                let marker_path = update::migrations::coexistence_marker_path(&home);
                if !marker_path.exists() {
                    // Build the toast payload up front so the spawn closure
                    // captures owned strings, not borrowed locals.
                    let message = format!(
                        "Two PaneFlow installs detected. Running from {} (this install); other install at {} (installed via {}). Remove the unused install to avoid version drift.",
                        report.running_path.display(),
                        report.other_path.display(),
                        report.other_method_label,
                    );
                    let actions = vec![crate::ToastAction::OpenReleasesPage(
                        "https://paneflow.dev/download#multiple-installs".to_string(),
                    )];
                    let hold_ms = crate::TOAST_HOLD_MS * 4;
                    // `push_toast` needs `&mut Self` + `&mut Context<Self>`,
                    // but `Self` doesn't exist yet at this point in `new()`.
                    // Defer via `cx.spawn` - the first `Timer::after` yield
                    // lets the ctor finish and hands control back with a
                    // resolvable `WeakEntity<Self>`. Matches the established
                    // spawn pattern in this file (see git-watcher, port-scan,
                    // stale-PID sweep above).
                    cx.spawn(
                        async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                            smol::Timer::after(std::time::Duration::from_millis(1)).await;
                            let pushed = cx
                                .update(|cx| {
                                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                                        app.push_toast(message, actions, hold_ms, cx);
                                    })
                                })
                                .is_ok();
                            // Only persist the marker if the toast actually
                            // went out - a failed update means the app window
                            // is tearing down, in which case letting the toast
                            // recur next session is the right behaviour.
                            if pushed {
                                update::migrations::write_coexistence_marker(&marker_path);
                            }
                        },
                    )
                    .detach();
                }
            }
        }

        // US-008: the diff panel's persistent file filter. Observe it so each
        // keystroke re-renders the app (the TextInput only notifies itself).
        let diff_file_filter =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Filter files…", cx));
        cx.observe(&diff_file_filter, |_, _, cx| cx.notify())
            .detach();
        // The Agents sidebar search field (same pattern): a real single-line
        // TextInput, observed so each keystroke re-renders the sidebar to
        // re-filter (the TextInput only notifies itself).
        let agents_filter_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search threads", cx));
        cx.observe(&agents_filter_input, |_, _, cx| cx.notify())
            .detach();
        // Codex settings nav search field - same pattern: a real single-line
        // TextInput, observed so each keystroke re-renders the nav to re-filter.
        let settings_search_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search settings…", cx));
        cx.observe(&settings_search_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_template_name_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Workspace name", cx));
        cx.observe(&workspace_template_name_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_template_project_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Project path", cx));
        cx.observe(&workspace_template_project_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_pane_name_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Pane name", cx));
        cx.observe(&workspace_pane_name_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_pane_cwd_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Pane cwd", cx));
        cx.observe(&workspace_pane_cwd_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_pane_command_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "clear && bun dev", cx));
        cx.observe(&workspace_pane_command_input, |_, _, cx| cx.notify())
            .detach();
        let workspace_pane_prompt_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Prompt to prefill", cx));
        cx.observe(&workspace_pane_prompt_input, |_, _, cx| cx.notify())
            .detach();

        let cached_config = paneflow_config::loader::load_config();
        let effective_shortcuts = keybindings::effective_shortcuts(&cached_config.shortcuts);
        let rosetta_passive_display_enabled = cached_config.rosetta_show_passive_enabled();

        let mut app = Self {
            workspaces,
            active_idx,
            renaming_idx: None,
            rename_text: String::new(),
            pending_config,
            save_seq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            // US-014: hydrate the render-path config cache once at startup.
            cached_config,
            ipc_rx,
            ipc_status,
            event_bus,
            last_broadcast_gen: std::collections::HashMap::new(),
            title_bar,
            primary_sidebar_visible: true,
            primary_sidebar_animation: None,
            title_bar_files_menu_open: None,
            title_bar_help_menu_open: None,
            git_watcher,
            git_event_rx,
            git_watch_counts,
            settings_section: None,
            settings_scroll: gpui::ScrollHandle::new(),
            settings_drag: None,
            settings_search_input,
            terminal_dropdown: None,
            general_dropdown: None,
            workspace_template_dropdown: None,
            workspace_template_selected: None,
            workspace_template_detail_open: false,
            workspace_template_selected_pane: 0,
            workspace_template_status: None,
            workspace_template_name_input,
            workspace_template_project_input,
            workspace_pane_name_input,
            workspace_pane_cwd_input,
            workspace_pane_command_input,
            workspace_pane_prompt_input,
            mcp_status: None,
            mcp_install: None,
            mcp_busy: false,
            // US-040: `$HOME` is unset by default on Windows (canonical home is
            // `%USERPROFILE%`), so the raw `var("HOME")` produced an empty
            // string and the sidebar never collapsed any cwd to `~`. `dirs`
            // resolves the home dir on all three platforms.
            home_dir: dirs::home_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            sidebar_scroll: gpui::ScrollHandle::new(),
            effective_shortcuts,
            recording_shortcut_idx: None,
            settings_focus: cx.focus_handle(),
            mono_font_names: Vec::new(),
            font_dropdown_open: false,
            font_search: String::new(),
            // Reflect the persisted theme in the Themes-page selector: a light
            // theme shows the Light segment active, anything else Dark. (A prior
            // "System" choice resolves to whichever concrete theme it picked.)
            theme_mode: if crate::theme::active_theme().background.l > 0.5 {
                crate::ThemeMode::Light
            } else {
                crate::ThemeMode::Dark
            },
            workspace_menu_open: None,
            tab_menu_open: None,
            pending_pane_focus: None,
            profile_menu_open: None,
            agent_sessions: crate::AgentSessionsState {
                sessions_sidebar_open: false,
                sessions_sidebar_animation: None,
                sessions_by_agent: std::array::from_fn(|_| Vec::new()),
                sessions_omitted: [0; crate::agent_sessions::SESSION_AGENT_COUNT],
                sessions_cwd: None,
                sessions_surface_id: None,
                sessions_scroll: gpui::ScrollHandle::new(),
                sessions_scan_generation: 0,
                sessions_selected: 0,
                sessions_focus: cx.focus_handle(),
                sessions_group_collapsed: [false; crate::agent_sessions::SESSION_AGENT_COUNT],
                sessions_group_show_all: [false; crate::agent_sessions::SESSION_AGENT_COUNT],
                sessions_scanning: [false; crate::agent_sessions::SESSION_AGENT_COUNT],
            },
            files_sidebar_open: false,
            files_sidebar_animation: None,
            files_tree: crate::app::files_tree::FilesTreeState::default(),
            files_tree_scroll: gpui::ScrollHandle::new(),
            files_selected: 0,
            files_focus: cx.focus_handle(),
            files_surface_id: None,
            files_watcher: None,
            files_event_rx: None,
            files_menu_open: None,
            toast: None,
            toast_queue: std::collections::VecDeque::new(),
            _toast_task: None,
            #[cfg(target_os = "windows")]
            windows_backdrop_light: None,
            jump_cursor: None,
            swap_source: None,
            closed_panes: Vec::new(),
            show_about_dialog: false,
            show_theme_picker: false,
            theme_picker_query: String::new(),
            theme_picker_selected_idx: 0,
            theme_picker_focus: cx.focus_handle(),
            theme_picker_scroll: gpui::ScrollHandle::new(),
            theme_picker_drag: None,
            // EP-001 (cli-cockpit): Composer closed, no groups, no buffers.
            composer: None,
            broadcast: crate::app::broadcast::BroadcastState::default(),
            broadcast_picker_open: false,
            broadcast_picker_query: String::new(),
            broadcast_picker_selected: 0,
            broadcast_picker_renaming: None,
            broadcast_picker_error: None,
            broadcast_picker_focus: cx.focus_handle(),
            // EP-002 (cli-cockpit): Attention Queue + Launch Pad closed.
            attention_queue_open: false,
            attention_queue_selected: 0,
            attention_queue_focus: cx.focus_handle(),
            rosetta_recent_history: crate::app::rosetta::RosettaRecentHistory::default(),
            rosetta_surface_expanded: false,
            rosetta_surface_selected: 0,
            rosetta_surface_selected_key: None,
            rosetta_surface_focus: cx.focus_handle(),
            rosetta_surface_scroll: gpui::ScrollHandle::new(),
            rosetta_surface_pending_focus: false,
            rosetta_snoozed_rows: std::collections::HashMap::new(),
            rosetta_dismissed_rows: std::collections::HashSet::new(),
            rosetta_read_rows: std::collections::HashSet::new(),
            rosetta_passive_display_enabled,
            // EP-006 US-018 (cli-cockpit): fleet grep closed.
            fleet_search: None,
            fleet_search_generation: 0,
            fleet_search_focus: cx.focus_handle(),
            fleet_search_pending_focus: false,
            launch_pad: None,
            launch_pad_focus: cx.focus_handle(),
            self_update: crate::SelfUpdateState {
                pending_update,
                update_status: None,
                self_update_status: update::SelfUpdateStatus::default(),
                install_method,
                update_attempt_count: 0,
                download_generation: 0,
            },
            custom_buttons_modal: None,
            custom_buttons_modal_focus: cx.focus_handle(),
            telemetry,
            launch_instant: std::time::Instant::now(),
            telemetry_enabled_last,
            // US-006: shared signal flipped by the theme watcher's debounce
            // thread; drained by the 50 ms IPC loop to schedule a repaint.
            theme_changed,
            diff_mode: crate::DiffModeState {
                diff_view: None,
                multi_diff_view: None,
                diff_view_cache: std::collections::HashMap::new(),
                diff_view_key: None,
                multi_diff_view_retained: None,
                diff_collapsed_branches: std::collections::HashSet::new(),
                diff_discovering: false,
                diff_chosen_worktrees: std::collections::HashMap::new(),
                diff_worktree_picker_open: false,
                diff_available_worktrees: Vec::new(),
                diff_available_repo: None,
                diff_scope: restored_diff_scope,
                diff_scope_picker_open: false,
                diff_project_picker_open: false,
                diff_selected_file: None,
                diff_files_collapsed: false,
                diff_files_tree: false,
                diff_collapsed_dirs: std::collections::HashSet::new(),
                diff_file_filter,
            },
            // US-008 (prd-agents-view.md): start in the mode the user
            // left on quit. The Agents view is terminal-only and works
            // without any agent installed, so there is no agent-presence
            // gate on restore.
            mode: restored_mode,
            // US-007 + US-009 (prd-agents-view.md): rehydrate project
            // metadata from session.json. Empty for users on first
            // launch and for legacy session.json (the `#[serde(default)]`
            // annotations make missing fields resolve to empty).
            projects: restored_projects,
            // US-002: free chats restored from session (empty pre-refonte).
            chats: restored_chats,
            active_project_idx: restored_active_project,
            // US-003: restore the selected thread/chat when its stable ID still
            // exists after capping/filtering; otherwise start at picker/home.
            agents_target: restored_agents_target,
            // US-005: default picker context is the active project.
            agents_picker_context: crate::project::AgentsPickerContext::Project,
            // US-011: rename / context-menu / confirm-delete state.
            // All start empty; the affordance handlers set them in
            // response to user actions.
            agents_view: crate::AgentsViewState {
                agents_renaming: None,
                agents_rename_text: String::new(),
                agents_rename_input: None,
                agents_menu_open: None,
                agents_confirm_delete: None,
                agents_delete_armed: None,
                agents_filter_input,
                agents_skills_visible: false,
                agents_skills_tab: crate::agents_view::SkillsTab::default(),
                agents_skills: Vec::new(),
                agents_skills_loading: false,
                agents_skills_copied: None,
                sidebar_actions_menu_open: false,
                sidebar_mode_picker_open: false,
                agents_branch_menu: None,
                agents_environment_git: std::collections::HashMap::new(),
                agents_environment_panel_open: true,
                agents_editor_menu_open: false,
                agents_diff_open: false,
                agents_diff: None,
                agents_diff_collapsed: std::collections::HashSet::new(),
                agents_diff_expanded_folds: std::collections::HashSet::new(),
                agents_diff_split: false,
                agents_diff_generation: 0,
                agents_diff_scroll: gpui::ScrollHandle::new(),
                agents_diff_width: crate::app::agents_diff::AGENTS_DIFF_PANEL_WIDTH,
                agents_diff_resize: None,
                agents_diff_h_scroll_drag: None,
                agents_diff_h_offsets: std::rc::Rc::new(Vec::new()),
                bottom_panel_open: false,
                bottom_panel_height: crate::app::agents_bottom_panel::BOTTOM_PANEL_DEFAULT_HEIGHT,
                bottom_panel_active: None,
                bottom_terminals: Vec::new(),
                bottom_terminal_seq: 0,
                bottom_panel_drag: None,
                agents_terminal_view_cache: std::collections::HashMap::new(),
                agents_terminal_cache_lru: Vec::new(),
                agents_terminal_cache_touched_at: std::collections::HashMap::new(),
            },
            // US-012: sidebar search/filter. Empty filter == show
            // everything; the focus handle is held here so the input
            // captures Backspace/Escape/Down without conflicting with
            // the global app key chain.
            sidebar_order_cache: std::cell::RefCell::new(Default::default()),
        };

        for cwd in app
            .projects
            .iter()
            .map(|project| project.cwd.clone())
            .collect::<Vec<_>>()
        {
            app.spawn_agents_environment_git_refresh(cwd, cx);
        }
        if matches!(app.mode, paneflow_config::schema::AppMode::Agents)
            && let Some(target) = app.current_thread_view_target()
        {
            app.mount_agents_terminal_for_target(target, cx);
        }

        // US-015 (prd-git-diff-mode-2026-Q3.md): restore Diff mode only when
        // it is reconstructable. The diff derives its repo from the restored
        // active workspace (Project / Worktree) or any open repo (Multi-project),
        // so no separate repo-root needs persisting. If viable, mount the diff
        // for the restored scope; otherwise collapse to CLI so the window never
        // opens onto an empty diff.
        if matches!(app.mode, paneflow_config::schema::AppMode::Diff) {
            let viable = match app.diff_mode.diff_scope {
                crate::diff::DiffScope::MultiProject => {
                    app.workspaces.iter().any(|ws| ws.repo_root.is_some())
                }
                _ => app
                    .workspaces
                    .get(app.active_idx)
                    .is_some_and(|ws| ws.repo_root.is_some()),
            };
            if viable {
                app.rebuild_diff_view(cx);
            } else {
                app.mode = paneflow_config::schema::AppMode::Cli;
            }
        }

        // US-013 AC #1 - fire `app_started` once per launch. `Null` clients
        // (opt-out / unanswered consent / env kill-switch) no-op; only a
        // consenting user produces an HTTP call, batched on the flusher
        // above. Must happen after the struct literal so `self.telemetry`
        // and `self.self_update.install_method` are both populated.
        app.emit_app_started(is_first_run_for_telemetry);
        // US-006: emit the corruption event after the client is up.
        // `Null` clients (consent off / kill-switch active) make this
        // a no-op without a network call.
        if let Some(info) = session_corruption {
            app.emit_session_corrupted(&info);
        }

        // EP-002 (memory): opportunistically release exited cached agent
        // terminals after their idle TTL even if the user never selects another
        // thread. Running PTYs stay protected by the eviction guard.
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(std::time::Duration::from_secs(60)).await;
                    let result = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            let agents_terminal_visible =
                                matches!(app.mode, paneflow_config::schema::AppMode::Agents)
                                    && !app.agents_view.agents_skills_visible;
                            let active_thread_id = agents_terminal_visible
                                .then(|| {
                                    app.current_thread_view_target().and_then(|target| {
                                        app.thread_for_target(target).map(|thread| thread.id)
                                    })
                                })
                                .flatten();
                            app.enforce_agents_terminal_cache_budget(active_thread_id, cx);
                            let bottom_panel_visible =
                                matches!(app.mode, paneflow_config::schema::AppMode::Agents)
                                    && app.agents_view.bottom_panel_open;
                            app.enforce_bottom_terminal_cache_budget(bottom_panel_visible, cx);
                        })
                    });
                    if result.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

        // Custom-button propagation runs once on the active workspace so
        // user-defined tab-bar buttons surface immediately after restore.
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(1)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        let active = app.active_idx;
                        if let Some(ws) = app.workspaces.get_mut(active) {
                            ws.propagate_custom_buttons(cx);
                        }
                    })
                });
            },
        )
        .detach();
        app
    }
}

// ---------------------------------------------------------------------------
// Free helper functions called from `fn main()` (US-002 extraction).
// ---------------------------------------------------------------------------

/// Build the copy-pasteable upgrade command for a system-package install.
///
/// `version` is safe to interpolate into a shell string without escaping: it
/// comes from `UpdateStatus::Available { version }`, which is set from a
/// `semver::Version::to_string()` - the semver parser rejects any input that
/// would survive into `;`/`$()`/whitespace/bidi, so malformed GitHub tags
/// short-circuit to `UpdateStatus::Failed` long before this function runs.
///
/// Version format notes:
/// - apt pinning uses `name=upstream-debrev`. `cargo-deb` emits `-1` as the
///   debian revision by default, so `paneflow=<v>-1` targets the exact tag.
/// - dnf accepts `name-upstream` as a NEVR prefix match. The `<v>` we pass is
///   already the raw upstream version from GitHub Releases.
/// - `PackageManager::Other` gets a plain-English hint rather than a command,
///   because we don't know the syntax (eopkg/xbps/apk all differ).
pub(crate) fn system_package_update_command(
    manager: Option<&update::install_method::PackageManager>,
    version: &str,
) -> String {
    match manager {
        Some(update::install_method::PackageManager::Apt) => {
            format!("sudo apt update && sudo apt install paneflow={version}-1")
        }
        Some(update::install_method::PackageManager::Dnf) => {
            // Match the validated pkexec argv (`dnf --refresh install …`): plain
            // `dnf upgrade <pkg>-<ver>` hits "No match for argument" right after
            // a publish because the cached metadata predates the new version.
            // `--refresh install` forces a metadata sync and installs the exact
            // version (dnf treats install of a higher version as an upgrade).
            format!("sudo dnf --refresh install paneflow-{version}")
        }
        // US-004: `rpm-ostree upgrade` takes no package argument - it
        // rebases the whole deployment. Version string is intentionally
        // NOT included, unlike the apt/dnf arms.
        Some(update::install_method::PackageManager::RpmOstree) => "rpm-ostree upgrade".to_string(),
        Some(update::install_method::PackageManager::Other) | None => {
            "Update PaneFlow via your system's package manager".to_string()
        }
    }
}

/// Install the macOS menu bar.
///
/// US-012: three top-level menus - PaneFlow / Edit / Window - populated with
/// the actions listed in the PRD. The `PaneFlow` menu name matches the
/// `CFBundleName` from the future US-013 Info.plist (AC6). Keyboard shortcuts
/// are derived from the global keybindings table (e.g. Quit shows `⌘Q`
/// because US-010's `MACOS_ONLY_DEFAULTS` binds `cmd-q → quit`; Window items
/// show `⌘⇧N` / `⌘⇧Q` / `⌘Tab` from US-009's `secondary-*` bindings).
/// Copy / Paste / Select All carry an `OsAction` hint so macOS routes them
/// through the native responder chain and renders `⌘C` / `⌘V` / `⌘A`.
#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_bar(cx: &mut gpui::App) {
    use gpui::{Menu, MenuItem, OsAction};

    use crate::{
        About, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp, Paste, Quit, SelectAll,
    };

    cx.set_menus(vec![
        Menu::new("PaneFlow").items(vec![
            MenuItem::action("About PaneFlow", About),
            MenuItem::separator(),
            MenuItem::action("Quit PaneFlow", Quit),
        ]),
        Menu::new("Edit").items(vec![
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::separator(),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("Window").items(vec![
            MenuItem::action("New Workspace", NewWorkspace),
            MenuItem::action("Close Workspace", CloseWorkspace),
            MenuItem::separator(),
            MenuItem::action("Next Workspace", NextWorkspace),
        ]),
        // macOS convention: every app ships a Help menu (even if it only
        // points to an online doc/repo). Without one, Apple's HIG-conforming
        // users perceive the app as unfinished. "PaneFlow Help" dispatches
        // `OpenHelp` which opens the GitHub README in the default browser.
        Menu::new("Help").items(vec![MenuItem::action("PaneFlow Help", OpenHelp)]),
    ]);
}

/// Register macOS menu actions as app-global fallbacks.
///
/// AppKit validates menu items via GPUI's `is_action_available`, which checks
/// the focused dispatch path plus app-global listeners. PaneFlow's normal
/// handlers live on the rendered root element so keyboard/menu dispatch works
/// while that root is in the focused path, but macOS can validate the native
/// menu while focus sits in a terminal/Agents surface whose current rendered
/// path does not expose the root listeners. These fallbacks make the native
/// menu items consistently enabled and mirror the root handlers when they are
/// otherwise unreachable.
#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_action_fallbacks(cx: &mut gpui::App) {
    use crate::{
        About, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp, PaneFlowApp, Paste,
        Quit, SelectAll, TerminalCopy, TerminalPaste,
    };

    fn with_active_paneflow_window(
        cx: &mut gpui::App,
        f: impl FnOnce(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>),
    ) {
        let Some(window) = cx.active_window() else {
            return;
        };
        let Some(window) = window.downcast::<PaneFlowApp>() else {
            return;
        };
        if let Err(err) = window.update(cx, f) {
            log::debug!("macOS menu fallback: active PaneFlow window unavailable: {err}");
        }
    }

    cx.on_action(|_: &Quit, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            app.save_session_blocking(cx);
            app.emit_app_exited_and_flush();
            cx.quit();
        });
    });

    cx.on_action(|_: &About, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            app.show_about_dialog = true;
            cx.notify();
        });
    });

    cx.on_action(|_: &Copy, cx| cx.dispatch_action(&TerminalCopy));
    cx.on_action(|_: &Paste, cx| cx.dispatch_action(&TerminalPaste));
    cx.on_action(|_: &SelectAll, _cx| {
        log::debug!("Edit > Select All dispatched (terminal select-all not yet wired)");
    });

    cx.on_action(|_: &NewWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.create_workspace_with_picker(window, cx);
        });
    });
    cx.on_action(|_: &CloseWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.close_workspace_at(app.active_idx, window, cx);
        });
    });
    cx.on_action(|_: &NextWorkspace, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            if !app.workspaces.is_empty() {
                let next = (app.active_idx + 1) % app.workspaces.len();
                app.select_workspace(next, window, cx);
            }
        });
    });

    cx.on_action(|_: &OpenHelp, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            if let Err(e) = open::that("https://github.com/ArthurDEV44/paneflow#readme") {
                log::warn!("Help > PaneFlow Help: could not open browser: {e}");
                app.show_toast(format!("Could not open help: {e}"), cx);
            }
        });
    });
}

/// Detect whether the Apple Silicon binary is running under Rosetta 2
/// translation on an Intel Mac (or, more commonly, an Intel binary on
/// Apple Silicon - which Apple translates transparently). Either way it
/// warns once at startup so a user who grabbed the wrong `.dmg` knows
/// why GPU performance is degraded instead of silently eating the hit.
///
/// Edge case 4 of the macOS port PRD. Uses `sysctl.proc_translated`: returns
/// `1` for a translated process, `0` native, ENOENT → native Intel kernel
/// (no Rosetta available at all). Failure to read the sysctl is silent -
/// this warning is diagnostic, not load-bearing.
#[cfg(target_os = "macos")]
pub(crate) fn warn_if_rosetta_translated() {
    use std::ffi::CString;
    use std::mem::size_of;

    let name = match CString::new("sysctl.proc_translated") {
        Ok(n) => n,
        Err(_) => return,
    };
    let mut translated: i32 = 0;
    let mut size = size_of::<i32>();
    // SAFETY: `sysctlbyname` reads a small integer into a stack buffer whose
    // size is passed by pointer. `name.as_ptr()` is a valid NUL-terminated
    // C string from a CString we just constructed. `translated` and `size`
    // are live stack variables for the duration of the call. Zero-initialized
    // buffer means a kernel short-write can't expose uninitialized memory.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut translated as *mut _ as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 && translated == 1 {
        log::warn!(
            "running under Rosetta 2 translation - GPU rendering will be \
             degraded. For best performance, download the matching \
             architecture from https://github.com/ArthurDEV44/paneflow/releases"
        );
    }
}

/// The old `.run` installer (removed in US-007) dropped a standalone binary
/// at `~/.local/bin/paneflow`. The new tar.gz installer instead drops a
/// `~/.local/paneflow.app/` directory and symlinks `~/.local/bin/paneflow`
/// into it. We warn when the old layout is detected so users know why the
/// in-app updater can no longer fetch a `.run` asset (there are none).
pub(crate) fn warn_if_legacy_run_install() {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return;
    };
    let app_dir = home.join(".local/paneflow.app");
    let legacy_bin = home.join(".local/bin/paneflow");

    let legacy_bin_is_regular_file = legacy_bin
        .symlink_metadata()
        .map(|m| m.file_type().is_file())
        .unwrap_or(false);

    if !app_dir.exists() && legacy_bin_is_regular_file {
        log::warn!(
            "legacy .run install detected at {} - see README for migration \
             to the .tar.gz / .deb / .AppImage formats",
            legacy_bin.display()
        );
    }
}
