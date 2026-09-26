use gpui::{
    AnyElement, AppContext as _, AsyncApp, ClickEvent, ClipboardItem, Context, Div, FocusHandle,
    FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ObjectFit, ParentElement, Pixels,
    SharedString, Styled, StyledImage, WeakEntity, Window, accesskit::Role, div, img, prelude::*,
    px,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    MODAL_PADDING, ModalKey, menu_panel, modal_backdrop, modal_card, modal_footer, modal_key,
    secondary_button, solid_button, switch_blue,
};
use crate::system_info::{ReportRow, ReportSection, SystemInfo, SystemInfoProbe};
use crate::theme::UiColors;
use crate::ui_primitives::{BODY, LABEL_SM};

const DIALOG_WIDTH: Pixels = px(560.);
const DIALOG_TITLE: Pixels = px(16.);
const APP_ICON_SIZE: Pixels = px(44.);
const CARD_RADIUS: Pixels = crate::app::constants::SETTINGS_CARD_RADIUS;
const LABEL_WIDTH: Pixels = px(116.);
const ROW_LINE_HEIGHT: Pixels = px(18.);
const SECTION_GAP: Pixels = px(16.);
const COLLECTING_MIN_HEIGHT: Pixels = px(272.);
const VALUE_FONT: &str = "Geist Mono";

pub(crate) struct SystemInfoDialog {
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    report: Option<SystemInfo>,
}

impl PaneFlowApp {
    pub(crate) fn open_system_info_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.system_info_dialog.is_some() {
            return;
        }
        let probe = SystemInfoProbe::capture(window, &self.self_update.install_method);
        let return_focus = window.focused(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        self.system_info_dialog = Some(SystemInfoDialog {
            focus,
            return_focus,
            report: None,
        });
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let report = cx.background_spawn(async move { probe.resolve() }).await;
            log::info!("system info:\n{report}");
            let _ = this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                if let Some(dialog) = app.system_info_dialog.as_mut() {
                    dialog.report = Some(report);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_system_info_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.system_info_dialog.take() else {
            return;
        };
        if let Some(return_focus) = dialog.return_focus {
            return_focus.focus(window, cx);
        }
        cx.notify();
    }

    fn copy_system_info(&mut self, cx: &mut Context<Self>) {
        let Some(report) = self
            .system_info_dialog
            .as_ref()
            .and_then(|dialog| dialog.report.as_ref())
        else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(report.to_string()));
        self.show_toast("System info copied to the clipboard", cx);
    }

    fn handle_system_info_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match modal_key(event) {
            Some(ModalKey::Dismiss) => self.close_system_info_dialog(window, cx),
            Some(ModalKey::Confirm) => self.copy_system_info(cx),
            None => return,
        }
        cx.stop_propagation();
    }

    pub(crate) fn render_system_info_dialog(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(dialog) = self.system_info_dialog.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let rows = dialog
            .report
            .as_ref()
            .map(SystemInfo::rows)
            .unwrap_or_default();
        let is_ready = dialog.report.is_some();

        let build_line: SharedString = rows
            .iter()
            .find(|(section, ..)| *section == ReportSection::Build)
            .map_or_else(
                || "Collecting…".into(),
                |(_, label, value)| format!("{label} {value}").into(),
            );

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(14.))
            .px(MODAL_PADDING)
            .pt(MODAL_PADDING)
            .child(
                img("icons/paneflow.png")
                    .size(APP_ICON_SIZE)
                    .flex_none()
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(DIALOG_TITLE)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child("System info"),
                    )
                    .child(
                        div()
                            .text_size(LABEL_SM)
                            .text_color(ui.muted)
                            .child(build_line),
                    ),
            );

        let mut body = div()
            .flex()
            .flex_col()
            .gap(SECTION_GAP)
            .px(MODAL_PADDING)
            .pt(px(20.))
            .when(!is_ready, |body| body.min_h(COLLECTING_MIN_HEIGHT));
        for block in rows.chunk_by(|a, b| a.0 == b.0) {
            let Some(&(section, ..)) = block.first() else {
                continue;
            };
            if section != ReportSection::Build {
                body = body.child(report_section(ui, section.title(), block));
            }
        }

        let footer = modal_footer()
            .justify_between()
            .gap(px(16.))
            .child(
                div()
                    .min_w_0()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child("Contains no paths or environment variables."),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap(px(8.))
                    .child(secondary_button(
                        "system-info-close",
                        "Close",
                        ui,
                        cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.close_system_info_dialog(window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(
                        solid_button("system-info-copy", "Copy", switch_blue())
                            .when(!is_ready, |button| button.opacity(0.5))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.copy_system_info(cx);
                                cx.stop_propagation();
                            })),
                    ),
            );

        let card = modal_card(
            "system-info-dialog",
            DIALOG_WIDTH,
            CARD_RADIUS,
            ui,
            div().child(header).child(body).child(footer),
        )
        .role(Role::Dialog)
        .aria_label("System info")
        .track_focus(&dialog.focus)
        .on_key_down(cx.listener(Self::handle_system_info_key_down));

        modal_backdrop(
            "system-info-backdrop",
            card,
            cx.listener(|this, _, window, cx| {
                this.close_system_info_dialog(window, cx);
            }),
        )
    }
}

fn report_section(ui: UiColors, title: &'static str, rows: &[ReportRow]) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .px(px(4.))
                .pb(px(8.))
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(title),
        )
        .child(
            menu_panel(div(), ui).children(
                rows.iter()
                    .map(|(_, label, value)| report_row(ui, label, value.clone())),
            ),
        )
}

fn report_row(ui: UiColors, label: &'static str, value: SharedString) -> Div {
    div()
        .flex()
        .flex_row()
        .items_start()
        .gap(px(12.))
        .px(px(10.))
        .py(px(7.))
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
                .font_family(VALUE_FONT)
                .text_size(BODY)
                .line_height(ROW_LINE_HEIGHT)
                .text_color(ui.text)
                .child(value),
        )
}
