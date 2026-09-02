use gpui::{
    AnyElement, App, ClickEvent, Context, Div, InteractiveElement, IntoElement, ListAlignment,
    ListState, MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Point, Styled,
    div, list, prelude::*, px, svg,
};

use std::collections::HashMap;
use std::ops::Range;

use crate::keybindings::ShortcutGroup;
use crate::settings::components::{
    SETTINGS_CONTROL_CORNER_RADIUS, card_color, destructive_button, hairline, secondary_button,
    section_header_with_action, setting_card,
};
use crate::terminal::element::{MIN_APCA_CONTRAST, ensure_minimum_contrast};
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};
use crate::widgets::scrollbar::{self, ScrollableHandle as _};
use crate::{PaneFlowApp, config_writer, keybindings};

const SHORTCUT_CARD_RADIUS: Pixels = px(13.);

const SHORTCUT_CARD_INSET: Pixels = px(4.);

const SHORTCUT_SECTION_GAP: Pixels = px(16.);

const SHORTCUT_HEADER_GAP: Pixels = px(6.);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ShortcutListRow {
    Header { group: ShortcutGroup, count: usize },
    Binding { idx: usize, first: bool, last: bool },
}

fn shortcut_group_span(rows: &[ShortcutListRow], group: ShortcutGroup) -> Option<Range<usize>> {
    let header = rows
        .iter()
        .position(|row| matches!(row, ShortcutListRow::Header { group: g, .. } if *g == group))?;
    let start = header + 1;
    let len = rows[start..]
        .iter()
        .take_while(|row| matches!(row, ShortcutListRow::Binding { .. }))
        .count();
    Some(start..start + len)
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

        let mut rows =
            Vec::with_capacity(self.effective_shortcuts.len() + ShortcutGroup::ALL.len());
        for group in ShortcutGroup::ALL {
            let Some(indices) = by_group.remove(group) else {
                continue;
            };
            let count = indices.len();
            rows.push(ShortcutListRow::Header {
                group: *group,
                count,
            });
            if !filtering && self.collapsed_shortcut_groups.contains(group) {
                continue;
            }
            for (position, idx) in indices.into_iter().enumerate() {
                rows.push(ShortcutListRow::Binding {
                    idx,
                    first: position == 0,
                    last: position + 1 == count,
                });
            }
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

    fn toggle_shortcut_group(&mut self, group: ShortcutGroup, cx: &mut Context<Self>) {
        if !self.collapsed_shortcut_groups.remove(&group) {
            self.collapsed_shortcut_groups.insert(group);
        }

        let before = shortcut_group_span(&self.shortcut_rows, group);
        self.shortcut_rows = self.shortcut_rows_for(cx);
        let after = shortcut_group_span(&self.shortcut_rows, group);

        match (before, after) {
            (Some(before), Some(after)) if before.start == after.start => {
                self.shortcut_list.splice(before, after.len());
            }
            _ => self.shortcut_list.reset(self.shortcut_rows.len()),
        }
        cx.notify();
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

        let hint = if self.shortcut_capture_active {
            "Press a chord to find what owns it. Escape to leave capture mode."
        } else {
            "Click a row to record a new shortcut. Escape to cancel."
        };

        let body = if self.shortcut_rows.is_empty() {
            div()
                .flex_none()
                .pt(SHORTCUT_SECTION_GAP)
                .child(
                    setting_card(ui).p(SHORTCUT_CARD_INSET).child(
                        div()
                            .px(px(8.))
                            .py(px(14.))
                            .text_size(px(12.))
                            .text_color(ui.muted)
                            .child("No shortcut matches this filter"),
                    ),
                )
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
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .bg(crate::settings::chrome::settings_chrome_bg())
            .flex()
            .flex_col()
            .items_start()
            .child(
                self.settings_reading_column()
                    .flex_1()
                    .min_h_0()
                    .pb(px(20.))
                    .child(heading)
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(SHORTCUT_SECTION_GAP)
                            .child(self.render_shortcut_toolbar(ui, filtering, cx))
                            .child(self.render_shortcut_group_controls(ui, filtering, cx)),
                    )
                    .child(body)
                    .child(
                        div()
                            .flex_none()
                            .pt(SHORTCUT_SECTION_GAP)
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child(hint.to_string()),
                    ),
            )
            .into_any_element()
    }

    fn render_shortcut_list(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let card_bg = card_color();
        let rows = list(
            self.shortcut_list.clone(),
            cx.processor(move |this, index: usize, _window, cx| {
                let Some(row) = this.shortcut_rows.get(index).copied() else {
                    return gpui::Empty.into_any_element();
                };
                match row {
                    ShortcutListRow::Header { group, count } => {
                        this.render_shortcut_section_header(ui, group, count, index == 0, cx)
                    }
                    ShortcutListRow::Binding { idx, first, last } => {
                        this.render_shortcut_row(ui, card_bg, idx, first, last, cx)
                    }
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
            .pt(SHORTCUT_SECTION_GAP)
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
            .pr(scrollbar::SCROLLBAR_GUTTER)
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

    fn render_shortcut_toolbar(
        &self,
        ui: crate::theme::UiColors,
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let capture_active = self.shortcut_capture_active;
        let on_accent = ensure_minimum_contrast(ui.text, ui.accent, MIN_APCA_CONTRAST);

        let field = crate::ui_primitives::filter_pill(
            "shortcut-search",
            "shortcut-search-clear",
            ui,
            self.shortcut_search_input.clone(),
            filtering,
            cx.listener(|this, _: &ClickEvent, _window, cx| {
                this.clear_shortcut_filters(cx);
                cx.notify();
            }),
        )
        .flex_1()
        .min_w_0();

        let capture_toggle = squircle_skin(
            div()
                .id("shortcut-capture-toggle")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .px(px(10.))
                .py(px(5.)),
            "shortcut-capture-skin",
            ROW_RADIUS,
            capture_active.then_some(ui.accent),
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            let next = !this.shortcut_capture_active;
            this.set_shortcut_capture(next, cx);
            if next {
                this.settings_focus.focus(window, cx);
            }
            cx.notify();
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/keyboard.svg")
                .text_color(if capture_active { on_accent } else { ui.muted }),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(if capture_active { on_accent } else { ui.muted })
                .child(if capture_active {
                    "Capturing"
                } else {
                    "Find by key"
                }),
        );

        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .child(field)
            .child(capture_toggle);

        row = if self.shortcut_reset_pending {
            row.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child("Reset all?"),
                    )
                    .child(secondary_button(
                        "reset-shortcuts-cancel",
                        "Cancel",
                        ui,
                        cx.listener(|this, _: &ClickEvent, _w, cx| {
                            this.shortcut_reset_pending = false;
                            cx.notify();
                        }),
                    ))
                    .child(
                        destructive_button("reset-shortcuts-confirm", "Reset").on_click(
                            cx.listener(|this, _: &ClickEvent, _w, cx| {
                                config_writer::reset_shortcuts();
                                let config = paneflow_config::loader::load_config();
                                keybindings::apply_keybindings(cx, &config.shortcuts);
                                this.effective_shortcuts =
                                    keybindings::effective_shortcuts(&config.shortcuts);
                                this.recording_shortcut_idx = None;
                                this.shortcut_reset_pending = false;
                                this.rebuild_shortcut_rows(cx);
                                cx.notify();
                            }),
                        ),
                    ),
            )
        } else {
            row.child(secondary_button(
                "reset-shortcuts",
                "Reset to defaults",
                ui,
                cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.shortcut_reset_pending = true;
                    cx.notify();
                }),
            ))
        };

        row.into_any_element()
    }

    fn render_shortcut_group_controls(
        &self,
        ui: crate::theme::UiColors,
        filtering: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if filtering {
            return section_header_with_action(ui, "Bindings", div()).into_any_element();
        }

        let visible_groups: Vec<ShortcutGroup> = self
            .shortcut_rows
            .iter()
            .filter_map(|row| match row {
                ShortcutListRow::Header { group, .. } => Some(*group),
                ShortcutListRow::Binding { .. } => None,
            })
            .collect();
        let all_collapsed = !visible_groups.is_empty()
            && visible_groups
                .iter()
                .all(|group| self.collapsed_shortcut_groups.contains(group));
        let (label, collapse) = if all_collapsed {
            ("Expand all", false)
        } else {
            ("Collapse all", true)
        };

        section_header_with_action(
            ui,
            "Bindings",
            secondary_button(
                "shortcut-toggle-all",
                label,
                ui,
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.collapsed_shortcut_groups.clear();
                    if collapse {
                        this.collapsed_shortcut_groups
                            .extend(ShortcutGroup::ALL.iter().copied());
                    }
                    this.rebuild_shortcut_rows(cx);
                    cx.notify();
                }),
            ),
        )
        .into_any_element()
    }

    fn render_shortcut_section_header(
        &self,
        ui: crate::theme::UiColors,
        group: ShortcutGroup,
        count: usize,
        first: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed =
            shortcut_group_span(&self.shortcut_rows, group).is_some_and(|span| span.is_empty());

        let header = squircle_skin(
            div()
                .id(("shortcut-group", group as usize))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .px(px(8.))
                .py(px(6.)),
            format!("shortcut-group-skin-{}", group as usize),
            ROW_RADIUS,
            None,
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.toggle_shortcut_group(group, cx);
        }))
        .child(
            svg()
                .size(px(11.))
                .flex_none()
                .path(if collapsed {
                    "icons/chevron-right.svg"
                } else {
                    "icons/chevron-down.svg"
                })
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .truncate()
                .child(group.label()),
        )
        .child(
            div()
                .text_size(px(11.))
                .text_color(ui.muted)
                .child(count.to_string()),
        );

        div()
            .w_full()
            .when(!first, |d| d.pt(SHORTCUT_SECTION_GAP))
            .pb(SHORTCUT_HEADER_GAP)
            .child(header)
            .into_any_element()
    }

    fn render_shortcut_row(
        &self,
        ui: crate::theme::UiColors,
        card_bg: gpui::Hsla,
        idx: usize,
        first: bool,
        last: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.effective_shortcuts.get(idx) else {
            return gpui::Empty.into_any_element();
        };
        let is_recording = self.recording_shortcut_idx == Some(idx);
        let unassigned = entry.key == "Unassigned";

        let key_badge = if is_recording {
            div()
                .px(px(10.))
                .py(px(3.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.accent)
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(ensure_minimum_contrast(
                    ui.text,
                    ui.accent,
                    MIN_APCA_CONTRAST,
                ))
                .child("Press a key…")
        } else {
            div()
                .px(px(10.))
                .py(px(3.))
                .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
                .bg(ui.subtle)
                .text_size(px(11.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(if unassigned { ui.muted } else { ui.text })
                .child(entry.key.clone())
        };

        let row = squircle_skin(
            div()
                .id(("shortcut", idx))
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(8.))
                .py(px(10.)),
            format!("shortcut-squircle-{idx}"),
            ROW_RADIUS,
            None,
            Some(ui.subtle),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.set_shortcut_capture(false, cx);
            this.recording_shortcut_idx = Some(idx);
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

        shortcut_card_slice(card_bg, first, last)
            .child(row)
            .when(!last, |d| d.child(hairline(ui)))
            .into_any_element()
    }
}

fn shortcut_card_slice(card_bg: gpui::Hsla, first: bool, last: bool) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .bg(card_bg)
        .px(SHORTCUT_CARD_INSET)
        .when(first, |d| {
            d.rounded_t(SHORTCUT_CARD_RADIUS).pt(SHORTCUT_CARD_INSET)
        })
        .when(last, |d| {
            d.rounded_b(SHORTCUT_CARD_RADIUS).pb(SHORTCUT_CARD_INSET)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(group: ShortcutGroup, count: usize) -> ShortcutListRow {
        ShortcutListRow::Header { group, count }
    }

    fn binding(idx: usize) -> ShortcutListRow {
        ShortcutListRow::Binding {
            idx,
            first: false,
            last: false,
        }
    }

    #[test]
    fn group_span_covers_only_its_own_bindings() {
        let rows = vec![
            header(ShortcutGroup::Panes, 2),
            binding(0),
            binding(1),
            header(ShortcutGroup::Tabs, 1),
            binding(2),
        ];
        assert_eq!(
            shortcut_group_span(&rows, ShortcutGroup::Panes),
            Some(1..3),
            "the first section must stop at the next header"
        );
        assert_eq!(
            shortcut_group_span(&rows, ShortcutGroup::Tabs),
            Some(4..5),
            "the last section must run to the end"
        );
    }

    #[test]
    fn group_span_of_a_folded_section_is_empty_not_missing() {
        let rows = vec![
            header(ShortcutGroup::Panes, 2),
            header(ShortcutGroup::Tabs, 1),
            binding(2),
        ];
        let span = shortcut_group_span(&rows, ShortcutGroup::Panes).expect("header is present");
        assert!(span.is_empty(), "a folded section owns no rows");
        assert_eq!(
            span.start, 1,
            "unfolding must insert right after the header"
        );
    }

    #[test]
    fn group_span_is_none_when_the_filter_removed_the_section() {
        let rows = vec![header(ShortcutGroup::Tabs, 1), binding(0)];
        assert_eq!(shortcut_group_span(&rows, ShortcutGroup::Panes), None);
    }

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
                let card = card_color();
                let ui = crate::theme::ui_colors();
                list(self.state.clone(), move |index, _window, _cx| {
                    let row = squircle_skin(
                        div()
                            .id(("probe", index))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .px(px(8.))
                            .py(px(10.)),
                        format!("probe-skin-{index}"),
                        ROW_RADIUS,
                        None,
                        Some(ui.subtle),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("action {index}")),
                    )
                    .child(div().px(px(10.)).py(px(3.)).child("Ctrl+X"));
                    shortcut_card_slice(card, index == 0, index + 1 == PROBE_ITEMS)
                        .debug_selector(move || format!("probe-card-{index}"))
                        .child(row)
                        .into_any_element()
                })
                .size_full()
            }
        }

        const PROBE_ITEMS: usize = 6;
        const WIDTH: f32 = 640.0;

        let (view, cx) = cx.add_window_view(|_, _| {
            let state = new_shortcut_list_state();
            state.reset(PROBE_ITEMS);
            Probe { state }
        });
        cx.simulate_resize(gpui::size(px(WIDTH), px(400.0)));
        cx.run_until_parked();

        let painted = cx
            .debug_bounds("probe-card-1")
            .expect("item 1 must be painted");
        let viewport = view.read_with(cx, |probe, _| probe.state.viewport_bounds().size.width);
        assert_eq!(
            painted.size.width, viewport,
            "a list item that does not span the list is shrink-wrapping its content"
        );
    }
}
