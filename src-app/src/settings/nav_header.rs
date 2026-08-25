use crate::PaneFlowApp;
use crate::theme::UiColors;
use crate::ui_primitives::TooltipDelayExt;
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
        // Same header band as the workspace rail: 36 px so it reads as the
        // list's first line rather than a floating title, a normal-weight
        // muted label, and an action skinned as a rail row.
        let hover_background = crate::app::constants::sidebar_tab_hover_background();

        div()
            .id("settings-nav-header")
            .flex_none()
            .h(px(36.))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .pl(px(8.))
                    .text_size(px(13.))
                    .text_color(ui.muted)
                    .child("Settings"),
            )
            .child(
                squircle_skin(
                    div()
                        .id("settings-back")
                        .role(Role::Button)
                        .aria_label("Close settings")
                        .flex_none()
                        .size(px(28.))
                        .flex()
                        .items_center()
                        .justify_center(),
                    "settings-back-group",
                    ROW_RADIUS,
                    None,
                    Some(hover_background),
                )
                .delayed_tooltip(crate::ui_primitives::text_tooltip("Close settings"))
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.close_settings(cx);
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path("icons/close.svg")
                        .text_color(ui.muted),
                ),
            )
            .into_any_element()
    }
}
