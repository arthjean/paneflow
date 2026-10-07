use std::borrow::Cow;
use std::path::PathBuf;

use gpui::{
    ClipboardEntry, ClipboardItem, Context, ExternalPaths, Focusable, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, TouchPhase,
    Window,
};

use paneflow_terminal_ghostty as ghostty;

use crate::app::diff_dock::code::spawn_blocking_then;
use crate::keys::TerminalKeySequence;
use crate::terminal::types::{
    HyperlinkSource, HyperlinkZone, Modes, Point, SelectionGeometry, SelectionKind, ShellQuoting,
    terminal_metric_to_u16,
};

use super::clipboard_image;
use super::path_picker::wsl;
#[cfg(debug_assertions)]
use super::probe_enabled;
use super::pty_session::{BackendInputResult, SelectionCopy};
use super::{TerminalEvent, TerminalView};

const SCROLLBAR_HIT_SLOP: gpui::Pixels = gpui::px(2.0);
const SMOOTH_SCROLL_MAX_FRACTION: f32 = 1.0 - f32::EPSILON;

fn smooth_scroll_applies(delta: &gpui::ScrollDelta, reduce_motion: bool) -> bool {
    matches!(delta, gpui::ScrollDelta::Pixels(_)) && !reduce_motion
}

fn smooth_scroll_step(remainder: f32, display_offset: usize, history: usize) -> (i32, f32) {
    let room_up = history.saturating_sub(display_offset) as f32;
    let room_down = display_offset as f32;
    let lines = remainder.floor().clamp(-room_down, room_up);
    let at_top = lines >= room_up;
    let fraction = if at_top {
        0.0
    } else {
        (remainder - lines).clamp(0.0, SMOOTH_SCROLL_MAX_FRACTION)
    };
    (lines as i32, fraction)
}

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

struct EnginePointer {
    x: f32,
    y: f32,
    screen_width: u32,
    screen_height: u32,
}

fn engine_pointer(
    offset: (f32, f32),
    measured_cell: (f32, f32),
    grid: (usize, usize),
) -> EnginePointer {
    let (x, screen_width) = engine_axis(offset.0, measured_cell.0, grid.0);
    let (y, screen_height) = engine_axis(offset.1, measured_cell.1, grid.1);
    EnginePointer {
        x,
        y,
        screen_width,
        screen_height,
    }
}

fn engine_axis(offset: f32, measured_cell: f32, cells: usize) -> (f32, u32) {
    let engine_cell = f32::from(terminal_metric_to_u16(measured_cell).max(1));
    let scaled = if measured_cell > 0.0 {
        offset * engine_cell / measured_cell
    } else {
        offset
    };
    let extent = (cells as f32 * engine_cell).clamp(1.0, u32::MAX as f32) as u32;
    (scaled, extent)
}

fn multiline_paste_prompt(lines: usize) -> String {
    if lines == 1 {
        "Paste a line that runs immediately?".to_owned()
    } else {
        format!("Paste {lines} lines?")
    }
}

pub(super) fn selection_too_large_notice(limit: usize) -> String {
    format!(
        "Selection not copied: it is larger than the {} KB copy limit.",
        limit / 1_000
    )
}

fn key_escape_sequence(
    keystroke: &gpui::Keystroke,
    mode: &Modes,
    option_as_meta: bool,
    prefer_character_input: bool,
) -> Option<TerminalKeySequence> {
    let sequence = crate::keys::terminal_key_sequence(keystroke, mode, option_as_meta)?;
    if prefer_character_input && matches!(&sequence, TerminalKeySequence::Protocol(_)) {
        return None;
    }
    Some(sequence)
}

pub(super) use paneflow_ipc_client::send_text::normalize_paste_text;

fn ghostty_modifiers(modifiers: gpui::Modifiers) -> ghostty::Modifiers {
    let mut result = ghostty::Modifiers::empty();
    if modifiers.shift {
        result = result | ghostty::Modifiers::SHIFT;
    }
    if modifiers.control {
        result = result | ghostty::Modifiers::CONTROL;
    }
    if modifiers.alt {
        result = result | ghostty::Modifiers::ALT;
    }
    if modifiers.platform {
        result = result | ghostty::Modifiers::SUPER;
    }
    result
}

fn ghostty_key(key: &str, key_char: Option<&str>) -> Option<ghostty::Key> {
    let named = match key {
        "enter" => Some(ghostty::Key::Enter),
        "tab" => Some(ghostty::Key::Tab),
        "backspace" => Some(ghostty::Key::Backspace),
        "delete" => Some(ghostty::Key::Delete),
        "escape" => Some(ghostty::Key::Escape),
        "up" => Some(ghostty::Key::Up),
        "down" => Some(ghostty::Key::Down),
        "left" => Some(ghostty::Key::Left),
        "right" => Some(ghostty::Key::Right),
        "home" => Some(ghostty::Key::Home),
        "end" => Some(ghostty::Key::End),
        "pageup" => Some(ghostty::Key::PageUp),
        "pagedown" => Some(ghostty::Key::PageDown),
        "insert" => Some(ghostty::Key::Insert),
        "space" => Some(ghostty::Key::Character(' ')),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    if let Some(number) = key.strip_prefix('f').and_then(|value| value.parse().ok())
        && (1..=25).contains(&number)
    {
        return Some(ghostty::Key::Function(number));
    }
    key.chars()
        .next()
        .filter(|_| key.chars().count() == 1)
        .or_else(|| {
            let value = key_char?;
            value.chars().next().filter(|_| value.chars().count() == 1)
        })
        .map(ghostty::Key::Character)
}

fn ghostty_key_input(
    keystroke: &gpui::Keystroke,
    action: ghostty::KeyAction,
) -> Option<ghostty::KeyInput> {
    let key = ghostty_key(&keystroke.key, keystroke.key_char.as_deref())?;
    let unshifted_codepoint = keystroke
        .key
        .chars()
        .next()
        .filter(|_| keystroke.key.chars().count() == 1);
    Some(ghostty::KeyInput {
        key,
        action,
        modifiers: ghostty_modifiers(keystroke.modifiers),
        consumed_modifiers: ghostty::Modifiers::empty(),
        text: String::new(),
        unshifted_codepoint,
        composing: false,
    })
}

pub(super) fn ghostty_text_key_input(
    keystroke: &gpui::Keystroke,
    action: ghostty::KeyAction,
    prefer_character_input: bool,
    text: &str,
) -> ghostty::KeyInput {
    let mut input = ghostty_key_input(keystroke, action).unwrap_or(ghostty::KeyInput {
        key: ghostty::Key::Unidentified,
        action,
        modifiers: ghostty_modifiers(keystroke.modifiers),
        consumed_modifiers: ghostty::Modifiers::empty(),
        text: String::new(),
        unshifted_codepoint: None,
        composing: false,
    });
    let mut consumed = ghostty::Modifiers::empty();
    if keystroke.modifiers.shift {
        consumed = consumed | ghostty::Modifiers::SHIFT;
    }
    if prefer_character_input && keystroke.modifiers.control && keystroke.modifiers.alt {
        consumed = consumed | ghostty::Modifiers::CONTROL | ghostty::Modifiers::ALT;
    }
    input.consumed_modifiers = consumed;
    input.text = text.to_owned();
    input
}

fn ghostty_release_id(keystroke: &gpui::Keystroke) -> String {
    keystroke.key.clone()
}

#[derive(Clone, Copy)]
enum ReportedMouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy)]
enum ReportedMouseButton {
    Left,
    Middle,
    Right,
    WheelUp,
    WheelDown,
}

struct ReportedMouseInput {
    position: gpui::Point<gpui::Pixels>,
    action: ReportedMouseAction,
    reported_button: Option<ReportedMouseButton>,
    modifiers: gpui::Modifiers,
    any_button_pressed: bool,
    repeat: usize,
}

impl ReportedMouseButton {
    fn from_gpui(button: MouseButton) -> Option<Self> {
        match button {
            MouseButton::Left => Some(Self::Left),
            MouseButton::Middle => Some(Self::Middle),
            MouseButton::Right => Some(Self::Right),
            MouseButton::Navigate(_) => None,
        }
    }
}

fn paths_to_pty_text(paths: &[std::path::PathBuf], shell_quoting: ShellQuoting) -> Option<String> {
    let quoted: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let s = p.to_string_lossy();
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

pub(super) fn quote_path_for_shell(path: &str, shell_quoting: ShellQuoting) -> String {
    match shell_quoting {
        ShellQuoting::Posix | ShellQuoting::Wsl => quote_posix_path(path),
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
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.key == "escape"
            && crate::app::workspace_ops::swap_mode_active_in(window.window_handle().window_id())
        {
            cx.emit(TerminalEvent::CancelSwapMode);
            return;
        }

        if self.search_active {
            return;
        }

        if !self.terminal.host_link.accepts_input() {
            if event.keystroke.key == "enter" && !self.copy_mode_active {
                self.resume_hosted_session(cx);
            }
            return;
        }

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
                    if keystroke.key_char.as_deref() == Some("q")
                        && !keystroke.modifiers.control
                        && !keystroke.modifiers.alt
                    {
                        self.exit_copy_mode(false, cx);
                    }
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

        if !self.cursor_visible {
            self.cursor_visible = true;
            cx.notify();
        }

        let keystroke = &event.keystroke;

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
                self.scroll_remainder = 0.0;
                cx.notify();
                return;
            }
        }

        let mode = self.terminal.session_backend().modes();

        self.ghostty_pending_text_key = None;

        if let Some(mapped_sequence) = key_escape_sequence(
            keystroke,
            &mode,
            self.option_as_meta,
            event.prefer_character_input,
        ) {
            let (seq, encode_with_backend) = match mapped_sequence {
                TerminalKeySequence::Protocol(seq) => (seq, true),
                TerminalKeySequence::Literal(seq) => (seq, false),
            };
            {
                let backend = self.terminal.session_backend();
                if backend.grid_metrics().display_offset > 0 {
                    backend.scroll_to_bottom();
                    self.terminal.dirty = true;
                }
                self.scroll_remainder = 0.0;
            }
            let backend_key = if encode_with_backend {
                ghostty_key_input(
                    keystroke,
                    if event.is_held {
                        ghostty::KeyAction::Repeat
                    } else {
                        ghostty::KeyAction::Press
                    },
                )
            } else {
                None
            };
            match backend_key {
                Some(input) => {
                    let mut release = input.clone();
                    release.action = ghostty::KeyAction::Release;
                    release.text.clear();
                    release.composing = false;
                    if self.terminal.write_ghostty_key(input) == BackendInputResult::Accepted {
                        self.ghostty_pressed_keys
                            .insert(ghostty_release_id(keystroke), release);
                    }
                }
                None => match seq {
                    Cow::Borrowed(s) => {
                        self.terminal.write_to_pty(Cow::Borrowed(s.as_bytes()));
                    }
                    Cow::Owned(s) => {
                        self.terminal.write_to_pty(s.into_bytes());
                    }
                },
            }
        } else {
            if ghostty_key(&keystroke.key, keystroke.key_char.as_deref()).is_some() {
                self.ghostty_pending_text_key = Some((
                    keystroke.clone(),
                    if event.is_held {
                        ghostty::KeyAction::Repeat
                    } else {
                        ghostty::KeyAction::Press
                    },
                    event.prefer_character_input,
                ));
            }
        }

        #[cfg(debug_assertions)]
        if let Some(start) = _probe_start {
            let elapsed = start.elapsed();
            self.terminal.last_keystroke_at = Some(start);
            if elapsed.as_millis() > 1 {
                log::warn!(
                    "[latency] input-handler: {:.2}ms",
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }

    pub(super) fn handle_key_up(
        &mut self,
        event: &KeyUpEvent,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if self.search_active || self.copy_mode_active {
            return;
        }
        let release_id = ghostty_release_id(&event.keystroke);
        if let Some(input) = self.ghostty_pressed_keys.remove(&release_id)
            && self.terminal.write_ghostty_key(input) == BackendInputResult::Rejected
        {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty rejected a key release"
            );
        }
    }

    pub(super) fn release_ghostty_pressed_keys(&mut self) {
        self.ghostty_pending_text_key = None;
        for (_, input) in std::mem::take(&mut self.ghostty_pressed_keys) {
            if self.terminal.write_ghostty_key(input) == BackendInputResult::Rejected {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty rejected a key release during focus loss"
                );
            }
        }
    }

    pub(super) fn pixel_to_grid(&self, pos: gpui::Point<gpui::Pixels>) -> Point {
        self.selection_geometry().cell_at(self.pane_relative(pos))
    }

    fn pane_relative(&self, pos: gpui::Point<gpui::Pixels>) -> (f32, f32) {
        let origin = *self
            .element_origin
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        (f32::from(pos.x - origin.x), f32::from(pos.y - origin.y))
    }

    fn grid_cell_at(&self, pos: gpui::Point<gpui::Pixels>) -> Option<Point> {
        let geometry = self.selection_geometry();
        let position = self.pane_relative(pos);
        let inside = position.0 >= 0.0
            && position.1 >= 0.0
            && position.0 < geometry.cell_width * geometry.columns as f32
            && position.1 < geometry.height();
        inside.then(|| geometry.cell_at(position))
    }

    fn selection_geometry(&self) -> SelectionGeometry {
        self.terminal
            .session_backend()
            .selection_geometry(f32::from(self.cell_width), f32::from(self.line_height))
    }

    fn write_mouse_report(&self, report: ReportedMouseInput) {
        let ReportedMouseInput {
            position,
            action,
            reported_button,
            modifiers,
            any_button_pressed,
            repeat,
        } = report;
        {
            let origin = *self
                .element_origin
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let metrics = self.terminal.session_backend().grid_metrics();
            let pointer = engine_pointer(
                (
                    (position.x - origin.x).max(gpui::px(0.0)).as_f32(),
                    (position.y - origin.y).max(gpui::px(0.0)).as_f32(),
                ),
                (self.cell_width.as_f32(), self.line_height.as_f32()),
                (metrics.columns, metrics.screen_lines),
            );
            let input = ghostty::MouseInput {
                action: match action {
                    ReportedMouseAction::Press => ghostty::MouseAction::Press,
                    ReportedMouseAction::Release => ghostty::MouseAction::Release,
                    ReportedMouseAction::Motion => ghostty::MouseAction::Motion,
                },
                button: reported_button.map(|button| match button {
                    ReportedMouseButton::Left => ghostty::MouseButton::Left,
                    ReportedMouseButton::Middle => ghostty::MouseButton::Middle,
                    ReportedMouseButton::Right => ghostty::MouseButton::Right,
                    ReportedMouseButton::WheelUp => ghostty::MouseButton::Four,
                    ReportedMouseButton::WheelDown => ghostty::MouseButton::Five,
                }),
                modifiers: ghostty_modifiers(modifiers),
                x: pointer.x,
                y: pointer.y,
                screen_width: pointer.screen_width,
                screen_height: pointer.screen_height,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
                any_button_pressed,
            };
            self.terminal.write_ghostty_mouse(input, repeat);
        }
    }

    fn scrollbar_hit(
        &self,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<super::element::ScrollbarMetrics> {
        if !self.scrollbar_enabled || !(self.scrollbar_visible || self.scrollbar_reveal.is_pinned())
        {
            return None;
        }
        let metrics = {
            *self
                .scrollbar_metrics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
        }?;
        metrics
            .track_contains(position, SCROLLBAR_HIT_SLOP)
            .then_some(metrics)
    }

    pub(super) fn scrollbar_gutter_contains(&self, position: gpui::Point<gpui::Pixels>) -> bool {
        if !self.scrollbar_enabled {
            return false;
        }
        let metrics = {
            *self
                .scrollbar_metrics
                .lock()
                .unwrap_or_else(|p| p.into_inner())
        };
        metrics.is_some_and(|metrics| metrics.track_contains(position, SCROLLBAR_HIT_SLOP))
    }

    pub(super) fn scrollbar_set_hovered(&mut self, hovered: bool) -> bool {
        self.scrollbar_reveal
            .set_hovered(hovered, std::time::Instant::now())
    }

    pub(super) fn scrollbar_presence(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> super::scrollbar_reveal::ScrollbarPresence {
        if !self.scrollbar_enabled {
            return super::scrollbar_reveal::ScrollbarPresence::HIDDEN;
        }
        let now = std::time::Instant::now();
        let offset = self
            .terminal
            .session_backend()
            .grid_metrics()
            .display_offset;
        if offset != self.scrollbar_seen_offset {
            self.scrollbar_seen_offset = offset;
            self.scrollbar_reveal.touch(now);
        }
        let presence = self
            .scrollbar_reveal
            .presence(now, crate::ui_primitives::reduce_motion());
        self.scrollbar_visible = presence.is_visible();
        if presence.animating {
            window.request_animation_frame();
        }
        if let Some(delay) = presence.hide_after
            && !self.scrollbar_hide_scheduled
        {
            self.scrollbar_hide_scheduled = true;
            cx.spawn(
                async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |view, cx| {
                        view.scrollbar_hide_scheduled = false;
                        cx.notify();
                    });
                },
            )
            .detach();
        }
        presence
    }

    fn apply_scrollbar_jump(&mut self, target_offset: usize, history_size: usize) -> bool {
        let row = history_size.saturating_sub(target_offset.min(history_size));
        if self.terminal.session_backend().scroll_to_viewport_row(row) {
            self.terminal.dirty = true;
            true
        } else {
            false
        }
    }

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

        if event.button == MouseButton::Left
            && let Some(metrics) = self.scrollbar_hit(event.position)
        {
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
            self.scrollbar_reveal
                .set_dragging(true, std::time::Instant::now());
            cx.notify();
            return;
        }

        if event.button == MouseButton::Left
            && open_link_modifier_held(&event.modifiers)
            && event.click_count == 1
            && self.ctrl_hovered_link.is_some()
        {
            self.mouse_down_link = self.ctrl_hovered_link.clone();
            let cell = self.pixel_to_grid(event.position);
            self.mouse_down_cell = Some(cell);
            self.terminal.session_backend().press_selection(
                SelectionKind::Simple,
                cell,
                self.pane_relative(event.position),
            );
            self.selecting = true;
            cx.notify();
            return;
        }

        let mode = self.terminal.session_backend().modes();

        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            if let Some(reported_button) = ReportedMouseButton::from_gpui(event.button) {
                self.write_mouse_report(ReportedMouseInput {
                    position: event.position,
                    action: ReportedMouseAction::Press,
                    reported_button: Some(reported_button),
                    modifiers: event.modifiers,
                    any_button_pressed: true,
                    repeat: 1,
                });
            }
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }

        let selection_type = match event.click_count {
            1 => SelectionKind::Simple,
            2 => SelectionKind::Semantic,
            3 => SelectionKind::Lines,
            _ => return,
        };

        self.terminal.session_backend().press_selection(
            selection_type,
            self.pixel_to_grid(event.position),
            self.pane_relative(event.position),
        );

        self.selecting = true;
        cx.notify();
    }

    pub(super) fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let over_scrollbar = self.scrollbar_gutter_contains(event.position);
        if self.scrollbar_set_hovered(over_scrollbar) {
            cx.notify();
        }
        if let Some(mut drag) = self.scrollbar_drag {
            if event.pressed_button == Some(MouseButton::Left) {
                let target = Self::scrollbar_drag_target(drag, event.position.y);
                let step = target as i64 - drag.last_target as i64;
                if target != drag.last_target && self.apply_scrollbar_drag_delta(step) {
                    drag.last_target = target;
                    self.scrollbar_drag = Some(drag);
                    cx.notify();
                }
            } else {
                self.scrollbar_drag = None;
                self.scrollbar_reveal
                    .set_dragging(false, std::time::Instant::now());
                cx.notify();
            }
            return;
        }

        if over_scrollbar && !self.selecting {
            self.hovered_cell = None;
            if self.ctrl_hovered_link.take().is_some() {
                cx.notify();
            }
            return;
        }

        let mode = self.terminal.session_backend().modes();

        if !event.modifiers.shift
            && (mode.contains(Modes::MOUSE_MOTION)
                || (mode.contains(Modes::MOUSE_DRAG) && event.pressed_button.is_some()))
        {
            let reported_button = match event.pressed_button {
                Some(button) => match ReportedMouseButton::from_gpui(button) {
                    Some(reported) => Some(reported),
                    None => return,
                },
                None => None,
            };
            self.write_mouse_report(ReportedMouseInput {
                position: event.position,
                action: ReportedMouseAction::Motion,
                reported_button,
                modifiers: event.modifiers,
                any_button_pressed: event.pressed_button.is_some(),
                repeat: 1,
            });
            return;
        }

        let hover_point = self.pixel_to_grid(event.position);
        let prev_hovered_cell = self.hovered_cell;
        self.hovered_cell = Some(hover_point);

        self.link_modifier_held = open_link_modifier_held(&event.modifiers);
        if self.link_modifier_held {
            let hovered_cell_changed = prev_hovered_cell != Some(hover_point);
            if !hovered_cell_changed {
                return;
            }

            self.refresh_hovered_link(hover_point, cx);
        } else if self.ctrl_hovered_link.is_some() {
            self.ctrl_hovered_link = None;
            cx.notify();
        }

        if !self.selecting {
            return;
        }

        let geometry = self.selection_geometry();
        let position = self.pane_relative(event.position);
        let cell = geometry.cell_at(position);
        if self.mouse_down_cell.is_some_and(|down| down != cell) {
            self.mouse_down_link = None;
        }
        self.terminal.session_backend().drag_selection(
            cell,
            position,
            geometry,
            event.modifiers.alt,
        );

        cx.notify();
    }

    fn refresh_hovered_link(&mut self, hover_point: Point, cx: &mut Context<Self>) {
        self.terminal
            .session_backend()
            .request_osc8_hyperlink_at(hover_point);
        self.resolve_links_at_hover(hover_point, cx);
        cx.notify();
    }

    pub(super) fn apply_resolved_hover_link(
        &mut self,
        point: Point,
        link: Option<HyperlinkZone>,
        cx: &mut Context<Self>,
    ) {
        let Some(link) = link else {
            return;
        };
        if !self.link_modifier_held || self.hovered_cell != Some(point) {
            return;
        }
        self.ctrl_hovered_link = Some(link);
        cx.notify();
    }

    pub(super) fn handle_modifiers_changed(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.link_modifier_held = open_link_modifier_held(&event.modifiers);
        if self.link_modifier_held {
            if let Some(point) = self.hovered_cell {
                self.refresh_hovered_link(point, cx);
            }
        } else if self.ctrl_hovered_link.is_some() {
            self.ctrl_hovered_link = None;
            cx.notify();
        }
    }

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
                let uri = link.uri.clone();
                cx.spawn(async move |this, cx| {
                    let Err(err) = crate::external_open::open_url_off_thread(uri).await else {
                        return;
                    };
                    log::warn!("terminal: open URL failed: {err}");
                    let _ = this.update(cx, |_, cx| {
                        cx.emit(TerminalEvent::Notice(
                            crate::external_open::open_url_failure_message(&err),
                        ));
                    });
                })
                .detach();
            }
        }
    }

    pub(super) fn handle_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.scrollbar_drag
            && event.button == MouseButton::Left
        {
            let target = Self::scrollbar_drag_target(drag, event.position.y);
            let step = target as i64 - drag.last_target as i64;
            if target != drag.last_target && self.apply_scrollbar_drag_delta(step) {
                cx.notify();
            }
            self.scrollbar_drag = None;
            self.scrollbar_set_hovered(self.scrollbar_gutter_contains(event.position));
            self.scrollbar_reveal
                .set_dragging(false, std::time::Instant::now());
            cx.notify();
            return;
        }

        let mode = self.terminal.session_backend().modes();

        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            self.mouse_down_link = None;
            self.mouse_down_cell = None;
            if let Some(reported_button) = ReportedMouseButton::from_gpui(event.button) {
                self.write_mouse_report(ReportedMouseInput {
                    position: event.position,
                    action: ReportedMouseAction::Release,
                    reported_button: Some(reported_button),
                    modifiers: event.modifiers,
                    any_button_pressed: false,
                    repeat: 1,
                });
            }
            return;
        }

        if event.button == MouseButton::Middle {
            #[cfg(target_os = "linux")]
            {
                if let Some(item) = cx.read_from_primary()
                    && let Some(text) = item.text()
                {
                    self.paste_with_confirmation(
                        &text,
                        ghostty::ClipboardLocation::Primary,
                        window,
                        cx,
                    );
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = window;
            return;
        }

        if event.button != MouseButton::Left {
            return;
        }
        self.selecting = false;
        let down_link = self.mouse_down_link.take();
        self.mouse_down_cell = None;
        let backend = self.terminal.session_backend();
        backend.release_selection(self.grid_cell_at(event.position));

        if let Some(link) = down_link
            && link.is_openable
        {
            backend.clear_selection();
            self.open_hyperlink(&link, cx);
            cx.notify();
            return;
        }

        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let copy = cx
                    .background_executor()
                    .spawn(async move { backend.take_selection_for_copy() })
                    .await;
                let _ = this.update(cx, |_view, cx| {
                    match copy {
                        SelectionCopy::Empty => {}
                        SelectionCopy::Text(text) => {
                            #[cfg(target_os = "linux")]
                            cx.write_to_primary(ClipboardItem::new_string(text.clone()));
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                            cx.emit(TerminalEvent::SelectionCopied);
                        }
                        SelectionCopy::TooLarge { limit } => {
                            cx.emit(TerminalEvent::Notice(selection_too_large_notice(limit)));
                        }
                    }
                    cx.notify();
                });
            },
        )
        .detach();
        cx.notify();
    }

    pub(super) fn handle_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle(cx).is_focused(window) {
            if self.search_field_focused(window, cx) {
                self.search_input
                    .update(cx, |input, cx| input.copy_selection(cx));
            }
            return;
        }
        if let Some(text) = self.terminal.session_backend().selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_select_all(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let backend = self.terminal.session_backend();
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let text = smol::unblock(move || backend.select_all_text()).await;
                let _ = this.update(cx, |view, cx| {
                    if let Some(text) = text.filter(|text| !text.is_empty()) {
                        #[cfg(target_os = "linux")]
                        cx.write_to_primary(ClipboardItem::new_string(text.clone()));
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                        cx.emit(TerminalEvent::SelectionCopied);
                    }
                    view.terminal.dirty = true;
                    cx.notify();
                });
            },
        )
        .detach();
    }

    fn search_field_focused(&self, window: &Window, cx: &gpui::App) -> bool {
        self.search_active && self.search_input.read(cx).focus_handle.is_focused(window)
    }

    pub(super) fn handle_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.focus_handle(cx).is_focused(window) {
            if self.search_field_focused(window, cx) {
                self.search_input
                    .update(cx, |input, cx| input.paste_clipboard(window, cx));
            }
            return;
        }
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

        for entry in clipboard.entries() {
            if let ClipboardEntry::ExternalPaths(ext_paths) = entry
                && let Some(text) =
                    paths_to_pty_text(ext_paths.paths(), self.terminal.shell_quoting)
            {
                self.write_paste_text(&text);
                return;
            }
        }

        if let Some(text) = clipboard.text() {
            self.paste_with_confirmation(&text, ghostty::ClipboardLocation::Standard, window, cx);
            return;
        }

        if let Some(image) = clipboard.into_entries().find_map(|entry| match entry {
            ClipboardEntry::Image(image) if !image.bytes.is_empty() => Some(image),
            _ => None,
        }) {
            self.paste_clipboard_image(image, cx);
        }
    }

    fn paste_clipboard_image(&self, image: gpui::Image, cx: &mut Context<Self>) {
        let Some(dir) = self.clipboard_image_dir.clone() else {
            log::warn!("clipboard image paste: no Paneflow home to store the image in");
            return;
        };
        let quoting = self.terminal.shell_quoting;
        spawn_blocking_then(
            cx,
            move || -> std::io::Result<PathBuf> {
                let path = clipboard_image::materialize(&image, &dir)?;
                Ok(match quoting {
                    ShellQuoting::Wsl => wsl::roots()
                        .and_then(|roots| roots.to_linux(&path))
                        .map_or(path, PathBuf::from),
                    _ => path,
                })
            },
            move |view: &mut Self, stored, _cx| match stored {
                Ok(path) => {
                    if let Some(text) = paths_to_pty_text(&[path], quoting) {
                        view.write_paste_text(&text);
                    }
                }
                Err(error) => log::warn!("clipboard image paste: {error}"),
            },
        );
    }

    pub(super) fn handle_file_drop(
        &mut self,
        paths: &ExternalPaths,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        if let Some(text) = paths_to_pty_text(paths.paths(), self.terminal.shell_quoting) {
            self.write_paste_text(&text);
        }
    }

    pub(super) fn write_paste_text(&self, text: &str) {
        self.write_paste_text_from(text, ghostty::ClipboardLocation::Standard);
    }

    fn write_paste_text_from(&self, text: &str, location: ghostty::ClipboardLocation) {
        self.terminal
            .write_ghostty_paste(normalize_paste_text(text), false, location);
    }

    fn paste_with_confirmation(
        &mut self,
        text: &str,
        location: ghostty::ClipboardLocation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = normalize_paste_text(text);
        let bracketed = self
            .terminal
            .session_backend()
            .modes()
            .contains(Modes::BRACKETED_PASTE);
        if bracketed || !text.contains('\n') {
            self.terminal.write_ghostty_paste(text, false, location);
            return;
        }
        let lines = text.lines().count();
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            &multiline_paste_prompt(lines),
            Some(
                "This program did not enable bracketed paste, so each line runs as soon as it arrives.",
            ),
            &[
                gpui::PromptButton::Ok("Paste".into()),
                gpui::PromptButton::Cancel("Cancel".into()),
            ],
            cx,
        );
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                if answer.await != Ok(0) {
                    return;
                }
                let _ = this.update(cx, |view, _cx| {
                    view.terminal.write_ghostty_paste(text, true, location);
                });
            },
        )
        .detach();
    }

    pub fn inject_text(&self, text: &str) {
        let _ = self.write_injected_text(text);
    }

    pub(crate) fn write_injected_text(&self, text: &str) -> Result<(), &'static str> {
        let mode = self.terminal.session_backend().modes();
        if mode.contains(Modes::BRACKETED_PASTE) {
            super::view::input_outcome(self.terminal.write_ghostty_paste(
                normalize_paste_text(text),
                false,
                ghostty::ClipboardLocation::Standard,
            ))
        } else {
            self.write_text(text)
        }
    }

    pub(super) fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mode = self.terminal.session_backend().modes();

        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            let delta_y = event.delta.pixel_delta(self.line_height).y;
            self.scroll_remainder += delta_y / self.line_height;
            self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);
            let lines = self.scroll_remainder as i32;
            if lines == 0 {
                return;
            }
            self.scroll_remainder -= lines as f32;

            let count = lines.unsigned_abs() as usize;
            self.write_mouse_report(ReportedMouseInput {
                position: event.position,
                action: ReportedMouseAction::Press,
                reported_button: Some(if lines > 0 {
                    ReportedMouseButton::WheelUp
                } else {
                    ReportedMouseButton::WheelDown
                }),
                modifiers: event.modifiers,
                any_button_pressed: false,
                repeat: count,
            });
            return;
        }

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

        let precise = smooth_scroll_applies(&event.delta, crate::ui_primitives::reduce_motion());
        match event.touch_phase {
            TouchPhase::Started => {
                if !precise {
                    self.scroll_remainder = 0.0;
                }
                return;
            }
            TouchPhase::Ended | TouchPhase::Cancelled => return,
            TouchPhase::Moved => {}
        }

        let delta_y = event.delta.pixel_delta(self.line_height).y;
        self.scroll_remainder += (delta_y / self.line_height) * self.scroll_multiplier;

        self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);

        if precise {
            self.scroll_smoothly(cx);
            return;
        }
        if std::mem::take(&mut self.smooth_scroll) {
            cx.notify();
        }

        let lines = self.scroll_remainder as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;

        if !self.terminal.session_backend().scroll_delta(lines) {
            return;
        }
        self.terminal.dirty = true;
        self.scrollbar_reveal.touch(std::time::Instant::now());

        cx.notify();
    }

    fn scroll_smoothly(&mut self, cx: &mut Context<Self>) {
        let backend = self.terminal.session_backend();
        backend.enable_smooth_scroll_overscan();
        let metrics = backend.grid_metrics();
        let history = usize::try_from(-i64::from(metrics.topmost_line.0)).unwrap_or(0);
        let (lines, fraction) =
            smooth_scroll_step(self.scroll_remainder, metrics.display_offset, history);
        let moved = lines != 0 || fraction != self.scroll_remainder || !self.smooth_scroll;
        self.smooth_scroll = true;
        self.scroll_remainder = fraction;
        if lines != 0 && backend.scroll_delta(lines) {
            self.terminal.dirty = true;
            self.scrollbar_reveal.touch(std::time::Instant::now());
        }
        if moved {
            cx.notify();
        }
    }

    pub(super) fn smooth_scroll_offset(&self) -> gpui::Pixels {
        if self.smooth_scroll {
            self.line_height * self.scroll_remainder.clamp(0.0, SMOOTH_SCROLL_MAX_FRACTION)
        } else {
            gpui::px(0.0)
        }
    }

    pub(super) fn handle_scroll_page_up(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
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
    use super::{
        engine_pointer, normalize_paste_text, paths_to_pty_text, smooth_scroll_applies,
        smooth_scroll_step,
    };
    use crate::terminal::types::{Modes, ShellQuoting};
    use std::path::PathBuf;

    #[test]
    fn only_precise_deltas_without_reduce_motion_scroll_by_the_pixel() {
        let pixels = gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(3.0)));
        let lines = gpui::ScrollDelta::Lines(gpui::point(0.0, 1.0));
        assert!(smooth_scroll_applies(&pixels, false));
        assert!(!smooth_scroll_applies(&pixels, true));
        assert!(!smooth_scroll_applies(&lines, false));
        assert!(!smooth_scroll_applies(&lines, true));
    }

    #[test]
    fn a_smooth_scroll_keeps_a_sub_line_fraction_and_crosses_whole_lines() {
        assert_eq!(smooth_scroll_step(0.25, 10, 100), (0, 0.25));
        assert_eq!(smooth_scroll_step(1.5, 10, 100), (1, 0.5));
        assert_eq!(smooth_scroll_step(-0.25, 10, 100), (-1, 0.75));
        assert_eq!(smooth_scroll_step(-2.0, 10, 100), (-2, 0.0));
        let (_, fraction) = smooth_scroll_step(0.999_999_9, 10, 100);
        assert!(fraction < 1.0);
    }

    #[test]
    fn a_smooth_scroll_never_shifts_past_either_end_of_the_scrollback() {
        assert_eq!(
            smooth_scroll_step(-0.25, 0, 100),
            (0, 0.0),
            "at the bottom, scrolling down must not bounce into the void"
        );
        assert_eq!(smooth_scroll_step(-3.5, 2, 100), (-2, 0.0));
        assert_eq!(
            smooth_scroll_step(0.5, 100, 100),
            (0, 0.0),
            "nothing exists above the first line of scrollback"
        );
        assert_eq!(smooth_scroll_step(5.5, 98, 100), (2, 0.0));
        assert_eq!(smooth_scroll_step(0.5, 0, 0), (0, 0.0));
    }

    #[test]
    fn a_click_centered_on_column_80_with_fractional_cells_reports_column_80() {
        use paneflow_terminal_ghostty as ghostty;

        let measured = (8.5_f32, 17.0_f32);
        let mut terminal = ghostty::DisplayTerminal::new(
            ghostty::WindowSize::new(
                80,
                24,
                u32::from(crate::terminal::types::terminal_metric_to_u16(measured.0)),
                u32::from(crate::terminal::types::terminal_metric_to_u16(measured.1)),
            )
            .unwrap(),
            1_000,
            ghostty::TerminalAppearance::default(),
        )
        .unwrap();
        terminal.feed(b"\x1b[?1000h\x1b[?1006h").unwrap();

        let pointer = engine_pointer((79.5 * measured.0, 0.5 * measured.1), measured, (80, 24));
        let report = terminal
            .encode_mouse(ghostty::MouseInput {
                action: ghostty::MouseAction::Press,
                button: Some(ghostty::MouseButton::Left),
                modifiers: ghostty::Modifiers::empty(),
                x: pointer.x,
                y: pointer.y,
                screen_width: pointer.screen_width,
                screen_height: pointer.screen_height,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
                any_button_pressed: true,
            })
            .unwrap();

        assert_eq!(report, b"\x1b[<0;80;1M");
    }

    #[test]
    fn printable_altgr_commit_preserves_key_metadata_and_consumes_ctrl_alt() {
        let keystroke = gpui::Keystroke::parse("ctrl-alt-0").unwrap();
        let input = super::ghostty_text_key_input(
            &keystroke,
            paneflow_terminal_ghostty::KeyAction::Press,
            true,
            "@",
        );

        assert_eq!(input.key, paneflow_terminal_ghostty::Key::Character('0'));
        assert!(input.modifiers.contains(
            paneflow_terminal_ghostty::Modifiers::CONTROL
                | paneflow_terminal_ghostty::Modifiers::ALT
        ));
        assert!(input.consumed_modifiers.contains(
            paneflow_terminal_ghostty::Modifiers::CONTROL
                | paneflow_terminal_ghostty::Modifiers::ALT
        ));
        assert_eq!(input.text, "@");
    }

    #[test]
    fn character_preferred_altgr_bypasses_control_escape_routing() {
        let keystroke = gpui::Keystroke::parse("ctrl-alt-q").unwrap();
        assert_eq!(
            crate::keys::to_esc_str(&keystroke, &Modes::empty(), false).as_deref(),
            Some("\x11"),
            "without the character-input signal, Ctrl+Q maps to DC1"
        );
        assert!(
            super::key_escape_sequence(&keystroke, &Modes::empty(), false, true).is_none(),
            "AltGr character input must wait for the text commit"
        );
    }

    #[test]
    fn character_preference_keeps_literal_shift_enter_routing() {
        let keystroke = gpui::Keystroke::parse("shift-enter").unwrap();
        let Some(crate::keys::TerminalKeySequence::Literal(sequence)) =
            super::key_escape_sequence(&keystroke, &Modes::empty(), false, true)
        else {
            panic!("Shift+Enter must bypass backend key encoding");
        };
        let expected = if cfg!(target_os = "windows") {
            "\x1b\r"
        } else {
            "\n"
        };
        assert_eq!(sequence.as_ref(), expected);
    }

    #[test]
    fn paste_text_is_normalized_to_lf_before_the_engine_frames_it() {
        assert_eq!(normalize_paste_text("hello world"), "hello world");
        assert_eq!(
            normalize_paste_text("line one\r\nline two\rline three\nline four"),
            "line one\nline two\nline three\nline four"
        );
    }

    #[test]
    fn paste_text_drops_esc_and_c1_so_a_payload_cannot_close_the_bracket() {
        assert_eq!(normalize_paste_text("a\x1b[201~b\u{0085}c"), "a[201~bc");
    }

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
        assert_eq!(
            ShellQuoting::for_shell(r"C:\Windows\System32\wsl.exe"),
            ShellQuoting::Wsl
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
