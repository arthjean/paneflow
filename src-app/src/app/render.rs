use crate::*;

const PRIMARY_SIDEBAR_ANIMATION_MS: u64 = 280;

pub(crate) const PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA: f32 = 0.5;

#[derive(Clone, Copy)]
pub(crate) struct SidebarWidthAnimation {
    pub(crate) from_width: f32,
    pub(crate) to_width: f32,
    pub(crate) started_at: std::time::Instant,
}

impl SidebarWidthAnimation {
    pub(crate) fn width_at(self, now: std::time::Instant) -> f32 {
        let duration = std::time::Duration::from_millis(PRIMARY_SIDEBAR_ANIMATION_MS);
        let progress = (now.duration_since(self.started_at).as_secs_f32() / duration.as_secs_f32())
            .clamp(0., 1.);
        let eased = 1. - (1. - progress).powi(3);
        self.from_width + (self.to_width - self.from_width) * eased
    }

    pub(crate) fn finishes_at(self) -> std::time::Instant {
        self.started_at + std::time::Duration::from_millis(PRIMARY_SIDEBAR_ANIMATION_MS)
    }

    pub(crate) fn is_finished(self, now: std::time::Instant) -> bool {
        now.duration_since(self.started_at)
            >= std::time::Duration::from_millis(PRIMARY_SIDEBAR_ANIMATION_MS)
    }
}

impl PaneFlowApp {
    fn primary_sidebar_expanded_width(&self) -> f32 {
        if self.settings_section.is_some() {
            crate::settings::chrome::SETTINGS_NAV_WIDTH
        } else {
            SIDEBAR_WIDTH
        }
    }

    fn primary_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if self.settings_section.is_some() {
            return crate::settings::chrome::SETTINGS_NAV_WIDTH;
        }
        if let Some(animation) = self.primary_sidebar_animation {
            animation.width_at(now)
        } else if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        }
    }

    fn rendered_primary_sidebar_width(&mut self, window: &mut Window) -> f32 {
        if self.settings_section.is_some() {
            self.primary_sidebar_animation = None;
            return crate::settings::chrome::SETTINGS_NAV_WIDTH;
        }

        let now = std::time::Instant::now();
        if let Some(animation) = self.primary_sidebar_animation {
            if animation.is_finished(now) {
                self.primary_sidebar_animation = None;
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        }
    }

    pub(crate) fn toggle_primary_sidebar(&mut self, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.primary_sidebar_width_at(now);
        self.primary_sidebar_visible = !self.primary_sidebar_visible;

        if self.settings_section.is_some() {
            self.primary_sidebar_animation = None;
            cx.notify();
            return;
        }

        let to_width = if self.primary_sidebar_visible {
            self.primary_sidebar_expanded_width()
        } else {
            0.
        };

        self.primary_sidebar_animation = if !crate::ui_primitives::reduce_motion()
            && (from_width - to_width).abs() > PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA
        {
            Some(SidebarWidthAnimation {
                from_width,
                to_width,
                started_at: now,
            })
        } else {
            None
        };
        cx.notify();
    }
}

impl Render for PaneFlowApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        startup_trace::on_app_render(window);
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        #[cfg(target_os = "windows")]
        {
            let is_light = theme.background.l > 0.5;
            if self.windows_backdrop_light != Some(is_light) {
                crate::window_chrome::backdrop::sync_wallpaper_mica_theme(window, is_light);
                self.windows_backdrop_light = Some(is_light);
            }
        }
        let chrome_material_suppressed =
            native_material_suppressed_by_fullscreen(window.is_fullscreen());
        #[cfg(target_os = "macos")]
        crate::window_chrome::macos_backdrop::sync_subtle_sidebar_material(
            theme.background.l > 0.5,
            self.cached_config.macos_chrome_material_enabled() && !chrome_material_suppressed,
        );
        let title_bar_h =
            (1.75 * window.rem_size()).max(crate::app::constants::TITLE_BAR_MIN_HEIGHT);
        let settings_open = self.settings_section.is_some();
        let sessions_sidebar_width = self.rendered_sessions_sidebar_width(window);
        let sessions_sidebar_mounted = self.agent_sessions.sessions_sidebar_open
            || self.agent_sessions.sessions_sidebar_animation.is_some();
        let sessions_sidebar_opacity = (sessions_sidebar_width
            / crate::app::sessions_sidebar::SESSIONS_SIDEBAR_WIDTH.max(1.))
        .clamp(0., 1.);
        let secondary_sidebar_open = sessions_sidebar_mounted;
        let right_rail_width = if sessions_sidebar_mounted {
            sessions_sidebar_width
        } else {
            0.
        };
        let terminal_material_active = self.cached_config.windows_terminal_material_enabled();
        let chrome_material_active =
            self.cached_config.cockpit_chrome_material_enabled() && !chrome_material_suppressed;
        let terminal_surface_mounted = self
            .active_workspace()
            .is_some_and(|ws| ws.active_tab().root.is_some());
        let terminal_material_visible =
            !settings_open && terminal_surface_mounted && terminal_material_active;
        let panel_inset_shell_visible = terminal_material_visible && !chrome_material_active;
        let native_material_active = native_backdrop_material_active(
            settings_open,
            terminal_material_active,
            chrome_material_active,
        );
        let is_window_active = window.is_window_active();
        let shell_color = if is_window_active {
            theme.title_bar_background
        } else {
            theme.title_bar_inactive_background
        };
        let opaque_shell_bg = gpui::Hsla {
            a: 1.,
            ..shell_color
        };
        let app_backdrop_bg =
            crate::app::constants::cockpit_backdrop_background(shell_color, native_material_active);
        let panel_bg = if settings_open {
            ui.base
        } else {
            gpui::transparent_black()
        };
        let panel_corner_mask_bg =
            crate::app::constants::cockpit_backdrop_background(shell_color, chrome_material_active);
        let panel_top = title_bar_h;
        let primary_sidebar_width = self.rendered_primary_sidebar_width(window);
        let title_bar_rail_width = self.primary_sidebar_expanded_width();
        let primary_sidebar_mounted = self.settings_section.is_some()
            || self.primary_sidebar_visible
            || self.primary_sidebar_animation.is_some();
        let primary_sidebar_opacity = if self.settings_section.is_some() {
            1.
        } else {
            (primary_sidebar_width / self.primary_sidebar_expanded_width().max(1.)).clamp(0., 1.)
        };
        let panel_edge_share = 1. - primary_sidebar_opacity;
        let main_panel_left_inset = crate::app::constants::PANEL_INSET * panel_edge_share;
        let pane_grid_left_gutter = crate::app::constants::PANE_OUTER_GUTTER * panel_edge_share;
        let pane_grid_right_gutter = if self.diff_dock_visible() {
            (crate::layout::PANE_GUTTER_PX + crate::app::constants::PANE_OUTER_GUTTER) / 2.
        } else {
            crate::app::constants::PANE_OUTER_GUTTER
        };
        let main_panel_corner_mask_bg = panel_corner_mask_bg;
        let main_panel_width = f32::from(window.viewport_size().width)
            - primary_sidebar_width
            - right_rail_width
            - main_panel_left_inset
            - crate::app::constants::PANEL_INSET;
        #[cfg(target_os = "linux")]
        crate::window_chrome::linux_backdrop::refresh_blur_region(window);

        if let Some(pane) = self.pending_pane_focus.take()
            && !Self::focus_pane_window(pane.clone(), cx)
        {
            pane.read(cx).focus_handle(cx).focus(window, cx);
        }
        if std::mem::take(&mut self.pending_palette_focus) {
            window.focus(&self.pane_palette_focus, cx);
        }
        if let Some(idx) = self.take_pane_palette_pending_launch() {
            self.pane_palette_launch(idx, window, cx);
        }
        let rename_focus = self.rename_input.read(cx).focus_handle.clone();
        let rename_live = self.renaming_tab.is_some();
        if rename_live != self.rename_focus_live {
            self.rename_focus_live = rename_live;
            if rename_live {
                window.focus(&rename_focus, cx);
            } else if rename_focus.is_focused(window)
                && let Some(ws) = self.workspaces.get(self.active_idx)
            {
                ws.focus_first(window, cx);
            }
        } else if rename_live && !rename_focus.is_focused(window) {
            self.commit_rename(cx);
            self.rename_focus_live = false;
        }
        self.prune_stale_split_palette(cx);
        self.ensure_pane_palette_for_empty_workspace(cx);
        if self.workspaces.is_empty()
            && self.settings_section.is_none()
            && window.focused(cx).is_none()
        {
            window.focus(&self.welcome_focus, cx);
        }
        let main_content = if self.settings_section.is_some() {
            self.tick_agents_list_animation(window);
            self.render_settings_content_panel(window, cx)
                .into_any_element()
        } else if let Some(ws) = self.active_workspace() {
            if let Some(root) = &ws.active_tab().root {
                let app_weak = cx.weak_entity();
                let on_resize_end = std::rc::Rc::new(move |cx: &mut App| {
                    let _ = app_weak.update(cx, |app, cx| app.save_session(cx));
                });
                root.sync_unfocused_dim(window, cx);
                let outer = div()
                    .flex()
                    .size_full()
                    .pl(px(pane_grid_left_gutter))
                    .pr(px(pane_grid_right_gutter))
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::app::constants::PANE_OUTER_GUTTER));
                let preview = self.pending_split_palette().map(|(target, direction)| {
                    crate::layout::SplitPreview {
                        target,
                        direction,
                        element: std::cell::RefCell::new(Some(self.render_pane_palette(cx))),
                    }
                });
                outer
                    .child(root.render_with_preview(
                        window,
                        cx,
                        Some(on_resize_end),
                        preview.as_ref(),
                    ))
                    .into_any_element()
            } else if self
                .pane_palette
                .as_ref()
                .is_some_and(|palette| palette.ws_id == ws.id)
            {
                div()
                    .flex()
                    .size_full()
                    .pl(px(pane_grid_left_gutter))
                    .pr(px(pane_grid_right_gutter))
                    .pt(px(crate::layout::PANE_GUTTER_PX))
                    .pb(px(crate::app::constants::PANE_OUTER_GUTTER))
                    .child(self.render_pane_palette(cx))
                    .into_any_element()
            } else {
                div().size_full().into_any_element()
            }
        } else {
            div()
                .flex()
                .size_full()
                .pl(px(pane_grid_left_gutter))
                .pr(px(pane_grid_right_gutter))
                .pt(px(crate::layout::PANE_GUTTER_PX))
                .pb(px(crate::app::constants::PANE_OUTER_GUTTER))
                .child(self.render_welcome(cx))
                .into_any_element()
        };
        let main_content = self.wrap_cli_diff_dock(
            main_content,
            main_panel_width,
            pane_grid_left_gutter,
            window,
            cx,
        );
        let ws_name = if self.settings_section.is_some() {
            None
        } else {
            self.active_workspace().map(|ws| ws.title.clone())
        };
        let update_info = self.update_pill_info();
        self.title_bar.update(cx, |tb, _| {
            tb.workspace_name = ws_name;
            tb.sidebar_visible = self.primary_sidebar_visible;
            tb.left_rail_width = title_bar_rail_width;
            tb.files_menu_open = self.title_bar_files_menu_open.is_some();
            tb.help_menu_open = self.title_bar_help_menu_open.is_some();
            tb.update_available = update_info;
            tb.update_check = self.update_check_pill();
            tb.ipc_state = self.ipc_status.state();
            tb.cockpit = true;
            tb.cockpit_material_active = chrome_material_active;
        });

        let mut app_content = div()
            .font_family("Geist")
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .cursor(CursorStyle::Arrow)
            .on_action(cx.listener(Self::handle_split_h))
            .on_action(cx.listener(Self::handle_split_v))
            .on_action(cx.listener(Self::handle_close_pane))
            .on_action(cx.listener(Self::handle_hide_pane))
            .on_action(cx.listener(Self::handle_stop_session))
            .on_action(cx.listener(Self::handle_resume_ended_sessions))
            .on_action(cx.listener(Self::handle_remove_ended_sessions))
            .on_action(cx.listener(Self::handle_new_tab))
            .on_action(cx.listener(Self::handle_close_tab))
            .on_action(cx.listener(Self::handle_next_tab))
            .on_action(cx.listener(Self::handle_previous_tab))
            .on_action(cx.listener(Self::handle_focus_left))
            .on_action(cx.listener(Self::handle_focus_right))
            .on_action(cx.listener(Self::handle_focus_up))
            .on_action(cx.listener(Self::handle_focus_down))
            .on_action(cx.listener(Self::handle_jump_next_waiting))
            .on_action(cx.listener(Self::handle_new_workspace))
            .on_action(cx.listener(Self::handle_close_workspace))
            .on_action(cx.listener(Self::handle_copy_workspace_path))
            .on_action(cx.listener(Self::handle_reveal_workspace_in_file_manager))
            .on_action(cx.listener(Self::handle_open_workspace_in_zed))
            .on_action(cx.listener(Self::handle_open_workspace_in_cursor))
            .on_action(cx.listener(Self::handle_open_workspace_in_vscode))
            .on_action(cx.listener(Self::handle_open_workspace_in_windsurf))
            .on_action(cx.listener(Self::handle_next_workspace))
            .on_action(cx.listener(Self::handle_toggle_zoom))
            .on_action(cx.listener(Self::handle_toggle_detached_pane))
            .on_action(cx.listener(Self::handle_layout_even_h))
            .on_action(cx.listener(Self::handle_layout_even_v))
            .on_action(cx.listener(Self::handle_layout_main_v))
            .on_action(cx.listener(Self::handle_layout_tiled))
            .on_action(cx.listener(Self::handle_split_equalize))
            .on_action(cx.listener(Self::handle_swap_pane))
            .on_action(cx.listener(Self::handle_undo_close_pane))
            .on_action(cx.listener(Self::handle_ws1))
            .on_action(cx.listener(Self::handle_ws2))
            .on_action(cx.listener(Self::handle_ws3))
            .on_action(cx.listener(Self::handle_ws4))
            .on_action(cx.listener(Self::handle_ws5))
            .on_action(cx.listener(Self::handle_ws6))
            .on_action(cx.listener(Self::handle_ws7))
            .on_action(cx.listener(Self::handle_ws8))
            .on_action(cx.listener(Self::handle_ws9))
            .on_action(cx.listener(|this: &mut Self, _: &Quit, _window, cx| {
                this.request_quit(cx);
            }))
            .on_action(cx.listener(|this: &mut Self, _: &About, _window, cx| {
                this.show_about_dialog = true;
                cx.notify();
            }))
            .on_action(cx.listener(|_this: &mut Self, _: &Copy, _window, cx| {
                cx.dispatch_action(&TerminalCopy);
            }))
            .on_action(cx.listener(|_this: &mut Self, _: &Paste, _window, cx| {
                cx.dispatch_action(&TerminalPaste);
            }))
            .on_action(cx.listener(|_this: &mut Self, _: &SelectAll, _window, cx| {
                cx.dispatch_action(&TerminalSelectAll);
            }))
            .on_action(
                cx.listener(|this: &mut Self, _: &ShowSystemInfo, window, cx| {
                    this.open_system_info_dialog(window, cx);
                }),
            )
            .on_action(cx.listener(|this: &mut Self, _: &OpenHelp, _window, cx| {
                this.open_documentation(cx);
            }))
            .on_action(
                cx.listener(|this: &mut Self, _: &OpenSettings, window, cx| {
                    this.open_settings_window(window, cx);
                }),
            )
            .on_action(
                cx.listener(|this: &mut Self, _: &CheckForUpdates, _window, cx| {
                    this.request_update_check(cx);
                }),
            )
            .on_action(cx.listener(Self::handle_start_self_update))
            .on_action(cx.listener(Self::handle_dismiss_update))
            .on_action(cx.listener(Self::handle_toggle_files_sidebar))
            .on_action(cx.listener(Self::handle_focus_workspaces_sidebar))
            .on_action(cx.listener(Self::handle_toggle_diff_dock_maximize))
            .on_action(cx.listener(Self::handle_open_composer))
            .on_action(cx.listener(Self::handle_toggle_broadcast_member))
            .on_action(cx.listener(Self::handle_open_broadcast_groups))
            .on_action(cx.listener(Self::handle_open_attention_queue))
            .on_action(cx.listener(Self::handle_open_command_palette))
            .on_action(cx.listener(Self::handle_clone_repository))
            .on_action(cx.listener(Self::handle_diff_new_file_tab))
            .on_action(cx.listener(Self::handle_diff_new_terminal_tab))
            .capture_key_down(cx.listener(|_this, e: &gpui::KeyDownEvent, window, cx| {
                if cx.has_active_drag() && e.keystroke.key == "escape" {
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                }
            }))
            .on_mouse_move(|_e, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .overflow_hidden()
                    .relative()
                    .when(
                        terminal_material_visible && primary_sidebar_mounted,
                        |row| {
                            row.child(
                                div()
                                    .absolute()
                                    .left_0()
                                    .top_0()
                                    .bottom_0()
                                    .w(px(primary_sidebar_width))
                                    .bg(panel_corner_mask_bg),
                            )
                        },
                    )
                    .when(terminal_material_visible && secondary_sidebar_open, |row| {
                        row.child(
                            div()
                                .absolute()
                                .right_0()
                                .top_0()
                                .bottom_0()
                                .w(px(sessions_sidebar_width))
                                .bg(panel_corner_mask_bg),
                        )
                    })
                    .when(primary_sidebar_mounted, |row| {
                        if self.settings_section.is_some() {
                            return row.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .h_full()
                                    .w(px(primary_sidebar_width))
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .pt(title_bar_h)
                                    .child(self.render_settings_nav(window, cx))
                                    .into_any_element(),
                            );
                        }
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(primary_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(primary_sidebar_opacity)
                                .pt(title_bar_h)
                                .child(self.render_sidebar(window, cx)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .relative()
                            .flex()
                            .flex_col()
                            .child(div().h(title_bar_h).flex_none())
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .overflow_hidden()
                                    .bg(panel_bg)
                                    .ml(px(main_panel_left_inset))
                                    .mr(px(crate::app::constants::PANEL_INSET))
                                    .mb(px(crate::app::constants::PANEL_INSET))
                                    .rounded(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .capture_any_mouse_down(cx.listener(
                                        |this, event: &gpui::MouseDownEvent, _window, cx| {
                                            if event.button == gpui::MouseButton::Left
                                                && this.settings_section.is_none()
                                            {
                                                this.acknowledge_visible_completions(cx);
                                            }
                                        },
                                    ))
                                    .child(main_content),
                            )
                            .when(panel_inset_shell_visible, |panel_shell| {
                                panel_shell
                                    .child(
                                        div()
                                            .absolute()
                                            .right_0()
                                            .top(panel_top)
                                            .bottom_0()
                                            .w(px(crate::app::constants::PANEL_INSET))
                                            .bg(opaque_shell_bg),
                                    )
                                    .child(
                                        div()
                                            .absolute()
                                            .left_0()
                                            .right_0()
                                            .bottom_0()
                                            .h(px(crate::app::constants::PANEL_INSET))
                                            .bg(opaque_shell_bg),
                                    )
                                    .when(main_panel_left_inset > 0., |panel_shell| {
                                        panel_shell.child(
                                            div()
                                                .absolute()
                                                .left_0()
                                                .top(panel_top)
                                                .bottom_0()
                                                .w(px(main_panel_left_inset))
                                                .bg(opaque_shell_bg),
                                        )
                                    })
                            })
                            .child(
                                div()
                                    .absolute()
                                    .left(px(main_panel_left_inset))
                                    .top(panel_top)
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::TopLeft,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(crate::app::constants::PANEL_INSET))
                                    .top(panel_top)
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::TopRight,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(main_panel_left_inset))
                                    .bottom(px(crate::app::constants::PANEL_INSET))
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::BottomLeft,
                                        main_panel_corner_mask_bg,
                                    )),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .right(px(crate::app::constants::PANEL_INSET))
                                    .bottom(px(crate::app::constants::PANEL_INSET))
                                    .size(crate::app::constants::PANEL_CORNER_RADIUS)
                                    .child(panel_corner_mask(
                                        PanelCorner::BottomRight,
                                        main_panel_corner_mask_bg,
                                    )),
                            ),
                    )
                    .when(sessions_sidebar_mounted, |row| {
                        row.child(
                            div()
                                .flex()
                                .flex_col()
                                .h_full()
                                .w(px(sessions_sidebar_width))
                                .flex_shrink_0()
                                .overflow_hidden()
                                .opacity(sessions_sidebar_opacity)
                                .pt(title_bar_h)
                                .child(self.render_sessions_sidebar(cx))
                                .into_any_element(),
                        )
                    }),
            );

        {
            app_content = app_content.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .w_full()
                    .overflow_hidden()
                    .child(self.title_bar.clone()),
            );
        }

        if let Some(toast) = &self.toast {
            app_content = app_content.child(self.render_toast(toast, ui, cx));
        }

        if let Some(anchor) = self.title_bar_files_menu_open {
            app_content = app_content.child(self.render_title_bar_files_menu(anchor, window, cx));
        }

        if let Some(anchor) = self.title_bar_help_menu_open {
            app_content = app_content.child(self.render_title_bar_help_menu(anchor, window, cx));
        }

        if self.broadcast_picker_open {
            app_content = app_content.child(self.render_broadcast_picker(cx));
        }

        if self.attention_queue_open {
            app_content = app_content.child(self.render_attention_queue(cx));
        }
        if self.branch_prompt.is_some() {
            app_content = app_content.child(self.render_branch_prompt(cx));
        }
        if self.fleet_search.is_some() {
            if std::mem::take(&mut self.fleet_search_pending_focus) {
                self.fleet_search_focus.focus(window, cx);
            }
            app_content = app_content.child(self.render_fleet_search(cx));
        }

        if self.clone_repo.is_some() {
            app_content = app_content.child(self.render_clone_repo(cx));
        }
        if self.command_palette_open {
            app_content = app_content.child(self.render_command_palette(window, cx));
        }
        if self.custom_buttons_modal.is_some() {
            app_content = app_content.child(self.render_custom_buttons_modal(cx));
        }

        if self.show_about_dialog {
            app_content = app_content.child(self.render_about_dialog(cx));
        }

        if self.system_info_dialog.is_some() {
            app_content = app_content.child(self.render_system_info_dialog(cx));
        }

        if self.quit_dialog.is_some() {
            app_content = app_content.child(self.render_quit_dialog(window, cx));
        }

        if self.close_dialog.is_some() {
            app_content = app_content.child(self.render_close_dialog(window, cx));
        }

        if self.worktree_remove_dialog.is_some() {
            app_content = app_content.child(self.render_worktree_remove_dialog(window, cx));
        }

        if let Some(menu) = self.workspace_menu_open
            && menu.idx < self.workspaces.len()
        {
            app_content =
                app_content.child(self.render_workspace_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.session_menu_open.clone() {
            app_content = app_content.child(self.render_session_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.tab_menu_open
            && self
                .workspaces
                .get(menu.ws_idx)
                .is_some_and(|ws| menu.tab_idx < ws.tab_count())
        {
            app_content = app_content.child(self.render_tab_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.pane_menu_open.clone() {
            app_content = app_content.child(self.render_pane_context_menu(menu, ui, window, cx));
        }

        if let Some(menu) = self.files_menu_open.clone() {
            app_content = app_content.child(self.render_files_context_menu(menu, ui, window, cx));
        }

        let shell = crate::window_chrome::csd::client_side_window_shell(
            app_content,
            window,
            app_backdrop_bg,
            if terminal_material_visible {
                gpui::transparent_black()
            } else {
                ui.border
            },
        );
        startup_trace::on_app_render_built();
        shell
    }
}
