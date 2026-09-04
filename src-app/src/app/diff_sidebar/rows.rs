use crate::PaneFlowApp;
use crate::diff::{FileChange, FileEntry};
use crate::theme::UiColors;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, prelude::*, px,
};

use super::{REVIEW_SIDEBAR_ROW_MARGIN_X, REVIEW_SIDEBAR_ROW_PADDING_X, REVIEW_SIDEBAR_ROW_RADIUS};

impl PaneFlowApp {
    pub(super) fn render_diff_file_row(
        &self,
        entry: &FileEntry,
        col_idx: usize,
        is_active: bool,
        indent_px: f32,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let diff = ui.diff_colors();
        let (letter, color) = match entry.change {
            FileChange::Added => ("A", diff.added),
            FileChange::Modified => ("M", ui.vc_modified),
            FileChange::Deleted => ("D", diff.deleted),
            FileChange::Renamed => ("R", ui.vc_modified),
        };
        let (dir, name) = match entry.path.rfind('/') {
            Some(i) => (entry.path[..i].to_string(), entry.path[i + 1..].to_string()),
            None => (String::new(), entry.path.clone()),
        };
        let dir = match (entry.change, &entry.old_path) {
            (FileChange::Renamed, Some(old)) => format!("← {old}"),
            _ if indent_px > 0.0 => String::new(),
            _ => dir,
        };
        let name_color = if matches!(entry.change, FileChange::Deleted) {
            ui.muted
        } else {
            ui.text
        };
        let selected =
            is_active && self.diff_mode.diff_selected_file.as_deref() == Some(entry.path.as_str());
        let path = entry.path.clone();
        let show_counts = !entry.is_binary && (entry.added > 0 || entry.removed > 0);
        let row_background = crate::app::constants::sidebar_tab_active_background();
        let resting_background = if selected {
            row_background
        } else {
            row_background.opacity(0.0)
        };

        div()
            .id(SharedString::from(format!(
                "diff-file-{col_idx}-{}",
                entry.path
            )))
            .flex_none()
            .h(px(28.))
            .mx(px(REVIEW_SIDEBAR_ROW_MARGIN_X))
            .pl(px(REVIEW_SIDEBAR_ROW_PADDING_X + indent_px))
            .pr(px(REVIEW_SIDEBAR_ROW_PADDING_X))
            .rounded(px(REVIEW_SIDEBAR_ROW_RADIUS))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .bg(resting_background)
            .hover(|s| s.bg(row_background))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.diff_mode.diff_selected_file = Some(path.clone());
                match this.diff_mode.diff_scope {
                    crate::diff::DiffScope::MultiProject => {
                        if let Some(mv) = this.diff_mode.multi_diff_view.clone() {
                            mv.update(cx, |mv, cx| {
                                mv.active_select_and_jump(col_idx, &path, window, cx)
                            });
                        }
                    }
                    _ => {
                        if let Some(dv) = this.diff_mode.diff_view.clone() {
                            dv.update(cx, |dv, cx| dv.select_and_jump(col_idx, &path, window, cx));
                        }
                    }
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_none()
                    .w(px(14.))
                    .text_color(color)
                    .text_size(crate::ui_primitives::LABEL_SM)
                    .font_weight(FontWeight::BOLD)
                    .child(letter),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex_none()
                            .min_w_0()
                            .flex_shrink(1.0)
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(name_color)
                            .text_size(crate::ui_primitives::BODY_EMPHASIS)
                            .when(matches!(entry.change, FileChange::Deleted), |d| {
                                d.line_through()
                            })
                            .child(name),
                    )
                    .when(!dir.is_empty(), |d| {
                        d.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .max_w(px(140.))
                                .truncate()
                                .text_color(ui.muted)
                                .text_size(crate::ui_primitives::LABEL_SM)
                                .child(dir),
                        )
                    }),
            )
            .when(show_counts, |d| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .gap(px(5.))
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .when(entry.added > 0, |d| {
                            d.child(
                                div()
                                    .text_color(diff.added)
                                    .child(format!("+{}", entry.added)),
                            )
                        })
                        .when(entry.removed > 0, |d| {
                            d.child(
                                div()
                                    .text_color(diff.deleted)
                                    .child(format!("-{}", entry.removed)),
                            )
                        }),
                )
            })
            .into_any_element()
    }
}
