#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use paneflow_host::ProcessIdentity;
use paneflow_host::runtime_observer::observe_foreground_runtime;
use paneflow_host::work_counters::{FOREGROUND_OBSERVATIONS, PROCESS_LISTINGS};

fn list_every_process() {
    #[cfg(unix)]
    paneflow_host::process::unix_process_entries().unwrap();
    #[cfg(windows)]
    paneflow_host::process::windows_process_entries_named().unwrap();
}

#[test]
fn a_forced_listing_and_a_foreground_observation_each_count_exactly_once() {
    let listings = PROCESS_LISTINGS.get();
    list_every_process();
    assert_eq!(PROCESS_LISTINGS.get() - listings, 1);
    list_every_process();
    assert_eq!(PROCESS_LISTINGS.get() - listings, 2);

    let observations = FOREGROUND_OBSERVATIONS.get();
    let this_process = ProcessIdentity::capture(std::process::id());
    let _ = observe_foreground_runtime(this_process, None);
    assert_eq!(FOREGROUND_OBSERVATIONS.get() - observations, 1);

    let gone = ProcessIdentity {
        pid: 1,
        started_at: None,
    };
    let _ = observe_foreground_runtime(gone, None);
    assert_eq!(
        FOREGROUND_OBSERVATIONS.get() - observations,
        1,
        "an unobservable leader is rejected before any walk"
    );
}
