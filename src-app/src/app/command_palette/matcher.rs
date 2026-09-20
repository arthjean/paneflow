use std::ops::Range;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Rank {
    LabelWordStart,
    LabelInside,
    Value,
    LabelSubsequence,
    Keyword,
}

#[derive(Debug, Clone)]
pub(crate) struct Match {
    pub(crate) rank: Rank,
    pub(crate) highlights: Vec<Range<usize>>,
}

pub(crate) fn match_entry(
    label: &str,
    value: Option<&str>,
    keywords: &str,
    query: &str,
) -> Option<Match> {
    let label_lower = label.to_ascii_lowercase();
    let value_lower = value.map(|value| value.to_lowercase());
    let keywords_lower = keywords.to_ascii_lowercase();

    let mut rank = Rank::LabelWordStart;
    let mut highlights = Vec::new();
    let mut matched_any = false;

    for word in query.split_whitespace() {
        matched_any = true;
        let word = word.to_lowercase();
        let word_rank =
            if let Some(literal) = literal_in_label(&label_lower, &word, &mut highlights) {
                literal
            } else if value_lower
                .as_deref()
                .is_some_and(|value| value.contains(&word))
            {
                Rank::Value
            } else if is_subsequence(&label_lower, &word) {
                Rank::LabelSubsequence
            } else if keywords_lower.contains(&word) || is_subsequence(&keywords_lower, &word) {
                Rank::Keyword
            } else {
                return None;
            };
        rank = rank.max(word_rank);
    }

    if !matched_any {
        return Some(Match {
            rank: Rank::Keyword,
            highlights: Vec::new(),
        });
    }

    Some(Match {
        rank,
        highlights: merge_ranges(highlights),
    })
}

fn literal_in_label(label: &str, word: &str, highlights: &mut Vec<Range<usize>>) -> Option<Rank> {
    let mut rank = None;
    for (start, matched) in label.match_indices(word) {
        let at_word_start = start == 0
            || !label[..start]
                .chars()
                .next_back()
                .is_some_and(char::is_alphanumeric);
        rank = Some(match (rank, at_word_start) {
            (Some(Rank::LabelWordStart), _) | (_, true) => Rank::LabelWordStart,
            _ => Rank::LabelInside,
        });
        highlights.push(start..start + matched.len());
    }
    rank
}

fn is_subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .all(|wanted| chars.any(|candidate| candidate == wanted))
}

fn merge_ranges(mut ranges: Vec<Range<usize>>) -> Vec<Range<usize>> {
    ranges.sort_by_key(|range| (range.start, range.end));
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => merged.push(range),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rank(label: &str, value: Option<&str>, keywords: &str, query: &str) -> Option<Rank> {
        match_entry(label, value, keywords, query).map(|matched| matched.rank)
    }

    #[test]
    fn a_word_start_literal_outranks_an_inside_literal() {
        assert_eq!(
            rank("Change theme", None, "", "the"),
            Some(Rank::LabelWordStart)
        );
        assert_eq!(rank("New window", None, "", "in"), Some(Rank::LabelInside));
    }

    #[test]
    fn a_current_value_matches_without_highlighting_the_label() {
        let matched = match_entry("Interface style", Some("Themed"), "", "them").unwrap();
        assert_eq!(matched.rank, Rank::Value);
        assert!(matched.highlights.is_empty());
    }

    #[test]
    fn a_label_subsequence_matches_last() {
        assert_eq!(
            rank("Light appearance", None, "", "in"),
            Some(Rank::LabelSubsequence)
        );
    }

    #[test]
    fn keywords_match_when_the_label_does_not() {
        assert_eq!(
            rank("Comfortable density", None, "interface spacing", "interf"),
            Some(Rank::Keyword)
        );
    }

    #[test]
    fn every_word_must_match_somewhere() {
        assert!(match_entry("Split vertical", None, "", "split vertical").is_some());
        assert!(match_entry("Split vertical", None, "", "split sideways").is_none());
    }

    #[test]
    fn the_worst_word_sets_the_rank() {
        assert_eq!(
            rank("Change theme", Some("Merino dark"), "", "change merino"),
            Some(Rank::Value)
        );
    }

    #[test]
    fn highlights_cover_every_literal_occurrence_and_never_overlap() {
        let matched = match_entry("Tab: new tab", None, "", "tab").unwrap();
        assert_eq!(matched.highlights, vec![0..3, 9..12]);
        let matched = match_entry("aaa", None, "", "aa a").unwrap();
        assert_eq!(matched.highlights, vec![0..3]);
    }

    #[test]
    fn an_empty_query_matches_everything() {
        assert!(match_entry("Anything", None, "", "   ").is_some());
    }
}
