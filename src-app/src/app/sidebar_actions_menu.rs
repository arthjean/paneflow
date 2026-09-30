use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::squircle_skin;

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::app::self_update_flow::ManualUpdateCheck;
use crate::settings::components::with_alpha;
use crate::ui_primitives::{DISMISS_BUTTON_SIZE, ROW_RADIUS, dismiss_button};

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
        let control_radius = px(7.);
        let retry = squircle_skin(
            div()
                .id("sidebar-update-check-retry")
                .flex_none()
                .h(DISMISS_BUTTON_SIZE)
                .px(px(8.))
                .flex()
                .items_center()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text),
            "sidebar-update-check-retry-group",
            control_radius,
            Some(with_alpha(ui.text, 0.08)),
            Some(with_alpha(ui.text, 0.12)),
        )
        .role(gpui::accesskit::Role::Button)
        .aria_label("Retry update check")
        .child("Retry")
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
            this.request_update_check(cx);
        }));
        let dismiss = dismiss_button(
            "sidebar-update-check-dismiss",
            "Dismiss",
            control_radius,
            ui.muted,
            ui.text,
            with_alpha(ui.text, 0.08),
        )
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            cx.stop_propagation();
            this.handle_dismiss_update(&crate::DismissUpdate, window, cx);
        }));
        squircle_skin(
            div()
                .id("sidebar-update-check-failed")
                .mx(px(6.))
                .mb(px(2.))
                .h(px(30.))
                .pl(px(8.))
                .pr(px(3.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.)),
            "sidebar-update-check-failed-group",
            ROW_RADIUS,
            Some(crate::app::constants::sidebar_tab_active_background()),
            None,
        )
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/triangle-alert.svg")
                .text_color(ui.vc_deleted),
        )
        .child(render_update_plain_label("Update check failed", ui))
        .child(retry)
        .child(dismiss)
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

    fn hand_sidebar_filter_focus_to_workspace(
        &self,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        window.blur();
        if let Some(ws) = self.workspaces.get(self.active_idx) {
            ws.focus_first(window, cx);
        }
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
        let filter = crate::ui_primitives::filter_field(
            "sidebar-filter",
            "sidebar-filter-clear",
            "Filter workspaces",
            ui,
            crate::ui_primitives::FilterFieldStyle::sidebar(
                crate::ui_primitives::FilterFieldGlyph::Filter,
            ),
            focused,
            has_query,
            expanded,
            None,
            self.sidebar_filter_input.clone(),
            cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.sidebar_filter_input
                    .update(cx, |input, cx| input.clear(cx));
                let focus = this.sidebar_filter_input.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            }),
        )
        .flex_1()
        .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
            this.sidebar_filter_hovered = *hovered;
            cx.notify();
        }))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .on_mouse_down_out(cx.listener(|this, _: &gpui::MouseDownEvent, window, cx| {
            let focus = this.sidebar_filter_input.read(cx).focus_handle.clone();
            if focus.is_focused(window) {
                this.hand_sidebar_filter_focus_to_workspace(window, cx);
            }
        }))
        .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.key == "escape" {
                this.sidebar_filter_input
                    .update(cx, |input, cx| input.clear(cx));
                this.hand_sidebar_filter_focus_to_workspace(window, cx);
                cx.stop_propagation();
            }
        }));
        let settings_row = squircle_skin(
            div()
                .id("sidebar-settings-trigger")
                .flex_none()
                .size(px(36.))
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .justify_center(),
            "sidebar-settings-trigger-group",
            px(12.),
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
                .size(px(18.))
                .flex_none()
                .path("icons/settings.svg")
                .text_color(ui.muted),
        );

        let footer_row: AnyElement = div()
            .id("sidebar-footer-row")
            .mx(px(crate::app::sidebar::SIDEBAR_ROW_MARGIN_X))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(filter)
            .child(settings_row)
            .into_any_element();

        let mut footer = div().relative().flex_none().pt(px(0.)).pb(px(9.5));
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
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.text)
        .truncate()
        .child(label.to_string())
        .into_any_element()
}
