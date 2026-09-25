use super::*;

#[derive(Default)]
struct StagingState {
    active: usize,
    bytes: usize,
    peak_bytes: usize,
    refused: u64,
}

#[derive(Default)]
pub(super) struct CheckpointStaging {
    state: Mutex<StagingState>,
    released: std::sync::Condvar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct StagingReport {
    pub active: usize,
    pub max_concurrent: usize,
    pub staged_bytes: usize,
    pub peak_staged_bytes: usize,
    pub budget_bytes: usize,
    pub refused: u64,
}

impl CheckpointStaging {
    fn lock(&self) -> std::sync::MutexGuard<'_, StagingState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn admit(
        self: &Arc<Self>,
        bytes: usize,
        deadline: Instant,
    ) -> Result<StagingLease, HostError> {
        if bytes > CHECKPOINT_STAGING_BUDGET_BYTES {
            self.lock().refused += 1;
            return Err(HostError::Busy(format!(
                "a {bytes} byte checkpoint exceeds the {CHECKPOINT_STAGING_BUDGET_BYTES} byte staging budget"
            )));
        }
        let mut state = self.lock();
        loop {
            let fits = state.active < MAX_CONCURRENT_CHECKPOINTS
                && state.bytes.saturating_add(bytes) <= CHECKPOINT_STAGING_BUDGET_BYTES;
            if fits {
                state.active += 1;
                state.bytes += bytes;
                state.peak_bytes = state.peak_bytes.max(state.bytes);
                return Ok(StagingLease {
                    staging: Arc::clone(self),
                    bytes,
                });
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                state.refused += 1;
                return Err(HostError::Busy(format!(
                    "checkpoint staging is at capacity ({} of {MAX_CONCURRENT_CHECKPOINTS} captures, {} of {CHECKPOINT_STAGING_BUDGET_BYTES} bytes); retry when an attachment finishes",
                    state.active, state.bytes
                )));
            }
            let (guard, _) = self
                .released
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = guard;
        }
    }

    pub(super) fn report(&self) -> StagingReport {
        let state = self.lock();
        StagingReport {
            active: state.active,
            max_concurrent: MAX_CONCURRENT_CHECKPOINTS,
            staged_bytes: state.bytes,
            peak_staged_bytes: state.peak_bytes,
            budget_bytes: CHECKPOINT_STAGING_BUDGET_BYTES,
            refused: state.refused,
        }
    }
}

pub struct StagingLease {
    staging: Arc<CheckpointStaging>,
    bytes: usize,
}

impl StagingLease {
    pub(super) fn adjust(&mut self, actual: usize) {
        let mut state = self.staging.lock();
        state.bytes = state
            .bytes
            .saturating_sub(self.bytes)
            .saturating_add(actual);
        state.peak_bytes = state.peak_bytes.max(state.bytes);
        self.bytes = actual;
    }
}

impl Drop for StagingLease {
    fn drop(&mut self) {
        let mut state = self.staging.lock();
        state.active = state.active.saturating_sub(1);
        state.bytes = state.bytes.saturating_sub(self.bytes);
        drop(state);
        self.staging.released.notify_all();
    }
}

pub struct StagedCheckpoint {
    pub(super) checkpoint: Checkpoint,
    pub(super) _lease: StagingLease,
}

impl StagedCheckpoint {
    pub fn into_inner(self) -> Checkpoint {
        self.checkpoint
    }
}

impl std::ops::Deref for StagedCheckpoint {
    type Target = Checkpoint;

    fn deref(&self) -> &Checkpoint {
        &self.checkpoint
    }
}
