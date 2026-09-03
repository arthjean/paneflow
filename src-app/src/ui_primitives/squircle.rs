use gpui::{
    Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Styled, canvas, div, point, px,
    size,
};

const EXPONENT: f32 = 4.0;

const CORNER_SAMPLES: usize = 16;

fn corner_offset(i: usize) -> (f32, f32) {
    let theta = std::f32::consts::FRAC_PI_2 * (i as f32) / (CORNER_SAMPLES as f32);
    let e = 2.0 / EXPONENT;
    let sup = |v: f32| v.max(0.0).powf(e);
    (1.0 - sup(theta.cos()), 1.0 - sup(theta.sin()))
}

fn trace(builder: &mut PathBuilder, bounds: Bounds<Pixels>, radius: Pixels) {
    let (l, r) = (bounds.left(), bounds.right());
    let (t, b) = (bounds.top(), bounds.bottom());
    let rad = radius
        .min(bounds.size.width / 2.)
        .min(bounds.size.height / 2.);
    if rad <= px(0.) {
        return;
    }

    builder.move_to(point(l + rad, t));
    builder.line_to(point(r - rad, t));
    for i in (0..=CORNER_SAMPLES).rev() {
        let (dx, dy) = corner_offset(i);
        builder.line_to(point(r - rad * dx, t + rad * dy));
    }
    builder.line_to(point(r, b - rad));
    for i in 0..=CORNER_SAMPLES {
        let (dx, dy) = corner_offset(i);
        builder.line_to(point(r - rad * dx, b - rad * dy));
    }
    builder.line_to(point(l + rad, b));
    for i in (0..=CORNER_SAMPLES).rev() {
        let (dx, dy) = corner_offset(i);
        builder.line_to(point(l + rad * dx, b - rad * dy));
    }
    builder.line_to(point(l, t + rad));
    for i in 0..=CORNER_SAMPLES {
        let (dx, dy) = corner_offset(i);
        builder.line_to(point(l + rad * dx, t + rad * dy));
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

    #[test]
    fn corner_endpoints_sit_on_the_edges() {
        let (dx, dy) = corner_offset(0);
        assert!(dx.abs() < 1e-5, "first sample leaves the edge: {dx}");
        assert!((dy - 1.0).abs() < 1e-5, "first sample is not a radius deep");

        let (dx, dy) = corner_offset(CORNER_SAMPLES);
        assert!((dx - 1.0).abs() < 1e-5, "last sample is not a radius along");
        assert!(dy.abs() < 1e-5, "last sample leaves the edge: {dy}");
    }

    #[test]
    fn corner_is_squarer_than_a_circular_arc() {
        let circular = 1.0 - std::f32::consts::FRAC_PI_4.cos();
        let (dx, dy) = corner_offset(CORNER_SAMPLES / 2);
        assert!(
            (dx - dy).abs() < 1e-5,
            "the corner must stay symmetric across its diagonal"
        );
        assert!(
            dx < circular,
            "superellipse midpoint {dx} is not tighter than the arc {circular}"
        );
    }

    #[test]
    fn samples_advance_monotonically() {
        let mut previous = corner_offset(0);
        for i in 1..=CORNER_SAMPLES {
            let current = corner_offset(i);
            assert!(
                current.0 > previous.0 && current.1 < previous.1,
                "sample {i} back-tracks: {previous:?} -> {current:?}"
            );
            previous = current;
        }
    }
}
