use std::collections::VecDeque;
use std::io::Write;
#[cfg(test)]
use std::io::{ErrorKind, Read};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use paneflow_terminal_ghostty as ghostty;

use crate::theme::ghostty_rgb;
use parking_lot::RwLock;
#[cfg(test)]
use portable_pty::{CommandBuilder, PtySize};

use paneflow_host::protocol::ERR_OUTPUT_EVICTED;
use paneflow_host::{HostClient, HostClientError};

use super::clipboard_gate::ClipboardGate;
use super::host_link::{CheckpointPayload, HostAttachment, HostLinkEnd, HostLinkState};
use super::marks::{CommandMark, Osc133Scanner, RawMark, SharedMarkRing};
#[cfg(test)]
use super::pty_session::SpawnParams;
use super::service_detector::ServiceOutputTail;
use super::types::{
    Cell, CellFlags, Color, Content, CursorShape, GridLineText, GridMetrics, HyperlinkSource,
    HyperlinkZone, Line, Modes, NamedColor, Point, RenderableCursor, Rgb, SelectionGeometry,
    SelectionKind, SelectionRange, TerminalWindowSize,
};

mod attached_runtime;
mod commands;
mod convert;
mod display_runtime;
mod events;
mod mailbox;
mod pty_runtime;
mod publish;

use attached_runtime::*;
use commands::*;
use convert::*;
use display_runtime::*;
use events::*;
use mailbox::*;
use pty_runtime::*;
use publish::*;

use super::element;
#[cfg(test)]
use super::{marks, pty_session, types};
pub(super) use convert::{CellMirror, blank_content};
pub(super) use events::{GhosttyUiEvent, ProgramNotification};
#[cfg(test)]
pub(super) use publish::simulate_gate_trickle;

#[cfg(not(any(unix, windows)))]
compile_error!("terminal::ghostty_session requires a Unix or Windows target");

const CONTROL_CAPACITY: usize = 256;
const PASTE_TEXT_MIME: &str = "text/plain;charset=utf-8";
const OUTPUT_BUFFER_COUNT: usize = 4;
const OUTPUT_CHUNK_BYTES: usize = 32 * 1024;
const OUTPUT_POOL_BYTES: usize = OUTPUT_BUFFER_COUNT * OUTPUT_CHUNK_BYTES;
const OUTPUT_BATCH_MAX_BYTES: usize = 128 * 1024;
const OUTPUT_BATCH_MAX_TIME: Duration = Duration::from_millis(1);
const MAX_QUEUED_INPUT_BYTES: usize = NFR_005_MAX_QUEUED_INPUT_BYTES;
const NFR_005_MAX_PENDING_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const NFR_005_MAX_QUEUED_INPUT_BYTES: usize = 1024 * 1024;
const RECENT_OUTPUT_REFRESH_INTERVAL: Duration = Duration::from_millis(300);
const SEARCH_RAIL_REFRESH_INTERVAL: Duration = Duration::from_millis(100);
const MIN_PUBLISH_INTERVAL: Duration = Duration::from_millis(8);
const INTERACTIVE_OUTPUT_WINDOW: Duration = Duration::from_millis(100);
const SELECT_ALL_TIMEOUT: Duration = Duration::from_secs(10);
const SYNC_OUTPUT_MAX_HOLD: Duration = Duration::from_millis(150);
const RUNTIME_IDLE_TICK: Duration = Duration::from_millis(10);
const RUNTIME_QUIET_TICK: Duration = Duration::from_millis(100);
const RUNTIME_QUIET_AFTER: Duration = Duration::from_secs(1);
const DISPLAY_RUNTIME_TICK: Duration = Duration::from_secs(1);

#[cfg(test)]
pub(super) static RUNTIME_LOOP_ITERATIONS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
pub(super) static RUNTIME_LOOP_QUIET_WAITS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(super) static RUNTIME_LOOP_MESSAGES: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(super) static RUNTIME_LOOP_ATTENTIVE_REASONS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
fn count_runtime_loop_iteration() {
    RUNTIME_LOOP_ITERATIONS.fetch_add(1, Ordering::Relaxed);
}

#[cfg(test)]
pub(super) static RUNTIME_LOOP_IDLE_WAITS: AtomicU64 = AtomicU64::new(0);
#[cfg(test)]
pub(super) static RUNTIME_LOOP_GATE_WAITS: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
fn count_runtime_loop_wait(wait: Duration, received_message: bool) {
    if wait == RUNTIME_QUIET_TICK {
        RUNTIME_LOOP_QUIET_WAITS.fetch_add(1, Ordering::Relaxed);
    } else if wait == RUNTIME_IDLE_TICK {
        RUNTIME_LOOP_IDLE_WAITS.fetch_add(1, Ordering::Relaxed);
    } else {
        RUNTIME_LOOP_GATE_WAITS.fetch_add(1, Ordering::Relaxed);
    }
    if received_message {
        RUNTIME_LOOP_MESSAGES.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(not(test))]
#[inline(always)]
fn count_runtime_loop_wait(_wait: Duration, _received_message: bool) {}

#[cfg(not(test))]
#[inline(always)]
fn count_runtime_loop_iteration() {}

static CONTENT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_content_generation() -> u64 {
    CONTENT_GENERATION
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_add(1)
}
const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);
#[cfg(test)]
const FINAL_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
#[cfg(all(test, unix))]
const SHUTDOWN_GRACE: Duration = Duration::from_millis(100);
#[cfg(all(test, target_os = "windows"))]
const WINDOWS_CHILD_POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_CLIPBOARD_EVENTS: usize = 8;
const MAX_NOTIFICATION_EVENTS: usize = 8;

const _: () = assert!(OUTPUT_POOL_BYTES <= NFR_005_MAX_PENDING_OUTPUT_BYTES);
const _: () = assert!(MAX_QUEUED_INPUT_BYTES <= NFR_005_MAX_QUEUED_INPUT_BYTES);

pub(super) struct GhosttyRuntimePending {
    mailbox: Arc<RuntimeMailbox>,
}

#[cfg(test)]
pub(super) struct SpawnedGhostty {
    pub(super) child_pid: u32,
    pub(super) cwd: std::path::PathBuf,
}

#[derive(Debug)]
pub(super) enum GhosttyStartError {
    Initialization(anyhow::Error),
}

impl std::fmt::Display for GhosttyStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialization(_) => formatter.write_str("Ghostty initialization failed"),
        }
    }
}

struct SharedState {
    content: Content,
    modes: Modes,
    metrics: GridMetrics,
    kitty: Arc<[crate::terminal::kitty::KittyPlacement]>,
    search: Arc<crate::search::NativeSearchState>,
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct DragTarget {
    point: ghostty::Point,
    position: (f64, f64),
    geometry: ghostty::GestureGeometry,
    rectangle: bool,
}

#[derive(Default)]
struct GestureUpdateState {
    kind: Option<SelectionKind>,
    generation: u64,
    requested: Option<DragTarget>,
    in_flight: Option<(u64, DragTarget)>,
    applied: Option<DragTarget>,
    queued_generation: Option<u64>,
}

struct SessionInner {
    mailbox: Arc<RuntimeMailbox>,
    events_tx: UnboundedSender<GhosttyUiEvent>,
    ui_events: Arc<UiEventState>,
    clipboard_gate: Arc<ClipboardGate>,
    state: RwLock<SharedState>,
    kitty_images: Mutex<crate::terminal::kitty::KittyImages>,
    recent_output_lines: RwLock<Arc<[String]>>,
    search_generation: AtomicU64,
    queued_input_bytes: AtomicUsize,
    command_backpressure: AtomicBool,
    promoted: AtomicBool,
    shutdown_sent: AtomicBool,
    exit_published: AtomicBool,
    option_as_alt: AtomicBool,
    #[cfg(test)]
    processed_output_bytes: AtomicUsize,
    #[cfg(test)]
    worker_crash_injected: AtomicBool,
    resize: Mutex<ResizeState>,
    gesture: Mutex<GestureUpdateState>,
    marks: SharedMarkRing,
}

#[derive(Clone)]
pub(super) struct GhosttySession {
    inner: Arc<SessionInner>,
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
                    kitty: Arc::from([]),
                    search: Arc::default(),
                }),
                kitty_images: Mutex::default(),
                recent_output_lines: RwLock::new(Arc::from(Vec::<String>::new())),
                search_generation: AtomicU64::new(0),
                queued_input_bytes: AtomicUsize::new(0),
                command_backpressure: AtomicBool::new(false),
                promoted: AtomicBool::new(false),
                shutdown_sent: AtomicBool::new(false),
                exit_published: AtomicBool::new(false),
                option_as_alt: AtomicBool::new(false),
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
                gesture: Mutex::new(GestureUpdateState::default()),
                marks: Arc::new(Mutex::new(Default::default())),
            }),
        };
        (session, GhosttyRuntimePending { mailbox }, events_rx)
    }

    #[cfg(test)]
    pub(super) fn start(
        &self,
        pending: GhosttyRuntimePending,
        params: SpawnParams,
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
            Ok(StartupReport::Failed(error)) => Err(GhosttyStartError::Initialization(error)),
            Err(error) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(
                "Ghostty runtime exited before startup completed: {error}"
            ))),
        }
    }

    pub(super) fn start_display(
        &self,
        pending: GhosttyRuntimePending,
        max_scrollback: usize,
    ) -> Result<(), GhosttyStartError> {
        let (startup_tx, startup_rx) = sync_channel(1);
        let inner = self.inner.clone();
        let runtime_mailbox = pending.mailbox.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-ghostty-display".into())
            .spawn(move || {
                run_display_runtime(inner, runtime_mailbox, max_scrollback, startup_tx);
            })
        {
            pending.mailbox.close();
            return Err(GhosttyStartError::Initialization(
                anyhow::Error::new(error).context("could not start Ghostty display runtime thread"),
            ));
        }

        match startup_rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(error))),
            Err(error) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(
                "Ghostty display runtime exited before startup completed: {error}"
            ))),
        }
    }

    pub(super) fn start_attached(
        &self,
        pending: GhosttyRuntimePending,
        attachment: HostAttachment,
        snapshot: CheckpointPayload,
        max_scrollback: usize,
    ) -> Result<(), GhosttyStartError> {
        let (startup_tx, startup_rx) = sync_channel(1);
        let inner = self.inner.clone();
        let runtime_mailbox = pending.mailbox.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-ghostty-attached".into())
            .spawn(move || {
                let boundary_inner = inner.clone();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_attached_runtime(
                        inner,
                        runtime_mailbox,
                        attachment,
                        snapshot,
                        max_scrollback,
                        startup_tx,
                    );
                }));
                if result.is_err() {
                    boundary_inner.shutdown_sent.store(true, Ordering::Release);
                    stop_session_input(&boundary_inner);
                    let _ = boundary_inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                            "Ghostty attached mirror terminated unexpectedly".to_owned(),
                        ));
                }
            })
        {
            pending.mailbox.close();
            return Err(GhosttyStartError::Initialization(
                anyhow::Error::new(error).context("could not start Ghostty attached mirror thread"),
            ));
        }

        match startup_rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(error))),
            Err(error) => Err(GhosttyStartError::Initialization(anyhow::anyhow!(
                "Ghostty attached mirror exited before startup completed: {error}"
            ))),
        }
    }

    pub(super) fn write_output(&self, bytes: &[u8]) {
        let _ = self.request(|reply| RuntimeMessage::WriteOutput {
            bytes: bytes.to_vec(),
            reply,
        });
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

    pub(super) fn write_paste(
        &self,
        text: String,
        allow_unsafe: bool,
        location: ghostty::ClipboardLocation,
    ) -> GhosttyInputSendResult {
        if text.is_empty() {
            return GhosttyInputSendResult::Sent;
        }
        self.enqueue_input(RuntimeMessage::PasteInput {
            text,
            allow_unsafe,
            location,
        })
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

    pub(super) fn requested_window_size(&self) -> TerminalWindowSize {
        self.inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .requested
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
        let mut gesture = self.lock_gesture();
        self.submit_requested_drag(&mut gesture);
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

    fn lock_gesture(&self) -> std::sync::MutexGuard<'_, GestureUpdateState> {
        self.inner
            .gesture
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn submit_requested_drag(&self, gesture: &mut GestureUpdateState) {
        if gesture.queued_generation == Some(gesture.generation) || gesture.requested.is_none() {
            return;
        }
        let generation = gesture.generation;
        match self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::DragSelection(generation))
        {
            Ok(()) => gesture.queued_generation = Some(generation),
            Err(TrySendError::Full(_)) => self
                .inner
                .command_backpressure
                .store(true, Ordering::Release),
            Err(TrySendError::Disconnected(_)) => gesture.requested = None,
        }
    }

    fn invalidate_gesture(&self, gesture: &mut GestureUpdateState) {
        gesture.generation = gesture.generation.wrapping_add(1);
        gesture.requested = None;
        gesture.in_flight = None;
        gesture.applied = None;
        gesture.queued_generation = None;
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

    pub(super) fn kitty_placements(&self) -> Arc<[crate::terminal::kitty::KittyPlacement]> {
        self.inner.state.read().kitty.clone()
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

    pub(super) fn press_selection(&self, kind: SelectionKind, point: Point, position: (f32, f32)) {
        {
            let mut gesture = self.lock_gesture();
            self.invalidate_gesture(&mut gesture);
            gesture.kind = Some(kind);
        }
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::PressSelection {
                point: ghostty_point(point),
                behavior: gesture_behavior(kind),
                position: pixel_position(position),
            });
    }

    pub(super) fn drag_selection(
        &self,
        point: Point,
        position: (f32, f32),
        geometry: SelectionGeometry,
        rectangle: bool,
    ) {
        let Some(geometry) = gesture_geometry(geometry) else {
            return;
        };
        let target = DragTarget {
            point: ghostty_point(point),
            position: pixel_position(position),
            geometry,
            rectangle,
        };
        let mut gesture = self.lock_gesture();
        if gesture.kind.is_none() {
            return;
        }
        if gesture.requested == Some(target)
            || (gesture.requested.is_none()
                && gesture.in_flight.is_some_and(|(generation, pending)| {
                    generation == gesture.generation && pending == target
                }))
            || (gesture.requested.is_none()
                && gesture.in_flight.is_none()
                && gesture.applied == Some(target))
        {
            return;
        }
        gesture.requested = Some(target);
        self.submit_requested_drag(&mut gesture);
    }

    pub(super) fn release_selection(&self, point: Option<Point>) {
        if self.lock_gesture().kind.is_none() {
            return;
        }
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::ReleaseSelection {
                point: point.map(ghostty_point),
            });
    }

    pub(super) fn selection_text(&self) -> Option<String> {
        let text = self
            .request(RuntimeMessage::SelectionText)
            .and_then(Result::ok)
            .flatten();
        let kind = self.lock_gesture().kind;
        filter_copyable_selection_text(kind, self.selection_range(), text)
    }

    pub(super) fn select_all_text(&self) -> Option<String> {
        {
            let mut gesture = self.lock_gesture();
            self.invalidate_gesture(&mut gesture);
            gesture.kind = None;
        }
        self.request_within(SELECT_ALL_TIMEOUT, RuntimeMessage::SelectAll)
            .and_then(Result::ok)
            .flatten()
    }

    pub(super) fn bind_runtime(&self, runtime_id: Option<&'static str>) {
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::BindRuntime(runtime_id));
    }

    pub(super) fn clear_history(&self) {
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::ClearScrollback);
    }

    pub(super) fn clear_selection(&self) {
        {
            let mut gesture = self.lock_gesture();
            self.invalidate_gesture(&mut gesture);
            gesture.kind = None;
        }
        let _ = self
            .inner
            .mailbox
            .try_send_control(RuntimeMessage::ClearSelection);
    }

    pub(super) fn selection_range(&self) -> Option<SelectionRange> {
        self.inner.state.read().content.selection
    }

    pub(super) fn set_default_cursor(&self, shape: ghostty::CursorShape, blink: bool) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::SetDefaultCursor { shape, blink })
            .is_ok()
    }

    pub(super) fn set_option_as_alt(&self, enabled: bool) -> bool {
        self.inner.option_as_alt.store(enabled, Ordering::Release);
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::SetOptionAsAlt(enabled))
            .is_ok()
    }

    pub(super) fn refresh_appearance(&self) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::UpdateAppearance(
                current_ghostty_appearance(),
            ))
            .is_ok()
    }

    pub(super) fn request_hyperlink_at(&self, point: Point) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::HyperlinkHover(ghostty_point(point)))
            .is_ok()
    }

    pub(super) fn line_text_at(&self, point: Point) -> Option<GridLineText> {
        let state = self.inner.state.read();
        let content = &state.content;
        let row = usize::try_from(point.line.0).ok()?;
        let start = row.checked_mul(content.cols)?;
        let cells = content
            .cells
            .get(start..start.checked_add(content.cols)?)
            .filter(|cells| {
                cells
                    .first()
                    .is_some_and(|cell| cell.point.line == point.line)
            })?;
        let mut text = String::with_capacity(cells.len());
        let mut char_to_column = Vec::with_capacity(cells.len());
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

    pub(super) fn set_native_search(&self, query: String) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::SetNativeSearch(query))
            .is_ok()
    }

    pub(super) fn select_native_search(&self, previous: bool, generation: u64) -> bool {
        self.inner
            .mailbox
            .try_send_control(RuntimeMessage::SelectNativeSearch {
                previous,
                generation,
            })
            .is_ok()
    }

    pub(super) fn native_search_state(&self) -> Arc<crate::search::NativeSearchState> {
        self.inner.state.read().search.clone()
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

    pub(super) fn screen_text(&self) -> Option<String> {
        self.request(RuntimeMessage::ScreenText)
            .and_then(Result::ok)
    }

    pub(super) fn capture_replay(&self) -> Option<Vec<u8>> {
        self.request(RuntimeMessage::CaptureReplay)
            .and_then(Result::ok)
            .filter(|replay| !replay.is_empty())
    }

    pub(super) fn restore_scrollback(&self, text: &str) {
        let _ = self.request(|reply| RuntimeMessage::RestoreScrollback {
            text: text.to_owned(),
            reply,
        });
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
        self.request_within(Duration::from_secs(1), command)
    }

    fn request_within<T>(
        &self,
        timeout: Duration,
        command: impl FnOnce(SyncSender<T>) -> RuntimeMessage,
    ) -> Option<T> {
        let (reply_tx, reply_rx) = sync_channel(1);
        self.inner
            .mailbox
            .try_send_control(command(reply_tx))
            .ok()?;
        reply_rx.recv_timeout(timeout).ok()
    }
}

const TERMINFO_NAME: &str = "xterm-256color";

const SCROLLBACK_BYTES_PER_LINE: usize = 1024;

const MAX_SCROLLBACK_BYTES: usize = 128 * 1024 * 1024;

fn option_as_alt(enabled: bool) -> ghostty::OptionAsAlt {
    if enabled {
        ghostty::OptionAsAlt::Always
    } else {
        ghostty::OptionAsAlt::Never
    }
}

fn configure_embedder_options(
    terminal: &mut ghostty::DisplayTerminal,
    max_scrollback: usize,
    option_as_meta: bool,
) {
    let apply = |what: &str, result: ghostty::Result<()>| {
        if let Err(error) = result {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty {what} could not be configured: {error}"
            );
        }
    };
    crate::terminal::kitty::enable(terminal);
    apply(
        "color palette",
        terminal.set_palette(&current_ghostty_palette()),
    );
    apply("terminfo name", terminal.set_terminfo_name(TERMINFO_NAME));
    apply("glyph protocol", terminal.set_glyph_protocol(false));
    terminal.set_option_as_alt(option_as_alt(option_as_meta));
    apply(
        "scrollback byte budget",
        terminal.set_scrollback_max_bytes(Some(
            max_scrollback
                .saturating_mul(SCROLLBACK_BYTES_PER_LINE)
                .min(MAX_SCROLLBACK_BYTES),
        )),
    );
    if log::log_enabled!(target: "paneflow::terminal::ghostty", log::Level::Debug) {
        apply(
            "unsupported sequence capture",
            terminal.capture_unknown_sequences(true),
        );
    }
}

fn current_ghostty_palette() -> [ghostty::Rgb; ghostty::PALETTE_LEN] {
    crate::theme::generated_terminal_palette(&crate::theme::active_theme())
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

#[cfg(test)]
mod tests {
    use super::super::pty_session::{BackendInputResult, TerminalState};
    use super::*;

    #[test]
    fn nfr_005_terminal_queue_caps_stay_below_budget() {
        assert_eq!(OUTPUT_POOL_BYTES, 128 * 1024);
        assert_eq!(MAX_QUEUED_INPUT_BYTES, 1024 * 1024);
    }

    #[test]
    fn promotion_replays_pending_input_once_in_order_and_enforces_cap() {
        let (mut state, pending) = TerminalState::new_pending(80, 24);
        let runtime_pending = pending.ghostty;

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
        let (mut state, pending) = TerminalState::new_pending(80, 24);
        let runtime_pending = pending.ghostty;
        let session = state.ghostty_session();
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
            state.write_ghostty_key(key.clone()),
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

    fn option_a_key() -> ghostty::KeyInput {
        let text = if cfg!(target_os = "macos") {
            "\u{e5}"
        } else {
            "a"
        };
        ghostty::KeyInput {
            key: ghostty::Key::Character('a'),
            action: ghostty::KeyAction::Press,
            modifiers: ghostty::Modifiers::ALT,
            consumed_modifiers: ghostty::Modifiers::empty(),
            text: text.to_string(),
            composing: false,
            unshifted_codepoint: Some('a'),
        }
    }

    fn embedder_terminal(option_as_meta: bool) -> ghostty::DisplayTerminal {
        let size = window_size(TerminalWindowSize::new(20, 4, 8, 16)).expect("window size");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("terminal");
        configure_embedder_options(&mut terminal, 100, option_as_meta);
        terminal
    }

    #[test]
    fn the_embedder_options_keep_glyph_protocol_queries_unanswered() {
        let mut terminal = embedder_terminal(false);
        terminal
            .feed(b"\x1b_25a1;s\x1b\\")
            .expect("glyph query parses");
        assert!(
            !terminal
                .drain_events()
                .into_iter()
                .any(|event| matches!(event, ghostty::BackendEvent::WritePty(_)))
        );
    }

    #[test]
    fn option_as_meta_reaches_the_key_encoder() {
        let composed: &[u8] = if cfg!(target_os = "macos") {
            "\u{e5}".as_bytes()
        } else {
            b"\x1ba"
        };
        let mut terminal = embedder_terminal(true);
        assert_eq!(
            terminal.encode_key(&option_a_key()).expect("encode"),
            b"\x1ba"
        );
        let mut terminal = embedder_terminal(false);
        assert_eq!(
            terminal.encode_key(&option_a_key()).expect("encode"),
            composed
        );

        let (session, _pending, _events) =
            GhosttySession::pending(TerminalWindowSize::new(20, 4, 8, 16));
        assert!(session.set_option_as_alt(true));
        assert!(session.inner.option_as_alt.load(Ordering::Acquire));
        let mut gate = PublishGate::new();
        let outcome = handle_terminal_command(
            &session.inner,
            &mut terminal,
            &mut gate,
            RuntimeMessage::SetOptionAsAlt(true),
        );
        assert!(matches!(outcome, CommandOutcome::Handled));
        assert_eq!(
            terminal.encode_key(&option_a_key()).expect("encode"),
            b"\x1ba"
        );

        let snapshot = terminal.encode_snapshot().expect("checkpoint");
        let size = TerminalWindowSize::new(20, 4, 8, 16);
        let mut restored = restore_terminal_from_checkpoint(&snapshot, size, 100, true)
            .expect("checkpoint restores");
        assert_eq!(
            restored.encode_key(&option_a_key()).expect("encode"),
            b"\x1ba"
        );
    }
}
