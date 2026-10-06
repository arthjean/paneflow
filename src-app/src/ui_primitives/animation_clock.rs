use std::time::{Duration, Instant};

pub(crate) const ANIMATION_STEP_MS: u64 = 90;

const TICK_LANDING: Duration = Duration::from_millis(1);

pub(crate) fn animation_epoch() -> Instant {
    static EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

pub(crate) fn until_next_tick(elapsed: Duration, period: Duration) -> Duration {
    let period_ns = period.as_nanos().max(1);
    let into_period = elapsed.as_nanos() % period_ns;
    Duration::from_nanos(u64::try_from(period_ns - into_period).unwrap_or(u64::MAX)) + TICK_LANDING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tick_lands_one_millisecond_past_the_next_period_boundary() {
        let step = Duration::from_millis(ANIMATION_STEP_MS);
        assert_eq!(
            until_next_tick(Duration::ZERO, step),
            Duration::from_millis(91)
        );
        assert_eq!(
            until_next_tick(Duration::from_millis(89), step),
            Duration::from_millis(2)
        );
        assert_eq!(
            until_next_tick(Duration::from_micros(181_500), step),
            Duration::from_micros(89_500)
        );
    }
}
