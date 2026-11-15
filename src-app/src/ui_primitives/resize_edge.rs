use std::time::{Duration, Instant};

pub(crate) const RESIZE_EDGE_NUDGE: f32 = 5.;
pub(crate) const RESIZE_EDGE_BOUNCE: Duration = Duration::from_millis(220);
pub(crate) const RESIZE_SETTLE: Duration = Duration::from_millis(120);
const RESIZE_EDGE_OUT_SHARE: f32 = 0.32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeEdge {
    Min,
    Max,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizeDragSample {
    pub(crate) size: f32,
    pub(crate) edge: Option<ResizeEdge>,
    pub(crate) starts_bounce: bool,
}

pub(crate) fn resize_drag_sample(
    requested: f32,
    min: f32,
    max: f32,
    latched_edge: Option<ResizeEdge>,
    animate: bool,
) -> ResizeDragSample {
    let max = max.max(min);
    let edge = if requested <= min {
        Some(ResizeEdge::Min)
    } else if requested >= max {
        Some(ResizeEdge::Max)
    } else {
        None
    };
    ResizeDragSample {
        size: requested.clamp(min, max),
        starts_bounce: animate && edge.is_some() && edge != latched_edge,
        edge,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizeEdgeBounce {
    edge: ResizeEdge,
    started_at: Instant,
}

impl ResizeEdgeBounce {
    pub(crate) fn new(edge: ResizeEdge, now: Instant) -> Self {
        Self {
            edge,
            started_at: now,
        }
    }

    pub(crate) fn offset_at(self, now: Instant) -> Option<f32> {
        let progress = now.saturating_duration_since(self.started_at).as_secs_f32()
            / RESIZE_EDGE_BOUNCE.as_secs_f32();
        (progress < 1.).then(|| resize_bounce_offset(self.edge, progress))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ResizeSettle {
    offset: f32,
    started_at: Instant,
}

impl ResizeSettle {
    pub(crate) fn new(offset: f32, now: Instant) -> Self {
        Self {
            offset,
            started_at: now,
        }
    }

    pub(crate) fn offset_at(self, now: Instant) -> Option<f32> {
        let progress = now.saturating_duration_since(self.started_at).as_secs_f32()
            / RESIZE_SETTLE.as_secs_f32();
        (progress < 1.).then(|| self.offset * (1. - super::ease_out_cubic(progress)))
    }
}

fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0., 1.);
    progress * progress * (3. - 2. * progress)
}

fn resize_bounce_offset(edge: ResizeEdge, progress: f32) -> f32 {
    let progress = progress.clamp(0., 1.);
    let reach = if progress < RESIZE_EDGE_OUT_SHARE {
        smoothstep(progress / RESIZE_EDGE_OUT_SHARE)
    } else {
        1. - smoothstep((progress - RESIZE_EDGE_OUT_SHARE) / (1. - RESIZE_EDGE_OUT_SHARE))
    };
    match edge {
        ResizeEdge::Min => -reach * RESIZE_EDGE_NUDGE,
        ResizeEdge::Max => reach * RESIZE_EDGE_NUDGE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f32 = 240.;
    const MAX: f32 = 480.;

    #[test]
    fn a_held_pointer_starts_one_bounce_per_visit_to_an_edge() {
        let reached = resize_drag_sample(MIN, MIN, MAX, None, true);
        assert_eq!(reached.size, MIN);
        assert_eq!(reached.edge, Some(ResizeEdge::Min));
        assert!(reached.starts_bounce);

        let held = resize_drag_sample(MIN - 80., MIN, MAX, reached.edge, true);
        assert_eq!(held.size, MIN);
        assert!(!held.starts_bounce);

        let inside = resize_drag_sample(MIN + 1., MIN, MAX, held.edge, true);
        assert_eq!(inside.edge, None);
        assert!(!inside.starts_bounce);

        let back = resize_drag_sample(MIN - 1., MIN, MAX, inside.edge, true);
        assert!(back.starts_bounce);

        let far = resize_drag_sample(MAX + 80., MIN, MAX, back.edge, true);
        assert_eq!(far.size, MAX);
        assert_eq!(far.edge, Some(ResizeEdge::Max));
        assert!(far.starts_bounce);
    }

    #[test]
    fn a_sample_never_leaves_the_legal_range() {
        for requested in [f32::MIN, MIN - 100., MIN, 300., MAX, MAX + 100., f32::MAX] {
            let sample = resize_drag_sample(requested, MIN, MAX, None, true);
            assert!((MIN..=MAX).contains(&sample.size), "{requested}");
        }
    }

    #[test]
    fn a_ceiling_below_the_floor_pins_the_size_to_the_floor() {
        let sample = resize_drag_sample(300., MIN, MIN - 40., None, true);
        assert_eq!(sample.size, MIN);
        assert_eq!(sample.edge, Some(ResizeEdge::Max));
    }

    #[test]
    fn a_still_frame_reaches_the_edge_without_a_bounce() {
        let sample = resize_drag_sample(MIN - 10., MIN, MAX, None, false);
        assert_eq!(sample.edge, Some(ResizeEdge::Min));
        assert!(!sample.starts_bounce);
    }

    #[test]
    fn the_bounce_leaves_from_rest_peaks_at_the_nudge_and_comes_home() {
        let started = Instant::now();
        let bounce = ResizeEdgeBounce::new(ResizeEdge::Max, started);
        assert_eq!(bounce.offset_at(started), Some(0.));

        let peak = started + RESIZE_EDGE_BOUNCE.mul_f32(RESIZE_EDGE_OUT_SHARE);
        let at_peak = bounce.offset_at(peak).expect("still bouncing");
        assert!((at_peak - RESIZE_EDGE_NUDGE).abs() < 0.01, "{at_peak}");

        let landing = started + RESIZE_EDGE_BOUNCE.mul_f32(0.99);
        let near_home = bounce.offset_at(landing).expect("still bouncing");
        assert!((0. ..0.01).contains(&near_home), "{near_home}");

        assert_eq!(bounce.offset_at(started + RESIZE_EDGE_BOUNCE), None);
    }

    #[test]
    fn the_bounce_rises_then_falls_without_a_second_swing() {
        let started = Instant::now();
        let bounce = ResizeEdgeBounce::new(ResizeEdge::Max, started);
        let offsets: Vec<f32> = (0..100)
            .map(|step| {
                bounce
                    .offset_at(started + RESIZE_EDGE_BOUNCE.mul_f32(step as f32 / 100.))
                    .expect("still bouncing")
            })
            .collect();
        let peak = offsets
            .iter()
            .position(|offset| *offset == offsets.iter().copied().fold(0., f32::max))
            .expect("a peak");
        assert!(offsets[..=peak].windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(offsets[peak..].windows(2).all(|pair| pair[0] >= pair[1]));
        assert!(
            offsets
                .iter()
                .all(|offset| (0. ..=RESIZE_EDGE_NUDGE).contains(offset))
        );
    }

    #[test]
    fn a_settle_starts_at_the_whole_jump_and_only_shrinks_until_it_is_gone() {
        let started = Instant::now();
        let settle = ResizeSettle::new(-6., started);
        assert_eq!(settle.offset_at(started), Some(-6.));

        let offsets: Vec<f32> = (0..50)
            .map(|step| {
                settle
                    .offset_at(started + RESIZE_SETTLE.mul_f32(step as f32 / 50.))
                    .expect("still settling")
            })
            .collect();
        assert!(offsets.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(offsets.iter().all(|offset| (-6. ..=0.).contains(offset)));
        assert_eq!(settle.offset_at(started + RESIZE_SETTLE), None);
    }

    #[test]
    fn the_floor_pushes_inward_and_the_ceiling_outward() {
        let started = Instant::now();
        let midway = started + RESIZE_EDGE_BOUNCE.mul_f32(0.2);
        let floor = ResizeEdgeBounce::new(ResizeEdge::Min, started).offset_at(midway);
        let ceiling = ResizeEdgeBounce::new(ResizeEdge::Max, started).offset_at(midway);
        assert!(floor.is_some_and(|offset| offset < 0.));
        assert!(ceiling.is_some_and(|offset| offset > 0.));
        assert_eq!(floor.map(|offset| -offset), ceiling);
    }
}
