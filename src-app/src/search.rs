use crate::terminal::types::Point;

pub const MAX_QUERY_LEN: usize = paneflow_terminal_ghostty::MAX_QUERY_LEN;

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub start: Point,
    pub end: Point,
}

#[derive(Clone, Debug, Default)]
pub struct NativeSearchState {
    pub query: String,
    pub total_matches: usize,
    pub viewport_matches: Vec<SearchMatch>,
    pub selected_match: Option<SearchMatch>,
    pub rail_offsets: std::sync::Arc<[usize]>,
    pub navigation_generation: u64,
    pub selected: Option<usize>,
    pub complete: bool,
    pub error: Option<String>,
}

pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub regex_error: Option<String>,
    pub truncated: bool,
}

pub fn shift_matches(matches: &mut [SearchMatch], delta_lines: i32) {
    if delta_lines == 0 {
        return;
    }
    for m in matches {
        m.start.line.0 = m.start.line.0.saturating_add(delta_lines);
        m.end.line.0 = m.end.line.0.saturating_add(delta_lines);
    }
}

pub fn reconcile_current(
    previous: Option<&SearchMatch>,
    matches: &[SearchMatch],
    fallback: usize,
) -> usize {
    if matches.is_empty() {
        return 0;
    }
    previous
        .and_then(|prev| matches.iter().position(|m| m.start == prev.start))
        .unwrap_or_else(|| fallback.min(matches.len() - 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::TerminalState;
    use std::sync::atomic::AtomicBool;

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
    fn shift_matches_moves_both_ends_and_ignores_zero() {
        let mut matches = vec![SearchMatch {
            start: Point::new(-2, 1),
            end: Point::new(-2, 4),
        }];
        shift_matches(&mut matches, 0);
        assert_eq!(matches[0].start, Point::new(-2, 1));
        shift_matches(&mut matches, -3);
        assert_eq!(matches[0].start, Point::new(-5, 1));
        assert_eq!(matches[0].end, Point::new(-5, 4));
    }

    #[test]
    fn reconcile_current_follows_the_previous_match_or_clamps() {
        let matches = vec![
            SearchMatch {
                start: Point::new(-4, 0),
                end: Point::new(-4, 2),
            },
            SearchMatch {
                start: Point::new(1, 5),
                end: Point::new(1, 7),
            },
        ];
        let previous = matches[1].clone();
        assert_eq!(reconcile_current(Some(&previous), &matches, 0), 1);
        let gone = SearchMatch {
            start: Point::new(9, 9),
            end: Point::new(9, 9),
        };
        assert_eq!(reconcile_current(Some(&gone), &matches, 7), 1);
        assert_eq!(reconcile_current(None, &[], 3), 0);
    }

    #[test]
    fn appended_scrollback_shifts_earlier_matches_up_by_the_appended_rows() {
        let state = TerminalState::new_display_only(5, 20);
        state.restore_scrollback("needle\nb\nc\nd\ne\nf\ng");
        let backend = state.session_backend();
        let before = backend.search("needle", false);
        assert_eq!(before.matches.len(), 1);
        let topmost_before = backend.grid_metrics().topmost_line;

        state.restore_scrollback("h\ni\nj");
        let after = backend.search("needle", false);
        assert_eq!(after.matches.len(), 1);
        let topmost_after = backend.grid_metrics().topmost_line;

        assert_eq!(topmost_after.0, topmost_before.0 - 3);
        let mut shifted = before.matches.clone();
        shift_matches(&mut shifted, topmost_after.0 - topmost_before.0);
        assert_eq!(shifted[0].start, after.matches[0].start);
        assert_eq!(shifted[0].end, after.matches[0].end);
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
