use gpui::{
    App, BorderStyle, Bounds, Font, FontStyle, FontWeight, Pixels, Point, SharedString, TextAlign,
    TextRun, Window, fill, outline, px,
};

use super::super::geometry::CellGeometry;
use super::super::{CursorInfo, LayoutState};
use crate::terminal::types::CursorShape;

fn cursor_text_color(cursor: &CursorInfo, layout: &LayoutState) -> gpui::Hsla {
    if cursor.cell_bg.a > 0.01 {
        cursor.cell_bg
    } else if layout.background_color.a > 0.01 {
        layout.background_color
    } else {
        gpui::hsla(0.0, 0.0, 0.08, 1.0)
    }
}

fn paint_cursor_info(
    cursor: &CursorInfo,
    layout: &LayoutState,
    geom: &CellGeometry,
    base_font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let cols = if cursor.wide { 2 } else { 1 };
    let cell_bounds = geom.cell_span_bounds(cursor.line, cursor.col, cols);
    let cx_ = cell_bounds.origin.x;
    let cy = cell_bounds.origin.y;
    let mut cw = cell_bounds.size.width;
    let ch = cell_bounds.size.height;
    let color = cursor.color;

    match cursor.shape {
        CursorShape::Vintage => {
            let vintage_height = (ch * 0.28).max(px(3.0));
            let cursor_bounds = Bounds::new(
                Point {
                    x: cx_,
                    y: cy + ch - vintage_height,
                },
                gpui::Size {
                    width: cw,
                    height: vintage_height,
                },
            );
            window.paint_quad(fill(cursor_bounds, color));
        }
        CursorShape::Block => {
            let shaped = cursor.text.map(|ch| {
                let mut cursor_font = base_font.clone();
                if cursor.bold {
                    cursor_font.weight = FontWeight::BOLD;
                }
                if cursor.italic {
                    cursor_font.style = FontStyle::Italic;
                }
                let cursor_font = super::display_font_for_intensity(&cursor_font, base_font.weight);
                let text = ch.to_string();
                let len = text.len();
                window.text_system().shape_line(
                    SharedString::from(text),
                    font_size,
                    &[TextRun {
                        len,
                        font: cursor_font,
                        color: cursor_text_color(cursor, layout),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    Some(geom.cell_width),
                )
            });

            if cursor.wide
                && let Some(ref shaped) = shaped
            {
                cw = cw.max(shaped.width());
            }

            let cursor_bounds = Bounds::new(
                Point { x: cx_, y: cy },
                gpui::Size {
                    width: cw,
                    height: ch,
                },
            );
            window.paint_quad(fill(cursor_bounds, color));

            if let Some(shaped) = shaped {
                let _ = shaped.paint(
                    Point { x: cx_, y: cy },
                    geom.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
        CursorShape::Beam => {
            let beam_width = px(2.0);
            let cursor_bounds = Bounds::new(
                Point { x: cx_, y: cy },
                gpui::Size {
                    width: beam_width,
                    height: ch,
                },
            );
            window.paint_quad(fill(cursor_bounds, color));
        }
        CursorShape::Underline => {
            let underline_height = px(2.0);
            let cursor_bounds = Bounds::new(
                Point {
                    x: cx_,
                    y: cy + ch - underline_height,
                },
                gpui::Size {
                    width: cw,
                    height: underline_height,
                },
            );
            window.paint_quad(fill(cursor_bounds, color));
        }
        CursorShape::DoubleUnderline => {
            let underline_height = px(2.0);
            let gap = px(2.0);
            let lower_y = cy + ch - underline_height;
            let upper_y = (lower_y - underline_height - gap).max(cy);
            for y in [upper_y, lower_y] {
                let cursor_bounds = Bounds::new(
                    Point { x: cx_, y },
                    gpui::Size {
                        width: cw,
                        height: underline_height,
                    },
                );
                window.paint_quad(fill(cursor_bounds, color));
            }
        }
        CursorShape::HollowBlock => {
            let cursor_bounds = Bounds::new(
                Point { x: cx_, y: cy },
                gpui::Size {
                    width: cw,
                    height: ch,
                },
            );
            window.paint_quad(
                outline(cursor_bounds, color, BorderStyle::Solid)
                    .border_widths(1.5)
                    .corner_radii(px(2.0)),
            );
        }
        CursorShape::Hidden => {}
    }
}

pub fn paint_cursor(
    layout: &LayoutState,
    geom: &CellGeometry,
    base_font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(cursor) = &layout.cursor else {
        return;
    };

    paint_cursor_info(cursor, layout, geom, base_font, font_size, window, cx);
}

pub fn paint_anchor_cursor(
    layout: &LayoutState,
    geom: &CellGeometry,
    base_font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(anchor) = &layout.anchor_cursor else {
        return;
    };

    paint_cursor_info(anchor, layout, geom, base_font, font_size, window, cx);
}
