use gpui::{App, MouseUpEvent, deferred};
use paneflow_config::schema::SidebarGroupBy;

use super::agents_section::{AgentGroupStatus, StatusGroupKind, render_status_group_header};
use super::customize_menu::menu_label;
use super::*;
use crate::settings::components::{floating_menu_panel, menu_row, menu_row_gap, menu_row_inset};

const NEW_TAB_MENU_WIDTH: f32 = 208.;
const NEW_TAB_MENU_MAX_HEIGHT: f32 = 320.;
const NEW_TAB_MENU_TOP: f32 = 32.;
const NEW_TAB_ICON: &str = "icons/plus.svg";
const NEW_TAB_WORKSPACE_ICON: &str = "icons/folder.svg";
const STATUS_ROWS_EMPTY_HEIGHT: f32 = 32.;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TabStatusGroup {
    pub(super) status: AgentGroupStatus,
    pub(super) tabs: Vec<(usize, usize)>,
}

pub(super) fn tab_group_status(summary: Option<SidebarAgentSummary>) -> AgentGroupStatus {
    match summary.map(|summary| summary.state) {
        Some(SidebarAgentState::NeedsInput) => AgentGroupStatus::NeedYou,
        Some(SidebarAgentState::Errored) => AgentGroupStatus::Error,
        Some(SidebarAgentState::Finished) => AgentGroupStatus::Done,
        Some(SidebarAgentState::Thinking) => AgentGroupStatus::Working,
        None => AgentGroupStatus::Idle,
    }
}

pub(super) fn group_tabs_by_status(
    tabs: impl IntoIterator<Item = (AgentGroupStatus, usize, usize)>,
) -> Vec<TabStatusGroup> {
    let tabs: Vec<_> = tabs.into_iter().collect();
    AgentGroupStatus::ORDER
        .into_iter()
        .filter_map(|status| {
            let members: Vec<(usize, usize)> = tabs
                .iter()
                .filter(|(held, _, _)| *held == status)
                .map(|&(_, workspace, tab)| (workspace, tab))
                .collect();
            (!members.is_empty()).then_some(TabStatusGroup {
                status,
                tabs: members,
            })
        })
        .collect()
}

pub(super) fn status_row_list_child(groups: &[TabStatusGroup], row: SidebarRow) -> Option<usize> {
    let SidebarRow::Tab(workspace, tab) = row else {
        return None;
    };
    let mut child = 0;
    for group in groups {
        child += 1;
        for &member in &group.tabs {
            if member == (workspace, tab) {
                return Some(child);
            }
            child += 1;
        }
    }
    None
}

fn tab_surfaces(tab: &Tab, cx: &App) -> std::collections::HashSet<u64> {
    tab.collect_panes()
        .iter()
        .flat_map(|pane| {
            pane.read(cx)
                .terminals()
                .map(|terminal| terminal.entity_id().as_u64())
                .collect::<Vec<_>>()
        })
        .collect()
}

pub(super) fn tab_row_workspace_suffix(workspace: &str) -> String {
    format!(" \u{b7} {workspace}")
}

impl PaneFlowApp {
    pub(crate) fn grouped_by_status(&self) -> bool {
        self.cached_config.resolved_sidebar_group_by() == SidebarGroupBy::Status
    }

    pub(super) fn agents_section_shown(&self) -> bool {
        self.cached_config.sidebar_show.agents_enabled() && !self.grouped_by_status()
    }

    pub(super) fn tab_status_groups(&self, query: &str, cx: &App) -> Vec<TabStatusGroup> {
        let order = self.sidebar_display_order();
        group_tabs_by_status(order.into_iter().flat_map(|ws_idx| {
            let ws = &self.workspaces[ws_idx];
            let tabs = if ws.is_empty_shell() {
                Vec::new()
            } else {
                sidebar_matching_tabs(ws, query).unwrap_or_default()
            };
            tabs.into_iter().map(move |tab_idx| {
                let surfaces = tab_surfaces(&ws.tabs()[tab_idx], cx);
                let summary = tab_row_agent_summary(TreeStatus::Shown, ws, &surfaces);
                (tab_group_status(summary), ws_idx, tab_idx)
            })
        }))
    }

    pub(super) fn status_row_child(&self, row: SidebarRow, cx: &App) -> Option<usize> {
        let query = self
            .sidebar_filter_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        status_row_list_child(&self.tab_status_groups(&query, cx), row)
    }

    pub(super) fn render_status_rows(
        &self,
        mut list: gpui::Stateful<gpui::Div>,
        query: &str,
        ui: crate::theme::UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let groups = self.tab_status_groups(query, cx);
        let keyboard_focused = self.sidebar_focus.is_focused(window);
        let workspaces_shown = self
            .workspaces
            .iter()
            .any(|ws| crate::app::spaces::in_space(ws, self.active_space.as_ref()));
        if groups.is_empty() && !query.is_empty() {
            return list.child(sidebar_no_match_message(query, ui));
        }
        if groups.is_empty() && workspaces_shown {
            return list.child(
                div()
                    .flex_none()
                    .h(px(STATUS_ROWS_EMPTY_HEIGHT))
                    .flex()
                    .items_center()
                    .px(px(SIDEBAR_ROW_MARGIN_X + SIDEBAR_ROW_PADDING_X))
                    .text_size(crate::ui_primitives::BODY_EMPHASIS)
                    .text_color(ui.muted)
                    .child("No tabs open"),
            );
        }
        for (group_index, group) in groups.iter().enumerate() {
            list = list.child(render_status_group_header(
                group.status,
                group.tabs.len(),
                StatusGroupKind::Tabs,
                group_index == 0,
                ui,
            ));
            for (index, &(ws_idx, tab_idx)) in group.tabs.iter().enumerate() {
                let cursor = keyboard_focused
                    && self.sidebar_cursor_matches(SidebarRow::Tab(ws_idx, tab_idx));
                list = list.child(
                    div()
                        .flex_none()
                        .w_full()
                        .when(index > 0, |row| row.mt(px(SIDEBAR_ROW_SPACING)))
                        .child(self.render_tab_row(ws_idx, tab_idx, cursor, ui, cx)),
                );
            }
        }
        list
    }

    fn close_sidebar_new_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_new_tab_menu_open {
            self.sidebar_new_tab_menu_open = false;
            cx.notify();
        }
    }

    fn open_new_tab_in(&mut self, ws_id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_new_tab_menu_open = false;
        if let Some(ws_idx) = self.workspaces.iter().position(|ws| ws.id == ws_id) {
            self.open_pane_palette(ws_idx, window, cx);
        }
        cx.notify();
    }

    fn press_sidebar_new_tab(
        &mut self,
        was_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.sidebar_display_order();
        match targets.as_slice() {
            [] => self.create_workspace_with_picker(window, cx),
            [only] => {
                let ws_id = self.workspaces[*only].id;
                self.open_new_tab_in(ws_id, window, cx);
            }
            _ => self.sidebar_new_tab_menu_open = !was_open,
        }
        cx.notify();
    }

    pub(super) fn render_sidebar_new_tab_button(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.sidebar_new_tab_menu_open;
        let hover = crate::app::constants::sidebar_tab_hover_background();
        squircle_skin(
            div()
                .id("sidebar-new-tab")
                .flex_none()
                .size(px(SIDEBAR_ACTION_BUTTON_SIZE))
                .flex()
                .items_center()
                .justify_center(),
            "sidebar-new-tab-group",
            px(7.),
            open.then_some(hover),
            (!open).then_some(hover),
        )
        .role(Role::Button)
        .aria_label("New tab")
        .aria_expanded(open)
        .delayed_tooltip(crate::ui_primitives::text_tooltip("New tab"))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.press_sidebar_new_tab(open, window, cx);
        }))
        .child(
            svg()
                .size(px(SIDEBAR_HEADER_ICON_WIDTH))
                .flex_none()
                .path(NEW_TAB_ICON)
                .text_color(if open { ui.text } else { ui.muted })
                .group_hover("sidebar-new-tab-group", move |style| {
                    style.text_color(ui.text)
                }),
        )
        .when(open, |trigger| {
            trigger.child(self.render_sidebar_new_tab_menu(ui, cx))
        })
        .into_any_element()
    }

    fn render_sidebar_new_tab_menu(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows: Vec<AnyElement> = self
            .sidebar_display_order()
            .into_iter()
            .map(|ws_idx| {
                let ws = &self.workspaces[ws_idx];
                let ws_id = ws.id;
                let title = SharedString::from(ws.title.clone());
                menu_row(
                    SharedString::from(format!("sidebar-new-tab-in-{ws_id}")),
                    false,
                    ui,
                )
                .role(Role::MenuItem)
                .aria_label(SharedString::from(format!("New tab in {title}")))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_new_tab_in(ws_id, window, cx);
                    cx.stop_propagation();
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path(NEW_TAB_WORKSPACE_ICON)
                        .text_color(ui.muted),
                )
                .child(menu_label(title, ui))
                .into_any_element()
            })
            .collect();
        let menu = floating_menu_panel(div().id("sidebar-new-tab-menu"), ui)
            .font_family("Geist")
            .w(px(NEW_TAB_MENU_WIDTH))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                    this.close_sidebar_new_tab_menu(cx);
                }),
            )
            .child(
                div()
                    .flex_none()
                    .px(menu_row_inset())
                    .pt(px(4.))
                    .pb(px(2.))
                    .text_size(crate::ui_primitives::BODY)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.muted)
                    .child("New tab in"),
            )
            .child(
                div()
                    .id("sidebar-new-tab-workspaces")
                    .flex()
                    .flex_col()
                    .gap(menu_row_gap())
                    .max_h(px(NEW_TAB_MENU_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .children(rows),
            );

        deferred(crate::ui_primitives::menu_reveal(
            "sidebar-new-tab-menu-reveal",
            div()
                .absolute()
                .top(px(NEW_TAB_MENU_TOP))
                .right(px(0.))
                .occlude()
                .child(menu),
        ))
        .with_priority(3)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(state: SidebarAgentState) -> Option<SidebarAgentSummary> {
        Some(SidebarAgentSummary {
            state,
            count: 1,
            tint: None,
        })
    }

    #[test]
    fn a_tab_takes_the_group_of_its_most_urgent_agent_and_idle_without_one() {
        assert_eq!(
            [
                tab_group_status(summary(SidebarAgentState::NeedsInput)),
                tab_group_status(summary(SidebarAgentState::Errored)),
                tab_group_status(summary(SidebarAgentState::Finished)),
                tab_group_status(summary(SidebarAgentState::Thinking)),
                tab_group_status(None),
            ],
            [
                AgentGroupStatus::NeedYou,
                AgentGroupStatus::Error,
                AgentGroupStatus::Done,
                AgentGroupStatus::Working,
                AgentGroupStatus::Idle,
            ]
        );
    }

    #[test]
    fn tabs_gather_by_status_in_the_agents_order_and_keep_their_workspace_order() {
        let groups = group_tabs_by_status([
            (AgentGroupStatus::Idle, 0, 0),
            (AgentGroupStatus::Working, 0, 1),
            (AgentGroupStatus::Idle, 1, 0),
            (AgentGroupStatus::NeedYou, 2, 3),
            (AgentGroupStatus::Working, 2, 0),
        ]);
        assert_eq!(
            groups,
            [
                TabStatusGroup {
                    status: AgentGroupStatus::NeedYou,
                    tabs: vec![(2, 3)],
                },
                TabStatusGroup {
                    status: AgentGroupStatus::Working,
                    tabs: vec![(0, 1), (2, 0)],
                },
                TabStatusGroup {
                    status: AgentGroupStatus::Idle,
                    tabs: vec![(0, 0), (1, 0)],
                },
            ]
        );
        assert!(group_tabs_by_status([]).is_empty());
    }

    #[test]
    fn a_row_is_located_among_the_list_children_by_counting_the_status_headers() {
        let groups = group_tabs_by_status([
            (AgentGroupStatus::NeedYou, 2, 3),
            (AgentGroupStatus::Idle, 0, 0),
            (AgentGroupStatus::Idle, 1, 0),
        ]);
        assert_eq!(
            status_row_list_child(&groups, SidebarRow::Tab(2, 3)),
            Some(1)
        );
        assert_eq!(
            status_row_list_child(&groups, SidebarRow::Tab(0, 0)),
            Some(3)
        );
        assert_eq!(
            status_row_list_child(&groups, SidebarRow::Tab(1, 0)),
            Some(4)
        );
        assert_eq!(status_row_list_child(&groups, SidebarRow::Tab(9, 9)), None);
        assert_eq!(status_row_list_child(&groups, SidebarRow::Folder(0)), None);
    }

    #[test]
    fn a_tab_row_names_its_workspace_after_a_middle_dot_as_an_agent_row_does() {
        assert_eq!(tab_row_workspace_suffix("paneflow"), " \u{b7} paneflow");
    }

    #[test]
    fn the_new_tab_button_and_its_menu_point_at_embedded_icons() {
        for icon in [NEW_TAB_ICON, NEW_TAB_WORKSPACE_ICON] {
            assert!(
                crate::assets::Assets::get(icon).is_some(),
                "missing icon {icon}"
            );
        }
    }
}
