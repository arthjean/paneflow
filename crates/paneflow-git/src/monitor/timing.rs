use std::time::{Duration, Instant, SystemTime};

pub const VERIFY_EVERY: Duration = Duration::from_secs(5 * 60);

pub const VERIFY_AFTER_PROBES: usize = 500;

pub const DRAIN_GAP: Duration = Duration::from_secs(60);

pub const RECOVERY_FIRST: Duration = Duration::from_secs(1);

pub const RECOVERY_MAX: Duration = Duration::from_secs(5 * 60);

pub const REBASELINE_SPACING: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub verify_every: Duration,
    pub verify_after_probes: usize,
    pub drain_gap: Duration,
    pub recovery_first: Duration,
    pub recovery_max: Duration,
    pub rebaseline_spacing: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            verify_every: VERIFY_EVERY,
            verify_after_probes: VERIFY_AFTER_PROBES,
            drain_gap: DRAIN_GAP,
            recovery_first: RECOVERY_FIRST,
            recovery_max: RECOVERY_MAX,
            rebaseline_spacing: REBASELINE_SPACING,
        }
    }
}

impl Timing {
    pub fn next_recovery(&self, previous: Option<Duration>) -> Duration {
        previous
            .map_or(self.recovery_first, |delay| delay.saturating_mul(2))
            .clamp(
                self.recovery_first,
                self.recovery_max.max(self.recovery_first),
            )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Shown,
    Hidden,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SinceFull {
    pub(crate) at: Instant,
    pub(crate) probes: usize,
}

impl SinceFull {
    pub(crate) fn now() -> Self {
        Self {
            at: Instant::now(),
            probes: 0,
        }
    }

    pub(crate) fn verification_due(&self, timing: &Timing) -> bool {
        self.probes >= timing.verify_after_probes || self.at.elapsed() >= timing.verify_every
    }
}

pub(crate) fn drain_gap_exceeded(last: Option<SystemTime>, now: SystemTime, gap: Duration) -> bool {
    last.is_some_and(|last| {
        now.duration_since(last)
            .map_or(true, |elapsed| elapsed > gap)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_doubles_from_one_second_and_stops_at_five_minutes() {
        let timing = Timing::default();
        let mut delays = Vec::new();
        let mut previous = None;
        for _ in 0..12 {
            let delay = timing.next_recovery(previous);
            delays.push(delay.as_secs());
            previous = Some(delay);
        }
        assert_eq!(delays, [1, 2, 4, 8, 16, 32, 64, 128, 256, 300, 300, 300]);
    }

    #[test]
    fn a_drain_gap_counts_only_past_the_limit_and_a_clock_stepped_back_counts_as_one() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let gap = DRAIN_GAP;
        assert!(!drain_gap_exceeded(None, start, gap));
        assert!(!drain_gap_exceeded(Some(start), start + gap, gap));
        assert!(drain_gap_exceeded(
            Some(start),
            start + gap + Duration::from_millis(1),
            gap
        ));
        assert!(drain_gap_exceeded(
            Some(start),
            start - Duration::from_secs(1),
            gap
        ));
    }

    #[test]
    fn verification_is_due_after_five_hundred_probes_or_five_minutes() {
        let timing = Timing::default();
        let fresh = SinceFull::now();
        assert!(!fresh.verification_due(&timing));
        assert!(SinceFull {
            probes: VERIFY_AFTER_PROBES,
            ..fresh
        }
        .verification_due(&timing));
        let old = Instant::now().checked_sub(VERIFY_EVERY);
        if let Some(at) = old {
            assert!(SinceFull { at, probes: 0 }.verification_due(&timing));
        }
    }
}
