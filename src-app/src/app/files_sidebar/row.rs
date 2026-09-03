use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, StyledText, div, img, prelude::*, px, svg,
};

use super::{DIMMED_OPACITY, INDENT_STEP, ROW_GAP, ROW_HEIGHT, ROW_SLOT};
use crate::PaneFlowApp;
use crate::app::files_tree::{self, VisibleRowRef};
use crate::app::sidebar::{SIDEBAR_ROW_LINE_HEIGHT, SIDEBAR_ROW_MARGIN_X, SIDEBAR_ROW_PADDING_X};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

pub(super) struct FilesRowLabel {
    pub text: SharedString,
    pub highlight: Option<Range<usize>>,
}

impl FilesRowLabel {
    pub(super) fn plain(text: SharedString) -> Self {
        Self {
            text,
            highlight: None,
        }
    }
}

impl PaneFlowApp {
    pub(super) fn files_row(
        &self,
        row: VisibleRowRef<'_>,
        label: FilesRowLabel,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = row.node;
        let refused = files_tree::editor_refuses(node);
        let dimmed = node.is_ignored || node.is_hidden;
        let text_color = if refused { ui.muted } else { ui.text };
        let indent = px(SIDEBAR_ROW_PADDING_X + row.depth as f32 * INDENT_STEP);
        let path = node.path.clone();
        let is_dir = node.is_dir;
        let group = SharedString::from(format!("files-row-group-{}", node.path.display()));
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
            img(crate::file_icons::language_icon_path(
                &files_tree::node_name(node),
            ))
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
            div().id(SharedString::from(format!(
                "files-row-{}",
                node.path.display()
            ))),
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
        .flex_none()
        .overflow_x_hidden()
        .mx(px(SIDEBAR_ROW_MARGIN_X))
        .pl(indent)
        .pr(px(SIDEBAR_ROW_PADDING_X))
        .when(dimmed, |s| s.opacity(DIMMED_OPACITY))
        .children(guides);

        let menu_path = path.clone();
        el = el.on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
            if e.is_right_click()
                && let Some(position) = e.mouse_position()
            {
                this.dismiss_transient_surfaces();
                this.files_focus.focus(window, cx);
                this.select_files_row(&menu_path, cx);
                this.files_menu_open = Some(crate::FilesContextMenu {
                    path: menu_path.clone(),
                    position,
                });
                cx.stop_propagation();
                cx.notify();
            }
        }));

        let click_path = path.clone();
        el = el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.files_focus.focus(window, cx);
            this.select_files_row(&click_path, cx);
            if is_dir {
                this.toggle_dir(&click_path, cx);
            } else {
                this.open_file_in_diff_dock(click_path.clone(), window, cx);
            }
            cx.stop_propagation();
        }));

        let name = match label.highlight {
            Some(range) => StyledText::new(label.text)
                .with_highlights([(
                    range,
                    HighlightStyle {
                        color: Some(ui.accent),
                        font_weight: Some(FontWeight::SEMIBOLD),
                        ..Default::default()
                    },
                )])
                .into_any_element(),
            None => label.text.into_any_element(),
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
