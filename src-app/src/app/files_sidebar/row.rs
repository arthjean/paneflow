//! Single Files-tree row render: indent + chevron + icon + name, with the
//! editor-refusal styling (US-019), click-to-open / expand (US-003/004),
//! markdown drag-to-pane (US-008), and the right-click copy-path menu trigger
//! (US-009). Split out of `view.rs` to keep each file under the 250-line budget.

use std::ops::Range;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, SharedString, Styled, StyledText, div, img, prelude::*, px, svg,
};

use super::{DIMMED_OPACITY, INDENT_STEP, ROW_GAP, ROW_HEIGHT, ROW_SLOT};
use crate::PaneFlowApp;
use crate::app::files_tree::{self, VisibleRowRef};
use crate::app::sidebar::{SIDEBAR_ROW_LINE_HEIGHT, SIDEBAR_ROW_MARGIN_X, SIDEBAR_ROW_PADDING_X};
use crate::pane_drag::{DragPreview, MarkdownFileDrag};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

/// What a row prints on its name line. In tree mode this is the node's own file
/// name with no highlight; under the US-020 filter it is the workspace-relative
/// path with the matched byte range picked out.
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
        let is_md = !node.is_dir && files_tree::is_markdown(&node.path);
        // US-019: the markdown lock is gone - every file is clickable and read
        // at full text color. The one remaining muted tier is a file the editor
        // would refuse (binary extension or over `MAX_FILE_BYTES`); it stays
        // clickable so opening it surfaces the US-003 error inside the tab.
        let refused = files_tree::editor_refuses(node);
        let dimmed = node.is_ignored || node.is_hidden;
        let text_color = if refused { ui.muted } else { ui.text };
        let indent = px(SIDEBAR_ROW_PADDING_X + row.depth as f32 * INDENT_STEP);
        let path = node.path.clone();
        let is_dir = node.is_dir;
        // Same card as a workspace-rail row: the rail's fills, traced by the
        // shared `squircle` primitive at `ROW_RADIUS` rather than GPUI's
        // circular `rounded()`. A selected row rests filled and drops its hover
        // layer, exactly like the rail's visible tab.
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

        // One leading slot, never two: a directory prints its chevron there
        // (right = collapsed, down = expanded - a static swap, legible under
        // reduced motion) and a file its language icon, so both start on the
        // same pixel and the name column stays straight. Directories carry no
        // folder glyph; the chevron alone says "container".
        //
        // The language icons ship their own `fill`, so they are painted as
        // images. `svg()` would flatten each one to a single text color.
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

        // One hairline per ancestor level, centered on that ancestor's slot, so
        // a deep row stays visually tied to the folder that holds it. Painted
        // after the card fill and before the content, which is why they read
        // over a selected row instead of under it.
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
        // Rail inset and padding box; only the left padding differs, and only
        // by the tree indent it carries. The name is centered by `items_center`
        // rather than vertical padding, so the row keeps its 28px whatever font
        // resolves.
        .mx(px(SIDEBAR_ROW_MARGIN_X))
        .pl(indent)
        .pr(px(SIDEBAR_ROW_PADDING_X))
        .when(dimmed, |s| s.opacity(DIMMED_OPACITY))
        .children(guides);

        // US-009: right-click any row (file or directory) to open the copy-path
        // menu.
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

        // Whole row toggles a directory (US-003), opens a markdown in the active
        // pane (US-004), or opens any other file in the diff dock's editor
        // (US-019).
        let click_path = path.clone();
        el = el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.files_focus.focus(window, cx);
            this.select_files_row(&click_path, cx);
            if is_dir {
                this.toggle_dir(&click_path, cx);
            } else if is_md {
                this.open_markdown_in_active_pane(click_path.clone(), window, cx);
            } else {
                this.open_file_in_diff_dock(click_path.clone(), window, cx);
            }
            cx.stop_propagation();
        }));

        // US-008: only markdown rows are draggable into a pane. The ghost reuses
        // the shared tab-drag preview; US-019 deliberately leaves drag alone.
        if is_md {
            let drag = MarkdownFileDrag {
                path: path.clone(),
                title: SharedString::from(files_tree::node_name(node)),
                icon: SharedString::from("icons/file-text.svg"),
            };
            el = el.on_drag(drag, |drag, _offset, _window, cx| {
                cx.new(|_| DragPreview {
                    title: drag.title.clone(),
                    icon: drag.icon.clone(),
                })
            });
        }

        // US-020: the matched segment is picked out with `StyledText`'s
        // highlight list - one text element with a styled byte range, not
        // nested spans.
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
                // Rail type scale: `text_sm` on a pinned line height, so the
                // row keeps its height whatever font resolves.
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
