use std::time::Duration;

use gpui::{Entity, Global};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_visible() {
        assert!(BlinkPhase::default().visible);
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
