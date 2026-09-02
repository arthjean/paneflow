//! Kitty graphics placements.
//!
//! The session runtime resolved the geometry and uploaded the textures; this
//! only positions them. Placements arrive sorted by z-index, which decides
//! both the layer they draw in and their order within it: negative z goes
//! under the text, zero and above over it.

use gpui::{Bounds, Corners, Pixels, Window, point, px, size};

use crate::terminal::element::geometry::CellGeometry;
use crate::terminal::kitty::KittyPlacement;

/// Draw the placements whose z-index puts them under the text.
pub fn paint_below_text(placements: &[KittyPlacement], geom: &CellGeometry, window: &mut Window) {
    paint_layer(placements, geom, window, |z| z < 0);
}

/// Draw the placements whose z-index puts them over the text.
pub fn paint_above_text(placements: &[KittyPlacement], geom: &CellGeometry, window: &mut Window) {
    paint_layer(placements, geom, window, |z| z >= 0);
}

fn paint_layer(
    placements: &[KittyPlacement],
    geom: &CellGeometry,
    window: &mut Window,
    in_layer: impl Fn(i32) -> bool,
) {
    for placement in placements.iter().filter(|p| in_layer(p.z)) {
        let Some((destination, image_bounds)) = resolve(placement, geom) else {
            continue;
        };
        // `bounds` clips, `image_bounds` positions and scales: their
        // intersection is what actually appears, which is how a source
        // rectangle is expressed without a second texture.
        let _ = window.paint_image(
            destination,
            image_bounds,
            Corners::default(),
            placement.image.clone(),
            0,
            false,
        );
    }
}

/// Where the placement lands, and where the whole image would have to sit for
/// its source rectangle to line up with that.
///
/// Returns `None` for a degenerate placement, which is a program asking for a
/// zero-area draw rather than an error.
fn resolve(
    placement: &KittyPlacement,
    geom: &CellGeometry,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    if placement.width == 0
        || placement.height == 0
        || placement.source_width == 0
        || placement.source_height == 0
    {
        return None;
    }
    let origin = point(
        geom.origin.x + geom.cell_width * placement.col as f32,
        geom.origin.y + geom.line_height * placement.row as f32,
    );
    let destination = Bounds::new(
        origin,
        size(px(placement.width as f32), px(placement.height as f32)),
    );

    // The rendered size covers the source rectangle, so the whole image is
    // that much larger and offset by where the rectangle starts inside it.
    let scale_x = placement.width as f32 / placement.source_width as f32;
    let scale_y = placement.height as f32 / placement.source_height as f32;
    let image_size = placement.image.size(0);
    let image_width = i32::from(image_size.width).max(1) as f32;
    let image_height = i32::from(image_size.height).max(1) as f32;
    let image_bounds = Bounds::new(
        point(
            origin.x - px(placement.source_x as f32 * scale_x),
            origin.y - px(placement.source_y as f32 * scale_y),
        ),
        size(px(image_width * scale_x), px(image_height * scale_y)),
    );
    Some((destination, image_bounds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::RenderImage;
    use std::sync::Arc;

    fn texture(width: u32, height: u32) -> Arc<RenderImage> {
        let buffer =
            image::RgbaImage::from_raw(width, height, vec![0; (width * height * 4) as usize])
                .expect("buffer matches its dimensions");
        Arc::new(RenderImage::new([image::Frame::new(buffer)]))
    }

    fn placement(width: u32, height: u32, source: (u32, u32, u32, u32)) -> KittyPlacement {
        KittyPlacement {
            image: texture(20, 10),
            col: 2,
            row: 1,
            width,
            height,
            source_x: source.0,
            source_y: source.1,
            source_width: source.2,
            source_height: source.3,
            z: 0,
        }
    }

    fn geometry() -> CellGeometry {
        use crate::terminal::element::font::{FaceMetrics, cell_metrics_from_face};
        // An 8x16 cell: advance 8, ascent 12 + descent 4.
        let metrics = cell_metrics_from_face(
            FaceMetrics {
                ascent: 12.0,
                descent: 4.0,
                line_gap: 0.0,
                advance: 8.0,
                underline_position: 0.0,
                underline_thickness: 0.0,
                strikethrough_position: 0.0,
                strikethrough_thickness: 0.0,
                x_height: 0.0,
                cap_height: 0.0,
            },
            1.0,
            1.0,
            1.0,
        );
        CellGeometry::new(point(px(10.0), px(20.0)), metrics)
    }

    #[test]
    fn a_full_image_placement_maps_one_to_one() {
        let (destination, image_bounds) =
            resolve(&placement(20, 10, (0, 0, 20, 10)), &geometry()).expect("resolvable");
        // origin + 2 columns, + 1 row.
        assert_eq!(destination.origin, point(px(26.0), px(36.0)));
        assert_eq!(destination.size, size(px(20.0), px(10.0)));
        // Nothing is cropped, so the image sits exactly where it is drawn.
        assert_eq!(image_bounds, destination);
    }

    #[test]
    fn a_source_rectangle_shifts_and_scales_the_whole_image() {
        // The right half of a 20x10 image, drawn at twice its size.
        let (destination, image_bounds) =
            resolve(&placement(20, 20, (10, 0, 10, 10)), &geometry()).expect("resolvable");
        assert_eq!(destination.size, size(px(20.0), px(20.0)));
        // 2x scale, so the full image is 40x20 and starts 20px left of the
        // destination: exactly enough for its right half to land in it.
        assert_eq!(image_bounds.size, size(px(40.0), px(20.0)));
        assert_eq!(image_bounds.origin, point(px(6.0), px(36.0)));
        // The visible region is the destination, which is the source rect.
        assert_eq!(destination.intersect(&image_bounds), destination);
    }

    #[test]
    fn a_row_scrolled_above_the_viewport_lands_above_the_grid() {
        let mut scrolled = placement(20, 10, (0, 0, 20, 10));
        scrolled.row = -2;
        let (destination, _) = resolve(&scrolled, &geometry()).expect("resolvable");
        // Two rows above the origin, which the content mask then clips.
        assert_eq!(destination.origin.y, px(20.0 - 32.0));
    }

    #[test]
    fn a_degenerate_placement_is_skipped() {
        assert!(resolve(&placement(0, 10, (0, 0, 20, 10)), &geometry()).is_none());
        assert!(resolve(&placement(20, 10, (0, 0, 0, 10)), &geometry()).is_none());
    }
}
