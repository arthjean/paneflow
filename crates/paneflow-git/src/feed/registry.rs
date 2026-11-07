use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{ChangeFeed, FeedOptions, Ineligible};

pub const IDLE_EVICTION: Duration = Duration::from_secs(5 * 60);

enum Slot {
    Live(Box<ChangeFeed>),
    Refused(Ineligible),
}

struct Entry {
    slot: Slot,
    last_probe: Instant,
}

pub struct Feeds {
    options: FeedOptions,
    idle: Duration,
    entries: BTreeMap<PathBuf, Entry>,
    starts: usize,
}

impl Feeds {
    pub fn new(options: FeedOptions) -> Self {
        Self::with_idle(options, IDLE_EVICTION)
    }

    pub fn with_idle(options: FeedOptions, idle: Duration) -> Self {
        Self {
            options,
            idle,
            entries: BTreeMap::new(),
            starts: 0,
        }
    }

    pub fn acquire(
        &mut self,
        worktree: &Path,
        now: Instant,
    ) -> Result<&mut ChangeFeed, Ineligible> {
        let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
        if !self.entries.contains_key(&key) {
            self.starts += 1;
            let slot = match ChangeFeed::start(&key, &self.options) {
                Ok(feed) => Slot::Live(Box::new(feed)),
                Err(reason) => Slot::Refused(reason),
            };
            self.entries.insert(
                key.clone(),
                Entry {
                    slot,
                    last_probe: now,
                },
            );
        }
        let entry = self.entries.get_mut(&key).ok_or_else(|| Ineligible::Scan {
            cause: "the feed registry lost its entry".to_string(),
        })?;
        entry.last_probe = now;
        match &mut entry.slot {
            Slot::Live(feed) => Ok(feed.as_mut()),
            Slot::Refused(reason) => Err(reason.clone()),
        }
    }

    pub fn release(&mut self, worktree: &Path) -> bool {
        let key = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
        self.entries.remove(&key).is_some()
    }

    pub fn evict_idle(&mut self, now: Instant) -> usize {
        let before = self.entries.len();
        let idle = self.idle;
        self.entries
            .retain(|_, entry| now.saturating_duration_since(entry.last_probe) < idle);
        before - self.entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn live(&self) -> usize {
        self.entries
            .values()
            .filter(|entry| matches!(entry.slot, Slot::Live(_)))
            .count()
    }

    pub fn starts(&self) -> usize {
        self.starts
    }
}
