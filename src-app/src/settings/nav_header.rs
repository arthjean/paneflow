use crate::PaneFlowApp;
use crate::settings::chrome::{
    SETTINGS_NAV_ICON_BASELINE_NUDGE, SETTINGS_NAV_ICON_SIZE, SETTINGS_NAV_ROW_MARGIN_X,
    SETTINGS_NAV_ROW_PADDING_X,
};
use crate::theme::UiColors;
use crate::ui_primitives::squircle_skin;
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
                        .mx(px(SETTINGS_NAV_ROW_MARGIN_X))
                        .px(px(SETTINGS_NAV_ROW_PADDING_X))
                        .py(px(6.))
                        .min_h(px(32.))
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.)),
                    "settings-back-group",
                    px(9.),
                    None,
                    Some(hover_background),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.close_settings(cx);
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(SETTINGS_NAV_ICON_SIZE))
                        .flex_none()
                        .relative()
                        .top(px(SETTINGS_NAV_ICON_BASELINE_NUDGE))
                        .path("icons/arrow_left.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(14.))
                        .line_height(px(20.))
                        .text_color(ui.text)
                        .truncate()
                        .child("Back to the app"),
                ),
            )
            .into_any_element()
    }
}
