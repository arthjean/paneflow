use gpui::{App, Font, Pixels, Point, ShapedLine, SharedString, TextRun, Window, point, px};

use super::super::face_tables::{self, GlyphInk};
use super::super::font::CellMetrics;
use super::super::geometry::CellGeometry;
use super::super::{LayoutState, SymbolGlyph};

pub fn paint_text_runs(
    layout: &LayoutState,
    geom: &CellGeometry,
    base_font: &Font,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    for run in &layout.batched_runs {
        let origin = geom.cell_origin(run.line, run.col_start);

        #[cfg(debug_assertions)]
        super::super::pixel_probe::record_glyph(run.line, run.col_start, origin.x, origin.y);

        let text_run = TextRun {
            len: run.text.len(),
            font: super::display_font_for_intensity(&run.font, base_font.weight),
            color: run.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(
            run.text.clone(),
            font_size,
            &[text_run],
            Some(geom.cell_width),
        );
        paint_shaped_line(
            &shaped,
            origin,
            run.color,
            geom,
            layout.color_emoji_enabled,
            window,
            cx,
        );
    }
}

pub(super) fn paint_shaped_line(
    shaped: &ShapedLine,
    cell_origin: Point<Pixels>,
    color: gpui::Hsla,
    geom: &CellGeometry,
    color_emoji_enabled: bool,
    window: &mut Window,
    _cx: &mut App,
) {
    let layout = &**shaped;
    let baseline = point(
        cell_origin.x + geom.logical(geom.metrics.face_center_dx()),
        cell_origin.y + geom.metrics.baseline_px(),
    );
    for run in &layout.runs {
        for glyph in &run.glyphs {
            let origin = point(baseline.x + glyph.position.x, baseline.y + glyph.position.y);
            let painted = if glyph.is_emoji && color_emoji_enabled {
                window.paint_emoji(origin, run.font_id, glyph.id, layout.font_size)
            } else {
                window.paint_glyph(origin, run.font_id, glyph.id, layout.font_size, color)
            };
            if let Err(error) = painted {
                log::debug!("terminal glyph paint failed: {error:#}");
            }
        }
    }
}

pub fn paint_symbols(
    layout: &LayoutState,
    geom: &CellGeometry,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    for symbol in &layout.symbols {
        if symbol.line < 0
            || symbol.line as usize >= layout.desired_rows
            || symbol.col >= layout.desired_cols
        {
            continue;
        }
        paint_symbol(symbol, geom, font_size, window, cx);
    }
}

fn shape_symbol(
    text: SharedString,
    font: &Font,
    color: gpui::Hsla,
    font_size: Pixels,
    window: &Window,
) -> ShapedLine {
    let run = TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(text, font_size, &[run], None)
}

fn paint_symbol(
    symbol: &SymbolGlyph,
    geom: &CellGeometry,
    font_size: Pixels,
    window: &mut Window,
    cx: &mut App,
) {
    let text = SharedString::from(symbol.ch.to_string());
    let shaped = shape_symbol(text.clone(), &symbol.font, symbol.color, font_size, window);
    let cell_origin = geom.cell_origin(symbol.line, symbol.col);

    let Some(run) = shaped.runs.first() else {
        return;
    };
    let family = window
        .text_system()
        .get_font_for_id(run.font_id)
        .map(|font| font.family);
    let ink = family
        .as_deref()
        .and_then(|family| face_tables::embedded_glyph_ink(family, symbol.ch));

    let Some(ink) = ink else {
        paint_shaped_line(&shaped, cell_origin, symbol.color, geom, true, window, cx);
        return;
    };

    let m = geom.metrics;
    let units_to_device = font_size.as_f32() * geom.scale_factor / ink.units_per_em.max(1.0);
    let placement = constrain_icon(&m, symbol.span, &ink, units_to_device);
    let scaled_size = px(font_size.as_f32() * placement.factor);
    let scaled = if (placement.factor - 1.0).abs() < 1e-3 {
        shaped
    } else {
        shape_symbol(text, &symbol.font, symbol.color, scaled_size, window)
    };
    let Some((run, glyph)) = scaled
        .runs
        .first()
        .and_then(|run| run.glyphs.first().map(|glyph| (run, glyph)))
    else {
        return;
    };

    let cell_x = geom.x_device(symbol.col) as f32;
    let cell_bottom = geom.y_device(symbol.line + 1) as f32;
    let origin = point(
        px((cell_x + placement.origin_x) / geom.scale_factor),
        px((cell_bottom - placement.baseline_from_bottom) / geom.scale_factor),
    );
    let painted = if glyph.is_emoji {
        window.paint_emoji(origin, run.font_id, glyph.id, scaled_size)
    } else {
        window.paint_glyph(origin, run.font_id, glyph.id, scaled_size, symbol.color)
    };
    if let Err(error) = painted {
        log::debug!("terminal icon paint failed: {error:#}");
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct IconPlacement {
    pub factor: f32,
    pub origin_x: f32,
    pub baseline_from_bottom: f32,
}

pub(super) fn constrain_icon(
    m: &CellMetrics,
    span: usize,
    ink: &GlyphInk,
    units_to_device: f32,
) -> IconPlacement {
    let glyph_w = (ink.width() * units_to_device).max(1e-3);
    let glyph_h = (ink.height() * units_to_device).max(1e-3);
    let span = span.max(1);

    let factors = |cells: usize| {
        let target_w = m.face_width + (cells - 1) as f32 * m.cell_width as f32;
        let target_h = if cells > 1 {
            m.icon_height
        } else {
            m.icon_height_single
        };
        (target_w / glyph_w).min(target_h / glyph_h)
    };
    let mut factor = factors(span);
    if span > 1 && factor > 1.0 {
        factor = factors(1).max(1.0);
    }

    let group_w = glyph_w * factor;
    let group_h = glyph_h * factor;
    let x = ((m.face_width - group_w) / 2.0).max(0.0);
    let y = (m.face_y + (m.face_y + m.face_height - group_h)) / 2.0;

    IconPlacement {
        factor,
        origin_x: x - ink.x_min * units_to_device * factor,
        baseline_from_bottom: y - ink.y_min * units_to_device * factor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::font::{FaceMetrics, cell_metrics_from_face};

    fn metrics() -> CellMetrics {
        cell_metrics_from_face(
            FaceMetrics {
                ascent: 16.32,
                descent: 4.8,
                line_gap: 0.0,
                advance: 9.6,
                underline_position: -2.48,
                underline_thickness: 0.8,
                strikethrough_position: 5.12,
                strikethrough_thickness: 0.8,
                x_height: 8.8,
                cap_height: 11.68,
            },
            1.0,
            1.0,
            1.0,
        )
    }

    fn wide_icon() -> GlyphInk {
        GlyphInk {
            units_per_em: 1000.0,
            x_min: 0.0,
            y_min: -76.0,
            x_max: 894.0,
            y_max: 796.0,
        }
    }

    #[test]
    fn wide_icon_shrinks_to_one_cell_when_followed_by_text() {
        let p = constrain_icon(&metrics(), 1, &wide_icon(), 16.0 / 1000.0);
        let width = 894.0 * 0.016 * p.factor;
        assert!(
            (width - 9.6).abs() < 1e-3,
            "icon should fill the face width, got {width}"
        );
        assert!(p.factor < 1.0);
        assert!(p.origin_x.abs() < 1e-3);
    }

    #[test]
    fn wide_icon_keeps_its_designed_size_over_two_cells() {
        let p = constrain_icon(&metrics(), 2, &wide_icon(), 16.0 / 1000.0);
        assert!((p.factor - 1.0).abs() < 1e-6, "got factor {}", p.factor);
        assert!(p.origin_x.abs() < 1e-3);
    }

    #[test]
    fn small_icon_grows_to_cover_its_cell_and_stays_centered() {
        let small = GlyphInk {
            units_per_em: 1000.0,
            x_min: 127.0,
            y_min: 48.0,
            x_max: 687.0,
            y_max: 798.0,
        };
        let one = constrain_icon(&metrics(), 1, &small, 16.0 / 1000.0);
        assert!(one.factor > 1.0);
        let two = constrain_icon(&metrics(), 2, &small, 16.0 / 1000.0);
        assert!(
            (one.factor - two.factor).abs() < 1e-6,
            "no growth when a space follows"
        );
        let width = 560.0 * 0.016 * one.factor;
        let ink_left = one.origin_x + 127.0 * 0.016 * one.factor;
        assert!(
            (ink_left - (9.6 - width) / 2.0).abs() < 1e-3,
            "centered in the first cell"
        );
    }

    #[test]
    fn icon_is_centered_vertically_in_the_face() {
        let m = metrics();
        let p = constrain_icon(&m, 1, &wide_icon(), 16.0 / 1000.0);
        let height = 872.0 * 0.016 * p.factor;
        let ink_bottom = p.baseline_from_bottom + (-76.0) * 0.016 * p.factor;
        let expected = m.face_y + (m.face_height - height) / 2.0;
        assert!((ink_bottom - expected).abs() < 1e-3);
    }
}
