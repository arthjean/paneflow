//! Linux-only Ghostty runtime adapter.
//!
//! The libghostty engine is owned by one worker thread. PTY bytes, protocol
//! replies, input, resize, search, selection, persistence, and shutdown all
//! pass through its bounded command queue, so no C handle or borrowed render
//! data crosses a thread or frame boundary.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use paneflow_terminal_ghostty as ghostty;
use parking_lot::RwLock;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use super::marks::{CommandMark, Osc133Scanner, RawMark, SharedMarkRing};
use super::pty_session::{ForegroundSignalMask, SpawnParams};
use super::service_detector::ServiceOutputTail;
use super::types::{
    Cell, CellFlags, Color, Content, CursorShape, GridLineText, GridMetrics, HyperlinkSource,
    HyperlinkZone, Line, Modes, NamedColor, Point, RenderableCursor, Rgb, SelectionKind,
    SelectionRange, SelectionSide, TerminalWindowSize,
};

const CONTROL_CAPACITY: usize = 256;
const OUTPUT_BUFFER_COUNT: usize = 4;
const OUTPUT_CHUNK_BYTES: usize = 32 * 1024;
const OUTPUT_BATCH_MAX_BYTES: usize = 128 * 1024;
const OUTPUT_BATCH_MAX_TIME: Duration = Duration::from_millis(1);
const MAX_QUEUED_INPUT_BYTES: usize = 64 * 1024;
const RECENT_OUTPUT_REFRESH_INTERVAL: Duration = Duration::from_millis(300);
const FINAL_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
const MAX_CLIPBOARD_EVENTS: usize = 8;

#[derive(Debug)]
pub(crate) enum GhosttyUiEvent {
    Wakeup(Arc<UiEventState>),
    Title(Arc<UiEventState>),
    WorkingDirectory(Arc<UiEventState>),
    Clipboard(Arc<UiEventState>),
    ServiceOutputReady(Arc<UiEventState>),
    ChildExited { code: i32, signal: Option<String> },
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

#[derive(Debug, Default)]
struct CoalescedSlot {
    latest: Option<String>,
    queued: bool,
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
    title: Mutex<CoalescedSlot>,
    working_directory: Mutex<CoalescedSlot>,
    clipboard: Mutex<ClipboardSlot>,
}

impl UiEventState {
    fn store(slot: &Mutex<CoalescedSlot>, value: String) -> bool {
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

    fn take(slot: &Mutex<CoalescedSlot>) -> Option<String> {
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
    Pty(anyhow::Error),
}

impl std::fmt::Display for GhosttyStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialization(error) => {
                write!(formatter, "Ghostty initialization failed: {error:#}")
            }
            Self::Pty(error) => write!(formatter, "Ghostty PTY failed: {error:#}"),
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
    state: RwLock<SharedState>,
    recent_output_lines: RwLock<Arc<[String]>>,
    queued_input_bytes: AtomicUsize,
    command_backpressure: AtomicBool,
    promoted: AtomicBool,
    shutdown_sent: AtomicBool,
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
    Resize(ResizeCommand),
    Scroll(ghostty::Scroll),
    ScrollToViewportRow(usize),
    ApplySelection(u64),
    SelectWord(ghostty::Point),
    SelectLine(ghostty::Point),
    ClearSelection,
    Search {
        query: String,
        regex: bool,
        reply: SyncSender<Result<ghostty::SearchResult, String>>,
    },
    SelectionText(SyncSender<Result<Option<String>, String>>),
    Hyperlink {
        point: ghostty::Point,
        reply: SyncSender<Result<Option<ghostty::Hyperlink>, String>>,
    },
    ExtractScrollback(SyncSender<Result<Option<String>, String>>),
    RestoreScrollback(String),
    Shutdown,
}

#[derive(Default)]
struct MailboxState {
    queue: VecDeque<RuntimeMessage>,
    control_count: usize,
    output_count: usize,
    available_output_buffers: Vec<Vec<u8>>,
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
            if let Some(mut buffer) = state.available_output_buffers.pop() {
                buffer.resize(OUTPUT_CHUNK_BYTES, 0);
                return Some(buffer);
            }
            if state.closed {
                return None;
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
        if state.closed || state.output_count >= OUTPUT_BUFFER_COUNT {
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

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
    InitializationFailed(String),
    PtyFailed(String),
}

impl GhosttySession {
    pub(super) fn pending(
        size: TerminalWindowSize,
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
                state: RwLock::new(SharedState {
                    content: blank_content(size.cols.max(1), size.rows.max(1)),
                    modes: Modes::empty(),
                    metrics: initial_grid_metrics(size.cols.max(1), size.rows.max(1)),
                }),
                recent_output_lines: RwLock::new(Arc::from(Vec::<String>::new())),
                queued_input_bytes: AtomicUsize::new(0),
                command_backpressure: AtomicBool::new(false),
                promoted: AtomicBool::new(false),
                shutdown_sent: AtomicBool::new(false),
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
        let inner = self.inner.clone();
        let runtime_mailbox = pending.mailbox.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-ghostty-runtime".into())
            .spawn(move || {
                run_runtime(
                    inner,
                    runtime_mailbox,
                    params,
                    signal_mask,
                    max_scrollback,
                    startup_tx,
                );
            })
        {
            pending.mailbox.close();
            return Err(GhosttyStartError::Initialization(anyhow::anyhow!(
                "could not start Ghostty runtime thread: {error}"
            )));
        }

        match startup_rx.recv() {
            Ok(StartupReport::Started(spawned)) => Ok(spawned),
            Ok(StartupReport::InitializationFailed(error)) => {
                Err(GhosttyStartError::Initialization(anyhow::anyhow!(error)))
            }
            Ok(StartupReport::PtyFailed(error)) => {
                Err(GhosttyStartError::Pty(anyhow::anyhow!(error)))
            }
            Err(error) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(
                "Ghostty runtime exited before startup completed: {error}"
            ))),
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

    pub(super) fn write(&self, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() {
            return true;
        }
        let len = bytes.len();
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
            return false;
        }
        match self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::Input(bytes))
        {
            Ok(()) => true,
            Err(TrySendError::Full(RuntimeMessage::Input(bytes))) => {
                self.inner
                    .queued_input_bytes
                    .fetch_sub(bytes.len(), Ordering::AcqRel);
                self.inner
                    .command_backpressure
                    .store(true, Ordering::Release);
                false
            }
            Err(TrySendError::Disconnected(RuntimeMessage::Input(bytes))) => {
                self.inner
                    .queued_input_bytes
                    .fetch_sub(bytes.len(), Ordering::AcqRel);
                false
            }
            Err(_) => unreachable!("try_send returns the submitted message"),
        }
    }

    pub(super) fn queued_input_bytes(&self) -> usize {
        self.inner.queued_input_bytes.load(Ordering::Acquire)
    }

    pub(super) fn resize(&self, size: TerminalWindowSize) {
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
        let result = self.request(|reply| RuntimeMessage::Search {
            query: query.to_owned(),
            regex,
            reply,
        });
        match result.and_then(Result::ok) {
            Some(result) => crate::search::SearchResult {
                matches: result
                    .matches
                    .into_iter()
                    .map(|found| crate::search::SearchMatch {
                        start: point_from_ghostty(found.start),
                        end: point_from_ghostty(found.end),
                    })
                    .collect(),
                regex_error: result.regex_error,
            },
            None => crate::search::SearchResult {
                matches: Vec::new(),
                regex_error: None,
            },
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

    pub(super) fn shutdown(&self) {
        if !self.inner.shutdown_sent.swap(true, Ordering::AcqRel) {
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

fn run_runtime(
    inner: Arc<SessionInner>,
    mailbox: Arc<RuntimeMailbox>,
    params: SpawnParams,
    signal_mask: Option<ForegroundSignalMask>,
    max_scrollback: usize,
    startup_tx: SyncSender<StartupReport>,
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
            let _ = startup_tx.send(StartupReport::InitializationFailed(error.to_string()));
            return;
        }
    };
    let mut terminal = match ghostty::DisplayTerminal::new(ghostty_size, max_scrollback) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::InitializationFailed(error.to_string()));
            return;
        }
    };
    let theme = crate::theme::active_theme();
    let foreground = ghostty_rgb(theme.foreground);
    let background = ghostty_rgb(theme.ansi_background);
    let cursor = ghostty_rgb(theme.cursor);
    if let Err(error) = terminal.set_default_colors(foreground, background, cursor) {
        let _ = startup_tx.send(StartupReport::InitializationFailed(error.to_string()));
        return;
    }
    if let Err(error) = refresh_shared_state(&inner, &mut terminal) {
        let _ = startup_tx.send(StartupReport::InitializationFailed(error));
        return;
    }

    let pair = match native_pty_system().openpty(pty_size(initial_size)) {
        Ok(pair) => pair,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::PtyFailed(format!(
                "failed to open native PTY: {error:#}"
            )));
            return;
        }
    };
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

    let restore_mask = super::pty_session::apply_thread_signal_mask(signal_mask);
    let child = pair.slave.spawn_command(command);
    super::pty_session::restore_thread_signal_mask(restore_mask);
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::PtyFailed(format!(
                "failed to spawn shell in PTY: {error:#}"
            )));
            return;
        }
    };
    let child_pid = child.process_id().unwrap_or(0);
    let process_group_id = verified_process_group(child_pid);
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            terminate_child(&mut *child, process_group_id);
            let _ = startup_tx.send(StartupReport::PtyFailed(format!(
                "failed to clone PTY reader: {error:#}"
            )));
            return;
        }
    };
    let mut writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            terminate_child(&mut *child, process_group_id);
            let _ = startup_tx.send(StartupReport::PtyFailed(format!(
                "failed to take PTY writer: {error:#}"
            )));
            return;
        }
    };
    let output_mailbox = mailbox.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("paneflow-ghostty-pty-reader".into())
        .spawn(move || read_pty(reader, output_mailbox))
    {
        terminate_child(&mut *child, process_group_id);
        let _ = startup_tx.send(StartupReport::PtyFailed(format!(
            "failed to start PTY reader: {error}"
        )));
        return;
    }

    drop(pair.slave);
    if startup_tx
        .send(StartupReport::Started(SpawnedGhostty {
            child_pid,
            cwd: params.cwd,
        }))
        .is_err()
    {
        terminate_child(&mut *child, process_group_id);
        return;
    }

    let mut marks_scanner = Osc133Scanner::default();
    let mut service_output_tail = ServiceOutputTail::default();
    let mut last_recent_output_refresh = None;
    let mut recent_output_pending = false;
    let mut eof = false;
    let mut exit = None;
    let mut exit_seen_at = None;
    let mut runtime_failed = false;

    loop {
        if inner.shutdown_sent.load(Ordering::Acquire) {
            terminate_child(&mut *child, process_group_id);
            break;
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
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
                    runtime_failed = true;
                }
            }
            Ok(RuntimeMessage::Eof) => eof = true,
            Ok(RuntimeMessage::Input(bytes)) => {
                inner
                    .queued_input_bytes
                    .fetch_sub(bytes.len(), Ordering::AcqRel);
                if let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush())
                    && !matches!(
                        error.kind(),
                        ErrorKind::BrokenPipe | ErrorKind::NotConnected
                    )
                {
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                            "Ghostty PTY write failed: {error}"
                        )));
                    runtime_failed = true;
                }
                notify_command_capacity(&inner);
            }
            Ok(RuntimeMessage::Resize(command)) => {
                let size = command.size;
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
                        pair.master
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
            Ok(RuntimeMessage::Search {
                query,
                regex,
                reply,
            }) => {
                let _ = reply.send(
                    terminal
                        .search(&query, regex)
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
            Ok(RuntimeMessage::Shutdown) | Err(MailboxRecvError::Disconnected) => {
                terminate_child(&mut *child, process_group_id);
                break;
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

        if runtime_failed {
            terminate_child(&mut *child, process_group_id);
            break;
        }

        if exit.is_none() {
            match observe_child_exit(child_pid) {
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
                    terminate_child(&mut *child, process_group_id);
                    break;
                }
            }
        }
        if let Some(status) = &exit
            && (eof || exit_seen_at.is_some_and(|seen| seen.elapsed() >= FINAL_DRAIN_TIMEOUT))
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
            terminate_child(&mut *child, process_group_id);
            let _ = inner
                .events_tx
                .unbounded_send(GhosttyUiEvent::ChildExited { code, signal });
            break;
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
    writer: &mut Box<dyn Write + Send>,
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

fn queue_clipboard(inner: &SessionInner, text: String) {
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
    writer: &mut Box<dyn Write + Send>,
) -> Result<(), String> {
    for event in terminal.drain_events() {
        match event {
            ghostty::BackendEvent::WritePty(bytes) => writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .map_err(|error| format!("Ghostty protocol reply failed: {error}"))?,
            ghostty::BackendEvent::ClipboardStore(text) => queue_clipboard(inner, text),
            ghostty::BackendEvent::Title(title) => queue_title(inner, title),
            ghostty::BackendEvent::WorkingDirectory(cwd) => {
                queue_working_directory(inner, cwd);
            }
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

fn verified_process_group(child_pid: u32) -> Option<i32> {
    let pid = i32::try_from(child_pid).ok().filter(|pid| *pid > 0)?;
    // SAFETY: getpgid only observes the freshly-spawned child. portable-pty
    // creates it as its own session leader, so equality authenticates the
    // process group before any wait can reap the leader or permit PID reuse.
    (unsafe { libc::getpgid(pid) } == pid).then_some(pid)
}

fn observe_child_exit(child_pid: u32) -> std::io::Result<Option<portable_pty::ExitStatus>> {
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

fn terminate_child(child: &mut dyn portable_pty::Child, process_group_id: Option<i32>) {
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
    use super::super::pty_session::TerminalState;
    use super::*;
    use paneflow_config::schema::TerminalSurfaceProfile;

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
        assert!(matches!(
            queued[0],
            RuntimeMessage::Resize(ResizeCommand {
                size: TerminalWindowSize {
                    cols: 1,
                    rows: 1,
                    ..
                },
                clear_initial: false,
            })
        ));
        let resize = session
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(resize.requested.cols, 199);
        assert_eq!(resize.requested.rows, 199);
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
    fn live_runtime_drains_final_output_and_reaps_child_once() {
        let cwd = std::env::current_dir().unwrap();
        let params = SpawnParams {
            shell: "/bin/sh".into(),
            shell_quoting: super::super::types::ShellQuoting::Posix,
            extra_args: Vec::new(),
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
        let child_pid = spawned.child_pid;
        session.promote();
        session.resize(TerminalWindowSize::new(100, 30, 8, 16));
        assert!(
            session.write(
                b"printf 'PANEFLOW_GHOSTTY_RUNTIME_OK:%s\\n' \"$TERM_PROGRAM\"; stty size; exit\n"
                    .to_vec()
            )
        );

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
        assert!(
            rendered.contains("30 100"),
            "resize must reach the child PTY; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        assert_eq!(exits, 1, "child exit must be published exactly once");
        assert_eq!(unsafe { libc::kill(child_pid as i32, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
