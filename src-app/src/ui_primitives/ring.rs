use std::f32::consts::{PI, TAU};

use gpui::{
    AnyElement, Bounds, Hsla, IntoElement, ParentElement, PathBuilder, Pixels, Point,
    StrokeOptions, Styled, Window, canvas, div, point, px,
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
            reason = "US-010 ships the 44 px size; the capsule and the Usage tiles draw 16 and 76 px"
        )
    )]
    Medium,
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
    let value = if value.is_finite() {
        value.clamp(0., 1.)
    } else {
        0.
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

#[derive(Clone, Copy, Debug, PartialEq)]
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

#[derive(Clone, Copy, Debug, PartialEq)]
enum GaugeLayer {
    Track,
    Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum GaugeShape {
    Circle,
    Arc(ArcSweep),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GaugeStroke {
    layer: GaugeLayer,
    shape: GaugeShape,
    options: StrokeOptions,
}

fn gauge_strokes(gauge: Gauge) -> Vec<GaugeStroke> {
    let geometry_valid = gauge.radius.is_finite() && gauge.stroke.is_finite() && gauge.stroke > 0.;
    if !geometry_valid || gauge.radius <= gauge.stroke / 2. {
        return Vec::new();
    }
    let circle = StrokeOptions {
        line_width: gauge.stroke,
        ..StrokeOptions::default()
    };
    let mut strokes = vec![GaugeStroke {
        layer: GaugeLayer::Track,
        shape: GaugeShape::Circle,
        options: circle,
    }];
    if let Some(sweep) = arc_sweep(gauge.value) {
        strokes.push(if sweep.closed {
            GaugeStroke {
                layer: GaugeLayer::Value,
                shape: GaugeShape::Circle,
                options: circle,
            }
        } else {
            GaugeStroke {
                layer: GaugeLayer::Value,
                shape: GaugeShape::Arc(sweep),
                options: super::stroke::round_stroke_options(px(gauge.stroke)),
            }
        });
    }
    strokes
}

#[cfg(test)]
thread_local! {
    static PAINTED_STROKES: std::cell::RefCell<Vec<GaugeStroke>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn paint_gauge(bounds: Bounds<Pixels>, gauge: Gauge, track: Hsla, window: &mut Window) {
    let center = bounds.center();
    for stroke in gauge_strokes(gauge) {
        let color = match stroke.layer {
            GaugeLayer::Track => track,
            GaugeLayer::Value => gauge.color,
        };
        if let Some(path) = stroke_path(center, gauge.radius, stroke) {
            #[cfg(test)]
            PAINTED_STROKES.with(|painted| painted.borrow_mut().push(stroke));
            window.paint_path(path, color);
        }
    }
}

fn stroke_path(center: Point<Pixels>, radius: f32, stroke: GaugeStroke) -> Option<gpui::Path> {
    let mut builder = super::stroke::stroke_with(stroke.options);
    match stroke.shape {
        GaugeShape::Circle => trace_circle(&mut builder, center, radius),
        GaugeShape::Arc(sweep) => {
            builder.move_to(on_circle(center, radius, sweep.start_radians));
            builder.arc_to(
                point(px(radius), px(radius)),
                px(0.),
                sweep.end_radians - sweep.start_radians > PI,
                sweep.clockwise,
                on_circle(center, radius, sweep.end_radians),
            );
        }
    }
    builder.build().ok()
}

fn on_circle(center: Point<Pixels>, radius: f32, radians: f32) -> Point<Pixels> {
    point(
        center.x + px(radius * radians.sin()),
        center.y - px(radius * radians.cos()),
    )
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
        assert_eq!(arc_sweep(f64::INFINITY), None);
        assert_eq!(arc_sweep(f64::NEG_INFINITY), None);
        assert_eq!(arc_sweep(1.7), arc_sweep(1.));
    }

    fn gauge(radius: f32, stroke: f32, value: f64) -> Gauge {
        Gauge {
            radius,
            stroke,
            value,
            color: gpui::white(),
        }
    }

    #[test]
    fn test_an_open_gauge_is_a_track_and_one_round_capped_value_stroke() {
        let strokes = gauge_strokes(gauge(6.75, 2.5, 0.54));
        assert_eq!(strokes.len(), 2, "track and value, no cap discs");
        assert_eq!(strokes[0].layer, GaugeLayer::Track);
        assert_eq!(strokes[0].shape, GaugeShape::Circle);
        assert_eq!(strokes[1].layer, GaugeLayer::Value);
        assert_eq!(strokes[1].shape, GaugeShape::Arc(arc_sweep(0.54).unwrap()));
        assert_eq!(strokes[1].options.line_cap, gpui::LineCap::Round);
        assert_eq!(strokes[1].options.line_width, 2.5);
    }

    #[test]
    fn test_a_full_gauge_strokes_a_closed_circle() {
        let strokes = gauge_strokes(gauge(6.75, 2.5, 1.));
        assert_eq!(
            strokes
                .iter()
                .map(|s| (s.layer, s.shape))
                .collect::<Vec<_>>(),
            vec![
                (GaugeLayer::Track, GaugeShape::Circle),
                (GaugeLayer::Value, GaugeShape::Circle)
            ]
        );
    }

    #[test]
    fn test_a_radius_at_or_below_half_the_stroke_paints_nothing() {
        for (radius, stroke) in [(1.25, 2.5), (1., 2.5), (0., 2.5), (-3., 2.5)] {
            assert!(gauge_strokes(gauge(radius, stroke, 0.5)).is_empty());
        }
        assert!(gauge_strokes(gauge(f32::NAN, 2.5, 0.5)).is_empty());
        assert!(gauge_strokes(gauge(6., f32::INFINITY, 0.5)).is_empty());
    }

    #[test]
    fn test_a_zero_or_non_finite_value_paints_only_the_track() {
        for value in [0., -0.2, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let strokes = gauge_strokes(gauge(6.75, 2.5, value));
            assert_eq!(
                strokes.iter().map(|s| s.layer).collect::<Vec<_>>(),
                vec![GaugeLayer::Track],
                "{value}"
            );
        }
    }

    struct RingHarness {
        value: f64,
    }

    impl gpui::Render for RingHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors();
            div()
                .flex()
                .child(ring(RingSize::Small, self.value, UsageLevel::Normal, ui))
                .child(concentric_rings(
                    RingSize::Large,
                    (self.value, UsageLevel::Warn),
                    (self.value, UsageLevel::Critical),
                    ui,
                ))
        }
    }

    fn painted_layers(cx: &mut gpui::TestAppContext, value: f64) -> Vec<(GaugeLayer, GaugeShape)> {
        let (_view, cx) = cx.add_window_view(|_, _| RingHarness { value });
        PAINTED_STROKES.with(|painted| painted.borrow_mut().clear());
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        PAINTED_STROKES.with(|painted| {
            painted
                .borrow()
                .iter()
                .map(|stroke| (stroke.layer, stroke.shape))
                .collect()
        })
    }

    #[gpui::test]
    fn test_rendered_rings_paint_at_most_two_paths_per_gauge(cx: &mut gpui::TestAppContext) {
        let open = arc_sweep(0.54).unwrap();
        let track = (GaugeLayer::Track, GaugeShape::Circle);
        let value = (GaugeLayer::Value, GaugeShape::Arc(open));
        assert_eq!(
            painted_layers(cx, 0.54),
            vec![track, value, track, value, track, value]
        );
        let full = (GaugeLayer::Value, GaugeShape::Circle);
        assert_eq!(
            painted_layers(cx, 1.),
            vec![track, full, track, full, track, full]
        );
        for value in [0., f64::NAN, f64::INFINITY] {
            assert_eq!(
                painted_layers(cx, value),
                vec![track, track, track],
                "{value}"
            );
        }
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
