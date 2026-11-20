use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, Div, Hsla, InteractiveElement, IntoElement,
    ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent, MouseMoveEvent,
    ParentElement, Pixels, Point, Stateful, StatefulInteractiveElement, Styled, Window, div, list,
    prelude::*, px, svg,
};

use std::collections::HashMap;

use crate::keybindings::ShortcutGroup;
use crate::settings::chrome::{SETTINGS_COLUMN_PADDING, settings_column};
use crate::settings::components::{
    KEYCAP_HEIGHT, KEYCAP_RADIUS, LIST_CARD_PADDING, destructive_color, ghost_icon_button,
    ghost_skin, keycap, keycap_tones, list_card, menu_row_inset, raised_button, select_item_shaped,
    with_alpha,
};
use crate::ui_primitives::interface_style::with_surface_shape;
use crate::ui_primitives::{
    AccessibleControlExt, BODY, BODY_EMPHASIS, FOCUS_BLUE, LABEL_SM, ROW_RADIUS, TooltipDelayExt,
    action_menu, section_eyebrow, text_tooltip,
};
use crate::widgets::scrollbar::{self, ScrollableHandle as _};
use crate::{PaneFlowApp, config_writer, keybindings};

const SHORTCUT_SECTION_GAP: Pixels = px(24.);
const SHORTCUT_LIST_BOTTOM: Pixels = px(32.);
const SHORTCUT_FILTER_GAP: Pixels = px(24.);
const FILTER_HEIGHT: Pixels = px(36.);
const CAPTURE_TOGGLE_SIZE: Pixels = px(28.);
const CAPTURE_ACTIVE_ALPHA: f32 = 0.15;
const CAPTURE_HOVER_ALPHA: f32 = 0.06;
const RESET_GLYPH_SIZE: Pixels = px(22.);
const RESET_GLYPH_REST_OPACITY: f32 = 0.5;
const RECORDING_FIELD_TINT: f32 = 0.12;
const EMPTY_GLYPH_ALPHA: f32 = 0.6;
const EMPTY_STATE_TOP: Pixels = px(48.);
const SHORTCUT_ROW_HEIGHT: Pixels = px(30.);
const SHORTCUT_ROW_CORNER: Pixels = px(8.);
const GROUP_HEADER_HEIGHT: Pixels = px(26.);
const REVEAL_MARGIN: Pixels = px(8.);
const DESCRIPTION_MIN_WIDTH: Pixels = px(96.);
const EMPTY_GLYPH_SIZE: Pixels = px(20.);
const CHORD_GAP: Pixels = px(6.);
const CAPTURE_PROMPT: &str = "Press a shortcut to see what it does";
const RECORDING_PROMPT: &str = "Press a shortcut";
const RECORDING_HINT: &str = if cfg!(target_os = "macos") {
    "Esc cancels · Delete clears"
} else {
    "Esc cancels · Backspace clears"
};

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct ShortcutListRow {
    group: ShortcutGroup,
    indices: Vec<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ShortcutCursor {
    pub(crate) action: &'static str,
    pub(crate) keyboard: bool,
}

pub(crate) struct ShortcutConflict {
    pub(crate) key: String,
    pub(crate) label: String,
    pub(crate) owner: String,
}

pub(crate) fn new_shortcut_list_state() -> ListState {
    ListState::new(0, ListAlignment::Top, px(0.)).measure_all()
}

fn group_label_inset() -> Pixels {
    LIST_CARD_PADDING + menu_row_inset()
}

fn group_top_gap(first: bool) -> Pixels {
    if first { px(0.) } else { SHORTCUT_SECTION_GAP }
}

fn row_top_in_group(first: bool, position: usize) -> Pixels {
    group_top_gap(first)
        + GROUP_HEADER_HEIGHT
        + LIST_CARD_PADDING
        + SHORTCUT_ROW_HEIGHT * position as f32
}

impl PaneFlowApp {
    fn shortcut_rows_for(&self, cx: &App) -> Vec<ShortcutListRow> {
        let query = self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .to_lowercase();

        let matches = |entry: &keybindings::ShortcutEntry| -> bool {
            if query.is_empty() {
                return true;
            }
            if self.shortcut_capture_active {
                return entry.key.to_lowercase() == query
                    || entry
                        .extra_raw_keys
                        .iter()
                        .any(|raw| keybindings::format_keystroke(raw).to_lowercase() == query);
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

        ShortcutGroup::ALL
            .iter()
            .filter_map(|group| {
                by_group.remove(group).map(|indices| ShortcutListRow {
                    group: *group,
                    indices,
                })
            })
            .collect()
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

    fn changed_shortcut_count(&self) -> usize {
        self.effective_shortcuts
            .iter()
            .filter(|entry| entry.customized)
            .count()
    }

    fn visible_shortcut_order(&self) -> Vec<(usize, usize, &'static str)> {
        self.shortcut_rows
            .iter()
            .enumerate()
            .flat_map(|(group_ix, row)| {
                row.indices
                    .iter()
                    .enumerate()
                    .filter_map(move |(position, idx)| {
                        self.effective_shortcuts
                            .get(*idx)
                            .map(|entry| (group_ix, position, entry.action_name))
                    })
            })
            .collect()
    }

    pub(crate) fn navigate_shortcut_list(
        &mut self,
        keystroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if keystroke.modifiers.modified() {
            return false;
        }
        let order = self.visible_shortcut_order();
        let current = self.shortcut_cursor.and_then(|cursor| {
            order
                .iter()
                .position(|(_, _, action)| *action == cursor.action)
        });
        let last = order.len().saturating_sub(1);
        let next = match keystroke.key.as_str() {
            "down" if !order.is_empty() => current.map_or(0, |at| (at + 1).min(last)),
            "up" if !order.is_empty() => current.map_or(last, |at| at.saturating_sub(1)),
            "enter" => {
                let Some(at) = current else {
                    return false;
                };
                let action = order[at].2;
                self.start_shortcut_recording(action, cx);
                self.settings_focus.focus(window, cx);
                cx.notify();
                return true;
            }
            _ => return false,
        };
        let (group_ix, position, action) = order[next];
        self.shortcut_cursor = Some(ShortcutCursor {
            action,
            keyboard: true,
        });
        self.reveal_shortcut_row(group_ix, position);
        cx.notify();
        true
    }

    fn reveal_shortcut_row(&self, group_ix: usize, position: usize) {
        let first = group_ix == 0;
        let row_top = row_top_in_group(first, position);
        let reveal_top = if position == 0 {
            group_top_gap(first)
        } else {
            row_top - REVEAL_MARGIN
        };
        let Some(item) = self.shortcut_list.bounds_for_item(group_ix) else {
            self.shortcut_list.scroll_to(ListOffset {
                item_ix: group_ix,
                offset_in_item: reveal_top.max(px(0.)),
            });
            return;
        };
        let viewport = self.shortcut_list.viewport_bounds();
        let top = item.top() + reveal_top;
        let bottom = item.top() + row_top + SHORTCUT_ROW_HEIGHT + REVEAL_MARGIN;
        if top < viewport.top() {
            self.shortcut_list.scroll_by(top - viewport.top());
        } else if bottom > viewport.bottom() {
            self.shortcut_list.scroll_by(bottom - viewport.bottom());
        }
    }

    pub(crate) fn render_shortcuts_heading(
        &self,
        title: Div,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        div()
            .pb(px(20.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .child(title.min_w_0())
            .children(self.render_shortcut_reset_control(ui, cx))
            .into_any_element()
    }

    fn render_shortcut_reset_control(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let changed = self.changed_shortcut_count();
        if self.shortcut_reset_armed_at.is_some() {
            let question = match changed {
                1 => "Reset 1 changed shortcut?".to_string(),
                n => format!("Reset {n} changed shortcuts?"),
            };
            return Some(
                div()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .child(
                        div()
                            .pr(px(8.))
                            .text_size(BODY)
                            .text_color(ui.muted)
                            .child(question),
                    )
                    .child(
                        ghost_pill("reset-shortcuts-cancel", ui)
                            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                                this.shortcut_reset_armed_at = None;
                                cx.notify();
                            }))
                            .child("Cancel"),
                    )
                    .child(
                        raised_button(
                            "reset-shortcuts-confirm",
                            "Reset",
                            destructive_color(),
                            gpui::white(),
                        )
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.confirm_shortcut_reset(cx);
                            },
                        )),
                    )
                    .into_any_element(),
            );
        }
        (changed > 0).then(|| {
            ghost_icon_button(
                "reset-shortcuts",
                "Reset all",
                "icons/settings/undo.svg",
                ui,
            )
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.arm_shortcut_reset();
                cx.notify();
            }))
            .into_any_element()
        })
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
            self.render_shortcut_empty_state(ui, cx)
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
                        .pb(SHORTCUT_FILTER_GAP)
                        .child(heading)
                        .child(self.render_shortcut_filter(ui, filtering, cx)),
                )
                .flex_none(),
            )
            .child(body)
            .into_any_element()
    }

    fn render_shortcut_empty_state(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self
            .shortcut_search_input
            .read(cx)
            .value()
            .trim()
            .to_string();
        let message = if self.shortcut_capture_active {
            format!("Nothing uses {query}")
        } else {
            format!("No shortcut matches \u{201c}{query}\u{201d}")
        };
        list_column(
            div()
                .w_full()
                .pt(EMPTY_STATE_TOP)
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .child(
                    svg()
                        .size(EMPTY_GLYPH_SIZE)
                        .flex_none()
                        .path("icons/settings/keyboard.svg")
                        .text_color(with_alpha(ui.muted, EMPTY_GLYPH_ALPHA)),
                )
                .child(div().text_size(BODY).text_color(ui.muted).child(message)),
        )
        .flex_none()
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
                let last = index + 1 == this.shortcut_rows.len();
                this.render_shortcut_group(ui, row.group, &row.indices, index == 0, last, cx)
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
                .truncate()
                .text_size(BODY_EMPHASIS)
                .text_color(ui.muted)
                .child(CAPTURE_PROMPT)
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
                .accessible_control(gpui::accesskit::Role::Button, "Clear search")
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
                        .path("icons/settings/cancel-01.svg")
                        .text_color(ui.text),
                )
        });

        let accent = ui.accent;
        let toggle_rest = if capture {
            with_alpha(accent, CAPTURE_ACTIVE_ALPHA)
        } else {
            with_alpha(ui.text, 0.)
        };
        let toggle_hover = if capture {
            toggle_rest
        } else {
            with_alpha(ui.text, CAPTURE_HOVER_ALPHA)
        };
        let capture_toggle = div()
            .id("shortcut-filter-by-key")
            .size(CAPTURE_TOGGLE_SIZE)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(toggle_rest)
            .hover(move |style| style.bg(toggle_hover))
            .cursor(CursorStyle::PointingHand)
            .accessible_control(
                gpui::accesskit::Role::Button,
                if capture {
                    "Stop finding by keys"
                } else {
                    "Find by keys"
                },
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                if capture {
                    this.clear_shortcut_filters(cx);
                } else {
                    this.set_shortcut_capture(true, cx);
                    this.settings_focus.focus(window, cx);
                }
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(16.))
                    .path("icons/settings/keyboard.svg")
                    .text_color(if capture { accent } else { ui.muted }),
            );

        div()
            .id("shortcut-search")
            .flex_none()
            .h(FILTER_HEIGHT)
            .pl(px(12.))
            .pr(px(4.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .rounded_full()
            .bg(ui.subtle)
            .cursor_text()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| {
                    if this.recording_shortcut.is_some() {
                        this.cancel_shortcut_recording();
                        cx.notify();
                    }
                }),
            )
            .child(
                svg()
                    .size(px(16.))
                    .flex_none()
                    .path("icons/settings/search-01.svg")
                    .text_color(ui.muted),
            )
            .child(body)
            .children(clear)
            .child(capture_toggle)
            .into_any_element()
    }

    fn render_shortcut_group(
        &self,
        ui: crate::theme::UiColors,
        group: ShortcutGroup,
        indices: &[usize],
        first: bool,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .h(GROUP_HEADER_HEIGHT)
            .pb(px(8.))
            .px(group_label_inset())
            .flex()
            .flex_row()
            .items_baseline()
            .gap(px(6.))
            .min_w_0()
            .child(section_eyebrow(group.label(), ui).flex_none())
            .children(group.context_hint().map(|hint| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(hint)
            }));

        let rows = indices
            .iter()
            .map(|idx| self.render_shortcut_row(ui, *idx, cx));

        list_column(
            div()
                .pt(group_top_gap(first))
                .when(last, |d| d.pb(SHORTCUT_LIST_BOTTOM))
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
        let action_name = entry.action_name;
        let is_recording = self.recording_shortcut == Some(action_name);
        let conflict = self.shortcut_conflict.as_ref().filter(|_| is_recording);
        let row_group = format!("shortcut-{idx}-squircle");
        let keyboard_focused = !is_recording
            && self
                .shortcut_cursor
                .is_some_and(|cursor| cursor.keyboard && cursor.action == action_name);
        let chord_label = if entry.key == "Unassigned" {
            "unassigned".to_string()
        } else {
            entry.key.clone()
        };

        let trailing = if let Some(conflict) = conflict {
            conflict_trailing(ui, conflict)
        } else if is_recording {
            recording_trailing(ui)
        } else {
            let reset = entry.customized.then(|| {
                ghost_skin(
                    div()
                        .id(("shortcut-reset", idx))
                        .size(RESET_GLYPH_SIZE)
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center(),
                    format!("shortcut-reset-{idx}-squircle"),
                    ROW_RADIUS,
                    ui,
                )
                .opacity(RESET_GLYPH_REST_OPACITY)
                .group_hover(row_group.clone(), |style| style.opacity(1.))
                .accessible_control(gpui::accesskit::Role::Button, "Reset to default")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                    cx.stop_propagation();
                    this.persist_shortcut_change(
                        move || config_writer::reset_shortcut(action_name),
                        cx,
                    );
                }))
                .child(
                    svg()
                        .size(px(13.))
                        .path("icons/settings/undo.svg")
                        .text_color(ui.text),
                )
            });
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .children(reset)
                .child(chord_list(ui, entry))
        };

        shortcut_row(("shortcut", idx), is_recording || keyboard_focused, ui)
            .relative()
            .role(gpui::accesskit::Role::Button)
            .aria_label(format!("{}, {chord_label}", entry.description))
            .when(keyboard_focused, |row| {
                row.child(crate::app::usage::focus_ring(SHORTCUT_ROW_CORNER, ui))
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.shortcut_cursor = Some(ShortcutCursor {
                    action: action_name,
                    keyboard: false,
                });
                this.start_shortcut_recording(action_name, cx);
                this.settings_focus.focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .id(("shortcut-label", idx))
                    .flex_1()
                    .min_w(DESCRIPTION_MIN_WIDTH)
                    .text_size(BODY)
                    .text_color(ui.text)
                    .truncate()
                    .delayed_tooltip(text_tooltip(entry.description.clone()))
                    .child(entry.description.clone()),
            )
            .child(trailing.min_w_0())
            .into_any_element()
    }
}

fn shortcut_row(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    with_surface_shape(|| {
        select_item_shaped(
            id,
            selected,
            ui,
            with_alpha(ui.text, action_menu::HIGHLIGHT_ALPHA),
            SHORTCUT_ROW_CORNER,
        )
        .h(SHORTCUT_ROW_HEIGHT)
    })
}

fn shortcut_group_card(
    ui: crate::theme::UiColors,
    rows: impl IntoIterator<Item = AnyElement>,
) -> Div {
    list_card(div(), ui).children(rows)
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

fn ghost_pill(id: &'static str, ui: crate::theme::UiColors) -> crate::ui_primitives::AnimatedHover {
    ghost_skin(
        div()
            .id(id)
            .flex_none()
            .h(px(28.))
            .px(px(10.))
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(ui.text),
        format!("{id}-squircle"),
        ROW_RADIUS,
        ui,
    )
}

fn chord_list(ui: crate::theme::UiColors, entry: &keybindings::ShortcutEntry) -> Div {
    let row = div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(CHORD_GAP);
    if entry.key == "Unassigned" {
        return row.child(
            div()
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child("Unassigned"),
        );
    }
    let chords = std::iter::once(entry.key.clone()).chain(
        entry
            .extra_raw_keys
            .iter()
            .map(|raw| keybindings::format_keystroke(raw)),
    );
    let mut row = row;
    for (position, chord) in chords.enumerate() {
        if position > 0 {
            row = row.child(div().text_size(LABEL_SM).text_color(ui.muted).child("or"));
        }
        row = row.child(keycaps(ui, &chord, KeycapTone::Normal));
    }
    row
}

fn recording_trailing(ui: crate::theme::UiColors) -> Div {
    let blue: Hsla = gpui::rgb(FOCUS_BLUE).into();
    div()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(LABEL_SM)
                .text_color(ui.muted)
                .child(RECORDING_HINT),
        )
        .child(
            div()
                .flex_none()
                .h(KEYCAP_HEIGHT)
                .px(px(8.))
                .flex()
                .items_center()
                .rounded(KEYCAP_RADIUS)
                .bg(with_alpha(blue, RECORDING_FIELD_TINT))
                .border_1()
                .border_color(blue)
                .text_size(LABEL_SM)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(RECORDING_PROMPT),
        )
}

fn conflict_trailing(ui: crate::theme::UiColors, conflict: &ShortcutConflict) -> Div {
    div()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .text_size(LABEL_SM)
                .child(
                    svg()
                        .size(px(12.))
                        .flex_none()
                        .path("icons/settings/alert-02.svg")
                        .text_color(ui.vc_conflict),
                )
                .child(
                    div()
                        .text_color(ui.vc_conflict)
                        .child(format!("Used by {}", conflict.owner)),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.muted)
                        .child("· press again to reassign"),
                ),
        )
        .child(keycaps(ui, &conflict.label, KeycapTone::Conflict))
}

#[derive(Clone, Copy)]
enum KeycapTone {
    Normal,
    Conflict,
}

fn keycaps(ui: crate::theme::UiColors, formatted: &str, tone: KeycapTone) -> Div {
    let (fill, edge, ink) = match tone {
        KeycapTone::Normal => keycap_tones(ui),
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
                .map(|cap| keycap(cap, fill, edge, ink)),
        )
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
                        shortcut_row(("probe", index * 10 + row), false, ui)
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
