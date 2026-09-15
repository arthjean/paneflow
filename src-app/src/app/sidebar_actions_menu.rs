use crate::app::sidebar::SIDEBAR_ROW_LINE_HEIGHT;
use crate::ui_primitives::{ROW_RADIUS, TooltipDelayExt, squircle_skin};

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

    pub(crate) fn render_sidebar_settings_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        use paneflow_config::schema::AppMode;

        let ui = crate::theme::ui_colors();
        let mode = self.mode;

        let active_bg = crate::app::constants::sidebar_tab_active_background();
        let hover_bg = crate::app::constants::sidebar_tab_hover_background();
        let settings_open = self.settings_section.is_some();
        let settings_trigger = squircle_skin(
            div()
                .id("sidebar-settings-trigger")
                .flex_none()
                .h(px(30.))
                .w(px(30.))
                .flex()
                .items_center()
                .justify_center(),
            "sidebar-settings-trigger-group",
            ROW_RADIUS,
            settings_open.then_some(active_bg),
            (!settings_open).then_some(hover_bg),
        )
        .delayed_tooltip(move |_window, cx| {
            cx.new(|_| crate::app::sidebar::SidebarTooltip {
                label: "Settings".into(),
            })
            .into()
        })
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.open_settings_window(window, cx);
        }))
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/settings.svg")
                .text_color(ui.muted),
        );

        type Activate = Box<dyn Fn(&mut PaneFlowApp, &mut gpui::Window, &mut Context<PaneFlowApp>)>;
        let mode_button =
            |id: &'static str, label: &'static str, is_active: bool, activate: Activate| {
                let button = squircle_skin(
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(30.))
                        .min_w_0()
                        .px(px(2.))
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center(),
                    format!("{id}-group"),
                    ROW_RADIUS,
                    is_active.then_some(active_bg),
                    (!is_active).then_some(hover_bg),
                )
                .child(
                    div()
                        .min_w_0()
                        .text_sm()
                        .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ui.text)
                        .truncate()
                        .child(label),
                );
                if is_active {
                    button.into_any_element()
                } else {
                    button
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            activate(this, window, cx);
                            cx.notify();
                        }))
                        .into_any_element()
                }
            };

        let footer_row: AnyElement = div()
            .id("sidebar-mode-tabs")
            .mx(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(3.))
            .child(mode_button(
                "sidebar-mode-cli",
                "Agents",
                matches!(mode, AppMode::Cli),
                Box::new(|this, window, cx| this.enter_cli_mode(window, cx)),
            ))
            .child(mode_button(
                "sidebar-mode-diff",
                "Review",
                matches!(mode, AppMode::Diff),
                Box::new(|this, window, cx| this.enter_diff_mode(window, cx)),
            ))
            .child(settings_trigger)
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
