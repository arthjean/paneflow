use std::sync::atomic::{AtomicU64, Ordering};

pub struct Counter(AtomicU64);

impl Counter {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn increment(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self::new()
    }
}

pub const GIT_SUBCOMMANDS: [&str; 18] = [
    "add",
    "branch",
    "cat-file",
    "clone",
    "config",
    "diff",
    "diff-index",
    "for-each-ref",
    "log",
    "ls-files",
    "merge-base",
    "rev-parse",
    "show",
    "status",
    "switch",
    "symbolic-ref",
    "worktree",
    "write-tree",
];

pub const OTHER_GIT_SUBCOMMAND: &str = "other";

static GIT_SPAWNS_BY_SUBCOMMAND: [Counter; GIT_SUBCOMMANDS.len() + 1] =
    [const { Counter::new() }; GIT_SUBCOMMANDS.len() + 1];

static GIT_PROBE_SPAWNS: Counter = Counter::new();

static GIT_USER_ACTION_SPAWNS: Counter = Counter::new();

#[cfg(any(test, feature = "test-support"))]
thread_local! {
    static COUNTED_ON_THIS_THREAD: std::cell::RefCell<std::collections::BTreeMap<usize, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

fn count(counter: &'static Counter) {
    counter.increment();
    #[cfg(any(test, feature = "test-support"))]
    COUNTED_ON_THIS_THREAD.with(|counted| {
        *counted
            .borrow_mut()
            .entry(std::ptr::from_ref(counter).addr())
            .or_default() += 1;
    });
}

#[cfg(any(test, feature = "test-support"))]
pub fn counted_on_this_thread(counter: &'static Counter) -> u64 {
    COUNTED_ON_THIS_THREAD.with(|counted| {
        counted
            .borrow()
            .get(&std::ptr::from_ref(counter).addr())
            .copied()
            .unwrap_or(0)
    })
}

pub fn subcommand_counter(subcommand: Option<&str>) -> &'static Counter {
    let index = subcommand
        .and_then(|name| GIT_SUBCOMMANDS.iter().position(|known| *known == name))
        .unwrap_or(GIT_SUBCOMMANDS.len());
    &GIT_SPAWNS_BY_SUBCOMMAND[index]
}

pub fn profile_counter(probe: bool) -> &'static Counter {
    if probe {
        &GIT_PROBE_SPAWNS
    } else {
        &GIT_USER_ACTION_SPAWNS
    }
}

pub fn record_spawn(probe: bool, subcommand: Option<&str>) {
    count(profile_counter(probe));
    count(subcommand_counter(subcommand));
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawns {
    pub probe: u64,
    pub user_action: u64,
    pub by_subcommand: Vec<(&'static str, u64)>,
}

pub fn spawns() -> Spawns {
    Spawns {
        probe: GIT_PROBE_SPAWNS.get(),
        user_action: GIT_USER_ACTION_SPAWNS.get(),
        by_subcommand: GIT_SUBCOMMANDS
            .iter()
            .copied()
            .chain([OTHER_GIT_SUBCOMMAND])
            .zip(&GIT_SPAWNS_BY_SUBCOMMAND)
            .map(|(name, counter)| (name, counter.get()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_git_subcommand_counts_as_other_and_a_known_one_under_its_name() {
        assert!(std::ptr::eq(
            subcommand_counter(Some("rebase")),
            subcommand_counter(None)
        ));
        assert!(!std::ptr::eq(
            subcommand_counter(Some("status")),
            subcommand_counter(None)
        ));
    }

    #[test]
    fn one_spawn_moves_its_profile_and_subcommand_by_exactly_one_on_this_thread() {
        let probe = counted_on_this_thread(profile_counter(true));
        let status = counted_on_this_thread(subcommand_counter(Some("status")));
        record_spawn(true, Some("status"));
        assert_eq!(counted_on_this_thread(profile_counter(true)) - probe, 1);
        assert_eq!(
            counted_on_this_thread(subcommand_counter(Some("status"))) - status,
            1
        );
        let total = spawns();
        assert_eq!(total.by_subcommand.len(), GIT_SUBCOMMANDS.len() + 1);
        assert!(total.probe >= 1);
    }
}
