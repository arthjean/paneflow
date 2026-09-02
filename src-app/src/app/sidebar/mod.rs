pub(crate) mod context_menu;
pub(crate) mod customize_menu;

use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    Animation, AnimationExt, AnyElement, AppContext, ClickEvent, Context, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, Render,
    SharedString, Styled, Window, div, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp, SIDEBAR_WIDTH, TabContextMenu, TabDrag, WorkspaceContextMenu, WorkspaceDrag,
    WorkspaceDragPreview, ai_types,
    app::pane_palette::PalettePlacement,
    pane_drag::PaneDrag,
    ui_primitives::{ROW_RADIUS, squircle, squircle_skin},
    workspace::{Tab, Workspace},
};

#[derive(Default)]
pub(crate) struct SidebarOrderCache {
    signature: Option<u64>,
    order: Vec<usize>,
}

struct SidebarRenderTimeCanary {
    start: std::time::Instant,
    workspace_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarAgentState {
    NeedsInput,
    Errored,
    Stalled,
    Finished,
    Thinking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SidebarRow {
    Folder(usize),
    Tab(usize, usize),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SidebarDropSlot {
    tab: Option<(usize, usize)>,
    workspace: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SidebarAgentSummary {
    state: SidebarAgentState,
    count: usize,
}

pub(crate) const SIDEBAR_ROW_MARGIN_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
pub(crate) const SIDEBAR_ROW_PADDING_Y: f32 = 6.0;
const SIDEBAR_ROW_GAP: f32 = 4.0;
pub(crate) const SIDEBAR_ROW_LINE_HEIGHT: f32 = 18.0;
const SIDEBAR_TITLE_ROW_GAP: f32 = 8.0;
const SIDEBAR_AGENT_STATUS_SLOT_WIDTH: f32 = 48.0;
const SIDEBAR_AGENT_ICON_SLOT_WIDTH: f32 = 20.0;
const SIDEBAR_DROP_GROUP: &str = "sidebar-drop-zone";
const SIDEBAR_DROP_PLACEHOLDER_MARGIN: f32 = 6.0;
const SIDEBAR_DROP_PLACEHOLDER_RADIUS: f32 = 8.0;
const SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA: f32 = 0.10;
const SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA: f32 = 0.22;
const SIDEBAR_ACTION_BUTTON_SIZE: f32 = 20.0;
const SIDEBAR_ACTION_BUTTON_GAP: f32 = 4.0;
const SIDEBAR_ACTION_LANE_WIDTH: f32 = SIDEBAR_TITLE_ROW_GAP + SIDEBAR_ACTION_BUTTON_SIZE;
const SIDEBAR_ROW_SPACING: f32 = 4.0;
const SIDEBAR_DROP_LINE_PX: f32 = 2.0;
const SIDEBAR_DROP_BAND_REACH: f32 = SIDEBAR_ROW_LINE_HEIGHT / 2.0 + SIDEBAR_ROW_PADDING_Y;
const SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH: f32 =
    SIDEBAR_WIDTH - SIDEBAR_ROW_MARGIN_X * 2.0 - SIDEBAR_ROW_PADDING_X * 2.0;
const SIDEBAR_FOLDER_ICON_WIDTH: f32 = 14.0;
const SIDEBAR_TAB_ICON_SIZE: f32 = 16.0;
const SIDEBAR_TAB_ICON_GAP: f32 = 3.0;
const SIDEBAR_TAB_ICON_CAP: usize = 4;
const SIDEBAR_TAB_CARD_WIDTH: f32 = 24.0;
const SIDEBAR_TAB_CARD_HEIGHT: f32 = 24.0;
const SIDEBAR_TAB_CARD_ICON_SIZE: f32 = 16.0;
const SIDEBAR_TAB_ICON_OVERLAP: f32 = 11.0;

fn sidebar_row_shell() -> gpui::Div {
    div()
        .px(px(SIDEBAR_ROW_PADDING_X))
        .py(px(SIDEBAR_ROW_PADDING_Y))
        .flex_none()
        .relative()
        .overflow_x_hidden()
        .flex()
        .flex_col()
        .gap(px(SIDEBAR_ROW_GAP))
}

fn render_sidebar_indent_guide(ui: crate::theme::UiColors, interrupted: bool) -> Vec<gpui::Div> {
    let color = ui.text.opacity(0.08);
    let left = px(SIDEBAR_ROW_PADDING_X + (SIDEBAR_FOLDER_ICON_WIDTH / 2.).floor());
    let segment = move || div().absolute().left(left).w(px(1.)).bg(color);
    if !interrupted {
        return vec![segment().top_0().bottom_0()];
    }
    let center = SIDEBAR_ROW_PADDING_Y + SIDEBAR_ROW_LINE_HEIGHT / 2.;
    let radius = SIDEBAR_AGENT_ICON_SLOT_WIDTH / 2.;
    vec![
        segment().top_0().h(px(center - radius)),
        segment().top(px(center + radius)).bottom_0(),
    ]
}

fn squircle_row(
    shell: gpui::Stateful<gpui::Div>,
    group: SharedString,
    resting: Option<gpui::Hsla>,
    hovered: Option<gpui::Hsla>,
    body: impl IntoElement,
) -> gpui::Stateful<gpui::Div> {
    squircle_skin(shell, group, ROW_RADIUS, resting, hovered).child(body)
}

fn sidebar_hover_actions(group: SharedString) -> gpui::Div {
    div()
        .absolute()
        .top(px(
            (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_ACTION_BUTTON_SIZE) / 2.
        ))
        .right(px(0.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_ACTION_BUTTON_GAP))
        .invisible()
        .group_hover(group, |style| style.visible())
}

fn sidebar_action_button(
    id: SharedString,
    icon: &'static str,
    icon_size: f32,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let active_bg = crate::app::constants::sidebar_tab_active_background();
    div()
        .id(id)
        .flex_none()
        .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .text_color(ui.muted)
        .hover(move |style| style.bg(active_bg))
        .child(
            svg()
                .size(px(icon_size))
                .flex_none()
                .path(icon)
                .text_color(ui.muted),
        )
}

impl SidebarAgentSummary {
    fn slot_width(self) -> f32 {
        if self.state == SidebarAgentState::NeedsInput {
            SIDEBAR_AGENT_STATUS_SLOT_WIDTH
        } else if self.count > 1 {
            28.0
        } else {
            SIDEBAR_AGENT_ICON_SLOT_WIDTH
        }
    }

    fn tooltip_state(self) -> String {
        match self.state {
            SidebarAgentState::NeedsInput => {
                agent_status_sentence(self.count, "needs input", "need input")
            }
            SidebarAgentState::Errored => agent_status_sentence(self.count, "errored", "errored"),
            SidebarAgentState::Stalled => agent_status_sentence(self.count, "stalled", "stalled"),
            SidebarAgentState::Thinking => {
                agent_status_sentence(self.count, "thinking", "thinking")
            }
            SidebarAgentState::Finished => {
                "Agent finished · Click workspace or pane to dismiss".to_string()
            }
        }
    }
}

fn agent_status_sentence(count: usize, singular_state: &str, plural_state: &str) -> String {
    if count == 1 {
        format!("1 agent {singular_state}")
    } else {
        format!("{count} agents {plural_state}")
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

fn tab_diffstat_visible(
    show: paneflow_config::schema::SidebarShow,
    stats: &crate::workspace::GitDiffStats,
) -> bool {
    show.diffstat_enabled() && (stats.insertions > 0 || stats.deletions > 0)
}

fn sidebar_agent_summary<'a, I>(sessions: I, completion_unread: bool) -> Option<SidebarAgentSummary>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
{
    let mut counts = [0usize; 4];
    for session in sessions {
        let index = match session.state {
            ai_types::AgentState::WaitingForInput => 0,
            ai_types::AgentState::Errored => 1,
            ai_types::AgentState::Stalled => 2,
            ai_types::AgentState::Thinking => 3,
            ai_types::AgentState::Finished => continue,
        };
        counts[index] += 1;
    }

    let priority = [
        SidebarAgentState::NeedsInput,
        SidebarAgentState::Errored,
        SidebarAgentState::Stalled,
    ];
    for (state, count) in priority.into_iter().zip(counts[..3].iter().copied()) {
        if count > 0 {
            return Some(SidebarAgentSummary { state, count });
        }
    }

    if completion_unread {
        return Some(SidebarAgentSummary {
            state: SidebarAgentState::Finished,
            count: 1,
        });
    }

    (counts[3] > 0).then_some(SidebarAgentSummary {
        state: SidebarAgentState::Thinking,
        count: counts[3],
    })
}

fn folder_row_sessions<'a, I>(
    sessions: I,
    expanded: bool,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| !expanded || session.surface_id.is_none())
}

fn tab_row_sessions<'a, I>(
    sessions: I,
    surfaces: &'a std::collections::HashSet<u64>,
) -> impl Iterator<Item = &'a ai_types::AgentSession>
where
    I: IntoIterator<Item = &'a ai_types::AgentSession>,
    I::IntoIter: 'a,
{
    sessions
        .into_iter()
        .filter(move |session| session.surface_id.is_some_and(|id| surfaces.contains(&id)))
}

fn tab_icon_cluster_split(pane_count: usize) -> (usize, usize) {
    let shown = pane_count.min(SIDEBAR_TAB_ICON_CAP);
    (shown, pane_count - shown)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TabPaneIcon {
    path: &'static str,
    label: &'static str,
}

fn tab_pane_icon(pane: &crate::pane::Pane, cx: &gpui::App) -> TabPaneIcon {
    let agent = pane
        .surface
        .as_terminal()
        .and_then(|terminal| terminal.read(cx).terminal.detected_agent);
    match agent {
        Some(agent) => TabPaneIcon {
            path: agent.icon_path(),
            label: agent.display_name(),
        },
        None => TabPaneIcon {
            path: pane.surface.kind_icon(),
            label: pane.surface.kind_label(),
        },
    }
}

fn tab_display_title(tab: &Tab, tab_idx: usize) -> String {
    if tab.title().trim().is_empty() {
        format!("Tab {}", tab_idx + 1)
    } else {
        tab.title().to_string()
    }
}

fn reorder_target(from: usize, slot: usize) -> usize {
    if from < slot { slot - 1 } else { slot }
}

fn sidebar_drop_slots(rows: &[SidebarRow], workspace_count: usize) -> Vec<SidebarDropSlot> {
    (0..=rows.len())
        .map(|k| SidebarDropSlot {
            tab: match k.checked_sub(1).map(|above| rows[above]) {
                Some(SidebarRow::Folder(ws)) => Some((ws, 0)),
                Some(SidebarRow::Tab(ws, tab)) => Some((ws, tab + 1)),
                None => None,
            },
            workspace: match rows.get(k) {
                Some(SidebarRow::Folder(ws)) => Some(*ws),
                None => Some(workspace_count),
                Some(SidebarRow::Tab(..)) => None,
            },
        })
        .collect()
}

impl PaneFlowApp {
    fn inline_rename_field(&self, ui: crate::theme::UiColors) -> gpui::Div {
        div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .text_color(ui.text)
            .text_sm()
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .bg(ui.overlay)
            .px_1()
            .rounded_sm()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.rename_input.clone())
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
            .relative()
            .w(px(SIDEBAR_WIDTH))
            .flex_shrink_0()
            .h_full()
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
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
                        .pl(px(8.))
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
                        .gap(px(2.))
                        .child(customize_menu::render_customize_sidebar_button(
                            customize_menu::CustomizeMenuState {
                                open: self.sidebar_customize_menu_open,
                                submenu_open: self.sidebar_show_submenu_open,
                                show: self.cached_config.sidebar_show,
                                all_expanded: self.all_workspaces_expanded(),
                            },
                            SIDEBAR_FOLDER_ICON_WIDTH,
                            ui,
                            cx,
                        ))
                        .child({
                            let hover_bg = crate::app::constants::sidebar_tab_hover_background();
                            squircle_skin(
                                div()
                                    .id("sidebar-new-workspace")
                                    .size(px(28.))
                                    .flex()
                                    .items_center()
                                    .justify_center(),
                                "sidebar-new-workspace-group",
                                ROW_RADIUS,
                                None,
                                Some(hover_bg),
                            )
                            .delayed_tooltip(move |_w, cx| {
                                cx.new(|_| SidebarTooltip {
                                    label: new_workspace_tooltip.clone().into(),
                                })
                                .into()
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.create_workspace_with_picker(window, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
                                    .flex_none()
                                    .path("icons/folder-plus.svg")
                                    .text_color(ui.muted),
                            )
                        }),
                ),
        );

        let mut list = div()
            .id("workspace-list")
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.sidebar_scroll)
            .flex()
            .flex_col()
            .pb(px(4.));

        if self.workspaces.is_empty() {
            list = list.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .px(px(16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Open a project folder"),
                    )
                    .child({
                        let hover_bg = crate::app::constants::sidebar_tab_active_background();
                        div()
                            .id("empty-new-ws")
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.))
                            .px(px(10.))
                            .py(px(5.))
                            .rounded(px(6.))
                            .bg(ui.subtle)
                            .text_color(ui.text)
                            .text_size(px(11.))
                            .font_weight(FontWeight::MEDIUM)
                            .hover(move |style| style.bg(hover_bg))
                            .on_click(cx.listener(|this, _: &ClickEvent, w, cx| {
                                this.create_workspace_with_picker(w, cx);
                            }))
                            .child(
                                svg()
                                    .size(px(12.))
                                    .flex_none()
                                    .path("icons/folder_open.svg")
                                    .text_color(ui.muted),
                            )
                            .child("Open folder")
                    }),
            );
        }

        list = self.render_workspace_rows(list, ui, cx);
        sidebar = sidebar.child(self.sidebar_list_wrapper(list, cx));
        sidebar = sidebar.child(self.render_sidebar_settings_footer(cx));
        sidebar
    }

    fn sidebar_rows(&self) -> Vec<SidebarRow> {
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
            rows.push(SidebarRow::Folder(i));
            if self.workspaces[i].sidebar_expanded && !self.workspaces[i].is_empty_shell() {
                for tab_idx in 0..self.workspaces[i].tab_count() {
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
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let rows = self.sidebar_rows();
        let slots = sidebar_drop_slots(&rows, self.workspaces.len());
        for (k, row) in rows.iter().enumerate() {
            list = list.child(self.render_drop_divider(k, slots[k], ui, cx));
            list = list.child(match *row {
                SidebarRow::Folder(i) => self.render_workspace_row(i, ui, cx).into_any_element(),
                SidebarRow::Tab(i, tab_idx) => {
                    self.render_tab_row(i, tab_idx, ui, cx).into_any_element()
                }
            });
        }
        if let Some(&trailing) = slots.last() {
            list = list.child(self.render_drop_divider(rows.len(), trailing, ui, cx));
        }
        list
    }

    fn render_drop_divider(
        &self,
        key: usize,
        slot: SidebarDropSlot,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let color = ui.text.opacity(0.5);
        let group = SharedString::from(format!("drop-slot-{key}"));
        let mut band = div()
            .id(SharedString::from(format!("drop-band-{key}")))
            .group(group.clone())
            .absolute()
            .top(px(-SIDEBAR_DROP_BAND_REACH))
            .w_full()
            .px(px(SIDEBAR_ROW_MARGIN_X))
            .h(px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0))
            .flex()
            .flex_col()
            .justify_center();
        let mut line = div()
            .group(SharedString::from(format!("drop-line-{key}")))
            .h(px(SIDEBAR_DROP_LINE_PX))
            .w_full()
            .rounded_full();

        if let Some((ws_idx, tab_idx)) = slot.tab {
            let ws_id = self.workspaces.get(ws_idx).map(|ws| ws.id);
            band = band
                .on_drop(cx.listener(move |this, drag: &TabDrag, window, cx| {
                    if ws_id == Some(drag.workspace_id) {
                        let Some(from) = this
                            .workspaces
                            .get(ws_idx)
                            .and_then(|ws| ws.tabs().iter().position(|tab| tab.id == drag.tab_id))
                        else {
                            return;
                        };
                        this.reorder_workspace_tab(drag, ws_idx, reorder_target(from, tab_idx), cx);
                    } else {
                        this.move_tab_to_workspace(drag, ws_idx, tab_idx, window, cx);
                    }
                }))
                .on_drop(cx.listener(move |this, drag: &PaneDrag, window, cx| {
                    this.move_pane_to_new_tab(drag.pane_id, ws_idx, tab_idx, window, cx);
                }));
            line = line
                .group_drag_over::<TabDrag>(group.clone(), move |style| style.bg(color))
                .group_drag_over::<PaneDrag>(group.clone(), move |style| style.bg(color));
        }

        if let Some(ws_slot) = slot.workspace {
            band = band.on_drop(cx.listener(move |this, drag: &WorkspaceDrag, _window, cx| {
                let Some(from) = this.workspaces.iter().position(|ws| ws.id == drag.id) else {
                    return;
                };
                this.reorder_workspace(drag.id, reorder_target(from, ws_slot), cx);
            }));
            line =
                line.group_drag_over::<WorkspaceDrag>(group.clone(), move |style| style.bg(color));
        }

        div()
            .h(px(SIDEBAR_ROW_SPACING))
            .flex_none()
            .relative()
            .child(band.child(line))
    }

    fn render_workspace_row(
        &self,
        i: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[i];

        let title = ws.title.clone();

        let idx = i;
        let ws_id = ws.id;
        let ws_title: SharedString = ws.title.clone().into();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let group_name = SharedString::from(format!("ws-row-{ws_id}"));
        let is_expanded = ws.sidebar_expanded;

        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("ws-{ws_id}")))
            .group(group_name.clone())
            .on_drag(
                WorkspaceDrag {
                    id: ws_id,
                    title: ws_title.clone(),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                this.dismiss_transient_surfaces();
                let was_renaming = this.renaming_tab.is_some();
                this.commit_rename(cx);
                this.select_workspace(idx, window, cx);
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count >= 2);
                if !was_renaming && !is_double {
                    this.toggle_workspace_expanded(idx, cx);
                }
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.workspace_menu_open = Some(WorkspaceContextMenu { idx, position });
                    cx.stop_propagation();
                    cx.notify();
                }
            }));

        let folder_sessions = || folder_row_sessions(ws.agent_sessions.values(), is_expanded);
        let agent_status = ai_types::workspace_agent_status(folder_sessions(), &ws.detected_agents);
        let completion_unread = if is_expanded {
            ws.agent_completion_notification.has_unattributed_unread()
        } else {
            ws.agent_completion_notification.is_unread()
        };
        let row_agent_status = sidebar_agent_summary(folder_sessions(), completion_unread);
        let title_el = div()
            .flex_1()
            .min_w_0()
            .overflow_x_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .text_color(ui.text)
            .text_sm()
            .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
            .font_weight(FontWeight::MEDIUM)
            .child(title);

        let folder_path = if is_expanded {
            "icons/folder-open.svg"
        } else {
            "icons/folder.svg"
        };
        let disclosure = div()
            .flex_none()
            .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .size(px(SIDEBAR_FOLDER_ICON_WIDTH))
                    .flex_none()
                    .path(folder_path)
                    .text_color(ui.muted),
            );

        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .overflow_x_hidden()
            .pr(px(SIDEBAR_ACTION_LANE_WIDTH))
            .child(disclosure)
            .child(title_el);
        if let Some(row_agent_status) = row_agent_status {
            let status_tooltip = sidebar_agent_status_tooltip(row_agent_status, &agent_status);
            title_row = title_row.child(render_workspace_agent_summary(
                row_agent_status,
                &format!("ws-{ws_id}"),
                status_tooltip,
                ui,
            ));
        }

        let mut body = div()
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_ROW_GAP))
            .child(title_row);

        body = body.child(
            sidebar_hover_actions(group_name.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("ws-new-tab-{ws_id}")),
                    "icons/plus.svg",
                    12.,
                    ui,
                )
                .delayed_tooltip({
                    let label = SharedString::from(format!("New pane in {ws_title}"));
                    move |_w, cx| {
                        cx.new(|_| SidebarTooltip {
                            label: label.clone(),
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_pane_palette(idx, window, cx);
                    cx.stop_propagation();
                })),
            ),
        );

        let row = squircle_row(row_shell, group_name.clone(), None, Some(hover_bg), body);

        div()
            .id(SharedString::from(format!("ws-drop-{ws_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .rounded(ROW_RADIUS)
            .child(row)
    }

    fn render_tab_row(
        &self,
        ws_idx: usize,
        tab_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ws = &self.workspaces[ws_idx];
        let ws_id = ws.id;
        let tab = &ws.tabs()[tab_idx];
        let tab_id = tab.id;
        let title = tab_display_title(tab, tab_idx);
        let is_active_tab = tab_idx == ws.active_tab_idx();
        let is_active_workspace = ws_idx == self.active_idx;
        let is_renaming = self.renaming_tab == Some((ws_idx, tab_idx));

        let panes = tab.collect_panes();
        let mut surfaces: std::collections::HashSet<u64> =
            std::collections::HashSet::with_capacity(panes.len());
        let mut pane_icons: Vec<TabPaneIcon> = Vec::with_capacity(panes.len());
        let mut tab_agents: std::collections::HashSet<String> = std::collections::HashSet::new();
        for pane in &panes {
            let pane = pane.read(cx);
            for terminal in pane.terminals() {
                surfaces.insert(terminal.entity_id().as_u64());
                if let Some(agent) = terminal.read(cx).terminal.detected_agent {
                    tab_agents.insert(agent.binary().to_string());
                }
            }
            pane_icons.push(tab_pane_icon(pane, cx));
        }
        let pending_split_pane = match self.pane_palette.as_ref().map(|p| &p.placement) {
            Some(PalettePlacement::Split { target, .. }) => {
                let target = target.entity_id();
                panes.iter().any(|pane| pane.entity_id() == target)
            }
            _ => false,
        };
        let tab_sessions = || tab_row_sessions(ws.agent_sessions.values(), &surfaces);
        let row_agent_status = sidebar_agent_summary(
            tab_sessions(),
            ws.agent_completion_notification.is_unread_for(&surfaces),
        );
        let agent_status = ai_types::workspace_agent_status(tab_sessions(), &tab_agents);
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let (resting_bg, hovered_bg) = if is_active_tab && is_active_workspace {
            (Some(hover_bg), None)
        } else {
            (None, Some(hover_bg))
        };
        let text_color = ui.text;

        let title_el = if is_renaming {
            self.inline_rename_field(ui)
        } else {
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(text_color)
                .text_sm()
                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                .child(title.clone())
        };

        let leading_slot = match row_agent_status {
            Some(status) => render_tab_agent_summary(
                status,
                &format!("tab-{tab_id}"),
                sidebar_agent_status_tooltip(status, &agent_status),
                ui,
            ),
            None => div()
                .flex_none()
                .w(px(SIDEBAR_FOLDER_ICON_WIDTH))
                .into_any_element(),
        };

        let mut title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(SIDEBAR_TITLE_ROW_GAP))
            .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
            .min_w_0()
            .child(leading_slot)
            .child(title_el);
        let tab_group = SharedString::from(format!("tab-row-group-{tab_id}"));
        match render_tab_pane_icons(
            &pane_icons,
            &format!("tab-{tab_id}"),
            pending_split_pane,
            ui,
        ) {
            Some(cluster) => {
                title_row = title_row.child(
                    div()
                        .flex_none()
                        .group_hover(tab_group.clone(), |style| style.invisible())
                        .child(cluster),
                );
            }
            None => {
                title_row = title_row.child(div().flex_none().w(px(SIDEBAR_ACTION_BUTTON_SIZE)));
            }
        }

        title_row = title_row.child(
            sidebar_hover_actions(tab_group.clone()).child(
                sidebar_action_button(
                    SharedString::from(format!("tab-close-{tab_id}")),
                    "icons/close.svg",
                    12.,
                    ui,
                )
                .delayed_tooltip({
                    let label = SharedString::from("Close tab");
                    move |_w, cx| {
                        cx.new(|_| SidebarTooltip {
                            label: label.clone(),
                        })
                        .into()
                    }
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    if let Some((at_ws, at_tab)) = this
                        .workspaces
                        .iter()
                        .position(|ws| ws.id == ws_id)
                        .and_then(|at_ws| {
                            this.workspaces[at_ws]
                                .tabs()
                                .iter()
                                .position(|tab| tab.id == tab_id)
                                .map(|at_tab| (at_ws, at_tab))
                        })
                    {
                        this.commit_rename(cx);
                        this.close_workspace_tab(at_ws, at_tab, window, cx);
                    }
                    cx.stop_propagation();
                })),
            ),
        );

        let row_shell = sidebar_row_shell()
            .id(SharedString::from(format!("tab-row-{tab_id}")))
            .group(tab_group.clone())
            .on_drag(
                TabDrag {
                    workspace_id: ws_id,
                    tab_id,
                    title: SharedString::from(title),
                },
                |drag, _offset, _window, cx| {
                    cx.new(|_| WorkspaceDragPreview {
                        title: drag.title.clone(),
                    })
                },
            )
            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                let is_double = matches!(e, ClickEvent::Mouse(m) if m.down.click_count == 2);
                if is_double {
                    this.begin_tab_rename(ws_idx, tab_idx, cx);
                } else {
                    this.select_workspace_tab(ws_idx, tab_idx, window, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_aux_click(cx.listener(move |this, e: &ClickEvent, _window, cx| {
                if e.is_right_click()
                    && let Some(position) = e.mouse_position()
                {
                    this.commit_rename(cx);
                    this.dismiss_transient_surfaces();
                    this.tab_menu_open = Some(TabContextMenu {
                        ws_idx,
                        tab_idx,
                        position,
                    });
                    this.spawn_worktree_listing(ws_idx, cx);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _window, cx| {
                if this.renaming_tab != Some((ws_idx, tab_idx)) {
                    return;
                }
                match e.keystroke.key.as_str() {
                    "enter" => {
                        this.commit_rename(cx);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" => {
                        this.renaming_tab = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
            }));

        let body = match self.render_tab_checkout_meta(
            ws,
            tab,
            SIDEBAR_FOLDER_ICON_WIDTH + SIDEBAR_TITLE_ROW_GAP,
            ui,
        ) {
            Some(meta) => div()
                .flex()
                .flex_col()
                .gap(px(SIDEBAR_ROW_GAP))
                .child(title_row)
                .child(meta)
                .into_any_element(),
            None => title_row.into_any_element(),
        };

        let row = squircle_row(row_shell, tab_group, resting_bg, hovered_bg, body);

        div()
            .id(SharedString::from(format!("tab-drop-{tab_id}")))
            .mx(px(SIDEBAR_ROW_MARGIN_X))
            .flex_none()
            .flex()
            .flex_col()
            .relative()
            .rounded(ROW_RADIUS)
            .when(
                self.cached_config.sidebar_show.indent_guide_enabled(),
                |el| el.children(render_sidebar_indent_guide(ui, row_agent_status.is_some())),
            )
            .child(row)
    }

    fn tab_row_branch(&self, ws: &Workspace, tab: &Tab) -> String {
        match tab.worktree.as_ref() {
            Some(_) => self
                .tab_checkout_git(tab)
                .map(|git| git.branch.clone())
                .unwrap_or_default(),
            None => ws.git_branch.clone(),
        }
    }

    fn tab_row_checkout(
        &self,
        ws: &Workspace,
        tab: &Tab,
    ) -> Option<(String, crate::workspace::GitDiffStats)> {
        let label = |branch: &str, path: &std::path::Path| {
            crate::workspace::worktree::checkout_label(Some(branch), path, &ws.worktree_root)
        };
        match tab.worktree.as_ref() {
            Some(path) => {
                let git = self.tab_checkout_git(tab)?;
                git.is_repo
                    .then(|| (label(&git.branch, path), git.stats.clone()))
            }
            None => ws.is_git_repo.then(|| {
                (
                    label(&ws.git_branch, &ws.worktree_root),
                    ws.git_stats.clone(),
                )
            }),
        }
    }

    fn render_tab_checkout_meta(
        &self,
        ws: &Workspace,
        tab: &Tab,
        indent: f32,
        ui: crate::theme::UiColors,
    ) -> Option<AnyElement> {
        let show = self.cached_config.sidebar_show;
        if !show.any_enabled() {
            return None;
        }
        let (label, stats) = self.tab_row_checkout(ws, tab)?;
        let draw_branch = show.branch_enabled() && !label.is_empty();
        let draw_counts = tab_diffstat_visible(show, &stats);
        if !draw_branch && !draw_counts {
            return None;
        }

        let pr = show
            .pr_enabled()
            .then(|| self.tab_row_branch(ws, tab))
            .and_then(|branch| {
                let repo_root = ws.repo_root.as_ref()?;
                (!branch.is_empty())
                    .then(|| self.pull_request_for(repo_root, &branch))
                    .flatten()
            });

        let branch = draw_branch.then(|| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(3.))
                .flex_1()
                .min_w_0()
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path(match pr {
                            Some(_) => "icons/git-pull-request.svg",
                            None => "icons/git-branch-sidebar.svg",
                        })
                        .text_color(match pr {
                            Some(pr) => pr.state.color(ui),
                            None => ui.muted,
                        }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .text_color(ui.muted)
                        .child(label),
                )
        });

        let counts = draw_counts.then(|| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .child(
                    div()
                        .text_color(ui.vc_added)
                        .child(format!("+{}", stats.insertions)),
                )
                .child(
                    div()
                        .text_color(ui.vc_deleted)
                        .child(format!("\u{2212}{}", stats.deletions)),
                )
        });

        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(6.))
                .h(px(SIDEBAR_ROW_LINE_HEIGHT))
                .pl(px(indent))
                .w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .max_w(px(SIDEBAR_WORKSPACE_ROW_CONTENT_WIDTH))
                .overflow_x_hidden()
                .when_some(branch, |row, branch| row.child(branch))
                .when_some(counts, |row, counts| row.child(counts))
                .into_any_element(),
        )
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

    fn render_sidebar_drop_placeholder(cx: &mut Context<Self>) -> impl IntoElement {
        let tint = crate::theme::ui_colors().text;
        div()
            .absolute()
            .top(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .left(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .right(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .bottom(px(SIDEBAR_DROP_PLACEHOLDER_MARGIN))
            .rounded(px(SIDEBAR_DROP_PLACEHOLDER_RADIUS))
            .bg(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_FILL_ALPHA))
            .border_2()
            .border_color(tint.opacity(SIDEBAR_DROP_PLACEHOLDER_BORDER_ALPHA))
            .invisible()
            .group_drag_over::<gpui::ExternalPaths>(SIDEBAR_DROP_GROUP, |style| style.visible())
            .on_drop(
                cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                    this.open_workspace_folders(paths.paths(), cx);
                }),
            )
    }
}

fn sidebar_agent_status_tooltip(
    summary: SidebarAgentSummary,
    status: &ai_types::WorkspaceAgentStatus,
) -> SharedString {
    let state = summary.tooltip_state();
    if summary.state == SidebarAgentState::Finished {
        return state.into();
    }

    let mut details: Vec<String> = status
        .hooked
        .iter()
        .map(|aggregate| {
            format!(
                "{}{}",
                aggregate.tool.display_name(),
                aggregate.extra_suffix()
            )
        })
        .chain(
            status
                .unhooked
                .iter()
                .map(|tool| format!("{} running", tool.display_name())),
        )
        .collect();
    for label in &status.active_labels {
        if !details.iter().any(|detail| detail.starts_with(label)) {
            details.push(label.clone());
        }
    }

    if details.is_empty() {
        state.into()
    } else {
        format!("{state} · {}", details.join(", ")).into()
    }
}

fn render_tab_pane_icons(
    icons: &[TabPaneIcon],
    row_key: &str,
    pending_pane: bool,
    ui: crate::theme::UiColors,
) -> Option<AnyElement> {
    if icons.is_empty() {
        return None;
    }
    let (shown, overflow) = tab_icon_cluster_split(icons.len());
    let tooltip: SharedString = icons
        .iter()
        .map(|icon| icon.label)
        .collect::<Vec<_>>()
        .join(", ")
        .into();

    let glyph = ui.text.opacity(0.75);
    if shown == 1 && !pending_pane {
        return Some(tab_pane_icon_lane(
            row_key,
            tooltip,
            overflow,
            ui,
            svg()
                .size(px(SIDEBAR_TAB_ICON_SIZE))
                .min_w(px(SIDEBAR_TAB_ICON_SIZE))
                .flex_none()
                .path(icons[0].path)
                .text_color(glyph),
        ));
    }

    let card_fill = crate::app::constants::sidebar_tab_icon_card_background();
    let card_border = ui.text.opacity(0.14);
    let card_glyph = ui.text.opacity(0.92);

    let step = SIDEBAR_TAB_CARD_WIDTH - SIDEBAR_TAB_ICON_OVERLAP;
    let cluster_width = SIDEBAR_TAB_CARD_WIDTH + (shown.saturating_sub(1) as f32) * step;
    let card_overhang = (SIDEBAR_ROW_LINE_HEIGHT - SIDEBAR_TAB_CARD_HEIGHT) / 2.0;
    let mut cluster = div()
        .flex_none()
        .relative()
        .w(px(cluster_width))
        .min_w(px(cluster_width))
        .h(px(SIDEBAR_ROW_LINE_HEIGHT));
    for (slot, icon) in icons[..shown].iter().enumerate() {
        cluster = cluster.child(
            div()
                .absolute()
                .top(px(card_overhang))
                .left(px(slot as f32 * step))
                .flex()
                .flex_row()
                .items_center()
                .justify_center()
                .w(px(SIDEBAR_TAB_CARD_WIDTH))
                .h(px(SIDEBAR_TAB_CARD_HEIGHT))
                .child(squircle::squircle_fill(ROW_RADIUS, card_fill))
                .child(squircle::squircle_border(ROW_RADIUS, px(1.), card_border))
                .child(
                    svg()
                        .size(px(SIDEBAR_TAB_CARD_ICON_SIZE))
                        .flex_none()
                        .path(icon.path)
                        .text_color(card_glyph),
                ),
        );
    }
    Some(tab_pane_icon_lane(row_key, tooltip, overflow, ui, cluster))
}

fn tab_pane_icon_lane(
    row_key: &str,
    tooltip: SharedString,
    overflow: usize,
    ui: crate::theme::UiColors,
    cluster: impl IntoElement,
) -> AnyElement {
    div()
        .id(SharedString::from(format!("tab-panes-{row_key}")))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(SIDEBAR_TAB_ICON_GAP))
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(cluster)
        .when(overflow > 0, |d| {
            d.child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .child(format!("+{overflow}")),
            )
        })
        .into_any_element()
}

fn render_workspace_agent_summary(
    summary: SidebarAgentSummary,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (color, glyph, label) = agent_summary_visual(summary, row_key, ui);

    div()
        .id(SharedString::from(format!("agent-status-{row_key}")))
        .w(px(summary.slot_width()))
        .h(px(20.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(3.))
        .text_size(px(10.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .when_some(label, |d, label| d.child(label))
        .into_any_element()
}

fn render_tab_agent_summary(
    summary: SidebarAgentSummary,
    row_key: &str,
    tooltip: SharedString,
    ui: crate::theme::UiColors,
) -> AnyElement {
    let (_, glyph, _) = agent_summary_visual(summary, row_key, ui);

    div()
        .id(SharedString::from(format!("agent-status-{row_key}")))
        .w(px(SIDEBAR_FOLDER_ICON_WIDTH))
        .h(px(20.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .delayed_tooltip(move |_w, cx| {
            cx.new(|_| SidebarTooltip {
                label: tooltip.clone(),
            })
            .into()
        })
        .child(glyph)
        .into_any_element()
}

fn agent_summary_visual(
    summary: SidebarAgentSummary,
    row_key: &str,
    ui: crate::theme::UiColors,
) -> (gpui::Hsla, AnyElement, Option<String>) {
    match summary.state {
        SidebarAgentState::NeedsInput => (
            rgb(0xFBBF24).into(),
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/bell.svg")
                .text_color(rgb(0xFBBF24))
                .into_any_element(),
            Some(if summary.count > 1 {
                format!("Input {}", summary.count)
            } else {
                "Input".to_string()
            }),
        ),
        SidebarAgentState::Errored => (
            ui.agent_error,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/x_circle.svg")
                .text_color(ui.agent_error)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Stalled => (
            ui.agent_stalled,
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.agent_stalled)
                .into_any_element(),
            (summary.count > 1).then(|| summary.count.to_string()),
        ),
        SidebarAgentState::Thinking => {
            let color = ui.muted;
            (
                color,
                render_comet_trail_loader(row_key, color),
                (summary.count > 1).then(|| summary.count.to_string()),
            )
        }
        SidebarAgentState::Finished => {
            let color: gpui::Hsla = rgb(0x83C3FF).into();
            (
                color,
                div()
                    .size(px(11.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().size(px(7.)).rounded_full().bg(color))
                    .into_any_element(),
                None,
            )
        }
    }
}

fn render_comet_trail_loader(row_key: &str, color: gpui::Hsla) -> AnyElement {
    static SYNC_EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

    const MATRIX_SIZE: usize = 3;
    const DOT_SIZE: f32 = 3.0;
    const DOT_GAP: f32 = 1.0;
    const CYCLE_MS: u64 = 720;
    const PERIMETER: usize = 8;
    const BASE_OPACITY: f32 = 0.06;
    const TAIL_OPACITIES: [f32; 3] = [0.8144, 0.4864, 0.2568];

    div()
        .size(px(11.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(DOT_GAP))
        .with_animation(
            SharedString::from(format!("comet-trail-{row_key}")),
            Animation::new(std::time::Duration::from_millis(CYCLE_MS)).repeat(),
            move |loader, _delta| {
                let cycle_elapsed = SYNC_EPOCH
                    .get_or_init(std::time::Instant::now)
                    .elapsed()
                    .as_millis()
                    % u128::from(CYCLE_MS);
                let head = (cycle_elapsed * PERIMETER as u128 / u128::from(CYCLE_MS)) as usize;

                loader.children((0..MATRIX_SIZE).map(|row| {
                    div()
                        .h(px(DOT_SIZE))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(DOT_GAP))
                        .children((0..MATRIX_SIZE).map(move |col| {
                            let order = match (row, col) {
                                (0, 0) => Some(0),
                                (0, 1) => Some(1),
                                (0, 2) => Some(2),
                                (1, 2) => Some(3),
                                (2, 2) => Some(4),
                                (2, 1) => Some(5),
                                (2, 0) => Some(6),
                                (1, 0) => Some(7),
                                _ => None,
                            };
                            let opacity = order.map_or_else(
                                || if head.is_multiple_of(2) { 0.1 } else { 0.18 },
                                |order| {
                                    let trail = (head + PERIMETER - order) % PERIMETER;
                                    TAIL_OPACITIES.get(trail).copied().unwrap_or(BASE_OPACITY)
                                },
                            );

                            div()
                                .size(px(DOT_SIZE))
                                .flex_none()
                                .rounded_full()
                                .bg(color.opacity(opacity))
                        }))
                }))
            },
        )
        .into_any_element()
}

pub(crate) struct SidebarTooltip {
    pub(crate) label: SharedString,
}

impl Render for SidebarTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::ui_primitives::tooltip_shell().child(self.label.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ROW_RADIUS, SIDEBAR_ACTION_BUTTON_SIZE, SIDEBAR_ACTION_LANE_WIDTH, SIDEBAR_DROP_BAND_REACH,
        SIDEBAR_DROP_LINE_PX, SIDEBAR_FOLDER_ICON_WIDTH, SIDEBAR_ROW_LINE_HEIGHT,
        SIDEBAR_ROW_MARGIN_X, SIDEBAR_ROW_PADDING_Y, SIDEBAR_ROW_SPACING, SIDEBAR_TAB_CARD_HEIGHT,
        SIDEBAR_TAB_CARD_ICON_SIZE, SIDEBAR_TAB_CARD_WIDTH, SIDEBAR_TAB_ICON_CAP,
        SIDEBAR_TAB_ICON_SIZE, SIDEBAR_TITLE_ROW_GAP, SIDEBAR_WIDTH, SidebarAgentState,
        SidebarAgentSummary, SidebarDropSlot, SidebarRow, folder_row_sessions, reorder_target,
        sidebar_agent_summary, sidebar_drop_slots, sidebar_row_shell, tab_diffstat_visible,
        tab_display_title, tab_icon_cluster_split, tab_row_sessions,
    };
    use crate::agent_launcher::TerminalAgent;
    use crate::ai_types::{AgentSession, AgentState};
    use crate::workspace::Tab;
    use gpui::{
        AvailableSpace, InteractiveElement, ParentElement, Styled, TestAppContext, div, point, px,
        size,
    };
    use std::collections::HashSet;

    fn session(state: AgentState) -> AgentSession {
        AgentSession::new(TerminalAgent::ClaudeCode, state)
    }

    fn show(diffstat: bool) -> paneflow_config::schema::SidebarShow {
        paneflow_config::schema::SidebarShow {
            branch: Some(false),
            diffstat: Some(diffstat),
            pr: Some(false),
            indent_guide: Some(false),
        }
    }

    fn dirty() -> crate::workspace::GitDiffStats {
        crate::workspace::GitDiffStats {
            files_changed: 3,
            insertions: 142,
            deletions: 38,
        }
    }

    #[test]
    fn the_diffstat_needs_both_a_switch_and_something_to_report() {
        let clean = crate::workspace::GitDiffStats::default();
        assert!(
            !tab_diffstat_visible(show(false), &dirty()),
            "the counts are opt-in even on a dirty checkout"
        );
        assert!(
            !tab_diffstat_visible(show(true), &clean),
            "a clean checkout prints its branch and stops"
        );
        assert!(tab_diffstat_visible(show(true), &dirty()));
    }

    #[test]
    fn reorder_target_accounts_for_the_removed_source() {
        assert_eq!(reorder_target(0, 3), 2);
        assert_eq!(reorder_target(4, 1), 1);
        assert_eq!(reorder_target(2, 2), 2);
        assert_eq!(reorder_target(2, 3), 2);
    }

    #[test]
    fn drop_slots_sit_between_the_rendered_rows() {
        let rows = [
            SidebarRow::Folder(0),
            SidebarRow::Tab(0, 0),
            SidebarRow::Tab(0, 1),
            SidebarRow::Folder(1),
        ];
        let slots = sidebar_drop_slots(&rows, 2);

        assert_eq!(slots.len(), rows.len() + 1);
        assert_eq!(
            slots[0],
            SidebarDropSlot {
                tab: None,
                workspace: Some(0)
            }
        );
        assert_eq!(
            slots[1],
            SidebarDropSlot {
                tab: Some((0, 0)),
                workspace: None
            }
        );
        assert_eq!(
            slots[2],
            SidebarDropSlot {
                tab: Some((0, 1)),
                workspace: None
            }
        );
        assert_eq!(
            slots[3],
            SidebarDropSlot {
                tab: Some((0, 2)),
                workspace: Some(1)
            }
        );
        assert_eq!(
            slots[4],
            SidebarDropSlot {
                tab: Some((1, 0)),
                workspace: Some(2)
            }
        );
    }

    #[gpui::test]
    fn a_drop_divider_spans_the_rail_and_reaches_over_its_neighbors(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(div().h(px(30.)))
                    .child(
                        div()
                            .h(px(SIDEBAR_ROW_SPACING))
                            .flex_none()
                            .relative()
                            .child(
                                div()
                                    .absolute()
                                    .top(px(-SIDEBAR_DROP_BAND_REACH))
                                    .w_full()
                                    .px(px(SIDEBAR_ROW_MARGIN_X))
                                    .h(px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0))
                                    .flex()
                                    .flex_col()
                                    .justify_center()
                                    .debug_selector(|| "band".into())
                                    .child(
                                        div()
                                            .h(px(SIDEBAR_DROP_LINE_PX))
                                            .w_full()
                                            .bg(gpui::rgb(0xff0000))
                                            .debug_selector(|| "line".into()),
                                    ),
                            ),
                    )
            },
        );
        let band = cx.debug_bounds("band").expect("drop band not painted");
        let line = cx.debug_bounds("line").expect("drop line not painted");

        assert_eq!(band.size.width, px(SIDEBAR_WIDTH));
        assert_eq!(
            line.size.width,
            px(SIDEBAR_WIDTH - 2. * SIDEBAR_ROW_MARGIN_X)
        );
        assert_eq!(band.origin.y, px(30.) - px(SIDEBAR_DROP_BAND_REACH));
        assert_eq!(
            band.size.height,
            px(SIDEBAR_ROW_SPACING + SIDEBAR_DROP_BAND_REACH * 2.0)
        );
        assert_eq!(line.origin.y, px(30.) + px(SIDEBAR_ROW_SPACING / 2.0 - 1.0));
    }

    #[gpui::test]
    fn a_single_line_row_is_thirty_pixels_tall(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(SIDEBAR_WIDTH)),
                AvailableSpace::Definite(px(100.)),
            ),
            |_, _| {
                sidebar_row_shell()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .child(div().flex_none().size(px(SIDEBAR_FOLDER_ICON_WIDTH)))
                            .child(
                                div()
                                    .text_sm()
                                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                    .child("paneflow"),
                            ),
                    )
                    .debug_selector(|| "row".into())
            },
        );

        let bounds = cx.debug_bounds("row").expect("row not painted");
        assert_eq!(
            bounds.size.height,
            px(SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y),
            "a title line must not let the font's own line height set the row height"
        );
        assert_eq!(bounds.size.height, px(30.));
        assert!(
            ROW_RADIUS <= bounds.size.height / 2.,
            "row corner {ROW_RADIUS:?} exceeds half of a {:?} row",
            bounds.size.height
        );
    }

    #[gpui::test]
    fn sidebar_workspace_rows_keep_height_when_list_overflows(cx: &mut TestAppContext) {
        const ROWS: [&str; 8] = [
            "sidebar-row-0",
            "sidebar-row-1",
            "sidebar-row-2",
            "sidebar-row-3",
            "sidebar-row-4",
            "sidebar-row-5",
            "sidebar-row-6",
            "sidebar-row-7",
        ];

        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.), px(0.)),
            size(
                AvailableSpace::Definite(px(240.)),
                AvailableSpace::Definite(px(200.)),
            ),
            |_, _| {
                let mut list = div()
                    .w(px(240.))
                    .h(px(200.))
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .gap(px(4.));

                for selector in ROWS {
                    list = list.child(
                        sidebar_row_shell()
                            .child(div().h(px(20.)).flex_none())
                            .child(div().h(px(14.)).flex_none())
                            .debug_selector(move || selector.into()),
                    );
                }
                list
            },
        );

        for selector in ROWS {
            let bounds = cx
                .debug_bounds(selector)
                .unwrap_or_else(|| panic!("{selector} not painted"));
            assert_eq!(bounds.size.height, px(50.), "{selector}");
        }
    }

    #[test]
    fn sidebar_agent_summary_hides_idle_without_signal() {
        assert_eq!(sidebar_agent_summary(std::iter::empty(), false), None);
    }

    #[test]
    fn sidebar_agent_summary_counts_winning_needs_input_sessions() {
        let sessions = [
            session(AgentState::WaitingForInput),
            session(AgentState::Errored),
            session(AgentState::WaitingForInput),
        ];
        assert_eq!(
            sidebar_agent_summary(sessions.iter(), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 2
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_applies_sidebar_priority() {
        let cases = [
            (
                vec![AgentState::Finished, AgentState::Thinking],
                SidebarAgentState::Thinking,
            ),
            (
                vec![AgentState::Thinking, AgentState::Stalled],
                SidebarAgentState::Stalled,
            ),
            (
                vec![AgentState::Stalled, AgentState::Errored],
                SidebarAgentState::Errored,
            ),
            (
                vec![AgentState::Errored, AgentState::WaitingForInput],
                SidebarAgentState::NeedsInput,
            ),
        ];
        for (states, expected) in cases {
            let sessions: Vec<_> = states.into_iter().map(session).collect();
            assert_eq!(
                sidebar_agent_summary(sessions.iter(), false).map(|summary| summary.state),
                Some(expected)
            );
        }
    }

    #[test]
    fn sidebar_agent_summary_surfaces_unread_completion_without_live_session() {
        assert_eq!(
            sidebar_agent_summary(std::iter::empty(), true),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Finished,
                count: 1
            })
        );
    }

    #[test]
    fn sidebar_agent_summary_hides_acknowledged_finished_session() {
        let sessions = [session(AgentState::Finished)];
        assert_eq!(sidebar_agent_summary(sessions.iter(), false), None);
    }

    #[test]
    fn tab_display_title_falls_back_to_position() {
        let unnamed = Tab::new(String::new(), None);
        assert_eq!(tab_display_title(&unnamed, 0), "Tab 1");
        assert_eq!(tab_display_title(&unnamed, 4), "Tab 5");

        let blank = Tab::new("   ".to_string(), None);
        assert_eq!(tab_display_title(&blank, 1), "Tab 2");

        let named = Tab::new("build".to_string(), None);
        assert_eq!(tab_display_title(&named, 3), "build");
    }

    fn attributed_sessions() -> [AgentSession; 3] {
        let mut mine = session(AgentState::WaitingForInput);
        mine.surface_id = Some(11);
        let mut other_tab = session(AgentState::Errored);
        other_tab.surface_id = Some(22);
        let unattributed = session(AgentState::Thinking);
        [mine, other_tab, unattributed]
    }

    #[test]
    fn tab_row_speaks_only_for_the_sessions_of_its_own_surfaces() {
        let sessions = attributed_sessions();
        let surfaces = HashSet::from([11u64]);
        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &surfaces), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1
            }),
            "a tab must not inherit a sibling tab's session, nor an unattributed one"
        );

        assert_eq!(
            sidebar_agent_summary(tab_row_sessions(sessions.iter(), &HashSet::new()), false),
            None
        );
    }

    #[test]
    fn expanded_folder_keeps_only_the_unattributed_sessions() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), true), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::Thinking,
                count: 1
            })
        );

        let resolved = [attributed_sessions()[0].clone()];
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(resolved.iter(), true), false),
            None
        );
    }

    #[test]
    fn collapsed_folder_re_aggregates_every_tab() {
        let sessions = attributed_sessions();
        assert_eq!(
            sidebar_agent_summary(folder_row_sessions(sessions.iter(), false), false),
            Some(SidebarAgentSummary {
                state: SidebarAgentState::NeedsInput,
                count: 1
            })
        );
    }

    #[test]
    fn a_late_resolution_never_double_counts() {
        let mut sessions = attributed_sessions().to_vec();
        let surfaces = HashSet::from([11u64, 33u64]);
        let folder_before = folder_row_sessions(sessions.iter(), true).count();
        let tab_before = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_before, tab_before), (1, 1));

        sessions[2].surface_id = Some(33);
        let folder_after = folder_row_sessions(sessions.iter(), true).count();
        let tab_after = tab_row_sessions(sessions.iter(), &surfaces).count();
        assert_eq!((folder_after, tab_after), (0, 2));
        assert_eq!(folder_before + tab_before, folder_after + tab_after);
    }

    #[test]
    fn the_folder_action_lane_holds_its_button() {
        let cluster = SIDEBAR_ACTION_BUTTON_SIZE + SIDEBAR_TITLE_ROW_GAP;
        assert_eq!(
            SIDEBAR_ACTION_LANE_WIDTH, cluster,
            "the reserved lane no longer matches the cluster it holds"
        );
        let narrowest_cluster = SIDEBAR_TAB_ICON_SIZE + SIDEBAR_TITLE_ROW_GAP;
        assert!(
            narrowest_cluster >= SIDEBAR_ACTION_BUTTON_SIZE,
            "a tab row's close button now overhangs past its pane cluster onto the title"
        );
    }

    #[test]
    fn tab_card_fits_inside_a_row() {
        let row_height = SIDEBAR_ROW_LINE_HEIGHT + 2. * SIDEBAR_ROW_PADDING_Y;
        assert!(
            SIDEBAR_TAB_CARD_HEIGHT <= row_height,
            "a {SIDEBAR_TAB_CARD_HEIGHT}px card overflows a {row_height}px row and would be clipped"
        );
        assert!(
            SIDEBAR_TAB_CARD_ICON_SIZE + 8. <= SIDEBAR_TAB_CARD_WIDTH.min(SIDEBAR_TAB_CARD_HEIGHT),
            "a {SIDEBAR_TAB_CARD_ICON_SIZE}px glyph leaves under 4px of padding in the card"
        );
        for side in [SIDEBAR_TAB_CARD_WIDTH, SIDEBAR_TAB_CARD_HEIGHT] {
            let gap = side - SIDEBAR_TAB_CARD_ICON_SIZE;
            assert_eq!(
                gap % 2.,
                0.,
                "a {gap}px gap around the glyph centers it on a half pixel"
            );
        }
    }

    #[test]
    fn tab_icon_cluster_caps_at_four_panes() {
        assert_eq!(tab_icon_cluster_split(0), (0, 0));
        assert_eq!(tab_icon_cluster_split(1), (1, 0));
        assert_eq!(
            tab_icon_cluster_split(SIDEBAR_TAB_ICON_CAP),
            (SIDEBAR_TAB_ICON_CAP, 0)
        );
        assert_eq!(
            tab_icon_cluster_split(SIDEBAR_TAB_ICON_CAP + 3),
            (SIDEBAR_TAB_ICON_CAP, 3)
        );
    }
}
