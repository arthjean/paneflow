use gpui::{App, Bounds, Font, Pixels, Point, SharedString, TextAlign, TextRun, Window, fill, px};
#[cfg(debug_assertions)]
use gpui::{BorderStyle, hsla, outline};

use super::super::LayoutState;
use super::super::TerminalElement;
use super::super::geometry::CellGeometry;

pub fn paint_search_highlights(layout: &LayoutState, geom: &CellGeometry, window: &mut Window) {
    for rect in &layout.search_rects {
        let rect_bounds = geom.cell_span_bounds(rect.line, rect.col, rect.num_cols);
        window.paint_quad(fill(rect_bounds, rect.color));
    }
}

pub fn paint_hyperlink_underline(
    element: &TerminalElement,
    layout: &LayoutState,
    geom: &CellGeometry,
    window: &mut Window,
) {
    let Some((link_line, col_start, col_end)) = element.hovered_link_range else {
        return;
    };

    let CellGeometry {
        origin,
        cell_width,
        line_height,
    } = *geom;

    let display_offset = layout.display_offset as i32;
    let screen_line = link_line + display_offset;
    if screen_line < 0 || (screen_line as usize) >= layout.desired_rows {
        return;
    }

    let x_start = origin.x + cell_width * col_start as f32;
    let x_end = origin.x + cell_width * (col_end + 1) as f32;
    let y = origin.y + line_height * (screen_line + 1) as f32 - gpui::px(1.0);
    let underline_bounds = Bounds::new(
        Point { x: x_start, y },
        gpui::Size {
            width: x_end - x_start,
            height: gpui::px(1.0),
        },
    );
    window.paint_quad(fill(underline_bounds, layout.link_text_color));
}

#[allow(clippy::too_many_arguments)]
pub fn paint_ime_preedit<H, F>(
    element: &TerminalElement,
    layout: &LayoutState,
    geom: &CellGeometry,
    font_size: Pixels,
    base_font: &Font,
    window: &mut Window,
    cx: &mut App,
    make_handler: F,
) where
    H: gpui::InputHandler,
    F: FnOnce(Option<Bounds<Pixels>>) -> H,
{
    if !element.focused {
        return;
    }

    let CellGeometry {
        origin,
        cell_width,
        line_height,
    } = *geom;

    let cursor_bounds = layout.ime_cursor_bounds.map(|b| {
        Bounds::new(
            Point {
                x: b.origin.x + origin.x,
                y: b.origin.y + origin.y,
            },
            b.size,
        )
    });
    let handler = make_handler(cursor_bounds);
    window.handle_input(&element.focus_handle, handler, cx);

    if !element.ime_marked_text.is_empty()
        && let Some(cb) = cursor_bounds
    {
        let ime_run = TextRun {
            len: element.ime_marked_text.len(),
            font: base_font.clone(),
            color: layout.background_color,
            background_color: None,
            underline: Some(gpui::UnderlineStyle {
                color: None,
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            SharedString::from(element.ime_marked_text.clone()),
            font_size,
            &[ime_run],
            Some(cell_width),
        );
        let preedit_width = shaped.width();
        let preedit_bg = Bounds::new(
            cb.origin,
            gpui::Size {
                width: preedit_width,
                height: line_height,
            },
        );
        window.paint_quad(fill(preedit_bg, layout.background_color));
        let _ = shaped.paint(cb.origin, line_height, TextAlign::Left, None, window, cx);
    }
}

#[allow(clippy::too_many_arguments)]
pub fn paint_exit_overlay(
    layout: &LayoutState,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
    font_size: Pixels,
    base_font: &Font,
    exit_fg: gpui::Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(code) = layout.exited else {
        return;
    };

    let CellGeometry {
        origin,
        line_height,
        ..
    } = *geom;

    let msg = match &layout.exit_signal {
        Some(sig) => format!("[Process terminated by signal: {sig}]"),
        None => format!("[Process exited with code {code}]"),
    };
    let run = TextRun {
        len: msg.len(),
        font: base_font.clone(),
        color: exit_fg,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped = window
        .text_system()
        .shape_line(SharedString::from(msg), font_size, &[run], None);
    let text_width = shaped.width();
    let x = origin.x + (bounds.size.width - text_width) * 0.5;
    let y = origin.y + (bounds.size.height - line_height) * 0.5;
    let _ = shaped.paint(
        Point { x, y },
        line_height,
        TextAlign::Left,
        None,
        window,
        cx,
    );
}

#[cfg(debug_assertions)]
pub fn paint_pixel_probe_overlay(layout: &LayoutState, geom: &CellGeometry, window: &mut Window) {
    let rows = layout.desired_rows;
    let cols = layout.desired_cols;
    if rows == 0 || cols == 0 {
        return;
    }

    let border_color = hsla(0.0, 1.0, 0.5, 0.3);
    let physical_one_px = 1.0 / window.scale_factor().max(1.0);
    let border_width = px(physical_one_px);

    for row in 0..rows {
        for col in 0..cols {
            let bounds = geom.cell_span_bounds(row as i32, col, 1);
            window.paint_quad(
                outline(bounds, border_color, BorderStyle::Solid).border_widths(border_width),
            );
        }
    }
}
