use std::time::{Duration, Instant};

use gpui::{
    Bounds, BoxShadow, ContentMask, Corners, Hsla, Pixels, Window, fill, linear_color_stop,
    linear_gradient, point, px, size,
};

const BAR_HEIGHT: f32 = 1.25;
const SHINE_HEIGHT: f32 = 0.5;
const GLOW_HEIGHT: f32 = 4.;
const GLOW_SIGMA: f32 = 4.;
const EDGE_GLOW_RADIUS: f32 = 2.5;
const GLOW_HEAD_WIDTH: f32 = 280.;
const EXIT_FEATHER_WIDTH: f32 = 140.;
const GLOW_EXIT_OUTSET_IN_SIGMAS: f32 = 3.;
const GLOW_SEGMENTS: usize = 10;

const SHINE_WHITE_MIX: f32 = 0.36;
const LINE_OPACITY: f32 = 0.64;
const LINE_EXIT_OPACITY: f32 = 0.45;
const GLOW_OPACITY: (f32, f32) = (0.50, 0.88);
const HEAD_OPACITY: (f32, f32) = (0.48, 0.84);
const GLOW_STOPS: [(f32, f32); 4] = [(0., 0.), (0.45, 0.18), (0.82, 0.68), (1., 1.)];
const HEAD_STOPS: [(f32, f32); 4] = [(0., 0.), (0.45, 0.16), (0.82, 0.68), (1., 1.)];

const MINIMUM_VISIBLE_PROGRESS: f32 = 0.05;
const EXIT_START: f32 = 0.90;
const RESTING_PULSE: f32 = 0.5;
const PROGRESS_DURATION: Duration = Duration::from_millis(180);
const PULSE_DURATION: Duration = Duration::from_millis(700);
const FADE_DURATION: Duration = Duration::from_millis(320);

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LoaderFrame {
    pub(crate) progress: f32,
    pub(crate) fade: f32,
    pub(crate) pulse: f32,
    pub(crate) exit: f32,
}

#[derive(Debug, Default)]
pub(crate) struct Loader {
    active: bool,
    finishing: bool,
    from: f32,
    to: f32,
    moved_at: Option<Instant>,
    began_at: Option<Instant>,
    fade_at: Option<Instant>,
}

impl Loader {
    pub(crate) fn begin(&mut self, now: Instant, progress: f32) {
        *self = Self {
            active: true,
            began_at: Some(now),
            ..Self::default()
        };
        let progress = if progress.is_finite() {
            progress
        } else {
            MINIMUM_VISIBLE_PROGRESS
        };
        self.move_to(now, progress.max(MINIMUM_VISIBLE_PROGRESS));
    }

    pub(crate) fn advance(&mut self, now: Instant, progress: f32) {
        if self.active && !self.finishing {
            self.move_to(now, progress);
        }
    }

    pub(crate) fn finish(&mut self, now: Instant) {
        if !self.active || self.finishing {
            return;
        }
        self.finishing = true;
        self.move_to(now, 1.);
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    fn move_to(&mut self, now: Instant, progress: f32) {
        let progress = progress.clamp(0., 1.);
        if progress <= self.to {
            return;
        }
        self.from = self.displayed(now, false);
        self.to = progress;
        self.moved_at = Some(now);
    }

    fn displayed(&self, now: Instant, reduced: bool) -> f32 {
        let Some(moved_at) = self.moved_at else {
            return self.to;
        };
        if reduced {
            return self.to;
        }
        let t = fraction(now.saturating_duration_since(moved_at), PROGRESS_DURATION);
        self.from + (self.to - self.from) * ease_out(t)
    }

    pub(crate) fn frame(&mut self, now: Instant, reduced: bool) -> Option<LoaderFrame> {
        if !self.active {
            return None;
        }
        let progress = self.displayed(now, reduced);
        if self.finishing && progress >= EXIT_START && self.fade_at.is_none() {
            self.fade_at = Some(now);
        }
        let fade = match self.fade_at {
            Some(_) if reduced => 0.,
            Some(at) => 1. - ease_out(fraction(now.saturating_duration_since(at), FADE_DURATION)),
            None => 1.,
        };
        if fade <= 0. {
            self.reset();
            return None;
        }
        let pulse = match self.began_at {
            Some(began) if !reduced && !self.finishing => {
                throb(now.saturating_duration_since(began))
            }
            _ => RESTING_PULSE,
        };
        Some(LoaderFrame {
            progress,
            fade,
            pulse,
            exit: if self.finishing { 1. - fade } else { 0. },
        })
    }
}

fn fraction(elapsed: Duration, total: Duration) -> f32 {
    (elapsed.as_secs_f32() / total.as_secs_f32()).clamp(0., 1.)
}

fn ease_out(t: f32) -> f32 {
    1. - (1. - t).powi(3)
}

fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4. * t * t * t
    } else {
        1. - (-2. * t + 2.).powi(3) / 2.
    }
}

fn throb(elapsed: Duration) -> f32 {
    let half = PULSE_DURATION.as_secs_f32();
    let phase = (elapsed.as_secs_f32() / half + RESTING_PULSE) % 2.;
    let rising = if phase < 1. { phase } else { 2. - phase };
    ease_in_out(rising)
}

fn lerp(range: (f32, f32), t: f32) -> f32 {
    range.0 + (range.1 - range.0) * t
}

fn ramp(stops: &[(f32, f32); 4], at: f32) -> f32 {
    let at = at.clamp(0., 1.);
    for pair in stops.windows(2) {
        let ((p0, v0), (p1, v1)) = (pair[0], pair[1]);
        if at <= p1 {
            let span = (p1 - p0).max(f32::EPSILON);
            return v0 + (v1 - v0) * ((at - p0) / span);
        }
    }
    stops[3].1
}

pub(crate) fn mix(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let a = a.to_rgb();
    let b = b.to_rgb();
    Hsla::from(gpui::Rgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    })
}

fn corner_inset(radius: f32, height: f32) -> f32 {
    let rise = (radius - height).max(0.);
    radius - (radius * radius - rise * rise).max(0.).sqrt()
}

pub(crate) fn paint(
    bounds: Bounds<Pixels>,
    radius: f32,
    frame: LoaderFrame,
    accent: Hsla,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let progress_width = width * frame.progress;
    if progress_width <= 0. {
        return;
    }
    let left = f32::from(bounds.origin.x);
    let bottom = f32::from(bounds.origin.y) + height;
    let shine = mix(accent, gpui::white(), SHINE_WHITE_MIX);
    let edge = corner_inset(radius, BAR_HEIGHT);
    let line_start = edge;
    let line_end = progress_width.min(width - edge);
    let rect = |x0: f32, x1: f32, top: f32, h: f32| Bounds {
        origin: point(px(left + x0), px(top)),
        size: size(px((x1 - x0).max(0.)), px(h)),
    };

    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        let line_opacity = lerp((LINE_EXIT_OPACITY, LINE_OPACITY), frame.fade);
        let feather_start =
            -EXIT_FEATHER_WIDTH + frame.exit * (progress_width + EXIT_FEATHER_WIDTH);
        let feather_end = feather_start + EXIT_FEATHER_WIDTH;
        let line_top = bottom - BAR_HEIGHT;
        let solid_from = feather_end.max(line_start);
        if feather_end > line_start {
            let x0 = feather_start.max(line_start);
            let x1 = feather_end.min(line_end);
            if x1 > x0 {
                let at = |x: f32| ((x - feather_start) / EXIT_FEATHER_WIDTH).clamp(0., 1.);
                window.paint_quad(fill(
                    rect(x0, x1, line_top, BAR_HEIGHT),
                    linear_gradient(
                        90.,
                        linear_color_stop(accent.opacity(line_opacity * at(x0)), 0.),
                        linear_color_stop(accent.opacity(line_opacity * at(x1)), 1.),
                    ),
                ));
            }
        }
        if line_end > solid_from {
            window.paint_quad(fill(
                rect(solid_from, line_end, line_top, BAR_HEIGHT),
                accent.opacity(line_opacity),
            ));
        }

        let glow_head = progress_width
            + frame.exit * (GLOW_HEAD_WIDTH + GLOW_EXIT_OUTSET_IN_SIGMAS * GLOW_SIGMA);
        let glow_origin = glow_head - GLOW_HEAD_WIDTH;
        let glow_start = glow_origin.max(0.);
        let glow_end = glow_head.min(width);
        if glow_end <= glow_start {
            return;
        }
        let glow_opacity = frame.fade * lerp(GLOW_OPACITY, frame.pulse);
        let segment = GLOW_HEAD_WIDTH / GLOW_SEGMENTS as f32;
        let shadows: Vec<(Bounds<Pixels>, BoxShadow)> = (0..GLOW_SEGMENTS)
            .filter_map(|index| {
                let x0 = (glow_origin + segment * index as f32).max(glow_start);
                let x1 = (glow_origin + segment * (index + 1) as f32).min(glow_end);
                (x1 > x0).then(|| {
                    let at = ((x0 + x1) / 2. - glow_origin) / GLOW_HEAD_WIDTH;
                    (
                        rect(x0, x1, bottom - GLOW_HEIGHT, GLOW_HEIGHT),
                        BoxShadow {
                            color: accent.opacity(glow_opacity * ramp(&GLOW_STOPS, at)),
                            offset: point(px(0.), px(0.)),
                            blur_radius: px(GLOW_SIGMA),
                            spread_radius: px(0.),
                            inset: false,
                        },
                    )
                })
            })
            .collect();
        for (bounds, shadow) in shadows {
            window.paint_drop_shadows(bounds, Corners::all(px(GLOW_HEIGHT / 2.)), &[shadow]);
        }
        let head_center = bottom - BAR_HEIGHT / 2.;
        window.paint_drop_shadows(
            Bounds {
                origin: point(
                    px(left + glow_head - EDGE_GLOW_RADIUS),
                    px(head_center - EDGE_GLOW_RADIUS),
                ),
                size: size(px(EDGE_GLOW_RADIUS * 2.), px(EDGE_GLOW_RADIUS * 2.)),
            },
            Corners::all(px(EDGE_GLOW_RADIUS)),
            &[BoxShadow {
                color: shine.opacity(glow_opacity),
                offset: point(px(0.), px(0.)),
                blur_radius: px(GLOW_SIGMA),
                spread_radius: px(0.),
                inset: false,
            }],
        );

        let head_opacity = frame.fade * lerp(HEAD_OPACITY, frame.pulse);
        let head_color = |at: f32| {
            let alpha = head_opacity * ramp(&HEAD_STOPS, at);
            if at >= 1. {
                shine.opacity(alpha)
            } else {
                mix(accent, shine, ((at - 0.82) / 0.18).clamp(0., 1.)).opacity(alpha)
            }
        };
        for pair in HEAD_STOPS.windows(2) {
            let x0 = (glow_origin + pair[0].0 * GLOW_HEAD_WIDTH).max(glow_start.max(line_start));
            let x1 = (glow_origin + pair[1].0 * GLOW_HEAD_WIDTH).min(glow_end.min(width - edge));
            if x1 <= x0 {
                continue;
            }
            let at = |x: f32| (x - glow_origin) / GLOW_HEAD_WIDTH;
            for (top, h) in [
                (bottom - BAR_HEIGHT, BAR_HEIGHT),
                (bottom - BAR_HEIGHT, SHINE_HEIGHT),
            ] {
                window.paint_quad(fill(
                    rect(x0, x1, top, h),
                    linear_gradient(
                        90.,
                        linear_color_stop(head_color(at(x0)), 0.),
                        linear_color_stop(head_color(at(x1)), 1.),
                    ),
                ));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_load_starts_visible_and_eases_to_each_reported_progress() {
        let start = Instant::now();
        let mut loader = Loader::default();
        loader.begin(start, 0.);
        let first = loader.frame(at(start, 400), false).unwrap();
        assert!((first.progress - MINIMUM_VISIBLE_PROGRESS).abs() < 1e-4);
        loader.advance(at(start, 400), 0.6);
        let halfway = loader.frame(at(start, 450), false).unwrap().progress;
        assert!(
            halfway > MINIMUM_VISIBLE_PROGRESS && halfway < 0.6,
            "{halfway}"
        );
        assert!((loader.frame(at(start, 600), false).unwrap().progress - 0.6).abs() < 1e-4);
        loader.advance(at(start, 600), 0.3);
        assert!((loader.frame(at(start, 900), false).unwrap().progress - 0.6).abs() < 1e-4);
    }

    #[test]
    fn a_finished_load_runs_to_the_end_then_fades_out_and_resets() {
        let start = Instant::now();
        let mut loader = Loader::default();
        loader.begin(start, 0.4);
        loader.finish(at(start, 100));
        let filling = loader.frame(at(start, 150), false).unwrap();
        assert_eq!(filling.fade, 1.);
        let exiting = loader.frame(at(start, 400), false).unwrap();
        assert!(exiting.progress >= EXIT_START);
        let fading = loader.frame(at(start, 560), false).unwrap();
        assert!(fading.fade < 1. && fading.exit > 0., "{fading:?}");
        assert!(loader.frame(at(start, 800), false).is_none());
        assert!(!loader.is_active());
    }

    #[test]
    fn with_reduced_motion_the_line_jumps_and_disappears_without_animating() {
        let start = Instant::now();
        let mut loader = Loader::default();
        loader.begin(start, 0.3);
        let frame = loader.frame(start, true).unwrap();
        assert_eq!((frame.progress, frame.pulse), (0.3, RESTING_PULSE));
        loader.finish(start);
        assert!(loader.frame(start, true).is_none());
    }

    #[test]
    fn the_pulse_rests_at_half_and_stays_between_zero_and_one() {
        assert!((throb(Duration::ZERO) - RESTING_PULSE).abs() < 1e-4);
        for ms in (0..3000).step_by(37) {
            let value = throb(Duration::from_millis(ms));
            assert!((0. ..=1.).contains(&value), "{ms}: {value}");
        }
    }

    #[test]
    fn the_glow_ramp_follows_its_stops() {
        assert_eq!(ramp(&GLOW_STOPS, 0.), 0.);
        assert_eq!(ramp(&GLOW_STOPS, 1.), 1.);
        assert!((ramp(&GLOW_STOPS, 0.45) - 0.18).abs() < 1e-5);
        assert!(corner_inset(8., BAR_HEIGHT) > 3. && corner_inset(8., BAR_HEIGHT) < 4.5);
    }
}
