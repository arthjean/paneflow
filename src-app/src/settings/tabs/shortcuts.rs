use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, Div, InteractiveElement, IntoElement,
    ListAlignment, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels,
    Point, Stateful, StatefulInteractiveElement, Styled, div, list, prelude::*, px, svg,
};

use std::collections::HashMap;

use crate::keybindings::ShortcutGroup;
use crate::settings::chrome::{SETTINGS_COLUMN_PADDING, settings_column};
use crate::settings::components::{
    MENU_ROW_HEIGHT, destructive_button, menu_panel, menu_row, select_item, with_alpha,
};
use crate::ui_primitives::{
    BODY, BODY_EMPHASIS, FOCUS_BLUE, LABEL_SM, ROW_RADIUS, TooltipDelayExt, lerp_color,
    squircle_skin, text_tooltip,
};
use crate::widgets::scrollbar::{self, ScrollableHandle as _};
use crate::{PaneFlowApp, config_writer, keybindings};

const SHORTCUT_SECTION_GAP: Pixels = px(20.);

const KEYCAP_HEIGHT: Pixels = px(20.);

const KEYCAP_RADIUS: Pixels = px(5.);

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) enum ShortcutListRow {
    Group {
        group: ShortcutGroup,
        indices: Vec<usize>,
    },
    Footer,
}

pub(crate) struct ShortcutConflict {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) owner: String,
}

pub(crate) fn new_shortcut_list_state() -> ListState {
    ListState::new(0, ListAlignment::Top, px(0.)).measure_all()
}

impl PaneFlowApp {
    fn shortcut_rows_for(&self, cx: &App) -> Vec<ShortcutListRow> {
        let query = self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();
        let filtering = !query.is_empty();

        let matches = |entry: &keybindings::ShortcutEntry| -> bool {
            if query.is_empty() {
                return true;
            }
            if self.shortcut_capture_active {
                return entry.key.to_lowercase() == query;
            }
            entry.description.to_lowercase().contains(&query)
                || entry.key.to_lowercase().contains(&query)
                || entry.search_key.contains(&query)
        };

        let mut by_group: HashMap<ShortcutGroup, Vec<usize>> = HashMap::new();
        for (idx, entry) in self.effective_shortcuts.iter().enumerate() {
            if matches(entry) {
                by_group.entry(entry.group).or_default().push(idx);
            }
        }

        let mut rows = Vec::with_capacity(ShortcutGroup::ALL.len() + 1);
        for group in ShortcutGroup::ALL {
            if let Some(indices) = by_group.remove(group) {
                rows.push(ShortcutListRow::Group {
                    group: *group,
                    indices,
                });
            }
        }
        if !filtering && !rows.is_empty() {
            rows.push(ShortcutListRow::Footer);
        }
        rows
    }

    pub(crate) fn rebuild_shortcut_rows(&mut self, cx: &mut Context<Self>) {
        let previous_len = self.shortcut_rows.len();
        let previous_top = self.shortcut_list.logical_scroll_top();

        self.shortcut_rows = self.shortcut_rows_for(cx);
        let len = self.shortcut_rows.len();
        self.shortcut_list.reset(len);

        if len > 0 && len == previous_len {
            self.shortcut_list.scroll_to(previous_top);
        }
    }

    pub(crate) fn render_shortcuts_page(
        &self,
        heading: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let filtering = !self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .is_empty();

        let body = if self.shortcut_rows.is_empty() {
            let empty = if self.shortcut_capture_active {
                format!(
                    "Nothing is bound to {}",
                    self.shortcut_search_input.read(cx).value().trim()
                )
            } else {
                "No shortcut matches this filter".to_string()
            };
            list_column(
                menu_panel(div(), ui).child(
                    div()
                        .h(MENU_ROW_HEIGHT)
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .text_size(BODY)
                        .text_color(ui.muted)
                        .child(empty),
                ),
            )
            .flex_none()
            .into_any_element()
        } else {
            self.render_shortcut_list(ui, cx)
        };

        div()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(
                list_column(
                    div()
                        .pt(SETTINGS_COLUMN_PADDING)
                        .child(heading)
                        .child(
                            div()
                                .flex_none()
                                .pb(px(16.))
                                .text_size(BODY)
                                .line_height(px(18.))
                                .text_color(ui.muted)
                                .child(
                                    "Click a row and press the new chord. Backspace clears it, \
                                     Esc cancels. A dot marks a binding you changed.",
                                ),
                        )
                        .child(self.render_shortcut_filter(ui, filtering, cx))
                        .pb(SHORTCUT_SECTION_GAP),
                )
                .flex_none(),
            )
            .child(body)
            .into_any_element()
    }

    fn render_shortcut_list(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let rows = list(
            self.shortcut_list.clone(),
            cx.processor(move |this, index: usize, _window, cx| {
                let Some(row) = this.shortcut_rows.get(index).cloned() else {
                    return gpui::Empty.into_any_element();
                };
                match row {
                    ShortcutListRow::Group { group, indices } => {
                        this.render_shortcut_group(ui, group, &indices, index == 0, cx)
                    }
                    ShortcutListRow::Footer => this.render_shortcut_footer(ui, cx),
                }
            }),
        )
        .size_full();

        let bar = scrollbar::render(
            &self.shortcut_list,
            ui,
            None,
            "shortcut-scrollbar-track",
            "shortcut-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(off) = scrollbar::track_click_offset(&this.shortcut_list, ev.position.y)
                {
                    this.shortcut_list.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.shortcut_drag =
                    Some(scrollbar::begin_drag(&this.shortcut_list, ev.position.y));
                cx.stop_propagation();
            }),
        );

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.shortcut_list_region(rows, bar, cx))
            .into_any_element()
    }

    fn shortcut_list_region(
        &self,
        rows: gpui::List,
        bar: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(drag) = this.shortcut_drag
                    && let Some(off) =
                        scrollbar::drag_offset(&this.shortcut_list, &drag, ev.position.y)
                {
                    this.shortcut_list.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let drag = this.shortcut_drag.take();
                    if scrollbar::end_drag(&this.shortcut_list, drag) {
                        cx.notify();
                    }
                }),
            )
            .child(rows)
            .when_some(bar, |d, sb| d.child(sb))
    }

    fn render_shortcut_filter(
        &self,
        ui: crate::theme::UiColors,
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture = self.shortcut_capture_active;

        let body = if capture && !filtering {
            div()
                .flex_1()
                .min_w_0()
                .text_size(BODY_EMPHASIS)
                .text_color(ui.muted)
                .child("Press a chord to see what owns it")
        } else {
            div()
                .flex_1()
                .min_w_0()
                .text_size(BODY_EMPHASIS)
                .text_color(ui.text)
                .child(self.shortcut_search_input.clone())
        };

        let clear = filtering.then(|| {
            div()
                .id("shortcut-search-clear")
                .size(px(18.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .bg(ui.muted.opacity(0.2))
                .cursor(CursorStyle::PointingHand)
                .delayed_tooltip(text_tooltip("Clear filter"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _: &ClickEvent, _window, cx| {
                    this.shortcut_search_input.update(cx, |input, cx| {
                        input.clear(cx);
                    });
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(12.))
                        .path("icons/close.svg")
                        .text_color(ui.text),
                )
        });

        let modes = div()
            .flex_none()
            .flex()
            .flex_row()
            .gap(px(2.))
            .child(
                filter_mode_button("shortcut-filter-by-name", "Name", !capture, ui).on_click(
                    cx.listener(|this, _: &ClickEvent, _window, cx| {
                        this.clear_shortcut_filters(cx);
                        cx.notify();
                    }),
                ),
            )
            .child(
                filter_mode_button("shortcut-filter-by-key", "Key", capture, ui).on_click(
                    cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.set_shortcut_capture(true, cx);
                        this.settings_focus.focus(window, cx);
                        cx.notify();
                    }),
                ),
            );

        div()
            .id("shortcut-search")
            .flex_none()
            .h(px(36.))
            .pl(px(12.))
            .pr(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .rounded_full()
            .bg(ui.subtle)
            .cursor_text()
            .child(
                svg()
                    .size(px(16.))
                    .flex_none()
                    .path(if capture {
                        "icons/keyboard.svg"
                    } else {
                        "icons/tool_search.svg"
                    })
                    .text_color(if capture { ui.accent } else { ui.muted }),
            )
            .child(body)
            .children(clear)
            .child(modes)
            .into_any_element()
    }

    fn render_shortcut_group(
        &self,
        ui: crate::theme::UiColors,
        group: ShortcutGroup,
        indices: &[usize],
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = match group.context_hint() {
            Some(hint) => format!("{} · {}", group.label(), hint),
            None => group.label().to_string(),
        };
        let header = div()
            .pb(px(8.))
            .px(px(4.))
            .flex()
            .flex_row()
            .items_baseline()
            .justify_between()
            .gap(px(12.))
            .child(
                div()
                    .min_w_0()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .truncate()
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(indices.len().to_string()),
            );

        let rows = indices
            .iter()
            .map(|idx| self.render_shortcut_row(ui, *idx, cx));

        list_column(
            div()
                .when(!first, |d| d.pt(SHORTCUT_SECTION_GAP))
                .child(header)
                .child(shortcut_group_card(ui, rows)),
        )
        .into_any_element()
    }

    fn render_shortcut_row(
        &self,
        ui: crate::theme::UiColors,
        idx: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.effective_shortcuts.get(idx) else {
            return gpui::Empty.into_any_element();
        };
        let is_recording = self.recording_shortcut_idx == Some(idx);
        let conflict = self.shortcut_conflict.as_ref().filter(|_| is_recording);
        let action_name = entry.action_name;
        let row_group = format!("shortcut-{idx}-squircle");

        let trailing = if let Some(conflict) = conflict {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.))
                        .text_size(LABEL_SM)
                        .text_color(ui.vc_conflict)
                        .child(
                            svg()
                                .size(px(12.))
                                .flex_none()
                                .path("icons/triangle-alert.svg")
                                .text_color(ui.vc_conflict),
                        )
                        .child(format!("Also {} · press again to take it", conflict.owner)),
                )
                .child(keycaps(ui, &conflict.label, KeycapTone::Conflict))
        } else if is_recording {
            recording_field()
        } else {
            let reset = entry.customized.then(|| {
                squircle_skin(
                    div()
                        .id(("shortcut-reset", idx))
                        .size(px(24.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor(CursorStyle::PointingHand),
                    format!("shortcut-reset-{idx}-squircle"),
                    ROW_RADIUS,
                    None,
                    Some(with_alpha(ui.text, 0.10)),
                )
                .invisible()
                .group_hover(row_group.clone(), |style| style.visible())
                .delayed_tooltip(text_tooltip("Reset to default"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    if !config_writer::reset_shortcut(action_name) {
                        this.show_toast("Could not save shortcut", cx);
                    }
                    this.reload_shortcuts(cx);
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .path("icons/refresh.svg")
                        .text_color(ui.muted),
                )
            });
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .children(reset)
                .child(if entry.key == "Unassigned" {
                    unassigned_cap(ui)
                } else {
                    keycaps(ui, &entry.key, KeycapTone::Normal)
                })
        };

        menu_row(("shortcut", idx), false, ui)
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.start_shortcut_recording(idx, cx);
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .size(px(5.))
                    .flex_none()
                    .rounded_full()
                    .when(entry.customized, |dot| dot.bg(ui.accent)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_color(ui.text)
                    .truncate()
                    .child(entry.description.clone()),
            )
            .child(trailing)
            .into_any_element()
    }

    fn render_shortcut_footer(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let total = self.effective_shortcuts.len();
        let changed = self
            .effective_shortcuts
            .iter()
            .filter(|entry| entry.customized)
            .count();

        let row = div()
            .h(MENU_ROW_HEIGHT)
            .pl(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.));

        let row = if self.shortcut_reset_pending {
            row.child(
                div()
                    .text_size(BODY)
                    .text_color(ui.text)
                    .child(format!("Reset all {total} shortcuts to their defaults?")),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        select_item("reset-shortcuts-cancel", false, ui)
                            .text_color(ui.text)
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.shortcut_reset_pending = false;
                                cx.notify();
                            }))
                            .child("Cancel"),
                    )
                    .child(
                        destructive_button("reset-shortcuts-confirm", "Reset").on_click(
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                config_writer::reset_shortcuts();
                                this.shortcut_reset_pending = false;
                                this.reload_shortcuts(cx);
                                cx.notify();
                            }),
                        ),
                    ),
            )
        } else {
            let summary = match changed {
                0 => "Every shortcut is at its default.".to_string(),
                1 => "1 shortcut differs from its default.".to_string(),
                n => format!("{n} shortcuts differ from their defaults."),
            };
            row.child(div().text_size(BODY).text_color(ui.muted).child(summary))
                .child(
                    select_item("reset-shortcuts", false, ui)
                        .text_color(ui.text)
                        .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                            this.cancel_shortcut_recording();
                            this.shortcut_reset_pending = true;
                            cx.notify();
                        }))
                        .child("Reset all to defaults"),
                )
        };

        list_column(
            div()
                .pt(SHORTCUT_SECTION_GAP)
                .pb(px(20.))
                .child(menu_panel(div(), ui).child(row)),
        )
        .into_any_element()
    }
}

fn shortcut_group_card(
    ui: crate::theme::UiColors,
    rows: impl IntoIterator<Item = AnyElement>,
) -> Div {
    menu_panel(div(), ui).children(rows)
}

fn list_column(inner: impl IntoElement) -> Div {
    div()
        .w_full()
        .pr(scrollbar::SCROLLBAR_GUTTER)
        .flex()
        .flex_col()
        .items_start()
        .child(settings_column().child(inner))
}

fn filter_mode_button(
    id: &'static str,
    label: &'static str,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    let selected_bg = with_alpha(ui.text, 0.10);
    let hover_bg = if selected {
        selected_bg
    } else {
        with_alpha(ui.text, 0.05)
    };
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .items_center()
        .rounded_full()
        .when(selected, |button| button.bg(selected_bg))
        .hover(move |style| style.bg(hover_bg))
        .cursor(CursorStyle::PointingHand)
        .text_size(LABEL_SM)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(if selected { ui.text } else { ui.muted })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label)
}

#[derive(Clone, Copy)]
enum KeycapTone {
    Normal,
    Conflict,
}

fn keycaps(ui: crate::theme::UiColors, formatted: &str, tone: KeycapTone) -> Div {
    let (fill, edge, ink) = match tone {
        KeycapTone::Normal => (ui.subtle, lerp_color(ui.subtle, ui.text, 0.08), ui.text),
        KeycapTone::Conflict => (
            with_alpha(ui.vc_conflict, 0.14),
            with_alpha(ui.vc_conflict, 0.5),
            ui.vc_conflict,
        ),
    };
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.))
        .children(
            keybindings::keystroke_caps(formatted)
                .into_iter()
                .map(|cap| {
                    div()
                        .h(KEYCAP_HEIGHT)
                        .min_w(KEYCAP_HEIGHT)
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(KEYCAP_RADIUS)
                        .bg(fill)
                        .border_1()
                        .border_color(edge)
                        .text_size(LABEL_SM)
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(ink)
                        .child(cap)
                }),
        )
}

fn recording_field() -> Div {
    div()
        .flex_none()
        .h(KEYCAP_HEIGHT)
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(KEYCAP_RADIUS)
        .bg(gpui::rgb(FOCUS_BLUE))
        .text_size(LABEL_SM)
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(gpui::white())
        .child("Press keys…")
}

fn unassigned_cap(ui: crate::theme::UiColors) -> Div {
    div()
        .flex_none()
        .h(KEYCAP_HEIGHT)
        .px(px(7.))
        .flex()
        .items_center()
        .rounded(KEYCAP_RADIUS)
        .border_1()
        .border_dashed()
        .border_color(with_alpha(ui.muted, 0.5))
        .text_size(LABEL_SM)
        .text_color(ui.muted)
        .child("Unassigned")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn list_items_span_the_full_list_width(cx: &mut gpui::TestAppContext) {
        struct Probe {
            state: ListState,
        }

        impl gpui::Render for Probe {
            fn render(
                &mut self,
                _window: &mut gpui::Window,
                _cx: &mut Context<Self>,
            ) -> impl IntoElement {
                let ui = crate::theme::ui_colors();
                list(self.state.clone(), move |index, _window, _cx| {
                    let rows = (0..3).map(|row| {
                        menu_row(("probe", index * 10 + row), false, ui)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(format!("action {row}")),
                            )
                            .child(keycaps(ui, "Ctrl+X", KeycapTone::Normal))
                            .into_any_element()
                    });
                    list_column(
                        shortcut_group_card(ui, rows)
                            .debug_selector(move || format!("probe-card-{index}")),
                    )
                    .debug_selector(move || format!("probe-item-{index}"))
                    .into_any_element()
                })
                .size_full()
            }
        }

        const PROBE_ITEMS: usize = 4;
        const WIDTH: f32 = 640.0;

        let (view, cx) = cx.add_window_view(|_, _| {
            let state = new_shortcut_list_state();
            state.reset(PROBE_ITEMS);
            Probe { state }
        });
        cx.simulate_resize(gpui::size(px(WIDTH), px(400.0)));
        cx.run_until_parked();

        let item = cx
            .debug_bounds("probe-item-1")
            .expect("item 1 must be painted");
        let card = cx
            .debug_bounds("probe-card-1")
            .expect("card 1 must be painted");
        let viewport = view.read_with(cx, |probe, _| probe.state.viewport_bounds().size.width);
        assert_eq!(
            item.size.width, viewport,
            "a list item that does not span the list is shrink-wrapping its content"
        );
        let expected_card = px(WIDTH) - scrollbar::SCROLLBAR_GUTTER - SETTINGS_COLUMN_PADDING * 2.;
        assert_eq!(
            card.size.width, expected_card,
            "the card must sit in the reading column, clear of the scrollbar gutter"
        );
        assert_eq!(
            card.origin.x, SETTINGS_COLUMN_PADDING,
            "the card must start at the column padding"
        );
    }
}
