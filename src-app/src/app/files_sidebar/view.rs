//! Files sidebar presentation: header (title + US-020 filter pill) + scrollable
//! body. The per-row render lives in `row.rs`; this file stays under the
//! 250-line component budget.

use std::cell::Cell;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, SharedString, Styled, div, prelude::*, px,
};

use super::filter;
use super::row::FilesRowLabel;
use crate::PaneFlowApp;
use crate::app::files_tree::VisibleRowRef;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

struct FilesSidebarRenderTimeCanary {
    start: std::time::Instant,
    row_count: Cell<usize>,
}

impl FilesSidebarRenderTimeCanary {
    fn new() -> Self {
        Self {
            start: std::time::Instant::now(),
            row_count: Cell::new(0),
        }
    }

    fn set_row_count(&self, row_count: usize) {
        self.row_count.set(row_count);
    }
}

impl Drop for FilesSidebarRenderTimeCanary {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > std::time::Duration::from_millis(16) {
            tracing::debug!(
                target: "paneflow_app::files_sidebar",
                "render_files_sidebar exceeded 16ms frame budget: {:.2}ms across {} visible rows",
                elapsed.as_secs_f64() * 1000.0,
                self.row_count.get()
            );
        }
    }
}

impl PaneFlowApp {
    pub(super) fn files_sidebar_header(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Title = the workspace folder's final component (the tree root name).
        let title: SharedString = self
            .files_tree
            .root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.files_tree.root.to_string_lossy().into_owned())
            .into();
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        let title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.))
            // Quiet header - no divider (Codex: separation by spacing, not
            // borders). 36px matches the unified chrome row height.
            .h(px(36.))
            .flex_none()
            .px(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child(title),
            )
            .child(
                div()
                    .id("files-sidebar-close")
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(22.))
                    .rounded(px(5.))
                    .text_size(px(14.))
                    .text_color(ui.muted)
                    .animated_hover(move |style, delta| {
                        style
                            .bg(lerp_color(
                                hover_background.opacity(0.0),
                                hover_background,
                                delta,
                            ))
                            .text_color(lerp_color(ui.muted, ui.text, delta));
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.close_files_sidebar(cx);
                        cx.stop_propagation();
                    }))
                    .child("×"),
            );

        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(title_row)
            .child(self.files_filter_row(ui, cx))
            .into_any_element()
    }

    /// US-020: the type-to-filter field, on the shared [`filter_pill`]
    /// primitive so it reads as the same system as the Agents and Settings
    /// search fields. Escape empties it and hands focus back to the tree; the
    /// unbound keys bubble out of the focused `TextInput` to this container.
    ///
    /// [`filter_pill`]: crate::ui_primitives::filter_pill
    fn files_filter_row(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let is_empty = self.files_filter_input.read(cx).value().is_empty();
        div()
            .flex()
            .flex_none()
            .px(px(8.))
            .pb(px(6.))
            .child(
                crate::ui_primitives::filter_pill(
                    "files-sidebar-filter",
                    "files-sidebar-filter-clear",
                    ui,
                    self.files_filter_input.clone(),
                    !is_empty,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.clear_files_filter(window, cx);
                    }),
                )
                .w_full()
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    // Only swallow the Escape that actually cleared something.
                    // On an already-empty field it keeps bubbling to the
                    // sidebar container, which closes the sidebar - the
                    // two-stage Escape the Settings search field already ships.
                    if ev.keystroke.key.as_str() == "escape" && this.clear_files_filter(window, cx)
                    {
                        cx.stop_propagation();
                    }
                })),
            )
            .into_any_element()
    }

    pub(super) fn files_sidebar_body(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let canary = FilesSidebarRenderTimeCanary::new();

        // US-020: a live needle swaps the tree for a flat, path-matched list.
        // The fold state is untouched - clearing the field restores it exactly.
        let lowered = self.files_filter_lowered(cx);
        if !lowered.is_empty() {
            let matches =
                filter::filter_rows(&self.files_tree.root, &self.files_tree.children, &lowered);
            canary.set_row_count(matches.len());
            if matches.is_empty() {
                return Self::files_sidebar_hint("No matching files", ui);
            }
            let mut body = self.files_sidebar_scroller();
            let selected = self.files_selected.min(matches.len() - 1);
            for (idx, row) in matches.into_iter().enumerate() {
                let label = FilesRowLabel {
                    text: SharedString::from(row.rel),
                    highlight: row.highlight,
                };
                let visible = VisibleRowRef {
                    node: row.node,
                    depth: 0,
                    expanded: false,
                };
                body = body.child(self.files_row(visible, label, idx == selected, ui, cx));
            }
            return body.into_any_element();
        }

        let rows = self.files_visible_rows();
        canary.set_row_count(rows.len());

        if rows.is_empty() {
            let message = if self.files_tree.root_listing_ready() {
                "This folder is empty."
            } else {
                "Loading files..."
            };
            return Self::files_sidebar_hint(message, ui);
        }

        let mut body = self.files_sidebar_scroller();
        let selected = self.files_selected.min(rows.len().saturating_sub(1));
        for (idx, row) in rows.iter().copied().enumerate() {
            let label = FilesRowLabel::plain(SharedString::from(
                crate::app::files_tree::node_name(row.node),
            ));
            body = body.child(self.files_row(row, label, idx == selected, ui, cx));
        }
        body.into_any_element()
    }

    fn files_sidebar_scroller(&self) -> gpui::Stateful<gpui::Div> {
        div()
            .id("files-sidebar-body")
            .flex()
            .flex_col()
            .flex_1()
            .py(px(4.))
            // US-003: vertical scroll only - long names ellipsize, never scroll
            // horizontally.
            .overflow_x_hidden()
            .overflow_y_scroll()
            .track_scroll(&self.files_tree_scroll)
    }

    fn files_sidebar_hint(message: &'static str, ui: crate::theme::UiColors) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .p(px(14.))
            .child(div().text_size(px(12.)).text_color(ui.muted).child(message))
            .into_any_element()
    }
}
