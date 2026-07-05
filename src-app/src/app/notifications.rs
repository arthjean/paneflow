//! Toast types, helpers and rendering.
//!
//! Owns:
//! - `Toast`: ephemeral bottom-right confirmation/error pop-ups.
//! - `ToastAction`: optional buttons inside a toast (retry/open-releases).
//! - `show_toast` / `show_update_error_toast` / `push_toast`: convenience
//!   helpers attached to `PaneFlowApp`.
//! - `render_toast`: the deferred rendering block used by `Render for
//!   PaneFlowApp` to paint the active toast.

use gpui::{
    Animation, AnimationExt, AnyElement, AsyncApp, Context, IntoElement, MouseButton,
    ParentElement, SharedString, Styled, WeakEntity, deferred, div, ease_in_out, prelude::*, px,
    svg,
};

use crate::app::constants::{TOAST_ENTER_MS, TOAST_EXIT_MS, TOAST_HOLD_MS};
use crate::settings::components::with_alpha;
use crate::theme::UiColors;
use crate::{PaneFlowApp, StartSelfUpdate, update};

#[derive(Clone)]
pub(crate) struct Toast {
    pub(crate) message: String,
    /// Optional action buttons shown inside the toast. Empty for the
    /// ordinary confirmation toasts ("Path copied", etc); populated for
    /// update-failure toasts (US-013) with Retry / "Open releases" buttons.
    pub(crate) actions: Vec<ToastAction>,
    /// How long the "hold" phase of the toast animation lasts, in ms.
    /// Must match the auto-dismiss timer in [`PaneFlowApp::push_toast`] -
    /// otherwise the exit animation plays early and the element persists
    /// as a ghost at opacity 0 until the dismiss task fires.
    pub(crate) hold_ms: u64,
}

#[derive(Clone)]
pub(crate) enum ToastAction {
    /// "Retry" - re-dispatches the `StartSelfUpdate` action. The action
    /// handler's existing guards (busy check, attempt counter) apply.
    RetryUpdate,
    /// "Open releases" - opens the given URL in the user's browser.
    /// Used for the 4th-attempt fallback (AC: "Download manually from the
    /// releases page").
    OpenReleasesPage(String),
}

impl PaneFlowApp {
    pub(crate) fn show_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.push_toast(message.into(), Vec::new(), TOAST_HOLD_MS, cx);
    }

    /// Surface an update failure as a toast with a "Retry" action button
    /// (US-013). Hold is extended so the user has time to click the button
    /// before auto-dismiss.
    pub(crate) fn show_update_error_toast(
        &mut self,
        err: &update::UpdateError,
        cx: &mut Context<Self>,
    ) {
        self.push_toast(
            err.user_message(),
            vec![ToastAction::RetryUpdate],
            TOAST_HOLD_MS * 4,
            cx,
        );
    }

    pub(crate) fn push_toast(
        &mut self,
        message: String,
        actions: Vec<ToastAction>,
        hold_ms: u64,
        cx: &mut Context<Self>,
    ) {
        let toast = Toast {
            message,
            actions,
            hold_ms,
        };
        if self.toast.is_some() {
            self.toast_queue.push_back(toast);
            cx.notify();
            return;
        }
        self.show_next_toast(toast, cx);
    }

    fn show_next_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let total = TOAST_ENTER_MS + toast.hold_ms + TOAST_EXIT_MS;
        self.toast = Some(Toast {
            message: toast.message,
            actions: toast.actions,
            hold_ms: toast.hold_ms,
        });
        cx.notify();

        self._toast_task = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(total)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        if let Some(next) = app.toast_queue.pop_front() {
                            app.show_next_toast(next, cx);
                        } else {
                            app.toast = None;
                            app._toast_task = None;
                            cx.notify();
                        }
                    })
                });
            },
        ));
    }

    /// Build the deferred element that paints the active toast at the
    /// bottom-right of the window. Caller is responsible for the
    /// `if let Some(toast) = &self.toast` guard.
    pub(crate) fn render_toast(&self, toast: &Toast, ui: UiColors) -> AnyElement {
        let has_actions = !toast.actions.is_empty();
        let is_error = has_actions || toast_message_reads_like_error(&toast.message);
        let (icon, icon_color, max_w) = if is_error {
            ("icons/triangle-alert.svg", ui.agent_error, px(440.))
        } else {
            ("icons/check.svg", ui.vc_added, px(340.))
        };

        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(9.))
            .child(
                svg()
                    .size(px(15.))
                    .flex_none()
                    .path(icon)
                    .text_color(icon_color),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(12.5))
                    .text_color(ui.text)
                    .child(toast.message.clone()),
            );

        let action_row = if has_actions {
            let mut row = div().flex().flex_row().gap(px(8.)).mt(px(10.)).pl(px(24.));
            for (idx, action) in toast.actions.iter().enumerate() {
                let (label, button_id): (&str, String) = match action {
                    ToastAction::RetryUpdate => ("Retry", format!("toast-retry-{idx}")),
                    ToastAction::OpenReleasesPage(_) => {
                        ("Open releases", format!("toast-releases-{idx}"))
                    }
                };
                let action_clone = action.clone();
                let btn = div()
                    .id(SharedString::from(button_id))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .bg(with_alpha(ui.text, 0.08))
                    .text_color(ui.text)
                    .text_size(px(12.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(with_alpha(ui.text, 0.12)))
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| match &action_clone {
                        ToastAction::RetryUpdate => {
                            window.dispatch_action(Box::new(StartSelfUpdate), cx);
                        }
                        ToastAction::OpenReleasesPage(url) => {
                            let _ = open::that(url);
                        }
                    });
                row = row.child(btn);
            }
            Some(row)
        } else {
            None
        };

        let hold_ms = toast.hold_ms;
        deferred(
            div()
                .id("copy-toast")
                .absolute()
                .right(px(18.))
                .bottom(px(18.))
                .max_w(max_w)
                .min_w(px(220.))
                .rounded(px(8.))
                .bg(with_alpha(ui.text, 0.08))
                .text_sm()
                .text_color(ui.text)
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .pl(px(12.))
                        .pr(px(14.))
                        .py(px(11.))
                        .child(header)
                        .children(action_row),
                )
                .with_animations(
                    SharedString::from("copy-toast-anim"),
                    vec![
                        Animation::new(std::time::Duration::from_millis(TOAST_ENTER_MS))
                            .with_easing(ease_in_out),
                        Animation::new(std::time::Duration::from_millis(hold_ms)),
                        Animation::new(std::time::Duration::from_millis(TOAST_EXIT_MS))
                            .with_easing(ease_in_out),
                    ],
                    |toast_el, stage, delta| match stage {
                        0 => {
                            let lift = 8.0 * (1.0 - delta);
                            toast_el.opacity(delta).bottom(px(20.0 + lift))
                        }
                        1 => toast_el.opacity(1.0).bottom(px(20.0)),
                        _ => {
                            let drop = 8.0 * delta;
                            toast_el.opacity(1.0 - delta).bottom(px(20.0 + drop))
                        }
                    },
                ),
        )
        .priority(2)
        .into_any_element()
    }
}

fn toast_message_reads_like_error(message: &str) -> bool {
    let message = message.to_lowercase();
    [
        "could not",
        "couldn't",
        "failed",
        "failure",
        "error",
        "invalid",
        "unavailable",
        "not found",
        "unsupported",
        "corrupt",
        "tampered",
        "timeout",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}
