use std::ops::Range;
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    Anchor, AnyElement, Bounds, ClickEvent, Context, Focusable, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, Render, ScrollStrategy, SharedString, Size,
    StatefulInteractiveElement, Styled, Window, anchored, deferred, div, img, point, px, svg,
    uniform_list,
};

use super::listing::{Listing, Row};
use super::{PathPicker, PathPickerEvent};
use crate::settings::components::{select_item_shaped, select_menu_surface, with_alpha};
use crate::ui_primitives::quick_pick::{
    FIELD_GAP, FIELD_INSET, ICON_GAP, ICON_SIZE, LABEL_SIZE, PANEL_PADDING, PANEL_RADIUS, ROW_GAP,
    ROW_HEIGHT, ROW_RADIUS, ROW_SPACING, ROW_TEXT_INSET, highlighted_label,
};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{FilterFieldStyle, filter_field, menu_reveal};

const PICKER_WIDTH: f32 = 420.;
const MIN_PICKER_WIDTH: f32 = 240.;
const MAX_VISIBLE_ROWS: usize = 10;
const CURSOR_GAP: f32 = 4.;
const WINDOW_MARGIN: f32 = 8.;
const RECENT_ICON_SIZE: f32 = 13.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Placement {
    above: bool,
    position: Point<Pixels>,
    list_max: Pixels,
    width: Pixels,
}

fn placement(cursor: Bounds<Pixels>, viewport: Size<Pixels>, field_height: Pixels) -> Placement {
    let chrome = field_height + px(2. * PANEL_PADDING + FIELD_GAP);
    let full_list = row_stride() * MAX_VISIBLE_ROWS as f32;
    let reserved = px(CURSOR_GAP + WINDOW_MARGIN);
    let room_below = viewport.height - cursor.bottom() - reserved;
    let room_above = cursor.top() - reserved;
    let above = room_below < chrome + full_list && room_above > room_below;
    let room = if above { room_above } else { room_below };
    let list_max = (room - chrome).min(full_list).max(row_stride());
    let position = if above {
        point(cursor.left(), cursor.top() - px(CURSOR_GAP))
    } else {
        point(cursor.left(), cursor.bottom() + px(CURSOR_GAP))
    };
    let width = px(PICKER_WIDTH)
        .min(viewport.width - px(2. * WINDOW_MARGIN))
        .max(px(MIN_PICKER_WIDTH));
    Placement {
        above,
        position,
        list_max,
        width,
    }
}

fn row_stride() -> Pixels {
    px(ROW_HEIGHT + ROW_SPACING)
}

impl PathPicker {
    fn visual_index(&self, row: usize) -> usize {
        if self.above {
            self.rows().len().saturating_sub(row + 1)
        } else {
            row
        }
    }

    fn render_field(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let focus = self.input.read(cx).focus_handle.clone();
        filter_field(
            "path-picker-field",
            "path-picker-field-clear",
            ui,
            FilterFieldStyle::palette(),
            focus.is_focused(window),
            !self.query.is_empty(),
            true,
            None,
            self.input.clone(),
            cx.listener(|picker, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                picker.input.update(cx, |input, cx| input.clear(cx));
                let focus = picker.focus_handle(cx);
                window.focus(&focus, cx);
            }),
        )
        .flex_none()
        .mx(px(FIELD_INSET))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .into_any_element()
    }

    fn render_row(&self, index: usize, row: &Row, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let selected = index == self.selected;
        let label_color = if selected {
            crate::theme::on_selection_color()
        } else {
            ui.text
        };
        let muted_color = if selected {
            crate::theme::on_selection_muted()
        } else {
            ui.muted
        };
        let icon = if row.is_dir {
            svg()
                .flex_none()
                .size(px(ICON_SIZE))
                .path("icons/folder.svg")
                .text_color(with_alpha(label_color, 0.8))
                .into_any_element()
        } else {
            let name = row
                .path
                .file_name()
                .map(|name| name.to_string_lossy())
                .unwrap_or_default();
            img(crate::file_icons::language_icon_path(&name))
                .flex_none()
                .size(px(ICON_SIZE))
                .into_any_element()
        };
        select_item_shaped(
            SharedString::from(format!("path-picker-row-{index}")),
            selected,
            ui,
            crate::theme::selection_color(),
            ROW_RADIUS,
        )
        .h(px(ROW_HEIGHT))
        .px(px(ROW_TEXT_INSET))
        .gap(px(ROW_GAP))
        .text_size(px(LABEL_SIZE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |picker, _: &ClickEvent, _, cx| {
            picker.pick(index, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(ICON_GAP))
                .child(icon)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis_middle()
                        .text_color(label_color)
                        .child(highlighted_label(&row.label, &row.highlights)),
                ),
        )
        .when(row.recent, |element| {
            element.child(
                svg()
                    .relative()
                    .flex_none()
                    .size(px(RECENT_ICON_SIZE))
                    .path("icons/clock.svg")
                    .text_color(muted_color),
            )
        })
        .into_any_element()
    }

    fn render_body(&self, list_max: Pixels, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ui = crate::theme::ui_colors();
        let rows = match &self.listing {
            Listing::Status(status) => {
                return Some(
                    div()
                        .h(px(ROW_HEIGHT))
                        .px(px(ROW_TEXT_INSET))
                        .flex()
                        .items_center()
                        .text_size(px(LABEL_SIZE))
                        .text_color(ui.muted)
                        .child(status.text())
                        .into_any_element(),
                );
            }
            Listing::Rows(rows) if rows.is_empty() => return None,
            Listing::Rows(rows) => Rc::new(rows.clone()),
        };
        let count = rows.len();
        let above = self.above;
        Some(
            uniform_list(
                "path-picker-list",
                count,
                cx.processor(move |picker, visible: Range<usize>, _, cx| {
                    visible
                        .filter_map(|visual| {
                            let index = if above { count - 1 - visual } else { visual };
                            let row = rows.get(index)?;
                            Some(
                                div()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .pb(px(ROW_SPACING))
                                    .child(picker.render_row(index, row, cx)),
                            )
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .w_full()
            .h((row_stride() * count as f32).min(list_max))
            .track_scroll(&self.scroll)
            .into_any_element(),
        )
    }
}

impl Render for PathPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let layout = placement(
            self.anchor,
            window.viewport_size(),
            FilterFieldStyle::palette().height,
        );
        self.above = layout.above;
        if std::mem::take(&mut self.reveal_selected) && !self.rows().is_empty() {
            self.scroll
                .scroll_to_item(self.visual_index(self.selected), ScrollStrategy::Nearest);
        }

        let field = self.render_field(window, cx);
        let body = self.render_body(layout.list_max, cx).map(|body| {
            div()
                .flex_none()
                .when(layout.above, |wrapper| wrapper.mb(px(FIELD_GAP)))
                .when(!layout.above, |wrapper| wrapper.mt(px(FIELD_GAP)))
                .child(body)
        });
        let panel = div()
            .id("path-picker")
            .key_context("PathPicker")
            .relative()
            .child(squircle_fill(PANEL_RADIUS, select_menu_surface(ui)))
            .child(squircle_border(
                PANEL_RADIUS,
                px(1.),
                with_alpha(ui.border, 0.6),
            ))
            .flex()
            .flex_col()
            .p(px(PANEL_PADDING))
            .w(layout.width)
            .occlude()
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| {
                cx.emit(PathPickerEvent::Dismissed { refocus: false });
            }))
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation());
        let panel = if layout.above {
            panel.children(body).child(field)
        } else {
            panel.child(field).children(body)
        };

        deferred(
            anchored()
                .anchor(if layout.above {
                    Anchor::BottomLeft
                } else {
                    Anchor::TopLeft
                })
                .position(layout.position)
                .snap_to_window_with_margin(px(WINDOW_MARGIN))
                .child(menu_reveal("path-picker-reveal", panel)),
        )
        .with_priority(3)
    }
}

#[cfg(test)]
mod tests {
    use gpui::size;

    use super::*;

    const FIELD_HEIGHT: f32 = 34.;

    fn cursor_at(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(40.), px(top)), size(px(9.), px(18.)))
    }

    fn viewport() -> Size<Pixels> {
        size(px(1200.), px(800.))
    }

    fn full_list() -> Pixels {
        row_stride() * MAX_VISIBLE_ROWS as f32
    }

    #[test]
    fn the_picker_opens_below_a_cursor_with_room_under_it() {
        let layout = placement(cursor_at(100.), viewport(), px(FIELD_HEIGHT));
        assert!(!layout.above);
        assert_eq!(layout.position, point(px(40.), px(100. + 18. + CURSOR_GAP)));
        assert_eq!(layout.list_max, full_list());
        assert_eq!(layout.width, px(PICKER_WIDTH));
    }

    #[test]
    fn the_picker_flips_above_a_cursor_near_the_bottom() {
        let layout = placement(cursor_at(760.), viewport(), px(FIELD_HEIGHT));
        assert!(layout.above);
        assert_eq!(layout.position, point(px(40.), px(760. - CURSOR_GAP)));
        assert_eq!(layout.list_max, full_list());
    }

    #[test]
    fn a_short_window_shrinks_the_list_to_the_larger_side() {
        let short = size(px(1200.), px(260.));
        let layout = placement(cursor_at(150.), short, px(FIELD_HEIGHT));
        assert!(layout.above);
        let chrome = px(FIELD_HEIGHT + 2. * PANEL_PADDING + FIELD_GAP);
        assert_eq!(
            layout.list_max,
            px(150. - CURSOR_GAP - WINDOW_MARGIN) - chrome
        );
    }

    #[test]
    fn the_list_always_keeps_room_for_one_row() {
        let tiny = size(px(1200.), px(60.));
        let layout = placement(cursor_at(20.), tiny, px(FIELD_HEIGHT));
        assert_eq!(layout.list_max, row_stride());
    }

    #[test]
    fn a_narrow_window_narrows_the_picker_down_to_a_floor() {
        let narrow = size(px(300.), px(800.));
        assert_eq!(
            placement(cursor_at(100.), narrow, px(FIELD_HEIGHT)).width,
            px(300. - 2. * WINDOW_MARGIN)
        );
        let tiny = size(px(100.), px(800.));
        assert_eq!(
            placement(cursor_at(100.), tiny, px(FIELD_HEIGHT)).width,
            px(MIN_PICKER_WIDTH)
        );
    }
}
