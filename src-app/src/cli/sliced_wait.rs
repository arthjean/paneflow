use std::time::{Duration, Instant};

pub(super) trait Clock {
    fn elapsed(&self) -> Duration;
}

pub(super) struct SystemClock(Instant);

impl SystemClock {
    pub(super) fn start() -> Self {
        Self(Instant::now())
    }
}

impl Clock for SystemClock {
    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

pub(super) struct Budget<'c> {
    clock: &'c dyn Clock,
    limit: Option<Duration>,
    cap_ms: u64,
}

impl<'c> Budget<'c> {
    pub(super) fn new(clock: &'c dyn Clock, limit: Option<Duration>, cap_ms: u64) -> Self {
        Self {
            clock,
            limit,
            cap_ms,
        }
    }

    fn remaining(&self) -> Option<Duration> {
        self.limit
            .map(|limit| limit.saturating_sub(self.clock.elapsed()))
    }

    pub(super) fn slice_ms(&self) -> u64 {
        match self.remaining() {
            None => self.cap_ms,
            Some(left) => {
                let millis = left.as_micros().div_ceil(1000);
                u64::try_from(millis).unwrap_or(u64::MAX).min(self.cap_ms)
            }
        }
    }

    pub(super) fn spent(&self) -> bool {
        self.remaining().is_some_and(|left| left.is_zero())
    }
}

#[cfg(test)]
pub(super) mod fake {
    use std::cell::Cell;
    use std::time::Duration;

    #[derive(Default)]
    pub(in crate::cli) struct FakeClock(Cell<Duration>);

    impl FakeClock {
        pub(in crate::cli) fn advance_ms(&self, millis: u64) {
            self.0.set(self.0.get() + Duration::from_millis(millis));
        }

        pub(in crate::cli) fn now_ms(&self) -> u64 {
            u64::try_from(self.0.get().as_millis()).unwrap_or(u64::MAX)
        }
    }

    impl super::Clock for FakeClock {
        fn elapsed(&self) -> Duration {
            self.0.get()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeClock;
    use super::*;

    #[test]
    fn a_bounded_budget_slices_by_the_cap_and_ends_once_spent() {
        let clock = FakeClock::default();
        let budget = Budget::new(&clock, Some(Duration::from_secs(60)), 25_000);
        assert_eq!(budget.slice_ms(), 25_000);
        clock.advance_ms(50_000);
        assert_eq!(budget.slice_ms(), 10_000);
        assert!(!budget.spent());
        clock.advance_ms(10_000);
        assert_eq!(budget.slice_ms(), 0);
        assert!(budget.spent());
    }

    #[test]
    fn an_unbounded_budget_always_takes_the_cap_and_is_never_spent() {
        let clock = FakeClock::default();
        let budget = Budget::new(&clock, None, 25_000);
        clock.advance_ms(3_600_000);
        assert_eq!(budget.slice_ms(), 25_000);
        assert!(!budget.spent());
    }
}
