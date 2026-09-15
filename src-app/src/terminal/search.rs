use gpui::{ClipboardItem, Context, Focusable};

use super::TerminalView;
use super::types::{Line, Point};

const SEARCH_REFRESH_DEBOUNCE_MS: u64 = 400;

#[derive(Clone, Copy)]
enum SearchScanKind {
    Query,
    Refresh,
}

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
        self.queue_native_search(String::new(), cx);
        self.search_matches.clear();
        self.search_current = 0;
        self.search_native_navigation_in_flight = None;
        self.search_native_navigation_queue.clear();
        self.search_native_snapshot = None;
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
        self.queue_native_search(String::new(), cx);
        self.search_matches.clear();
        self.search_current = 0;
        self.search_native_navigation_in_flight = None;
        self.search_native_navigation_queue.clear();
        self.search_native_snapshot = None;
        self.search_regex_error = None;
        self.search_truncated = false;
        self.terminal.session_backend().scroll_to_bottom();
        cx.notify();
    }

    pub(super) fn toggle_search_regex(&mut self, cx: &mut Context<Self>) {
        self.search_regex_mode = !self.search_regex_mode;
        self.schedule_search(cx);
        cx.notify();
    }

    pub(super) fn search_next(&mut self, cx: &mut Context<Self>) {
        if !self.search_regex_mode {
            self.navigate_native_search(false, cx);
            return;
        }
        if self.search_matches.is_empty() {
            return;
        }
        self.search_current = (self.search_current + 1) % self.search_matches.len();
        self.scroll_to_current_match();
        cx.notify();
    }

    pub(super) fn search_prev(&mut self, cx: &mut Context<Self>) {
        if !self.search_regex_mode {
            self.navigate_native_search(true, cx);
            return;
        }
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
        self.search_refresh_dirty = false;
        self.search_seen_output_generation = self.terminal.output_generation;
        self.search_matches.clear();
        self.search_regex_error = None;
        self.search_truncated = false;
        self.search_current = 0;
        self.search_native_snapshot = None;
        self.search_native_navigation_in_flight = None;
        self.search_native_navigation_queue.clear();
        if !self.search_regex_mode {
            self.search_scan_in_flight = !self.search_query.is_empty();
            self.queue_native_search(self.search_query.clone(), cx);
            return;
        }
        self.queue_native_search(String::new(), cx);
        if self.search_query.is_empty() {
            return;
        }
        self.spawn_search_scan(LOCAL_SEARCH_DEBOUNCE_MS, SearchScanKind::Query, cx);
    }

    pub(super) fn sync_search_with_terminal(&mut self, cx: &mut Context<Self>) {
        if let Some(query) = self.search_native_pending.take() {
            self.queue_native_search(query, cx);
        }
        if !self.search_active || self.search_query.is_empty() {
            return;
        }
        if !self.search_regex_mode {
            let state = self.terminal.session_backend().native_search_state();
            if state.query != self.search_query || self.search_native_pending.is_some() {
                return;
            }
            if self
                .search_native_snapshot
                .as_ref()
                .is_some_and(|previous| std::sync::Arc::ptr_eq(previous, &state))
            {
                return;
            }
            self.search_current = state.selected.unwrap_or(0);
            self.search_regex_error.clone_from(&state.error);
            self.search_scan_in_flight = !state.complete && state.error.is_none();
            self.search_truncated = !state.complete;
            if self.search_native_navigation_in_flight == Some(state.navigation_generation) {
                self.search_native_navigation_in_flight = None;
            }
            self.search_native_snapshot = Some(state);
            return;
        }
        let topmost = self.terminal.session_backend().grid_metrics().topmost_line;
        if topmost != self.search_anchor_topmost {
            crate::search::shift_matches(
                &mut self.search_matches,
                topmost.0 - self.search_anchor_topmost.0,
            );
            self.search_anchor_topmost = topmost;
        }
        let output_generation = self.terminal.output_generation;
        if output_generation == self.search_seen_output_generation {
            return;
        }
        self.search_seen_output_generation = output_generation;
        if self.search_scan_in_flight {
            self.search_refresh_dirty = true;
        } else {
            self.spawn_search_scan(SEARCH_REFRESH_DEBOUNCE_MS, SearchScanKind::Refresh, cx);
        }
    }

    fn spawn_search_scan(
        &mut self,
        debounce_ms: u64,
        kind: SearchScanKind,
        cx: &mut Context<Self>,
    ) {
        let generation = self.search_generation;
        let backend = self.terminal.session_backend();
        let query = self.search_query.clone();
        let regex = self.search_regex_mode;
        let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.search_cancellation = Some(cancellation.clone());
        self.search_scan_in_flight = true;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(debounce_ms)).await;
                let worker_query = query.clone();
                let scan = smol::unblock(move || {
                    let anchor = backend.grid_metrics().topmost_line;
                    let result = backend.search_with_cancel(&worker_query, regex, &cancellation);
                    (anchor, result)
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |view, cx| {
                        let current = view.search_generation == generation
                            && view.search_active
                            && view.search_query == query
                            && view.search_regex_mode == regex;
                        if current {
                            view.search_scan_in_flight = false;
                            let (anchor, result) = scan;
                            view.apply_search_result(result, anchor, kind);
                            cx.notify();
                        }
                        if !view.search_active {
                            view.search_refresh_dirty = false;
                        }
                        if current && view.search_regex_mode && view.search_refresh_dirty {
                            view.search_refresh_dirty = false;
                            view.spawn_search_scan(
                                SEARCH_REFRESH_DEBOUNCE_MS,
                                SearchScanKind::Refresh,
                                cx,
                            );
                        }
                    })
                });
            },
        )
        .detach();
    }

    fn apply_search_result(
        &mut self,
        result: crate::search::SearchResult,
        anchor: Line,
        kind: SearchScanKind,
    ) {
        self.search_cancellation = None;
        let previous = self.search_matches.get(self.search_current).cloned();
        self.search_matches = result.matches;
        self.search_regex_error = result.regex_error;
        self.search_truncated = result.truncated;
        self.search_anchor_topmost = anchor;
        let topmost = self.terminal.session_backend().grid_metrics().topmost_line;
        if topmost != anchor {
            crate::search::shift_matches(&mut self.search_matches, topmost.0 - anchor.0);
            self.search_anchor_topmost = topmost;
        }
        match kind {
            SearchScanKind::Query => {
                self.search_current = 0;
                if !self.search_matches.is_empty() {
                    self.scroll_to_current_match();
                }
            }
            SearchScanKind::Refresh => {
                self.search_current = crate::search::reconcile_current(
                    previous.as_ref(),
                    &self.search_matches,
                    self.search_current,
                );
            }
        }
    }

    fn cancel_pending_search(&mut self) {
        if let Some(cancellation) = self.search_cancellation.take() {
            cancellation.store(true, std::sync::atomic::Ordering::Release);
        }
        self.search_scan_in_flight = false;
        self.search_refresh_dirty = false;
    }

    fn queue_native_search(&mut self, query: String, cx: &mut Context<Self>) {
        if self
            .terminal
            .session_backend()
            .set_native_search(query.clone())
        {
            self.search_native_pending = None;
            if !self.search_regex_mode {
                self.search_regex_error = None;
            }
            return;
        }
        if !self.search_regex_mode && !query.is_empty() {
            self.search_regex_error =
                Some("The terminal could not accept the search; retrying".into());
        }
        self.search_native_pending = Some(query);
        self.schedule_native_search_retry(cx);
    }

    fn schedule_native_search_retry(&mut self, cx: &mut Context<Self>) {
        if self.search_native_retry_scheduled {
            return;
        }
        self.search_native_retry_scheduled = true;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(LOCAL_SEARCH_DEBOUNCE_MS))
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.search_native_retry_scheduled = false;
                    cx.notify();
                });
            },
        )
        .detach();
    }

    fn navigate_native_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self.search_match_count() == 0 {
            return;
        }
        self.search_native_navigation_queue.push_back(previous);
        self.dispatch_native_search_navigation(cx);
        cx.notify();
    }

    pub(super) fn search_match_count(&self) -> usize {
        if self.search_regex_mode {
            self.search_matches.len()
        } else {
            self.search_native_snapshot
                .as_ref()
                .map_or(0, |state| state.total_matches)
        }
    }

    pub(super) fn dispatch_native_search_navigation(&mut self, cx: &mut Context<Self>) {
        if self.search_native_navigation_in_flight.is_some() {
            return;
        }
        let Some(&previous) = self.search_native_navigation_queue.front() else {
            return;
        };
        let generation = self.search_native_navigation_generation.wrapping_add(1);
        if self
            .terminal
            .session_backend()
            .select_native_search(previous, generation)
        {
            self.search_native_navigation_queue.pop_front();
            self.search_native_navigation_generation = generation;
            self.search_native_navigation_in_flight = Some(generation);
        } else {
            self.search_regex_error =
                Some("The terminal could not accept search navigation".into());
            self.schedule_native_search_retry(cx);
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
