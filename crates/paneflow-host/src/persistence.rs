use std::collections::{BTreeMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionGeneration, SessionId};
use serde::Serialize;

use crate::manifest::{self, SessionManifest};

pub const QUEUE_BUDGET_BYTES: usize = 8 * 1024 * 1024;

pub const FINAL_RESERVE_BYTES: usize = 16 * 1024;

pub const CRITICAL_DEADLINE: Duration = Duration::from_secs(5);

pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

pub const METADATA_FLUSH_BOUND: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteClass {
    Metadata,
    Critical,
    Final,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PersistError {
    #[error("the persistence queue is full ({queued} of {budget} bytes queued)")]
    QueueFull { queued: usize, budget: usize },
    #[error("persistence did not complete within {0:?}; the revision remains queued")]
    Timeout(Duration),
    #[error("the persistence service has stopped")]
    Stopped,
    #[error("{0}")]
    Storage(String),
}

#[derive(Default)]
pub struct SessionPersistence {
    revision: AtomicU64,
    written: AtomicU64,
    removed: AtomicBool,
    reserved: AtomicBool,
    error: Mutex<Option<String>>,
}

impl SessionPersistence {
    pub fn error(&self) -> Option<String> {
        self.error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn clear_error(&self) {
        *self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    fn set_error(&self, error: Option<String>) {
        *self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = error;
    }

    pub fn next_revision(&self) -> u64 {
        self.revision.fetch_add(1, Ordering::AcqRel) + 1
    }

    pub fn written_revision(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    pub fn mark_removed(&self) {
        self.removed.store(true, Ordering::Release);
    }

    pub fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Acquire)
    }
}

pub struct ManifestRevision {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub revision: u64,
    pub manifest: Vec<u8>,
    pub seed: Option<Vec<u8>>,
    pub state: Arc<SessionPersistence>,
    pub class: WriteClass,
}

impl ManifestRevision {
    pub fn encode(
        manifest: &SessionManifest,
        revision: u64,
        state: &Arc<SessionPersistence>,
        class: WriteClass,
    ) -> Result<Self, PersistError> {
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| PersistError::Storage(error.to_string()))?;
        if bytes.len() as u64 > manifest::MAX_MANIFEST_BYTES {
            return Err(PersistError::Storage(
                "session manifest exceeds its reload size limit".to_string(),
            ));
        }
        let seed = manifest.last_hook.as_ref().map(|hook| {
            manifest::encode_hook_seed(
                &hook.hook_event_name,
                hook.tool_name.as_deref(),
                manifest.generation,
                manifest.hook_revision,
            )
        });
        Ok(Self {
            session: manifest.session.clone(),
            generation: manifest.generation,
            revision,
            manifest: bytes,
            seed,
            state: Arc::clone(state),
            class,
        })
    }

    fn bytes(&self) -> usize {
        self.manifest.len() + self.seed.as_ref().map_or(0, Vec::len)
    }
}

type Completion = SyncSender<Result<(), PersistError>>;

type Exclusive = Box<dyn FnOnce() -> Result<(), PersistError> + Send + 'static>;

enum Job {
    Manifest(ManifestRevision, Option<Completion>),
    Remove {
        session: SessionId,
        state: Arc<SessionPersistence>,
        done: Option<Completion>,
    },
    Exclusive(Exclusive, Option<Completion>),
    Barrier(Completion),
}

impl Job {
    fn bytes(&self) -> usize {
        match self {
            Job::Manifest(record, _) => record.bytes(),
            _ => 0,
        }
    }
}

#[derive(Default)]
struct Queue {
    ordered: VecDeque<Job>,
    metadata: BTreeMap<SessionId, ManifestRevision>,
    metadata_order: VecDeque<SessionId>,
    pending_final: BTreeMap<SessionId, ManifestRevision>,
    queued_bytes: usize,
    reserved_bytes: usize,
    stopped: bool,
    peak_queued_bytes: usize,
    metadata_rejected: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct QueueReport {
    pub queued_bytes: usize,
    pub peak_queued_bytes: usize,
    pub budget_bytes: usize,
    pub reserved_bytes: usize,
    pub queued_jobs: usize,
    pub pending_final_revisions: usize,
    pub metadata_rejected: u64,
}

struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

pub struct Persistence {
    shared: Arc<Shared>,
}

impl Persistence {
    pub fn start(home: &Path) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            queue: Mutex::new(Queue::default()),
            wake: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let worker_home = home.to_path_buf();
        std::thread::Builder::new()
            .name("paneflow-host-persist".into())
            .spawn(move || writer_loop(&worker_home, &worker))?;
        Ok(Self { shared })
    }

    pub fn report(&self) -> QueueReport {
        let queue = self.lock();
        QueueReport {
            queued_bytes: queue.queued_bytes,
            peak_queued_bytes: queue.peak_queued_bytes,
            budget_bytes: QUEUE_BUDGET_BYTES,
            reserved_bytes: queue.reserved_bytes,
            queued_jobs: queue.ordered.len() + queue.metadata.len(),
            pending_final_revisions: queue.pending_final.len(),
            metadata_rejected: queue.metadata_rejected,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queue> {
        self.shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn reserve_final(&self, state: &SessionPersistence) {
        if !state.reserved.swap(true, Ordering::AcqRel) {
            self.lock().reserved_bytes += FINAL_RESERVE_BYTES;
        }
    }

    pub fn release_final(&self, state: &SessionPersistence) {
        if state.reserved.swap(false, Ordering::AcqRel) {
            let mut queue = self.lock();
            queue.reserved_bytes = queue.reserved_bytes.saturating_sub(FINAL_RESERVE_BYTES);
        }
    }

    pub fn submit(&self, record: ManifestRevision) -> Result<(), PersistError> {
        let mut queue = self.lock();
        if queue.stopped {
            return Err(PersistError::Stopped);
        }
        let size = record.bytes();
        match record.class {
            WriteClass::Metadata => {
                if let Some(previous) = queue.metadata.remove(&record.session) {
                    queue.queued_bytes = queue.queued_bytes.saturating_sub(previous.bytes());
                } else {
                    let admitted = queue
                        .queued_bytes
                        .saturating_add(queue.reserved_bytes)
                        .saturating_add(size)
                        <= QUEUE_BUDGET_BYTES;
                    if !admitted {
                        queue.metadata_rejected += 1;
                        let queued = queue.queued_bytes;
                        drop(queue);
                        let error = PersistError::QueueFull {
                            queued,
                            budget: QUEUE_BUDGET_BYTES,
                        };
                        record.state.set_error(Some(error.to_string()));
                        return Err(error);
                    }
                    queue.metadata_order.push_back(record.session.clone());
                }
                queue.queued_bytes += size;
                queue.metadata.insert(record.session.clone(), record);
            }
            WriteClass::Critical => {
                let admitted = queue.queued_bytes.saturating_add(size) <= QUEUE_BUDGET_BYTES;
                if !admitted {
                    let queued = queue.queued_bytes;
                    drop(queue);
                    let error = PersistError::QueueFull {
                        queued,
                        budget: QUEUE_BUDGET_BYTES,
                    };
                    record.state.set_error(Some(error.to_string()));
                    return Err(error);
                }
                queue.queued_bytes += size;
                queue.ordered.push_back(Job::Manifest(record, None));
            }
            WriteClass::Final => {
                let admitted = queue.queued_bytes.saturating_add(size) <= QUEUE_BUDGET_BYTES;
                if !admitted {
                    record.state.set_error(Some(
                        "final state is queued for retry: the persistence queue is full"
                            .to_string(),
                    ));
                    retain_pending_final(&mut queue, record);
                } else {
                    queue.queued_bytes += size;
                    queue.ordered.push_back(Job::Manifest(record, None));
                }
            }
        }
        queue.peak_queued_bytes = queue.peak_queued_bytes.max(queue.queued_bytes);
        drop(queue);
        self.shared.wake.notify_one();
        Ok(())
    }

    pub fn submit_and_wait(
        &self,
        record: ManifestRevision,
        deadline: Duration,
    ) -> Result<(), PersistError> {
        let (tx, rx) = sync_channel(1);
        {
            let mut queue = self.lock();
            if queue.stopped {
                return Err(PersistError::Stopped);
            }
            let size = record.bytes();
            let admitted = queue.queued_bytes.saturating_add(size) <= QUEUE_BUDGET_BYTES;
            if !admitted {
                let queued = queue.queued_bytes;
                drop(queue);
                let error = PersistError::QueueFull {
                    queued,
                    budget: QUEUE_BUDGET_BYTES,
                };
                record.state.set_error(Some(error.to_string()));
                return Err(error);
            }
            queue.queued_bytes += size;
            queue.peak_queued_bytes = queue.peak_queued_bytes.max(queue.queued_bytes);
            queue.ordered.push_back(Job::Manifest(record, Some(tx)));
        }
        self.shared.wake.notify_one();
        wait(rx, deadline)
    }

    pub fn remove_and_wait(
        &self,
        session: &SessionId,
        state: &Arc<SessionPersistence>,
        deadline: Duration,
    ) -> Result<(), PersistError> {
        let (tx, rx) = sync_channel(1);
        self.enqueue_remove(session, state, Some(tx))?;
        wait(rx, deadline)
    }

    pub fn remove(&self, session: &SessionId, state: &Arc<SessionPersistence>) {
        let _ = self.enqueue_remove(session, state, None);
    }

    fn enqueue_remove(
        &self,
        session: &SessionId,
        state: &Arc<SessionPersistence>,
        done: Option<Completion>,
    ) -> Result<(), PersistError> {
        state.mark_removed();
        {
            let mut queue = self.lock();
            if queue.stopped {
                return Err(PersistError::Stopped);
            }
            if let Some(previous) = queue.metadata.remove(session) {
                queue.queued_bytes = queue.queued_bytes.saturating_sub(previous.bytes());
            }
            queue.pending_final.remove(session);
            queue.ordered.push_back(Job::Remove {
                session: session.clone(),
                state: Arc::clone(state),
                done,
            });
        }
        self.release_final(state);
        self.shared.wake.notify_one();
        Ok(())
    }

    pub fn run_exclusive<T: Send + 'static>(
        &self,
        deadline: Duration,
        job: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PersistError> {
        let (tx, rx) = sync_channel(1);
        let (value_tx, value_rx) = sync_channel(1);
        {
            let mut queue = self.lock();
            if queue.stopped {
                return Err(PersistError::Stopped);
            }
            queue.ordered.push_back(Job::Exclusive(
                Box::new(move || {
                    let _ = value_tx.send(job());
                    Ok(())
                }),
                Some(tx),
            ));
        }
        self.shared.wake.notify_one();
        wait(rx, deadline)?;
        value_rx.try_recv().map_err(|_| PersistError::Stopped)
    }

    pub fn spawn_exclusive(&self, job: impl FnOnce() -> Result<(), PersistError> + Send + 'static) {
        {
            let mut queue = self.lock();
            if queue.stopped {
                return;
            }
            queue.ordered.push_back(Job::Exclusive(Box::new(job), None));
        }
        self.shared.wake.notify_one();
    }

    pub fn drain(&self, deadline: Duration) -> Result<(), Vec<String>> {
        let started = Instant::now();
        {
            let mut queue = self.lock();
            let pending: Vec<_> = std::mem::take(&mut queue.pending_final)
                .into_values()
                .collect();
            for record in pending {
                queue.queued_bytes += record.bytes();
                queue.ordered.push_back(Job::Manifest(record, None));
            }
        }
        self.shared.wake.notify_one();
        let (tx, rx) = sync_channel(1);
        {
            let mut queue = self.lock();
            if queue.stopped {
                return Err(vec![PersistError::Stopped.to_string()]);
            }
            queue.ordered.push_back(Job::Barrier(tx));
        }
        self.shared.wake.notify_one();
        let remaining = deadline.saturating_sub(started.elapsed());
        if let Err(error) = wait(rx, remaining) {
            return Err(vec![error.to_string()]);
        }
        let queue = self.lock();
        let failures: Vec<String> = queue
            .pending_final
            .values()
            .map(|record| {
                format!(
                    "session {} generation {} revision {} is not durable: {}",
                    record.session,
                    record.generation,
                    record.revision,
                    record.state.error().unwrap_or_default()
                )
            })
            .collect();
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }
}

impl Drop for Persistence {
    fn drop(&mut self) {
        self.lock().stopped = true;
        self.shared.wake.notify_all();
    }
}

fn wait(
    rx: std::sync::mpsc::Receiver<Result<(), PersistError>>,
    deadline: Duration,
) -> Result<(), PersistError> {
    match rx.recv_timeout(deadline) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => Err(PersistError::Timeout(deadline)),
        Err(RecvTimeoutError::Disconnected) => Err(PersistError::Stopped),
    }
}

fn retain_pending_final(queue: &mut Queue, record: ManifestRevision) {
    let newer_pending = queue
        .pending_final
        .get(&record.session)
        .is_some_and(|held| held.revision >= record.revision);
    if !newer_pending {
        queue.pending_final.insert(record.session.clone(), record);
    }
}

fn writer_loop(home: &Path, shared: &Arc<Shared>) {
    let mut prefer_metadata = false;
    loop {
        let next = {
            let mut queue = shared
                .queue
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            loop {
                if let Some(job) = take_next(&mut queue, prefer_metadata) {
                    prefer_metadata = !prefer_metadata;
                    break Some(job);
                }
                if queue.stopped {
                    break None;
                }
                let has_retries = !queue.pending_final.is_empty();
                if has_retries {
                    let (guard, timeout) = shared
                        .wake
                        .wait_timeout(queue, RETRY_INTERVAL)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    queue = guard;
                    if timeout.timed_out() {
                        let pending: Vec<_> = std::mem::take(&mut queue.pending_final)
                            .into_values()
                            .collect();
                        for record in pending {
                            queue.queued_bytes += record.bytes();
                            queue.ordered.push_back(Job::Manifest(record, None));
                        }
                    }
                } else {
                    queue = shared
                        .wake
                        .wait(queue)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        };
        let Some(job) = next else {
            return;
        };
        let bytes = job.bytes();
        let outcome = execute(home, job);
        let mut queue = shared
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.queued_bytes = queue.queued_bytes.saturating_sub(bytes);
        if let Some(record) = outcome {
            retain_pending_final(&mut queue, record);
        }
    }
}

fn take_next(queue: &mut Queue, prefer_metadata: bool) -> Option<Job> {
    let metadata = |queue: &mut Queue| {
        while let Some(session) = queue.metadata_order.pop_front() {
            if let Some(record) = queue.metadata.remove(&session) {
                return Some(Job::Manifest(record, None));
            }
        }
        None
    };
    let barrier_next = matches!(queue.ordered.front(), Some(Job::Barrier(_)));
    if prefer_metadata || barrier_next {
        metadata(queue).or_else(|| queue.ordered.pop_front())
    } else {
        queue.ordered.pop_front().or_else(|| metadata(queue))
    }
}

fn execute(home: &Path, job: Job) -> Option<ManifestRevision> {
    match job {
        Job::Manifest(record, done) => {
            let result = write_revision(home, &record);
            let retained = match &result {
                Ok(()) => {
                    record.state.set_error(None);
                    None
                }
                Err(error) => {
                    record.state.set_error(Some(error.to_string()));
                    (record.class == WriteClass::Final).then_some(record)
                }
            };
            if let Some(done) = done {
                let _ = done.send(result);
            }
            retained
        }
        Job::Remove {
            session,
            state,
            done,
        } => {
            let path = manifest::manifest_path(home, &session);
            let result = match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(PersistError::Storage(format!(
                    "cannot delete the manifest of {session}: {error}"
                ))),
            };
            manifest::remove_session_data(home, &session);
            state.set_error(result.as_ref().err().map(ToString::to_string));
            if let Some(done) = done {
                let _ = done.send(result);
            }
            None
        }
        Job::Exclusive(run, done) => {
            let result = run();
            if let Some(done) = done {
                let _ = done.send(result);
            }
            None
        }
        Job::Barrier(done) => {
            let _ = done.send(Ok(()));
            None
        }
    }
}

fn write_revision(home: &Path, record: &ManifestRevision) -> Result<(), PersistError> {
    if record.state.is_removed() {
        return Ok(());
    }
    if record.revision < record.state.written_revision() {
        return Ok(());
    }
    let durable = record.class != WriteClass::Metadata;
    manifest::write_manifest_bytes(home, &record.session, &record.manifest, durable).map_err(
        |error| {
            PersistError::Storage(format!(
                "cannot write the manifest of {}: {error}",
                record.session
            ))
        },
    )?;
    if let Some(seed) = &record.seed {
        manifest::write_hook_seed(home, &record.session, seed, durable)
            .map_err(|error| PersistError::Storage(format!("hook seed: {error}")))?;
    }
    record
        .state
        .written
        .fetch_max(record.revision, Ordering::AcqRel);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{MANIFEST_SCHEMA_VERSION, SessionLaunch, SessionLifecycle};
    use paneflow_config::schema::HostInstanceToken;

    fn sample(session: &SessionId, title: &str) -> SessionManifest {
        SessionManifest {
            schema: MANIFEST_SCHEMA_VERSION,
            session: session.clone(),
            workspace: None,
            generation: SessionGeneration::FIRST,
            host_instance: HostInstanceToken::new(),
            cwd: "/work".to_string(),
            launch: SessionLaunch {
                shell: "/bin/sh".to_string(),
                args: vec![],
                env: BTreeMap::new(),
                cols: 80,
                rows: 24,
            },
            lifecycle: SessionLifecycle::Running,
            process: None,
            title: Some(title.to_string()),
            current_cwd: None,
            last_hook: None,
            hook_revision: 0,
            generation_started_at_ms: None,
            screen_changed_at_ms: None,
            screen_activity: None,
            menu_prompt_active: false,
            runtime: None,
            final_output: None,
            host_protocol_version: crate::protocol::HOST_PROTOCOL_VERSION,
            host_build_id: crate::protocol::host_build_id(),
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn encode(
        manifest: &SessionManifest,
        state: &Arc<SessionPersistence>,
        class: WriteClass,
    ) -> ManifestRevision {
        ManifestRevision::encode(manifest, state.next_revision(), state, class).unwrap()
    }

    fn stored_title(home: &Path, session: &SessionId) -> Option<String> {
        manifest::read_manifest(&manifest::manifest_path(home, session))
            .ok()
            .and_then(|manifest| manifest.title)
    }

    #[test]
    fn metadata_revisions_coalesce_to_the_latest_and_a_critical_barrier_completes() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        for index in 0..200 {
            let record = encode(
                &sample(&session, &format!("title {index}")),
                &state,
                WriteClass::Metadata,
            );
            service.submit(record).unwrap();
        }
        let record = encode(
            &sample(&session, "final title"),
            &state,
            WriteClass::Critical,
        );
        service.submit_and_wait(record, CRITICAL_DEADLINE).unwrap();
        assert_eq!(
            stored_title(home.path(), &session).as_deref(),
            Some("final title")
        );
        let report = service.report();
        assert_eq!(report.queued_bytes, 0);
        assert!(report.peak_queued_bytes <= 2 * 64 * 1024);
        assert_eq!(state.error(), None);
    }

    #[test]
    fn an_older_revision_queued_behind_a_newer_one_never_regresses_the_file() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let older = encode(&sample(&session, "older"), &state, WriteClass::Critical);
        let newer = encode(&sample(&session, "newer"), &state, WriteClass::Critical);
        service.submit(newer).unwrap();
        service.submit_and_wait(older, CRITICAL_DEADLINE).unwrap();
        assert_eq!(
            stored_title(home.path(), &session).as_deref(),
            Some("newer")
        );
        assert_eq!(state.written_revision(), 2);
    }

    #[test]
    fn a_revision_queued_before_removal_cannot_resurrect_the_record() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let data_dir = paneflow_home::host_session_data_dir_in(home.path(), session.as_str());
        std::fs::create_dir_all(&data_dir).unwrap();
        let first = encode(&sample(&session, "first"), &state, WriteClass::Critical);
        service.submit_and_wait(first, CRITICAL_DEADLINE).unwrap();
        let late = encode(&sample(&session, "late"), &state, WriteClass::Metadata);
        service
            .remove_and_wait(&session, &state, CRITICAL_DEADLINE)
            .unwrap();
        service.submit(late).unwrap();
        let after = encode(&sample(&session, "after"), &state, WriteClass::Final);
        service.submit(after).unwrap();
        service.drain(CRITICAL_DEADLINE).unwrap();
        assert!(!manifest::manifest_path(home.path(), &session).exists());
        assert!(!data_dir.exists());
    }

    #[test]
    fn a_failed_final_revision_is_retained_and_retried_after_storage_recovers() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let path = manifest::manifest_path(home.path(), &session);
        std::fs::create_dir_all(&path).unwrap();
        let record = encode(&sample(&session, "final"), &state, WriteClass::Final);
        service.submit(record).unwrap();
        let failures = service.drain(CRITICAL_DEADLINE).unwrap_err();
        assert_eq!(failures.len(), 1);
        assert!(state.error().is_some());
        assert_eq!(service.report().pending_final_revisions, 1);
        std::fs::remove_dir(&path).unwrap();
        service.drain(CRITICAL_DEADLINE).unwrap();
        assert_eq!(
            stored_title(home.path(), &session).as_deref(),
            Some("final")
        );
        assert_eq!(state.error(), None);
        assert_eq!(service.report().pending_final_revisions, 0);
    }

    #[test]
    fn a_stalled_exclusive_job_times_out_the_waiter_without_losing_the_queue() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        service.spawn_exclusive(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
            Ok(())
        });
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let record = encode(
            &sample(&session, "after stall"),
            &state,
            WriteClass::Critical,
        );
        let stalled = service.submit_and_wait(record, Duration::from_millis(100));
        assert!(matches!(stalled, Err(PersistError::Timeout(_))));
        release_tx.send(()).unwrap();
        service.drain(CRITICAL_DEADLINE).unwrap();
        assert_eq!(
            stored_title(home.path(), &session).as_deref(),
            Some("after stall")
        );
    }

    #[test]
    fn metadata_admission_respects_the_byte_budget_and_reservations() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        service.spawn_exclusive(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(30));
            Ok(())
        });
        let mut states = Vec::new();
        let mut rejected = 0;
        for _ in 0..(QUEUE_BUDGET_BYTES / FINAL_RESERVE_BYTES - 1) {
            let session = SessionId::new();
            let state = Arc::new(SessionPersistence::default());
            service.reserve_final(&state);
            let record = encode(&sample(&session, "queued"), &state, WriteClass::Metadata);
            if service.submit(record).is_err() {
                rejected += 1;
            }
            states.push(state);
        }
        let report = service.report();
        assert!(rejected > 0, "the budget was never reached: {report:?}");
        assert!(report.queued_bytes <= QUEUE_BUDGET_BYTES);
        assert_eq!(report.metadata_rejected, rejected);
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let record = encode(
            &sample(&session, "over budget"),
            &state,
            WriteClass::Metadata,
        );
        assert!(matches!(
            service.submit(record),
            Err(PersistError::QueueFull { .. })
        ));
        assert!(state.error().is_some());
        release_tx.send(()).unwrap();
        service.drain(Duration::from_secs(30)).unwrap();
        assert_eq!(service.report().queued_bytes, 0);
    }

    #[test]
    fn a_metadata_write_starts_within_the_flush_bound() {
        let home = tempfile::tempdir().unwrap();
        let service = Persistence::start(home.path()).unwrap();
        let session = SessionId::new();
        let state = Arc::new(SessionPersistence::default());
        let started = Instant::now();
        let record = encode(&sample(&session, "prompt"), &state, WriteClass::Metadata);
        service.submit(record).unwrap();
        while stored_title(home.path(), &session).is_none() {
            assert!(
                started.elapsed() < METADATA_FLUSH_BOUND,
                "the flush did not start within {METADATA_FLUSH_BOUND:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}
