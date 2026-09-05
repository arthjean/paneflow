use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    Render, Styled, Window, div, px,
};

use super::list;
use super::panel::FilesSidebar;

impl FilesSidebar {
    fn files_filter_row(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let is_empty = self.filter_input.read(cx).value().is_empty();
        div()
            .flex()
            .flex_none()
            .px(px(8.))
            .pt(px(8.))
            .child(
                crate::ui_primitives::filter_pill(
                    "files-sidebar-filter",
                    "files-sidebar-filter-clear",
                    ui,
                    self.filter_input.clone(),
                    !is_empty,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.clear_files_filter(window, cx);
                    }),
                )
                .w_full()
                .h(px(28.))
                .py_0()
                .px(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(ui.border)
                .bg(ui.text.opacity(0.025))
                .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                    if ev.keystroke.key.as_str() == "escape" && this.clear_files_filter(window, cx)
                    {
                        cx.stop_propagation();
                    }
                })),
            )
            .into_any_element()
    }

    fn files_sidebar_body(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        if self.projection.rows.is_empty() {
            let message = if self.projection_task.is_some() && !self.tree.root_listing_ready() {
                "Loading files..."
            } else if !self.query.is_empty() {
                "No matching files"
            } else if self.tree.root_listing_ready() {
                "This folder is empty."
            } else {
                "Loading files..."
            };
            return div()
                .flex_1()
                .p(px(14.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(message)
                .into_any_element();
        }
        list::files_list(
            self.projection.rows.len(),
            &self.scroll,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let projection = this.projection.clone();
                range
                    .filter_map(|index| projection.rows.get(index))
                    .map(|row| {
                        this.files_row(
                            row,
                            this.selected.as_deref() == Some(row.node.path.as_path()),
                            ui,
                            cx,
                        )
                    })
                    .collect()
            }),
        )
        .into_any_element()
    }
}

impl Render for FilesSidebar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.render_count += 1;
        }
        let ui = crate::theme::ui_colors();
        div()
            .id("files-sidebar")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_files_sidebar_key_down))
            .bg(ui.base)
            .child(self.files_filter_row(ui, cx))
            .child(self.files_sidebar_body(ui, cx))
    }
}
