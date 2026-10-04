use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crate::process::SystemEntries;

pub(crate) const LISTING_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub(crate) struct Listing {
    pub(crate) taken_at: Instant,
    pub(crate) entries: Arc<io::Result<SystemEntries>>,
}

pub(crate) type ListingSlot = Arc<Mutex<Option<Listing>>>;

type Lister = Box<dyn Fn() -> io::Result<SystemEntries> + Send + Sync>;

struct Waiter {
    due: Instant,
    slot: ListingSlot,
    wake: Box<dyn Fn() + Send>,
}

#[derive(Default)]
struct State {
    waiters: Vec<Waiter>,
    latest: Option<Listing>,
    finished_at: Option<Instant>,
    listing: bool,
    serving: bool,
}

pub(crate) struct SharedListing {
    state: Mutex<State>,
    changed: Condvar,
    interval: Duration,
    list: Lister,
}

pub(crate) fn shared() -> &'static Arc<SharedListing> {
    static SHARED: OnceLock<Arc<SharedListing>> = OnceLock::new();
    SHARED.get_or_init(|| {
        SharedListing::new(
            LISTING_INTERVAL,
            Box::new(crate::process::list_system_processes),
        )
    })
}

pub(crate) fn take(slot: &ListingSlot) -> Option<Listing> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).take()
}

impl SharedListing {
    pub(crate) fn new(interval: Duration, list: Lister) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            interval,
            list,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn request(
        self: &Arc<Self>,
        due: Instant,
        slot: ListingSlot,
        wake: impl Fn() + Send + 'static,
    ) {
        let mut state = self.lock();
        state.waiters.push(Waiter {
            due,
            slot,
            wake: Box::new(wake),
        });
        if !state.serving {
            let service = Arc::clone(self);
            let spawned = std::thread::Builder::new()
                .name("paneflow-host-process-listing".into())
                .spawn(move || service.serve());
            match spawned {
                Ok(_) => state.serving = true,
                Err(error) => {
                    let waiters = std::mem::take(&mut state.waiters);
                    drop(state);
                    log::warn!("paneflow-host: cannot start the process listing thread: {error}");
                    let failed = Listing {
                        taken_at: Instant::now(),
                        entries: Arc::new(Err(io::Error::other(
                            "the process listing thread is unavailable",
                        ))),
                    };
                    deliver(waiters, &failed);
                    return;
                }
            }
        }
        self.changed.notify_all();
    }

    fn serve(&self) {
        let mut state = self.lock();
        loop {
            if let Some(latest) = state.latest.clone() {
                let horizon = latest.taken_at + self.interval / 2;
                let served = take_due(&mut state.waiters, horizon);
                if !served.is_empty() {
                    drop(state);
                    deliver(served, &latest);
                    state = self.lock();
                    continue;
                }
            }
            let Some(due) = state.waiters.iter().map(|waiter| waiter.due).min() else {
                state = self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            };
            let at = state
                .finished_at
                .map_or(due, |finished| due.max(finished + self.interval));
            let now = Instant::now();
            if state.listing || now < at {
                let wait = if state.listing {
                    self.interval
                } else {
                    at - now
                };
                state = self
                    .changed
                    .wait_timeout(state, wait)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                continue;
            }
            state = self.list_now(state).0;
        }
    }

    fn list_now<'a>(
        &'a self,
        mut state: MutexGuard<'a, State>,
    ) -> (MutexGuard<'a, State>, Listing) {
        state.listing = true;
        drop(state);
        let taken_at = Instant::now();
        let entries = Arc::new((self.list)());
        let mut state = self.lock();
        state.listing = false;
        state.finished_at = Some(Instant::now());
        let listing = Listing { taken_at, entries };
        state.latest = Some(listing.clone());
        self.changed.notify_all();
        (state, listing)
    }

    #[cfg(any(windows, test))]
    pub(crate) fn recent(&self, max_age: Duration) -> Listing {
        let mut state = self.lock();
        loop {
            if let Some(latest) = state
                .latest
                .as_ref()
                .filter(|latest| latest.taken_at.elapsed() <= max_age)
            {
                return latest.clone();
            }
            if !state.listing {
                break;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if let (Some(latest), Some(finished)) = (state.latest.as_ref(), state.finished_at)
            && finished.elapsed() < self.interval
        {
            return latest.clone();
        }
        self.list_now(state).1
    }
}

fn take_due(waiters: &mut Vec<Waiter>, horizon: Instant) -> Vec<Waiter> {
    let (served, waiting) = std::mem::take(waiters)
        .into_iter()
        .partition(|waiter| waiter.due <= horizon);
    *waiters = waiting;
    served
}

fn deliver(waiters: Vec<Waiter>, listing: &Listing) {
    for waiter in waiters {
        *waiter.slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(listing.clone());
        (waiter.wake)();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, channel};

    const INTERVAL: Duration = Duration::from_millis(80);

    fn counted(calls: &Arc<AtomicUsize>, fail: bool) -> Lister {
        let calls = Arc::clone(calls);
        Box::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            if fail {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "/proc"))
            } else {
                Ok(SystemEntries::new())
            }
        })
    }

    fn request(service: &Arc<SharedListing>, due: Instant) -> (ListingSlot, Receiver<()>) {
        let slot: ListingSlot = Arc::default();
        let (tx, rx) = channel();
        service.request(due, Arc::clone(&slot), move || {
            let _ = tx.send(());
        });
        (slot, rx)
    }

    #[test]
    fn requests_from_many_sessions_share_one_listing() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = SharedListing::new(INTERVAL, counted(&calls, false));
        let due = Instant::now();
        let requests: Vec<_> = (0..8).map(|_| request(&service, due)).collect();
        for (slot, wake) in &requests {
            wake.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(take(slot).is_some_and(|listing| listing.entries.is_ok()));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn continuous_demand_lists_at_most_once_per_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = SharedListing::new(INTERVAL, counted(&calls, false));
        let started = Instant::now();
        let window = INTERVAL * 10;
        let sessions: Vec<_> = (0..8)
            .map(|_| {
                let service = Arc::clone(&service);
                std::thread::spawn(move || {
                    while started.elapsed() < window {
                        let (_, wake) = request(&service, Instant::now() + INTERVAL);
                        let _ = wake.recv_timeout(Duration::from_secs(5));
                    }
                })
            })
            .collect();
        for session in sessions {
            session.join().unwrap();
        }
        let elapsed = started.elapsed();
        let budget = (elapsed.as_millis() / INTERVAL.as_millis()) as usize + 1;
        let listings = calls.load(Ordering::SeqCst);
        assert!(
            listings <= budget,
            "{listings} listings in {elapsed:?} exceed one per {INTERVAL:?}"
        );
        assert!(listings >= 2, "continuous demand keeps listing: {listings}");
    }

    #[test]
    fn a_failed_listing_is_delivered_and_retried_only_at_the_next_interval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = SharedListing::new(INTERVAL, counted(&calls, true));
        let (slot, wake) = request(&service, Instant::now());
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        let first = take(&slot).unwrap();
        assert!(first.entries.is_err());
        let after_failure = Instant::now();

        std::thread::sleep(INTERVAL * 3);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a failure without demand never loops"
        );

        let (slot, wake) = request(&service, Instant::now());
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        let retried = take(&slot).unwrap();
        assert!(retried.entries.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(retried.taken_at >= after_failure);

        let (slot, wake) = request(&service, Instant::now() + INTERVAL);
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        let spaced = take(&slot).unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(
            spaced.taken_at.duration_since(retried.taken_at) >= INTERVAL,
            "the retry waits for the next interval"
        );
    }

    #[test]
    fn a_due_request_is_served_by_a_recent_listing_without_a_new_one() {
        let calls = Arc::new(AtomicUsize::new(0));
        let service = SharedListing::new(INTERVAL, counted(&calls, false));
        let fresh = service.recent(Duration::from_secs(60));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let (slot, wake) = request(&service, fresh.taken_at);
        wake.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(take(&slot).unwrap().taken_at, fresh.taken_at);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.recent(Duration::from_secs(60)).taken_at,
            fresh.taken_at
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
