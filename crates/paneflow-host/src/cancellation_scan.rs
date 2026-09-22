use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use crate::hook_assets::{self, Cancellation};
use crate::host::SessionHost;
use crate::session_input::InputSignal;

pub const CANCELLATION_SCAN_INTERVAL: Duration = Duration::from_millis(100);

pub fn spawn(host: &Arc<SessionHost>) {
    let weak = Arc::downgrade(host);
    let spawned = std::thread::Builder::new()
        .name("paneflow-host-cancellation".into())
        .spawn(move || scan_loop(weak));
    if let Err(error) = spawned {
        log::warn!("paneflow-host: cannot start the cancellation scan: {error}");
    }
}

fn scan_loop(host: Weak<SessionHost>) {
    loop {
        std::thread::sleep(CANCELLATION_SCAN_INTERVAL);
        let Some(host) = host.upgrade() else {
            return;
        };
        scan_once(&host, SystemTime::now());
    }
}

fn scan_once(host: &Arc<SessionHost>, now: SystemTime) {
    for (session, input) in host.fenced_input_targets() {
        let signals = {
            let mut guard = input
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.settle(now);
            guard.take_pending()
        };
        for captured in signals {
            let generation = captured.generation;
            let signal = captured.signal;
            let marker = host
                .commit_marker(&session, generation, move |directory| {
                    record(directory, generation.get(), signal)
                })
                .flatten();
            if let Some(marker) = marker {
                host.announce_cancellation(&session, generation, &marker);
            }
        }
    }
}

fn record(
    directory: &std::path::Path,
    generation: u64,
    signal: InputSignal,
) -> Option<Cancellation> {
    let written = match signal {
        InputSignal::Cancelled(at) => hook_assets::record_cancellation(directory, generation, at),
        InputSignal::Submitted(at) => hook_assets::record_submission(directory, generation, at),
    };
    match written {
        Ok(marker) => marker,
        Err(error) => {
            log::warn!(
                "paneflow-host: cannot record the cancellation fence in {}: {error}",
                directory.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::UNIX_EPOCH;

    fn at(milliseconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(milliseconds)
    }

    #[test]
    fn an_escape_then_a_submission_lands_in_one_marker_in_order() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            record(dir.path(), 3, InputSignal::Submitted(at(500))),
            None,
            "a submission without a fence writes nothing"
        );
        let cancelled =
            record(dir.path(), 3, InputSignal::Cancelled(at(900))).expect("the fence is recorded");
        assert_eq!(cancelled.runtime_generation, 3);
        assert_eq!(cancelled.cancelled_at, 900);
        assert_eq!(cancelled.submitted_at, None);

        let resumed = record(dir.path(), 3, InputSignal::Submitted(at(1_100)))
            .expect("the submission is recorded");
        assert_eq!(resumed.cancelled_at, 900);
        assert_eq!(resumed.submitted_at, Some(1_100));
        assert_eq!(hook_assets::read_cancellation(dir.path()), Some(resumed));
    }
}
