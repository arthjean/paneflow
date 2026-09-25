pub(crate) mod context_menu;
pub(crate) mod customize_menu;
mod drop;
pub(crate) mod keyboard;
mod lane;
mod meta;
mod pull_request;
mod session_rows;
mod style;
mod tab_row;
mod workspace_row;

use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    AnyElement, AppContext, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Role, SharedString, Styled, Window, div, prelude::*,
    px, svg,
};

use crate::ui_primitives::squircle_skin;
use drop::*;
use lane::{Lane, infer_lane, render_lane_slot};
use meta::*;
pub(crate) use style::*;
pub(crate) use tab_row::*;

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, TabContextMenu, TabDrag, WorkspaceContextMenu, WorkspaceDrag,
    WorkspaceDragPreview, ai_types,
    ai_types::AgentState,
    app::host_agents::HostAgentRow,
    app::hosted_sessions::{OwnedSession, SessionRowScope, lifecycle_sentence, relative_age},
    app::pull_request::PullRequest,
    pane_drag::PaneDrag,
    settings::components::with_alpha,
    workspace::{Tab, Workspace},
};

const SIDEBAR_EMPTY_STATE_WIDTH: f32 = 192.0;

fn sidebar_empty_state_button(
    id: &'static str,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    crate::settings::components::select_item(SharedString::from(id), false, ui)
        .w_full()
        .justify_center()
}

#[derive(Default)]
pub(crate) struct SidebarOrderCache {
    signature: Option<u64>,
    order: Vec<usize>,
}

struct SidebarRenderTimeCanary {
    start: std::time::Instant,
    workspace_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum SidebarRow {
    Folder(usize),
    Tab(usize, usize),
}

#[derive(Default)]
pub(crate) struct SidebarFilterMotion {
    topology: Vec<(u64, Vec<u64>)>,
    query: String,
    initialized: bool,
    rows: Vec<(SidebarRow, f32)>,
    from: Vec<(SidebarRow, f32)>,
    target: Vec<SidebarRow>,
    started: Option<std::time::Instant>,
    heights: std::collections::HashMap<SidebarRow, f32>,
}

impl SidebarFilterMotion {
    fn sample(&mut self, now: std::time::Instant) {
        let Some(started) = self.started else { return };
        let progress = (now.duration_since(started).as_secs_f32() / 0.18).min(1.);
        let eased = 1. - (1. - progress).powi(3);
        for (row, amount) in &mut self.rows {
            let from = self
                .from
                .iter()
                .find(|(key, _)| key == row)
                .map_or(0., |(_, value)| *value);
            let to = if self.target.contains(row) { 1. } else { 0. };
            *amount = from + (to - from) * eased;
        }
        if progress >= 1. {
            self.rows.retain(|(_, amount)| *amount > 0.);
            self.started = None;
        }
    }

    fn update(
        &mut self,
        topology: Vec<(u64, Vec<u64>)>,
        query: &str,
        target: &[SidebarRow],
        order: &[SidebarRow],
        now: std::time::Instant,
    ) {
        self.sample(now);
        if !self.initialized
            || self.topology != topology
            || crate::ui_primitives::reduce_motion()
            || (self.query == query && self.target != target)
        {
            if self.topology != topology {
                self.heights.clear();
            }
            self.rows = target.iter().map(|row| (*row, 1.)).collect();
            self.started = None;
        } else if self.query != query {
            self.from = self.rows.clone();
            self.rows = order
                .iter()
                .filter_map(|row| {
                    let amount = self
                        .from
                        .iter()
                        .find(|(key, _)| key == row)
                        .map_or(0., |(_, value)| *value);
                    (amount > 0. || target.contains(row)).then_some((*row, amount))
                })
                .collect();
            self.started = Some(now);
        }
        self.initialized = true;
        self.topology = topology;
        self.query = query.to_string();
        self.target = target.to_vec();
    }
}

impl SidebarRenderTimeCanary {
    fn new(workspace_count: usize) -> Self {
        Self {
            start: std::time::Instant::now(),
            workspace_count,
        }
    }
}

impl Drop for SidebarRenderTimeCanary {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > std::time::Duration::from_millis(16) {
            tracing::debug!(
                target: "paneflow_app::sidebar",
                "render_sidebar exceeded 16ms frame budget: {:.2}ms across {} workspaces",
                elapsed.as_secs_f64() * 1000.0,
                self.workspace_count
            );
        }
    }
}

impl PaneFlowApp {
    fn sidebar_filter_label(&self, label: String, cx: &gpui::App) -> gpui::StyledText {
        let query = self
            .sidebar_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        crate::ui_primitives::highlight_matches(label, &query)
    }

    fn sidebar_order_signature(workspaces: &[Workspace]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        workspaces.len().hash(&mut hasher);
        for workspace in workspaces {
            workspace.id.hash(&mut hasher);
            match &workspace.repo_root {
                Some(root) => root.hash(&mut hasher),
                None => 0u8.hash(&mut hasher),
            }
        }
        hasher.finish()
    }

    fn compute_display_order(workspaces: &[Workspace]) -> Vec<usize> {
        let mut repo_members: std::collections::HashMap<&std::path::Path, Vec<usize>> =
            std::collections::HashMap::new();
        for (index, workspace) in workspaces.iter().enumerate() {
            if let Some(root) = &workspace.repo_root {
                repo_members.entry(root.as_path()).or_default().push(index);
            }
        }

        let mut order = Vec::with_capacity(workspaces.len());
        let mut placed = vec![false; workspaces.len()];
        for (index, workspace) in workspaces.iter().enumerate() {
            if placed[index] {
                continue;
            }
            if let Some(root) = &workspace.repo_root
                && let Some(members) = repo_members.get(root.as_path())
                && members.len() >= 2
            {
                for &member in members {
                    order.push(member);
                    placed[member] = true;
                }
                continue;
            }
            order.push(index);
            placed[index] = true;
        }
        order
    }

    pub(crate) fn render_sidebar(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let _render_canary = SidebarRenderTimeCanary::new(self.workspaces.len());
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let mut sidebar = div()
            .font_family(".SystemUIFont")
            .text_size(px(13.))
            .font_weight(FontWeight::NORMAL)
            .relative()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .flex()
            .flex_col();

        let new_workspace_tooltip = self.shortcut_for_action("new_workspace").map_or_else(
            || "New workspace".to_string(),
            |key| format!("New workspace  {key}"),
        );
        sidebar = sidebar.child(
            div()
                .h(px(36.))
                .flex_none()
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .pl(px(SIDEBAR_ROW_PADDING_X))
                        .text_size(px(13.))
                        .text_color(ui.muted)
                        .child("Workspaces"),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(SIDEBAR_ACTION_BUTTON_GAP))
                        .child(customize_menu::render_customize_sidebar_button(
                            customize_menu::CustomizeMenuState {
                                open: self.sidebar_customize_menu_open,
                                submenu_open: self.sidebar_show_submenu_open,
                                show: self.cached_config.sidebar_show,
                                all_expanded: self.all_workspaces_expanded(),
                            },
                            SIDEBAR_HEADER_ICON_WIDTH,
                            ui,
                            cx,
                        ))
                        .child({
                            let hover_bg = crate::app::constants::sidebar_tab_hover_background();
                            squircle_skin(
                                div()
                                    .id("sidebar-new-workspace")
                                    .size(px(22.))
                                    .flex()
                                    .items_center()
                                    .justify_center(),
                                "sidebar-new-workspace-group",
                                px(7.),
                                None,
                                Some(hover_bg),
                            )
                            .role(Role::Button)
                            .aria_label("New workspace")
                            .delayed_tooltip(crate::ui_primitives::text_tooltip(
                                new_workspace_tooltip,
                            ))
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.create_workspace_with_picker(window, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(SIDEBAR_HEADER_ICON_WIDTH))
                                    .flex_none()
                                    .path("icons/folder-plus.svg")
                                    .text_color(ui.muted)
                                    .group_hover("sidebar-new-workspace-group", move |style| {
                                        style.text_color(ui.text)
                                    }),
                            )
                        }),
                ),
        );

        if let Some(banner) = self.render_worker_banner(ui, cx) {
            sidebar = sidebar.child(banner);
        }

        let mut list = div()
            .id("workspace-list")
            .role(Role::Tree)
            .aria_label("Workspaces")
            .track_focus(&self.sidebar_focus)
            .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
            .key_context("WorkspacesSidebar")
            .on_key_down(cx.listener(Self::handle_sidebar_key_down))
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            .pb(px(4.));

        if self.workspaces.is_empty() {
            list = list.child(self.render_sidebar_empty_state(ui, cx));
        }

        list = self.render_workspace_rows(list, ui, window, cx);
        list = self.render_fallback_session_group(list, ui, cx);
        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(window, cx));
        sidebar
    }

    fn render_worker_banner(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let message = crate::worker_bootstrap::banner(&crate::worker_bootstrap::state())?;
        let label = SharedString::from(message.clone());
        let mut banner = div()
            .flex_none()
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .mb(px(4.))
            .px(px(SIDEBAR_ROW_PADDING_X))
            .py(px(SIDEBAR_ROW_PADDING_Y))
            .rounded(ROW_RADIUS)
            .bg(with_alpha(ui.agent_error, 0.08))
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(ui.agent_error)
                    .child(label),
            );
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        banner = banner.child(
            squircle_skin(
                div()
                    .id("worker-retry")
                    .self_start()
                    .px(px(6.))
                    .py(px(2.))
                    .flex()
                    .items_center()
                    .justify_center(),
                "worker-retry-group",
                px(6.),
                None,
                Some(hover_bg),
            )
            .cursor_pointer()
            .on_click(cx.listener(|_this, _: &ClickEvent, _window, cx| {
                crate::worker_bootstrap::retry();
                cx.notify();
            }))
            .child(div().text_size(px(11.)).text_color(ui.muted).child("Retry")),
        );
        Some(banner.into_any_element())
    }

    fn render_sidebar_empty_state(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let open_key = self
            .shortcut_for_action("new_workspace")
            .map(str::to_string);
        let divider = || div().flex_1().h(px(1.)).bg(ui.border);

        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(16.))
            .child(
                div()
                    .w(px(SIDEBAR_EMPTY_STATE_WIDTH))
                    .max_w_full()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .pb(px(8.))
                            .text_center()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Open a folder to start a workspace"),
                    )
                    .child(
                        sidebar_empty_state_button("sidebar-empty-open-folder", ui)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.create_workspace_with_picker(window, cx);
                            }))
                            .child(div().text_color(ui.text).child("Open folder"))
                            .when_some(open_key, |button, key| {
                                button.child(
                                    div()
                                        .flex_none()
                                        .text_size(px(11.))
                                        .text_color(ui.muted)
                                        .child(key),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.))
                            .py(px(2.))
                            .child(divider())
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(ui.muted)
                                    .child("or"),
                            )
                            .child(divider()),
                    )
                    .child(
                        sidebar_empty_state_button("sidebar-empty-clone-repo", ui)
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_clone_repo(window, cx);
                            }))
                            .child(div().text_color(ui.text).child("Clone repository")),
                    ),
            )
    }

    fn sidebar_rows(&self, query: &str) -> Vec<SidebarRow> {
        let signature = Self::sidebar_order_signature(&self.workspaces);
        if self.sidebar_order_cache.borrow().signature != Some(signature) {
            let order = Self::compute_display_order(&self.workspaces);
            let mut cache = self.sidebar_order_cache.borrow_mut();
            cache.order = order;
            cache.signature = Some(signature);
        }
        let order_cache = self.sidebar_order_cache.borrow();
        let mut rows = Vec::with_capacity(order_cache.order.len());
        for &i in &order_cache.order {
            let ws = &self.workspaces[i];
            let workspace_matches = query.is_empty()
                || [&ws.title, &ws.cwd, &ws.git_branch]
                    .iter()
                    .any(|value| value.to_lowercase().contains(query));
            let tabs: Vec<_> = ws
                .tabs()
                .iter()
                .enumerate()
                .filter(|(index, tab)| {
                    workspace_matches
                        || tab_display_title(tab, *index)
                            .to_lowercase()
                            .contains(query)
                })
                .map(|(index, _)| index)
                .collect();
            if !workspace_matches && tabs.is_empty() {
                continue;
            }
            rows.push(SidebarRow::Folder(i));
            if (ws.sidebar_expanded || !query.is_empty()) && !ws.is_empty_shell() {
                for tab_idx in tabs {
                    rows.push(SidebarRow::Tab(i, tab_idx));
                }
            }
        }
        rows
    }

    fn all_workspaces_expanded(&self) -> Option<bool> {
        let mut rows_somewhere = false;
        let mut all = true;
        for ws in &self.workspaces {
            if ws.is_empty_shell() {
                continue;
            }
            rows_somewhere = true;
            all &= ws.sidebar_expanded;
        }
        rows_somewhere.then_some(all)
    }

    fn render_workspace_rows(
        &self,
        mut list: gpui::Stateful<gpui::Div>,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let query = self
            .sidebar_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let rows = self.sidebar_rows(&query);
        let keyboard_focused = self.sidebar_focus.is_focused(window);
        let topology = self
            .workspaces
            .iter()
            .map(|ws| (ws.id, ws.tabs().iter().map(|tab| tab.id).collect()))
            .collect();
        let order: Vec<_> = Self::compute_display_order(&self.workspaces)
            .into_iter()
            .flat_map(|i| {
                std::iter::once(SidebarRow::Folder(i)).chain(
                    (0..self.workspaces[i].tab_count()).map(move |tab| SidebarRow::Tab(i, tab)),
                )
            })
            .collect();
        let (animated_rows, animating) = {
            let mut motion = self.sidebar_filter_motion.borrow_mut();
            motion.update(topology, &query, &rows, &order, std::time::Instant::now());
            (motion.rows.clone(), motion.started.is_some())
        };
        if animating {
            window.request_animation_frame();
        }
        if rows.is_empty() && !query.is_empty() && !animating {
            list = list.child(
                div()
                    .px(px(SIDEBAR_ROW_MARGIN_X + SIDEBAR_ROW_PADDING_X))
                    .py(px(8.))
                    .text_color(ui.muted)
                    .text_size(crate::ui_primitives::BODY_EMPHASIS)
                    .child(format!("No workspaces or tabs match \"{query}\"")),
            );
        }
        let slots = sidebar_drop_slots(&rows, self.workspaces.len());
        for (k, (row, amount)) in animated_rows.iter().enumerate() {
            let opens_group = matches!(row, SidebarRow::Folder(_))
                && k.checked_sub(1)
                    .is_some_and(|above| matches!(animated_rows[above].0, SidebarRow::Tab(..)));
            let spacing = if opens_group {
                SIDEBAR_GROUP_SPACING
            } else {
                SIDEBAR_ROW_SPACING
            };
            if query.is_empty() && !animating {
                list = list.child(self.render_drop_divider(k, slots[k], spacing, ui, cx));
            } else {
                list = list.child(div().flex_none().h(px(spacing * amount)));
            }
            let content = match *row {
                SidebarRow::Folder(i) => self
                    .render_workspace_row(
                        i,
                        keyboard_focused && self.sidebar_cursor_matches(*row),
                        ui,
                        cx,
                    )
                    .into_any_element(),
                SidebarRow::Tab(i, tab_idx) => self
                    .render_tab_row(
                        i,
                        tab_idx,
                        keyboard_focused && self.sidebar_cursor_matches(*row),
                        ui,
                        cx,
                    )
                    .into_any_element(),
            };
            let key = *row;
            let app = cx.weak_entity();
            let natural = div().relative().flex_none().w_full().child(content).child(
                gpui::canvas(
                    move |bounds, _, cx| {
                        let _ = app.update(cx, |this, _| {
                            this.sidebar_filter_motion
                                .borrow_mut()
                                .heights
                                .insert(key, f32::from(bounds.size.height));
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            );
            let height = self
                .sidebar_filter_motion
                .borrow()
                .heights
                .get(row)
                .copied()
                .unwrap_or_else(|| {
                    let meta = match *row {
                        SidebarRow::Folder(_) => false,
                        SidebarRow::Tab(ws, tab) => self
                            .render_tab_checkout_meta(
                                &self.workspaces[ws],
                                &self.workspaces[ws].tabs()[tab],
                                0.,
                                SIDEBAR_WIDTH,
                                cx,
                            )
                            .is_some(),
                    };
                    SIDEBAR_ROW_PADDING_Y * 2.
                        + SIDEBAR_ROW_LINE_HEIGHT
                        + if meta {
                            SIDEBAR_ROW_GAP + SIDEBAR_ROW_LINE_HEIGHT
                        } else {
                            0.
                        }
                });
            list = list.child(
                div()
                    .flex_none()
                    .w_full()
                    .overflow_hidden()
                    .opacity(*amount)
                    .when(*amount < 1., |row| row.h(px(height * amount)))
                    .child(natural),
            );
        }
        if query.is_empty()
            && !animating
            && let Some(&trailing) = slots.last()
        {
            list = list.child(self.render_drop_divider(
                rows.len(),
                trailing,
                SIDEBAR_ROW_SPACING,
                ui,
                cx,
            ));
        }
        list
    }

    pub(crate) fn sidebar_list_wrapper(
        &self,
        list: gpui::Stateful<gpui::Div>,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        div()
            .id("sidebar-list-wrapper")
            .relative()
            .group(SIDEBAR_DROP_GROUP)
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    this.open_workspace_folders(paths.paths(), cx);
                }),
            )
            .child(list)
            .child(Self::render_sidebar_drop_placeholder(cx))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sidebar_filter_motion_fades_out_and_removes_rows_at_completion() {
        let now = std::time::Instant::now();
        let row = super::SidebarRow::Folder(0);
        let mut motion = super::SidebarFilterMotion {
            rows: vec![(row, 1.)],
            from: vec![(row, 1.)],
            started: Some(now),
            ..Default::default()
        };
        motion.sample(now + std::time::Duration::from_millis(90));
        assert!((motion.rows[0].1 - 0.125).abs() < 0.001);
        motion.sample(now + std::time::Duration::from_millis(180));
        assert!(motion.rows.is_empty());
        assert!(motion.started.is_none());
    }

    #[test]
    fn sidebar_filter_motion_reverses_from_current_visibility() {
        let now = std::time::Instant::now();
        let row = super::SidebarRow::Folder(0);
        let mut motion = super::SidebarFilterMotion {
            rows: vec![(row, 1.)],
            from: vec![(row, 1.)],
            started: Some(now),
            ..Default::default()
        };
        let retarget = now + std::time::Duration::from_millis(60);
        motion.sample(retarget);
        let current = motion.rows[0].1;
        motion.from = motion.rows.clone();
        motion.target = vec![row];
        motion.started = Some(retarget);
        motion.sample(retarget);
        assert_eq!(motion.rows[0].1, current);
        motion.sample(retarget + std::time::Duration::from_millis(90));
        assert!(motion.rows[0].1 > current);
        motion.sample(retarget + std::time::Duration::from_millis(180));
        assert_eq!(motion.rows, vec![(row, 1.)]);
        assert!(motion.started.is_none());
    }
}
