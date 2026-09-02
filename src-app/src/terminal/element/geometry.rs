use gpui::{Bounds, Pixels, Point, px};

#[derive(Clone, Copy)]
pub(super) struct CellGeometry {
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
}

impl CellGeometry {
    pub(super) fn x_boundary(&self, col: usize) -> Pixels {
        cell_x_boundary(self.origin.x, self.cell_width, col)
    }

    pub(super) fn y_boundary(&self, line: i32) -> Pixels {
        cell_y_boundary(self.origin.y, self.line_height, line)
    }

    pub(super) fn cell_origin(&self, line: i32, col: usize) -> Point<Pixels> {
        Point {
            x: self.x_boundary(col),
            y: self.y_boundary(line),
        }
    }

    pub(super) fn cell_span_bounds(
        &self,
        line: i32,
        col: usize,
        num_cols: usize,
    ) -> Bounds<Pixels> {
        let x = self.x_boundary(col);
        let right = self.x_boundary(col.saturating_add(num_cols));
        let y = self.y_boundary(line);
        let bottom = self.y_boundary(line.saturating_add(1));
        Bounds::new(
            Point { x, y },
            gpui::Size {
                width: (right - x).max(px(0.0)),
                height: (bottom - y).max(px(0.0)),
            },
        )
    }

    pub(super) fn x_boundaries(&self, col_count: usize) -> Vec<Pixels> {
        cell_x_boundaries(self.origin.x, self.cell_width, col_count)
    }

    pub(super) fn y_boundaries(&self, row_count: usize) -> Vec<Pixels> {
        cell_y_boundaries(self.origin.y, self.line_height, row_count)
    }
}

fn cell_x_boundary(origin_x: Pixels, cell_width: Pixels, col: usize) -> Pixels {
    (origin_x + cell_width * col as f32).floor()
}

fn cell_y_boundary(origin_y: Pixels, line_height: Pixels, line: i32) -> Pixels {
    (origin_y + line_height * line as f32).floor()
}

pub(super) fn cell_x_boundaries(
    origin_x: Pixels,
    cell_width: Pixels,
    col_count: usize,
) -> Vec<Pixels> {
    (0..=col_count)
        .map(|col| cell_x_boundary(origin_x, cell_width, col))
        .collect()
}

pub(super) fn cell_y_boundaries(
    origin_y: Pixels,
    line_height: Pixels,
    row_count: usize,
) -> Vec<Pixels> {
    (0..=row_count)
        .map(|row| cell_y_boundary(origin_y, line_height, row as i32))
        .collect()
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;
    use crate::terminal::element::pixel_probe::assert_pixel_aligned;

    #[test]
    fn cell_x_boundaries_count_matches_col_count_plus_one() {
        let b = cell_x_boundaries(px(0.0), px(9.0), 5);
        assert_eq!(b.len(), 6);
    }

    #[test]
    fn ten_cell_run_at_8_4_yields_84_px_total() {
        let b = cell_x_boundaries(px(0.0), px(8.4), 10);
        let total_width = b[10] - b[0];
        assert_eq!(total_width, px(84.0));
        for boundary in &b {
            assert_pixel_aligned(boundary.as_f32(), "x boundary");
        }
    }

    #[test]
    fn adjacent_cells_have_non_negative_width() {
        let b = cell_x_boundaries(px(0.0), px(8.4), 7);
        for window in b.windows(2) {
            let cell_width = window[1] - window[0];
            assert!(
                cell_width >= px(0.0),
                "boundaries must be monotonic; got cell width {cell_width:?}"
            );
        }
    }

    #[test]
    fn boundaries_are_integer_with_fractional_origin() {
        let b = cell_x_boundaries(px(0.4), px(8.4), 10);
        for boundary in &b {
            assert_pixel_aligned(boundary.as_f32(), "x boundary with fractional origin");
        }
    }

    #[test]
    fn cell_y_boundaries_18_2_yields_expected_values() {
        let b = cell_y_boundaries(px(0.0), px(18.2), 5);
        assert_eq!(b[0], px(0.0));
        assert_eq!(b[1], px(18.0));
        assert_eq!(b[2], px(36.0));
        assert_eq!(b[3], px(54.0));
        assert_eq!(b[4], px(72.0));
        assert_eq!(b[5], px(91.0));
    }

    #[test]
    fn single_cell_yields_two_element_array() {
        let b = cell_x_boundaries(px(0.0), px(8.4), 1);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0], px(0.0));
        assert_eq!(b[1], px(8.0));
    }

    #[test]
    fn integer_cell_width_is_no_op_post_us_002() {
        let b = cell_x_boundaries(px(0.0), px(9.0), 5);
        assert_eq!(
            b,
            vec![px(0.0), px(9.0), px(18.0), px(27.0), px(36.0), px(45.0)]
        );
    }

    #[test]
    fn cell_span_bounds_uses_shared_boundaries() {
        let geom = CellGeometry {
            origin: Point {
                x: px(0.4),
                y: px(0.2),
            },
            cell_width: px(8.4),
            line_height: px(18.2),
        };
        let bounds = geom.cell_span_bounds(1, 1, 2);
        assert_eq!(bounds.origin.x, px(8.0));
        assert_eq!(bounds.origin.y, px(18.0));
        assert_eq!(bounds.size.width, px(17.0));
        assert_eq!(bounds.size.height, px(18.0));
    }
}
