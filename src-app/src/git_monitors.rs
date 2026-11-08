use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use paneflow_git::engine::{Fallback, Selector};
use paneflow_git::feed::{FeedOptions, IDLE_EVICTION};
use paneflow_git::{Error, Freshness, Monitor, Objects, Options, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Keep {
    Alive,
    IfLive,
}

pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Default)]
struct Fanout {
    next: AtomicU64,
    wakers: Mutex<Vec<(u64, Wake)>>,
}

impl Fanout {
    fn wake_all(&self) {
        let wakers: Vec<Wake> = lock(&self.wakers)
            .iter()
            .map(|(_, wake)| Arc::clone(wake))
            .collect();
        for wake in wakers {
            wake();
        }
    }
}

pub(crate) struct Subscription {
    fanout: Arc<Fanout>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        lock(&self.fanout.wakers).retain(|(id, _)| *id != self.id);
    }
}

struct Slot {
    monitor: Mutex<Monitor>,
    kept_at: Mutex<Instant>,
}

pub(crate) struct GitMonitors {
    enabled: bool,
    idle: Duration,
    feed_options: FeedOptions,
    slots: Mutex<HashMap<PathBuf, Arc<Slot>>>,
    fanouts: Mutex<HashMap<PathBuf, Arc<Fanout>>>,
}

impl GitMonitors {
    pub(crate) fn new(feed_options: FeedOptions, enabled: bool, idle: Duration) -> Self {
        Self {
            enabled,
            idle,
            feed_options,
            slots: Mutex::new(HashMap::new()),
            fanouts: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn global() -> &'static GitMonitors {
        static GLOBAL: OnceLock<GitMonitors> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            GitMonitors::new(
                FeedOptions::for_paneflow_home(),
                paneflow_git::monitor::monitor_enabled(),
                IDLE_EVICTION,
            )
        })
    }

    pub(crate) fn with_snapshot<T>(
        &self,
        selector: &Selector,
        cwd: &Path,
        options: &Options,
        freshness: Freshness,
        keep: Keep,
        read: impl FnOnce(Snapshot, &Objects<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let Some(slot) = self.slot(cwd, keep) else {
            return paneflow_git::with_snapshot(cwd, options, read);
        };
        let probed = {
            let mut monitor = lock(&slot.monitor);
            let probed = monitor.probe(cwd, options, freshness);
            if probed.is_ok() && monitor.last_probe().kind.fell_back() {
                selector.record_fallback(Fallback::Monitor);
            }
            probed
        }?;
        probed.read(read)
    }

    pub(crate) fn subscribe(&self, cwd: &Path, wake: Wake) -> Option<Subscription> {
        if !self.enabled {
            return None;
        }
        let (key, _) = worktree_key(cwd)?;
        let fanout = self.fanout(&key);
        let id = fanout.next.fetch_add(1, Ordering::Relaxed);
        lock(&fanout.wakers).push((id, wake));
        Some(Subscription { fanout, id })
    }

    pub(crate) fn evict_idle(&self) -> usize {
        let evicted = self.take_idle(Instant::now());
        let count = evicted.len();
        drop(evicted);
        count
    }

    pub(crate) fn release(&self, cwd: &Path) -> bool {
        let Some((key, _)) = worktree_key(cwd) else {
            return false;
        };
        let released = lock(&self.slots).remove(&key);
        released.is_some()
    }

    fn slot(&self, cwd: &Path, keep: Keep) -> Option<Arc<Slot>> {
        if !self.enabled {
            return None;
        }
        let (key, root) = worktree_key(cwd)?;
        let now = Instant::now();
        let evicted = self.take_idle(now);
        let slot = {
            let mut slots = lock(&self.slots);
            match keep {
                Keep::Alive => {
                    let slot = match slots.get(&key) {
                        Some(slot) => Arc::clone(slot),
                        None => {
                            let slot = Arc::new(self.new_slot(&key, &root, now));
                            slots.insert(key, Arc::clone(&slot));
                            slot
                        }
                    };
                    *lock(&slot.kept_at) = now;
                    Some(slot)
                }
                Keep::IfLive => slots.get(&key).map(Arc::clone),
            }
        };
        drop(evicted);
        slot
    }

    fn new_slot(&self, key: &Path, root: &Path, now: Instant) -> Slot {
        let mut monitor = Monitor::configured(root, self.feed_options.clone(), true);
        let fanout = self.fanout(key);
        monitor.set_waker(move || fanout.wake_all());
        Slot {
            monitor: Mutex::new(monitor),
            kept_at: Mutex::new(now),
        }
    }

    fn fanout(&self, key: &Path) -> Arc<Fanout> {
        Arc::clone(lock(&self.fanouts).entry(key.to_path_buf()).or_default())
    }

    fn take_idle(&self, now: Instant) -> Vec<Arc<Slot>> {
        let mut slots = lock(&self.slots);
        let idle: Vec<PathBuf> = slots
            .iter()
            .filter(|(_, slot)| now.saturating_duration_since(*lock(&slot.kept_at)) >= self.idle)
            .map(|(root, _)| root.clone())
            .collect();
        idle.iter().filter_map(|root| slots.remove(root)).collect()
    }

    #[cfg(test)]
    pub(crate) fn monitors(&self) -> usize {
        lock(&self.slots).len()
    }

    #[cfg(test)]
    pub(crate) fn live_feeds(&self) -> usize {
        let slots: Vec<Arc<Slot>> = lock(&self.slots).values().map(Arc::clone).collect();
        slots
            .iter()
            .filter(|slot| lock(&slot.monitor).feed().is_some())
            .count()
    }
}

fn worktree_key(cwd: &Path) -> Option<(PathBuf, PathBuf)> {
    let root = paneflow_git::worktree_root(cwd)?;
    let key = std::fs::canonicalize(&root).ok()?;
    Some((key, root))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests;
