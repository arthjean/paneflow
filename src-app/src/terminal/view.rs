//! GPUI view layer for a single terminal pane.
//!
//! Holds the `TerminalView` struct, its constructor + event batch loop,
//! IME wiring, URL hover detection, the `TerminalEvent` enum emitted to
//! consumers (pane / app), and the `Render` impl that composes
//! `TerminalElement` with the search overlay and copy-mode badge.
//!
//! Extracted from `terminal.rs` per US-016 of the src-app refactor PRD.

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyContext, MouseButton, Render, Styled, Window, div, prelude::*,
};
use paneflow_config::schema::{TerminalBackendConfig, TerminalConfig, TerminalSurfaceProfile};

use super::TerminalState;
use super::element::TerminalElement;
use super::pty_session::{
    ClipboardOp, TerminalBackendEvent, TerminalBackendFailureDiagnostics,
    TerminalBackendFailurePhase, raw_os_error_from_anyhow,
};
use super::service_detector::ServiceInfo;
use super::types::{
    CopyModeCursorState, CursorShape, HyperlinkZone, Line, Modes, Point, SearchHighlight,
    TerminalWindowSize, terminal_metric_to_u16,
};
use crate::limits::MAX_OSC52_BYTES;
use crate::ui_primitives::{AnimatedHoverExt, lerp_color};

#[cfg(any(
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
use super::ghostty_session::{GhosttySession, GhosttyStartError, SpawnedGhostty};

#[cfg(any(
    test,
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
fn should_start_ghostty_for_policy(
    requested: TerminalBackendConfig,
    available: bool,
    auto_selects: bool,
) -> bool {
    available && policy_requests_ghostty(requested, auto_selects)
}

fn policy_requests_ghostty(requested: TerminalBackendConfig, auto_selects: bool) -> bool {
    match requested {
        TerminalBackendConfig::Auto => auto_selects,
        TerminalBackendConfig::Ghostty => true,
        TerminalBackendConfig::Alacritty => false,
    }
}

fn auto_selects_ghostty_for_target() -> bool {
    cfg!(any(
        target_os = "linux",
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc"
        )
    ))
}

#[cfg(any(
    test,
    all(target_os = "linux", feature = "libghostty-linux"),
    all(
        target_os = "windows",
        target_arch = "x86_64",
        target_env = "msvc",
        feature = "libghostty-windows"
    )
))]
fn should_start_ghostty(requested: TerminalBackendConfig) -> bool {
    should_start_ghostty_for_policy(
        requested,
        cfg!(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        )),
        auto_selects_ghostty_for_target(),
    )
}

enum BackgroundSpawnOutcome {
    Alacritty(anyhow::Result<super::pty_session::SpawnedPty>),
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    Ghostty(SpawnedGhostty),
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    GhosttyFallback {
        spawned: anyhow::Result<super::pty_session::SpawnedPty>,
        failure: TerminalBackendFailureDiagnostics,
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
    GhosttyPostSpawnFailed {
        child_pid: u32,
        failure: TerminalBackendFailureDiagnostics,
    },
}

#[cfg(all(target_os = "linux", feature = "libghostty-linux"))]
fn should_render_ghostty_wakeup_immediately(event: &TerminalBackendEvent) -> bool {
    matches!(event, TerminalBackendEvent::Ghostty(_))
}

#[cfg(not(all(target_os = "linux", feature = "libghostty-linux")))]
fn should_render_ghostty_wakeup_immediately(_event: &TerminalBackendEvent) -> bool {
    false
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
fn warn_ghostty_unavailable_once() {
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        log::warn!(
            target: "paneflow::terminal::backend",
            "Ghostty backend requested but unavailable on this platform/build; using Alacritty"
        );
    });
}

fn log_backend_diagnostics(terminal: &TerminalState) {
    let diagnostics = terminal.backend_diagnostics();
    log::info!(
        target: "paneflow::terminal::backend",
        "Terminal backend selected: {diagnostics}"
    );
}

// ---------------------------------------------------------------------------
// Debug latency probes - zero overhead in release builds
// ---------------------------------------------------------------------------

/// Check once whether PANEFLOW_LATENCY_PROBE=1 is set.
/// Cached in a OnceLock so the env var is read only on first call.
#[cfg(debug_assertions)]
pub(crate) fn probe_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("PANEFLOW_LATENCY_PROBE").as_deref() == Ok("1"))
}

/// Human-readable in-pane message for a failed PTY spawn - written into the
/// display-only placeholder kept by the US-012 background-spawn path (and the
/// old synchronous fallback). ANSI-formatted; `\r\n` because there is no PTY to
/// translate bare `\n`.
fn spawn_error_message(e: &anyhow::Error) -> String {
    format!(
        "\x1b[1;31mError\x1b[0m: failed to start shell.\r\n\
         \r\n\
         Common causes:\r\n\
         \x20 \x20- PTY pool exhausted\r\n\
         \x20 \x20- Shell binary not found ($SHELL / default_shell)\r\n\
         \x20 \x20- Permission denied on /dev/ptmx\r\n\
         \r\n\
         \x1b[2m{e:#}\x1b[0m\r\n",
    )
}

fn renderer_cursor_shape_from_config(
    shape: paneflow_config::schema::CursorShapeConfig,
) -> CursorShape {
    use paneflow_config::schema::CursorShapeConfig as C;
    match shape {
        C::Vintage => CursorShape::Vintage,
        C::Block => CursorShape::Block,
        C::Beam => CursorShape::Beam,
        C::Underline => CursorShape::Underline,
        C::DoubleUnderline => CursorShape::DoubleUnderline,
        C::Hollow => CursorShape::HollowBlock,
    }
}

pub(crate) fn hsla_from_hex_color(raw: &str) -> Option<Hsla> {
    let normalized = paneflow_config::schema::normalize_hex_color(raw)?;
    let rgb = u32::from_str_radix(&normalized[1..], 16).ok()?;
    Some(Hsla::from(gpui::rgb(rgb)))
}

fn cursor_color_override_from_config(terminal_config: &TerminalConfig) -> Option<Hsla> {
    terminal_config
        .cursor_color
        .as_deref()
        .and_then(hsla_from_hex_color)
}

/// Strip control characters from an OSC 52 clipboard payload so a hostile PTY
/// program can't plant a paste-injection (U-023). Keeps TAB and LF (legitimate
/// in clipboard text); drops CR (the byte that commits a line on paste into a
/// non-bracketed context), ESC (the ANSI intro), every other C0 control, DEL,
/// and the C1 range (U+0080-U+009F). Applied symmetrically to the Store (write)
/// and Load (read) paths so they can't drift apart again - `char::is_control()`
/// already covers C0 + DEL + C1.
pub(super) fn sanitize_osc52(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\t' || c == '\n' || !c.is_control())
        .collect()
}

// ---------------------------------------------------------------------------
// Terminal View - GPUI Render impl
// ---------------------------------------------------------------------------

// US-006: cursor blink interval moved to `terminal::blink::CURSOR_BLINK_INTERVAL`.
// The blink itself is now driven by a single app-scoped `BlinkPhase` entity
// observed by every `TerminalView`, replacing the per-terminal `smol::Timer`
// loop that lived here.

/// US-015: stable authority for an in-progress scrollbar drag. Geometry is
/// frozen at grab time so output and reflow cannot rescale the gesture, while
/// `last_target` suppresses duplicate line targets from dense pointer events.
#[derive(Clone, Copy)]
pub(super) struct ScrollbarDrag {
    pub(super) anchor_y: gpui::Pixels,
    pub(super) anchor_offset: usize,
    pub(super) metrics: super::element::ScrollbarMetrics,
    pub(super) last_target: usize,
}

#[derive(Clone)]
pub(super) struct HoverLinkCache {
    line: Line,
    cwd: Option<String>,
    line_text: String,
    zones: Vec<HyperlinkZone>,
}

pub struct TerminalView {
    pub terminal: TerminalState,
    focus_handle: FocusHandle,
    pub(super) cursor_visible: bool,
    /// Track mouse button state for drag selection
    pub(super) selecting: bool,
    /// Last known cell dimensions (from element::resolve_frame_metrics)
    pub(super) cell_width: gpui::Pixels,
    pub(super) line_height: gpui::Pixels,
    /// Element origin in window coordinates - set by TerminalElement::paint(),
    /// read by mouse handlers for pixel→grid conversion.
    pub(super) element_origin: Arc<Mutex<gpui::Point<gpui::Pixels>>>,
    /// US-015: painted scrollbar geometry - set by TerminalElement::paint(),
    /// read by the mouse handlers to hit-test click-to-jump / drag.
    pub(super) scrollbar_metrics: Arc<Mutex<Option<super::element::ScrollbarMetrics>>>,
    /// US-015: active scrollbar drag, or `None`. Holds the cursor Y and the
    /// `display_offset` captured at grab time; moves apply the pixel delta
    /// RELATIVE to this anchor, so grabbing the thumb anywhere never makes it
    /// jump. Set in `handle_mouse_down`, cleared on left mouse-up.
    pub(super) scrollbar_drag: Option<ScrollbarDrag>,
    /// Sub-line scroll accumulator for smooth trackpad scrolling
    pub(super) scroll_remainder: f32,
    /// Whether the search overlay is visible
    pub(super) search_active: bool,
    /// Real single-line input backing the find bar - the same `TextInput`
    /// widget the Agents sidebar uses. Focused on open so keystrokes land in
    /// the field (cursor, selection, IME, clipboard) instead of the PTY.
    pub(super) search_input: gpui::Entity<crate::widgets::text_input::TextInput>,
    /// Current search query string (kept in sync with `search_input` via
    /// `cx.observe`; the source of truth for match scanning + the counter).
    pub(super) search_query: String,
    /// Monotonic token used to discard stale async local-search results.
    pub(super) search_generation: u64,
    /// Cached search matches (grid coordinates)
    pub(super) search_matches: Vec<crate::search::SearchMatch>,
    /// Index of the currently focused match (for navigation)
    pub(super) search_current: usize,
    /// Whether regex search mode is active (vs plain text)
    pub(super) search_regex_mode: bool,
    /// Regex compilation error message (None when valid or plain text mode)
    pub(super) search_regex_error: Option<String>,
    /// Whether Alt key is treated as Meta (ESC prefix). Read from config.
    pub(super) option_as_meta: bool,
    /// US-008: cursor blink override (On / Off / TerminalControlled). Read
    /// from config at construction.
    pub(super) cursor_blink_mode: paneflow_config::schema::CursorBlinkConfig,
    /// Default cursor shape used before applications override it through
    /// DECSCUSR. Custom shapes are painted by Paneflow.
    pub(super) default_cursor_shape: CursorShape,
    /// Cursor color override from `terminal.cursor_color`; `None` keeps the
    /// active color scheme cursor color.
    pub(super) cursor_color_override: Option<Hsla>,
    /// US-022: resolved scroll-wheel multiplier for scrollback (1.0 = default).
    /// Read from config at construction (like the cursor settings) - NOT
    /// per scroll event, so the hot scroll path does no config I/O. Takes
    /// effect on the next new terminal, consistent with the other terminal
    /// settings here.
    pub(super) scroll_multiplier: f32,
    /// Platform appearance switch: default terminal backgrounds become
    /// transparent so the native window material can show through.
    pub(super) terminal_material_active: bool,
    /// Renderer switch: block elements use Paneflow's built-in quad renderer
    /// instead of font glyphs.
    pub(super) integrated_glyphs_enabled: bool,
    /// Renderer switch: emoji glyphs use GPUI's platform color-emoji path.
    pub(super) color_emoji_enabled: bool,
    /// Whether copy mode (keyboard-driven selection) is active
    pub(super) copy_mode_active: bool,
    /// Copy mode cursor position in grid coordinates
    pub(super) copy_cursor: Point,
    /// Display offset frozen at copy mode entry to prevent auto-scroll
    pub(super) copy_mode_frozen_offset: usize,
    /// Previous focus state, used to detect focus transitions for DEC 1004 events.
    was_focused: bool,
    /// Focus subscriptions update the clipboard gate at event time, before
    /// queued terminal output can reach the GPUI event drain.
    focus_subscriptions: Option<(gpui::Subscription, gpui::Subscription)>,
    /// Key presses accepted by Ghostty and therefore eligible for a matching
    /// release event. Prevents app-consumed shortcuts from leaking key-up data.
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) ghostty_pressed_keys:
        std::collections::HashMap<String, paneflow_terminal_ghostty::KeyInput>,
    /// Printable key metadata held until GPUI commits the final text. This
    /// keeps IME as the single text source while still giving Kitty encoding
    /// the logical key, modifiers, repeat action, and matching release.
    #[cfg(any(
        all(target_os = "linux", feature = "libghostty-linux"),
        all(
            target_os = "windows",
            target_arch = "x86_64",
            target_env = "msvc",
            feature = "libghostty-windows"
        )
    ))]
    pub(super) ghostty_pending_text_key:
        Option<(gpui::Keystroke, paneflow_terminal_ghostty::KeyAction, bool)>,
    /// Last hovered cell position for URL regex detection (US-015).
    pub(super) hovered_cell: Option<Point>,
    /// Active hyperlink under Ctrl+hover - drives underline rendering and Ctrl+click.
    pub(super) ctrl_hovered_link: Option<HyperlinkZone>,
    /// Last full-line link detection result. Avoids repeating canonicalize on
    /// every mouse move while the pointer stays on the same terminal line.
    pub(super) hover_link_cache: Option<HoverLinkCache>,
    /// US-012: the link under the cursor at modifier+mouse-down. The open is
    /// deferred to mouse-up and fires only if no drag occurred (empty
    /// selection), so a Ctrl+drag starting on a link selects text instead of
    /// opening it. Mirrors Zed's mouse_down/up hyperlink match.
    pub(super) mouse_down_link: Option<HyperlinkZone>,
    /// IME preedit text (in-progress composition). Empty when no composition active.
    ime_marked_text: String,
    /// Gate for clearing pre-resize shell startup content on first render.
    /// The PTY is spawned before the first `build_layout()` measures the actual
    /// window dimensions, so shell init bytes land in a 120×40 grid. After the
    /// first resize we clear the grid so those garbled bytes don't appear.
    needs_initial_clear: Arc<std::sync::atomic::AtomicBool>,
    /// Last window size measured by `TerminalElement::build_layout`.
    terminal_window_size: Arc<Mutex<Option<TerminalWindowSize>>>,
}

impl TerminalView {
    fn recorded_window_size(&self) -> Option<TerminalWindowSize> {
        *self
            .terminal_window_size
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn current_window_size(&self) -> TerminalWindowSize {
        if let Some(size) = self.recorded_window_size() {
            return size;
        }

        let (cols, rows) = self.terminal.session_backend().grid_size();
        TerminalWindowSize::new(
            cols,
            rows,
            terminal_metric_to_u16(self.cell_width.as_f32()),
            terminal_metric_to_u16(self.line_height.as_f32()),
        )
    }

    fn process_dirty_terminal(&mut self, cx: &mut Context<Self>) {
        if !self.terminal.dirty {
            return;
        }
        self.terminal.dirty = false;

        // Leading edge + throttle (replaces the old every-10th-tick
        // modulo): the first dirty update after a quiet spell fires
        // immediately, while sustained output re-fires at most every 300ms.
        const BURST_THROTTLE: std::time::Duration = std::time::Duration::from_millis(300);
        let now = std::time::Instant::now();
        if self
            .terminal
            .last_activity_burst
            .is_none_or(|t| now.duration_since(t) >= BURST_THROTTLE)
        {
            self.terminal.last_activity_burst = Some(now);
            for service in self.terminal.scan_output() {
                cx.emit(TerminalEvent::ServiceDetected(service));
            }
            cx.emit(TerminalEvent::ActivityBurst);
        }

        if self.copy_mode_active {
            self.terminal
                .session_backend()
                .restore_display_offset(self.copy_mode_frozen_offset);
        }

        cx.notify();
    }

    pub(crate) fn restore_scrollback(&self, text: &str) {
        self.needs_initial_clear
            .store(false, std::sync::atomic::Ordering::Relaxed);
        self.terminal.restore_scrollback(text);
    }

    pub(crate) fn set_terminal_material_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.terminal_material_active != active {
            self.terminal_material_active = active;
            cx.notify();
        }
    }

    pub(crate) fn set_integrated_glyphs_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.integrated_glyphs_enabled != enabled {
            self.integrated_glyphs_enabled = enabled;
            cx.notify();
        }
    }

    pub(crate) fn set_color_emoji_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.color_emoji_enabled != enabled {
            self.color_emoji_enabled = enabled;
            cx.notify();
        }
    }

    pub(crate) fn set_cursor_color_override(
        &mut self,
        color: Option<Hsla>,
        cx: &mut Context<Self>,
    ) {
        if self.cursor_color_override != color {
            self.cursor_color_override = color;
            self.terminal.cursor_color_override = color;
            cx.notify();
        }
    }

    pub fn new(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        Self::with_cwd(workspace_id, None, None, cx)
    }

    pub fn with_cwd(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_and_env(workspace_id, cwd, initial_size, None, cx)
    }

    pub fn with_cwd_and_profile(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        profile: TerminalSurfaceProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_env_and_profile(workspace_id, cwd, initial_size, None, profile, cx)
    }

    /// Spawn a terminal with an explicit per-surface env map (US-014). The
    /// global `terminal.env` default is merged underneath in
    /// [`TerminalState::new`]; `user_env` here is the per-surface override
    /// (surface wins on key collision). Use this from the session-restore path
    /// where a [`SurfaceDefinition::env`] is present.
    pub fn with_cwd_and_env(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::with_cwd_env_and_profile(
            workspace_id,
            cwd,
            initial_size,
            user_env,
            TerminalSurfaceProfile::Normal,
            cx,
        )
    }

    pub fn with_cwd_env_and_profile(
        workspace_id: u64,
        cwd: Option<std::path::PathBuf>,
        initial_size: Option<(usize, usize)>,
        user_env: Option<std::collections::HashMap<String, String>>,
        profile: TerminalSurfaceProfile,
        cx: &mut Context<Self>,
    ) -> Self {
        let surface_id = cx.entity_id().as_u64();

        // US-012: paint immediately. Phase 1 - resolve the (cheap) spawn params
        // and build a display-only placeholder on the render thread. Phase 2 -
        // open the PTY on the background executor and `promote()` the
        // placeholder in place when it resolves, so an N-pane restore never
        // serializes N blocking spawns on the main thread.
        let params = TerminalState::resolve_spawn_params_with_profile(
            cwd,
            workspace_id,
            surface_id,
            initial_size,
            user_env,
            profile,
        );
        let (mut terminal, pending) = TerminalState::new_pending_with_profile_and_shell_quoting(
            params.cols,
            params.rows,
            params.profile,
            params.shell_quoting,
        );
        let requested_backend = paneflow_config::loader::load_config()
            .terminal
            .unwrap_or_default()
            .backend;
        terminal.set_backend_request(requested_backend);

        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        let ghostty_spawn = if should_start_ghostty(requested_backend) {
            let initial_window_size = TerminalWindowSize::new(params.cols, params.rows, 0, 0);
            let (ghostty, runtime_pending, events_rx) = GhosttySession::pending_with_clipboard_gate(
                initial_window_size,
                terminal.clipboard_gate(),
            );
            terminal.attach_ghostty(ghostty.clone(), events_rx);
            Some((ghostty, runtime_pending))
        } else {
            None
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
        if policy_requests_ghostty(requested_backend, auto_selects_ghostty_for_target()) {
            warn_ghostty_unavailable_once();
            terminal.record_backend_failure(TerminalBackendFailureDiagnostics::new(
                TerminalBackendFailurePhase::Availability,
                TerminalBackendFailureDiagnostics::GHOSTTY_UNAVAILABLE,
                None,
            ));
        }
        // Route the Drop-time force-kill timer through GPUI's background
        // executor instead of a detached OS thread (Zed parity, prevents a
        // thread leak per closed pane under heavy use).
        terminal.set_background_executor(cx.background_executor().clone());
        // The background `EventLoop` attaches to this same shared `term` + event
        // channel, so the placeholder's event loop keeps working after promotion
        // - no view-side rewiring needed.
        // Capture the foreground signal mask on the MAIN thread so the
        // background-spawned child still gets correct Ctrl-C / Ctrl-Z (US-012).
        let signal_mask = crate::terminal::pty_session::capture_foreground_signal_mask();

        let view = Self::from_terminal_state(workspace_id, terminal, cx);

        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // Blocking PTY open (`tty::new` forks) runs off the render thread.
                let outcome = executor
                    .spawn(async move {
                        #[cfg(any(
                            all(target_os = "linux", feature = "libghostty-linux"),
                            all(
                                target_os = "windows",
                                target_arch = "x86_64",
                                target_env = "msvc",
                                feature = "libghostty-windows"
                            )
                        ))]
                        if let Some((ghostty, runtime_pending)) = ghostty_spawn {
                            let max_scrollback = paneflow_config::loader::load_config()
                                .terminal
                                .unwrap_or_default()
                                .resolved_scrollback_lines_for_profile(params.profile);
                            match ghostty.start(
                                runtime_pending,
                                params.clone(),
                                signal_mask,
                                max_scrollback,
                            ) {
                                Ok(spawned) => BackgroundSpawnOutcome::Ghostty(spawned),
                                Err(GhosttyStartError::Initialization(error)) => {
                                    let failure = TerminalBackendFailureDiagnostics::new(
                                        TerminalBackendFailurePhase::Initialization,
                                        TerminalBackendFailureDiagnostics::GHOSTTY_INITIALIZATION_FAILED,
                                        raw_os_error_from_anyhow(&error),
                                    );
                                    let spawned = TerminalState::open_pty_and_eventloop(
                                        params,
                                        pending,
                                        signal_mask,
                                    );
                                    BackgroundSpawnOutcome::GhosttyFallback {
                                        spawned,
                                        failure,
                                    }
                                }
                                Err(GhosttyStartError::OpenPty(error)) => {
                                    let failure = TerminalBackendFailureDiagnostics::new(
                                        TerminalBackendFailurePhase::OpenPty,
                                        TerminalBackendFailureDiagnostics::GHOSTTY_OPEN_PTY_FAILED,
                                        raw_os_error_from_anyhow(&error),
                                    );
                                    let spawned = TerminalState::open_pty_and_eventloop(
                                        params,
                                        pending,
                                        signal_mask,
                                    );
                                    BackgroundSpawnOutcome::GhosttyFallback {
                                        spawned,
                                        failure,
                                    }
                                }
                                Err(GhosttyStartError::Spawn(error)) => {
                                    let failure = TerminalBackendFailureDiagnostics::new(
                                        TerminalBackendFailurePhase::Spawn,
                                        TerminalBackendFailureDiagnostics::GHOSTTY_SPAWN_FAILED,
                                        raw_os_error_from_anyhow(&error),
                                    );
                                    let spawned = TerminalState::open_pty_and_eventloop(
                                        params,
                                        pending,
                                        signal_mask,
                                    );
                                    BackgroundSpawnOutcome::GhosttyFallback {
                                        spawned,
                                        failure,
                                    }
                                }
                                Err(GhosttyStartError::PostSpawn { child_pid, error }) => {
                                    BackgroundSpawnOutcome::GhosttyPostSpawnFailed {
                                        child_pid,
                                        failure: TerminalBackendFailureDiagnostics::new(
                                            TerminalBackendFailurePhase::PostSpawn,
                                            TerminalBackendFailureDiagnostics::GHOSTTY_POST_SPAWN_FAILED,
                                            raw_os_error_from_anyhow(&error),
                                        ),
                                    }
                                }
                            }
                        } else {
                            BackgroundSpawnOutcome::Alacritty(
                                TerminalState::open_pty_and_eventloop(
                                    params,
                                    pending,
                                    signal_mask,
                                ),
                            )
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
                            BackgroundSpawnOutcome::Alacritty(
                                TerminalState::open_pty_and_eventloop(
                                    params,
                                    pending,
                                    signal_mask,
                                ),
                            )
                        }
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    match outcome {
                        BackgroundSpawnOutcome::Alacritty(Ok(spawned)) => {
                            view.terminal.promote(spawned);
                            if let Some(size) = view.recorded_window_size() {
                                view.terminal.notify_window_size(size);
                            }
                        }
                        BackgroundSpawnOutcome::Alacritty(Err(error)) => {
                            // Spawn failed: keep the display-only placeholder and
                            // surface the error in-pane (no orphan, no panic - same
                            // outcome as the old synchronous fallback).
                            log::error!(
                                target: "paneflow::terminal::backend",
                                "Alacritty startup failed: reason_code=alacritty_startup_failed os_error={:?}",
                                raw_os_error_from_anyhow(&error),
                            );
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            view.terminal
                                .write_output(spawn_error_message(&error).as_bytes());
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
                        BackgroundSpawnOutcome::Ghostty(spawned) => {
                            view.terminal.promote_ghostty(spawned);
                            if let Some(size) = view.recorded_window_size() {
                                view.terminal.notify_window_size(size);
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
                        BackgroundSpawnOutcome::GhosttyFallback { spawned, failure } => {
                            log::warn!(
                                target: "paneflow::terminal::backend",
                                "Ghostty startup failed before child creation; using Alacritty: failure_phase={} reason_code={} os_error={:?}",
                                failure.phase.as_str(),
                                failure.reason_code,
                                failure.os_error,
                            );
                            view.terminal.fallback_from_ghostty(failure);
                            match spawned {
                                Ok(spawned) => {
                                    view.terminal.promote(spawned);
                                    if let Some(size) = view.recorded_window_size() {
                                        view.terminal.notify_window_size(size);
                                    }
                                }
                                Err(error) => {
                                    log::error!(
                                        target: "paneflow::terminal::backend",
                                        "Alacritty startup failed after Ghostty fallback: reason_code=alacritty_startup_failed os_error={:?}",
                                        raw_os_error_from_anyhow(&error),
                                    );
                                    view.needs_initial_clear.store(
                                        false,
                                        std::sync::atomic::Ordering::Relaxed,
                                    );
                                    view.terminal
                                        .write_output(spawn_error_message(&error).as_bytes());
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
                        BackgroundSpawnOutcome::GhosttyPostSpawnFailed {
                            child_pid,
                            failure,
                        } => {
                            log::error!(
                                target: "paneflow::terminal::backend",
                                "Ghostty startup failed after child creation (pid={child_pid}); refusing Alacritty child fallback: failure_phase={} reason_code={} os_error={:?}",
                                failure.phase.as_str(),
                                failure.reason_code,
                                failure.os_error,
                            );
                            view.terminal.fail_ghostty_after_spawn(failure);
                            view.needs_initial_clear
                                .store(false, std::sync::atomic::Ordering::Relaxed);
                            view.terminal.write_output(
                                spawn_error_message(&anyhow::anyhow!(
                                    "ghostty_post_spawn_failed"
                                ))
                                .as_bytes(),
                            );
                        }
                    }
                    log_backend_diagnostics(&view.terminal);
                    cx.notify();
                });
            },
        )
        .detach();

        view
    }

    fn from_terminal_state(
        _workspace_id: u64,
        mut terminal: TerminalState,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();

        // Find bar input - same widget as the Agents sidebar filter. Observe it
        // so every keystroke re-runs the in-buffer search (no submit needed).
        let search_input =
            cx.new(|cx| crate::widgets::text_input::TextInput::new("", "Search", cx));
        cx.observe(&search_input, |this, _input, cx| {
            this.on_search_input_changed(cx);
        })
        .detach();

        // Backend event coalescing:
        // Phase 1: Block until first event (zero CPU when idle)
        // Phase 2: Render the leading Linux Ghostty wakeup immediately. Windows
        // Ghostty and Alacritty wait 4ms (max 100, dedup Wakeup) so ConPTY cannot
        // expose a partial multi-write terminal frame.
        // Phase 3: Process batch, yield to other GPUI tasks
        let events_rx = terminal.take_backend_events();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut events_rx = events_rx;
                let mut immediate_ghostty_wakeup_burst_active = false;
                loop {
                    // Phase 1: Block until first event arrives (zero CPU when idle)
                    let Some(first_event) = events_rx.next().await else {
                        break; // Channel closed
                    };

                    // Phase 2: Keep the existing low-latency path for Linux PTYs. Windows
                    // Ghostty holds its first wakeup until the batch closes because ConPTY can
                    // split or normalize the synchronized-output sequence around TUI redraws.
                    let mut batch = Vec::with_capacity(32);
                    let mut dequeued = 1usize;
                    let render_wakeup_immediately =
                        should_render_ghostty_wakeup_immediately(&first_event);
                    let mut had_wakeup = first_event.is_wakeup();
                    let leading_immediate_wakeup = render_wakeup_immediately
                        && had_wakeup
                        && !immediate_ghostty_wakeup_burst_active;
                    if leading_immediate_wakeup {
                        immediate_ghostty_wakeup_burst_active = true;
                        let result = cx.update(|cx| {
                            this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                                view.terminal.process_backend_wakeup();
                                view.process_dirty_terminal(cx);
                            })
                        });
                        if result.is_err() {
                            break;
                        }
                        had_wakeup = false;
                    }
                    if !had_wakeup && !leading_immediate_wakeup {
                        batch.push(first_event);
                    }

                    let mut batch_window_elapsed = false;
                    {
                        let timer = futures::FutureExt::fuse(smol::Timer::after(
                            std::time::Duration::from_millis(4),
                        ));
                        futures::pin_mut!(timer);
                        loop {
                            futures::select_biased! {
                                event = events_rx.next() => {
                                    match event {
                                        Some(event) if event.is_wakeup() => {
                                            had_wakeup = true;
                                            dequeued += 1;
                                        }
                                        Some(event) => {
                                            batch.push(event);
                                            dequeued += 1;
                                        }
                                        None => break,
                                    }
                                    if dequeued >= 100 { break; }
                                }
                                _ = timer => {
                                    batch_window_elapsed = true;
                                    break;
                                },
                            }
                        }
                    }
                    if batch_window_elapsed {
                        immediate_ghostty_wakeup_burst_active = false;
                    }

                    // Phase 3: Process the batch in a single entity update
                    let result = cx.update(|cx| {
                        this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            let old_title = view.terminal.title.clone();
                            let old_cwd = view.terminal.current_cwd.clone();
                            view.terminal.sync_channels();
                            if had_wakeup {
                                view.terminal.process_backend_wakeup();
                            }
                            for event in batch {
                                view.terminal.process_backend_event(event);
                            }

                            // Execute deferred clipboard operations (OSC 52)
                            let clipboard_ops =
                                std::mem::take(&mut view.terminal.pending_clipboard_ops);
                            for op in clipboard_ops {
                                match op {
                                    ClipboardOp::Store(text) => {
                                        // U-023: sanitize untrusted PTY output before it
                                        // reaches the system clipboard - symmetric with the
                                        // Load path below, so an embedded CR/ESC can't commit
                                        // a hidden command when the user later pastes it.
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            sanitize_osc52(&text),
                                        ));
                                    }
                                    ClipboardOp::Load(format_fn) => {
                                        // Match the OSC 52 Store cap (100 KiB) on
                                        // Load responses too - without this, a
                                        // very large clipboard (multi-MB) would
                                        // be base64-encoded and streamed into
                                        // the PTY in one shot. Cap centralized
                                        // in `crate::limits` (US-013).
                                        let mut text = cx
                                            .read_from_clipboard()
                                            .and_then(|c| c.text())
                                            .unwrap_or_default();
                                        if text.len() > MAX_OSC52_BYTES {
                                            // Truncate on a UTF-8 boundary so
                                            // the response remains valid text.
                                            let mut cut = MAX_OSC52_BYTES;
                                            while cut > 0 && !text.is_char_boundary(cut) {
                                                cut -= 1;
                                            }
                                            text.truncate(cut);
                                        }
                                        // U-023: same shared control-char filter as the Store
                                        // path (now also strips CR / other C0 / DEL, not just
                                        // ESC + C1).
                                        let response = format_fn(&sanitize_osc52(&text));
                                        view.terminal.write_to_pty_silent(response.into_bytes());
                                    }
                                }
                            }

                            // OSC 10/11/12 color queries are now handled
                            // synchronously inside `process_event` (matches
                            // Zed's pattern at crates/terminal/src/terminal.rs:997).
                            // Deferring them here used to lose the response
                            // window for crossterm-based clients like the
                            // OpenAI Codex CLI, which then dropped its
                            // input-bar background tint silently.

                            // Cap pending TextAreaSize replies to keep a runaway TUI
                            // from accumulating thousands of pending responders.
                            // Keep the most recent entries - older replies would
                            // race the writer that requested them and are
                            // effectively stale by the time we'd answer.
                            let size = view.current_window_size();
                            for response in view.terminal.drain_size_responses(size) {
                                view.terminal.write_to_pty_silent(response.into_bytes());
                            }

                            // US-002: close only on a user-initiated or clean
                            // exit. A non-zero exit with no prior user input is
                            // a spawn/launch failure (bad shell, missing agent
                            // binary) - keep the pane open so the exit overlay
                            // renders the code instead of vanishing silently.
                            if view.terminal.exited.is_some()
                                && view.terminal.should_close_on_exit()
                            {
                                cx.emit(TerminalEvent::ChildExited);
                            }
                            if view.terminal.title != old_title {
                                cx.emit(TerminalEvent::TitleChanged);
                            }
                            if view.terminal.current_cwd != old_cwd
                                && let Some(ref cwd) = view.terminal.current_cwd
                            {
                                cx.emit(TerminalEvent::CwdChanged(cwd.clone()));
                            }

                            view.process_dirty_terminal(cx);
                        })
                    });
                    if result.is_err() {
                        break;
                    }

                    // Yield to other GPUI tasks between batches
                    smol::future::yield_now().await;
                }
            },
        )
        .detach();

        // US-006: subscribe to the app-scoped `BlinkPhase` so this terminal's
        // cursor visibility tracks the shared toggle. Replaces the
        // per-terminal `smol::Timer` loop that previously lived here.
        // Short-circuit preserved: skip when the PTY has exited; force visible
        // when alacritty disabled blinking (DECSCUSR / VT100 cursor style).
        //
        // `try_global` rather than `global` so a future code path that
        // constructs a TerminalView outside `PaneFlowApp::new` (test
        // harness, headless tooling) degrades to "always-visible cursor"
        // instead of panicking on the missing global. The current invariant
        // is that bootstrap installs the global before any TerminalView is
        // built; this is a defensive fallback only.
        if let Some(global) = cx.try_global::<crate::terminal::blink::BlinkPhaseGlobal>() {
            let blink_phase = global.0.clone();
            cx.observe(
                &blink_phase,
                |view: &mut Self, phase, cx: &mut Context<Self>| {
                    if view.terminal.exited.is_some() {
                        return;
                    }
                    let new_visible = resolve_cursor_visible(
                        view.cursor_blink_mode,
                        view.terminal.cursor_blinking,
                        phase.read(cx).visible,
                    );
                    if new_visible != view.cursor_visible {
                        view.cursor_visible = new_visible;
                        cx.notify();
                    }
                },
            )
            .detach();
        } else {
            log::warn!(
                "BlinkPhaseGlobal not installed - cursor will not blink for this TerminalView"
            );
        }

        let config = paneflow_config::loader::load_config();
        let terminal_config = config.terminal.clone().unwrap_or_default();
        let scroll_multiplier = terminal_config.resolved_scroll_multiplier();
        let cursor_blink_mode = terminal_config.cursor_blink.unwrap_or_default();
        let default_cursor_shape =
            renderer_cursor_shape_from_config(terminal_config.cursor_shape.unwrap_or_default());
        let cursor_color_override = cursor_color_override_from_config(&terminal_config);
        let integrated_glyphs_enabled = terminal_config.resolved_integrated_glyphs();
        let color_emoji_enabled = terminal_config.resolved_color_emoji();

        Self {
            terminal,
            focus_handle,
            cursor_visible: true,
            selecting: false,
            cell_width: gpui::px(8.0),
            line_height: gpui::px(16.0),
            element_origin: Arc::new(Mutex::new(gpui::Point::default())),
            scrollbar_metrics: Arc::new(Mutex::new(None)),
            scrollbar_drag: None,
            scroll_remainder: 0.0,
            search_active: false,
            search_input,
            search_query: String::new(),
            search_generation: 0,
            search_matches: Vec::new(),
            search_current: 0,
            search_regex_mode: false,
            search_regex_error: None,
            option_as_meta: config
                .option_as_meta
                .unwrap_or_else(crate::keys::default_option_as_meta),
            cursor_blink_mode,
            default_cursor_shape,
            cursor_color_override,
            scroll_multiplier,
            terminal_material_active: false,
            integrated_glyphs_enabled,
            color_emoji_enabled,
            copy_mode_active: false,
            copy_cursor: Point::new(0, 0),
            copy_mode_frozen_offset: 0,
            was_focused: false,
            focus_subscriptions: None,
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty_pressed_keys: std::collections::HashMap::new(),
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            ghostty_pending_text_key: None,
            hovered_cell: None,
            ctrl_hovered_link: None,
            hover_link_cache: None,
            mouse_down_link: None,
            ime_marked_text: String::new(),
            needs_initial_clear: Arc::new(std::sync::atomic::AtomicBool::new(true)),
            terminal_window_size: Arc::new(Mutex::new(None)),
        }
    }

    #[cfg(test)]
    pub(crate) fn display_only_for_test(workspace_id: u64, cx: &mut Context<Self>) -> Self {
        let terminal = TerminalState::new_display_only(24, 80);
        Self::from_terminal_state(workspace_id, terminal, cx)
    }
}

// ---------------------------------------------------------------------------
// IME composition methods (US-017)
// ---------------------------------------------------------------------------

impl TerminalView {
    /// Set preedit text during IME composition.
    pub fn set_marked_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.ime_marked_text = text;
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
            self.ghostty_pending_text_key = None;
        }
        cx.notify();
    }

    /// Clear preedit text (cancel composition).
    pub fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.ime_marked_text.clear();
        cx.notify();
    }

    /// Commit composed text to the PTY.
    pub fn commit_text(&mut self, text: &str, _cx: &mut Context<Self>) {
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        let was_composing = !self.ime_marked_text.is_empty();
        self.ime_marked_text.clear();
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
            let pending = if was_composing {
                self.ghostty_pending_text_key.take();
                None
            } else {
                self.ghostty_pending_text_key.take()
            };
            let release_id = pending
                .as_ref()
                .map(|(keystroke, _, _)| keystroke.key.clone());
            let input = pending
                .as_ref()
                .map(|(keystroke, action, prefer_character_input)| {
                    super::input::ghostty_text_key_input(
                        keystroke,
                        *action,
                        *prefer_character_input,
                        text,
                    )
                })
                .unwrap_or_else(|| paneflow_terminal_ghostty::KeyInput {
                    key: paneflow_terminal_ghostty::Key::Unidentified,
                    action: paneflow_terminal_ghostty::KeyAction::Press,
                    modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
                    consumed_modifiers: paneflow_terminal_ghostty::Modifiers::empty(),
                    text: text.to_string(),
                    unshifted_codepoint: None,
                    composing: false,
                });
            let mut release = input.clone();
            release.action = paneflow_terminal_ghostty::KeyAction::Release;
            release.text.clear();
            let result = self.terminal.write_ghostty_key(input, None);
            if result == super::pty_session::BackendInputResult::Accepted
                && let Some(release_id) = release_id
            {
                self.ghostty_pressed_keys.insert(release_id, release);
            }
            if result.is_handled() {
                return;
            }
        }
        self.terminal.write_to_pty(text.as_bytes().to_vec());
    }

    /// Send arbitrary text to the PTY (no bracketed paste wrapping).
    /// Used by AI agents and automation tools via IPC.
    pub fn send_text(&self, text: &str) {
        self.terminal.write_to_pty(text.as_bytes().to_vec());
    }

    /// True once the foreground terminal application has enabled DEC
    /// bracketed-paste mode (`ESC[?2004h`).
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal
            .session_backend()
            .modes()
            .contains(Modes::BRACKETED_PASTE)
    }

    /// Send a shell command to the PTY and execute it (appends `\r`).
    /// Used by tab-bar command buttons.
    pub fn send_command(&self, command: &str) {
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        self.terminal.write_to_pty(bytes);
    }

    /// Send a keystroke to the PTY by converting it to an escape sequence.
    /// `keystroke_str` is a dash-separated description like "ctrl-c", "enter", "alt-f".
    /// Returns Ok(()) on success, Err(message) if the keystroke string is invalid.
    ///
    /// US-005 (orchestration-v2): refuses any keystroke whose RESOLVED escape
    /// sequence carries CR/LF (`enter`, `ctrl-m`, `ctrl-j`, …). The IPC-level
    /// CR/LF check only sees the keystroke *name*, so without this guard
    /// `paneflow key <t> enter` would submit a pre-filled prompt and bypass the
    /// human-in-loop invariant - submission must stay exclusive to
    /// `surface.send_text` with `submit=true`.
    pub fn send_keystroke(&self, keystroke_str: &str) -> Result<(), String> {
        let keystroke = gpui::Keystroke::parse(keystroke_str).map_err(|e| format!("{e}"))?;
        let mode = self.terminal.session_backend().modes();
        if let Some(seq) = crate::keys::to_esc_str(&keystroke, &mode, self.option_as_meta) {
            if sequence_would_submit(&seq) {
                return Err(format!(
                    "keystroke '{keystroke_str}' would submit (CR/LF); use \
                     surface.send_text with submit=true (`paneflow send --submit`) instead"
                ));
            }
            self.terminal.write_to_pty(seq.as_bytes().to_vec());
        } else if let Some(ref key_char) = keystroke.key_char {
            if sequence_would_submit(key_char) {
                return Err(format!(
                    "keystroke '{keystroke_str}' would submit (CR/LF); use \
                     surface.send_text with submit=true (`paneflow send --submit`) instead"
                ));
            }
            self.terminal.write_to_pty(key_char.as_bytes().to_vec());
        }
        Ok(())
    }

    /// Return the UTF-16 range of the current preedit text, if any.
    pub fn marked_text_range(&self) -> Option<std::ops::Range<usize>> {
        if self.ime_marked_text.is_empty() {
            None
        } else {
            let utf16_len: usize = self.ime_marked_text.encode_utf16().count();
            Some(0..utf16_len)
        }
    }
}

/// True when an escape sequence (or raw key char) would submit a line at the
/// PTY. Single choke point for the `send_keystroke` refusal above (US-005,
/// orchestration-v2) - pure so the rule is unit-testable.
fn sequence_would_submit(seq: &str) -> bool {
    seq.contains('\r') || seq.contains('\n')
}

// ---------------------------------------------------------------------------
// URL detection on hover (US-015)
// ---------------------------------------------------------------------------

impl TerminalView {
    fn hovered_line_text(&self) -> Option<(Line, String, Vec<usize>)> {
        let point = self.hovered_cell?;
        let line = self.terminal.session_backend().line_text_at(point)?;
        Some((line.line, line.text, line.char_to_column))
    }

    pub(super) fn detect_links_at_hover(&mut self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            self.hover_link_cache = None;
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd_key = self.terminal.current_cwd.clone();
        if let Some(cache) = &self.hover_link_cache
            && cache.line == line
            && cache.cwd == cwd_key
            && cache.line_text == trimmed
        {
            return cache.zones.clone();
        }
        let cwd = cwd_key.as_deref().map(std::path::Path::new);

        let mut zones = crate::terminal::element::detect_urls_on_line_mapped(trimmed, line, map);
        zones.extend(crate::terminal::element::detect_file_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
        zones.extend(crate::terminal::element::detect_code_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
        self.hover_link_cache = Some(HoverLinkCache {
            line,
            cwd: cwd_key,
            line_text: trimmed.to_string(),
            zones: zones.clone(),
        });
        zones
    }

    /// Detect regex URLs on the line at the given grid point.
    /// Extracts line text from the locked term grid, runs the URL regex,
    /// and returns zones that cover the given column (for hover hit-testing).
    #[allow(dead_code)]
    pub fn detect_url_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        crate::terminal::element::detect_urls_on_line_mapped(
            trimmed,
            line,
            &char_to_col[..trimmed_chars],
        )
    }

    /// Detect `.md` / `.markdown` file paths on the line at the hovered grid
    /// point (US-019). Mirrors `detect_url_at_hover`: extracts line text with
    /// wide-char-aware char→column mapping, then runs the file-path scanner
    /// against the pane's tracked CWD.
    #[allow(dead_code)]
    pub(super) fn detect_file_path_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd = self
            .terminal
            .current_cwd
            .as_deref()
            .map(std::path::Path::new);
        crate::terminal::element::detect_file_paths_on_line_mapped(trimmed, line, map, cwd)
    }

    /// Detect source-code file paths with optional `:line[:col]` on the
    /// hovered line. Mirrors `detect_file_path_at_hover`'s extraction; the
    /// returned zones carry `line`/`col` populated from `path:42` or
    /// `path:42:7` style references so the click handler can pass the
    /// location through to the editor.
    #[allow(dead_code)]
    pub(super) fn detect_code_path_at_hover(&self) -> Vec<HyperlinkZone> {
        let Some((line, line_text, char_to_col)) = self.hovered_line_text() else {
            return Vec::new();
        };
        let trimmed = line_text.trim_end();
        let trimmed_chars = trimmed.chars().count();
        let map = &char_to_col[..trimmed_chars];
        let cwd = self
            .terminal
            .current_cwd
            .as_deref()
            .map(std::path::Path::new);
        crate::terminal::element::detect_code_paths_on_line_mapped(trimmed, line, map, cwd)
    }
}

// ---------------------------------------------------------------------------
// Terminal events
// ---------------------------------------------------------------------------

/// Events emitted by TerminalView via GPUI's EventEmitter.
/// Pane subscribes for ChildExited/TitleChanged; PaneFlowApp subscribes
/// for CwdChanged/ActivityBurst/ServiceDetected to drive sidebar updates.
pub enum TerminalEvent {
    /// The shell process exited (e.g. user typed `exit`).
    ChildExited,
    /// The terminal title changed (via OSC 0/2 escape sequence).
    TitleChanged,
    /// The shell's working directory changed (detected via OSC 7 escape sequence).
    CwdChanged(String),
    /// Terminal output activity detected - triggers an OS port scan
    /// (`workspace::ports`; Linux `/proc/net/tcp`, macOS libproc, Windows IP Helper).
    /// Emitted alongside `ServiceDetected` during output scan ticks.
    ActivityBurst,
    /// A server/service was detected in PTY output (e.g. "Listening on :3000").
    /// Enriches the bare port from the OS port scan with label and URL.
    ServiceDetected(ServiceInfo),
    /// Escape pressed while swap mode is active - requests cancellation.
    CancelSwapMode,
    /// A mouse selection was auto-copied to the clipboard on mouse release.
    /// Consumed by `PaneFlowApp` to surface a "Copied" toast.
    SelectionCopied,
    /// US-020 - Cmd/Ctrl-click on a `.md`/`.markdown` path detected by the
    /// US-019 file-path scanner. The receiver (PaneFlowApp) splits the
    /// containing pane vertically and inserts a markdown viewer in the
    /// new half. The path is the canonical absolute path produced by
    /// `terminal::element::detect_file_paths_on_line_mapped`.
    OpenMarkdownPath(std::path::PathBuf),
    /// Cmd/Ctrl-click on a source-code path with optional `:line[:col]`
    /// suffix (`error[E0382]: ... at src/lib.rs:42:7`). The receiver
    /// (PaneFlowApp) resolves the user's preferred editor via the
    /// `$VISUAL`/`$EDITOR` env chain plus a probed fallback list and
    /// invokes it with the right argv for the detected editor family
    /// (`code -g path:L:C`, `nvim +L path`, `emacs +L:C path`, etc.).
    OpenCodePath {
        path: std::path::PathBuf,
        line: Option<u32>,
        col: Option<u32>,
    },
    /// EP-006 US-019 - the per-pane font override changed. The receiver
    /// (PaneFlowApp) persists the session so the zoom survives a crash,
    /// not just a clean quit (same rationale as `SurfaceRenamed`).
    FontZoomChanged,
    /// EP-006 US-018 - the user toggled the fleet scope from this view's
    /// find bar. The receiver (PaneFlowApp) fans the query out to every
    /// pane of every workspace off the render thread and opens the fleet
    /// results overlay.
    FleetSearchRequested { query: String, regex: bool },
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl gpui::Focusable for TerminalView {
    fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl TerminalView {
    /// Build a rich key context from the terminal's current mode flags.
    /// Enables keybindings scoped to terminal state (e.g. `"Terminal && screen == alt"`).
    fn dispatch_context(&self) -> KeyContext {
        let mode = self.terminal.session_backend().modes();
        let mut ctx = KeyContext::default();
        ctx.add("Terminal");

        // Screen mode
        if mode.contains(Modes::ALT_SCREEN) {
            ctx.set("screen", "alt");
        } else {
            ctx.set("screen", "normal");
        }

        // DEC private modes
        if mode.contains(Modes::APP_CURSOR) {
            ctx.add("DECCKM");
        }
        if mode.contains(Modes::APP_KEYPAD) {
            ctx.add("DECPAM");
        }
        if mode.contains(Modes::BRACKETED_PASTE) {
            ctx.add("bracketed_paste");
        }
        if mode.contains(Modes::FOCUS_IN_OUT) {
            ctx.add("report_focus");
        }
        if mode.contains(Modes::ALTERNATE_SCROLL) {
            ctx.add("alternate_scroll");
        }

        // Mouse reporting mode
        if mode.intersects(Modes::MOUSE_MODE) {
            ctx.add("any_mouse_reporting");
            if mode.contains(Modes::MOUSE_MOTION) {
                ctx.set("mouse_reporting", "motion");
            } else if mode.contains(Modes::MOUSE_DRAG) {
                ctx.set("mouse_reporting", "drag");
            } else {
                ctx.set("mouse_reporting", "click");
            }
        } else {
            ctx.set("mouse_reporting", "off");
        }

        // Mouse encoding format
        if mode.contains(Modes::SGR_MOUSE) {
            ctx.set("mouse_format", "sgr");
        } else if mode.contains(Modes::UTF8_MOUSE) {
            ctx.set("mouse_format", "utf8");
        } else {
            ctx.set("mouse_format", "normal");
        }

        ctx
    }

    /// Build the top-right search overlay bar. Caller is responsible for
    /// adding it to the main element tree (and for gating on `search_active`).
    fn render_search_overlay(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        use gpui::{FontWeight, Hsla, MouseButton, hsla, px, svg};

        // Themed chrome (One Dark / PaneFlow Light), not hardcoded Catppuccin -
        // keeps the find bar consistent with the fleet-search card and sidebar.
        let ui = crate::theme::ui_colors();

        let regex_active = self.search_regex_mode;
        let has_regex_error = self.search_regex_error.is_some();
        let match_count = self.search_matches.len();
        let has_matches = match_count > 0;
        let current_match = if has_matches {
            self.search_current + 1
        } else {
            0
        };

        let (status_text, status_color) = if has_regex_error {
            ("Invalid regex".to_string(), ui.agent_error)
        } else if self.search_query.is_empty() {
            (String::new(), ui.muted)
        } else if !has_matches {
            ("No results".to_string(), ui.muted)
        } else {
            (format!("{current_match}/{match_count}"), ui.muted)
        };

        // Real input entity (cursor, selection, IME, clipboard) - the same
        // widget the Agents sidebar uses, focused on open. The caret and
        // "Search" placeholder are painted by the widget itself; we only own
        // the wrapper box (width + inherited text size/colour).
        let field = div()
            .id("search-field")
            .flex()
            .items_center()
            .min_w(px(160.))
            .max_w(px(320.))
            .text_size(px(13.))
            .text_color(ui.text)
            .child(self.search_input.clone());

        // Regex toggle (.*): active state reads as a pressed pill with an accent
        // hairline - a full accent fill would drop below 4.5:1 on the light theme.
        let regex_background = if regex_active {
            ui.subtle
        } else {
            ui.subtle.opacity(0.0)
        };
        let regex_toggle = div()
            .id("search-regex-toggle")
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(5.))
            .border_1()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .bg(regex_background)
            .border_color(if regex_active {
                ui.accent
            } else {
                hsla(0., 0., 0., 0.)
            })
            .text_color(if regex_active { ui.text } else { ui.muted })
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(regex_background, ui.subtle, delta));
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.toggle_search_regex(cx)),
            )
            .child(".*");

        // EP-006 US-018: fan the query out to every pane of every workspace. The
        // clickable counterpart of the remappable `toggle_fleet_search` action.
        let fleet_toggle = div()
            .id("search-fleet-toggle")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .h(px(22.))
            .px(px(7.))
            .rounded(px(5.))
            .text_size(px(12.))
            .text_color(ui.muted)
            .animated_hover(move |style, delta| {
                style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
            })
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/world.svg")
                    .text_color(ui.muted),
            )
            .child("Fleet")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _window, cx| this.request_fleet_search(cx)),
            );

        // Icon-only square button (chevrons + close): hover surface, dimmable.
        let icon_btn = |id: &'static str, icon: &'static str, color: Hsla| {
            div()
                .id(id)
                .flex()
                .items_center()
                .justify_center()
                .size(px(22.))
                .rounded(px(5.))
                .animated_hover(move |style, delta| {
                    style.bg(lerp_color(ui.subtle.opacity(0.0), ui.subtle, delta));
                })
                .child(svg().size(px(14.)).flex_none().path(icon).text_color(color))
        };
        let nav_color = if has_matches {
            ui.muted
        } else {
            ui.muted.opacity(0.35)
        };

        let prev_btn = icon_btn("search-prev", "icons/chevron_up.svg", nav_color).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window, cx| this.search_prev(cx)),
        );
        let next_btn = icon_btn("search-next", "icons/chevron_down.svg", nav_color).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, _window, cx| this.search_next(cx)),
        );
        let close_btn = icon_btn("search-close", "icons/close.svg", ui.muted).on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| {
                this.dismiss_search(cx);
                this.focus_handle.clone().focus(window, cx);
            }),
        );

        div()
            .id("search-overlay")
            .occlude()
            .absolute()
            .top_2()
            .right_2()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(ui.overlay)
            .border_1()
            .border_color(ui.border)
            .shadow_lg()
            .child(
                svg()
                    .size(px(15.))
                    .flex_none()
                    .path("icons/tool_search.svg")
                    .text_color(ui.muted),
            )
            .child(field)
            .child(regex_toggle)
            .child(fleet_toggle)
            .when(!status_text.is_empty(), |el| {
                el.child(
                    div()
                        .id("search-status")
                        .flex_none()
                        .text_size(px(12.))
                        .text_color(status_color)
                        .child(status_text.clone()),
                )
            })
            .child(div().flex_none().w(px(1.)).h(px(16.)).bg(ui.border))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(2.))
                    .child(prev_btn)
                    .child(next_btn)
                    .child(close_btn),
            )
            .into_any_element()
    }
}

impl TerminalView {
    fn apply_terminal_focus(&mut self, focused: bool) {
        if focused == self.was_focused {
            return;
        }

        self.terminal.set_terminal_focused(focused);
        #[cfg(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ))]
        if !focused {
            self.release_ghostty_pressed_keys();
        }
        let reports_focus = self
            .terminal
            .session_backend()
            .modes()
            .contains(Modes::FOCUS_IN_OUT);
        if reports_focus {
            // This protocol write is not user input. It must not mark a failed
            // spawn as interactive merely because its pane received focus.
            #[cfg(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            ))]
            let ghostty_encoded = self
                .terminal
                .write_ghostty_focus(if focused {
                    paneflow_terminal_ghostty::FocusEvent::Gained
                } else {
                    paneflow_terminal_ghostty::FocusEvent::Lost
                })
                .is_handled();
            #[cfg(not(any(
                all(target_os = "linux", feature = "libghostty-linux"),
                all(
                    target_os = "windows",
                    target_arch = "x86_64",
                    target_env = "msvc",
                    feature = "libghostty-windows"
                )
            )))]
            let ghostty_encoded = false;
            if !ghostty_encoded {
                let report = if focused { b"\x1b[I" } else { b"\x1b[O" };
                self.terminal.write_to_pty_silent(report.to_vec());
            }
        }
        self.was_focused = focused;
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.focus_subscriptions.is_none() {
            let focus_handle = self.focus_handle.clone();
            let focus_in = cx.on_focus_in(&focus_handle, window, |view, _window, cx| {
                view.apply_terminal_focus(true);
                cx.notify();
            });
            let focus_out = cx.on_focus_out(&focus_handle, window, |view, _event, _window, cx| {
                view.apply_terminal_focus(false);
                cx.notify();
            });
            self.focus_subscriptions = Some((focus_in, focus_out));
        }

        let focused = self.focus_handle.is_focused(window);
        self.apply_terminal_focus(focused);
        let backend = self.terminal.session_backend();
        let terminal_mode = backend.modes();

        // Update cell dimensions for mouse → grid mapping
        let frame_metrics = crate::terminal::element::resolve_frame_metrics(
            window,
            cx,
            self.terminal.font_size_override,
        );
        self.cell_width = frame_metrics.dimensions.cell_width;
        self.line_height = frame_metrics.dimensions.line_height;

        #[cfg(debug_assertions)]
        let keystroke_at = self.terminal.last_keystroke_at.take();

        // Collect search match rects for the element to paint
        let search_match_rects = if self.search_active && !self.search_matches.is_empty() {
            self.search_matches
                .iter()
                .enumerate()
                .map(|(i, m)| SearchHighlight {
                    start: m.start,
                    end: m.end,
                    is_active: i == self.search_current,
                })
                .collect()
        } else {
            Vec::new()
        };

        // Build copy mode cursor state for the element. When a selection is active,
        // also expose the anchor (selection start) so the element can render it as
        // a distinct tmux-style marker.
        let copy_cursor_state = if self.copy_mode_active {
            let (anchor_grid_line, anchor_col) = backend
                .selection_range()
                .map(|range| (Some(range.start.line.0), range.start.column.0))
                .unwrap_or((None, 0));
            Some(CopyModeCursorState {
                grid_line: self.copy_cursor.line.0,
                col: self.copy_cursor.column.0,
                anchor_grid_line,
                anchor_col,
            })
        } else {
            None
        };

        // ALT_SCREEN: cursor always visible (no blink-off) for TUI apps
        let alt_screen = terminal_mode.contains(Modes::ALT_SCREEN);
        let cursor_visible = self.cursor_visible || alt_screen;

        // EP-006 US-017: match positions for the scrollbar rail, converted
        // from grid-absolute lines to lines-from-bottom under a short lock
        // (the `scroll_to_match` reference conversion). Empty when no
        // search → the rail disappears at the same repaint (US-017 AC).
        let search_rail_lines: Vec<usize> = if self.search_active && !self.search_matches.is_empty()
        {
            let bottom = backend.bottommost_line();
            self.search_matches
                .iter()
                .map(|m| bottom.0.saturating_sub(m.start.line.0).max(0) as usize)
                .collect()
        } else {
            Vec::new()
        };

        let terminal_element = TerminalElement::new(
            self.terminal.session_backend(),
            cursor_visible,
            focused,
            self.terminal.exited,
            self.terminal.exit_signal.clone(),
            self.element_origin.clone(),
            search_match_rects,
            copy_cursor_state,
            self.ctrl_hovered_link
                .as_ref()
                .map(|link| (link.start.line.0, link.start.column.0, link.end.column.0)),
            self.ime_marked_text.clone(),
            self.focus_handle.clone(),
            cx.entity().clone(),
            self.needs_initial_clear.clone(),
            self.terminal_window_size.clone(),
            self.scrollbar_metrics.clone(),
            search_rail_lines,
            self.default_cursor_shape,
            self.cursor_color_override,
            self.terminal_material_active,
            self.integrated_glyphs_enabled,
            self.color_emoji_enabled,
            frame_metrics,
            alt_screen,
            #[cfg(debug_assertions)]
            keystroke_at,
        );

        let terminal_body = terminal_element;

        // Search overlay bar
        let search_active = self.search_active;

        let mut el = div()
            .id("terminal-view")
            .key_context(self.dispatch_context())
            .track_focus(&self.focus_handle)
            // US-010: hand cursor over a hovered link, text IBeam otherwise -
            // the universal "this is clickable" affordance (mirrors Zed
            // terminal_element.rs:1364-1371).
            .cursor(if self.ctrl_hovered_link.is_some() {
                gpui::CursorStyle::PointingHand
            } else {
                gpui::CursorStyle::IBeam
            })
            // US-011: reveal/clear a link the instant Ctrl/Cmd is pressed or
            // released over a stationary cursor (no mouse move required).
            .on_modifiers_changed(cx.listener(Self::handle_modifiers_changed))
            .on_key_down(cx.listener(Self::handle_key_down))
            .on_key_up(cx.listener(Self::handle_key_up))
            .on_any_mouse_down(cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
            .on_action(cx.listener(|this, _: &crate::TerminalCopy, window, cx| {
                this.handle_copy(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::TerminalPaste, window, cx| {
                this.handle_paste(window, cx);
            }))
            .on_scroll_wheel(cx.listener(Self::handle_scroll_wheel))
            .on_action(cx.listener(|this, _: &crate::ScrollPageUp, window, cx| {
                this.handle_scroll_page_up(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ScrollPageDown, window, cx| {
                this.handle_scroll_page_down(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::JumpPrevPrompt, _window, cx| {
                this.jump_to_prompt(true, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::JumpNextPrompt, _window, cx| {
                this.jump_to_prompt(false, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleSearch, window, cx| {
                this.toggle_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::DismissSearch, window, cx| {
                this.dismiss_search(cx);
                this.focus_handle.clone().focus(window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::ToggleSearchRegex, _window, cx| {
                    this.toggle_search_regex(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::SearchNext, _window, cx| {
                this.search_next(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::SearchPrev, _window, cx| {
                this.search_prev(cx);
            }))
            .on_action(cx.listener(|this, _: &crate::ToggleCopyMode, _window, cx| {
                this.toggle_copy_mode(cx);
            }))
            // EP-006 US-019: per-pane font zoom (±1 pt, clamp [8, 32]).
            .on_action(
                cx.listener(|this, _: &crate::FontSizeIncrease, _window, cx| {
                    this.font_zoom_step(1.0, cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::FontSizeDecrease, _window, cx| {
                    this.font_zoom_step(-1.0, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::FontSizeReset, _window, cx| {
                this.font_zoom_reset(cx);
            }))
            // EP-006 US-018: fan the current query out to the whole fleet.
            .on_action(
                cx.listener(|this, _: &crate::ToggleFleetSearch, _window, cx| {
                    this.request_fleet_search(cx);
                }),
            )
            .on_drop(cx.listener(Self::handle_file_drop))
            .on_action(
                cx.listener(|this, _: &crate::ClearScrollHistory, _window, cx| {
                    this.clear_scroll_history(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::ResetTerminal, _window, cx| {
                this.reset_terminal(cx);
            }))
            .size_full()
            .child(terminal_body);

        if search_active {
            // Add "Search" key context for search-scoped bindings
            el = el.key_context("Search");
            el = el.child(self.render_search_overlay(cx));
        }

        if self.copy_mode_active {
            let copy_badge = div()
                .id("copy-mode-badge")
                .absolute()
                .top_1()
                .right_1()
                .px_2()
                .py(gpui::px(2.0))
                .rounded_md()
                .bg(gpui::rgba(0x89b4facc))
                .text_color(gpui::rgb(0x1e1e2e))
                .text_size(gpui::px(11.0))
                .font_weight(gpui::FontWeight::BOLD)
                .child("COPY");
            el = el.child(copy_badge);
        }

        el
    }
}

/// US-008: decide cursor visibility for one blink tick. `On` always blinks
/// (follows the shared phase), `Off` is always solid, `TerminalControlled`
/// defers to the program's DECSCUSR-driven blink flag. Pure so it is
/// unit-testable without the GPUI observer.
fn resolve_cursor_visible(
    mode: paneflow_config::schema::CursorBlinkConfig,
    decscusr_blinking: bool,
    phase_visible: bool,
) -> bool {
    use paneflow_config::schema::CursorBlinkConfig as M;
    match mode {
        M::On => phase_visible,
        M::Off => true,
        M::TerminalControlled => {
            if decscusr_blinking {
                phase_visible
            } else {
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::pty_session::strip_partial_ansi_tail;

    #[test]
    fn terminal_backend_policy_keeps_alacritty_as_explicit_rollback() {
        let ghostty_available = cfg!(any(
            all(target_os = "linux", feature = "libghostty-linux"),
            all(
                target_os = "windows",
                target_arch = "x86_64",
                target_env = "msvc",
                feature = "libghostty-windows"
            )
        ));

        assert_eq!(
            should_start_ghostty(TerminalBackendConfig::Auto),
            ghostty_available && auto_selects_ghostty_for_target()
        );
        assert_eq!(
            should_start_ghostty(TerminalBackendConfig::Ghostty),
            ghostty_available
        );
        assert!(!should_start_ghostty(TerminalBackendConfig::Alacritty));
    }

    #[test]
    fn promoted_auto_policy_preserves_rollback_and_unavailable_fallback() {
        assert!(should_start_ghostty_for_policy(
            TerminalBackendConfig::Auto,
            true,
            true,
        ));
        assert!(!should_start_ghostty_for_policy(
            TerminalBackendConfig::Auto,
            false,
            true,
        ));
        assert!(!should_start_ghostty_for_policy(
            TerminalBackendConfig::Auto,
            true,
            false,
        ));
        assert!(should_start_ghostty_for_policy(
            TerminalBackendConfig::Ghostty,
            true,
            false,
        ));
        assert!(!should_start_ghostty_for_policy(
            TerminalBackendConfig::Alacritty,
            true,
            false,
        ));
        assert!(policy_requests_ghostty(TerminalBackendConfig::Auto, true));
        assert!(!policy_requests_ghostty(
            TerminalBackendConfig::Alacritty,
            true
        ));
    }

    // --- send_keystroke submission guard (US-005, orchestration-v2) ---

    #[test]
    fn sequence_would_submit_flags_cr_and_lf_only() {
        assert!(sequence_would_submit("\r"));
        assert!(sequence_would_submit("\n"));
        assert!(sequence_would_submit("text\rmore"));
        assert!(!sequence_would_submit("\x1b[A")); // arrow key
        assert!(!sequence_would_submit("\x03")); // ctrl-c
        assert!(!sequence_would_submit("a"));
    }

    #[test]
    fn enter_like_keystrokes_resolve_to_submitting_sequences() {
        // The IPC handler's CR/LF check sees only the keystroke NAME ("enter"
        // contains no CR byte), so the guard must catch the RESOLVED sequence.
        // This pins that `enter` / `ctrl-m` / `ctrl-j` all resolve to CR/LF and
        // would therefore be refused by `send_keystroke`.
        for name in ["enter", "ctrl-m", "ctrl-j"] {
            let ks = gpui::Keystroke::parse(name).expect("parse");
            let seq = crate::keys::to_esc_str(&ks, &Modes::empty(), false)
                .unwrap_or_else(|| panic!("{name} must resolve to a sequence"));
            assert!(
                sequence_would_submit(&seq),
                "{name} resolved to {seq:?}, expected a CR/LF sequence"
            );
        }
    }

    #[test]
    fn sanitize_osc52_strips_injection_controls_keeps_tab_and_newline() {
        // U-023: CR / ESC / other C0 / DEL / C1 are dropped; TAB and LF survive
        // (legitimate clipboard text), and printable multibyte is untouched.
        let dirty = "echo hi\r\x1b[31mX\x1b[0m\u{7f}\u{0085}\tcol\nnext - café 🦀";
        let clean = sanitize_osc52(dirty);
        assert_eq!(clean, "echo hi[31mX[0m\tcol\nnext - café 🦀");
        assert!(
            !clean.contains('\r'),
            "CR (commits a line on paste) removed"
        );
        assert!(!clean.contains('\u{1b}'), "ESC removed");
        assert!(!clean.contains('\u{7f}'), "DEL removed");
        assert!(!clean.contains('\u{85}'), "C1 (NEL) removed");
        assert!(clean.contains('\t') && clean.contains('\n'), "TAB/LF kept");
    }

    // --- strip_partial_ansi_tail tests (US-012 / US-015) ---

    #[test]
    fn strip_ansi_plain_text_unchanged() {
        let mut s = "hello world\nline two".to_string();
        strip_partial_ansi_tail(&mut s);
        assert_eq!(s, "hello world\nline two");
    }

    #[test]
    fn strip_ansi_lone_esc_removed() {
        let mut s = "hello\x1b".to_string();
        strip_partial_ansi_tail(&mut s);
        assert_eq!(s, "hello");
    }

    #[test]
    fn strip_ansi_incomplete_csi_removed() {
        // Incomplete CSI: \x1b[38;2; (no terminating byte in 0x40..0x7E)
        let mut s = "text\x1b[38;2;".to_string();
        strip_partial_ansi_tail(&mut s);
        assert_eq!(s, "text");
    }

    #[test]
    fn strip_ansi_complete_csi_kept() {
        // Complete CSI: \x1b[0m (terminated by 'm')
        let mut s = "text\x1b[0m".to_string();
        strip_partial_ansi_tail(&mut s);
        assert_eq!(s, "text\x1b[0m");
    }

    #[test]
    fn strip_ansi_incomplete_osc_removed() {
        // Incomplete OSC: \x1b]7;file:// (no BEL or ST)
        let mut s = "prompt\x1b]7;file://host/dir".to_string();
        strip_partial_ansi_tail(&mut s);
        assert_eq!(s, "prompt");
    }

    // --- extract_scrollback / restore_scrollback tests (US-011 / US-015) ---

    #[test]
    fn scrollback_round_trip() {
        // EP-002 US-004: the mockable PtyBackend is gone; a display-only
        // TerminalState has a real `Term` (no PTY) and is the right harness for
        // the grid-only history round-trip.
        let state = TerminalState::new_display_only(3, 80);

        state.restore_scrollback("history one\nhistory two\nvisible three\nvisible four");

        let scrollback = state.extract_scrollback();
        assert!(scrollback.is_some(), "Expected scrollback content");
        let text = scrollback.unwrap();
        assert!(
            text.contains("history one"),
            "Missing 'history one' in: {text}"
        );
        assert!(
            text.contains("history two"),
            "Missing 'history two' in: {text}"
        );
        assert!(!text.contains("visible three"), "Leaked viewport: {text}");
        assert!(!text.contains("visible four"), "Leaked viewport: {text}");
    }

    #[test]
    fn extract_scrollback_empty_terminal_returns_none() {
        let state = TerminalState::new_display_only(24, 80);
        // Fresh terminal with no content beyond the initial blank grid
        // May return None or Some with only whitespace - both are acceptable
        let scrollback = state.extract_scrollback();
        if let Some(ref text) = scrollback {
            assert!(
                text.trim().is_empty(),
                "Expected empty or whitespace-only scrollback, got: {text}"
            );
        }
    }

    #[test]
    fn cursor_blink_override_resolves_correctly() {
        use paneflow_config::schema::CursorBlinkConfig as M;
        // US-008: On always blinks (follows phase), ignoring DECSCUSR.
        assert!(resolve_cursor_visible(M::On, false, true));
        assert!(!resolve_cursor_visible(M::On, false, false));
        // Off is always solid (visible), ignoring phase and DECSCUSR.
        assert!(resolve_cursor_visible(M::Off, true, false));
        // TerminalControlled defers to DECSCUSR: blink → follow phase.
        assert!(!resolve_cursor_visible(M::TerminalControlled, true, false));
        assert!(resolve_cursor_visible(M::TerminalControlled, true, true));
        // TerminalControlled + DECSCUSR not blinking → always solid.
        assert!(resolve_cursor_visible(M::TerminalControlled, false, false));
    }
}
