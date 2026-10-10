use gpui::{Bounds, Pixels, Point, Window, fill, px};

use super::super::LayoutState;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridEdges {
    pub top: Pixels,
    pub bottom: Pixels,
}

impl GridEdges {
    pub fn inset_within(bounds: Bounds<Pixels>) -> Self {
        let inset_y = px(crate::app::constants::PANE_CONTENT_INSET_Y);
        Self {
            top: bounds.origin.y + inset_y,
            bottom: bounds.origin.y + bounds.size.height - inset_y,
        }
    }
}

pub fn paint_base_fill(layout: &LayoutState, bounds: Bounds<Pixels>, window: &mut Window) {
    if layout.background_color.a > 0.0 {
        window.paint_quad(fill(bounds, layout.background_color));
    }
}

fn rect_vertical_span(
    line_start: usize,
    line_end: usize,
    row_count: usize,
    y_boundaries: &[Pixels],
    edges: Option<GridEdges>,
) -> (Pixels, Pixels) {
    let top = y_boundaries[line_start];
    let bottom = y_boundaries[line_end];
    let Some(edges) = edges else {
        return (top, bottom);
    };
    let top = if line_start == 0 {
        edges.top.max(top)
    } else {
        top
    };
    let bottom = if line_end == row_count {
        edges.bottom.max(bottom)
    } else {
        bottom
    };
    (top, bottom)
}

pub fn paint_cell_backgrounds(
    layout: &LayoutState,
    edges: Option<GridEdges>,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    window: &mut Window,
) {
    let col_count = layout.desired_cols;
    let row_count = layout.desired_rows;

    if col_count == 0 || row_count == 0 {
        return;
    }

    for rect in &layout.rects {
        if rect.color.a <= 0.0 {
            continue;
        }

        let col_end = rect.col + rect.num_cols;
        let line_end_signed = rect.line + rect.num_lines as i32;

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
        let (y, bottom) = rect_vertical_span(line_start, line_end, row_count, y_boundaries, edges);

        let rect_bounds = Bounds::new(
            Point { x, y },
            gpui::Size {
                width: (right - x).max(px(0.0)),
                height: (bottom - y).max(px(0.0)),
            },
        );

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

    for bq in layout.block_quads() {
        let col_end = bq.col + bq.num_cols;
        if bq.num_cols == 0 || col_end > col_count || bq.line < 0 || (bq.line as usize) >= row_count
        {
            continue;
        }
        let line = bq.line as usize;

        let cell_x_left = x_boundaries[bq.col];
        let cell_x_right = x_boundaries[col_end];
        let cell_y_top = y_boundaries[line];
        let cell_y_bottom = y_boundaries[line + 1];
        let cell_w = cell_x_right - cell_x_left;
        let cell_h = cell_y_bottom - cell_y_top;

        let (fx, fy, fw, fh) = bq.coverage;
        let qx = (cell_x_left + cell_w * fx).floor();
        let qy = (cell_y_top + cell_h * fy).floor();
        let q_right = (cell_x_left + cell_w * (fx + fw)).floor();
        let q_bottom = (cell_y_top + cell_h * (fy + fh)).floor();
        let qw = (q_right - qx).max(px(0.0));
        let qh = (q_bottom - qy).max(px(0.0));

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

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: f32 = 20.0;
    const ROWS: usize = 3;
    const EDGES: GridEdges = GridEdges {
        top: px(6.0),
        bottom: px(66.0),
    };

    fn boundaries(origin_y: f32, rows: usize) -> Vec<Pixels> {
        (0..=rows)
            .map(|row| px(origin_y + LINE * row as f32))
            .collect()
    }

    #[test]
    fn an_unshifted_viewport_stretches_its_edge_rows_to_the_inset() {
        let y = boundaries(5.5, ROWS);
        assert_eq!(
            rect_vertical_span(0, 1, ROWS, &y, Some(EDGES)),
            (px(6.0), px(25.5))
        );
        assert_eq!(
            rect_vertical_span(2, 3, ROWS, &y, Some(EDGES)),
            (px(45.5), px(66.0))
        );
        assert_eq!(
            rect_vertical_span(1, 2, ROWS, &y, Some(EDGES)),
            (px(25.5), px(45.5))
        );
    }

    #[test]
    fn a_shifted_first_row_never_paints_over_the_row_above_it() {
        let y = boundaries(6.0 + 7.0, ROWS);
        assert_eq!(
            rect_vertical_span(0, 1, ROWS, &y, Some(EDGES)),
            (px(13.0), px(33.0))
        );
        assert_eq!(
            rect_vertical_span(2, 3, ROWS, &y, Some(EDGES)),
            (px(53.0), px(73.0)),
            "a last row pushed past the inset keeps its full height"
        );
    }

    #[test]
    fn the_overscan_row_paints_only_its_own_line() {
        let shift = 7.0;
        let above = boundaries(6.0 + shift - LINE, 1);
        assert_eq!(
            rect_vertical_span(0, 1, 1, &above, None),
            (px(6.0 + shift - LINE), px(6.0 + shift))
        );
    }
}
