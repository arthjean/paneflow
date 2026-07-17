//! Single Files-tree row render: indent + chevron + icon + name, with the
//! markdown/greyed styling (US-004), click-to-open / expand (US-003/004),
//! markdown drag-to-pane (US-008), and the right-click copy-path menu trigger
//! (US-009). Split out of `view.rs` to keep each file under the 250-line budget.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, div, prelude::*, px, svg,
};

use super::{DIMMED_OPACITY, INDENT_STEP, ROW_HEIGHT};
use crate::PaneFlowApp;
use crate::app::files_tree::{self, VisibleRowRef};
use crate::pane_drag::{MarkdownFileDrag, TabDragPreview};

impl PaneFlowApp {
    pub(super) fn files_row(
        &self,
        row: VisibleRowRef<'_>,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = row.node;
        let name: SharedString = files_tree::node_name(node).into();
        let is_md = !node.is_dir && files_tree::is_markdown(&node.path);
        let actionable = node.is_dir || is_md;
        let dimmed = node.is_ignored || node.is_hidden;
        // US-004: directories + markdown read at full text color; every other
        // file is greyed (muted). Ignored/hidden adds an opacity knock-down.
        let text_color = if actionable { ui.text } else { ui.muted };
        let indent = px(8. + row.depth as f32 * INDENT_STEP);
        let path = node.path.clone();
        let is_dir = node.is_dir;

        // Leading 14px slot: a chevron for directories (right = collapsed,
        // down = expanded - a static swap, legible under reduced motion), an
        // invisible spacer for files so names align.
        let chevron = if is_dir {
            svg()
                .size(px(14.))
                .flex_none()
                .path(if row.expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .text_color(ui.muted)
                .into_any_element()
        } else {
            div().size(px(14.)).flex_none().into_any_element()
        };

        let icon = if is_dir {
            if row.expanded {
                "icons/folder-open.svg"
            } else {
                "icons/folder.svg"
            }
        } else {
            "icons/file-text.svg"
        };

        let mut el = div()
            .id(SharedString::from(format!(
                "files-row-{}",
                node.path.display()
            )))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .h(ROW_HEIGHT)
            // Quiet-row geometry (matches the sessions sidebar): inset rounded
            // rows so the hover fill reads as a discrete pill, not a full-bleed
            // band.
            .mx(px(6.))
            .rounded(px(6.))
            .pl(indent)
            .pr(px(8.))
            .when(selected, |s| {
                s.bg(crate::app::constants::sidebar_tab_active_background())
            })
            .when(dimmed, |s| s.opacity(DIMMED_OPACITY));

        // US-009: right-click any row (markdown, greyed file, or directory) to
        // open the copy-path menu. Sits on the base row so it works regardless
        // of actionability.
        let menu_path = path.clone();
        el = el.on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
            if e.is_right_click()
                && let Some(position) = e.mouse_position()
            {
                this.dismiss_transient_surfaces();
                this.files_focus.focus(window, cx);
                this.select_files_row(&menu_path);
                this.files_menu_open = Some(crate::FilesContextMenu {
                    path: menu_path.clone(),
                    position,
                });
                cx.stop_propagation();
                cx.notify();
            }
        }));

        if actionable {
            // Whole row toggles a directory (US-003) / opens a markdown (US-004).
            el = el.hover(|s| s.bg(crate::app::constants::sidebar_tab_hover_background()));
            let click_path = path.clone();
            el = el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.files_focus.focus(window, cx);
                this.select_files_row(&click_path);
                if is_dir {
                    this.toggle_dir(&click_path, cx);
                } else {
                    this.open_markdown_in_active_pane(click_path.clone(), window, cx);
                }
                cx.stop_propagation();
            }));
        }

        // US-008: only markdown rows are draggable into a pane. The ghost reuses
        // the shared tab-drag preview; non-markdown rows have no drag path.
        if is_md {
            let drag = MarkdownFileDrag {
                path: path.clone(),
                title: name.clone(),
                icon: SharedString::from("icons/file-text.svg"),
            };
            el = el.on_drag(drag, |drag, _offset, _window, cx| {
                cx.new(|_| TabDragPreview {
                    title: drag.title.clone(),
                    icon: drag.icon.clone(),
                })
            });
        }

        el.child(chevron)
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path(icon)
                    .text_color(if is_md { ui.accent } else { ui.muted }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(text_color)
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .into_any_element()
    }
}
