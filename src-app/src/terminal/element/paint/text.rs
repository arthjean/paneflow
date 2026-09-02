//! Glyph paint pass: one `shape_line` per `BatchedTextRun`, painted glyph
//! by glyph on the integer baseline of the cell metrics.

use gpui::{App, Font, Pixels, Point, ShapedLine, TextRun, Window, point};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;

/// Paint all batched text runs produced during `build_layout`.
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

        // PANEFLOW_PIXEL_PROBE: log glyph X/Y per run (sampled to first 16
        // columns of each row inside the probe). Cell edges are device
        // pixels by construction; a fractional value here would mean the
        // geometry was bypassed.
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

/// Paint a shaped line with its baseline on the cell metrics' pixel row,
/// instead of GPUI's centering of `ascent + descent` inside the line height,
/// which lands the baseline between pixels and lets descenders cross into
/// the next row.
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
