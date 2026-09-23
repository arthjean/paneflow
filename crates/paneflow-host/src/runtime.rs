use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use paneflow_config::schema::SessionGeneration;
use paneflow_terminal_ghostty as ghostty;
use portable_pty::{CommandBuilder, PtySize};

use crate::process::ProcessIdentity;
use crate::protocol::{MAX_CHECKPOINT_BYTES, MAX_OUTPUT_TAIL_BYTES, REQUEST_DEADLINE};
use crate::stream::OutputStream;

const READ_CHUNK_BYTES: usize = 32 * 1024;
const PTY_INBOX_BYTES: usize = 2 * 1024 * 1024;
const DRAIN_SLICE_BYTES: usize = 64 * 1024;
const CONTROL_QUEUE_SLOTS: usize = 64;
const INPUT_QUEUE_SLOTS: usize = 64;
pub const MAX_INPUT_QUEUE_BYTES: usize = 2 * 1024 * 1024;
pub const PTY_INBOX_CHUNK_SLOTS: usize = 64;
const QUEUE_RETRY: Duration = Duration::from_millis(5);
const PROCESS_SCAN_INTERVAL: Duration = Duration::from_millis(500);
const DESCENDANT_RECONCILE_MIN: Duration = Duration::from_millis(100);
const DESCENDANT_RECONCILE_MAX: Duration = Duration::from_secs(1);
#[cfg(target_os = "linux")]
const ROOT_EXIT_DISCOVERY_DEADLINE: Duration = Duration::from_secs(2);
const UNATTENDED_STOP_RETRY: Duration = Duration::from_secs(1);
#[cfg(unix)]
const FORCE_SIGNAL_AFTER: Duration = Duration::from_millis(100);
#[cfg(windows)]
const TREE_TERMINATE_WAIT: Duration = Duration::from_millis(250);
pub const FINAL_DRAIN_BUDGET: Duration = Duration::from_secs(2);
const TAIL_RELEASE_GRACE: Duration = Duration::from_secs(1);
pub const FINAL_TEXT_MAX_BYTES: usize = 512 * 1024;
pub const STARTUP_DEADLINE: Duration = Duration::from_secs(10);
pub const STOP_BUDGET: Duration = Duration::from_secs(5);
const CELL_WIDTH_PX: u32 = 8;
const CELL_HEIGHT_PX: u32 = 16;
const TERMINFO_NAME: &str = "xterm-256color";
const SCROLLBACK_BYTES_PER_LINE: usize = 1024;
const MAX_SCROLLBACK_BYTES: usize = 128 * 1024 * 1024;
pub const CONTINUATION_MAX_BYTES: usize = 64 * 1024;
const NEWLINE: &str = "\n";
const NEWLINE_CHAR: char = '\n';
const RUNTIME_PANIC_REASON: &str =
    "the session runtime panicked; its child process is held for recovery";
const LATE_LAUNCH_CANCELLED: &str = "the launch was cancelled before its process was published";
const EXIT_UNOBSERVED: &str = "the child exit status was not observed";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub shell: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub cols: u16,
    pub rows: u16,
    pub scrollback_lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitOutcome {
    pub code: i64,
    pub signal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedRecord {
    pub generation: SessionGeneration,
    pub exit: ExitOutcome,
    pub final_offset: u64,
    pub complete: bool,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeNotice {
    Title(String),
    WorkingDirectory(String),
    Exited(ExitOutcome),
    Unverified(String),
    Completed(CompletedRecord),
}

pub type RuntimeObserver = Arc<dyn Fn(RuntimeNotice) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub generation: SessionGeneration,
    pub offset: u64,
    pub cols: u16,
    pub rows: u16,
    pub snapshot: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewportScan {
    pub screen: String,
    pub foreground_process_group: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSlice {
    pub offset: u64,
    pub data: Vec<u8>,
    pub end_offset: u64,
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StopReport {
    pub exit: Option<ExitOutcome>,
    pub descendants_unresolved: usize,
    pub unverified: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    #[error("the session runtime is gone")]
    Gone,
    #[error("the session did not answer within {0:?}")]
    Deadline(Duration),
    #[error("the session is not running")]
    NotLive,
    #[error("the session process could not be verified: {0}")]
    Unverified(String),
    #[error("terminal state of {bytes} bytes exceeds the {limit}-byte attachment limit")]
    CheckpointTooLarge { bytes: usize, limit: usize },
    #[error("output offset {requested} was evicted; the tail now covers {tail_start}..{tail_end}")]
    OutputEvicted {
        requested: u64,
        tail_start: u64,
        tail_end: u64,
    },
    #[error("terminal engine error: {0}")]
    Engine(String),
    #[error("pty error: {0}")]
    Pty(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SpawnError(pub String);

enum Command {
    Checkpoint(SyncSender<Result<Checkpoint, RuntimeError>>),
    CheckpointSize(SyncSender<Result<usize, RuntimeError>>),
    Text(SyncSender<Result<String, RuntimeError>>),
    Viewport(SyncSender<Result<ViewportScan, RuntimeError>>),
    BracketedPaste(SyncSender<Result<bool, RuntimeError>>),
    Input(Vec<u8>, SyncSender<Result<usize, RuntimeError>>),
    Resize {
        cols: u16,
        rows: u16,
        reply: SyncSender<Result<(), RuntimeError>>,
    },
    Stop {
        deadline: Instant,
        reply: SyncSender<StopReport>,
    },
    #[cfg(test)]
    InjectPanic,
}

enum Message {
    OutputReady,
    Eof,
    ChildExited(Result<ExitOutcome, String>),
    #[cfg(target_os = "linux")]
    RootExiting(SyncSender<()>),
    Command(Command),
}

#[derive(Default)]
struct InboxState {
    chunks: VecDeque<Vec<u8>>,
    bytes: usize,
    notified: bool,
    closed: bool,
}

#[derive(Default)]
struct PtyInbox {
    state: Mutex<InboxState>,
    drained: Condvar,
}

impl PtyInbox {
    fn push(&self, chunk: Vec<u8>, tx: &SyncSender<Message>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !state.closed
            && state.bytes + chunk.len() > PTY_INBOX_BYTES
            && !state.chunks.is_empty()
        {
            state = self
                .drained
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        if state.closed {
            return true;
        }
        state.bytes += chunk.len();
        state.chunks.push_back(chunk);
        let notify = !state.notified;
        state.notified = true;
        drop(state);
        !notify || tx.send(Message::OutputReady).is_ok()
    }

    fn usage(&self) -> (usize, usize) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (state.bytes, state.chunks.len())
    }

    fn take_slice(&self, max_bytes: usize) -> (VecDeque<Vec<u8>>, bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut taken = VecDeque::new();
        let mut taken_bytes = 0usize;
        while let Some(chunk) = state.chunks.front() {
            if taken_bytes > 0 && taken_bytes + chunk.len() > max_bytes {
                break;
            }
            taken_bytes += chunk.len();
            let chunk = state.chunks.pop_front().unwrap_or_default();
            taken.push_back(chunk);
        }
        state.bytes = state.bytes.saturating_sub(taken_bytes);
        let more = !state.chunks.is_empty();
        state.notified = more;
        drop(state);
        self.drained.notify_all();
        (taken, more)
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        state.chunks.clear();
        state.bytes = 0;
        drop(state);
        self.drained.notify_all();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct RuntimeResources {
    pub tail_retained_bytes: usize,
    pub tail_allocated_bytes: usize,
    pub tail_budget_bytes: usize,
    pub inbox_bytes: usize,
    pub inbox_chunks: usize,
    pub inbox_budget_bytes: usize,
    pub input_queued_bytes: usize,
    pub input_budget_bytes: usize,
}

struct Shared {
    exit: Mutex<Option<ExitOutcome>>,
    unverified: Mutex<Option<String>>,
    descendants_unresolved: AtomicUsize,
    input_queued_bytes: AtomicUsize,
    stop_requested: AtomicBool,
    retired: AtomicBool,
    output_changed_at_ms: AtomicU64,
    stream: Arc<OutputStream>,
    inbox: PtyInbox,
}

impl Shared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            exit: Mutex::new(None),
            unverified: Mutex::new(None),
            descendants_unresolved: AtomicUsize::new(0),
            input_queued_bytes: AtomicUsize::new(0),
            stop_requested: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            output_changed_at_ms: AtomicU64::new(0),
            stream: OutputStream::new(MAX_OUTPUT_TAIL_BYTES),
            inbox: PtyInbox::default(),
        })
    }

    fn exit(&self) -> Option<ExitOutcome> {
        self.exit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn unverified(&self) -> Option<String> {
        self.unverified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_unverified(&self, reason: Option<String>) {
        *self
            .unverified
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = reason;
    }
}

pub struct SessionRuntime {
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
    generation: SessionGeneration,
    process: ProcessIdentity,
}

pub struct LaunchHandle {
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
    generation: SessionGeneration,
    startup_rx: Receiver<Result<ProcessIdentity, String>>,
}

pub enum LaunchWait {
    Ready(SessionRuntime),
    Recovery(SessionRuntime),
    Failed(String),
    Pending(LaunchHandle),
}

#[derive(Clone)]
pub struct LaunchCancel(Arc<Shared>);

impl LaunchCancel {
    pub fn cancel(&self) {
        self.0.stop_requested.store(true, Ordering::Release);
    }
}

impl LaunchHandle {
    pub fn wait(self, budget: Duration) -> LaunchWait {
        match self.startup_rx.recv_timeout(budget) {
            Ok(Ok(process)) => {
                if self.shared.stop_requested.load(Ordering::Acquire) {
                    self.shared
                        .set_unverified(Some(LATE_LAUNCH_CANCELLED.into()));
                }
                let runtime = SessionRuntime {
                    tx: self.tx,
                    shared: self.shared,
                    generation: self.generation,
                    process,
                };
                if runtime.unverified().is_some() {
                    LaunchWait::Recovery(runtime)
                } else {
                    LaunchWait::Ready(runtime)
                }
            }
            Ok(Err(reason)) => LaunchWait::Failed(reason),
            Err(RecvTimeoutError::Timeout) => LaunchWait::Pending(self),
            Err(RecvTimeoutError::Disconnected) => {
                LaunchWait::Failed("the session thread ended before reporting its start".into())
            }
        }
    }

    pub fn cancel(&self) {
        self.shared.stop_requested.store(true, Ordering::Release);
    }

    pub fn canceller(&self) -> LaunchCancel {
        LaunchCancel(Arc::clone(&self.shared))
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }
}

impl SessionRuntime {
    pub fn launch(
        spec: SpawnSpec,
        generation: SessionGeneration,
        observer: RuntimeObserver,
    ) -> Result<LaunchHandle, SpawnError> {
        let (tx, rx) = sync_channel::<Message>(CONTROL_QUEUE_SLOTS);
        let (startup_tx, startup_rx) = sync_channel::<Result<ProcessIdentity, String>>(1);
        let shared = Shared::new();
        let thread_shared = Arc::clone(&shared);
        let thread_tx = tx.clone();
        std::thread::Builder::new()
            .name("paneflow-host-session".into())
            .spawn(move || {
                run(
                    spec,
                    generation,
                    thread_tx,
                    rx,
                    thread_shared,
                    observer,
                    startup_tx,
                )
            })
            .map_err(|e| SpawnError(format!("could not start the session thread: {e}")))?;
        Ok(LaunchHandle {
            tx,
            shared,
            generation,
            startup_rx,
        })
    }

    pub fn spawn(
        spec: SpawnSpec,
        generation: SessionGeneration,
        observer: RuntimeObserver,
    ) -> Result<Self, SpawnError> {
        match Self::launch(spec, generation, observer)?.wait(STARTUP_DEADLINE) {
            LaunchWait::Ready(runtime) => Ok(runtime),
            LaunchWait::Recovery(runtime) => {
                let reason = runtime
                    .unverified()
                    .unwrap_or_else(|| "startup needs recovery".into());
                let _ = runtime.stop();
                Err(SpawnError(reason))
            }
            LaunchWait::Failed(reason) => Err(SpawnError(reason)),
            LaunchWait::Pending(handle) => {
                handle.cancel();
                Err(SpawnError("the session did not start in time".to_string()))
            }
        }
    }

    pub fn generation(&self) -> SessionGeneration {
        self.generation
    }

    pub fn process(&self) -> ProcessIdentity {
        self.process
    }

    pub fn exit(&self) -> Option<ExitOutcome> {
        self.shared.exit()
    }

    pub fn unverified(&self) -> Option<String> {
        self.shared.unverified()
    }

    pub fn descendants_unresolved(&self) -> usize {
        self.shared.descendants_unresolved.load(Ordering::Acquire)
    }

    pub fn output_changed_at_ms(&self) -> Option<u64> {
        let changed_at = self.shared.output_changed_at_ms.load(Ordering::Acquire);
        (changed_at != 0).then_some(changed_at)
    }

    pub fn is_live(&self) -> bool {
        self.shared.exit().is_none() && self.shared.unverified().is_none()
    }

    pub fn owns_process(&self) -> bool {
        self.shared.exit().is_none() || self.descendants_unresolved() > 0
    }

    pub fn retired(&self) -> bool {
        self.shared.retired.load(Ordering::Acquire)
    }

    pub fn stream(&self) -> Arc<OutputStream> {
        Arc::clone(&self.shared.stream)
    }

    fn send_bounded(&self, message: Message, budget: Duration) -> Result<(), RuntimeError> {
        use std::sync::mpsc::TrySendError;
        let deadline = Instant::now() + budget;
        let mut message = message;
        loop {
            match self.tx.try_send(message) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Disconnected(_)) => return Err(RuntimeError::Gone),
                Err(TrySendError::Full(returned)) => {
                    if Instant::now() >= deadline {
                        return Err(RuntimeError::Deadline(budget));
                    }
                    message = returned;
                    std::thread::sleep(QUEUE_RETRY);
                }
            }
        }
    }

    fn ask<T>(
        &self,
        build: impl FnOnce(SyncSender<Result<T, RuntimeError>>) -> Command,
    ) -> Result<T, RuntimeError> {
        self.ask_within(REQUEST_DEADLINE, build)
    }

    fn ask_within<T>(
        &self,
        budget: Duration,
        build: impl FnOnce(SyncSender<Result<T, RuntimeError>>) -> Command,
    ) -> Result<T, RuntimeError> {
        if self.retired() {
            return Err(match self.unverified() {
                Some(reason) => RuntimeError::Unverified(reason),
                None => RuntimeError::NotLive,
            });
        }
        let (reply_tx, reply_rx) = sync_channel(1);
        self.send_bounded(Message::Command(build(reply_tx)), budget)?;
        match reply_rx.recv_timeout(budget) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(RuntimeError::Deadline(budget)),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Gone),
        }
    }

    pub fn checkpoint(&self) -> Result<Checkpoint, RuntimeError> {
        self.ask(Command::Checkpoint)
    }

    pub fn checkpoint_size(&self) -> Result<usize, RuntimeError> {
        self.ask(Command::CheckpointSize)
    }

    pub fn resources(&self) -> RuntimeResources {
        let stream = self.shared.stream.status();
        let (inbox_bytes, inbox_chunks) = self.shared.inbox.usage();
        RuntimeResources {
            tail_retained_bytes: stream.retained_bytes,
            tail_allocated_bytes: stream.allocated_bytes,
            tail_budget_bytes: MAX_OUTPUT_TAIL_BYTES,
            inbox_bytes,
            inbox_chunks,
            inbox_budget_bytes: PTY_INBOX_BYTES,
            input_queued_bytes: self.shared.input_queued_bytes.load(Ordering::Acquire),
            input_budget_bytes: MAX_INPUT_QUEUE_BYTES,
        }
    }

    pub fn text(&self) -> Result<String, RuntimeError> {
        self.ask(Command::Text)
    }

    pub fn viewport_scan(&self, budget: Duration) -> Result<ViewportScan, RuntimeError> {
        self.ask_within(budget, Command::Viewport)
    }

    pub fn bracketed_paste_enabled(&self) -> Result<bool, RuntimeError> {
        self.ask(Command::BracketedPaste)
    }

    pub fn output(&self, from: u64, max: usize) -> Result<OutputSlice, RuntimeError> {
        self.shared
            .stream
            .read(from, max)
            .map_err(|evicted| RuntimeError::OutputEvicted {
                requested: evicted.requested,
                tail_start: evicted.tail_start,
                tail_end: evicted.tail_end,
            })
    }

    pub fn input(&self, bytes: Vec<u8>) -> Result<usize, RuntimeError> {
        if !self.is_live() {
            return Err(RuntimeError::NotLive);
        }
        self.ask(|reply| Command::Input(bytes, reply))
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        self.ask(|reply| Command::Resize { cols, rows, reply })
    }

    pub fn stop(&self) -> Result<StopReport, RuntimeError> {
        self.stop_until(Instant::now() + STOP_BUDGET)
    }

    pub fn stop_until(&self, deadline: Instant) -> Result<StopReport, RuntimeError> {
        self.shared.stop_requested.store(true, Ordering::Release);
        if self.retired()
            && self.exit().is_some()
            && self.descendants_unresolved() == 0
            && self.unverified().is_none()
        {
            return Ok(StopReport {
                exit: self.exit(),
                descendants_unresolved: 0,
                unverified: None,
            });
        }
        let (reply_tx, reply_rx) = sync_channel(1);
        let remaining = deadline.saturating_duration_since(Instant::now());
        self.send_bounded(
            Message::Command(Command::Stop {
                deadline,
                reply: reply_tx,
            }),
            remaining,
        )?;
        match reply_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(report) => Ok(report),
            Err(RecvTimeoutError::Timeout) => Err(RuntimeError::Deadline(STOP_BUDGET)),
            Err(RecvTimeoutError::Disconnected) => Err(RuntimeError::Gone),
        }
    }

    #[cfg(test)]
    pub(crate) fn inject_panic(&self) {
        let _ = self.send_bounded(Message::Command(Command::InjectPanic), REQUEST_DEADLINE);
    }
}

#[cfg(unix)]
use crate::process::UnixProcessTreeOwner as ProcessTreeOwner;
#[cfg(windows)]
use crate::process::WindowsProcessTreeOwner as ProcessTreeOwner;

struct PendingStop {
    deadline: Instant,
    replies: Vec<SyncSender<StopReport>>,
    #[cfg(unix)]
    force_at: Option<Instant>,
}

struct Session {
    terminal: Option<ghostty::DisplayTerminal>,
    writer: Option<SyncSender<Vec<u8>>>,
    master: Option<Box<dyn portable_pty::MasterPty + Send>>,
    child_pid: u32,
    process_tree: ProcessTreeOwner,
    cols: u16,
    rows: u16,
    reader_eof: bool,
    exit: Option<ExitOutcome>,
    exit_published: bool,
    descendants_unresolved: usize,
    generation: SessionGeneration,
    observer: RuntimeObserver,
    shared: Arc<Shared>,
    next_process_scan: Option<Instant>,
    reconcile_at: Option<Instant>,
    reconcile_backoff: Duration,
    drain_deadline: Option<Instant>,
    tail_release_at: Option<Instant>,
    stop: Option<PendingStop>,
    unattended: bool,
    waiter_failed: bool,
    unobserved_exit: bool,
    completed: bool,
}

fn run(
    spec: SpawnSpec,
    generation: SessionGeneration,
    tx: SyncSender<Message>,
    rx: Receiver<Message>,
    shared: Arc<Shared>,
    observer: RuntimeObserver,
    startup_tx: SyncSender<Result<ProcessIdentity, String>>,
) {
    let mut session = match start(spec, generation, tx, &shared, observer) {
        Ok(session) => session,
        Err(reason) => {
            let _ = startup_tx.send(Err(reason));
            return;
        }
    };
    if shared.stop_requested.load(Ordering::Acquire) {
        session.mark_unverified(LATE_LAUNCH_CANCELLED.to_string());
    }
    if startup_tx
        .send(Ok(ProcessIdentity::capture(session.child_pid)))
        .is_err()
    {
        session.unattended = true;
        session.begin_stop(None, Instant::now() + STOP_BUDGET);
    }
    loop {
        let served = catch_unwind(AssertUnwindSafe(|| serve_loop(&mut session, &rx)));
        match served {
            Ok(()) => break,
            Err(_) => {
                session.mark_unverified(RUNTIME_PANIC_REASON.to_string());
                session.retire_engine();
            }
        }
    }
    let _ = catch_unwind(AssertUnwindSafe(|| session.retire_terminal()));
}

fn serve_loop(session: &mut Session, rx: &Receiver<Message>) {
    let mut disconnected = false;
    let mut output_pending = false;
    loop {
        let deadline = session.next_deadline();
        let received = if disconnected {
            match deadline {
                Some(deadline) => {
                    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                    Err(RecvTimeoutError::Timeout)
                }
                None => return,
            }
        } else if output_pending {
            match rx.try_recv() {
                Ok(message) => Ok(message),
                Err(TryRecvError::Empty) => Ok(Message::OutputReady),
                Err(TryRecvError::Disconnected) => Err(RecvTimeoutError::Disconnected),
            }
        } else {
            match deadline {
                Some(deadline) => {
                    rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                }
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            }
        };
        match received {
            Ok(Message::OutputReady) => output_pending = session.drain_inbox(),
            Ok(Message::Eof) => session.reader_eof = true,
            Ok(Message::ChildExited(outcome)) => session.on_child_exited(outcome),
            #[cfg(target_os = "linux")]
            Ok(Message::RootExiting(ack)) => {
                session.on_root_exiting();
                let _ = ack.send(());
            }
            Ok(Message::Command(command)) => session.handle(command),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                disconnected = true;
                if session.finished() {
                    return;
                }
                if session.stop.is_none() {
                    session.unattended = true;
                    session.begin_stop(None, Instant::now() + STOP_BUDGET);
                }
            }
        }
        session.advance();
        if session.finished() {
            return;
        }
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_freed_heap() {
    unsafe {
        libc::malloc_trim(0);
    }
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_freed_heap() {}

fn stop_is_confirmed(report: &StopReport) -> bool {
    report.exit.is_some() && report.unverified.is_none() && report.descendants_unresolved == 0
}

fn new_terminal(spec: &SpawnSpec) -> Result<ghostty::DisplayTerminal, String> {
    let size = ghostty::WindowSize::new(
        usize::from(spec.cols),
        usize::from(spec.rows),
        CELL_WIDTH_PX,
        CELL_HEIGHT_PX,
    )
    .map_err(|e| format!("invalid terminal size: {e}"))?;
    let mut terminal = ghostty::DisplayTerminal::new(
        size,
        spec.scrollback_lines,
        ghostty::TerminalAppearance::default(),
    )
    .map_err(|e| format!("terminal engine initialization failed: {e}"))?;
    terminal
        .set_continuation_max_bytes(CONTINUATION_MAX_BYTES)
        .map_err(|e| format!("continuation tracking could not be enabled: {e}"))?;
    if let Err(error) = terminal.set_terminfo_name(TERMINFO_NAME) {
        log::warn!("paneflow-host: terminfo name could not be configured: {error}");
    }
    let scrollback_bytes = spec
        .scrollback_lines
        .saturating_mul(SCROLLBACK_BYTES_PER_LINE)
        .min(MAX_SCROLLBACK_BYTES);
    if let Err(error) = terminal.set_scrollback_max_bytes(Some(scrollback_bytes)) {
        log::warn!("paneflow-host: scrollback budget could not be configured: {error}");
    }
    Ok(terminal)
}

fn start(
    spec: SpawnSpec,
    generation: SessionGeneration,
    tx: SyncSender<Message>,
    shared: &Arc<Shared>,
    observer: RuntimeObserver,
) -> Result<Session, String> {
    let terminal = new_terminal(&spec)?;

    let pair = crate::pty::open(pty_size(spec.cols, spec.rows))
        .map_err(|e| format!("failed to open a native PTY: {e}"))?;
    let mut command = CommandBuilder::new(&spec.shell);
    command.args(&spec.args);
    command.cwd(&spec.cwd);
    for key in crate::env::inherited_env_keys_to_strip() {
        command.env_remove(&key);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    #[cfg(debug_assertions)]
    if let Some(delay) = spec
        .env
        .get("PANEFLOW_TEST_SPAWN_DELAY_MS")
        .and_then(|delay| delay.parse::<u64>().ok())
    {
        std::thread::sleep(Duration::from_millis(delay.min(30_000)));
    }
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| format!("failed to spawn {} in the PTY: {e}", spec.shell))?;
    let child_pid = child.process_id().unwrap_or(0);
    drop(pair.slave);
    let mut session = Session {
        terminal: Some(terminal),
        writer: None,
        master: Some(pair.master),
        child_pid,
        process_tree: ProcessTreeOwner::new(ProcessIdentity::capture(child_pid)),
        cols: spec.cols,
        rows: spec.rows,
        reader_eof: false,
        exit: None,
        exit_published: false,
        descendants_unresolved: 0,
        generation,
        observer,
        shared: Arc::clone(shared),
        next_process_scan: Some(Instant::now()),
        reconcile_at: None,
        reconcile_backoff: DESCENDANT_RECONCILE_MIN,
        drain_deadline: None,
        tail_release_at: None,
        stop: None,
        unattended: false,
        waiter_failed: false,
        unobserved_exit: false,
        completed: false,
    };
    #[cfg(test)]
    let fail_wait = spec.env.contains_key("PANEFLOW_TEST_WAIT_FAILURE");
    #[cfg(not(test))]
    let fail_wait = false;
    let waited = spawn_child_waiter(child, child_pid, tx.clone(), fail_wait);
    if let Err(reason) = waited {
        session.waiter_failed = true;
        session.mark_unverified(reason);
    }
    let wired = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        if spec.env.contains_key("PANEFLOW_TEST_WIRING_PANIC") {
            panic!("injected PTY wiring panic");
        }
        #[cfg(test)]
        if spec.env.contains_key("PANEFLOW_TEST_WIRING_FAILURE") {
            return Err("injected PTY thread creation failure".into());
        }
        session
            .master
            .as_ref()
            .ok_or_else(|| "the PTY master is missing".to_string())
            .and_then(|master| wire_pty(master.as_ref(), tx, Arc::clone(shared)))
    }))
    .unwrap_or_else(|_| Err("PTY wiring panicked after child creation".into()));
    if ProcessIdentity::capture(child_pid).started_at.is_none() {
        session.mark_unverified("the spawned child identity could not be captured".into());
    }
    match wired {
        Ok(writer) => session.writer = Some(writer),
        Err(reason) => session.mark_unverified(reason),
    }
    Ok(session)
}

fn spawn_child_waiter(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    child_pid: u32,
    tx: SyncSender<Message>,
    fail_wait: bool,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("paneflow-host-child-waiter".into())
        .spawn(move || {
            if fail_wait {
                let _ = tx.send(Message::ChildExited(Err(
                    "injected child wait failure".to_string()
                )));
                let _ = child.wait();
                return;
            }
            #[cfg(target_os = "linux")]
            if crate::process::await_exit_unreaped(child_pid) {
                let (ack_tx, ack_rx) = sync_channel::<()>(1);
                if tx.send(Message::RootExiting(ack_tx)).is_ok() {
                    let _ = ack_rx.recv_timeout(ROOT_EXIT_DISCOVERY_DEADLINE);
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = child_pid;
            let outcome = child
                .wait()
                .map(|status| exit_outcome(&status))
                .map_err(|error| format!("child wait failed: {error}"));
            let _ = tx.send(Message::ChildExited(outcome));
        })
        .map(|_| ())
        .map_err(|e| format!("failed to start the child waiter: {e}"))
}

fn wire_pty(
    master: &(dyn portable_pty::MasterPty + Send),
    tx: SyncSender<Message>,
    shared: Arc<Shared>,
) -> Result<SyncSender<Vec<u8>>, String> {
    let reader = master
        .try_clone_reader()
        .map_err(|e| format!("failed to clone the PTY reader: {e}"))?;
    let writer = master
        .take_writer()
        .map_err(|e| format!("failed to take the PTY writer: {e}"))?;
    let reader_shared = Arc::clone(&shared);
    let accounting = shared;
    std::thread::Builder::new()
        .name("paneflow-host-pty-reader".into())
        .spawn(move || read_pty(reader, tx, reader_shared))
        .map_err(|e| format!("failed to start the PTY reader: {e}"))?;
    let (input_tx, input_rx) = sync_channel::<Vec<u8>>(INPUT_QUEUE_SLOTS);
    std::thread::Builder::new()
        .name("paneflow-host-pty-writer".into())
        .spawn(move || write_pty_queue(writer, input_rx, accounting))
        .map_err(|e| format!("failed to start the PTY writer: {e}"))?;
    Ok(input_tx)
}

fn read_pty(mut reader: Box<dyn Read + Send>, tx: SyncSender<Message>, shared: Arc<Shared>) {
    let mut buffer = vec![0u8; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                if !shared.inbox.push(buffer[..read].to_vec(), &tx) {
                    return;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::yield_now();
            }
            Err(_) => break,
        }
    }
    let _ = tx.send(Message::Eof);
}

fn write_pty_queue(mut writer: Box<dyn Write + Send>, rx: Receiver<Vec<u8>>, shared: Arc<Shared>) {
    while let Ok(bytes) = rx.recv() {
        let written = writer.write_all(&bytes).and_then(|()| writer.flush());
        shared
            .input_queued_bytes
            .fetch_sub(bytes.len(), Ordering::AcqRel);
        if let Err(error) = written {
            log::debug!("paneflow-host: PTY write failed, closing the input queue: {error}");
            break;
        }
    }
    while let Ok(bytes) = rx.try_recv() {
        shared
            .input_queued_bytes
            .fetch_sub(bytes.len(), Ordering::AcqRel);
    }
}

fn earliest(deadlines: impl IntoIterator<Item = Option<Instant>>) -> Option<Instant> {
    deadlines.into_iter().flatten().min()
}

impl Session {
    fn next_deadline(&self) -> Option<Instant> {
        let stop = self.stop.as_ref().map(|stop| {
            #[cfg(unix)]
            let force = stop.force_at;
            #[cfg(not(unix))]
            let force = None;
            let poll = (self.exit.is_none() || self.descendants_unresolved > 0)
                .then(|| Instant::now() + DESCENDANT_RECONCILE_MIN);
            earliest([Some(stop.deadline), force, poll]).unwrap_or(stop.deadline)
        });
        earliest([
            self.next_process_scan,
            self.reconcile_at,
            self.drain_deadline,
            self.tail_release_at,
            stop,
        ])
    }

    fn finished(&self) -> bool {
        self.completed
            && self.tail_release_at.is_none()
            && self.stop.is_none()
            && self.exit.is_some()
            && self.descendants_unresolved == 0
            && self.shared.unverified().is_none()
    }

    fn drain_inbox(&mut self) -> bool {
        let (chunks, more) = self.shared.inbox.take_slice(DRAIN_SLICE_BYTES);
        for chunk in chunks {
            self.feed(&chunk);
        }
        more
    }

    fn feed(&mut self, chunk: &[u8]) {
        self.shared.stream.append(chunk);
        self.shared
            .output_changed_at_ms
            .store(crate::manifest::now_ms(), Ordering::Release);
        if self.next_process_scan.is_none() && self.exit.is_none() {
            self.next_process_scan = Some(Instant::now() + PROCESS_SCAN_INTERVAL);
        }
        let Some(terminal) = self.terminal.as_mut() else {
            return;
        };
        if let Err(error) = terminal.feed(chunk) {
            log::warn!("paneflow-host: terminal feed failed: {error}");
        }
        for event in terminal.drain_events() {
            match event {
                ghostty::BackendEvent::WritePty(bytes) => {
                    if let Err(error) = write_pty(&mut self.writer, &self.shared, bytes) {
                        log::debug!("paneflow-host: terminal reply to the PTY failed: {error}");
                    }
                }
                ghostty::BackendEvent::Title(title) => (self.observer)(RuntimeNotice::Title(title)),
                ghostty::BackendEvent::WorkingDirectory(cwd) => {
                    (self.observer)(RuntimeNotice::WorkingDirectory(cwd));
                }
                ghostty::BackendEvent::ClipboardStore(_)
                | ghostty::BackendEvent::Bell
                | ghostty::BackendEvent::DesktopNotification { .. }
                | ghostty::BackendEvent::Progress(_) => {}
                ghostty::BackendEvent::UnknownSequence { .. }
                | ghostty::BackendEvent::CallbackPanicked
                | ghostty::BackendEvent::InputDropped { .. }
                | ghostty::BackendEvent::EffectsOverflow { .. } => {
                    log::debug!("paneflow-host: terminal engine event ignored: {event:?}");
                }
            }
        }
    }

    fn handle(&mut self, command: Command) {
        if let Command::Stop { deadline, reply } = command {
            self.begin_stop(Some(reply), deadline);
            return;
        }
        #[cfg(test)]
        if let Command::InjectPanic = command {
            panic!("injected runtime panic");
        }
        if let Some(reason) = self.shared.unverified() {
            refuse(command, RuntimeError::Unverified(reason));
            return;
        }
        let Some(terminal) = self.terminal.as_mut() else {
            refuse(command, RuntimeError::NotLive);
            return;
        };
        match command {
            Command::Checkpoint(reply) => {
                let _ = reply.send(checkpoint(
                    terminal,
                    self.generation,
                    self.shared.stream.end_offset(),
                    self.cols,
                    self.rows,
                ));
            }
            Command::CheckpointSize(reply) => {
                let _ = reply.send(
                    terminal
                        .encode_snapshot_size()
                        .map_err(|e| RuntimeError::Engine(e.to_string())),
                );
            }
            Command::Text(reply) => {
                let _ = reply.send(text(terminal));
            }
            Command::Viewport(reply) => {
                let _ = reply.send(self.viewport_scan());
            }
            Command::BracketedPaste(reply) => {
                let modes = terminal
                    .modes()
                    .map_err(|e| RuntimeError::Engine(e.to_string()));
                let _ = reply.send(modes.map(|modes| modes.bracketed_paste));
            }
            Command::Input(bytes, reply) => {
                let result = if self.exit.is_some() || self.writer.is_none() {
                    Err(RuntimeError::NotLive)
                } else {
                    write_pty(&mut self.writer, &self.shared, bytes)
                        .map_err(|e| RuntimeError::Pty(e.to_string()))
                };
                let _ = reply.send(result);
            }
            Command::Resize { cols, rows, reply } => {
                let _ = reply.send(self.resize(cols, rows));
            }
            Command::Stop { .. } => {}
            #[cfg(test)]
            Command::InjectPanic => {}
        }
    }

    fn viewport_scan(&mut self) -> Result<ViewportScan, RuntimeError> {
        let foreground_process_group = self.foreground_process_group();
        let terminal = self.terminal.as_mut().ok_or(RuntimeError::NotLive)?;
        let screen = terminal
            .format(ghostty::FormatterOptions::plain_text())
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        Ok(ViewportScan {
            screen,
            foreground_process_group,
        })
    }

    #[cfg(unix)]
    fn foreground_process_group(&self) -> Option<i32> {
        self.master
            .as_ref()
            .and_then(|master| master.process_group_leader())
    }

    #[cfg(not(unix))]
    fn foreground_process_group(&self) -> Option<i32> {
        None
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), RuntimeError> {
        if cols == 0 || rows == 0 {
            return Err(RuntimeError::Engine(
                "terminal size must be non-zero".to_string(),
            ));
        }
        if let Some(master) = self.master.as_ref() {
            master
                .resize(pty_size(cols, rows))
                .map_err(|e| RuntimeError::Pty(e.to_string()))?;
        }
        let size = ghostty::WindowSize::new(
            usize::from(cols),
            usize::from(rows),
            CELL_WIDTH_PX,
            CELL_HEIGHT_PX,
        )
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        self.terminal
            .as_mut()
            .ok_or(RuntimeError::NotLive)?
            .resize(size)
            .map_err(|e| RuntimeError::Engine(e.to_string()))?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn on_root_exiting(&mut self) {
        self.process_tree.discover();
        self.refresh_descendants();
    }

    fn on_child_exited(&mut self, outcome: Result<ExitOutcome, String>) {
        match outcome {
            Ok(outcome) => {
                self.process_tree.discover();
                self.refresh_descendants();
                self.record_exit(outcome);
            }
            Err(reason) => {
                self.waiter_failed = true;
                self.mark_unverified(reason);
                self.schedule_reconcile();
            }
        }
    }

    fn advance(&mut self) {
        let now = Instant::now();
        if self.next_process_scan.is_some_and(|at| now >= at) {
            self.next_process_scan = None;
            if self.exit.is_none() {
                self.process_tree.discover();
            }
        }
        if self.reconcile_at.is_some_and(|at| now >= at) {
            self.reconcile_at = None;
            self.reconcile_descendants();
        }
        if self.drain_deadline.is_some_and(|at| now >= at) {
            self.drain_deadline = None;
            self.finalize(false);
        }
        if self.exit.is_some() && self.reader_eof && !self.completed {
            self.drain_deadline = None;
            self.finalize(true);
        }
        if self.tail_release_at.is_some_and(|at| now >= at) {
            self.tail_release_at = None;
            self.shared.stream.release();
        }
        self.advance_stop(now);
    }

    fn schedule_reconcile(&mut self) {
        if self.reconcile_at.is_none() {
            self.reconcile_at = Some(Instant::now() + self.reconcile_backoff);
            self.reconcile_backoff = (self.reconcile_backoff * 2).min(DESCENDANT_RECONCILE_MAX);
        }
    }

    fn reconcile_descendants(&mut self) {
        if self.exit.is_none() {
            return;
        }
        let before = self.descendants_unresolved;
        self.refresh_descendants();
        if self.descendants_unresolved == 0 {
            if before > 0 || !self.exit_published {
                self.publish_exit();
            }
        } else {
            self.schedule_reconcile();
        }
    }

    fn refresh_descendants(&mut self) {
        self.descendants_unresolved = self.process_tree.unresolved();
        self.shared
            .descendants_unresolved
            .store(self.descendants_unresolved, Ordering::Release);
    }

    fn mark_unverified(&mut self, reason: String) {
        if self.shared.unverified().as_deref() == Some(reason.as_str()) {
            return;
        }
        log::warn!(
            "paneflow-host: child pid={} ownership is unverified: {reason}",
            self.child_pid
        );
        self.shared.set_unverified(Some(reason.clone()));
        (self.observer)(RuntimeNotice::Unverified(reason));
    }

    fn publish_exit(&mut self) {
        let Some(outcome) = self.exit.clone() else {
            return;
        };
        self.shared.set_unverified(None);
        self.exit_published = true;
        (self.observer)(RuntimeNotice::Exited(outcome));
    }

    fn record_exit(&mut self, outcome: ExitOutcome) {
        if self.exit.is_some() {
            return;
        }
        #[cfg(target_os = "macos")]
        self.process_tree.root_reaped();
        self.exit = Some(outcome.clone());
        self.shared.set_unverified(None);
        *self
            .shared
            .exit
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(outcome);
        self.writer = None;
        self.next_process_scan = None;
        self.release_master();
        if !self.reader_eof {
            self.drain_deadline = Some(Instant::now() + FINAL_DRAIN_BUDGET);
        }
        if self.descendants_unresolved == 0 {
            self.publish_exit();
        } else {
            self.mark_unverified(format!(
                "{} descendant process(es) remain unresolved",
                self.descendants_unresolved
            ));
            self.reconcile_backoff = DESCENDANT_RECONCILE_MIN;
            self.schedule_reconcile();
        }
    }

    fn finalize(&mut self, complete: bool) {
        if self.completed {
            return;
        }
        let Some(exit) = self.exit.clone() else {
            return;
        };
        while self.drain_inbox() {}
        self.completed = true;
        let text = self
            .terminal
            .as_mut()
            .and_then(|terminal| text(terminal).ok())
            .map(|text| bounded_final_text(text, FINAL_TEXT_MAX_BYTES))
            .unwrap_or_default();
        self.shared.stream.finish();
        let final_offset = self.shared.stream.end_offset();
        self.retire_terminal();
        self.tail_release_at = Some(Instant::now() + TAIL_RELEASE_GRACE);
        (self.observer)(RuntimeNotice::Completed(CompletedRecord {
            generation: self.generation,
            exit,
            final_offset,
            complete: complete && self.reader_eof,
            text,
        }));
    }

    fn retire_terminal(&mut self) {
        self.writer = None;
        self.release_master();
        self.retire_engine();
        release_freed_heap();
    }

    fn retire_engine(&mut self) {
        self.terminal = None;
        if self.completed {
            self.shared.inbox.close();
        }
        self.shared.retired.store(true, Ordering::Release);
    }

    fn release_master(&mut self) {
        if let Some(master) = self.master.take() {
            let _ = std::thread::Builder::new()
                .name("paneflow-host-pty-closer".into())
                .spawn(move || drop(master));
        }
    }

    fn report(&self) -> StopReport {
        StopReport {
            exit: self.exit.clone(),
            descendants_unresolved: self.descendants_unresolved,
            unverified: self.shared.unverified(),
        }
    }

    fn begin_stop(&mut self, reply: Option<SyncSender<StopReport>>, deadline: Instant) {
        self.writer = None;
        if let Some(pending) = self.stop.as_mut() {
            pending.replies.extend(reply);
            pending.deadline = pending.deadline.max(deadline);
            return;
        }
        self.stop = Some(PendingStop {
            deadline,
            replies: reply.into_iter().collect(),
            #[cfg(unix)]
            force_at: Some(Instant::now() + FORCE_SIGNAL_AFTER),
        });
        self.signal_tree(false, deadline);
        self.advance_stop(Instant::now());
    }

    fn signal_tree(&mut self, force: bool, deadline: Instant) {
        #[cfg(windows)]
        {
            let _ = force;
            let wait_until = deadline.min(Instant::now() + TREE_TERMINATE_WAIT);
            let _ = self.process_tree.terminate(wait_until);
        }
        #[cfg(unix)]
        {
            let _ = deadline;
            self.process_tree.signal(force);
        }
    }

    fn advance_stop(&mut self, now: Instant) {
        let Some(deadline) = self.stop.as_ref().map(|stop| stop.deadline) else {
            return;
        };
        #[cfg(unix)]
        {
            let force_due = self
                .stop
                .as_ref()
                .is_some_and(|stop| stop.force_at.is_some_and(|at| now >= at));
            if force_due {
                if let Some(stop) = self.stop.as_mut() {
                    stop.force_at = None;
                }
                self.signal_tree(true, deadline);
            }
        }
        if self.exit.is_none() && self.waiter_failed {
            self.reconcile_unobserved_exit();
        }
        self.refresh_descendants();
        let confirmed = self.exit.is_some() && self.descendants_unresolved == 0;
        if confirmed {
            if !self.exit_published {
                self.publish_exit();
            }
            self.complete_stop();
            return;
        }
        if self.unobserved_exit && self.descendants_unresolved == 0 {
            self.complete_stop();
            return;
        }
        if now >= deadline {
            self.mark_unverified(format!(
                "child pid={} or {} descendant process(es) remain unresolved after termination",
                self.child_pid, self.descendants_unresolved
            ));
            self.complete_stop();
        }
    }

    fn reconcile_unobserved_exit(&mut self) {
        if self.shared.unverified().is_none() || self.process_tree.root_is_running() {
            return;
        }
        let root = ProcessIdentity::capture(self.child_pid);
        if root.verify() == crate::process::ProcessVerdict::Gone {
            self.unobserved_exit = true;
            self.mark_unverified(format!("{EXIT_UNOBSERVED}; the process is gone"));
        }
    }

    fn complete_stop(&mut self) {
        let Some(stop) = self.stop.take() else {
            return;
        };
        let report = self.report();
        for reply in stop.replies {
            let _ = reply.send(report.clone());
        }
        if self.unattended && !stop_is_confirmed(&report) && !self.unobserved_exit {
            self.stop = Some(PendingStop {
                deadline: Instant::now() + UNATTENDED_STOP_RETRY + STOP_BUDGET,
                replies: Vec::new(),
                #[cfg(unix)]
                force_at: Some(Instant::now() + UNATTENDED_STOP_RETRY),
            });
        }
    }
}

fn refuse(command: Command, error: RuntimeError) {
    match command {
        Command::Checkpoint(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Text(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Viewport(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::BracketedPaste(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Input(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::CheckpointSize(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Resize { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        Command::Stop { .. } => {}
        #[cfg(test)]
        Command::InjectPanic => {}
    }
}

fn write_pty(
    writer: &mut Option<SyncSender<Vec<u8>>>,
    shared: &Shared,
    bytes: Vec<u8>,
) -> std::io::Result<usize> {
    use std::sync::mpsc::TrySendError;
    let Some(sender) = writer.as_ref() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "the PTY writer is closed",
        ));
    };
    let len = bytes.len();
    let queued = shared.input_queued_bytes.load(Ordering::Acquire);
    if queued.saturating_add(len) > MAX_INPUT_QUEUE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            format!(
                "the child is not reading its input; {queued} bytes are queued against a {MAX_INPUT_QUEUE_BYTES} byte budget"
            ),
        ));
    }
    shared.input_queued_bytes.fetch_add(len, Ordering::AcqRel);
    match sender.try_send(bytes) {
        Ok(()) => Ok(len),
        Err(TrySendError::Full(_)) => {
            shared.input_queued_bytes.fetch_sub(len, Ordering::AcqRel);
            Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "the child is not reading its input; the input queue is full",
            ))
        }
        Err(TrySendError::Disconnected(_)) => {
            shared.input_queued_bytes.fetch_sub(len, Ordering::AcqRel);
            *writer = None;
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "the PTY writer is closed",
            ))
        }
    }
}

fn checkpoint(
    terminal: &mut ghostty::DisplayTerminal,
    generation: SessionGeneration,
    offset: u64,
    cols: u16,
    rows: u16,
) -> Result<Checkpoint, RuntimeError> {
    let bytes = terminal
        .encode_snapshot_size()
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
    if bytes > MAX_CHECKPOINT_BYTES {
        return Err(RuntimeError::CheckpointTooLarge {
            bytes,
            limit: MAX_CHECKPOINT_BYTES,
        });
    }
    let snapshot = terminal
        .encode_snapshot()
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
    if snapshot.len() > MAX_CHECKPOINT_BYTES {
        return Err(RuntimeError::CheckpointTooLarge {
            bytes: snapshot.len(),
            limit: MAX_CHECKPOINT_BYTES,
        });
    }
    Ok(Checkpoint {
        generation,
        offset,
        cols,
        rows,
        snapshot,
    })
}

fn text(terminal: &mut ghostty::DisplayTerminal) -> Result<String, RuntimeError> {
    let history = terminal
        .extract_scrollback()
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
    let screen = terminal
        .format(ghostty::FormatterOptions::plain_text())
        .map_err(|e| RuntimeError::Engine(e.to_string()))?;
    let screen = screen.trim_end_matches([NEWLINE_CHAR, ' ']).to_string();
    Ok(match (history, screen.is_empty()) {
        (Some(history), false) => [history, screen].join(NEWLINE),
        (Some(history), true) => history,
        (None, false) => screen,
        (None, true) => String::new(),
    })
}

pub fn bounded_final_text(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
}

fn exit_outcome(status: &portable_pty::ExitStatus) -> ExitOutcome {
    ExitOutcome {
        code: i64::from(status.exit_code()),
        signal: status.signal().map(str::to_owned),
    }
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: u16::try_from(u32::from(cols) * CELL_WIDTH_PX).unwrap_or(u16::MAX),
        pixel_height: u16::try_from(u32::from(rows) * CELL_HEIGHT_PX).unwrap_or(u16::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn echo_shell_spec(cols: u16, rows: u16) -> SpawnSpec {
        #[cfg(windows)]
        let (shell, args) = (
            "cmd.exe".to_string(),
            vec!["/Q".to_string(), "/D".to_string()],
        );
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh".to_string(), Vec::new());
        SpawnSpec {
            shell,
            args,
            cwd: std::env::temp_dir(),
            env: BTreeMap::from([("TERM".to_string(), "xterm-256color".to_string())]),
            cols,
            rows,
            scrollback_lines: 500,
        }
    }

    fn wait_for_output(runtime: &SessionRuntime, marker: &str, from: u64) -> (u64, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut collected = Vec::new();
        let mut offset = from;
        while Instant::now() < deadline {
            let slice = runtime.output(offset, 64 * 1024).expect("output readable");
            collected.extend_from_slice(&slice.data);
            offset = slice.offset + slice.data.len() as u64;
            if String::from_utf8_lossy(&collected).contains(marker) {
                return (offset, collected);
            }
            runtime
                .stream()
                .wait_past(offset, Instant::now() + Duration::from_millis(200));
        }
        panic!(
            "marker {marker:?} never appeared; got {:?}",
            String::from_utf8_lossy(&collected)
        );
    }

    fn wait_until(deadline: Duration, mut check: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_session_runs_a_shell_answers_input_and_checkpoints_atomically() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let observer: RuntimeObserver = Arc::new(move |notice| sink.lock().unwrap().push(notice));
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        assert!(runtime.is_live());
        assert!(runtime.process().pid != 0);

        runtime
            .input(b"echo PANEFLOW_HOST_ALPHA\r\n".to_vec())
            .expect("input accepted");
        let (offset, _) = wait_for_output(&runtime, "PANEFLOW_HOST_ALPHA", 0);

        let checkpoint = runtime.checkpoint().expect("checkpoint");
        assert!(
            checkpoint.offset + 32 >= offset,
            "offset covers the echoed output"
        );
        assert!(!checkpoint.snapshot.is_empty());
        assert_eq!((checkpoint.cols, checkpoint.rows), (80, 24));
        let mut decoder =
            ghostty::SnapshotDecoder::from_bytes(&checkpoint.snapshot).expect("snapshot decodes");
        let terminal = decoder
            .decode(ghostty::SnapshotRestore {
                cell_width: 8,
                cell_height: 16,
                max_scrollback: 500,
                appearance: ghostty::TerminalAppearance::default(),
            })
            .expect("terminal restores from the checkpoint");
        let content = terminal.snapshot().expect("restored screen renders");
        let text: String = content.cells.iter().map(|cell| cell.character).collect();
        assert!(
            text.contains("PANEFLOW_HOST_ALPHA"),
            "restored screen must contain the echoed marker; got {text:?}"
        );

        runtime.resize(100, 30).expect("resize");
        let after = runtime.checkpoint().expect("checkpoint after resize");
        assert_eq!((after.cols, after.rows), (100, 30));
        assert!(
            after.offset >= checkpoint.offset,
            "offsets never go backwards"
        );

        let report = runtime.stop().expect("stop completes");
        assert!(report.exit.is_some(), "stop records an exit outcome");
        assert_eq!(report.unverified, None);
        assert!(!runtime.is_live());
        assert_eq!(runtime.input(b"x".to_vec()), Err(RuntimeError::NotLive));
        assert!(
            wait_until(Duration::from_secs(5), || runtime.retired()),
            "a confirmed exit retires the terminal runtime"
        );
        assert!(
            wait_until(Duration::from_secs(5), || runtime
                .stream()
                .status()
                .retained_bytes
                == 0),
            "the live tail is released within the retirement budget"
        );
        let observed = notices.lock().unwrap();
        assert!(
            observed
                .iter()
                .any(|n| matches!(n, RuntimeNotice::Exited(_))),
            "the observer sees the exit"
        );
        let completed = observed
            .iter()
            .find_map(|n| match n {
                RuntimeNotice::Completed(record) => Some(record.clone()),
                _ => None,
            })
            .expect("the observer receives the completed record");
        assert!(completed.text.contains("PANEFLOW_HOST_ALPHA"));
        assert!(completed.final_offset >= offset);
    }

    #[test]
    fn natural_exit_drains_the_final_output_before_the_stream_ends() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let observer: RuntimeObserver = Arc::new(move |notice| sink.lock().unwrap().push(notice));
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        runtime
            .input(b"echo PANEFLOW_LAST_WORDS\r\nexit\r\n".to_vec())
            .expect("input accepted");
        let stream = runtime.stream();
        let mut offset = 0;
        let mut collected = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let slice = runtime.output(offset, 64 * 1024).expect("readable");
            collected.extend_from_slice(&slice.data);
            offset = slice.offset + slice.data.len() as u64;
            if !slice.live && slice.end_offset <= offset {
                break;
            }
            assert!(Instant::now() < deadline, "the stream must end after exit");
            stream.wait_past(offset, Instant::now() + Duration::from_millis(500));
        }
        assert!(String::from_utf8_lossy(&collected).contains("PANEFLOW_LAST_WORDS"));
        assert!(wait_until(Duration::from_secs(5), || runtime.retired()));
        let observed = notices.lock().unwrap();
        let completed = observed
            .iter()
            .find_map(|n| match n {
                RuntimeNotice::Completed(record) => Some(record.clone()),
                _ => None,
            })
            .expect("completed record");
        assert!(completed.complete, "EOF followed the exit: {completed:?}");
        assert_eq!(completed.final_offset, offset);
        assert!(completed.text.contains("PANEFLOW_LAST_WORDS"));
    }

    #[test]
    fn a_child_that_stops_reading_its_input_never_starves_the_control_path() {
        let runtime = SessionRuntime::spawn(
            echo_shell_spec(80, 24),
            SessionGeneration::FIRST,
            Arc::new(|_| {}),
        )
        .expect("shell spawns");
        runtime
            .input(b"echo BLOCK_READY\r\n".to_vec())
            .expect("input accepted");
        wait_for_output(&runtime, "BLOCK_READY", 0);
        #[cfg(windows)]
        let hold = b"ping -n 6 127.0.0.1 >nul\r\n".to_vec();
        #[cfg(unix)]
        let hold = b"sleep 5\n".to_vec();
        runtime.input(hold).expect("the holding command starts");
        std::thread::sleep(Duration::from_millis(300));
        let chunk = vec![b' '; READ_CHUNK_BYTES];
        let mut slowest = Duration::ZERO;
        for _ in 0..(INPUT_QUEUE_SLOTS * 3) {
            let started = Instant::now();
            match runtime.input(chunk.clone()) {
                Ok(_) => {}
                Err(RuntimeError::Pty(reason)) => {
                    assert!(
                        reason.contains("input queue is full") || reason.contains("byte budget"),
                        "{reason}"
                    );
                }
                Err(other) => panic!("blocked input surfaced {other:?}"),
            }
            slowest = slowest.max(started.elapsed());
            assert!(
                runtime.resources().input_queued_bytes <= MAX_INPUT_QUEUE_BYTES,
                "US-012: queued input stays within its byte budget"
            );
        }
        assert!(
            slowest < Duration::from_secs(1),
            "US-007: input never blocks the runtime thread, slowest call {slowest:?}"
        );
        let started = Instant::now();
        let checkpoint = runtime.checkpoint().expect("the runtime still answers");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "US-007: a blocked writer leaves the control path responsive"
        );
        assert_eq!(checkpoint.generation, SessionGeneration::FIRST);
        assert!(runtime.is_live(), "refused input never ends the session");
        let report = runtime.stop().expect("stop answers");
        assert!(report.exit.is_some(), "the stop confirms the child exit");
        assert_eq!(report.descendants_unresolved, 0);
    }

    #[test]
    fn a_child_wait_failure_stays_unverified_without_a_fabricated_exit() {
        let mut spec = echo_shell_spec(80, 24);
        spec.env
            .insert("PANEFLOW_TEST_WAIT_FAILURE".into(), "1".into());
        let launched = SessionRuntime::launch(spec, SessionGeneration::FIRST, Arc::new(|_| {}))
            .expect("the session thread starts")
            .wait(STARTUP_DEADLINE);
        let runtime = match launched {
            LaunchWait::Ready(runtime) | LaunchWait::Recovery(runtime) => runtime,
            LaunchWait::Failed(reason) => panic!("the shell must spawn: {reason}"),
            LaunchWait::Pending(_) => panic!("the shell must spawn within the startup deadline"),
        };
        assert!(wait_until(Duration::from_secs(5), || runtime
            .unverified()
            .is_some()));
        assert_eq!(runtime.exit(), None, "no exit code is fabricated");
        assert!(runtime.owns_process());
        let report = runtime.stop().expect("stop answers");
        assert_eq!(report.exit, None);
        assert!(report.unverified.is_some(), "{report:?}");
        assert!(!runtime.process().is_provably_live());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_descendant_started_just_before_the_root_exits_stays_owned_until_the_stop() {
        let mut spec = echo_shell_spec(80, 24);
        spec.args = vec![
            "-c".into(),
            "sleep 0.3; (trap '' HUP; exec sleep 30) & exit 0".into(),
        ];
        let runtime = SessionRuntime::spawn(spec, SessionGeneration::FIRST, Arc::new(|_| {}))
            .expect("shell spawns");
        let root = runtime.process();
        assert!(wait_until(Duration::from_secs(5), || runtime
            .exit()
            .is_some()));
        assert!(!root.is_provably_live());
        assert_eq!(
            runtime.descendants_unresolved(),
            1,
            "the background child that outlived the root is owned"
        );
        assert!(runtime.unverified().is_some());
        assert!(runtime.owns_process());
        let report = runtime.stop().expect("stop answers");
        assert_eq!(report.descendants_unresolved, 0, "{report:?}");
        assert!(report.unverified.is_none(), "{report:?}");
    }

    #[test]
    fn startup_wiring_failures_and_panics_retain_a_stoppable_owner() {
        for fault in ["PANEFLOW_TEST_WIRING_FAILURE", "PANEFLOW_TEST_WIRING_PANIC"] {
            let mut spec = echo_shell_spec(80, 24);
            spec.env.insert(fault.into(), "1".into());
            let launch =
                SessionRuntime::launch(spec, SessionGeneration::FIRST, Arc::new(|_| {})).unwrap();
            let LaunchWait::Recovery(runtime) = launch.wait(STARTUP_DEADLINE) else {
                panic!("wiring fault must retain the native child");
            };
            assert!(runtime.owns_process());
            let identity = runtime.process();
            assert!(stop_is_confirmed(&runtime.stop().unwrap()));
            assert!(!identity.is_provably_live());
        }
    }

    #[test]
    fn a_startup_deadline_then_cancellation_retains_the_late_child() {
        let mut spec = echo_shell_spec(80, 24);
        spec.env
            .insert("PANEFLOW_TEST_SPAWN_DELAY_MS".into(), "100".into());
        let launch =
            SessionRuntime::launch(spec, SessionGeneration::FIRST, Arc::new(|_| {})).unwrap();
        let LaunchWait::Pending(launch) = launch.wait(Duration::from_millis(10)) else {
            panic!("spawn must remain pending after its caller deadline");
        };
        launch.cancel();
        let LaunchWait::Recovery(runtime) = launch.wait(STARTUP_DEADLINE) else {
            panic!("late child must transfer to recovery");
        };
        let identity = runtime.process();
        assert!(stop_is_confirmed(&runtime.stop().unwrap()));
        assert!(!identity.is_provably_live());
    }

    #[test]
    fn a_checkpoint_survives_a_partial_escape_sequence() {
        let mut terminal = new_terminal(&echo_shell_spec(80, 24)).expect("terminal");
        terminal.feed(b"\x1b[1;2").expect("partial CSI parses");
        let snapshot = terminal
            .encode_snapshot()
            .expect("continuation tracking lets the checkpoint capture the pending bytes");
        assert!(!snapshot.is_empty());
        assert_eq!(
            terminal.continuation().expect("continuation readback"),
            Some(b"\x1b[1;2".to_vec())
        );
    }

    #[test]
    fn a_restarted_generation_is_carried_into_its_checkpoints() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let generation = SessionGeneration::FIRST.next();
        let runtime = SessionRuntime::spawn(echo_shell_spec(80, 24), generation, observer)
            .expect("shell spawns");
        assert_eq!(runtime.generation(), generation);
        assert_eq!(
            runtime.checkpoint().expect("checkpoint").generation,
            generation
        );
        let _ = runtime.stop();
    }

    #[test]
    fn output_before_the_tail_is_reported_as_evicted_not_fabricated() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        let checkpoint = runtime.checkpoint().expect("checkpoint");
        let beyond = checkpoint.offset + 1_000_000;
        assert!(matches!(
            runtime.output(beyond, 1024),
            Err(RuntimeError::OutputEvicted { requested, .. }) if requested == beyond
        ));
        let _ = runtime.stop();
    }

    #[test]
    fn final_text_keeps_its_tail_on_a_char_boundary() {
        let text = format!("{}é{}", "a".repeat(10), "b".repeat(5));
        let bounded = bounded_final_text(text, 7);
        assert_eq!(bounded, "é".to_string() + &"b".repeat(5));
        assert_eq!(bounded_final_text("short".into(), 512), "short");
    }

    #[test]
    fn a_cancelled_launch_terminates_the_late_child_instead_of_publishing_it() {
        let observer: RuntimeObserver = Arc::new(|_| {});
        let handle =
            SessionRuntime::launch(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("the session thread starts");
        handle.cancel();
        let outcome = handle.wait(Duration::from_secs(30));
        match outcome {
            LaunchWait::Recovery(runtime) => {
                assert!(!runtime.is_live());
                assert!(runtime.owns_process());
                let process = runtime.process();
                let report = runtime.stop().expect("the recovery owner accepts stop");
                assert!(stop_is_confirmed(&report), "{report:?}");
                assert!(!process.is_provably_live());
            }
            LaunchWait::Failed(reason) => {
                panic!("a created child must transfer to its recovery owner: {reason}")
            }
            LaunchWait::Ready(_) => panic!("a cancelled launch must never publish a runtime"),
            LaunchWait::Pending(_) => panic!("the cancellation must resolve within the budget"),
        }
    }

    #[test]
    fn a_runtime_panic_keeps_the_child_owned_until_an_explicit_stop_confirms_its_exit() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&notices);
        let observer: RuntimeObserver = Arc::new(move |notice| sink.lock().unwrap().push(notice));
        let runtime =
            SessionRuntime::spawn(echo_shell_spec(80, 24), SessionGeneration::FIRST, observer)
                .expect("shell spawns");
        let process = runtime.process();
        runtime.inject_panic();
        assert!(wait_until(Duration::from_secs(5), || runtime
            .unverified()
            .is_some()));
        assert_eq!(
            runtime.unverified().as_deref(),
            Some(RUNTIME_PANIC_REASON),
            "a panic leaves the session unverified, never exited"
        );
        assert!(runtime.exit().is_none());
        assert!(wait_until(Duration::from_secs(5), || runtime.retired()));
        assert!(
            !wait_until(Duration::from_secs(1), || !process.is_provably_live()),
            "the child survives the runtime panic under a recovery owner"
        );
        assert!(matches!(
            runtime.checkpoint(),
            Err(RuntimeError::Unverified(_)) | Err(RuntimeError::NotLive)
        ));
        let report = runtime.stop().expect("the recovery owner answers a stop");
        assert!(report.exit.is_some(), "the stop confirms the child outcome");
        assert!(!process.is_provably_live());
        assert!(
            notices
                .lock()
                .unwrap()
                .iter()
                .any(|n| matches!(n, RuntimeNotice::Unverified(_))),
            "the observer saw the unverified transition"
        );
    }
}
