use gpui::{
    AnyElement, AppContext as _, AsyncApp, ClickEvent, ClipboardItem, Context, CursorStyle,
    FontWeight, InteractiveElement, IntoElement, ParentElement, Pixels, Styled, WeakEntity, Window,
    div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    MODAL_PADDING, modal_backdrop, modal_card, modal_footer, secondary_button,
};
use crate::system_info::{SystemInfo, SystemInfoProbe};
use crate::ui_primitives::{BODY, LABEL_SM, TITLE, squircle_skin};

const DIALOG_WIDTH: Pixels = px(560.);
const LABEL_WIDTH: Pixels = px(116.);
const CARD_RADIUS: Pixels = crate::app::constants::SETTINGS_CARD_RADIUS;
const ROW_LINE_HEIGHT: Pixels = px(18.);
const COLLECTING_MIN_HEIGHT: Pixels = px(148.);

pub(crate) enum SystemInfoDialog {
    Collecting,
    Ready(SystemInfo),
}

impl PaneFlowApp {
    pub(crate) fn open_system_info_dialog(&mut self, window: &Window, cx: &mut Context<Self>) {
        let probe = SystemInfoProbe::capture(window, &self.self_update.install_method);
        self.system_info_dialog = Some(SystemInfoDialog::Collecting);
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let report = cx.background_spawn(async move { probe.resolve() }).await;
            log::info!("system info:\n{report}");
            let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                if app.system_info_dialog.is_some() {
                    app.system_info_dialog = Some(SystemInfoDialog::Ready(report));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn close_system_info_dialog(&mut self, cx: &mut Context<Self>) {
        if self.system_info_dialog.take().is_some() {
            cx.notify();
        }
    }

    fn copy_system_info(&mut self, cx: &mut Context<Self>) {
        let Some(SystemInfoDialog::Ready(report)) = self.system_info_dialog.as_ref() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(report.to_string()));
        self.show_toast("System info copied to the clipboard", cx);
    }

    pub(crate) fn render_system_info_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.system_info_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();

        let close_x = squircle_skin(
            div()
                .id("system-info-close")
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(24.))
                .mt(px(-2.))
                .mr(px(-6.))
                .cursor(CursorStyle::PointingHand)
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_system_info_dialog(cx);
                    cx.stop_propagation();
                })),
            "system-info-close-skin",
            px(8.),
            None,
            Some(ui.subtle),
        )
        .child(
            svg()
                .size(px(12.))
                .flex_none()
                .path("icons/close.svg")
                .text_color(ui.muted),
        );

        let header = div()
            .flex()
            .flex_row()
            .items_start()
            .justify_between()
            .gap(px(12.))
            .px(MODAL_PADDING)
            .pt(px(16.))
            .pb(px(16.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(TITLE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child("System info"),
                    )
                    .child(
                        div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child("Goes into a bug report. No paths, no environment."),
                    ),
            )
            .child(close_x);

        let body = match dialog {
            SystemInfoDialog::Collecting => div()
                .px(MODAL_PADDING)
                .pb(px(4.))
                .min_h(COLLECTING_MIN_HEIGHT)
                .text_size(BODY)
                .line_height(ROW_LINE_HEIGHT)
                .text_color(ui.muted)
                .child("Collecting..."),
            SystemInfoDialog::Ready(report) => {
                let mut rows = div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .px(MODAL_PADDING)
                    .pb(px(4.));
                for (label, value) in report.rows() {
                    rows = rows.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_start()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex_none()
                                    .w(LABEL_WIDTH)
                                    .text_size(BODY)
                                    .line_height(ROW_LINE_HEIGHT)
                                    .text_color(ui.muted)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .font_family("monospace")
                                    .text_size(BODY)
                                    .line_height(ROW_LINE_HEIGHT)
                                    .text_color(ui.text)
                                    .child(value),
                            ),
                    );
                }
                rows
            }
        };

        let is_ready = matches!(dialog, SystemInfoDialog::Ready(_));
        let footer = modal_footer()
            .child(secondary_button(
                "system-info-close-button",
                "Close",
                ui,
                cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.close_system_info_dialog(cx);
                    cx.stop_propagation();
                }),
            ))
            .when(is_ready, |footer| {
                footer.child(secondary_button(
                    "system-info-copy",
                    "Copy",
                    ui,
                    cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.copy_system_info(cx);
                        cx.stop_propagation();
                    }),
                ))
            });

        let card = modal_card(
            "system-info-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div().child(header).child(body).child(footer),
        );

        modal_backdrop(
            "system-info-backdrop",
            card,
            cx.listener(|this, _, _, cx| {
                this.close_system_info_dialog(cx);
            }),
        )
    }
}
