use super::*;

pub(super) struct WebDialog {
    pub document: Document,
    pub request: u64,
    pub kind: String,
    pub origin: String,
    pub message: String,
}

impl BrowserView {
    pub(super) fn answer_dialog(&mut self, accept: bool, cx: &mut Context<Self>) {
        let Some(dialog) = self.web_dialog.take() else {
            return;
        };
        let Some(live) = &self.live else {
            return;
        };
        if live.document() != Some(&dialog.document) {
            return;
        }
        if accept && dialog.kind == "permission" {
            self.permissions_allowed = true;
        }
        if accept && dialog.kind == "beforeunload" {
            self.notice = None;
        }
        let text = self.dialog_input.read(cx).value();
        self.input(
            InputEvent::WebResponse {
                request: dialog.request,
                accept,
                text,
            },
            cx,
        );
        if !accept && dialog.kind == "beforeunload" {
            self.close_requested = false;
            self.sleep_requested = false;
        }
        cx.notify();
    }

    pub(super) fn render_web_dialog(
        &mut self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.web_dialog.as_ref()?;
        let origin = super::super::origin_of(&dialog.origin)
            .unwrap_or_else(|_| "Unknown origin".to_string());
        let message = dialog.message.clone();
        let prompt = dialog.kind == "prompt";
        Some(
            div()
                .id("browser-web-dialog")
                .role(gpui::accesskit::Role::Dialog)
                .aria_label(format!("Web request from {origin}"))
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(CONTROL_GAP))
                .p(px(TOOLBAR_PADDING))
                .border_b_1()
                .border_color(ui.border)
                .text_color(ui.text)
                .text_size(px(12.))
                .child(origin)
                .child(
                    div()
                        .id("browser-dialog-message")
                        .max_h(px(120.))
                        .overflow_y_scroll()
                        .child(message),
                )
                .when(prompt, |dialog| dialog.child(self.dialog_input.clone()))
                .child(
                    div()
                        .flex()
                        .gap(px(CONTROL_GAP))
                        .child(text_button(
                            "browser-dialog-deny",
                            "Cancel",
                            ui,
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.answer_dialog(false, cx);
                                this.focus_document(window, cx)
                            }),
                        ))
                        .child(text_button(
                            "browser-dialog-accept",
                            "Allow",
                            ui,
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.answer_dialog(true, cx);
                                this.focus_document(window, cx)
                            }),
                        )),
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
                    if event.keystroke.key == "escape" {
                        this.answer_dialog(false, cx);
                        cx.stop_propagation();
                    }
                }))
                .into_any_element(),
        )
    }
}
