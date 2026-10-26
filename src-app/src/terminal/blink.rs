use std::time::Duration;

use gpui::{App, Entity, Global};

use crate::ui_primitives::animation_clock::{ANIMATION_STEP_MS, animation_epoch, until_next_tick};

const BLINK_STEPS: u64 = 6;

pub const CURSOR_BLINK_INTERVAL: Duration = Duration::from_millis(ANIMATION_STEP_MS * BLINK_STEPS);

pub fn until_next_blink(now: std::time::Instant) -> Duration {
    until_next_tick(
        now.saturating_duration_since(animation_epoch()),
        CURSOR_BLINK_INTERVAL,
    )
}

pub struct BlinkPhase {
    pub visible: bool,
}

impl Default for BlinkPhase {
    fn default() -> Self {
        Self { visible: true }
    }
}

pub struct BlinkPhaseGlobal(pub Entity<BlinkPhase>);

impl Global for BlinkPhaseGlobal {}

pub fn step_blink(any_window_active: bool, cx: &mut App) {
    set_blink_phase(cx, |visible| !any_window_active || !visible);
}

pub fn show_cursor(cx: &mut App) {
    set_blink_phase(cx, |_| true);
}

fn set_blink_phase(cx: &mut App, next: impl FnOnce(bool) -> bool) {
    let Some(global) = cx.try_global::<BlinkPhaseGlobal>() else {
        return;
    };
    let phase = global.0.clone();
    phase.update(cx, |phase, cx| {
        let visible = next(phase.visible);
        if visible != phase.visible {
            phase.visible = visible;
            cx.notify();
        }
    });
}

#[cfg(test)]
mod tests {
    use gpui::AppContext as _;

    use super::*;

    #[test]
    fn default_is_visible() {
        assert!(BlinkPhase::default().visible);
    }

    fn install_phase(cx: &mut gpui::TestAppContext) -> Entity<BlinkPhase> {
        cx.update(|cx| {
            let phase = cx.new(|_| BlinkPhase::default());
            cx.set_global(BlinkPhaseGlobal(phase.clone()));
            phase
        })
    }

    fn observe_notifications(
        phase: &Entity<BlinkPhase>,
        cx: &mut gpui::TestAppContext,
    ) -> std::rc::Rc<std::cell::Cell<u32>> {
        let hits = std::rc::Rc::new(std::cell::Cell::new(0));
        let counter = hits.clone();
        cx.update(|cx| {
            cx.observe(phase, move |_, _| counter.set(counter.get() + 1))
                .detach()
        });
        hits
    }

    #[gpui::test]
    fn an_active_window_toggles_the_caret_on_every_blink(cx: &mut gpui::TestAppContext) {
        let phase = install_phase(cx);
        let visible = |cx: &mut gpui::TestAppContext| phase.read_with(cx, |phase, _| phase.visible);
        cx.update(|cx| step_blink(true, cx));
        assert!(!visible(cx));
        cx.update(|cx| step_blink(true, cx));
        assert!(visible(cx));
    }

    #[gpui::test]
    fn without_an_active_window_the_caret_holds_visible_and_stays_silent(
        cx: &mut gpui::TestAppContext,
    ) {
        let phase = install_phase(cx);
        cx.update(|cx| step_blink(true, cx));
        let hits = observe_notifications(&phase, cx);
        cx.update(|cx| step_blink(false, cx));
        cx.run_until_parked();
        assert!(phase.read_with(cx, |phase, _| phase.visible));
        assert_eq!(hits.get(), 1, "the hidden caret is shown once");
        for _ in 0..56 {
            cx.update(|cx| step_blink(false, cx));
        }
        cx.run_until_parked();
        assert!(phase.read_with(cx, |phase, _| phase.visible));
        assert_eq!(
            hits.get(),
            1,
            "30 s of blinks in an inactive window notify nothing"
        );
    }

    #[gpui::test]
    fn an_activation_change_shows_the_caret_before_the_next_blink_hides_it(
        cx: &mut gpui::TestAppContext,
    ) {
        let phase = install_phase(cx);
        cx.update(|cx| step_blink(true, cx));
        cx.update(show_cursor);
        assert!(phase.read_with(cx, |phase, _| phase.visible));
        cx.update(|cx| step_blink(true, cx));
        assert!(!phase.read_with(cx, |phase, _| phase.visible));
    }

    #[test]
    fn every_blink_lands_on_a_spinner_step_so_both_share_one_frame() {
        let step = Duration::from_millis(ANIMATION_STEP_MS);
        for offset_us in (0..2 * CURSOR_BLINK_INTERVAL.as_micros() as u64).step_by(1_337) {
            let elapsed = Duration::from_micros(offset_us);
            let blink_at = elapsed + until_next_tick(elapsed, CURSOR_BLINK_INTERVAL);
            let spinner_asleep_since = blink_at - Duration::from_millis(2);
            assert_eq!(
                spinner_asleep_since + until_next_tick(spinner_asleep_since, step),
                blink_at,
                "the blink from {elapsed:?} wakes off the spinner grid"
            );
        }
    }
}
