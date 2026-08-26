//! `TerminalState` and its PTY lifecycle - spawn, notifier wiring, event
//! processing, OSC channel drains, CWD resolution, scrollback I/O, and the
//! drop-time force-kill path.
//!
//! Cross-platform: POSIX syscalls (`libc::kill`, `proc_pidinfo`) are behind
//! `#[cfg(unix)]` / `#[cfg(target_os = "macos")]`; Windows paths use
//! `windows-sys` (`TerminateProcess`, `WaitForSingleObject`).
//!
//! Extracted from `terminal.rs` per US-012 of the src-app refactor PRD.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::sync::Arc;

use alacritty_terminal::Term;
use alacritty_terminal::event::{Event as AlacEvent, Notify, WindowSize as AlacWindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::{Dimensions, Scroll as AlacScroll};
use alacritty_terminal::index::{
    Column as GridCol, Line as GridLine, Point as AlacPoint, Side as AlacSide,
};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::Rgb as AlacRgb;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::element::color::palette_color_at;
use super::listener::{ClipboardGate, SpikeTermSize, ZedListener};
use super::marks::{CommandMark, Osc133Scanner, RawMark, SharedMarkRing};
use super::service_detector::{ServiceInfo, detect_framework, parse_service_line};
use super::shell::{resolve_default_shell, setup_shell_integration};
use super::types::{
    Content, GridLineText, GridMetrics, HyperlinkSource, HyperlinkZone, Line, Modes, Point,
    SelectionKind, SelectionRange, SelectionSide, SharedTerm, ShellQuoting, TerminalWindowSize,
    content_from_term_visible, resize_if_needed,
};
use crate::limits::{MAX_CHARS, MAX_OSC52_BYTES};
use paneflow_config::schema::{TerminalBackendConfig, TerminalConfig, TerminalSurfaceProfile};

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
use super::ghostty_session::{
    GhosttyInputSendResult, GhosttySession, GhosttyUiEvent, SpawnedGhostty,
};

/// Default scrollback history length, in lines. Paneflow keeps this standard
/// for predictable terminal memory use. `TermConfig::default()` is `0`, which
/// disables scrollback entirely. Overridable via
/// `terminal.scrollback_lines` in `paneflow.json` - see
/// [`paneflow_config::TerminalConfig::resolved_scrollback_lines`].
const DEFAULT_SCROLLBACK_LINES: usize = TerminalConfig::DEFAULT_SCROLLBACK_LINES;
const PTY_DRAIN_ON_EXIT: bool = true;
/// Identity and credential markers Claude Code exports into the processes it
/// spawns. A pane that inherits them makes the agent running inside it believe
/// it is a *nested child* of the Claude Code session that launched Paneflow.
///
/// The consequence is silent and expensive: on inheriting
/// `CLAUDE_CODE_CHILD_SESSION` the agent turns transcript saving off, so its
/// conversation never lands in `~/.claude/projects/<slug>/<uuid>.jsonl`. The
/// thread then has no session file to come back to, and reopening it after a
/// restart starts an empty conversation instead of resuming - the failure only
/// announces itself as one dim line at the bottom of the agent's own TUI.
///
/// The messaging socket/token pair is stripped for a second reason: it is a
/// live credential for the launching session's IPC channel, and no agent
/// spawned in a pane has any business holding it.
///
/// A pane must always look like a fresh terminal, never like a continuation of
/// whatever spawned Paneflow. Inheriting is the only way these reach the child
/// - Paneflow itself namespaces everything it sets under `PANEFLOW_*`.
const INHERITED_AGENT_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];
const MAX_PENDING_CLIPBOARD_OPS: usize = 8;

/// Read the user's configured scrollback length, clamped to the
/// [`paneflow_config::TerminalConfig`] allowed range. Falls back to
/// [`DEFAULT_SCROLLBACK_LINES`] when no `terminal` block exists.
fn resolved_scrollback_lines(profile: TerminalSurfaceProfile) -> usize {
    paneflow_config::loader::load_config()
        .terminal
        .unwrap_or(TerminalConfig {
            scrollback_lines: Some(DEFAULT_SCROLLBACK_LINES),
            ..Default::default()
        })
        .resolved_scrollback_lines_for_profile(profile)
}

/// US-007: map the pure config cursor shape to the backend's nearest native
/// shape. Paneflow paints `Vintage` and `DoubleUnderline` itself on top of
/// these fallback shapes.
fn map_cursor_shape(
    c: paneflow_config::schema::CursorShapeConfig,
) -> alacritty_terminal::vte::ansi::CursorShape {
    use alacritty_terminal::vte::ansi::CursorShape;
    use paneflow_config::schema::CursorShapeConfig as C;
    match c {
        C::Vintage => CursorShape::Block,
        C::Block => CursorShape::Block,
        C::Beam => CursorShape::Beam,
        C::Underline => CursorShape::Underline,
        C::DoubleUnderline => CursorShape::Underline,
        C::Hollow => CursorShape::HollowBlock,
    }
}

/// US-007: resolve the configured default cursor shape into an alacritty
/// `CursorStyle`, applied as `TermConfig.default_cursor_style` so it is the
/// fallback before any app-driven DECSCUSR escape. Blinking stays at the
/// alacritty default; cursor blink is overridden at the view layer (US-008).
fn resolved_cursor_style() -> alacritty_terminal::vte::ansi::CursorStyle {
    use alacritty_terminal::vte::ansi::{CursorShape, CursorStyle};
    let shape = paneflow_config::loader::load_config()
        .terminal
        .as_ref()
        .and_then(|t| t.cursor_shape)
        .map(map_cursor_shape)
        .unwrap_or(CursorShape::Block);
    CursorStyle {
        shape,
        ..CursorStyle::default()
    }
}

fn resolved_cursor_color_override() -> Option<gpui::Hsla> {
    paneflow_config::loader::load_config()
        .terminal
        .and_then(|t| t.normalized_cursor_color())
        .and_then(|hex| u32::from_str_radix(&hex[1..], 16).ok())
        .map(|rgb| gpui::Hsla::from(gpui::rgb(rgb)))
}

// ---------------------------------------------------------------------------
// PTY notifier - replaces alacritty's Notifier (US-007, portable-pty)
// ---------------------------------------------------------------------------

/// Whether a terminal is backed by a real PTY (driven by an alacritty
/// `EventLoop`) or is display-only (VTE-rendered content, no PTY, no input).
/// Mirrors Zed's `TerminalType` (`crates/terminal/src/terminal.rs:1281-1287`):
/// the `Pty` variant owns the `EventLoop` write channel; `DisplayOnly` drops
/// every write. Held inside [`PtySender`] so the Pty-vs-display-only state is
/// one named enum instead of an anonymous `Option`, and so US-012 can *promote*
/// a `DisplayOnly` terminal to `Pty` once a background spawn resolves.
#[derive(Clone)]
pub enum TerminalType {
    /// A live PTY: writes go to the alacritty `EventLoop` channel.
    Pty(EventLoopSender),
    /// No PTY: input / resize / shutdown are dropped.
    DisplayOnly,
}

/// The write side of a terminal - routes input / resize / shutdown to the PTY
/// `EventLoop` (or drops them for a display-only terminal). Mirrors Zed's
/// `PtySender` (`crates/terminal/src/alacritty.rs:84-108`), which exposes only
/// notify / resize / shutdown - never the raw `Msg` channel.
#[derive(Clone)]
pub struct PtySender(TerminalType);

impl PtySender {
    fn new(kind: TerminalType) -> Self {
        Self(kind)
    }

    /// Real sender wired to a live `EventLoop` channel.
    pub(super) fn pty(sender: EventLoopSender) -> Self {
        Self::new(TerminalType::Pty(sender))
    }

    /// Display-only sender: every write is dropped (no PTY, no `EventLoop`).
    pub(super) fn display_only() -> Self {
        Self::new(TerminalType::DisplayOnly)
    }

    /// Whether this is a live PTY (vs display-only / not-yet-promoted). A
    /// display-only sender already drops every write, so this is an explicit
    /// readiness query for callers/tests rather than a guard the write path
    /// needs.
    #[allow(dead_code)]
    pub fn is_pty(&self) -> bool {
        matches!(self.0, TerminalType::Pty(_))
    }

    /// Internal: drop the message for a display-only terminal, otherwise hand it
    /// to the `EventLoop`. The send error is ignored - a closed channel means
    /// the child already exited, which the exit path handles.
    fn send(&self, msg: Msg) {
        if let TerminalType::Pty(sender) = &self.0 {
            let _ = sender.send(msg);
        }
    }

    /// Forward input bytes to the child (the [`Notify`] path).
    pub fn write(&self, bytes: Cow<'static, [u8]>) {
        // alacritty: the terminal hangs if 0 bytes are sent through.
        if bytes.is_empty() {
            return;
        }
        self.send(Msg::Input(bytes));
    }

    /// Resize the PTY grid (drives SIGWINCH to the child).
    pub fn resize(&self, size: AlacWindowSize) {
        self.send(Msg::Resize(size));
    }

    /// Ask the `EventLoop` to shut down (sent from `Drop` before the teardown
    /// ladder).
    pub fn shutdown(&self) {
        self.send(Msg::Shutdown);
    }
}

/// Wrapper for the PTY write channel. Implements `Notify` for input and exposes
/// the resize convenience - same usage pattern as alacritty's `Notifier` (which
/// [`PtySender`] now wraps).
#[derive(Clone)]
pub struct PtyNotifier(pub PtySender);

impl Notify for PtyNotifier {
    fn notify<B: Into<Cow<'static, [u8]>>>(&self, bytes: B) {
        self.0.write(bytes.into());
    }
}

#[inline]
fn saturating_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

pub(crate) fn alacritty_window_size(size: TerminalWindowSize) -> AlacWindowSize {
    AlacWindowSize {
        num_cols: saturating_u16(size.cols),
        num_lines: saturating_u16(size.rows),
        cell_width: size.cell_width,
        cell_height: size.cell_height,
    }
}

impl PtyNotifier {
    /// Resize the PTY using the full cached terminal window size. This includes
    /// metrics-only changes, such as font zoom, that do not alter grid columns
    /// or rows but still affect size query replies and platform PTY pixel fields.
    pub fn notify_window_size(&self, size: TerminalWindowSize) {
        self.0.resize(alacritty_window_size(size));
    }
}

/// Cloneable renderer-facing session facade. The concrete Alacritty handles
/// stay private to this backend module; GPUI receives only Paneflow-owned
/// commands and snapshots. EP-002 can add a Ghostty implementation behind the
/// same facade without changing `TerminalElement`.
#[derive(Clone)]
pub(crate) struct TerminalSessionBackend {
    term: SharedTerm,
    notifier: PtyNotifier,
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    ghostty: Option<GhosttySession>,
}

/// Opaque event emitted by the concrete backend. The view can coalesce wakeups
/// without importing or pattern-matching Alacritty's event enum.
pub(crate) enum TerminalBackendEvent {
    Alacritty(AlacEvent),
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Ghostty(GhosttyUiEvent),
}

impl TerminalBackendEvent {
    pub(crate) fn is_wakeup(&self) -> bool {
        match self {
            Self::Alacritty(event) => matches!(event, AlacEvent::Wakeup),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Ghostty(event) => event.is_wakeup(),
        }
    }
}

pub(crate) struct TerminalBackendEvents {
    alacritty: Option<UnboundedReceiver<AlacEvent>>,
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    ghostty: Option<UnboundedReceiver<GhosttyUiEvent>>,
}

impl futures::Stream for TerminalBackendEvents {
    type Item = TerminalBackendEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(receiver) = self.ghostty.as_mut() {
            match futures::Stream::poll_next(std::pin::Pin::new(receiver), cx) {
                std::task::Poll::Ready(Some(event)) => {
                    return std::task::Poll::Ready(Some(TerminalBackendEvent::Ghostty(event)));
                }
                std::task::Poll::Ready(None) => self.ghostty = None,
                std::task::Poll::Pending => {}
            }
        }
        if let Some(receiver) = self.alacritty.as_mut() {
            match futures::Stream::poll_next(std::pin::Pin::new(receiver), cx) {
                std::task::Poll::Ready(Some(event)) => {
                    return std::task::Poll::Ready(Some(TerminalBackendEvent::Alacritty(event)));
                }
                std::task::Poll::Ready(None) => self.alacritty = None,
                std::task::Poll::Pending => {}
            }
        }
        if futures::stream::FusedStream::is_terminated(&*self) {
            std::task::Poll::Ready(None)
        } else {
            std::task::Poll::Pending
        }
    }
}

impl futures::stream::FusedStream for TerminalBackendEvents {
    fn is_terminated(&self) -> bool {
        self.alacritty.is_none() && {
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            {
                self.ghostty.is_none()
            }
            #[cfg(not(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            )))]
            {
                true
            }
        }
    }
}

/// Concrete spawn state that can cross the background executor boundary but
/// cannot be inspected by the view.
pub(crate) struct PendingTerminalBackend {
    term: SharedTerm,
    events_tx: UnboundedSender<AlacEvent>,
    clipboard_gate: Arc<ClipboardGate>,
}

#[cfg(test)]
static RENDER_CONTENT_TIMING_ENABLED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(test)]
static RENDER_CONTENT_LOCK_DURATIONS: std::sync::Mutex<Vec<std::time::Duration>> =
    std::sync::Mutex::new(Vec::new());

#[cfg(test)]
pub(crate) fn start_render_content_timing_probe() {
    let mut durations = RENDER_CONTENT_LOCK_DURATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    durations.clear();
    RENDER_CONTENT_TIMING_ENABLED.store(true, std::sync::atomic::Ordering::Release);
}

#[cfg(test)]
pub(crate) fn take_render_content_lock_durations() -> Vec<std::time::Duration> {
    RENDER_CONTENT_TIMING_ENABLED.store(false, std::sync::atomic::Ordering::Release);
    let mut durations = RENDER_CONTENT_LOCK_DURATIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *durations)
}

impl TerminalSessionBackend {
    fn alacritty(term: SharedTerm, notifier: PtyNotifier) -> Self {
        Self {
            term,
            notifier,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty: None,
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn ghostty(term: SharedTerm, notifier: PtyNotifier, ghostty: GhosttySession) -> Self {
        Self {
            term,
            notifier,
            ghostty: Some(ghostty),
        }
    }

    /// Resize and snapshot under one terminal lock, then return owned neutral
    /// content. No Alacritty handle or borrowed grid data crosses this call.
    pub(crate) fn render_content(
        &self,
        window_size: TerminalWindowSize,
        first_visible_row: i32,
        last_visible_row: i32,
        clear_on_resize: bool,
    ) -> (Content, bool) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.render_content(
                window_size,
                first_visible_row,
                last_visible_row,
                clear_on_resize,
            );
        }
        #[cfg(test)]
        let measure_lock_duration =
            RENDER_CONTENT_TIMING_ENABLED.load(std::sync::atomic::Ordering::Acquire);
        let mut term = self.term.lock();
        #[cfg(test)]
        let lock_acquired_at = measure_lock_duration.then(std::time::Instant::now);
        let resized = resize_if_needed(&mut term, window_size.cols, window_size.rows);
        let initial_clear_consumed = clear_on_resize && resized;
        if initial_clear_consumed {
            term.grid_mut().reset();
        }
        let content = content_from_term_visible(&term, first_visible_row, last_visible_row);
        drop(term);
        #[cfg(test)]
        if let Some(lock_acquired_at) = lock_acquired_at {
            RENDER_CONTENT_LOCK_DURATIONS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(lock_acquired_at.elapsed());
        }
        (content, initial_clear_consumed)
    }

    pub(crate) fn notify_window_size(&self, size: TerminalWindowSize) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            ghostty.resize(size);
            return;
        }
        self.notifier.notify_window_size(size);
    }

    pub(crate) fn modes(&self) -> Modes {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.modes();
        }
        Modes::from(*self.term.lock_unfair().mode())
    }

    pub(crate) fn grid_metrics(&self) -> GridMetrics {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.grid_metrics();
        }
        let term = self.term.lock_unfair();
        GridMetrics {
            columns: term.columns(),
            screen_lines: term.screen_lines(),
            display_offset: term.grid().display_offset(),
            topmost_line: Line(term.topmost_line().0),
            bottommost_line: Line(term.bottommost_line().0),
            cursor: term.renderable_content().cursor.point.into(),
        }
    }

    pub(crate) fn grid_size(&self) -> (usize, usize) {
        let metrics = self.grid_metrics();
        (metrics.columns, metrics.screen_lines)
    }

    pub(crate) fn clear_history(&self) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if self.ghostty.is_some() {
            return;
        }
        self.term.lock().grid_mut().clear_history();
    }

    pub(crate) fn scroll_to_bottom(&self) -> bool {
        self.scroll(AlacScroll::Bottom)
    }

    pub(crate) fn scroll_delta(&self, delta: i32) -> bool {
        self.scroll(AlacScroll::Delta(delta))
    }

    pub(crate) fn scroll_page_up(&self) -> bool {
        self.scroll(AlacScroll::PageUp)
    }

    pub(crate) fn scroll_page_down(&self) -> bool {
        self.scroll(AlacScroll::PageDown)
    }

    fn scroll(&self, scroll: AlacScroll) -> bool {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let metrics = ghostty.grid_metrics();
            let scroll = match scroll {
                AlacScroll::Top => paneflow_terminal_ghostty::Scroll::Top,
                AlacScroll::Bottom => paneflow_terminal_ghostty::Scroll::Bottom,
                AlacScroll::Delta(delta) => paneflow_terminal_ghostty::Scroll::Delta(delta),
                AlacScroll::PageUp => paneflow_terminal_ghostty::Scroll::Delta(
                    i32::try_from(metrics.screen_lines).unwrap_or(i32::MAX),
                ),
                AlacScroll::PageDown => paneflow_terminal_ghostty::Scroll::Delta(
                    -i32::try_from(metrics.screen_lines).unwrap_or(i32::MAX),
                ),
            };
            // Shared metrics can lag commands already accepted by the worker.
            // Queue every non-zero gesture and let Ghostty's live viewport
            // perform the authoritative boundary clamp in FIFO order.
            if matches!(scroll, paneflow_terminal_ghostty::Scroll::Delta(0)) {
                return false;
            }
            return ghostty.scroll(scroll);
        }
        let mut term = self.term.lock();
        let before = term.grid().display_offset();
        term.scroll_display(scroll);
        term.grid().display_offset() != before
    }

    pub(crate) fn restore_display_offset(&self, target: usize) -> bool {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let metrics = ghostty.grid_metrics();
            let history_size = usize::try_from(-i64::from(metrics.topmost_line.0)).unwrap_or(0);
            let row = history_size.saturating_sub(target.min(history_size));
            return ghostty.scroll_to_viewport_row(row);
        }
        let current = self.grid_metrics().display_offset;
        let delta = target as i64 - current as i64;
        delta != 0 && self.scroll_delta(delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
    }

    pub(crate) fn scroll_to_viewport_row(&self, row: usize) -> bool {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.scroll_to_viewport_row(row);
        }
        let mut term = self.term.lock();
        let history_size = usize::try_from(-i64::from(term.topmost_line().0)).unwrap_or(0);
        let target = history_size.saturating_sub(row.min(history_size));
        let current = term.grid().display_offset();
        let delta = target as i64 - current as i64;
        if delta == 0 {
            return false;
        }
        term.scroll_display(AlacScroll::Delta(
            delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
        ));
        term.grid().display_offset() != current
    }

    pub(crate) fn start_selection(&self, kind: SelectionKind, point: Point, side: SelectionSide) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let _ = side;
            ghostty.start_selection(kind, point);
            return;
        }
        let kind = match kind {
            SelectionKind::Simple => SelectionType::Simple,
            SelectionKind::Semantic => SelectionType::Semantic,
            SelectionKind::Lines => SelectionType::Lines,
        };
        let side = match side {
            SelectionSide::Left => AlacSide::Left,
            SelectionSide::Right => AlacSide::Right,
        };
        self.term.lock().selection = Some(Selection::new(kind, point.into(), side));
    }

    pub(crate) fn update_selection(&self, point: Point, side: SelectionSide) -> Option<String> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.update_selection(point, side);
        }
        let mut term = self.term.lock();
        let side = match side {
            SelectionSide::Left => AlacSide::Left,
            SelectionSide::Right => AlacSide::Right,
        };
        if let Some(selection) = &mut term.selection {
            selection.update(point.into(), side);
        }
        term.selection_to_string()
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.selection_text();
        }
        self.term.lock_unfair().selection_to_string()
    }

    pub(crate) fn finish_selection(&self) -> (bool, Option<String>) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let copied = ghostty.selection_text();
            let is_empty = copied.as_ref().is_none_or(String::is_empty);
            ghostty.clear_selection();
            return (is_empty, copied);
        }
        let mut term = self.term.lock();
        let is_empty = term.selection.as_ref().is_none_or(Selection::is_empty);
        let copied = (!is_empty).then(|| term.selection_to_string()).flatten();
        term.selection = None;
        (is_empty, copied)
    }

    pub(crate) fn clear_selection(&self) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            ghostty.clear_selection();
            return;
        }
        self.term.lock().selection = None;
    }

    pub(crate) fn osc8_hyperlink_at(&self, point: Point) -> Option<HyperlinkZone> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.hyperlink_at(point);
        }
        let term = self.term.lock_unfair();
        let metrics = GridMetrics {
            columns: term.columns(),
            screen_lines: term.screen_lines(),
            display_offset: term.grid().display_offset(),
            topmost_line: Line(term.topmost_line().0),
            bottommost_line: Line(term.bottommost_line().0),
            cursor: term.renderable_content().cursor.point.into(),
        };
        if point.line < metrics.topmost_line
            || point.line > metrics.bottommost_line
            || point.column.0 >= metrics.columns
        {
            return None;
        }
        let cell = &term.grid()[AlacPoint::from(point)];
        cell.hyperlink().map(|hyperlink| HyperlinkZone {
            uri: hyperlink.uri().to_owned(),
            id: hyperlink.id().to_owned(),
            start: point,
            end: point,
            is_openable: super::element::is_url_scheme_openable(hyperlink.uri()),
            source: HyperlinkSource::Osc8,
            line: None,
            col: None,
        })
    }

    pub(crate) fn line_text_at(&self, point: Point) -> Option<GridLineText> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.line_text_at(point);
        }
        use alacritty_terminal::term::cell::Flags;

        let term = self.term.lock_unfair();
        if point.line.0 < term.topmost_line().0 || point.line.0 > term.bottommost_line().0 {
            return None;
        }
        let line = GridLine(point.line.0);
        let mut text = String::with_capacity(term.columns());
        let mut char_to_column = Vec::with_capacity(term.columns());
        for column in 0..term.columns() {
            let cell = &term.grid()[line][GridCol(column)];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            char_to_column.push(column);
            text.push(cell.c);
        }
        Some(GridLineText {
            line: point.line,
            text,
            char_to_column,
        })
    }

    pub(crate) fn move_copy_cursor(&self, current: Point, dx: i32, dy: i32, extend: bool) -> Point {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let metrics = ghostty.grid_metrics();
            let column = (current.column.0 as i32 + dx)
                .clamp(0, metrics.columns.saturating_sub(1) as i32)
                as usize;
            let line =
                (current.line.0 + dy).clamp(metrics.topmost_line.0, metrics.bottommost_line.0);
            let next = Point::new(line, column);
            if extend {
                if ghostty.selection_range().is_none() {
                    ghostty.start_selection(SelectionKind::Simple, current);
                }
                ghostty.update_selection(next, SelectionSide::Right);
            } else {
                ghostty.clear_selection();
            }
            return next;
        }
        let mut term = self.term.lock();
        if extend && term.selection.is_none() {
            term.selection = Some(Selection::new(
                SelectionType::Simple,
                current.into(),
                AlacSide::Left,
            ));
        } else if !extend {
            term.selection = None;
        }
        let column = (current.column.0 as i32 + dx)
            .clamp(0, term.columns().saturating_sub(1) as i32) as usize;
        let line = (current.line.0 + dy).clamp(term.topmost_line().0, term.bottommost_line().0);
        let next = Point::new(line, column);
        if extend && let Some(selection) = &mut term.selection {
            selection.update(next.into(), AlacSide::Right);
        }
        next
    }

    pub(crate) fn selection_range(&self) -> Option<SelectionRange> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.selection_range();
        }
        let term = self.term.lock_unfair();
        term.selection
            .as_ref()
            .and_then(|selection| selection.to_range(&term))
            .map(SelectionRange::from)
    }

    pub(crate) fn bottommost_line(&self) -> Line {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.grid_metrics().bottommost_line;
        }
        Line(self.term.lock_unfair().bottommost_line().0)
    }

    pub(crate) fn search(&self, query: &str, regex: bool) -> crate::search::SearchResult {
        self.search_with_cancel(query, regex, &std::sync::atomic::AtomicBool::new(false))
    }

    pub(crate) fn search_with_cancel(
        &self,
        query: &str,
        regex: bool,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> crate::search::SearchResult {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.search_with_cancel(query, regex, cancelled);
        }
        crate::search::search_term_with_cancel(&self.term, query, regex, cancelled)
    }

    pub(crate) fn refresh_appearance(&self) -> bool {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.refresh_appearance();
        }
        true
    }

    pub(crate) fn scroll_to_match(&self, search_match: &crate::search::SearchMatch) -> usize {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let metrics = ghostty.grid_metrics();
            let target = (metrics.bottommost_line.0 - search_match.start.line.0).max(0) as usize;
            let _ = self.restore_display_offset(target);
            return target;
        }
        crate::search::scroll_to_match(&self.term, search_match)
    }
}

// ---------------------------------------------------------------------------
// OSC 52 clipboard mode
// ---------------------------------------------------------------------------

/// Controls OSC 52 clipboard access. Default: CopyOnly (write-only).
/// Read path (CopyPaste) is a security risk - clipboard exfiltration.
#[derive(Clone, Copy, PartialEq)]
pub enum Osc52Mode {
    Disabled,
    CopyOnly,
    CopyPaste,
}

/// Deferred clipboard operation from sync() - executed in cx.update() closure.
pub(super) enum ClipboardOp {
    Store(String),
    Load(std::sync::Arc<dyn Fn(&str) -> String + Sync + Send + 'static>),
}

/// Convert GPUI Hsla to alacritty Rgb for color query responses.
pub(super) fn hsla_to_alac_rgb(hsla: gpui::Hsla) -> AlacRgb {
    let rgba = gpui::Rgba::from(hsla);
    AlacRgb {
        r: (rgba.r.clamp(0.0, 1.0) * 255.0) as u8,
        g: (rgba.g.clamp(0.0, 1.0) * 255.0) as u8,
        b: (rgba.b.clamp(0.0, 1.0) * 255.0) as u8,
    }
}

// ---------------------------------------------------------------------------
// Terminal state
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhosttyBuildDiagnostics {
    pub version: &'static str,
    pub source_sha: &'static str,
    pub api_version: &'static str,
    pub zig_version: &'static str,
    pub optimization: &'static str,
    pub simd: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(
    dead_code,
    reason = "native backend failure phases are cfg-dependent across the target matrix"
)]
pub enum TerminalBackendFailurePhase {
    Availability,
    Initialization,
    OpenPty,
    Spawn,
    PostSpawn,
}

impl TerminalBackendFailurePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Availability => "availability",
            Self::Initialization => "initialization",
            Self::OpenPty => "open_pty",
            Self::Spawn => "spawn",
            Self::PostSpawn => "post_spawn",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalBackendFailureDiagnostics {
    pub phase: TerminalBackendFailurePhase,
    pub reason_code: &'static str,
    pub os_error: Option<i32>,
}

#[allow(
    dead_code,
    reason = "native backend reason codes are cfg-dependent across the target matrix"
)]
impl TerminalBackendFailureDiagnostics {
    pub(super) const GHOSTTY_UNAVAILABLE: &'static str = "ghostty_unavailable";
    pub(super) const GHOSTTY_INITIALIZATION_FAILED: &'static str = "ghostty_initialization_failed";
    pub(super) const GHOSTTY_OPEN_PTY_FAILED: &'static str = "ghostty_open_pty_failed";
    pub(super) const GHOSTTY_SPAWN_FAILED: &'static str = "ghostty_spawn_failed";
    pub(super) const GHOSTTY_POST_SPAWN_FAILED: &'static str = "ghostty_post_spawn_failed";

    pub(super) fn new(
        phase: TerminalBackendFailurePhase,
        reason_code: &'static str,
        os_error: Option<i32>,
    ) -> Self {
        Self {
            phase,
            reason_code,
            os_error,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalBackendDiagnostics {
    pub requested: TerminalBackendConfig,
    pub effective: &'static str,
    pub failure: Option<TerminalBackendFailureDiagnostics>,
    pub target_triple: &'static str,
    pub ghostty: Option<GhosttyBuildDiagnostics>,
}

impl std::fmt::Display for TerminalBackendDiagnostics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (failure_phase, reason_code, os_error) =
            self.failure
                .as_ref()
                .map_or(("none", "none", None), |failure| {
                    (
                        failure.phase.as_str(),
                        failure.reason_code,
                        failure.os_error,
                    )
                });
        write!(
            formatter,
            "requested={:?} effective={} failure_phase={} reason_code={} target={} os_error=",
            self.requested, self.effective, failure_phase, reason_code, self.target_triple
        )?;
        match os_error {
            Some(code) => write!(formatter, "{code}")?,
            None => formatter.write_str("none")?,
        }
        if let Some(ghostty) = self.ghostty.as_ref() {
            write!(
                formatter,
                " ghostty_version={} ghostty_source_sha={} ghostty_api_version={} zig_version={} optimization={} simd={}",
                ghostty.version,
                ghostty.source_sha,
                ghostty.api_version,
                ghostty.zig_version,
                ghostty.optimization,
                ghostty.simd,
            )?;
        }
        Ok(())
    }
}

pub(super) fn raw_os_error_from_anyhow(error: &anyhow::Error) -> Option<i32> {
    error.chain().find_map(|source| {
        source
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error)
    })
}

#[derive(Clone)]
enum PendingTerminalInput {
    Raw(Cow<'static, [u8]>),
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Key {
        input: paneflow_terminal_ghostty::KeyInput,
        legacy: Option<Cow<'static, [u8]>>,
    },
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Mouse {
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
        legacy: Option<Vec<u8>>,
    },
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Focus(paneflow_terminal_ghostty::FocusEvent),
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Paste {
        text: String,
        allow_unsafe: bool,
        legacy_bracketed: bool,
    },
}

impl PendingTerminalInput {
    fn queued_bytes(&self) -> usize {
        match self {
            Self::Raw(bytes) => bytes.len(),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Key { input, legacy } => {
                std::mem::size_of::<paneflow_terminal_ghostty::KeyInput>()
                    .saturating_add(input.text.len())
                    .saturating_add(legacy.as_ref().map_or(0, |bytes| bytes.len()))
            }
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Mouse { repeat, legacy, .. } => {
                std::mem::size_of::<paneflow_terminal_ghostty::MouseInput>()
                    .saturating_add(*repeat)
                    .saturating_add(legacy.as_ref().map_or(0, Vec::len))
            }
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Focus(_) => std::mem::size_of::<paneflow_terminal_ghostty::FocusEvent>(),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Paste { text, .. } => text.len(),
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn queue_limit(&self) -> usize {
        match self {
            Self::Raw(_) => MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Paste { .. } => MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Key { input, .. }
                if input.action == paneflow_terminal_ghostty::KeyAction::Release =>
            {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Mouse { input, .. }
                if input.action == paneflow_terminal_ghostty::MouseAction::Release =>
            {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Focus(_) => MAX_PENDING_INPUT_BYTES,
            Self::Key { .. } | Self::Mouse { .. } => {
                MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES
            }
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn fits_after(&self, queued_bytes: usize) -> bool {
        queued_bytes.saturating_add(self.queued_bytes()) <= self.queue_limit()
    }

    fn into_legacy_bytes(self) -> Option<Cow<'static, [u8]>> {
        match self {
            Self::Raw(bytes) => Some(bytes),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Key { input, legacy } => legacy
                .or_else(|| (!input.text.is_empty()).then(|| Cow::Owned(input.text.into_bytes()))),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Mouse { repeat, legacy, .. } => legacy.map(|bytes| {
                if repeat == 1 {
                    return Cow::Owned(bytes);
                }
                let mut repeated = Vec::with_capacity(bytes.len().saturating_mul(repeat));
                for _ in 0..repeat {
                    repeated.extend_from_slice(&bytes);
                }
                Cow::Owned(repeated)
            }),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Focus(event) => Some(Cow::Borrowed(match event {
                paneflow_terminal_ghostty::FocusEvent::Gained => b"\x1b[I",
                paneflow_terminal_ghostty::FocusEvent::Lost => b"\x1b[O",
            })),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            Self::Paste {
                text,
                legacy_bracketed,
                ..
            } => {
                if legacy_bracketed {
                    let mut bytes = Vec::with_capacity(text.len().saturating_add(12));
                    bytes.extend_from_slice(b"\x1b[200~");
                    bytes.extend_from_slice(text.as_bytes());
                    bytes.extend_from_slice(b"\x1b[201~");
                    Some(Cow::Owned(bytes))
                } else {
                    Some(Cow::Owned(text.into_bytes()))
                }
            }
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn try_send(&self, ghostty: &GhosttySession) -> GhosttyInputSendResult {
        match self {
            Self::Raw(bytes) => ghostty.write(bytes.clone().into_owned()),
            Self::Key { input, .. } => ghostty.write_key(input.clone()),
            Self::Mouse { input, repeat, .. } => ghostty.write_mouse(*input, *repeat),
            Self::Focus(event) => ghostty.write_focus(*event),
            Self::Paste {
                text, allow_unsafe, ..
            } => ghostty.write_paste(text.clone(), *allow_unsafe),
        }
    }
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BackendInputResult {
    NotHandled,
    Accepted,
    Rejected,
}

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
impl BackendInputResult {
    pub(super) fn is_handled(self) -> bool {
        self != Self::NotHandled
    }
}

pub struct TerminalState {
    term: Arc<FairMutex<Term<ZedListener>>>,
    notifier: PtyNotifier,
    events_rx: Option<UnboundedReceiver<AlacEvent>>,
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    ghostty: Option<GhosttySession>,
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    ghostty_events_rx: Option<UnboundedReceiver<GhosttyUiEvent>>,
    requested_backend: TerminalBackendConfig,
    effective_backend: &'static str,
    backend_failure: Option<TerminalBackendFailureDiagnostics>,
    cwd_rx: Option<UnboundedReceiver<String>>,
    marks_rx: Option<std::sync::mpsc::Receiver<RawMark>>,
    pub(crate) marks: SharedMarkRing,
    pub exited: Option<i32>,
    /// US-002: set true once any user input (keystroke, paste, mouse report,
    /// IME commit, user scroll) has been written via `write_to_pty`.
    /// Distinguishes a user-initiated exit (always close the pane) from a
    /// spawn/launch failure (keep the pane open so the exit overlay is
    /// visible). Atomic because `write_to_pty` takes `&self`. Mirrors Zed's
    /// keyboard_input_sent (crates/terminal/src/terminal.rs:2572-2576).
    keyboard_input_sent: std::sync::atomic::AtomicBool,
    /// EP-002 US-005: numeric signal + name if the child was terminated by a
    /// signal (crash), formatted "N (Name)" e.g. "11 (Segmentation fault)".
    /// `None` for a normal code exit. The numeric signal comes directly from
    /// alacritty's native `ChildExit(ExitStatus)` via `ExitStatusExt::signal()`
    /// (no strsignal reversal); the name is `strsignal(n)`. Set in
    /// `process_event`. Rendered by the exit overlay to flag a crash.
    pub exit_signal: Option<String>,
    /// PID of the shell child process, used for port detection.
    pub child_pid: u32,
    /// US-019: raw fd of the PTY master, captured at spawn before the master
    /// moves into the message-loop thread. macOS uses it to call
    /// `tcgetpgrp(fd)` for live foreground-process naming. `None` on the
    /// display-only / mock paths (no real PTY). macOS-only - Linux resolves the
    /// foreground process from `/proc`, Windows from `child_pid`.
    #[cfg(target_os = "macos")]
    pty_master_fd: Option<i32>,
    /// Terminal title set via OSC 0/2 escape sequences (e.g. shell prompt, Claude Code).
    pub title: String,
    /// Current working directory of the shell process. EP-002 US-007: OSC 7
    /// updates are captured by the PTY byte tap before Alacritty consumes the
    /// sequence; Unix/macOS also refresh from the process table via `cwd_now()`.
    pub current_cwd: Option<String>,
    /// User-assigned custom name (US-013). When `Some`, it overrides the
    /// auto-derived surface name in `surface.list` / MCP / the sidebar, and is
    /// persisted to `session.json`. `None` falls back to derivation.
    pub custom_name: Option<String>,
    /// EP-005 US-013: agent CLI detected in this terminal's PTY subtree by
    /// the per-pane scan - PID-authoritative, never the spoofable OSC
    /// title. Drives the tab identity pill; persisted to `session.json`
    /// as the agent's stable `tag()`.
    pub detected_agent: Option<crate::agent_launcher::TerminalAgent>,
    /// US-013: `false` while `detected_agent` is a declared or
    /// session-restored "last known" value awaiting its first scan
    /// confirmation; flipped `true` (or the agent cleared) by every scan
    /// deposit.
    pub agent_confirmed: bool,
    /// Deadline protecting a *launch-declared* `detected_agent` from being
    /// cleared by a scan that ran before the CLI process existed.
    ///
    /// Paneflow knows which agent a launch is about to run (cmux's declared
    /// `SessionAgent`), so the surface carries its identity from frame zero -
    /// but the shell still needs a moment to start and `exec` the binary, and
    /// the first scan lands inside that window with an empty subtree. Without
    /// a grace period the deposit would clear the declaration and the logo
    /// would flicker off then back on.
    ///
    /// Set by [`TerminalView::declare_agent`] only. A restored "last known"
    /// value leaves it `None` and is still cleared by the first scan, because
    /// nothing is being launched there. Cleared as soon as any scan resolves
    /// the surface either way.
    pub agent_declared_until: Option<std::time::Instant>,
    /// EP-005 US-014: LISTEN ports attributed to this terminal's PTY
    /// subtree by the per-pane scan, each with the clickable frontend URL
    /// when the workspace's `service_labels` knows one. Sorted by port,
    /// deduplicated, and kept as live resource state rather than persisted.
    pub detected_ports: Vec<(u16, Option<String>)>,
    /// US-014: ports whose service URL was announced in THIS terminal
    /// while the LISTEN socket belongs to another pane's subtree -
    /// `(port, owner pane display name)`. Info-level heuristic; known
    /// false positives (proxies, port-forwards, re-announcements) are
    /// tolerated in v1.
    pub port_conflicts: Vec<(u16, String)>,
    /// US-014: ports announced by service URLs detected in this terminal's
    /// own output (untrusted text - used only to cross-check against the
    /// scan's LISTEN attribution, never to open anything). Bounded.
    pub announced_ports: Vec<u16>,
    /// EP-006 US-019: per-pane font-size override in points. `None` follows
    /// the global config (and live global changes); `Some` is clamped to
    /// [8.0, 32.0] at every write site (zoom actions, session ingress) and
    /// wins over the global. Persisted to `session.json`.
    pub font_size_override: Option<f32>,
    /// Cursor color override used for OSC 12 color-query replies.
    pub(super) cursor_color_override: Option<gpui::Hsla>,
    /// OSC 52 clipboard access mode (default: copy-only for security).
    pub osc52_mode: Osc52Mode,
    /// OSC 52 is accepted only while this terminal owns focus. Updated from
    /// the GPUI focus transition before any focus protocol report is queued.
    terminal_focused: bool,
    /// Shared with both parser backends so focus and policy are checked when
    /// OSC 52 is emitted, before the asynchronous UI event queue.
    clipboard_gate: Arc<ClipboardGate>,
    /// Shell syntax used when Paneflow inserts OS file paths into the PTY.
    pub(super) shell_quoting: ShellQuoting,
    /// Deferred clipboard operations from sync() - drained in the poll loop
    /// where cx is available for clipboard read/write.
    pub(super) pending_clipboard_ops: Vec<ClipboardOp>,
    /// Foreground command cached by the off-thread pane process scanner.
    /// `surface.list` reads this synchronously, so it must never perform
    /// process-table I/O on the GPUI thread.
    pub cached_foreground_command: Option<String>,
    #[cfg(all(unix, not(test)))]
    pty_guard: Option<crate::agents::parent_guard::PtyGuardHandle>,
    /// Deferred text area size request responses from sync().
    pub(super) pending_size_ops:
        Vec<std::sync::Arc<dyn Fn(AlacWindowSize) -> String + Sync + Send + 'static>>,
    /// Whether the terminal wants the cursor to blink (from CursorBlinkingChange).
    pub cursor_blinking: bool,
    /// Set when PTY output has been processed (Wakeup event received).
    /// Cleared after cx.notify() triggers a repaint.
    pub dirty: bool,
    /// US-010 (cli-agent-orchestration): monotonic count of processed
    /// PTY-output events (`AlacEvent::Wakeup`). Never reset. `workspace.up`
    /// polls this as a readiness signal for prompt prefill - it is the only
    /// screen-agnostic "the agent produced output" signal available: `dirty`
    /// is cleared on every repaint, and `extract_scrollback` misses content
    /// painted on the alternate screen (where TUI agents live).
    pub output_generation: u64,
    /// Counter for throttling output scans - scans every 50th dirty tick.
    /// Leading-edge throttle for ActivityBurst/service-scan emission
    /// (view.rs): when the last burst was emitted for this terminal.
    pub(super) last_activity_burst: Option<std::time::Instant>,
    /// EP-002 US-007: throttle counter for the proc-based CWD refresh in
    /// `sync_channels` (the OSC 7 byte-scanner was removed with the 2-thread
    /// reader; the EventLoop owns the read path with no pre-parse hook).
    cwd_poll_ticks: u32,
    /// Ports already reported via ServiceDetected (dedup guard).
    /// Cleared on ChildExit so a restarted server is re-detected.
    /// U-052: a `HashSet` bounds membership to O(1) and the structure to a
    /// flat per-distinct-port cost, vs. the old `Vec` whose linear `.contains`
    /// and unbounded growth scaled with every detected service.
    reported_ports: std::collections::HashSet<u16>,
    /// Timestamp of the most recent keystroke, used by latency probes
    /// to measure total keystroke-to-pixel time. Debug builds only.
    /// Note: on rapid keystrokes before a render frame, earlier timestamps are overwritten.
    #[cfg(debug_assertions)]
    pub(crate) last_keystroke_at: Option<std::time::Instant>,
    /// GPUI background executor used by `Drop` to schedule the
    /// grace-period force-kill task. Wired by `TerminalView::with_cwd`
    /// immediately after construction. `None` only on display-only /
    /// test paths, where `Drop` falls back to a detached OS thread.
    /// Mirrors Zed `crates/terminal/src/terminal.rs:2451-2457` which
    /// uses `background_executor.spawn(...).detach()` to keep the
    /// kill timer under the GPUI scheduler instead of leaking an
    /// orphan OS thread per closed pane.
    background_executor: Option<gpui::BackgroundExecutor>,
    /// US-012: input written through `write_to_pty` while the terminal is
    /// still display-only (the PTY opens on a background thread and is
    /// installed later by [`promote`](Self::promote)). The display-only
    /// notifier silently drops every write, so without this queue an
    /// auto-launch command issued the instant a terminal mounts (the
    /// launch pad's agent picker) - or a keystroke typed in the brief
    /// pre-promotion window - would be lost. [`promote`](Self::promote)
    /// flushes it in order. `Mutex` (not `RefCell`) keeps `TerminalState`
    /// `Send` and matches the crate's interior-mutability idiom; the lock is
    /// uncontended (main thread only).
    pending_input: std::sync::Mutex<VecDeque<PendingTerminalInput>>,
}

/// Cap on input buffered during the pre-promotion window. Generous for a
/// launch command plus a burst of typing, tight enough that a terminal that
/// never promotes (spawn failure - `promote` is never called) cannot
/// accumulate input without bound.
const MAX_PENDING_INPUT_BYTES: usize = 1024 * 1024;
#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
const INPUT_CONTROL_RESERVE_BYTES: usize = 64 * 1024;

/// The cheap, render-thread-safe half of a spawn: resolved shell, assembled
/// child env, cwd, and grid size. Produced by
/// [`TerminalState::resolve_spawn_params`] and consumed by
/// [`TerminalState::open_pty_and_eventloop`] (which may run on a background
/// thread). All fields are `Send`.
#[derive(Clone)]
pub(super) struct SpawnParams {
    pub(super) shell: String,
    pub(super) shell_quoting: ShellQuoting,
    pub(super) extra_args: Vec<String>,
    pub(super) env: std::collections::HashMap<String, String>,
    pub(super) cwd: std::path::PathBuf,
    pub(super) cols: usize,
    pub(super) rows: usize,
    pub(super) profile: TerminalSurfaceProfile,
    pub(super) surface_id: u64,
}

/// The live PTY handles produced by [`TerminalState::open_pty_and_eventloop`]:
/// the `EventLoop` write channel, the child PID, the resolved launch cwd, and
/// (macOS) the master fd. Crosses the background→main boundary to
/// [`TerminalState::promote`]; all fields are `Send`.
pub(super) struct SpawnedPty {
    channel: EventLoopSender,
    child_pid: u32,
    /// The directory the shell was spawned in. Seeds [`TerminalState::current_cwd`]
    /// at promotion so the sessions sidebar can resolve a project before the
    /// first `cwd_now()` poll - and at all on Windows, where `cwd_now()` is a
    /// stub.
    cwd: std::path::PathBuf,
    cwd_rx: UnboundedReceiver<String>,
    marks_rx: std::sync::mpsc::Receiver<RawMark>,
    #[cfg(all(unix, not(test)))]
    pty_guard: Option<crate::agents::parent_guard::PtyGuardHandle>,
    #[cfg(target_os = "macos")]
    pty_master_fd: Option<i32>,
}

const OSC7_MAX_PAYLOAD: usize = 4096;
// VTE's std parser uses an unbounded Vec for OSC bytes. Hold each OSC before
// that parser and drop it once it exceeds the largest valid OSC 52 payload
// plus protocol overhead. This bounds every OSC family, not only clipboard.
const MAX_OSC_SEQUENCE_BYTES: usize = MAX_OSC52_BYTES.div_ceil(3) * 4 + 64;

#[derive(Debug, Default)]
enum BoundedOscState {
    #[default]
    Ground,
    Esc,
    Collect(Vec<u8>),
    Drop,
}

#[derive(Debug, Default)]
struct BoundedOscFilter {
    state: BoundedOscState,
    pending: VecDeque<u8>,
}

impl BoundedOscFilter {
    fn advance(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let state = std::mem::take(&mut self.state);
            self.state = match state {
                BoundedOscState::Ground if byte == 0x1b => BoundedOscState::Esc,
                BoundedOscState::Ground => {
                    self.pending.push_back(byte);
                    BoundedOscState::Ground
                }
                BoundedOscState::Esc if byte == b']' => BoundedOscState::Collect(vec![0x1b, b']']),
                BoundedOscState::Esc if byte == 0x1b => {
                    self.pending.push_back(0x1b);
                    BoundedOscState::Esc
                }
                BoundedOscState::Esc => {
                    self.pending.extend([0x1b, byte]);
                    BoundedOscState::Ground
                }
                BoundedOscState::Collect(buffer) if byte == 0x1b => {
                    self.pending.extend(buffer);
                    BoundedOscState::Esc
                }
                BoundedOscState::Collect(mut buffer) => {
                    buffer.push(byte);
                    if buffer.len() > MAX_OSC_SEQUENCE_BYTES {
                        BoundedOscState::Drop
                    // vte 0.15 treats raw C1 ST (0x9c) as OSC payload.
                    // Only BEL, CAN/SUB, or the ESC-based ST can end it.
                    } else if matches!(byte, 0x07 | 0x18 | 0x1a) {
                        self.pending.extend(buffer);
                        BoundedOscState::Ground
                    } else {
                        BoundedOscState::Collect(buffer)
                    }
                }
                BoundedOscState::Drop if byte == 0x1b => {
                    self.pending.push_back(0x18);
                    BoundedOscState::Esc
                }
                BoundedOscState::Drop if matches!(byte, 0x07 | 0x18 | 0x1a) => {
                    self.pending.push_back(0x18);
                    BoundedOscState::Ground
                }
                BoundedOscState::Drop => BoundedOscState::Drop,
            };
        }
    }

    fn drain_into(&mut self, output: &mut [u8]) -> usize {
        let mut written = 0;
        while written < output.len() {
            let Some(byte) = self.pending.pop_front() else {
                break;
            };
            output[written] = byte;
            written += 1;
        }
        written
    }
}

#[derive(Debug, Default)]
enum Osc7ScanState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
    Discard,
    DiscardEscape,
}

#[derive(Debug, Default)]
struct Osc7Scanner {
    state: Osc7ScanState,
    payload: Vec<u8>,
}

impl Osc7Scanner {
    fn advance<F>(&mut self, bytes: &[u8], mut emit: F)
    where
        F: FnMut(String),
    {
        for &byte in bytes {
            match self.state {
                Osc7ScanState::Ground => {
                    if byte == 0x1b {
                        self.state = Osc7ScanState::Esc;
                    }
                }
                Osc7ScanState::Esc => {
                    if byte == b']' {
                        self.payload.clear();
                        self.state = Osc7ScanState::Osc;
                    } else if byte != 0x1b {
                        self.state = Osc7ScanState::Ground;
                    }
                }
                Osc7ScanState::Osc => match byte {
                    0x07 => self.finish(&mut emit),
                    0x18 | 0x1a => self.reset(),
                    0x1b => self.state = Osc7ScanState::OscEsc,
                    _ => {
                        self.push_payload_byte(byte);
                    }
                },
                Osc7ScanState::OscEsc => match byte {
                    b'\\' => self.finish(&mut emit),
                    0x18 | 0x1a => self.reset(),
                    _ => {
                        if self.push_payload_byte(0x1b) {
                            if byte == 0x1b {
                                self.state = Osc7ScanState::OscEsc;
                            } else if self.push_payload_byte(byte) {
                                self.state = Osc7ScanState::Osc;
                            }
                        }
                    }
                },
                Osc7ScanState::Discard => match byte {
                    0x07 | 0x18 | 0x1a => self.reset(),
                    0x1b => self.state = Osc7ScanState::DiscardEscape,
                    _ => {}
                },
                Osc7ScanState::DiscardEscape => match byte {
                    b'\\' | 0x07 | 0x18 | 0x1a => self.reset(),
                    0x1b => {}
                    _ => self.state = Osc7ScanState::Discard,
                },
            }
        }
    }

    fn push_payload_byte(&mut self, byte: u8) -> bool {
        if self.payload.len() < OSC7_MAX_PAYLOAD {
            self.payload.push(byte);
            true
        } else {
            self.payload.clear();
            self.state = Osc7ScanState::Discard;
            false
        }
    }

    fn finish<F>(&mut self, emit: &mut F)
    where
        F: FnMut(String),
    {
        if let Ok(payload) = std::str::from_utf8(&self.payload)
            && let Some(cwd) = cwd_from_osc7_payload(payload)
        {
            emit(cwd);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.state = Osc7ScanState::Ground;
        self.payload.clear();
    }
}

struct Osc7Pty<T: tty::EventedPty> {
    inner: T,
    osc_filter: BoundedOscFilter,
    scanner: Osc7Scanner,
    marks_scanner: Osc133Scanner,
    cwd_tx: UnboundedSender<String>,
    marks_tx: std::sync::mpsc::SyncSender<RawMark>,
}

impl<T: tty::EventedPty> Osc7Pty<T> {
    fn new(
        inner: T,
        cwd_tx: UnboundedSender<String>,
        marks_tx: std::sync::mpsc::SyncSender<RawMark>,
    ) -> Self {
        Self {
            inner,
            osc_filter: BoundedOscFilter::default(),
            scanner: Osc7Scanner::default(),
            marks_scanner: Osc133Scanner::default(),
            cwd_tx,
            marks_tx,
        }
    }
}

impl<T: tty::EventedPty> Read for Osc7Pty<T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let pending = self.osc_filter.drain_into(buf);
        if pending > 0 || buf.is_empty() {
            return Ok(pending);
        }

        loop {
            let read = self.inner.reader().read(buf)?;
            if read == 0 {
                return Ok(0);
            }
            let cwd_tx = self.cwd_tx.clone();
            self.scanner.advance(&buf[..read], |cwd| {
                let _ = cwd_tx.unbounded_send(cwd);
            });
            let marks_tx = &self.marks_tx;
            self.marks_scanner.feed(&buf[..read], &mut |mark| {
                let _ = marks_tx.try_send(mark);
            });
            self.osc_filter.advance(&buf[..read]);
            let filtered = self.osc_filter.drain_into(buf);
            if filtered > 0 {
                return Ok(filtered);
            }
        }
    }
}

impl<T: tty::EventedPty> Write for Osc7Pty<T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.writer().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.writer().flush()
    }
}

impl<T: tty::EventedPty> tty::EventedReadWrite for Osc7Pty<T> {
    type Reader = Self;
    type Writer = Self;

    unsafe fn register(
        &mut self,
        poller: &Arc<polling::Poller>,
        event: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        unsafe { self.inner.register(poller, event, mode) }
    }

    fn reregister(
        &mut self,
        poller: &Arc<polling::Poller>,
        event: polling::Event,
        mode: polling::PollMode,
    ) -> io::Result<()> {
        self.inner.reregister(poller, event, mode)
    }

    fn deregister(&mut self, poller: &Arc<polling::Poller>) -> io::Result<()> {
        self.inner.deregister(poller)
    }

    fn reader(&mut self) -> &mut Self::Reader {
        self
    }

    fn writer(&mut self) -> &mut Self::Writer {
        self
    }
}

impl<T: tty::EventedPty> tty::EventedPty for Osc7Pty<T> {
    fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
        self.inner.next_child_event()
    }
}

impl<T> alacritty_terminal::event::OnResize for Osc7Pty<T>
where
    T: tty::EventedPty + alacritty_terminal::event::OnResize,
{
    fn on_resize(&mut self, window_size: AlacWindowSize) {
        self.inner.on_resize(window_size);
    }
}

fn cwd_from_osc7_payload(payload: &str) -> Option<String> {
    let rest = payload.strip_prefix("7;file://")?;
    let path = if rest.starts_with('/') {
        Cow::Borrowed(rest)
    } else {
        let (_, path) = rest.split_once('/')?;
        Cow::Owned(format!("/{path}"))
    };
    let decoded = percent_decode_uri_path(&path)?;
    #[cfg(windows)]
    if let Some(msys_path) = msys_path_to_windows_path(&decoded) {
        return Some(msys_path);
    }
    #[cfg(windows)]
    if decoded.len() >= 3
        && decoded.as_bytes()[0] == b'/'
        && decoded.as_bytes()[1].is_ascii_alphabetic()
        && decoded.as_bytes()[2] == b':'
    {
        return Some(decoded[1..].replace('/', "\\"));
    }
    Some(decoded)
}

#[cfg(windows)]
fn msys_path_to_windows_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    if bytes.len() < 2
        || bytes[0] != b'/'
        || !bytes[1].is_ascii_alphabetic()
        || (bytes.len() > 2 && bytes[2] != b'/')
    {
        return None;
    }

    let drive = (bytes[1] as char).to_ascii_uppercase();
    if bytes.len() == 2 {
        Some(format!("{drive}:\\"))
    } else {
        Some(format!("{drive}:\\{}", path[3..].replace('/', "\\")))
    }
}

fn percent_decode_uri_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            match (
                bytes.get(i + 1).copied().and_then(hex_value),
                bytes.get(i + 2).copied().and_then(hex_value),
            ) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Foreground (main-thread) signal mask, captured so an off-thread PTY spawn
/// (US-012) doesn't hand the child the background executor's mask (which blocks
/// SIGINT/SIGTSTP and would break Ctrl-C / Ctrl-Z). Unix-only - a ZST on
/// Windows.
#[cfg(unix)]
pub type ForegroundSignalMask = libc::sigset_t;
#[cfg(not(unix))]
pub type ForegroundSignalMask = ();

/// Capture the calling thread's signal mask. Call on the main thread before
/// scheduling an off-thread spawn; thread the result through to
/// [`TerminalState::open_pty_and_eventloop`].
pub(super) fn capture_foreground_signal_mask() -> Option<ForegroundSignalMask> {
    #[cfg(unix)]
    {
        // SAFETY: `pthread_sigmask` with a null `set` only reads the current
        // mask into `oldset`; nothing is changed.
        unsafe {
            let mut oldset: libc::sigset_t = std::mem::zeroed();
            if libc::pthread_sigmask(libc::SIG_SETMASK, std::ptr::null(), &mut oldset) == 0 {
                Some(oldset)
            } else {
                None
            }
        }
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Install `mask` on the current thread, returning the previous mask to restore.
/// Brackets the `tty::new` fork so the child inherits the foreground signal
/// disposition even when the spawn runs on a background thread (US-012).
#[cfg(unix)]
pub(super) fn apply_thread_signal_mask(
    mask: Option<ForegroundSignalMask>,
) -> Option<libc::sigset_t> {
    let fg = mask?;
    // SAFETY: set this thread's mask to the captured foreground mask, saving the
    // previous one into `saved` for `restore_thread_signal_mask`.
    unsafe {
        let mut saved: libc::sigset_t = std::mem::zeroed();
        if libc::pthread_sigmask(libc::SIG_SETMASK, &fg, &mut saved) == 0 {
            Some(saved)
        } else {
            None
        }
    }
}

/// Restore a thread mask saved by [`apply_thread_signal_mask`].
#[cfg(unix)]
pub(super) fn restore_thread_signal_mask(saved: Option<libc::sigset_t>) {
    if let Some(saved) = saved {
        // SAFETY: restore the previously-saved mask on this thread.
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &saved, std::ptr::null_mut());
        }
    }
}

impl TerminalState {
    pub(crate) fn session_backend(&self) -> TerminalSessionBackend {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return TerminalSessionBackend::ghostty(
                self.term.clone(),
                PtyNotifier(self.notifier.0.clone()),
                ghostty.clone(),
            );
        }
        TerminalSessionBackend::alacritty(self.term.clone(), PtyNotifier(self.notifier.0.clone()))
    }

    pub(crate) fn take_backend_events(&mut self) -> TerminalBackendEvents {
        let alacritty = self.events_rx.take();
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        let ghostty = self.ghostty_events_rx.take();
        TerminalBackendEvents {
            alacritty,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty,
        }
    }

    pub(crate) fn process_backend_event(&mut self, event: TerminalBackendEvent) {
        match event {
            TerminalBackendEvent::Alacritty(event) => self.process_event(event),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            TerminalBackendEvent::Ghostty(event) => self.process_ghostty_event(event),
        }
    }

    pub(crate) fn process_backend_wakeup(&mut self) {
        self.dirty = true;
        self.output_generation = self.output_generation.saturating_add(1);
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        {
            self.flush_ghostty_pending_input();
            if let Some(ghostty) = &self.ghostty {
                ghostty.retry_backpressured_commands();
            }
        }
    }

    pub(crate) fn notify_window_size(&self, size: TerminalWindowSize) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            ghostty.resize(size);
            return;
        }
        self.notifier.notify_window_size(size);
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn attach_ghostty(
        &mut self,
        ghostty: GhosttySession,
        events_rx: UnboundedReceiver<GhosttyUiEvent>,
    ) {
        self.effective_backend = "ghostty";
        self.marks = ghostty.marks();
        self.ghostty = Some(ghostty);
        self.ghostty_events_rx = Some(events_rx);
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn promote_ghostty(&mut self, spawned: SpawnedGhostty) {
        let Some(ghostty) = self.ghostty.as_ref() else {
            return;
        };
        ghostty.promote();
        self.child_pid = spawned.child_pid;
        self.current_cwd = Some(spawned.cwd.to_string_lossy().into_owned());
        #[cfg(all(unix, not(test)))]
        {
            self.pty_guard = crate::agents::parent_guard::spawn_pty_guard(spawned.child_pid);
        }
        self.set_osc52_mode(Osc52Mode::CopyOnly);
        self.cursor_blinking = true;
        self.dirty = true;
        self.flush_ghostty_pending_input();
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn flush_ghostty_pending_input(&self) {
        let Some(ghostty) = self
            .ghostty
            .as_ref()
            .filter(|session| session.is_promoted())
        else {
            return;
        };
        let Ok(mut pending) = self.pending_input.lock() else {
            return;
        };
        while let Some(input) = pending.front().cloned() {
            match input.try_send(ghostty) {
                GhosttyInputSendResult::Sent => {
                    pending.pop_front();
                }
                GhosttyInputSendResult::Full => break,
                GhosttyInputSendResult::Closed => {
                    let discarded = pending.len();
                    pending.clear();
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty input closed with {discarded} deferred events"
                    );
                    break;
                }
            }
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn fallback_from_ghostty(&mut self, failure: TerminalBackendFailureDiagnostics) {
        if let Some(ghostty) = self.ghostty.take() {
            ghostty.shutdown();
        }
        self.effective_backend = "alacritty";
        self.backend_failure = Some(failure);
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn fail_ghostty_after_spawn(&mut self, failure: TerminalBackendFailureDiagnostics) {
        if let Some(ghostty) = self.ghostty.take() {
            ghostty.shutdown();
        }
        self.backend_failure = Some(failure);
    }

    pub(super) fn set_backend_request(&mut self, requested: TerminalBackendConfig) {
        self.requested_backend = requested;
        self.backend_failure = None;
    }

    // Used by the cfg branch compiled when no native Ghostty backend is present.
    #[allow(dead_code)]
    pub(super) fn record_backend_failure(&mut self, failure: TerminalBackendFailureDiagnostics) {
        self.backend_failure = Some(failure);
    }

    pub fn backend_diagnostics(&self) -> TerminalBackendDiagnostics {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        let ghostty = {
            let identity = paneflow_terminal_ghostty::build_identity();
            Some(GhosttyBuildDiagnostics {
                version: paneflow_terminal_ghostty::GHOSTTY_APP_VERSION,
                source_sha: identity.source_sha,
                api_version: identity.api_version,
                zig_version: identity.zig_version,
                optimization: identity.optimization,
                simd: identity.simd,
            })
        };
        #[cfg(not(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        )))]
        let ghostty = None;

        TerminalBackendDiagnostics {
            requested: self.requested_backend,
            effective: self.effective_backend,
            failure: self.backend_failure.clone(),
            target_triple: env!("PANEFLOW_TARGET_TRIPLE"),
            ghostty,
        }
    }

    pub(crate) fn drain_size_responses(&mut self, size: TerminalWindowSize) -> Vec<String> {
        if self.pending_size_ops.len() > 8 {
            let drop_count = self.pending_size_ops.len() - 8;
            self.pending_size_ops.drain(..drop_count);
        }
        let alacritty_size = alacritty_window_size(size);
        self.pending_size_ops
            .drain(..)
            .map(|format| format(alacritty_size))
            .collect()
    }

    /// Spawn a real PTY-backed terminal synchronously. Resolves the shell + env
    /// ([`resolve_spawn_params`]), builds a display-only `Term`
    /// ([`new_pending`]), opens the PTY ([`open_pty_and_eventloop`]), and
    /// promotes it to a live `Pty` ([`promote`]). The off-thread path
    /// (`TerminalView::with_cwd_and_env`, US-012) runs the same four steps but
    /// spreads the blocking one across the background executor with a
    /// `signal_mask` so the render thread never blocks on the spawn.
    ///
    /// `signal_mask` is `None` on the synchronous main-thread path (the
    /// foreground mask is already active); the off-thread path passes the
    /// captured foreground mask so the child still gets correct Ctrl-C.
    ///
    /// The production GUI path spawns off-thread (`with_cwd_and_env` →
    /// `new_pending` + `open_pty_and_eventloop` + `promote`); this synchronous
    /// composition is the reference path, exercised end-to-end by the live
    /// `eventloop_pty_echoes_input_into_grid` smoke and available to any future
    /// non-GUI (headless) caller.
    #[allow(dead_code)]
    pub fn new(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        signal_mask: Option<ForegroundSignalMask>,
    ) -> anyhow::Result<Self> {
        Self::new_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
            signal_mask,
        )
    }

    #[allow(dead_code)]
    pub fn new_with_profile(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
        signal_mask: Option<ForegroundSignalMask>,
    ) -> anyhow::Result<Self> {
        let params = Self::resolve_spawn_params_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            profile,
        );
        let (mut state, pending) = Self::new_pending_with_profile_and_shell_quoting(
            params.cols,
            params.rows,
            params.profile,
            params.shell_quoting,
        );
        let spawned = Self::open_pty_and_eventloop(params, pending, signal_mask)?;
        state.promote(spawned);
        Ok(state)
    }

    /// Resolve the shell, the merged + assembled child env, the cwd, and the
    /// grid size - the cheap, render-thread-safe half of a spawn. Factored out
    /// of `new` so the off-thread path (US-012) runs the *blocking* half
    /// ([`open_pty_and_eventloop`]) on the background executor.
    #[allow(dead_code)]
    pub(super) fn resolve_spawn_params(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
    ) -> SpawnParams {
        Self::resolve_spawn_params_with_profile(
            working_directory,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
        )
    }

    pub(super) fn resolve_spawn_params_with_profile(
        working_directory: Option<std::path::PathBuf>,
        workspace_id: u64,
        surface_id: u64,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
    ) -> SpawnParams {
        // Fallback chain handled by `resolve_default_shell` (US-006):
        // Unix:    config → $SHELL → /bin/sh
        // Windows: config → pwsh.exe → powershell.exe → %ComSpec% →
        //          C:\Windows\System32\cmd.exe → bare "cmd.exe"
        //          (PowerShell preferred so we don't default to the legacy
        //          cmd.exe console - mirrors Zed's get_windows_system_shell)
        let config = paneflow_config::loader::load_config();
        let shell = {
            let configured = config
                .default_shell
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            resolve_default_shell(configured)
        };
        let shell_quoting = ShellQuoting::for_shell(&shell);
        // US-014: layer the per-surface `user_env` on top of the global
        // `terminal.env` default (surface wins on key collision).
        let global_env = config.terminal.as_ref().and_then(|t| t.env.clone());
        let merged_env = match (global_env, user_env) {
            (None, None) => None,
            (Some(g), None) => Some(g),
            (None, Some(s)) => Some(s),
            (Some(mut g), Some(s)) => {
                g.extend(s);
                Some(g)
            }
        };
        let mut env = std::collections::HashMap::new();
        // EP-003 US-007: clean opt-out - with `shell_integration: false` no
        // rc snippet is written or wired and the shell starts untouched.
        let extra_args = if config.shell_integration.unwrap_or(true) {
            setup_shell_integration(&shell, &mut env, profile)
        } else {
            vec![]
        };
        // Assemble the child environment (identity vars, TERM, AI-hook PATH
        // prepend, user-env merge with protected keys). Pure function so the env
        // contract stays unit-testable (the mockable `PtyBackend::spawn` seam is
        // gone - EP-002 US-004).
        let mut env = assemble_pty_env(env, workspace_id, surface_id, merged_env);
        // Keep terminal.env and identity propagation independent from shell
        // integration: opting out disables rc hooks, not the terminal env contract.
        if is_wsl_shell(&shell) {
            augment_wslenv(&mut env);
        }
        // U-026 + issue #11: when no cwd is explicit, avoid inheriting a GUI
        // launch cwd that is the filesystem root. Explicit root cwd requests
        // still arrive through `working_directory` and are preserved.
        let cwd = working_directory.unwrap_or_else(crate::launch_cwd::implicit_launch_cwd);
        let (cols, rows) = initial_size.unwrap_or((120, 40));
        SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env,
            cwd,
            cols,
            rows,
            profile,
            surface_id,
        }
    }

    /// Build a display-only terminal that retains its event-channel *sender* so
    /// a background spawn can later attach a real `EventLoop` to the same
    /// channel and [`promote`](Self::promote) it (US-012). The returned
    /// opaque pending handle is handed to [`open_pty_and_eventloop`].
    #[allow(dead_code)]
    pub(super) fn new_pending(cols: usize, rows: usize) -> (Self, PendingTerminalBackend) {
        Self::new_pending_with_profile(cols, rows, TerminalSurfaceProfile::Normal)
    }

    pub(super) fn new_pending_with_profile(
        cols: usize,
        rows: usize,
        profile: TerminalSurfaceProfile,
    ) -> (Self, PendingTerminalBackend) {
        Self::new_pending_with_profile_and_shell_quoting(
            cols,
            rows,
            profile,
            ShellQuoting::default_for_platform(),
        )
    }

    pub(super) fn new_pending_with_profile_and_shell_quoting(
        cols: usize,
        rows: usize,
        profile: TerminalSurfaceProfile,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        Self::build_display_only(cols, rows, profile, shell_quoting)
    }

    /// Open the PTY and start its `EventLoop` on the given (shared) `term` and
    /// event channel - the *blocking* half of a spawn (`tty::new` forks). Safe
    /// to call on a background thread: when `signal_mask` is `Some`, it is
    /// installed on this thread around the `tty::new` fork so the child inherits
    /// the foreground (main-thread) signal disposition and Ctrl-C / Ctrl-Z keep
    /// working, then the thread's mask is restored. Upstream `alacritty_terminal`
    /// exposes no `child_signal_mask` pty option (Zed's #58004 is a fork
    /// addition), so bracketing the fork with the thread mask is the
    /// upstream-only equivalent.
    pub(super) fn open_pty_and_eventloop(
        params: SpawnParams,
        pending: PendingTerminalBackend,
        signal_mask: Option<ForegroundSignalMask>,
    ) -> anyhow::Result<SpawnedPty> {
        let PendingTerminalBackend {
            term,
            events_tx,
            clipboard_gate,
        } = pending;
        let listener = ZedListener::with_clipboard_gate(events_tx, clipboard_gate);
        let (cwd_tx, cwd_rx) = unbounded();
        let (marks_tx, marks_rx) = std::sync::mpsc::sync_channel(256);
        // Pixel size unknown at spawn (apps use the char grid); the live size is
        // pushed via `Msg::Resize` on the first frame.
        let window_size = AlacWindowSize {
            num_cols: params.cols as u16,
            num_lines: params.rows as u16,
            cell_width: 0,
            cell_height: 0,
        };
        // Keep the resolved launch cwd to seed `current_cwd` at promotion;
        // `params.cwd` is moved into `working_directory` just below.
        let launch_cwd = params.cwd.clone();
        let options = tty::Options {
            shell: Some(tty::Shell::new(params.shell, params.extra_args)),
            working_directory: Some(params.cwd),
            // Keep reading after child exit so a shell's final burst reaches
            // the grid before the exit overlay lands. Mirrors Zed's terminal
            // path and must match the EventLoop flag below.
            drain_on_exit: PTY_DRAIN_ON_EXIT,
            env: params.env,
            #[cfg(windows)]
            escape_args: true,
        };

        // EP-002 US-004: open the PTY via alacritty's own cross-platform `tty`
        // (Unix openpty + setsid, Windows ConPTY) and drive it with alacritty's
        // `EventLoop`. Mirrors Zed `crates/terminal/src/alacritty.rs`.
        //
        // US-012: bracket the fork with the captured foreground signal mask so
        // an off-thread spawn doesn't hand the child the background executor's
        // signal-blocking mask. No-op on the synchronous path (`signal_mask` is
        // `None`) and on Windows.
        #[cfg(unix)]
        let restore_mask = apply_thread_signal_mask(signal_mask);
        #[cfg(not(unix))]
        let _ = signal_mask;

        let pty = tty::new(&options, window_size, params.surface_id);

        #[cfg(unix)]
        restore_thread_signal_mask(restore_mask);

        let pty = pty.map_err(|e| anyhow::anyhow!("failed to open pty: {e}"))?;

        // Capture the child PID (teardown ladder + port detection) and, on
        // macOS, the PTY master fd (`tcgetpgrp` foreground naming) BEFORE the
        // EventLoop consumes the `Pty`. alacritty `pre_exec`s `setsid()`, so the
        // child is its own session/group leader and `child_pid` is also the PGID
        // (the `kill(-pid, …)` group teardown in `Drop` stays valid). Mirrors Zed
        // `ProcessIdGetter::from(&AlacrittyPty)`.
        #[cfg(unix)]
        let child_pid = pty.child().id();
        #[cfg(windows)]
        let child_pid = pty.child_watcher().pid().map(u32::from).unwrap_or(0);
        #[cfg(all(unix, not(test)))]
        let pty_guard = crate::agents::parent_guard::spawn_pty_guard(child_pid);
        #[cfg(target_os = "macos")]
        let pty_master_fd = {
            use std::os::unix::io::AsRawFd;
            // US-034: `dup()` the master fd so we own a copy whose lifetime we
            // control (closed in `Drop`). The borrowed `pty.file().as_raw_fd()`
            // is closed when the EventLoop (which takes ownership of `pty`
            // below) tears the PTY down on child exit, and the OS may reuse
            // that fd number - `tcgetpgrp(stale_fd)` would then report an
            // unrelated process group, defeating the `p > 0` filter.
            let raw = pty.file().as_raw_fd();
            // SAFETY: `raw` is a valid open fd for the PTY master; `dup`
            // returns a fresh owned fd or -1 on error (filtered out).
            let dup = unsafe { libc::dup(raw) };
            (dup >= 0).then_some(dup)
        };

        let pty = Osc7Pty::new(pty, cwd_tx, marks_tx);
        let event_loop = EventLoop::new(
            term,
            listener,
            pty,
            PTY_DRAIN_ON_EXIT, // drain_on_exit
            false,             // ref_test
        )
        .map_err(|e| anyhow::anyhow!("failed to start pty event loop: {e}"))?;
        let channel = event_loop.channel();
        // The IO thread runs detached; shutdown is driven by `Msg::Shutdown` in
        // `Drop`. The handle is dropped (the thread joins itself on shutdown).
        let _io_thread = event_loop.spawn();

        Ok(SpawnedPty {
            channel,
            child_pid,
            cwd: launch_cwd,
            cwd_rx,
            marks_rx,
            #[cfg(all(unix, not(test)))]
            pty_guard,
            #[cfg(target_os = "macos")]
            pty_master_fd,
        })
    }

    /// Promote a display-only / pending terminal to a live PTY by installing the
    /// `EventLoop` write channel, child PID, and interactive defaults produced
    /// by [`open_pty_and_eventloop`]. The grid `Term` is unchanged - the
    /// background `EventLoop` was attached to the same shared `term`, so output
    /// already flows; this just opens the write side and lets `Drop` reach the
    /// child.
    pub(super) fn promote(&mut self, spawned: SpawnedPty) {
        let sender = PtySender::pty(spawned.channel);
        self.notifier = PtyNotifier(sender);
        self.cwd_rx = Some(spawned.cwd_rx);
        self.marks_rx = Some(spawned.marks_rx);
        self.child_pid = spawned.child_pid;
        #[cfg(all(unix, not(test)))]
        {
            self.pty_guard = spawned.pty_guard;
        }
        // Seed the working directory from the launch cwd. On Unix `sync_channels`
        // refines this to the live shell cwd within a few poll ticks via
        // `cwd_now()` (/proc, libproc); on Windows `cwd_now()` is a stub, so this
        // launch-dir seed is the ONLY source of `current_cwd` - without it the
        // value stayed `None` and the agent-sessions sidebar, which scans the
        // active terminal's cwd, had nothing to resolve and rendered empty.
        if self.current_cwd.is_none() {
            self.current_cwd = Some(spawned.cwd.to_string_lossy().into_owned());
        }
        #[cfg(target_os = "macos")]
        {
            self.pty_master_fd = spawned.pty_master_fd;
        }
        // Interactive defaults (a display-only terminal had these off).
        self.set_osc52_mode(Osc52Mode::CopyOnly);
        self.cursor_blinking = true;
        self.dirty = true;
        // Flush input queued while display-only (US-012): the launch command
        // an auto-launched agent issues the instant it mounts, plus any
        // keystrokes typed before the off-thread fork resolved. Order is
        // preserved; the now-live `Pty` notifier delivers each to the child.
        for input in self.drain_pending_legacy_input() {
            self.notifier.notify(input);
        }
    }

    fn drain_pending_legacy_input(&self) -> Vec<Cow<'static, [u8]>> {
        let Ok(mut pending) = self.pending_input.lock() else {
            return Vec::new();
        };
        pending
            .drain(..)
            .filter_map(PendingTerminalInput::into_legacy_bytes)
            .collect()
    }

    /// Wire a GPUI background executor for the grace-period force-kill
    /// task spawned in `Drop`. Without this, the kill timer runs on a
    /// detached OS thread (works, but leaks one thread per closed pane
    /// on intensive use). Called by `TerminalView::with_cwd` so the
    /// production path always goes through GPUI's scheduler.
    pub fn set_background_executor(&mut self, executor: gpui::BackgroundExecutor) {
        self.background_executor = Some(executor);
    }

    /// Create a display-only terminal with no PTY, no reader thread, no message loop.
    /// Content is rendered via `write_output()` which processes bytes through VTE directly.
    /// The terminal supports full ANSI rendering but does not accept keyboard input.
    /// Used by tests (the production spawn-failure fallback keeps the
    /// already-built pending placeholder and writes the error into it).
    #[allow(dead_code)]
    pub fn new_display_only(rows: usize, cols: usize) -> Self {
        Self::new_display_only_with_profile(rows, cols, TerminalSurfaceProfile::Normal)
    }

    #[allow(dead_code)]
    pub fn new_display_only_with_profile(
        rows: usize,
        cols: usize,
        profile: TerminalSurfaceProfile,
    ) -> Self {
        Self::build_display_only(cols, rows, profile, ShellQuoting::default_for_platform()).0
    }

    /// Shared constructor for the display-only / pending state. Returns the
    /// terminal plus a clone of its event-channel *sender*, so the off-thread
    /// spawn path ([`new_pending`]) can wire a real `EventLoop` to the same
    /// channel and [`promote`](Self::promote) it (US-012). `new_display_only`
    /// discards the sender (its `Term` only emits Wakeups on its own VTE writes).
    fn build_display_only(
        cols: usize,
        rows: usize,
        profile: TerminalSurfaceProfile,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        let (events_tx, events_rx) = unbounded();
        let clipboard_gate = Arc::new(ClipboardGate::default());
        // The Term keeps one clone (emits Wakeup after VTE mutations); the
        // returned clone is for a later `EventLoop` on promotion.
        let listener = ZedListener::with_clipboard_gate(events_tx.clone(), clipboard_gate.clone());

        let config = TermConfig {
            scrolling_history: resolved_scrollback_lines(profile),
            default_cursor_style: resolved_cursor_style(),
            ..TermConfig::default()
        };
        let dimensions = SpikeTermSize {
            columns: cols,
            screen_lines: rows,
        };
        let term = Term::new(config, &dimensions, listener);
        let term = Arc::new(FairMutex::new(term));
        let notifier_sender = PtySender::display_only();

        let pending = PendingTerminalBackend {
            term: term.clone(),
            events_tx,
            clipboard_gate: clipboard_gate.clone(),
        };
        let state = Self {
            term,
            // No PTY / EventLoop yet - notifier sends are silently dropped until
            // `promote()` installs a `Pty` sender.
            notifier: PtyNotifier(notifier_sender),
            events_rx: Some(events_rx),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty: None,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty_events_rx: None,
            requested_backend: TerminalBackendConfig::Auto,
            effective_backend: "alacritty",
            backend_failure: None,
            cwd_rx: None,
            marks_rx: None,
            marks: Arc::new(std::sync::Mutex::new(Default::default())),
            exited: None,
            keyboard_input_sent: std::sync::atomic::AtomicBool::new(false),
            exit_signal: None,
            child_pid: 0,
            #[cfg(target_os = "macos")]
            pty_master_fd: None,
            current_cwd: None,
            custom_name: None,
            detected_agent: None,
            agent_confirmed: false,
            agent_declared_until: None,
            detected_ports: Vec::new(),
            port_conflicts: Vec::new(),
            announced_ports: Vec::new(),
            font_size_override: None,
            cursor_color_override: resolved_cursor_color_override(),
            osc52_mode: Osc52Mode::Disabled,
            terminal_focused: false,
            clipboard_gate,
            shell_quoting,
            pending_clipboard_ops: Vec::new(),
            cached_foreground_command: None,
            #[cfg(all(unix, not(test)))]
            pty_guard: None,
            pending_size_ops: Vec::new(),
            cursor_blinking: false,
            title: String::from("Terminal"),
            dirty: true,
            output_generation: 0,
            last_activity_burst: None,
            cwd_poll_ticks: 0,
            reported_ports: std::collections::HashSet::new(),
            #[cfg(debug_assertions)]
            last_keystroke_at: None,
            background_executor: None,
            pending_input: std::sync::Mutex::new(VecDeque::new()),
        };
        (state, pending)
    }

    /// Write ANSI-formatted content to a display-only terminal.
    /// Converts bare `\n` to `\r\n` (since there is no PTY to perform CR insertion).
    /// Processes bytes through VTE for full ANSI color/attribute support.
    /// Note: callers must not split a `\r\n` pair across two calls (the second call
    /// would insert an extra `\r`, producing `\r\r\n`). Prefer complete chunks.
    #[allow(dead_code)]
    pub fn write_output(&self, bytes: &[u8]) {
        // Convert \n to \r\n - bare LF without preceding CR needs CR insertion
        let mut converted = Vec::with_capacity(bytes.len());
        let mut prev = 0u8;
        for &b in bytes {
            if b == b'\n' && prev != b'\r' {
                converted.push(b'\r');
            }
            converted.push(b);
            prev = b;
        }

        let mut term = self.term.lock();
        let mut processor = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        processor.advance(&mut *term, &converted);
    }

    /// Drain the CWD channel, then drain any remaining events.
    /// Sets `dirty = true` when PTY output was processed.
    #[allow(dead_code)]
    pub fn sync(&mut self) {
        self.sync_channels();
        if let Some(mut rx) = self.events_rx.take() {
            while let Ok(event) = rx.try_recv() {
                self.process_event(event);
            }
            self.events_rx = Some(rx);
        }
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(mut rx) = self.ghostty_events_rx.take() {
            while let Ok(event) = rx.try_recv() {
                self.process_ghostty_event(event);
            }
            self.ghostty_events_rx = Some(rx);
        }
    }

    /// Refresh the shell CWD from the process table (EP-002 US-007).
    ///
    /// OSC 7 updates are captured by the `Osc7Pty` read tap before VTE consumes
    /// the bytes. Unix/macOS also refresh from process-table state via
    /// `cwd_now()` as a fallback. The fallback is throttled so we don't
    /// `readlink` on every poll tick.
    pub fn sync_channels(&mut self) {
        if let Some(mut rx) = self.cwd_rx.take() {
            while let Ok(cwd) = rx.try_recv() {
                self.current_cwd = Some(cwd);
            }
            self.cwd_rx = Some(rx);
        }

        self.cwd_poll_ticks = self.cwd_poll_ticks.wrapping_add(1);
        if self.cwd_poll_ticks.is_multiple_of(25)
            && let Some(cwd) = self.cwd_now()
        {
            self.current_cwd = Some(cwd.to_string_lossy().into_owned());
        }
        self.drain_marks();
    }

    fn drain_marks(&mut self) {
        let Some(receiver) = self.marks_rx.as_ref() else {
            return;
        };
        let Ok(first) = receiver.try_recv() else {
            return;
        };
        let metrics = self.session_backend().grid_metrics();
        let history_size = i64::from(metrics.topmost_line.0.saturating_neg());
        let abs_line = history_size.saturating_add(i64::from(metrics.cursor.line.0));
        let bottom_abs = history_size.saturating_add(metrics.screen_lines.saturating_sub(1) as i64);
        let at = std::time::Instant::now();
        let mut marks = self
            .marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for raw in std::iter::once(first).chain(receiver.try_iter()) {
            marks.push(CommandMark {
                kind: raw.kind,
                exit_code: raw.exit_code,
                abs_line,
                at,
            });
        }
        marks.retain_at_or_below(bottom_abs);
    }

    /// Defensively reset terminal modes that could corrupt the outer terminal.
    /// Called on child exit before marking the terminal as exited.
    /// Only resets modes that are actually active (clean exits won't trigger).
    fn reset_active_modes(&mut self) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if self.ghostty.is_some() {
            return;
        }
        // Guard against double-reset: if we've already recorded the exit
        // status, the PTY writer is already closed and the next notify()
        // would log a swallowed EPIPE.
        if self.exited.is_some() {
            return;
        }
        let mode = *self.term.lock_unfair().mode();
        if mode.contains(TermMode::BRACKETED_PASTE) {
            self.notifier.notify(b"\x1b[?2004l" as &[u8]);
        }
        if mode.contains(TermMode::FOCUS_IN_OUT) {
            self.notifier.notify(b"\x1b[?1004l" as &[u8]);
        }
        if mode.intersects(TermMode::MOUSE_MODE) {
            self.notifier
                .notify(b"\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l" as &[u8]);
        }
        if mode.contains(TermMode::ALT_SCREEN) {
            self.notifier.notify(b"\x1b[?1049l" as &[u8]);
        }
    }

    /// Process a single alacritty event.
    fn process_event(&mut self, event: AlacEvent) {
        match event {
            AlacEvent::Wakeup => {
                self.dirty = true;
                // US-010: advance the readiness signal `workspace.up` polls.
                // Saturating (not wrapping) so the count is monotone for the
                // lifetime of a pane; u64 never realistically saturates.
                self.output_generation = self.output_generation.saturating_add(1);
            }
            AlacEvent::ChildExit(status) => {
                self.reset_active_modes();
                // EP-002 US-005: exit status now comes natively from alacritty's
                // `ChildExit(ExitStatus)`. On Unix a signal-kill has no `code()`
                // but carries the numeric signal via `ExitStatusExt::signal()`;
                // pair it with `strsignal` for the overlay ("11 (Segmentation
                // fault)"). No `from_raw(code<<8)` reconstruction and no
                // in-process strsignal reversal - alacritty hands us the number.
                #[cfg(unix)]
                if status.code().is_none()
                    && let Some(sig) = std::os::unix::process::ExitStatusExt::signal(&status)
                {
                    self.exit_signal = Some(format_signal(sig));
                }
                self.exited = Some(status.code().unwrap_or(-1));
                self.dirty = true;
                self.cached_foreground_command = None;
                #[cfg(all(unix, not(test)))]
                {
                    self.pty_guard = None;
                }
                self.reported_ports.clear();
            }
            AlacEvent::Exit => {
                self.reset_active_modes();
                // First-write-wins (US-003 AC): `Exit` is the EOF fallback with no
                // status. A real `ChildExit` code must never be clobbered by the -1
                // sentinel if both events fire. Mirrors Zed's register_task_finished
                // (crates/terminal/src/terminal.rs:2561-2563), where only ChildExit
                // stores a status and Exit is a status no-op.
                if self.exited.is_none() {
                    self.exited = Some(-1);
                }
                self.dirty = true;
                self.cached_foreground_command = None;
                #[cfg(all(unix, not(test)))]
                {
                    self.pty_guard = None;
                }
            }
            AlacEvent::Title(t) if !is_executable_path_title(&t) => {
                self.title = t;
            }
            // Windows consoles (pwsh/powershell/cmd) set their initial window
            // title to their own executable path before the user's profile runs
            // - e.g. `C:\Program Files\PowerShell\7\pwsh.exe`. Adopted verbatim
            // that leaks the shell install dir as the surface label (tab title,
            // persisted session `name`) and is never a
            // meaningful name, so a title that is just an absolute path to an
            // `.exe` is dropped - keep the previous/default name. Real titles
            // (Claude Code, prompt-driven labels) take the guarded arm above.
            AlacEvent::Title(_) => {}
            AlacEvent::ResetTitle => {
                self.title = String::from("Terminal");
            }
            AlacEvent::PtyWrite(text) => {
                self.notifier.notify(text.into_bytes());
            }
            AlacEvent::ClipboardStore(_selection, text) => {
                // Cap to prevent memory DoS from malicious programs (crate::limits).
                let within_cap = self.terminal_focused
                    && self.osc52_mode != Osc52Mode::Disabled
                    && text.len() <= MAX_OSC52_BYTES;
                if within_cap {
                    self.queue_clipboard_op(ClipboardOp::Store(text));
                }
            }
            AlacEvent::ClipboardLoad(_selection, format_fn)
                if self.terminal_focused && self.osc52_mode == Osc52Mode::CopyPaste =>
            {
                self.queue_clipboard_op(ClipboardOp::Load(format_fn));
            }
            AlacEvent::ClipboardLoad(..) => {}

            AlacEvent::ColorRequest(index, format_fn) => {
                // Respond synchronously to preserve PTY-write order - match
                // Zed (`crates/terminal/src/terminal.rs:997-1009`). Crossterm's
                // `query_foreground_color` / `query_background_color` (used by
                // the OpenAI Codex CLI to detect terminal colors and decide
                // whether to paint its input-bar tint) has a short timeout;
                // a deferred reply both misses it and scrambles ordering with
                // a following `\e[c` (DA1) query, after which Codex falls back
                // to "unknown bg" and silently drops the tint.
                //
                // The `index` here is alacritty's internal `NamedColor`
                // discriminant, NOT the OSC code itself: the VTE parser at
                // `vte-0.15/src/ansi.rs:1431` translates OSC 10/11/12 to
                // `NamedColor::Foreground (256) + (osc_code - 10)`. So the
                // 256/257/258 arms below match OSC 10/11/12; indices 0..=255
                // cover OSC 4 (`OSC 4 ; n ; ?` color-palette queries) which
                // some apps (vim, neovim, python-rich) use to detect themes.
                let theme = crate::theme::active_theme();
                use alacritty_terminal::vte::ansi::NamedColor;
                let color = if index == NamedColor::Foreground as usize {
                    Some(theme.foreground)
                } else if index == NamedColor::Background as usize {
                    Some(theme.ansi_background)
                } else if index == NamedColor::Cursor as usize {
                    Some(self.cursor_color_override.unwrap_or(theme.cursor))
                } else if index < 256 {
                    Some(palette_color_at(index as u8, &theme))
                } else {
                    None
                };
                if let Some(hsla) = color {
                    let rgb = hsla_to_alac_rgb(hsla);
                    let response = format_fn(rgb);
                    self.notifier.notify(response.into_bytes());
                }
            }
            AlacEvent::Bell => {}
            AlacEvent::CursorBlinkingChange => {
                let term = self.term.lock_unfair();
                self.cursor_blinking = term.cursor_style().blinking;
            }
            AlacEvent::TextAreaSizeRequest(format_fn) => {
                self.pending_size_ops.push(format_fn);
            }
            _ => {} // MouseCursorDirty, etc.
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn process_ghostty_event(&mut self, event: GhosttyUiEvent) {
        match event {
            GhosttyUiEvent::Wakeup(events) => {
                events.acknowledge_wakeup();
                self.dirty = true;
                self.output_generation = self.output_generation.saturating_add(1);
            }
            GhosttyUiEvent::Title(events) => {
                if let Some(title) = events.take_title()
                    && !is_executable_path_title(&title)
                {
                    self.title = title;
                }
            }
            GhosttyUiEvent::WorkingDirectory(events) => {
                if let Some(cwd) = events.take_working_directory() {
                    self.current_cwd = Some(cwd);
                }
            }
            GhosttyUiEvent::Clipboard(events) => {
                for text in events.take_clipboard() {
                    if self.terminal_focused
                        && self.osc52_mode != Osc52Mode::Disabled
                        && text.len() <= MAX_OSC52_BYTES
                    {
                        self.queue_clipboard_op(ClipboardOp::Store(text));
                    }
                }
            }
            GhosttyUiEvent::ServiceOutputReady(events) => {
                events.acknowledge_service_output();
                self.last_activity_burst = None;
                self.dirty = true;
            }
            GhosttyUiEvent::ChildExited { code, signal } => {
                if self.exited.is_none() {
                    self.exited = Some(code);
                    self.exit_signal = signal;
                }
                self.dirty = true;
                self.cached_foreground_command = None;
                self.reported_ports.clear();
                #[cfg(all(unix, not(test)))]
                {
                    self.pty_guard = None;
                }
            }
            GhosttyUiEvent::InputRejected(error) => {
                log::warn!(target: "paneflow::terminal::ghostty", "{error}");
            }
            GhosttyUiEvent::RuntimeFailed(error) => {
                log::error!(target: "paneflow::terminal::ghostty", "{error}");
                if self.exited.is_none() {
                    self.exited = Some(-1);
                }
                self.dirty = true;
            }
        }
    }

    fn queue_clipboard_op(&mut self, op: ClipboardOp) {
        if self.pending_clipboard_ops.len() >= MAX_PENDING_CLIPBOARD_OPS {
            self.pending_clipboard_ops.remove(0);
        }
        self.pending_clipboard_ops.push(op);
    }

    /// Read the shell's CWD from the OS on demand.
    /// Fallback for shells that don't emit OSC 7 - used at split time.
    #[cfg(target_os = "linux")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        // US-034: once the child has exited, `child_pid` is stale and the OS
        // may have reused it for an unrelated process - reading
        // `/proc/<pid>/cwd` would silently return a third party's CWD. Bail.
        if self.exited.is_some() {
            return None;
        }
        // Display-only terminal (no real PTY): `child_pid` is 0 → `/proc/0/cwd`
        // doesn't exist. Bail explicitly to match the macOS/Windows guards.
        if self.child_pid == 0 {
            return None;
        }
        let proc_path = format!("/proc/{}/cwd", self.child_pid);
        std::fs::read_link(&proc_path).ok()
    }

    /// macOS implementation of `cwd_now`: read the PTY child shell's current
    /// working directory from the kernel via
    /// `proc_pidinfo(pid, PROC_PIDVNODEPATHINFO, 0, &buf, size)`.
    #[cfg(target_os = "macos")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        use std::ffi::CStr;
        use std::mem::MaybeUninit;
        use std::os::raw::c_void;

        // US-034: after exit, `child_pid` may have been reused - `proc_pidinfo`
        // would return an unrelated process's CWD. Bail.
        if self.exited.is_some() {
            return None;
        }

        // Display-only terminal (no real PTY) or a child whose pid hasn't been
        // resolved yet: `child_pid` is 0. Bail before the FFI - `proc_pidinfo(0,
        // …)` targets the kernel swapper (pid 0), fails with EPERM, and would
        // spam a misleading "shell may have exited" warning on every poll tick.
        // Mirrors the `foreground_command` guards on every platform.
        if self.child_pid == 0 {
            return None;
        }

        let pid = self.child_pid as libc::c_int;
        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;

        // SAFETY: `info` is a stack-allocated MaybeUninit zeroed above; we
        // only read from it if the syscall reports the full struct size
        // was written. Zeroing first leaves it in a defined state on any
        // partial-write error path.
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                info.as_mut_ptr() as *mut c_void,
                size,
            )
        };

        if written <= 0 {
            let err = std::io::Error::last_os_error();
            log::warn!(
                "cwd_now: proc_pidinfo(pid={pid}) returned {written} ({err}) - shell may have exited or SIP / sandbox is denying the read"
            );
            return None;
        }

        if written < size {
            log::warn!(
                "cwd_now: proc_pidinfo(pid={pid}) wrote {written} of {size} bytes - truncated result discarded"
            );
            return None;
        }

        // SAFETY: `written == size` implies the kernel fully populated the
        // buffer with a valid `proc_vnodepathinfo`.
        let info = unsafe { info.assume_init() };

        let ptr = info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char;
        // SAFETY: the kernel guarantees `vip_path` holds a NUL-terminated
        // C string not exceeding `MAXPATHLEN` bytes when the syscall
        // succeeds with full size.
        let cstr = unsafe { CStr::from_ptr(ptr) };
        match cstr.to_str() {
            Ok(s) if !s.is_empty() => Some(std::path::PathBuf::from(s)),
            _ => None,
        }
    }

    /// Stub for other non-Linux platforms (Windows, BSDs).
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// Scan the last 100 lines of terminal output for server/service patterns.
    /// Returns newly detected services (deduped against previously reported ports).
    /// Lock on `self.term` is held only for text extraction, then released before parsing.
    pub fn scan_output(&mut self) -> Vec<ServiceInfo> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            let lines = ghostty.recent_output_lines();
            return self.detect_services_in_lines(&lines);
        }
        let lines: Vec<String> = {
            // Read-only grid scan; unfair lock avoids queueing behind the
            // PTY reader thread on the periodic service-detection sweep.
            let term = self.term.lock_unfair();
            let bottom = term.bottommost_line();
            let top_limit = term.topmost_line();
            let cols = term.last_column();

            let mut buf = Vec::with_capacity(100);
            let mut row = bottom.0;
            while row >= top_limit.0 && buf.len() < 100 {
                let line = term.bounds_to_string(
                    AlacPoint::new(GridLine(row), GridCol(0)),
                    AlacPoint::new(GridLine(row), cols),
                );
                let trimmed = line.trim_end().to_string();
                if !trimmed.is_empty() {
                    buf.push(trimmed);
                }
                row -= 1;
            }
            buf
            // term lock dropped here
        };

        self.detect_services_in_lines(&lines)
    }

    fn detect_services_in_lines(&mut self, lines: &[String]) -> Vec<ServiceInfo> {
        // Detect framework from ALL lines (context-wide), not just the port line.
        // Next.js prints "▲ Next.js 16.1.6" on one line and "localhost:3000" on another.
        let all_text = lines.join(" ");
        let (global_label, global_is_frontend) = detect_framework(&all_text);

        let mut results = Vec::new();
        for line in lines {
            if let Some(mut info) = parse_service_line(line)
                && !self.reported_ports.contains(&info.port)
            {
                if info.label.is_none() {
                    info.label = global_label.clone();
                    info.is_frontend = global_is_frontend;
                }
                self.reported_ports.insert(info.port);
                results.push(info);
            }
        }

        results
    }

    pub fn write_to_pty(&self, input: impl Into<Cow<'static, [u8]>>) {
        // US-002: any path through write_to_pty is genuine user input
        // (keystroke, paste, mouse report, IME commit, user scroll). Mark the
        // session user-initiated so a later exit closes the pane. Automated
        // protocol writes (focus reports, search RIS reset, OSC responses)
        // deliberately bypass this by calling `self.notifier.notify` directly.
        self.keyboard_input_sent
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let input = input.into();
        self.notify_or_buffer(input);
    }

    pub(super) fn set_terminal_focused(&mut self, focused: bool) {
        self.terminal_focused = focused;
        self.clipboard_gate.set_focused(focused);
    }

    fn set_osc52_mode(&mut self, mode: Osc52Mode) {
        self.osc52_mode = mode;
        self.clipboard_gate
            .set_policy(mode != Osc52Mode::Disabled, mode == Osc52Mode::CopyPaste);
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn clipboard_gate(&self) -> Arc<ClipboardGate> {
        self.clipboard_gate.clone()
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn dispatch_ghostty_input(
        &self,
        input: PendingTerminalInput,
        user_initiated: bool,
    ) -> BackendInputResult {
        let Some(ghostty) = self.ghostty.as_ref() else {
            return BackendInputResult::NotHandled;
        };
        let Ok(mut pending) = self.pending_input.lock() else {
            return BackendInputResult::Rejected;
        };
        let pending_bytes = pending.iter().fold(0usize, |total, item| {
            total.saturating_add(item.queued_bytes())
        });
        let total = pending_bytes.saturating_add(ghostty.queued_input_bytes());
        let queue_limit = input.queue_limit();
        if !input.fits_after(total) {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty input rejected at the {} byte queue limit",
                queue_limit
            );
            return BackendInputResult::Rejected;
        }

        if ghostty.is_promoted() && pending.is_empty() {
            match input.try_send(ghostty) {
                GhosttyInputSendResult::Sent => {
                    if user_initiated {
                        self.keyboard_input_sent
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    return BackendInputResult::Accepted;
                }
                GhosttyInputSendResult::Full => {}
                GhosttyInputSendResult::Closed => return BackendInputResult::Rejected,
            }
        }

        pending.push_back(input);
        if user_initiated {
            self.keyboard_input_sent
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        BackendInputResult::Accepted
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn write_ghostty_key(
        &self,
        input: paneflow_terminal_ghostty::KeyInput,
        legacy: Option<Cow<'static, [u8]>>,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Key { input, legacy }, true)
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn write_ghostty_mouse(
        &self,
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
        legacy: Option<Vec<u8>>,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(
            PendingTerminalInput::Mouse {
                input,
                repeat,
                legacy,
            },
            true,
        )
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn write_ghostty_focus(
        &self,
        event: paneflow_terminal_ghostty::FocusEvent,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Focus(event), false)
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) fn write_ghostty_paste(
        &self,
        text: String,
        legacy_bracketed: bool,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(
            PendingTerminalInput::Paste {
                text,
                allow_unsafe: true,
                legacy_bracketed,
            },
            true,
        )
    }

    /// Send input to the live PTY, or queue it when the terminal is still
    /// display-only (US-012 pre-promotion window). The display-only notifier
    /// drops every write, so an auto-launch command or a
    /// keystroke typed before the off-thread fork resolved would otherwise be
    /// lost; [`promote`](Self::promote) flushes the queue in order. Bounded by
    /// [`MAX_PENDING_INPUT_BYTES`] so a never-promoted terminal can't grow it
    /// without bound.
    fn notify_or_buffer(&self, input: Cow<'static, [u8]>) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            if !input.is_empty() {
                let _ = ghostty;
                self.dispatch_ghostty_input(PendingTerminalInput::Raw(input), false);
            }
            return;
        }
        if self.notifier.0.is_pty() {
            self.notifier.notify(input);
            return;
        }
        if input.is_empty() {
            return;
        }
        if let Ok(mut pending) = self.pending_input.lock() {
            let queued: usize = pending.iter().map(PendingTerminalInput::queued_bytes).sum();
            if queued + input.len() <= MAX_PENDING_INPUT_BYTES {
                pending.push_back(PendingTerminalInput::Raw(input));
            }
        }
    }

    /// US-002: write to the PTY WITHOUT marking the session user-initiated.
    /// For automated protocol writes (DEC 1004 focus in/out reports, search
    /// RIS reset) that must not flip `keyboard_input_sent` - otherwise a
    /// failed-spawn pane that merely gains focus would wrongly close on exit.
    pub fn write_to_pty_silent(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.notify_or_buffer(input.into());
    }

    /// US-002: whether a child exit should close the pane. A user-initiated
    /// session (any input was sent) always closes; otherwise only a clean exit
    /// (code 0) closes - a non-zero exit with no input is a spawn/launch
    /// failure and stays open so the exit overlay shows the code. Mirrors Zed's
    /// discriminator (crates/terminal/src/terminal.rs:2572-2576).
    pub fn should_close_on_exit(&self) -> bool {
        self.keyboard_input_sent
            .load(std::sync::atomic::Ordering::Relaxed)
            || self.exited == Some(0)
    }

    /// Extract terminal history as plain text (ANSI stripped) for session persistence.
    /// The active viewport is deliberately excluded so restoring a session cannot
    /// replay the previous visible frame ahead of fresh shell output.
    /// Caps at 4000 lines and 400,000 characters. Returns None if history is empty.
    pub fn extract_scrollback(&self) -> Option<String> {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.extract_scrollback();
        }
        Self::extract_scrollback_from(&self.term)
    }

    /// US-011: scrollback drain decoupled from `&self` so `save_session` can
    /// run it on a background thread against a cloned [`SharedTerm`] handle
    /// (the term mutex is `Send + Sync` - it is the only cross-thread state in
    /// the app) instead of holding the GPUI main thread. US-012's windowing
    /// keeps the lock bounded to the most-recent `MAX_LINES` rows.
    fn extract_scrollback_from(term: &SharedTerm) -> Option<String> {
        const MAX_LINES: usize = 4000;

        // Read-only scrollback drain for session persistence.
        let term = term.lock_unfair();
        let top = term.topmost_line();
        let cols = term.last_column();

        // Alacritty addresses real history with negative grid lines. Line zero
        // starts the active viewport, which must never be persisted as history.
        if top.0 >= 0 {
            return None;
        }

        // US-012: window to the most-recent MAX_LINES *before* the loop so the
        // lock is never held while materializing the full history (scrollback
        // can be very large - see DEFAULT_SCROLLBACK_LINES). Walk oldest to
        // newest from the bounded negative-line window through line -1.
        let start = (-(MAX_LINES as i32)).max(top.0);
        let mut lines: Vec<String> = Vec::with_capacity((-start).max(0) as usize);
        let mut row = start;
        while row < 0 {
            let text = term.bounds_to_string(
                AlacPoint::new(GridLine(row), GridCol(0)),
                AlacPoint::new(GridLine(row), cols),
            );
            lines.push(text.trim_end().to_string());
            row += 1;
        }

        // Trim trailing empty lines
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }

        if lines.is_empty() {
            return None;
        }

        // Keep only the most recent MAX_LINES
        if lines.len() > MAX_LINES {
            lines.drain(..lines.len() - MAX_LINES);
        }

        let mut result = lines.join("\n");

        // Cap at MAX_CHARS, then trim to last complete line and strip any
        // partial ANSI escape at the boundary. Shared by both the background
        // save path and the synchronous quit path (`save_session_blocking`).
        cap_scrollback_at_char_boundary(&mut result, MAX_CHARS);

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Best-effort foreground command of this surface, cached by the off-thread
    /// pane process scanner. Returns the shell command while idle or the current
    /// child approximation while busy. `None` means callers fall back to the OSC
    /// title.
    pub fn foreground_command(&self) -> Option<String> {
        self.cached_foreground_command.clone()
    }

    /// Search the scrollback for `pattern` (plain-text, case-insensitive) and
    /// return matching lines as `(grid_line, text)` pairs, deduped by line and
    /// capped at `max_matches`. The bool is `true` when the cap truncated the
    /// result. Backs the `surface.search` IPC method (US-004). Alacritty holds
    /// its grid lock only for text extraction; Ghostty performs search and
    /// matched-line extraction atomically on its runtime thread.
    pub fn search_scrollback(
        &self,
        pattern: &str,
        max_matches: usize,
    ) -> (Vec<(i32, String)>, bool) {
        if pattern.is_empty() || max_matches == 0 {
            return (Vec::new(), false);
        }
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            return ghostty.search_scrollback(pattern, max_matches);
        }
        let result = crate::search::search_term(&self.term, pattern, false);

        // Collect unique line numbers in order of first appearance.
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<i32> = Vec::new();
        let mut hit_cap = false;
        for m in &result.matches {
            let row = m.start.line.0;
            if seen.insert(row) {
                rows.push(row);
                if rows.len() >= max_matches {
                    hit_cap = true;
                    break;
                }
            }
        }

        let term = self.term.lock_unfair();
        let cols = term.last_column();
        let out: Vec<(i32, String)> = rows
            .into_iter()
            .map(|row| {
                let text = term.bounds_to_string(
                    AlacPoint::new(GridLine(row), GridCol(0)),
                    AlacPoint::new(GridLine(row), cols),
                );
                (row, text.trim_end().to_string())
            })
            .collect();
        (out, hit_cap)
    }

    /// Strip every byte that could re-introduce a live escape/CSI/OSC/DCS
    /// sequence (or C1 control) from a single restored-scrollback line, so the
    /// documented "plain, ANSI stripped" invariant (schema.rs `scrollback`
    /// field) is *enforced* on the restore path - not merely assumed.
    ///
    /// A tampered/imported `session.json` can carry raw VT bytes in
    /// `surface.scrollback`; feeding them verbatim into the VTE processor
    /// allows single-line title-spoof / OSC8 clickable-link injection into the
    /// restored grid (phishing primitive). We drop the ESC introducer
    /// (`0x1b`), all other C0 control code points (keeping only `\t` - `\n`
    /// has already been consumed by the line split and `\r\n` is re-added by
    /// the caller), and the C1 control range (U+0080..=U+009F, which alacritty
    /// also treats as escape introducers). Pure string op: cross-platform, no
    /// OS/`libc` calls, no fallible step.
    fn sanitize_scrollback_line(line: &str) -> String {
        line.chars()
            .filter(|&c| {
                c == '\t'
                    || (!c.is_control()
                        // Reject C1 controls (0x80..=0x9f); `is_control`
                        // already covers them, but spell it out for intent.
                        && !('\u{80}'..='\u{9f}').contains(&c))
            })
            .collect()
    }

    /// EP-005 US-014: remember a port announced by a service URL in this
    /// terminal's output, for the collision cross-check against the scan's
    /// LISTEN attribution. The source text is UNTRUSTED terminal output, so
    /// the list is bounded: a flood of fake announcements can at most fill
    /// 16 slots (oldest kept - the legitimate dev-server line is printed
    /// once at startup, i.e. first).
    /// Drop announce-dedup state for ports that are no longer LISTEN
    /// anywhere in the workspace. Without this, `reported_ports` was only
    /// cleared on ChildExit - a dev server restarted inside a live shell
    /// (nodemon, plain re-run) re-printed its banner but could never re-fire
    /// `ServiceDetected`, leaving the sidebar chip stale until the pane died.
    pub fn retain_reported_ports(&mut self, live: &[u16]) {
        self.reported_ports.retain(|p| live.contains(p));
    }

    pub fn note_announced_port(&mut self, port: u16) {
        const MAX_ANNOUNCED_PORTS: usize = 16;
        if !self.announced_ports.contains(&port) && self.announced_ports.len() < MAX_ANNOUNCED_PORTS
        {
            self.announced_ports.push(port);
        }
    }

    /// Feed saved scrollback text into the terminal grid via VTE processor.
    /// Called during session restore, before the shell has produced output.
    /// Prepends `\x1b[0m` (SGR reset) to clear any dangling style state from
    /// a prior truncated scrollback - ANSI-safe defense-in-depth (US-012).
    pub fn restore_scrollback(&self, text: &str) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = &self.ghostty {
            ghostty.restore_scrollback(text);
            return;
        }
        let mut term = self.term.lock();
        let mut processor = alacritty_terminal::vte::ansi::Processor::<
            alacritty_terminal::vte::ansi::StdSyncHandler,
        >::new();
        // Reset any dangling style state before feeding restored content
        processor.advance(&mut *term, b"\x1b[0m");
        // Feed each line with \r\n to advance the cursor
        for line in text.split('\n') {
            // Enforce the "plain, ANSI stripped" invariant: untrusted bytes
            // from a deserialized session must never reach the VTE parser as
            // live escape/CSI/OSC sequences (title-spoof / OSC8 link
            // injection). Sanitize before advancing.
            let sanitized = Self::sanitize_scrollback_line(line);
            let bytes = sanitized.as_bytes();
            if !bytes.is_empty() {
                processor.advance(&mut *term, bytes);
            }
            processor.advance(&mut *term, b"\r\n");
        }
    }
}

/// Cap `result` at `max_chars` bytes, cutting on a UTF-8 char boundary, then
/// trim to the last complete line and strip any partial ANSI escape at the cut.
///
/// U-001: `String::truncate` panics if the byte index is not on a char
/// boundary. Scrollback is built from real grid cells (CJK, emoji,
/// box-drawing are routine coding-agent output), so a raw `truncate(max_chars)`
/// panics whenever `max_chars` lands mid-codepoint. `floor_char_boundary`
/// rounds the index down to the nearest boundary first (no-op when already
/// aligned), so the result is always a valid `&str` of length ≤ `max_chars`.
pub(super) fn cap_scrollback_at_char_boundary(result: &mut String, max_chars: usize) {
    if result.len() > max_chars {
        let boundary = result.floor_char_boundary(max_chars);
        result.truncate(boundary);
        // `rfind('\n')` always returns a char boundary, so this second
        // truncate is already safe.
        if let Some(last_newline) = result.rfind('\n') {
            result.truncate(last_newline);
        }
        strip_partial_ansi_tail(result);
    }
}

/// Strip any partial ANSI escape sequence from the end of a truncated string.
///
/// Scans backward from the end for an ESC (`\x1b`) that starts a CSI (`\x1b[`),
/// OSC (`\x1b]`), or DCS (`\x1bP`) sequence. If the sequence is unterminated
/// (no final byte in the valid range), it is removed. Plain text strings with
/// no ESC bytes are returned unmodified - truncation is identical to naive splitting.
pub(super) fn strip_partial_ansi_tail(text: &mut String) {
    let Some(esc_pos) = text.rfind('\x1b') else {
        return; // No escape sequences at all
    };

    let tail = &text[esc_pos..];
    let bytes = tail.as_bytes();

    if bytes.len() < 2 {
        text.truncate(esc_pos);
        return;
    }

    match bytes[1] {
        b'[' => {
            // CSI sequence: \x1b[ ... terminated by byte in 0x40..=0x7E
            let terminated = bytes[2..].iter().any(|&b| (0x40..=0x7E).contains(&b));
            if !terminated {
                text.truncate(esc_pos);
            }
        }
        b']' => {
            // OSC sequence: \x1b] ... terminated by BEL (0x07) or ST (\x1b\\)
            let terminated = bytes[2..].contains(&0x07) || tail[2..].contains("\x1b\\");
            if !terminated {
                text.truncate(esc_pos);
            }
        }
        b'P' => {
            // DCS sequence: \x1bP ... terminated by ST (\x1b\\)
            let terminated = tail[2..].contains("\x1b\\");
            if !terminated {
                text.truncate(esc_pos);
            }
        }
        _ => {
            // Other ESC sequences (SS2, SS3, etc.) are 2 bytes - complete as-is.
        }
    }
}

/// Compute the PaneFlow IPC socket path, delegating to `runtime_paths` so
/// the fallback chain stays in sync with `ipc::socket_path`.
fn paneflow_socket_path() -> Option<String> {
    crate::runtime_paths::socket_path().map(|p| p.display().to_string())
}

/// US-009 - extract the embedded AI-hook binaries into the user's cache
/// dir, then expose that dir via `PANEFLOW_BIN_DIR` and prepend it to
/// the child shell's `PATH`.
///
/// Silent-fail: any error (extraction IO failure, unresolvable
/// `cache_dir`) is logged at `warn` and then swallowed so the terminal
/// opens normally without the AI-hook loader for this session. PRD
/// constraint C4 mandates the terminal must never fail to open because
/// of AI-hook wiring.
///
/// Factored out of `TerminalState::new` so the helper is independently
/// testable - the extraction side-effect lives in `ai_hooks::extract`
/// (already unit-tested in US-008); this glue only layers the env
/// mutations on top of a returned `PathBuf`.
fn inject_ai_hook_env(env: &mut std::collections::HashMap<String, String>) {
    let bin_dir = match crate::ai_hooks::extract::ensure_binaries_extracted() {
        Ok(p) => p,
        Err(e) => {
            // `{e:#}` emits the full anyhow context chain (each
            // `.with_context()` frame) rather than just the outermost
            // message - crucial for diagnosing cache-dir permission
            // errors that arrive with a useful inner IO error.
            log::warn!(
                "paneflow: AI-hook binary extraction failed ({e:#}); sidebar loader will not activate for this terminal session"
            );
            return;
        }
    };

    // `PANEFLOW_BIN_DIR` is the source-of-truth the shim uses for its
    // self-exclusion PATH walk (US-004). Set it even in the unlikely
    // event the PATH-prepend below fails, so the shim can still
    // identify its own dir if a later code path routes into it.
    env.insert("PANEFLOW_BIN_DIR".into(), bin_dir.display().to_string());

    prepend_bin_dir_to_path(env, &bin_dir);
}

fn reassert_paneflow_bin_dir_first(env: &mut std::collections::HashMap<String, String>) {
    let Some(bin_dir) = env.get("PANEFLOW_BIN_DIR").cloned() else {
        return;
    };
    if bin_dir.is_empty() {
        return;
    }
    prepend_bin_dir_to_path(env, std::path::Path::new(&bin_dir));
}

/// Prepend `bin_dir` to `env["PATH"]` (or to the process `PATH` if the
/// env map does not yet carry one). Cross-platform: uses
/// `std::env::join_paths`, which emits `:` on Unix and `;` on Windows.
///
/// If join-paths fails (e.g. a `PATH` entry contains a platform
/// separator byte - invalid but physically possible), logs a warning
/// and leaves the env map unchanged. Better "no prepend" than "broken
/// PATH".
fn prepend_bin_dir_to_path(
    env: &mut std::collections::HashMap<String, String>,
    bin_dir: &std::path::Path,
) {
    // Order of precedence: explicit map entry first, then process env.
    // `setup_shell_integration` (shell.rs) does not set PATH, so in
    // practice this always falls through to the process PATH - but the
    // explicit-map branch makes the helper reusable and keeps tests
    // decoupled from the process environment.
    let existing: Option<std::ffi::OsString> = env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));

    let mut components: Vec<std::path::PathBuf> = vec![bin_dir.to_path_buf()];
    // Guard against an empty `PATH` string: on Unix, `split_paths("")`
    // yields a single `PathBuf::from("")` which `execvp` resolves as the
    // current working directory - that would silently put `.` on the
    // child's PATH (a classic shell-injection surface). Treat empty and
    // absent identically.
    if let Some(existing) = existing.as_deref()
        && !existing.is_empty()
    {
        components.extend(std::env::split_paths(existing));
    }

    match std::env::join_paths(components) {
        Ok(joined) => {
            // `join_paths` always produces valid UTF-8 when all inputs
            // were UTF-8 PathBufs + an OsString PATH - on all three
            // supported OSes, PATH is conventionally UTF-8 so the
            // `to_string_lossy` round-trip is safe. If a real-world PATH
            // entry contains non-UTF-8 bytes, we lose those in the
            // lossy conversion - but the env map is keyed on
            // `HashMap<String, String>` to begin with, so this is a
            // pre-existing constraint inherited from
            // `PtyBackend::spawn`, not introduced here.
            env.insert("PATH".into(), joined.to_string_lossy().into_owned());
        }
        Err(e) => {
            log::warn!(
                "paneflow: could not prepend AI-hook bin dir {} to PATH: {e}",
                bin_dir.display()
            );
        }
    }
}

/// True if `key` names a dynamic-loader-influencing environment variable that
/// an untrusted source (an imported `session.json` surface env, or the global
/// `terminal.env` config) must NOT be allowed to inject into a spawned child:
/// `LD_PRELOAD` / `LD_LIBRARY_PATH` / `LD_AUDIT` and any `LD_*` on Linux, plus
/// any `DYLD_*` on macOS. Letting these through is an RCE vector - the operator
/// treats imported sessions as untrusted, and the child is always the
/// configured shell. The match is case-sensitive on purpose: the unix loaders
/// only honour the exact upper-case spelling, so a lower-case `ld_preload` is
/// inert and need not be dropped. On Windows these names are meaningless, so the
/// check is a harmless no-op there (the caller has already upper-cased `key`).
fn is_loader_influencing_env_key(key: &str) -> bool {
    key.starts_with("LD_") || key.starts_with("DYLD_")
}

/// True if `key` is one of the launching agent session's identity/credential
/// markers - see [`INHERITED_AGENT_SESSION_ENV`].
fn is_inherited_agent_session_env_key(key: &str) -> bool {
    INHERITED_AGENT_SESSION_ENV.contains(&key)
}

fn is_forbidden_child_env_key(key: &str) -> bool {
    is_inherited_agent_session_env_key(key) || is_loader_influencing_env_key(key)
}

/// True if `key` is a well-formed environment variable name safe to insert into
/// a child env block: non-empty and free of `=` and NUL, which would otherwise
/// corrupt the name/value framing. Charset is intentionally NOT restricted to a
/// strict POSIX set - legitimate user vars (e.g. `ANTHROPIC_API_KEY`) are
/// already all-caps `[A-Z0-9_]`, and over-restricting would silently drop valid
/// keys.
fn is_valid_env_name(key: &str) -> bool {
    !key.is_empty() && !key.contains('=') && !key.contains('\0')
}

fn is_wsl_shell(shell: &str) -> bool {
    let executable = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    executable.eq_ignore_ascii_case("wsl.exe") || executable.eq_ignore_ascii_case("wsl")
}

fn is_wslenv_identifier(key: &str) -> bool {
    let mut bytes = key.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn wslenv_entry_covers(entry: &str, key: &str, requires_path_translation: bool) -> bool {
    let (name, flags) = entry.split_once('/').unwrap_or((entry, ""));
    name == key
        && (!flags.contains('w') || flags.contains('u'))
        && (!requires_path_translation || flags.contains('p'))
}

fn merge_wslenv<'a>(
    initial: Option<&str>,
    env_keys: impl IntoIterator<Item = &'a str>,
) -> Option<String> {
    let existing_entries = initial
        .map(|value| value.split(':').collect::<Vec<_>>())
        .unwrap_or_default();
    let mut keys = env_keys
        .into_iter()
        .filter(|key| {
            is_wslenv_identifier(key) && !matches!(*key, "PATH" | "WSLENV" | "SHLVL" | "LANG")
        })
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();

    let additions = keys
        .into_iter()
        .filter_map(|key| {
            let requires_path_translation = matches!(key, "PANEFLOW_BIN_DIR" | "PANEFLOW_HOOK_LOG");
            if existing_entries
                .iter()
                .any(|entry| wslenv_entry_covers(entry, key, requires_path_translation))
            {
                None
            } else if requires_path_translation {
                Some(format!("{key}/up"))
            } else {
                Some(format!("{key}/u"))
            }
        })
        .collect::<Vec<_>>();

    if additions.is_empty() {
        return initial.map(str::to_owned);
    }

    let additions = additions.join(":");
    Some(match initial.filter(|value| !value.is_empty()) {
        Some(initial) => format!("{initial}:{additions}"),
        None => additions,
    })
}

fn augment_wslenv(env: &mut std::collections::HashMap<String, String>) {
    let initial = env
        .get("WSLENV")
        .cloned()
        .or_else(|| std::env::var("WSLENV").ok());
    if let Some(merged) = merge_wslenv(initial.as_deref(), env.keys().map(String::as_str)) {
        env.insert("WSLENV".into(), merged);
    }
}

/// True for an OSC 0/2 title that is merely an absolute path to an `.exe` - the
/// self-title Windows shells (`pwsh.exe`, `powershell.exe`, `cmd.exe`) emit at
/// startup before the user's profile runs. Such a title is never a human-facing
/// surface label, so callers drop it and keep the previous (or default) name.
/// Matches nothing on a Unix title (a backslash path is not absolute there, and
/// a `/usr/bin/pwsh` title has no `.exe` extension), so it is a Windows-targeted
/// filter with no false positives on real labels (e.g. `Claude Code`) or on a
/// prompt that sets the title to a bare cwd (no `.exe`).
fn is_executable_path_title(title: &str) -> bool {
    let p = std::path::Path::new(title);
    p.is_absolute()
        && p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
}

#[cfg(all(test, windows))]
mod title_filter_tests {
    use super::is_executable_path_title;

    #[test]
    fn drops_shell_self_path_title_but_keeps_human_labels() {
        // The exact pwsh self-title that leaked into the sidebar / tabs.
        assert!(is_executable_path_title(
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        ));
        assert!(is_executable_path_title(r"C:\Windows\System32\cmd.exe"));
        // Real labels and cwd-as-title prompts must survive.
        assert!(!is_executable_path_title("Claude Code"));
        assert!(!is_executable_path_title(r"C:\dev\paneflow"));
        assert!(!is_executable_path_title(""));
        // A bare relative name is not an absolute path → kept (only the
        // absolute self-path is the offender).
        assert!(!is_executable_path_title("pwsh.exe"));
    }
}

/// Assemble the child PTY environment: PaneFlow identity vars, explicit TERM /
/// locale / terminal-program identification, the AI-hook PATH prepend, and the
/// user-env merge (a user var wins on collision EXCEPT the protected keys
/// PaneFlow owns and `PANEFLOW_BIN_DIR` is re-prepended after any user PATH).
/// Pure except for `inject_ai_hook_env` staging the shim
/// binaries, so the env contract stays unit-testable now that the mockable
/// `PtyBackend::spawn` seam is gone (EP-002 US-004). Mirrors Zed's
/// `insert_zed_terminal_env`.
fn assemble_pty_env(
    mut env: std::collections::HashMap<String, String>,
    workspace_id: u64,
    surface_id: u64,
    user_env: Option<std::collections::HashMap<String, String>>,
) -> std::collections::HashMap<String, String> {
    // PaneFlow identity vars (AI-hook + MCP bridge integration).
    // `0` is reserved for detached terminals such as discovered worktree Review
    // terminals. Do not advertise a fake workspace id to the IPC hook.
    if workspace_id != 0 {
        env.insert("PANEFLOW_WORKSPACE_ID".into(), workspace_id.to_string());
    }
    env.insert("PANEFLOW_SURFACE_ID".into(), surface_id.to_string());
    if let Some(socket_path) = paneflow_socket_path() {
        env.insert("PANEFLOW_SOCKET_PATH".into(), socket_path);
    }

    // Propagate the opt-in hook-diagnostic log path explicitly so the whole
    // chain (shell → shim → agent → ai-hook) appends to the same file even if
    // a PTY backend ever clears the inherited env. No-op when unset.
    if let Some(log_path) = std::env::var_os("PANEFLOW_HOOK_LOG")
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
    {
        env.insert("PANEFLOW_HOOK_LOG".into(), log_path);
    }

    // Explicit TERM so TUI apps detect capabilities correctly.
    env.insert("TERM".into(), "xterm-256color".into());

    // Ensure a UTF-8 locale in minimal environments (containers, etc.).
    if std::env::var("LANG").map_or(true, |v| v.is_empty()) {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }

    // Standard terminal identification for capability detection.
    env.insert("TERM_PROGRAM".into(), "paneflow".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    env.insert("COLORTERM".into(), "truecolor".into());

    // Reset SHLVL so the child shell starts fresh at 1. alacritty's `tty`
    // inherits the parent environment (no `env_clear`), so unlike the old
    // portable-pty `env_remove("SHLVL")` we must actively override the value
    // PaneFlow itself inherited (typically >= 2 when launched from a terminal),
    // which otherwise breaks nested-shell prompt detection (oh-my-zsh subshell
    // banner, fish $SHLVL gating). "0" makes the shell initialize it to 1.
    env.insert("SHLVL".into(), "0".into());

    // Cross-platform AI-hook PATH-prepend: stage the embedded shim binaries and
    // prepend their dir to `$PATH` so `claude`/`codex` route through the shim.
    // Silent-fail (the terminal still opens). Sets `PANEFLOW_BIN_DIR`.
    inject_ai_hook_env(&mut env);

    // Merge user-supplied env on top, EXCEPT the protected keys PaneFlow owns:
    // TERM/COLORTERM/TERM_PROGRAM drive capability detection; SHLVL is reset so
    // shells start fresh; the PANEFLOW_* identity vars are how the MCP bridge
    // and the AI-hook shim find PaneFlow - letting a user clobber them would
    // silently break those features.
    if let Some(user_vars) = user_env {
        const PROTECTED: &[&str] = &[
            "TERM",
            "COLORTERM",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "SHLVL",
            "PANEFLOW_WORKSPACE_ID",
            "PANEFLOW_SURFACE_ID",
            "PANEFLOW_SOCKET_PATH",
            "PANEFLOW_BIN_DIR",
        ];
        for (k, v) in user_vars {
            // Windows env names are case-insensitive; normalise so a user
            // `Path` cannot shadow inherited `PATH` and the protected-key check
            // is not bypassed by casing.
            #[cfg(windows)]
            let k = k.to_uppercase();
            // Reject malformed env names (empty / `=` / NUL) and drop
            // dynamic-loader-influencing keys (LD_* / DYLD_*) outright: an
            // imported `session.json` surface env or the global `terminal.env`
            // is untrusted, and these inject a bundled `.so` into the spawned
            // shell (RCE). `PATH` is deliberately still mergeable here (a
            // documented US-014 use case), but PANEFLOW_BIN_DIR is re-prepended
            // after the merge so agent commands still route through the shim.
            if !is_valid_env_name(&k) || is_forbidden_child_env_key(&k) {
                continue;
            }
            if PROTECTED.contains(&k.as_str()) {
                continue;
            }
            env.insert(k, v);
        }
    }

    // Runs after the user/session merge so neither an inherited value nor a
    // hand-written `terminal.env` entry can put these back.
    env.retain(|k, _| !is_inherited_agent_session_env_key(k));
    reassert_paneflow_bin_dir_first(&mut env);

    env
}

/// True when `pid` is still the leader of its own process group - i.e. the
/// session we spawned. alacritty's `tty::new` calls `setsid()` on the child, so
/// `child_pid` is both the PID and the PGID of the session leader and
/// `getpgid(pid) == pid` holds for as long as that session lives. After the
/// child exits the kernel can recycle `pid` onto an unrelated process whose
/// pgid differs; this identity check closes the PID-reuse window that a bare
/// `kill(-pid, 0)` existence probe leaves open, so teardown never signals a
/// stranger's group.
#[cfg(unix)]
fn is_own_session_group(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: getpgid is a pure query; it returns the pgid, or -1 (ESRCH) when
    // no such process exists - neither equals our positive `pid` unless `pid`
    // is genuinely its own group leader.
    unsafe { libc::getpgid(pid) == pid }
}

/// Send SIGTERM to the child's process group, guarded by the session-identity
/// check ([`is_own_session_group`]) so a dead or recycled `pid` is a harmless
/// no-op. Returns true if SIGTERM was delivered. Factored out of `Drop` so the
/// graceful-shutdown step is unit-testable (US-001).
#[cfg(unix)]
fn terminate_process_group(pid: i32) -> bool {
    if !is_own_session_group(pid) {
        return false;
    }
    // SAFETY: kill(-pid, SIGTERM) signals every member of the group; FFI-safe
    // with the positive `pid` we just confirmed is our session leader.
    unsafe { libc::kill(-pid, libc::SIGTERM) == 0 }
}

/// EP-002 US-005: format a numeric signal (from alacritty's native
/// `ExitStatus::signal()`) as "N (Name)" for the exit overlay, e.g.
/// "11 (Segmentation fault)". The name comes from `strsignal`; the number is
/// authoritative (no reversal). Falls back to "signal N" when `strsignal` is
/// null for the signal.
#[cfg(unix)]
fn format_signal(sig: i32) -> String {
    // SAFETY: strsignal is a pure query; the returned C string is copied
    // immediately via CStr before any further libc call.
    let name = unsafe {
        let p = libc::strsignal(sig);
        if p.is_null() {
            None
        } else {
            Some(std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned())
        }
    };
    match name {
        Some(n) => format!("{sig} ({n})"),
        None => format!("signal {sig}"),
    }
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if let Some(ghostty) = self.ghostty.take() {
            ghostty.shutdown();
            self.child_pid = 0;
        }
        self.notifier.0.shutdown();

        // US-034: close the dup'd PTY master fd we own (macOS). Done exactly
        // once here - the fd was duplicated at spawn so `tcgetpgrp` stayed
        // valid for this session's lifetime independent of the EventLoop's
        // own copy.
        #[cfg(target_os = "macos")]
        if let Some(fd) = self.pty_master_fd.take() {
            // SAFETY: `fd` is our owned dup; close it once.
            unsafe {
                libc::close(fd);
            }
        }

        // Grace period + force-kill: if the child process ignores the PTY
        // master close signal (SIGHUP on Unix, ClosePseudoConsole on Windows),
        // force-kill it after 100ms.
        //
        // Scheduling: prefer the GPUI `background_executor` (Zed parity:
        // `crates/terminal/src/terminal.rs:2451-2457`) so the kill timer
        // lives under the GPUI runtime and gets cleanly torn down with
        // the app. Tests / display-only paths have no executor wired and
        // fall back to a detached OS thread (safe but un-trackable).
        let executor = self.background_executor.clone();

        #[cfg(unix)]
        {
            let pid = self.child_pid as i32;
            // US-034: skip the kill ladder entirely once the child has exited.
            // `child_pid` may have been reused by the OS by now, and signaling
            // a reused PGID would terminate an unrelated process group - the
            // synchronous SIGTERM below has the same PID-reuse window as the
            // delayed SIGKILL. An already-exited child has nothing to kill.
            if pid > 0 && self.exited.is_none() {
                // US-001: graceful shutdown ladder - send SIGTERM to the group
                // synchronously FIRST so agents/shells run their TERM handlers
                // (state checkpoint, HISTFILE flush) before the 100ms-grace
                // SIGKILL escalation below. Mirrors Zed's
                // terminate_child_process() -> 100ms -> kill_child_process()
                // (crates/terminal/src/terminal.rs:2697-2704, pty_info.rs:142-151).
                terminate_process_group(pid);

                let kill = move || {
                    // Target the entire process group (`-pid`) so any
                    // sub-process the shell forked (cargo build, npm dev,
                    // long-running scripts) dies with the shell instead of
                    // becoming an orphan reparented to PID 1. alacritty's
                    // `tty::new` calls `setsid()` on the child, so `child_pid`
                    // is both the PID and the PGID of the session leader -
                    // `kill(-pgid, sig)` is the canonical POSIX idiom to signal
                    // every process in that group.
                    //
                    // Re-check identity at fire time: 100ms after the child
                    // died the kernel may have recycled `pid` onto an unrelated
                    // group, so confirm `getpgid(pid) == pid` (our setsid
                    // session leader) before the SIGKILL - a bare `kill(-pid,0)`
                    // existence probe would not catch the reuse.
                    if is_own_session_group(pid) {
                        // SAFETY: kill(-pid, SIGKILL) signals every member of
                        // the process group; FFI-safe with the positive `pid`
                        // captured by value and just confirmed to be ours.
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                        }
                    }
                };
                match executor {
                    Some(bg) => {
                        bg.spawn(async move {
                            smol::Timer::after(std::time::Duration::from_millis(100)).await;
                            kill();
                        })
                        .detach();
                    }
                    None => {
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            kill();
                        });
                    }
                }
            }
        }

        #[cfg(windows)]
        {
            let pid = self.child_pid;
            // US-034 (mirrors the Unix path above): skip the kill ladder entirely
            // once the child has exited. Without this guard a normal exit (the
            // user typed `exit`) still ran the deferred `TerminateProcess`, which
            // fails with `ERROR_ACCESS_DENIED` on an already-dead process and
            // logged a spurious warning every time a shell closed.
            if pid != 0 && self.exited.is_none() {
                // US-001 asymmetry: Windows console processes have no
                // SIGTERM-equivalent graceful signal. TerminateProcess is a
                // hard kill and serves as the escalation; there is no Windows
                // mirror of the Unix synchronous-SIGTERM grace step above.
                let terminate = move || {
                    let started_at = std::time::Instant::now();
                    let deadline = started_at
                        .checked_add(WINDOWS_PROCESS_TREE_TERMINATION_BUDGET)
                        .unwrap_or(started_at);
                    let _ = terminate_windows_process_tree(pid, deadline);
                };
                match executor {
                    Some(bg) => {
                        bg.spawn(async move {
                            smol::Timer::after(std::time::Duration::from_millis(100)).await;
                            terminate();
                        })
                        .detach();
                    }
                    None => {
                        std::thread::spawn(move || {
                            std::thread::sleep(std::time::Duration::from_millis(100));
                            terminate();
                        });
                    }
                }
            }
        }
    }
}

#[cfg(windows)]
pub(super) const WINDOWS_PROCESS_TREE_TERMINATION_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(5);

/// Metadata-only outcome of one pane-scoped Windows process-tree cleanup.
/// Counts describe Win32 operations, never process command lines or arguments.
#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct WindowsProcessTreeTerminationResult {
    pub(super) targeted: usize,
    pub(super) terminate_requested: usize,
    pub(super) already_exited: usize,
    pub(super) failures: usize,
    pub(super) timed_out: usize,
    pub(super) deadline_exhausted: bool,
}

#[cfg(windows)]
fn windows_process_entries() -> io::Result<Vec<(u32, u32)>> {
    use std::mem;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };

    // SAFETY: Win32 call; the returned snapshot handle is closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    // Collect (pid, parent_pid) for every process in the snapshot.
    let mut entries: Vec<(u32, u32)> = Vec::with_capacity(256);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: `snap` is valid; `entry` is correctly sized and zeroed.
    if unsafe { Process32FirstW(snap, &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        // SAFETY: `snap` is a valid handle obtained above.
        unsafe { CloseHandle(snap) };
        return Err(error);
    }
    loop {
        entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: same invariants; iterates until the snapshot is exhausted.
        if unsafe { Process32NextW(snap, &mut entry) } == 0 {
            break;
        }
    }
    // SAFETY: `snap` is a valid handle obtained above.
    unsafe { CloseHandle(snap) };
    Ok(entries)
}

#[cfg(windows)]
fn windows_descendants_postorder(root_pid: u32, entries: &[(u32, u32)]) -> Vec<u32> {
    fn visit(
        pid: u32,
        entries: &[(u32, u32)],
        seen: &mut std::collections::HashSet<u32>,
        out: &mut Vec<u32>,
    ) -> bool {
        if !seen.insert(pid) {
            return false;
        }
        let mut children: Vec<u32> = entries
            .iter()
            .filter_map(|(child, parent)| (*parent == pid).then_some(*child))
            .collect();
        children.sort_unstable();
        for child in children {
            if visit(child, entries, seen, out) {
                out.push(child);
            }
        }
        true
    }

    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let _ = visit(root_pid, entries, &mut seen, &mut out);
    out
}

#[cfg(windows)]
fn windows_process_tree_targets(
    root_pid: u32,
    entries: &[(u32, u32)],
    include_root: bool,
) -> Vec<u32> {
    let mut targets = windows_descendants_postorder(root_pid, entries);
    if include_root && root_pid != 0 {
        targets.push(root_pid);
    }
    targets
}

/// Convert an absolute-deadline remainder to the largest Win32 millisecond
/// wait that cannot exceed it. A sub-millisecond remainder is intentionally
/// unusable: rounding up would break the single global budget.
#[cfg(windows)]
fn windows_wait_timeout_ms(remaining: std::time::Duration) -> Option<u32> {
    // `u32::MAX` is Win32's INFINITE sentinel, so the largest finite wait is
    // one millisecond below it.
    const MAX_FINITE_WAIT_MS: u32 = u32::MAX - 1;
    let milliseconds = remaining.as_millis().min(u128::from(MAX_FINITE_WAIT_MS)) as u32;
    (milliseconds != 0).then_some(milliseconds)
}

#[cfg(windows)]
struct WindowsTerminationHandle {
    pid: u32,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl Drop for WindowsTerminationHandle {
    fn drop(&mut self) {
        // SAFETY: this guard owns one non-null OpenProcess handle.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn request_windows_pid_termination(
    pid: u32,
    result: &mut WindowsProcessTreeTerminationResult,
) -> Option<WindowsTerminationHandle> {
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    // SYNCHRONIZE access right is required for WaitForSingleObject on the
    // returned handle; PROCESS_TERMINATE alone makes WaitForSingleObject
    // return WAIT_FAILED. Value mirrors winnt.h (0x0010_0000). Declared
    // locally to avoid pulling the Win32_Storage_FileSystem feature flag.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    // SAFETY: the returned handle is immediately wrapped in an owning guard.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        let os_error = io::Error::last_os_error().raw_os_error();
        result.failures = result.failures.saturating_add(1);
        log::debug!(
            "paneflow: Windows pane cleanup could not open pid={pid} (os_error={os_error:?})"
        );
        return None;
    }
    let process = WindowsTerminationHandle { pid, handle };

    // The process can still exit during the 100ms grace window. Confirm that
    // state before requesting a hard kill so an ordinary exit is not a failure.
    // SAFETY: `process` owns a valid synchronizable process handle.
    match unsafe { WaitForSingleObject(process.handle, 0) } {
        WAIT_OBJECT_0 => {
            result.already_exited = result.already_exited.saturating_add(1);
            return None;
        }
        WAIT_TIMEOUT => {}
        WAIT_FAILED => {
            let os_error = io::Error::last_os_error().raw_os_error();
            result.failures = result.failures.saturating_add(1);
            log::warn!(
                "paneflow: Windows pane cleanup precheck failed for pid={pid} (os_error={os_error:?})"
            );
        }
        status => {
            result.failures = result.failures.saturating_add(1);
            log::warn!(
                "paneflow: Windows pane cleanup precheck returned status={status:#x} for pid={pid}"
            );
        }
    }

    // SAFETY: `process.handle` carries PROCESS_TERMINATE access for this PID.
    if unsafe { TerminateProcess(process.handle, 1) } == 0 {
        let terminate_error = io::Error::last_os_error().raw_os_error();
        // A process may exit between the zero-timeout precheck and this call.
        // Treat that confirmed race as an ordinary already-exited outcome.
        // SAFETY: `process` still owns the same valid synchronizable handle.
        let exited = unsafe { WaitForSingleObject(process.handle, 0) } == WAIT_OBJECT_0;
        if exited {
            result.already_exited = result.already_exited.saturating_add(1);
        } else {
            result.failures = result.failures.saturating_add(1);
            log::debug!(
                "paneflow: Windows pane cleanup could not terminate pid={pid} (os_error={terminate_error:?})"
            );
        }
        return None;
    }

    result.terminate_requested = result.terminate_requested.saturating_add(1);
    Some(process)
}

#[cfg(windows)]
fn wait_for_windows_terminations(
    handles: Vec<WindowsTerminationHandle>,
    deadline: std::time::Instant,
    result: &mut WindowsProcessTreeTerminationResult,
) {
    use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    if handles.is_empty() {
        return;
    }

    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if windows_wait_timeout_ms(remaining).is_none() {
        result.deadline_exhausted = true;
        result.timed_out = result.timed_out.saturating_add(handles.len());
        return;
    }

    // Observe every process once without blocking before allowing one slow PID
    // to consume the shared deadline. All TerminateProcess requests have
    // already been issued, so this pass only improves result accuracy.
    let mut pending = Vec::with_capacity(handles.len());
    for process in handles {
        // SAFETY: every guard owns a valid synchronizable process handle.
        match unsafe { WaitForSingleObject(process.handle, 0) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => pending.push(process),
            WAIT_FAILED => {
                let os_error = io::Error::last_os_error().raw_os_error();
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow: Windows pane cleanup wait failed for pid={} (os_error={os_error:?})",
                    process.pid
                );
            }
            status => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow: Windows pane cleanup wait returned status={status:#x} for pid={}",
                    process.pid
                );
            }
        }
    }

    let pending_count = pending.len();
    for (index, process) in pending.into_iter().enumerate() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(timeout_ms) = windows_wait_timeout_ms(remaining) else {
            result.deadline_exhausted = true;
            result.timed_out = result
                .timed_out
                .saturating_add(pending_count.saturating_sub(index));
            break;
        };

        // SAFETY: `process` owns a valid synchronizable process handle, and
        // `timeout_ms` is finite and no larger than the deadline remainder.
        match unsafe { WaitForSingleObject(process.handle, timeout_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                // This PID consumed every usable whole millisecond. The rest
                // have already received TerminateProcess, but cannot be waited
                // independently without exceeding the one shared deadline.
                result.deadline_exhausted = true;
                result.timed_out = result
                    .timed_out
                    .saturating_add(pending_count.saturating_sub(index));
                break;
            }
            WAIT_FAILED => {
                let os_error = io::Error::last_os_error().raw_os_error();
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow: Windows pane cleanup wait failed for pid={} (os_error={os_error:?})",
                    process.pid
                );
            }
            status => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow: Windows pane cleanup wait returned status={status:#x} for pid={}",
                    process.pid
                );
            }
        }
    }
}

/// Best-effort hard cleanup for one Windows pane process tree.
///
/// Descendants discovered from each pane-rooted snapshot are targeted in
/// postorder, and the root is targeted exactly once after the first snapshot.
/// Every blocking wait shares the caller-supplied absolute `deadline`; later
/// PIDs never receive a fresh per-process timeout. Snapshot, OpenProcess, and
/// wait failures are recorded while cleanup continues where possible.
#[cfg(windows)]
pub(super) fn terminate_windows_process_tree(
    root_pid: u32,
    deadline: std::time::Instant,
) -> WindowsProcessTreeTerminationResult {
    let mut result = WindowsProcessTreeTerminationResult::default();
    if root_pid == 0 {
        return result;
    }

    const KILL_PASSES: usize = 3;
    let mut targeted = std::collections::HashSet::new();
    let mut handles = Vec::new();
    for pass in 0..KILL_PASSES {
        // Always run the first pass so an expired deadline still issues the
        // root hard-kill request. The deadline bounds every blocking wait and
        // prevents optional race-catching snapshots from extending cleanup.
        if pass > 0 && std::time::Instant::now() >= deadline {
            result.deadline_exhausted = true;
            break;
        }

        let (entries, snapshot_failed) = match windows_process_entries() {
            Ok(entries) => (entries, false),
            Err(error) => {
                result.failures = result.failures.saturating_add(1);
                log::warn!(
                    "paneflow: Windows pane cleanup snapshot failed for root_pid={root_pid} (os_error={:?})",
                    error.raw_os_error()
                );
                (Vec::new(), true)
            }
        };
        let targets = windows_process_tree_targets(root_pid, &entries, pass == 0);
        let had_descendants = targets.iter().any(|pid| *pid != root_pid);
        for pid in targets {
            if !targeted.insert(pid) {
                continue;
            }
            result.targeted = result.targeted.saturating_add(1);
            if let Some(handle) = request_windows_pid_termination(pid, &mut result) {
                handles.push(handle);
            }
        }

        // A successful follow-up snapshot with no descendants proves there is
        // nothing left to discover. A failed snapshot gets one final retry.
        if pass > 0 && !snapshot_failed && !had_descendants {
            break;
        }
    }

    wait_for_windows_terminations(handles, deadline, &mut result);
    if result.failures != 0 || result.timed_out != 0 {
        log::warn!(
            "paneflow: Windows pane cleanup incomplete (root_pid={root_pid}, targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
            result.targeted,
            result.terminate_requested,
            result.already_exited,
            result.failures,
            result.timed_out,
            result.deadline_exhausted
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn platform_sep() -> char {
        if cfg!(windows) { ';' } else { ':' }
    }

    #[test]
    fn display_only_sender_is_inert() {
        // US-011: the DisplayOnly variant of TerminalType has no EventLoop, so
        // write/resize/shutdown are dropped (no panic) and `is_pty` is false.
        // This is the spawn-failure fallback's write side - input must never
        // reach a child that doesn't exist.
        let s = PtySender::display_only();
        assert!(!s.is_pty());
        s.write(b"echo hi\n".as_slice().into());
        s.resize(AlacWindowSize {
            num_cols: 80,
            num_lines: 24,
            cell_width: 0,
            cell_height: 0,
        });
        s.shutdown();
    }

    #[test]
    fn new_display_only_terminal_has_no_pty() {
        // US-011: the spawn-failure fallback builds a DisplayOnly terminal; its
        // notifier must report no PTY so the input path drops bytes.
        let state = TerminalState::new_display_only(24, 80);
        assert!(!state.notifier.0.is_pty());
    }

    #[test]
    fn new_pending_terminal_starts_display_only_then_promotes_conceptually() {
        // US-012: a pending terminal is display-only (no PTY) until promoted.
        // (Promotion needs a real EventLoop channel, exercised by the live
        // `eventloop_pty_echoes_input_into_grid` smoke via the synchronous
        // `new`, which composes new_pending + open_pty_and_eventloop + promote.)
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        assert!(!state.notifier.0.is_pty());
        assert_eq!(state.child_pid, 0);
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    #[test]
    fn ghostty_fallback_preserves_the_auto_request_in_structured_diagnostics() {
        let mut state = TerminalState::new_display_only(24, 80);
        state.set_backend_request(TerminalBackendConfig::Auto);
        state.effective_backend = "ghostty";

        state.fallback_from_ghostty(TerminalBackendFailureDiagnostics::new(
            TerminalBackendFailurePhase::Initialization,
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            Some(5),
        ));

        let diagnostics = state.backend_diagnostics();
        assert_eq!(diagnostics.requested, TerminalBackendConfig::Auto);
        assert_eq!(diagnostics.effective, "alacritty");
        assert_eq!(
            diagnostics.failure,
            Some(TerminalBackendFailureDiagnostics::new(
                TerminalBackendFailurePhase::Initialization,
                TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
                Some(5),
            ))
        );
    }

    #[test]
    fn backend_diagnostics_extract_os_codes_without_sensitive_error_text() {
        const CANARY: &str =
            r#"C:\Users\synthetic-user\private\launch.ps1 --token super-secret-canary"#;
        let error = anyhow::Error::new(io::Error::from_raw_os_error(5)).context(CANARY);
        let os_error = raw_os_error_from_anyhow(&error);
        assert_eq!(os_error, Some(5));

        let mut state = TerminalState::new_display_only(24, 80);
        state.set_backend_request(TerminalBackendConfig::Ghostty);
        state.record_backend_failure(TerminalBackendFailureDiagnostics::new(
            TerminalBackendFailurePhase::OpenPty,
            TerminalBackendFailureDiagnostics::GHOSTTY_OPEN_PTY_FAILED,
            os_error,
        ));

        let formatted = state.backend_diagnostics().to_string();
        assert!(formatted.contains("failure_phase=open_pty"));
        assert!(formatted.contains("reason_code=ghostty_open_pty_failed"));
        assert!(formatted.contains("os_error=5"));
        assert!(!formatted.contains(CANARY));
        assert!(!formatted.contains("private"));
        assert!(!formatted.contains("super-secret-canary"));
    }

    #[test]
    fn backend_failure_phases_and_reason_codes_are_stable() {
        assert_eq!(
            TerminalBackendFailurePhase::Availability.as_str(),
            "availability"
        );
        assert_eq!(
            TerminalBackendFailurePhase::Initialization.as_str(),
            "initialization"
        );
        assert_eq!(TerminalBackendFailurePhase::OpenPty.as_str(), "open_pty");
        assert_eq!(TerminalBackendFailurePhase::Spawn.as_str(), "spawn");
        assert_eq!(
            TerminalBackendFailurePhase::PostSpawn.as_str(),
            "post_spawn"
        );
        assert_eq!(
            TerminalBackendFailureDiagnostics::GHOSTTY_UNAVAILABLE,
            "ghostty_unavailable"
        );
        assert_eq!(
            TerminalBackendFailureDiagnostics::GHOSTTY_POST_SPAWN_FAILED,
            "ghostty_post_spawn_failed"
        );
    }

    #[test]
    fn backend_diagnostics_expose_target_triple() {
        let diagnostics = TerminalState::new_display_only(24, 80).backend_diagnostics();
        assert_eq!(diagnostics.target_triple, env!("PANEFLOW_TARGET_TRIPLE"));
        #[cfg(all(target_os = "windows", target_arch = "x86_64", target_env = "msvc"))]
        assert_eq!(diagnostics.target_triple, "x86_64-pc-windows-msvc");
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    #[test]
    fn backend_diagnostics_expose_pinned_ghostty_build_identity() {
        let diagnostics = TerminalState::new_display_only(24, 80).backend_diagnostics();
        let ghostty = diagnostics.ghostty.expect("Ghostty build identity");
        let identity = paneflow_terminal_ghostty::build_identity();
        assert_eq!(
            ghostty.version,
            paneflow_terminal_ghostty::GHOSTTY_APP_VERSION
        );
        assert_eq!(ghostty.source_sha, identity.source_sha);
        assert_eq!(ghostty.api_version, identity.api_version);
        assert_eq!(ghostty.zig_version, identity.zig_version);
        assert_eq!(ghostty.optimization, identity.optimization);
        assert_eq!(ghostty.simd, identity.simd);
    }

    #[test]
    fn write_to_pty_buffers_input_while_display_only() {
        // US-012 regression: the launch pad's agent picker writes the
        // launch command the instant a terminal mounts - before the off-thread
        // fork promotes the PTY. The display-only notifier drops writes, so
        // without this queue the command (e.g. `claude`) is lost and the
        // terminal opens to a bare shell. `write_to_pty` must buffer instead.
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        assert!(!state.notifier.0.is_pty());
        state.write_to_pty(b"claude\r".to_vec());
        let queued = state.pending_input.lock().expect("pending_input lock");
        assert_eq!(queued.len(), 1);
        assert!(
            matches!(&queued[0], PendingTerminalInput::Raw(bytes) if bytes.as_ref() == b"claude\r")
        );
    }

    #[test]
    fn pending_input_is_bounded() {
        // A terminal that never promotes (spawn failure) must not accumulate
        // input without bound: writes past the cap are dropped, not queued.
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        let chunk = vec![b'x'; 8 * 1024];
        for _ in 0..(MAX_PENDING_INPUT_BYTES / chunk.len() + 2) {
            state.write_to_pty(chunk.clone());
        }
        let queued: usize = state
            .pending_input
            .lock()
            .expect("pending_input lock")
            .iter()
            .map(PendingTerminalInput::queued_bytes)
            .sum();
        assert!(
            queued <= MAX_PENDING_INPUT_BYTES,
            "buffered {queued} bytes exceeds the {MAX_PENDING_INPUT_BYTES} cap"
        );
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn test_key_input(
        action: paneflow_terminal_ghostty::KeyAction,
    ) -> paneflow_terminal_ghostty::KeyInput {
        paneflow_terminal_ghostty::KeyInput {
            key: paneflow_terminal_ghostty::Key::Function(5),
            action,
            modifiers: paneflow_terminal_ghostty::Modifiers::CONTROL,
            consumed_modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
            text: String::new(),
            unshifted_codepoint: None,
            composing: false,
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    fn test_mouse_input(
        action: paneflow_terminal_ghostty::MouseAction,
    ) -> paneflow_terminal_ghostty::MouseInput {
        paneflow_terminal_ghostty::MouseInput {
            action,
            button: Some(paneflow_terminal_ghostty::MouseButton::Left),
            modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
            x: 8.0,
            y: 16.0,
            screen_width: 640,
            screen_height: 384,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: action != paneflow_terminal_ghostty::MouseAction::Release,
        }
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    #[test]
    fn control_releases_fit_after_general_input_saturates() {
        let general_limit = MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES;
        let press = PendingTerminalInput::Key {
            input: test_key_input(paneflow_terminal_ghostty::KeyAction::Press),
            legacy: None,
        };
        let key_release = PendingTerminalInput::Key {
            input: test_key_input(paneflow_terminal_ghostty::KeyAction::Release),
            legacy: None,
        };
        let mouse_release = PendingTerminalInput::Mouse {
            input: test_mouse_input(paneflow_terminal_ghostty::MouseAction::Release),
            repeat: 1,
            legacy: Some(b"mouse-up".to_vec()),
        };
        let focus = PendingTerminalInput::Focus(paneflow_terminal_ghostty::FocusEvent::Lost);

        assert!(!press.fits_after(general_limit));
        assert!(key_release.fits_after(general_limit));
        assert!(mouse_release.fits_after(general_limit));
        assert!(focus.fits_after(general_limit));
    }

    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    #[test]
    fn ghostty_startup_fallback_preserves_structured_input_in_order() {
        let (mut state, _alacritty_pending) = TerminalState::new_pending(80, 24);
        let (ghostty, _runtime_pending, events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 0, 0));
        state.attach_ghostty(ghostty, events_rx);

        assert_eq!(
            state.write_ghostty_key(
                test_key_input(paneflow_terminal_ghostty::KeyAction::Press),
                Some(Cow::Borrowed(b"\x1b[15~")),
            ),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_mouse(
                test_mouse_input(paneflow_terminal_ghostty::MouseAction::Press),
                2,
                Some(b"M".to_vec()),
            ),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_focus(paneflow_terminal_ghostty::FocusEvent::Gained),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_paste("paste".to_string(), true),
            BackendInputResult::Accepted
        );

        state.fallback_from_ghostty(TerminalBackendFailureDiagnostics::new(
            TerminalBackendFailurePhase::Initialization,
            TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
            None,
        ));
        let legacy: Vec<Vec<u8>> = state
            .drain_pending_legacy_input()
            .into_iter()
            .map(Cow::into_owned)
            .collect();
        assert_eq!(
            legacy,
            vec![
                b"\x1b[15~".to_vec(),
                b"MM".to_vec(),
                b"\x1b[I".to_vec(),
                b"\x1b[200~paste\x1b[201~".to_vec(),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn promote_flushes_buffered_input_into_grid() {
        // US-012 end-to-end: input written while display-only must reach the
        // child after promotion. Mirrors the synchronous `new` composition
        // (new_pending + open_pty_and_eventloop + promote) but injects a write
        // *between* new_pending and promote - the exact auto-launch ordering.
        let params = TerminalState::resolve_spawn_params(None, 1, 1, Some((80, 24)), None);
        let (mut state, pending) = TerminalState::new_pending(params.cols, params.rows);
        // Buffered while display-only - the live notifier does not exist yet.
        state.write_to_pty(b"echo PANEFLOW_FLUSH_OK\n".to_vec());
        assert!(!state.notifier.0.is_pty());

        let spawned = TerminalState::open_pty_and_eventloop(params, pending, None)
            .expect("US-012: open a PTY-backed terminal via tty::new + EventLoop");
        state.promote(spawned);
        assert!(state.notifier.0.is_pty());

        let mut found = false;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            state.sync();
            if grid_to_string(&state.term).contains("PANEFLOW_FLUSH_OK") {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "US-012: input buffered before promotion never reached the shell"
        );
    }

    #[test]
    fn resolve_spawn_params_honors_initial_size() {
        // US-012: the cheap, render-thread-safe half of a spawn picks up the
        // requested grid size (and the 120x40 default when unspecified).
        let p = TerminalState::resolve_spawn_params(None, 1, 1, Some((100, 30)), None);
        assert_eq!((p.cols, p.rows), (100, 30));
        let d = TerminalState::resolve_spawn_params(None, 1, 1, None, None);
        assert_eq!((d.cols, d.rows), (120, 40));
    }

    #[cfg(unix)]
    #[test]
    fn capture_foreground_signal_mask_succeeds_on_unix() {
        // US-012: the foreground mask snapshot must succeed on the main thread
        // so the off-thread spawn can hand it to the child (Ctrl-C parity).
        assert!(capture_foreground_signal_mask().is_some());
    }

    #[test]
    fn cursor_shape_maps_hollow_to_hollow_block() {
        // US-007: config shapes map to renderer (vte) shapes; the config's
        // `Hollow` maps to the renderer's `HollowBlock`.
        use alacritty_terminal::vte::ansi::CursorShape;
        use paneflow_config::schema::CursorShapeConfig as C;
        assert_eq!(map_cursor_shape(C::Vintage), CursorShape::Block);
        assert_eq!(map_cursor_shape(C::Block), CursorShape::Block);
        assert_eq!(map_cursor_shape(C::Beam), CursorShape::Beam);
        assert_eq!(map_cursor_shape(C::Underline), CursorShape::Underline);
        assert_eq!(map_cursor_shape(C::DoubleUnderline), CursorShape::Underline);
        assert_eq!(map_cursor_shape(C::Hollow), CursorShape::HollowBlock);
    }

    #[test]
    fn prepend_puts_bin_dir_first_and_preserves_existing_entries() {
        let mut env: HashMap<String, String> = HashMap::new();
        let sep = platform_sep();
        env.insert("PATH".into(), format!("/usr/bin{sep}/usr/local/bin"));

        let bin_dir = PathBuf::from("/home/u/.cache/paneflow/bin/0.2.6");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009 AC: bin_dir must be first on PATH; got {components:?}"
        );
        assert!(
            components.iter().any(|p| p == Path::new("/usr/bin")),
            "US-009: original PATH entries must be preserved; got {components:?}"
        );
        assert!(
            components.iter().any(|p| p == Path::new("/usr/local/bin")),
            "US-009: original PATH entries must be preserved; got {components:?}"
        );
    }

    #[test]
    fn prepend_inserts_bin_dir_even_when_env_path_absent() {
        // AC: "If env map has no PATH, helper still sets PATH so the
        // child inherits the shim dir rather than silently no-op."
        let mut env: HashMap<String, String> = HashMap::new();
        let bin_dir = PathBuf::from("/tmp/paneflow-bins");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009: bin_dir must be first on PATH in the no-prior-PATH case"
        );
    }

    #[test]
    fn prepend_uses_platform_separator() {
        // Round-trip invariant: split_paths(join_paths(X)) == X. This
        // implicitly tests that `;` on Windows / `:` on Unix is handled
        // correctly - we do not assert the raw bytes because that
        // would hardcode per-OS expectations.
        let mut env: HashMap<String, String> = HashMap::new();
        let sep = platform_sep();
        env.insert("PATH".into(), format!("/a{sep}/b{sep}/c"));
        let bin_dir = PathBuf::from("/z");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").unwrap();
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert_eq!(
            components,
            vec![
                PathBuf::from("/z"),
                PathBuf::from("/a"),
                PathBuf::from("/b"),
                PathBuf::from("/c"),
            ],
            "US-009: split_paths(join_paths(...)) must round-trip on all platforms"
        );
    }

    #[test]
    fn prepend_treats_empty_path_as_absent() {
        // An empty `PATH` is not absent - `split_paths("")` on Unix
        // yields one `PathBuf::from("")` component that `execvp`
        // resolves as the CWD. We must NOT inherit that phantom entry
        // onto the child's PATH (shell-injection surface).
        let mut env: HashMap<String, String> = HashMap::new();
        env.insert("PATH".into(), String::new());
        let bin_dir = PathBuf::from("/z");
        prepend_bin_dir_to_path(&mut env, &bin_dir);

        let joined = env.get("PATH").expect("PATH set by helper");
        let components: Vec<PathBuf> = std::env::split_paths(joined).collect();
        assert!(
            !components.iter().any(|p| p.as_os_str().is_empty()),
            "US-009 hardening: empty PATH must not yield a phantom CWD entry; got {components:?}"
        );
        assert_eq!(
            components.first(),
            Some(&bin_dir),
            "US-009: bin_dir must still be first when empty PATH is treated as absent"
        );
    }

    // -----------------------------------------------------------------
    // US-003 - exit-status correctness (real code, first-write-wins).
    // -----------------------------------------------------------------

    #[test]
    fn child_exit_records_real_code_not_sentinel() {
        // US-003 AC: a real child exit code must round-trip into `exited`,
        // not the -1 fallback. The status is built the same way
        // `pty_reader_loop` builds it from `child.wait()` per platform, so
        // this exercises the Windows path on the Windows CI leg and the Unix
        // path elsewhere.
        let mut state = TerminalState::new_display_only(24, 80);
        assert!(state.exited.is_none(), "fresh terminal has no exit code");

        #[cfg(unix)]
        let status: std::process::ExitStatus =
            std::os::unix::process::ExitStatusExt::from_raw(42 << 8);
        #[cfg(windows)]
        let status: std::process::ExitStatus =
            std::os::windows::process::ExitStatusExt::from_raw(42u32);

        #[cfg(any(unix, windows))]
        {
            state.process_event(AlacEvent::ChildExit(status));
            assert_eq!(
                state.exited,
                Some(42),
                "US-003: the real exit code must be recorded, not -1"
            );
        }
    }

    #[test]
    fn exit_fallback_does_not_clobber_real_child_exit_code() {
        // US-003 AC: first-write-wins. A bare `Exit` (EOF, no status) must
        // never overwrite a real code already recorded by `ChildExit`.
        let mut state = TerminalState::new_display_only(24, 80);

        #[cfg(unix)]
        let status: std::process::ExitStatus =
            std::os::unix::process::ExitStatusExt::from_raw(1 << 8);
        #[cfg(windows)]
        let status: std::process::ExitStatus =
            std::os::windows::process::ExitStatusExt::from_raw(1u32);

        #[cfg(any(unix, windows))]
        {
            state.process_event(AlacEvent::ChildExit(status));
            state.process_event(AlacEvent::Exit);
            assert_eq!(
                state.exited,
                Some(1),
                "US-003: Exit must not clobber the real ChildExit code"
            );
        }
    }

    // -----------------------------------------------------------------
    // US-002 - keep pane open on launch failure (keyboard_input_sent).
    // -----------------------------------------------------------------

    #[test]
    fn close_on_exit_discriminator_covers_both_branches() {
        // US-002 AC: clean exit (code 0) closes even with no input.
        let mut clean = TerminalState::new_display_only(24, 80);
        clean.exited = Some(0);
        assert!(
            clean.should_close_on_exit(),
            "US-002: a clean exit (code 0) must close the pane"
        );

        // Non-zero exit with NO user input = spawn/launch failure → stays open
        // so the exit overlay can render the code.
        let mut failed = TerminalState::new_display_only(24, 80);
        failed.exited = Some(127);
        assert!(
            !failed.should_close_on_exit(),
            "US-002: a non-zero exit with no input must keep the pane open"
        );

        // ...but once the user has interacted, ANY exit closes.
        failed.write_to_pty(b"x".as_slice());
        assert!(
            failed.should_close_on_exit(),
            "US-002: after user input, a non-zero exit must close the pane"
        );
    }

    #[test]
    fn write_to_pty_marks_user_input_but_fresh_state_does_not() {
        // US-002: a fresh terminal has not received user input; write_to_pty
        // flips the flag. (Automated writes use notifier.notify and are tested
        // implicitly by the discriminator staying false here.)
        let state = TerminalState::new_display_only(24, 80);
        assert!(
            !state
                .keyboard_input_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            "fresh terminal must report no user input"
        );
        state.write_to_pty(b"a".as_slice());
        assert!(
            state
                .keyboard_input_sent
                .load(std::sync::atomic::Ordering::Relaxed),
            "write_to_pty must mark the session user-initiated"
        );
    }

    // -----------------------------------------------------------------
    // US-001 - graceful teardown sends SIGTERM before SIGKILL.
    // -----------------------------------------------------------------

    #[cfg(unix)]
    #[test]
    fn terminate_process_group_delivers_sigterm_and_is_honored() {
        // US-001 AC: the process group receives SIGTERM (not a hard SIGKILL).
        // The child is its own session/group leader (setsid) and traps SIGTERM
        // to exit 42; a SIGKILL would instead show signal 9 with no exit code.
        // Proving the trap ran proves SIGTERM was delivered to the group - and
        // by construction `Drop` sends it synchronously *before* scheduling the
        // 100ms-grace SIGKILL.
        use std::io::{BufRead, BufReader};
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        use std::process::{Command, Stdio};
        use std::time::{Duration, Instant};

        // `sleep 30 &` + `wait` (not a foreground sleep): POSIX requires the
        // `wait` builtin to be interrupted by a trapped signal, so the trap
        // runs promptly even if the group SIGTERM races `sleep`'s fork→exec
        // window (where an inherited blocked mask can leave it alive - a
        // foreground sleep then pins the shell for its full 30s before the
        // trap fires, which is exactly the aarch64-CI hang this replaces).
        // `echo ready` is the readiness handshake: once the parent reads it,
        // setsid + trap + background spawn are all done - no blind warmup.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "trap 'exit 42' TERM; sleep 30 & echo ready; wait"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: setsid() runs in the forked child before exec; it detaches
        // the child into its own session/group so kill(-pid, ...) targets
        // exactly this group, with no shared-state hazard.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut child = cmd.spawn().expect("spawn test child");
        let pid = child.id() as i32;

        let mut ready = String::new();
        BufReader::new(child.stdout.take().expect("piped stdout"))
            .read_line(&mut ready)
            .expect("read readiness line");
        assert_eq!(ready.trim_end(), "ready", "handshake line");

        assert!(
            terminate_process_group(pid),
            "US-001: SIGTERM must be delivered to the live process group"
        );

        // The trap exits 42 well within the 100ms grace window; poll for exit
        // with a generous ceiling - the suite runs fully parallel on 4-core CI
        // runners and a 5s deadline has flaked under that load (same class as
        // the v0.3.9 stdout_cap deflake). A regression still fails, just slower.
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(status) = child.try_wait().expect("try_wait child") {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("US-001: child did not exit after SIGTERM within 30s");
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        assert_eq!(
            status.code(),
            Some(42),
            "US-001: child must exit via its SIGTERM handler (42), not be SIGKILLed (signal={:?})",
            status.signal()
        );
    }

    #[cfg(unix)]
    #[test]
    fn terminate_process_group_is_noop_for_dead_or_invalid_group() {
        // US-001 AC (unhappy path): an empty/invalid group must be a harmless
        // no-op guarded by the `getpgid(pid) == pid` identity check - no panic,
        // returns false.
        assert!(
            !terminate_process_group(0),
            "pid 0 must be rejected (would signal the caller's own group)"
        );
        assert!(
            !terminate_process_group(-5),
            "negative pid must be rejected"
        );
        // A very high pid is almost certainly not its own live group leader;
        // getpgid returns ESRCH (≠ pid) so SIGTERM is never sent.
        assert!(
            !terminate_process_group(0x7FFF_FFF0),
            "non-existent group must be a no-op, not a panic"
        );
    }

    // -----------------------------------------------------------------
    // Env assembly contract. EP-002 US-004 removed the mockable
    // `PtyBackend::spawn` seam (the IO core now opens the PTY via alacritty's
    // `tty::new`), so the env that the child inherits is asserted directly
    // against the pure `assemble_pty_env`.
    // -----------------------------------------------------------------

    #[test]
    fn wslenv_merge_preserves_existing_entries_and_deduplicates() {
        let merged = merge_wslenv(
            Some("EXISTING/p:ALREADY/u:CUSTOM/uw"),
            ["ZED", "ALREADY", "EXISTING", "ZED", "PANEFLOW_BIN_DIR"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("EXISTING/p:ALREADY/u:CUSTOM/uw:PANEFLOW_BIN_DIR/up:ZED/u")
        );
    }

    #[test]
    fn wslenv_merge_adds_u_when_w_is_one_way() {
        let merged = merge_wslenv(
            Some("FORWARD_ONLY/w:UNCHANGED/l"),
            ["FORWARD_ONLY", "UNCHANGED"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("FORWARD_ONLY/w:UNCHANGED/l:FORWARD_ONLY/u")
        );
    }

    #[test]
    fn wslenv_merge_adds_up_when_paneflow_paths_lack_path_flag() {
        let merged = merge_wslenv(
            Some("PANEFLOW_HOOK_LOG/u:PANEFLOW_BIN_DIR/u"),
            ["PANEFLOW_HOOK_LOG", "PANEFLOW_BIN_DIR"],
        );

        assert_eq!(
            merged.as_deref(),
            Some("PANEFLOW_HOOK_LOG/u:PANEFLOW_BIN_DIR/u:PANEFLOW_BIN_DIR/up:PANEFLOW_HOOK_LOG/up")
        );
    }

    #[test]
    fn wslenv_merge_skips_excluded_and_invalid_names() {
        let merged = merge_wslenv(
            None,
            [
                "PATH",
                "WSLENV",
                "SHLVL",
                "LANG",
                "9INVALID",
                "HAS-DASH",
                "NON_ASCII_é",
                "",
                "_ALSO_2",
                "GOOD_VAR",
            ],
        );

        assert_eq!(merged.as_deref(), Some("GOOD_VAR/u:_ALSO_2/u"));
    }

    #[test]
    fn wslenv_shell_detection_is_exact() {
        assert!(is_wsl_shell("wsl"));
        assert!(is_wsl_shell("WSL.EXE"));
        assert!(is_wsl_shell(r"C:\Windows\System32\wsl.exe"));
        assert!(!is_wsl_shell("pwsh.exe"));
        assert!(!is_wsl_shell("my-wsl.exe"));
    }

    #[test]
    fn pty_spawn_injects_paneflow_bin_dir_and_prepends_path() {
        // Skip where the cache dir is unresolvable - the helper silent-fails
        // (correct behavior), but then there's nothing to assert on.
        if dirs::cache_dir().is_none() {
            eprintln!("skip: dirs::cache_dir() unresolvable in this environment");
            return;
        }

        let env = assemble_pty_env(HashMap::new(), 7, 3, None);

        let bin_dir = env
            .get("PANEFLOW_BIN_DIR")
            .expect("US-009 AC: PANEFLOW_BIN_DIR must be set in the child env")
            .clone();
        assert!(
            !bin_dir.is_empty(),
            "US-009: PANEFLOW_BIN_DIR must not be empty"
        );

        let path = env
            .get("PATH")
            .expect("US-009 AC: PATH must be set after injection");
        let first = std::env::split_paths(path)
            .next()
            .expect("PATH must have at least one component");
        assert_eq!(
            first,
            PathBuf::from(&bin_dir),
            "US-009 AC: PANEFLOW_BIN_DIR must be first on PATH"
        );
    }

    #[test]
    fn detached_terminal_does_not_advertise_fake_workspace_id() {
        let env = assemble_pty_env(HashMap::new(), 0, 3, None);

        assert!(
            !env.contains_key("PANEFLOW_WORKSPACE_ID"),
            "workspace id 0 is a detached sentinel and must not reach child hooks"
        );
        assert_eq!(
            env.get("PANEFLOW_SURFACE_ID").map(String::as_str),
            Some("3")
        );
    }

    // US-014: user-supplied env vars are merged into the child PTY env.
    #[test]
    fn user_env_is_merged_into_pty_env() {
        let mut user = HashMap::new();
        user.insert("ANTHROPIC_API_KEY".to_string(), "sk-test-123".to_string());
        user.insert("MY_CUSTOM_VAR".to_string(), "hello".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("ANTHROPIC_API_KEY").map(String::as_str),
            Some("sk-test-123"),
            "US-014 AC: user env var must be present in the child env"
        );
        assert_eq!(
            env.get("MY_CUSTOM_VAR").map(String::as_str),
            Some("hello"),
            "US-014 AC: a second user env var must also be present"
        );
    }

    #[test]
    fn user_path_cannot_shadow_paneflow_bin_dir() {
        let mut user = HashMap::new();
        user.insert("PATH".to_string(), "/custom/bin".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));
        let Some(bin_dir) = env.get("PANEFLOW_BIN_DIR") else {
            eprintln!("skip: PANEFLOW_BIN_DIR unavailable in this environment");
            return;
        };
        let path = env.get("PATH").expect("PATH must be present");
        let mut parts = std::env::split_paths(path);
        assert_eq!(
            parts.next().as_deref(),
            Some(std::path::Path::new(bin_dir)),
            "PANEFLOW_BIN_DIR must stay first even when user env sets PATH"
        );
        assert!(
            parts.any(|part| part == std::path::Path::new("/custom/bin")),
            "user PATH entries must still be preserved after the shim prepend"
        );
    }

    // US-014: TERM/COLORTERM are protected and cannot be overridden by user env.
    #[test]
    fn protected_keys_cannot_be_overridden_by_user_env() {
        let mut user = HashMap::new();
        user.insert("TERM".to_string(), "dumb".to_string());
        user.insert("COLORTERM".to_string(), "nope".to_string());
        user.insert("TERM_PROGRAM".to_string(), "spoofed".to_string());
        user.insert("TERM_PROGRAM_VERSION".to_string(), "0.0.0".to_string());
        user.insert("SHLVL".to_string(), "99".to_string());
        user.insert("KEEP_ME".to_string(), "yes".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("TERM").map(String::as_str),
            Some("xterm-256color"),
            "US-014 AC: TERM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("COLORTERM").map(String::as_str),
            Some("truecolor"),
            "US-014 AC: COLORTERM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some("paneflow"),
            "TERM_PROGRAM must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("TERM_PROGRAM_VERSION").map(String::as_str),
            Some(env!("CARGO_PKG_VERSION")),
            "TERM_PROGRAM_VERSION must stay Paneflow-owned even if the user sets it"
        );
        assert_eq!(
            env.get("SHLVL").map(String::as_str),
            Some("0"),
            "SHLVL must stay reset so the child shell starts at level 1"
        );
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "US-014: a non-protected user var alongside protected ones still wins"
        );
    }

    // f010: dynamic-loader env vars from an untrusted source (imported
    // session.json surface env / global config env) must never reach the child
    // shell - letting LD_PRELOAD/LD_*/DYLD_* through is an RCE vector.
    #[test]
    fn loader_influencing_env_vars_are_dropped() {
        let mut user = HashMap::new();
        user.insert("LD_PRELOAD".to_string(), "/tmp/evil.so".to_string());
        user.insert("LD_LIBRARY_PATH".to_string(), "/tmp/evil".to_string());
        user.insert("LD_AUDIT".to_string(), "/tmp/audit.so".to_string());
        user.insert(
            "DYLD_INSERT_LIBRARIES".to_string(),
            "/tmp/e.dylib".to_string(),
        );
        user.insert("KEEP_ME".to_string(), "yes".to_string());
        let env = assemble_pty_env(HashMap::new(), 1, 1, Some(user));

        assert_eq!(
            env.get("LD_PRELOAD"),
            None,
            "f010: LD_PRELOAD from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("LD_LIBRARY_PATH"),
            None,
            "f010: LD_LIBRARY_PATH from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("LD_AUDIT"),
            None,
            "f010: LD_AUDIT from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("DYLD_INSERT_LIBRARIES"),
            None,
            "f010: DYLD_* from untrusted env must be dropped"
        );
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "f010: a benign var alongside loader vars must still pass through"
        );
    }

    #[test]
    fn claudecode_env_is_dropped_from_child_env() {
        let mut base = HashMap::new();
        base.insert("CLAUDECODE".to_string(), "1".to_string());
        let mut user = HashMap::new();
        user.insert("CLAUDECODE".to_string(), "1".to_string());
        user.insert("KEEP_ME".to_string(), "yes".to_string());

        let env = assemble_pty_env(base, 1, 1, Some(user));

        assert_eq!(
            env.get("CLAUDECODE"),
            None,
            "CLAUDECODE must never reach agent child processes"
        );
        assert_eq!(env.get("KEEP_ME").map(String::as_str), Some("yes"));
    }

    #[test]
    fn inherited_agent_session_markers_are_dropped_from_child_env() {
        // Launching Paneflow from inside an agent session (or from an IDE
        // terminal that carries these) otherwise leaks the parent session's
        // identity into every pane. `CLAUDE_CODE_CHILD_SESSION` in particular
        // makes the agent disable transcript saving, so its conversation never
        // reaches `~/.claude/projects` and the thread cannot be resumed after a
        // restart.
        let mut base = HashMap::new();
        let mut user = HashMap::new();
        for key in INHERITED_AGENT_SESSION_ENV {
            base.insert((*key).to_string(), "inherited".to_string());
            // Also offered through `terminal.env`: the strip runs after the
            // merge, so a hand-written config cannot reinstate them either.
            user.insert((*key).to_string(), "from-config".to_string());
        }
        user.insert("KEEP_ME".to_string(), "yes".to_string());

        let env = assemble_pty_env(base, 1, 1, Some(user));

        for key in INHERITED_AGENT_SESSION_ENV {
            assert_eq!(
                env.get(*key),
                None,
                "{key} must never reach an agent spawned in a pane"
            );
        }
        assert_eq!(
            env.get("KEEP_ME").map(String::as_str),
            Some("yes"),
            "a benign var alongside the markers must still pass through"
        );
    }

    // US-019: foreground_command degrades gracefully (no panic, None) on a
    // display-only terminal (child_pid == 0, no real PTY) on every platform.
    #[test]
    fn foreground_command_none_for_display_only() {
        let state = TerminalState::new_display_only(24, 80);
        assert!(
            state.foreground_command().is_none(),
            "display-only terminal has no foreground process to resolve"
        );
    }

    #[test]
    fn scan_output_uses_multiline_framework_context() {
        let mut state = TerminalState::new_display_only(24, 80);
        state.restore_scrollback("▲ Next.js 16.1.6\n- Local: http://localhost:3000\n");

        let services = state.scan_output();

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].port, 3000);
        assert_eq!(services[0].label.as_deref(), Some("Next.js"));
        assert!(services[0].is_frontend);
    }

    #[test]
    fn scan_output_dedups_until_port_leaves_live_set() {
        let mut state = TerminalState::new_display_only(24, 80);
        state.restore_scrollback("Vite ready at http://localhost:5173\n");

        assert_eq!(state.scan_output().len(), 1);
        assert!(state.scan_output().is_empty());

        state.retain_reported_ports(&[]);
        let services = state.scan_output();

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].port, 5173);
    }

    #[test]
    fn announced_ports_are_deduped_and_bounded() {
        let mut state = TerminalState::new_display_only(24, 80);
        state.note_announced_port(3000);
        state.note_announced_port(3000);
        for port in 3001..3025 {
            state.note_announced_port(port);
        }

        assert_eq!(state.announced_ports.len(), 16);
        assert_eq!(state.announced_ports[0], 3000);
        assert_eq!(
            state.announced_ports.iter().filter(|&&p| p == 3000).count(),
            1
        );
    }

    #[test]
    fn search_scrollback_returns_unique_lines_and_preserves_cap() {
        let state = TerminalState::new_display_only(5, 80);
        state.write_output(b"first needle needle\nsecond needle\nthird needle\nwithout marker");

        let (limited, hit_cap) = state.search_scrollback("needle", 2);
        assert_eq!(limited.len(), 2);
        assert!(hit_cap);
        assert!(limited[0].1.contains("first needle needle"));
        assert!(limited[1].1.contains("second needle"));

        let (all, hit_cap) = state.search_scrollback("needle", 8);
        assert_eq!(all.len(), 3);
        assert!(!hit_cap);
        assert!(all[2].1.contains("third needle"));
    }

    // A display-only terminal (child_pid == 0, no real PTY) must resolve no CWD
    // and, critically, must NOT reach the platform process-table FFI: on macOS
    // `proc_pidinfo(0, …)` targets the kernel swapper, fails with EPERM, and
    // would spam a misleading "shell may have exited" warning on every poll.
    #[test]
    fn cwd_now_none_for_display_only() {
        let state = TerminalState::new_display_only(24, 80);
        assert_eq!(state.child_pid, 0);
        assert!(
            state.cwd_now().is_none(),
            "display-only terminal has no shell CWD to resolve"
        );
    }

    fn drain_osc_filter(filter: &mut BoundedOscFilter) -> Vec<u8> {
        let mut output = vec![0; MAX_OSC_SEQUENCE_BYTES + 256];
        let written = filter.drain_into(&mut output);
        output.truncate(written);
        output
    }

    struct ChunkedTestPty {
        chunks: VecDeque<Vec<u8>>,
        writes: Vec<u8>,
        read_calls: usize,
    }

    impl Read for ChunkedTestPty {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.read_calls += 1;
            let Some(chunk) = self.chunks.pop_front() else {
                return Err(io::ErrorKind::WouldBlock.into());
            };
            let written = chunk.len().min(buf.len());
            buf[..written].copy_from_slice(&chunk[..written]);
            if written < chunk.len() {
                self.chunks.push_front(chunk[written..].to_vec());
            }
            Ok(written)
        }
    }

    impl Write for ChunkedTestPty {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl tty::EventedReadWrite for ChunkedTestPty {
        type Reader = Self;
        type Writer = Self;

        unsafe fn register(
            &mut self,
            _poller: &Arc<polling::Poller>,
            _event: polling::Event,
            _mode: polling::PollMode,
        ) -> io::Result<()> {
            Ok(())
        }

        fn reregister(
            &mut self,
            _poller: &Arc<polling::Poller>,
            _event: polling::Event,
            _mode: polling::PollMode,
        ) -> io::Result<()> {
            Ok(())
        }

        fn deregister(&mut self, _poller: &Arc<polling::Poller>) -> io::Result<()> {
            Ok(())
        }

        fn reader(&mut self) -> &mut Self::Reader {
            self
        }

        fn writer(&mut self) -> &mut Self::Writer {
            self
        }
    }

    impl tty::EventedPty for ChunkedTestPty {
        fn next_child_event(&mut self) -> Option<tty::ChildEvent> {
            None
        }
    }

    #[test]
    fn osc_filter_read_continues_after_a_fully_buffered_fragment() {
        let inner = ChunkedTestPty {
            chunks: VecDeque::from([b"\x1b]52;c;SGV".to_vec(), b"sbG8=\x07after".to_vec()]),
            writes: Vec::new(),
            read_calls: 0,
        };
        let (cwd_tx, _cwd_rx) = unbounded();
        let (marks_tx, _marks_rx) = std::sync::mpsc::sync_channel(4);
        let mut pty = Osc7Pty::new(inner, cwd_tx, marks_tx);
        let mut output = [0; 64];

        let read = pty.read(&mut output).expect("filtered PTY read");

        assert_eq!(&output[..read], b"\x1b]52;c;SGVsbG8=\x07after");
        assert_eq!(pty.inner.read_calls, 2);
    }

    #[test]
    fn bounded_osc_filter_preserves_fragmented_sequences_exactly() {
        let mut filter = BoundedOscFilter::default();
        let mut output = Vec::new();

        filter.advance(b"before\x1b]52;c;SGV");
        output.extend(drain_osc_filter(&mut filter));
        filter.advance(b"sbG8=\x1b");
        output.extend(drain_osc_filter(&mut filter));
        filter.advance(b"\\after");
        output.extend(drain_osc_filter(&mut filter));

        assert_eq!(output, b"before\x1b]52;c;SGVsbG8=\x1b\\after");
    }

    #[test]
    fn bounded_osc_filter_drops_oversized_sequences_before_vte() {
        let mut filter = BoundedOscFilter::default();
        filter.advance(b"before\x1b]0;");
        filter.advance(&vec![b'x'; MAX_OSC_SEQUENCE_BYTES + 1]);
        assert!(matches!(filter.state, BoundedOscState::Drop));
        filter.advance(b"\x07after");

        assert_eq!(drain_osc_filter(&mut filter), b"before\x18after");
        assert!(matches!(filter.state, BoundedOscState::Ground));
    }

    #[test]
    fn bounded_osc_filter_preserves_raw_c1_for_vte_7_bit_parser() {
        let input = b"\x1b[\xd1\x9dtext\x9c";
        let mut filter = BoundedOscFilter::default();
        for chunk in input.chunks(1) {
            filter.advance(chunk);
        }
        assert_eq!(drain_osc_filter(&mut filter), input);
    }

    #[test]
    fn osc7_scanner_extracts_bel_and_st_terminated_file_uris() {
        let mut scanner = Osc7Scanner::default();
        let mut seen = Vec::new();
        scanner.advance(b"pre\x1b]7;file:///tmp/project\x07post", |cwd| {
            seen.push(cwd)
        });
        scanner.advance(b"\x1b]7;file://host/home/me/project\x1b\\", |cwd| {
            seen.push(cwd)
        });

        assert_eq!(
            seen,
            vec!["/tmp/project".to_string(), "/home/me/project".to_string()]
        );
    }

    #[test]
    fn osc7_scanner_discards_oversized_payload_through_nested_sequence() {
        let mut scanner = Osc7Scanner::default();
        let mut seen = Vec::new();
        let mut input = b"\x1b]7;file:///tmp/".to_vec();
        input.extend(std::iter::repeat_n(b'a', OSC7_MAX_PAYLOAD + 1));
        input.extend_from_slice(b"\x1b]7;file:///tmp/nested\x07");
        input.extend_from_slice(b"\x1b]7;file:///tmp/recovered\x07");

        scanner.advance(&input, |cwd| seen.push(cwd));

        assert_eq!(seen, ["/tmp/recovered"]);
    }

    #[test]
    fn osc7_scanner_cancels_sequences_on_can_and_sub() {
        let mut scanner = Osc7Scanner::default();
        let mut seen = Vec::new();

        scanner.advance(
            b"\x1b]7;file:///tmp/can\x18\x1b]7;file:///tmp/sub\x1a\x1b]7;file:///tmp/recovered\x07",
            |cwd| seen.push(cwd),
        );

        assert_eq!(seen, ["/tmp/recovered"]);
    }

    #[cfg(not(windows))]
    #[test]
    fn osc7_payload_preserves_drive_like_posix_path() {
        assert_eq!(
            cwd_from_osc7_payload("7;file:///C:/dev/path%20with%20space"),
            Some("/C:/dev/path with space".to_string())
        );
    }

    #[cfg(windows)]
    #[test]
    fn osc7_payload_decodes_windows_file_uri() {
        assert_eq!(
            cwd_from_osc7_payload("7;file:///C:/dev/path%20with%20space"),
            Some(r"C:\dev\path with space".to_string())
        );
        assert_eq!(
            cwd_from_osc7_payload("7;file://DESKTOP-123/C:/dev/paneflow"),
            Some(r"C:\dev\paneflow".to_string())
        );
    }

    #[cfg(windows)]
    #[test]
    fn osc7_payload_decodes_git_bash_msys_file_uri() {
        assert_eq!(
            cwd_from_osc7_payload("7;file://DESKTOP-123/c/dev/path%20with%20space"),
            Some(r"C:\dev\path with space".to_string())
        );
        assert_eq!(
            cwd_from_osc7_payload("7;file:///c"),
            Some("C:\\".to_string())
        );
    }

    #[test]
    fn osc7_payload_accepts_raw_percent_from_posix_shells() {
        assert_eq!(
            cwd_from_osc7_payload("7;file://host/tmp/100% legit"),
            Some("/tmp/100% legit".to_string())
        );
    }

    #[test]
    fn osc7_payload_rejects_legacy_malformed_windows_uri() {
        assert_eq!(
            cwd_from_osc7_payload(r"7;file://DESKTOP-123C:\dev\paneflow"),
            None
        );
    }

    #[test]
    fn pending_clipboard_ops_are_bounded() {
        let mut state = TerminalState::new_display_only(5, 20);

        for i in 0..(MAX_PENDING_CLIPBOARD_OPS + 2) {
            state.queue_clipboard_op(ClipboardOp::Store(format!("op-{i}")));
        }

        assert_eq!(state.pending_clipboard_ops.len(), MAX_PENDING_CLIPBOARD_OPS);
        match &state.pending_clipboard_ops[0] {
            ClipboardOp::Store(text) => assert_eq!(text, "op-2"),
            ClipboardOp::Load(_) => panic!("expected store op"),
        }
    }

    #[test]
    fn osc52_store_requires_focus_and_respects_the_shared_cap() {
        let mut state = TerminalState::new_display_only(5, 20);
        state.set_osc52_mode(Osc52Mode::CopyOnly);

        state.process_event(AlacEvent::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            "unfocused".into(),
        ));
        assert!(state.pending_clipboard_ops.is_empty());

        state.set_terminal_focused(true);
        state.process_event(AlacEvent::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            "focused".into(),
        ));
        assert!(matches!(
            state.pending_clipboard_ops.as_slice(),
            [ClipboardOp::Store(text)] if text == "focused"
        ));

        state.process_event(AlacEvent::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            "x".repeat(MAX_OSC52_BYTES + 1),
        ));
        assert_eq!(state.pending_clipboard_ops.len(), 1);

        state.set_terminal_focused(false);
        state.process_event(AlacEvent::ClipboardStore(
            alacritty_terminal::term::ClipboardType::Clipboard,
            "lost-focus".into(),
        ));
        assert_eq!(state.pending_clipboard_ops.len(), 1);
    }

    #[test]
    fn restore_scrollback_strips_escape_and_osc_injection() {
        // A tampered session.json line carrying live VT bytes: an OSC8
        // clickable-link injection, an OSC0 title-spoof, a raw CSI, an ESC
        // introducer, a NUL, and a C1 control. None may survive sanitization.
        let hostile = "\x1b]8;;https://evil.example/\x07click\x1b]8;;\x07\
                       \x1b]0;PWNED\x07\x1b[31mred\x00\u{9b}38m";
        let cleaned = TerminalState::sanitize_scrollback_line(hostile);

        // No control byte that could start a VT sequence survives.
        assert!(
            !cleaned.contains('\x1b'),
            "ESC introducer must be stripped; got {cleaned:?}"
        );
        assert!(
            !cleaned.contains('\x07'),
            "BEL (OSC terminator) must be stripped; got {cleaned:?}"
        );
        assert!(
            !cleaned.contains('\x00'),
            "NUL / C0 controls must be stripped; got {cleaned:?}"
        );
        assert!(
            !cleaned.chars().any(|c| ('\u{80}'..='\u{9f}').contains(&c)),
            "C1 controls must be stripped; got {cleaned:?}"
        );
        // Visible glyphs are preserved verbatim (no live sequence remains, so
        // these read as plain text rather than executing).
        for marker in ["https://evil.example/", "click", "PWNED", "red", "38m"] {
            assert!(
                cleaned.contains(marker),
                "plain glyphs must survive; {marker:?} missing from {cleaned:?}"
            );
        }
        // A tab is the one C0 byte we intentionally keep.
        assert_eq!(
            TerminalState::sanitize_scrollback_line("a\tb"),
            "a\tb",
            "tab must be preserved"
        );
    }

    /// `extract_scrollback_from` can read a cloned `SharedTerm` handle while
    /// excluding active viewport rows from the extracted terminal history.
    #[test]
    fn extract_scrollback_from_drains_cloned_history_only() {
        let state = TerminalState::new_display_only(3, 80);
        state.restore_scrollback("history-alpha\nhistory-bravo\nvisible-charlie\nvisible-delta");

        // Clone the Arc, then extract via the associated fn without `&self`.
        let handle = state.term.clone();
        let drained = TerminalState::extract_scrollback_from(&handle)
            .expect("seeded scrollback should not be empty");

        for marker in ["history-alpha", "history-bravo"] {
            assert!(
                drained.contains(marker),
                "drained scrollback must contain {marker:?}; got:\n{drained}"
            );
        }
        for marker in ["visible-charlie", "visible-delta"] {
            assert!(
                !drained.contains(marker),
                "active viewport must exclude {marker:?}; got:\n{drained}"
            );
        }
    }

    /// U-001: a multibyte codepoint straddling the byte cap must not panic
    /// `String::truncate`; the cut lands on a char boundary at or below the cap.
    #[test]
    fn cap_scrollback_truncates_on_char_boundary() {
        const MAX: usize = 100;
        // 99 ASCII bytes, then a 4-byte '🦀' occupying byte indices 99..103, so
        // byte index `MAX` (100) falls inside the codepoint - the case that
        // panics a raw `truncate(MAX)`. No newline, so the line-trim is a no-op.
        let mut s = "a".repeat(MAX - 1);
        s.push('🦀');
        assert!(s.len() > MAX, "fixture must exceed the cap");

        cap_scrollback_at_char_boundary(&mut s, MAX);

        // `String` already guarantees valid UTF-8; the contract is length ≤ cap
        // and that the straddling char was dropped whole rather than split.
        assert!(s.len() <= MAX, "capped length {} must be ≤ {MAX}", s.len());
        assert_eq!(s, "a".repeat(MAX - 1));
    }

    /// Already-aligned cap is a no-op beyond the existing line trim.
    #[test]
    fn cap_scrollback_noop_under_cap() {
        let mut s = "short line".to_string();
        let before = s.clone();
        cap_scrollback_at_char_boundary(&mut s, 100);
        assert_eq!(s, before);
    }

    /// Dump the viewport grid to a string for the live smoke test.
    fn grid_to_string(term: &Arc<FairMutex<Term<ZedListener>>>) -> String {
        let term = term.lock();
        let grid = term.grid();
        let mut out = String::new();
        for line in 0..grid.screen_lines() {
            for col in 0..grid.columns() {
                out.push(grid[AlacPoint::new(GridLine(line as i32), GridCol(col))].c);
            }
            out.push('\n');
        }
        out
    }

    /// EP-002 US-004 live smoke: spawn a REAL PTY-backed shell via
    /// `alacritty_terminal::tty` + `EventLoop`, write a marker command, and
    /// confirm the EventLoop read->parse path lands the echoed output in the
    /// `Term` grid. This is the only test that exercises `tty::new` +
    /// `EventLoop::spawn` + `Notifier` end-to-end - the others use the
    /// display-only path. Unix-only (drives `/bin/sh`); the process group is
    /// torn down by `Drop` at scope exit.
    #[cfg(unix)]
    #[test]
    fn eventloop_pty_echoes_input_into_grid() {
        let mut state = TerminalState::new(None, 1, 1, Some((80, 24)), None, None)
            .expect("EP-002: spawn a PTY-backed terminal via tty::new + EventLoop");
        assert!(state.child_pid > 0, "a real PTY child must have a pid");

        // Let the shell initialize, then send a unique marker command.
        std::thread::sleep(std::time::Duration::from_millis(250));
        state.notifier.notify(b"echo PANEFLOW_SMOKE_OK\n".to_vec());

        // Poll the grid (the EventLoop mutates it on its own thread) until the
        // echoed marker appears, draining events meanwhile. Generous budget so
        // a slow runner doesn't flake.
        let mut found = false;
        for _ in 0..60 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            state.sync();
            if grid_to_string(&state.term).contains("PANEFLOW_SMOKE_OK") {
                found = true;
                break;
            }
        }
        assert!(
            found,
            "EP-002: the EventLoop read path did not deliver shell output to the grid"
        );
    }

    #[test]
    fn eventloop_drains_final_output_after_exit() {
        let mut state = TerminalState::new(None, 1, 1, Some((80, 24)), None, None)
            .expect("spawn a PTY-backed terminal");

        std::thread::sleep(std::time::Duration::from_millis(250));
        state
            .notifier
            .notify(b"echo PANEFLOW_FINAL_OK\nexit\n".to_vec());

        let mut found = false;
        for _ in 0..240 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            state.sync();
            let visible = grid_to_string(&state.term);
            let history = state.extract_scrollback().unwrap_or_default();
            if visible.contains("PANEFLOW_FINAL_OK") || history.contains("PANEFLOW_FINAL_OK") {
                found = true;
                break;
            }
        }

        assert!(
            found,
            "final PTY output must survive a fast shell exit before the overlay lands"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_descendants_postorder_places_children_before_parent() {
        let entries = vec![(10, 1), (11, 10), (12, 10), (13, 12), (20, 1)];

        assert_eq!(
            windows_descendants_postorder(10, &entries),
            vec![11, 13, 12]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_tree_targets_stay_scoped_and_put_root_last() {
        let entries = vec![(10, 1), (11, 10), (12, 10), (13, 12), (20, 1)];

        assert_eq!(
            windows_process_tree_targets(10, &entries, true),
            vec![11, 13, 12, 10]
        );
        assert_eq!(
            windows_process_tree_targets(10, &entries, false),
            vec![11, 13, 12]
        );

        let cyclic_entries = vec![(10, 11), (11, 10), (20, 1)];
        assert_eq!(
            windows_process_tree_targets(10, &cyclic_entries, true),
            vec![11, 10],
            "a malformed cycle must still target the pane root exactly once"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_wait_timeout_never_rounds_past_global_budget() {
        use std::time::Duration;

        assert_eq!(windows_wait_timeout_ms(Duration::ZERO), None);
        assert_eq!(windows_wait_timeout_ms(Duration::from_micros(999)), None);
        assert_eq!(windows_wait_timeout_ms(Duration::from_millis(1)), Some(1));
        assert_eq!(
            windows_wait_timeout_ms(Duration::from_micros(1_999)),
            Some(1)
        );
        assert_eq!(
            windows_wait_timeout_ms(Duration::from_millis(u64::from(u32::MAX) + 1)),
            Some(u32::MAX - 1)
        );
    }

    #[test]
    fn output_generation_advances_on_pty_output() {
        // US-010: `workspace.up` polls `output_generation` as its prefill
        // readiness signal. A fresh terminal has produced nothing (0); the
        // counter must advance once the shell emits output (Wakeup events
        // drained by `sync`), proving the signal tracks real PTY activity.
        let mut state = TerminalState::new(None, 1, 1, Some((80, 24)), None, None)
            .expect("spawn a PTY-backed terminal");
        assert_eq!(
            state.output_generation, 0,
            "a fresh terminal has produced no output"
        );

        std::thread::sleep(std::time::Duration::from_millis(250));
        state.notifier.notify(b"echo PANEFLOW_GEN_OK\n".to_vec());

        // Up to 12s. This test runs on every OS, and PowerShell (the Windows
        // default shell) can cold-start slowly on a loaded CI runner before
        // emitting its banner - output_generation only advances once the PTY
        // produces any output. The loop breaks immediately on success, so the
        // larger budget only costs wall-time when the signal never arrives.
        let mut advanced = false;
        for _ in 0..240 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            state.sync();
            if state.output_generation > 0 {
                advanced = true;
                break;
            }
        }
        assert!(
            advanced,
            "output_generation must advance once the PTY emits output"
        );
    }
}
