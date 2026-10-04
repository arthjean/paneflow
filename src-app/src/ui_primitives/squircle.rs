use std::cell::RefCell;

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement, ParentElement, Path, PathBuilder, Pixels, Point, Styled,
    canvas, div, point, px, size,
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
    #[cfg(test)]
    BUILDS.with(|builds| builds.set(builds.get() + 1));
    let mut builder = PathBuilder::fill();
    trace(&mut builder, bounds, radius);
    builder.build().ok()
}

pub(crate) fn squircle_stroke_path(
    bounds: Bounds<Pixels>,
    radius: Pixels,
    width: Pixels,
) -> Option<Path<Pixels>> {
    #[cfg(test)]
    BUILDS.with(|builds| builds.set(builds.get() + 1));
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
    builder.build().ok()
}

pub(crate) fn squircle_fill(radius: Pixels, color: Hsla) -> AnyElement {
    if translucent_over_transparent_window(color) {
        return div()
            .absolute()
            .inset_0()
            .rounded(radius)
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
                    if let Some(path) = squircle_path(bounds, radius) {
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
    div().absolute().inset_0().child(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                if color.a <= f32::EPSILON {
                    return;
                }
                if let Some(path) = squircle_stroke_path(bounds, radius, width) {
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
    fn corners_cannot_overlap_in_small_bounds() {
        for (width, height) in [(1., 28.), (284., 1.), (0., 0.)] {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
            let extent = limited_radius(bounds, px(9.)) * CORNER_EXTENT;
            assert!(extent <= px(width / 2.));
            assert!(extent <= px(height / 2.));
        }
    }
}
