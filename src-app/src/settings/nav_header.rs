use crate::PaneFlowApp;
use crate::settings::chrome::{
    SETTINGS_NAV_ROW_GAP, SETTINGS_NAV_ROW_MARGIN_X, SETTINGS_NAV_ROW_PADDING_X, settings_nav_icon,
};
use crate::theme::UiColors;
use crate::ui_primitives::squircle_skin;
use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, ParentElement, Role,
    StatefulInteractiveElement, Styled, div, px,
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
                        .gap(px(SETTINGS_NAV_ROW_GAP)),
                    "settings-back-group",
                    crate::app::constants::rail_row_radius(),
                    None,
                    Some(hover_background),
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.close_settings(cx);
                    cx.notify();
                }))
                .child(settings_nav_icon("icons/settings/arrow-left-02.svg", ui))
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
