use std::borrow::Cow;
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedReceiver;

use super::clipboard_gate::ClipboardGate;
use super::ghostty_session::{
    GhosttyInputSendResult, GhosttyRuntimePending, GhosttySession, GhosttyUiEvent,
    ProgramNotification, SpawnedGhostty,
};
use super::marks::SharedMarkRing;
use super::service_detector::{ServiceInfo, detect_framework, parse_service_line};
use super::shell::{resolve_default_shell, setup_shell_integration};
use super::types::{
    Content, GridLineText, GridMetrics, HyperlinkZone, Line, Modes, Point, SelectionGeometry,
    SelectionKind, SelectionRange, ShellQuoting, TerminalWindowSize,
};
use crate::limits::MAX_OSC52_BYTES;
use paneflow_config::schema::{TerminalConfig, TerminalSurfaceProfile};
use paneflow_terminal_ghostty::Scroll as GhosttyScroll;

const DEFAULT_SCROLLBACK_LINES: usize = TerminalConfig::DEFAULT_SCROLLBACK_LINES;
const INHERITED_AGENT_SESSION_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
];
const INHERITED_HOST_TERMINAL_ENV: &[&str] = &[
    "WT_SESSION",
    "WT_PROFILE_ID",
    "TMUX",
    "TMUX_PANE",
    "STY",
    "ZELLIJ",
    "ZELLIJ_SESSION_NAME",
    "ZELLIJ_PANE_ID",
    "KITTY_WINDOW_ID",
    "KITTY_LISTEN_ON",
    "TERMINAL_EMULATOR",
    "VTE_VERSION",
    "ITERM_SESSION_ID",
    "LC_TERMINAL",
    "LC_TERMINAL_VERSION",
    "ALACRITTY_WINDOW_ID",
    "ALACRITTY_SOCKET",
];
const CONEMU_ENV_PREFIX: &str = "conemu";
const MAX_PENDING_CLIPBOARD_OPS: usize = 8;
const MAX_PENDING_NOTIFICATIONS: usize = 8;

fn resolved_scrollback_lines(profile: TerminalSurfaceProfile) -> usize {
    paneflow_config::loader::load_config()
        .terminal
        .unwrap_or(TerminalConfig {
            scrollback_lines: Some(DEFAULT_SCROLLBACK_LINES),
            ..Default::default()
        })
        .resolved_scrollback_lines_for_profile(profile)
}

#[derive(Clone)]
pub(crate) struct TerminalSessionBackend {
    ghostty: GhosttySession,
}

pub(crate) struct TerminalBackendEvent(GhosttyUiEvent);

impl TerminalBackendEvent {
    pub(crate) fn is_wakeup(&self) -> bool {
        self.0.is_wakeup()
    }
}

pub(crate) struct TerminalBackendEvents(Option<UnboundedReceiver<GhosttyUiEvent>>);

impl futures::Stream for TerminalBackendEvents {
    type Item = TerminalBackendEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let Some(receiver) = self.0.as_mut() else {
            return std::task::Poll::Pending;
        };
        match std::pin::Pin::new(receiver).poll_next(cx) {
            std::task::Poll::Ready(Some(event)) => {
                std::task::Poll::Ready(Some(TerminalBackendEvent(event)))
            }
            std::task::Poll::Ready(None) | std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl futures::stream::FusedStream for TerminalBackendEvents {
    fn is_terminated(&self) -> bool {
        false
    }
}

pub(crate) struct PendingTerminalBackend {
    pub(super) ghostty: GhosttyRuntimePending,
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
    fn new(ghostty: GhosttySession) -> Self {
        Self { ghostty }
    }

    pub(crate) fn render_content(
        &self,
        window_size: TerminalWindowSize,
        first_visible_row: i32,
        last_visible_row: i32,
        clear_on_resize: bool,
    ) -> (Content, bool) {
        #[cfg(test)]
        let snapshot_started_at = RENDER_CONTENT_TIMING_ENABLED
            .load(std::sync::atomic::Ordering::Acquire)
            .then(std::time::Instant::now);
        let rendered = self.ghostty.render_content(
            window_size,
            first_visible_row,
            last_visible_row,
            clear_on_resize,
        );
        #[cfg(test)]
        if let Some(snapshot_started_at) = snapshot_started_at {
            RENDER_CONTENT_LOCK_DURATIONS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(snapshot_started_at.elapsed());
        }
        rendered
    }

    pub(crate) fn notify_window_size(&self, size: TerminalWindowSize) {
        self.ghostty.resize(size);
    }

    pub(crate) fn modes(&self) -> Modes {
        self.ghostty.modes()
    }

    pub(crate) fn grid_metrics(&self) -> GridMetrics {
        self.ghostty.grid_metrics()
    }

    pub(crate) fn clear_history(&self) {
        self.ghostty.clear_history();
    }

    pub(crate) fn scroll_to_bottom(&self) -> bool {
        self.scroll(GhosttyScroll::Bottom)
    }

    pub(crate) fn scroll_delta(&self, delta: i32) -> bool {
        self.scroll(GhosttyScroll::Delta(delta))
    }

    pub(crate) fn scroll_page_up(&self) -> bool {
        let lines = i32::try_from(self.grid_metrics().screen_lines).unwrap_or(i32::MAX);
        self.scroll(GhosttyScroll::Delta(lines))
    }

    pub(crate) fn scroll_page_down(&self) -> bool {
        let lines = i32::try_from(self.grid_metrics().screen_lines).unwrap_or(i32::MAX);
        self.scroll(GhosttyScroll::Delta(-lines))
    }

    fn scroll(&self, scroll: GhosttyScroll) -> bool {
        if matches!(scroll, GhosttyScroll::Delta(0)) {
            return false;
        }
        self.ghostty.scroll(scroll)
    }

    pub(crate) fn restore_display_offset(&self, target: usize) -> bool {
        let metrics = self.ghostty.grid_metrics();
        let history_size = usize::try_from(-i64::from(metrics.topmost_line.0)).unwrap_or(0);
        let row = history_size.saturating_sub(target.min(history_size));
        self.ghostty.scroll_to_viewport_row(row)
    }

    pub(crate) fn scroll_to_viewport_row(&self, row: usize) -> bool {
        self.ghostty.scroll_to_viewport_row(row)
    }

    pub(crate) fn selection_geometry(
        &self,
        cell_width: f32,
        line_height: f32,
    ) -> SelectionGeometry {
        let metrics = self.ghostty.grid_metrics();
        SelectionGeometry {
            columns: metrics.columns,
            screen_lines: metrics.screen_lines,
            display_offset: metrics.display_offset,
            cell_width,
            line_height,
        }
    }

    pub(crate) fn press_selection(&self, kind: SelectionKind, point: Point, position: (f32, f32)) {
        self.ghostty.press_selection(kind, point, position);
    }

    pub(crate) fn drag_selection(
        &self,
        point: Point,
        position: (f32, f32),
        geometry: SelectionGeometry,
        rectangle: bool,
    ) {
        self.ghostty
            .drag_selection(point, position, geometry, rectangle);
    }

    pub(crate) fn release_selection(&self, point: Option<Point>) {
        self.ghostty.release_selection(point);
    }

    pub(crate) fn selection_text(&self) -> Option<String> {
        self.ghostty.selection_text()
    }

    pub(crate) fn finish_selection(&self) -> (bool, Option<String>) {
        let copied = self.ghostty.selection_text();
        let is_empty = copied.as_ref().is_none_or(String::is_empty);
        self.ghostty.clear_selection();
        (is_empty, copied)
    }

    pub(crate) fn clear_selection(&self) {
        self.ghostty.clear_selection();
    }

    pub(crate) fn request_osc8_hyperlink_at(&self, point: Point) -> bool {
        self.ghostty.request_hyperlink_at(point)
    }

    pub(crate) fn line_text_at(&self, point: Point) -> Option<GridLineText> {
        self.ghostty.line_text_at(point)
    }

    pub(crate) fn move_copy_cursor(&self, current: Point, dx: i32, dy: i32, extend: bool) -> Point {
        let metrics = self.ghostty.grid_metrics();
        let column = (current.column.0 as i32 + dx)
            .clamp(0, metrics.columns.saturating_sub(1) as i32) as usize;
        let line = (current.line.0 + dy).clamp(metrics.topmost_line.0, metrics.bottommost_line.0);
        let next = Point::new(line, column);
        if extend {
            let geometry = self.selection_geometry(1.0, 1.0);
            if self.ghostty.selection_range().is_none() {
                self.ghostty
                    .press_selection(SelectionKind::Simple, current, (0.0, 0.0));
            }
            self.ghostty
                .drag_selection(next, (0.0, 0.0), geometry, false);
        } else {
            self.ghostty.clear_selection();
        }
        next
    }

    pub(crate) fn selection_range(&self) -> Option<SelectionRange> {
        self.ghostty.selection_range()
    }

    pub(crate) fn bottommost_line(&self) -> Line {
        self.ghostty.grid_metrics().bottommost_line
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
        self.ghostty.search_with_cancel(query, regex, cancelled)
    }

    pub(crate) fn set_default_cursor(
        &self,
        shape: paneflow_terminal_ghostty::CursorShape,
        blink: bool,
    ) -> bool {
        self.ghostty.set_default_cursor(shape, blink)
    }

    pub(crate) fn kitty_placements(
        &self,
    ) -> std::sync::Arc<[crate::terminal::kitty::KittyPlacement]> {
        self.ghostty.kitty_placements()
    }

    pub(crate) fn refresh_appearance(&self) -> bool {
        self.ghostty.refresh_appearance()
    }

    pub(crate) fn scroll_to_match(&self, search_match: &crate::search::SearchMatch) -> usize {
        let metrics = self.ghostty.grid_metrics();
        let target = (metrics.bottommost_line.0 - search_match.start.line.0).max(0) as usize;
        let _ = self.restore_display_offset(target);
        target
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Osc52Mode {
    Disabled,
    CopyOnly,
    CopyPaste,
}

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
    Initialization,
    OpenPty,
    Spawn,
    PostSpawn,
}

impl TerminalBackendFailurePhase {
    pub fn as_str(self) -> &'static str {
        match self {
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
    pub failure: Option<TerminalBackendFailureDiagnostics>,
    pub target_triple: &'static str,
    pub ghostty: GhosttyBuildDiagnostics,
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
            "backend=ghostty failure_phase={failure_phase} reason_code={reason_code} target={} os_error=",
            self.target_triple
        )?;
        match os_error {
            Some(code) => write!(formatter, "{code}")?,
            None => formatter.write_str("none")?,
        }
        write!(
            formatter,
            " ghostty_version={} ghostty_source_sha={} ghostty_api_version={} zig_version={} optimization={} simd={}",
            self.ghostty.version,
            self.ghostty.source_sha,
            self.ghostty.api_version,
            self.ghostty.zig_version,
            self.ghostty.optimization,
            self.ghostty.simd,
        )
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
    Key(paneflow_terminal_ghostty::KeyInput),
    Mouse {
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
    },
    Focus(paneflow_terminal_ghostty::FocusEvent),
    Paste {
        text: String,
        allow_unsafe: bool,
    },
}

impl PendingTerminalInput {
    fn queued_bytes(&self) -> usize {
        match self {
            Self::Raw(bytes) => bytes.len(),
            Self::Key(input) => std::mem::size_of::<paneflow_terminal_ghostty::KeyInput>()
                .saturating_add(input.text.len()),
            Self::Mouse { repeat, .. } => {
                std::mem::size_of::<paneflow_terminal_ghostty::MouseInput>().saturating_add(*repeat)
            }
            Self::Focus(_) => std::mem::size_of::<paneflow_terminal_ghostty::FocusEvent>(),
            Self::Paste { text, .. } => text.len(),
        }
    }

    fn queue_limit(&self) -> usize {
        match self {
            Self::Raw(_) | Self::Paste { .. } => {
                MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES
            }
            Self::Key(input) if input.action == paneflow_terminal_ghostty::KeyAction::Release => {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Mouse { input, .. }
                if input.action == paneflow_terminal_ghostty::MouseAction::Release =>
            {
                MAX_PENDING_INPUT_BYTES
            }
            Self::Focus(_) => MAX_PENDING_INPUT_BYTES,
            Self::Key(_) | Self::Mouse { .. } => {
                MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES
            }
        }
    }

    fn fits_after(&self, queued_bytes: usize) -> bool {
        queued_bytes.saturating_add(self.queued_bytes()) <= self.queue_limit()
    }

    fn try_send(&self, ghostty: &GhosttySession) -> GhosttyInputSendResult {
        match self {
            Self::Raw(bytes) => ghostty.write(bytes.clone().into_owned()),
            Self::Key(input) => ghostty.write_key(input.clone()),
            Self::Mouse { input, repeat } => ghostty.write_mouse(*input, *repeat),
            Self::Focus(event) => ghostty.write_focus(*event),
            Self::Paste { text, allow_unsafe } => ghostty.write_paste(text.clone(), *allow_unsafe),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BackendInputResult {
    Accepted,
    Rejected,
}

pub struct TerminalState {
    ghostty: GhosttySession,
    ghostty_events_rx: Option<UnboundedReceiver<GhosttyUiEvent>>,
    backend_failure: Option<TerminalBackendFailureDiagnostics>,
    pub(crate) marks: SharedMarkRing,
    last_prompt_seq: u64,
    pub exited: Option<i32>,
    resolved_hover_link: Option<(Point, Option<HyperlinkZone>)>,
    keyboard_input_sent: std::sync::atomic::AtomicBool,
    pub exit_signal: Option<String>,
    pub child_pid: u32,
    pub title: String,
    pub current_cwd: Option<String>,
    pub progress: Option<paneflow_terminal_ghostty::ProgressReport>,
    pub custom_name: Option<String>,
    pub detected_agent: Option<crate::agent_launcher::TerminalAgent>,
    pub agent_confirmed: bool,
    pub agent_declared_until: Option<std::time::Instant>,
    pub detected_ports: Vec<(u16, Option<String>)>,
    pub port_conflicts: Vec<(u16, String)>,
    pub announced_ports: Vec<u16>,
    pub font_size_override: Option<f32>,
    pub osc52_mode: Osc52Mode,
    terminal_focused: bool,
    clipboard_gate: Arc<ClipboardGate>,
    pub(super) shell_quoting: ShellQuoting,
    pub(super) pending_clipboard_ops: Vec<String>,
    pub(super) pending_notifications: Vec<ProgramNotification>,
    pub cached_foreground_command: Option<String>,
    #[cfg(all(unix, not(test)))]
    pty_guard: Option<crate::agents::parent_guard::PtyGuardHandle>,
    pub cursor_blinking: bool,
    pub dirty: bool,
    pub output_generation: u64,
    pub(super) last_activity_burst: Option<std::time::Instant>,
    cwd_poll_ticks: u32,
    reported_ports: std::collections::HashSet<u16>,
    #[cfg(debug_assertions)]
    pub(crate) last_keystroke_at: Option<std::time::Instant>,
    pending_input: std::sync::Mutex<VecDeque<PendingTerminalInput>>,
}

const MAX_PENDING_INPUT_BYTES: usize = 1024 * 1024;
const INPUT_CONTROL_RESERVE_BYTES: usize = 64 * 1024;

const SPAWN_FAILURE_SCROLLBACK_LINES: usize = 256;

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
}

#[cfg(unix)]
pub type ForegroundSignalMask = libc::sigset_t;
#[cfg(not(unix))]
pub type ForegroundSignalMask = ();

pub(super) fn capture_foreground_signal_mask() -> Option<ForegroundSignalMask> {
    #[cfg(unix)]
    {
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

#[cfg(unix)]
pub(super) fn apply_thread_signal_mask(
    mask: Option<ForegroundSignalMask>,
) -> Option<libc::sigset_t> {
    let fg = mask?;
    unsafe {
        let mut saved: libc::sigset_t = std::mem::zeroed();
        if libc::pthread_sigmask(libc::SIG_SETMASK, &fg, &mut saved) == 0 {
            Some(saved)
        } else {
            None
        }
    }
}

#[cfg(unix)]
pub(super) fn restore_thread_signal_mask(saved: Option<libc::sigset_t>) {
    if let Some(saved) = saved {
        unsafe {
            libc::pthread_sigmask(libc::SIG_SETMASK, &saved, std::ptr::null_mut());
        }
    }
}

impl TerminalState {
    pub(crate) fn session_backend(&self) -> TerminalSessionBackend {
        TerminalSessionBackend::new(self.ghostty.clone())
    }

    pub(super) fn ghostty_session(&self) -> GhosttySession {
        self.ghostty.clone()
    }

    pub(crate) fn take_backend_events(&mut self) -> TerminalBackendEvents {
        TerminalBackendEvents(self.ghostty_events_rx.take())
    }

    pub(crate) fn process_backend_event(&mut self, event: TerminalBackendEvent) {
        self.process_ghostty_event(event.0);
    }

    pub(crate) fn process_backend_wakeup(&mut self) {
        self.dirty = true;
        self.output_generation = self.output_generation.saturating_add(1);
        self.flush_ghostty_pending_input();
        self.ghostty.retry_backpressured_commands();
    }

    pub(crate) fn notify_window_size(&self, size: TerminalWindowSize) {
        self.ghostty.resize(size);
    }

    pub(super) fn promote_ghostty(&mut self, spawned: SpawnedGhostty) {
        self.ghostty.promote();
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

    fn flush_ghostty_pending_input(&self) {
        if !self.ghostty.is_promoted() {
            return;
        }
        let Ok(mut pending) = self.pending_input.lock() else {
            return;
        };
        while let Some(input) = pending.front().cloned() {
            match input.try_send(&self.ghostty) {
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

    pub(super) fn report_spawn_failure(
        &mut self,
        failure: TerminalBackendFailureDiagnostics,
        message: &str,
    ) {
        self.backend_failure = Some(failure);
        self.ghostty.shutdown();

        let size = self.ghostty.requested_window_size();
        let (session, pending, events_rx) =
            GhosttySession::pending_with_clipboard_gate(size, self.clipboard_gate.clone());
        if let Err(error) = session.start_display(pending, SPAWN_FAILURE_SCROLLBACK_LINES) {
            log::error!(
                target: "paneflow::terminal::ghostty",
                "could not open the spawn-failure pane: {error}"
            );
            return;
        }

        self.marks = session.marks();
        self.ghostty = session;
        self.ghostty_events_rx = Some(events_rx);
        self.write_output(message.as_bytes());
        self.dirty = true;
    }

    pub fn backend_diagnostics(&self) -> TerminalBackendDiagnostics {
        let identity = paneflow_terminal_ghostty::build_identity();
        TerminalBackendDiagnostics {
            failure: self.backend_failure.clone(),
            target_triple: env!("PANEFLOW_TARGET_TRIPLE"),
            ghostty: GhosttyBuildDiagnostics {
                version: paneflow_terminal_ghostty::GHOSTTY_APP_VERSION,
                source_sha: identity.source_sha,
                api_version: identity.api_version,
                zig_version: identity.zig_version,
                optimization: identity.optimization,
                simd: identity.simd,
            },
        }
    }

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
        let max_scrollback = resolved_scrollback_lines(params.profile);
        let (mut state, pending) = Self::new_pending_with_profile_and_shell_quoting(
            params.cols,
            params.rows,
            params.profile,
            params.shell_quoting,
        );
        let spawned = state
            .ghostty_session()
            .start(pending.ghostty, params, signal_mask, max_scrollback)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        state.promote_ghostty(spawned);
        Ok(state)
    }

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
        let config = paneflow_config::loader::load_config();
        let shell = {
            let configured = config
                .default_shell
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let resolved = resolve_default_shell(configured);
            log::info!(
                target: "paneflow::terminal::backend",
                "Terminal shell resolved: {resolved:?} (default_shell={configured:?})"
            );
            resolved
        };
        let shell_quoting = ShellQuoting::for_shell(&shell);
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
        let extra_args = if config.shell_integration.unwrap_or(true) {
            setup_shell_integration(&shell, &mut env)
        } else {
            vec![]
        };
        let mut env = assemble_pty_env(env, workspace_id, surface_id, merged_env);
        if is_wsl_shell(&shell) {
            augment_wslenv(&mut env);
        }
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
        }
    }

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
        _profile: TerminalSurfaceProfile,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        Self::build_display_only(cols, rows, shell_quoting)
    }

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
        let (state, pending) =
            Self::build_display_only(cols, rows, ShellQuoting::default_for_platform());
        if let Err(error) = state
            .ghostty
            .start_display(pending.ghostty, resolved_scrollback_lines(profile))
        {
            log::error!(
                target: "paneflow::terminal::ghostty",
                "could not start the display-only runtime: {error}"
            );
        }
        state
    }

    fn build_display_only(
        cols: usize,
        rows: usize,
        shell_quoting: ShellQuoting,
    ) -> (Self, PendingTerminalBackend) {
        let clipboard_gate = Arc::new(ClipboardGate::default());
        let (ghostty, runtime_pending, events_rx) = GhosttySession::pending_with_clipboard_gate(
            TerminalWindowSize::new(cols, rows, 0, 0),
            clipboard_gate.clone(),
        );
        let marks = ghostty.marks();
        let state = Self {
            ghostty,
            ghostty_events_rx: Some(events_rx),
            backend_failure: None,
            marks,
            last_prompt_seq: 0,
            exited: None,
            keyboard_input_sent: std::sync::atomic::AtomicBool::new(false),
            exit_signal: None,
            resolved_hover_link: None,
            child_pid: 0,
            current_cwd: None,
            progress: None,
            custom_name: None,
            detected_agent: None,
            agent_confirmed: false,
            agent_declared_until: None,
            detected_ports: Vec::new(),
            port_conflicts: Vec::new(),
            announced_ports: Vec::new(),
            font_size_override: None,
            osc52_mode: Osc52Mode::Disabled,
            terminal_focused: false,
            clipboard_gate,
            shell_quoting,
            pending_clipboard_ops: Vec::new(),
            pending_notifications: Vec::new(),
            cached_foreground_command: None,
            #[cfg(all(unix, not(test)))]
            pty_guard: None,
            cursor_blinking: false,
            title: String::from("Terminal"),
            dirty: true,
            output_generation: 0,
            last_activity_burst: None,
            cwd_poll_ticks: 0,
            reported_ports: std::collections::HashSet::new(),
            #[cfg(debug_assertions)]
            last_keystroke_at: None,
            pending_input: std::sync::Mutex::new(VecDeque::new()),
        };
        (
            state,
            PendingTerminalBackend {
                ghostty: runtime_pending,
            },
        )
    }

    #[allow(dead_code)]
    pub fn write_output(&self, bytes: &[u8]) {
        let mut converted = Vec::with_capacity(bytes.len());
        let mut prev = 0u8;
        for &b in bytes {
            if b == b'\n' && prev != b'\r' {
                converted.push(b'\r');
            }
            converted.push(b);
            prev = b;
        }
        self.ghostty.write_output(&converted);
    }

    #[allow(dead_code)]
    pub fn sync(&mut self) {
        self.sync_channels();
        if let Some(mut rx) = self.ghostty_events_rx.take() {
            while let Ok(event) = rx.try_recv() {
                self.process_ghostty_event(event);
            }
            self.ghostty_events_rx = Some(rx);
        }
    }

    pub fn sync_channels(&mut self) {
        self.cwd_poll_ticks = self.cwd_poll_ticks.wrapping_add(1);
        if self.cwd_poll_ticks.is_multiple_of(25)
            && let Some(cwd) = self.cwd_now()
        {
            self.current_cwd = Some(cwd.to_string_lossy().into_owned());
        }
    }

    pub(crate) fn take_resolved_hover_link(&mut self) -> Option<(Point, Option<HyperlinkZone>)> {
        self.resolved_hover_link.take()
    }

    #[cfg(test)]
    pub(super) fn processed_output_bytes_for_test(&self) -> usize {
        self.ghostty.processed_output_bytes_for_test()
    }

    pub(crate) fn take_shell_prompt_ready(&mut self) -> bool {
        let seq = self
            .marks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .prompt_start_seq();
        let fired = seq != self.last_prompt_seq;
        self.last_prompt_seq = seq;
        fired
    }

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
            GhosttyUiEvent::Progress(events) => {
                if let Some(report) = events.take_progress() {
                    self.progress = match report.state {
                        paneflow_terminal_ghostty::ProgressState::Remove => None,
                        _ => Some(report),
                    };
                }
            }
            GhosttyUiEvent::Notification(events) => {
                for notification in events.take_notifications() {
                    if self.pending_notifications.len() >= MAX_PENDING_NOTIFICATIONS {
                        self.pending_notifications.remove(0);
                    }
                    self.pending_notifications.push(notification);
                }
            }
            GhosttyUiEvent::Clipboard(events) => {
                for text in events.take_clipboard() {
                    self.deliver_clipboard_text(text);
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
                self.progress = None;
                self.cached_foreground_command = None;
                self.reported_ports.clear();
                #[cfg(all(unix, not(test)))]
                {
                    self.pty_guard = None;
                }
            }
            GhosttyUiEvent::HyperlinkResolved { point, link } => {
                self.resolved_hover_link = Some((point, link));
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

    fn deliver_clipboard_text(&mut self, text: String) {
        if self.terminal_focused
            && self.osc52_mode != Osc52Mode::Disabled
            && text.len() <= MAX_OSC52_BYTES
        {
            self.queue_clipboard_op(text);
        }
    }

    fn queue_clipboard_op(&mut self, text: String) {
        if self.pending_clipboard_ops.len() >= MAX_PENDING_CLIPBOARD_OPS {
            self.pending_clipboard_ops.remove(0);
        }
        self.pending_clipboard_ops.push(text);
    }

    #[cfg(target_os = "linux")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        if self.exited.is_some() {
            return None;
        }
        if self.child_pid == 0 {
            return None;
        }
        let proc_path = format!("/proc/{}/cwd", self.child_pid);
        std::fs::read_link(&proc_path).ok()
    }

    #[cfg(target_os = "macos")]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        use std::ffi::CStr;
        use std::mem::MaybeUninit;
        use std::os::raw::c_void;

        if self.exited.is_some() {
            return None;
        }

        if self.child_pid == 0 {
            return None;
        }

        let pid = self.child_pid as libc::c_int;
        let mut info = MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as libc::c_int;

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

        let info = unsafe { info.assume_init() };

        let ptr = info.pvi_cdir.vip_path.as_ptr() as *const libc::c_char;
        let cstr = unsafe { CStr::from_ptr(ptr) };
        match cstr.to_str() {
            Ok(s) if !s.is_empty() => Some(std::path::PathBuf::from(s)),
            _ => None,
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub fn cwd_now(&self) -> Option<std::path::PathBuf> {
        None
    }

    pub fn scan_output(&mut self) -> Vec<ServiceInfo> {
        let lines = self.ghostty.recent_output_lines();
        self.detect_services_in_lines(&lines)
    }

    fn detect_services_in_lines(&mut self, lines: &[String]) -> Vec<ServiceInfo> {
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
        self.keyboard_input_sent
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.notify_or_buffer(input.into());
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

    fn dispatch_ghostty_input(
        &self,
        input: PendingTerminalInput,
        user_initiated: bool,
    ) -> BackendInputResult {
        let Ok(mut pending) = self.pending_input.lock() else {
            return BackendInputResult::Rejected;
        };
        let pending_bytes = pending.iter().fold(0usize, |total, item| {
            total.saturating_add(item.queued_bytes())
        });
        let total = pending_bytes.saturating_add(self.ghostty.queued_input_bytes());
        let queue_limit = input.queue_limit();
        if !input.fits_after(total) {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty input rejected at the {} byte queue limit",
                queue_limit
            );
            return BackendInputResult::Rejected;
        }

        if self.ghostty.is_promoted() && pending.is_empty() {
            match input.try_send(&self.ghostty) {
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

    pub(super) fn write_ghostty_key(
        &self,
        input: paneflow_terminal_ghostty::KeyInput,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Key(input), true)
    }

    pub(super) fn write_ghostty_mouse(
        &self,
        input: paneflow_terminal_ghostty::MouseInput,
        repeat: usize,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Mouse { input, repeat }, true)
    }

    pub(super) fn write_ghostty_focus(
        &self,
        event: paneflow_terminal_ghostty::FocusEvent,
    ) -> BackendInputResult {
        self.dispatch_ghostty_input(PendingTerminalInput::Focus(event), false)
    }

    pub(super) fn write_ghostty_paste(&self, text: String) -> BackendInputResult {
        self.dispatch_ghostty_input(
            PendingTerminalInput::Paste {
                text,
                allow_unsafe: true,
            },
            true,
        )
    }

    fn notify_or_buffer(&self, input: Cow<'static, [u8]>) {
        if input.is_empty() {
            return;
        }
        self.dispatch_ghostty_input(PendingTerminalInput::Raw(input), false);
    }

    pub fn write_to_pty_silent(&self, input: impl Into<Cow<'static, [u8]>>) {
        self.notify_or_buffer(input.into());
    }

    pub fn should_close_on_exit(&self) -> bool {
        self.keyboard_input_sent
            .load(std::sync::atomic::Ordering::Relaxed)
            || self.exited == Some(0)
    }

    pub fn extract_scrollback(&self) -> Option<String> {
        self.ghostty.extract_scrollback()
    }

    pub fn screen_text(&self) -> Option<String> {
        let text = self.ghostty.screen_text()?;
        let trimmed = text.trim_end_matches(['\n', ' ']);
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    }

    pub fn capture_replay(&self) -> Option<Vec<u8>> {
        self.ghostty.capture_replay()
    }

    pub fn foreground_command(&self) -> Option<String> {
        self.cached_foreground_command.clone()
    }

    pub fn search_scrollback(
        &self,
        pattern: &str,
        max_matches: usize,
    ) -> (Vec<(i32, String)>, bool) {
        if pattern.is_empty() || max_matches == 0 {
            return (Vec::new(), false);
        }
        self.ghostty.search_scrollback(pattern, max_matches)
    }

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

    pub fn restore_scrollback(&self, text: &str) {
        self.ghostty.restore_scrollback(text);
    }

    pub fn restore_replay(&self, bytes: &[u8]) {
        self.ghostty.write_output(bytes);
    }
}

fn paneflow_socket_path() -> Option<String> {
    crate::runtime_paths::socket_path().map(|p| p.display().to_string())
}

fn inject_ai_hook_env(env: &mut std::collections::HashMap<String, String>) {
    let bin_dir = match crate::ai_hooks::extract::ensure_binaries_extracted() {
        Ok(p) => p,
        Err(e) => {
            log::warn!(
                "paneflow: AI-hook binary extraction failed ({e:#}); sidebar loader will not activate for this terminal session"
            );
            return;
        }
    };

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

fn prepend_bin_dir_to_path(
    env: &mut std::collections::HashMap<String, String>,
    bin_dir: &std::path::Path,
) {
    let existing: Option<std::ffi::OsString> = env
        .get("PATH")
        .map(std::ffi::OsString::from)
        .or_else(|| std::env::var_os("PATH"));

    let mut components: Vec<std::path::PathBuf> = vec![bin_dir.to_path_buf()];
    if let Some(existing) = existing.as_deref()
        && !existing.is_empty()
    {
        components.extend(std::env::split_paths(existing));
    }

    match std::env::join_paths(components) {
        Ok(joined) => {
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

fn is_loader_influencing_env_key(key: &str) -> bool {
    key.starts_with("LD_") || key.starts_with("DYLD_")
}

fn is_inherited_agent_session_env_key(key: &str) -> bool {
    INHERITED_AGENT_SESSION_ENV.contains(&key)
}

fn is_forbidden_child_env_key(key: &str) -> bool {
    is_inherited_agent_session_env_key(key) || is_loader_influencing_env_key(key)
}

pub(super) fn is_inherited_host_terminal_env_key(key: &str) -> bool {
    INHERITED_HOST_TERMINAL_ENV
        .iter()
        .any(|known| key.eq_ignore_ascii_case(known))
        || key.len() > CONEMU_ENV_PREFIX.len()
            && key[..CONEMU_ENV_PREFIX.len()].eq_ignore_ascii_case(CONEMU_ENV_PREFIX)
}

pub(super) fn inherited_env_keys_to_strip() -> Vec<std::ffi::OsString> {
    std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| {
            key.to_str().is_some_and(|key| {
                is_inherited_host_terminal_env_key(key) || is_inherited_agent_session_env_key(key)
            })
        })
        .collect()
}

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
        assert!(is_executable_path_title(
            r"C:\Program Files\PowerShell\7\pwsh.exe"
        ));
        assert!(is_executable_path_title(r"C:\Windows\System32\cmd.exe"));
        assert!(!is_executable_path_title("Claude Code"));
        assert!(!is_executable_path_title(r"C:\dev\paneflow"));
        assert!(!is_executable_path_title(""));
        assert!(!is_executable_path_title("pwsh.exe"));
    }
}

fn assemble_pty_env(
    mut env: std::collections::HashMap<String, String>,
    workspace_id: u64,
    surface_id: u64,
    user_env: Option<std::collections::HashMap<String, String>>,
) -> std::collections::HashMap<String, String> {
    if workspace_id != 0 {
        env.insert("PANEFLOW_WORKSPACE_ID".into(), workspace_id.to_string());
    }
    env.insert("PANEFLOW_SURFACE_ID".into(), surface_id.to_string());
    if let Some(socket_path) = paneflow_socket_path() {
        env.insert("PANEFLOW_SOCKET_PATH".into(), socket_path);
    }

    if let Some(log_path) = std::env::var_os("PANEFLOW_HOOK_LOG")
        .filter(|v| !v.is_empty())
        .map(|v| v.to_string_lossy().into_owned())
    {
        env.insert("PANEFLOW_HOOK_LOG".into(), log_path);
    }

    env.insert("TERM".into(), "xterm-256color".into());

    if std::env::var("LANG").map_or(true, |v| v.is_empty()) {
        env.insert("LANG".into(), "en_US.UTF-8".into());
    }

    env.insert("TERM_PROGRAM".into(), "paneflow".into());
    env.insert(
        "TERM_PROGRAM_VERSION".into(),
        env!("CARGO_PKG_VERSION").into(),
    );
    env.insert("COLORTERM".into(), "truecolor".into());

    env.insert("SHLVL".into(), "0".into());

    inject_ai_hook_env(&mut env);

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
            #[cfg(windows)]
            let k = k.to_uppercase();
            if !is_valid_env_name(&k) || is_forbidden_child_env_key(&k) {
                continue;
            }
            if PROTECTED.contains(&k.as_str()) {
                continue;
            }
            env.insert(k, v);
        }
    }

    env.retain(|k, _| !is_inherited_agent_session_env_key(k));
    reassert_paneflow_bin_dir_first(&mut env);

    env
}

impl Drop for TerminalState {
    fn drop(&mut self) {
        self.ghostty.shutdown();
        self.child_pid = 0;
    }
}

#[cfg(windows)]
pub(super) const WINDOWS_PROCESS_TREE_TERMINATION_BUDGET: std::time::Duration =
    std::time::Duration::from_secs(5);

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

    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }

    let mut entries: Vec<(u32, u32)> = Vec::with_capacity(256);
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    if unsafe { Process32FirstW(snap, &mut entry) } == 0 {
        let error = io::Error::last_os_error();
        unsafe { CloseHandle(snap) };
        return Err(error);
    }
    loop {
        entries.push((entry.th32ProcessID, entry.th32ParentProcessID));
        if unsafe { Process32NextW(snap, &mut entry) } == 0 {
            break;
        }
    }
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

#[cfg(windows)]
fn windows_wait_timeout_ms(remaining: std::time::Duration) -> Option<u32> {
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
    const SYNCHRONIZE: u32 = 0x0010_0000;

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

    if unsafe { TerminateProcess(process.handle, 1) } == 0 {
        let terminate_error = io::Error::last_os_error().raw_os_error();
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

    let mut pending = Vec::with_capacity(handles.len());
    for process in handles {
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

        match unsafe { WaitForSingleObject(process.handle, timeout_ms) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
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
    fn new_pending_terminal_has_no_child_until_promoted() {
        let (state, _pending) = TerminalState::new_pending(80, 24);
        assert_eq!(state.child_pid, 0);
    }

    #[test]
    fn spawn_failure_is_reported_once_without_sensitive_error_text() {
        const CANARY: &str =
            r#"C:\Users\synthetic-user\private\launch.ps1 --token super-secret-canary"#;
        let error = anyhow::Error::new(io::Error::from_raw_os_error(5)).context(CANARY);
        let os_error = raw_os_error_from_anyhow(&error);
        assert_eq!(os_error, Some(5));

        let failure = TerminalBackendFailureDiagnostics::new(
            TerminalBackendFailurePhase::OpenPty,
            TerminalBackendFailureDiagnostics::GHOSTTY_OPEN_PTY_FAILED,
            os_error,
        );
        let mut state = TerminalState::new_display_only(24, 80);
        state.report_spawn_failure(failure.clone(), "engine start failed");

        let diagnostics = state.backend_diagnostics();
        assert_eq!(diagnostics.failure, Some(failure));
        let formatted = diagnostics.to_string();
        assert_eq!(formatted.matches("reason_code=").count(), 1);
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
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(diagnostics.target_triple, "aarch64-apple-darwin");
    }

    #[test]
    fn backend_diagnostics_expose_pinned_ghostty_build_identity() {
        let diagnostics = TerminalState::new_display_only(24, 80).backend_diagnostics();
        let ghostty = diagnostics.ghostty;
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
        let (state, _events_tx) = TerminalState::new_pending(80, 24);
        state.write_to_pty(b"claude\r".to_vec());
        let queued = state.pending_input.lock().expect("pending_input lock");
        assert_eq!(queued.len(), 1);
        assert!(
            matches!(&queued[0], PendingTerminalInput::Raw(bytes) if bytes.as_ref() == b"claude\r")
        );
    }

    #[test]
    fn pending_input_is_bounded() {
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

    #[test]
    fn control_releases_fit_after_general_input_saturates() {
        let general_limit = MAX_PENDING_INPUT_BYTES - INPUT_CONTROL_RESERVE_BYTES;
        let press =
            PendingTerminalInput::Key(test_key_input(paneflow_terminal_ghostty::KeyAction::Press));
        let key_release = PendingTerminalInput::Key(test_key_input(
            paneflow_terminal_ghostty::KeyAction::Release,
        ));
        let mouse_release = PendingTerminalInput::Mouse {
            input: test_mouse_input(paneflow_terminal_ghostty::MouseAction::Release),
            repeat: 1,
        };
        let focus = PendingTerminalInput::Focus(paneflow_terminal_ghostty::FocusEvent::Lost);

        assert!(!press.fits_after(general_limit));
        assert!(key_release.fits_after(general_limit));
        assert!(mouse_release.fits_after(general_limit));
        assert!(focus.fits_after(general_limit));
    }

    #[test]
    fn structured_input_is_queued_in_order_before_promotion() {
        let (state, _pending) = TerminalState::new_pending(80, 24);

        assert_eq!(
            state.write_ghostty_key(test_key_input(paneflow_terminal_ghostty::KeyAction::Press)),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_mouse(
                test_mouse_input(paneflow_terminal_ghostty::MouseAction::Press),
                2,
            ),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_focus(paneflow_terminal_ghostty::FocusEvent::Gained),
            BackendInputResult::Accepted
        );
        assert_eq!(
            state.write_ghostty_paste("paste".to_string()),
            BackendInputResult::Accepted
        );

        let queued = state.pending_input.lock().expect("pending_input lock");
        assert_eq!(queued.len(), 4);
        assert!(matches!(queued[0], PendingTerminalInput::Key(_)));
        assert!(matches!(queued[1], PendingTerminalInput::Mouse { .. }));
        assert!(matches!(queued[2], PendingTerminalInput::Focus(_)));
        assert!(matches!(queued[3], PendingTerminalInput::Paste { .. }));
    }

    #[test]
    fn resolve_spawn_params_honors_initial_size() {
        let p = TerminalState::resolve_spawn_params(None, 1, 1, Some((100, 30)), None);
        assert_eq!((p.cols, p.rows), (100, 30));
        let d = TerminalState::resolve_spawn_params(None, 1, 1, None, None);
        assert_eq!((d.cols, d.rows), (120, 40));
    }

    #[cfg(unix)]
    #[test]
    fn capture_foreground_signal_mask_succeeds_on_unix() {
        assert!(capture_foreground_signal_mask().is_some());
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

    #[test]
    fn child_exit_records_real_code_not_sentinel() {
        let mut state = TerminalState::new_display_only(24, 80);
        assert!(state.exited.is_none(), "fresh terminal has no exit code");

        state.process_ghostty_event(GhosttyUiEvent::ChildExited {
            code: 42,
            signal: None,
        });
        assert_eq!(
            state.exited,
            Some(42),
            "US-003: the real exit code must be recorded, not -1"
        );
    }

    #[test]
    fn exit_fallback_does_not_clobber_real_child_exit_code() {
        let mut state = TerminalState::new_display_only(24, 80);

        state.process_ghostty_event(GhosttyUiEvent::ChildExited {
            code: 1,
            signal: None,
        });
        state.process_ghostty_event(GhosttyUiEvent::RuntimeFailed(
            "engine mailbox closed".to_owned(),
        ));
        assert_eq!(
            state.exited,
            Some(1),
            "US-003: a later failure must not clobber the real exit code"
        );
    }

    #[test]
    fn close_on_exit_discriminator_covers_both_branches() {
        let mut clean = TerminalState::new_display_only(24, 80);
        clean.exited = Some(0);
        assert!(
            clean.should_close_on_exit(),
            "US-002: a clean exit (code 0) must close the pane"
        );

        let mut failed = TerminalState::new_display_only(24, 80);
        failed.exited = Some(127);
        assert!(
            !failed.should_close_on_exit(),
            "US-002: a non-zero exit with no input must keep the pane open"
        );

        failed.write_to_pty(b"x".as_slice());
        assert!(
            failed.should_close_on_exit(),
            "US-002: after user input, a non-zero exit must close the pane"
        );
    }

    #[test]
    fn write_to_pty_marks_user_input_but_fresh_state_does_not() {
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
        let mut base = HashMap::new();
        let mut user = HashMap::new();
        for key in INHERITED_AGENT_SESSION_ENV {
            base.insert((*key).to_string(), "inherited".to_string());
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

    #[test]
    fn host_terminal_markers_are_recognized_whatever_their_casing() {
        for key in INHERITED_HOST_TERMINAL_ENV {
            assert!(
                is_inherited_host_terminal_env_key(key),
                "{key} is listed and must be recognized"
            );
            assert!(
                is_inherited_host_terminal_env_key(&key.to_lowercase()),
                "{key} must be recognized case-insensitively"
            );
        }
        for key in ["ConEmuANSI", "ConEmuPID", "ConEmuTask", "CONEMUBUILD"] {
            assert!(
                is_inherited_host_terminal_env_key(key),
                "{key} must be matched by the ConEmu prefix rule"
            );
        }
    }

    #[test]
    fn host_terminal_matcher_does_not_swallow_unrelated_names() {
        for key in [
            "conemu",
            "CONEMU",
            "TERM",
            "TERM_PROGRAM",
            "TMUXINATOR_CONFIG",
            "STYLE",
            "PATH",
            "KITTY_WINDOW_IDS",
            "PANEFLOW_SURFACE_ID",
        ] {
            assert!(
                !is_inherited_host_terminal_env_key(key),
                "{key} must survive - it is not a host-terminal identity marker"
            );
        }
    }

    #[test]
    fn the_strip_list_covers_both_families_it_claims_to() {
        for key in INHERITED_AGENT_SESSION_ENV {
            assert!(
                is_inherited_agent_session_env_key(key) || is_inherited_host_terminal_env_key(key),
                "{key} must be stripped from the inherited env, not just the map"
            );
        }
        assert!(
            !is_inherited_agent_session_env_key("PANEFLOW_SURFACE_ID")
                && !is_inherited_host_terminal_env_key("PANEFLOW_SURFACE_ID"),
            "the strip must not reach a variable Paneflow sets for the pane"
        );
    }

    #[test]
    fn host_terminal_markers_are_not_smuggled_through_the_assembled_env() {
        let env = assemble_pty_env(HashMap::new(), 1, 1, None);
        for key in env.keys() {
            assert!(
                !is_inherited_host_terminal_env_key(key),
                "assemble_pty_env must never introduce the host marker {key}"
            );
        }
        assert_eq!(
            env.get("TERM_PROGRAM").map(String::as_str),
            Some("paneflow")
        );
    }

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

        let services = state.detect_services_in_lines(&[
            "▲ Next.js 16.1.6".to_string(),
            "- Local: http://localhost:3000".to_string(),
        ]);

        assert_eq!(services.len(), 1);
        assert_eq!(services[0].port, 3000);
        assert_eq!(services[0].label.as_deref(), Some("Next.js"));
        assert!(services[0].is_frontend);
    }

    #[test]
    fn scan_output_dedups_until_port_leaves_live_set() {
        let mut state = TerminalState::new_display_only(24, 80);
        let lines = ["Vite ready at http://localhost:5173".to_string()];

        assert_eq!(state.detect_services_in_lines(&lines).len(), 1);
        assert!(state.detect_services_in_lines(&lines).is_empty());

        state.retain_reported_ports(&[]);
        let services = state.detect_services_in_lines(&lines);

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

    #[test]
    fn cwd_now_none_for_display_only() {
        let state = TerminalState::new_display_only(24, 80);
        assert_eq!(state.child_pid, 0);
        assert!(
            state.cwd_now().is_none(),
            "display-only terminal has no shell CWD to resolve"
        );
    }

    #[test]
    fn pending_clipboard_ops_are_bounded() {
        let mut state = TerminalState::new_display_only(5, 20);

        for i in 0..(MAX_PENDING_CLIPBOARD_OPS + 2) {
            state.queue_clipboard_op(format!("op-{i}"));
        }

        assert_eq!(state.pending_clipboard_ops.len(), MAX_PENDING_CLIPBOARD_OPS);
        assert_eq!(state.pending_clipboard_ops[0], "op-2");
    }

    #[test]
    fn osc52_store_requires_focus_and_respects_the_shared_cap() {
        let mut state = TerminalState::new_display_only(5, 20);
        state.set_osc52_mode(Osc52Mode::CopyOnly);

        state.deliver_clipboard_text("unfocused".into());
        assert!(state.pending_clipboard_ops.is_empty());

        state.set_terminal_focused(true);
        state.deliver_clipboard_text("focused".into());
        assert_eq!(state.pending_clipboard_ops, vec!["focused".to_string()]);

        state.deliver_clipboard_text("x".repeat(MAX_OSC52_BYTES + 1));
        assert_eq!(state.pending_clipboard_ops.len(), 1);

        state.set_terminal_focused(false);
        state.deliver_clipboard_text("lost-focus".into());
        assert_eq!(state.pending_clipboard_ops.len(), 1);
    }

    #[test]
    fn restore_scrollback_strips_escape_and_osc_injection() {
        let hostile = "\x1b]8;;https://evil.example/\x07click\x1b]8;;\x07\
                       \x1b]0;PWNED\x07\x1b[31mred\x00\u{9b}38m";
        let state = TerminalState::new_display_only(6, 80);
        state.restore_scrollback(hostile);
        state.restore_scrollback("a\tb");

        assert_eq!(state.title, "Terminal", "OSC 0 must not retitle the pane");

        let backend = state.session_backend();
        let restored = (0..6)
            .filter_map(|row| backend.line_text_at(Point::new(row, 0)))
            .map(|line| line.text)
            .collect::<Vec<_>>()
            .join("\n");
        for marker in ["https://evil.example/", "click", "PWNED", "red", "38m"] {
            assert!(
                restored.contains(marker),
                "plain glyphs must survive; {marker:?} missing from {restored:?}"
            );
        }
        assert!(
            !restored.contains('\x1b') && !restored.contains('\x07'),
            "no VT introducer may reach the grid; got {restored:?}"
        );
    }

    #[test]
    fn extract_scrollback_drains_history_only() {
        let state = TerminalState::new_display_only(3, 80);
        state.restore_scrollback("history-alpha\nhistory-bravo\nvisible-charlie\nvisible-delta");

        let drained = state
            .extract_scrollback()
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
        let mut state = TerminalState::new(None, 1, 1, Some((80, 24)), None, None)
            .expect("spawn a PTY-backed terminal");
        assert_eq!(
            state.output_generation, 0,
            "a fresh terminal has produced no output"
        );

        std::thread::sleep(std::time::Duration::from_millis(250));
        state.write_to_pty_silent(b"echo PANEFLOW_GEN_OK\n".to_vec());

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
