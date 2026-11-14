use std::cell::RefCell;

use gpui::{
    AnyElement, Bounds, Corners, Hsla, IntoElement, ParentElement, Path, PathBuilder, Pixels,
    Point, Styled, canvas, div, point, px, size,
};

use super::interface_style::{Shape, circular, corner, corners, scoped, shape};

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

fn trace(builder: &mut PathBuilder, bounds: Bounds<Pixels>, radii: Corners<Pixels>) {
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
    builder.move_to(point(left + radii[3] * extent, top));
    for (corner, radius) in radii.into_iter().enumerate() {
        let transform = |(x, y): (f32, f32)| match corner {
            0 => point(right - radius * x, top + radius * y),
            1 => point(right - radius * y, bottom - radius * x),
            2 => point(left + radius * x, bottom - radius * y),
            _ => point(left + radius * y, top + radius * x),
        };
        builder.line_to(transform((extent, 0.)));
        for &[control_a, control_b, end] in curves {
            builder.cubic_bezier_to(transform(end), transform(control_a), transform(control_b));
        }
    }
    builder.close();
}

const MAX_CACHED_SHAPES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Outline {
    Fill,
    Stroke(Pixels),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShapeKey {
    outline: Outline,
    width: Pixels,
    height: Pixels,
    radius: Pixels,
    scale_factor: f32,
    shape: Shape,
}

thread_local! {
    static SHAPES: RefCell<Vec<(ShapeKey, Path<Pixels>)>> = const { RefCell::new(Vec::new()) };
}

#[cfg(test)]
thread_local! {
    static BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn squircle_builds() -> usize {
    BUILDS.with(std::cell::Cell::get)
}

pub(crate) fn cached_squircle_path(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    scale_factor: f32,
) -> Option<Path<Pixels>> {
    cached_shape(bounds, radius, Outline::Fill, scale_factor)
}

pub(crate) fn cached_squircle_stroke_path(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    width: Pixels,
    scale_factor: f32,
) -> Option<Path<Pixels>> {
    cached_shape(bounds, radius, Outline::Stroke(width), scale_factor)
}

fn cached_shape(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    outline: Outline,
    scale_factor: f32,
) -> Option<Path<Pixels>> {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) {
        return None;
    }
    let key = ShapeKey {
        outline,
        width: bounds.size.width,
        height: bounds.size.height,
        radius,
        scale_factor,
        shape: shape(),
    };
    SHAPES.with(|shapes| {
        let mut shapes = shapes.borrow_mut();
        if let Some((_, shape)) = shapes.iter().find(|(held, _)| *held == key) {
            return Some(translated(shape, bounds.origin));
        }
        let local = Bounds::new(point(px(0.), px(0.)), bounds.size);
        let shape = match outline {
            Outline::Fill => squircle_path(local, radius),
            Outline::Stroke(width) => squircle_stroke_path(local, radius, width),
        }?;
        if shapes.len() >= MAX_CACHED_SHAPES {
            shapes.clear();
        }
        let placed = translated(&shape, bounds.origin);
        shapes.push((key, shape));
        Some(placed)
    })
}

fn translated(shape: &Path<Pixels>, offset: Point<Pixels>) -> Path<Pixels> {
    let mut placed = shape.clone();
    placed.bounds.origin += offset;
    for vertex in &mut placed.vertices {
        vertex.xy_position += offset;
    }
    placed
}

pub(crate) fn squircle_path(bounds: Bounds<Pixels>, radius: Pixels) -> Option<Path<Pixels>> {
    squircle_corners_path(bounds, Corners::all(radius))
}

fn squircle_corners_path(bounds: Bounds<Pixels>, radii: Corners<Pixels>) -> Option<Path<Pixels>> {
    #[cfg(test)]
    BUILDS.with(|builds| builds.set(builds.get() + 1));
    let mut builder = PathBuilder::fill();
    trace(&mut builder, bounds, radii);
    builder.build().ok()
}

pub(crate) fn squircle_stroke_path(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    width: Pixels,
) -> Option<Path<Pixels>> {
    squircle_corners_stroke_path(bounds, Corners::all(radius), width)
}

fn squircle_corners_stroke_path(
    bounds: Bounds<Pixels>,
    radii: Corners<Pixels>,
    width: Pixels,
) -> Option<Path<Pixels>> {
    #[cfg(test)]
    BUILDS.with(|builds| builds.set(builds.get() + 1));
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
                move |bounds, _, window, _| {
                    if color.a <= f32::EPSILON {
                        return;
                    }
                    if let Some(path) = squircle_corners_path(bounds, radii) {
                        window.paint_path(path, color);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
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
            trace(&mut builder, bounds, Corners::all(px(radius)));
            assert!(builder.build().is_ok());
        }
    }

    fn close(a: Point<Pixels>, b: Point<Pixels>) -> bool {
        let delta = a - b;
        delta.x.abs() < px(0.001) && delta.y.abs() < px(0.001)
    }

    fn assert_same_shape(cached: &Path<Pixels>, fresh: &Path<Pixels>) {
        assert!(close(cached.bounds.origin, fresh.bounds.origin));
        assert!(close(
            point(cached.bounds.size.width, cached.bounds.size.height),
            point(fresh.bounds.size.width, fresh.bounds.size.height)
        ));
        assert_eq!(cached.vertices.len(), fresh.vertices.len());
        for (cached, fresh) in cached.vertices.iter().zip(&fresh.vertices) {
            assert!(close(cached.xy_position, fresh.xy_position));
            assert_eq!(cached.st_position, fresh.st_position);
        }
    }

    #[test]
    fn a_cached_shape_matches_a_fresh_build_at_any_origin() {
        let radius = px(5.);
        for origin in [
            point(px(0.), px(0.)),
            point(px(37.5), px(-1.)),
            point(px(400.), px(12.)),
        ] {
            let bounds = Bounds::new(origin, size(px(24.), px(18.)));
            let fresh = squircle_path(bounds, radius).unwrap();
            let cached = cached_squircle_path(bounds, radius, 2.).unwrap();
            assert_same_shape(&cached, &fresh);
            let stroke = squircle_stroke_path(bounds, radius, px(1.)).unwrap();
            let cached_stroke = cached_squircle_stroke_path(bounds, radius, px(1.), 2.).unwrap();
            assert_same_shape(&cached_stroke, &stroke);
        }
    }

    #[test]
    fn zero_sized_bounds_draw_nothing_and_cache_nothing() {
        let before = SHAPES.with(|shapes| shapes.borrow().len());
        let builds = squircle_builds();
        for bounds in [
            Bounds::new(point(px(3.), px(3.)), size(px(0.), px(18.))),
            Bounds::new(point(px(3.), px(3.)), size(px(24.), px(0.))),
            Bounds::new(point(px(3.), px(3.)), size(px(24.), px(-1.))),
        ] {
            assert!(cached_squircle_path(bounds, px(5.), 1.).is_none());
            assert!(cached_squircle_stroke_path(bounds, px(5.), px(1.), 1.).is_none());
        }
        assert_eq!(SHAPES.with(|shapes| shapes.borrow().len()), before);
        assert_eq!(squircle_builds(), builds);
    }

    #[test]
    fn a_surface_shape_is_a_circular_rounded_rectangle_on_the_surface_scale() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        scoped(Shape::Surface, || {
            assert_eq!(limited_radius(bounds, px(24.)), px(12.));
            assert_eq!(limited_radius(bounds, px(18.)), px(14.));
            let mut builder = PathBuilder::fill();
            trace(&mut builder, bounds, Corners::all(px(24.)));
            let shape = builder.build().unwrap();
            assert!(close(shape.bounds.origin, bounds.origin));
            let arc_start = point(px(300. - 12.), px(0.));
            assert!(
                shape
                    .vertices
                    .iter()
                    .any(|vertex| close(vertex.xy_position, arc_start)),
                "the top edge runs to one corner radius before the corner"
            );
            assert!(squircle_stroke_path(bounds, px(24.), px(1.)).is_some());
        });
        assert_eq!(limited_radius(bounds, px(24.)), px(24.));
    }

    #[test]
    fn a_classic_shape_is_a_circular_rounded_rectangle_on_the_classic_scale() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(200.)));
        let row = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(28.)));
        let surface = scoped(Shape::Surface, || {
            cached_squircle_path(row, px(14.), 1.5).unwrap()
        });
        scoped(Shape::Classic, || {
            assert_eq!(limited_radius(bounds, px(24.)), px(12.));
            assert_eq!(limited_radius(bounds, px(18.)), px(14.));
            let mut builder = PathBuilder::fill();
            trace(&mut builder, bounds, Corners::all(px(24.)));
            let shape = builder.build().unwrap();
            let arc_start = point(px(300. - 12.), px(0.));
            assert!(
                shape
                    .vertices
                    .iter()
                    .any(|vertex| close(vertex.xy_position, arc_start)),
                "the top edge runs to one corner radius before the corner"
            );
            let classic = cached_squircle_path(row, px(14.), 1.5).unwrap();
            assert!(
                classic
                    .vertices
                    .iter()
                    .zip(&surface.vertices)
                    .any(|(classic, surface)| !close(classic.xy_position, surface.xy_position)),
                "the shape cache keeps the surface and the Classic corner apart"
            );
        });
    }

    #[test]
    fn the_shape_cache_keeps_one_entry_per_shape() {
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(120.), px(90.)));
        let continuous = cached_squircle_path(bounds, px(24.), 1.25).unwrap();
        let circular = scoped(Shape::Surface, || {
            cached_squircle_path(bounds, px(24.), 1.25).unwrap()
        });
        assert_ne!(continuous.vertices.len(), circular.vertices.len());
        let again = cached_squircle_path(bounds, px(24.), 1.25).unwrap();
        assert_same_shape(&again, &continuous);
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
