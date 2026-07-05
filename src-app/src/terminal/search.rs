//! Search, copy-mode navigation, and terminal reset actions on `TerminalView`.
//!
//! Text matching, scroll-to-match, and the `SearchMatch` type live in the
//! crate-level `crate::search` module - this file only owns the `TerminalView`
//! plumbing that wires those utilities to keyboard actions and updates copy
//! mode state.
//!
//! Extracted from `terminal.rs` per US-014 of the src-app refactor PRD.

use alacritty_terminal::grid::{Dimensions, Scroll as AlacScroll};
use alacritty_terminal::index::{Column as GridCol, Line as GridLine, Point as AlacPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui::{ClipboardItem, Context, Focusable};

use super::TerminalView;

const LOCAL_SEARCH_DEBOUNCE_MS: u64 = 80;

fn copy_mode_entry_cursor(
    cursor_point: AlacPoint,
    display_offset: usize,
    screen_lines: usize,
) -> AlacPoint {
    let cursor_display_line = cursor_point.line.0 + display_offset as i32;
    if cursor_display_line >= 0 && cursor_display_line < screen_lines as i32 {
        cursor_point
    } else {
        let center_display = screen_lines as i32 / 2;
        AlacPoint::new(GridLine(center_display - display_offset as i32), GridCol(0))
    }
}

impl TerminalView {
    // --- Terminal control actions ---

    pub(super) fn clear_scroll_history(&mut self, cx: &mut Context<Self>) {
        let mut term = self.terminal.term.lock();
        term.grid_mut().clear_history();
        drop(term);
        cx.notify();
    }

    pub(super) fn reset_terminal(&mut self, cx: &mut Context<Self>) {
        // Automated RIS reset, NOT user input - go through the notifier
        // directly so US-002's keyboard_input_sent flag is not tripped.
        // Explicit `&[u8]` cast: pulling `markdown` adds `palette` to the
        // dep graph, whose blanket `AsRef` impls on byte arrays make a bare
        // `b"...".as_ref()` ambiguous.
        self.terminal.write_to_pty_silent(b"\x1bc".as_slice());
        cx.notify();
    }

    // --- Per-pane font zoom (EP-006 US-019) ---

    /// ±1 pt per-pane font zoom, clamped to [8.0, 32.0]; at a bound the
    /// step is a silent no-op (PRD AC - no toast). Writing the override is
    /// the whole job: the next frame re-measures the cell with it,
    /// recomputes cols/rows from the pane bounds, and `resize_if_needed`
    /// notifies the PTY - the exact window-resize path, so fullscreen TUIs
    /// reflow. Strictly per-view: sibling panes never change.
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
            // Global default already at the bound - don't pin a no-op
            // override that would stop tracking future global changes.
            return;
        }
        self.terminal.font_size_override = Some(next);
        cx.emit(super::TerminalEvent::FontZoomChanged);
        cx.notify();
    }

    /// Reset to the global font size (`override = None` - the pane follows
    /// live global changes again).
    pub(super) fn font_zoom_reset(&mut self, cx: &mut Context<Self>) {
        if self.terminal.font_size_override.take().is_some() {
            cx.emit(super::TerminalEvent::FontZoomChanged);
            cx.notify();
        }
    }

    // --- Search ---

    /// EP-006 US-018: hand the current query to the app for a fleet-wide
    /// fan-out. Empty query is a silent no-op; the regex validity check
    /// happens app-side ONCE (a single error surface, never N copies).
    pub(super) fn request_fleet_search(&mut self, cx: &mut Context<Self>) {
        if !self.search_active || self.search_query.trim().is_empty() {
            return;
        }
        cx.emit(super::TerminalEvent::FleetSearchRequested {
            query: self.search_query.clone(),
            regex: self.search_regex_mode,
        });
    }

    /// EP-006 US-018: arm THIS view's local search with a fleet query (the
    /// Enter-on-result teleport). Same effect as typing it in the find bar:
    /// overlay open, matches computed, viewport on the first hit - and the
    /// US-017 match rail renders from the same state.
    pub fn arm_search(&mut self, query: &str, regex: bool, cx: &mut Context<Self>) {
        self.search_active = true;
        self.search_query = query.to_string();
        self.search_regex_mode = regex;
        self.schedule_search(cx);
        cx.notify();
    }

    pub(super) fn toggle_search(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) {
        self.search_active = !self.search_active;
        self.search_generation = self.search_generation.wrapping_add(1);
        // Always reset the query state; the field starts empty on every open.
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_regex_error = None;
        self.search_input.update(cx, |input, cx| {
            input.clear(cx);
        });

        if self.search_active {
            // Move keyboard focus to the real input so keystrokes land in the
            // find bar, not the terminal/PTY - this is the whole point of using
            // a `TextInput` entity instead of capturing keys by hand.
            let handle = self.search_input.read(cx).focus_handle(cx);
            handle.focus(window, cx);
        } else {
            // Reset scroll position and hand focus back to the terminal.
            {
                let mut term = self.terminal.term.lock();
                term.scroll_display(AlacScroll::Bottom);
            }
            self.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// Re-run the search whenever the bound [`TextInput`] entity changes (wired
    /// via `cx.observe` in the view constructor). Keeps `search_query` - the
    /// source of truth for match scanning and the result counter - in sync with
    /// the field content, clamped to `MAX_QUERY_LEN` on a char boundary.
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
        self.search_active = false;
        self.search_generation = self.search_generation.wrapping_add(1);
        self.search_query.clear();
        self.search_matches.clear();
        self.search_current = 0;
        self.search_regex_error = None;
        let mut term = self.terminal.term.lock();
        term.scroll_display(AlacScroll::Bottom);
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
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        if self.search_query.is_empty() {
            self.search_matches.clear();
            self.search_regex_error = None;
            self.search_current = 0;
            return;
        }

        let term = self.terminal.term.clone();
        let query = self.search_query.clone();
        let regex = self.search_regex_mode;
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                smol::Timer::after(std::time::Duration::from_millis(LOCAL_SEARCH_DEBOUNCE_MS))
                    .await;
                let worker_query = query.clone();
                let result =
                    smol::unblock(move || crate::search::search_term(&term, &worker_query, regex))
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
        self.search_matches = result.matches;
        self.search_regex_error = result.regex_error;
        self.search_current = 0;
        if !self.search_matches.is_empty() {
            self.scroll_to_current_match();
        }
    }

    fn scroll_to_current_match(&mut self) {
        if let Some(m) = self.search_matches.get(self.search_current) {
            crate::search::scroll_to_match(&self.terminal.term, m);
        }
    }

    // --- Copy mode ---

    pub(super) fn toggle_copy_mode(&mut self, cx: &mut Context<Self>) {
        if self.copy_mode_active {
            self.exit_copy_mode(false, cx);
        } else {
            self.enter_copy_mode(cx);
        }
    }

    pub(super) fn enter_copy_mode(&mut self, cx: &mut Context<Self>) {
        // Dismiss search if active
        if self.search_active {
            self.dismiss_search(cx);
        }

        let mut term = self.terminal.term.lock();
        let cursor_point = term.renderable_content().cursor.point;
        let display_offset = term.grid().display_offset();
        let screen_lines = term.screen_lines();
        term.selection = None;

        let copy_cursor = copy_mode_entry_cursor(cursor_point, display_offset, screen_lines);
        drop(term);

        self.copy_cursor = copy_cursor;
        self.copy_mode_frozen_offset = display_offset;
        self.copy_mode_active = true;

        cx.notify();
    }

    pub(super) fn exit_copy_mode(&mut self, copy_to_clipboard: bool, cx: &mut Context<Self>) {
        let mut term = self.terminal.term.lock();

        if copy_to_clipboard {
            if let Some(text) = term.selection_to_string() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            // After copying, scroll to bottom
            term.scroll_display(AlacScroll::Bottom);
        } else {
            // On cancel, restore the scroll position from before copy mode entry
            let current = term.grid().display_offset();
            let frozen = self.copy_mode_frozen_offset;
            if current != frozen {
                let delta = frozen as i32 - current as i32;
                term.scroll_display(AlacScroll::Delta(delta));
            }
        }

        term.selection = None;
        drop(term);

        self.copy_mode_active = false;
        cx.notify();
    }

    pub(super) fn move_copy_cursor(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        let (cols, top, bottom) = {
            let mut term = self.terminal.term.lock();
            term.selection = None;
            (term.columns(), term.topmost_line(), term.bottommost_line())
        };

        let new_col = (self.copy_cursor.column.0 as i32 + dx)
            .max(0)
            .min(cols as i32 - 1) as usize;
        let new_line = (self.copy_cursor.line.0 + dy).max(top.0).min(bottom.0);
        self.copy_cursor = AlacPoint::new(GridLine(new_line), GridCol(new_col));

        self.ensure_copy_cursor_visible();
        cx.notify();
    }

    pub(super) fn extend_copy_selection(&mut self, dx: i32, dy: i32, cx: &mut Context<Self>) {
        let mut term = self.terminal.term.lock();
        let cols = term.columns();
        let top = term.topmost_line();
        let bottom = term.bottommost_line();

        // Start a new selection if none exists
        if term.selection.is_none() {
            let sel = Selection::new(SelectionType::Simple, self.copy_cursor, Side::Left);
            term.selection = Some(sel);
        }

        // Move cursor and update selection endpoint - all under the same lock
        let new_col = (self.copy_cursor.column.0 as i32 + dx)
            .max(0)
            .min(cols as i32 - 1) as usize;
        let new_line = (self.copy_cursor.line.0 + dy).max(top.0).min(bottom.0);
        self.copy_cursor = AlacPoint::new(GridLine(new_line), GridCol(new_col));

        if let Some(ref mut sel) = term.selection {
            sel.update(self.copy_cursor, Side::Right);
        }
        drop(term);

        self.ensure_copy_cursor_visible();
        cx.notify();
    }

    /// Scroll the view to keep the copy cursor visible, updating the frozen offset.
    fn ensure_copy_cursor_visible(&mut self) {
        let offset = self.copy_mode_frozen_offset as i32;
        let cursor_display_line = self.copy_cursor.line.0 + offset;

        let mut term = self.terminal.term.lock();
        let screen_lines = term.screen_lines() as i32;

        let new_offset = if cursor_display_line < 0 {
            // Cursor is above visible area - scroll up
            Some((offset - cursor_display_line) as usize)
        } else if cursor_display_line >= screen_lines {
            // Cursor is below visible area - scroll down
            let excess = cursor_display_line - screen_lines + 1;
            Some((offset - excess).max(0) as usize)
        } else {
            None
        };

        if let Some(new_offset) = new_offset {
            self.copy_mode_frozen_offset = new_offset;
            let current = term.grid().display_offset();
            let delta = new_offset as i32 - current as i32;
            if delta != 0 {
                term.scroll_display(AlacScroll::Delta(delta));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_mode_entry_keeps_visible_raw_cursor_at_live_edge() {
        let cursor = AlacPoint::new(GridLine(12), GridCol(8));
        assert_eq!(copy_mode_entry_cursor(cursor, 0, 24), cursor);
    }

    #[test]
    fn copy_mode_entry_centers_when_live_cursor_is_scrolled_out() {
        let cursor = AlacPoint::new(GridLine(23), GridCol(8));
        assert_eq!(
            copy_mode_entry_cursor(cursor, 10, 24),
            AlacPoint::new(GridLine(2), GridCol(0))
        );
    }

    #[test]
    fn copy_mode_entry_keeps_scrollback_cursor_when_visible() {
        let cursor = AlacPoint::new(GridLine(-5), GridCol(3));
        assert_eq!(copy_mode_entry_cursor(cursor, 10, 24), cursor);
    }
}
