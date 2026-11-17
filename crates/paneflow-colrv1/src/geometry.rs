use skrifa::color::Extend;
use tiny_skia::Point;

const DEGENERATE: f64 = 1e-9;

pub fn apply_extend(t: f32, extend: Extend) -> f32 {
    if !t.is_finite() {
        return if t > 0.0 { 1.0 } else { 0.0 };
    }
    match extend {
        Extend::Repeat => t - t.floor(),
        Extend::Reflect => {
            let folded = t.rem_euclid(2.0);
            if folded > 1.0 {
                2.0 - folded
            } else {
                folded
            }
        }
        Extend::Pad | Extend::Unknown => t.clamp(0.0, 1.0),
    }
}

pub fn linear_t(point: Point, p0: Point, p1: Point) -> Option<f32> {
    let axis_x = f64::from(p1.x - p0.x);
    let axis_y = f64::from(p1.y - p0.y);
    let length_squared = axis_x * axis_x + axis_y * axis_y;
    if length_squared < DEGENERATE {
        return None;
    }
    let along = f64::from(point.x - p0.x) * axis_x + f64::from(point.y - p0.y) * axis_y;
    Some((along / length_squared) as f32)
}

pub fn radial_t(point: Point, c0: Point, r0: f32, c1: Point, r1: f32) -> Option<f32> {
    let center_dx = f64::from(c1.x - c0.x);
    let center_dy = f64::from(c1.y - c0.y);
    let radius_delta = f64::from(r1 - r0);
    let r0 = f64::from(r0);
    let point_dx = f64::from(point.x - c0.x);
    let point_dy = f64::from(point.y - c0.y);
    let radius_at = |omega: f64| r0 + omega * radius_delta;

    let a = center_dx * center_dx + center_dy * center_dy - radius_delta * radius_delta;
    let half_b = point_dx * center_dx + point_dy * center_dy + r0 * radius_delta;
    let c = point_dx * point_dx + point_dy * point_dy - r0 * r0;

    if a.abs() < DEGENERATE {
        if half_b.abs() < DEGENERATE {
            return None;
        }
        let omega = c / (2.0 * half_b);
        return (radius_at(omega) >= 0.0).then_some(omega as f32);
    }
    let discriminant = half_b * half_b - a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let first = (half_b + root) / a;
    let second = (half_b - root) / a;
    let (larger, smaller) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    if radius_at(larger) >= 0.0 {
        Some(larger as f32)
    } else if radius_at(smaller) >= 0.0 {
        Some(smaller as f32)
    } else {
        None
    }
}

pub fn clockwise_degrees(point: Point, center: Point) -> f32 {
    let dx = f64::from(point.x - center.x);
    let dy = f64::from(point.y - center.y);
    let counter_clockwise = dy.atan2(dx).to_degrees();
    let clockwise = (-counter_clockwise).rem_euclid(360.0);
    if clockwise >= 360.0 {
        0.0
    } else {
        clockwise as f32
    }
}

pub fn sweep_t(point: Point, center: Point, start_degrees: f32, end_degrees: f32) -> f32 {
    let angle = clockwise_degrees(point, center);
    let span = end_degrees - start_degrees;
    if span.abs() < f32::EPSILON {
        return if angle < start_degrees {
            f32::NEG_INFINITY
        } else {
            f32::INFINITY
        };
    }
    (angle - start_degrees) / span
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, y: f32) -> Point {
        Point::from_xy(x, y)
    }

    fn close(actual: f32, expected: f32) -> bool {
        (actual - expected).abs() < 1e-4
    }

    #[test]
    fn sweep_angles_run_clockwise_from_the_positive_x_axis_in_y_up_font_space() {
        let center = point(10.0, 20.0);
        let cases = [
            (point(15.0, 20.0), 0.0),
            (point(10.0, 15.0), 90.0),
            (point(5.0, 20.0), 180.0),
            (point(10.0, 25.0), 270.0),
            (point(15.0, 15.0), 45.0),
            (point(15.0, 25.0), 315.0),
        ];
        for (sample, expected) in cases {
            let angle = clockwise_degrees(sample, center);
            assert!(close(angle, expected), "{sample:?}: {angle} != {expected}");
        }
    }

    #[test]
    fn sweep_t_maps_the_angular_span_onto_the_color_line() {
        let center = point(0.0, 0.0);
        assert!(close(sweep_t(point(1.0, 0.0), center, 0.0, 360.0), 0.0));
        assert!(close(sweep_t(point(0.0, -1.0), center, 0.0, 360.0), 0.25));
        assert!(close(sweep_t(point(-1.0, 0.0), center, 0.0, 360.0), 0.5));
        assert!(close(sweep_t(point(0.0, 1.0), center, 0.0, 360.0), 0.75));
        assert!(close(sweep_t(point(-1.0, 0.0), center, 90.0, 270.0), 0.5));
        assert!(close(sweep_t(point(1.0, 1.0), center, 90.0, 270.0), 1.25));
        assert!(close(sweep_t(point(1.0, -1.0), center, 90.0, 270.0), -0.25));
    }

    #[test]
    fn a_zero_span_sweep_splits_the_circle_into_the_two_end_colors() {
        let center = point(0.0, 0.0);
        let before = sweep_t(point(1.0, -0.2), center, 90.0, 90.0);
        let after = sweep_t(point(-1.0, 0.0), center, 90.0, 90.0);
        assert!(close(apply_extend(before, Extend::Pad), 0.0));
        assert!(close(apply_extend(after, Extend::Pad), 1.0));
    }

    #[test]
    fn every_extend_mode_folds_the_color_line_as_the_spec_describes() {
        let cases = [
            (Extend::Pad, -0.25, 0.0),
            (Extend::Pad, 0.25, 0.25),
            (Extend::Pad, 1.25, 1.0),
            (Extend::Repeat, -0.25, 0.75),
            (Extend::Repeat, 0.25, 0.25),
            (Extend::Repeat, 1.25, 0.25),
            (Extend::Repeat, 2.75, 0.75),
            (Extend::Reflect, -0.25, 0.25),
            (Extend::Reflect, 0.25, 0.25),
            (Extend::Reflect, 1.25, 0.75),
            (Extend::Reflect, 2.75, 0.75),
            (Extend::Reflect, -1.25, 0.75),
            (Extend::Unknown, 1.5, 1.0),
        ];
        for (extend, t, expected) in cases {
            let folded = apply_extend(t, extend);
            assert!(
                close(folded, expected),
                "{extend:?} at {t}: {folded} != {expected}"
            );
        }
    }

    #[test]
    fn linear_t_projects_onto_the_p0_p1_axis() {
        let p0 = point(10.0, 0.0);
        let p1 = point(30.0, 0.0);
        assert_eq!(linear_t(point(20.0, 50.0), p0, p1), Some(0.5));
        assert_eq!(linear_t(point(0.0, -3.0), p0, p1), Some(-0.5));
        assert_eq!(linear_t(point(1.0, 1.0), p0, p0), None);
    }

    #[test]
    fn concentric_radial_t_is_the_distance_along_the_radius() {
        let center = point(0.0, 0.0);
        let at = |x: f32, y: f32| radial_t(point(x, y), center, 0.0, center, 10.0);
        assert!(at(5.0, 0.0).is_some_and(|t| close(t, 0.5)));
        assert!(at(0.0, -10.0).is_some_and(|t| close(t, 1.0)));
        assert!(at(14.0, 0.0).is_some_and(|t| close(t, 1.4)));
    }

    #[test]
    fn two_point_conical_t_picks_the_largest_circle_through_the_point() {
        let c0 = point(0.0, 0.0);
        let c1 = point(5.0, 0.0);
        assert!(radial_t(point(15.0, 0.0), c0, 0.0, c1, 10.0).is_some_and(|t| close(t, 1.0)));
        assert!(radial_t(point(-5.0, 0.0), c0, 0.0, c1, 10.0).is_some_and(|t| close(t, 1.0)));
        assert!(radial_t(point(2.5, 0.0), c0, 0.0, c1, 10.0).is_some_and(|t| close(t, 1.0 / 6.0)));
    }

    #[test]
    fn radial_t_leaves_points_outside_a_focal_cone_unpainted() {
        let c0 = point(0.0, 0.0);
        let c1 = point(10.0, 0.0);
        assert_eq!(radial_t(point(-5.0, 0.0), c0, 0.0, c1, 2.0), None);
        assert!(radial_t(point(10.0, 1.0), c0, 0.0, c1, 2.0).is_some());
    }

    #[test]
    fn a_negative_radius_cylinder_paints_nothing() {
        let center = point(0.0, 0.0);
        let shifted = point(4.0, 0.0);
        assert_eq!(radial_t(point(1.0, 0.0), center, -2.0, shifted, -2.0), None);
        assert!(radial_t(point(1.0, 0.0), center, 2.0, shifted, 2.0).is_some());
    }
}
