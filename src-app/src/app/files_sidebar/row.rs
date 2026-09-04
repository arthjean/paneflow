use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Styled, StyledText, div, img, prelude::*, px, svg,
};

use super::panel::{FilesEvent, FilesSidebar};
use super::projection::FileRow;
use super::{DIMMED_OPACITY, INDENT_STEP, ROW_GAP, ROW_HEIGHT, ROW_SLOT};
use crate::app::files_tree;
use crate::app::sidebar::{SIDEBAR_ROW_LINE_HEIGHT, SIDEBAR_ROW_PADDING_X};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

impl FilesSidebar {
    pub(super) fn files_row(
        &self,
        row: &FileRow,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = &row.node;
        let refused = files_tree::editor_refuses(node);
        let dimmed = node.is_ignored || node.is_hidden;
        let text_color = if refused { ui.muted } else { ui.text };
        let indent = px(SIDEBAR_ROW_PADDING_X + row.depth as f32 * INDENT_STEP);
        let path = node.path.clone();
        let is_dir = node.is_dir;
        let group = row.group.clone();
        let (resting, hovered) = if selected {
            (
                Some(crate::app::constants::sidebar_tab_active_background()),
                None,
            )
        } else {
            (
                None,
                Some(crate::app::constants::sidebar_tab_hover_background()),
            )
        };

        let slot = if is_dir {
            svg()
                .size(px(ROW_SLOT))
                .flex_none()
                .path(if row.expanded {
                    "icons/chevron-down.svg"
                } else {
                    "icons/chevron-right.svg"
                })
                .text_color(ui.muted)
                .into_any_element()
        } else {
            img(row.icon)
                .size(px(ROW_SLOT))
                .flex_none()
                .into_any_element()
        };

        let guide_color = ui.text.opacity(0.08);
        let guides = (0..row.depth)
            .map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(SIDEBAR_ROW_PADDING_X
                        + level as f32 * INDENT_STEP
                        + (ROW_SLOT / 2.).floor()))
                    .w(px(1.))
                    .bg(guide_color)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut el = squircle_skin(
            div().id(row.id.clone()),
            group,
            ROW_RADIUS,
            resting,
            hovered,
        )
        .flex()
        .flex_row()
        .items_center()
        .gap(px(ROW_GAP))
        .h(ROW_HEIGHT)
        .w_full()
        .flex_none()
        .overflow_x_hidden()
        .pl(indent)
        .pr(px(SIDEBAR_ROW_PADDING_X))
        .when(dimmed, |s| s.opacity(DIMMED_OPACITY))
        .children(guides);

        #[cfg(test)]
        {
            el = el.debug_selector(|| row.id.to_string());
        }

        let menu_path = path.clone();
        el = el.on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
            if this.active
                && e.is_right_click()
                && let Some(position) = e.mouse_position()
            {
                this.focus.focus(window, cx);
                this.select_path(&menu_path, cx);
                cx.emit(FilesEvent::ContextMenu(crate::FilesContextMenu {
                    root: this.tree.root.clone(),
                    path: menu_path.clone(),
                    position,
                }));
                cx.stop_propagation();
                cx.notify();
            }
        }));

        let click_path = path.clone();
        el = el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.focus.focus(window, cx);
            this.activate_path(&click_path, is_dir, window, cx);
            cx.stop_propagation();
        }));

        let name = match row.highlight.clone() {
            Some(range) => StyledText::new(row.label.clone())
                .with_highlights([(
                    range,
                    HighlightStyle {
                        color: Some(ui.accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )])
                .into_any_element(),
            None => row.label.clone().into_any_element(),
        };

        let el = el.child(slot).child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                .text_color(text_color)
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        );

        el.into_any_element()
    }
}
