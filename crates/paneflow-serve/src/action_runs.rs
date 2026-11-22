use std::collections::BTreeMap;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_api::methods::config::{Invoked, MAX_ACTION_WAIT_MS, RunStatus};

use crate::registry::ServeError;

pub const RUN_RETENTION_MS: u64 = 120_000;
pub const MAX_FINISHED_RUNS: usize = 256;

enum Slot {
    Running,
    Done {
        outcome: Result<Invoked, ServeError>,
        at_ms: u64,
    },
}

struct Run {
    asker: Option<String>,
    slot: Slot,
}

#[derive(Default)]
struct RunsState {
    next: u64,
    runs: BTreeMap<u64, Run>,
}

impl RunsState {
    fn sweep(&mut self, now_ms: u64) {
        self.runs.retain(|_, run| match run.slot {
            Slot::Running => true,
            Slot::Done { at_ms, .. } => now_ms.saturating_sub(at_ms) < RUN_RETENTION_MS,
        });
        let finished: Vec<u64> = self
            .runs
            .iter()
            .filter(|(_, run)| matches!(run.slot, Slot::Done { .. }))
            .map(|(id, _)| *id)
            .collect();
        for id in finished
            .iter()
            .take(finished.len().saturating_sub(MAX_FINISHED_RUNS))
        {
            self.runs.remove(id);
        }
    }
}

#[derive(Default)]
pub struct ActionRuns {
    state: Mutex<RunsState>,
    changed: Condvar,
}

pub fn unknown_run(run_id: u64) -> ServeError {
    ServeError::invalid_params(format!(
        "no action run with id {run_id}; its result was already returned or it never existed"
    ))
}

fn now_ms() -> u64 {
    crate::state::now_ms()
}

impl ActionRuns {
    fn lock(&self) -> MutexGuard<'_, RunsState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn open(&self, asker: Option<String>) -> u64 {
        let mut state = self.lock();
        state.sweep(now_ms());
        state.next += 1;
        let id = state.next;
        state.runs.insert(
            id,
            Run {
                asker,
                slot: Slot::Running,
            },
        );
        id
    }

    pub fn finish(&self, run_id: u64, outcome: Result<Invoked, ServeError>) {
        if let Some(run) = self.lock().runs.get_mut(&run_id)
            && matches!(run.slot, Slot::Running)
        {
            run.slot = Slot::Done {
                outcome,
                at_ms: now_ms(),
            };
        }
        self.changed.notify_all();
    }

    pub fn discard(&self, run_id: u64) {
        self.lock().runs.remove(&run_id);
    }

    pub fn running(&self) -> usize {
        self.lock()
            .runs
            .values()
            .filter(|run| matches!(run.slot, Slot::Running))
            .count()
    }

    pub fn wait(
        &self,
        run_id: u64,
        wait_ms: Option<u64>,
        caller: Option<&str>,
    ) -> Result<Invoked, ServeError> {
        let wait_ms = wait_ms.unwrap_or(0);
        if wait_ms > MAX_ACTION_WAIT_MS {
            return Err(ServeError::invalid_params(format!(
                "'wait_ms' is at most {MAX_ACTION_WAIT_MS}"
            )));
        }
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut state = self.lock();
        loop {
            let Some(run) = state.runs.get(&run_id) else {
                return Err(unknown_run(run_id));
            };
            if let (Some(asker), Some(caller)) = (run.asker.as_deref(), caller)
                && asker != caller
            {
                return Err(unknown_run(run_id));
            }
            if matches!(run.slot, Slot::Done { .. }) {
                let Some(Run {
                    slot: Slot::Done { outcome, .. },
                    ..
                }) = state.runs.remove(&run_id)
                else {
                    return Err(unknown_run(run_id));
                };
                return outcome.map(|mut invoked| {
                    invoked.status = Some(RunStatus::Done);
                    invoked.run_id = None;
                    invoked
                });
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Invoked::running(run_id));
            }
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_run_answers_running_until_it_ends_then_its_result_once() {
        let runs = ActionRuns::default();
        let id = runs.open(Some("a".into()));
        assert_eq!(
            runs.wait(id, None, Some("a")).unwrap(),
            Invoked::running(id)
        );
        assert_eq!(runs.running(), 1);
        runs.finish(
            id,
            Ok(Invoked {
                result: Some(json!({"ok": true})),
                ..Invoked::default()
            }),
        );
        assert_eq!(
            runs.wait(id, None, Some("a")).unwrap(),
            Invoked::done(Some(json!({"ok": true})))
        );
        assert!(runs.wait(id, None, Some("a")).is_err(), "taken once");
    }

    #[test]
    fn the_first_end_of_a_run_is_the_one_its_asker_reads() {
        let runs = ActionRuns::default();
        let id = runs.open(None);
        runs.finish(id, Ok(Invoked::default()));
        runs.finish(id, Err(ServeError::invalid_params("late")));
        assert_eq!(runs.wait(id, None, None).unwrap(), Invoked::done(None));
    }

    #[test]
    fn another_agent_cannot_wait_on_a_run_and_an_error_reaches_its_asker() {
        let runs = ActionRuns::default();
        let id = runs.open(Some("a".into()));
        assert!(runs.wait(id, None, Some("b")).is_err());
        runs.finish(id, Err(ServeError::invalid_params("boom")));
        assert_eq!(runs.wait(id, None, None).unwrap_err().message, "boom");
        assert!(
            runs.wait(id, Some(MAX_ACTION_WAIT_MS + 1), None).is_err(),
            "'wait_ms' is bounded"
        );
    }

    #[test]
    fn a_waiter_wakes_when_the_run_ends() {
        let runs = std::sync::Arc::new(ActionRuns::default());
        let id = runs.open(None);
        let finisher = std::sync::Arc::clone(&runs);
        let finishing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            finisher.finish(id, Ok(Invoked::default()));
        });
        let outcome = runs.wait(id, Some(MAX_ACTION_WAIT_MS), None).unwrap();
        finishing.join().unwrap();
        assert_eq!(outcome.status, Some(RunStatus::Done));
    }
}
