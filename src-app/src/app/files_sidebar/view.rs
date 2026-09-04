use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, div, prelude::*, px,
};

use super::panel::{FilesEvent, FilesSidebar};
use super::{SIDEBAR_WIDTH, list};

use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

impl FilesSidebar {
    pub(super) fn files_sidebar_header(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let title = self.title.clone();
        let hover_background = crate::app::constants::sidebar_tab_hover_background();
        let title_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.))
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
                    .on_click(cx.listener(|_, _: &ClickEvent, _window, cx| {
                        cx.emit(FilesEvent::Close);
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

    fn files_filter_row(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let is_empty = self.filter_input.read(cx).value().is_empty();
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
                    self.filter_input.clone(),
                    !is_empty,
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.clear_files_filter(window, cx);
                    }),
                )
                .w_full()
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
        let theme = crate::theme::active_theme();
        div()
            .id("files-sidebar")
            .flex()
            .flex_col()
            .w(SIDEBAR_WIDTH)
            .h_full()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_files_sidebar_key_down))
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                self.window_active,
                self.material,
            ))
            .child(self.files_sidebar_header(ui, cx))
            .child(self.files_sidebar_body(ui, cx))
    }
}
