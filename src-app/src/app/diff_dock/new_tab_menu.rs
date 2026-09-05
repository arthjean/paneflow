use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, StatefulInteractiveElement, Styled, deferred, div, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_surface, select_item};

const MENU_WIDTH: f32 = 236.0;

impl PaneFlowApp {
    pub(crate) fn toggle_diff_new_tab_menu(&mut self, open: bool, cx: &mut Context<Self>) {
        self.diff_dock.diff_new_tab_menu_open = open;
        cx.notify();
    }

    pub(crate) fn close_diff_new_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.diff_dock.diff_new_tab_menu_open {
            self.diff_dock.diff_new_tab_menu_open = false;
            cx.notify();
        }
    }
}

pub(super) fn render_diff_new_tab_menu(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let menu = menu_surface(div().id("diff-dock-new-tab-menu"), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .w(px(MENU_WIDTH))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_diff_new_tab_menu(cx);
            }),
        )
        .child(
            menu_row(
                "diff-dock-new-tab-changes",
                "icons/plus-minus.svg",
                "Changes",
                None,
                ui,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.close_diff_new_tab_menu(cx);
                this.open_diff_changes_tab(cx);
            })),
        )
        .child(
            menu_row(
                "diff-dock-new-tab-file",
                "icons/file-text.svg",
                "File",
                Some("secondary-g"),
                ui,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_diff_new_tab_menu(cx);
                this.open_diff_file_picker(window, cx);
            })),
        )
        .child(
            menu_row(
                "diff-dock-new-tab-terminal",
                "icons/terminal.svg",
                "Terminal",
                Some("secondary-j"),
                ui,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                this.close_diff_new_tab_menu(cx);
                this.open_diff_terminal_tab(window, cx);
            })),
        );

    deferred(
        div()
            .absolute()
            .top(px(30.))
            .left(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(3)
    .into_any_element()
}

fn menu_row(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    shortcut: Option<&'static str>,
    ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    select_item(id, false, ui)
        .h(px(30.))
        .gap(px(9.))
        .child(
            svg()
                .size(px(15.))
                .flex_none()
                .path(icon)
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .whitespace_nowrap()
                .text_size(px(13.))
                .text_color(ui.text)
                .child(label),
        )
        .children(shortcut.map(|shortcut| {
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(crate::keybindings::format_keystroke(shortcut))
        }))
}
