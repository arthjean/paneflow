//! Keyboard, mouse, clipboard, and scroll handlers for `TerminalView`.
//!
//! Every method here is an `impl TerminalView` entry reached from the GPUI
//! event dispatch wired in `mod.rs::Render`. Field access from these methods
//! is what forces the `pub(super)` visibility on `TerminalView`'s fields.
//!
//! Extracted from `terminal.rs` per US-013 of the src-app refactor PRD.

use std::borrow::Cow;

use gpui::{
    ClipboardEntry, ClipboardItem, Context, ExternalPaths, Focusable, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, TouchPhase, Window,
};

use crate::mouse;
use crate::terminal::types::{
    HyperlinkSource, HyperlinkZone, Modes, Point, SelectionKind, SelectionSide, ShellQuoting,
};

#[cfg(debug_assertions)]
use super::probe_enabled;
use super::{TerminalEvent, TerminalView};

/// Returns true when the platform-appropriate "open link" modifier is held:
/// Cmd on macOS, Ctrl on Linux/Windows (US-019 AC).
#[inline]
fn open_link_modifier_held(modifiers: &gpui::Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.platform
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control
    }
}

/// Sanitize and wrap `text` for a single bracketed-paste PTY write
/// (`ESC[200~` … `ESC[201~`). ESC and C1 control bytes (U+0080..=U+009F) are
/// stripped so the payload cannot close the paste early or smuggle a CSI
/// escape. No carriage return is ever appended - submission stays a SEPARATE
/// `\r` write (EP-001 US-001, agent-control-plane-hardening), so an agent that
/// reads a burst as an unconfirmed paste never swallows the Enter. Embedded
/// newlines are kept literal on purpose: that is the whole point of bracketed
/// paste (the agent's input editor receives them as text, not as submit).
pub(super) fn wrap_bracketed_paste(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let sanitized: String = normalized
        .chars()
        .filter(|&c| c != '\x1b' && !(('\u{0080}'..='\u{009f}').contains(&c)))
        .collect();
    format!("\x1b[200~{sanitized}\x1b[201~")
}

/// Convert a slice of OS paths to a single space-joined, shell-quoted string
/// for pasting into a PTY (US-021). `None` when every path is filtered out
/// (newline, carriage-return, or null bytes). Newline and CR are both rejected
/// because the non-bracketed paste sink rewrites `\n` to `\r` and passes a bare
/// `\r` verbatim, which the shell treats as Enter.
///
/// Only a conservative ASCII path subset is left unquoted; metacharacters like
/// `;`, `&`, `$`, spaces, and quotes are always quoted. Windows uses the
/// PowerShell-compatible single-quote form because Paneflow's default Windows
/// shell resolver prefers PowerShell over cmd.exe. Shared by `handle_file_drop`
/// and `handle_paste`.
fn paths_to_pty_text(paths: &[std::path::PathBuf], shell_quoting: ShellQuoting) -> Option<String> {
    let quoted: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let s = p.to_string_lossy();
            // Reject paths with newline, carriage-return, or null bytes: NUL
            // breaks shell quoting; LF/CR can inject a line submit (Enter) past
            // the single-quote wrapping.
            if s.contains('\n') || s.contains('\r') || s.contains('\0') {
                return None;
            }
            Some(quote_path_for_shell(&s, shell_quoting))
        })
        .collect();
    if quoted.is_empty() {
        None
    } else {
        Some(quoted.join(" "))
    }
}

fn quote_path_for_shell(path: &str, shell_quoting: ShellQuoting) -> String {
    match shell_quoting {
        ShellQuoting::Posix => quote_posix_path(path),
        ShellQuoting::PowerShell => quote_powershell_path(path),
        ShellQuoting::Cmd => quote_cmd_path(path),
    }
}

fn quote_posix_path(path: &str) -> String {
    if path.chars().all(posix_unquoted_path_char) {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

fn posix_unquoted_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':')
}

fn quote_powershell_path(path: &str) -> String {
    if path.chars().all(windows_unquoted_path_char) {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "''"))
    }
}

fn windows_unquoted_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '\\' | '.' | '_' | '-' | ':')
}

fn quote_cmd_path(path: &str) -> String {
    if path.chars().all(windows_unquoted_path_char) {
        path.to_string()
    } else {
        let escaped = path
            .replace('^', "^^")
            .replace('%', "^%")
            .replace('!', "^!")
            .replace('"', "\"\"");
        format!("\"{escaped}\"")
    }
}

impl TerminalView {
    pub(super) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cancel swap mode on Escape - checked before any other mode handling
        if crate::SWAP_MODE.load(std::sync::atomic::Ordering::Relaxed)
            && event.keystroke.key == "escape"
        {
            cx.emit(TerminalEvent::CancelSwapMode);
            return;
        }

        // The find bar owns keyboard input via its focused `TextInput` entity
        // (typing, IME, selection, clipboard); search-scoped action bindings
        // (SearchNext / SearchPrev / DismissSearch / regex / fleet) are
        // dispatched by GPUI before this handler. The terminal must not also
        // forward these keys to the PTY, so bail out while the overlay is open.
        if self.search_active {
            return;
        }

        // When copy mode is active, intercept navigation and exit keys
        if self.copy_mode_active {
            let keystroke = &event.keystroke;
            let key = keystroke.key.as_str();
            let shift = keystroke.modifiers.shift;

            match key {
                "left" | "right" | "up" | "down" => {
                    let (dx, dy): (i32, i32) = match key {
                        "left" => (-1, 0),
                        "right" => (1, 0),
                        "up" => (0, -1),
                        "down" => (0, 1),
                        _ => unreachable!(),
                    };
                    if shift {
                        self.extend_copy_selection(dx, dy, cx);
                    } else {
                        self.move_copy_cursor(dx, dy, cx);
                    }
                }
                "enter" => {
                    self.exit_copy_mode(true, cx);
                }
                "escape" => {
                    self.exit_copy_mode(false, cx);
                }
                _ => {
                    // 'q' exits copy mode (vi-style)
                    if keystroke.key_char.as_deref() == Some("q")
                        && !keystroke.modifiers.control
                        && !keystroke.modifiers.alt
                    {
                        self.exit_copy_mode(false, cx);
                    }
                    // All other keys consumed - not sent to PTY
                }
            }
            return;
        }

        #[cfg(debug_assertions)]
        let _probe_start = if probe_enabled() {
            Some(std::time::Instant::now())
        } else {
            None
        };

        // Reset cursor blink on keystroke
        self.cursor_visible = true;

        let keystroke = &event.keystroke;

        // End key (no modifiers) while scrolled back - snap to bottom instead of
        // sending "end of line" to the shell.
        if keystroke.key == "end"
            && !keystroke.modifiers.shift
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            let backend = self.terminal.session_backend();
            if backend.grid_metrics().display_offset > 0 {
                backend.scroll_to_bottom();
                self.terminal.dirty = true;
                // Reset accumulated sub-line scroll so the next wheel tick
                // does not "snap back" by the leftover fraction.
                self.scroll_remainder = 0.0;
                cx.notify();
                return;
            }
        }

        // Get current TermMode for key mapping (APP_CURSOR, etc.)
        let mode = self.terminal.session_backend().modes();

        // Special keys / modifiers → write the escape sequence directly.
        // Printable characters are NOT handled here: GPUI's InputHandler
        // (replace_text_in_range) is the single source of truth for them on
        // both normal and alt screens. Writing them here as well caused
        // character doubling in ALT_SCREEN mode (e.g. Claude Code fullscreen TUI).
        if let Some(seq) = crate::keys::to_esc_str(keystroke, &mode, self.option_as_meta) {
            // Snap to bottom on input. Matches Zed `terminal.rs:input()` - if
            // the user is scrolled back in the history and types, the shell's
            // echo would otherwise be invisible.
            {
                let backend = self.terminal.session_backend();
                if backend.grid_metrics().display_offset > 0 {
                    backend.scroll_to_bottom();
                    self.terminal.dirty = true;
                    self.scroll_remainder = 0.0;
                }
            }
            match seq {
                Cow::Borrowed(s) => {
                    self.terminal.write_to_pty(Cow::Borrowed(s.as_bytes()));
                }
                Cow::Owned(s) => {
                    self.terminal.write_to_pty(s.into_bytes());
                }
            }
        }

        #[cfg(debug_assertions)]
        if let Some(start) = _probe_start {
            let elapsed = start.elapsed();
            // Store timestamp for total keystroke→pixel measurement in paint()
            self.terminal.last_keystroke_at = Some(start);
            if elapsed.as_millis() > 1 {
                log::warn!(
                    "[latency] keystroke→PTY: {:.2}ms",
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }

    // --- Pixel → grid coordinate conversion ---

    pub(super) fn pixel_to_grid(&self, pos: gpui::Point<gpui::Pixels>) -> (Point, SelectionSide) {
        // Poison-safe: if a panic happened inside paint() while holding the
        // lock, the inner Point is still a valid value - recover and continue.
        let origin = *self
            .element_origin
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let relative_x = (pos.x - origin.x).max(gpui::px(0.0));
        let relative_y = (pos.y - origin.y).max(gpui::px(0.0));

        let col_f = relative_x / self.cell_width;
        let half_cell = self.cell_width / 2.0;
        let cell_x = relative_x % self.cell_width;
        let side = if cell_x > half_cell {
            SelectionSide::Right
        } else {
            SelectionSide::Left
        };

        let metrics = self.terminal.session_backend().grid_metrics();
        let max_col = metrics.columns.saturating_sub(1);
        let max_line = metrics.screen_lines.saturating_sub(1) as i32;

        let col = (col_f as usize).min(max_col);
        let line = ((relative_y / self.line_height) as i32).min(max_line);

        (Point::new(line - metrics.display_offset as i32, col), side)
    }

    /// Convert pixel position to viewport grid coordinates (for mouse reporting).
    /// Unlike `pixel_to_grid`, this returns 0-based viewport coordinates without
    /// the scrollback display_offset subtraction.
    pub(super) fn pixel_to_viewport(&self, pos: gpui::Point<gpui::Pixels>) -> Point {
        let origin = *self
            .element_origin
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let relative_x = (pos.x - origin.x).max(gpui::px(0.0));
        let relative_y = (pos.y - origin.y).max(gpui::px(0.0));
        let col_f = relative_x / self.cell_width;
        let metrics = self.terminal.session_backend().grid_metrics();
        let max_col = metrics.columns.saturating_sub(1);
        let max_line = metrics.screen_lines.saturating_sub(1) as i32;
        let col = (col_f as usize).min(max_col);
        let line = ((relative_y / self.line_height) as i32).min(max_line);
        Point::new(line, col)
    }

    /// Write a mouse report to the PTY using the appropriate encoding format.
    pub(super) fn write_mouse_report(&self, point: Point, button: u8, pressed: bool, mode: Modes) {
        let format = mouse::MouseFormat::from_mode(mode);
        let bytes = match format {
            mouse::MouseFormat::Sgr => mouse::sgr_mouse_report(point, button, pressed).into_bytes(),
            mouse::MouseFormat::Normal { utf8 } => {
                // Normal/UTF-8 encoding: release always uses button code 3 (no per-button release)
                let btn = if pressed { button } else { 3 };
                match mouse::normal_mouse_report(point, btn, utf8) {
                    Some(b) => b,
                    None => return, // position exceeds encoding limits
                }
            }
        };
        self.terminal.write_to_pty(bytes);
    }

    // --- Mouse selection handlers ---

    /// US-015: if `x` falls on the (widened) scrollbar strip and there is
    /// scrollback to navigate, return the painted geometry for hit-testing.
    fn scrollbar_hit(&self, x: gpui::Pixels) -> Option<super::element::ScrollbarMetrics> {
        let metrics = {
            *self
                .scrollbar_metrics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
        }?;
        metrics
            .strip_contains_x(x, gpui::px(6.0))
            .then_some(metrics)
    }

    /// US-015: scroll the grid so `target_offset` scrollback lines sit above the
    /// viewport. `target_offset` is pre-clamped to history by the caller
    /// (`ScrollbarMetrics::offset_for_y`). No-op when already there.
    fn apply_scrollbar_jump(&mut self, target_offset: usize, history_size: usize) -> bool {
        let row = history_size.saturating_sub(target_offset.min(history_size));
        if self.terminal.session_backend().scroll_to_viewport_row(row) {
            self.terminal.dirty = true;
            true
        } else {
            false
        }
    }

    /// Apply a drag step relative to the last target accepted by the backend.
    /// Relative steps compose with Ghostty's live viewport pin when output or
    /// reflow changes the scrollback while the pointer is held.
    fn apply_scrollbar_drag_delta(&mut self, delta_lines: i64) -> bool {
        if delta_lines == 0
            || !self
                .terminal
                .session_backend()
                .scroll_delta(delta_lines.clamp(i32::MIN as i64, i32::MAX as i64) as i32)
        {
            return false;
        }
        self.terminal.dirty = true;
        true
    }

    fn scrollbar_drag_target(drag: super::view::ScrollbarDrag, pointer_y: gpui::Pixels) -> usize {
        let usable = drag.metrics.thumb_travel().max(gpui::px(1.0));
        let dy = (pointer_y - drag.anchor_y) / usable;
        let delta_lines = (dy * drag.metrics.history_size as f32).round() as i64;
        (drag.anchor_offset as i64 - delta_lines).clamp(0, drag.metrics.history_size as i64)
            as usize
    }

    pub(super) fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_handle(cx).focus(window, cx);

        // US-015: a Left press on the scrollbar strip starts a jump/drag and
        // consumes the event - no text selection, no mouse report. Checked
        // first so the strip wins over selection on the right edge. Gated on
        // scrollback existing (alt-screen TUIs have none, so this never fires
        // over them).
        if event.button == MouseButton::Left
            && let Some(metrics) = self.scrollbar_hit(event.position.x)
        {
            // A press on the bare track first jumps to that proportional
            // position; a press on the thumb grabs it in place (no jump).
            // Either way the pointer gesture owns an absolute target. The
            // painted geometry stays frozen until release so output or reflow
            // cannot change its scale under the cursor.
            let mut last_target = metrics.display_offset;
            let anchor_offset = if metrics.y_on_thumb(event.position.y) {
                metrics.display_offset
            } else {
                let target = metrics.offset_for_y(event.position.y);
                if self.apply_scrollbar_jump(target, metrics.history_size) {
                    last_target = target;
                }
                target
            };
            self.scrollbar_drag = Some(super::view::ScrollbarDrag {
                anchor_y: event.position.y,
                anchor_offset,
                metrics,
                last_target,
            });
            cx.notify();
            return;
        }

        // Cmd/Ctrl+Left-click on a link (US-012): DEFER the open to mouse-up.
        // Record the link under the press and start a selection so a Ctrl+drag
        // selects text instead of opening; the open fires on release only if
        // the selection is still empty (no drag). Mirrors Zed's
        // mouse_down/mouse_up hyperlink match (terminal.rs:2209-2310).
        if event.button == MouseButton::Left
            && open_link_modifier_held(&event.modifiers)
            && event.click_count == 1
            && self.ctrl_hovered_link.is_some()
        {
            self.mouse_down_link = self.ctrl_hovered_link.clone();
            let (point, side) = self.pixel_to_grid(event.position);
            self.terminal
                .session_backend()
                .start_selection(SelectionKind::Simple, point, side);
            self.selecting = true;
            cx.notify();
            return;
        }

        let mode = self.terminal.session_backend().modes();

        // Forward to PTY when mouse reporting is active.
        // Shift overrides mouse mode for text selection (standard terminal convention).
        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            // Side/Navigate mouse buttons have no terminal report encoding;
            // skip them instead of injecting a phantom Left click.
            if let Some(button) = mouse::mouse_button_code(event.button, event.modifiers) {
                let point = self.pixel_to_viewport(event.position);
                self.write_mouse_report(point, button, true, mode);
            }
            return;
        }

        // Text selection (mouse mode inactive or Shift held)
        if event.button != MouseButton::Left {
            return;
        }

        let (point, side) = self.pixel_to_grid(event.position);

        let selection_type = match event.click_count {
            1 => SelectionKind::Simple,
            2 => SelectionKind::Semantic,
            3 => SelectionKind::Lines,
            _ => return,
        };

        self.terminal
            .session_backend()
            .start_selection(selection_type, point, side);

        self.selecting = true;
        cx.notify();
    }

    pub(super) fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // US-015: while dragging the scrollbar, map the pixel delta since the
        // grab to an absolute scrollback target and consume the event (no
        // selection / mouse report). The drag continues even if the pointer
        // leaves the strip horizontally.
        if let Some(mut drag) = self.scrollbar_drag {
            if event.pressed_button == Some(MouseButton::Left) {
                // Drag down (positive dy) scrolls toward the live edge. Use
                // the exact frozen course swept by the painted thumb.
                let target = Self::scrollbar_drag_target(drag, event.position.y);
                let step = target as i64 - drag.last_target as i64;
                if target != drag.last_target && self.apply_scrollbar_drag_delta(step) {
                    drag.last_target = target;
                    self.scrollbar_drag = Some(drag);
                    cx.notify();
                }
            } else {
                // Button released without our up-handler seeing it (defensive).
                self.scrollbar_drag = None;
            }
            return;
        }

        let mode = self.terminal.session_backend().modes();

        // Forward motion to PTY when mouse tracking is active.
        // Shift overrides mouse mode for text selection.
        if !event.modifiers.shift
            && (mode.contains(Modes::MOUSE_MOTION)
                || (mode.contains(Modes::MOUSE_DRAG) && event.pressed_button.is_some()))
        {
            // Skip motion reports for side/Navigate buttons - they have no
            // terminal mouse-report encoding.
            let button_base = match event.pressed_button {
                Some(btn) => match mouse::mouse_button_code(btn, event.modifiers) {
                    Some(b) => b,
                    None => return,
                },
                None => 3, // no button held = release code in motion reports
            };
            let point = self.pixel_to_viewport(event.position);
            // Motion events add +32 to the button code per protocol spec
            let button = button_base + 32;
            self.write_mouse_report(point, button, true, mode);
            return;
        }

        // Track hovered cell for URL regex detection (US-015).
        // Save the prior cell so we can throttle the per-frame rescan below.
        let (hover_point, _) = self.pixel_to_grid(event.position);
        let prev_hovered_cell = self.hovered_cell;
        self.hovered_cell = Some(hover_point);

        // Cmd/Ctrl+hover: detect link under cursor for hyperlink rendering
        // (US-016 + US-019). OSC 8 takes priority over regex URL detection,
        // which takes priority over file-path detection.
        if open_link_modifier_held(&event.modifiers) {
            // Throttle: only re-scan the line when the hovered cell changed.
            // Without this, 60 fps of MouseMove with the modifier held = 60
            // regex scans / s and a Term lock per frame. The scan result is
            // cached per-cell REGARDLESS of whether a link was found, so a
            // stationary Ctrl-hold over non-link text (the common case) does
            // NOT rescan on sub-cell jitter (US-011 AC3: no per-event scanning).
            // Same-cell first-detect on modifier press is handled separately by
            // `handle_modifiers_changed`. Matches Zed's FIND_HYPERLINK_THROTTLE_PX.
            let hovered_cell_changed = prev_hovered_cell != Some(hover_point);
            if !hovered_cell_changed {
                return;
            }

            self.refresh_hovered_link(hover_point, cx);
        } else if self.ctrl_hovered_link.is_some() {
            self.ctrl_hovered_link = None;
            cx.notify();
        }

        // Text selection (mouse mode inactive)
        if !self.selecting {
            return;
        }

        let (point, side) = self.pixel_to_grid(event.position);

        let primary_text = self
            .terminal
            .session_backend()
            .update_selection(point, side);
        // On Linux/freebsd, mirror backends that can format the in-progress
        // selection without blocking into the X11/Wayland PRIMARY buffer.
        // Ghostty defers formatting until mouse-up to keep this UI hot path
        // asynchronous; `finish_selection` writes the committed value below.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        if let Some(text) = primary_text
            && !text.is_empty()
        {
            cx.write_to_primary(ClipboardItem::new_string(text));
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd")))]
        drop(primary_text);

        cx.notify();
    }

    /// US-011/US-012: resolve the hyperlink under `hover_point` through the
    /// OSC 8 → URL → markdown → code-path priority chain and store it in
    /// `ctrl_hovered_link`. Shared by mouse-move (throttled by the caller) and
    /// the modifiers-changed handler (runs on Ctrl/Cmd press without a move).
    fn refresh_hovered_link(&mut self, hover_point: Point, cx: &mut Context<Self>) {
        // OSC 8 explicit hyperlink on the hovered cell takes priority.
        let osc8_link = self
            .terminal
            .session_backend()
            .osc8_hyperlink_at(hover_point);
        let in_zone = |z: &HyperlinkZone| {
            hover_point.line == z.start.line
                && hover_point.column >= z.start.column
                && hover_point.column <= z.end.column
        };
        self.ctrl_hovered_link = osc8_link.or_else(|| {
            self.detect_links_at_hover()
                .into_iter()
                .find(|z| in_zone(z))
        });
        cx.notify();
    }

    pub(super) fn handle_modifiers_changed(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // US-011: GPUI fires no MouseMove when only a modifier changes, so link
        // detection would otherwise not run until the mouse jiggles - making
        // the first Ctrl-click miss. Re-run detection on the last hovered cell
        // when the open-modifier becomes held, and clear on release.
        if open_link_modifier_held(&event.modifiers) {
            if let Some(point) = self.hovered_cell {
                self.refresh_hovered_link(point, cx);
            }
        } else if self.ctrl_hovered_link.is_some() {
            self.ctrl_hovered_link = None;
            cx.notify();
        }
    }

    /// US-012: open a resolved hyperlink. `.md` routes to the in-pane markdown
    /// viewer, code paths to the editor chain (both via app-level events so the
    /// VISUAL/EDITOR resolution stays testable), and URLs / OSC 8 to the OS
    /// handler. Shared routing for the mouse-up open.
    fn open_hyperlink(&self, link: &HyperlinkZone, cx: &mut Context<Self>) {
        match link.source {
            HyperlinkSource::FilePath => {
                cx.emit(TerminalEvent::OpenMarkdownPath(std::path::PathBuf::from(
                    &link.uri,
                )));
            }
            HyperlinkSource::CodePath => {
                cx.emit(TerminalEvent::OpenCodePath {
                    path: std::path::PathBuf::from(&link.uri),
                    line: link.line,
                    col: link.col,
                });
            }
            HyperlinkSource::Osc8 | HyperlinkSource::Regex => {
                if let Err(err) = crate::external_open::open_url(&link.uri) {
                    log::warn!("terminal: open URL failed: {err}");
                }
            }
        }
    }

    pub(super) fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // US-015: end a scrollbar drag on the LEFT release without running the
        // selection cleanup below (we never started a selection). Scoped to
        // Left so a right-click release mid-drag still reaches the PTY
        // mouse-report path and is not swallowed (which would strand a
        // mouse-mode TUI in a phantom-button-held state).
        if let Some(drag) = self.scrollbar_drag
            && event.button == MouseButton::Left
        {
            let target = Self::scrollbar_drag_target(drag, event.position.y);
            let step = target as i64 - drag.last_target as i64;
            if target != drag.last_target && self.apply_scrollbar_drag_delta(step) {
                cx.notify();
            }
            self.scrollbar_drag = None;
            return;
        }

        let mode = self.terminal.session_backend().modes();

        // Forward release to PTY when mouse reporting is active.
        // Shift overrides mouse mode for text selection.
        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            // US-012: a Ctrl-press may have stashed a pending link on mouse-down
            // (that path returns before this mouse-mode check). If the modifier
            // is released before this mouse-mode release, the link-open path
            // below is skipped and the stash would otherwise survive - clear it
            // here so it cannot phantom-open on a later plain click once mouse
            // mode ends.
            self.mouse_down_link = None;
            if let Some(button) = mouse::mouse_button_code(event.button, event.modifiers) {
                let point = self.pixel_to_viewport(event.position);
                self.write_mouse_report(point, button, false, mode);
            }
            return;
        }

        // Middle-click: paste from primary selection (X11/Wayland only;
        // `read_from_primary` is gated to linux+freebsd in GPUI - mirror
        // the same gate here. On macOS/Windows middle-click has no primary
        // paste convention, so the block is a no-op and we just return.
        if event.button == MouseButton::Middle {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            {
                if let Some(item) = cx.read_from_primary()
                    && let Some(text) = item.text()
                {
                    self.write_paste_text(&text, mode);
                }
            }
            return;
        }

        // Text selection cleanup (mouse mode inactive or Shift held)
        if event.button != MouseButton::Left {
            return;
        }
        self.selecting = false;
        // US-012: a Ctrl/Cmd-click stashed the link under the press. It opens
        // below only if the selection is empty (no drag); a Ctrl+drag that
        // started on a link became a text selection and copies instead.
        let down_link = self.mouse_down_link.take();

        // Clear empty selections, or auto-copy non-empty selections (tmux-style):
        // write to both PRIMARY (middle-click paste) and CLIPBOARD (Ctrl+V),
        // then clear the selection so the disappearing highlight signals the copy.
        let (selection_empty, copied) = self.terminal.session_backend().finish_selection();

        // US-012: open on a genuine click (empty selection = no drag).
        if selection_empty
            && let Some(link) = down_link
            && link.is_openable
        {
            self.open_hyperlink(&link, cx);
            cx.notify();
            return;
        }

        if let Some(text) = copied {
            #[cfg(any(target_os = "linux", target_os = "freebsd"))]
            cx.write_to_primary(ClipboardItem::new_string(text.clone()));
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            cx.emit(TerminalEvent::SelectionCopied);
        }

        cx.notify();
    }

    // --- Clipboard handlers ---

    pub(super) fn handle_copy(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.session_backend().selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        // US-021: file(s) copied in the OS file manager (Nautilus/Finder/
        // Explorer/Thunar) arrive as `ExternalPaths`. Insert the shell-quoted
        // path(s). Checked BEFORE `clipboard.text()`, which falls back to
        // unquoted path display strings - those would break on spaces. Iterate
        // all entries (some backends emit a String entry alongside the paths)
        // and fall through to text() when no `ExternalPaths` is present (e.g.
        // Wayland compositors that copy a `file://` URI as text instead).
        for entry in clipboard.entries() {
            if let ClipboardEntry::ExternalPaths(ext_paths) = entry
                && let Some(text) =
                    paths_to_pty_text(ext_paths.paths(), self.terminal.shell_quoting)
            {
                let mode = self.terminal.session_backend().modes();
                self.write_paste_text(&text, mode);
                return;
            }
        }

        // Text paste (normal Ctrl+V)
        if let Some(text) = clipboard.text() {
            let mode = self.terminal.session_backend().modes();
            self.write_paste_text(&text, mode);
            return;
        }

        // Image-only clipboard: forward raw Ctrl+V (0x16) so TUI agents can read
        // it. Text and ExternalPaths win above because they are deterministic PTY
        // input; Ctrl+V has shell-specific "literal next" behavior.
        if clipboard
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(image) if !image.bytes.is_empty()))
        {
            self.terminal.write_to_pty(vec![0x16]);
        }
    }

    /// Prepare and write paste text to PTY, respecting bracketed paste mode.
    /// Strips ESC and C1 control chars when bracketed paste is active.
    pub(super) fn handle_file_drop(
        &mut self,
        paths: &ExternalPaths,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Some(text) = paths_to_pty_text(paths.paths(), self.terminal.shell_quoting) {
            let mode = self.terminal.session_backend().modes();
            self.write_paste_text(&text, mode);
        }
    }

    pub(super) fn write_paste_text(&self, text: &str, mode: Modes) {
        let paste_text = if mode.contains(Modes::BRACKETED_PASTE) {
            wrap_bracketed_paste(text)
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.terminal.write_to_pty(paste_text.into_bytes());
    }

    /// EP-001 US-001 (agent-control-plane-hardening): deliver an automation /
    /// agent payload while NEVER synthesizing a submit out of the body. When the
    /// target has bracketed paste active, the bytes are wrapped (embedded
    /// newlines stay literal inside the agent's editor); when it does NOT, they
    /// are written VERBATIM - crucially not through the interactive `\n` -> `\r`
    /// rewrite that `write_paste_text` applies, which would turn a multi-line
    /// prompt into N carriage returns and defeat the single, SEPARATE deferred
    /// `\r` that is the only sanctioned submission (US-005 human-in-loop
    /// invariant). This is the divergence from `paste_text`: a human pressing
    /// Ctrl+V into a bare shell still wants newline -> run, but a `send_text`
    /// inject toward an agent that has not (yet) enabled `ESC[?2004h` must not
    /// smuggle Enters through the burst.
    pub fn inject_text(&self, text: &str) {
        let mode = self.terminal.session_backend().modes();
        if mode.contains(Modes::BRACKETED_PASTE) {
            self.write_paste_text(text, mode);
        } else {
            self.send_text(text);
        }
    }

    // --- Scroll handlers ---

    pub(super) fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = self.terminal.session_backend().modes();

        // Forward scroll to PTY when mouse reporting is active.
        // Shift overrides mouse mode for scrollback.
        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            let delta_y = event.delta.pixel_delta(self.line_height).y;
            self.scroll_remainder += delta_y / self.line_height;
            self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);
            let lines = self.scroll_remainder as i32;
            if lines == 0 {
                return;
            }
            self.scroll_remainder -= lines as f32;

            let point = self.pixel_to_viewport(event.position);
            let direction = if lines > 0 {
                mouse::ScrollDirection::Up
            } else {
                mouse::ScrollDirection::Down
            };
            let button = mouse::scroll_button_code(direction, event.modifiers);
            let format = mouse::MouseFormat::from_mode(mode);
            let report = match format {
                mouse::MouseFormat::Sgr => {
                    mouse::sgr_mouse_report(point, button, true).into_bytes()
                }
                mouse::MouseFormat::Normal { utf8 } => {
                    match mouse::normal_mouse_report(point, button, utf8) {
                        Some(bytes) => bytes,
                        None => return,
                    }
                }
            };
            let count = lines.unsigned_abs() as usize;
            let mut buf = Vec::with_capacity(report.len() * count);
            for _ in 0..lines.unsigned_abs() {
                buf.extend_from_slice(&report);
            }
            self.terminal.write_to_pty(buf);
            return;
        }

        // Alternate scroll: ALT_SCREEN + ALTERNATE_SCROLL without MOUSE_MODE
        // Synthesize arrow key sequences so scroll works in less, vim, htop, etc.
        if mode.contains(Modes::ALT_SCREEN | Modes::ALTERNATE_SCROLL) && !event.modifiers.shift {
            let delta_y = event.delta.pixel_delta(self.line_height).y;
            self.scroll_remainder += delta_y / self.line_height;
            self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);
            let lines = self.scroll_remainder as i32;
            if lines == 0 {
                return;
            }
            self.scroll_remainder -= lines as f32;

            let app_cursor = mode.contains(Modes::APP_CURSOR);
            let arrow: &[u8] = match (lines > 0, app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            let count = lines.unsigned_abs() as usize;
            let mut buf = Vec::with_capacity(arrow.len() * count);
            for _ in 0..count {
                buf.extend_from_slice(arrow);
            }
            self.terminal.write_to_pty(buf);
            return;
        }

        // Scrollback (mouse mode inactive, not alt screen alternate scroll).
        // US-022: reset the sub-line accumulator on gesture start so an
        // opposite-direction flick is crisp (no leftover momentum). Mouse wheels
        // arrive as `Moved`; only trackpad gestures emit Started/Ended. Mirrors
        // Zed terminal.rs `determine_scroll_lines` (TouchPhase::Started → reset).
        match event.touch_phase {
            TouchPhase::Started => {
                self.scroll_remainder = 0.0;
                return;
            }
            TouchPhase::Ended | TouchPhase::Cancelled => return,
            TouchPhase::Moved => {}
        }

        // US-022: scroll-sensitivity multiplier, cached on the view at
        // construction (no config I/O on this hot per-event path). Applied ONLY
        // here in the scrollback path - the mouse-mode and alt-scroll branches
        // above already returned, so the PTY protocol framing is never scaled
        // (Zed forces 1.0 in mouse mode for the same reason).
        let delta_y = event.delta.pixel_delta(self.line_height).y;
        self.scroll_remainder += (delta_y / self.line_height) * self.scroll_multiplier;

        // Clamp to prevent extreme values from synthesised events
        self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);

        let lines = self.scroll_remainder as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;

        // Positive wheel delta means scrolling up (toward history in natural-scroll
        // convention), which matches AlacScroll::Delta positive = scroll toward history.
        if !self.terminal.session_backend().scroll_delta(lines) {
            return;
        }
        self.terminal.dirty = true;

        cx.notify();
    }

    pub(super) fn handle_scroll_page_up(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // US-009: in an alt-screen TUI (lazygit, less, vim, k9s, full-screen
        // agent UIs) scrollback is empty, so scroll_display is a no-op and the
        // key would be silently swallowed. Forward the PageUp escape instead so
        // the TUI actually pages. `\x1b[5~` matches what `keys::to_esc_str`
        // emits for a plain PageUp - the single source of truth (asserted by a
        // test in `keys.rs`).
        let alt_screen = self
            .terminal
            .session_backend()
            .modes()
            .contains(Modes::ALT_SCREEN);
        if alt_screen {
            self.terminal.write_to_pty(b"\x1b[5~".as_slice());
            return;
        }
        if !self.terminal.session_backend().scroll_page_up() {
            return;
        }
        self.terminal.dirty = true;
        cx.notify();
    }

    pub(super) fn handle_scroll_page_down(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // US-009: see handle_scroll_page_up. `\x1b[6~` is plain PageDown.
        let alt_screen = self
            .terminal
            .session_backend()
            .modes()
            .contains(Modes::ALT_SCREEN);
        if alt_screen {
            self.terminal.write_to_pty(b"\x1b[6~".as_slice());
            return;
        }
        if !self.terminal.session_backend().scroll_page_down() {
            return;
        }
        self.terminal.dirty = true;
        cx.notify();
    }

    pub(super) fn jump_to_prompt(&mut self, backward: bool, cx: &mut Context<Self>) {
        let backend = self.terminal.session_backend();
        let metrics = backend.grid_metrics();
        let history_size = i64::from(metrics.topmost_line.0.saturating_neg());
        let top_abs = history_size.saturating_sub(metrics.display_offset as i64);
        let target = {
            let marks = self
                .terminal
                .marks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if backward {
                marks.prompt_before(top_abs)
            } else {
                marks.prompt_after(top_abs)
            }
        };
        let Some(target) = target else {
            return;
        };
        let offset = history_size.saturating_sub(target).clamp(0, history_size) as usize;
        if backend.restore_display_offset(offset) {
            self.terminal.dirty = true;
            cx.notify();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{paths_to_pty_text, wrap_bracketed_paste};
    use crate::terminal::types::ShellQuoting;
    use std::path::PathBuf;

    // EP-001 US-001 (agent-control-plane-hardening): the wrap is the burst that
    // reaches an agent; the `\r` must NEVER ride inside it.
    #[test]
    fn bracketed_wrap_has_both_sentinels_and_no_cr() {
        let wrapped = wrap_bracketed_paste("hello world");
        assert!(wrapped.starts_with("\x1b[200~"), "opens with paste-start");
        assert!(wrapped.ends_with("\x1b[201~"), "closes with paste-end");
        assert_eq!(wrapped, "\x1b[200~hello world\x1b[201~");
        assert!(!wrapped.contains('\r'), "no carriage return in the burst");
    }

    #[test]
    fn bracketed_wrap_keeps_newlines_literal() {
        // A multi-line prompt stays literal inside the sentinels (the agent's
        // editor sees text, not N submits); crucially still no `\r` injected.
        let wrapped = wrap_bracketed_paste("line one\nline two");
        assert_eq!(wrapped, "\x1b[200~line one\nline two\x1b[201~");
        assert!(!wrapped.contains('\r'));
    }

    #[test]
    fn bracketed_wrap_normalizes_crlf_to_lf() {
        let wrapped = wrap_bracketed_paste("line one\r\nline two\rline three");
        assert_eq!(wrapped, "\x1b[200~line one\nline two\nline three\x1b[201~");
        assert!(!wrapped.contains('\r'));
    }

    #[test]
    fn bracketed_wrap_strips_esc_and_c1_to_block_paste_escape() {
        // An embedded ESC[201~ or C1 control could otherwise terminate the
        // paste early and smuggle a CSI; both bytes are filtered out.
        let wrapped = wrap_bracketed_paste("a\x1b[201~b\u{0085}c");
        assert_eq!(wrapped, "\x1b[200~a[201~bc\x1b[201~");
        // Exactly one opener and one closer survive (the wrapper's own).
        assert_eq!(wrapped.matches("\x1b[200~").count(), 1);
        assert_eq!(wrapped.matches("\x1b[201~").count(), 1);
    }

    // US-021: shell-quoting of file-manager paths for paste.
    #[test]
    fn shell_quoting_detects_common_shells() {
        assert_eq!(ShellQuoting::for_shell("/bin/zsh"), ShellQuoting::Posix);
        assert_eq!(
            ShellQuoting::for_shell(r"C:\Windows\System32\cmd.exe"),
            ShellQuoting::Cmd
        );
        assert_eq!(
            ShellQuoting::for_shell(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ShellQuoting::PowerShell
        );
    }

    #[test]
    fn clean_path_passes_through_unquoted_for_posix() {
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/clean/path")], ShellQuoting::Posix),
            Some("/clean/path".to_string())
        );
    }

    #[test]
    fn path_with_space_is_single_quoted() {
        assert_eq!(
            paths_to_pty_text(
                &[PathBuf::from("/home/user/my file.txt")],
                ShellQuoting::Posix
            ),
            Some("'/home/user/my file.txt'".to_string())
        );
    }

    #[test]
    fn embedded_single_quote_is_escaped() {
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/path/it's/here")], ShellQuoting::Posix),
            Some("'/path/it'\\''s/here'".to_string())
        );
        assert_eq!(
            paths_to_pty_text(
                &[PathBuf::from(r"C:\path\it's\here")],
                ShellQuoting::PowerShell
            ),
            Some("'C:\\path\\it''s\\here'".to_string())
        );
    }

    #[test]
    fn multiple_paths_join_with_space() {
        assert_eq!(
            paths_to_pty_text(
                &[PathBuf::from("/a"), PathBuf::from("/b c")],
                ShellQuoting::Posix
            ),
            Some("/a '/b c'".to_string())
        );
    }

    #[test]
    fn newline_path_is_rejected() {
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/bad\npath")], ShellQuoting::Posix),
            None
        );
    }

    #[test]
    fn carriage_return_path_is_rejected() {
        // A bare CR survives the non-bracketed paste rewrite and submits a
        // line (Enter), so a path like `evil\rrm -rf ~` must be dropped.
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/bad\rpath")], ShellQuoting::Posix),
            None
        );
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("evil\rrm -rf ~")], ShellQuoting::Posix),
            None
        );
    }

    #[test]
    fn empty_after_filter_is_none() {
        assert_eq!(paths_to_pty_text(&[], ShellQuoting::Posix), None);
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/bad\0null")], ShellQuoting::Posix),
            None
        );
    }

    #[test]
    fn shell_metacharacter_path_is_quoted() {
        assert_eq!(
            paths_to_pty_text(&[PathBuf::from("/tmp/a;b")], ShellQuoting::Posix),
            Some("'/tmp/a;b'".to_string())
        );
    }

    #[test]
    fn windows_path_with_spaces_uses_powershell_quotes() {
        assert_eq!(
            paths_to_pty_text(
                &[PathBuf::from(r"C:\dev\my file.txt")],
                ShellQuoting::PowerShell
            ),
            Some("'C:\\dev\\my file.txt'".to_string())
        );
    }

    #[test]
    fn cmd_path_with_spaces_uses_cmd_quotes_and_escapes_expansion() {
        assert_eq!(
            paths_to_pty_text(
                &[PathBuf::from(r"C:\dev\100% done\bang!")],
                ShellQuoting::Cmd
            ),
            Some("\"C:\\dev\\100^% done\\bang^!\"".to_string())
        );
    }
}
