//! About Paneflow modal, styled as a compact native application dialog.

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ObjectFit,
    ParentElement, Styled, deferred, div, hsla, img, prelude::*, px, rgb, svg,
};

use crate::{
    PaneFlowApp,
    ui_primitives::{AnimatedHoverExt, lerp_color},
};

impl PaneFlowApp {
    pub(crate) fn render_about_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let version = env!("CARGO_PKG_VERSION");
        let button_hover_bg = gpui::Hsla::from(rgb(0x3a3a3a));

        let close_x = div()
            .id("about-close-x")
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(30.))
            .h(px(30.))
            .rounded(px(7.))
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(
                    button_hover_bg.opacity(0.0),
                    button_hover_bg,
                    delta,
                ));
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_about_dialog = false;
                cx.notify();
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(12.))
                    .flex_none()
                    .path("icons/close.svg")
                    .text_color(ui.text),
            );

        let header = div()
            .h(px(32.))
            .w_full()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .pl(px(10.))
            .pr(px(2.))
            .bg(rgb(0x232323))
            .border_b_1()
            .border_color(rgb(0x343434))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        img("icons/paneflow.png")
                            .w(px(16.))
                            .h(px(16.))
                            .object_fit(ObjectFit::Contain),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(ui.text)
                            .child("About Paneflow"),
                    ),
            )
            .child(close_x);

        let body = div()
            .w_full()
            .h(px(225.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(rgb(0x202020))
            .child(
                img("icons/paneflow.png")
                    .w(px(64.))
                    .h(px(64.))
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .mt(px(14.))
                    .text_color(ui.text)
                    .text_size(px(16.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("Paneflow"),
            )
            .child(
                div()
                    .mt(px(20.))
                    .text_color(ui.muted)
                    .text_size(px(12.))
                    .child(format!("Version {version}")),
            )
            .child(
                div()
                    .mt(px(14.))
                    .text_color(ui.muted)
                    .text_size(px(12.))
                    .child("© Arthur Jean"),
            );

        let ok_button = div()
            .id("about-ok")
            .w(px(76.))
            .h(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(3.))
            .border_1()
            .border_color(rgb(0x666666))
            .bg(rgb(0x2d2d2d))
            .text_size(px(12.))
            .text_color(ui.text)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(
                    gpui::Hsla::from(rgb(0x2d2d2d)),
                    button_hover_bg,
                    delta,
                ));
            })
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.show_about_dialog = false;
                cx.notify();
                cx.stop_propagation();
            }))
            .child("OK");

        let footer = div()
            .w_full()
            .h(px(56.))
            .flex_none()
            .flex()
            .items_center()
            .justify_end()
            .px(px(14.))
            .bg(rgb(0x252525))
            .border_t_1()
            .border_color(rgb(0x343434))
            .child(ok_button);

        let dialog = div()
            .id("about-dialog")
            .occlude()
            .w(px(382.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(0x202020))
            .border_1()
            .border_color(rgb(0x3a3a3a))
            .rounded(px(10.))
            .shadow_lg()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer);

        deferred(
            div()
                .id("about-dialog-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.55))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.show_about_dialog = false;
                        cx.notify();
                    }),
                )
                .child(dialog),
        )
        .with_priority(10)
        .into_any_element()
    }
}
