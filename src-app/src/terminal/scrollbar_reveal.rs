use std::time::{Duration, Instant};

pub(crate) const SCROLLBAR_HOLD: Duration = Duration::from_millis(1000);
pub(crate) const SCROLLBAR_FADE: Duration = Duration::from_millis(200);
pub(crate) const SCROLLBAR_EXPAND: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ScrollbarPresence {
    pub(crate) alpha: f32,
    pub(crate) expansion: f32,
    pub(crate) hide_after: Option<Duration>,
    pub(crate) animating: bool,
}

impl ScrollbarPresence {
    pub(crate) const HIDDEN: Self = Self {
        alpha: 0.0,
        expansion: 0.0,
        hide_after: None,
        animating: false,
    };

    pub(crate) fn is_visible(&self) -> bool {
        self.alpha > 0.0
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ScrollbarReveal {
    last_activity: Option<Instant>,
    hovered: bool,
    expansion_changed_at: Option<Instant>,
    expansion_from: f32,
    dragging: bool,
}

impl ScrollbarReveal {
    pub(crate) fn touch(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    pub(crate) fn set_hovered(&mut self, hovered: bool, now: Instant) -> bool {
        if self.hovered == hovered {
            return false;
        }
        self.retarget(hovered, self.dragging, now);
        true
    }

    pub(crate) fn set_dragging(&mut self, dragging: bool, now: Instant) {
        self.retarget(self.hovered, dragging, now);
    }

    pub(crate) fn is_pinned(&self) -> bool {
        self.hovered || self.dragging
    }

    fn retarget(&mut self, hovered: bool, dragging: bool, now: Instant) {
        if self.is_pinned() != (hovered || dragging) {
            self.expansion_from = self.expansion(now, false).0;
            self.expansion_changed_at = Some(now);
        }
        self.hovered = hovered;
        self.dragging = dragging;
        self.last_activity = Some(now);
    }

    pub(crate) fn presence(&self, now: Instant, reduce_motion: bool) -> ScrollbarPresence {
        let (expansion, expanding) = self.expansion(now, reduce_motion);
        if self.is_pinned() {
            return ScrollbarPresence {
                alpha: 1.0,
                expansion,
                hide_after: None,
                animating: expanding,
            };
        }
        let Some(last_activity) = self.last_activity else {
            return ScrollbarPresence::HIDDEN;
        };
        let elapsed = now.saturating_duration_since(last_activity);
        let (alpha, hide_after, fading) = if elapsed < SCROLLBAR_HOLD {
            (1.0, Some(SCROLLBAR_HOLD - elapsed), false)
        } else if reduce_motion {
            (0.0, None, false)
        } else if elapsed < SCROLLBAR_HOLD + SCROLLBAR_FADE {
            let progress = (elapsed - SCROLLBAR_HOLD).as_secs_f32() / SCROLLBAR_FADE.as_secs_f32();
            (1.0 - ease_out_quint(progress), None, true)
        } else {
            (0.0, None, false)
        };
        ScrollbarPresence {
            alpha,
            expansion,
            hide_after,
            animating: expanding || fading,
        }
    }

    fn expansion(&self, now: Instant, reduce_motion: bool) -> (f32, bool) {
        let target = if self.is_pinned() { 1.0 } else { 0.0 };
        let Some(changed_at) = self.expansion_changed_at else {
            return (target, false);
        };
        if reduce_motion {
            return (target, false);
        }
        let elapsed = now.saturating_duration_since(changed_at);
        let duration = SCROLLBAR_EXPAND.mul_f32((target - self.expansion_from).abs());
        if elapsed >= duration {
            return (target, false);
        }
        let progress = ease_out_quint(elapsed.as_secs_f32() / duration.as_secs_f32());
        (
            self.expansion_from + (target - self.expansion_from) * progress,
            true,
        )
    }
}

fn ease_out_quint(delta: f32) -> f32 {
    1.0 - (1.0 - delta.clamp(0.0, 1.0)).powi(5)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn an_untouched_scrollbar_stays_hidden() {
        let reveal = ScrollbarReveal::default();
        assert_eq!(
            reveal.presence(Instant::now(), false),
            ScrollbarPresence::HIDDEN
        );
    }

    #[test]
    fn activity_shows_the_scrollbar_for_the_hold_then_fades_it_out() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.touch(base);

        let held = reveal.presence(at(base, 500), false);
        assert_eq!(held.alpha, 1.0);
        assert_eq!(held.hide_after, Some(Duration::from_millis(500)));
        assert!(!held.animating);

        let fading = reveal.presence(at(base, 1100), false);
        assert!(fading.alpha > 0.0 && fading.alpha < 1.0, "{fading:?}");
        assert!(fading.animating);
        assert_eq!(fading.hide_after, None);

        let gone = reveal.presence(at(base, 1300), false);
        assert_eq!(gone.alpha, 0.0);
        assert_eq!(gone.hide_after, None);
        assert!(!gone.animating);
    }

    #[test]
    fn reduce_motion_hides_without_a_fade() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.touch(base);
        let after_hold = reveal.presence(at(base, 1050), true);
        assert_eq!(after_hold.alpha, 0.0);
        assert_eq!(after_hold.hide_after, None);
        assert!(!after_hold.animating);
    }

    #[test]
    fn hover_pins_the_scrollbar_and_expands_it_over_time() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        assert!(reveal.set_hovered(true, base));
        assert!(!reveal.set_hovered(true, base));

        let mid = reveal.presence(at(base, 60), false);
        assert_eq!(mid.alpha, 1.0);
        assert!(mid.expansion > 0.0 && mid.expansion < 1.0, "{mid:?}");
        assert!(mid.animating);

        let settled = reveal.presence(at(base, 5000), false);
        assert_eq!(settled.expansion, 1.0);
        assert_eq!(settled.alpha, 1.0);
        assert_eq!(settled.hide_after, None);
        assert!(!settled.animating);

        assert!(reveal.set_hovered(false, at(base, 5000)));
        let collapsing = reveal.presence(at(base, 5060), false);
        assert!(collapsing.expansion > 0.0 && collapsing.expansion < 1.0);
        assert_eq!(collapsing.alpha, 1.0);
        let released = reveal.presence(at(base, 7000), false);
        assert_eq!(released, ScrollbarPresence::HIDDEN);
    }

    #[test]
    fn dragging_keeps_the_scrollbar_visible_until_released() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.set_dragging(true, base);
        assert_eq!(reveal.presence(at(base, 9000), true).alpha, 1.0);
        reveal.set_dragging(false, at(base, 9000));
        assert_eq!(reveal.presence(at(base, 9500), true).alpha, 1.0);
        assert_eq!(reveal.presence(at(base, 10_500), true).alpha, 0.0);
    }

    #[test]
    fn reduce_motion_snaps_the_expansion() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.set_hovered(true, base);
        let presence = reveal.presence(at(base, 1), true);
        assert_eq!(presence.expansion, 1.0);
        assert_eq!(presence.hide_after, None);
        assert!(!presence.animating);
    }

    #[test]
    fn reversing_hover_preserves_the_current_width() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.set_hovered(true, base);
        let before_exit = reveal.presence(at(base, 10), false).expansion;
        reveal.set_hovered(false, at(base, 10));
        assert_eq!(reveal.presence(at(base, 10), false).expansion, before_exit);
        let before_entry = reveal.presence(at(base, 20), false).expansion;
        reveal.set_hovered(true, at(base, 20));
        assert_eq!(reveal.presence(at(base, 20), false).expansion, before_entry);
    }

    #[test]
    fn leaving_the_gutter_during_drag_does_not_restart_expansion() {
        let base = Instant::now();
        let mut reveal = ScrollbarReveal::default();
        reveal.set_hovered(true, base);
        reveal.set_dragging(true, at(base, 200));
        reveal.set_hovered(false, at(base, 300));
        assert_eq!(reveal.presence(at(base, 300), false).expansion, 1.0);
        reveal.set_dragging(false, at(base, 400));
        assert_eq!(reveal.presence(at(base, 400), false).expansion, 1.0);
        assert_eq!(reveal.presence(at(base, 600), false).expansion, 0.0);
    }
}
