//! In-buffer scrollback search for terminal panes.
//!
//! Searches the alacritty_terminal grid (scrollback + visible area) for
//! plain text or regex matches, returning grid-coordinate spans
//! that TerminalElement can highlight.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column as GridCol, Point as AlacPoint};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use paneflow_terminal_ghostty::SearchEngine;

use crate::terminal::ZedListener;
use crate::terminal::types::Point;

/// Maximum query length (bytes).
pub const MAX_QUERY_LEN: usize = paneflow_terminal_ghostty::MAX_QUERY_LEN;

/// A single search match: start and end points in the terminal grid.
#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub start: Point,
    pub end: Point,
}

/// Result of a search operation.
pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    /// If regex mode and the pattern is invalid, contains the error message.
    pub regex_error: Option<String>,
    /// The cell or match budget stopped the scan before the grid ended.
    pub truncated: bool,
}

fn extract_line_text_and_columns(
    term: &Term<ZedListener>,
    line: alacritty_terminal::index::Line,
    cols: usize,
    line_text: &mut String,
    char_to_col: &mut Vec<usize>,
) {
    line_text.clear();
    char_to_col.clear();
    line_text.reserve(cols);
    char_to_col.reserve(cols);
    for col in 0..cols {
        let cell = &term.grid()[AlacPoint::new(line, GridCol(col))];
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        char_to_col.push(col);
        if cell.c == '\0' {
            line_text.push(' ');
        } else {
            line_text.push(cell.c);
        }
        if let Some(zero_width) = cell.zerowidth() {
            for &character in zero_width {
                char_to_col.push(col);
                line_text.push(character);
            }
        }
    }
}

/// Search the terminal's full grid (scrollback + visible) for matches.
/// In plain text mode, performs case-insensitive substring matching.
/// In regex mode, compiles the query as a regex pattern.
pub fn search_term(
    term: &Arc<FairMutex<Term<ZedListener>>>,
    query: &str,
    regex_mode: bool,
) -> SearchResult {
    search_term_with_cancel(term, query, regex_mode, &AtomicBool::new(false))
}

pub fn search_term_with_cancel(
    term: &Arc<FairMutex<Term<ZedListener>>>,
    query: &str,
    regex_mode: bool,
    cancelled: &AtomicBool,
) -> SearchResult {
    let mut search = match SearchEngine::new(query, regex_mode) {
        Ok(search) => search,
        Err(error) => {
            return SearchResult {
                matches: Vec::new(),
                regex_error: Some(error.to_string()),
                truncated: false,
            };
        }
    };
    if search.is_done() {
        return from_shared_result(search.finish(false));
    }

    let (top, bottom, initial_cols) = {
        let term = term.lock();
        (term.topmost_line(), term.bottommost_line(), term.columns())
    };

    // Keep the `Term` lock only while copying one row. Regex and lowercase
    // matching can be expensive on large scrollback, and holding the FairMutex
    // for the whole scan blocks PTY output processing.
    let mut line_text = String::with_capacity(initial_cols);
    let mut char_to_col = Vec::with_capacity(initial_cols);
    let mut line = top;
    let mut scanned_cells = 0usize;
    let mut truncated = false;
    while line <= bottom {
        if cancelled.load(Ordering::Acquire) {
            truncated = true;
            break;
        }
        if scanned_cells >= paneflow_terminal_ghostty::MAX_SEARCH_CELLS {
            truncated = true;
            break;
        }
        let copied = {
            let term = term.lock();
            if line < term.topmost_line() || line > term.bottommost_line() {
                None
            } else {
                let cols = term.columns();
                if scanned_cells.saturating_add(cols) > paneflow_terminal_ghostty::MAX_SEARCH_CELLS
                {
                    truncated = true;
                    None
                } else {
                    scanned_cells += cols;
                    extract_line_text_and_columns(
                        &term,
                        line,
                        cols,
                        &mut line_text,
                        &mut char_to_col,
                    );
                    Some(())
                }
            }
        };
        if truncated {
            break;
        }
        let Some(()) = copied else {
            line += 1;
            continue;
        };

        if !search.push_line(line.0, &line_text, &char_to_col) {
            break;
        }
        line += 1;
    }
    from_shared_result(search.finish(truncated))
}

fn from_shared_result(result: paneflow_terminal_ghostty::SearchResult) -> SearchResult {
    SearchResult {
        matches: result
            .matches
            .into_iter()
            .map(|found| SearchMatch {
                start: Point::new(found.start.line, found.start.column),
                end: Point::new(found.end.line, found.end.column),
            })
            .collect(),
        regex_error: result.regex_error,
        truncated: result.truncated,
    }
}

/// Compute the display offset for scrolling to a match, and apply the scroll
/// in a single lock acquisition. Returns the applied display_offset.
pub fn scroll_to_match(term: &Arc<FairMutex<Term<ZedListener>>>, m: &SearchMatch) -> usize {
    use alacritty_terminal::grid::Scroll as AlacScroll;

    let mut term = term.lock();
    let bottom = term.bottommost_line();
    let screen_lines = term.screen_lines();

    // lines_from_bottom is always >= 0 because matches come from topmost..=bottommost
    let lines_from_bottom = bottom.0.saturating_sub(m.start.line.0);
    let half_screen = screen_lines / 2;

    let target_offset = if lines_from_bottom <= half_screen as i32 {
        0
    } else {
        (lines_from_bottom - half_screen as i32).max(0) as usize
    };

    let current = term.grid().display_offset();
    let delta = target_offset as i32 - current as i32;
    if delta != 0 {
        term.scroll_display(AlacScroll::Delta(delta));
    }

    target_offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalState;

    fn restored_search(text: &str, query: &str, regex_mode: bool) -> SearchResult {
        let state = TerminalState::new_display_only(5, 20);
        state.restore_scrollback(text);
        state.session_backend().search(query, regex_mode)
    }

    #[test]
    fn plain_search_matches_across_wide_char_spacer() {
        let result = restored_search("中abc", "中a", false);

        assert!(!result.matches.is_empty());
        assert_eq!(result.matches[0].start.column.0, 0);
        assert_eq!(result.matches[0].end.column.0, 2);
    }

    #[test]
    fn plain_search_column_mapping_survives_lowercase_expansion() {
        let result = restored_search("İabc", "abc", false);

        assert!(!result.matches.is_empty());
        assert_eq!(result.matches[0].start.column.0, 1);
        assert_eq!(result.matches[0].end.column.0, 3);
    }

    #[test]
    fn regex_search_matches_across_wide_char_spacer() {
        let result = restored_search("中abc", "中a", true);

        assert!(!result.matches.is_empty());
        assert_eq!(result.matches[0].start.column.0, 0);
        assert_eq!(result.matches[0].end.column.0, 2);
    }

    #[test]
    fn search_includes_combining_characters_at_their_base_column() {
        let result = restored_search("e\u{301}abc", "e\u{301}", false);

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].start.column.0, 0);
        assert_eq!(result.matches[0].end.column.0, 0);
    }

    #[test]
    fn cancelled_search_stops_before_scanning() {
        let state = TerminalState::new_display_only(5, 20);
        state.restore_scrollback("needle");
        let cancelled = AtomicBool::new(true);
        let result = state
            .session_backend()
            .search_with_cancel("needle", false, &cancelled);

        assert!(result.matches.is_empty());
        assert!(result.truncated);
    }
}
