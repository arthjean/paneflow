//! US-008 (prd-git-diff-mode-2026-Q3.md): one changed-file row for the Git
//! Diff mode git panel. Split out of `diff_sidebar/mod.rs` to keep that file
//! under the 250-line cap.

use crate::PaneFlowApp;
use crate::diff::{FileChange, FileEntry};
use crate::theme::UiColors;
use crate::ui_primitives::AnimatedHoverExt;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, Styled, div, prelude::*, px,
};

impl PaneFlowApp {
    /// One changed-file row: status-colored letter + filename + dimmed
    /// directory + +/- counts. Click selects `col_idx`'s branch AND scrolls its
    /// diff body to the file (so clicking a file in any branch section focuses
    /// that branch); no re-diff. `is_active` is whether `col_idx` is the column
    /// currently driving the body - only then does the selected-file highlight
    /// light up, so a filename present in several branches isn't highlighted in
    /// every section.
    pub(super) fn render_diff_file_row(
        &self,
        entry: &FileEntry,
        col_idx: usize,
        is_active: bool,
        indent_px: f32,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Shared diff palette (Codex green/red on dark, theme vc_* on light) so
        // the sidebar status hues match the diff body and the Agents dock.
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
        // For a rename, show the source path as the dimmed tail (`← old`) instead
        // of the destination directory, so the move is legible at a glance. In
        // tree mode (indent > 0) the enclosing directory is already the folder
        // node above, so the plain dir tail is redundant and dropped - renames
        // still keep their `← old` source.
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
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        let resting_background = if selected {
            crate::app::constants::sidebar_tab_active_background()
        } else {
            hover_background.opacity(0.0)
        };

        div()
            // Include `col_idx` so the same file path in two branch sections
            // doesn't collide on a duplicate element id.
            .id(SharedString::from(format!(
                "diff-file-{col_idx}-{}",
                entry.path
            )))
            .flex_none()
            .h(px(28.))
            .pl(px(10. + indent_px))
            .pr(px(12.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .bg(resting_background)
            .animated_hover_bg(resting_background, hover_background)
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.diff_mode.diff_selected_file = Some(path.clone());
                // Select that branch's column AND scroll its body to this file.
                // Multi-project routes through the selected repo tab; the other
                // scopes through the single mounted DiffView.
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
            // Middle: filename (prominent) + dimmed directory tail. Takes the
            // remaining width so the +/- counts always pin to the right edge.
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
