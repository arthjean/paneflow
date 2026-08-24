//! Continuous-corner (squircle) silhouettes for pane cards.
//!
//! GPUI's `Quad` carries `corner_radii: Corners<ScaledPixels>` and resolves
//! them with a circular-arc SDF; there is no corner-smoothing knob anywhere in
//! the crate. A circular arc joins the straight edge with a curvature step, and
//! at the radius the pane cards use that discontinuity is what reads as
//! "rounded rectangle" rather than the softer silhouette Apple ships as its
//! continuous corner.
//!
//! So the card paints its own outline as a path. The curve is a superellipse
//! quarter, `|u|^n + |v|^n = 1`, the family the continuous corner approximates:
//! curvature grows smoothly out of the edge instead of jumping to `1/r`. The
//! exponent is the only knob; `n = 2` degenerates back to the circular arc.
//!
//! Both helpers position themselves absolutely over the host's bounds, so the
//! host must be `relative()`. They are the only surface that may paint the card
//! fill: everything nested inside a pane card is transparent precisely so this
//! single path owns the corners (GPUI does not clip children to a parent
//! radius, so a child `bg` would repaint the arcs square).

use gpui::{
    Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Styled, canvas, div, point, px,
    size,
};

/// Superellipse exponent. Higher is squarer at the corner's midpoint and
/// gentler where it meets the edge. 4 is the classic squircle and sits at the
/// low end of the range that reads as the macOS/iOS continuous corner; going
/// higher flattens the midpoint enough that the corner starts reading as
/// chamfered rather than rounded.
const EXPONENT: f32 = 4.0;

/// Line segments emitted per corner. The parametrization below clusters
/// samples where curvature is highest (the corner's diagonal) and spreads them
/// where the curve is already flat against the edge, so 16 keeps the worst
/// chord deviation far under a tenth of a pixel at any radius a card uses.
const CORNER_SAMPLES: usize = 16;

/// Offsets of one corner sample from the corner vertex, as fractions of the
/// radius. `i = 0` sits on the first edge, `i = CORNER_SAMPLES` on the second.
fn corner_offset(i: usize) -> (f32, f32) {
    let theta = std::f32::consts::FRAC_PI_2 * (i as f32) / (CORNER_SAMPLES as f32);
    let e = 2.0 / EXPONENT;
    // `f32::cos(FRAC_PI_2)` lands a hair below zero, and a fractional `powf` of
    // a negative base is NaN - which lyon turns into a tessellation panic
    // rather than a bad pixel. Clamp the base instead of the result.
    let sup = |v: f32| v.max(0.0).powf(e);
    (1.0 - sup(theta.cos()), 1.0 - sup(theta.sin()))
}

/// Traces the closed squircle outline of `bounds` into `builder`, clockwise
/// from the top edge. The radius is clamped to half the shorter side so a
/// short pane degrades to a stadium instead of self-intersecting.
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

/// A filled squircle covering the host's bounds.
///
/// Fully transparent fills paint nothing rather than tessellating a path the
/// compositor would discard, which matters because the pane dim layer holds
/// this element at alpha 0 through the tail of its fade.
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

/// A stroked squircle hairline on the host's bounds.
///
/// The outline is inset by half the line width and its radius shrunk by the
/// same amount, so the stroke stays concentric with [`squircle_fill`] at the
/// same radius instead of straddling it.
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
        // At the corner's diagonal a circular arc sits `1 - cos(pi/4)` deep.
        // The whole point of the superellipse is to hug the corner more
        // tightly there while meeting the edges more gently.
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
