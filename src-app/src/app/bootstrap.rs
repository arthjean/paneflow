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

        let blink_phase = cx.new(|_| BlinkPhase::default());
        cx.set_global(BlinkPhaseGlobal(blink_phase.clone()));
        crate::theme::install_theme_signal(cx);
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(CURSOR_BLINK_INTERVAL).await;
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

        let pending_config = std::sync::Arc::new(std::sync::Mutex::new(
            None::<paneflow_config::schema::PaneFlowConfig>,
        ));
        let pending_config_writer = std::sync::Arc::clone(&pending_config);
        let running_config_watcher = paneflow_config::watcher::ConfigWatcher::new(
            std::sync::Arc::new(move |cfg: paneflow_config::schema::PaneFlowConfig| {
                *pending_config_writer
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(cfg);
            }),
        )
        .and_then(|config_watcher| match config_watcher.start() {
            Ok(running) => Some(running),
            Err(error) => {
                log::warn!("config watcher failed to start: {error}; config hot-reload disabled");
                None
            }
        });

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

        let (saved_session, session_corruption) = Self::load_session();

        let restored_mode = saved_session.as_ref().map(|s| s.mode).unwrap_or_default();
        let restored_diff_scope = saved_session
            .as_ref()
            .and_then(|s| s.diff_scope.as_deref())
            .and_then(crate::diff::DiffScope::from_persisted)
            .unwrap_or_default();

        let (workspaces, active_idx) = match saved_session {
            Some(session) => {
                log::info!(
                    "restoring session: {} workspace(s), mode={:?}",
                    session.workspaces.len(),
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
                                            app.refresh_diff_dock_if_open_for_cwd(cwd, cx);
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

        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                loop {
                    smol::Timer::after(crate::app::agent_status::REGISTRY_POLL_INTERVAL).await;
                    let alive = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            app.sweep_claude_session_registry(cx);
                        })
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let _config_watcher = running_config_watcher;
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
                                        app.refresh_diff_dock_if_open_for_cwd(cwd, cx);
                                }
                            }
                            if changed && !refreshed_diff {
                                cx.notify();
                            }
                            app.refresh_pull_requests(cx);
                        })
                    });
                    if apply.is_err() {
                        break;
                    }
                }
            },
        )
        .detach();

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

        let install_method = update::install_method::detect();
        #[cfg(target_os = "linux")]
        update::migrations::run_startup_migrations(&install_method);

        let posthog_api_key = option_env!("POSTHOG_API_KEY").unwrap_or("");
        let posthog_host = option_env!("POSTHOG_HOST").unwrap_or("https://eu.i.posthog.com");
        let telemetry_config_snapshot = paneflow_config::loader::load_config();
        let telemetry_enabled_last = telemetry_config_snapshot
            .telemetry
            .as_ref()
            .and_then(|t| t.enabled);
        let telemetry_consent =
            telemetry::client::TelemetryConsent::from_config(telemetry_enabled_last);
        let (telemetry_client, is_first_run_for_telemetry) =
            telemetry::client::TelemetryClient::from_consent(
                telemetry_consent,
                posthog_api_key,
                posthog_host,
                telemetry::id::telemetry_id_with_first_run,
            );
        let telemetry = std::sync::Arc::new(telemetry_client);
        if telemetry_config_snapshot.ai_unrestricted_enabled() {
            tracing::debug!(
                "ai.unrestricted is ON; same-UID callers may auto-submit prompts to agent panes without PANEFLOW_IPC_SCRIPTING (toggle in Settings -> AI Agent)"
            );
        }
        let pending_update = update::checker::spawn_check(std::sync::Arc::clone(&telemetry));
        Self::spawn_telemetry_flusher(std::sync::Arc::clone(&telemetry), cx);

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
                            if pushed {
                                update::migrations::write_coexistence_marker(&marker_path);
                            }
                        },
                    )
                    .detach();
                }
            }
        }

        let diff_file_filter =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Filter files…", cx));
        cx.observe(&diff_file_filter, |_, _, cx| cx.notify())
            .detach();
        let agents_filter_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search threads", cx));
        cx.observe(&agents_filter_input, |_, _, cx| cx.notify())
            .detach();
        let files_sidebar = cx.new(crate::app::files_sidebar::FilesSidebar::new);
        cx.subscribe(&files_sidebar, Self::handle_files_event)
            .detach();
        let settings_search_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search settings…", cx));
        cx.observe(&settings_search_input, |_, _, cx| cx.notify())
            .detach();
        let shortcut_search_input = cx.new(|cx| {
            crate::widgets::text_input::TextInput::new("", "Search actions or keys…", cx)
        });
        cx.observe(&shortcut_search_input, |this: &mut Self, _, cx| {
            this.rebuild_shortcut_rows(cx);
            cx.notify();
        })
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

        let rename_input = cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Name", cx));
        cx.observe(&rename_input, |_, _, cx| cx.notify()).detach();

        let cached_config = paneflow_config::loader::load_config();
        let effective_shortcuts = keybindings::effective_shortcuts(&cached_config.shortcuts);
        let theme_mode = crate::ThemeMode::from_config(
            cached_config.theme_mode.as_deref(),
            cached_config.theme.as_deref(),
        );

        let mut app = Self {
            workspaces,
            active_idx,
            renaming_tab: None,
            rename_input,
            rename_focus_live: false,
            pending_config,
            save_seq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
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
            claude_registry_seen: std::collections::HashMap::new(),
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
            sidebar_scroll: gpui::ScrollHandle::new(),
            effective_shortcuts,
            recording_shortcut_idx: None,
            shortcut_search_input,
            shortcut_capture_active: false,
            shortcut_reset_pending: false,
            collapsed_shortcut_groups: std::collections::HashSet::new(),
            shortcut_rows: Vec::new(),
            shortcut_list: crate::settings::tabs::shortcuts::new_shortcut_list_state(),
            shortcut_drag: None,
            settings_focus: cx.focus_handle(),
            mono_font_names: Vec::new(),
            font_dropdown_open: false,
            theme_dropdown_open: false,
            font_search: String::new(),
            theme_mode,
            workspace_menu_open: None,
            worktree_states: crate::app::tab_worktree::WorktreeStates::default(),
            branch_checkout_pending: None,
            pr_states: crate::app::pull_request::PrStates::default(),
            sidebar_customize_menu_open: false,
            sidebar_show_submenu_open: false,
            tab_menu_open: None,
            pane_menu_open: None,
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
            files_sidebar,
            files_sidebar_root: None,
            files_sidebar_workspace: None,
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
            system_info_dialog: None,
            show_theme_picker: false,
            theme_picker_query: String::new(),
            theme_picker_selected_idx: 0,
            theme_picker_focus: cx.focus_handle(),
            theme_picker_scroll: gpui::ScrollHandle::new(),
            theme_picker_drag: None,
            composer: None,
            broadcast: crate::app::broadcast::BroadcastState::default(),
            broadcast_picker_open: false,
            broadcast_picker_query: String::new(),
            broadcast_picker_selected: 0,
            broadcast_picker_renaming: None,
            broadcast_picker_error: None,
            broadcast_picker_focus: cx.focus_handle(),
            attention_queue_open: false,
            attention_queue_selected: 0,
            attention_queue_focus: cx.focus_handle(),
            fleet_search: None,
            fleet_search_generation: 0,
            fleet_search_focus: cx.focus_handle(),
            fleet_search_pending_focus: false,
            launch_pad: None,
            launch_pad_focus: cx.focus_handle(),
            pane_palette: None,
            pane_palette_focus: cx.focus_handle(),
            pending_palette_focus: false,
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
            theme_changed,
            diff_mode: crate::DiffModeState {
                diff_view: None,
                multi_diff_view: None,
                diff_view_cache: std::collections::HashMap::new(),
                diff_view_key: None,
                multi_diff_view_retained: None,
                diff_collapsed_branches: std::collections::HashSet::new(),
                diff_discovering: false,
                diff_discovering_root: None,
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
            mode: restored_mode,
            diff_dock: crate::DiffDockState {
                open: false,
                data: None,
                collapsed: std::collections::HashSet::new(),
                expanded_folds: std::collections::HashSet::new(),
                split: true,
                generation: 0,
                scroll: gpui::ScrollHandle::new(),
                diff_options_menu_open: false,
                diff_options_submenu: None,
                diff_options: crate::diff::DiffOptions::default(),
                diff_new_tab_menu_open: false,
                picker: false,
                picked: false,
                owner: None,
                parked: std::collections::HashMap::new(),
                diff_tabs: vec![crate::app::diff_dock::DiffDockTab::Changes],
                diff_active_tab: 0,
                diff_tab_close_armed: None,
                diff_branch_menu: None,
                width: crate::app::diff_dock::DIFF_DOCK_PANEL_WIDTH,
                resize: None,
                h_scroll_drag: None,
                h_offsets: std::rc::Rc::new(Vec::new()),
                hover: None,
            },
            sidebar_order_cache: std::cell::RefCell::new(Default::default()),
        };

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

        app.emit_app_started(is_first_run_for_telemetry);
        if let Some(info) = session_corruption {
            app.emit_session_corrupted(&info);
        }

        crate::ui_primitives::set_reduce_motion(app.cached_config.reduce_motion_enabled());

        app
    }
}

pub(crate) fn system_package_update_command(
    manager: Option<&update::install_method::PackageManager>,
    version: &str,
) -> String {
    match manager {
        Some(update::install_method::PackageManager::Apt) => {
            format!("sudo apt update && sudo apt install paneflow={version}-1")
        }
        Some(update::install_method::PackageManager::Dnf) => {
            format!("sudo dnf --refresh install paneflow-{version}")
        }
        Some(update::install_method::PackageManager::Zypper) => {
            format!(
                "sudo zypper --non-interactive --gpg-auto-import-keys refresh && sudo zypper --non-interactive install --no-recommends --force paneflow={version}"
            )
        }
        Some(update::install_method::PackageManager::RpmOstree) => "rpm-ostree upgrade".to_string(),
        Some(update::install_method::PackageManager::Other) | None => {
            "Update PaneFlow via your system's package manager".to_string()
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_bar(cx: &mut gpui::App) {
    use gpui::{Menu, MenuItem, OsAction};

    use crate::{
        About, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp, Paste, Quit, SelectAll,
        ShowSystemInfo,
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
        Menu::new("Help").items(vec![
            MenuItem::action("PaneFlow Help", OpenHelp),
            MenuItem::separator(),
            MenuItem::action("System Info…", ShowSystemInfo),
        ]),
    ]);
}

#[cfg(target_os = "macos")]
pub(crate) fn install_macos_menu_action_fallbacks(cx: &mut gpui::App) {
    use crate::{
        About, CloseWorkspace, Copy, NewWorkspace, NextWorkspace, OpenHelp, PaneFlowApp, Paste,
        Quit, SelectAll, ShowSystemInfo, TerminalCopy, TerminalPaste,
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

    cx.on_action(|_: &ShowSystemInfo, cx| {
        with_active_paneflow_window(cx, |app, window, cx| {
            app.open_system_info_dialog(window, cx);
        });
    });

    cx.on_action(|_: &OpenHelp, cx| {
        with_active_paneflow_window(cx, |app, _window, cx| {
            if let Err(e) =
                crate::external_open::open_url("https://github.com/arthjean/paneflow#readme")
            {
                log::warn!("Help > PaneFlow Help: could not open browser: {e}");
                app.show_toast(format!("Could not open help: {e}"), cx);
            }
        });
    });
}

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
             architecture from https://github.com/arthjean/paneflow/releases"
        );
    }
}

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
