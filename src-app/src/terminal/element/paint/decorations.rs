use gpui::{App, Window, fill, px};

use super::super::geometry::CellGeometry;
use super::super::{DecorationKind, LayoutState, UnderlineKind};
use super::masks::{MaskKey, MaskShape, Outline, paint_mask};

pub fn paint_decorations(layout: &LayoutState, geom: &CellGeometry, window: &mut Window, cx: &App) {
    if layout.decorations().next().is_none() || layout.desired_cols == 0 || layout.desired_rows == 0
    {
        return;
    }
    let m = geom.metrics;
    let cell_w = m.cell_width.max(1);
    let cell_h = m.cell_height.max(1);
    let padding = cell_h / 4;

    for d in layout.decorations() {
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
                let key = MaskKey {
                    shape: MaskShape::Undercurl,
                    width: cell_w,
                    height: cell_h + padding,
                    thickness: m.underline_thickness.max(1),
                    offset: m.underline_position,
                };
                for col in d.col_start..col_end {
                    paint_mask(window, cx, geom, (geom.x_device(col), y0), key, d.color);
                }
            }
        }
    }
}

fn undercurl_extent(key: MaskKey) -> (f32, f32, f32) {
    let t = key.thickness.max(1) as f32;
    let amplitude = key.width as f32 / std::f32::consts::PI;
    let top = (key.offset as f32).min(key.height as f32 - amplitude - t);
    (t, top, top + amplitude)
}

pub(super) fn undercurl_band(key: MaskKey) -> (i32, i32) {
    let (t, top, bottom) = undercurl_extent(key);
    let first = ((top - t / 2.0).floor() as i32 - 1).max(0);
    let end = ((bottom + t / 2.0).ceil() as i32 + 1).min(key.height);
    (first, (end - first).max(0))
}

pub(super) fn undercurl_outline(key: MaskKey) -> Outline {
    let (t, top, bottom) = undercurl_extent(key);
    let w = key.width as f32;
    let r = 0.4;
    let center = 0.5 * w;
    let mut outline = Outline::stroke(t);
    outline.move_to(0.0, bottom);
    outline.cubic_to(
        (center, top),
        (center * r, bottom),
        (center - center * r, top),
    );
    outline.cubic_to(
        (w, bottom),
        (center + center * r, top),
        (w - center * r, bottom),
    );
    outline
}
