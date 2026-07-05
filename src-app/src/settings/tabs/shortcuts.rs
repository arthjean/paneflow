//! "Shortcuts" settings tab - agents-styled list of every rebindable
//! action with click-to-record key capture.
//!
//! Layout: a "Bindings" eyebrow with an inline "Reset to defaults" button on
//! the right, then a single `setting_card` containing one row per shortcut,
//! separated by 1px hairlines. Click capture is driven by
//! `PaneFlowApp::handle_shortcut_recording` (in `app::settings`).

use gpui::{
    ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::*, px,
};

use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, hairline, secondary_button, section_header_with_action,
    setting_card,
};
use crate::{PaneFlowApp, config_writer, keybindings};

impl PaneFlowApp {
    pub(crate) fn render_shortcuts_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let recording_idx = self.recording_shortcut_idx;

        let reset_btn = secondary_button(
            "reset-shortcuts",
            "Reset to defaults",
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                config_writer::reset_shortcuts();
                let config = paneflow_config::loader::load_config();
                keybindings::apply_keybindings(cx, &config.shortcuts);
                this.effective_shortcuts = keybindings::effective_shortcuts(&config.shortcuts);
                this.recording_shortcut_idx = None;
                cx.notify();
            }),
        );

        let header = section_header_with_action(ui, "Bindings", reset_btn);

        let mut list = setting_card(ui);

        let total = self.effective_shortcuts.len();
        for (i, entry) in self.effective_shortcuts.iter().enumerate() {
            let is_recording = recording_idx == Some(i);
            let is_last = i + 1 == total;

            let key_badge = if is_recording {
                div()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(ui.accent)
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child("Press a key…")
            } else {
                div()
                    .px(px(10.))
                    .py(px(3.))
                    .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                    .bg(ui.subtle)
                    .text_size(px(11.))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(entry.key.clone())
            };

            let row = div()
                .id(("shortcut", i))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(12.))
                .py(px(10.))
                .cursor(CursorStyle::PointingHand)
                .hover(|s| s.bg(ui.subtle))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.recording_shortcut_idx = Some(i);
                    this.settings_focus.focus(window, cx);
                    cx.notify();
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(ui.text)
                        .truncate()
                        .child(entry.description.clone()),
                )
                .child(key_badge);

            list = list.child(row);
            if !is_last {
                list = list.child(hairline(ui));
            }
        }

        let hint = div()
            .pt(px(10.))
            .text_size(px(11.))
            .text_color(ui.muted)
            .child("Click a row to record a new shortcut. Escape to cancel.");

        div()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .child(hint)
    }
}
