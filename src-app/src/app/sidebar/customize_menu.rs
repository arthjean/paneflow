use gpui::{
    AnyElement, AppContext, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton,
    MouseUpEvent, ParentElement, StatefulInteractiveElement, Styled, deferred, div,
    prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_surface, select_item};
use crate::ui_primitives::{ROW_RADIUS, TooltipDelayExt, squircle_skin};

use super::SidebarTooltip;
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
        }
        let value = serde_json::json!({
            "branch": show.branch_enabled(),
            "diffstat": show.diffstat_enabled(),
            "pr": show.pr_enabled(),
            "indent_guide": show.indent_guide_enabled(),
        });
        if !crate::config_writer::save_config_value_checked("sidebar_show", value) {
            self.show_toast("Could not save the sidebar setting", cx);
            return;
        }
        self.cached_config.sidebar_show = show;
        self.refresh_pull_requests(cx);
        cx.notify();
    }
}

#[derive(Clone, Copy)]
enum SidebarShowLine {
    Branch,
    Diffstat,
    Pr,
    IndentGuide,
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
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center(),
        "sidebar-customize-group",
        ROW_RADIUS,
        open.then_some(hover),
        Some(hover),
    )
    .delayed_tooltip(|_w, cx| {
        cx.new(|_| SidebarTooltip {
            label: "Customize Sidebar".into(),
        })
        .into()
    })
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
    let menu = menu_surface(div().id("sidebar-customize-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
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
            menu.child(
                div()
                    .mx(px(6.))
                    .my(px(4.))
                    .h(px(1.))
                    .bg(crate::settings::components::menu_divider_color(ui)),
            )
            .child(render_expand_all_row(all_expanded, ui, cx))
        });

    deferred(
        div()
            .absolute()
            .top(px(32.))
            .left(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

impl PaneFlowApp {
    fn set_all_workspaces_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        self.sidebar_customize_menu_open = true;
        for ws in &mut self.workspaces {
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
    select_item("sidebar-customize-expand-all", false, ui)
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
    select_item("sidebar-customize-show", submenu_open, ui)
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
    let menu = menu_surface(div().id("sidebar-show-submenu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
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
        ));

    deferred(
        div()
            .absolute()
            .top(px(-5.))
            .left(px(MENU_WIDTH - 12.))
            .occlude()
            .child(menu),
    )
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
    };

    select_item(id, false, ui)
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

fn menu_label(label: &'static str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .whitespace_nowrap()
        .text_size(px(13.))
        .text_color(ui.text)
        .child(label)
        .into_any_element()
}
