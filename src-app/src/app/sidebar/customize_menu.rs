use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, deferred, div,
    prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_panel, menu_row};
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::squircle_skin;

use paneflow_config::schema::SidebarShow;

#[derive(Clone, Copy)]
pub(super) struct CustomizeMenuState {
    pub open: bool,
    pub submenu_open: bool,
    pub show: SidebarShow,
    pub all_expanded: Option<bool>,
}

const MENU_WIDTH: f32 = 208.0;
const SUBMENU_WIDTH: f32 = 156.0;

impl PaneFlowApp {
    pub(crate) fn close_sidebar_customize_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_customize_menu_open {
            self.sidebar_customize_menu_open = false;
            self.sidebar_show_submenu_open = false;
            cx.notify();
        }
    }

    fn toggle_sidebar_show(&mut self, line: SidebarShowLine, cx: &mut Context<Self>) {
        self.sidebar_customize_menu_open = true;
        self.sidebar_show_submenu_open = true;

        let mut show = self.cached_config.sidebar_show;
        match line {
            SidebarShowLine::Branch => show.branch = Some(!show.branch_enabled()),
            SidebarShowLine::Diffstat => show.diffstat = Some(!show.diffstat_enabled()),
            SidebarShowLine::Pr => show.pr = Some(!show.pr_enabled()),
            SidebarShowLine::IndentGuide => {
                show.indent_guide = Some(!show.indent_guide_enabled());
            }
            SidebarShowLine::Agents => show.agents = Some(!show.agents_enabled()),
        }
        let value = sidebar_show_value(show);
        self.persist_setting(false, "sidebar_show", value, cx);
        self.refresh_pull_requests(cx);
        cx.notify();
    }
}

fn sidebar_show_value(show: SidebarShow) -> serde_json::Value {
    serde_json::json!({
        "branch": show.branch_enabled(),
        "diffstat": show.diffstat_enabled(),
        "pr": show.pr_enabled(),
        "indent_guide": show.indent_guide_enabled(),
        "agents": show.agents_enabled(),
    })
}

#[derive(Clone, Copy)]
enum SidebarShowLine {
    Branch,
    Diffstat,
    Pr,
    IndentGuide,
    Agents,
}

pub(super) fn render_customize_sidebar_button(
    state: CustomizeMenuState,
    icon_size: f32,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let open = state.open;
    let hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("sidebar-customize")
            .flex_none()
            .size(px(22.))
            .flex()
            .items_center()
            .justify_center(),
        "sidebar-customize-group",
        px(7.),
        open.then_some(hover),
        (!open).then_some(hover),
    )
    .role(Role::Button)
    .aria_label("Customize sidebar")
    .delayed_tooltip(crate::ui_primitives::text_tooltip("Customize sidebar"))
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.sidebar_customize_menu_open = !open;
        if open {
            this.sidebar_show_submenu_open = false;
        }
        cx.notify();
    }))
    .child(
        svg()
            .size(px(icon_size))
            .flex_none()
            .path("icons/filter-2.svg")
            .text_color(ui.muted),
    )
    .when(open, |trigger| trigger.child(render_menu(state, ui, cx)))
    .into_any_element()
}

fn render_menu(
    state: CustomizeMenuState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_panel(div().id("sidebar-customize-menu"), ui)
        .font_family("Geist")
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_sidebar_customize_menu(cx);
            }),
        )
        .child(render_show_row(state, ui, cx))
        .when_some(state.all_expanded, |menu, all_expanded| {
            menu.child(menu_divider(ui))
                .child(render_expand_all_row(all_expanded, ui, cx))
        });

    deferred(crate::ui_primitives::menu_reveal(
        "sidebar-customize-menu-reveal",
        div()
            .absolute()
            .top(px(32.))
            .left(px(0.))
            .occlude()
            .child(menu),
    ))
    .with_priority(3)
    .into_any_element()
}

impl PaneFlowApp {
    fn set_all_workspaces_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.sidebar_customize_menu_open = true;
        let space = self.active_space.clone();
        for ws in self
            .workspaces
            .iter_mut()
            .filter(|ws| crate::app::spaces::in_space(ws, space.as_ref()))
        {
            ws.sidebar_expanded = expanded;
        }
        self.save_session(cx);
        cx.notify();
    }
}

fn render_expand_all_row(
    all_expanded: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    menu_row("sidebar-customize-expand-all", false, ui)
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.set_all_workspaces_expanded(!all_expanded, cx);
        }))
        .child(menu_label(
            if all_expanded {
                "Collapse all"
            } else {
                "Expand all"
            },
            ui,
        ))
        .into_any_element()
}

fn render_show_row(
    state: CustomizeMenuState,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let submenu_open = state.submenu_open;
    menu_row("sidebar-customize-show", submenu_open, ui)
        .relative()
        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.sidebar_show_submenu_open = !this.sidebar_show_submenu_open;
            cx.notify();
        }))
        .child(menu_label("Show", ui))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/chevron-right.svg")
                .text_color(ui.muted),
        )
        .when(submenu_open, |row| {
            row.child(render_show_submenu(state.show, ui, cx))
        })
        .into_any_element()
}

fn render_show_submenu(
    show: SidebarShow,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_panel(div().id("sidebar-show-submenu"), ui)
        .font_family("Geist")
        .w(px(SUBMENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(render_show_option(
            "Branch",
            "icons/git-branch-sidebar.svg",
            SidebarShowLine::Branch,
            show.branch_enabled(),
            ui,
            cx,
        ))
        .child(render_show_option(
            "Diffstat",
            "icons/diff-unified.svg",
            SidebarShowLine::Diffstat,
            show.diffstat_enabled(),
            ui,
            cx,
        ))
        .child(render_show_option(
            "PR",
            "icons/git-pull-request.svg",
            SidebarShowLine::Pr,
            show.pr_enabled(),
            ui,
            cx,
        ))
        .child(render_show_option(
            "Indent guide",
            "icons/list.svg",
            SidebarShowLine::IndentGuide,
            show.indent_guide_enabled(),
            ui,
            cx,
        ))
        .child(menu_divider(ui))
        .child(render_show_option(
            "Agents",
            "icons/sparkles.svg",
            SidebarShowLine::Agents,
            show.agents_enabled(),
            ui,
            cx,
        ));

    deferred(crate::ui_primitives::menu_reveal(
        "sidebar-show-submenu-reveal",
        div()
            .absolute()
            .top(px(-5.))
            .left(px(MENU_WIDTH - 12.))
            .occlude()
            .child(menu),
    ))
    .with_priority(4)
    .into_any_element()
}

fn render_show_option(
    label: &'static str,
    icon: &'static str,
    line: SidebarShowLine,
    checked: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let id = match line {
        SidebarShowLine::Branch => "sidebar-show-branch",
        SidebarShowLine::Diffstat => "sidebar-show-diffstat",
        SidebarShowLine::Pr => "sidebar-show-pr",
        SidebarShowLine::IndentGuide => "sidebar-show-indent-guide",
        SidebarShowLine::Agents => "sidebar-show-agents",
    };

    menu_row(id, false, ui)
        .role(Role::MenuItemCheckBox)
        .aria_label(label)
        .aria_toggled(if checked {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_sidebar_show(line, cx);
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(ui.muted),
        )
        .child(menu_label(label, ui))
        .child(div().w(px(14.)).flex_none().child(if checked {
            svg()
                .size(px(13.))
                .path("icons/check.svg")
                .text_color(ui.text)
                .into_any_element()
        } else {
            div().size(px(13.)).into_any_element()
        }))
        .into_any_element()
}

fn menu_divider(ui: crate::theme::UiColors) -> gpui::Div {
    div()
        .mx(px(6.))
        .my(px(4.))
        .h(px(1.))
        .bg(crate::settings::components::menu_divider_color(ui))
}

pub(super) fn menu_label(label: impl Into<SharedString>, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(crate::ui_primitives::BODY)
        .text_color(ui.text)
        .child(Into::<SharedString>::into(label))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::PaneFlowConfig;

    #[test]
    fn the_show_object_carries_every_line_including_agents() {
        let value = sidebar_show_value(SidebarShow::default());
        assert_eq!(
            value,
            serde_json::json!({
                "branch": false,
                "diffstat": false,
                "pr": false,
                "indent_guide": false,
                "agents": true,
            })
        );
    }

    #[test]
    fn hiding_the_agents_section_round_trips_through_the_config_writer() {
        let config = PaneFlowConfig::default();
        assert!(config.sidebar_show.agents_enabled());
        let mut show = config.sidebar_show;
        show.agents = Some(!show.agents_enabled());

        let next = crate::config_writer::with_field(
            &config,
            false,
            "sidebar_show",
            sidebar_show_value(show),
        )
        .expect("written");

        assert!(!next.sidebar_show.agents_enabled());
        assert_eq!(next.sidebar_show.indent_guide, Some(false));
    }
}
