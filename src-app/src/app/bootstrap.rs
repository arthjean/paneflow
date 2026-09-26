use gpui::{AppContext, Context};

use crate::telemetry;
use crate::terminal::blink::{BlinkPhase, BlinkPhaseGlobal, CURSOR_BLINK_INTERVAL};
use crate::window_chrome::title_bar;
use crate::{PaneFlowApp, ipc, keybindings, update};

impl PaneFlowApp {
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
        crate::startup_trace::mark("ipc_server_started");
        cx.spawn(
            async |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::unblock(crate::agent_launcher::refresh_installed_binaries).await;
                let _ = this.update(cx, |_app: &mut Self, cx: &mut Context<Self>| cx.notify());
            },
        )
        .detach();

        let blink_phase = cx.new(|_| BlinkPhase::default());
        cx.set_global(BlinkPhaseGlobal(blink_phase.clone()));
        crate::theme::install_theme_signal(cx);
        Self::spawn_cursor_blink(cx);

        let (pending_config, running_config_watcher) = Self::start_config_watcher();

        let theme_changed = Self::start_theme_watcher();

        let cached_config = paneflow_config::loader::load_config();
        crate::terminal::element::apply_font_config(&cached_config);
        let (saved_session, session_corruption) = Self::load_session();
        let session_restore_failed = session_corruption.is_some();
        crate::startup_trace::mark("session_loaded");

        let pending_detached_panes = saved_session
            .as_ref()
            .map(|session| session.detached_panes.clone())
            .unwrap_or_default();

        let mut pull_request_seeds = Vec::new();
        let (workspaces, active_idx, session_restored) = match saved_session {
            Some(session) => {
                log::info!(
                    "restoring session: {} workspace(s)",
                    session.workspaces.len()
                );
                let (workspaces, active_idx) = Self::restore_workspaces(&session, cx);
                pull_request_seeds = Self::pull_request_seeds(&session, &workspaces);
                if workspaces.is_empty() {
                    log::info!(
                        "session restore: no restorable workspace; opening the welcome screen"
                    );
                    (workspaces, 0, false)
                } else {
                    (workspaces, active_idx, true)
                }
            }
            None => (Vec::new(), 0, false),
        };
        crate::startup_trace::mark("workspaces_restored");

        let (git_watcher, git_event_rx, git_watch_counts) = Self::start_git_watcher(&workspaces);

        Self::spawn_git_event_refresh(cx);
        Self::spawn_automation_tick(running_config_watcher, cx);
        Self::spawn_git_poll(cx);
        Self::spawn_stale_pid_sweep(cx);
        Self::spawn_port_rescans(cx);

        let install_method = update::install_method::detect();
        #[cfg(target_os = "linux")]
        update::migrations::run_startup_migrations(&install_method);

        let (posthog_api_key, posthog_host) = super::telemetry_events::posthog_endpoint();
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
                "ai.unrestricted is ON; same-UID callers may auto-submit prompts to agent panes without PANEFLOW_IPC_SCRIPTING (toggle in Settings -> Agents)"
            );
        }
        let (pending_update, check_trigger) =
            update::checker::spawn_check(std::sync::Arc::clone(&telemetry));
        Self::spawn_telemetry_flusher(std::sync::Arc::clone(&telemetry), cx);

        #[cfg(target_os = "linux")]
        Self::schedule_coexistence_toast(&install_method, cx);

        Self::schedule_release_toast(cx);

        let agents_filter_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search threads", cx));
        cx.observe(&agents_filter_input, |_, _, cx| cx.notify())
            .detach();
        let files_sidebar = cx.new(crate::app::files_sidebar::FilesSidebar::new);
        cx.subscribe(&files_sidebar, Self::handle_files_event)
            .detach();
        let settings_search_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search settings…", cx));
        cx.observe(&settings_search_input, |this: &mut Self, _, cx| {
            this.follow_settings_search(cx);
            cx.notify();
        })
        .detach();
        let shortcut_search_input = cx
            .new(|cx| crate::widgets::text_input::TextInput::new("", "Filter by action name", cx));
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

        let agent_profile_name_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Claude perso", cx));
        cx.observe(&agent_profile_name_input, |_, _, cx| cx.notify())
            .detach();
        let agent_profile_args_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "--model opus", cx));
        cx.observe(&agent_profile_args_input, |_, _, cx| cx.notify())
            .detach();

        let rename_input = cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Name", cx));
        let sidebar_filter_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Filter", cx));
        let command_palette_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search commands…", cx));
        cx.observe(
            &command_palette_input,
            |this: &mut PaneFlowApp, input, cx| {
                let value = input.read(cx).value();
                if this.command_palette_open && value != this.command_palette_query_seen {
                    this.command_palette_query_seen = value;
                    this.command_palette_selected = 0;
                    this.command_palette_scroll.scroll_to_item(0);
                }
                cx.notify();
            },
        )
        .detach();
        cx.observe(&sidebar_filter_input, |_, _, cx| cx.notify())
            .detach();
        cx.observe(&rename_input, |_, _, cx| cx.notify()).detach();

        crate::workspace::worktree::set_worktrees_root(cached_config.worktrees.dir_path());
        let effective_shortcuts = keybindings::effective_shortcuts(&cached_config.shortcuts);
        let theme_mode = crate::ThemeMode::from_config(
            cached_config.theme_mode.as_deref(),
            cached_config.theme.as_deref(),
        );

        crate::startup_trace::mark("app_fields_prepared");
        let mut app = Self {
            pending_detached_panes,
            workspaces,
            active_idx,
            renaming_tab: None,
            sidebar_focus: cx.focus_handle(),
            sidebar_cursor: None,
            rename_input,
            sidebar_filter_input,
            sidebar_filter_hovered: false,
            sidebar_row_motion: std::cell::RefCell::new(Default::default()),
            rename_focus_live: false,
            pending_config,
            save_seq: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            session_restore_failed,
            session_exit_pending: false,
            session_save_error_shown: false,
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
            settings_search_motion: Default::default(),
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
            agent_profile_editor: None,
            agents_list_expanded: false,
            agents_list_animation: None,
            integration_status: None,
            integration_busy: None,
            integration_errors: std::collections::HashMap::new(),
            agent_profile_name_input,
            agent_profile_args_input,
            mcp_status: None,
            mcp_install: None,
            mcp_busy: false,
            sidebar_scroll: gpui::ScrollHandle::new(),
            effective_shortcuts,
            recording_shortcut_idx: None,
            shortcut_search_input,
            shortcut_capture_active: false,
            shortcut_reset_pending: false,
            shortcut_conflict: None,
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
            session_menu_open: None,
            worktree_states: crate::app::tab_worktree::WorktreeStates::default(),
            branch_checkout_pending: None,
            pr_states: crate::app::pull_request::PrStates::default(),
            sidebar_customize_menu_open: false,
            sidebar_show_submenu_open: false,
            tab_menu_open: None,
            pane_menu_open: None,
            pending_pane_focus: None,
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
            owned_sessions: Default::default(),
            resume_batch: None,
            close_dialog: None,
            close_dialog_focus: cx.focus_handle(),
            worktree_remove_dialog: None,
            worktree_remove_focus: cx.focus_handle(),
            host_agents: Default::default(),
            show_about_dialog: false,
            system_info_dialog: None,
            quit_dialog: None,
            quit_dialog_focus: cx.focus_handle(),
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
            branch_prompt: None,
            branch_prompt_focus: cx.focus_handle(),
            recent_workspaces: crate::app::recents::load_pruned(),
            welcome_focus: cx.focus_handle(),
            clone_repo: None,
            clone_repo_focus: cx.focus_handle(),
            command_palette_open: false,
            command_palette_input,
            command_palette_selected: 0,
            command_palette_query_seen: String::new(),
            command_palette_scroll: gpui::ScrollHandle::new(),
            command_palette_scope: None,
            command_palette_context: Default::default(),
            command_palette_restore_focus: None,
            pane_palette: None,
            pane_palette_focus: cx.focus_handle(),
            pending_palette_focus: false,
            self_update: crate::SelfUpdateState {
                pending_update,
                check_trigger,
                manual_check: None,
                update_status: None,
                self_update_status: update::SelfUpdateStatus::default(),
                install_method,
                update_attempt_count: 0,
                download_generation: 0,
                dismissed_version: None,
                staged_msi: None,
            },
            custom_buttons_modal: None,
            custom_buttons_modal_focus: cx.focus_handle(),
            telemetry,
            launch_instant: std::time::Instant::now(),
            telemetry_enabled_last,
            theme_changed,
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
                diff_tabs: Vec::new(),
                diff_active_tab: 0,
                diff_tab_close_armed: None,
                diff_branch_menu: None,
                width: crate::app::diff_dock::DIFF_DOCK_PANEL_WIDTH,
                maximized: None,
                maximize_animation: None,
                reveal_animation: None,
                pane_grid_width: std::rc::Rc::default(),
                resize: None,
                h_scroll_drag: None,
                vertical_scrollbar: Default::default(),
                h_offsets: std::rc::Rc::new(Vec::new()),
                hover: None,
            },
            sidebar_order_cache: std::cell::RefCell::new(Default::default()),
        };

        if session_restored {
            let restored_paths =
                crate::app::recents::restored_session_paths(&app.workspaces, app.active_idx);
            app.record_recent_workspaces(&restored_paths, cx);
            let restored_terminals = app.attached_terminals(cx);
            app.track_resume_batch(restored_terminals, cx);
        }

        for (repo_root, branch, pr) in pull_request_seeds {
            app.pr_states
                .seed(&repo_root.to_string_lossy(), &branch, pr);
        }
        app.refresh_pull_requests(cx);
        app.refresh_owned_sessions(cx);
        app.start_host_agent_stream();

        app.emit_app_started(is_first_run_for_telemetry);
        if let Some(info) = session_corruption {
            app.emit_session_corrupted(&info);
        }

        crate::ui_primitives::set_reduce_motion(app.cached_config.reduce_motion_enabled());
        crate::app::diff_dock::code::controls::set_editor_display(
            crate::app::diff_dock::code::controls::EditorDisplay::from_config(
                &app.cached_config.editor,
            ),
        );

        app
    }

    fn spawn_cursor_blink(cx: &mut Context<Self>) {
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
    }

    fn start_config_watcher() -> (
        std::sync::Arc<std::sync::Mutex<Option<paneflow_config::schema::PaneFlowConfig>>>,
        Option<paneflow_config::watcher::RunningConfigWatcher>,
    ) {
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
        (pending_config, running_config_watcher)
    }

    fn start_theme_watcher() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
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
        theme_changed
    }

    fn spawn_automation_tick(
        running_config_watcher: Option<paneflow_config::watcher::RunningConfigWatcher>,
        cx: &mut Context<Self>,
    ) {
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
