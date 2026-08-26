//! The diff dock's overflow menu: the `...` header button and the popover it
//! opens (layout, expand/collapse, refresh).
//!
//! Modeled on Cursor's changes-panel menu: a "Layout" row carrying the current
//! value and opening a side submenu, a divider, then the plain actions. Only the
//! dock's real capabilities are listed - Cursor's whitespace/word-wrap toggles
//! and in-diff search have no counterpart here.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, StatefulInteractiveElement, Styled, deferred, div, prelude::FluentBuilder, px,
    svg,
};

use super::model::DiffChrome;
use crate::PaneFlowApp;
use crate::settings::components::{menu_divider_color, menu_surface, select_item};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

/// Width of the overflow popover. Sized so "Refresh Changes" and the "Layout /
/// Split" pair breathe without the menu overhanging a narrow dock.
const MENU_WIDTH: f32 = 232.0;

impl PaneFlowApp {
    /// Drive the dock's `...` menu to `open`. Closing it also folds the layout
    /// submenu, so reopening always starts collapsed.
    fn set_diff_options_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.diff_dock.diff_options_menu_open = open;
        if !open {
            self.diff_dock.diff_layout_submenu_open = false;
        }
        cx.notify();
    }

    pub(crate) fn close_diff_options_menu(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.diff_options_menu_open {
            self.diff_dock.diff_options_menu_open = false;
            self.diff_dock.diff_layout_submenu_open = false;
            cx.notify();
        }
    }
}

/// The header's `...` trigger, with the popover deferred over it while open.
pub(super) fn render_diff_options_button(
    chrome: &DiffChrome<'_>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let open = chrome.options_open;
    // Same skin as the sidebar's rail actions (28 px box, `ROW_RADIUS`
    // superellipse, rail hover tint); while the menu is up the hover fill is
    // pinned on as the resting fill so the trigger stays lit.
    let hover = crate::app::constants::sidebar_tab_hover_background();

    squircle_skin(
        div()
            .id("diff-dock-options")
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center(),
        "diff-dock-options-group",
        ROW_RADIUS,
        open.then_some(hover),
        Some(hover),
    )
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    // Toggle off the render-time `open` snapshot, not the live state: while
    // the menu is up, its `on_mouse_up_out` fires on this same release and has
    // already cleared the flag, so a live toggle would re-open it and a second
    // press on the trigger could never close the menu.
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.set_diff_options_menu(!open, cx);
    }))
    .child(
        svg()
            .size(px(16.))
            .flex_none()
            .path("icons/dots.svg")
            .text_color(ui.muted),
    )
    .when(open, |trigger| {
        trigger.child(render_diff_options_menu(chrome, ui, cx))
    })
    .into_any_element()
}

fn render_diff_options_menu(
    chrome: &DiffChrome<'_>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome
        .data
        .as_ref()
        .filter(|d| !d.loading && d.error.is_none() && d.file_count > 0);
    let submenu_open = chrome.layout_submenu_open;
    let cwd = chrome.cwd.clone();

    // Built on `menu_surface` rather than `select_menu`: the latter bakes in an
    // `overflow_y_scroll` host, and the layout submenu has to fly out of the
    // menu's own box.
    let mut menu = menu_surface(div().id("diff-dock-options-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Dismiss on release, not on press: `on_mouse_down_out` runs in the
        // capture phase (no bubble `stop_propagation` can hold it back), and the
        // layout submenu flies out of this menu's own bounds - so a press on a
        // submenu row read as "outside", closed the menu, and the row's click
        // died with the frame. Closing on mouse-up keeps that click alive,
        // because the capture close and the bubble click share one event.
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_diff_options_menu(cx);
            }),
        )
        .child(render_layout_row(chrome.split, submenu_open, ui, cx));

    menu = menu.child(
        div()
            .flex_none()
            .h(px(1.))
            .my(px(4.))
            .mx(px(4.))
            .bg(menu_divider_color(ui)),
    );

    if let Some(data) = loaded {
        let all_collapsed = data.all_collapsed(chrome.collapsed);
        let label = if all_collapsed {
            "Expand All"
        } else {
            "Collapse All"
        };
        let next_collapse = !all_collapsed;
        let paths = data.paths();
        menu = menu.child(
            select_item("diff-dock-options-collapse", false, ui)
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.set_all_diff_collapsed(&paths, next_collapse, cx);
                    this.close_diff_options_menu(cx);
                }))
                .child(menu_label(label, ui)),
        );
    }

    menu = menu.child(
        select_item("diff-dock-options-refresh", false, ui)
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.refresh_diff_dock(cwd.clone(), cx);
                this.close_diff_options_menu(cx);
            }))
            .child(menu_label("Refresh Changes", ui)),
    );

    deferred(
        div()
            .absolute()
            .top(px(32.))
            .right(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

/// "Layout  <value> >": the current mode on the right, a chevron opening the
/// side submenu. The submenu anchors to the row so it flies out leftward,
/// clearing the dock's right edge.
fn render_layout_row(
    split: bool,
    submenu_open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let value = if split { "Split" } else { "Unified" };

    select_item("diff-dock-options-layout", submenu_open, ui)
        .relative()
        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
            this.diff_dock.diff_layout_submenu_open = !this.diff_dock.diff_layout_submenu_open;
            cx.notify();
        }))
        .child(menu_label("Layout", ui))
        .child(
            div()
                .flex_none()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(value),
        )
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/chevron-right.svg")
                .text_color(ui.muted),
        )
        .when(submenu_open, |row| {
            row.child(render_layout_submenu(split, ui, cx))
        })
        .into_any_element()
}

fn render_layout_submenu(
    split: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_surface(div().id("diff-dock-layout-submenu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(150.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(render_layout_option("Split", true, split, ui, cx))
        .child(render_layout_option("Unified", false, !split, ui, cx));

    deferred(
        div()
            .absolute()
            .top(px(-5.))
            .right(px(MENU_WIDTH - 12.))
            .occlude()
            .child(menu),
    )
    .with_priority(4)
    .into_any_element()
}

fn render_layout_option(
    label: &'static str,
    split: bool,
    selected: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    select_item(
        if split {
            "diff-dock-layout-split"
        } else {
            "diff-dock-layout-unified"
        },
        selected,
        ui,
    )
    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
        this.set_diff_dock_split(split, cx);
        this.close_diff_options_menu(cx);
    }))
    .child(menu_label(label, ui))
    .child(div().w(px(14.)).flex_none().child(if selected {
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
