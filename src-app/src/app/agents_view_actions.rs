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
use crate::{AgentsBranchMenuState, OpenAgentsView, PaneFlowApp};
use gpui::{
    AppContext, ClickEvent, Context, CursorStyle, Focusable, FontWeight, InteractiveElement,
    IntoElement, MouseButton, ParentElement, SharedString, StatefulInteractiveElement, Styled,
    Window, deferred, div, prelude::FluentBuilder, px, rgb, svg,
};
use paneflow_config::schema::{AppMode, TerminalSurfaceProfile};
use serde_json::Value;

/// Sidebar width when in [`AppMode::Agents`]. Slightly wider than the
/// CLI sidebar (220 px) because thread rows carry more metadata
/// (agent icon, status dot, relative timestamp) than workspace rows.
/// US-009 surfaces this constant to the title bar so the resize edge
/// snaps to the right slot on mode toggle.
pub(crate) const AGENTS_SIDEBAR_WIDTH: f32 = 280.0;

const AGENTS_ENVIRONMENT_PANEL_WIDTH: f32 = 300.0;
/// Empty band reserved at the top of the agents terminal surface so the
/// floating environment toolbar (model selector + layout toggles) lives in its
/// own strip and never paints over the CLI when the window or right diff panel
/// is resized narrow. Sized to clear the overlay: top inset (20) + button
/// height (28) + breathing room (8). Keep in sync with the toolbar `top` and
/// button heights below.
const AGENTS_TOOLBAR_BAND_HEIGHT: f32 = 56.0;
const AGENTS_BRANCH_GIT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const AGENTS_BRANCH_GIT_OUTPUT_CAP: u64 = 512 * 1024;
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
            // Both panel resizes (the bottom dock's top edge and the diff dock's
            // left edge) are captured here, on the full-height main area, so a
            // drag keeps tracking even when the cursor outruns its handle and
            // crosses into the surface beside it.
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
                    } else if this.agents_view.agents_diff_h_scroll_drag.is_some() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.drag_agents_diff_h_scrollbar(event.position.x, cx);
                        } else {
                            this.end_agents_diff_h_scrollbar_drag(cx);
                        }
                    } else if this.agents_view.agents_diff_resize.is_some() {
                        if event.pressed_button == Some(MouseButton::Left) {
                            this.drag_agents_diff_resize(f32::from(event.position.x), cx);
                        } else {
                            this.end_agents_diff_resize(cx);
                        }
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _e: &gpui::MouseUpEvent, _w, cx| {
                    this.end_bottom_panel_resize(cx);
                    this.end_agents_diff_h_scrollbar_drag(cx);
                    this.end_agents_diff_resize(cx);
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
            let environment = self.agents_environment_summary(target);
            let diff_open = self.agents_view.agents_diff_open;
            let surface = render_terminal_thread_surface(
                view,
                max_content_width,
                environment,
                AgentsEnvironmentOverlayState {
                    panel_open: self.agents_view.agents_environment_panel_open,
                    editor_menu_open: self.agents_view.agents_editor_menu_open,
                    editor_value: self
                        .cached_config
                        .external_editor
                        .clone()
                        .unwrap_or_else(|| "auto".to_string()),
                    branch_menu: self.agents_view.agents_branch_menu.clone(),
                    diff_open,
                    bottom_open: self.agents_view.bottom_panel_open,
                },
                cx,
            );
            // Codex-style diff dock: when open, the thread surface shares the
            // main area with a fixed-width diff panel on the right.
            if diff_open {
                let ui = crate::theme::ui_colors();
                return div()
                    .size_full()
                    .flex()
                    .flex_row()
                    .child(div().flex_1().min_w_0().h_full().child(surface))
                    .child(self.render_agents_diff_panel(ui, cx))
                    .into_any_element();
            }
            return surface;
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

    fn agents_environment_summary(
        &self,
        target: crate::project::AgentsTarget,
    ) -> AgentsEnvironmentSummary {
        match target {
            crate::project::AgentsTarget::Thread { project_idx, .. } => {
                let Some(project) = self.projects.get(project_idx) else {
                    return AgentsEnvironmentSummary::default();
                };
                let (branch, is_repo, git_stats) =
                    self.agents_environment_git_for_cwd(&project.cwd);
                AgentsEnvironmentSummary {
                    cwd: project.cwd.clone(),
                    branch,
                    is_repo,
                    git_stats,
                }
            }
            crate::project::AgentsTarget::Chat { .. } => {
                let cwd = self
                    .thread_for_target(target)
                    .map(|thread| thread.cwd.clone())
                    .unwrap_or_default();
                let (branch, is_repo, git_stats) = self.agents_environment_git_for_cwd(&cwd);
                AgentsEnvironmentSummary {
                    cwd,
                    branch,
                    is_repo,
                    git_stats,
                }
            }
        }
    }

    pub(crate) fn agents_environment_git_for_cwd(
        &self,
        cwd: &str,
    ) -> (String, bool, crate::workspace::GitDiffStats) {
        let cached = self.agents_view.agents_environment_git.get(cwd);
        let workspace = self
            .workspaces
            .iter()
            .find(|workspace| workspace.cwd.as_str() == cwd);
        let project = self.projects.iter().find(|project| project.cwd == cwd);
        let is_repo = workspace
            .map(|workspace| workspace.is_git_repo)
            .or_else(|| cached.map(|state| state.is_repo))
            .unwrap_or(false);
        let branch = if is_repo {
            workspace
                .and_then(|workspace| agents_environment_branch_label(&workspace.git_branch))
                .or_else(|| cached.and_then(|state| agents_environment_branch_label(&state.branch)))
                .unwrap_or_else(|| "Repository".to_string())
        } else {
            "No repository".to_string()
        };
        let git_stats = cached
            .map(|state| state.stats.clone())
            .or_else(|| project.map(|project| project.git_stats.clone()))
            .or_else(|| workspace.map(|workspace| workspace.git_stats.clone()))
            .unwrap_or_default();
        (branch, is_repo, git_stats)
    }

    fn toggle_agents_branch_menu(
        &mut self,
        cwd: String,
        current: String,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if cwd.trim().is_empty() {
            return;
        }
        let same_menu_open = self
            .agents_view
            .agents_branch_menu
            .as_ref()
            .is_some_and(|menu| menu.cwd == cwd);
        if same_menu_open {
            self.agents_view.agents_branch_menu = None;
            cx.notify();
            return;
        }

        self.agents_view.agents_editor_menu_open = false;
        let query_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search branches", cx));
        cx.observe(&query_input, |_, _, cx| cx.notify()).detach();
        self.agents_view.agents_branch_menu = Some(AgentsBranchMenuState {
            cwd: cwd.clone(),
            current: current.clone(),
            branches: Vec::new(),
            loading: true,
            error: None,
            query_input: query_input.clone(),
        });
        query_input.read(cx).focus_handle.clone().focus(window, cx);
        cx.notify();

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock({
                    let cwd = cwd.clone();
                    move || list_agents_environment_branches(&cwd)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| {
                        let Some(menu) = app.agents_view.agents_branch_menu.as_mut() else {
                            return;
                        };
                        if menu.cwd != cwd {
                            return;
                        }
                        menu.loading = false;
                        match result {
                            Ok(branches) => {
                                menu.branches = branches;
                                menu.error = None;
                            }
                            Err(error) => {
                                menu.branches.clear();
                                menu.error = Some(error);
                            }
                        }
                        cx.notify();
                    })
                });
            },
        )
        .detach();
    }

    fn close_agents_branch_menu(
        &mut self,
        _: &gpui::MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agents_view.agents_branch_menu.take().is_some() {
            cx.notify();
        }
    }

    fn toggle_agents_environment_panel(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_environment_panel_open =
            !self.agents_view.agents_environment_panel_open;
        self.agents_view.agents_editor_menu_open = false;
        if !self.agents_view.agents_environment_panel_open {
            self.agents_view.agents_branch_menu = None;
        } else if let Some(target) = self.current_thread_view_target()
            && let Some(cwd) = self
                .thread_for_target(target)
                .map(|thread| thread.cwd.clone())
        {
            self.spawn_agents_environment_git_refresh(cwd, cx);
        }
        cx.notify();
    }

    fn toggle_agents_editor_menu(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_editor_menu_open = !self.agents_view.agents_editor_menu_open;
        if self.agents_view.agents_editor_menu_open {
            self.agents_view.agents_branch_menu = None;
        }
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

    fn switch_agents_branch(
        &mut self,
        cwd: String,
        branch: String,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.agents_view.agents_branch_menu = None;
        self.focus_current_agents_terminal(window, cx);
        cx.notify();
        self.spawn_switch_branch(cwd, branch, cx);
    }

    /// Background `git switch` to an existing branch, then refresh the cached git
    /// state for every workspace/project rooted at `cwd`. Shared by the branch-row
    /// click and the search field's Enter-on-exact-match.
    fn spawn_switch_branch(&mut self, cwd: String, branch: String, cx: &mut Context<Self>) {
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock({
                    let cwd = cwd.clone();
                    let branch = branch.clone();
                    move || switch_agents_environment_branch(&cwd, &branch)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| match result {
                        Ok((branch_now, is_repo, stats)) => {
                            app.apply_git_state_for_cwd(&cwd, branch_now, is_repo, stats);
                            app.refresh_agents_diff_if_open_for_cwd(&cwd, cx);
                            app.show_toast(format!("Switched to {branch}"), cx);
                            cx.notify();
                        }
                        Err(error) => {
                            app.show_toast(format!("Couldn't switch to {branch}: {error}"), cx);
                        }
                    })
                });
            },
        )
        .detach();
    }

    /// Return keyboard focus to the active thread's terminal after the branch
    /// picker closes, so typing resumes in the PTY instead of landing on the
    /// dropped menu focus handle.
    fn focus_current_agents_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.current_thread_view_target() else {
            return;
        };
        let Some(thread_id) = self.thread_for_target(target).map(|t| t.id) else {
            return;
        };
        if let Some(view) = self
            .agents_view
            .agents_terminal_view_cache
            .get(&thread_id)
            .cloned()
        {
            view.read(cx).focus_handle(cx).focus(window, cx);
        }
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

    /// Keyboard handling for branch-picker commands that bubble out of the
    /// focused `TextInput`: Enter switches to an exact match, Escape dismisses.
    pub(crate) fn handle_agents_branch_menu_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.agents_view.agents_branch_menu.is_none() {
            return;
        }
        match event.keystroke.key.as_str() {
            "escape" => {
                self.agents_view.agents_branch_menu = None;
                self.focus_current_agents_terminal(window, cx);
                cx.notify();
            }
            "enter" => {
                // With the create action removed, Enter only switches to an exact
                // match; a non-matching query is a no-op (keep filtering / click).
                let resolved = {
                    let Some(menu) = self.agents_view.agents_branch_menu.as_ref() else {
                        return;
                    };
                    let name = menu.query_input.read(cx).value().trim().to_string();
                    if name.is_empty() || !menu.branches.contains(&name) {
                        return;
                    }
                    (menu.cwd.clone(), name)
                };
                let (cwd, name) = resolved;
                self.agents_view.agents_branch_menu = None;
                self.focus_current_agents_terminal(window, cx);
                cx.notify();
                self.spawn_switch_branch(cwd, name, cx);
            }
            _ => {}
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
        let state = crate::AgentsGitState {
            branch: branch.clone(),
            is_repo,
            stats: stats.clone(),
        };
        if self.agents_view.agents_environment_git.get(cwd) != Some(&state) {
            self.agents_view
                .agents_environment_git
                .insert(cwd.to_string(), state);
            changed = true;
        }
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
        let state = self
            .agents_view
            .agents_environment_git
            .entry(cwd.to_string())
            .or_default();
        if state.stats != stats {
            state.stats = stats.clone();
            changed = true;
        }
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
    environment: AgentsEnvironmentSummary,
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
        .child(render_agents_environment_overlay(
            environment,
            overlay,
            ui,
            cx,
        ))
        .into_any_element()
}

pub(crate) struct AgentsEnvironmentOverlayState {
    panel_open: bool,
    editor_menu_open: bool,
    editor_value: String,
    branch_menu: Option<AgentsBranchMenuState>,
    diff_open: bool,
    bottom_open: bool,
}

#[derive(Clone)]
pub(crate) struct AgentsEnvironmentSummary {
    cwd: String,
    branch: String,
    is_repo: bool,
    git_stats: crate::workspace::GitDiffStats,
}

impl Default for AgentsEnvironmentSummary {
    fn default() -> Self {
        Self {
            cwd: String::new(),
            branch: "No repository".to_string(),
            is_repo: false,
            git_stats: crate::workspace::GitDiffStats::default(),
        }
    }
}

fn agents_environment_branch_label(branch: &str) -> Option<String> {
    let branch = branch.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

fn render_agents_environment_overlay(
    summary: AgentsEnvironmentSummary,
    overlay: AgentsEnvironmentOverlayState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    div()
        .absolute()
        .top(px(20.))
        .right(px(12.))
        .w(px(AGENTS_ENVIRONMENT_PANEL_WIDTH))
        .flex()
        .flex_col()
        .items_end()
        .gap(px(22.))
        .occlude()
        .child(render_agents_environment_toolbar(
            summary.cwd.clone(),
            overlay.panel_open,
            overlay.editor_menu_open,
            overlay.editor_value,
            overlay.diff_open,
            overlay.bottom_open,
            ui,
            cx,
        ))
        .when(overlay.panel_open, |element| {
            element.child(render_agents_environment_card(
                summary,
                overlay.branch_menu,
                ui,
                cx,
            ))
        })
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn render_agents_environment_toolbar(
    cwd: String,
    environment_panel_open: bool,
    editor_menu_open: bool,
    editor_value: String,
    diff_open: bool,
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
        .child(render_agents_environment_toggle_button(
            environment_panel_open,
            ui,
            cx,
        ))
        .child(crate::app::agents_diff::render_agents_diff_toggle_button(
            diff_open, ui, cx,
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

fn render_agents_environment_toggle_button(
    open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    // Codex hierarchy: the list toggle is a bare glyph at rest (no resting
    // fill, unlike the filled editor split-button beside it); a whisper fill
    // only on hover or while the panel is open.
    let fill = crate::settings::components::with_alpha(ui.text, if open { 0.08 } else { 0.0 });
    let hover = crate::settings::components::with_alpha(ui.text, 0.08);
    div()
        .id("agents-env-toolbar-list")
        .flex_none()
        .h(px(28.))
        .w(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .bg(fill)
        .animated_hover_bg(fill, hover)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.toggle_agents_environment_panel(event, window, cx);
        }))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path("icons/list-details.svg")
                .text_color(crate::settings::components::with_alpha(ui.text, 0.7)),
        )
        .into_any_element()
}

fn render_agents_environment_card(
    summary: AgentsEnvironmentSummary,
    branch_menu: Option<AgentsBranchMenuState>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let panel_bg = if crate::theme::active_theme().background.l > 0.5 {
        gpui::Hsla::from(rgb(0xffffff))
    } else {
        gpui::Hsla::from(rgb(0x2d2d2d))
    };
    let panel_border = if crate::theme::active_theme().background.l > 0.5 {
        gpui::Hsla::from(rgb(0xdedee6))
    } else {
        gpui::Hsla::from(rgb(0x383838))
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .rounded(px(20.))
        .bg(panel_bg)
        .border_1()
        .border_color(panel_border)
        .child(render_agents_environment_header(ui, cx))
        .child(render_agents_environment_changes_row(&summary, ui, cx))
        .child(render_agents_environment_branch_row(
            summary,
            branch_menu,
            ui,
            cx,
        ))
        .into_any_element()
}

/// Codex "Environment" card header: a muted section title on the left and a
/// settings gear on the right that opens the app's Settings window. The gear is
/// the only header affordance Codex exposes here, so it is fully wired (no
/// decorative controls).
fn render_agents_environment_header(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let resting_background = crate::settings::components::with_alpha(ui.text, 0.0);
    let hover_background = crate::settings::components::with_alpha(ui.text, 0.08);

    div()
        .h(px(20.))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::NORMAL)
                .text_color(ui.muted)
                .child("Environment"),
        )
        .child(
            div()
                .id("agents-env-settings-gear")
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(resting_background)
                .animated_hover_bg(resting_background, hover_background)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_settings_window(window, cx);
                }))
                .child(
                    svg()
                        .size(px(15.))
                        .flex_none()
                        .path("icons/settings.svg")
                        .text_color(ui.muted),
                ),
        )
        .into_any_element()
}

fn render_agents_environment_changes_row(
    summary: &AgentsEnvironmentSummary,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let cwd = summary.cwd.clone();
    let insertions = summary.git_stats.insertions;
    let deletions = summary.git_stats.deletions;
    let is_repo = summary.is_repo;
    let resting_background = crate::settings::components::with_alpha(ui.text, 0.0);
    let hover_background =
        crate::settings::components::with_alpha(ui.text, if is_repo { 0.06 } else { 0.0 });
    // Reuse the right diff panel's palette so the +/- counts match the washes
    // there (Codex green/red on dark themes, theme vc_* on light).
    let (added_color, deleted_color) = crate::app::agents_diff::agents_diff_count_colors(ui);
    let row = div()
        .id("agents-env-changes-row")
        .relative()
        .w(px(AGENTS_ENVIRONMENT_PANEL_WIDTH - 16.0))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .mx(px(-8.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(8.))
        .bg(resting_background)
        .animated_hover_bg(resting_background, hover_background)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(is_repo, |row| {
            row.on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.spawn_agents_environment_git_refresh(cwd.clone(), cx);
                this.open_agents_diff_panel(cwd.clone(), cx);
            }))
        });
    row.child(render_agents_environment_label(
        "icons/file-text.svg",
        "Changes",
        ui,
    ))
    .child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .text_size(px(13.))
            .child(
                div()
                    .text_color(if is_repo { added_color } else { ui.muted })
                    .child(format!("+{insertions}")),
            )
            .child(
                div()
                    .text_color(if is_repo { deleted_color } else { ui.muted })
                    .child(format!("-{deletions}")),
            ),
    )
    .into_any_element()
}

fn render_agents_environment_branch_row(
    summary: AgentsEnvironmentSummary,
    branch_menu: Option<AgentsBranchMenuState>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let menu_open = branch_menu
        .as_ref()
        .is_some_and(|menu| menu.cwd == summary.cwd);
    let current = summary.branch.clone();
    let cwd = summary.cwd.clone();
    let files_changed = summary.git_stats.files_changed;
    let resting_background = crate::settings::components::with_alpha(ui.text, 0.0);
    let hover_background =
        crate::settings::components::with_alpha(ui.text, if summary.is_repo { 0.06 } else { 0.0 });
    let mut row = div()
        .id("agents-env-branch-row")
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        // Codex/sidebar-tab breathing room: pad the clickable branch button and
        // bleed it 8px into the card padding (mx -8) so the hover reads as a full
        // tab while the label still lines up with the "Changes" row above.
        .mx(px(-8.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(8.))
        .child(render_agents_environment_label(
            "icons/git-branch.svg",
            summary.branch,
            ui,
        ));
    if summary.is_repo {
        row = row
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toggle_agents_branch_menu(cwd.clone(), current.clone(), event, window, cx);
            }))
            .child(
                svg()
                    .size(px(12.))
                    .path("icons/chevron-down.svg")
                    .text_color(ui.muted),
            );
    }
    row.bg(resting_background)
        .animated_hover_bg(resting_background, hover_background)
        .when(menu_open && summary.is_repo, |row| {
            if let Some(menu) = branch_menu {
                row.child(render_agents_branch_menu(menu, files_changed, ui, cx))
            } else {
                row
            }
        })
        .into_any_element()
}

fn render_agents_branch_menu(
    menu_state: AgentsBranchMenuState,
    files_changed: usize,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let cwd = menu_state.cwd.clone();
    let query_input = menu_state.query_input.clone();
    let query = query_input.read(cx).value();
    let query_lc = query.trim().to_lowercase();

    let mut menu =
        crate::settings::components::menu_surface(div().id("agents-env-branch-menu"), ui)
            .flex()
            .flex_col()
            .gap(px(2.))
            .p(px(6.))
            .w(px(280.))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(PaneFlowApp::close_agents_branch_menu))
            .on_key_down(cx.listener(PaneFlowApp::handle_agents_branch_menu_key_down))
            .child(render_agents_branch_search_row(query_input, ui));

    if menu_state.loading {
        menu = menu.child(render_agents_branch_menu_status("Loading branches…", ui));
    } else if let Some(error) = menu_state.error {
        menu = menu.child(render_agents_branch_menu_status(error, ui));
    } else {
        menu = menu.child(
            div()
                .px(px(8.))
                .pt(px(4.))
                .pb(px(2.))
                .text_size(px(11.))
                .text_color(ui.muted)
                .child("Branches"),
        );

        let filtered: Vec<String> = menu_state
            .branches
            .iter()
            .filter(|branch| query_lc.is_empty() || branch.to_lowercase().contains(&query_lc))
            .cloned()
            .collect();

        if filtered.is_empty() {
            menu = menu.child(render_agents_branch_menu_status("No branches", ui));
        } else {
            let mut list = div()
                .id("agents-env-branch-list")
                .flex()
                .flex_col()
                .gap(px(1.))
                .max_h(px(200.))
                .overflow_y_scroll();
            for (idx, branch) in filtered.into_iter().enumerate() {
                let selected = branch == menu_state.current;
                list = list.child(render_agents_branch_item(
                    idx,
                    branch,
                    selected,
                    if selected { files_changed } else { 0 },
                    cwd.clone(),
                    ui,
                    cx,
                ));
            }
            menu = menu.child(list);
        }
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

/// Branch-picker search field. Editing is handled by `TextInput`, while the
/// parent menu handles only Enter/Escape after those keys bubble.
fn render_agents_branch_search_row(
    query_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    ui: crate::theme::UiColors,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .h(px(30.))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/tool_search.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(13.))
                .text_color(ui.text)
                .child(query_input.into_any_element()),
        )
        .into_any_element()
}

/// One branch row: leading branch glyph, the name, an optional "Uncommitted: N
/// files" sub-label on the checked-out branch, and a trailing check.
fn render_agents_branch_item(
    idx: usize,
    branch: String,
    selected: bool,
    files_changed: usize,
    cwd: String,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let item_branch = branch.clone();
    let selected_background = crate::settings::components::with_alpha(ui.text, 0.10);
    let resting_background = if selected {
        selected_background
    } else {
        crate::settings::components::with_alpha(ui.text, 0.0)
    };
    let hover_background = if selected {
        selected_background
    } else {
        crate::settings::components::with_alpha(ui.text, 0.05)
    };
    let row = div()
        .id(SharedString::from(format!("agents-env-branch-{idx}")))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(8.))
        .py(px(6.))
        .rounded(px(8.))
        .bg(resting_background)
        .animated_hover_bg(resting_background, hover_background)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.switch_agents_branch(cwd.clone(), item_branch.clone(), event, window, cx);
        }));
    row.child(
        svg()
            .size(px(16.))
            .flex_none()
            .path("icons/git-branch.svg")
            .text_color(ui.muted),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(1.))
            .child(
                div()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(13.))
                    .text_color(ui.text)
                    .child(branch),
            )
            .when(files_changed > 0, |d| {
                d.child(div().text_size(px(11.)).text_color(ui.muted).child(format!(
                    "Uncommitted: {files_changed} file{}",
                    if files_changed > 1 { "s" } else { "" }
                )))
            }),
    )
    .child(div().w(px(14.)).flex_none().child(if selected {
        svg()
            .size(px(14.))
            .path("icons/check.svg")
            .text_color(ui.text)
            .into_any_element()
    } else {
        div().size(px(14.)).into_any_element()
    }))
    .into_any_element()
}

fn render_agents_branch_menu_status(
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> gpui::AnyElement {
    div()
        .h(px(28.))
        .px(px(8.))
        .flex()
        .items_center()
        .text_size(px(12.))
        .text_color(ui.muted)
        .child(label.into())
        .into_any_element()
}

fn render_agents_environment_label(
    icon_path: &'static str,
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> gpui::AnyElement {
    div()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(ui.text),
        )
        .child(
            div()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(13.))
                .font_weight(FontWeight::NORMAL)
                .text_color(ui.text)
                .child(label.into()),
        )
        .into_any_element()
}

fn list_agents_environment_branches(cwd: &str) -> Result<Vec<String>, String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["branch", "--format=%(refname:short)"])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output = paneflow_process::run_with_timeout(
        command,
        AGENTS_BRANCH_GIT_DEADLINE,
        AGENTS_BRANCH_GIT_OUTPUT_CAP,
    )
    .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(git_output_error(&output));
    }

    let mut branches = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    branches.sort();
    branches.dedup();
    Ok(branches)
}

fn read_agents_environment_git_state(cwd: &str) -> (String, bool, crate::workspace::GitDiffStats) {
    let (branch, is_repo) = crate::workspace::detect_branch(cwd);
    let stats = crate::workspace::GitDiffStats::from_cwd(cwd);
    (branch, is_repo, stats)
}

fn switch_agents_environment_branch(
    cwd: &str,
    branch: &str,
) -> Result<(String, bool, crate::workspace::GitDiffStats), String> {
    let mut command = std::process::Command::new("git");
    command
        .args(["switch", "--", branch])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output = paneflow_process::run_with_timeout(
        command,
        AGENTS_BRANCH_GIT_DEADLINE,
        AGENTS_BRANCH_GIT_OUTPUT_CAP,
    )
    .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(git_output_error(&output));
    }

    Ok(read_agents_environment_git_state(cwd))
}

fn git_output_error(output: &paneflow_process::BoundedOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.trim();
    if message.is_empty() {
        format!("git exited with {}", output.status)
    } else {
        message.lines().next().unwrap_or(message).to_string()
    }
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
