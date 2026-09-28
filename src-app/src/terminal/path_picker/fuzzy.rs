use std::ops::Range;

const SCORE_MATCH: i32 = 16;
const BONUS_BOUNDARY: i32 = 10;
const BONUS_CAMEL: i32 = 8;
const BONUS_CONSECUTIVE: i32 = 6;
const PENALTY_GAP_START: i32 = 3;
const PENALTY_GAP_EXTENSION: i32 = 1;
const BONUS_IN_NAME: i32 = 64;
const BONUS_WHOLE_NAME: i32 = 32;

pub(super) struct Pattern {
    folded: Vec<char>,
}

impl Pattern {
    pub(super) fn new(query: &str) -> Self {
        Self {
            folded: query.chars().map(fold).collect(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.folded.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PathMatch {
    pub(super) score: i32,
    pub(super) highlights: Vec<Range<usize>>,
}

pub(super) fn match_path(path: &str, pattern: &Pattern) -> Option<PathMatch> {
    if pattern.is_empty() {
        return Some(PathMatch {
            score: 0,
            highlights: Vec::new(),
        });
    }
    let name_start = path
        .rfind(std::path::is_separator)
        .map_or(0, |index| index + 1);
    let name = &path[name_start..];
    if let Some(positions) = align(name, &pattern.folded) {
        let mut score = score(name, &positions) + BONUS_IN_NAME;
        if name.chars().map(fold).eq(pattern.folded.iter().copied()) {
            score += BONUS_WHOLE_NAME;
        }
        return Some(PathMatch {
            score,
            highlights: ranges(path, positions.into_iter().map(|index| index + name_start)),
        });
    }
    let positions = align(path, &pattern.folded)?;
    Some(PathMatch {
        score: score(path, &positions),
        highlights: ranges(path, positions),
    })
}

fn align(haystack: &str, needle: &[char]) -> Option<Vec<usize>> {
    let mut remaining = needle.iter().peekable();
    let mut end = None;
    for (index, ch) in haystack.char_indices() {
        let Some(&&wanted) = remaining.peek() else {
            break;
        };
        if fold(ch) == wanted {
            remaining.next();
            if remaining.peek().is_none() {
                end = Some(index + ch.len_utf8());
                break;
            }
        }
    }
    let end = end?;
    let mut positions = Vec::with_capacity(needle.len());
    let mut remaining = needle.iter().rev().peekable();
    for (index, ch) in haystack[..end].char_indices().rev() {
        let Some(&&wanted) = remaining.peek() else {
            break;
        };
        if fold(ch) == wanted {
            positions.push(index);
            remaining.next();
        }
    }
    positions.reverse();
    Some(positions)
}

fn score(haystack: &str, positions: &[usize]) -> i32 {
    let mut total = 0;
    let mut previous_end = None;
    for (nth, &index) in positions.iter().enumerate() {
        let Some(current) = haystack[index..].chars().next() else {
            continue;
        };
        let bonus = boundary_bonus(haystack[..index].chars().next_back(), current);
        total += SCORE_MATCH + if nth == 0 { 2 * bonus } else { bonus };
        match previous_end {
            Some(end) if end == index => total += BONUS_CONSECUTIVE,
            Some(end) => {
                let gap = haystack[end..index].chars().count() as i32;
                total -= PENALTY_GAP_START + PENALTY_GAP_EXTENSION * (gap - 1);
            }
            None => {}
        }
        previous_end = Some(index + current.len_utf8());
    }
    total
}

fn boundary_bonus(before: Option<char>, current: char) -> i32 {
    match before {
        None => BONUS_BOUNDARY,
        Some(before) if !before.is_alphanumeric() => BONUS_BOUNDARY,
        Some(before) if before.is_lowercase() && current.is_uppercase() => BONUS_CAMEL,
        Some(_) => 0,
    }
}

fn ranges(text: &str, positions: impl IntoIterator<Item = usize>) -> Vec<Range<usize>> {
    let mut merged: Vec<Range<usize>> = Vec::new();
    for start in positions {
        let end = start + text[start..].chars().next().map_or(0, char::len_utf8);
        match merged.last_mut() {
            Some(last) if last.end == start => last.end = end,
            _ => merged.push(start..end),
        }
    }
    merged
}

fn fold(ch: char) -> char {
    if ch.is_ascii() {
        ch.to_ascii_lowercase()
    } else {
        ch.to_lowercase().next().unwrap_or(ch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(path: &str, query: &str) -> Option<PathMatch> {
        match_path(path, &Pattern::new(query))
    }

    fn score_of(path: &str, query: &str) -> i32 {
        found(path, query)
            .unwrap_or_else(|| panic!("{query:?} must match {path:?}"))
            .score
    }

    #[test]
    fn a_query_must_appear_in_order() {
        assert!(found("src/main.rs", "smr").is_some());
        assert!(found("src/main.rs", "nms").is_none());
        assert!(found("abc", "abd").is_none());
    }

    #[test]
    fn an_empty_query_matches_without_highlights() {
        assert_eq!(
            found("anything", ""),
            Some(PathMatch {
                score: 0,
                highlights: Vec::new(),
            })
        );
    }

    #[test]
    fn matching_ignores_case() {
        let matched = found("README.md", "readme").expect("case-insensitive match");
        assert_eq!(matched.highlights, vec![0..6]);
    }

    #[test]
    fn a_match_inside_the_file_name_outranks_one_spread_over_directories() {
        assert!(score_of("docs/main.md", "main") > score_of("mod/ai/notes.rs", "main"));
    }

    #[test]
    fn the_whole_name_outranks_a_longer_name_with_the_same_prefix() {
        assert!(score_of("LICENSE", "license") > score_of("license-lookup-app", "license"));
    }

    #[test]
    fn consecutive_characters_outrank_scattered_ones() {
        assert!(score_of("abc", "abc") > score_of("axbxc", "abc"));
    }

    #[test]
    fn a_word_start_outranks_a_mid_word_start() {
        assert!(score_of("foo_bar.rs", "bar") > score_of("foobar.rs", "bar"));
        assert!(score_of("fooBar.rs", "bar") > score_of("foobar.rs", "bar"));
    }

    #[test]
    fn the_alignment_prefers_the_tightest_window() {
        let matched = found("xaxxab", "ab").expect("match");
        assert_eq!(matched.highlights, vec![4..6]);
    }

    #[test]
    fn highlights_land_in_the_directories_when_the_name_does_not_match() {
        let path = ["apps", "license-lookup-app", "src"].join(std::path::MAIN_SEPARATOR_STR);
        let matched = found(&path, "lic").expect("match");
        assert_eq!(matched.highlights, vec![5..8]);
    }

    #[test]
    fn highlights_offset_into_the_file_name() {
        let path = ["apps", "types", "License.ts"].join(std::path::MAIN_SEPARATOR_STR);
        let matched = found(&path, "lic").expect("match");
        let start = path.rfind("License").expect("name");
        assert_eq!(matched.highlights, vec![start..start + 3]);
    }

    #[test]
    fn non_ascii_characters_fold_and_highlight_on_char_boundaries() {
        let path = ["Écrits", "Été.md"].join(std::path::MAIN_SEPARATOR_STR);
        let matched = found(&path, "été").expect("folded match");
        let start = path.find("Été").expect("name");
        assert_eq!(matched.highlights, vec![start..start + "Été".len()]);
    }

    #[cfg(windows)]
    #[test]
    fn a_backslash_separates_the_file_name_on_windows() {
        assert_eq!(
            score_of(r"src\main.rs", "main"),
            score_of("main.rs", "main")
        );
    }
}
