use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Point, SharedString, Styled,
    Window, deferred, div, prelude::*, px,
};

use crate::settings::components::{menu_divider_color, menu_surface, select_item};
use crate::widgets::scrollbar;
use crate::{PaneFlowApp, ThemeMode, config_writer};

impl PaneFlowApp {
    pub(crate) fn current_theme_name(&self) -> String {
        self.cached_config
            .theme
            .as_deref()
            .and_then(crate::theme::canonical_theme_name)
            .unwrap_or(crate::theme::DEFAULT_THEME)
            .to_string()
    }

    pub(crate) fn current_theme_preset(&self) -> &'static crate::theme::ThemePreset {
        crate::theme::preset_for_theme(&self.current_theme_name())
    }

    fn current_theme_index(&self) -> usize {
        let name = self.current_theme_name();
        crate::theme::THEMES
            .iter()
            .position(|(n, _)| *n == name)
            .unwrap_or(0)
    }

    fn theme_picker_matches(&self) -> Vec<&'static str> {
        let q = self.theme_picker_query.to_lowercase();
        crate::theme::THEMES
            .iter()
            .filter(|(name, _)| q.is_empty() || name.to_lowercase().contains(&q))
            .map(|(name, _)| *name)
            .collect()
    }

    pub(crate) fn open_theme_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_theme_picker = true;
        self.theme_picker_query.clear();
        self.theme_picker_selected_idx = self.current_theme_index();
        self.theme_picker_scroll = gpui::ScrollHandle::new();
        self.theme_picker_drag = None;
        self.theme_picker_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_theme_picker(&mut self, cx: &mut Context<Self>) {
        self.show_theme_picker = false;
        self.theme_picker_query.clear();
        self.theme_picker_selected_idx = 0;
        self.theme_picker_drag = None;
        cx.notify();
    }

    pub(crate) fn persist_theme_selection(
        &mut self,
        mode: ThemeMode,
        name: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let ok = config_writer::save_config_values_checked([
            (
                "theme_mode",
                serde_json::Value::String(mode.as_config_str().to_string()),
            ),
            ("theme", serde_json::Value::String(name.to_string())),
        ]);
        if !ok {
            self.show_toast("Could not save theme", cx);
            return false;
        }
        self.theme_mode = mode;
        self.cached_config.theme_mode = Some(mode.as_config_str().to_string());
        self.cached_config.theme = Some(name.to_string());
        crate::theme::invalidate_theme_cache();
        cx.notify();
        true
    }

    pub(crate) fn apply_theme_by_name(&mut self, name: &str, cx: &mut Context<Self>) -> bool {
        self.persist_theme_selection(ThemeMode::from_theme_name(name), name, cx)
    }

    pub(crate) fn apply_theme_preset(
        &mut self,
        preset: &crate::theme::ThemePreset,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mode = self.theme_mode;
        let name = mode.resolved_theme_name(preset, window.appearance());
        self.persist_theme_selection(mode, name, cx)
    }

    pub(crate) fn reset_theme_selection(&mut self, cx: &mut Context<Self>) {
        let ok = config_writer::save_config_values_checked([
            ("theme_mode", serde_json::Value::Null),
            ("theme", serde_json::Value::Null),
        ]);
        if !ok {
            self.show_toast("Could not reset theme", cx);
            return;
        }
        self.theme_mode = ThemeMode::Dark;
        self.cached_config.theme_mode = None;
        self.cached_config.theme = None;
        crate::theme::invalidate_theme_cache();
        cx.notify();
    }

    fn commit_theme_picker_selection(&mut self, cx: &mut Context<Self>) {
        let matches = self.theme_picker_matches();
        if let Some(name) = matches.get(self.theme_picker_selected_idx)
            && !self.apply_theme_by_name(name, cx)
        {
            return;
        }
        self.close_theme_picker(cx);
    }

    pub(crate) fn handle_theme_picker_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let len = self.theme_picker_matches().len();

        match key {
            "escape" => self.close_theme_picker(cx),
            "enter" => {
                if len > 0 {
                    self.commit_theme_picker_selection(cx);
                }
            }
            "up" => {
                if len > 0 && self.theme_picker_selected_idx > 0 {
                    self.theme_picker_selected_idx -= 1;
                    cx.notify();
                }
            }
            "down" => {
                if len > 0 && self.theme_picker_selected_idx + 1 < len {
                    self.theme_picker_selected_idx += 1;
                    cx.notify();
                }
            }
            "backspace" => {
                if self.theme_picker_query.pop().is_some() {
                    self.theme_picker_selected_idx = 0;
                    cx.notify();
                }
            }
            _ => {
                if let Some(ch) = &event.keystroke.key_char
                    && !ch.is_empty()
                    && !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.alt
                {
                    self.theme_picker_query.push_str(ch);
                    self.theme_picker_selected_idx = 0;
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn render_theme_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let matches = self.theme_picker_matches();
        let current_name = self.current_theme_name();

        let query_text: SharedString = if self.theme_picker_query.is_empty() {
            "Select Theme…".into()
        } else {
            format!("{}|", self.theme_picker_query).into()
        };
        let query_color = if self.theme_picker_query.is_empty() {
            ui.muted
        } else {
            ui.text
        };

        let search_input = div()
            .px(px(14.))
            .py(px(10.))
            .text_size(px(13.))
            .text_color(query_color)
            .border_b_1()
            .border_color(menu_divider_color(ui))
            .child(query_text);

        let mut list = div()
            .id("theme-picker-list")
            .flex()
            .flex_col()
            .gap(px(1.))
            .pl(px(6.))
            .py(px(4.))
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .max_h(px(360.))
            .overflow_y_scroll()
            .track_scroll(&self.theme_picker_scroll);

        if matches.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No matching themes"),
            );
        } else {
            for (idx, name) in matches.iter().enumerate() {
                let is_selected = idx == self.theme_picker_selected_idx;
                let is_current = *name == current_name.as_str();
                let label = if *name == crate::theme::DEFAULT_THEME {
                    format!("{} (Default)", name)
                } else {
                    (*name).to_string()
                };
                let name_owned = name.to_string();

                list = list.child(
                    select_item(
                        SharedString::from(format!("theme-picker-row-{idx}")),
                        is_selected,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        if this.apply_theme_by_name(&name_owned, cx) {
                            this.close_theme_picker(cx);
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(ui.text)
                            .child(label),
                    )
                    .when(is_current, |d| {
                        d.child(div().flex_none().text_color(ui.muted).child("✓"))
                    }),
                );
            }
        }

        const PER_ROW: f32 = 25.0;
        let est_content = (matches.len() as f32 * PER_ROW).max(0.0);
        let max_viewport = 360.0;

        let bar = scrollbar::render(
            &self.theme_picker_scroll,
            ui,
            Some((est_content, max_viewport)),
            "theme-picker-scrollbar-track",
            "theme-picker-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(off) =
                    scrollbar::track_click_offset(&this.theme_picker_scroll, ev.position.y)
                {
                    this.theme_picker_scroll
                        .set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.theme_picker_drag = Some(scrollbar::begin_drag(
                    &this.theme_picker_scroll,
                    ev.position.y,
                ));
                cx.stop_propagation();
            }),
        );

        let list_wrapper = div()
            .id("theme-picker-list-wrapper")
            .relative()
            .flex()
            .flex_col()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .child(list)
            .when_some(bar, |d, sb| d.child(sb));

        deferred(
            div()
                .id("theme-picker-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(96.))
                .bg(gpui::hsla(0., 0., 0., 0.4))
                .child(
                    menu_surface(div().id("theme-picker"), ui)
                        .occlude()
                        .track_focus(&self.theme_picker_focus)
                        .on_key_down(cx.listener(Self::handle_theme_picker_key_down))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.close_theme_picker(cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                            if let Some(drag) = this.theme_picker_drag
                                && let Some(off) = scrollbar::drag_offset(
                                    &this.theme_picker_scroll,
                                    &drag,
                                    ev.position.y,
                                )
                            {
                                this.theme_picker_scroll
                                    .set_offset(Point::new(px(0.), px(off)));
                                cx.notify();
                            }
                        }))
                        .on_mouse_up(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                if this.theme_picker_drag.take().is_some() {
                                    cx.notify();
                                }
                            }),
                        )
                        .w(px(520.))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(search_input)
                        .child(list_wrapper),
                ),
        )
        .with_priority(6)
        .into_any_element()
    }
}
