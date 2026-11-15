use gpui::{
    AnyElement, Bounds, Corners, Hsla, IntoElement, ParentElement, Path, PathBuilder, Pixels,
    Point, Styled, Window, canvas, div, point, px, size,
};

use super::interface_style::{circular, corner, corners, scoped, shape};

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
const ARC_EXTENT: f32 = 1.;
const ARC_HANDLE: f32 = 0.447_715_25;
const ARC_CURVES: [[(f32, f32); 3]; 1] = [[(ARC_HANDLE, 0.), (0., ARC_HANDLE), (0., ARC_EXTENT)]];

pub(crate) fn corner_for_height(height: Pixels, radius: Pixels) -> Pixels {
    radius.min(height / (2. * CORNER_EXTENT)).max(px(0.))
}

fn limited_radius(bounds: Bounds<Pixels>, radius: Pixels) -> Pixels {
    if circular() {
        return corner(radius, bounds.size.width.min(bounds.size.height));
    }
    corner_for_height(bounds.size.height, radius)
        .min(bounds.size.width / (2. * CORNER_EXTENT))
        .max(px(0.))
}

trait Contour {
    fn move_to(&mut self, to: Point<Pixels>);
    fn line_to(&mut self, to: Point<Pixels>);
    fn cubic_to(&mut self, to: Point<Pixels>, control_a: Point<Pixels>, control_b: Point<Pixels>);
    fn close(&mut self);
}

impl Contour for PathBuilder {
    fn move_to(&mut self, to: Point<Pixels>) {
        PathBuilder::move_to(self, to);
    }

    fn line_to(&mut self, to: Point<Pixels>) {
        PathBuilder::line_to(self, to);
    }

    fn cubic_to(&mut self, to: Point<Pixels>, control_a: Point<Pixels>, control_b: Point<Pixels>) {
        self.cubic_bezier_to(to, control_a, control_b);
    }

    fn close(&mut self) {
        PathBuilder::close(self);
    }
}

fn trace(contour: &mut impl Contour, bounds: Bounds<Pixels>, radii: Corners<Pixels>) {
    let radii = [
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
        radii.top_left,
    ]
    .map(|radius| limited_radius(bounds, radius));
    let (extent, curves): (f32, &[[(f32, f32); 3]]) = if circular() {
        (ARC_EXTENT, &ARC_CURVES)
    } else {
        (CORNER_EXTENT, &CORNER_CURVES)
    };
    let (left, right) = (bounds.left(), bounds.right());
    let (top, bottom) = (bounds.top(), bounds.bottom());
    contour.move_to(point(left + radii[3] * extent, top));
    for (corner, radius) in radii.into_iter().enumerate() {
        let transform = |(x, y): (f32, f32)| match corner {
            0 => point(right - radius * x, top + radius * y),
            1 => point(right - radius * y, bottom - radius * x),
            2 => point(left + radius * x, bottom - radius * y),
            _ => point(left + radius * y, top + radius * x),
        };
        contour.line_to(transform((extent, 0.)));
        for &[control_a, control_b, end] in curves {
            contour.cubic_to(transform(end), transform(control_a), transform(control_b));
        }
    }
    contour.close();
}

#[cfg(test)]
thread_local! {
    static BUILDS: std::cell::RefCell<Vec<Bounds<Pixels>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn squircle_builds() -> usize {
    BUILDS.with(|builds| builds.borrow().len())
}

#[cfg(test)]
pub(crate) fn squircle_builds_since(start: usize) -> Vec<Bounds<Pixels>> {
    BUILDS.with(|builds| builds.borrow().get(start..).unwrap_or_default().to_vec())
}

fn positive(value: Pixels) -> bool {
    let value = f32::from(value);
    value.is_finite() && value > 0.
}

fn traceable(bounds: Bounds<Pixels>, radii: Corners<Pixels>) -> bool {
    let edges = [bounds.left(), bounds.right(), bounds.top(), bounds.bottom()];
    let radii = [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ];
    positive(bounds.size.width)
        && positive(bounds.size.height)
        && edges
            .into_iter()
            .chain(radii)
            .all(|value| f32::from(value).is_finite())
}

pub(crate) fn squircle_path(bounds: Bounds<Pixels>, radius: Pixels) -> Option<Path> {
    squircle_corners_path(bounds, Corners::all(radius))
}

fn squircle_corners_path(bounds: Bounds<Pixels>, radii: Corners<Pixels>) -> Option<Path> {
    if !traceable(bounds, radii) {
        return None;
    }
    #[cfg(test)]
    BUILDS.with(|builds| builds.borrow_mut().push(bounds));
    let mut builder = PathBuilder::fill();
    trace(&mut builder, bounds, radii);
    builder.build().ok()
}

pub(crate) fn squircle_stroke_path(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    width: Pixels,
) -> Option<Path> {
    squircle_corners_stroke_path(bounds, Corners::all(radius), width)
}

fn squircle_corners_stroke_path(
    bounds: Bounds<Pixels>,
    radii: Corners<Pixels>,
    width: Pixels,
) -> Option<Path> {
    if !traceable(bounds, radii) || !positive(width) {
        return None;
    }
    #[cfg(test)]
    BUILDS.with(|builds| builds.borrow_mut().push(bounds));
    let half = width / 2.;
    let inset = |radius: Pixels| (limited_radius(bounds, radius) - half).max(px(0.));
    let inner = Bounds {
        origin: bounds.origin + point(half, half),
        size: size(
            (bounds.size.width - width).max(px(0.)),
            (bounds.size.height - width).max(px(0.)),
        ),
    };
    let radii = Corners {
        top_left: inset(radii.top_left),
        top_right: inset(radii.top_right),
        bottom_right: inset(radii.bottom_right),
        bottom_left: inset(radii.bottom_left),
    };
    let mut builder = PathBuilder::stroke(width);
    trace(&mut builder, inner, radii);
    builder.build().ok()
}

pub(crate) fn squircle_fill(radius: Pixels, color: Hsla) -> AnyElement {
    squircle_corners_fill(Corners::all(radius), color)
}

pub(crate) fn squircle_corners_fill(radii: Corners<Pixels>, color: Hsla) -> AnyElement {
    if circular() {
        let shape = shape();
        return div()
            .absolute()
            .inset_0()
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        if color.a <= f32::EPSILON {
                            return;
                        }
                        window.paint_quad(
                            gpui::fill(bounds, color)
                                .corner_radii(scoped(shape, || corners(bounds, radii))),
                        );
                    },
                )
                .size_full(),
            )
            .into_any_element();
    }
    if translucent_over_transparent_window(color) {
        return div()
            .absolute()
            .inset_0()
            .rounded_tl(radii.top_left)
            .rounded_tr(radii.top_right)
            .rounded_br(radii.bottom_right)
            .rounded_bl(radii.bottom_left)
            .bg(color)
            .into_any_element();
    }
    div()
        .absolute()
        .inset_0()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| paint_squircle_fill(bounds, radii, color, window),
            )
            .size_full(),
        )
        .into_any_element()
}

fn paint_squircle_fill(
    bounds: Bounds<Pixels>,
    radii: Corners<Pixels>,
    color: Hsla,
    window: &mut Window,
) {
    if color.a <= f32::EPSILON {
        return;
    }
    if let Some(path) = squircle_corners_path(bounds, radii) {
        window.paint_path(path, color);
    }
}

pub(crate) fn squircle_fill_blend(
    radius: Pixels,
    from: Hsla,
    to: Hsla,
    progress: std::rc::Rc<std::cell::Cell<f32>>,
) -> AnyElement {
    let circular = circular();
    let shape = shape();
    div()
        .absolute()
        .inset_0()
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let color = crate::ui_primitives::lerp_color(from, to, progress.get());
                    if color.a <= f32::EPSILON {
                        return;
                    }
                    if circular {
                        window.paint_quad(
                            gpui::fill(bounds, color).corner_radii(scoped(shape, || {
                                corners(bounds, Corners::all(radius))
                            })),
                        );
                    } else if translucent_over_transparent_window(color) {
                        window.paint_quad(gpui::fill(bounds, color).corner_radii(radius));
                    } else if let Some(path) = squircle_path(bounds, radius) {
                        window.paint_path(path, color);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn translucent_over_transparent_window(color: Hsla) -> bool {
    #[cfg(target_os = "linux")]
    {
        color.a < 1.0 && crate::window_chrome::linux_backdrop::translucent_window_active()
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = color;
        false
    }
}

pub(crate) fn squircle_border(radius: Pixels, width: Pixels, color: Hsla) -> impl IntoElement {
    squircle_corners_border(Corners::all(radius), width, color)
}

pub(crate) fn squircle_corners_border(
    radii: Corners<Pixels>,
    width: Pixels,
    color: Hsla,
) -> impl IntoElement {
    let circular = circular();
    let shape = shape();
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if color.a <= f32::EPSILON {
                    return;
                }
                if circular {
                    window.paint_quad(gpui::quad(
                        bounds,
                        scoped(shape, || corners(bounds, radii)),
                        gpui::transparent_black(),
                        width,
                        color,
                        gpui::BorderStyle::default(),
                    ));
                } else if let Some(path) = squircle_corners_stroke_path(bounds, radii, width) {
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
    use crate::ui_primitives::interface_style::Shape;
    use gpui::{Render, TestAppContext, size};

    #[derive(Default)]
    struct Trace(Vec<Point<Pixels>>);

    impl Contour for Trace {
        fn move_to(&mut self, to: Point<Pixels>) {
            self.0.push(to);
        }

        fn line_to(&mut self, to: Point<Pixels>) {
            self.0.push(to);
        }

        fn cubic_to(
            &mut self,
            to: Point<Pixels>,
            control_a: Point<Pixels>,
            control_b: Point<Pixels>,
        ) {
            self.0.extend([control_a, control_b, to]);
        }

        fn close(&mut self) {}
    }

    fn traced(bounds: Bounds<Pixels>, radius: Pixels) -> Vec<Point<Pixels>> {
        let mut recorder = Trace::default();
        trace(&mut recorder, bounds, Corners::all(radius));
        recorder.0
    }

    fn close(a: Point<Pixels>, b: Point<Pixels>) -> bool {
        let delta = a - b;
        delta.x.abs() < px(0.001) && delta.y.abs() < px(0.001)
    }

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
            assert!(squircle_path(bounds, px(radius)).is_some());
        }
    }

    #[test]
    fn every_request_builds_one_shape_at_the_requested_bounds() {
        let radius = px(5.);
        let start = squircle_builds();
        let mut requested = Vec::new();
        for origin in [
            point(px(0.), px(0.)),
            point(px(37.5), px(-1.)),
            point(px(400.), px(12.)),
        ] {
            let bounds = Bounds::new(origin, size(px(24.), px(18.)));
            for _ in 0..2 {
                assert!(squircle_path(bounds, radius).is_some());
                assert!(squircle_stroke_path(bounds, radius, px(1.)).is_some());
                requested.extend([bounds, bounds]);
            }
        }
        assert_eq!(squircle_builds_since(start), requested);
    }

    #[test]
    fn zero_sized_or_negative_bounds_build_nothing() {
        let builds = squircle_builds();
        for bounds in [
            Bounds::new(point(px(3.), px(3.)), size(px(0.), px(18.))),
            Bounds::new(point(px(3.), px(3.)), size(px(24.), px(0.))),
            Bounds::new(point(px(3.), px(3.)), size(px(24.), px(-1.))),
            Bounds::new(point(px(3.), px(3.)), size(px(-24.), px(18.))),
        ] {
            assert!(squircle_path(bounds, px(5.)).is_none());
            assert!(squircle_stroke_path(bounds, px(5.), px(1.)).is_none());
            assert!(squircle_corners_path(bounds, Corners::all(px(5.))).is_none());
            assert!(squircle_corners_stroke_path(bounds, Corners::all(px(5.)), px(1.)).is_none());
        }
        assert_eq!(squircle_builds(), builds);
    }

    #[test]
    fn non_finite_geometry_never_reaches_the_path_builder() {
        let builds = squircle_builds();
        let shape = size(px(24.), px(18.));
        for bounds in [
            Bounds::new(point(px(f32::INFINITY), px(0.)), shape),
            Bounds::new(point(px(0.), px(f32::NAN)), shape),
            Bounds::new(point(px(0.), px(0.)), size(px(f32::NAN), px(18.))),
            Bounds::new(point(px(0.), px(0.)), size(px(24.), px(f32::INFINITY))),
            Bounds::new(point(px(f32::MAX), px(0.)), size(px(f32::MAX), px(18.))),
        ] {
            assert!(squircle_path(bounds, px(5.)).is_none());
            assert!(squircle_stroke_path(bounds, px(5.), px(1.)).is_none());
        }
        let bounds = Bounds::new(point(px(0.), px(0.)), shape);
        assert!(squircle_path(bounds, px(f32::NAN)).is_none());
        assert!(squircle_stroke_path(bounds, px(f32::INFINITY), px(1.)).is_none());
        for width in [px(f32::NAN), px(0.), px(-1.)] {
            assert!(squircle_stroke_path(bounds, px(5.), width).is_none());
        }
        assert_eq!(squircle_builds(), builds);
    }

    struct BrokenThenValid;

    impl Render for BrokenThenValid {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            canvas(
                |_, _, _| {},
                |bounds, _, window, _| {
                    let broken = Bounds::new(point(px(f32::NAN), px(0.)), bounds.size);
                    paint_squircle_fill(broken, Corners::all(px(8.)), gpui::white(), window);
                    paint_squircle_fill(bounds, Corners::all(px(8.)), gpui::white(), window);
                },
            )
            .size_full()
        }
    }

    #[gpui::test]
    fn a_shape_that_cannot_be_built_is_skipped_and_the_frame_completes(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| BrokenThenValid);
        cx.simulate_resize(size(px(120.), px(60.)));
        let valid = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(60.)));
        let start = squircle_builds();
        for _ in 0..2 {
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
        assert_eq!(squircle_builds_since(start), vec![valid, valid]);
    }

    #[test]
    fn a_surface_shape_is_a_circular_rounded_rectangle_on_the_surface_scale() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        scoped(Shape::Surface, || {
            assert_eq!(limited_radius(bounds, px(24.)), px(12.));
            assert_eq!(limited_radius(bounds, px(18.)), px(14.));
            let arc_start = point(px(300. - 12.), px(0.));
            assert!(
                traced(bounds, px(24.))
                    .iter()
                    .any(|&vertex| close(vertex, arc_start)),
                "the top edge runs to one corner radius before the corner"
            );
            assert!(squircle_path(bounds, px(24.)).is_some());
            assert!(squircle_stroke_path(bounds, px(24.), px(1.)).is_some());
        });
        assert_eq!(limited_radius(bounds, px(24.)), px(24.));
    }

    #[test]
    fn a_classic_shape_is_a_circular_rounded_rectangle_on_the_classic_scale() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        let row = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(28.)));
        let surface = scoped(Shape::Surface, || traced(row, px(14.)));
        scoped(Shape::Classic, || {
            assert_eq!(limited_radius(bounds, px(24.)), px(12.));
            assert_eq!(limited_radius(bounds, px(18.)), px(14.));
            let arc_start = point(px(300. - 12.), px(0.));
            assert!(
                traced(bounds, px(24.))
                    .iter()
                    .any(|&vertex| close(vertex, arc_start)),
                "the top edge runs to one corner radius before the corner"
            );
            let classic = traced(row, px(14.));
            assert_eq!(classic.len(), surface.len());
            assert!(
                classic
                    .iter()
                    .zip(&surface)
                    .any(|(&classic, &surface)| !close(classic, surface)),
                "the surface and the Classic corner trace different outlines"
            );
        });
    }

    #[test]
    fn continuous_corners_trace_three_curves_where_circular_corners_trace_one() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(90.)));
        let continuous = traced(bounds, px(24.));
        let circular = scoped(Shape::Surface, || traced(bounds, px(24.)));
        assert_eq!(continuous.len(), 1 + 4 * (1 + 3 * 3));
        assert_eq!(circular.len(), 1 + 4 * (1 + 3));
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
