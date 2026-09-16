use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::app::self_update_flow::ManualUpdateCheck;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

impl PaneFlowApp {
    pub(crate) fn render_sidebar_update_banner(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.self_update.manual_check == Some(ManualUpdateCheck::Failed) {
            return Some(self.render_sidebar_check_failed_banner(cx));
        }
        None
    }

    fn render_sidebar_check_failed_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let muted = ui.muted;
        let text = ui.text;
        div()
            .id("sidebar-update-check-failed")
            .mx(px(6.))
            .mb(px(2.))
            .h(px(30.))
            .px(px(8.))
            .rounded(crate::app::constants::SIDEBAR_TAB_CORNER_RADIUS)
            .bg(crate::app::constants::sidebar_tab_active_background())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path("icons/triangle-alert.svg")
                    .text_color(ui.vc_deleted),
            )
            .child(render_update_plain_label("Update check failed", ui))
            .child(
                div()
                    .id("sidebar-update-check-dismiss")
                    .px(px(4.))
                    .text_color(muted)
                    .text_size(px(13.))
                    .font_weight(FontWeight::BOLD)
                    .animated_hover(move |style, delta| {
                        style.text_color(lerp_color(muted, text, delta));
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        cx.stop_propagation();
                        this.handle_dismiss_update(&crate::DismissUpdate, window, cx);
                    }))
                    .child("×"),
            )
            .opacity(0.8)
            .animated_hover(move |style, delta| {
                style.opacity(lerp(0.8, 1.0, delta));
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.request_update_check(cx);
                }),
            )
            .into_any_element()
    }

    pub(crate) fn render_sidebar_ipc_banner(&self, _cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.ipc_status.state() != crate::ipc::IpcState::Disabled {
            return None;
        }
        let ui = crate::theme::ui_colors();
        Some(
            div()
                .id("sidebar-ipc-banner")
                .mx(px(6.))
                .mb(px(2.))
                .px(px(8.))
                .py(px(6.))
                .rounded(px(6.))
                .border_1()
                .border_color(ui.border)
                .bg(ui.subtle)
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path("icons/triangle-alert.svg")
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(ui.text)
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child("IPC offline"),
                )
                .into_any_element(),
        )
    }

    pub(crate) fn render_sidebar_settings_footer(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let active_bg = crate::app::constants::sidebar_tab_active_background();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let settings_open = self.settings_section.is_some();
        let focus = self.sidebar_filter_input.read(cx).focus_handle.clone();
        let has_query = !self.sidebar_filter_input.read(cx).value().is_empty();
        let focused = focus.is_focused(window);
        let expanded = self.sidebar_filter_hovered || focused || has_query;
        let filter = div()
            .id("sidebar-filter")
            .flex_1()
            .min_w_0()
            .h(px(30.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(8.))
            .rounded_full()
            .when(focused || has_query, |field| field.bg(active_bg))
            .when(expanded && !focused && !has_query, |field| {
                field.bg(hover_bg)
            })
            .hover(move |style| {
                style.bg(if focused || has_query {
                    active_bg
                } else {
                    hover_bg
                })
            })
            .cursor_text()
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.sidebar_filter_hovered = *hovered;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&focus, cx);
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    this.sidebar_filter_input
                        .update(cx, |input, cx| input.clear(cx));
                    window.blur();
                    if let Some(ws) = this.workspaces.get(this.active_idx) {
                        ws.focus_first(window, cx);
                    }
                    cx.stop_propagation();
                }
            }))
            .child(
                svg()
                    .size(px(18.))
                    .flex_none()
                    .path(if focused {
                        "icons/filter-circle.svg"
                    } else {
                        "icons/filter-circle-outline.svg"
                    })
                    .text_color(if focused {
                        crate::app::constants::sidebar_filter_icon_color()
                    } else {
                        ui.muted
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(ui.text)
                    .when(!expanded, |field| field.invisible())
                    .child(self.sidebar_filter_input.clone()),
            )
            .when(has_query, |field| {
                field.child(
                    div()
                        .id("sidebar-filter-clear")
                        .size(px(18.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_full()
                        .bg(ui.muted.opacity(0.2))
                        .cursor_pointer()
                        .delayed_tooltip(crate::ui_primitives::text_tooltip("Clear filter"))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            this.sidebar_filter_input
                                .update(cx, |input, cx| input.clear(cx));
                            let focus = this.sidebar_filter_input.read(cx).focus_handle.clone();
                            window.focus(&focus, cx);
                        }))
                        .child(
                            svg()
                                .size(px(10.))
                                .path("icons/close.svg")
                                .text_color(ui.text),
                        ),
                )
            });
        let settings_row = squircle_skin(
            div()
                .id("sidebar-settings-trigger")
                .flex_none()
                .w(px(30.))
                .h(px(30.))
                .min_w_0()
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.)),
            "sidebar-settings-trigger-group",
            ROW_RADIUS,
            settings_open.then_some(active_bg),
            (!settings_open).then_some(hover_bg),
        )
        .cursor_pointer()
        .delayed_tooltip(crate::ui_primitives::text_tooltip("Settings"))
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.open_settings_window(window, cx);
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/sidebar-settings.svg")
                .text_color(ui.muted),
        );

        let footer_row: AnyElement = div()
            .id("sidebar-footer-row")
            .mx(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .child(filter)
            .child(settings_row)
            .into_any_element();

        let mut footer = div().relative().flex_none().pt(px(6.)).pb(px(8.));
        if let Some(banner) = self.render_sidebar_ipc_banner(cx) {
            footer = footer.child(banner);
        }
        if let Some(banner) = self.render_sidebar_update_banner(cx) {
            footer = footer.child(banner);
        }
        footer.child(footer_row).into_any_element()
    }
}

fn render_update_plain_label(label: &str, ui: crate::theme::UiColors) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .text_size(px(12.))
        .font_weight(FontWeight::BOLD)
        .text_color(ui.text)
        .truncate()
        .child(label.to_string())
        .into_any_element()
}

fn lerp(from: f32, to: f32, amount: f32) -> f32 {
    from + (to - from) * amount
}
