use super::*;

#[derive(Clone)]
pub(crate) struct TerminalSessionBackend {
    ghostty: GhosttySession,
}

pub(crate) struct TerminalBackendEvent(pub(super) GhosttyUiEvent);

impl TerminalBackendEvent {
    pub(crate) fn is_wakeup(&self) -> bool {
        self.0.is_wakeup()
    }
}

pub(crate) struct TerminalBackendEvents(pub(super) Option<UnboundedReceiver<GhosttyUiEvent>>);

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
    pub(in crate::terminal) ghostty: GhosttyRuntimePending,
}

#[cfg(test)]
thread_local! {
    static RENDER_CONTENT_TIMING_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static RENDER_CONTENT_LOCK_DURATIONS: std::cell::RefCell<Vec<std::time::Duration>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(crate) fn start_render_content_timing_probe() {
    RENDER_CONTENT_LOCK_DURATIONS.with(|durations| durations.borrow_mut().clear());
    RENDER_CONTENT_TIMING_ENABLED.with(|enabled| enabled.set(true));
}

#[cfg(test)]
pub(crate) fn take_render_content_lock_durations() -> Vec<std::time::Duration> {
    RENDER_CONTENT_TIMING_ENABLED.with(|enabled| enabled.set(false));
    RENDER_CONTENT_LOCK_DURATIONS.with(|durations| std::mem::take(&mut *durations.borrow_mut()))
}

impl TerminalSessionBackend {
    pub(super) fn new(ghostty: GhosttySession) -> Self {
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
            .with(|enabled| enabled.get())
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
                .with(|durations| durations.borrow_mut().push(snapshot_started_at.elapsed()));
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

    pub(crate) fn select_all_text(&self) -> Option<String> {
        self.ghostty.select_all_text()
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

    pub(crate) fn set_native_search(&self, query: String) -> bool {
        self.ghostty.set_native_search(query)
    }

    pub(crate) fn select_native_search(&self, previous: bool, generation: u64) -> bool {
        self.ghostty.select_native_search(previous, generation)
    }

    pub(crate) fn native_search_state(&self) -> Arc<crate::search::NativeSearchState> {
        self.ghostty.native_search_state()
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

    pub(crate) fn set_option_as_alt(&self, enabled: bool) -> bool {
        self.ghostty.set_option_as_alt(enabled)
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
