use std::f32::consts::{PI, TAU};

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Point, Styled,
    Window, canvas, div, point, px,
};
use paneflow_agent_config::usage::model::UsageLevel;

use crate::settings::components::with_alpha;
use crate::theme::UiColors;

const MIN_DIAMETER: f32 = 8.;
const CONCENTRIC_GAP: f32 = 3.;
const TRACK_ALPHA: f32 = 0.13;
const INNER_ARC_ALPHA: f32 = 0.72;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RingSize {
    Small,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the Settings Usage tiles of US-014 draw the 44 px ring"
        )
    )]
    Medium,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the Settings Usage tiles of US-014 draw the 76 px ring"
        )
    )]
    Large,
}

impl RingSize {
    pub(crate) const fn diameter(self) -> f32 {
        match self {
            Self::Small => 16.,
            Self::Medium => 44.,
            Self::Large => 76.,
        }
    }

    pub(crate) const fn stroke(self) -> f32 {
        match self {
            Self::Small => 2.5,
            Self::Medium => 4.5,
            Self::Large => 7.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ArcSweep {
    pub(crate) start_radians: f32,
    pub(crate) end_radians: f32,
    pub(crate) clockwise: bool,
    pub(crate) closed: bool,
}

pub(crate) fn arc_sweep(value: f64) -> Option<ArcSweep> {
    let value = if value.is_nan() {
        0.
    } else {
        value.clamp(0., 1.)
    };
    if value <= 0. {
        return None;
    }
    Some(ArcSweep {
        start_radians: 0.,
        end_radians: value as f32 * TAU,
        clockwise: true,
        closed: value >= 1.,
    })
}

pub(crate) fn drawable(diameter: f32, stroke: f32) -> bool {
    diameter >= MIN_DIAMETER && stroke > 0. && stroke < diameter / 2.
}

pub(crate) fn level_color(level: UsageLevel, ui: UiColors) -> Hsla {
    match level {
        UsageLevel::Normal => ui.accent,
        UsageLevel::Warn => crate::widgets::callout::warning_hue(),
        UsageLevel::Critical => ui.agent_error,
    }
}

#[derive(Clone, Copy)]
struct Gauge {
    radius: f32,
    stroke: f32,
    value: f64,
    color: Hsla,
}

pub(crate) fn ring(size: RingSize, value: f64, level: UsageLevel, ui: UiColors) -> AnyElement {
    let gauge = Gauge {
        radius: (size.diameter() - size.stroke()) / 2.,
        stroke: size.stroke(),
        value,
        color: level_color(level, ui),
    };
    ring_canvas(size, vec![gauge], ui)
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the Settings Usage tiles of US-014 draw the week and session rings"
    )
)]
pub(crate) fn concentric_rings(
    size: RingSize,
    outer: (f64, UsageLevel),
    inner: (f64, UsageLevel),
    ui: UiColors,
) -> AnyElement {
    let stroke = size.stroke();
    let outer_radius = (size.diameter() - stroke) / 2.;
    let gauges = vec![
        Gauge {
            radius: outer_radius,
            stroke,
            value: outer.0,
            color: level_color(outer.1, ui),
        },
        Gauge {
            radius: outer_radius - stroke - CONCENTRIC_GAP,
            stroke,
            value: inner.0,
            color: with_alpha(level_color(inner.1, ui), INNER_ARC_ALPHA),
        },
    ];
    ring_canvas(size, gauges, ui)
}

fn ring_canvas(size: RingSize, gauges: Vec<Gauge>, ui: UiColors) -> AnyElement {
    let diameter = size.diameter();
    let track = with_alpha(ui.text, TRACK_ALPHA);
    div()
        .flex_none()
        .size(px(diameter))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if !drawable(diameter, size.stroke()) {
                        return;
                    }
                    for gauge in &gauges {
                        paint_gauge(bounds, *gauge, track, window);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
}

fn paint_gauge(bounds: Bounds<Pixels>, gauge: Gauge, track: Hsla, window: &mut Window) {
    if gauge.radius <= gauge.stroke / 2. {
        return;
    }
    let center = bounds.center();
    if let Some(path) = circle_stroke(center, gauge.radius, gauge.stroke) {
        window.paint_path(path, track);
    }
    let Some(sweep) = arc_sweep(gauge.value) else {
        return;
    };
    if sweep.closed {
        if let Some(path) = circle_stroke(center, gauge.radius, gauge.stroke) {
            window.paint_path(path, gauge.color);
        }
        return;
    }
    let start = on_circle(center, gauge.radius, sweep.start_radians);
    let end = on_circle(center, gauge.radius, sweep.end_radians);
    let radii = point(px(gauge.radius), px(gauge.radius));
    let mut arc = PathBuilder::stroke(px(gauge.stroke));
    arc.move_to(start);
    arc.arc_to(
        radii,
        px(0.),
        sweep.end_radians - sweep.start_radians > PI,
        true,
        end,
    );
    if let Ok(path) = arc.build() {
        window.paint_path(path, gauge.color);
    }
    for cap in [start, end] {
        if let Some(path) = disc(cap, gauge.stroke / 2.) {
            window.paint_path(path, gauge.color);
        }
    }
}

fn on_circle(center: Point<Pixels>, radius: f32, radians: f32) -> Point<Pixels> {
    point(
        center.x + px(radius * radians.sin()),
        center.y - px(radius * radians.cos()),
    )
}

fn circle_stroke(center: Point<Pixels>, radius: f32, stroke: f32) -> Option<gpui::Path<Pixels>> {
    let mut builder = PathBuilder::stroke(px(stroke));
    trace_circle(&mut builder, center, radius);
    builder.build().ok()
}

fn disc(center: Point<Pixels>, radius: f32) -> Option<gpui::Path<Pixels>> {
    let mut builder = PathBuilder::fill();
    trace_circle(&mut builder, center, radius);
    builder.build().ok()
}

fn trace_circle(builder: &mut PathBuilder, center: Point<Pixels>, radius: f32) {
    let radii = point(px(radius), px(radius));
    let top = on_circle(center, radius, 0.);
    let bottom = on_circle(center, radius, PI);
    builder.move_to(top);
    builder.arc_to(radii, px(0.), false, true, bottom);
    builder.arc_to(radii, px(0.), false, true, top);
    builder.close();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_point(radians: f32) -> (f32, f32) {
        let placed = on_circle(point(px(0.), px(0.)), 1., radians);
        (f32::from(placed.x), f32::from(placed.y))
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn test_arc_starts_at_noon_and_runs_clockwise() {
        for (value, end_degrees) in [(0.25, 90.), (0.54, 194.4), (1.0, 360.)] {
            let sweep = arc_sweep(value).expect("a positive value draws an arc");
            assert!(close(sweep.start_radians, 0.));
            assert!(sweep.clockwise);
            assert!(
                close(sweep.end_radians.to_degrees(), end_degrees),
                "{value} ends at {} degrees",
                sweep.end_radians.to_degrees()
            );
        }
        let (x, y) = unit_point(0.);
        assert!(close(x, 0.) && close(y, -1.), "noon is straight up");
        let (x, y) = unit_point(arc_sweep(0.25).unwrap().end_radians);
        assert!(
            close(x, 1.) && close(y, 0.),
            "a quarter ends at three o'clock"
        );
        let (x, y) = unit_point(arc_sweep(0.54).unwrap().end_radians);
        assert!(x < 0. && y > 0., "54% ends past six o'clock, lower left");
    }

    #[test]
    fn test_zero_shows_only_the_track_and_one_closes_the_circle() {
        assert_eq!(arc_sweep(0.), None);
        assert!(!arc_sweep(0.54).unwrap().closed);
        assert!(arc_sweep(1.).unwrap().closed);
    }

    #[test]
    fn test_values_outside_the_unit_range_are_bounded() {
        assert_eq!(arc_sweep(-0.3), None);
        assert_eq!(arc_sweep(f64::NAN), None);
        assert_eq!(arc_sweep(1.7), arc_sweep(1.));
    }

    #[test]
    fn test_a_tiny_ring_or_a_stroke_wider_than_the_radius_draws_nothing() {
        assert!(!drawable(7.5, 1.));
        assert!(!drawable(16., 8.));
        assert!(!drawable(16., 9.));
        for size in [RingSize::Small, RingSize::Medium, RingSize::Large] {
            assert!(drawable(size.diameter(), size.stroke()));
        }
    }

    #[test]
    fn test_the_ring_sizes_match_the_design_contract() {
        assert_eq!(
            [RingSize::Small, RingSize::Medium, RingSize::Large]
                .map(|size| (size.diameter(), size.stroke())),
            [(16., 2.5), (44., 4.5), (76., 7.)]
        );
    }

    #[test]
    fn test_the_concentric_variant_builds_for_every_size() {
        let ui = crate::theme::ui_colors();
        for size in [RingSize::Small, RingSize::Medium, RingSize::Large] {
            let _ = concentric_rings(
                size,
                (0.54, UsageLevel::Normal),
                (1.4, UsageLevel::Critical),
                ui,
            );
            let _ = ring(size, 0.9, UsageLevel::Warn, ui);
        }
    }
}
