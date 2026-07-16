//! Integrated single-stroke box drawing.
//!
//! Font-rendered box glyphs can expose side-bearing gaps between cells. This
//! pass draws their topology directly to shared grid boundaries, matching the
//! integrated-glyph approach used by Ghostty's sprite font.

use gpui::{Hsla, PathBuilder, Pixels, Point, Window, point, px};

use super::super::{BoxDrawingGlyph, BoxDrawingShape, LayoutState};

pub fn paint_box_drawing_glyphs(
    layout: &LayoutState,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    window: &mut Window,
) {
    if layout.box_drawing_glyphs.is_empty() || layout.desired_cols == 0 || layout.desired_rows == 0
    {
        return;
    }

    let scale_factor = window.scale_factor().max(1.0);
    let desired_device_width = f32::from(layout.dimensions.line_height) * 0.055 * scale_factor;
    let device_width = desired_device_width.round().max(1.0);
    let stroke_width = px(device_width / scale_factor);
    let mut builder = PathBuilder::stroke(stroke_width);
    let mut current_color: Option<Hsla> = None;

    for glyph in &layout.box_drawing_glyphs {
        if glyph.col >= layout.desired_cols
            || glyph.line < 0
            || glyph.line as usize >= layout.desired_rows
        {
            continue;
        }

        if current_color.is_some_and(|color| color != glyph.color) {
            paint_path(builder, current_color, window);
            builder = PathBuilder::stroke(stroke_width);
        }

        current_color = Some(glyph.color);
        append_glyph(
            &mut builder,
            glyph,
            x_boundaries,
            y_boundaries,
            scale_factor,
            device_width,
        );
    }

    paint_path(builder, current_color, window);
}

fn paint_path(builder: PathBuilder, color: Option<Hsla>, window: &mut Window) {
    if let Some(color) = color
        && let Ok(path) = builder.build()
    {
        window.paint_path(path, color);
    }
}

fn append_glyph(
    builder: &mut PathBuilder,
    glyph: &BoxDrawingGlyph,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    scale_factor: f32,
    device_width: f32,
) {
    let line = glyph.line as usize;
    let left = x_boundaries[glyph.col];
    let right = x_boundaries[glyph.col + 1];
    let top = y_boundaries[line];
    let bottom = y_boundaries[line + 1];
    let center_x = snap_stroke_center((left + right) / 2.0, scale_factor, device_width);
    let center_y = snap_stroke_center((top + bottom) / 2.0, scale_factor, device_width);
    let center = point(center_x, center_y);

    if glyph.shape.rounded {
        append_rounded_corner(builder, glyph.shape, left, right, top, bottom, center);
        return;
    }

    if glyph.shape.left || glyph.shape.right {
        builder.move_to(point(
            if glyph.shape.left { left } else { center_x },
            center_y,
        ));
        builder.line_to(point(
            if glyph.shape.right { right } else { center_x },
            center_y,
        ));
    }
    if glyph.shape.up || glyph.shape.down {
        builder.move_to(point(center_x, if glyph.shape.up { top } else { center_y }));
        builder.line_to(point(
            center_x,
            if glyph.shape.down { bottom } else { center_y },
        ));
    }
}

fn append_rounded_corner(
    builder: &mut PathBuilder,
    shape: BoxDrawingShape,
    left: Pixels,
    right: Pixels,
    top: Pixels,
    bottom: Pixels,
    center: Point<Pixels>,
) {
    let horizontal_edge = if shape.left {
        point(left, center.y)
    } else {
        point(right, center.y)
    };
    let vertical_edge = if shape.up {
        point(center.x, top)
    } else {
        point(center.x, bottom)
    };
    builder.move_to(horizontal_edge);
    builder.curve_to(vertical_edge, center);
}

fn snap_stroke_center(value: Pixels, scale_factor: f32, device_width: f32) -> Pixels {
    let device_value = f32::from(value) * scale_factor;
    px(((device_value - device_width / 2.0).round() + device_width / 2.0) / scale_factor)
}
