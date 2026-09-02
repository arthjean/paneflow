use std::borrow::Cow;

use gpui::{
    ClipboardEntry, ClipboardItem, Context, ExternalPaths, Focusable, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, TouchPhase,
    Window,
};

use paneflow_terminal_ghostty as ghostty;

use crate::keys::TerminalKeySequence;
use crate::terminal::types::{
    HyperlinkSource, HyperlinkZone, Modes, Point, SelectionGeometry, SelectionKind, ShellQuoting,
};

#[cfg(debug_assertions)]
use super::probe_enabled;
use super::pty_session::BackendInputResult;
use super::{TerminalEvent, TerminalView};

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

pub(super) fn sanitize_bracketed_paste(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .chars()
        .filter(|&c| c != '\x1b' && !(('\u{0080}'..='\u{009f}').contains(&c)))
        .collect()
}

#[cfg(test)]
pub(super) fn wrap_bracketed_paste(text: &str) -> String {
    format!("\x1b[200~{}\x1b[201~", sanitize_bracketed_paste(text))
}

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
        if crate::SWAP_MODE.load(std::sync::atomic::Ordering::Relaxed)
            && event.keystroke.key == "escape"
        {
            cx.emit(TerminalEvent::CancelSwapMode);
            return;
        }

        if self.search_active {
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

        self.cursor_visible = true;

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
                    self.scroll_remainder = 0.0;
                }
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
                    "[latency] keystroke→PTY: {:.2}ms",
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
            let screen_width = (metrics.columns as f32 * self.cell_width.as_f32())
                .max(1.0)
                .min(u32::MAX as f32) as u32;
            let screen_height = (metrics.screen_lines as f32 * self.line_height.as_f32())
                .max(1.0)
                .min(u32::MAX as f32) as u32;
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
                x: (position.x - origin.x).max(gpui::px(0.0)).as_f32(),
                y: (position.y - origin.y).max(gpui::px(0.0)).as_f32(),
                screen_width,
                screen_height,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
                any_button_pressed,
            };
            self.terminal.write_ghostty_mouse(input, repeat);
        }
    }

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
            && let Some(metrics) = self.scrollbar_hit(event.position.x)
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
            cx.notify();
            return;
        }

        if event.button == MouseButton::Left
            && open_link_modifier_held(&event.modifiers)
            && event.click_count == 1
            && self.ctrl_hovered_link.is_some()
        {
            self.mouse_down_link = self.ctrl_hovered_link.clone();
            self.terminal.session_backend().press_selection(
                SelectionKind::Simple,
                self.pixel_to_grid(event.position),
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
        self.terminal.session_backend().drag_selection(
            geometry.cell_at(position),
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
        let in_zone = |z: &HyperlinkZone| {
            hover_point.line == z.start.line
                && hover_point.column >= z.start.column
                && hover_point.column <= z.end.column
        };
        self.ctrl_hovered_link = self
            .detect_links_at_hover()
            .into_iter()
            .find(|z| in_zone(z));
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

        if mode.intersects(Modes::MOUSE_MODE) && !event.modifiers.shift {
            self.mouse_down_link = None;
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

        if event.button != MouseButton::Left {
            return;
        }
        self.selecting = false;
        let down_link = self.mouse_down_link.take();

        self.terminal
            .session_backend()
            .release_selection(self.grid_cell_at(event.position));
        let (selection_empty, copied) = self.terminal.session_backend().finish_selection();

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

    pub(super) fn handle_copy(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.terminal.session_backend().selection_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(super) fn handle_paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(clipboard) = cx.read_from_clipboard() else {
            return;
        };

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

        if let Some(text) = clipboard.text() {
            let mode = self.terminal.session_backend().modes();
            self.write_paste_text(&text, mode);
            return;
        }

        if clipboard
            .entries()
            .iter()
            .any(|entry| matches!(entry, ClipboardEntry::Image(image) if !image.bytes.is_empty()))
        {
            self.terminal.write_to_pty(vec![0x16]);
        }
    }

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
        let payload = if mode.contains(Modes::BRACKETED_PASTE) {
            sanitize_bracketed_paste(text)
        } else {
            text.replace("\r\n", "\r").replace('\n', "\r")
        };
        self.terminal.write_ghostty_paste(payload);
    }

    pub fn inject_text(&self, text: &str) {
        let mode = self.terminal.session_backend().modes();
        if mode.contains(Modes::BRACKETED_PASTE) {
            self.write_paste_text(text, mode);
        } else {
            self.send_text(text);
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

        match event.touch_phase {
            TouchPhase::Started => {
                self.scroll_remainder = 0.0;
                return;
            }
            TouchPhase::Ended | TouchPhase::Cancelled => return,
            TouchPhase::Moved => {}
        }

        let delta_y = event.delta.pixel_delta(self.line_height).y;
        self.scroll_remainder += (delta_y / self.line_height) * self.scroll_multiplier;

        self.scroll_remainder = self.scroll_remainder.clamp(-500.0, 500.0);

        let lines = self.scroll_remainder as i32;
        if lines == 0 {
            return;
        }
        self.scroll_remainder -= lines as f32;

        if !self.terminal.session_backend().scroll_delta(lines) {
            return;
        }
        self.terminal.dirty = true;

        cx.notify();
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
    use super::{paths_to_pty_text, wrap_bracketed_paste};
    use crate::terminal::types::{Modes, ShellQuoting};
    use std::path::PathBuf;

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
    fn bracketed_wrap_has_both_sentinels_and_no_cr() {
        let wrapped = wrap_bracketed_paste("hello world");
        assert!(wrapped.starts_with("\x1b[200~"), "opens with paste-start");
        assert!(wrapped.ends_with("\x1b[201~"), "closes with paste-end");
        assert_eq!(wrapped, "\x1b[200~hello world\x1b[201~");
        assert!(!wrapped.contains('\r'), "no carriage return in the burst");
    }

    #[test]
    fn bracketed_wrap_keeps_newlines_literal() {
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
        let wrapped = wrap_bracketed_paste("a\x1b[201~b\u{0085}c");
        assert_eq!(wrapped, "\x1b[200~a[201~bc\x1b[201~");
        assert_eq!(wrapped.matches("\x1b[200~").count(), 1);
        assert_eq!(wrapped.matches("\x1b[201~").count(), 1);
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
