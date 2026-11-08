use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

pub const ENTRIES_PER_THREAD: usize = 10_000;

#[cfg(windows)]
pub const PLATFORM_CAP: usize = 2;
#[cfg(not(windows))]
pub const PLATFORM_CAP: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Threads {
    #[default]
    Scheduled,
    Fixed(usize),
}

impl Threads {
    pub(crate) fn before_index(self, share: usize) -> usize {
        match self {
            Self::Scheduled => share.clamp(1, PLATFORM_CAP),
            Self::Fixed(threads) => threads.max(1),
        }
    }

    pub(crate) fn for_entries(self, share: usize, entries: usize) -> usize {
        match self {
            Self::Scheduled => scheduled(entries, share, PLATFORM_CAP),
            Self::Fixed(threads) => threads.max(1),
        }
    }
}

pub(crate) fn scheduled(entries: usize, share: usize, cap: usize) -> usize {
    entries
        .div_ceil(ENTRIES_PER_THREAD)
        .clamp(1, cap.max(1))
        .min(share.max(1))
}

pub(crate) struct Probes {
    running: AtomicUsize,
}

pub(crate) struct Lease<'a> {
    probes: &'a Probes,
    share: usize,
}

impl Probes {
    pub(crate) const fn new() -> Self {
        Self {
            running: AtomicUsize::new(0),
        }
    }

    pub(crate) fn enter(&self, budget: usize) -> Lease<'_> {
        let running = self.running.fetch_add(1, Ordering::AcqRel) + 1;
        Lease {
            probes: self,
            share: (budget / running).max(1),
        }
    }
}

impl Lease<'_> {
    pub(crate) fn share(&self) -> usize {
        self.share
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.probes.running.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) static PROBES: Probes = Probes::new();

pub(crate) fn budget() -> usize {
    static BUDGET: OnceLock<usize> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        (std::thread::available_parallelism().map_or(1, NonZeroUsize::get) / 2).max(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_repository_gets_one_thread_whatever_the_share() {
        assert_eq!(scheduled(1_675, 8, 3), 1);
        assert_eq!(scheduled(0, 8, 3), 1);
    }

    #[test]
    fn a_large_repository_alone_gets_the_platform_cap() {
        assert_eq!(scheduled(64_264, 8, 3), 3);
        assert_eq!(scheduled(64_264, 8, 2), 2);
    }

    #[test]
    fn a_mid_sized_repository_gets_one_thread_per_slice_of_entries() {
        assert_eq!(scheduled(15_000, 8, 3), 2);
        assert_eq!(scheduled(29_069, 8, 4), 3);
    }

    #[test]
    fn concurrent_probes_split_the_budget_down_to_one_thread() {
        let probes = Probes::new();
        let first = probes.enter(8);
        let second = probes.enter(8);
        let third = probes.enter(8);
        let fourth = probes.enter(8);
        assert_eq!(
            [first.share(), second.share(), third.share(), fourth.share()],
            [8, 4, 2, 2]
        );
        assert_eq!(scheduled(64_264, fourth.share(), 3), 2);
        let crowd: Vec<_> = (0..12).map(|_| probes.enter(8)).collect();
        assert_eq!(crowd.last().map(Lease::share), Some(1));
        assert_eq!(scheduled(64_264, 1, 3), 1);
    }

    #[test]
    fn a_finished_probe_returns_its_place_to_the_next_one() {
        let probes = Probes::new();
        let first = probes.enter(8);
        drop(probes.enter(8));
        assert_eq!(probes.enter(8).share(), 4);
        drop(first);
        assert_eq!(probes.enter(8).share(), 8);
    }

    #[test]
    fn a_fixed_thread_count_ignores_the_schedule() {
        assert_eq!(Threads::Fixed(6).for_entries(1, 10), 6);
        assert_eq!(Threads::Fixed(0).before_index(8), 1);
        assert_eq!(Threads::Scheduled.before_index(8), PLATFORM_CAP);
        assert_eq!(Threads::Scheduled.for_entries(8, 1_000), 1);
    }
}
