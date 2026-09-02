use gpui::{ClipboardItem, Context, Focusable};

use super::TerminalView;
use super::types::Point;

const LOCAL_SEARCH_DEBOUNCE_MS: u64 = 80;

fn copy_mode_entry_cursor(
    cursor_point: Point,
    display_offset: usize,
    screen_lines: usize,
) -> Point {
    let cursor_display_line = cursor_point.line.0 + display_offset as i32;
    if cursor_display_line >= 0 && cursor_display_line < screen_lines as i32 {
        cursor_point
    } else {
        let center_display = screen_lines as i32 / 2;
        Point::new(center_display - display_offset as i32, 0)
    }
}

impl TerminalView {
    pub(super) fn clear_scroll_history(&mut self, cx: &mut Context<Self>) {
        self.terminal.session_backend().clear_history();
        cx.notify();
    }

    pub(super) fn reset_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal.write_to_pty_silent(b"\x1bc".as_slice());
        cx.notify();
    }

    pub(super) fn font_zoom_step(&mut self, delta: f32, cx: &mut Context<Self>) {
        let current = self
            .terminal
            .font_size_override
            .unwrap_or_else(crate::terminal::element::global_font_size);
        let next = (current + delta).clamp(
            crate::terminal::element::MIN_FONT_SIZE,
            crate::terminal::element::MAX_FONT_SIZE,
        );
        if next == current && self.terminal.font_size_override.is_some() {
            return;
        }
        if next == current && self.terminal.font_size_override.is_none() {
            return;
        }
        self.terminal.font_size_override = Some(next);
        cx.emit(super::TerminalEvent::FontZoomChanged);
        cx.notify();
    }

    pub(super) fn font_zoom_reset(&mut self, cx: &mut Context<Self>) {
        if self.terminal.font_size_override.take().is_some() {
            cx.emit(super::TerminalEvent::FontZoomChanged);
            cx.notify();
        }
    }

    pub(super) fn request_fleet_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_active || self.search_query.trim().is_empty() {
            return;
        }
        cx.emit(super::TerminalEvent::FleetSearchRequested {
            query: self.search_query.clone(),
            regex: self.search_regex_mode,
        });
    }

    pub fn arm_search(&mut self, query: &str, regex: bool, cx: &mut Context<Self>) {
        self.search_active = true;
        self.search_query = query.to_string();
        self.search_regex_mode = regex;
        self.schedule_search(cx);
        cx.notify();
    }

    pub(super) fn toggle_search(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.cancel_pending_search();
        self.search_active = !self.search_active;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_regex_error = None;
        self.search_truncated = false;
        self.search_input.update(cx, |input, cx| {
            input.clear(cx);
        });

        if self.search_active {
            let handle = self.search_input.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        } else {
            {
                self.terminal.session_backend().scroll_to_bottom();
            }
            self.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_search_input_changed(&mut self, cx: &mut Context<Self>) {
        if !self.search_active {
            return;
        }
        let mut q = self.search_input.read(cx).value();
        if q.len() > crate::search::MAX_QUERY_LEN {
            let mut end = crate::search::MAX_QUERY_LEN;
            while end > 0 && !q.is_char_boundary(end) {
                end -= 1;
            }
            q.truncate(end);
        }
        if q != self.search_query {
            self.search_query = q;
            self.schedule_search(cx);
            cx.notify();
        }
    }

    pub(super) fn dismiss_search(&mut self, cx: &mut Context<Self>) {
        self.cancel_pending_search();
        self.search_active = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_regex_error = None;
        self.search_truncated = false;
        self.terminal.session_backend().scroll_to_bottom();
        cx.notify();
    }

    pub(super) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.search_regex_mode = !self.search_regex_mode;
        if !self.search_query.is_empty() {
            self.schedule_search(cx);
        }
        cx.notify();
    }

    pub(super) fn search_next(&mut self, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.scroll_to_current_match();
        cx.notify();
    }

    pub(super) fn search_prev(&mut self, cx: &mut Context<Self>) {
        if self.search_matches.is_empty() {
            return;
        }
        if self.search_current == 0 {
            self.search_current = self.search_matches.len() - 1;
        } else {
            self.search_current -= 1;
        }
        self.scroll_to_current_match();
        cx.notify();
    }

    fn schedule_search(&mut self, cx: &mut Context<Self>) {
        self.cancel_pending_search();
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        if self.search_query.is_empty() {
            self.search_matches.clear();
            self.search_regex_error = None;
            self.search_truncated = false;
            self.search_current = 0;
            return;
        }

        let backend = self.terminal.session_backend();
        let query = self.search_query.clone();
        let regex = self.search_regex_mode;
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.search_cancellation = Some(cancellation.clone());
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(LOCAL_SEARCH_DEBOUNCE_MS))
                    .await;
                let worker_query = query.clone();
                let result = smol::unblock(move || {
                    backend.search_with_cancel(&worker_query, regex, &cancellation)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        if view.search_generation == generation
                            && view.search_active
                            && view.search_query == query
                            && view.search_regex_mode == regex
                        {
                            view.apply_search_result(result);
                            cx.notify();
                        }
                    })
                });
            },
        )
        .detach();
    }

    fn apply_search_result(&mut self, result: crate::search::SearchResult) {
        self.search_cancellation = None;
        self.search_matches = result.matches;
        self.search_regex_error = result.regex_error;
        self.search_truncated = result.truncated;
        self.search_current = 0;
        if !self.search_matches.is_empty() {
            self.scroll_to_current_match();
        }
    }

    fn cancel_pending_search(&mut self) {
        if let Some(cancellation) = self.search_cancellation.take() {
            cancellation.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    fn scroll_to_current_match(&mut self) {
        if let Some(m) = self.search_matches.get(self.search_current) {
            self.terminal.session_backend().scroll_to_match(m);
        }
    }

    pub(super) fn toggle_copy_mode(&mut self, cx: &mut Context<Self>) {
        if self.copy_mode_active {
            self.exit_copy_mode(false, cx);
        } else {
            self.enter_copy_mode(cx);
        }
    }

    pub(super) fn enter_copy_mode(&mut self, cx: &mut Context<Self>) {
        if self.search_active {
            self.dismiss_search(cx);
        }

        let backend = self.terminal.session_backend();
        let metrics = backend.grid_metrics();
        backend.clear_selection();

        let copy_cursor =
            copy_mode_entry_cursor(metrics.cursor, metrics.display_offset, metrics.screen_lines);

        self.copy_cursor = copy_cursor;
        self.copy_mode_frozen_offset = metrics.display_offset;
        self.copy_mode_active = true;

        cx.notify();
    }

    pub(super) fn exit_copy_mode(&mut self, copy_to_clipboard: bool, cx: &mut Context<Self>) {
        let backend = self.terminal.session_backend();

        if copy_to_clipboard {
            if let Some(text) = backend.selection_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            backend.scroll_to_bottom();
        } else {
            backend.restore_display_offset(self.copy_mode_frozen_offset);
        }

        backend.clear_selection();

        self.copy_mode_active = false;
        cx.notify();
    }

    pub(super) fn move_copy_cursor(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        self.copy_cursor =
            self.terminal
                .session_backend()
                .move_copy_cursor(self.copy_cursor, dx, dy, false);

        self.ensure_copy_cursor_visible();
        cx.notify();
    }

    pub(super) fn extend_copy_selection(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        self.copy_cursor =
            self.terminal
                .session_backend()
                .move_copy_cursor(self.copy_cursor, dx, dy, true);

        self.ensure_copy_cursor_visible();
        cx.notify();
    }

    fn ensure_copy_cursor_visible(&mut self) {
        let offset = self.copy_mode_frozen_offset as i32;
        let cursor_display_line = self.copy_cursor.line.0 + offset;

        let backend = self.terminal.session_backend();
        let screen_lines = backend.grid_metrics().screen_lines as i32;

        let new_offset = if cursor_display_line < 0 {
            Some((offset - cursor_display_line) as usize)
        } else if cursor_display_line >= screen_lines {
            let excess = cursor_display_line - screen_lines + 1;
            Some((offset - excess).max(0) as usize)
        } else {
            None
        };

        if let Some(new_offset) = new_offset {
            self.copy_mode_frozen_offset = new_offset;
            backend.restore_display_offset(new_offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_mode_entry_keeps_visible_raw_cursor_at_live_edge() {
        let cursor = Point::new(12, 8);
        assert_eq!(copy_mode_entry_cursor(cursor, 0, 24), cursor);
    }

    #[test]
    fn copy_mode_entry_centers_when_live_cursor_is_scrolled_out() {
        let cursor = Point::new(23, 8);
        assert_eq!(copy_mode_entry_cursor(cursor, 10, 24), Point::new(2, 0));
    }

    #[test]
    fn copy_mode_entry_keeps_scrollback_cursor_when_visible() {
        let cursor = Point::new(-5, 3);
        assert_eq!(copy_mode_entry_cursor(cursor, 10, 24), cursor);
    }
}
