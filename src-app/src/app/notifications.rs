use gpui::{
    Animation, AnimationExt, AnyElement, AsyncApp, Context, CursorStyle, IntoElement, MouseButton,
    ParentElement, SharedString, Styled, WeakEntity, deferred, div, ease_in_out, prelude::*, px,
    svg,
};

use crate::app::constants::{TOAST_ENTER_MS, TOAST_EXIT_MS, TOAST_HOLD_MS};
use crate::settings::components::with_alpha;
use crate::theme::UiColors;
use crate::ui_primitives::{AnimatedHoverExt, ROW_RADIUS, lerp_color, squircle_skin};
use crate::{PaneFlowApp, StartSelfUpdate, update};

#[derive(Clone)]
pub(crate) struct Toast {
    pub(crate) message: String,
    pub(crate) actions: Vec<ToastAction>,
    pub(crate) hold_ms: u64,
    pub(crate) click_url: Option<String>,
    pub(crate) persistent: bool,
}

#[derive(Clone)]
pub(crate) enum ToastAction {
    RetryUpdate,
    OpenReleasesPage(String),
    OpenReleaseNotes(String),
}

impl PaneFlowApp {
    pub(crate) fn show_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.push_toast(message.into(), Vec::new(), TOAST_HOLD_MS, cx);
    }

    pub(crate) fn show_release_notes_toast(&mut self, version: &str, cx: &mut Context<Self>) {
        let url = crate::update::release_notes::changelog_url(version);
        self.enqueue_toast(
            Toast {
                message: format!("Updated to PaneFlow {version}"),
                actions: vec![ToastAction::OpenReleaseNotes(url.clone())],
                hold_ms: 0,
                click_url: Some(url),
                persistent: true,
            },
            cx,
        );
    }

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
            click_url: None,
            persistent: false,
        };
        self.enqueue_toast(toast, cx);
    }

    fn enqueue_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        if self.toast.is_some() {
            self.toast_queue.push_back(toast);
            cx.notify();
            return;
        }
        self.show_next_toast(toast, cx);
    }

    fn show_next_toast(&mut self, toast: Toast, cx: &mut Context<Self>) {
        let total = TOAST_ENTER_MS + toast.hold_ms + TOAST_EXIT_MS;
        let persistent = toast.persistent;
        self.toast = Some(toast);
        cx.notify();

        if persistent {
            self._toast_task = None;
            return;
        }

        self._toast_task = Some(cx.spawn(
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(total)).await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                        app.dismiss_toast(cx);
                    })
                });
            },
        ));
    }

    pub(crate) fn dismiss_toast(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.toast_queue.pop_front() {
            self.show_next_toast(next, cx);
        } else {
            self.toast = None;
            self._toast_task = None;
            cx.notify();
        }
    }

    pub(crate) fn render_toast(
        &self,
        toast: &Toast,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let has_actions = !toast.actions.is_empty();
        if toast
            .actions
            .iter()
            .any(|action| matches!(action, ToastAction::OpenReleaseNotes(_)))
        {
            return self.render_release_toast(toast, ui, cx);
        }
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
                    ToastAction::OpenReleaseNotes(_) => {
                        ("View release notes", format!("toast-release-notes-{idx}"))
                    }
                };
                let action_clone = action.clone();
                let resting_background = with_alpha(ui.text, 0.08);
                let hover_background = with_alpha(ui.text, 0.12);
                let btn = div()
                    .id(SharedString::from(button_id))
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .bg(resting_background)
                    .text_color(ui.text)
                    .text_size(px(12.))
                    .animated_hover(move |style, delta| {
                        style.bg(lerp_color(resting_background, hover_background, delta));
                    })
                    .child(label)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| match &action_clone {
                        ToastAction::RetryUpdate => {
                            window.dispatch_action(Box::new(StartSelfUpdate), cx);
                        }
                        ToastAction::OpenReleasesPage(url) => {
                            if let Err(err) = crate::external_open::open_url(url) {
                                log::warn!("toast: open releases URL failed: {err}");
                            }
                        }
                        ToastAction::OpenReleaseNotes(url) => {
                            if let Err(err) = crate::external_open::open_url(url) {
                                log::warn!("toast: open changelog URL failed: {err}");
                            }
                        }
                    });
                row = row.child(btn);
            }
            Some(row)
        } else {
            None
        };

        let hold_ms = toast.hold_ms;
        let click_url = toast.click_url.clone();
        deferred(
            div()
                .id("copy-toast")
                .absolute()
                .right(px(18.))
                .bottom(px(18.))
                .max_w(max_w)
                .min_w(px(220.))
                .rounded(px(8.))
                .bg(ui.subtle)
                .text_sm()
                .text_color(ui.text)
                .overflow_hidden()
                .when_some(click_url, |el, url| {
                    el.cursor_pointer().on_click(move |_, _, _| {
                        if let Err(err) = crate::external_open::open_url(&url) {
                            log::warn!("toast: open changelog URL failed: {err}");
                        }
                    })
                })
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

impl PaneFlowApp {
    fn render_release_toast(
        &self,
        toast: &Toast,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_light = ui.base.l > 0.5;
        let shadow = vec![
            gpui::BoxShadow::new(
                px(0.),
                px(2.),
                gpui::hsla(0., 0., 0., if is_light { 0.06 } else { 0.12 }),
            )
            .blur_radius(px(3.)),
            gpui::BoxShadow::new(
                px(0.),
                px(3.),
                gpui::hsla(0., 0., 0., if is_light { 0.06 } else { 0.08 }),
            )
            .blur_radius(px(6.)),
            gpui::BoxShadow::new(px(0.), px(6.), gpui::hsla(0., 0., 0., 0.04)).blur_radius(px(12.)),
            gpui::BoxShadow::new(
                px(0.),
                px(1.),
                gpui::hsla(0., 0., 0., if is_light { 0.04 } else { 0.12 }),
            ),
        ];

        let resting_background = with_alpha(ui.text, 0.08);
        let hover_background = with_alpha(ui.text, 0.12);
        let action_url = toast.actions.iter().find_map(|action| match action {
            ToastAction::OpenReleaseNotes(url) => Some(url.clone()),
            _ => None,
        });
        let action = action_url.map(|url| {
            squircle_skin(
                div()
                    .id("toast-release-notes")
                    .flex_none()
                    .h(px(26.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .cursor(CursorStyle::PointingHand)
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text),
                "toast-release-notes-squircle",
                ROW_RADIUS,
                Some(resting_background),
                Some(hover_background),
            )
            .child("View release notes")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Err(err) = crate::external_open::open_url(&url) {
                    log::warn!("toast: open changelog URL failed: {err}");
                }
                this.dismiss_toast(cx);
            }))
        });

        let close_hover = with_alpha(ui.text, 0.08);
        let close = div()
            .id("toast-release-close")
            .flex_none()
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor(CursorStyle::PointingHand)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(with_alpha(close_hover, 0.), close_hover, delta));
            })
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/close.svg")
                    .text_color(ui.muted),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, _, cx| this.dismiss_toast(cx)));

        let click_url = toast.click_url.clone();
        deferred(
            div()
                .id("release-toast")
                .absolute()
                .right(px(12.))
                .bottom(px(12.))
                .w(px(448.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .p(px(12.))
                .rounded(px(8.))
                .border_1()
                .border_color(with_alpha(ui.text, 0.10))
                .bg(crate::theme::active_theme().title_bar_background)
                .shadow(shadow)
                .when_some(click_url, |el, url| {
                    el.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Err(err) = crate::external_open::open_url(&url) {
                                log::warn!("toast: open changelog URL failed: {err}");
                            }
                            this.dismiss_toast(cx);
                        }))
                })
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_start()
                        .gap(px(16.))
                        .w_full()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(14.))
                                .text_color(ui.text)
                                .child(toast.message.clone()),
                        )
                        .child(close),
                )
                .children(action)
                .with_animations(
                    SharedString::from("release-toast-anim"),
                    vec![
                        Animation::new(std::time::Duration::from_millis(TOAST_ENTER_MS))
                            .with_easing(ease_in_out),
                    ],
                    |toast_el, _stage, delta| {
                        let lift = 8.0 * (1.0 - delta);
                        toast_el.opacity(delta).bottom(px(12.0 + lift))
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
