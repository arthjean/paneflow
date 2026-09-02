use gpui::{
    App, BorderStyle, Font, FontStyle, FontWeight, Pixels, SharedString, TextRun, Window, fill,
    outline,
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
    let m = geom.metrics;
    let x0 = geom.x_device(cursor.col);
    let y0 = geom.y_device(cursor.line);
    let w = geom.x_device(cursor.col + cols) - x0;
    let h = geom.y_device(cursor.line + 1) - y0;
    if w <= 0 || h <= 0 {
        return;
    }
    let t = m.cursor_thickness.max(1);
    let color = cursor.color;

    match cursor.shape {
        CursorShape::Vintage => {
            let height = ((h as f32 * 0.28).round() as i32).max(t * 2).min(h);
            window.paint_quad(fill(
                geom.device_rect(x0, y0 + h - height, w, height),
                color,
            ));
        }
        CursorShape::Block => {
            window.paint_quad(fill(geom.device_rect(x0, y0, w, h), color));
            if let Some(ch) = cursor.text {
                let mut cursor_font = base_font.clone();
                if cursor.bold {
                    cursor_font.weight = FontWeight::BOLD;
                }
                if cursor.italic {
                    cursor_font.style = FontStyle::Italic;
                }
                let cursor_font = super::display_font_for_intensity(&cursor_font, base_font.weight);
                let text = SharedString::from(ch.to_string());
                let text_color = cursor_text_color(cursor, layout);
                let shaped = window.text_system().shape_line(
                    text.clone(),
                    font_size,
                    &[TextRun {
                        len: text.len(),
                        font: cursor_font,
                        color: text_color,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    Some(geom.cell_width),
                );
                super::text::paint_shaped_line(
                    &shaped,
                    geom.cell_origin(cursor.line, cursor.col),
                    text_color,
                    geom,
                    layout.color_emoji_enabled,
                    window,
                    cx,
                );
            }
        }
        CursorShape::Beam => {
            let x = x0 - (t + 1) / 2;
            window.paint_quad(fill(geom.device_rect(x, y0, t, h), color));
        }
        CursorShape::Underline => {
            let y = m.underline_position.min(h + h / 4 - t);
            window.paint_quad(fill(geom.device_rect(x0, y0 + y, w, t), color));
        }
        CursorShape::DoubleUnderline => {
            let y = m.underline_position.min(h + h / 4 - 2 * t);
            window.paint_quad(fill(geom.device_rect(x0, y0 + (y - t).max(0), w, t), color));
            window.paint_quad(fill(geom.device_rect(x0, y0 + y + t, w, t), color));
        }
        CursorShape::HollowBlock => {
            window.paint_quad(
                outline(geom.device_rect(x0, y0, w, h), color, BorderStyle::Solid)
                    .border_widths(geom.logical(t)),
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
