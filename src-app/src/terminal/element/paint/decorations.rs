//! Underline and strikethrough paint pass, from the cell metrics.
//!
//! Positions and thicknesses come from the font tables through
//! [`super::super::font::CellMetrics`], and every style is its own sprite
//! (single, double, dotted, dashed, curly), following Ghostty's
//! `src/font/sprite/draw/special.zig`. Painted before the glyphs so
//! descenders stay legible over a colored underline.

use gpui::{PathBuilder, Window, fill, px};

use super::super::geometry::CellGeometry;
use super::super::{DecorationKind, LayoutState, UnderlineKind};

pub fn paint_decorations(layout: &LayoutState, geom: &CellGeometry, window: &mut Window) {
    if layout.decorations.is_empty() || layout.desired_cols == 0 || layout.desired_rows == 0 {
        return;
    }
    let m = geom.metrics;
    let cell_w = m.cell_width.max(1);
    let cell_h = m.cell_height.max(1);
    // A decoration may dip below its cell by up to a quarter cell (Ghostty's
    // canvas padding) but never further, so bad tables cannot hide it.
    let padding = cell_h / 4;

    for d in &layout.decorations {
        let col_end = d.col_start + d.num_cols;
        if d.num_cols == 0
            || col_end > layout.desired_cols
            || d.line < 0
            || d.line as usize >= layout.desired_rows
        {
            continue;
        }
        let x0 = geom.x_device(d.col_start);
        let x1 = geom.x_device(col_end);
        let y0 = geom.y_device(d.line);
        let span_w = x1 - x0;

        match d.kind {
            DecorationKind::Strikethrough => {
                let t = m.strikethrough_thickness.max(1);
                let y = m.strikethrough_position.clamp(0, cell_h - t);
                window.paint_quad(fill(geom.device_rect(x0, y0 + y, span_w, t), d.color));
            }
            DecorationKind::Underline(UnderlineKind::None) => {}
            DecorationKind::Underline(UnderlineKind::Single) => {
                let t = m.underline_thickness.max(1);
                let y = m.underline_position.min(cell_h + padding - t);
                window.paint_quad(fill(geom.device_rect(x0, y0 + y, span_w, t), d.color));
            }
            DecorationKind::Underline(UnderlineKind::Double) => {
                // One line above and one below the single position, so the
                // single underline becomes the gap between them.
                let t = m.underline_thickness.max(1);
                let y = m.underline_position.min(cell_h + padding - 2 * t);
                window.paint_quad(fill(
                    geom.device_rect(x0, y0 + (y - t).max(0), span_w, t),
                    d.color,
                ));
                window.paint_quad(fill(geom.device_rect(x0, y0 + y + t, span_w, t), d.color));
            }
            DecorationKind::Underline(UnderlineKind::Dashed) => {
                let t = m.underline_thickness.max(1);
                let y = m.underline_position.min(cell_h + padding - t);
                let dash_w = cell_w / 3 + 1;
                let dash_count = cell_w / dash_w + 1;
                for col in d.col_start..col_end {
                    let cx = geom.x_device(col);
                    let mut i = 0;
                    while i < dash_count {
                        let x = i * dash_w;
                        let w = dash_w.min(cell_w - x);
                        window.paint_quad(fill(geom.device_rect(cx + x, y0 + y, w, t), d.color));
                        i += 2;
                    }
                }
            }
            DecorationKind::Underline(UnderlineKind::Dotted) => {
                // Diameter sqrt(2) times the thickness: plain-thickness dots
                // look anemic. Enough dots that the gaps match the diameter,
                // never so many that a gap is under a radius or a pixel.
                let t = m.underline_thickness.max(1) as f32;
                let w = cell_w as f32;
                let radius = std::f32::consts::FRAC_1_SQRT_2 * t;
                let center_y = (m.underline_position as f32 + 0.5 * t)
                    .min((cell_h + padding) as f32 - radius.ceil());
                let dot_count = (w / (4.0 * radius))
                    .ceil()
                    .min((w / (3.0 * radius)).floor())
                    .min((w / (2.0 * radius + 1.0)).floor())
                    .max(1.0);
                let step = w / dot_count;
                let diameter = px(2.0 * radius / geom.scale_factor);
                let corner = px(radius / geom.scale_factor);
                for col in d.col_start..col_end {
                    let cx = geom.x_device(col) as f32;
                    let mut x = cx + step / 2.0;
                    for _ in 0..dot_count as usize {
                        let origin = gpui::Point {
                            x: px((x - radius) / geom.scale_factor),
                            y: px((y0 as f32 + center_y - radius) / geom.scale_factor),
                        };
                        let bounds = gpui::Bounds::new(
                            origin,
                            gpui::Size {
                                width: diameter,
                                height: diameter,
                            },
                        );
                        window.paint_quad(fill(bounds, d.color).corner_radii(corner));
                        x += step;
                    }
                }
            }
            DecorationKind::Underline(UnderlineKind::Curly) => {
                // One wave per cell peaking at its center, amplitude w/pi,
                // curvature 0.4: adjacent cells continue the same wave.
                let t = m.underline_thickness.max(1) as f32;
                let w = cell_w as f32;
                let amplitude = w / std::f32::consts::PI;
                let top =
                    (m.underline_position as f32).min((cell_h + padding) as f32 - amplitude - t);
                let bottom = top + amplitude;
                let r = 0.4;
                let center = 0.5 * w;
                let mut path = PathBuilder::stroke(px(t / geom.scale_factor));
                let pt = |x: f32, y: f32| gpui::Point {
                    x: px(x / geom.scale_factor),
                    y: px(y / geom.scale_factor),
                };
                for col in d.col_start..col_end {
                    let cx = geom.x_device(col) as f32;
                    let cy = y0 as f32;
                    path.move_to(pt(cx, cy + bottom));
                    path.cubic_bezier_to(
                        pt(cx + center, cy + top),
                        pt(cx + center * r, cy + bottom),
                        pt(cx + center - center * r, cy + top),
                    );
                    path.cubic_bezier_to(
                        pt(cx + w, cy + bottom),
                        pt(cx + center + center * r, cy + top),
                        pt(cx + w - center * r, cy + bottom),
                    );
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, d.color);
                }
            }
        }
    }
}
