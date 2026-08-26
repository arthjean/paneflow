use crate::PaneFlowApp;
use crate::theme::UiColors;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement, Styled, div, px, svg,
};

impl PaneFlowApp {
    pub(crate) fn render_settings_nav_header(
        &self,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The rail's first line is the way out, not a title: settings already
        // name themselves in the content header, so a static "Settings" label
        // plus a lone close glyph spent a whole band saying nothing actionable.
        // This is a normal nav row - same geometry, skin and hover as the
        // section rows below it - so leaving reads as one more destination.
        let hover_background = crate::app::constants::sidebar_tab_hover_background();

        div()
            .flex_none()
            .h(px(36.))
            .flex()
            .flex_col()
            .justify_center()
            .child(
                squircle_skin(
                    div()
                        .id("settings-back")
                        .role(Role::Button)
                        .aria_label("Back to the app")
                        .mx(px(8.))
                        .px(px(8.))
                        .py(px(6.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.)),
                    "settings-back-group",
                    ROW_RADIUS,
                    None,
                    Some(hover_background),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.close_settings(cx);
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(15.))
                        .flex_none()
                        .path("icons/arrow_left.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(ui.text)
                        .truncate()
                        .child("Back to the app"),
                ),
            )
            .into_any_element()
    }
}
