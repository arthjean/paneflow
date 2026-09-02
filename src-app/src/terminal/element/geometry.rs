//! Cell ↔ pixel conversion state.
//!
//! `CellGeometry` bundles what every paint pass needs to map grid coordinates
//! to window pixels: the snapped origin, the cell strides, the display scale,
//! and the integer device-pixel [`CellMetrics`] the strides were rounded
//! from. Passing it around avoids copy-paste of five arguments per call site.
//!
//! Every boundary is computed in device pixels and converted back, so a cell
//! is a whole number of device pixels at any scale factor (Ghostty keeps its
//! grid in device pixels for the same reason). GPUI snaps quads to device
//! pixels with a round, which recovers the exact integer from the division.

use gpui::{Bounds, Pixels, Point, px};

use super::font::CellMetrics;

/// Pixel-space geometry for a single terminal grid, shared across paint passes.
#[derive(Clone, Copy)]
pub(super) struct CellGeometry {
    /// Top-left corner of the usable grid in window coordinates (includes the
    /// fixed content insets applied in `paint()`), already snapped to a
    /// device pixel.
    pub origin: Point<Pixels>,
    pub cell_width: Pixels,
    pub line_height: Pixels,
    pub scale_factor: f32,
    pub metrics: CellMetrics,
}

impl CellGeometry {
    pub(super) fn new(origin: Point<Pixels>, metrics: CellMetrics) -> Self {
        Self {
            origin,
            cell_width: metrics.cell_width_px(),
            line_height: metrics.cell_height_px(),
            scale_factor: metrics.scale_factor,
            metrics,
        }
    }

    /// Device-pixel X of the left edge of `col`.
    pub(super) fn x_device(&self, col: usize) -> i32 {
        device_boundary(
            self.origin.x,
            self.scale_factor,
            self.metrics.cell_width,
            col,
        )
    }

    /// Device-pixel Y of the top edge of `line`.
    pub(super) fn y_device(&self, line: i32) -> i32 {
        device_boundary(
            self.origin.y,
            self.scale_factor,
            self.metrics.cell_height,
            line.max(0) as usize,
        )
    }

    /// Logical pixels for a device-pixel coordinate.
    pub(super) fn logical(&self, device: i32) -> Pixels {
        px(device as f32 / self.scale_factor)
    }

    /// Logical bounds of a device-pixel rectangle.
    pub(super) fn device_rect(&self, x: i32, y: i32, width: i32, height: i32) -> Bounds<Pixels> {
        Bounds::new(
            Point {
                x: self.logical(x),
                y: self.logical(y),
            },
            gpui::Size {
                width: self.logical(width.max(0)),
                height: self.logical(height.max(0)),
            },
        )
    }

    /// Pixel X boundary for a column edge.
    pub(super) fn x_boundary(&self, col: usize) -> Pixels {
        self.logical(self.x_device(col))
    }

    /// Pixel Y boundary for a row edge.
    pub(super) fn y_boundary(&self, line: i32) -> Pixels {
        self.logical(self.y_device(line))
    }

    /// Top-left pixel for a cell. Used by text and cursor paint passes.
    pub(super) fn cell_origin(&self, line: i32, col: usize) -> Point<Pixels> {
        Point {
            x: self.x_boundary(col),
            y: self.y_boundary(line),
        }
    }

    /// Bounds for a single-row span of terminal cells.
    pub(super) fn cell_span_bounds(
        &self,
        line: i32,
        col: usize,
        num_cols: usize,
    ) -> Bounds<Pixels> {
        let x = self.x_device(col);
        let right = self.x_device(col.saturating_add(num_cols));
        let y = self.y_device(line);
        let bottom = self.y_device(line.saturating_add(1));
        self.device_rect(x, y, right - x, bottom - y)
    }

    pub(super) fn x_boundaries(&self, col_count: usize) -> Vec<Pixels> {
        (0..=col_count).map(|col| self.x_boundary(col)).collect()
    }

    pub(super) fn y_boundaries(&self, row_count: usize) -> Vec<Pixels> {
        (0..=row_count)
            .map(|row| self.y_boundary(row as i32))
            .collect()
    }
}

/// Device-pixel edge `index` of a grid whose stride is `stride` device pixels,
/// starting at `origin` logical pixels. The origin floors to a device pixel;
/// the strides are integers, so every edge is an integer with no accumulated
/// float error.
fn device_boundary(origin: Pixels, scale_factor: f32, stride: i32, index: usize) -> i32 {
    let origin_device = (origin.as_f32() * scale_factor).floor() as i32;
    origin_device + stride.max(1) * index as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::element::font::{FaceMetrics, cell_metrics_from_face};

    fn metrics(advance: f32, height: f32, scale_factor: f32) -> CellMetrics {
        cell_metrics_from_face(
            FaceMetrics {
                ascent: height * 0.8,
                descent: height * 0.2,
                line_gap: 0.0,
                advance,
                underline_position: 0.0,
                underline_thickness: 0.0,
                strikethrough_position: 0.0,
                strikethrough_thickness: 0.0,
                x_height: 0.0,
                cap_height: 0.0,
            },
            scale_factor,
            1.0,
            1.0,
        )
    }

    fn geometry(origin: Point<Pixels>, advance: f32, height: f32, scale: f32) -> CellGeometry {
        CellGeometry::new(origin, metrics(advance, height, scale))
    }

    #[test]
    fn boundaries_count_matches_cell_count_plus_one() {
        let geom = geometry(Point::default(), 9.0, 18.0, 1.0);
        assert_eq!(geom.x_boundaries(5).len(), 6);
        assert_eq!(geom.y_boundaries(3).len(), 4);
    }

    #[test]
    fn integer_strides_at_scale_one() {
        let geom = geometry(Point::default(), 9.0, 18.0, 1.0);
        assert_eq!(
            geom.x_boundaries(5),
            vec![px(0.0), px(9.0), px(18.0), px(27.0), px(36.0), px(45.0)]
        );
        assert_eq!(geom.y_boundaries(2), vec![px(0.0), px(18.0), px(36.0)]);
    }

    #[test]
    fn fractional_advance_rounds_once_then_tiles_exactly() {
        // A 10.4 px advance becomes a 10 px cell: ten cells are 100 px, not
        // the 104 px the unrounded advance would accumulate to.
        let geom = geometry(Point::default(), 10.4, 22.88, 1.0);
        let b = geom.x_boundaries(10);
        assert_eq!(b[10] - b[0], px(100.0));
        assert_eq!(geom.y_boundaries(1)[1], px(23.0));
    }

    #[test]
    fn edges_are_whole_device_pixels_at_fractional_scale() {
        // 21 device px cell at 1.25x = 16.8 logical px. Every edge must land
        // on a device pixel even though the logical stride is not exact.
        let geom = geometry(Point::default(), 21.0, 25.0, 1.25);
        assert_eq!(geom.metrics.cell_width, 21);
        assert_eq!(geom.cell_width, px(16.8));
        for col in 0..=40 {
            let device = geom.x_boundary(col).as_f32() * 1.25;
            assert!(
                (device - device.round()).abs() < 1e-3,
                "col {col} edge {device} is not a device pixel"
            );
            assert_eq!(geom.x_device(col), 21 * col as i32);
        }
    }

    #[test]
    fn fractional_origin_floors_to_a_device_pixel() {
        let geom = geometry(
            Point {
                x: px(0.4),
                y: px(0.7),
            },
            8.0,
            18.0,
            1.0,
        );
        assert_eq!(geom.x_boundary(0), px(0.0));
        assert_eq!(geom.y_boundary(0), px(0.0));
        assert_eq!(geom.x_boundary(3), px(24.0));
    }

    #[test]
    fn cell_span_bounds_uses_shared_boundaries() {
        let geom = geometry(
            Point {
                x: px(0.4),
                y: px(0.2),
            },
            8.4,
            18.2,
            1.0,
        );
        let bounds = geom.cell_span_bounds(1, 1, 2);
        assert_eq!(bounds.origin.x, px(8.0));
        assert_eq!(bounds.origin.y, px(18.0));
        assert_eq!(bounds.size.width, px(16.0));
        assert_eq!(bounds.size.height, px(18.0));
    }

    #[test]
    fn device_rect_converts_through_the_scale() {
        let geom = geometry(Point::default(), 10.0, 20.0, 2.0);
        let bounds = geom.device_rect(3, 5, 7, 9);
        assert_eq!(bounds.origin.x, px(1.5));
        assert_eq!(bounds.origin.y, px(2.5));
        assert_eq!(bounds.size.width, px(3.5));
        assert_eq!(bounds.size.height, px(4.5));
        assert_eq!(geom.device_rect(0, 0, -1, -1).size.width, px(0.0));
    }
}
