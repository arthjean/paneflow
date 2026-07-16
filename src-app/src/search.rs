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
use regex::Regex;
use std::sync::Arc;

use crate::terminal::ZedListener;
use crate::terminal::types::Point;

/// Maximum number of matches to collect before stopping search.
const MAX_MATCHES: usize = 10_000;

/// Maximum query length (bytes).
pub const MAX_QUERY_LEN: usize = 512;

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
    }
}

fn byte_to_char_index(text: &str, byte: usize) -> usize {
    text[..byte].chars().count()
}

fn search_match_from_chars(
    line: alacritty_terminal::index::Line,
    char_to_col: &[usize],
    char_start: usize,
    char_count: usize,
) -> Option<SearchMatch> {
    if char_count == 0 {
        return None;
    }
    let char_end = char_start + char_count - 1;
    let start_col = *char_to_col.get(char_start)?;
    let end_col = *char_to_col.get(char_end)?;
    Some(SearchMatch {
        start: Point::new(line.0, start_col),
        end: Point::new(line.0, end_col),
    })
}

fn fold_char(c: char) -> String {
    c.to_lowercase().collect()
}

fn push_plain_matches(
    line_text: &str,
    line: alacritty_terminal::index::Line,
    char_to_col: &[usize],
    query_folded: &[String],
    matches: &mut Vec<SearchMatch>,
) -> bool {
    if query_folded.is_empty() {
        return true;
    }
    let line_folded: Vec<String> = line_text.chars().map(fold_char).collect();
    let query_len = query_folded.len();
    if line_folded.len() < query_len {
        return true;
    }

    for char_start in 0..=(line_folded.len() - query_len) {
        if line_folded[char_start..char_start + query_len] == *query_folded
            && let Some(m) = search_match_from_chars(line, char_to_col, char_start, query_len)
        {
            matches.push(m);
            if matches.len() >= MAX_MATCHES {
                return false;
            }
        }
    }
    true
}

/// Search the terminal's full grid (scrollback + visible) for matches.
/// In plain text mode, performs case-insensitive substring matching.
/// In regex mode, compiles the query as a regex pattern.
pub fn search_term(
    term: &Arc<FairMutex<Term<ZedListener>>>,
    query: &str,
    regex_mode: bool,
) -> SearchResult {
    if query.is_empty() {
        return SearchResult {
            matches: Vec::new(),
            regex_error: None,
        };
    }

    // In regex mode, compile the pattern (case-insensitive)
    let compiled_regex = if regex_mode {
        match Regex::new(&format!("(?i)(?:{})", query)) {
            Ok(re) => Some(re),
            Err(e) => {
                return SearchResult {
                    matches: Vec::new(),
                    regex_error: Some(e.to_string()),
                };
            }
        }
    } else {
        None
    };

    let query_folded: Vec<String> = if regex_mode {
        Vec::new()
    } else {
        query.chars().map(fold_char).collect()
    };
    let mut matches = Vec::new();

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
    while line <= bottom {
        let Some(()) = ({
            let term = term.lock();
            if line < term.topmost_line() || line > term.bottommost_line() {
                None
            } else {
                let cols = term.columns();
                extract_line_text_and_columns(&term, line, cols, &mut line_text, &mut char_to_col);
                Some(())
            }
        }) else {
            line += 1;
            continue;
        };

        if let Some(re) = &compiled_regex {
            // Regex mode: use find_iter for all non-overlapping matches
            for m in re.find_iter(&line_text) {
                let char_start = byte_to_char_index(&line_text, m.start());
                let match_char_count = line_text[m.start()..m.end()].chars().count();
                if let Some(search_match) =
                    search_match_from_chars(line, &char_to_col, char_start, match_char_count)
                {
                    matches.push(search_match);
                    if matches.len() >= MAX_MATCHES {
                        return SearchResult {
                            matches,
                            regex_error: None,
                        };
                    }
                }
            }
        } else {
            if !push_plain_matches(&line_text, line, &char_to_col, &query_folded, &mut matches) {
                return SearchResult {
                    matches,
                    regex_error: None,
                };
            }
        }

        line += 1;
    }

    SearchResult {
        matches,
        regex_error: None,
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
}
