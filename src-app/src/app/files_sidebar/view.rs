use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, SharedString, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use crate::ui_primitives::{AccessibleControlExt, ROW_RADIUS, squircle_skin};

use super::list;
use super::panel::FilesSidebar;

pub(crate) const TOOLBAR_HEIGHT: f32 = 40.;
pub(crate) const REFRESH_LABEL: &str = "Refresh files";
pub(crate) const SEARCH_LABEL: &str = "Search files";
pub(crate) const EXPAND_ALL_LABEL: &str = "Expand all";
pub(crate) const COLLAPSE_ALL_LABEL: &str = "Collapse all";
pub(crate) const MORE_MATCHES: &str = "More matches available. Refine your search.";
pub(crate) const NO_MATCHES: &str = "No matching files.";
const REFRESH_ICON: &str = crate::app::menu_icons::REFRESH;
const EXPAND_ALL_ICON: &str = crate::app::menu_icons::EXPAND_ALL;
const COLLAPSE_ALL_ICON: &str = crate::app::menu_icons::COLLAPSE_ALL;
const TOOL_ICON: f32 = 14.;
const TOOL_SIZE: f32 = 28.;

fn tool_button(
    id: &'static str,
    label: &'static str,
    icon: AnyElement,
) -> gpui::Stateful<gpui::Div> {
    let hover = crate::app::constants::sidebar_tab_hover_background();
    let button = squircle_skin(
        div()
            .id(id)
            .flex_none()
            .size(px(TOOL_SIZE))
            .flex()
            .items_center()
            .justify_center(),
        SharedString::from(format!("{id}-group")),
        ROW_RADIUS,
        None,
        Some(hover),
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .child(icon);
    #[cfg(test)]
    let button = button.debug_selector(|| id.into());
    button
}

impl FilesSidebar {
    pub(super) fn expand_toggle_label(&self) -> &'static str {
        if self.all_expanded {
            COLLAPSE_ALL_LABEL
        } else {
            EXPAND_ALL_LABEL
        }
    }

    fn files_toolbar(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        let is_empty = self.filter_input.read(cx).value().is_empty();
        let refresh_icon = if self.refreshing {
            crate::ui_primitives::spinning_icon(REFRESH_ICON, px(TOOL_ICON), ui.muted)
        } else {
            svg()
                .size(px(TOOL_ICON))
                .flex_none()
                .path(REFRESH_ICON)
                .text_color(ui.muted)
                .into_any_element()
        };
        let refresh = tool_button("files-tree-refresh", REFRESH_LABEL, refresh_icon).on_press(
            cx.listener(|this, _: &crate::ui_primitives::Press, _window, cx| this.refresh(cx)),
        );
        let search = crate::ui_primitives::filter_pill(
            "files-sidebar-filter",
            "files-sidebar-filter-clear",
            SEARCH_LABEL,
            ui,
            self.filter_input.clone(),
            !is_empty,
            cx.listener(|this, _: &ClickEvent, window, cx| {
                this.clear_files_filter(window, cx);
            }),
        )
        .flex_1()
        .min_w_0()
        .h(px(28.))
        .py_0()
        .px(px(8.))
        .rounded(px(8.))
        .border_1()
        .border_color(ui.border)
        .bg(ui.text.opacity(0.025))
        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
            if ev.keystroke.key.as_str() == "escape" && this.clear_files_filter(window, cx) {
                cx.stop_propagation();
            }
        }));
        #[cfg(test)]
        let search = search.debug_selector(|| "files-sidebar-filter".into());
        let expand_label = self.expand_toggle_label();
        let expand_icon = svg()
            .size(px(TOOL_ICON))
            .flex_none()
            .path(if self.all_expanded {
                COLLAPSE_ALL_ICON
            } else {
                EXPAND_ALL_ICON
            })
            .text_color(ui.muted);
        let expand =
            tool_button(
                "files-tree-expand-all",
                expand_label,
                expand_icon.into_any_element(),
            )
            .on_press(cx.listener(
                |this, _: &crate::ui_primitives::Press, _window, cx| this.toggle_expand_all(cx),
            ));
        div()
            .flex()
            .flex_none()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .h(px(TOOLBAR_HEIGHT))
            .px(px(6.))
            .child(refresh)
            .child(search)
            .child(expand)
            .into_any_element()
    }

    fn files_notice(&self, ui: crate::theme::UiColors) -> Option<AnyElement> {
        (!self.query.is_empty() && self.search.truncated && !self.search.pending).then(|| {
            let notice = div()
                .id("files-search-truncated")
                .flex_none()
                .px(px(12.))
                .py(px(4.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(MORE_MATCHES);
            #[cfg(test)]
            let notice = notice.debug_selector(|| "files-search-truncated".into());
            notice.into_any_element()
        })
    }

    fn files_unreadable_body(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let retry = div()
            .id("files-sidebar-retry")
            .flex_none()
            .h(px(26.))
            .px(px(10.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(ui.border)
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(ui.subtle))
            .text_size(crate::ui_primitives::BODY)
            .text_color(ui.text)
            .accessible_control(gpui::accesskit::Role::Button, "Retry")
            .on_activate(cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.retry(cx);
            }))
            .child("Retry");
        #[cfg(test)]
        let retry = retry.debug_selector(|| "files-sidebar-retry".into());
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .p(px(14.))
            .text_size(px(12.))
            .text_color(ui.muted)
            .child("Unable to load folder.")
            .child(retry)
            .into_any_element()
    }

    pub(super) fn empty_message(&self) -> &'static str {
        if !self.query.is_empty() {
            if self.search.pending || self.search.hits.is_none() {
                "Searching files..."
            } else {
                NO_MATCHES
            }
        } else if self.model.root_listing_ready() {
            "This folder is empty."
        } else {
            "Loading files..."
        }
    }

    fn files_sidebar_body(&self, ui: crate::theme::UiColors, cx: &mut Context<Self>) -> AnyElement {
        if self.model.root_unreadable() {
            return self.files_unreadable_body(ui, cx);
        }
        if self.row_count() == 0 {
            let message = div()
                .flex_1()
                .p(px(14.))
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(self.empty_message());
            #[cfg(test)]
            let message = message.debug_selector(|| "files-sidebar-message".into());
            return message.into_any_element();
        }
        list::files_list(
            self.row_count(),
            &self.scroll,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                this.rows_in(range)
                    .iter()
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
        let notice = self.files_notice(ui);
        div()
            .id("files-sidebar")
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .min_h_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_files_sidebar_key_down))
            .child(self.files_toolbar(ui, cx))
            .when_some(notice, |el, notice| el.child(notice))
            .child(self.files_sidebar_body(ui, cx))
    }
}
