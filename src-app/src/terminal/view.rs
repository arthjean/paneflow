//! GPUI view layer for a single terminal pane.
//!
//! Holds the `TerminalView` struct, its constructor + event batch loop,
//! IME wiring, URL hover detection, the `TerminalEvent` enum emitted to
//! consumers (pane / app), and the `Render` impl that composes
//! `TerminalElement` with the search overlay and copy-mode badge.
//!
//! Extracted from `terminal.rs` per US-016 of the src-app refactor PRD.

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event as AlacEvent, Notify};
use alacritty_terminal::grid::{Dimensions, Scroll as AlacScroll};
use alacritty_terminal::index::{Column as GridCol, Line as GridLine, Point as AlacPoint};
use alacritty_terminal::term::TermMode;
use futures::StreamExt;
use gpui::{
    App, ClipboardItem, Context, EventEmitter, FocusHandle, Hsla, InteractiveElement, IntoElement,
    KeyContext, MouseButton, Render, Styled, Window, div, prelude::*,
};
use paneflow_config::schema::{TerminalConfig, TerminalSurfaceProfile};

use super::element::TerminalElement;
use super::pty_session::ClipboardOp;
use super::service_detector::ServiceInfo;
use super::types::{CopyModeCursorState, CursorShape, HyperlinkZone, Modes, SearchHighlight};
use super::{PtyNotifier, TerminalState};
use crate::limits::MAX_OSC52_BYTES;

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

#[cfg(feature = "hera-dogfood")]
fn render_hera_side_by_side_surface(
    surface: crate::terminal::hera_dogfood::SideBySideDiagnosticSurface,
    copy_mode_active: bool,
    ime_active: bool,
) -> impl IntoElement {
    let ui = crate::theme::ui_colors();
    let counters = surface.counters();
    let mut diagnostics: Vec<String> = surface
        .diagnostics()
        .iter()
        .map(|diagnostic| format!("{}: {}", diagnostic.code(), diagnostic.message()))
        .collect();
    if let Some(reason) = surface.skipped_reason() {
        diagnostics.push(format!("render_skipped: {reason}"));
    }
    if copy_mode_active {
        diagnostics.push("copy_mode: primary_only".to_owned());
    }
    if ime_active {
        diagnostics.push("ime_preedit: primary_only".to_owned());
    }
    if diagnostics.is_empty() {
        diagnostics.push("ok".to_owned());
    }

    let rows = surface
        .rows()
        .iter()
        .take(32)
        .map(|row| {
            div()
                .font_family("monospace")
                .text_size(gpui::px(11.0))
                .text_color(ui.text)
                .child(row.trim_end().to_owned())
        })
        .collect::<Vec<_>>();

    let diagnostics = diagnostics
        .into_iter()
        .take(8)
        .map(|line| {
            div()
                .text_size(gpui::px(11.0))
                .text_color(ui.muted)
                .child(line)
        })
        .collect::<Vec<_>>();

    div()
        .id("hera-side-by-side")
        .w(gpui::px(360.0))
        .min_w(gpui::px(280.0))
        .h_full()
        .flex()
        .flex_col()
        .gap(gpui::px(8.0))
        .p(gpui::px(10.0))
        .border_l_1()
        .border_color(ui.border)
        .bg(ui.subtle)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_size(gpui::px(12.0))
                        .font_weight(gpui::FontWeight::BOLD)
                        .text_color(ui.text)
                        .child("Hera"),
                )
                .child(
                    div()
                        .text_size(gpui::px(11.0))
                        .text_color(ui.muted)
                        .child(format!(
                            "eq={} diff={} skip={}",
                            counters.equal(),
                            counters.mismatch(),
                            counters.side_by_side_skipped()
                        )),
                ),
        )
        .child(div().flex().flex_col().gap(gpui::px(1.0)).children(rows))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(gpui::px(2.0))
                .children(diagnostics),
        )
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

/// US-015: anchor for an in-progress scrollbar drag. The cursor Y and the
/// `display_offset` at grab time; each move maps the pixel delta since the grab
/// to a relative scrollback delta, so the thumb never jumps on grab regardless
/// of where on it the user clicked.
#[derive(Clone, Copy)]
pub(super) struct ScrollbarDrag {
    pub(super) anchor_y: gpui::Pixels,
    pub(super) anchor_offset: usize,
}

pub struct TerminalView {
    pub terminal: TerminalState,
    focus_handle: FocusHandle,
    pub(super) cursor_visible: bool,
    /// Track mouse button state for drag selection
    pub(super) selecting: bool,
    /// Last known cell dimensions (from TerminalElement::measure_cell)
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
    /// Windows-only appearance switch: default terminal backgrounds become
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
    pub(super) copy_cursor: AlacPoint,
    /// Display offset frozen at copy mode entry to prevent auto-scroll
    pub(super) copy_mode_frozen_offset: usize,
    /// Previous focus state, used to detect focus transitions for DEC 1004 events.
    was_focused: bool,
    /// Last hovered cell position for URL regex detection (US-015).
    pub(super) hovered_cell: Option<AlacPoint>,
    /// Active hyperlink under Ctrl+hover - drives underline rendering and Ctrl+click.
    pub(super) ctrl_hovered_link: Option<HyperlinkZone>,
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
}

impl TerminalView {
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
        let (mut terminal, events_tx) =
            TerminalState::new_pending_with_profile(params.cols, params.rows, params.profile);
        // Route the Drop-time force-kill timer through GPUI's background
        // executor instead of a detached OS thread (Zed parity, prevents a
        // thread leak per closed pane under heavy use).
        terminal.set_background_executor(cx.background_executor().clone());
        // The background `EventLoop` attaches to this same shared `term` + event
        // channel, so the placeholder's event loop keeps working after promotion
        // - no view-side rewiring needed.
        let term = terminal.term.clone();
        #[cfg(feature = "hera-dogfood")]
        let hera_pty_tap = terminal.hera_pty_tap();
        // Capture the foreground signal mask on the MAIN thread so the
        // background-spawned child still gets correct Ctrl-C / Ctrl-Z (US-012).
        let signal_mask = crate::terminal::pty_session::capture_foreground_signal_mask();

        let view = Self::from_terminal_state(workspace_id, terminal, cx);

        let executor = cx.background_executor().clone();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                // Blocking PTY open (`tty::new` forks) runs off the render thread.
                let spawned = executor
                    .spawn(async move {
                        TerminalState::open_pty_and_eventloop(
                            params,
                            term,
                            events_tx,
                            signal_mask,
                            #[cfg(feature = "hera-dogfood")]
                            hera_pty_tap,
                        )
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    match spawned {
                        Ok(spawned) => {
                            view.terminal.promote(spawned);
                            // The grid may have been resized to the real display
                            // size during the pending phase (before the PTY existed,
                            // so that SIGWINCH was dropped). Push the current grid
                            // size to the freshly-opened child now, or it stays at
                            // the initial spawn size until the next resize.
                            let (cols, rows) = crate::terminal::types::grid_size(
                                &view.terminal.term.lock_unfair(),
                            );
                            view.terminal.notifier.notify_resize(
                                cols as u16,
                                rows as u16,
                                view.cell_width.as_f32() as u16,
                                view.line_height.as_f32() as u16,
                            );
                        }
                        Err(e) => {
                            // Spawn failed: keep the display-only placeholder and
                            // surface the error in-pane (no orphan, no panic - same
                            // outcome as the old synchronous fallback).
                            log::error!("PTY creation failed: {e:#}");
                            view.terminal
                                .write_output(spawn_error_message(&e).as_bytes());
                        }
                    }
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

        // Event batch coalescing (Zed pattern):
        // Phase 1: Block until first event (zero CPU when idle)
        // Phase 2: Batch for 4ms, max 100 events, dedup Wakeup
        // Phase 3: Process batch, yield to other GPUI tasks
        let events_rx = terminal.events_rx.take().expect("events_rx already taken");
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let mut events_rx = events_rx;
                loop {
                    // Phase 1: Block until first event arrives (zero CPU when idle)
                    let Some(first_event) = events_rx.next().await else {
                        break; // Channel closed
                    };

                    // Phase 2: Batch additional events for 4ms, max 100
                    let mut batch: Vec<AlacEvent> = Vec::with_capacity(32);
                    let mut had_wakeup = matches!(first_event, AlacEvent::Wakeup);
                    if !had_wakeup {
                        batch.push(first_event);
                    }

                    {
                        let timer = futures::FutureExt::fuse(smol::Timer::after(
                            std::time::Duration::from_millis(4),
                        ));
                        futures::pin_mut!(timer);
                        loop {
                            futures::select_biased! {
                                event = events_rx.next() => {
                                    match event {
                                        Some(AlacEvent::Wakeup) => had_wakeup = true,
                                        Some(e) => batch.push(e),
                                        None => break,
                                    }
                                    if batch.len() >= 100 { break; }
                                }
                                _ = timer => break,
                            }
                        }
                    }

                    // Phase 3: Process the batch in a single entity update
                    let result = cx.update(|cx| {
                        this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                            let old_title = view.terminal.title.clone();
                            let old_cwd = view.terminal.current_cwd.clone();
                            view.terminal.sync_channels();
                            if had_wakeup {
                                view.terminal.process_event(AlacEvent::Wakeup);
                            }
                            for event in batch {
                                view.terminal.process_event(event);
                            }

                            // Execute deferred clipboard operations (OSC 52)
                            for op in view.terminal.pending_clipboard_ops.drain(..) {
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
                                        view.terminal.notifier.notify(response.into_bytes());
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
                            if view.terminal.pending_size_ops.len() > 8 {
                                let drop_count = view.terminal.pending_size_ops.len() - 8;
                                view.terminal.pending_size_ops.drain(..drop_count);
                            }
                            for format_fn in view.terminal.pending_size_ops.drain(..) {
                                // Read-only snapshot; lock_unfair avoids queueing
                                // behind the PTY reader thread on the main path.
                                let term = view.terminal.term.lock_unfair();
                                let size = alacritty_terminal::event::WindowSize {
                                    num_cols: term.columns() as u16,
                                    num_lines: term.screen_lines() as u16,
                                    cell_width: view.cell_width.as_f32() as u16,
                                    cell_height: view.line_height.as_f32() as u16,
                                };
                                drop(term);
                                let response = format_fn(size);
                                view.terminal.notifier.notify(response.into_bytes());
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

                            if view.terminal.dirty {
                                view.terminal.dirty = false;
                                // Leading edge + throttle (replaces the old
                                // every-10th-tick modulo): the FIRST dirty
                                // batch after a quiet spell fires immediately
                                // - a dev server printing its banner in fewer
                                // than 10 batches then going silent used to
                                // be missed entirely - and sustained output
                                // re-fires at most every 300ms.
                                const BURST_THROTTLE: std::time::Duration =
                                    std::time::Duration::from_millis(300);
                                let now = std::time::Instant::now();
                                if view
                                    .terminal
                                    .last_activity_burst
                                    .is_none_or(|t| now.duration_since(t) >= BURST_THROTTLE)
                                {
                                    view.terminal.last_activity_burst = Some(now);
                                    for service in view.terminal.scan_output() {
                                        cx.emit(TerminalEvent::ServiceDetected(service));
                                    }
                                    cx.emit(TerminalEvent::ActivityBurst);
                                }

                                // Copy mode: restore frozen display offset
                                if view.copy_mode_active {
                                    let mut term = view.terminal.term.lock();
                                    let current = term.grid().display_offset();
                                    let frozen = view.copy_mode_frozen_offset;
                                    if current != frozen {
                                        let delta = frozen as i32 - current as i32;
                                        term.scroll_display(AlacScroll::Delta(delta));
                                    }
                                }

                                cx.notify();
                            }
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
            copy_cursor: AlacPoint::new(GridLine(0), GridCol(0)),
            copy_mode_frozen_offset: 0,
            was_focused: false,
            hovered_cell: None,
            ctrl_hovered_link: None,
            mouse_down_link: None,
            ime_marked_text: String::new(),
            needs_initial_clear: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
        cx.notify();
    }

    /// Clear preedit text (cancel composition).
    pub fn clear_marked_text(&mut self, cx: &mut Context<Self>) {
        self.ime_marked_text.clear();
        cx.notify();
    }

    /// Commit composed text to the PTY.
    pub fn commit_text(&mut self, text: &str, _cx: &mut Context<Self>) {
        self.terminal.write_to_pty(text.as_bytes().to_vec());
    }

    /// Send arbitrary text to the PTY (no bracketed paste wrapping).
    /// Used by AI agents and automation tools via IPC.
    pub fn send_text(&self, text: &str) {
        self.terminal.write_to_pty(text.as_bytes().to_vec());
    }

    /// Deliver text through the bracketed-paste path (EP-001 cli-cockpit):
    /// when the application enabled bracketed paste, the payload is wrapped
    /// in `ESC[200~ … ESC[201~` so embedded newlines stay literal inside a
    /// TUI's input editor instead of acting as Enter. No CR is ever appended
    /// here - submission stays a separate, explicit `send_text("\r")` write.
    pub fn paste_text(&self, text: &str) {
        let mode = *self.terminal.term.lock().mode();
        self.write_paste_text(text, mode);
    }

    /// True once the foreground terminal application has enabled DEC
    /// bracketed-paste mode (`ESC[?2004h`).
    pub fn bracketed_paste_enabled(&self) -> bool {
        self.terminal
            .term
            .lock_unfair()
            .mode()
            .contains(TermMode::BRACKETED_PASTE)
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
        let mode = *self.terminal.term.lock_unfair().mode();
        if let Some(seq) =
            crate::keys::to_esc_str(&keystroke, &Modes::from(mode), self.option_as_meta)
        {
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
    fn hovered_line_text(&self) -> Option<(alacritty_terminal::index::Line, String, Vec<usize>)> {
        let point = self.hovered_cell?;
        // Read-only grid scan for hover detection; unfair lock keeps the
        // mouse-move hot path off the PTY reader's fair queue.
        let term = self.terminal.term.lock_unfair();
        let grid = term.grid();
        let line = point.line;
        // US-011 hardening: a stale hovered cell (captured before a resize,
        // `clear`, or alt-screen swap) may now be outside the grid's line
        // range. alacritty bounds-checks the grid only under debug_assert!, so
        // guard before indexing to avoid a release-mode panic.
        if line < term.topmost_line() || line > term.bottommost_line() {
            return None;
        }

        let cols = term.columns();
        let mut line_text = String::with_capacity(cols);
        let mut char_to_col: Vec<usize> = Vec::with_capacity(cols);
        for col in 0..cols {
            let cell = &grid[line][alacritty_terminal::index::Column(col)];
            if cell
                .flags
                .contains(alacritty_terminal::term::cell::Flags::WIDE_CHAR_SPACER)
            {
                continue;
            }
            char_to_col.push(col);
            line_text.push(cell.c);
        }
        Some((line, line_text, char_to_col))
    }

    pub(super) fn detect_links_at_hover(&self) -> Vec<HyperlinkZone> {
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

        let mut zones = crate::terminal::element::detect_urls_on_line_mapped(trimmed, line, map);
        zones.extend(crate::terminal::element::detect_file_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
        zones.extend(crate::terminal::element::detect_code_paths_on_line_mapped(
            trimmed, line, map, cwd,
        ));
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
        let mode = *self.terminal.term.lock_unfair().mode();
        let mut ctx = KeyContext::default();
        ctx.add("Terminal");

        // Screen mode
        if mode.contains(TermMode::ALT_SCREEN) {
            ctx.set("screen", "alt");
        } else {
            ctx.set("screen", "normal");
        }

        // DEC private modes
        if mode.contains(TermMode::APP_CURSOR) {
            ctx.add("DECCKM");
        }
        if mode.contains(TermMode::APP_KEYPAD) {
            ctx.add("DECPAM");
        }
        if mode.contains(TermMode::BRACKETED_PASTE) {
            ctx.add("bracketed_paste");
        }
        if mode.contains(TermMode::FOCUS_IN_OUT) {
            ctx.add("report_focus");
        }
        if mode.contains(TermMode::ALTERNATE_SCROLL) {
            ctx.add("alternate_scroll");
        }

        // Mouse reporting mode
        if mode.intersects(TermMode::MOUSE_MODE) {
            ctx.add("any_mouse_reporting");
            if mode.contains(TermMode::MOUSE_MOTION) {
                ctx.set("mouse_reporting", "motion");
            } else if mode.contains(TermMode::MOUSE_DRAG) {
                ctx.set("mouse_reporting", "drag");
            } else {
                ctx.set("mouse_reporting", "click");
            }
        } else {
            ctx.set("mouse_reporting", "off");
        }

        // Mouse encoding format
        if mode.contains(TermMode::SGR_MOUSE) {
            ctx.set("mouse_format", "sgr");
        } else if mode.contains(TermMode::UTF8_MOUSE) {
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
        let regex_toggle = div()
            .id("search-regex-toggle")
            .flex()
            .items_center()
            .justify_center()
            .size(px(22.))
            .rounded(px(5.))
            .cursor_pointer()
            .border_1()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .when(regex_active, |el| {
                el.bg(ui.subtle).border_color(ui.accent).text_color(ui.text)
            })
            .when(!regex_active, |el| {
                el.border_color(hsla(0., 0., 0., 0.))
                    .text_color(ui.muted)
                    .hover(|s| {
                        let ui = crate::theme::ui_colors();
                        s.bg(ui.subtle)
                    })
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
            .cursor_pointer()
            .text_size(px(12.))
            .text_color(ui.muted)
            .hover(|s| {
                let ui = crate::theme::ui_colors();
                s.bg(ui.subtle)
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
                .cursor_pointer()
                .hover(|s| {
                    let ui = crate::theme::ui_colors();
                    s.bg(ui.subtle)
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

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);

        // DEC 1004: send focus in/out events on focus transitions
        if focused != self.was_focused {
            let mode = { *self.terminal.term.lock_unfair().mode() };
            if mode.contains(TermMode::FOCUS_IN_OUT) {
                // Automated protocol write, NOT user input - go through the
                // notifier directly so US-002's keyboard_input_sent flag is not
                // tripped by a mere focus change (a failed-spawn pane that gets
                // focused must still count as "no input" and stay open).
                if focused {
                    self.terminal.write_to_pty_silent(b"\x1b[I".to_vec());
                } else {
                    self.terminal.write_to_pty_silent(b"\x1b[O".to_vec());
                }
            }
            self.was_focused = focused;
        }

        // Update cell dimensions for mouse → grid mapping
        let dims =
            crate::terminal::element::measure_cell(window, cx, self.terminal.font_size_override);
        self.cell_width = dims.cell_width;
        self.line_height = dims.line_height;

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
            let (anchor_grid_line, anchor_col) = {
                let term = self.terminal.term.lock_unfair();
                term.selection
                    .as_ref()
                    .and_then(|sel| sel.to_range(&term))
                    .map(|range| (Some(range.start.line.0), range.start.column.0))
                    .unwrap_or((None, 0))
            };
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
        let cursor_visible = self.cursor_visible
            || self
                .terminal
                .term
                .lock_unfair()
                .mode()
                .contains(TermMode::ALT_SCREEN);

        // EP-006 US-017: match positions for the scrollbar rail, converted
        // from grid-absolute lines to lines-from-bottom under a short lock
        // (the `scroll_to_match` reference conversion). Empty when no
        // search → the rail disappears at the same repaint (US-017 AC).
        let search_rail_lines: Vec<usize> = if self.search_active && !self.search_matches.is_empty()
        {
            let bottom = self.terminal.term.lock_unfair().bottommost_line();
            self.search_matches
                .iter()
                .map(|m| (bottom - m.start.line).0.max(0) as usize)
                .collect()
        } else {
            Vec::new()
        };

        let terminal_element = TerminalElement::new(
            self.terminal.term.clone(),
            PtyNotifier(self.terminal.notifier.0.clone()),
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
            self.ctrl_hovered_link.as_ref().map(|link| link.uri.clone()),
            self.ime_marked_text.clone(),
            self.focus_handle.clone(),
            cx.entity().clone(),
            self.needs_initial_clear.clone(),
            self.scrollbar_metrics.clone(),
            search_rail_lines,
            self.default_cursor_shape,
            self.cursor_color_override,
            self.terminal_material_active,
            self.integrated_glyphs_enabled,
            self.color_emoji_enabled,
            #[cfg(debug_assertions)]
            keystroke_at,
        );

        #[cfg(feature = "hera-dogfood")]
        let terminal_body = match self.terminal.hera_shadow.side_by_side_surface() {
            Some(surface) => div()
                .size_full()
                .flex()
                .flex_row()
                .child(div().flex_1().size_full().child(terminal_element))
                .child(render_hera_side_by_side_surface(
                    surface,
                    self.copy_mode_active,
                    !self.ime_marked_text.is_empty(),
                )),
            None => div().size_full().child(terminal_element),
        };

        #[cfg(not(feature = "hera-dogfood"))]
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
            .on_any_mouse_down(cx.listener(Self::handle_mouse_down))
            .on_mouse_move(cx.listener(Self::handle_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
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
        // the grid-only scrollback round-trip.
        let state = TerminalState::new_display_only(24, 80);

        // Feed some text into the terminal grid
        state.restore_scrollback("line one\nline two\nline three");

        // Extract it back
        let scrollback = state.extract_scrollback();
        assert!(scrollback.is_some(), "Expected scrollback content");
        let text = scrollback.unwrap();
        assert!(text.contains("line one"), "Missing 'line one' in: {text}");
        assert!(text.contains("line two"), "Missing 'line two' in: {text}");
        assert!(
            text.contains("line three"),
            "Missing 'line three' in: {text}"
        );
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
