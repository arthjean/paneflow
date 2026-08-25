use crate::PaneFlowApp;
use crate::theme::UiColors;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::{AnimatedHover, AnimatedHoverExt};
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, StatefulInteractiveElement, Styled, div, prelude::*, px, svg,
};

use super::REVIEW_SIDEBAR_ROW_RADIUS;

fn header_icon_button(
    id: &'static str,
    icon: &'static str,
    tooltip: &'static str,
    icon_color: Hsla,
    row_background: Hsla,
) -> AnimatedHover {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(tooltip)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(REVIEW_SIDEBAR_ROW_RADIUS))
        .animated_hover_bg(row_background.opacity(0.0), row_background)
        .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(icon_color),
        )
}

impl PaneFlowApp {
    pub(super) fn render_diff_files_header(
        &self,
        ui: UiColors,
        collapsed: bool,
        total_added: u32,
        total_removed: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_background = crate::app::constants::sidebar_tab_active_background();
        let collapse_tooltip = if collapsed {
            "Expand changes"
        } else {
            "Collapse changes"
        };
        let tree_tooltip = if self.diff_mode.diff_files_tree {
            "Show flat list"
        } else {
            "Show file tree"
        };
        let tree_icon = if self.diff_mode.diff_files_tree {
            "icons/list.svg"
        } else {
            "icons/file_tree.svg"
        };
        let collapse_icon = if collapsed {
            "icons/chevron-right.svg"
        } else {
            "icons/chevron-down.svg"
        };

        div()
            .id("diff-files-header")
            .flex_none()
            .h(px(48.))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.diff_mode.diff_files_collapsed = !this.diff_mode.diff_files_collapsed;
                cx.notify();
            }))
            .child(
                div()
                    .pl(px(8.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Changes"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .when(total_added > 0 || total_removed > 0, |d| {
                        d.child(
                            div()
                                .flex_none()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(4.))
                                .mr(px(4.))
                                .text_size(crate::ui_primitives::LABEL_SM)
                                .when(total_added > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(ui.diff_colors().added)
                                            .child(format!("+{total_added}")),
                                    )
                                })
                                .when(total_removed > 0, |d| {
                                    d.child(
                                        div()
                                            .text_color(ui.diff_colors().deleted)
                                            .child(format!("-{total_removed}")),
                                    )
                                }),
                        )
                    })
                    .child(
                        header_icon_button(
                            "diff-files-tree-toggle",
                            tree_icon,
                            tree_tooltip,
                            ui.muted,
                            row_background,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.diff_mode.diff_files_tree = !this.diff_mode.diff_files_tree;
                                cx.stop_propagation();
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        header_icon_button(
                            "diff-files-collapse-toggle",
                            collapse_icon,
                            collapse_tooltip,
                            ui.muted,
                            row_background,
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.diff_mode.diff_files_collapsed =
                                    !this.diff_mode.diff_files_collapsed;
                                cx.stop_propagation();
                                cx.notify();
                            },
                        )),
                    ),
            )
            .into_any_element()
    }
}
