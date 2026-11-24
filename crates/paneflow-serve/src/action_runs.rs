use std::collections::HashMap;
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
    opened: u64,
    slot: Slot,
}

impl Run {
    fn waitable_by(&self, caller: Option<&str>) -> bool {
        match self.asker.as_deref() {
            Some(asker) => caller == Some(asker),
            None => true,
        }
    }
}

#[derive(Default)]
struct RunsState {
    opened: u64,
    runs: HashMap<String, Run>,
}

impl RunsState {
    fn sweep(&mut self, now_ms: u64) {
        self.runs.retain(|_, run| match run.slot {
            Slot::Running => true,
            Slot::Done { at_ms, .. } => now_ms.saturating_sub(at_ms) < RUN_RETENTION_MS,
        });
        let mut finished: Vec<(u64, String)> = self
            .runs
            .iter()
            .filter(|(_, run)| matches!(run.slot, Slot::Done { .. }))
            .map(|(id, run)| (run.opened, id.clone()))
            .collect();
        finished.sort();
        let excess = finished.len().saturating_sub(MAX_FINISHED_RUNS);
        for (_, id) in finished.into_iter().take(excess) {
            self.runs.remove(&id);
        }
    }
}

#[derive(Default)]
pub struct ActionRuns {
    state: Mutex<RunsState>,
    changed: Condvar,
}

pub fn unknown_run(run_id: &str) -> ServeError {
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

    pub fn open(&self, asker: Option<String>) -> Result<String, ServeError> {
        let id = crate::handles::fresh_secret().map_err(|detail| ServeError {
            code: crate::protocol::ERR_INTERNAL,
            message: detail,
            data: None,
        })?;
        let mut state = self.lock();
        state.sweep(now_ms());
        state.opened += 1;
        let opened = state.opened;
        state.runs.insert(
            id.clone(),
            Run {
                asker,
                opened,
                slot: Slot::Running,
            },
        );
        Ok(id)
    }

    pub fn finish(&self, run_id: &str, outcome: Result<Invoked, ServeError>) {
        if let Some(run) = self.lock().runs.get_mut(run_id)
            && matches!(run.slot, Slot::Running)
        {
            run.slot = Slot::Done {
                outcome,
                at_ms: now_ms(),
            };
        }
        self.changed.notify_all();
    }

    pub fn discard(&self, run_id: &str) {
        self.lock().runs.remove(run_id);
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
        run_id: &str,
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
            let Some(run) = state.runs.get(run_id) else {
                return Err(unknown_run(run_id));
            };
            if !run.waitable_by(caller) {
                return Err(unknown_run(run_id));
            }
            if matches!(run.slot, Slot::Done { .. }) {
                let Some(Run {
                    slot: Slot::Done { outcome, .. },
                    ..
                }) = state.runs.remove(run_id)
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
                return Ok(Invoked::running(run_id.to_string()));
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
        let id = runs.open(Some("a".into())).unwrap();
        assert_eq!(id.len(), crate::handles::SECRET_HEX_LEN, "128 random bits");
        assert_eq!(
            runs.wait(&id, None, Some("a")).unwrap(),
            Invoked::running(id.clone())
        );
        assert_eq!(runs.running(), 1);
        runs.finish(
            &id,
            Ok(Invoked {
                result: Some(json!({"ok": true})),
                ..Invoked::default()
            }),
        );
        assert_eq!(
            runs.wait(&id, None, Some("a")).unwrap(),
            Invoked::done(Some(json!({"ok": true})))
        );
        assert!(runs.wait(&id, None, Some("a")).is_err(), "taken once");
    }

    #[test]
    fn the_first_end_of_a_run_is_the_one_its_asker_reads() {
        let runs = ActionRuns::default();
        let id = runs.open(None).unwrap();
        runs.finish(&id, Ok(Invoked::default()));
        runs.finish(&id, Err(ServeError::invalid_params("late")));
        assert_eq!(runs.wait(&id, None, None).unwrap(), Invoked::done(None));
    }

    #[test]
    fn only_the_asker_waits_on_an_agent_run_and_a_caller_outside_every_pane_is_not_it() {
        let runs = ActionRuns::default();
        let id = runs.open(Some("a".into())).unwrap();
        assert!(runs.wait(&id, None, Some("b")).is_err());
        assert!(
            runs.wait(&id, None, None).is_err(),
            "a caller with no pane session is not the asker"
        );
        runs.finish(&id, Err(ServeError::invalid_params("boom")));
        assert!(runs.wait(&id, None, None).is_err());
        assert_eq!(runs.wait(&id, None, Some("a")).unwrap_err().message, "boom");
        assert!(
            runs.wait(&id, Some(MAX_ACTION_WAIT_MS + 1), Some("a"))
                .is_err(),
            "'wait_ms' is bounded"
        );
    }

    #[test]
    fn any_other_handle_answers_no_action_run() {
        let runs = ActionRuns::default();
        let id = runs.open(None).unwrap();
        let other = runs.open(None).unwrap();
        assert_ne!(id, other);
        for guess in ["1", "2", "", &"0".repeat(32), &id[..31]] {
            let refused = runs.wait(guess, None, None).unwrap_err();
            assert!(
                refused.message.starts_with("no action run"),
                "{}",
                refused.message
            );
        }
    }

    #[test]
    fn the_oldest_finished_runs_are_forgotten_first() {
        let runs = ActionRuns::default();
        let ids: Vec<String> = (0..=MAX_FINISHED_RUNS)
            .map(|_| {
                let id = runs.open(None).unwrap();
                runs.finish(&id, Ok(Invoked::default()));
                id
            })
            .collect();
        let _trigger = runs.open(None).unwrap();
        assert!(runs.wait(&ids[0], None, None).is_err(), "the oldest went");
        assert!(runs.wait(&ids[MAX_FINISHED_RUNS], None, None).is_ok());
    }

    #[test]
    fn a_waiter_wakes_when_the_run_ends() {
        let runs = std::sync::Arc::new(ActionRuns::default());
        let id = runs.open(None).unwrap();
        let finisher = std::sync::Arc::clone(&runs);
        let finishing_id = id.clone();
        let finishing = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            finisher.finish(&finishing_id, Ok(Invoked::default()));
        });
        let outcome = runs.wait(&id, Some(MAX_ACTION_WAIT_MS), None).unwrap();
        finishing.join().unwrap();
        assert_eq!(outcome.status, Some(RunStatus::Done));
    }
}
