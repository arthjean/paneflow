use gpui::{
    Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Styled, canvas, div, point, px,
    size,
};

const CORNER_EXTENT: f32 = 1.528_665;
const CORNER_CURVES: [[(f32, f32); 3]; 3] = [
    [(1.088_493, 0.), (0.868_407, 0.), (0.631_494, 0.074_911)],
    [
        (0.372_824, 0.169_060),
        (0.169_060, 0.372_824),
        (0.074_911, 0.631_494),
    ],
    [(0., 0.868_407), (0., 1.088_493), (0., CORNER_EXTENT)],
];

pub(crate) fn corner_for_height(height: Pixels, radius: Pixels) -> Pixels {
    radius.min(height / (2. * CORNER_EXTENT)).max(px(0.))
}

fn limited_radius(bounds: Bounds<Pixels>, radius: Pixels) -> Pixels {
    corner_for_height(bounds.size.height, radius)
        .min(bounds.size.width / (2. * CORNER_EXTENT))
        .max(px(0.))
}

fn trace(builder: &mut PathBuilder, bounds: Bounds<Pixels>, radius: Pixels) {
    let radius = limited_radius(bounds, radius);
    let (left, right) = (bounds.left(), bounds.right());
    let (top, bottom) = (bounds.top(), bounds.bottom());
    builder.move_to(point(left + radius * CORNER_EXTENT, top));
    for corner in 0..4 {
        let transform = |(x, y): (f32, f32)| match corner {
            0 => point(right - radius * x, top + radius * y),
            1 => point(right - radius * y, bottom - radius * x),
            2 => point(left + radius * x, bottom - radius * y),
            _ => point(left + radius * y, top + radius * x),
        };
        builder.line_to(transform((CORNER_EXTENT, 0.)));
        for [control_a, control_b, end] in CORNER_CURVES {
            builder.cubic_bezier_to(transform(end), transform(control_a), transform(control_b));
        }
    }
    builder.close();
}

pub(crate) fn squircle_fill(radius: Pixels, color: Hsla) -> impl IntoElement {
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if color.a <= f32::EPSILON {
                    return;
                }
                let mut builder = PathBuilder::fill();
                trace(&mut builder, bounds, radius);
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full(),
    )
}

pub(crate) fn squircle_border(radius: Pixels, width: Pixels, color: Hsla) -> impl IntoElement {
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if color.a <= f32::EPSILON {
                    return;
                }
                let half = width / 2.;
                let radius = limited_radius(bounds, radius);
                let inner = Bounds {
                    origin: bounds.origin + point(half, half),
                    size: size(
                        (bounds.size.width - width).max(px(0.)),
                        (bounds.size.height - width).max(px(0.)),
                    ),
                };
                let mut builder = PathBuilder::stroke(width);
                trace(&mut builder, inner, (radius - half).max(px(0.)));
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            },
        )
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    #[test]
    fn component_radii_fit_without_scaling() {
        for (width, height, radius) in [
            (284., 28., 9.),
            (22., 22., 7.),
            (22., 22., 6.),
            (300., 100., 20.),
            (200., 80., 18.),
            (200., 48., 14.),
        ] {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
            assert_eq!(limited_radius(bounds, px(radius)), px(radius));
            let mut builder = PathBuilder::fill();
            trace(&mut builder, bounds, px(radius));
            assert!(builder.build().is_ok());
        }
    }

    #[test]
    fn corners_cannot_overlap_in_small_bounds() {
        for (width, height) in [(1., 28.), (284., 1.), (0., 0.)] {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
            let extent = limited_radius(bounds, px(9.)) * CORNER_EXTENT;
            assert!(extent <= px(width / 2.));
            assert!(extent <= px(height / 2.));
        }
    }
}
