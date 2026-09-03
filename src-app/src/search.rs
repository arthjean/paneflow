use crate::terminal::types::Point;

pub const MAX_QUERY_LEN: usize = paneflow_terminal_ghostty::MAX_QUERY_LEN;

#[derive(Clone, Debug)]
pub struct SearchMatch {
    pub start: Point,
    pub end: Point,
}

pub struct SearchResult {
    pub matches: Vec<SearchMatch>,
    pub regex_error: Option<String>,
    pub truncated: bool,
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
