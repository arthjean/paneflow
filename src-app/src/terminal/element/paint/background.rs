use gpui::{Bounds, Pixels, Point, Window, fill, px};

use super::super::LayoutState;

pub fn paint_base_fill(layout: &LayoutState, bounds: Bounds<Pixels>, window: &mut Window) {
    if layout.background_color.a > 0.0 {
        window.paint_quad(fill(bounds, layout.background_color));
    }
}

pub fn paint_cell_backgrounds(
    layout: &LayoutState,
    bounds: Bounds<Pixels>,
    x_boundaries: &[Pixels],
    y_boundaries: &[Pixels],
    window: &mut Window,
) {
    let inset_y = px(crate::app::constants::PANE_CONTENT_INSET_Y);
    let widget_top = bounds.origin.y + inset_y;
    let widget_bottom = bounds.origin.y + bounds.size.height - inset_y;

    let col_count = layout.desired_cols;
    let row_count = layout.desired_rows;

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

    for bq in &layout.block_quads {
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
