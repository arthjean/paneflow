use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, Pixels, SharedString, Styled, Window, div, img, prelude::*, px,
    rgb, svg,
};

use crate::PaneFlowApp;
use crate::agent_launcher::AgentCommandSpec;
use crate::agent_sessions::{SessionAgent, SessionMeta, format_relative_time};
use crate::app::ipc_handler::find_pane_by_surface_id;
use crate::pane_drag::{DragPreview, SessionDrag};
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

pub(crate) const SESSIONS_SIDEBAR_WIDTH: f32 = 300.;
const ROW_HEIGHT: Pixels = px(30.);

impl PaneFlowApp {
    pub(crate) fn open_sessions_sidebar_for_pane(
        &mut self,
        pane: &gpui::Entity<crate::pane::Pane>,
        focus_window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        let terminal = pane.read(cx).active_terminal_opt();
        let surface_id = terminal.as_ref().map(|tv| tv.entity_id().as_u64());
        let cwd_str = terminal.as_ref().and_then(|tv| {
            let view = tv.read(cx);
            view.terminal.current_cwd.clone().or_else(|| {
                view.terminal
                    .cwd_now()
                    .map(|p| p.to_string_lossy().into_owned())
            })
        });

        if self.files_sidebar_open {
            self.close_files_sidebar(cx);
        }

        self.dismiss_transient_surfaces();

        self.set_sessions_sidebar_open(true, cx);
        self.agent_sessions.sessions_cwd = cwd_str.clone();
        self.agent_sessions.sessions_surface_id = surface_id;
        for sessions in &mut self.agent_sessions.sessions_by_agent {
            sessions.clear();
        }
        self.agent_sessions.sessions_omitted = [0; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_group_collapsed =
            [false; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_group_show_all =
            [false; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_scanning = [false; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_selected = 0;
        self.agent_sessions.sessions_scan_generation =
            self.agent_sessions.sessions_scan_generation.wrapping_add(1);
        let scan_generation = self.agent_sessions.sessions_scan_generation;
        let enabled_agents =
            crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config);
        self.agent_sessions.sessions_scroll = gpui::ScrollHandle::new();

        if let Some(window) = focus_window {
            self.agent_sessions.sessions_focus.focus(window, cx);
        }

        if let Some(cwd) = cwd_str {
            for agent in enabled_agents {
                self.spawn_sessions_scan(agent, cwd.clone(), scan_generation, cx);
            }
        }
        cx.notify();
    }

    fn spawn_sessions_scan(
        &mut self,
        agent: SessionAgent,
        cwd: String,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let idx = agent_index(agent);
        self.agent_sessions.sessions_scanning[idx] = true;
        cx.spawn(async move |this, cx| {
            let scan_cwd = cwd.clone();
            let started = std::time::Instant::now();
            let (sessions, omitted) = smol::unblock(move || {
                crate::agent_sessions::read_sessions_for_cwd_with_omitted(agent, &scan_cwd)
            })
            .await;
            let elapsed = started.elapsed();
            let retained = sessions.len();
            log::debug!(
                "agent sessions scan {:?} cwd={} retained={} omitted={} elapsed={:?}",
                agent,
                cwd,
                retained,
                omitted,
                elapsed
            );
            let _ = this.update(cx, |app, cx| {
                if should_apply_scan_result(
                    app.agent_sessions.sessions_sidebar_open,
                    app.agent_sessions.sessions_cwd.as_deref(),
                    &cwd,
                    app.agent_sessions.sessions_scan_generation,
                    generation,
                ) {
                    *app.sessions_for_mut(agent) = sessions;
                    app.agent_sessions.sessions_omitted[idx] = omitted;
                    app.agent_sessions.sessions_scanning[idx] = false;
                    app.clamp_sessions_selection();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn render_sessions_sidebar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        div()
            .id("sessions-sidebar")
            .flex()
            .flex_col()
            .w(px(SESSIONS_SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .track_focus(&self.agent_sessions.sessions_focus)
            .on_key_down(cx.listener(Self::handle_sessions_sidebar_key_down))
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .child(self.sessions_sidebar_header(ui, cx))
            .child(self.sessions_sidebar_body(ui, cx))
            .into_any_element()
    }

    fn sessions_sidebar_header(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .h(px(46.))
            .flex_none()
            .px(px(12.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child("Agent sessions"),
                    )
                    .when_some(self.agent_sessions.sessions_cwd.as_deref(), |d, cwd| {
                        d.child(
                            div()
                                .id("sessions-sidebar-cwd")
                                .overflow_x_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .text_color(ui.muted)
                                .delayed_tooltip(crate::ui_primitives::text_tooltip(
                                    cwd.to_string(),
                                ))
                                .child(compact_cwd_label(cwd)),
                        )
                    }),
            )
            .child(
                div()
                    .id("sessions-sidebar-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded(px(5.))
                    .text_size(px(14.))
                    .text_color(ui.muted)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(
                                hover_background.opacity(0.0),
                                hover_background,
                                delta,
                            ))
                            .text_color(lerp_color(ui.muted, ui.text, delta));
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_sessions_sidebar(cx);
                        cx.stop_propagation();
                    }))
                    .child("×"),
            )
            .into_any_element()
    }

    fn sessions_sidebar_body(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.agent_sessions.sessions_cwd.is_none() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .p(px(14.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("Could not detect the terminal's working directory."),
                )
                .into_any_element();
        }

        let enabled =
            crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config);
        if enabled.is_empty() {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .p(px(14.))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(ui.muted)
                        .child("No AI agents enabled. Enable one in Settings → AI Agent."),
                )
                .into_any_element();
        }

        let mut body = div()
            .id("sessions-sidebar-body")
            .flex()
            .flex_col()
            .flex_1()
            .py(px(6.))
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.agent_sessions.sessions_scroll);

        let selected = self.selected_session_target();
        let mut groups_rendered = 0usize;
        let mut scanning_any = false;
        for agent in enabled {
            let idx = agent_index(agent);
            let scanning = self.agent_sessions.sessions_scanning[idx];
            scanning_any |= scanning;
            if scanning || !self.sessions_for(agent).is_empty() {
                groups_rendered += 1;
                body = body.child(self.sessions_group(agent, ui, selected, cx));
            }
        }
        if groups_rendered == 0 {
            let message = if scanning_any {
                "Scanning sessions..."
            } else {
                "No sessions for this directory yet."
            };
            body = body.child(
                div()
                    .p(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child(message),
            );
        }
        body.into_any_element()
    }

    fn sessions_group(
        &self,
        agent: SessionAgent,
        ui: crate::theme::UiColors,
        selected: Option<SessionNavTarget<'_>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let idx = agent_index(agent);
        let collapsed = self.agent_sessions.sessions_group_collapsed[idx];
        let show_all = self.agent_sessions.sessions_group_show_all[idx];
        let scanning = self.agent_sessions.sessions_scanning[idx];
        let sessions = self.sessions_for(agent);
        let omitted = self.sessions_omitted_for(agent);
        let chevron = if collapsed {
            "icons/chevron-right.svg"
        } else {
            "icons/chevron-down.svg"
        };

        let header = div()
            .id(SharedString::from(format!(
                "sessions-group-{}",
                agent_id_prefix(agent)
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .pt(px(12.))
            .pb(px(4.))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.agent_sessions.sessions_focus.focus(window, cx);
                this.agent_sessions.sessions_group_collapsed[idx] =
                    !this.agent_sessions.sessions_group_collapsed[idx];
                this.clamp_sessions_selection();
                cx.notify();
            }))
            .child(agent_icon_element(agent, px(14.), ui))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.muted)
                    .child(agent_label(agent)),
            )
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path(chevron)
                    .text_color(ui.muted),
            );

        let mut group = div().flex().flex_col().child(header);

        if collapsed {
            return group.into_any_element();
        }

        if sessions.is_empty() {
            let msg: SharedString = if scanning {
                SharedString::from("Scanning\u{2026}")
            } else {
                empty_message(agent)
            };
            group = group.child(
                div()
                    .mx(px(14.))
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(px(11.))
                    .text_color(ui.muted.opacity(0.8))
                    .child(msg),
            );
        } else {
            let (visible, remaining) = visible_window(sessions.len(), show_all, CAP);
            for session in sessions.iter().take(visible) {
                group = group.child(self.sessions_row(
                    session,
                    ui,
                    selected.is_some_and(|target| {
                        target.agent == agent && target.session_id == session.session_id
                    }),
                    cx,
                ));
            }
            if sessions.len() > CAP {
                let hover_background = crate::app::constants::sidebar_tab_hover_background();
                let label: SharedString = if show_all {
                    SharedString::from("Show less")
                } else {
                    format!("Show {remaining} more").into()
                };
                group = group.child(
                    div()
                        .id(SharedString::from(format!(
                            "{}-show-more",
                            agent_id_prefix(agent)
                        )))
                        .mx(px(6.))
                        .px(px(8.))
                        .py(px(5.))
                        .rounded(px(6.))
                        .text_size(px(11.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui.muted)
                        .animated_hover(move |style, delta| {
                            style
                                .bg(lerp_color(
                                    hover_background.opacity(0.0),
                                    hover_background,
                                    delta,
                                ))
                                .text_color(lerp_color(ui.muted, ui.text, delta));
                        })
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.agent_sessions.sessions_focus.focus(window, cx);
                            this.agent_sessions.sessions_group_show_all[idx] =
                                !this.agent_sessions.sessions_group_show_all[idx];
                            this.clamp_sessions_selection();
                            cx.notify();
                        }))
                        .child(label),
                );
            }
            if omitted > 0 {
                group = group.child(
                    div()
                        .mx(px(14.))
                        .px(px(8.))
                        .py(px(4.))
                        .text_size(px(10.))
                        .text_color(ui.muted.opacity(0.8))
                        .child(older_sessions_hidden_label(omitted)),
                );
            }
        }

        group.into_any_element()
    }

    fn sessions_for(&self, agent: SessionAgent) -> &[SessionMeta] {
        &self.agent_sessions.sessions_by_agent[agent_index(agent)]
    }

    fn sessions_for_mut(&mut self, agent: SessionAgent) -> &mut Vec<SessionMeta> {
        &mut self.agent_sessions.sessions_by_agent[agent_index(agent)]
    }

    fn sessions_omitted_for(&self, agent: SessionAgent) -> usize {
        self.agent_sessions.sessions_omitted[agent_index(agent)]
    }

    fn sessions_row(
        &self,
        session: &SessionMeta,
        ui: crate::theme::UiColors,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent = session.agent;
        let session_id = session.session_id.clone();
        let row_id = SharedString::from(format!("{}-session-{session_id}", agent_id_prefix(agent)));
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        let resting_background = if selected {
            hover_background
        } else {
            hover_background.opacity(0.0)
        };
        let when = SharedString::from(format_relative_time(&session.timestamp));
        let title: SharedString = session
            .summary
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| short_session_id(&session_id))
            .into();

        let drag_payload = SessionDrag {
            agent,
            session_id: session_id.clone(),
            cwd: session.cwd.clone(),
            title: title.clone(),
            icon: SharedString::from(agent_icon_path(agent)),
        };

        div()
            .id(row_id)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .h(ROW_HEIGHT)
            .mx(px(8.))
            .my(px(1.))
            .px(px(8.))
            .rounded(px(6.))
            .on_drag(drag_payload, |drag, _offset, _window, cx| {
                cx.new(|_| DragPreview {
                    title: drag.title.clone(),
                    icon: drag.icon.clone(),
                })
            })
            .bg(resting_background)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(resting_background, hover_background, delta));
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.agent_sessions.sessions_focus.focus(window, cx);
                this.select_session_row(agent, &session_id);
                this.resume_session_from_sidebar(agent, &session_id, cx);
                cx.stop_propagation();
            }))
            .child(agent_icon_element(agent, px(13.), ui))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(when),
            )
            .into_any_element()
    }

    fn resume_session_from_sidebar(
        &mut self,
        agent: SessionAgent,
        session_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(command) = resume_command(agent, session_id, &self.cached_config) else {
            self.show_toast("Could not resume session - invalid session id", cx);
            return;
        };
        match self.send_command_to_sessions_surface(&command, cx) {
            ResumeSendResult::Sent => {}
            ResumeSendResult::Missing => {
                self.show_toast("Could not resume session - target terminal is gone", cx);
            }
            ResumeSendResult::WrongCwd => {
                self.show_toast(
                    "Could not resume session - target terminal changed directory",
                    cx,
                );
            }
        }
    }

    fn send_command_to_sessions_surface(
        &self,
        command: &str,
        cx: &mut Context<Self>,
    ) -> ResumeSendResult {
        let Some(surface_id) = self.agent_sessions.sessions_surface_id else {
            return ResumeSendResult::Missing;
        };
        let Some(expected_cwd) = self.agent_sessions.sessions_cwd.as_deref() else {
            return ResumeSendResult::Missing;
        };
        let Some(loc) = find_pane_by_surface_id(&self.workspaces, surface_id, cx) else {
            return ResumeSendResult::Missing;
        };
        let Some(terminal) = loc.pane.read(cx).active_terminal_opt().cloned() else {
            return ResumeSendResult::Missing;
        };
        let current_cwd = {
            let view = terminal.read(cx);
            view.terminal.current_cwd.clone().or_else(|| {
                view.terminal
                    .cwd_now()
                    .map(|p| p.to_string_lossy().into_owned())
            })
        };
        if !current_cwd
            .as_deref()
            .is_some_and(|cwd| crate::agent_sessions::cwd_matches(cwd, expected_cwd))
        {
            return ResumeSendResult::WrongCwd;
        }
        terminal.read(cx).send_command(command);
        terminal.update(cx, |view, _cx| view.declare_agent_from_command(command));
        ResumeSendResult::Sent
    }

    fn handle_sessions_sidebar_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.sessions_nav_len();
        match event.keystroke.key.as_str() {
            "escape" => self.close_sessions_sidebar(cx),
            "enter" | "space" if len > 0 => {
                let selected = self.agent_sessions.sessions_selected.min(len - 1);
                if let Some(target) = self.sessions_nav_target_at(selected) {
                    let agent = target.agent;
                    let session_id = target.session_id.to_string();
                    self.resume_session_from_sidebar(agent, &session_id, cx);
                }
            }
            "up" if len > 0 => {
                self.agent_sessions.sessions_selected = moved_session_selection(
                    self.agent_sessions.sessions_selected,
                    len,
                    SessionSelectionMove::Previous,
                );
                cx.notify();
            }
            "down" if len > 0 => {
                self.agent_sessions.sessions_selected = moved_session_selection(
                    self.agent_sessions.sessions_selected,
                    len,
                    SessionSelectionMove::Next,
                );
                cx.notify();
            }
            "home" if len > 0 => {
                self.agent_sessions.sessions_selected = moved_session_selection(
                    self.agent_sessions.sessions_selected,
                    len,
                    SessionSelectionMove::First,
                );
                cx.notify();
            }
            "end" if len > 0 => {
                self.agent_sessions.sessions_selected = moved_session_selection(
                    self.agent_sessions.sessions_selected,
                    len,
                    SessionSelectionMove::Last,
                );
                cx.notify();
            }
            _ => {}
        }
    }

    fn sessions_nav_len(&self) -> usize {
        let mut len = 0;
        for agent in crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config)
        {
            let idx = agent_index(agent);
            if self.agent_sessions.sessions_group_collapsed[idx] {
                continue;
            }
            let sessions = self.sessions_for(agent);
            let (visible, _) = visible_window(
                sessions.len(),
                self.agent_sessions.sessions_group_show_all[idx],
                CAP,
            );
            len += visible;
        }
        len
    }

    fn sessions_nav_target_at(&self, index: usize) -> Option<SessionNavTarget<'_>> {
        let mut cursor = 0usize;
        for agent in crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config)
        {
            let idx = agent_index(agent);
            if self.agent_sessions.sessions_group_collapsed[idx] {
                continue;
            }
            let sessions = self.sessions_for(agent);
            let (visible, _) = visible_window(
                sessions.len(),
                self.agent_sessions.sessions_group_show_all[idx],
                CAP,
            );
            if index < cursor + visible {
                let session = &sessions[index - cursor];
                return Some(SessionNavTarget {
                    agent,
                    session_id: &session.session_id,
                });
            }
            cursor += visible;
        }
        None
    }

    fn selected_session_target(&self) -> Option<SessionNavTarget<'_>> {
        let len = self.sessions_nav_len();
        if len == 0 {
            return None;
        }
        self.sessions_nav_target_at(self.agent_sessions.sessions_selected.min(len - 1))
    }

    fn select_session_row(&mut self, agent: SessionAgent, session_id: &str) {
        let mut cursor = 0usize;
        for row_agent in
            crate::agent_sessions::enabled_session_agents_from_config(&self.cached_config)
        {
            let idx = agent_index(row_agent);
            if self.agent_sessions.sessions_group_collapsed[idx] {
                continue;
            }
            let sessions = self.sessions_for(row_agent);
            let (visible, _) = visible_window(
                sessions.len(),
                self.agent_sessions.sessions_group_show_all[idx],
                CAP,
            );
            for session in sessions.iter().take(visible) {
                if row_agent == agent && session.session_id == session_id {
                    self.agent_sessions.sessions_selected = cursor;
                    return;
                }
                cursor += 1;
            }
        }
    }

    fn clamp_sessions_selection(&mut self) {
        let len = self.sessions_nav_len();
        if len == 0 {
            self.agent_sessions.sessions_selected = 0;
        } else if self.agent_sessions.sessions_selected >= len {
            self.agent_sessions.sessions_selected = len - 1;
        }
    }

    fn sessions_sidebar_width_at(&self, now: std::time::Instant) -> f32 {
        if let Some(animation) = self.agent_sessions.sessions_sidebar_animation {
            animation.width_at(now)
        } else if self.agent_sessions.sessions_sidebar_open {
            SESSIONS_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    pub(crate) fn rendered_sessions_sidebar_width(&mut self, window: &mut Window) -> f32 {
        let now = std::time::Instant::now();
        if let Some(animation) = self.agent_sessions.sessions_sidebar_animation {
            if animation.is_finished(now) {
                self.agent_sessions.sessions_sidebar_animation = None;
                if !self.agent_sessions.sessions_sidebar_open {
                    self.clear_sessions_sidebar_state();
                }
                animation.to_width
            } else {
                window.request_animation_frame();
                animation.width_at(now)
            }
        } else if self.agent_sessions.sessions_sidebar_open {
            SESSIONS_SIDEBAR_WIDTH
        } else {
            0.
        }
    }

    fn set_sessions_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let from_width = self.sessions_sidebar_width_at(now);
        self.agent_sessions.sessions_sidebar_open = open;
        let to_width = if open { SESSIONS_SIDEBAR_WIDTH } else { 0. };

        self.agent_sessions.sessions_sidebar_animation =
            if (from_width - to_width).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA {
                Some(crate::SidebarWidthAnimation {
                    from_width,
                    to_width,
                    started_at: now,
                })
            } else {
                None
            };

        if !open && self.agent_sessions.sessions_sidebar_animation.is_none() {
            self.clear_sessions_sidebar_state();
        }
        cx.notify();
    }

    fn clear_sessions_sidebar_state(&mut self) {
        for sessions in &mut self.agent_sessions.sessions_by_agent {
            sessions.clear();
        }
        self.agent_sessions.sessions_omitted = [0; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_cwd = None;
        self.agent_sessions.sessions_surface_id = None;
        self.agent_sessions.sessions_selected = 0;
        self.agent_sessions.sessions_group_collapsed =
            [false; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_group_show_all =
            [false; crate::agent_sessions::SESSION_AGENT_COUNT];
        self.agent_sessions.sessions_scanning = [false; crate::agent_sessions::SESSION_AGENT_COUNT];
    }

    pub(crate) fn close_sessions_sidebar_immediate(&mut self, cx: &mut Context<Self>) {
        self.agent_sessions.sessions_sidebar_open = false;
        self.agent_sessions.sessions_sidebar_animation = None;
        self.agent_sessions.sessions_scan_generation =
            self.agent_sessions.sessions_scan_generation.wrapping_add(1);
        self.clear_sessions_sidebar_state();
        cx.notify();
    }

    pub(crate) fn close_sessions_sidebar(&mut self, cx: &mut Context<Self>) {
        self.agent_sessions.sessions_scan_generation =
            self.agent_sessions.sessions_scan_generation.wrapping_add(1);
        self.set_sessions_sidebar_open(false, cx);
    }
}

const CAP: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SessionNavTarget<'a> {
    agent: SessionAgent,
    session_id: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SessionSelectionMove {
    First,
    Last,
    Previous,
    Next,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeSendResult {
    Sent,
    Missing,
    WrongCwd,
}

fn moved_session_selection(current: usize, len: usize, movement: SessionSelectionMove) -> usize {
    if len == 0 {
        return 0;
    }
    match movement {
        SessionSelectionMove::First => 0,
        SessionSelectionMove::Last => len - 1,
        SessionSelectionMove::Previous => current.saturating_sub(1),
        SessionSelectionMove::Next => (current + 1).min(len - 1),
    }
}

fn should_apply_scan_result(
    sidebar_open: bool,
    current_cwd: Option<&str>,
    expected_cwd: &str,
    current_generation: u64,
    expected_generation: u64,
) -> bool {
    sidebar_open && current_cwd == Some(expected_cwd) && current_generation == expected_generation
}

pub(crate) fn agent_index(agent: SessionAgent) -> usize {
    agent.index()
}

fn visible_window(len: usize, show_all: bool, cap: usize) -> (usize, usize) {
    if show_all || len <= cap {
        (len, 0)
    } else {
        (cap, len - cap)
    }
}

fn older_sessions_hidden_label(omitted: usize) -> SharedString {
    if omitted == 1 {
        SharedString::from("1 older session hidden")
    } else {
        format!("{omitted} older sessions hidden").into()
    }
}

fn compact_cwd_label(cwd: &str) -> SharedString {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let label = trimmed
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or(trimmed);
    if label.is_empty() {
        cwd.into()
    } else {
        label.into()
    }
}

fn empty_message(agent: SessionAgent) -> SharedString {
    format!("No {} sessions for this directory yet.", agent_label(agent)).into()
}

fn short_session_id(id: &str) -> String {
    id.split('-').next().unwrap_or(id).to_string()
}

fn agent_id_prefix(agent: SessionAgent) -> &'static str {
    agent.terminal_agent().tag()
}

fn agent_label(agent: SessionAgent) -> &'static str {
    agent.label()
}

fn agent_icon_path(agent: SessionAgent) -> &'static str {
    agent.icon_path()
}

fn agent_brand_color(agent: SessionAgent, ui: crate::theme::UiColors) -> Hsla {
    agent
        .terminal_agent()
        .accent()
        .map(|accent| rgb(accent).into())
        .unwrap_or(ui.text)
}

fn agent_icon_element(agent: SessionAgent, size: Pixels, ui: crate::theme::UiColors) -> AnyElement {
    if agent.terminal_agent().icon_multicolor() {
        img(agent_icon_path(agent))
            .size(size)
            .flex_none()
            .into_any_element()
    } else {
        svg()
            .size(size)
            .flex_none()
            .path(agent_icon_path(agent))
            .text_color(agent_brand_color(agent, ui))
            .into_any_element()
    }
}

fn claude_bypass_enabled(config: &paneflow_config::schema::PaneFlowConfig) -> bool {
    config.claude_code_bypass_permissions.unwrap_or(false)
}

pub(crate) fn resume_command(
    agent: SessionAgent,
    session_id: &str,
    config: &paneflow_config::schema::PaneFlowConfig,
) -> Option<String> {
    resume_command_spec(agent, session_id, config).map(|spec| spec.render_shell_command())
}

fn resume_command_spec(
    agent: SessionAgent,
    session_id: &str,
    config: &paneflow_config::schema::PaneFlowConfig,
) -> Option<AgentCommandSpec> {
    if !crate::agent_sessions::is_valid_session_id(session_id) {
        log::warn!("resume_command: refused invalid session id, not sending to PTY");
        return None;
    }
    let spec = match agent {
        SessionAgent::Claude => {
            let mut spec = AgentCommandSpec::new("claude");
            spec.push_arg("--resume");
            spec.push_arg(session_id);
            if claude_bypass_enabled(config) {
                spec.push_arg("--permission-mode");
                spec.push_arg("bypassPermissions");
            }
            spec
        }
        SessionAgent::Codex => {
            let mut spec = AgentCommandSpec::new("codex");
            spec.push_arg("resume");
            spec.push_arg(session_id);
            spec
        }
        SessionAgent::OpenCode => {
            let mut spec = AgentCommandSpec::new("opencode");
            spec.push_arg("--session");
            spec.push_arg(session_id);
            spec
        }
        SessionAgent::Pi => {
            let mut spec = AgentCommandSpec::new("pi");
            spec.push_arg("--session");
            spec.push_arg(session_id);
            spec
        }
        SessionAgent::Hermes => resume_flag_spec("hermes", session_id),
        SessionAgent::Grok => resume_flag_spec("grok", session_id),
        SessionAgent::Cursor => {
            let mut spec = AgentCommandSpec::new("cursor-agent");
            spec.push_arg(format!("--resume={session_id}"));
            spec
        }
        SessionAgent::Gemini => resume_flag_spec("gemini", session_id),
        SessionAgent::Kiro => {
            let mut spec = AgentCommandSpec::new("kiro-cli");
            spec.push_arg("chat");
            spec.push_arg("--resume-id");
            spec.push_arg(session_id);
            spec
        }
    };
    debug_assert!(crate::agent_launcher::is_plain_shell_token(session_id));
    Some(spec)
}

fn resume_flag_spec(program: &'static str, session_id: &str) -> AgentCommandSpec {
    let mut spec = AgentCommandSpec::new(program);
    spec.push_arg("--resume");
    spec.push_arg(session_id);
    spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_index_is_stable() {
        for (idx, agent) in SessionAgent::ALL.into_iter().enumerate() {
            assert_eq!(agent_index(agent), idx);
        }
    }

    #[test]
    fn resume_command_neutralizes_flag_shaped_session_id() {
        let cfg = paneflow_config::schema::PaneFlowConfig::default();
        for agent in SessionAgent::ALL {
            assert_eq!(
                resume_command(agent, "--dangerously-skip-permissions", &cfg),
                None,
                "{agent:?}: a `--`-prefixed id must not build a command"
            );
            assert_eq!(resume_command(agent, "-x", &cfg), None);
            assert_eq!(resume_command(agent, "ses_x; rm -rf ~", &cfg), None);
            assert_eq!(resume_command(agent, "$(reboot)", &cfg), None);
        }
        let valid = "019dc9ea-38d7-7372-9cc4-253ce944d41b";
        for agent in SessionAgent::ALL {
            assert!(resume_command(agent, valid, &cfg).is_some());
        }
    }

    #[test]
    fn resume_command_renders_expected_agent_commands() {
        let cfg = paneflow_config::schema::PaneFlowConfig::default();
        let id = "019dc9ea-38d7-7372-9cc4-253ce944d41b";

        let cases = [
            (SessionAgent::Claude, format!("claude --resume {id}")),
            (SessionAgent::Codex, format!("codex resume {id}")),
            (SessionAgent::OpenCode, format!("opencode --session {id}")),
            (SessionAgent::Pi, format!("pi --session {id}")),
            (SessionAgent::Hermes, format!("hermes --resume {id}")),
            (SessionAgent::Grok, format!("grok --resume {id}")),
            (SessionAgent::Cursor, format!("cursor-agent --resume={id}")),
            (SessionAgent::Gemini, format!("gemini --resume {id}")),
            (
                SessionAgent::Kiro,
                format!("kiro-cli chat --resume-id {id}"),
            ),
        ];

        for (agent, expected) in cases {
            assert_eq!(resume_command(agent, id, &cfg), Some(expected));
        }
    }

    #[test]
    fn resume_command_composes_claude_bypass_as_structured_args() {
        let cfg = paneflow_config::schema::PaneFlowConfig {
            claude_code_bypass_permissions: Some(true),
            ..Default::default()
        };
        let id = "019dc9ea-38d7-7372-9cc4-253ce944d41b";

        assert_eq!(
            resume_command(SessionAgent::Claude, id, &cfg),
            Some(format!(
                "claude --resume {id} --permission-mode bypassPermissions"
            ))
        );
    }

    #[test]
    fn visible_window_empty() {
        assert_eq!(visible_window(0, false, CAP), (0, 0));
    }

    #[test]
    fn visible_window_at_cap_has_no_remainder() {
        assert_eq!(visible_window(5, false, CAP), (5, 0));
    }

    #[test]
    fn visible_window_over_cap_caps_and_reports_remainder() {
        assert_eq!(visible_window(6, false, CAP), (5, 1));
        assert_eq!(visible_window(100, false, CAP), (5, 95));
    }

    #[test]
    fn visible_window_show_all_reveals_everything() {
        assert_eq!(visible_window(6, true, CAP), (6, 0));
        assert_eq!(visible_window(100, true, CAP), (100, 0));
    }

    #[test]
    fn older_sessions_hidden_label_pluralizes() {
        assert_eq!(older_sessions_hidden_label(1), "1 older session hidden");
        assert_eq!(older_sessions_hidden_label(2), "2 older sessions hidden");
    }

    #[test]
    fn scan_result_requires_matching_generation() {
        assert!(should_apply_scan_result(true, Some("/repo"), "/repo", 2, 2));
        assert!(
            !should_apply_scan_result(true, Some("/repo"), "/repo", 3, 2),
            "a stale same-cwd scan must not overwrite a newer open"
        );
        assert!(!should_apply_scan_result(
            false,
            Some("/repo"),
            "/repo",
            2,
            2
        ));
        assert!(!should_apply_scan_result(
            true,
            Some("/other"),
            "/repo",
            2,
            2
        ));
    }

    #[test]
    fn moved_session_selection_clamps_to_visible_rows() {
        assert_eq!(
            moved_session_selection(0, 3, SessionSelectionMove::Previous),
            0
        );
        assert_eq!(moved_session_selection(0, 3, SessionSelectionMove::Next), 1);
        assert_eq!(moved_session_selection(2, 3, SessionSelectionMove::Next), 2);
        assert_eq!(
            moved_session_selection(1, 3, SessionSelectionMove::First),
            0
        );
        assert_eq!(moved_session_selection(1, 3, SessionSelectionMove::Last), 2);
        assert_eq!(moved_session_selection(7, 0, SessionSelectionMove::Last), 0);
    }

    #[test]
    fn compact_cwd_label_uses_last_path_component() {
        assert_eq!(compact_cwd_label("/home/arthur/paneflow"), "paneflow");
        assert_eq!(compact_cwd_label("/home/arthur/paneflow/"), "paneflow");
        assert_eq!(compact_cwd_label(r"C:\dev\paneflow"), "paneflow");
        assert_eq!(compact_cwd_label("/"), "/");
    }
}
