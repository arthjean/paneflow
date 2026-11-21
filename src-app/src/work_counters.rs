use paneflow_host::work_counters::{Counter, counters_value};
use serde_json::{Map, Value, json};

pub(crate) static ROOT_RENDERS: Counter = Counter::new();

pub(crate) static SIDEBAR_RENDERS: Counter = Counter::new();

pub(crate) static HOST_AGENT_SNAPSHOTS_APPLIED: Counter = Counter::new();

pub(crate) static SESSION_LIST_CALLS: Counter = Counter::new();

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

pub(crate) fn counters() -> Value {
    let spawns = paneflow_git::counters::spawns();
    let mut by_subcommand = Map::new();
    for (name, count) in &spawns.by_subcommand {
        by_subcommand.insert((*name).to_string(), json!(count));
    }
    let probe = spawns.probe;
    let user_action = spawns.user_action;
    let mut counters = Map::new();
    counters.insert("root_renders".to_string(), json!(ROOT_RENDERS.get()));
    counters.insert("sidebar_renders".to_string(), json!(SIDEBAR_RENDERS.get()));
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
