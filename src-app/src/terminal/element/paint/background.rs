//! Background paint pass - terminal fill, per-cell background rects with edge
//! extension, and pixel-perfect block quads.
//!
//! ## Pixel alignment (US-004)
//!
//! Cell rects and block quads share a single pair of per-frame integer
//! pixel boundary arrays - `cell_x_boundaries` and `cell_y_boundaries`.
//! Looking up edges through those arrays makes adjacency exact by
//! construction: `next_rect.x == prev_rect.x + prev_rect.width` for any
//! two horizontally-adjacent rects, regardless of whether the underlying
//! `cell_width` is fractional or which way per-rect rounding would lean.
//! This replaces the previous per-rect `floor(x) + ceil(width)` pattern,
//! which could leave a 1-px gap or overlap at the seam when `cell_width`
//! was fractional. Block quads use the same arrays so a full-block (`█`,
//! `▀`, `▄`) coverage shares its outer edges with the cell background
//! underneath - the canonical anti-gap fix from `debug_block_char_rendering.md`.

use gpui::{Bounds, Pixels, Point, Window, fill, px};

use super::super::LayoutState;

/// Paint the terminal background fill.
pub fn paint_base_fill(layout: &LayoutState, bounds: Bounds<Pixels>, window: &mut Window) {
    if layout.background_color.a > 0.0 {
        window.paint_quad(fill(bounds, layout.background_color));
    }
}

/// Paint per-cell background rects with edge extension (Ghostty-style
/// EXTEND_LEFT/RIGHT/UP/DOWN for neverExtendBg).
pub fn paint_cell_backgrounds(
    layout: &LayoutState,
    bounds: Bounds<Pixels>,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    window: &mut Window,
) {
    // Vertical extension targets the GRID band, not the raw element bounds:
    // the pane inset (Ghostty's `window-padding-y`) is deliberate empty space,
    // so a full-width cell background must stop at the first/last row's edge
    // instead of bleeding into it.
    let inset_y = px(crate::app::constants::PANE_CONTENT_INSET_Y);
    let widget_top = bounds.origin.y + inset_y;
    let widget_bottom = bounds.origin.y + bounds.size.height - inset_y;

    let col_count = layout.desired_cols;
    let row_count = layout.desired_rows;

    // Empty viewport (window minimised, mid-resize) - nothing to paint. The
    // caller passes empty boundary slices in that case (US-047), so guard
    // before indexing them.
    if col_count == 0 || row_count == 0 {
        return;
    }

    let last_row = row_count.saturating_sub(1) as i32;

    for rect in &layout.rects {
        if rect.color.a <= 0.0 {
            continue;
        }

        let col_end = rect.col + rect.num_cols;
        let line_end_signed = rect.line + rect.num_lines as i32;

        // Defensive bounds check. `build_layout` should never emit a rect
        // outside the viewport or with zero extent - silent skip beats
        // indexing past the boundary arrays or queueing a zero-area quad
        // for the GPU. If this trips in practice, the layout pass has a
        // bug worth surfacing via the probe.
        if rect.num_cols == 0
            || rect.num_lines == 0
            || col_end > col_count
            || rect.line < 0
            || line_end_signed < 0
            || (line_end_signed as usize) > row_count
        {
            continue;
        }

        let line_start = rect.line as usize;
        let line_end = line_end_signed as usize;

        let x = x_boundaries[rect.col];
        let right = x_boundaries[col_end];
        let mut y = y_boundaries[line_start];
        let mut bottom = y_boundaries[line_end];
        let last_rect_line = rect.line + rect.num_lines as i32 - 1;

        // Horizontal extension into the gutter is intentionally NOT applied:
        // matching Zed (`crates/terminal_view/src/terminal_element.rs`
        // BackgroundRect::paint), bg rects stay strictly inside the cell
        // grid. Extending col-0 / last-col rects to the widget edges
        // caused the OpenAI Codex input-bar tint to leak into the gutter,
        // producing an unbounded gray bar instead of the inset Zed-style
        // band. The widget-wide `paint_base_fill` covers the gutter with
        // the theme background; cell rects sit on top of it inside the grid.
        //
        // Vertical extension remains unconditional - half-pixel residue at
        // the top and bottom of the grid would otherwise show a thin band
        // of widget-bg between the first/last cell row and the widget edge.
        if rect.line == 0 {
            y = widget_top;
        }
        if last_rect_line == last_row {
            bottom = widget_bottom;
        }

        let rect_bounds = Bounds::new(
            Point { x, y },
            gpui::Size {
                width: (right - x).max(px(0.0)),
                height: (bottom - y).max(px(0.0)),
            },
        );

        // PANEFLOW_PIXEL_PROBE: capture the post-extension rect coordinates
        // so a future investigation can verify shared-boundary adjacency
        // (`prev.x + prev.width == next.x`) directly from the log.
        #[cfg(debug_assertions)]
        super::super::pixel_probe::record_background(
            rect.col,
            rect.line,
            rect_bounds.origin.x,
            rect_bounds.origin.y,
            rect_bounds.size.width,
            rect_bounds.size.height,
        );

        window.paint_quad(fill(rect_bounds, rect.color));
    }
}

/// Paint block-element quads (half-blocks, 1/8-blocks, etc.) as filled
/// rects to avoid font-glyph sub-pixel gaps.
///
/// Uses the same shared-boundary arrays as [`paint_cell_backgrounds`] so
/// a full-block coverage (`█`, `▀`, `▄` with `fx=fy=0, fw=fh=1`) lines
/// up exactly with the cell background underneath - no sub-pixel seam.
/// Partial-block coverage applies floor on both inner edges to preserve
/// the same shared-boundary property between adjacent block cells.
pub fn paint_block_quads(
    layout: &LayoutState,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    window: &mut Window,
) {
    let col_count = layout.desired_cols;
    let row_count = layout.desired_rows;
    if col_count == 0 || row_count == 0 {
        return;
    }

    for bq in &layout.block_quads {
        let col_end = bq.col + bq.num_cols;
        // Defensive: skip out-of-range or zero-extent quads. Symmetric
        // with the guard in `paint_cell_backgrounds` above.
        if bq.num_cols == 0 || col_end > col_count || bq.line < 0 || (bq.line as usize) >= row_count
        {
            continue;
        }
        let line = bq.line as usize;

        // Outer cell extents from the shared-boundary arrays - these are
        // identical to the cell background's edges, by construction.
        let cell_x_left = x_boundaries[bq.col];
        let cell_x_right = x_boundaries[col_end];
        let cell_y_top = y_boundaries[line];
        let cell_y_bottom = y_boundaries[line + 1];
        let cell_w = cell_x_right - cell_x_left;
        let cell_h = cell_y_bottom - cell_y_top;

        // Apply fractional coverage WITHIN the cell extents. Floor on
        // both inner edges (rather than `floor(start) + ceil(width)`) so
        // adjacent partial-block cells share their inner boundary the
        // same way full cells share their outer boundary. For full-cell
        // coverage (fx=fy=0, fw=fh=1) this collapses to the cell extents.
        let (fx, fy, fw, fh) = bq.coverage;
        let qx = (cell_x_left + cell_w * fx).floor();
        let qy = (cell_y_top + cell_h * fy).floor();
        let q_right = (cell_x_left + cell_w * (fx + fw)).floor();
        let q_bottom = (cell_y_top + cell_h * (fy + fh)).floor();
        let qw = (q_right - qx).max(px(0.0));
        let qh = (q_bottom - qy).max(px(0.0));

        // PANEFLOW_PIXEL_PROBE: block quads are the canonical "fix this
        // gap" surface from `debug_block_char_rendering.md` - log the
        // exact submitted geometry so a future investigation can compare
        // against the corresponding cell background and glyph X.
        #[cfg(debug_assertions)]
        super::super::pixel_probe::record_block_quad(bq.col, bq.line, qx, qy, qw, qh);

        window.paint_quad(fill(
            Bounds::new(
                Point { x: qx, y: qy },
                gpui::Size {
                    width: qw,
                    height: qh,
                },
            ),
            bq.color,
        ));
    }
}
