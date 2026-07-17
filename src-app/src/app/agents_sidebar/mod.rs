//! Agents-mode sidebar: project headers + thread rows.
//!
//! This module owns the [`AppMode::Agents`] arm of the sidebar render.
//! It mirrors the visual language of [`crate::app::sidebar`] (cards,
//! hover, scroll wrapper) but speaks the Agents domain model:
//! collapsible project groups, per-thread agent icon, status dot and
//! relative-timestamp.
//!
//! Per the PRD the sidebar is the only surface the user has for
//! cross-thread navigation; it must stay responsive at 60 fps with up
//! to 100 threads per project (5 000 threads total once US-006's DB
//! query lands). For US-010 we lean on the same `overflow_y_scroll +
//! sidebar_list_wrapper` pattern the workspace sidebar uses (200 rows
//! of plain `div` render comfortably under 16 ms on a mid-range
//! laptop, verified manually). When real-world data sizes exceed that
//! envelope, switching to `gpui::list` is a localised change inside
//! [`Self::render_agents_sidebar`] -- no consumer touches the row
//! widgets directly.
//!
//! See US-010 of `tasks/prd-agents-view.md`.

mod affordances;
mod context_menus;
mod filter;
mod header;
mod state;

pub(crate) use context_menus::render_open_agents_menu;
pub(crate) use state::{AgentsContextMenu, AgentsDeleteTarget, AgentsRenameTarget};

use gpui::{
    Animation, AnimationExt, ClickEvent, Context, Font, FontFeatures, FontStyle, FontWeight, Hsla,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, StyledText, TextRun, Transformation, div, percentage,
    prelude::*, px, rgb, svg,
};

use crate::PaneFlowApp;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

use super::agents_view_actions::AGENTS_SIDEBAR_WIDTH;

const AGENTS_SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
const AGENTS_SIDEBAR_LIST_GAP: f32 = 4.0;

/// US-010 (review follow-up): emit a `tracing::debug!` when
/// [`PaneFlowApp::render_agents_sidebar`] exceeds the 16 ms frame
/// budget. We intentionally do NOT debounce filter keystrokes (the
/// lowercase-once fix moved per-keystroke cost well below the 16 ms
/// frame budget; VSCode #6899 and Slack's Quick Switcher both
/// document the same call). This guard is the early-warning if a
/// future regression invalidates that assumption.
///
/// Threshold is 16 ms (the actual single-frame drop boundary at
/// 60 Hz) instead of a safety-margin 12 ms: a 13-15 ms render
/// still hits the frame and is uninteresting; only past 16 ms does
/// the user see a dropped frame. Level is `debug!` instead of
/// `warn!` so the line stays out of `paneflow-debug.log` at
/// `info` level (which is the user-facing default per
/// `main.rs::env_logger`) -- enable `RUST_LOG=paneflow_app::
/// agents_sidebar=debug` to surface it on demand.
struct RenderTimeCanary {
    start: std::time::Instant,
    project_count: usize,
}

impl RenderTimeCanary {
    fn new(project_count: usize) -> Self {
        Self {
            start: std::time::Instant::now(),
            project_count,
        }
    }
}

impl Drop for RenderTimeCanary {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > std::time::Duration::from_millis(16) {
            tracing::debug!(
                target: "paneflow_app::agents_sidebar",
                "render_agents_sidebar exceeded 16ms frame budget: {:.2}ms across {} projects -- US-010 chose no-debounce on the bet that the work stays sub-frame. If this fires repeatedly, profile and consider a 50ms input debounce.",
                elapsed.as_secs_f64() * 1000.0,
                self.project_count,
            );
        }
    }
}

impl PaneFlowApp {
    /// Render the Agents-mode sidebar as Codex-style sections (US-004):
    /// header + New chat action, Search, then the PINNED / PROJECTS (+) /
    /// CHATS eyebrows with their rows, then the Settings footer + mode toggle.
    ///
    /// Visual language matches [`Self::render_sidebar`] (card-style rows,
    /// `sidebar_list_wrapper` scrollbar). Data binds directly from
    /// `self.projects` + `self.chats`. Newest rows appear first (we iterate
    /// in reverse, since [`crate::project::next_thread_id`] is monotonic so
    /// insertion order tracks `created_at`). Empty sections hide their
    /// eyebrow rather than leave an orphan label.
    pub(crate) fn render_agents_sidebar(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // US-010 (review follow-up): render-time canary. The PRD AC
        // proposed a 50ms keystroke debounce, but the lowercase-once
        // fix moved per-keystroke cost well under the 16ms frame
        // budget so debounce was skipped on VSCode/Slack precedent.
        // This Drop-based guard emits a debug-level log if the
        // 16 ms frame budget is ever exceeded -- an early-warning
        // canary so a future regression (added complexity, larger
        // thread count, slower filter impl) gets noticed under a
        // targeted `RUST_LOG=...=debug` profiling session, without
        // spamming `paneflow-debug.log` for every user on slower
        // hardware.
        let _render_canary = RenderTimeCanary::new(self.projects.len());
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();

        let mut sidebar = div()
            .relative()
            .w(px(AGENTS_SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            // Cockpit rail/chrome (#141414) against the #181818 right panel;
            // no divider - the bg step + the
            // floating rounded panel provide the separation.
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col();

        sidebar = sidebar.child(self.render_agents_sidebar_header(ui, cx));

        // -- Scrollable list area. The wheel-scroll behaviour comes
        // from `overflow_y_scroll + track_scroll`; the visible scroll
        // bar has been removed, so the list uses the full sidebar
        // width and there is no trailing gutter.
        //
        // US-004: the rail is structured into Codex sections, top to
        // bottom: Search, PINNED, PROJECTS (with `+`), CHATS, then the
        // bottom Settings footer + mode toggle. New chat lives in the
        // fixed header so the primary action stays visible while scrolling.
        // Empty sections hide their eyebrow (no orphan label over a void).
        let mut list = div()
            .id("agents-sidebar-list")
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            .gap(px(AGENTS_SIDEBAR_LIST_GAP))
            .pt(px(AGENTS_SIDEBAR_LIST_GAP))
            .pb(px(8.))
            // US-009: search sits directly under the fixed Agents header.
            .child(self.render_agents_filter_row(ui, cx));

        // US-010 (audit P1-4): lowercase the needle exactly once per render
        // (the matchers all take a pre-lowered needle). `query` keeps the
        // original case for the user-facing empty-state hint.
        let query = self.agents_view.agents_filter_input.read(cx).value();
        let query_lower = query.to_lowercase();
        let filtering = !query.is_empty();

        // US-009 AC: when a filter is active and matches nothing across ALL
        // sources (projects, threads, chats), swap the body for the hint.
        if filtering && filter::nothing_matches(&self.projects, &self.chats, &query_lower) {
            list = list.child(no_matches_hint(&query, ui));
            sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
            sidebar =
                sidebar.child(self.render_sidebar_settings_footer(self.agents_menu_items(), cx));
            sidebar = sidebar.child(self.render_mode_toggle(cx));
            return sidebar.into_any_element();
        }

        let now_ms = now_unix_millis();
        // US-003: the active row is whatever the unified target points at.
        let agents_target = self.agents_target;
        let renaming = self.agents_view.agents_renaming;
        let rename_input = self.agents_view.agents_rename_input.clone();
        let shared = RowSharedState {
            agents_target,
            renaming,
            rename_input,
            now_ms,
            filtering,
            ui,
        };

        // ---- PINNED section (US-006): pinned threads + chats, cross-source.
        let mut pinned_rows: Vec<gpui::AnyElement> = Vec::new();
        for project_idx in 0..self.projects.len() {
            let tlen = self.projects[project_idx].threads.len();
            for thread_idx in (0..tlen).rev() {
                let thread = &self.projects[project_idx].threads[thread_idx];
                if !thread.pinned {
                    continue;
                }
                if filtering
                    && !filter::thread_visible_in_project(
                        thread,
                        &self.projects[project_idx],
                        &query_lower,
                    )
                {
                    continue;
                }
                let target = crate::project::AgentsTarget::Thread {
                    project_idx,
                    thread_idx,
                };
                pinned_rows.push(self.agents_thread_row_for(
                    target,
                    thread,
                    true,
                    "pinned",
                    &shared,
                    &query_lower,
                    cx,
                ));
            }
        }
        for chat_idx in (0..self.chats.len()).rev() {
            let chat = &self.chats[chat_idx];
            if !chat.pinned {
                continue;
            }
            if filtering && !filter::chat_visible(chat, &query_lower) {
                continue;
            }
            let target = crate::project::AgentsTarget::Chat { chat_idx };
            pinned_rows.push(self.agents_thread_row_for(
                target,
                chat,
                true,
                "pinned",
                &shared,
                &query_lower,
                cx,
            ));
        }
        if !pinned_rows.is_empty() {
            list = list.child(section_eyebrow("PINNED", None, ui, cx));
            for row in pinned_rows {
                list = list.child(row);
            }
        }

        // ---- PROJECTS section (US-007): eyebrow + `+`, then headers/rows.
        list = list.child(section_eyebrow(
            "PROJECTS",
            Some(SharedString::from("agents-projects-add")),
            ui,
            cx,
        ));
        if self.projects.is_empty() {
            list = list.child(projects_empty_hint(ui));
        }
        for project_idx in 0..self.projects.len() {
            let project = &self.projects[project_idx];
            // US-012: skip projects that neither match the filter
            // themselves nor have any matching thread.
            if filtering && !filter::project_visible(project, &query_lower) {
                continue;
            }

            let project_id = project.id;
            // While a filter is active, force-expand matching projects
            // so users see their hits immediately (AC #2's intent --
            // results "filter" the list, not gate it behind a click).
            let is_expanded = if filtering { true } else { project.is_expanded };
            let title = project.title.clone();
            let git_stats = project.git_stats.clone();
            let is_renaming_project = matches!(renaming, Some(AgentsRenameTarget::Project { project_idx: r }) if r == project_idx);

            list = list.child(self.project_header_row(
                ProjectHeaderArgs {
                    project_idx,
                    project_id,
                    title,
                    is_expanded,
                    rename_input: if is_renaming_project {
                        shared.rename_input.clone()
                    } else {
                        None
                    },
                    git_stats,
                    ui,
                },
                cx,
            ));

            if !is_expanded {
                continue;
            }

            // Iterate threads in reverse (newest first). Indices stay
            // tied to the underlying Vec position so `select_thread`
            // and `remove_thread` resolve to the correct row.
            let thread_count_in_project = self.projects[project_idx].threads.len();
            let mut shown_threads = 0usize;
            for thread_idx in (0..thread_count_in_project).rev() {
                let thread = &self.projects[project_idx].threads[thread_idx];
                if filtering && !filter::thread_visible_in_project(thread, project, &query_lower) {
                    continue;
                }
                shown_threads += 1;
                let target = crate::project::AgentsTarget::Thread {
                    project_idx,
                    thread_idx,
                };
                list = list.child(self.agents_thread_row_for(
                    target,
                    thread,
                    thread.pinned,
                    "project",
                    &shared,
                    &query_lower,
                    cx,
                ));
            }

            // Empty-project hint (US-010 AC #10 -> sidebar-level CTA)
            // applies when the underlying project is genuinely empty,
            // NOT when the filter merely hid every thread (then the
            // project header alone is enough signal that there are
            // hidden children).
            if thread_count_in_project == 0 && !filtering {
                list = list.child(empty_project_hint(ui));
            } else if filtering && shown_threads == 0 {
                // The project title matched but no threads did; this
                // never happens with the current `thread_visible_in_project`
                // (which surfaces every thread when the project title
                // matches), but the branch stays here so a future
                // tightening of that rule does not silently collapse
                // the row.
                list = list.child(empty_project_hint(ui));
            }
        }

        // ---- CHATS section (US-008): free chats, newest-first.
        let visible_chats: Vec<usize> = (0..self.chats.len())
            .rev()
            .filter(|&c| !filtering || filter::chat_visible(&self.chats[c], &query_lower))
            .collect();
        if !visible_chats.is_empty() {
            list = list.child(section_eyebrow("CHATS", None, ui, cx));
            for chat_idx in visible_chats {
                let chat = &self.chats[chat_idx];
                let target = crate::project::AgentsTarget::Chat { chat_idx };
                list = list.child(self.agents_thread_row_for(
                    target,
                    chat,
                    chat.pinned,
                    "chat",
                    &shared,
                    &query_lower,
                    cx,
                ));
            }
        }

        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(self.agents_menu_items(), cx));
        sidebar = sidebar.child(self.render_mode_toggle(cx));
        sidebar.into_any_element()
    }

    /// US-006/008: build one thread/chat row from a unified target +
    /// shared per-render state. Centralises the rename-input / match-pos /
    /// is-active wiring so the PINNED, PROJECTS and CHATS sections emit
    /// identical rows (the only difference is `row_scope`, which keeps the
    /// element ids unique when a pinned thread also appears in its project).
    #[allow(clippy::too_many_arguments)]
    fn agents_thread_row_for(
        &self,
        target: crate::project::AgentsTarget,
        thread: &crate::project::Thread,
        is_pinned: bool,
        row_scope: &'static str,
        shared: &RowSharedState,
        query_lower: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let match_pos = if shared.filtering {
            filter::match_positions(&thread.title, query_lower)
        } else {
            None
        };
        self.thread_row(
            ThreadRowArgs {
                target,
                thread_id: thread.id,
                title: thread.title.clone(),
                rename_input: if is_renaming_target(shared.renaming, target) {
                    shared.rename_input.clone()
                } else {
                    None
                },
                created_at_ms: thread.created_at,
                is_active: shared.agents_target == Some(target),
                is_pinned,
                row_scope,
                now_ms: shared.now_ms,
                match_pos,
                ui: shared.ui,
                status: thread.status,
            },
            cx,
        )
    }

    /// Items rendered inside the bottom Settings popover when in
    /// Agents mode. Order: creation actions first, then navigation,
    /// then escape hatch to the real Settings window.
    fn agents_menu_items(&self) -> Vec<crate::app::sidebar_actions_menu::SidebarMenuItem> {
        use crate::app::sidebar_actions_menu::SidebarMenuItem;
        vec![
            SidebarMenuItem {
                id: "agents-menu-skills".into(),
                icon: "icons/tool.svg",
                label: "Skills".into(),
                on_click: Box::new(|app, _w, cx| {
                    app.show_agents_skills(cx);
                }),
            },
            SidebarMenuItem {
                id: "agents-menu-themes".into(),
                icon: "icons/palette.svg",
                label: "Themes".into(),
                on_click: Box::new(|app, w, cx| {
                    app.open_theme_picker(w, cx);
                }),
            },
            SidebarMenuItem {
                id: "agents-menu-about".into(),
                icon: "icons/info-circle.svg",
                label: "About Paneflow".into(),
                on_click: Box::new(|app, _w, cx| {
                    app.show_about_dialog = true;
                    cx.notify();
                }),
            },
            SidebarMenuItem {
                id: "agents-menu-open-settings".into(),
                icon: "icons/settings.svg",
                label: "Settings".into(),
                on_click: Box::new(|app, w, cx| {
                    app.open_settings_window(w, cx);
                }),
            },
        ]
    }

    fn project_header_row(
        &self,
        args: ProjectHeaderArgs,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let ProjectHeaderArgs {
            project_idx,
            project_id,
            title,
            is_expanded,
            rename_input,
            git_stats,
            ui,
        } = args;
        let folder_path = if is_expanded {
            "icons/folder-open.svg"
        } else {
            "icons/folder.svg"
        };
        let hover_background = crate::app::constants::sidebar_tab_active_background();
        let resting_background = hover_background.opacity(0.0);

        // Title element switches between read-only and inline-input
        // mode. The input is a full [`TextArea`] entity (cursor,
        // selection, IME, clipboard, click-to-position, double-click
        // word select) -- same widget the chat composer uses, so the
        // editing experience is consistent across the app.
        let title_el: gpui::AnyElement = if let Some(input) = rename_input {
            div()
                .flex_1()
                .min_w_0()
                .bg(ui.overlay)
                .px_1()
                .rounded_sm()
                .child(input)
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .text_color(ui.text)
                .text_size(px(12.))
                .font_weight(FontWeight::NORMAL)
                .truncate()
                .child(title)
                .into_any_element()
        };

        div()
            .id(SharedString::from(format!("agents-project-{project_id}")))
            .mx(px(AGENTS_SIDEBAR_ROW_MARGIN_X))
            .px(px(8.))
            .py(px(6.))
            .rounded(crate::app::constants::SIDEBAR_TAB_CORNER_RADIUS)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .bg(resting_background)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(resting_background, hover_background, delta));
            })
            .on_click(cx.listener(move |this, e: &ClickEvent, w, cx| {
                this.close_agents_menu(cx);
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    // PRD AC #3: double-click -> inline rename mode.
                    this.begin_agents_rename(AgentsRenameTarget::Project { project_idx }, w, cx);
                } else {
                    // Single click toggles collapse. If a rename is in
                    // progress on this row, commit it first so the
                    // toggle isn't blocked.
                    this.commit_agents_rename(cx);
                    if let Some(project) = this.projects.get_mut(project_idx) {
                        project.is_expanded = !project.is_expanded;
                        cx.notify();
                    }
                }
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _w, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_agents_rename(cx);
                    this.open_agents_project_menu(project_idx, position, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path(folder_path)
                    .text_color(ui.muted),
            )
            .child(title_el)
            .when(!git_stats.is_empty(), |d| {
                // Trailing `+N -N` badge - git diff --shortstat of the
                // project's cwd, cached + refreshed on the 30 s poller
                // in `app/bootstrap.rs`. Shared diff palette (Codex green/red on
                // dark, theme vc_* on light) so it matches the diff dock, the
                // Diff/Review view, and the CLI sidebar diffstat.
                let diff = ui.diff_colors();
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.))
                        .text_size(px(10.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(
                            div()
                                .text_color(diff.added)
                                .child(format!("+{}", git_stats.insertions)),
                        )
                        .child(
                            div()
                                .text_color(diff.deleted)
                                .child(format!("-{}", git_stats.deletions)),
                        ),
                )
            })
            .into_any_element()
    }

    fn thread_row(&self, args: ThreadRowArgs, cx: &mut Context<Self>) -> gpui::AnyElement {
        let ThreadRowArgs {
            target,
            thread_id,
            title,
            rename_input,
            created_at_ms,
            is_active,
            is_pinned,
            row_scope,
            now_ms,
            match_pos,
            ui,
            status,
        } = args;
        let timestamp = format_relative_ts(now_ms, created_at_ms);
        // Codex-style activity indicator: while the agent's turn is in
        // flight the relative timestamp gives way to a rotating arc -
        // the SAME muted `loader-circle.svg` for every agent (Arthur:
        // no per-agent branding here, match the Codex app's sidebar) -
        // or a static compact dot for attention/error. Idle restores the
        // timestamp. The arc self-animates via the declarative
        // Animation+Transformation API (same pattern as the title-bar update
        // pill), so no shared loader-loop state is involved.
        let right_slot: gpui::AnyElement = match status {
            crate::project::ThreadStatus::Thinking => svg()
                .size(px(11.))
                .flex_none()
                .path("icons/loader-circle.svg")
                .text_color(ui.muted)
                .with_animation(
                    SharedString::from(format!("agents-{row_scope}-spinner-{thread_id}")),
                    Animation::new(std::time::Duration::from_secs(1)).repeat(),
                    |svg, delta| svg.with_transformation(Transformation::rotate(percentage(delta))),
                )
                .into_any_element(),
            crate::project::ThreadStatus::WaitingForInput => div()
                .text_color(rgb(0xFBBF24))
                .child("●")
                .into_any_element(),
            crate::project::ThreadStatus::Failed => div()
                .text_color(ui.agent_error)
                .child("●")
                .into_any_element(),
            crate::project::ThreadStatus::Idle => div().child(timestamp).into_any_element(),
        };
        let title_color = ui.text;
        let title_weight = FontWeight::NORMAL;
        let is_renaming = rename_input.is_some();
        // Inline delete-confirm: this row is "armed" (shows a red Delete button
        // in place of the trash icon) when its target matches the armed slot.
        let armed = self.agents_view.agents_delete_armed == Some(target);

        let title_el: gpui::AnyElement = if let Some(input) = rename_input {
            // Inline rename input -- full TextArea entity (same
            // widget the composer uses) so the user gets real cursor,
            // selection, IME, copy/paste, click-to-position, double-
            // click word select. Background pill matches the project
            // header rename styling.
            div()
                .flex_1()
                .min_w_0()
                .bg(ui.overlay)
                .px_1()
                .rounded_sm()
                .child(input)
                .into_any_element()
        } else if let Some((m_start, m_end)) = match_pos {
            // US-021: paint a single highlight run on the matched
            // substring (Zed `highlight_positions` slot on ThreadItem).
            // Surrounding text keeps `title_color`; the match gets
            // `ui.accent` over a tinted `ui.subtle` background.
            let runs =
                build_title_highlight_runs(&title, m_start, m_end, title_color, ui, title_weight);
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .overflow_hidden()
                .child(StyledText::new(SharedString::from(title)).with_runs(runs))
                .into_any_element()
        } else {
            div()
                .flex_1()
                .min_w_0()
                .text_color(title_color)
                .text_size(px(12.))
                .font_weight(title_weight)
                .truncate()
                .child(title)
                .into_any_element()
        };

        // Match the project_header_row's layout exactly: same `mx`,
        // same `px` (instead of split `pl`/`pr`), same `gap`. The only
        // difference is the leading child -- project rows render a
        // folder icon, thread rows render an invisible 14px spacer
        // (mirrors Zed's `ThreadItem` pattern at
        // `crates/ui/src/components/ai/thread_item.rs:264-270` where
        // the icon container is always rendered with `.size_4()` and
        // toggled to `.invisible()` to keep layout space). Padding
        // numbers were identical before the icon was removed but the
        // titles read as shifted because removing the child also
        // removed the `gap` slot. Reserving an invisible placeholder
        // restores pixel-for-pixel alignment with the project title.
        let row_background = crate::app::constants::sidebar_tab_active_background();
        let resting_background = if is_active {
            row_background
        } else {
            row_background.opacity(0.0)
        };
        let hover_actions =
            hover_actions_cluster(target, thread_id, is_pinned, row_scope, armed, ui, cx);
        let row = div()
            .id(SharedString::from(format!(
                "agents-{row_scope}-thread-{thread_id}"
            )))
            .relative()
            .mx(px(AGENTS_SIDEBAR_ROW_MARGIN_X))
            .px(px(8.))
            .py(px(6.))
            .rounded(crate::app::constants::SIDEBAR_TAB_CORNER_RADIUS)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            // Codex-style selection: a neutral, slightly translucent light-gray
            // overlay against the rail's cockpit color. Selection remains
            // immediate; only the inactive row's pointer hover interpolates.
            .bg(resting_background)
            .animated_hover_element(move |row, delta| {
                row.style()
                    .bg(lerp_color(resting_background, row_background, delta));

                let right_slot_opacity = if armed { 0.0 } else { 1.0 - delta };
                let right_slot = div()
                    // Reserve a fixed 48px so the title always truncates
                    // before the absolute hover-action cluster.
                    .flex_none()
                    .w(px(48.))
                    .flex()
                    .justify_end()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(
                        div()
                            .opacity(right_slot_opacity)
                            .when(right_slot_opacity <= f32::EPSILON, |slot| slot.hidden())
                            .child(right_slot),
                    )
                    .into_any_element();

                let actions_opacity = if armed { 1.0 } else { delta };
                let hover_actions = hover_actions
                    .opacity(actions_opacity)
                    .when(actions_opacity <= f32::EPSILON, |actions| actions.hidden())
                    .into_any_element();
                row.extend([right_slot, hover_actions]);
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.close_agents_menu(cx);
                // While the row is in rename mode we let the click
                // pass through to the embedded TextArea (which then
                // positions the caret / extends the selection). The
                // row's own selection is skipped so an in-place mouse
                // click on the input doesn't navigate away.
                if is_renaming {
                    return;
                }
                this.commit_agents_rename(cx);
                // US-006/008: a Pinned row routes to its original source;
                // a chat row selects the chat. Unified dispatch.
                this.select_agents_target(target, cx);
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _w, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_agents_rename(cx);
                    this.open_agents_menu_for_target(target, position, cx);
                    cx.stop_propagation();
                }
            }))
            // Invisible 14px placeholder where the project row's
            // folder icon sits. Keeps the title's X-position aligned
            // with the project title's X-position one row up. Mirrors
            // Zed's `icon_container().when(!icon_visible, |this| this.invisible())`.
            .child(div().size(px(14.)).flex_none())
            .child(title_el);

        row.into_any_element()
    }

    /// Filter row wraps the US-012 search input in a flex container so
    /// future trailing affordances can sit on the same row at any
    /// sidebar width without re-plumbing the outer layout.
    pub(crate) fn render_agents_filter_row(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .child(
                div()
                    .flex_1()
                    .child(self.render_agents_filter_input(ui, cx)),
            )
            .into_any_element()
    }

    /// Render the sidebar search/filter input (US-012). Uses the same
    /// inline-cursor pattern as the workspace `font_search`: the text
    /// is rendered as plain string + `|` suffix when the input has
    /// focus, and a placeholder when empty.
    pub(crate) fn render_agents_filter_input(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Real single-line text input: cursor, arrow keys, Delete, Ctrl+A/C/V/X,
        // mouse selection / click-to-position all come from `TextInput` and its
        // registered keybindings. The needle is read from `value()` at render.
        let is_empty = self
            .agents_view
            .agents_filter_input
            .read(cx)
            .value()
            .is_empty();

        // Mirrors the Settings "Search settings…" pill (`settings/chrome.rs`):
        // a filled `ui.subtle` gray, borderless, and fully inert - nothing
        // changes on focus or hover, the blinking caret is the only focus cue -
        // so both app search fields read as one system.
        // Mirrors the Settings "Search settings…" pill via the shared
        // [`filter_pill`] primitive (same recipe as the diff sidebar filter).
        // Escape clears the query; Enter jumps to the first matching thread;
        // clicking outside drops focus. Cursor movement / Delete / Ctrl+A,C,V,X /
        // mouse selection are all handled inside the focused TextInput via its
        // own keybindings; the unbound Escape/Enter bubble up to this container.
        crate::ui_primitives::filter_pill(
            "agents-sidebar-filter",
            "agents-sidebar-filter-clear",
            ui,
            self.agents_view.agents_filter_input.clone(),
            !is_empty,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.agents_view.agents_filter_input.update(cx, |inp, cx| {
                    inp.clear(cx);
                });
            }),
        )
        .on_key_down(cx.listener(
            |this, ev: &KeyDownEvent, _w, cx| match ev.keystroke.key.as_str() {
                "escape" => {
                    this.agents_view.agents_filter_input.update(cx, |inp, cx| {
                        inp.clear(cx);
                    });
                    cx.stop_propagation();
                }
                "enter" => {
                    let q = this
                        .agents_view
                        .agents_filter_input
                        .read(cx)
                        .value()
                        .to_lowercase();
                    if let Some(target) =
                        filter::first_matching_target(&this.projects, &this.chats, &q)
                    {
                        this.select_agents_target(target, cx);
                    }
                    cx.stop_propagation();
                }
                _ => {}
            },
        ))
        .on_mouse_down_out(cx.listener(|this, _, window, cx| {
            if this
                .agents_view
                .agents_filter_input
                .read(cx)
                .focus_handle
                .is_focused(window)
            {
                window.blur();
                cx.notify();
            }
        }))
        .into_any_element()
    }
}

/// Argument carrier for [`PaneFlowApp::project_header_row`]. Lets the
/// fn signature stay under clippy's `too_many_arguments` threshold
/// while keeping the per-row state explicit at the call site.
struct ProjectHeaderArgs {
    project_idx: usize,
    project_id: u64,
    title: String,
    is_expanded: bool,
    /// `Some` when this row is the current inline-rename target; the
    /// entity is rendered in place of the static title and owns its
    /// own keyboard / mouse handling (cursor, selection, IME, ...).
    rename_input: Option<gpui::Entity<crate::widgets::text_area::TextArea>>,
    git_stats: crate::workspace::GitDiffStats,
    ui: crate::theme::UiColors,
}

struct ThreadRowArgs {
    /// US-006/008: the unified selection target this row drives (a project
    /// thread or a free chat). Replaces the old positional pair so the same
    /// widget serves the PINNED, PROJECTS and CHATS sections.
    target: crate::project::AgentsTarget,
    thread_id: u64,
    title: String,
    /// `Some` when this row is the current inline-rename target; the
    /// entity is rendered in place of the static title and owns its
    /// own keyboard / mouse handling (cursor, selection, IME, ...).
    rename_input: Option<gpui::Entity<crate::widgets::text_area::TextArea>>,
    created_at_ms: u64,
    is_active: bool,
    /// US-006: whether this thread/chat is pinned (drives the hover ★/☆
    /// toggle glyph).
    is_pinned: bool,
    /// US-006: section discriminant (`"pinned"` / `"project"` / `"chat"`)
    /// woven into the element ids so a pinned thread rendered twice (once in
    /// PINNED, once in its project) never collides on a GPUI id.
    row_scope: &'static str,
    now_ms: u64,
    /// US-021: byte-range of the current filter query inside `title`,
    /// or `None` when no filter is active or the query does not hit
    /// this row's title. The renderer uses this to paint the matched
    /// substring with a tinted background -- mirrors Zed's
    /// `highlight_positions` slot on `ThreadItem`.
    match_pos: Option<(usize, usize)>,
    ui: crate::theme::UiColors,
    /// Live agent-turn state (driven by the `ai.*` IPC hooks). While the
    /// turn is in flight the row's relative timestamp gives way to a
    /// spinner / attention dot, Codex-app style.
    status: crate::project::ThreadStatus,
}

/// Per-render state shared by every thread/chat row, captured once in
/// [`PaneFlowApp::render_agents_sidebar`] and threaded into
/// [`PaneFlowApp::agents_thread_row_for`] so the three sections build
/// identical rows without re-reading `self` per row.
struct RowSharedState {
    agents_target: Option<crate::project::AgentsTarget>,
    renaming: Option<AgentsRenameTarget>,
    rename_input: Option<gpui::Entity<crate::widgets::text_area::TextArea>>,
    now_ms: u64,
    filtering: bool,
    ui: crate::theme::UiColors,
}

/// Does the active inline-rename target point at `target`'s row? Maps the
/// rename enum (which still discriminates project rows from chat rows) onto
/// the unified [`crate::project::AgentsTarget`].
fn is_renaming_target(
    renaming: Option<AgentsRenameTarget>,
    target: crate::project::AgentsTarget,
) -> bool {
    use crate::project::AgentsTarget;
    matches!(
        (renaming, target),
        (
            Some(AgentsRenameTarget::Thread {
                project_idx: rp,
                thread_idx: rt,
            }),
            AgentsTarget::Thread {
                project_idx,
                thread_idx,
            },
        ) if rp == project_idx && rt == thread_idx
    ) || matches!(
        (renaming, target),
        (
            Some(AgentsRenameTarget::Chat { chat_idx: rc }),
            AgentsTarget::Chat { chat_idx },
        ) if rc == chat_idx
    )
}

/// US-004: section eyebrow - a small uppercase muted label introducing a
/// rail section (PINNED / PROJECTS / CHATS). When `add_button_id` is
/// `Some`, a trailing `+` opens the folder picker (the PROJECTS section's
/// create affordance, US-007). The caller passes an already-uppercased
/// label so this stays a pure layout helper.
fn section_eyebrow(
    label: &str,
    add_button_id: Option<SharedString>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::AnyElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .mt(px(8.))
        .pl(px(16.))
        .pr(px(8.))
        .py(px(2.))
        .child(
            crate::ui_primitives::section_eyebrow(label.to_string(), ui)
                .flex_1()
                .min_w_0()
                .truncate(),
        );
    if let Some(id) = add_button_id {
        let hover_background = crate::app::constants::sidebar_tab_active_background();
        let resting_background = hover_background.opacity(0.0);
        row = row.child(
            div()
                .id(id)
                .role(Role::Button)
                .aria_label("New project")
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .bg(resting_background)
                .text_color(ui.muted)
                .animated_hover_element(move |button, delta| {
                    let icon_color = lerp_color(ui.muted, ui.text, delta);
                    button
                        .style()
                        .bg(lerp_color(resting_background, hover_background, delta))
                        .text_color(icon_color);
                    button.extend([svg()
                        .size(px(12.))
                        .flex_none()
                        .path("icons/plus.svg")
                        .text_color(icon_color)
                        .into_any_element()]);
                })
                .tooltip(crate::ui_primitives::text_tooltip("New project"))
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.create_agents_project_with_picker(cx);
                })),
        );
    }
    row.into_any_element()
}

/// US-007: compact inline hint shown under the PROJECTS eyebrow when no
/// project exists yet. The eyebrow's `+` (or the header's New chat action)
/// is the create affordance; this is just guidance copy.
fn projects_empty_hint(ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .mx(px(AGENTS_SIDEBAR_ROW_MARGIN_X))
        .px(px(8.))
        .py(px(6.))
        .text_size(px(11.))
        .text_color(ui.muted)
        .child("No projects yet. Click + to add one.")
}

/// Inline hint shown directly under an expanded project header that
/// has zero threads. Right-click the project (or use "New thread") to
/// open the agent picker.
fn empty_project_hint(ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .mx(px(AGENTS_SIDEBAR_ROW_MARGIN_X))
        .pl(px(28.))
        .py(px(4.))
        .text_size(px(11.))
        .text_color(ui.muted)
        .child("No threads yet. Right-click to start one.")
}

/// US-012 AC #7: inline empty-state row shown when the filter
/// matches nothing. Backticks around the query mirror the PRD copy
/// verbatim: "No threads match `<query>`. Press Esc to clear."
fn no_matches_hint(query: &str, ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .mx(px(AGENTS_SIDEBAR_ROW_MARGIN_X))
        .my(px(8.))
        .px(px(8.))
        .py(px(10.))
        .rounded(px(6.))
        .bg(ui.subtle)
        .text_size(px(11.))
        .text_color(ui.muted)
        .child(format!("No threads match `{query}`. Press Esc to clear."))
}

/// Compact relative timestamp matching the PRD AC ("2m", "1h", "3d").
/// Inputs are Unix milliseconds; clock skew (now < created) is
/// clamped to `now` -> "now" so a sidebar restored from a session
/// saved on a different time zone does not show negative deltas.
fn format_relative_ts(now_ms: u64, created_at_ms: u64) -> String {
    let delta_ms = now_ms.saturating_sub(created_at_ms);
    let secs = delta_ms / 1000;
    if secs < 5 {
        return "now".to_string();
    }
    if secs < 60 {
        return format!("{secs}s");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{days}d");
    }
    let weeks = days / 7;
    if weeks < 5 {
        return format!("{weeks}w");
    }
    // Fallback for ancient threads -- a single quantum rather than a
    // calendar date keeps the row width predictable.
    let months = days / 30;
    format!("{months}mo")
}

/// Suppress an unused-import warning when none of the rotation-based
/// chevron animations is referenced. Keeps `Transformation` /
/// `percentage` available for follow-up stories without re-importing.
#[allow(dead_code)]
fn _keep_rotation_imports(t: Transformation) -> Transformation {
    let _ = percentage(0.0);
    t
}

fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// US-021: build per-run text styling for the thread title when a
/// search filter is active. Returns three runs: `[before, match,
/// after]`. The matched span uses `ui.accent` over a `ui.subtle`
/// background tint; surrounding text keeps `base_color`. When the
/// match covers the entire title the before/after runs have length
/// 0 and the shaper skips them safely.
fn build_title_highlight_runs(
    title: &str,
    m_start: usize,
    m_end: usize,
    base_color: Hsla,
    ui: crate::theme::UiColors,
    weight: FontWeight,
) -> Vec<TextRun> {
    let font = Font {
        family: ".SystemUIFont".into(),
        features: FontFeatures::default(),
        fallbacks: None,
        weight,
        style: FontStyle::Normal,
    };
    let make = |len: usize, color: Hsla, bg: Option<Hsla>| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: bg,
        underline: None,
        strikethrough: None,
    };
    vec![
        make(m_start, base_color, None),
        make(m_end - m_start, ui.accent, Some(ui.subtle)),
        make(title.len().saturating_sub(m_end), base_color, None),
    ]
}

/// US-023: small h_flex cluster of per-row action buttons (currently
/// just Delete). Its parent row applies the shared hover progress to this
/// cluster, preserving a reversible crossfade and zero layout shift.
// Render helper: every input (target/id/pin state/scope/armed + theme) is
// genuinely needed per row; bundling into a struct would only move the noise.
#[allow(clippy::too_many_arguments)]
fn hover_actions_cluster(
    target: crate::project::AgentsTarget,
    thread_id: u64,
    is_pinned: bool,
    row_scope: &'static str,
    armed: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> gpui::Div {
    // Inline delete-confirm (ergonomics): once the trash is clicked the row
    // arms - show a single red "Delete" button, always visible (the cursor has
    // left the trash icon), and run the delete on the next click. Clicking
    // elsewhere (selecting a row / opening a menu / arming another) cancels it.
    if armed {
        let resting_background: Hsla = rgb(0xe5484d).into();
        let hover_background: Hsla = rgb(0xc73d41).into();
        return div()
            .absolute()
            .top(px(0.))
            .bottom(px(0.))
            .right(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "agents-{row_scope}-thread-{thread_id}-confirm-delete"
                    )))
                    .flex_none()
                    .h(px(20.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .bg(resting_background)
                    .text_color(rgb(0xffffff))
                    .text_size(px(11.))
                    .font_weight(FontWeight::MEDIUM)
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(resting_background, hover_background, delta));
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        this.execute_armed_delete(cx);
                        cx.stop_propagation();
                    }))
                    .child("Delete"),
            );
    }

    // US-006: pin / unpin toggle. A text glyph (★ filled = pinned, ☆ outline
    // = unpinned) instead of an SVG - Paneflow ships no pin asset and the
    // glyph reads correctly at this size. Toggling persists via
    // `toggle_pin_for_target` (flips `thread.pinned` + saves the session).
    let pin_glyph = if is_pinned { "★" } else { "☆" };
    let pin_tooltip = if is_pinned { "Unpin" } else { "Pin" };
    let hover_background = crate::app::constants::sidebar_tab_active_background();
    let resting_background = hover_background.opacity(0.0);
    let pin_resting_text = if is_pinned { ui.accent } else { ui.muted };
    let pin_btn = div()
        .id(SharedString::from(format!(
            "agents-{row_scope}-thread-{thread_id}-pin"
        )))
        .role(Role::Button)
        .aria_label(pin_tooltip)
        .flex_none()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        // Keep the glyph optically aligned with the neighboring 12px trash
        // icon inside the same 20px action box.
        .text_size(px(14.))
        .bg(resting_background)
        .text_color(pin_resting_text)
        .animated_hover(move |style, delta| {
            style
                .bg(lerp_color(resting_background, hover_background, delta))
                .text_color(lerp_color(pin_resting_text, ui.text, delta));
        })
        .tooltip(crate::ui_primitives::text_tooltip(pin_tooltip))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_pin_for_target(target, cx);
            cx.stop_propagation();
        }))
        .child(pin_glyph);

    let trash_btn = div()
        .id(SharedString::from(format!(
            "agents-{row_scope}-thread-{thread_id}-trash"
        )))
        .role(Role::Button)
        .aria_label("Delete")
        .flex_none()
        .w(px(20.))
        .h(px(20.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .bg(resting_background)
        .text_color(ui.muted)
        .animated_hover(move |style, delta| {
            style
                .bg(lerp_color(resting_background, hover_background, delta))
                .text_color(lerp_color(ui.muted, ui.text, delta));
        })
        .tooltip(crate::ui_primitives::text_tooltip("Delete"))
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            // First click arms the inline delete-confirm (no dialog): the row's
            // cluster flips to a red "Delete" button (see the `armed` branch).
            this.arm_delete_for_target(target, cx);
            cx.stop_propagation();
        }))
        .child(svg().size(px(12.)).flex_none().path("icons/trash.svg"));

    div()
        .absolute()
        .top(px(0.))
        .bottom(px(0.))
        .right(px(8.))
        .flex()
        .flex_row()
        .items_center()
        // Zed `gap_1` = 4px (4px grid base unit). US-023 AC #2.
        .gap(px(4.))
        .child(pin_btn)
        .child(trash_btn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_ts_buckets() {
        // Anchor: 2026-05-23T12:00:00Z in millis.
        let now: u64 = 1_777_022_400_000;
        assert_eq!(format_relative_ts(now, now), "now");
        assert_eq!(format_relative_ts(now, now - 3_000), "now");
        assert_eq!(format_relative_ts(now, now - 6_000), "6s");
        assert_eq!(format_relative_ts(now, now - 59_000), "59s");
        assert_eq!(format_relative_ts(now, now - 120_000), "2m");
        assert_eq!(format_relative_ts(now, now - 3_600_000), "1h");
        assert_eq!(format_relative_ts(now, now - 26 * 3_600_000), "1d");
        assert_eq!(format_relative_ts(now, now - 8 * 24 * 3_600_000), "1w");
        // Clock skew: created in the future clamps to "now".
        assert_eq!(format_relative_ts(now, now + 60_000), "now");
    }

    #[test]
    fn is_renaming_target_maps_rename_enum_to_unified_target() {
        use crate::project::AgentsTarget;
        let p_target = AgentsTarget::Thread {
            project_idx: 1,
            thread_idx: 2,
        };
        let c_target = AgentsTarget::Chat { chat_idx: 3 };

        // A project-thread rename matches only its exact thread row.
        let renaming_thread = Some(AgentsRenameTarget::Thread {
            project_idx: 1,
            thread_idx: 2,
        });
        assert!(is_renaming_target(renaming_thread, p_target));
        assert!(!is_renaming_target(renaming_thread, c_target));
        assert!(!is_renaming_target(
            renaming_thread,
            AgentsTarget::Thread {
                project_idx: 1,
                thread_idx: 5
            }
        ));

        // A chat rename matches only its exact chat row.
        let renaming_chat = Some(AgentsRenameTarget::Chat { chat_idx: 3 });
        assert!(is_renaming_target(renaming_chat, c_target));
        assert!(!is_renaming_target(renaming_chat, p_target));
        assert!(!is_renaming_target(
            renaming_chat,
            AgentsTarget::Chat { chat_idx: 9 }
        ));

        // A project rename never matches a thread/chat row (it targets the
        // header, not a row).
        let renaming_project = Some(AgentsRenameTarget::Project { project_idx: 1 });
        assert!(!is_renaming_target(renaming_project, p_target));
        assert!(!is_renaming_target(renaming_project, c_target));

        // No active rename -> nothing matches.
        assert!(!is_renaming_target(None, p_target));
    }
}
