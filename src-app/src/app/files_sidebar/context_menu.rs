//! Per-file right-click context menu for the Files sidebar (PRD
//! `prd-files-tree-sidebar-2026-Q3`, EP-003 US-009).
//!
//! Mirrors `render_workspace_context_menu` (`deferred().priority(3)`,
//! `occlude()`, `on_mouse_down_out` dismiss): a small two-item menu offering
//! "Copy path" (absolute) and "Copy relative path" (relative to the workspace
//! root) for any row - markdown, greyed file, or directory. Both write to the
//! clipboard and surface a confirmation toast.

use gpui::{
    AnyElement, ClickEvent, Context, IntoElement, MouseButton, ParentElement, Styled, deferred,
    div, prelude::*, px,
};

use crate::app::files_tree;
use crate::app::sidebar::context_menu::clamped_context_menu_position;
use crate::{FilesContextMenu, PaneFlowApp};

impl PaneFlowApp {
    pub(crate) fn render_files_context_menu(
        &self,
        menu: FilesContextMenu,
        ui: crate::theme::UiColors,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Two items × ~25px + 8px padding. Flip above the click when there
        // isn't room below (mirrors the workspace menu).
        let menu_height = px(66.);
        let menu_width = px(220.);
        let menu_pos =
            clamped_context_menu_position(menu.position, menu_width, menu_height, window);

        let abs_path = menu.path.clone();
        let rel_root = self.files_tree.root.clone();
        let rel_path = menu.path.clone();

        let context_menu = div()
            .id("files-context-menu")
            .occlude()
            .absolute()
            .left(menu_pos.x)
            .top(menu_pos.y)
            .w(menu_width)
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .rounded(px(8.))
            .flex()
            .flex_col()
            .p(px(4.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.files_menu_open = None;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.render_context_menu_item(
                "files-context-copy-path".into(),
                "Copy Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let value = abs_path.to_string_lossy().into_owned();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                    this.files_menu_open = None;
                    this.show_toast("Copied path", cx);
                    cx.stop_propagation();
                }),
            ))
            .child(self.render_context_menu_item(
                "files-context-copy-rel".into(),
                "Copy Relative Path",
                None,
                ui,
                cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    let value = files_tree::workspace_relative_path(&rel_root, &rel_path);
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(value));
                    this.files_menu_open = None;
                    this.show_toast("Copied relative path", cx);
                    cx.stop_propagation();
                }),
            ));

        deferred(context_menu).priority(3).into_any_element()
    }
}
