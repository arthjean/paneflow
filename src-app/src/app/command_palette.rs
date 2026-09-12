use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, ParentElement, SharedString, Styled, Window, deferred, div, prelude::*, px,
};

use crate::PaneFlowApp;
use crate::settings::components::{menu_divider_color, menu_surface, select_item};

const COMMAND_PALETTE_WIDTH: f32 = 544.0;
const COMMAND_PALETTE_MAX_LIST_HEIGHT: f32 = 360.0;

pub(crate) struct CommandMatch {
    pub(crate) action_name: &'static str,
    pub(crate) description: String,
    pub(crate) shortcut: Option<String>,
}

fn matches_query(haystack: &str, query: &str) -> bool {
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

impl PaneFlowApp {
    pub(crate) fn command_palette_matches(&self) -> Vec<CommandMatch> {
        let query = self.command_palette_query.to_lowercase();
        let mut matches: Vec<CommandMatch> = self
            .effective_shortcuts
            .iter()
            .filter(|entry| entry.action_name != "open_command_palette")
            .filter(|entry| crate::keybindings::action_is_global(entry.action_name))
            .filter(|entry| {
                query.is_empty() || matches_query(&entry.description.to_lowercase(), &query)
            })
            .map(|entry| CommandMatch {
                action_name: entry.action_name,
                description: entry.description.clone(),
                shortcut: (entry.key != "Unassigned").then(|| entry.key.clone()),
            })
            .collect();
        matches.sort_by(|a, b| a.description.cmp(&b.description));
        matches
    }

    pub(crate) fn open_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.command_palette_open {
            self.close_command_palette(cx);
            return;
        }
        self.dismiss_transient_surfaces();
        self.command_palette_open = true;
        self.command_palette_query.clear();
        self.command_palette_selected = 0;
        self.command_palette_scroll = gpui::ScrollHandle::new();
        self.command_palette_focus.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn close_command_palette(&mut self, cx: &mut Context<Self>) {
        if !self.command_palette_open {
            return;
        }
        self.command_palette_open = false;
        self.command_palette_query.clear();
        self.command_palette_selected = 0;
        cx.notify();
    }

    pub(crate) fn handle_open_command_palette(
        &mut self,
        _: &crate::OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_command_palette(window, cx);
    }

    fn command_palette_run(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(action) = self
            .command_palette_matches()
            .get(idx)
            .and_then(|entry| crate::keybindings::action_for_name(entry.action_name))
        else {
            return;
        };
        self.close_command_palette(cx);
        window.dispatch_action(action, cx);
    }

    pub(crate) fn handle_command_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self.command_palette_matches().len();
        match event.keystroke.key.as_str() {
            "escape" => self.close_command_palette(cx),
            "enter" => {
                if self.command_palette_selected < len {
                    self.command_palette_run(self.command_palette_selected, window, cx);
                }
            }
            "up" => {
                if self.command_palette_selected > 0 {
                    self.command_palette_selected -= 1;
                    cx.notify();
                }
            }
            "down" => {
                if self.command_palette_selected + 1 < len {
                    self.command_palette_selected += 1;
                    cx.notify();
                }
            }
            "backspace" => {
                if self.command_palette_query.pop().is_some() {
                    self.command_palette_selected = 0;
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
                    self.command_palette_query.push_str(ch);
                    self.command_palette_selected = 0;
                    cx.notify();
                }
            }
        }
    }

    pub(crate) fn render_command_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let matches = self.command_palette_matches();

        let query_text: SharedString = if self.command_palette_query.is_empty() {
            "Execute a command…".into()
        } else {
            format!("{}|", self.command_palette_query).into()
        };
        let query_color = if self.command_palette_query.is_empty() {
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
            .id("command-palette-list")
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .max_h(px(COMMAND_PALETTE_MAX_LIST_HEIGHT))
            .overflow_y_scroll()
            .track_scroll(&self.command_palette_scroll);

        if matches.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No matching command"),
            );
        } else {
            for (idx, entry) in matches.iter().enumerate() {
                let is_selected = idx == self.command_palette_selected;
                list = list.child(
                    select_item(
                        SharedString::from(format!("command-palette-row-{idx}")),
                        is_selected,
                        ui,
                    )
                    .cursor(CursorStyle::PointingHand)
                    .justify_between()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.command_palette_run(idx, window, cx);
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
                            .child(entry.description.clone()),
                    )
                    .when_some(entry.shortcut.clone(), |row, key| {
                        row.child(
                            div()
                                .flex_none()
                                .pl(px(8.))
                                .text_size(px(11.))
                                .text_color(ui.muted)
                                .child(key),
                        )
                    }),
                );
            }
        }

        deferred(
            div()
                .id("command-palette-backdrop")
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
                    menu_surface(div().id("command-palette"), ui)
                        .occlude()
                        .track_focus(&self.command_palette_focus)
                        .on_key_down(cx.listener(Self::handle_command_palette_key_down))
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.close_command_palette(cx);
                        }))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .w(px(COMMAND_PALETTE_WIDTH))
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .child(search_input)
                        .child(list),
                ),
        )
        .with_priority(7)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::matches_query;

    #[test]
    fn a_query_matches_a_description_in_any_word_order() {
        assert!(matches_query("split horizontal", "horizontal split"));
    }

    #[test]
    fn a_query_matches_on_a_prefix_of_a_word() {
        assert!(matches_query("launch pad", "laun"));
    }

    #[test]
    fn a_query_rejects_a_word_the_description_lacks() {
        assert!(!matches_query("split horizontal", "split vertical"));
    }
}
