//! Cross-platform Ghostty runtime adapter for native PTYs.
//!
//! The libghostty engine is owned by one worker thread. PTY bytes, protocol
//! replies, input, resize, search, selection, persistence, and shutdown all
//! pass through its bounded command queue, so no C handle or borrowed render
//! data crosses a thread or frame boundary.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
// This module is only reached under `cfg(ghostty_native)` - see the `mod
// ghostty_session` gate in `terminal/mod.rs` - so the import needs no gate of
// its own. It used to be spelled twice, once per supported platform.
use paneflow_terminal_ghostty as ghostty;
use parking_lot::RwLock;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::listener::ClipboardGate;
use super::marks::{CommandMark, Osc133Scanner, RawMark, SharedMarkRing};
use super::pty_session::{ForegroundSignalMask, SpawnParams};
use super::service_detector::ServiceOutputTail;
use super::types::{
    Cell, CellFlags, Color, Content, CursorShape, GridLineText, GridMetrics, HyperlinkSource,
    HyperlinkZone, Line, Modes, NamedColor, Point, RenderableCursor, Rgb, SelectionKind,
    SelectionRange, SelectionSide, TerminalWindowSize,
};

// The session is written against exactly two process models: the POSIX one
// (`cfg(unix)`: `getpgid`, `waitid`, `kill(-pid, ...)`, `strsignal`) and the
// Win32 one. Every other target would silently assemble a half-configured
// session type out of whichever arms happened to match, so it is rejected
// here, at one named location, instead. `ghostty_native` is only emitted for
// declared targets - see `ghostty_native_target` in `src-app/build.rs` - so
// this is a backstop against a predicate mistake, never a normal build state.
#[cfg(not(any(unix, windows)))]
compile_error!(
    "terminal::ghostty_session requires a Unix or Windows target; \
     `ghostty_native` must not be emitted for this target"
);

const CONTROL_CAPACITY: usize = 256;
const OUTPUT_BUFFER_COUNT: usize = 4;
const OUTPUT_CHUNK_BYTES: usize = 32 * 1024;
const OUTPUT_POOL_BYTES: usize = OUTPUT_BUFFER_COUNT * OUTPUT_CHUNK_BYTES;
const OUTPUT_BATCH_MAX_BYTES: usize = 128 * 1024;
const OUTPUT_BATCH_MAX_TIME: Duration = Duration::from_millis(1);
const MAX_QUEUED_INPUT_BYTES: usize = NFR_005_MAX_QUEUED_INPUT_BYTES;
const NFR_005_MAX_PENDING_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const NFR_005_MAX_QUEUED_INPUT_BYTES: usize = 1024 * 1024;
const RECENT_OUTPUT_REFRESH_INTERVAL: Duration = Duration::from_millis(300);
const FINAL_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(unix)]
const SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
#[cfg(target_os = "windows")]
const WINDOWS_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_CLIPBOARD_EVENTS: usize = 8;

const _: () = assert!(OUTPUT_POOL_BYTES <= NFR_005_MAX_PENDING_OUTPUT_BYTES);
const _: () = assert!(MAX_QUEUED_INPUT_BYTES <= NFR_005_MAX_QUEUED_INPUT_BYTES);

#[derive(Debug)]
pub(crate) enum GhosttyUiEvent {
    Wakeup(Arc<UiEventState>),
    Title(Arc<UiEventState>),
    WorkingDirectory(Arc<UiEventState>),
    Progress(Arc<UiEventState>),
    Clipboard(Arc<UiEventState>),
    ServiceOutputReady(Arc<UiEventState>),
    ChildExited { code: i32, signal: Option<String> },
    InputRejected(String),
    RuntimeFailed(String),
}

impl GhosttyUiEvent {
    pub(super) fn is_wakeup(&self) -> bool {
        if let Self::Wakeup(events) = self {
            events.wakeup_queued.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct CoalescedSlot<T> {
    latest: Option<T>,
    queued: bool,
}

impl<T> Default for CoalescedSlot<T> {
    fn default() -> Self {
        Self {
            latest: None,
            queued: false,
        }
    }
}

#[derive(Debug, Default)]
struct ClipboardSlot {
    pending: VecDeque<String>,
    queued: bool,
}

#[derive(Debug, Default)]
pub(crate) struct UiEventState {
    wakeup_queued: AtomicBool,
    service_output_queued: AtomicBool,
    title: Mutex<CoalescedSlot<String>>,
    working_directory: Mutex<CoalescedSlot<String>>,
    progress: Mutex<CoalescedSlot<ghostty::ProgressReport>>,
    clipboard: Mutex<ClipboardSlot>,
}

impl UiEventState {
    fn store<T>(slot: &Mutex<CoalescedSlot<T>>, value: T) -> bool {
        let mut slot = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.latest = Some(value);
        if slot.queued {
            false
        } else {
            slot.queued = true;
            true
        }
    }

    fn take<T>(slot: &Mutex<CoalescedSlot<T>>) -> Option<T> {
        let mut slot = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.queued = false;
        slot.latest.take()
    }

    pub(super) fn take_title(&self) -> Option<String> {
        Self::take(&self.title)
    }

    pub(super) fn take_working_directory(&self) -> Option<String> {
        Self::take(&self.working_directory)
    }

    pub(super) fn take_progress(&self) -> Option<ghostty::ProgressReport> {
        Self::take(&self.progress)
    }

    pub(super) fn take_clipboard(&self) -> Vec<String> {
        let mut slot = self
            .clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.queued = false;
        slot.pending.drain(..).collect()
    }

    pub(super) fn acknowledge_wakeup(&self) {
        self.wakeup_queued.store(false, Ordering::Release);
    }

    pub(super) fn acknowledge_service_output(&self) {
        self.service_output_queued.store(false, Ordering::Release);
    }
}

pub(super) struct GhosttyRuntimePending {
    mailbox: Arc<RuntimeMailbox>,
}

pub(super) struct SpawnedGhostty {
    pub(super) child_pid: u32,
    pub(super) cwd: std::path::PathBuf,
}

#[derive(Debug)]
pub(super) enum GhosttyStartError {
    Initialization(anyhow::Error),
    OpenPty(anyhow::Error),
    Spawn(anyhow::Error),
    PostSpawn {
        child_pid: u32,
        error: anyhow::Error,
    },
}

impl std::fmt::Display for GhosttyStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialization(_) => formatter.write_str("Ghostty initialization failed"),
            Self::OpenPty(_) => formatter.write_str("Ghostty PTY open failed"),
            Self::Spawn(_) => formatter.write_str("Ghostty child spawn failed"),
            Self::PostSpawn { .. } => {
                formatter.write_str("Ghostty startup failed after child creation")
            }
        }
    }
}

struct SharedState {
    content: Content,
    modes: Modes,
    metrics: GridMetrics,
}

struct ResizeState {
    requested: TerminalWindowSize,
    submitted: Option<ResizeCommand>,
    applied: Option<TerminalWindowSize>,
    clear_initial_requested: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ResizeCommand {
    size: TerminalWindowSize,
    clear_initial: bool,
}

#[derive(Default)]
struct SelectionUpdateState {
    generation: u64,
    requested: Option<ghostty::SelectionRange>,
    in_flight: Option<(u64, ghostty::SelectionRange)>,
    applied: Option<ghostty::SelectionRange>,
    queued_generation: Option<u64>,
}

struct SessionInner {
    mailbox: Arc<RuntimeMailbox>,
    events_tx: UnboundedSender<GhosttyUiEvent>,
    ui_events: Arc<UiEventState>,
    clipboard_gate: Arc<ClipboardGate>,
    state: RwLock<SharedState>,
    recent_output_lines: RwLock<Arc<[String]>>,
    search_generation: AtomicU64,
    queued_input_bytes: AtomicUsize,
    command_backpressure: AtomicBool,
    promoted: AtomicBool,
    shutdown_sent: AtomicBool,
    exit_published: AtomicBool,
    #[cfg(test)]
    processed_output_bytes: AtomicUsize,
    #[cfg(test)]
    worker_crash_injected: AtomicBool,
    resize: Mutex<ResizeState>,
    selection_anchor: Mutex<Option<(SelectionKind, Point)>>,
    selection_update: Mutex<SelectionUpdateState>,
    marks: SharedMarkRing,
}

#[derive(Clone)]
pub(super) struct GhosttySession {
    inner: Arc<SessionInner>,
}

enum RuntimeMessage {
    Output(Vec<u8>),
    Eof,
    Input(Vec<u8>),
    KeyInput(ghostty::KeyInput),
    MouseInput {
        input: ghostty::MouseInput,
        repeat: usize,
    },
    FocusInput(ghostty::FocusEvent),
    PasteInput {
        text: String,
        allow_unsafe: bool,
    },
    Resize(ResizeCommand),
    Scroll(ghostty::Scroll),
    ScrollToViewportRow(usize),
    ApplySelection(u64),
    SelectWord(ghostty::Point),
    SelectLine(ghostty::Point),
    ClearSelection,
    UpdateAppearance(ghostty::TerminalAppearance),
    SearchChunk {
        start_row: usize,
        max_cells: usize,
        reply: SyncSender<Result<ghostty::SearchChunk, String>>,
    },
    LineTexts {
        lines: Vec<i32>,
        reply: SyncSender<Result<Vec<(i32, String)>, String>>,
    },
    SelectionText(SyncSender<Result<Option<String>, String>>),
    Hyperlink {
        point: ghostty::Point,
        reply: SyncSender<Result<Option<ghostty::Hyperlink>, String>>,
    },
    ExtractScrollback(SyncSender<Result<Option<String>, String>>),
    RestoreScrollback(String),
    #[cfg(test)]
    SimulateWorkerCrash,
    Shutdown,
}

impl RuntimeMessage {
    fn queued_input_bytes(&self) -> Option<usize> {
        match self {
            Self::Input(bytes) => Some(bytes.len()),
            Self::KeyInput(input) => {
                Some(std::mem::size_of::<ghostty::KeyInput>().saturating_add(input.text.len()))
            }
            Self::MouseInput { repeat, .. } => {
                Some(std::mem::size_of::<ghostty::MouseInput>().saturating_add(*repeat))
            }
            Self::FocusInput(_) => Some(std::mem::size_of::<ghostty::FocusEvent>()),
            Self::PasteInput { text, .. } => Some(text.len()),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GhosttyInputSendResult {
    Sent,
    Full,
    Closed,
}

impl GhosttyInputSendResult {
    #[cfg(test)]
    pub(super) fn is_sent(self) -> bool {
        self == Self::Sent
    }
}

#[derive(Default)]
struct MailboxState {
    queue: VecDeque<RuntimeMessage>,
    control_count: usize,
    output_count: usize,
    available_output_buffers: Vec<Vec<u8>>,
    accepting_input: bool,
    accepting_output: bool,
    closed: bool,
}

struct RuntimeMailbox {
    state: Mutex<MailboxState>,
    ready: Condvar,
    output_buffer_ready: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailboxRecvError {
    Timeout,
    Disconnected,
}

impl RuntimeMailbox {
    fn new() -> Self {
        let available_output_buffers = (0..OUTPUT_BUFFER_COUNT)
            .map(|_| vec![0; OUTPUT_CHUNK_BYTES])
            .collect();
        Self {
            state: Mutex::new(MailboxState {
                available_output_buffers,
                accepting_input: true,
                accepting_output: true,
                ..MailboxState::default()
            }),
            ready: Condvar::new(),
            output_buffer_ready: Condvar::new(),
        }
    }

    fn try_send_control(
        &self,
        message: RuntimeMessage,
    ) -> Result<(), TrySendError<RuntimeMessage>> {
        debug_assert!(!matches!(
            message,
            RuntimeMessage::Output(_) | RuntimeMessage::Eof
        ));
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(TrySendError::Disconnected(message));
        }
        if !state.accepting_input && message.queued_input_bytes().is_some() {
            return Err(TrySendError::Disconnected(message));
        }
        if let RuntimeMessage::ScrollToViewportRow(row) = &message
            && let Some(RuntimeMessage::ScrollToViewportRow(queued_row)) = state.queue.back_mut()
        {
            *queued_row = *row;
            return Ok(());
        }
        if state.control_count >= CONTROL_CAPACITY {
            return Err(TrySendError::Full(message));
        }
        state.control_count += 1;
        state.queue.push_back(message);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn take_output_buffer(&self) -> Option<Vec<u8>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.closed || !state.accepting_output {
                return None;
            }
            if let Some(mut buffer) = state.available_output_buffers.pop() {
                buffer.resize(OUTPUT_CHUNK_BYTES, 0);
                return Some(buffer);
            }
            state = self
                .output_buffer_ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    fn recycle_output_buffer(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.available_output_buffers.push(buffer);
        drop(state);
        self.output_buffer_ready.notify_one();
    }

    fn send_output(&self, buffer: Vec<u8>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed || !state.accepting_output || state.output_count >= OUTPUT_BUFFER_COUNT {
            return false;
        }
        state.output_count += 1;
        state.queue.push_back(RuntimeMessage::Output(buffer));
        drop(state);
        self.ready.notify_one();
        true
    }

    fn send_eof(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.control_count += 1;
        state.queue.push_back(RuntimeMessage::Eof);
        drop(state);
        self.ready.notify_one();
    }

    fn pop_front(state: &mut MailboxState) -> Option<RuntimeMessage> {
        let message = state.queue.pop_front()?;
        if matches!(message, RuntimeMessage::Output(_)) {
            state.output_count = state.output_count.saturating_sub(1);
        } else {
            state.control_count = state.control_count.saturating_sub(1);
        }
        Some(message)
    }

    fn recv_timeout(&self, timeout: Duration) -> Result<RuntimeMessage, MailboxRecvError> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(message) = Self::pop_front(&mut state) {
                return Ok(message);
            }
            if state.closed {
                return Err(MailboxRecvError::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MailboxRecvError::Timeout);
            }
            let (next_state, wait) = self
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next_state;
            if wait.timed_out() && state.queue.is_empty() {
                return Err(MailboxRecvError::Timeout);
            }
        }
    }

    fn try_recv_consecutive_output(&self) -> Option<Vec<u8>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(state.queue.front(), Some(RuntimeMessage::Output(_))) {
            return None;
        }
        let RuntimeMessage::Output(bytes) = state.queue.pop_front()? else {
            return None;
        };
        state.output_count = state.output_count.saturating_sub(1);
        Some(bytes)
    }

    fn pending_output_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .output_count
    }

    fn stop_accepting_input(&self) -> usize {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_input = false;
        let mut discarded_input_bytes = 0usize;
        let mut retained = VecDeque::with_capacity(state.queue.len());
        while let Some(message) = state.queue.pop_front() {
            if let Some(bytes) = message.queued_input_bytes() {
                discarded_input_bytes = discarded_input_bytes.saturating_add(bytes);
                state.control_count = state.control_count.saturating_sub(1);
            } else {
                retained.push_back(message);
            }
        }
        state.queue = retained;
        drop(state);
        self.ready.notify_all();
        discarded_input_bytes
    }

    /// Seal the producer side at the bounded drain deadline. The mailbox lock
    /// makes this atomic with `send_output`: every buffer admitted before the
    /// seal remains queued, while no later read can race exit publication.
    #[cfg(any(target_os = "windows", test))]
    fn stop_accepting_output(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_output = false;
        drop(state);
        self.output_buffer_ready.notify_all();
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_input = false;
        state.accepting_output = false;
        state.closed = true;
        drop(state);
        self.ready.notify_all();
        self.output_buffer_ready.notify_all();
    }

    #[cfg(test)]
    fn try_recv(&self) -> Result<RuntimeMessage, std::sync::mpsc::TryRecvError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(message) = Self::pop_front(&mut state) {
            Ok(message)
        } else if state.closed {
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        } else {
            Err(std::sync::mpsc::TryRecvError::Empty)
        }
    }

    #[cfg(test)]
    fn drain(&self) -> Vec<RuntimeMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = self.try_recv() {
            messages.push(message);
        }
        messages
    }
}

struct MailboxCloseGuard(Arc<RuntimeMailbox>);

impl Drop for MailboxCloseGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

enum StartupReport {
    Started(SpawnedGhostty),
    InitializationFailed(anyhow::Error),
    OpenPtyFailed(anyhow::Error),
    SpawnFailed(anyhow::Error),
    PostSpawnFailed {
        child_pid: u32,
        error: anyhow::Error,
    },
}

#[derive(Default)]
struct StartupState {
    child_spawned: AtomicBool,
    child_pid: AtomicU32,
    runtime_started: AtomicBool,
}

impl StartupState {
    fn mark_child_spawned(&self, child_pid: u32) {
        self.child_pid.store(child_pid, Ordering::Relaxed);
        self.child_spawned.store(true, Ordering::Release);
    }

    fn child_pid_if_spawned(&self) -> Option<u32> {
        self.child_spawned
            .load(Ordering::Acquire)
            .then(|| self.child_pid.load(Ordering::Relaxed))
    }

    fn mark_runtime_started(&self) {
        self.runtime_started.store(true, Ordering::Release);
    }

    fn clear_runtime_started(&self) {
        self.runtime_started.store(false, Ordering::Release);
    }

    fn runtime_started(&self) -> bool {
        self.runtime_started.load(Ordering::Acquire)
    }
}

struct StartupChildGuard {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    termination_target: ChildTerminationTarget,
}

impl StartupChildGuard {
    fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        termination_target: ChildTerminationTarget,
    ) -> Self {
        Self {
            child: Some(child),
            termination_target,
        }
    }

    fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut *child, self.termination_target);
        }
    }

    fn take_child(&mut self) -> Option<Box<dyn portable_pty::Child + Send + Sync>> {
        self.child.take()
    }
}

impl Drop for StartupChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

struct RuntimeChildCleanupGuard {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    termination_target: ChildTerminationTarget,
    armed: bool,
}

impl RuntimeChildCleanupGuard {
    fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        termination_target: ChildTerminationTarget,
    ) -> Self {
        Self {
            child,
            termination_target,
            armed: true,
        }
    }

    fn child_mut(&mut self) -> &mut dyn portable_pty::Child {
        &mut *self.child
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RuntimeChildCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            // A second panic while unwinding the runtime would abort the whole
            // process. Cleanup is best effort and the outer boundary publishes
            // a deterministic terminal failure even if a third-party child
            // implementation panics here.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                terminate_child(&mut *self.child, self.termination_target);
            }));
        }
    }
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ChildExitReport {
    code: i32,
    signal: Option<String>,
}

#[cfg(any(test, target_os = "windows"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeLifecyclePhase {
    Running,
    Draining,
    Published,
}

#[cfg(any(test, target_os = "windows"))]
struct RuntimeLifecycle {
    phase: RuntimeLifecyclePhase,
    eof: bool,
    output_sealed: bool,
    exit: Option<ChildExitReport>,
    drain_deadline: Option<Instant>,
}

#[cfg(any(test, target_os = "windows"))]
impl RuntimeLifecycle {
    fn new() -> Self {
        Self {
            phase: RuntimeLifecyclePhase::Running,
            eof: false,
            output_sealed: false,
            exit: None,
            drain_deadline: None,
        }
    }

    fn is_running(&self) -> bool {
        self.phase == RuntimeLifecyclePhase::Running
    }

    fn record_eof(&mut self) {
        self.eof = true;
        self.output_sealed = true;
    }

    fn start_draining(&mut self, exit: ChildExitReport, now: Instant) -> bool {
        if !self.is_running() {
            return false;
        }
        self.phase = RuntimeLifecyclePhase::Draining;
        self.exit = Some(exit);
        self.drain_deadline = now.checked_add(FINAL_DRAIN_TIMEOUT);
        true
    }

    #[cfg(target_os = "windows")]
    fn replace_exit(&mut self, exit: ChildExitReport) {
        if self.phase == RuntimeLifecyclePhase::Draining {
            self.exit = Some(exit);
        }
    }

    fn drain_deadline_reached(&self, now: Instant) -> bool {
        self.phase == RuntimeLifecyclePhase::Draining
            && !self.output_sealed
            && self.drain_deadline.is_none_or(|deadline| now >= deadline)
    }

    fn seal_output(&mut self) {
        self.output_sealed = true;
    }

    fn take_ready_exit(
        &mut self,
        _now: Instant,
        pending_output_count: usize,
    ) -> Option<ChildExitReport> {
        if self.phase != RuntimeLifecyclePhase::Draining {
            return None;
        }
        if pending_output_count > 0 || !self.output_sealed {
            return None;
        }
        self.phase = RuntimeLifecyclePhase::Published;
        self.exit.take()
    }
}

#[cfg(any(test, target_os = "windows"))]
struct PtyCloser<M: Send + 'static> {
    sender: Option<std::sync::mpsc::Sender<M>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(any(test, target_os = "windows"))]
impl<M: Send + 'static> PtyCloser<M> {
    fn new(thread_name: &str) -> std::io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                if let Ok(master) = receiver.recv() {
                    drop(master);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn submit(&mut self, master: M) -> Result<(), M> {
        let Some(sender) = self.sender.take() else {
            return Err(master);
        };
        sender.send(master).map_err(|error| error.0)
    }

    fn join_until(&mut self, deadline: Instant) -> bool {
        loop {
            let Some(worker) = self.worker.as_ref() else {
                return true;
            };
            if worker.is_finished() {
                return self
                    .worker
                    .take()
                    .is_none_or(|worker| worker.join().is_ok());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }
}

#[cfg(any(test, target_os = "windows"))]
impl<M: Send + 'static> Drop for PtyCloser<M> {
    fn drop(&mut self) {
        drop(self.sender.take());
        if self
            .worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            let _ = self.worker.take().and_then(|worker| worker.join().ok());
        }
    }
}

/// Own the ConPTY master and guarantee that even a runtime unwind transfers
/// `ClosePseudoConsole` to a dedicated thread. The runtime must remain free to
/// consume its fixed output pool while that API drains the output pipe.
#[cfg(any(test, target_os = "windows"))]
struct DrainablePtyMaster<M: Send + 'static> {
    master: Option<M>,
    closer: PtyCloser<M>,
}

#[cfg(any(test, target_os = "windows"))]
impl<M: Send + 'static> DrainablePtyMaster<M> {
    fn new(master: M, closer: PtyCloser<M>) -> Self {
        Self {
            master: Some(master),
            closer,
        }
    }

    #[cfg(target_os = "windows")]
    fn get(&self) -> Option<&M> {
        self.master.as_ref()
    }

    fn close_async(&mut self) -> bool {
        let Some(master) = self.master.take() else {
            return true;
        };
        match self.closer.submit(master) {
            Ok(()) => true,
            Err(master) => {
                // A dead closer thread is already a fatal invariant failure.
                // Leaking the handle here preserves bounded shutdown instead
                // of risking a synchronous ClosePseudoConsole deadlock.
                std::mem::forget(master);
                false
            }
        }
    }

    fn join_until(&mut self, deadline: Instant) -> bool {
        self.closer.join_until(deadline)
    }
}

#[cfg(any(test, target_os = "windows"))]
impl<M: Send + 'static> Drop for DrainablePtyMaster<M> {
    fn drop(&mut self) {
        let _ = self.close_async();
    }
}

#[cfg(any(test, target_os = "windows"))]
fn close_pty_for_final_drain<W, M: Send + 'static>(
    writer: &mut Option<W>,
    master: &mut DrainablePtyMaster<M>,
) -> bool {
    drop(writer.take());
    master.close_async()
}

fn publish_child_exit_once(inner: &SessionInner, code: i32, signal: Option<String>) {
    if inner
        .exit_published
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::ChildExited { code, signal });
    }
}

fn release_queued_input_bytes(inner: &SessionInner, released: usize) {
    if released == 0 {
        return;
    }
    let _ = inner
        .queued_input_bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            Some(queued.saturating_sub(released))
        });
}

fn stop_session_input(inner: &SessionInner) {
    let discarded = inner.mailbox.stop_accepting_input();
    release_queued_input_bytes(inner, discarded);
}

impl GhosttySession {
    #[cfg(test)]
    pub(super) fn pending(
        size: TerminalWindowSize,
    ) -> (
        Self,
        GhosttyRuntimePending,
        UnboundedReceiver<GhosttyUiEvent>,
    ) {
        Self::pending_with_clipboard_gate(size, Arc::new(ClipboardGate::default()))
    }

    pub(super) fn pending_with_clipboard_gate(
        size: TerminalWindowSize,
        clipboard_gate: Arc<ClipboardGate>,
    ) -> (
        Self,
        GhosttyRuntimePending,
        UnboundedReceiver<GhosttyUiEvent>,
    ) {
        let mailbox = Arc::new(RuntimeMailbox::new());
        let (events_tx, events_rx) = unbounded();
        let session = Self {
            inner: Arc::new(SessionInner {
                mailbox: mailbox.clone(),
                events_tx,
                ui_events: Arc::new(UiEventState::default()),
                clipboard_gate,
                state: RwLock::new(SharedState {
                    content: blank_content(size.cols.max(1), size.rows.max(1)),
                    modes: Modes::empty(),
                    metrics: initial_grid_metrics(size.cols.max(1), size.rows.max(1)),
                }),
                recent_output_lines: RwLock::new(Arc::from(Vec::<String>::new())),
                search_generation: AtomicU64::new(0),
                queued_input_bytes: AtomicUsize::new(0),
                command_backpressure: AtomicBool::new(false),
                promoted: AtomicBool::new(false),
                shutdown_sent: AtomicBool::new(false),
                exit_published: AtomicBool::new(false),
                #[cfg(test)]
                processed_output_bytes: AtomicUsize::new(0),
                #[cfg(test)]
                worker_crash_injected: AtomicBool::new(false),
                resize: Mutex::new(ResizeState {
                    requested: size,
                    submitted: None,
                    applied: Some(size),
                    clear_initial_requested: false,
                }),
                selection_anchor: Mutex::new(None),
                selection_update: Mutex::new(SelectionUpdateState::default()),
                marks: Arc::new(Mutex::new(Default::default())),
            }),
        };
        (session, GhosttyRuntimePending { mailbox }, events_rx)
    }

    pub(super) fn start(
        &self,
        pending: GhosttyRuntimePending,
        params: SpawnParams,
        signal_mask: Option<ForegroundSignalMask>,
        max_scrollback: usize,
    ) -> Result<SpawnedGhostty, GhosttyStartError> {
        let (startup_tx, startup_rx) = sync_channel(1);
        let startup_state = Arc::new(StartupState::default());
        let inner = self.inner.clone();
        let runtime_mailbox = pending.mailbox.clone();
        let runtime_startup_state = startup_state.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-ghostty-runtime".into())
            .spawn(move || {
                let boundary_inner = inner.clone();
                let boundary_startup_state = runtime_startup_state.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_runtime(
                        inner,
                        runtime_mailbox,
                        params,
                        signal_mask,
                        max_scrollback,
                        startup_tx,
                        runtime_startup_state,
                    );
                }));
                if result.is_err() && boundary_startup_state.runtime_started() {
                    boundary_inner.shutdown_sent.store(true, Ordering::Release);
                    stop_session_input(&boundary_inner);
                    let _ = boundary_inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                            "Ghostty runtime worker terminated unexpectedly".to_owned(),
                        ));
                    publish_child_exit_once(&boundary_inner, -1, None);
                }
            })
        {
            pending.mailbox.close();
            return Err(GhosttyStartError::Initialization(
                anyhow::Error::new(error).context("could not start Ghostty runtime thread"),
            ));
        }

        match startup_rx.recv() {
            Ok(StartupReport::Started(spawned)) => Ok(spawned),
            Ok(StartupReport::InitializationFailed(error)) => {
                Err(GhosttyStartError::Initialization(error))
            }
            Ok(StartupReport::OpenPtyFailed(error)) => Err(GhosttyStartError::OpenPty(error)),
            Ok(StartupReport::SpawnFailed(error)) => Err(GhosttyStartError::Spawn(error)),
            Ok(StartupReport::PostSpawnFailed { child_pid, error }) => {
                Err(GhosttyStartError::PostSpawn { child_pid, error })
            }
            Err(error) => {
                let error =
                    anyhow::anyhow!("Ghostty runtime exited before startup completed: {error}");
                if let Some(child_pid) = startup_state.child_pid_if_spawned() {
                    Err(GhosttyStartError::PostSpawn { child_pid, error })
                } else {
                    Err(GhosttyStartError::Initialization(error))
                }
            }
        }
    }

    pub(super) fn promote(&self) {
        self.inner.promoted.store(true, Ordering::Release);
    }

    pub(super) fn is_promoted(&self) -> bool {
        self.inner.promoted.load(Ordering::Acquire)
    }

    pub(super) fn marks(&self) -> SharedMarkRing {
        self.inner.marks.clone()
    }

    pub(super) fn write(&self, bytes: Vec<u8>) -> GhosttyInputSendResult {
        if bytes.is_empty() {
            return GhosttyInputSendResult::Sent;
        }
        self.enqueue_input(RuntimeMessage::Input(bytes))
    }

    pub(super) fn write_key(&self, input: ghostty::KeyInput) -> GhosttyInputSendResult {
        self.enqueue_input(RuntimeMessage::KeyInput(input))
    }

    pub(super) fn write_mouse(
        &self,
        input: ghostty::MouseInput,
        repeat: usize,
    ) -> GhosttyInputSendResult {
        if repeat == 0 {
            return GhosttyInputSendResult::Sent;
        }
        self.enqueue_input(RuntimeMessage::MouseInput { input, repeat })
    }

    pub(super) fn write_focus(&self, event: ghostty::FocusEvent) -> GhosttyInputSendResult {
        self.enqueue_input(RuntimeMessage::FocusInput(event))
    }

    pub(super) fn write_paste(&self, text: String, allow_unsafe: bool) -> GhosttyInputSendResult {
        if text.is_empty() {
            return GhosttyInputSendResult::Sent;
        }
        self.enqueue_input(RuntimeMessage::PasteInput { text, allow_unsafe })
    }

    fn enqueue_input(&self, message: RuntimeMessage) -> GhosttyInputSendResult {
        if self.inner.shutdown_sent.load(Ordering::Acquire) {
            return GhosttyInputSendResult::Closed;
        }
        let len = message
            .queued_input_bytes()
            .expect("enqueue_input only accepts input messages");
        let reserved = self.inner.queued_input_bytes.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |queued| {
                queued
                    .checked_add(len)
                    .filter(|next| *next <= MAX_QUEUED_INPUT_BYTES)
            },
        );
        if reserved.is_err() {
            self.inner
                .command_backpressure
                .store(true, Ordering::Release);
            return GhosttyInputSendResult::Full;
        }
        match self.inner.mailbox.try_send_control(message) {
            Ok(()) => GhosttyInputSendResult::Sent,
            Err(TrySendError::Full(message)) => {
                let released = message
                    .queued_input_bytes()
                    .expect("try_send returns the submitted input message");
                self.inner
                    .queued_input_bytes
                    .fetch_sub(released, Ordering::AcqRel);
                self.inner
                    .command_backpressure
                    .store(true, Ordering::Release);
                GhosttyInputSendResult::Full
            }
            Err(TrySendError::Disconnected(message)) => {
                let released = message
                    .queued_input_bytes()
                    .expect("try_send returns the submitted input message");
                self.inner
                    .queued_input_bytes
                    .fetch_sub(released, Ordering::AcqRel);
                GhosttyInputSendResult::Closed
            }
        }
    }

    pub(super) fn queued_input_bytes(&self) -> usize {
        self.inner.queued_input_bytes.load(Ordering::Acquire)
    }

    pub(super) fn resize(&self, size: TerminalWindowSize) {
        if self.inner.shutdown_sent.load(Ordering::Acquire) {
            return;
        }
        let size = normalized_window_size(size);
        let mut resize = self
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resize.requested = size;
        self.submit_requested_resize(&mut resize);
    }

    pub(super) fn retry_backpressured_commands(&self) {
        {
            let mut resize = self
                .inner
                .resize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.submit_requested_resize(&mut resize);
        }
        let mut selection = self
            .inner
            .selection_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.submit_requested_selection(&mut selection);
    }

    fn submit_requested_resize(&self, resize: &mut ResizeState) {
        if self.inner.shutdown_sent.load(Ordering::Acquire) {
            return;
        }
        if resize.submitted.is_some()
            || (resize.applied == Some(resize.requested) && !resize.clear_initial_requested)
        {
            return;
        }
        let command = ResizeCommand {
            size: resize.requested,
            clear_initial: resize.clear_initial_requested,
        };
        match self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::Resize(command))
        {
            Ok(()) => {
                resize.submitted = Some(command);
                if command.clear_initial {
                    resize.clear_initial_requested = false;
                }
            }
            Err(TrySendError::Full(_)) => {
                self.inner
                    .command_backpressure
                    .store(true, Ordering::Release);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn begin_selection(&self, range: ghostty::SelectionRange) {
        let mut selection = self
            .inner
            .selection_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        selection.generation = selection.generation.wrapping_add(1);
        selection.requested = Some(range);
        selection.in_flight = None;
        selection.applied = None;
        selection.queued_generation = None;
        self.submit_requested_selection(&mut selection);
    }

    fn queue_selection(&self, range: ghostty::SelectionRange) {
        let mut selection = self
            .inner
            .selection_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if selection.requested.as_ref() == Some(&range)
            || (selection.requested.is_none()
                && selection
                    .in_flight
                    .as_ref()
                    .is_some_and(|(generation, pending)| {
                        *generation == selection.generation && pending == &range
                    }))
            || (selection.requested.is_none()
                && selection.in_flight.is_none()
                && selection.applied.as_ref() == Some(&range))
        {
            return;
        }
        selection.requested = Some(range);
        self.submit_requested_selection(&mut selection);
    }

    fn submit_requested_selection(&self, selection: &mut SelectionUpdateState) {
        if selection.queued_generation == Some(selection.generation)
            || selection.requested.is_none()
        {
            return;
        }
        let generation = selection.generation;
        match self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::ApplySelection(generation))
        {
            Ok(()) => selection.queued_generation = Some(generation),
            Err(TrySendError::Full(_)) => self
                .inner
                .command_backpressure
                .store(true, Ordering::Release),
            Err(TrySendError::Disconnected(_)) => selection.requested = None,
        }
    }

    fn invalidate_selection_updates(&self) {
        let mut selection = self
            .inner
            .selection_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        selection.generation = selection.generation.wrapping_add(1);
        selection.requested = None;
        selection.in_flight = None;
        selection.applied = None;
        selection.queued_generation = None;
    }

    pub(super) fn render_content(
        &self,
        window_size: TerminalWindowSize,
        _first_visible_row: i32,
        _last_visible_row: i32,
        clear_on_resize: bool,
    ) -> (Content, bool) {
        let window_size = normalized_window_size(window_size);
        let content = self.inner.state.read().content.clone();
        let mut initial_clear_consumed = false;
        if clear_on_resize {
            let mut resize = self
                .inner
                .resize
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let requested_grid_matches = resize.requested.cols == window_size.cols
                && resize.requested.rows == window_size.rows;
            let applied_grid_matches = resize.applied.is_some_and(|applied| {
                applied.cols == window_size.cols && applied.rows == window_size.rows
            });
            let initial_resize = content.cols != window_size.cols
                || content.rows != window_size.rows
                || !requested_grid_matches
                || !applied_grid_matches;
            if clear_on_resize && initial_resize {
                resize.requested = window_size;
                resize.clear_initial_requested = true;
                initial_clear_consumed = true;
                self.submit_requested_resize(&mut resize);
            }
        }
        (content, initial_clear_consumed)
    }

    pub(super) fn modes(&self) -> Modes {
        self.inner.state.read().modes
    }

    pub(super) fn recent_output_lines(&self) -> Arc<[String]> {
        self.inner.recent_output_lines.read().clone()
    }

    #[cfg(test)]
    pub(super) fn processed_output_bytes_for_test(&self) -> usize {
        self.inner.processed_output_bytes.load(Ordering::Acquire)
    }

    pub(super) fn grid_metrics(&self) -> GridMetrics {
        self.inner.state.read().metrics
    }

    pub(super) fn scroll(&self, scroll: ghostty::Scroll) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::Scroll(scroll))
            .is_ok()
    }

    pub(super) fn scroll_to_viewport_row(&self, row: usize) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::ScrollToViewportRow(row))
            .is_ok()
    }

    pub(super) fn start_selection(&self, kind: SelectionKind, point: Point) {
        *self
            .inner
            .selection_anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((kind, point));
        let point = ghostty_point(point);
        match kind {
            SelectionKind::Simple => self.begin_selection(ghostty::SelectionRange {
                start: point,
                end: point,
                rectangle: false,
            }),
            SelectionKind::Semantic => {
                self.invalidate_selection_updates();
                let _ = self
                    .inner
                    .mailbox
                    .try_send_control(RuntimeMessage::SelectWord(point));
            }
            SelectionKind::Lines => {
                self.invalidate_selection_updates();
                let _ = self
                    .inner
                    .mailbox
                    .try_send_control(RuntimeMessage::SelectLine(point));
            }
        }
    }

    pub(super) fn update_selection(&self, point: Point, _side: SelectionSide) -> Option<String> {
        let anchor = *self
            .inner
            .selection_anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (kind, start) = anchor?;
        let range = ghostty::SelectionRange {
            start: ghostty_point(start),
            end: ghostty_point(point),
            rectangle: false,
        };
        if matches!(kind, SelectionKind::Simple) {
            self.queue_selection(range);
        }
        // Formatting the selection here would block GPUI on the runtime thread
        // for every pointer event. Ghostty updates PRIMARY when the gesture is
        // committed, which Paneflow already does in `finish_selection`.
        None
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let text = self
            .request(RuntimeMessage::SelectionText)
            .and_then(Result::ok)
            .flatten();
        let kind = self
            .inner
            .selection_anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|(kind, _)| *kind);
        filter_copyable_selection_text(kind, self.selection_range(), text)
    }

    pub(super) fn clear_selection(&self) {
        *self
            .inner
            .selection_anchor
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        self.invalidate_selection_updates();
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::ClearSelection);
    }

    pub(super) fn selection_range(&self) -> Option<SelectionRange> {
        self.inner.state.read().content.selection
    }

    pub(super) fn refresh_appearance(&self) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::UpdateAppearance(
                current_ghostty_appearance(),
            ))
            .is_ok()
    }

    pub(super) fn hyperlink_at(&self, point: Point) -> Option<HyperlinkZone> {
        self.request(|reply| RuntimeMessage::Hyperlink {
            point: ghostty_point(point),
            reply,
        })
        .and_then(Result::ok)
        .flatten()
        .map(|link| HyperlinkZone {
            uri: link.uri.clone(),
            id: String::new(),
            start: point,
            end: point,
            is_openable: super::element::is_url_scheme_openable(&link.uri),
            source: HyperlinkSource::Osc8,
            line: None,
            col: None,
        })
    }

    pub(super) fn line_text_at(&self, point: Point) -> Option<GridLineText> {
        let state = self.inner.state.read();
        let mut cells: Vec<_> = state
            .content
            .cells
            .iter()
            .filter(|cell| cell.point.line == point.line)
            .collect();
        cells.sort_by_key(|cell| cell.point.column);
        if cells.is_empty() {
            return None;
        }
        let mut text = String::new();
        let mut char_to_column = Vec::new();
        for cell in cells {
            if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                continue;
            }
            char_to_column.push(cell.point.column.0);
            text.push(cell.c);
            if let Some(zero_width) = &cell.zerowidth {
                for character in zero_width.iter() {
                    char_to_column.push(cell.point.column.0);
                    text.push(*character);
                }
            }
        }
        Some(GridLineText {
            line: point.line,
            text,
            char_to_column,
        })
    }

    pub(super) fn search(&self, query: &str, regex: bool) -> crate::search::SearchResult {
        self.search_with_cancel(query, regex, &AtomicBool::new(false))
    }

    pub(super) fn search_with_cancel(
        &self,
        query: &str,
        regex: bool,
        cancelled: &AtomicBool,
    ) -> crate::search::SearchResult {
        let mut search = match ghostty::SearchEngine::new(query, regex) {
            Ok(search) => search,
            Err(error) => {
                return crate::search::SearchResult {
                    matches: Vec::new(),
                    regex_error: Some(error.to_string()),
                    truncated: false,
                };
            }
        };
        if search.is_done() {
            return search_result_from_ghostty(search.finish(false));
        }

        let generation = self
            .inner
            .search_generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let mut next_row = 0usize;
        let mut scanned_cells = 0usize;
        loop {
            if cancelled.load(Ordering::Acquire)
                || self.inner.search_generation.load(Ordering::Acquire) != generation
            {
                return search_result_from_ghostty(search.finish(true));
            }
            let remaining = ghostty::MAX_SEARCH_CELLS.saturating_sub(scanned_cells);
            if remaining == 0 {
                return search_result_from_ghostty(search.finish(true));
            }
            let requested_cells = remaining.min(ghostty::SEARCH_CHUNK_CELLS);
            let chunk = self
                .request(|reply| RuntimeMessage::SearchChunk {
                    start_row: next_row,
                    max_cells: requested_cells,
                    reply,
                })
                .and_then(Result::ok);
            let Some(chunk) = chunk else {
                return search_result_from_ghostty(search.finish(true));
            };
            if chunk.next_row == next_row && chunk.next_row < chunk.total_rows {
                return search_result_from_ghostty(search.finish(true));
            }
            scanned_cells =
                scanned_cells.saturating_add(chunk.lines.len().saturating_mul(chunk.cols));
            for line in chunk.lines {
                if !search.push_line(line.line, &line.text, &line.char_to_column) {
                    return search_result_from_ghostty(search.finish(false));
                }
            }
            if chunk.next_row >= chunk.total_rows {
                return search_result_from_ghostty(search.finish(false));
            }
            next_row = chunk.next_row;
        }
    }

    pub(super) fn search_scrollback(
        &self,
        query: &str,
        max_matches: usize,
    ) -> (Vec<(i32, String)>, bool) {
        if query.is_empty() || max_matches == 0 {
            return (Vec::new(), false);
        }
        let search = self.search(query, false);
        let mut seen = std::collections::HashSet::new();
        let mut rows = Vec::new();
        let mut hit_cap = search.truncated;
        for found in &search.matches {
            if seen.insert(found.start.line.0) {
                rows.push(found.start.line.0);
                if rows.len() >= max_matches {
                    hit_cap = true;
                    break;
                }
            }
        }
        let lines = self
            .request(|reply| RuntimeMessage::LineTexts { lines: rows, reply })
            .and_then(Result::ok);
        match lines {
            Some(mut lines) => {
                for (_, text) in &mut lines {
                    let trimmed_len = text.trim_end().len();
                    text.truncate(trimmed_len);
                }
                (lines, hit_cap)
            }
            None => (Vec::new(), true),
        }
    }

    pub(super) fn extract_scrollback(&self) -> Option<String> {
        self.request(RuntimeMessage::ExtractScrollback)
            .and_then(Result::ok)
            .flatten()
    }

    pub(super) fn restore_scrollback(&self, text: &str) {
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::RestoreScrollback(text.to_owned()));
    }

    #[cfg(test)]
    pub(super) fn simulate_worker_crash_for_test(&self) -> bool {
        if self.inner.shutdown_sent.load(Ordering::Acquire)
            || self
                .inner
                .worker_crash_injected
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return false;
        }
        if self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::SimulateWorkerCrash)
            .is_ok()
        {
            true
        } else {
            self.inner
                .worker_crash_injected
                .store(false, Ordering::Release);
            false
        }
    }

    pub(super) fn shutdown(&self) {
        if !self.inner.shutdown_sent.swap(true, Ordering::AcqRel) {
            stop_session_input(&self.inner);
            let _ = self
                .inner
                .mailbox
                .try_send_control(RuntimeMessage::Shutdown);
        }
    }

    fn request<T>(&self, command: impl FnOnce(SyncSender<T>) -> RuntimeMessage) -> Option<T> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.inner
            .mailbox
            .try_send_control(command(reply_tx))
            .ok()?;
        reply_rx.recv_timeout(Duration::from_secs(1)).ok()
    }
}

fn search_result_from_ghostty(result: ghostty::SearchResult) -> crate::search::SearchResult {
    crate::search::SearchResult {
        matches: result
            .matches
            .into_iter()
            .map(|found| crate::search::SearchMatch {
                start: point_from_ghostty(found.start),
                end: point_from_ghostty(found.end),
            })
            .collect(),
        regex_error: result.regex_error,
        truncated: result.truncated,
    }
}

fn reject_input(inner: &SessionInner, input_kind: &'static str, error: impl std::fmt::Display) {
    let _ = inner
        .events_tx
        .unbounded_send(GhosttyUiEvent::InputRejected(format!(
            "Ghostty {input_kind} encoder rejected input: {error}"
        )));
}

fn write_input_bytes<W: Write>(
    inner: &SessionInner,
    writer: &mut Option<W>,
    bytes: &[u8],
    runtime_failed: &mut bool,
) {
    if bytes.is_empty() {
        return;
    }
    let Some(active_writer) = writer.as_mut() else {
        return;
    };
    if let Err(error) = active_writer
        .write_all(bytes)
        .and_then(|()| active_writer.flush())
    {
        let expected_close = matches!(
            error.kind(),
            ErrorKind::BrokenPipe | ErrorKind::NotConnected
        );
        if !expected_close {
            let _ = inner
                .events_tx
                .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                    "Ghostty PTY write failed: {error}"
                )));
        }
        #[cfg(unix)]
        {
            *runtime_failed = !expected_close;
        }
        #[cfg(target_os = "windows")]
        {
            *runtime_failed = true;
        }
    }
}

fn run_runtime(
    inner: Arc<SessionInner>,
    mailbox: Arc<RuntimeMailbox>,
    params: SpawnParams,
    signal_mask: Option<ForegroundSignalMask>,
    max_scrollback: usize,
    startup_tx: SyncSender<StartupReport>,
    startup_state: Arc<StartupState>,
) {
    let _mailbox_close = MailboxCloseGuard(mailbox.clone());
    let initial_size = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .requested;
    let ghostty_size = match window_size(initial_size) {
        Ok(size) => size,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::InitializationFailed(anyhow::anyhow!(
                error.to_string()
            )));
            return;
        }
    };
    let appearance = current_ghostty_appearance();
    let mut terminal = match ghostty::DisplayTerminal::new(ghostty_size, max_scrollback, appearance)
    {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::InitializationFailed(anyhow::anyhow!(
                error.to_string()
            )));
            return;
        }
    };
    if let Err(error) = refresh_shared_state(&inner, &mut terminal) {
        let _ = startup_tx.send(StartupReport::InitializationFailed(anyhow::anyhow!(error)));
        return;
    }

    let pair = match native_pty_system().openpty(pty_size(initial_size)) {
        Ok(pair) => pair,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::OpenPtyFailed(
                error.context("failed to open native PTY"),
            ));
            return;
        }
    };
    #[cfg(unix)]
    let master = pair.master;
    #[cfg(target_os = "windows")]
    let master_closer = match PtyCloser::<Box<dyn portable_pty::MasterPty + Send>>::new(
        "paneflow-ghostty-pty-closer",
    ) {
        Ok(closer) => closer,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::OpenPtyFailed(
                anyhow::Error::new(error).context("failed to start ConPTY close worker"),
            ));
            return;
        }
    };
    #[cfg(target_os = "windows")]
    let mut master = DrainablePtyMaster::new(pair.master, master_closer);
    let mut command = CommandBuilder::new(&params.shell);
    command.args(&params.extra_args);
    command.cwd(&params.cwd);
    for (key, value) in &params.env {
        command.env(key, value);
    }
    // Match Ghostty and cmux: keep the portable TERM contract while exposing
    // the renderer identity that terminal applications use for capabilities.
    command.env("TERM_PROGRAM", "ghostty");
    command.env("TERM_PROGRAM_VERSION", ghostty::GHOSTTY_APP_VERSION);

    #[cfg(unix)]
    let child = {
        let restore_mask = super::pty_session::apply_thread_signal_mask(signal_mask);
        let child = pair.slave.spawn_command(command);
        super::pty_session::restore_thread_signal_mask(restore_mask);
        child
    };
    #[cfg(not(unix))]
    let child = {
        let _ = signal_mask;
        pair.slave.spawn_command(command)
    };
    let child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::SpawnFailed(
                error.context("failed to spawn shell in PTY"),
            ));
            return;
        }
    };
    let child_pid = child.process_id().unwrap_or(0);
    startup_state.mark_child_spawned(child_pid);
    let termination_target = child_termination_target(child_pid);
    let mut startup_child = StartupChildGuard::new(child, termination_target);
    #[cfg(unix)]
    let reader = master.try_clone_reader();
    #[cfg(target_os = "windows")]
    let reader = master
        .get()
        .ok_or_else(|| anyhow::anyhow!("ConPTY master unavailable before reader clone"))
        .and_then(|master| master.try_clone_reader());
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::PostSpawnFailed {
                child_pid,
                error: error.context("failed to clone PTY reader"),
            });
            return;
        }
    };
    #[cfg(unix)]
    let writer = master.take_writer();
    #[cfg(target_os = "windows")]
    let writer = master
        .get()
        .ok_or_else(|| anyhow::anyhow!("ConPTY master unavailable before writer take"))
        .and_then(|master| master.take_writer());
    let writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::PostSpawnFailed {
                child_pid,
                error: error.context("failed to take PTY writer"),
            });
            return;
        }
    };
    let output_mailbox = mailbox.clone();
    let reader_worker = match std::thread::Builder::new()
        .name("paneflow-ghostty-pty-reader".into())
        .spawn(move || read_pty(reader, output_mailbox))
    {
        Ok(worker) => worker,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::PostSpawnFailed {
                child_pid,
                error: anyhow::Error::new(error).context("failed to start PTY reader"),
            });
            return;
        }
    };
    #[cfg(target_os = "windows")]
    let mut reader_worker = Some(reader_worker);
    #[cfg(unix)]
    drop(reader_worker);

    drop(pair.slave);
    startup_state.mark_runtime_started();
    if startup_tx
        .send(StartupReport::Started(SpawnedGhostty {
            child_pid,
            cwd: params.cwd,
        }))
        .is_err()
    {
        startup_state.clear_runtime_started();
        startup_child.terminate();
        return;
    }
    let Some(child) = startup_child.take_child() else {
        return;
    };
    let mut child = RuntimeChildCleanupGuard::new(child, termination_target);
    let mut writer = Some(writer);

    let mut marks_scanner = Osc133Scanner::default();
    let mut service_output_tail = ServiceOutputTail::default();
    let mut last_recent_output_refresh = None;
    let mut recent_output_pending = false;
    #[cfg(unix)]
    let mut eof = false;
    #[cfg(unix)]
    let mut exit = None;
    #[cfg(unix)]
    let mut exit_seen_at = None;
    #[cfg(unix)]
    let mut child_cleaned = false;
    #[cfg(target_os = "windows")]
    let mut lifecycle = RuntimeLifecycle::new();
    #[cfg(target_os = "windows")]
    let mut shutdown_requested = false;
    #[cfg(target_os = "windows")]
    let mut child_reaped = false;
    #[cfg(target_os = "windows")]
    let mut child_wait_failure_reported = false;
    let mut runtime_failed = false;

    loop {
        if inner.shutdown_sent.load(Ordering::Acquire) {
            #[cfg(unix)]
            {
                if exit.is_none() {
                    terminate_child(child.child_mut(), termination_target);
                    child.disarm();
                    break;
                }
            }
            #[cfg(target_os = "windows")]
            {
                shutdown_requested = true;
            }
        }
        #[cfg(target_os = "windows")]
        if shutdown_requested && lifecycle.is_running() {
            begin_windows_shutdown(
                &inner,
                &mut writer,
                child.child_mut(),
                child_pid,
                &mut lifecycle,
                &mut child_reaped,
                &mut master,
            );
        }
        match mailbox.recv_timeout(Duration::from_millis(10)) {
            Ok(RuntimeMessage::Output(bytes)) => {
                if let Err(error) = process_output_batch(
                    &inner,
                    &mailbox,
                    &mut terminal,
                    &mut writer,
                    &mut marks_scanner,
                    &mut service_output_tail,
                    &mut last_recent_output_refresh,
                    &mut recent_output_pending,
                    bytes,
                ) {
                    if !runtime_failed {
                        let _ = inner
                            .events_tx
                            .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
                    }
                    runtime_failed = true;
                }
            }
            Ok(RuntimeMessage::Eof) => {
                #[cfg(unix)]
                {
                    eof = true;
                }
                #[cfg(target_os = "windows")]
                {
                    lifecycle.record_eof();
                }
            }
            Ok(RuntimeMessage::Input(bytes)) => {
                release_queued_input_bytes(&inner, bytes.len());
                write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed);
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::KeyInput(input)) => {
                release_queued_input_bytes(
                    &inner,
                    std::mem::size_of::<ghostty::KeyInput>().saturating_add(input.text.len()),
                );
                match terminal.encode_key(&input) {
                    Ok(bytes) => {
                        write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                    }
                    Err(error) => reject_input(&inner, "key", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::MouseInput { input, repeat }) => {
                release_queued_input_bytes(
                    &inner,
                    std::mem::size_of::<ghostty::MouseInput>().saturating_add(repeat),
                );
                for _ in 0..repeat {
                    match terminal.encode_mouse(input) {
                        Ok(bytes) => {
                            write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                        }
                        Err(error) => {
                            reject_input(&inner, "mouse", error);
                            break;
                        }
                    }
                }
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::FocusInput(event)) => {
                release_queued_input_bytes(&inner, std::mem::size_of::<ghostty::FocusEvent>());
                match terminal.encode_focus(event) {
                    Ok(bytes) => {
                        write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                    }
                    Err(error) => reject_input(&inner, "focus", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::PasteInput { text, allow_unsafe }) => {
                release_queued_input_bytes(&inner, text.len());
                match terminal.encode_paste(&text, allow_unsafe) {
                    Ok(bytes) => {
                        write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                    }
                    Err(error) => reject_input(&inner, "paste", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::Resize(command)) => {
                let size = command.size;
                #[cfg(unix)]
                let resize_allowed = true;
                #[cfg(target_os = "windows")]
                let resize_allowed = lifecycle.is_running();
                if !resize_allowed {
                    complete_resize_during_drain(&inner);
                } else {
                    let resized = window_size(size)
                        .map_err(|error| error.to_string())
                        .and_then(|ghostty_size| {
                            terminal
                                .resize(ghostty_size)
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|()| {
                            if command.clear_initial {
                                terminal
                                    .clear_screen_and_scrollback()
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        })
                        .and_then(|()| {
                            #[cfg(unix)]
                            let active_master = Some(master.as_ref());
                            #[cfg(target_os = "windows")]
                            let active_master = master.get().map(|master| master.as_ref());
                            active_master
                                .ok_or_else(|| {
                                    "Ghostty PTY master closed during resize".to_owned()
                                })?
                                .resize(pty_size(size))
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|()| {
                            update_shared_state(&inner, &mut terminal)?;
                            queue_wakeup(&inner);
                            Ok(())
                        });
                    let resize_succeeded = match resized {
                        Ok(()) => true,
                        Err(error) => {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty resize to {}x{} failed: {error}",
                                size.cols,
                                size.rows,
                            );
                            false
                        }
                    };
                    complete_resize(&inner, command, resize_succeeded);
                }
            }
            Ok(RuntimeMessage::Scroll(scroll)) => {
                terminal.scroll(scroll);
                if let Err(error) = refresh_shared_state(&inner, &mut terminal) {
                    log::warn!(target: "paneflow::terminal::ghostty", "Ghostty scroll failed: {error}");
                }
            }
            Ok(RuntimeMessage::ScrollToViewportRow(row)) => {
                let result = terminal
                    .scroll_to_viewport_row(row)
                    .map_err(|error| error.to_string())
                    .and_then(|()| refresh_shared_state(&inner, &mut terminal));
                if let Err(error) = result {
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty absolute scroll failed: {error}"
                    );
                }
            }
            Ok(RuntimeMessage::ApplySelection(generation)) => {
                let range = {
                    let mut selection = inner
                        .selection_update
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if selection.queued_generation == Some(generation) {
                        selection.queued_generation = None;
                    }
                    if selection.generation != generation {
                        None
                    } else {
                        let range = selection.requested.take();
                        selection.in_flight =
                            range.as_ref().map(|range| (generation, range.clone()));
                        range
                    }
                };
                if let Some(range) = range {
                    let shared_range = selection_range_from_ghostty(range.clone());
                    match terminal.set_selection(range.clone()) {
                        Ok(()) => {
                            let mut selection = inner
                                .selection_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if selection.in_flight.as_ref().is_some_and(
                                |(pending_generation, pending)| {
                                    *pending_generation == generation && pending == &range
                                },
                            ) {
                                selection.in_flight = None;
                            }
                            let publish = selection.generation == generation;
                            if publish {
                                selection.applied = Some(range);
                            }
                            drop(selection);
                            if publish {
                                update_shared_selection(&inner, Some(shared_range));
                            }
                        }
                        Err(error) => {
                            let mut selection = inner
                                .selection_update
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            if selection.in_flight.as_ref().is_some_and(
                                |(pending_generation, pending)| {
                                    *pending_generation == generation && pending == &range
                                },
                            ) {
                                selection.in_flight = None;
                            }
                            drop(selection);
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty selection update failed: {error}"
                            );
                        }
                    }
                }
            }
            Ok(RuntimeMessage::SelectWord(point)) => {
                let _ = terminal.select_word(point);
                let mut selection = inner
                    .selection_update
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                selection.in_flight = None;
                selection.applied = None;
                drop(selection);
                let _ = refresh_shared_state(&inner, &mut terminal);
            }
            Ok(RuntimeMessage::SelectLine(point)) => {
                let _ = terminal.select_line(point);
                let mut selection = inner
                    .selection_update
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                selection.in_flight = None;
                selection.applied = None;
                drop(selection);
                let _ = refresh_shared_state(&inner, &mut terminal);
            }
            Ok(RuntimeMessage::ClearSelection) => match terminal.clear_selection() {
                Ok(()) => {
                    let mut selection = inner
                        .selection_update
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    selection.in_flight = None;
                    selection.applied = None;
                    drop(selection);
                    update_shared_selection(&inner, None);
                }
                Err(error) => log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection clear failed: {error}"
                ),
            },
            Ok(RuntimeMessage::UpdateAppearance(appearance)) => {
                if let Err(error) = terminal.set_appearance(appearance) {
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                            "Ghostty appearance update failed: {error}"
                        )));
                }
            }
            Ok(RuntimeMessage::SearchChunk {
                start_row,
                max_cells,
                reply,
            }) => {
                let _ = reply.send(
                    terminal
                        .search_chunk(start_row, max_cells)
                        .map_err(|error| error.to_string()),
                );
            }
            Ok(RuntimeMessage::LineTexts { lines, reply }) => {
                let _ = reply.send(
                    terminal
                        .line_texts(&lines)
                        .map_err(|error| error.to_string()),
                );
            }
            Ok(RuntimeMessage::SelectionText(reply)) => {
                let _ = reply.send(terminal.selection_text().map_err(|error| error.to_string()));
            }
            Ok(RuntimeMessage::Hyperlink { point, reply }) => {
                let _ = reply.send(
                    terminal
                        .hyperlink_at(point)
                        .map_err(|error| error.to_string()),
                );
            }
            Ok(RuntimeMessage::ExtractScrollback(reply)) => {
                let _ = reply.send(
                    terminal
                        .extract_scrollback()
                        .map_err(|error| error.to_string()),
                );
            }
            Ok(RuntimeMessage::RestoreScrollback(text)) => {
                let _ = terminal.restore_scrollback(&text);
                let _ = refresh_shared_state(&inner, &mut terminal);
            }
            #[cfg(test)]
            Ok(RuntimeMessage::SimulateWorkerCrash) => {
                panic!("Ghostty runtime worker failure injected for test");
            }
            Ok(RuntimeMessage::Shutdown) => {
                #[cfg(unix)]
                {
                    if exit.is_none() {
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    shutdown_requested = true;
                }
            }
            Err(MailboxRecvError::Disconnected) => {
                #[cfg(unix)]
                {
                    if exit.is_none() {
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                    eof = true;
                }
                #[cfg(target_os = "windows")]
                {
                    lifecycle.record_eof();
                    shutdown_requested = true;
                }
            }
            Err(MailboxRecvError::Timeout) => {}
        }

        if refresh_recent_output_lines(
            &inner,
            &service_output_tail,
            &mut last_recent_output_refresh,
            &mut recent_output_pending,
        ) {
            queue_service_output_ready(&inner);
        }

        notify_command_capacity(&inner);

        #[cfg(unix)]
        {
            if runtime_failed && exit.is_none() {
                inner.shutdown_sent.store(true, Ordering::Release);
                stop_session_input(&inner);
                drop(writer.take());
                terminate_child(child.child_mut(), termination_target);
                child_cleaned = true;
                exit_seen_at = Some(Instant::now());
                exit = Some(portable_pty::ExitStatus::with_exit_code(u32::MAX));
            }

            if exit.is_none() {
                match observe_child_exit(child.child_mut(), child_pid) {
                    Ok(Some(status)) => {
                        exit_seen_at = Some(Instant::now());
                        exit = Some(status);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = inner
                            .events_tx
                            .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                                "Ghostty child wait failed: {error}"
                            )));
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                }
            }
            if let Some(status) = &exit
                && (eof
                    || (exit_seen_at.is_some_and(|seen| seen.elapsed() >= FINAL_DRAIN_TIMEOUT)
                        && mailbox.pending_output_count() == 0))
            {
                if recent_output_pending {
                    publish_recent_output_lines(
                        &inner,
                        &service_output_tail,
                        &mut recent_output_pending,
                    );
                    queue_service_output_ready(&inner);
                }
                let code = i32::try_from(status.exit_code()).unwrap_or(-1);
                let signal = status.signal().map(str::to_owned);
                if !child_cleaned {
                    terminate_child(child.child_mut(), termination_target);
                }
                child.disarm();
                publish_child_exit_once(&inner, code, signal);
                break;
            }
        }

        #[cfg(target_os = "windows")]
        {
            if runtime_failed && lifecycle.is_running() {
                let _ = refresh_shared_state(&inner, &mut terminal);
                shutdown_requested = true;
            }
            if lifecycle.is_running() {
                match observe_windows_child_exit(child.child_mut()) {
                    Ok(Some(exit)) => {
                        child_reaped = true;
                        inner.shutdown_sent.store(true, Ordering::Release);
                        stop_session_input(&inner);
                        let started = Instant::now();
                        let deadline = started
                            .checked_add(
                                super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET,
                            )
                            .unwrap_or(started);
                        let tree =
                            super::pty_session::terminate_windows_process_tree(child_pid, deadline);
                        if tree.failures > 0 || tree.timed_out > 0 || tree.deadline_exhausted {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty Windows descendant cleanup incomplete (targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
                                tree.targeted,
                                tree.terminate_requested,
                                tree.already_exited,
                                tree.failures,
                                tree.timed_out,
                                tree.deadline_exhausted,
                            );
                        }
                        if !close_pty_for_final_drain(&mut writer, &mut master) {
                            let _ = inner
                                .events_tx
                                .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                                    "Ghostty ConPTY close worker disconnected".to_owned(),
                                ));
                        }
                        lifecycle.start_draining(exit, Instant::now());
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !child_wait_failure_reported {
                            let _ = inner
                                .events_tx
                                .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                                    "Ghostty child wait failed: {error}"
                                )));
                            child_wait_failure_reported = true;
                        }
                        shutdown_requested = true;
                    }
                }
            }
            if lifecycle.is_running() && lifecycle.eof {
                shutdown_requested = true;
            }
            if shutdown_requested && lifecycle.is_running() {
                begin_windows_shutdown(
                    &inner,
                    &mut writer,
                    child.child_mut(),
                    child_pid,
                    &mut lifecycle,
                    &mut child_reaped,
                    &mut master,
                );
            }
            if !child_reaped && !lifecycle.is_running() {
                match observe_windows_child_exit(child.child_mut()) {
                    Ok(Some(exit)) => {
                        child_reaped = true;
                        lifecycle.replace_exit(exit);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !child_wait_failure_reported {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty Windows child reap failed (kind={:?}, os_error={:?})",
                                error.kind(),
                                error.raw_os_error(),
                            );
                            child_wait_failure_reported = true;
                        }
                    }
                }
            }
            let now = Instant::now();
            if lifecycle.drain_deadline_reached(now) {
                mailbox.stop_accepting_output();
                lifecycle.seal_output();
                let _ = inner
                    .events_tx
                    .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                        "Ghostty final drain timed out before PTY EOF".to_owned(),
                    ));
            }
            if lifecycle.eof {
                let closer_deadline = Instant::now()
                    .checked_add(Duration::from_millis(100))
                    .unwrap_or_else(Instant::now);
                if !master.join_until(closer_deadline) {
                    continue;
                }
                if let Some(worker) = reader_worker.take()
                    && worker.join().is_err()
                {
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                            "Ghostty PTY reader terminated unexpectedly".to_owned(),
                        ));
                }
            }
            if let Some(exit) = lifecycle.take_ready_exit(now, mailbox.pending_output_count()) {
                if recent_output_pending {
                    publish_recent_output_lines(
                        &inner,
                        &service_output_tail,
                        &mut recent_output_pending,
                    );
                    queue_service_output_ready(&inner);
                }
                if child_reaped {
                    child.disarm();
                }
                // `ChildExited` is the externally observable teardown barrier.
                // Release the ConPTY and child process handles before publishing
                // it so rapid host churn cannot accumulate still-live resources.
                drop(child);
                drop(master);
                publish_child_exit_once(&inner, exit.code, exit.signal);
                return;
            }
        }
    }
}

// These parameters are the mutable runtime-loop state. Grouping them would add
// a second state container without improving ownership or call-site clarity.
#[allow(clippy::too_many_arguments)]
fn process_output_batch(
    inner: &SessionInner,
    mailbox: &RuntimeMailbox,
    terminal: &mut ghostty::DisplayTerminal,
    writer: &mut Option<Box<dyn Write + Send>>,
    marks_scanner: &mut Osc133Scanner,
    service_output_tail: &mut ServiceOutputTail,
    last_recent_output_refresh: &mut Option<Instant>,
    recent_output_pending: &mut bool,
    first: Vec<u8>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut processed_bytes = 0usize;
    let mut chunks = Vec::with_capacity(OUTPUT_BUFFER_COUNT);
    let mut raw_marks = Vec::new();
    let mut next = Some(first);

    let result = (|| {
        while let Some(bytes) = next.take() {
            processed_bytes = processed_bytes.saturating_add(bytes.len());
            chunks.push(bytes);
            let Some(bytes) = chunks.last() else {
                return Err("Ghostty output batch lost its current chunk".into());
            };
            terminal
                .feed(bytes)
                .map_err(|error| format!("Ghostty VT feed failed: {error}"))?;
            service_output_tail.advance(bytes);
            let emitted_mark = scan_chunk_for_marks(marks_scanner, bytes, &mut raw_marks);
            handle_engine_events(inner, terminal, writer)?;
            #[cfg(test)]
            inner
                .processed_output_bytes
                .fetch_add(bytes.len(), Ordering::AcqRel);

            // A command mark is positioned against the snapshot immediately
            // following its PTY chunk. Continuing the batch would attach it to
            // a cursor location produced by later chunks.
            if emitted_mark
                || inner.shutdown_sent.load(Ordering::Acquire)
                || processed_bytes >= OUTPUT_BATCH_MAX_BYTES
                || started.elapsed() >= OUTPUT_BATCH_MAX_TIME
            {
                break;
            }
            next = mailbox.try_recv_consecutive_output();
        }

        *recent_output_pending = true;
        let service_output_ready = refresh_recent_output_lines(
            inner,
            service_output_tail,
            last_recent_output_refresh,
            recent_output_pending,
        );
        update_shared_state(inner, terminal)?;
        record_command_marks(inner, &raw_marks);
        queue_wakeup(inner);
        if service_output_ready {
            queue_service_output_ready(inner);
        }
        Ok(())
    })();

    for bytes in chunks {
        mailbox.recycle_output_buffer(bytes);
    }
    result
}

fn refresh_recent_output_lines(
    inner: &SessionInner,
    service_output_tail: &ServiceOutputTail,
    last_refresh: &mut Option<Instant>,
    pending: &mut bool,
) -> bool {
    if !*pending {
        return false;
    }
    let now = Instant::now();
    if last_refresh.is_some_and(|last| now.duration_since(last) < RECENT_OUTPUT_REFRESH_INTERVAL) {
        return false;
    }
    let notify_trailing_edge = last_refresh.is_some();
    *last_refresh = Some(now);
    publish_recent_output_lines(inner, service_output_tail, pending);
    notify_trailing_edge
}

fn publish_recent_output_lines(
    inner: &SessionInner,
    service_output_tail: &ServiceOutputTail,
    pending: &mut bool,
) {
    *pending = false;
    *inner.recent_output_lines.write() = Arc::from(service_output_tail.recent_lines());
}

fn scan_chunk_for_marks(
    scanner: &mut Osc133Scanner,
    bytes: &[u8],
    raw_marks: &mut Vec<RawMark>,
) -> bool {
    let previous_len = raw_marks.len();
    scanner.feed(bytes, &mut |raw| raw_marks.push(raw));
    raw_marks.len() != previous_len
}

fn record_command_marks(inner: &SessionInner, raw_marks: &[RawMark]) {
    let state = inner.state.read();
    let history_size = state.content.history_size as i64;
    let abs_line = history_size.saturating_add(i64::from(state.content.cursor.point.line.0));
    let screen_lines = state
        .content
        .cells
        .iter()
        .map(|cell| cell.point.line.0)
        .max()
        .map_or(1_i64, |line| i64::from(line.max(0)) + 1);
    drop(state);

    let at = Instant::now();
    let mut marks = inner
        .marks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for raw in raw_marks {
        marks.push(CommandMark {
            kind: raw.kind,
            exit_code: raw.exit_code,
            abs_line,
            at,
        });
    }
    marks.retain_at_or_below(history_size.saturating_add(screen_lines.saturating_sub(1)));
}

fn notify_command_capacity(inner: &SessionInner) {
    if inner.command_backpressure.swap(false, Ordering::AcqRel) {
        queue_wakeup(inner);
    }
}

fn complete_resize(inner: &SessionInner, command: ResizeCommand, succeeded: bool) {
    let size = command.size;
    let mut resize = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    resize.submitted = None;
    if succeeded {
        resize.applied = Some(size);
    } else if command.clear_initial {
        resize.clear_initial_requested = true;
    }
    if resize.requested != size || resize.clear_initial_requested {
        inner.command_backpressure.store(true, Ordering::Release);
    }
    drop(resize);
    notify_command_capacity(inner);
}

fn complete_resize_during_drain(inner: &SessionInner) {
    let mut resize = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    resize.submitted = None;
    resize.clear_initial_requested = false;
    drop(resize);
    notify_command_capacity(inner);
}

fn queue_wakeup(inner: &SessionInner) {
    if !inner.ui_events.wakeup_queued.swap(true, Ordering::AcqRel) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Wakeup(inner.ui_events.clone()));
    }
}

fn queue_service_output_ready(inner: &SessionInner) {
    if !inner
        .ui_events
        .service_output_queued
        .swap(true, Ordering::AcqRel)
    {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::ServiceOutputReady(inner.ui_events.clone()));
    }
}

fn queue_title(inner: &SessionInner, title: String) {
    if UiEventState::store(&inner.ui_events.title, title) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Title(inner.ui_events.clone()));
    }
}

fn queue_working_directory(inner: &SessionInner, cwd: String) {
    if UiEventState::store(&inner.ui_events.working_directory, cwd) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::WorkingDirectory(inner.ui_events.clone()));
    }
}

fn queue_progress(inner: &SessionInner, report: ghostty::ProgressReport) {
    if UiEventState::store(&inner.ui_events.progress, report) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Progress(inner.ui_events.clone()));
    }
}

fn queue_clipboard(inner: &SessionInner, text: String) {
    if !inner.clipboard_gate.allows_store() {
        return;
    }
    let mut slot = inner
        .ui_events
        .clipboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.pending.len() == MAX_CLIPBOARD_EVENTS {
        slot.pending.pop_front();
    }
    slot.pending.push_back(text);
    if !slot.queued {
        slot.queued = true;
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Clipboard(inner.ui_events.clone()));
    }
}

fn read_pty(mut reader: Box<dyn Read + Send>, mailbox: Arc<RuntimeMailbox>) {
    loop {
        let Some(mut buffer) = mailbox.take_output_buffer() else {
            return;
        };
        match reader.read(&mut buffer) {
            Ok(0) => {
                mailbox.recycle_output_buffer(buffer);
                break;
            }
            Ok(read) => {
                buffer.truncate(read);
                if !mailbox.send_output(buffer) {
                    return;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {
                mailbox.recycle_output_buffer(buffer);
                continue;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                mailbox.recycle_output_buffer(buffer);
                std::thread::yield_now();
            }
            Err(_) => {
                mailbox.recycle_output_buffer(buffer);
                break;
            }
        }
    }
    mailbox.send_eof();
}

fn handle_engine_events(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    writer: &mut Option<Box<dyn Write + Send>>,
) -> Result<(), String> {
    for event in terminal.drain_events() {
        match event {
            ghostty::BackendEvent::WritePty(bytes) => {
                if let Some(active_writer) = writer.as_mut() {
                    active_writer
                        .write_all(&bytes)
                        .and_then(|()| active_writer.flush())
                        .map_err(|error| format!("Ghostty protocol reply failed: {error}"))?;
                }
            }
            ghostty::BackendEvent::ClipboardStore(text) => queue_clipboard(inner, text),
            ghostty::BackendEvent::Title(title) => queue_title(inner, title),
            ghostty::BackendEvent::WorkingDirectory(cwd) => {
                queue_working_directory(inner, cwd);
            }
            ghostty::BackendEvent::Progress(report) => queue_progress(inner, report),
            ghostty::BackendEvent::Bell => {}
            ghostty::BackendEvent::CallbackPanicked => {
                return Err("Ghostty callback panicked at the FFI boundary".into());
            }
            ghostty::BackendEvent::InputDropped { bytes } => {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty dropped oversized callback input ({bytes} bytes)"
                );
            }
            ghostty::BackendEvent::EffectsOverflow {
                dropped_events,
                dropped_bytes,
            } => {
                return Err(format!(
                    "Ghostty callback effects overflowed ({dropped_events} events, {dropped_bytes} bytes)"
                ));
            }
        }
    }
    Ok(())
}

fn refresh_shared_state(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
) -> Result<(), String> {
    update_shared_state(inner, terminal)?;
    queue_wakeup(inner);
    Ok(())
}

fn update_shared_state(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
) -> Result<(), String> {
    let content = terminal.snapshot().map_err(|error| error.to_string())?;
    let modes = terminal.modes().map_err(|error| error.to_string())?;
    let metrics = grid_metrics_from_ghostty(&content);
    let content = content_from_ghostty(content);
    let modes = modes_from_ghostty(modes);
    *inner.state.write() = SharedState {
        content,
        modes,
        metrics,
    };
    Ok(())
}

fn update_shared_selection(inner: &SessionInner, selection: Option<SelectionRange>) {
    let mut state = inner.state.write();
    if state.content.selection == selection {
        return;
    }
    state.content.selection = selection;
    drop(state);
    queue_wakeup(inner);
}

fn ghostty_rgb(color: gpui::Hsla) -> ghostty::Rgb {
    let color = super::pty_session::hsla_to_alac_rgb(color);
    ghostty::Rgb {
        r: color.r,
        g: color.g,
        b: color.b,
    }
}

fn current_ghostty_appearance() -> ghostty::TerminalAppearance {
    let theme = crate::theme::active_theme();
    ghostty::TerminalAppearance::new(
        ghostty_rgb(theme.foreground),
        ghostty_rgb(theme.ansi_background),
        ghostty_rgb(theme.cursor),
        if theme.ansi_background.l > 0.5 {
            ghostty::ColorScheme::Light
        } else {
            ghostty::ColorScheme::Dark
        },
    )
}

#[cfg(unix)]
type ChildTerminationTarget = Option<i32>;

#[cfg(target_os = "windows")]
type ChildTerminationTarget = u32;

#[cfg(unix)]
fn child_termination_target(child_pid: u32) -> ChildTerminationTarget {
    verified_process_group(child_pid)
}

#[cfg(target_os = "windows")]
fn child_termination_target(child_pid: u32) -> ChildTerminationTarget {
    child_pid
}

#[cfg(unix)]
fn verified_process_group(child_pid: u32) -> Option<i32> {
    let pid = i32::try_from(child_pid).ok().filter(|pid| *pid > 0)?;
    // SAFETY: getpgid only observes the freshly-spawned child. portable-pty
    // creates it as its own session leader, so equality authenticates the
    // process group before any wait can reap the leader or permit PID reuse.
    (unsafe { libc::getpgid(pid) } == pid).then_some(pid)
}

#[cfg(unix)]
fn observe_child_exit(
    _child: &mut dyn portable_pty::Child,
    child_pid: u32,
) -> std::io::Result<Option<portable_pty::ExitStatus>> {
    let pid = i32::try_from(child_pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "child PID unavailable"))?;
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: waitid initializes siginfo_t on success. WNOWAIT observes the
    // exit without reaping, keeping the leader PID reserved until remaining
    // group members are terminated and portable-pty performs the final wait.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: the successful waitid call initialized info, and WEXITED makes
    // si_pid/si_status valid for the child-state variants handled below.
    let info = unsafe { info.assume_init() };
    let observed_pid = unsafe { info.si_pid() };
    if observed_pid == 0 {
        return Ok(None);
    }
    let status = unsafe { info.si_status() };
    let exit = match info.si_code {
        libc::CLD_EXITED => portable_pty::ExitStatus::with_exit_code(status.max(0) as u32),
        libc::CLD_KILLED | libc::CLD_DUMPED => {
            let signal = unsafe { libc::strsignal(status) };
            let signal = if signal.is_null() {
                format!("Signal {status}")
            } else {
                unsafe { std::ffi::CStr::from_ptr(signal) }
                    .to_string_lossy()
                    .into_owned()
            };
            portable_pty::ExitStatus::with_signal(&signal)
        }
        code => {
            return Err(std::io::Error::other(format!(
                "unexpected waitid child state {code}"
            )));
        }
    };
    Ok(Some(exit))
}

#[cfg(target_os = "windows")]
fn child_exit_report(status: &portable_pty::ExitStatus) -> ChildExitReport {
    ChildExitReport {
        code: i32::try_from(status.exit_code()).unwrap_or(-1),
        signal: status.signal().map(str::to_owned),
    }
}

#[cfg(target_os = "windows")]
fn observe_windows_child_exit(
    child: &mut dyn portable_pty::Child,
) -> std::io::Result<Option<ChildExitReport>> {
    let Some(observed) = child.try_wait()? else {
        return Ok(None);
    };
    let exit = match child.wait() {
        Ok(waited) => child_exit_report(&waited),
        Err(error) => {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty Windows child wait after observed exit failed (kind={:?}, os_error={:?})",
                error.kind(),
                error.raw_os_error(),
            );
            child_exit_report(&observed)
        }
    };
    Ok(Some(exit))
}

#[cfg(unix)]
fn terminate_child(child: &mut dyn portable_pty::Child, process_group_id: ChildTerminationTarget) {
    if let Some(pid) = process_group_id {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            let group_exists = unsafe { libc::kill(-pid, 0) == 0 }
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            if !group_exists {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "windows")]
struct WindowsChildTerminationOutcome {
    exit: ChildExitReport,
    reaped: bool,
}

#[cfg(target_os = "windows")]
fn terminate_windows_child_until(
    child: &mut dyn portable_pty::Child,
    child_pid: u32,
    deadline: Instant,
) -> WindowsChildTerminationOutcome {
    let tree = super::pty_session::terminate_windows_process_tree(child_pid, deadline);
    match observe_windows_child_exit(child) {
        Ok(Some(exit)) => return WindowsChildTerminationOutcome { exit, reaped: true },
        Ok(None) => {}
        Err(error) => log::warn!(
            target: "paneflow::terminal::ghostty",
            "Ghostty Windows child pre-kill observation failed (kind={:?}, os_error={:?})",
            error.kind(),
            error.raw_os_error(),
        ),
    }

    if let Err(error) = child.kill() {
        log::warn!(
            target: "paneflow::terminal::ghostty",
            "Ghostty Windows portable child kill failed (kind={:?}, os_error={:?})",
            error.kind(),
            error.raw_os_error(),
        );
    }
    loop {
        match observe_windows_child_exit(child) {
            Ok(Some(exit)) => return WindowsChildTerminationOutcome { exit, reaped: true },
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty Windows child post-kill observation failed (kind={:?}, os_error={:?})",
                    error.kind(),
                    error.raw_os_error(),
                );
                break;
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(WINDOWS_CHILD_POLL_INTERVAL));
    }
    log::warn!(
        target: "paneflow::terminal::ghostty",
        "Ghostty Windows child cleanup reached its deadline (targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
        tree.targeted,
        tree.terminate_requested,
        tree.already_exited,
        tree.failures,
        tree.timed_out,
        tree.deadline_exhausted,
    );
    WindowsChildTerminationOutcome {
        exit: ChildExitReport {
            code: -1,
            signal: None,
        },
        reaped: false,
    }
}

#[cfg(target_os = "windows")]
fn begin_windows_shutdown(
    inner: &SessionInner,
    writer: &mut Option<Box<dyn Write + Send>>,
    child: &mut dyn portable_pty::Child,
    child_pid: u32,
    lifecycle: &mut RuntimeLifecycle,
    child_reaped: &mut bool,
    master: &mut DrainablePtyMaster<Box<dyn portable_pty::MasterPty + Send>>,
) {
    if !lifecycle.is_running() {
        return;
    }
    inner.shutdown_sent.store(true, Ordering::Release);
    stop_session_input(inner);
    let started = Instant::now();
    let deadline = started
        .checked_add(super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET)
        .unwrap_or(started);
    let outcome = terminate_windows_child_until(child, child_pid, deadline);
    if !close_pty_for_final_drain(writer, master) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                "Ghostty ConPTY close worker disconnected".to_owned(),
            ));
    }
    *child_reaped = outcome.reaped;
    lifecycle.start_draining(outcome.exit, Instant::now());
}

#[cfg(target_os = "windows")]
fn terminate_child(child: &mut dyn portable_pty::Child, child_pid: ChildTerminationTarget) {
    let started = Instant::now();
    let deadline = started
        .checked_add(super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET)
        .unwrap_or(started);
    let _ = terminate_windows_child_until(child, child_pid, deadline);
}

fn pty_size(size: TerminalWindowSize) -> PtySize {
    PtySize {
        rows: size.rows.clamp(1, u16::MAX as usize) as u16,
        cols: size.cols.clamp(1, u16::MAX as usize) as u16,
        pixel_width: size
            .cols
            .saturating_mul(usize::from(size.cell_width))
            .min(u16::MAX as usize) as u16,
        pixel_height: size
            .rows
            .saturating_mul(usize::from(size.cell_height))
            .min(u16::MAX as usize) as u16,
    }
}

fn normalized_window_size(size: TerminalWindowSize) -> TerminalWindowSize {
    TerminalWindowSize::new(
        size.cols.clamp(1, u16::MAX as usize),
        size.rows.clamp(1, u16::MAX as usize),
        size.cell_width,
        size.cell_height,
    )
}

fn window_size(size: TerminalWindowSize) -> ghostty::Result<ghostty::WindowSize> {
    ghostty::WindowSize::new(
        size.cols,
        size.rows,
        u32::from(size.cell_width),
        u32::from(size.cell_height),
    )
}

fn ghostty_point(point: Point) -> ghostty::Point {
    ghostty::Point::new(point.line.0, point.column.0)
}

fn point_from_ghostty(point: ghostty::Point) -> Point {
    Point::new(point.line, point.column)
}

fn selection_range_from_ghostty(selection: ghostty::SelectionRange) -> SelectionRange {
    SelectionRange {
        start: point_from_ghostty(selection.start),
        end: point_from_ghostty(selection.end),
        is_block: selection.rectangle,
    }
}

fn filter_copyable_selection_text(
    kind: Option<SelectionKind>,
    range: Option<SelectionRange>,
    text: Option<String>,
) -> Option<String> {
    // libghostty formats a point-only simple selection as the cell under the
    // cursor. Alacritty treats that same gesture as an empty focus click.
    let is_focus_click = matches!(kind, Some(SelectionKind::Simple))
        && range.is_some_and(|range| range.start == range.end);
    (!is_focus_click).then_some(text).flatten()
}

pub(super) fn modes_from_ghostty(modes: ghostty::Modes) -> Modes {
    let mut result = Modes::empty();
    if modes.alternate_screen {
        result = result | Modes::ALT_SCREEN;
    }
    if modes.application_cursor {
        result = result | Modes::APP_CURSOR;
    }
    if modes.application_keypad {
        result = result | Modes::APP_KEYPAD;
    }
    if modes.bracketed_paste {
        result = result | Modes::BRACKETED_PASTE;
    }
    if modes.focus_reporting {
        result = result | Modes::FOCUS_IN_OUT;
    }
    if modes.alternate_scroll {
        result = result | Modes::ALTERNATE_SCROLL;
    }
    if modes.sgr_mouse {
        result = result | Modes::SGR_MOUSE;
    }
    if modes.utf8_mouse {
        result = result | Modes::UTF8_MOUSE;
    }
    if modes.mouse_report_click {
        result = result | Modes::MOUSE_REPORT_CLICK;
    }
    if modes.mouse_drag {
        result = result | Modes::MOUSE_DRAG;
    }
    if modes.mouse_motion {
        result = result | Modes::MOUSE_MOTION;
    }
    if modes.kitty_keyboard {
        result = result | Modes::KITTY_KEYBOARD;
    }
    result
}

pub(super) fn content_from_ghostty(content: ghostty::Content) -> Content {
    let cursor_viewport_line = content.cursor.point.line + content.display_offset as i32;
    let cursor_cell = content.cells.iter().find(|cell| {
        cell.point.line == cursor_viewport_line && cell.point.column == content.cursor.point.column
    });
    let cursor_flags = cursor_cell.map_or(CellFlags::empty(), ghostty_cell_flags);
    let cursor = RenderableCursor {
        point: point_from_ghostty(content.cursor.point),
        shape: if content.cursor.visible {
            match content.cursor.shape {
                ghostty::CursorShape::Bar => CursorShape::Beam,
                ghostty::CursorShape::Block => CursorShape::Block,
                ghostty::CursorShape::Underline => CursorShape::Underline,
                ghostty::CursorShape::HollowBlock => CursorShape::HollowBlock,
            }
        } else {
            CursorShape::Hidden
        },
        fg: cursor_cell.map_or(Color::Named(NamedColor::Foreground), |cell| {
            color_from_ghostty(cell.foreground, NamedColor::Foreground)
        }),
        bg: cursor_cell.map_or(Color::Named(NamedColor::Background), |cell| {
            color_from_ghostty(cell.background, NamedColor::Background)
        }),
        flags: cursor_flags,
        wide: cursor_cell.is_some_and(|cell| matches!(cell.wide, ghostty::WideCell::Wide)),
        text: cursor_cell.map_or(' ', |cell| cell.character),
        bold: cursor_flags.contains(CellFlags::BOLD),
        italic: cursor_flags.contains(CellFlags::ITALIC),
    };
    let cells: Arc<[Cell]> = content
        .cells
        .iter()
        .map(|cell| Cell {
            point: point_from_ghostty(cell.point),
            c: cell.character,
            fg: color_from_ghostty(cell.foreground, NamedColor::Foreground),
            bg: color_from_ghostty(cell.background, NamedColor::Background),
            flags: ghostty_cell_flags(cell),
            zerowidth: cell.zerowidth.as_deref().map(<[_]>::to_vec),
            hyperlink: cell.hyperlink,
        })
        .collect::<Vec<_>>()
        .into();
    Content {
        cols: content.cols,
        rows: content.rows,
        cells,
        cursor,
        selection: content.selection.map(selection_range_from_ghostty),
        display_offset: content.display_offset,
        history_size: content.history_size,
    }
}

fn ghostty_cell_flags(cell: &ghostty::Cell) -> CellFlags {
    let mut flags = CellFlags::empty();
    if cell.flags.inverse {
        flags |= CellFlags::INVERSE;
    }
    if cell.flags.bold {
        flags |= CellFlags::BOLD;
    }
    if cell.flags.italic {
        flags |= CellFlags::ITALIC;
    }
    if cell.flags.dim {
        flags |= CellFlags::DIM;
    }
    if cell.flags.strikethrough {
        flags |= CellFlags::STRIKEOUT;
    }
    match cell.flags.underline {
        ghostty::UnderlineStyle::None => {}
        ghostty::UnderlineStyle::Single => flags |= CellFlags::UNDERLINE,
        ghostty::UnderlineStyle::Double => flags |= CellFlags::DOUBLE_UNDERLINE,
        ghostty::UnderlineStyle::Curly => flags |= CellFlags::UNDERCURL,
        ghostty::UnderlineStyle::Dotted => flags |= CellFlags::DOTTED_UNDERLINE,
        ghostty::UnderlineStyle::Dashed => flags |= CellFlags::DASHED_UNDERLINE,
    }
    match cell.wide {
        ghostty::WideCell::Wide | ghostty::WideCell::SpacerHead => {
            flags |= CellFlags::WIDE_CHAR;
        }
        ghostty::WideCell::SpacerTail => flags |= CellFlags::WIDE_CHAR_SPACER,
        ghostty::WideCell::Narrow => {}
    }
    flags
}

fn color_from_ghostty(color: ghostty::Color, default: NamedColor) -> Color {
    match color {
        ghostty::Color::Default => Color::Named(default),
        ghostty::Color::Palette(index) => match index {
            0 => Color::Named(NamedColor::Black),
            1 => Color::Named(NamedColor::Red),
            2 => Color::Named(NamedColor::Green),
            3 => Color::Named(NamedColor::Yellow),
            4 => Color::Named(NamedColor::Blue),
            5 => Color::Named(NamedColor::Magenta),
            6 => Color::Named(NamedColor::Cyan),
            7 => Color::Named(NamedColor::White),
            8 => Color::Named(NamedColor::BrightBlack),
            9 => Color::Named(NamedColor::BrightRed),
            10 => Color::Named(NamedColor::BrightGreen),
            11 => Color::Named(NamedColor::BrightYellow),
            12 => Color::Named(NamedColor::BrightBlue),
            13 => Color::Named(NamedColor::BrightMagenta),
            14 => Color::Named(NamedColor::BrightCyan),
            15 => Color::Named(NamedColor::BrightWhite),
            _ => Color::Indexed(index),
        },
        ghostty::Color::Rgb(rgb) => Color::Spec(Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
    }
}

fn blank_content(cols: usize, rows: usize) -> Content {
    let cells: Arc<[Cell]> = (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |column| Cell {
                point: Point::new(row as i32, column),
                c: ' ',
                fg: Color::Spec(Rgb {
                    r: 0xd0,
                    g: 0xd0,
                    b: 0xd0,
                }),
                bg: Color::Spec(Rgb::default()),
                flags: CellFlags::empty(),
                zerowidth: None,
                hyperlink: false,
            })
        })
        .collect::<Vec<_>>()
        .into();
    Content {
        cols,
        rows,
        cells,
        cursor: RenderableCursor {
            point: Point::new(0, 0),
            shape: CursorShape::Block,
            fg: Color::Spec(Rgb::default()),
            bg: Color::Spec(Rgb::default()),
            flags: CellFlags::empty(),
            wide: false,
            text: ' ',
            bold: false,
            italic: false,
        },
        selection: None,
        display_offset: 0,
        history_size: 0,
    }
}

fn initial_grid_metrics(cols: usize, rows: usize) -> GridMetrics {
    GridMetrics {
        columns: cols,
        screen_lines: rows,
        display_offset: 0,
        topmost_line: Line(0),
        bottommost_line: Line(i32::try_from(rows.saturating_sub(1)).unwrap_or(i32::MAX)),
        cursor: Point::new(0, 0),
    }
}

fn grid_metrics_from_ghostty(content: &ghostty::Content) -> GridMetrics {
    GridMetrics {
        columns: content.cols,
        screen_lines: content.rows,
        display_offset: content.display_offset,
        topmost_line: Line(-i32::try_from(content.history_size).unwrap_or(i32::MAX)),
        bottommost_line: Line(i32::try_from(content.rows.saturating_sub(1)).unwrap_or(i32::MAX)),
        cursor: point_from_ghostty(content.cursor.point),
    }
}

#[cfg(test)]
mod tests {
    use super::super::pty_session::{BackendInputResult, TerminalState};
    use super::*;
    use paneflow_config::schema::TerminalSurfaceProfile;

    #[test]
    fn nfr_005_terminal_queue_caps_stay_below_budget() {
        assert_eq!(OUTPUT_POOL_BYTES, 128 * 1024);
        assert_eq!(MAX_QUEUED_INPUT_BYTES, 1024 * 1024);
    }

    #[test]
    fn clipboard_store_is_filtered_at_the_ghostty_source() {
        let gate = Arc::new(ClipboardGate::default());
        let (session, _pending, mut events_rx) = GhosttySession::pending_with_clipboard_gate(
            TerminalWindowSize::new(80, 24, 8, 16),
            gate.clone(),
        );

        queue_clipboard(&session.inner, "unfocused".into());
        assert!(events_rx.try_recv().is_err());

        gate.set_policy(true, false);
        gate.set_focused(true);
        queue_clipboard(&session.inner, "focused".into());
        let event_state = match events_rx.try_recv() {
            Ok(GhosttyUiEvent::Clipboard(state)) => state,
            other => panic!("expected a focused clipboard event, got {other:?}"),
        };
        assert_eq!(event_state.take_clipboard(), ["focused"]);

        gate.set_focused(false);
        queue_clipboard(&session.inner, "lost-focus".into());
        assert!(events_rx.try_recv().is_err());
    }

    #[test]
    fn slow_output_consumer_cannot_grow_the_fixed_buffer_pool() {
        let mailbox = Arc::new(RuntimeMailbox::new());
        for index in 0..OUTPUT_BUFFER_COUNT {
            let mut buffer = mailbox
                .take_output_buffer()
                .expect("fixed output buffer must be available");
            buffer[0] = index as u8;
            buffer.truncate(1);
            assert!(mailbox.send_output(buffer));
        }
        assert_eq!(mailbox.pending_output_count(), OUTPUT_BUFFER_COUNT);

        let waiting_mailbox = mailbox.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let waiting_barrier = barrier.clone();
        let (available_tx, available_rx) = sync_channel(1);
        let waiter = std::thread::spawn(move || {
            waiting_barrier.wait();
            let length = waiting_mailbox
                .take_output_buffer()
                .map(|buffer| buffer.len());
            let _ = available_tx.send(length);
        });
        barrier.wait();
        assert!(matches!(
            available_rx.recv_timeout(Duration::from_millis(20)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        let RuntimeMessage::Output(buffer) = mailbox
            .recv_timeout(Duration::ZERO)
            .expect("slow consumer must release one queued buffer")
        else {
            panic!("expected queued output");
        };
        mailbox.recycle_output_buffer(buffer);
        assert_eq!(
            available_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("blocked reader must receive the recycled buffer"),
            Some(OUTPUT_CHUNK_BYTES)
        );
        mailbox.close();
        waiter.join().expect("buffer waiter must exit");
    }

    #[test]
    fn sealing_output_preserves_admitted_buffers_and_rejects_late_producers() {
        let mailbox = RuntimeMailbox::new();
        assert!(mailbox.send_output(vec![1, 2, 3]));

        mailbox.stop_accepting_output();

        assert!(mailbox.take_output_buffer().is_none());
        assert!(!mailbox.send_output(vec![4, 5, 6]));
        assert_eq!(mailbox.pending_output_count(), 1);
        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Output(bytes)) if bytes == [1, 2, 3]
        ));
        assert_eq!(mailbox.pending_output_count(), 0);
    }

    #[test]
    fn mailbox_bounds_output_without_blocking_control_admission() {
        let mailbox = RuntimeMailbox::new();
        for index in 0..OUTPUT_BUFFER_COUNT {
            assert!(mailbox.send_output(vec![index as u8]));
        }
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"input".to_vec()))
                .is_ok()
        );

        let queued = mailbox.drain();
        assert_eq!(queued.len(), OUTPUT_BUFFER_COUNT + 1);
        assert!(
            queued[..OUTPUT_BUFFER_COUNT]
                .iter()
                .all(|message| matches!(message, RuntimeMessage::Output(_)))
        );
        assert!(matches!(
            queued.last(),
            Some(RuntimeMessage::Input(bytes)) if bytes == b"input"
        ));
    }

    #[test]
    fn output_batching_stops_at_the_next_control_message() {
        let mailbox = RuntimeMailbox::new();
        assert!(mailbox.send_output(vec![1]));
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(vec![2]))
                .is_ok()
        );
        assert!(mailbox.send_output(vec![3]));

        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Output(bytes)) if bytes == vec![1]
        ));
        assert!(mailbox.try_recv_consecutive_output().is_none());
        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Input(bytes)) if bytes == vec![2]
        ));
        assert!(matches!(
            mailbox.try_recv_consecutive_output(),
            Some(bytes) if bytes == vec![3]
        ));
    }

    #[test]
    fn absolute_scroll_rows_coalesce_at_queue_tail() {
        let mailbox = RuntimeMailbox::new();
        for row in [10, 20, 30] {
            assert!(
                mailbox
                    .try_send_control(RuntimeMessage::ScrollToViewportRow(row))
                    .is_ok()
            );
        }

        let queued = mailbox.drain();
        assert!(matches!(
            queued.as_slice(),
            [RuntimeMessage::ScrollToViewportRow(30)]
        ));
    }

    #[test]
    fn absolute_scroll_coalescing_preserves_fifo_barriers() {
        let mailbox = RuntimeMailbox::new();
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(10))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(20))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"barrier".to_vec()))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(30))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(40))
                .is_ok()
        );

        let queued = mailbox.drain();
        assert_eq!(queued.len(), 3);
        assert!(matches!(queued[0], RuntimeMessage::ScrollToViewportRow(20)));
        assert!(matches!(
            &queued[1],
            RuntimeMessage::Input(bytes) if bytes == b"barrier"
        ));
        assert!(matches!(queued[2], RuntimeMessage::ScrollToViewportRow(40)));
    }

    #[test]
    fn absolute_scroll_target_replaces_tail_at_control_capacity() {
        let mailbox = RuntimeMailbox::new();
        for _ in 0..CONTROL_CAPACITY - 1 {
            assert!(
                mailbox
                    .try_send_control(RuntimeMessage::ClearSelection)
                    .is_ok()
            );
        }
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(10))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(20))
                .is_ok()
        );
        assert!(matches!(
            mailbox.try_send_control(RuntimeMessage::ClearSelection),
            Err(TrySendError::Full(RuntimeMessage::ClearSelection))
        ));

        let queued = mailbox.drain();
        assert_eq!(queued.len(), CONTROL_CAPACITY);
        assert!(matches!(
            queued.last(),
            Some(RuntimeMessage::ScrollToViewportRow(20))
        ));
    }

    #[test]
    fn queued_row_jump_does_not_reject_a_relative_drag_step() {
        let (mut state, _alacritty_pending) = TerminalState::new_pending(80, 24);
        let (ghostty, runtime_pending, events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        state.attach_ghostty(ghostty, events_rx);
        state.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });

        let backend = state.session_backend();
        assert!(backend.scroll_to_viewport_row(0));
        assert!(backend.scroll_delta(-1));

        let queued = runtime_pending.mailbox.drain();
        assert!(matches!(
            queued.as_slice(),
            [
                RuntimeMessage::ScrollToViewportRow(0),
                RuntimeMessage::Scroll(ghostty::Scroll::Delta(-1))
            ]
        ));
    }

    #[test]
    fn output_batching_barrier_trips_only_when_a_chunk_completes_a_mark() {
        let mut scanner = Osc133Scanner::default();
        let mut marks = Vec::new();

        assert!(!scan_chunk_for_marks(
            &mut scanner,
            b"before\x1b]133;D;7",
            &mut marks
        ));
        assert!(scan_chunk_for_marks(&mut scanner, b"\x07after", &mut marks));
        assert!(!scan_chunk_for_marks(
            &mut scanner,
            b"plain output",
            &mut marks
        ));
        assert_eq!(
            marks,
            vec![RawMark {
                kind: super::super::marks::MarkKind::CommandFinished,
                exit_code: Some(7),
            }]
        );
    }

    #[test]
    fn pty_size_reports_cells_and_total_pixels() {
        assert_eq!(
            pty_size(TerminalWindowSize::new(80, 24, 8, 16)),
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 640,
                pixel_height: 384,
            }
        );
    }

    #[test]
    fn content_conversion_preserves_snapshot_grid_dimensions() {
        let content = content_from_ghostty(ghostty::Content {
            cells: Vec::<ghostty::Cell>::new().into(),
            cursor: ghostty::Cursor {
                point: ghostty::Point::new(0, 0),
                shape: ghostty::CursorShape::Block,
                visible: true,
                blinking: false,
                wide_tail: false,
            },
            selection: None,
            cols: 80,
            rows: 24,
            display_offset: 0,
            history_size: 0,
        });

        assert_eq!((content.cols, content.rows), (80, 24));
    }

    #[test]
    fn service_tail_refresh_requests_a_trailing_scan() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut tail = ServiceOutputTail::default();
        tail.advance(b"first\n");
        let mut last_refresh = None;
        let mut pending = true;

        assert!(!refresh_recent_output_lines(
            &session.inner,
            &tail,
            &mut last_refresh,
            &mut pending,
        ));
        assert_eq!(session.recent_output_lines().as_ref(), ["first"]);

        tail.advance(b"http://127.0.0.1:3000\n");
        last_refresh = Some(Instant::now() - RECENT_OUTPUT_REFRESH_INTERVAL);
        pending = true;
        assert!(refresh_recent_output_lines(
            &session.inner,
            &tail,
            &mut last_refresh,
            &mut pending,
        ));
        assert_eq!(
            session.recent_output_lines().first().map(String::as_str),
            Some("http://127.0.0.1:3000")
        );
    }

    #[test]
    fn resize_storm_is_coalesced_and_zero_dimensions_are_clamped() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        for index in 0..200 {
            session.resize(TerminalWindowSize::new(index, index, 8, 16));
        }

        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 1);
        let first = match &queued[0] {
            RuntimeMessage::Resize(command) => *command,
            _ => panic!("expected coalesced resize"),
        };
        assert_eq!(first.size, TerminalWindowSize::new(1, 1, 8, 16));
        assert!(!first.clear_initial);
        let resize = session
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(resize.requested.cols, 199);
        assert_eq!(resize.requested.rows, 199);
        assert_eq!(resize.requested.cell_width, 8);
        assert_eq!(resize.requested.cell_height, 16);
        drop(resize);

        complete_resize(&session.inner, first, true);
        session.retry_backpressured_commands();
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)]
                if command.size == TerminalWindowSize::new(199, 199, 8, 16)
                    && !command.clear_initial
        ));
    }

    #[test]
    fn resize_during_drain_is_completed_without_apply_or_requeue() {
        let initial = TerminalWindowSize::new(80, 24, 8, 16);
        let requested = TerminalWindowSize::new(100, 30, 9, 18);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);
        session.resize(requested);
        let command = match pending.mailbox.try_recv().unwrap() {
            RuntimeMessage::Resize(command) => command,
            _ => panic!("expected queued resize"),
        };

        session.inner.shutdown_sent.store(true, Ordering::Release);
        complete_resize_during_drain(&session.inner);
        session.resize(TerminalWindowSize::new(120, 40, 10, 20));
        session.retry_backpressured_commands();

        assert!(pending.mailbox.drain().is_empty());
        let resize = session
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(resize.submitted, None);
        assert_eq!(resize.applied, Some(initial));
        assert_eq!(resize.requested, command.size);
    }

    #[test]
    fn applied_resize_is_not_resubmitted_on_backend_wakeup() {
        let initial = TerminalWindowSize::new(80, 24, 8, 16);
        let resized = TerminalWindowSize::new(100, 30, 8, 16);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);

        session.retry_backpressured_commands();
        assert!(pending.mailbox.drain().is_empty());

        session.resize(resized);
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)] if command.size == resized && !command.clear_initial
        ));
        complete_resize(
            &session.inner,
            ResizeCommand {
                size: resized,
                clear_initial: false,
            },
            true,
        );

        session.retry_backpressured_commands();
        assert!(pending.mailbox.drain().is_empty());
    }

    #[test]
    fn provisional_matching_layout_does_not_consume_initial_clear() {
        let initial = TerminalWindowSize::new(120, 40, 0, 0);
        let desired = TerminalWindowSize::new(91, 33, 10, 21);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);

        let (_, provisional_clear_consumed) = session.render_content(initial, 0, 40, true);

        assert!(!provisional_clear_consumed);
        assert!(pending.mailbox.drain().is_empty());

        let (_, actual_clear_consumed) = session.render_content(desired, 0, 33, true);

        assert!(actual_clear_consumed);
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)]
                if command.size == desired && command.clear_initial
        ));
    }

    #[test]
    fn selection_drag_updates_are_coalesced_without_text_requests() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        session.start_selection(SelectionKind::Simple, Point::new(2, 3));
        for column in 4..80 {
            assert_eq!(
                session.update_selection(Point::new(2, column), SelectionSide::Right),
                None
            );
        }

        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 1);
        assert!(matches!(queued[0], RuntimeMessage::ApplySelection(_)));
        let selection = session
            .inner
            .selection_update
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(selection.queued_generation, Some(selection.generation));
        assert_eq!(
            selection.requested,
            Some(ghostty::SelectionRange {
                start: ghostty::Point::new(2, 3),
                end: ghostty::Point::new(2, 79),
                rectangle: false,
            })
        );
        drop(selection);

        session.clear_selection();
        session.start_selection(SelectionKind::Simple, Point::new(2, 3));
        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 2);
        assert!(matches!(queued[0], RuntimeMessage::ClearSelection));
        assert!(matches!(queued[1], RuntimeMessage::ApplySelection(_)));
    }

    #[test]
    fn point_only_simple_selection_is_not_copyable() {
        let point = Point::new(2, 3);
        let point_range = SelectionRange {
            start: point,
            end: point,
            is_block: false,
        };
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Simple),
                Some(point_range),
                Some("x".into()),
            ),
            None
        );

        let drag_range = SelectionRange {
            end: Point::new(2, 4),
            ..point_range
        };
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Simple),
                Some(drag_range),
                Some("xy".into()),
            ),
            Some("xy".into())
        );
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Semantic),
                Some(point_range),
                Some("x".into()),
            ),
            Some("x".into())
        );
    }

    #[test]
    fn promotion_replays_pending_input_once_in_order_and_enforces_cap() {
        let (mut state, _alacritty_pending) = TerminalState::new_pending(80, 24);
        let (ghostty, runtime_pending, events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 0, 0));
        state.attach_ghostty(ghostty, events_rx);

        state.write_to_pty(b"first".to_vec());
        state.write_to_pty(b"second".to_vec());
        state.write_to_pty(vec![b'x'; MAX_QUEUED_INPUT_BYTES]);
        assert!(matches!(
            runtime_pending.mailbox.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));

        state.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });
        let first = runtime_pending
            .mailbox
            .recv_timeout(Duration::from_millis(50))
            .unwrap();
        let second = runtime_pending
            .mailbox
            .recv_timeout(Duration::from_millis(50))
            .unwrap();
        assert!(matches!(first, RuntimeMessage::Input(bytes) if bytes == b"first"));
        assert!(matches!(second, RuntimeMessage::Input(bytes) if bytes == b"second"));
        assert!(matches!(
            runtime_pending.mailbox.try_recv(),
            Err(std::sync::mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn command_backpressure_retries_structured_key_without_raw_fallback() {
        let (mut state, _alacritty_pending) = TerminalState::new_pending(80, 24);
        let (session, runtime_pending, events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 0, 0));
        state.attach_ghostty(session.clone(), events_rx);
        state.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });
        for _ in 0..CONTROL_CAPACITY {
            assert!(session.write(vec![b'x']).is_sent());
        }

        let key = ghostty::KeyInput {
            key: ghostty::Key::Function(5),
            action: ghostty::KeyAction::Press,
            modifiers: ghostty::Modifiers::CONTROL,
            consumed_modifiers: ghostty::Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        };
        assert_eq!(
            state.write_ghostty_key(key.clone(), None),
            BackendInputResult::Accepted
        );
        let saturated = runtime_pending.mailbox.drain();
        assert_eq!(saturated.len(), CONTROL_CAPACITY);
        assert!(
            saturated
                .iter()
                .all(|message| matches!(message, RuntimeMessage::Input(bytes) if bytes == b"x"))
        );

        state.process_backend_wakeup();
        assert!(matches!(
            runtime_pending.mailbox.try_recv(),
            Ok(RuntimeMessage::KeyInput(retried)) if retried == key
        ));
    }

    #[cfg(target_os = "windows")]
    fn windows_executable(name: &str) -> Option<String> {
        let output = std::process::Command::new("where.exe")
            .arg(name)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
            .map(str::to_owned)
    }

    #[cfg(target_os = "windows")]
    fn wsl_has_distribution() -> bool {
        let Some(wsl) = windows_executable("wsl.exe") else {
            return false;
        };
        std::process::Command::new(wsl)
            .args(["--list", "--quiet"])
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && output
                        .stdout
                        .chunks_exact(2)
                        .any(|pair| u16::from_le_bytes([pair[0], pair[1]]) > 0x20)
            })
    }

    #[cfg(target_os = "windows")]
    fn run_windows_shell_case(
        name: &str,
        shell: String,
        shell_quoting: super::super::types::ShellQuoting,
        extra_args: Vec<String>,
        cwd: &std::path::Path,
    ) -> String {
        let params = SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env: std::collections::HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
                ("PANEFLOW_MATRIX".into(), "matrix-é中".into()),
                (
                    "WSLENV".into(),
                    "PANEFLOW_MATRIX/u:TERM/u:COLORTERM/u:TERM_PROGRAM/u".into(),
                ),
            ]),
            cwd: cwd.to_path_buf(),
            cols: 100,
            rows: 30,
            profile: TerminalSurfaceProfile::Normal,
            surface_id: 1,
        };
        let (session, pending, mut events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(100, 30, 8, 16));
        let spawned = session
            .start(pending, params, None, 1_000)
            .unwrap_or_else(|error| panic!("{name} must spawn through ConPTY: {error}"));
        assert!(spawned.child_pid > 0, "{name} child PID");
        session.promote();
        session.resize(TerminalWindowSize::new(120, 36, 8, 16));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut exit = None;
        let mut failures = Vec::new();
        while Instant::now() < deadline {
            while let Ok(event) = events_rx.try_recv() {
                match event {
                    GhosttyUiEvent::ChildExited { code, .. } => exit = Some(code),
                    GhosttyUiEvent::RuntimeFailed(error) => failures.push(error),
                    _ => {}
                }
            }
            if exit.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(exit, Some(0), "{name} exit; failures={failures:?}");
        assert!(failures.is_empty(), "{name} runtime failures: {failures:?}");
        let (content, _) =
            session.render_content(TerminalWindowSize::new(120, 36, 8, 16), -100, 100, false);
        content.cells.iter().map(|cell| cell.c).collect()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_matrix_preserves_unicode_environment_and_cwd() {
        use super::super::types::ShellQuoting;

        let cwd = tempfile::Builder::new()
            .prefix("paneflow shell é ")
            .tempdir()
            .expect("create Unicode shell-matrix cwd");
        let mut cases = Vec::new();
        cases.push((
            "cmd",
            windows_executable("cmd.exe").expect("cmd.exe is required on Windows"),
            ShellQuoting::Cmd,
            vec![
                "/D".into(),
                "/Q".into(),
                "/C".into(),
                "chcp 65001>nul & echo PANEFLOW_SHELL:cmd:%PANEFLOW_MATRIX% & echo %CD% & echo \x1b[38;2;1;2;3mCOLOR\x1b[0m"
                    .into(),
            ],
        ));
        cases.push((
            "powershell",
            windows_executable("powershell.exe")
                .expect("Windows PowerShell 5.1 is required on Windows"),
            ShellQuoting::PowerShell,
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Write-Output \"PANEFLOW_SHELL:powershell:$env:PANEFLOW_MATRIX\"; Write-Output (Get-Location).Path; Write-Output \"$([char]27)[38;2;1;2;3mCOLOR$([char]27)[0m\""
                    .into(),
            ],
        ));
        cases.push((
            "pwsh",
            windows_executable("pwsh.exe").expect("PowerShell 7 is required on Windows CI"),
            ShellQuoting::PowerShell,
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "Write-Output \"PANEFLOW_SHELL:pwsh:$env:PANEFLOW_MATRIX\"; Write-Output (Get-Location).Path; Write-Output \"$([char]27)[38;2;1;2;3mCOLOR$([char]27)[0m\""
                    .into(),
            ],
        ));
        let git_bash = std::path::PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        assert!(git_bash.is_file(), "Git Bash is required on Windows CI");
        cases.push((
            "git-bash",
            git_bash.to_string_lossy().into_owned(),
            ShellQuoting::Posix,
            vec![
                "--noprofile".into(),
                "--norc".into(),
                "-lc".into(),
                "printf 'PANEFLOW_SHELL:git-bash:%s\\n%s\\n\\033[38;2;1;2;3mCOLOR\\033[0m\\n' \"$PANEFLOW_MATRIX\" \"$PWD\""
                    .into(),
            ],
        ));
        if wsl_has_distribution() {
            cases.push((
                "wsl",
                windows_executable("wsl.exe").expect("wsl.exe was detected"),
                ShellQuoting::Posix,
                vec![
                    "--cd".into(),
                    cwd.path().to_string_lossy().into_owned(),
                    "--exec".into(),
                    "sh".into(),
                    "-lc".into(),
                    "printf 'PANEFLOW_SHELL:wsl:%s\\n%s\\n\\033[38;2;1;2;3mCOLOR\\033[0m\\n' \"$PANEFLOW_MATRIX\" \"$PWD\""
                        .into(),
                ],
            ));
        }

        for (name, shell, quoting, args) in cases {
            let rendered = run_windows_shell_case(name, shell, quoting, args, cwd.path());
            assert!(
                rendered.contains(&format!("PANEFLOW_SHELL:{name}:matrix-é中")),
                "{name} lost Unicode or env propagation: {rendered:?}"
            );
            assert!(
                rendered.contains("COLOR"),
                "{name} lost truecolor output: {rendered:?}"
            );
            assert!(
                rendered.contains("paneflow shell é"),
                "{name} lost cwd with spaces/non-ASCII: {rendered:?}"
            );
        }
    }

    #[test]
    fn live_runtime_runs_platform_shell_and_reports_one_exit() {
        let cwd = std::env::current_dir().unwrap();
        #[cfg(unix)]
        let (shell, shell_quoting, extra_args) = (
            "/bin/sh".into(),
            super::super::types::ShellQuoting::Posix,
            Vec::new(),
        );
        #[cfg(target_os = "windows")]
        let (shell, shell_quoting, extra_args) = (
            "cmd.exe".into(),
            super::super::types::ShellQuoting::Cmd,
            vec!["/D".into(), "/Q".into()],
        );
        let params = SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env: std::collections::HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
            ]),
            cwd,
            cols: 80,
            rows: 24,
            profile: TerminalSurfaceProfile::Normal,
            surface_id: 1,
        };
        let (session, pending, mut events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let spawned = session
            .start(pending, params, None, 1_000)
            .expect("Ghostty runtime must spawn a portable PTY shell");
        assert!(spawned.child_pid > 0);
        #[cfg(unix)]
        let child_pid = spawned.child_pid;
        session.promote();
        #[cfg(target_os = "windows")]
        {
            assert!(
                session
                    .write(b"echo PANEFLOW_CONPTY^_READY\r\n".to_vec())
                    .is_sent()
            );
            let ready_deadline = Instant::now() + Duration::from_secs(5);
            let mut child_ready = false;
            while Instant::now() < ready_deadline {
                if session
                    .recent_output_lines()
                    .iter()
                    .any(|line| line.contains("PANEFLOW_CONPTY_READY"))
                {
                    child_ready = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                child_ready,
                "ConPTY child must process input before the resize probe"
            );
        }
        session.resize(TerminalWindowSize::new(100, 30, 8, 16));
        #[cfg(unix)]
        let command =
            b"printf 'PANEFLOW_GHOSTTY_RUNTIME_OK:%s\\n' \"$TERM_PROGRAM\"; stty size; exit\n"
                .to_vec();
        #[cfg(target_os = "windows")]
        let command = {
            let mut command = br#"powershell.exe -NoLogo -NoProfile -NonInteractive -Command "$deadline = [DateTime]::UtcNow.AddSeconds(3); do { $height = [Console]::WindowHeight; $width = [Console]::WindowWidth; if ($height -eq 30 -and $width -eq 100) { break }; Start-Sleep -Milliseconds 20 } while ([DateTime]::UtcNow -lt $deadline); Write-Output ('PANEFLOW_GHOSTTY_RUNTIME_OK:' + $env:TERM_PROGRAM); Write-Output ('PANEFLOW_SIZE:' + $height + 'x' + $width)" & exit"#.to_vec();
            command.extend_from_slice(b"\r\n");
            command
        };
        assert!(session.write(command).is_sent());

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut exits = 0;
        let mut runtime_failures = Vec::new();
        while Instant::now() < deadline {
            while let Ok(event) = events_rx.try_recv() {
                match event {
                    GhosttyUiEvent::ChildExited { .. } => exits += 1,
                    GhosttyUiEvent::RuntimeFailed(error) => runtime_failures.push(error),
                    _ => {}
                }
            }
            if exits > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(50));
        while let Ok(event) = events_rx.try_recv() {
            match event {
                GhosttyUiEvent::ChildExited { .. } => exits += 1,
                GhosttyUiEvent::RuntimeFailed(error) => runtime_failures.push(error),
                _ => {}
            }
        }

        let (content, _) =
            session.render_content(TerminalWindowSize::new(100, 30, 8, 16), -100, 100, false);
        let rendered: String = content.cells.iter().map(|cell| cell.c).collect();
        assert!(
            rendered.contains("PANEFLOW_GHOSTTY_RUNTIME_OK:ghostty"),
            "Ghostty runtime must identify itself to terminal applications; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        #[cfg(unix)]
        assert!(
            rendered.contains("30 100"),
            "resize must reach the child PTY; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        #[cfg(target_os = "windows")]
        assert!(
            rendered.contains("PANEFLOW_SIZE:30x100"),
            "resize must reach ConPTY; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        assert_eq!(exits, 1, "child exit must be published exactly once");
        #[cfg(unix)]
        {
            assert_eq!(unsafe { libc::kill(child_pid as i32, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    #[test]
    fn stopping_input_discards_queued_bytes_and_rejects_new_input() {
        let mailbox = RuntimeMailbox::new();
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"first".to_vec()))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ClearSelection)
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"second".to_vec()))
                .is_ok()
        );

        assert_eq!(mailbox.stop_accepting_input(), 11);
        assert!(matches!(
            mailbox.try_send_control(RuntimeMessage::Input(b"late".to_vec())),
            Err(TrySendError::Disconnected(RuntimeMessage::Input(bytes))) if bytes == b"late"
        ));
        assert!(matches!(
            mailbox.drain().as_slice(),
            [RuntimeMessage::ClearSelection]
        ));
    }

    #[test]
    fn simulated_worker_crash_is_admitted_once_and_rejected_after_shutdown() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        assert!(session.simulate_worker_crash_for_test());
        assert!(!session.simulate_worker_crash_for_test());
        assert!(matches!(
            pending.mailbox.try_recv(),
            Ok(RuntimeMessage::SimulateWorkerCrash)
        ));

        let (shutdown_session, shutdown_pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        shutdown_session.shutdown();
        assert!(!shutdown_session.simulate_worker_crash_for_test());
        assert!(matches!(
            shutdown_pending.mailbox.try_recv(),
            Ok(RuntimeMessage::Shutdown)
        ));
    }

    #[test]
    fn lifecycle_publishes_once_after_eof() {
        let now = Instant::now();
        let exit = ChildExitReport {
            code: 7,
            signal: None,
        };
        let mut lifecycle = RuntimeLifecycle::new();

        assert!(lifecycle.start_draining(exit.clone(), now));
        assert!(!lifecycle.start_draining(
            ChildExitReport {
                code: 99,
                signal: None,
            },
            now,
        ));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
        lifecycle.record_eof();
        assert_eq!(lifecycle.take_ready_exit(now, 1), None);
        assert_eq!(lifecycle.take_ready_exit(now, 0), Some(exit));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
    }

    #[test]
    fn lifecycle_deadline_and_early_eof_converge() {
        let now = Instant::now();
        let deadline = now.checked_add(FINAL_DRAIN_TIMEOUT).unwrap_or(now);
        let mut timed = RuntimeLifecycle::new();
        assert!(timed.start_draining(
            ChildExitReport {
                code: -1,
                signal: None,
            },
            now,
        ));
        assert_eq!(timed.take_ready_exit(now, 0), None);
        assert!(timed.drain_deadline_reached(deadline));
        assert_eq!(timed.take_ready_exit(deadline, 0), None);
        timed.seal_output();
        assert_eq!(timed.take_ready_exit(deadline, 1), None);
        assert_eq!(
            timed.take_ready_exit(deadline, 0),
            Some(ChildExitReport {
                code: -1,
                signal: None,
            })
        );

        let mut eof_first = RuntimeLifecycle::new();
        eof_first.record_eof();
        assert!(eof_first.start_draining(
            ChildExitReport {
                code: 0,
                signal: None,
            },
            now,
        ));
        assert_eq!(
            eof_first.take_ready_exit(now, 0),
            Some(ChildExitReport {
                code: 0,
                signal: None,
            })
        );
    }

    #[test]
    fn final_drain_closes_writer_and_master_before_reader_eof() {
        struct DropProbe {
            dropped: Arc<AtomicBool>,
        }

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let writer_dropped = Arc::new(AtomicBool::new(false));
        let master_dropped = Arc::new(AtomicBool::new(false));
        let mut writer = Some(DropProbe {
            dropped: writer_dropped.clone(),
        });
        let closer = PtyCloser::new("paneflow-ghostty-test-pty-closer")
            .expect("test closer thread must start");
        let mut master = DrainablePtyMaster::new(
            DropProbe {
                dropped: master_dropped.clone(),
            },
            closer,
        );
        assert!(close_pty_for_final_drain(&mut writer, &mut master));
        assert!(writer_dropped.load(Ordering::Acquire));
        assert!(master.join_until(Instant::now() + Duration::from_secs(1)));
        assert!(master_dropped.load(Ordering::Acquire));

        let now = Instant::now();
        let mut lifecycle = RuntimeLifecycle::new();
        assert!(lifecycle.start_draining(
            ChildExitReport {
                code: 0,
                signal: None,
            },
            now,
        ));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
        lifecycle.record_eof();
        assert!(lifecycle.take_ready_exit(now, 0).is_some());
    }

    /// Spawn `/bin/sh -c <script>` on its own PTY, the same shape
    /// `run_runtime` uses, and hand back the pieces the POSIX lifecycle
    /// helpers operate on. Every assertion below therefore exercises the
    /// production helper against a real Darwin/Linux process, not a model.
    #[cfg(unix)]
    fn spawn_posix_lifecycle_probe(
        script: &str,
    ) -> (
        Box<dyn portable_pty::MasterPty + Send>,
        Box<dyn portable_pty::Child + Send + Sync>,
        u32,
    ) {
        let pair = native_pty_system()
            .openpty(pty_size(TerminalWindowSize::new(80, 24, 8, 16)))
            .expect("POSIX lifecycle probe must open a PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.cwd(std::env::current_dir().expect("probe cwd must resolve"));
        let child = pair
            .slave
            .spawn_command(command)
            .expect("POSIX lifecycle probe must spawn /bin/sh");
        drop(pair.slave);
        let pid = child
            .process_id()
            .expect("POSIX lifecycle probe must report a child PID");
        (pair.master, child, pid)
    }

    #[cfg(unix)]
    fn probe_pid(pid: u32) -> i32 {
        i32::try_from(pid).expect("a probe PID must fit in pid_t")
    }

    /// Wait for `observe_child_exit` to report the probe's exit without ever
    /// reaping it, so the caller can re-observe the same status afterwards.
    #[cfg(unix)]
    fn observe_probe_exit(
        child: &mut (dyn portable_pty::Child + Send + Sync),
        pid: u32,
    ) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match observe_child_exit(child, pid) {
                Ok(Some(status)) => return status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("waitid failed for probe {pid}: {error}"),
            }
        }
        panic!("probe {pid} never reported an exit status");
    }

    #[cfg(unix)]
    #[test]
    fn posix_process_group_helper_authenticates_the_session_leader() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("exec sleep 30");
        let expected = probe_pid(pid);
        assert_eq!(
            verified_process_group(pid),
            Some(expected),
            "portable-pty must spawn the child as its own process-group leader"
        );
        assert_eq!(child_termination_target(pid), Some(expected));
        terminate_child(child.as_mut(), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn waitid_probe_is_non_blocking_and_leaves_the_exit_status_unconsumed() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("sleep 0.3; exit 7");
        let started = Instant::now();
        let pending =
            observe_child_exit(child.as_mut(), pid).expect("waitid must succeed for a live child");
        assert!(
            pending.is_none(),
            "a still-running child must not report an exit status"
        );
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "WNOHANG must return immediately instead of waiting for the child"
        );

        let exit = observe_probe_exit(child.as_mut(), pid);
        assert_eq!(exit.exit_code(), 7, "CLD_EXITED must carry the exit code");
        assert!(exit.signal().is_none());

        // WNOWAIT left the zombie in place, so the leader PID is still
        // reserved and a second observation sees the same status.
        let again = observe_probe_exit(child.as_mut(), pid);
        assert_eq!(
            again.exit_code(),
            7,
            "WNOWAIT must not consume the exit status"
        );
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn waitid_probe_maps_a_killed_child_to_a_named_signal() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("kill -KILL $$; sleep 30");
        let exit = observe_probe_exit(child.as_mut(), pid);
        let signal = exit
            .signal()
            .expect("CLD_KILLED must be reported as a signal, not an exit code");
        assert!(!signal.is_empty());
        assert!(
            !signal.starts_with("Signal "),
            "strsignal must name the signal; {signal:?} is the null-pointer fallback"
        );
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_escalates_to_sigkill_for_a_child_that_ignores_sigterm() {
        let (_master, mut child, pid) =
            spawn_posix_lifecycle_probe("trap '' TERM; while :; do sleep 0.05; done");
        let group = verified_process_group(pid).expect("probe must lead its own process group");
        let started = Instant::now();
        terminate_child(child.as_mut(), Some(group));
        assert!(
            started.elapsed() <= SHUTDOWN_GRACE + Duration::from_secs(1),
            "SIGKILL must land within SHUTDOWN_GRACE plus one second"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut group_error = None;
        while Instant::now() < deadline {
            // SAFETY: signal 0 performs no delivery; it only probes whether
            // any member of the process group still exists.
            if unsafe { libc::kill(-group, 0) } == -1 {
                group_error = std::io::Error::last_os_error().raw_os_error();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            group_error,
            Some(libc::ESRCH),
            "the whole process group must be gone after the SIGKILL escalation"
        );
    }
}
