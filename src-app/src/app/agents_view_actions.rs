//! Action handler + lifecycle helpers + render branch entry points for
//! the Agents view.
//!
//! [`paneflow_config::schema::AppMode`] is the source of truth for which
//! top-level screen renders; `self.mode` decides whether the Agents view
//! is currently visible. The main area is terminal-only: a selected
//! thread renders its PTY, and the no-thread state renders the agent
//! picker for the active project (the home/empty state).
//!
//! Toggled by the [`crate::OpenAgentsView`] action (Ctrl+Shift+A on
//! Linux/Windows, Cmd+Shift+A on macOS). Both render branches
//! ([`PaneFlowApp::render_agents_main`] and
//! [`PaneFlowApp::render_agents_sidebar`]) are no-ops when
//! `self.mode == AppMode::Cli` -- main `render` only calls them on the
//! Agents arm.

use crate::ui_primitives::AnimatedHoverExt;
use crate::{OpenAgentsView, PaneFlowApp};
use gpui::{
    AppContext, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, MouseButton,
    ParentElement, StatefulInteractiveElement, Styled, Window, deferred, div,
    prelude::FluentBuilder, px, svg,
};
use paneflow_config::schema::{AppMode, TerminalSurfaceProfile};
use serde_json::Value;

/// Sidebar width when in [`AppMode::Agents`]. Slightly wider than the
/// CLI sidebar (220 px) because thread rows carry more metadata
/// (agent icon, status dot, relative timestamp) than workspace rows.
/// US-009 surfaces this constant to the title bar so the resize edge
/// snaps to the right slot on mode toggle.
pub(crate) const AGENTS_SIDEBAR_WIDTH: f32 = 280.0;

/// Empty band reserved at the top of the agents terminal surface so the
/// floating environment toolbar (editor launcher + bottom-dock toggle) lives in
/// its own strip and never paints over the CLI when the window is resized
/// narrow. Sized to clear the overlay: top inset (20) + button
/// height (28) + breathing room (8). Keep in sync with the toolbar `top` and
/// button heights below.
const AGENTS_TOOLBAR_BAND_HEIGHT: f32 = 56.0;
const AGENTS_EXITED_TERMINAL_CACHE_LIMIT: usize = 8;
const AGENTS_TERMINAL_CACHE_IDLE_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

fn touch_lru(order: &mut Vec<u64>, id: u64) {
    order.retain(|existing| *existing != id);
    order.push(id);
}

fn oldest_evictable_terminal_id(
    order: &[u64],
    active: Option<u64>,
    cache_len: usize,
    limit: usize,
    mut is_evictable: impl FnMut(u64) -> bool,
) -> Option<u64> {
    if cache_len <= limit {
        return None;
    }
    order
        .iter()
        .copied()
        .find(|id| Some(*id) != active && is_evictable(*id))
}

impl PaneFlowApp {
    /// Toggle between [`AppMode::Cli`] and [`AppMode::Agents`].
    ///
    /// Focus contract (US-008 AC): when toggling back to CLI, the
    /// previously active workspace's first pane re-receives focus.
    /// The reverse direction (CLI -> Agents) does not steal focus
    /// proactively; the Agents view rendering takes over the main
    /// surface and any subsequent keystroke targets the new tree.
    pub(crate) fn handle_open_agents_view(
        &mut self,
        _: &OpenAgentsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.mode {
            AppMode::Agents => {
                self.exit_agents_mode(window, cx);
            }
            // From CLI or the Diff mode, pressing the Agents binding
            // switches into Agents (US-003 of prd-git-diff-mode-2026-Q3.md;
            // `enter_agents_mode` clears any other non-CLI surface).
            AppMode::Cli | AppMode::Diff => {
                self.enter_agents_mode(cx);
            }
        }
    }

    /// Switch the main pane to the Skills browser (~/.claude/skills,
    /// ~/.codex/skills, ~/.agents/skills).
    ///
    pub(crate) fn show_agents_skills(&mut self, cx: &mut Context<Self>) {
        // US-003: clearing the unified target drops to the picker/home
        // state; the Skills page then takes precedence in the render branch.
        self.agents_target = None;
        self.agents_view.agents_skills_visible = true;
        if self.agents_view.agents_skills.is_empty() {
            self.refresh_agents_skills(cx);
        }
        cx.notify();
    }

    pub(crate) fn refresh_agents_skills(&mut self, cx: &mut Context<Self>) {
        if self.agents_view.agents_skills_loading {
            return;
        }
        self.agents_view.agents_skills_loading = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let skills = smol::unblock(crate::agents_view::discover_skills).await;
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    app.agents_view.agents_skills = skills;
                    app.agents_view.agents_skills_loading = false;
                    cx.notify();
                })
            });
        })
        .detach();
    }

    /// Mark a skill id as "just copied" so its card label flips to
    /// "Copied". A scheduled task clears the slot iff it still holds the same
    /// id, so duplicate skill names do not collide.
    pub(crate) fn mark_skill_copied(&mut self, id: String, cx: &mut Context<Self>) {
        self.agents_view.agents_skills_copied = Some(id.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            smol::Timer::after(std::time::Duration::from_millis(1500)).await;
            let _ = cx.update(|cx| {
                this.update(cx, |app, cx| {
                    if app.agents_view.agents_skills_copied.as_deref() == Some(id.as_str()) {
                        app.agents_view.agents_skills_copied = None;
                        cx.notify();
                    }
                })
            });
        })
        .detach();
    }

    pub(crate) fn enter_agents_mode(&mut self, cx: &mut Context<Self>) {
        self.mode = AppMode::Agents;
        // US-016 warm-resume: entering Agents from Diff suspends the diff host
        // (releases its watchers + ends its debounce loop) while the cache keeps
        // its computed rows for an instant warm return; no-op from CLI (already
        // parked). Keeps the non-CLI surfaces mutually exclusive (prd-git-diff
        // US-003/US-005) without throwing away the diff.
        self.park_displayed_diff(cx);
        if let Some(target) = self.current_thread_view_target() {
            self.mount_agents_terminal_for_target(target, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    fn exit_agents_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.mode = AppMode::Cli;
        // Focus contract: restore focus to the active workspace's
        // first pane so the keyboard immediately targets the
        // terminal the user left, not a stray top-level handler.
        // Terminal PTYs are detached, so the previously running
        // process is still alive (verified by spawning and switching
        // mid-stream).
        if let Some(ws) = self.workspaces.get_mut(self.active_idx) {
            ws.focus_first(window, cx);
        }
        self.save_session(cx);
        cx.notify();
    }

    /// Main-content render branch for [`AppMode::Agents`].
    ///
    /// Priority order:
    /// 1. The Skills page, if open.
    /// 2. A selected thread -> its terminal surface (the PTY launching
    ///    the thread's CLI agent).
    /// 3. A project open but no thread selected -> the agent picker for
    ///    that project (the home/empty state).
    /// 4. No project at all -> the "no project" empty state.
    pub(crate) fn render_agents_main(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let body: gpui::AnyElement = self.render_agents_main_body(cx);
        let bottom_panel_max_height =
            crate::app::agents_bottom_panel::bottom_panel_max_height(window);
        // The main area stacks vertically: the agent surface (terminal/picker)
        // fills the space, and the Codex-style bottom dock - when open - takes a
        // resizable, full-width slice below it.
        let mut root = div()
            .size_full()
            .flex()
            .flex_col()
            // The bottom dock's top-edge resize is captured here, on the
            // full-height main area, so a drag keeps tracking even when the
            // cursor outruns its handle and crosses into the surface above it.
            .on_mouse_move(
                cx.listener(move |this, event: &gpui::MouseMoveEvent, _w, cx| {
                    if this.agents_view.bottom_panel_drag.is_some() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.drag_bottom_panel_resize(
                                f32::from(event.position.y),
                                bottom_panel_max_height,
                                cx,
                            );
                        } else {
                            this.end_bottom_panel_resize(cx);
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, cx| {
                    this.end_bottom_panel_resize(cx);
                }),
            )
            .child(div().flex_1().min_h(px(0.)).child(body));
        if self.agents_view.bottom_panel_open {
            root = root.child(self.render_agents_bottom_panel(bottom_panel_max_height, cx));
        }
        root.into_any_element()
    }

    /// The inner Agents-view body (skills page / terminal surface /
    /// agent picker / empty state). Pulled out so the toolbar wrapping
    /// logic stays in one place.
    fn render_agents_main_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        // Sidebar "Skills" affordance takes precedence over the thread /
        // picker surfaces.
        if self.agents_view.agents_skills_visible {
            return crate::agents_view::render_skills_page(
                self.agents_view.agents_skills_tab,
                self.agents_view.agents_skills_copied.clone(),
                self.agents_view.agents_skills.clone(),
                self.agents_view.agents_skills_loading,
                cx,
            );
        }
        // A selected thread renders its cached terminal surface. Creation is
        // driven by selection/restore paths, not by render, so repainting this
        // branch cannot spawn a PTY.
        if let Some(target) = self.current_thread_view_target()
            && let Some(view) = self.cached_agents_terminal_view(target, cx)
        {
            let max_content_width = self.cached_config.agent_panel.as_ref().map_or(
                paneflow_config::schema::AgentPanelConfig::DEFAULT_MAX_CONTENT_WIDTH,
                |cfg| cfg.resolved_max_content_width(),
            );
            return render_terminal_thread_surface(
                view,
                max_content_width,
                self.agents_environment_cwd(target),
                AgentsEnvironmentOverlayState {
                    editor_menu_open: self.agents_view.agents_editor_menu_open,
                    editor_value: self
                        .cached_config
                        .external_editor
                        .clone()
                        .unwrap_or_else(|| "auto".to_string()),
                    bottom_open: self.agents_view.bottom_panel_open,
                },
                cx,
            );
        } else if self.current_thread_view_target().is_some() {
            return render_agents_terminal_loading();
        }
        // No thread selected: the picker/home state. US-005 -- the picker
        // context decides what a launched agent is created into.
        match self.agents_picker_context {
            crate::project::AgentsPickerContext::NewChat => {
                self.render_agents_launcher(LauncherContext::NewChat, cx)
            }
            crate::project::AgentsPickerContext::Project => {
                if !self.projects.is_empty() && self.active_project_idx < self.projects.len() {
                    self.render_agents_launcher(
                        LauncherContext::Project(self.active_project_idx),
                        cx,
                    )
                } else {
                    // No project at all: a minimal empty state mirroring the
                    // sidebar's "No projects yet" copy.
                    render_agents_no_project()
                }
            }
        }
    }

    /// Agent picker: a centered card list of the CLI coding agents
    /// enabled in Settings → AI Agent. Clicking one creates a Terminal
    /// Thread that auto-launches that agent in a PTY (honoring the
    /// bypass-permission flag). US-005 -- the [`LauncherContext`] decides
    /// the create target: a thread in `project_idx`, or a free chat in the
    /// home dir. This is the Agents view's home/empty state whenever no
    /// thread/chat is selected.
    fn render_agents_launcher(
        &mut self,
        ctx: LauncherContext,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use crate::agent_launcher::TerminalAgent;
        use gpui::{
            ClickEvent, FontWeight, InteractiveElement, MouseButton, SharedString,
            StatefulInteractiveElement, rgb,
        };

        let ui = crate::theme::ui_colors();
        let agents = TerminalAgent::visible(&self.cached_config);

        // Codex-style hover: a whisper darken of the filled `ui.subtle` tile,
        // mirroring the settings `select_trigger` - no border, no accent ring.
        let hover_bg = gpui::Hsla {
            l: (ui.subtle.l - 0.04).max(0.0),
            ..ui.subtle
        };

        let tiles: Vec<gpui::AnyElement> = agents
            .into_iter()
            .map(|agent| {
                let name = agent.display_name();
                let installed = agent.is_installed();
                let icon_color: gpui::Hsla =
                    agent.accent().map(|c| rgb(c).into()).unwrap_or(ui.text);
                div()
                    .id(SharedString::from(format!(
                        "agents-launcher-{}",
                        agent.tag()
                    )))
                    // Equal-width grid cell (3 per row); `min_w_0` lets a long
                    // agent name truncate instead of widening the column.
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .rounded(px(10.))
                    .bg(ui.subtle)
                    .when(!installed, |d| d.opacity(0.58))
                    .animated_hover_bg(ui.subtle, hover_bg)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| {
                        cx.stop_propagation();
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| match ctx {
                        LauncherContext::Project(project_idx) => {
                            this.create_agent_terminal_thread_in(project_idx, agent, cx);
                        }
                        LauncherContext::NewChat => {
                            this.create_agent_chat(agent, cx);
                        }
                    }))
                    // Multi-color logos render via `img()` (resvg rasterizes
                    // the SVG, keeping every native fill); monochrome logos
                    // stay a `text_color`-tinted `svg()` mask.
                    .child(if agent.icon_multicolor() {
                        gpui::img(agent.icon_path())
                            .size(px(18.))
                            .flex_none()
                            .into_any_element()
                    } else {
                        gpui::svg()
                            .size(px(18.))
                            .flex_none()
                            .path(agent.icon_path())
                            .text_color(icon_color)
                            .into_any_element()
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .child(SharedString::from(name)),
                    )
                    .into_any_element()
            })
            .collect();

        let body: gpui::AnyElement = if tiles.is_empty() {
            div()
                .text_size(px(13.))
                .text_color(ui.muted)
                .child(
                    "Every agent is hidden in Settings → AI Agent. Enable one to start a thread.",
                )
                .into_any_element()
        } else {
            // Three-column grid: rows of 3 equal-width tiles, the final row
            // padded with flex spacers so its tiles keep their 1/3 width
            // instead of stretching.
            let mut grid = div().flex().flex_col().gap(px(10.));
            let mut row = div().flex().flex_row().gap(px(10.));
            let mut in_row = 0u32;
            for tile in tiles {
                row = row.child(tile);
                in_row += 1;
                if in_row == 3 {
                    grid = grid.child(row);
                    row = div().flex().flex_row().gap(px(10.));
                    in_row = 0;
                }
            }
            if in_row > 0 {
                for _ in in_row..3 {
                    row = row.child(div().flex_1().min_w_0());
                }
                grid = grid.child(row);
            }
            grid.into_any_element()
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            // Transparent: the Agents main wrapper paints the panel bg
            // (Codex floating-panel look); the picker inherits it.
            .text_color(ui.text)
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(20.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(640.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .mb(px(4.))
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ui.text)
                                    .child(match ctx {
                                        LauncherContext::NewChat => "Start a new chat",
                                        LauncherContext::Project(_) => "Start a new thread",
                                    }),
                            )
                            .child(
                                div()
                                    .mb(px(12.))
                                    .text_size(px(12.))
                                    .text_color(ui.muted)
                                    .child(match ctx {
                                        LauncherContext::NewChat => {
                                            "Pick an agent to start a chat in your home directory."
                                        }
                                        LauncherContext::Project(_) => {
                                            "Pick an agent to launch in a terminal."
                                        }
                                    }),
                            )
                            .child(body),
                    ),
            )
            .into_any_element()
    }

    /// The currently selected center target, validated against the live
    /// `projects` / `chats` vectors (US-003). Returns `None` when nothing is
    /// selected (picker/home) OR the stored target points past the end of
    /// its source (e.g. its row was just removed) - both collapse to the
    /// picker rather than rendering a stale row.
    pub(crate) fn current_thread_view_target(&self) -> Option<crate::project::AgentsTarget> {
        use crate::project::AgentsTarget;
        match self.agents_target? {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => {
                let project = self.projects.get(project_idx)?;
                (thread_idx < project.threads.len()).then_some(AgentsTarget::Thread {
                    project_idx,
                    thread_idx,
                })
            }
            AgentsTarget::Chat { chat_idx } => {
                (chat_idx < self.chats.len()).then_some(AgentsTarget::Chat { chat_idx })
            }
        }
    }

    /// Working directory the Agents toolbar acts on: a thread's project root,
    /// or a free chat's own cwd.
    fn agents_environment_cwd(&self, target: crate::project::AgentsTarget) -> String {
        match target {
            crate::project::AgentsTarget::Thread { project_idx, .. } => self
                .projects
                .get(project_idx)
                .map(|project| project.cwd.clone())
                .unwrap_or_default(),
            crate::project::AgentsTarget::Chat { .. } => self
                .thread_for_target(target)
                .map(|thread| thread.cwd.clone())
                .unwrap_or_default(),
        }
    }

    fn toggle_agents_editor_menu(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_editor_menu_open = !self.agents_view.agents_editor_menu_open;
        cx.notify();
    }

    fn close_agents_editor_menu(
        &mut self,
        _: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agents_view.agents_editor_menu_open {
            self.agents_view.agents_editor_menu_open = false;
            cx.notify();
        }
    }

    fn select_agents_environment_editor(
        &mut self,
        value: String,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_editor_menu_open = false;
        self.persist_setting(false, "external_editor", Value::String(value), cx);
    }

    fn open_agents_environment_in_editor(
        &mut self,
        cwd: String,
        editor_value: String,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_editor_menu_open = false;
        match open_agents_cwd_with_editor(&cwd, &editor_value) {
            Ok(label) => self.show_toast(format!("Opened folder in {label}"), cx),
            Err(err) => self.show_toast(err, cx),
        }
        cx.notify();
    }

    fn touch_agents_terminal_cache(&mut self, thread_id: u64) {
        touch_lru(&mut self.agents_view.agents_terminal_cache_lru, thread_id);
        self.agents_view
            .agents_terminal_cache_touched_at
            .insert(thread_id, std::time::Instant::now());
    }

    pub(crate) fn remove_agents_terminal_cache_entry(&mut self, thread_id: u64) {
        self.agents_view
            .agents_terminal_view_cache
            .remove(&thread_id);
        self.agents_view
            .agents_terminal_cache_lru
            .retain(|id| *id != thread_id);
        self.agents_view
            .agents_terminal_cache_touched_at
            .remove(&thread_id);
    }

    pub(crate) fn enforce_agents_terminal_cache_budget(
        &mut self,
        active_thread_id: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let cache_keys: std::collections::HashSet<u64> = self
            .agents_view
            .agents_terminal_view_cache
            .keys()
            .copied()
            .collect();
        self.agents_view
            .agents_terminal_cache_lru
            .retain(|id| cache_keys.contains(id));
        self.agents_view
            .agents_terminal_cache_touched_at
            .retain(|id, _| cache_keys.contains(id));

        // V1 fallback for live scrollback trim: dropping a live TerminalView
        // terminates its PTY, so the TTL path only releases exited terminals.
        let now = std::time::Instant::now();
        let expired: Vec<u64> = self
            .agents_view
            .agents_terminal_cache_lru
            .iter()
            .copied()
            .filter(|id| Some(*id) != active_thread_id)
            .filter(|id| {
                self.agents_view
                    .agents_terminal_cache_touched_at
                    .get(id)
                    .is_some_and(|last| now.duration_since(*last) >= AGENTS_TERMINAL_CACHE_IDLE_TTL)
            })
            .filter(|id| {
                self.agents_view
                    .agents_terminal_view_cache
                    .get(id)
                    .is_some_and(|view| view.read(cx).terminal.exited.is_some())
            })
            .collect();
        for thread_id in expired {
            self.remove_agents_terminal_cache_entry(thread_id);
        }

        while self.agents_view.agents_terminal_view_cache.len() > AGENTS_EXITED_TERMINAL_CACHE_LIMIT
        {
            let lru = self.agents_view.agents_terminal_cache_lru.clone();
            let cache_len = self.agents_view.agents_terminal_view_cache.len();
            let evict = oldest_evictable_terminal_id(
                &lru,
                active_thread_id,
                cache_len,
                AGENTS_EXITED_TERMINAL_CACHE_LIMIT,
                |id| {
                    self.agents_view
                        .agents_terminal_view_cache
                        .get(&id)
                        .is_some_and(|view| view.read(cx).terminal.exited.is_some())
                },
            );
            let Some(thread_id) = evict else {
                log::debug!(
                    "agents terminal cache remains over budget; active/running terminals are protected"
                );
                break;
            };
            self.remove_agents_terminal_cache_entry(thread_id);
        }
    }

    pub(crate) fn spawn_agents_environment_git_refresh(
        &mut self,
        cwd: String,
        cx: &mut Context<Self>,
    ) {
        let cwd = cwd.trim().to_string();
        if cwd.is_empty() {
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let (branch, is_repo, stats) = smol::unblock({
                    let cwd = cwd.clone();
                    move || read_agents_environment_git_state(&cwd)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        let changed = app.apply_git_state_for_cwd(&cwd, branch, is_repo, stats);
                        let refreshed = app.refresh_agents_diff_if_open_for_cwd(&cwd, cx);
                        if changed || refreshed {
                            cx.notify();
                        }
                    })
                });
            },
        )
        .detach();
    }

    pub(crate) fn apply_git_state_for_cwd(
        &mut self,
        cwd: &str,
        branch: String,
        is_repo: bool,
        stats: crate::workspace::GitDiffStats,
    ) -> bool {
        let mut changed = false;
        for workspace in &mut self.workspaces {
            if workspace.cwd == cwd {
                if workspace.git_branch != branch {
                    workspace.git_branch = branch.clone();
                    changed = true;
                }
                if workspace.is_git_repo != is_repo {
                    workspace.is_git_repo = is_repo;
                    changed = true;
                }
                if workspace.git_stats != stats {
                    workspace.git_stats = stats.clone();
                    changed = true;
                }
            }
        }
        for project in &mut self.projects {
            if project.cwd == cwd && project.git_stats != stats {
                project.git_stats = stats.clone();
                changed = true;
            }
        }
        changed
    }

    pub(crate) fn apply_git_stats_for_cwd(
        &mut self,
        cwd: &str,
        stats: crate::workspace::GitDiffStats,
    ) -> bool {
        let mut changed = false;
        for workspace in &mut self.workspaces {
            if workspace.cwd == cwd && workspace.git_stats != stats {
                workspace.git_stats = stats.clone();
                changed = true;
            }
        }
        for project in &mut self.projects {
            if project.cwd == cwd && project.git_stats != stats {
                project.git_stats = stats.clone();
                changed = true;
            }
        }
        changed
    }

    /// Resolve a center target to its backing [`Thread`], whether it lives
    /// in a project or in the free `chats` list (US-003). `None` when the
    /// target is out of range.
    pub(crate) fn thread_for_target(
        &self,
        target: crate::project::AgentsTarget,
    ) -> Option<&crate::project::Thread> {
        use crate::project::AgentsTarget;
        match target {
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => self.projects.get(project_idx)?.threads.get(thread_idx),
            AgentsTarget::Chat { chat_idx } => self.chats.get(chat_idx),
        }
    }

    fn cached_agents_terminal_view(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<crate::terminal::view::TerminalView>> {
        let thread_id = self.thread_for_target(target)?.id;
        let cached = self
            .agents_view
            .agents_terminal_view_cache
            .get(&thread_id)
            .cloned()?;
        self.touch_agents_terminal_cache(thread_id);
        self.enforce_agents_terminal_cache_budget(Some(thread_id), cx);
        Some(cached)
    }

    /// Mount (or reuse from cache) the [`TerminalView`] entity that
    /// backs a Terminal Thread at `target`. Returns the entity ready
    /// to be wrapped by [`render_terminal_thread_surface`].
    ///
    /// Cache hit re-binds the existing entity so the running shell
    /// process survives sidebar navigation; cache miss spawns a fresh
    /// PTY in the thread's cwd via [`TerminalView::with_cwd`] and (when
    /// the thread is bound to a CLI agent) auto-launches it.
    ///
    /// `workspace_id` for the new view is the thread's own `id` offset
    /// into the Agents namespace ([`crate::project::thread_env_id`]) so
    /// PTY tracking keys off a stable per-thread identifier AND the
    /// `ai.*` hook frames emitted from inside this PTY route back to
    /// the thread (spinner / attention state) instead of colliding with
    /// a same-numbered CLI-mode workspace.
    pub(crate) fn mount_agents_terminal_for_target(
        &mut self,
        target: crate::project::AgentsTarget,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Entity<crate::terminal::view::TerminalView>> {
        // US-003: resolve the target (project thread OR free chat) to its
        // backing Thread. The cache key below is the stable `Thread::id`,
        // shared across both sources, so a project thread and a chat can
        // never collide and warm-resume survives navigation between them.
        let thread = self.thread_for_target(target)?;
        let thread_id = thread.id;
        if let Some(cached) = self
            .agents_view
            .agents_terminal_view_cache
            .get(&thread_id)
            .cloned()
        {
            self.touch_agents_terminal_cache(thread_id);
            self.enforce_agents_terminal_cache_budget(Some(thread_id), cx);
            return Some(cached.clone());
        }
        let cwd = std::path::PathBuf::from(&thread.cwd);
        // The thread's forced agent session id (Claude only), spliced into
        // the launch command below so the live PTY binds 1:1 to its on-disk
        // session file (and resumes the same session after a restart).
        let bound_session = thread.session_id.clone();
        // Resolved against the on-disk session store below: the first launch
        // mints the id, every reopen reattaches to it. See
        // [`crate::agent_launcher::SessionBinding`].
        let thread_cwd = thread.cwd.clone();
        let terminal_agent = thread.terminal_agent;
        let view = cx.new(|cx| {
            crate::terminal::view::TerminalView::with_cwd_and_profile(
                crate::project::thread_env_id(thread_id),
                Some(cwd),
                None,
                TerminalSurfaceProfile::Agent,
                cx,
            )
        });
        // Cache miss = first mount of this thread's PTY (fresh creation
        // or first reopen after a restart). When the thread is bound to
        // a CLI agent, auto-run its launch command so opening the thread
        // drops the user straight into the agent. The command honors
        // `claude_code_bypass_permissions` via `launch_command`. Writing
        // immediately is safe even though `with_cwd` opens the PTY on a
        // background thread (US-012): `send_command` → `write_to_pty`
        // buffers into the display-only terminal's `pending_input` queue and
        // `TerminalState::promote` flushes it the moment the PTY goes live,
        // so the command is never dropped on the pre-promotion race.
        // Cache hits (in-session re-selection) skip this, so a running
        // agent is never relaunched on navigation.
        if let Some(agent) = terminal_agent {
            let binding = crate::agent_launcher::SessionBinding::resolve(
                bound_session.as_deref(),
                &thread_cwd,
            );
            let cmd = agent.launch_command_with_session(&self.cached_config, binding);
            view.read(cx).send_command(&cmd);
        }
        // Mirror Zed's `AgentTerminal::refresh_terminal_metadata`
        // (agent_panel.rs around `TerminalEvent::TitleChanged`): every
        // OSC 0/2 title update from the running process is reflected
        // into the sidebar row label. That's what lets a `claude`
        // session inside a Terminal Thread surface its auto-summary
        // ("Refactor auth middleware") in the sidebar instead of the
        // generic "Terminal" placeholder. The subscription is detached
        // -- the entity owns its lifecycle and the listener drops with
        // it when the cache evicts the entry.
        cx.subscribe(
            &view,
            move |this, src, event: &crate::terminal::view::TerminalEvent, cx| {
                if let crate::terminal::view::TerminalEvent::TitleChanged = event {
                    let new_title = src.read(cx).terminal.title.clone();
                    this.handle_terminal_thread_title_changed(thread_id, new_title, cx);
                }
            },
        )
        .detach();
        self.agents_view
            .agents_terminal_view_cache
            .insert(thread_id, view.clone());
        self.touch_agents_terminal_cache(thread_id);
        self.enforce_agents_terminal_cache_budget(Some(thread_id), cx);
        Some(view)
    }

    /// Resolve a thread by its stable [`crate::project::Thread::id`] across
    /// project threads and free chats (both allocate from the same counter,
    /// so the id is globally unique).
    pub(crate) fn agents_thread_mut_by_id(
        &mut self,
        thread_id: u64,
    ) -> Option<&mut crate::project::Thread> {
        self.projects
            .iter_mut()
            .flat_map(|p| p.threads.iter_mut())
            .chain(self.chats.iter_mut())
            .find(|t| t.id == thread_id)
    }

    /// US-010 (Agents UI redesign): the title-bar brand
    /// labels for Agents mode. Returns `(primary, context, overflow_enabled)`:
    /// - selected project thread -> (thread title, project name, true)
    /// - selected free chat      -> (chat title, "Chat", true)
    /// - picker/home state       -> (neutral label, None, false)
    ///
    /// The primary always passes through [`crate::project::clean_sidebar_title`]
    /// so a CLI spinner glyph never leaks into the chrome. Pushed by
    /// `PaneFlowApp::render` into the (separate) `TitleBar` entity; the
    /// neutral picker label satisfies US-010 AC4 (no broken alignment).
    pub(crate) fn agents_titlebar_labels(&self) -> (Option<String>, Option<String>, bool) {
        use crate::project::AgentsTarget;
        let clean =
            |raw: &str| crate::project::clean_sidebar_title(raw).unwrap_or_else(|| raw.to_string());
        match self.agents_target {
            Some(AgentsTarget::Thread {
                project_idx,
                thread_idx,
            }) => {
                if let Some(project) = self.projects.get(project_idx)
                    && let Some(thread) = project.threads.get(thread_idx)
                {
                    return (
                        Some(clean(&thread.title)),
                        Some(project.title.clone()),
                        true,
                    );
                }
                (Some("Agents".to_string()), None, false)
            }
            Some(AgentsTarget::Chat { chat_idx }) => {
                if let Some(chat) = self.chats.get(chat_idx) {
                    return (Some(clean(&chat.title)), Some("Chat".to_string()), true);
                }
                (Some("Agents".to_string()), None, false)
            }
            None => {
                // Picker/home state - neutral label (AC4): the new-chat intent,
                // else the active project name, else a plain "Agents".
                let neutral =
                    if self.agents_picker_context == crate::project::AgentsPickerContext::NewChat {
                        "New chat".to_string()
                    } else if let Some(project) = self.projects.get(self.active_project_idx) {
                        project.title.clone()
                    } else {
                        "Agents".to_string()
                    };
                (Some(neutral), None, false)
            }
        }
    }

    /// US-011: handle the title-bar `⋯` dispatch. Resolves the current
    /// thread/chat target and opens the shared context menu anchored just
    /// below the title bar. A no-op outside Agents mode or when nothing is
    /// selected (the button only renders with a live target, but guard
    /// anyway). The menu reuses `agents_menu_open` so click-outside-to-close
    /// and the deferred render path are shared with the right-click menus.
    pub(crate) fn handle_open_agents_thread_menu(
        &mut self,
        _: &crate::OpenAgentsThreadMenu,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(self.mode, AppMode::Agents) {
            return;
        }
        let Some(target) = self.agents_target else {
            return;
        };
        if self.thread_for_target(target).is_none() {
            return;
        }
        // Anchor below the title bar near the brand slot. `render_open_agents_menu`
        // clamps to the window bounds if it would overflow the bottom.
        let position = gpui::point(px(12.), px(40.));
        let menu = match target {
            crate::project::AgentsTarget::Thread {
                project_idx,
                thread_idx,
            } => crate::app::agents_sidebar::AgentsContextMenu::Thread {
                project_idx,
                thread_idx,
                position,
            },
            crate::project::AgentsTarget::Chat { chat_idx } => {
                crate::app::agents_sidebar::AgentsContextMenu::Chat { chat_idx, position }
            }
        };
        self.cancel_agents_rename(cx);
        self.agents_view.agents_menu_open = Some(menu);
        cx.notify();
    }

    /// React to an OSC-driven title update from the PTY backing a
    /// Terminal Thread. Updates the matching sidebar row's title and
    /// persists the session so the new label survives a restart.
    ///
    /// Skips two cases on purpose:
    /// 1. Empty / whitespace-only titles -- some shells emit a stray
    ///    blank `ESC]0;\x07` on startup before the real prompt loads.
    /// 2. The literal `"Terminal"` fallback alacritty stamps after a
    ///    `ResetTitle` OSC, so a child shell exiting (e.g. `claude`
    ///    completing a session) does not wipe the meaningful
    ///    process-reported title with a generic placeholder.
    pub(crate) fn handle_terminal_thread_title_changed(
        &mut self,
        thread_id: u64,
        new_title: String,
        cx: &mut Context<Self>,
    ) {
        // Strips whitespace + leading spinner/bullet glyphs (Codex
        // braille, Claude Code pinwheel, generic `●`/`•`). Returns
        // `None` if nothing meaningful is left.
        let Some(normalized) = crate::project::clean_sidebar_title(&new_title) else {
            return;
        };
        if normalized == "Terminal" {
            // Don't let alacritty's `ResetTitle` fallback wipe a
            // meaningful process-reported title once a child shell
            // exits and the title resets to the default.
            return;
        }
        for project in self.projects.iter_mut() {
            if let Some(thread) = project.threads.iter_mut().find(|t| t.id == thread_id) {
                // A manual rename is authoritative: neither an OSC update nor
                // the `ai-title` backfill may clobber a deliberate label.
                if thread.title_user_set || thread.title == normalized {
                    return;
                }
                thread.title = normalized;
                self.save_session(cx);
                cx.notify();
                return;
            }
        }
        // US-003: a chat is a Thread too - its PTY emits OSC titles just
        // like a project thread, so the same label-sync applies to the
        // free `chats` list.
        if let Some(thread) = self.chats.iter_mut().find(|t| t.id == thread_id) {
            if thread.title_user_set || thread.title == normalized {
                return;
            }
            thread.title = normalized;
            self.save_session(cx);
            cx.notify();
        }
    }

    /// Adopt the live agent session's LLM `ai-title` as the thread's sidebar
    /// label at turn end - the same summary `/resume` surfaces. Reads the
    /// on-disk session store off the main thread, then picks only the bound
    /// session created by the thread's forced session id. Agents without a
    /// forced id intentionally skip this path: cwd-newest matching can rename
    /// the wrong thread when several agents share a repository.
    pub(crate) fn spawn_thread_title_backfill(
        &self,
        thread_id: u64,
        cwd: String,
        agent: crate::agent_sessions::SessionAgent,
        bound_session: String,
        cx: &mut Context<Self>,
    ) {
        if cwd.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let sessions = smol::unblock(move || read_sessions_for(agent, &cwd)).await;
            if let Some(summary) = title_summary_for_bound_session(sessions, &bound_session) {
                let _ = this.update(cx, |app, cx| {
                    app.handle_terminal_thread_title_changed(thread_id, summary, cx);
                });
            }
        })
        .detach();
    }

    // Sidebar render branch for [`AppMode::Agents`] now lives in
    // [`crate::app::agents_sidebar`] -- US-010 replaced the
    // placeholder shipped here in US-008.
}

/// Dispatch a cwd-scoped session scan to the matching on-disk reader.
/// **Blocking I/O** - call from inside `smol::unblock`.
fn read_sessions_for(
    agent: crate::agent_sessions::SessionAgent,
    cwd: &str,
) -> Vec<crate::agent_sessions::SessionMeta> {
    crate::agent_sessions::read_sessions_for_cwd(agent, cwd)
}

fn title_summary_for_bound_session(
    sessions: Vec<crate::agent_sessions::SessionMeta>,
    bound_session: &str,
) -> Option<String> {
    sessions
        .into_iter()
        .find(|s| s.session_id == bound_session)
        .and_then(|s| s.summary)
        .filter(|summary| !summary.is_empty())
}

/// US-005: where the agent picker creates its launched agent. Drives the
/// launcher title and the on-click create path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LauncherContext {
    /// Create a terminal thread in `projects[idx]`.
    Project(usize),
    /// Create a free chat in the home dir.
    NewChat,
}

/// Wrap a [`TerminalView`] entity into the Agents main area surface.
/// Pulled into a free function so the dispatch branch in
/// [`PaneFlowApp::render_agents_main_body`] stays one line and so the
/// PTY background/padding policy (match the CLI pane shell) lives in a
/// single named spot.
pub(crate) fn render_terminal_thread_surface(
    view: gpui::Entity<crate::terminal::view::TerminalView>,
    max_content_width: u32,
    cwd: String,
    overlay: AgentsEnvironmentOverlayState,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let ui = crate::theme::ui_colors();
    div()
        .size_full()
        .relative()
        .flex()
        .flex_col()
        .bg(ui.base)
        // Reserved top band: pushes the terminal down so the absolutely-anchored
        // toolbar overlay below sits above the CLI, never over it, at any width.
        .child(div().h(px(AGENTS_TOOLBAR_BAND_HEIGHT)).flex_none())
        .child(
            div().flex_1().min_h_0().flex().justify_center().child(
                div()
                    .h_full()
                    .w_full()
                    .max_w(px(max_content_width as f32))
                    .child(view.into_any_element()),
            ),
        )
        .child(render_agents_environment_overlay(cwd, overlay, ui, cx))
        .into_any_element()
}

pub(crate) struct AgentsEnvironmentOverlayState {
    editor_menu_open: bool,
    editor_value: String,
    bottom_open: bool,
}

fn render_agents_environment_overlay(
    cwd: String,
    overlay: AgentsEnvironmentOverlayState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    div()
        .absolute()
        .top(px(20.))
        .right(px(12.))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(22.))
        .occlude()
        .child(render_agents_environment_toolbar(
            cwd,
            overlay.editor_menu_open,
            overlay.editor_value,
            overlay.bottom_open,
            ui,
            cx,
        ))
        .into_any_element()
}

fn render_agents_environment_toolbar(
    cwd: String,
    editor_menu_open: bool,
    editor_value: String,
    bottom_open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(render_agents_editor_split_button(
            cwd,
            editor_value,
            editor_menu_open,
            ui,
            cx,
        ))
        .child(
            crate::app::agents_bottom_panel::render_agents_bottom_toggle_button(
                bottom_open,
                ui,
                cx,
            ),
        )
        .into_any_element()
}

fn render_agents_editor_split_button(
    cwd: String,
    editor_value: String,
    editor_menu_open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    // No resting fill: the control reads as two bare sub-buttons sharing one
    // shell. Each half owns a rounded hover background that lights independently
    // - hovering the logo never tints the chevron, and vice-versa.
    let resting_bg = crate::settings::components::with_alpha(ui.text, 0.0);
    let hover_bg = crate::settings::components::with_alpha(ui.text, 0.10);
    let open_cwd = cwd.clone();
    let open_editor = editor_value.clone();
    let mut button = div()
        .id("agents-env-toolbar-editor")
        .relative()
        .flex_none()
        .h(px(28.))
        .flex()
        .flex_row()
        .items_center()
        // Codex shell: a hairline border wraps both halves; the two hover
        // backgrounds meet flush at the center (square inner corners) while the
        // outer corners follow the shell radius (matches the toggle buttons).
        .rounded(px(10.))
        .border_1()
        .border_color(crate::settings::components::with_alpha(ui.text, 0.14))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            // Logo half - opens the folder in the selected editor. Shows that
            // editor's own (colored) logo so the button surfaces which editor
            // will launch.
            div()
                .id("agents-env-toolbar-editor-open")
                .h_full()
                .w(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_tl(px(9.))
                .rounded_bl(px(9.))
                .bg(resting_bg)
                .animated_hover_bg(resting_bg, hover_bg)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.open_agents_environment_in_editor(
                        open_cwd.clone(),
                        open_editor.clone(),
                        event,
                        window,
                        cx,
                    );
                }))
                .child(render_agents_editor_toolbar_icon(&editor_value, ui)),
        )
        .child(
            // Chevron half - opens the editor picker menu.
            div()
                .id("agents-env-toolbar-editor-chevron")
                .h_full()
                .w(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_tr(px(9.))
                .rounded_br(px(9.))
                .bg(resting_bg)
                .animated_hover_bg(resting_bg, hover_bg)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.toggle_agents_editor_menu(event, window, cx);
                }))
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path("icons/chevron-down.svg")
                        .text_color(ui.muted),
                ),
        );

    if editor_menu_open {
        button = button.child(render_agents_editor_menu(editor_value, ui, cx));
    }

    button.into_any_element()
}

fn render_agents_editor_toolbar_icon(
    editor_value: &str,
    ui: crate::theme::UiColors,
) -> gpui::AnyElement {
    if let Some(icon) = crate::settings::tabs::general::editor_icon(editor_value) {
        crate::settings::components::render_logo(icon, ui)
    } else {
        svg()
            .size(px(14.))
            .flex_none()
            .path("icons/edit.svg")
            .text_color(ui.muted)
            .into_any_element()
    }
}

fn render_agents_editor_menu(
    current: String,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let mut menu =
        crate::settings::components::menu_surface(div().id("agents-env-editor-menu"), ui)
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .w(px(220.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(PaneFlowApp::close_agents_editor_menu));

    for (idx, (label, value)) in crate::settings::tabs::general::EDITOR_PRESETS
        .iter()
        .enumerate()
    {
        let value_owned = (*value).to_string();
        let selected = current == *value;
        let mut item =
            crate::settings::components::select_item(("agents-env-editor", idx), selected, ui)
                .cursor(CursorStyle::Arrow)
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.select_agents_environment_editor(value_owned.clone(), event, window, cx);
                }));

        if let Some(icon) = crate::settings::tabs::general::editor_icon(value) {
            item = item.child(crate::settings::components::render_logo(icon, ui));
        } else {
            item = item.child(div().size(px(14.)).flex_none());
        }

        menu = menu.child(
            item.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(ui.text)
                    .child(*label),
            ),
        );
    }

    deferred(
        div()
            .absolute()
            .top(px(34.))
            .right(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

fn read_agents_environment_git_state(cwd: &str) -> (String, bool, crate::workspace::GitDiffStats) {
    let (branch, is_repo) = crate::workspace::detect_branch(cwd);
    let stats = crate::workspace::GitDiffStats::from_cwd(cwd);
    (branch, is_repo, stats)
}

fn open_agents_cwd_with_editor(cwd: &str, editor_value: &str) -> Result<String, String> {
    if cwd.trim().is_empty() {
        return Err("No folder is associated with this thread".to_string());
    }
    let path = std::path::Path::new(cwd);

    match editor_value {
        "system" => open_agents_cwd_with_system_handler(path),
        "auto" => open_agents_cwd_auto(path),
        "zed" | "cursor" | "windsurf" | "code" | "visual_studio" => {
            let (label, command) = agents_editor_command(editor_value);
            spawn_agents_editor(path, command, label).map(|_| label.to_string())
        }
        other => {
            let label = agents_editor_label(other);
            spawn_agents_editor(path, other, &label).map(|_| label)
        }
    }
}

fn open_agents_cwd_auto(path: &std::path::Path) -> Result<String, String> {
    let mut last_error = None;
    for value in ["zed", "cursor", "windsurf", "code"] {
        let (label, command) = agents_editor_command(value);
        match spawn_agents_editor(path, command, label) {
            Ok(()) => return Ok(label.to_string()),
            Err(err) => last_error = Some(err),
        }
    }

    open_agents_cwd_with_system_handler(path).map_err(|system_err| last_error.unwrap_or(system_err))
}

fn open_agents_cwd_with_system_handler(path: &std::path::Path) -> Result<String, String> {
    crate::app::workspace_ops::open_folder_in_file_manager(path)
        .map(|_| "System default".to_string())
}

fn spawn_agents_editor(path: &std::path::Path, command: &str, label: &str) -> Result<(), String> {
    let bin = crate::app::workspace_ops::resolve_editor_binary(command);
    let mut cmd = std::process::Command::new(&bin);
    cmd.current_dir(path).arg(".");
    paneflow_process::spawn_detached(&mut cmd)
        .map_err(|err| format!("Couldn't open in {label}: {err}"))
}

fn agents_editor_command(value: &str) -> (&'static str, &str) {
    match value {
        "zed" => ("Zed", "zed"),
        "cursor" => ("Cursor", "cursor"),
        "windsurf" => ("Windsurf", "windsurf"),
        "code" => ("VS Code", "code"),
        "visual_studio" => ("Visual Studio", "devenv"),
        _ => ("System default", value),
    }
}

fn agents_editor_label(value: &str) -> String {
    crate::settings::tabs::general::EDITOR_PRESETS
        .iter()
        .find(|(_, preset_value)| *preset_value == value)
        .map(|(label, _)| (*label).to_string())
        .unwrap_or_else(|| value.to_string())
}

/// US-013: unified welcome/home empty-state when the Agents cockpit has no
/// project AND no chat to open. Invites the two entry points the rail now
/// exposes: the header's compose action (a quick home-dir session) and the
/// `+` next to the PROJECTS eyebrow (add a repo). Copy stays in sync with
/// those visible affordances.
fn render_agents_no_project() -> gpui::AnyElement {
    let ui = crate::theme::ui_colors();
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .px(px(20.))
        // Transparent: the Agents main wrapper paints the panel bg.
        .child(
            div()
                .text_size(px(16.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child("Start working with agents"),
        )
        .child(
            div()
                .max_w(px(420.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .text_center()
                .child(
                    "Use the compose button in the Agents header for a quick session in your \
                     home directory, or + next to Projects to add a repository.",
                ),
        )
        .into_any_element()
}

fn render_agents_terminal_loading() -> gpui::AnyElement {
    let ui = crate::theme::ui_colors();
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(12.))
        .text_color(ui.muted)
        .child("Opening terminal")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_lru_moves_existing_id_to_back() {
        let mut order = vec![1, 2, 3];
        touch_lru(&mut order, 2);
        assert_eq!(order, vec![1, 3, 2]);
    }

    #[test]
    fn oldest_evictable_terminal_id_protects_active_and_running() {
        let order = vec![1, 2, 3, 4];
        let exited = std::collections::HashSet::from([1, 2, 4]);

        let evict = oldest_evictable_terminal_id(&order, Some(1), 9, 8, |id| exited.contains(&id));
        assert_eq!(
            evict,
            Some(2),
            "oldest active entry is protected, next exited entry is evicted"
        );

        let evict = oldest_evictable_terminal_id(&order, Some(2), 9, 8, |id| id == 3);
        assert_eq!(
            evict,
            Some(3),
            "a running active entry is skipped but an evictable inactive entry can drop"
        );

        let evict = oldest_evictable_terminal_id(&order, None, 8, 8, |_| true);
        assert_eq!(evict, None, "cache at budget does not evict");
    }

    #[test]
    fn title_summary_for_bound_session_requires_exact_id_match() {
        let sessions = vec![
            session_meta("newest", "wrong title"),
            session_meta("bound", "right title"),
        ];

        assert_eq!(
            title_summary_for_bound_session(sessions, "bound").as_deref(),
            Some("right title"),
            "title backfill must not choose the newest cwd session"
        );
    }

    #[test]
    fn title_summary_for_bound_session_drops_missing_or_empty_summary() {
        let mut empty = session_meta("bound", "");
        empty.summary = Some(String::new());
        let missing = session_meta("other", "other title");

        assert_eq!(
            title_summary_for_bound_session(vec![missing], "bound"),
            None
        );
        assert_eq!(title_summary_for_bound_session(vec![empty], "bound"), None);
    }

    fn session_meta(id: &str, summary: &str) -> crate::agent_sessions::SessionMeta {
        crate::agent_sessions::SessionMeta {
            agent: crate::agent_sessions::SessionAgent::Claude,
            session_id: id.to_string(),
            timestamp: "2026-07-05T12:00:00Z".to_string(),
            cwd: "/repo".to_string(),
            git_branch: "main".to_string(),
            summary: Some(summary.to_string()),
            model: None,
            usage: None,
        }
    }
}
