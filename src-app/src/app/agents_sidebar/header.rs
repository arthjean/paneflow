use crate::PaneFlowApp;
use crate::theme::UiColors;
use crate::ui_primitives::AnimatedHoverExt;
use crate::ui_primitives::TooltipDelayExt;
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Role, StatefulInteractiveElement, Styled, div, px, svg,
};

impl PaneFlowApp {
    pub(super) fn render_agents_sidebar_header(
        &self,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let hover_background = crate::app::constants::sidebar_tab_active_background();

        div()
            .id("agents-sidebar-header")
            .flex_none()
            .h(px(48.))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .pl(px(8.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Agents"),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    // Skills used to hang off the footer Settings popover.
                    // That popover is gone (Settings now opens the settings
                    // surface in one click), so Skills is anchored here, next
                    // to the only other Agents-scoped header action.
                    .child(
                        div()
                            .id("agents-sidebar-skills")
                            .role(Role::Button)
                            .aria_label("Skills")
                            .flex_none()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .animated_hover_bg(hover_background.opacity(0.0), hover_background)
                            .delayed_tooltip(crate::ui_primitives::text_tooltip("Skills"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.show_agents_skills(cx);
                            }))
                            .child(
                                svg()
                                    .size(px(13.))
                                    .flex_none()
                                    .path("icons/tool.svg")
                                    .text_color(ui.muted),
                            ),
                    )
                    .child(
                        div()
                            .id("agents-sidebar-new-chat")
                            .role(Role::Button)
                            .aria_label("New chat")
                            .flex_none()
                            .size(px(28.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.))
                            .animated_hover_bg(hover_background.opacity(0.0), hover_background)
                            .delayed_tooltip(crate::ui_primitives::text_tooltip("New chat"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.start_new_chat(cx);
                            }))
                            .child(
                                svg()
                                    .size(px(13.))
                                    .flex_none()
                                    .path("icons/edit.svg")
                                    .text_color(ui.muted),
                            ),
                    ),
            )
            .into_any_element()
    }
}
