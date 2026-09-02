use gpui::{Window, fill};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;

pub fn paint_selection(layout: &LayoutState, geom: &CellGeometry, window: &mut Window) {
    for rect in &layout.selection_rects {
        let rect_bounds = geom.cell_span_bounds(rect.line, rect.col, rect.num_cols);
        window.paint_quad(fill(rect_bounds, rect.color));
    }
}
