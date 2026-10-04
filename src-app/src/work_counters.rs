use paneflow_host::work_counters::{Counter, counters_value};
use serde_json::{Map, Value, json};

pub(crate) static ROOT_RENDERS: Counter = Counter::new();

pub(crate) static HOST_AGENT_SNAPSHOTS_APPLIED: Counter = Counter::new();

pub(crate) static SESSION_LIST_CALLS: Counter = Counter::new();

pub(crate) const GIT_SUBCOMMANDS: [&str; 16] = [
    "add",
    "branch",
    "clone",
    "config",
    "diff",
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

const OTHER_GIT_SUBCOMMAND: &str = "other";

static GIT_SPAWNS_BY_SUBCOMMAND: [Counter; GIT_SUBCOMMANDS.len() + 1] =
    [const { Counter::new() }; GIT_SUBCOMMANDS.len() + 1];

static GIT_PROBE_SPAWNS: Counter = Counter::new();

static GIT_USER_ACTION_SPAWNS: Counter = Counter::new();

#[cfg(test)]
thread_local! {
    static COUNTED_ON_THIS_THREAD: std::cell::RefCell<std::collections::BTreeMap<usize, u64>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

pub(crate) fn count(counter: &'static Counter) {
    counter.increment();
    #[cfg(test)]
    COUNTED_ON_THIS_THREAD.with(|counted| {
        *counted
            .borrow_mut()
            .entry(std::ptr::from_ref(counter).addr())
            .or_default() += 1;
    });
}

#[cfg(test)]
pub(crate) fn counted_on_this_thread(counter: &'static Counter) -> u64 {
    COUNTED_ON_THIS_THREAD.with(|counted| {
        counted
            .borrow()
            .get(&std::ptr::from_ref(counter).addr())
            .copied()
            .unwrap_or(0)
    })
}

pub(crate) fn git_subcommand_counter(subcommand: Option<&str>) -> &'static Counter {
    let index = subcommand
        .and_then(|name| GIT_SUBCOMMANDS.iter().position(|known| *known == name))
        .unwrap_or(GIT_SUBCOMMANDS.len());
    &GIT_SPAWNS_BY_SUBCOMMAND[index]
}

pub(crate) fn git_profile_counter(probe: bool) -> &'static Counter {
    if probe {
        &GIT_PROBE_SPAWNS
    } else {
        &GIT_USER_ACTION_SPAWNS
    }
}

pub(crate) fn record_git_spawn(probe: bool, subcommand: Option<&str>) {
    count(git_profile_counter(probe));
    count(git_subcommand_counter(subcommand));
}

pub(crate) fn counters() -> Value {
    let mut by_subcommand = Map::new();
    for (name, counter) in GIT_SUBCOMMANDS
        .iter()
        .chain([&OTHER_GIT_SUBCOMMAND])
        .zip(&GIT_SPAWNS_BY_SUBCOMMAND)
    {
        by_subcommand.insert((*name).to_string(), json!(counter.get()));
    }
    let probe = GIT_PROBE_SPAWNS.get();
    let user_action = GIT_USER_ACTION_SPAWNS.get();
    let mut counters = Map::new();
    counters.insert("root_renders".to_string(), json!(ROOT_RENDERS.get()));
    counters.insert(
        "host_agent_snapshots_applied".to_string(),
        json!(HOST_AGENT_SNAPSHOTS_APPLIED.get()),
    );
    counters.insert(
        "session_list_calls".to_string(),
        json!(SESSION_LIST_CALLS.get()),
    );
    counters.insert(
        "git_spawns".to_string(),
        json!({
            "total": probe + user_action,
            "probe": probe,
            "user_action": user_action,
            "by_subcommand": by_subcommand,
        }),
    );
    counters.insert(
        "process_spawns".to_string(),
        json!(paneflow_process::spawn_count()),
    );
    json!({ "counters": counters_value(counters) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_count_moves_its_counter_by_exactly_one_on_this_thread() {
        let before = counted_on_this_thread(&ROOT_RENDERS);
        let global = ROOT_RENDERS.get();
        count(&ROOT_RENDERS);
        assert_eq!(counted_on_this_thread(&ROOT_RENDERS) - before, 1);
        assert!(ROOT_RENDERS.get() > global);
    }

    #[test]
    fn an_unknown_git_subcommand_counts_as_other_and_a_known_one_under_its_name() {
        assert!(std::ptr::eq(
            git_subcommand_counter(Some("rebase")),
            git_subcommand_counter(None)
        ));
        assert!(!std::ptr::eq(
            git_subcommand_counter(Some("status")),
            git_subcommand_counter(None)
        ));
    }

    #[test]
    fn the_desktop_counters_name_every_counter_with_the_process_identity() {
        let value = counters();
        let sample = paneflow_host::work_counters::sample(
            "desktop",
            &value,
            paneflow_host::work_counters::DESKTOP_COUNTERS,
        );
        assert_eq!(
            sample.identity.map(|identity| identity.pid),
            Some(std::process::id())
        );
        for (name, reading) in &sample.readings {
            assert!(
                matches!(reading, paneflow_host::work_counters::Reading::Measured(_)),
                "{name}: {reading:?}"
            );
        }
    }
}
