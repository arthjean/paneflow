//! The diff dock's "new tab" menu: the popover the tab strip's `+` opens.
//!
//! Codex-app shape (the closest match to Paneflow's own chrome): a floating
//! rounded surface, one row per surface a tab can host, each carrying its icon,
//! its label and its shortcut hint. Cursor's Browser and Canvas rows have no
//! counterpart here, and its "Open any file, URL" search field belongs to a
//! command palette Paneflow does not have.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, MouseUpEvent,
    ParentElement, StatefulInteractiveElement, Styled, deferred, div, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_surface, select_item};

/// Width of the popover. Sized so the widest label and its shortcut hint sit on
/// one line with the gutter Codex leaves between the two columns.
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

/// The popover, deferred over the `+` trigger while open. Anchored to the
/// trigger's left edge so it unfolds into the dock rather than off its side.
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
        // Dismiss on release, not on press: `on_mouse_down_out` runs in the
        // capture phase, which would close the menu before a row's click can
        // bubble (same reasoning as the overflow menu next door).
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                this.close_diff_new_tab_menu(cx);
            }),
        )
        .child(
            menu_row(
                "diff-dock-new-tab-file",
                "icons/file-text.svg",
                "File",
                "secondary-g",
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
                "secondary-j",
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

/// One surface row: icon, label, then the shortcut hint pinned right. The
/// caller attaches the click, so each row states for itself what it opens.
/// `shortcut` is the binding as `keybindings::defaults` declares it, not a
/// pre-rendered label: `secondary-` resolves to Ctrl on Linux and Windows and
/// to Cmd on macOS, so the row has to be formatted here or it advertises a
/// chord that does not exist on one of the three platforms.
fn menu_row(
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    shortcut: &'static str,
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
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(crate::keybindings::format_keystroke(shortcut)),
        )
}
