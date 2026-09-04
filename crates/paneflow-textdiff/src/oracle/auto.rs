use std::time::Instant;

use crate::manager::LineOffsets;
use crate::text::{count_non_space_chars, is_equals, UNIMPORTANT_LINE_CHAR_COUNT};
use crate::{
    compare_lines, compare_lines_inner, split_lines, ComparisonPolicy, DiffFragment,
    HighlightPolicy, LineFragment,
};

use super::{check_diff_consistency, check_line_consistency, CH_FACE, CH_GUN, CH_MAN, CH_SMILE};

const RUNS: usize = 500;
const MAX_LENGTH: usize = 300;
const SEED: u64 = 0x5EED_D1FF_0000_0012;

const POLICIES: [ComparisonPolicy; 3] = [
    ComparisonPolicy::Default,
    ComparisonPolicy::TrimWhitespaces,
    ComparisonPolicy::IgnoreWhitespaces,
];

const HIGHLIGHTS: [HighlightPolicy; 2] = [HighlightPolicy::Words, HighlightPolicy::Lines];

struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound.max(1) as u64) as usize
    }
}

const TABLE: [&str; 11] = ["\n", "\n", "\t", " ", " ", ".", "<", "!", "a", "b", "c"];

const LETTERS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

const SURROGATES: [&str; 4] = [CH_SMILE, CH_FACE, CH_GUN, CH_MAN];

fn generate_text(rng: &mut Lcg, max_length: usize, high_surrogates: bool) -> String {
    let length = rng.below(max_length + 1);
    let extra = if high_surrogates { SURROGATES.len() } else { 0 };
    let count = TABLE.len() + LETTERS.len() + extra;
    let mut text = String::with_capacity(length);
    for _ in 0..length {
        let pick = rng.below(count);
        let piece = if pick < TABLE.len() {
            TABLE[pick]
        } else if pick < TABLE.len() + LETTERS.len() {
            LETTERS[pick - TABLE.len()]
        } else {
            SURROGATES[pick - TABLE.len() - LETTERS.len()]
        };
        text.push_str(piece);
    }
    text
}

#[test]
fn five_hundred_random_pairs_stay_fair_under_every_policy_and_highlight() {
    let started = Instant::now();
    let mut rng = Lcg(SEED);
    for run in 0..RUNS {
        for high_surrogates in [true, false] {
            let text1 = generate_text(&mut rng, MAX_LENGTH, high_surrogates);
            let text2 = generate_text(&mut rng, MAX_LENGTH, high_surrogates);
            for policy in POLICIES {
                for highlight in HIGHLIGHTS {
                    let context = format!(
                        "run {run}, surrogates {high_surrogates}, {policy:?}, {highlight:?}, text1 {text1:?}, text2 {text2:?}"
                    );
                    let fragments = compare_lines_inner(&text1, &text2, policy, highlight);
                    check_result_line(&text1, &text2, &fragments, policy, highlight, &context);
                }
                let lines1 = split_lines(&text1);
                let lines2 = split_lines(&text2);
                let ranges = compare_lines(&lines1, &lines2, policy);
                let context = format!("run {run}, {policy:?}, lines {lines1:?} vs {lines2:?}");
                verify_fair(&ranges, lines1.len(), lines2.len(), &context);
            }
        }
    }
    assert!(
        started.elapsed().as_secs() < 10,
        "the random suite must stay under ten seconds, took {:?}",
        started.elapsed()
    );
}

fn verify_fair(ranges: &[crate::Range], len1: usize, len2: usize, context: &str) {
    let mut last: Option<(usize, usize)> = None;
    for range in ranges {
        assert!(
            range.start1 <= range.end1 && range.start2 <= range.end2,
            "{context}"
        );
        assert!(!range.is_empty(), "{context}");
        if let Some((end1, end2)) = last {
            assert!(end1 <= range.start1 && end2 <= range.start2, "{context}");
            assert!(
                end1 != range.start1 || end2 != range.start2,
                "adjacent ranges were not merged ({context})"
            );
        }
        assert!(range.end1 <= len1 && range.end2 <= len2, "{context}");
        last = Some((range.end1, range.end2));
    }
}

fn check_result_line(
    text1: &str,
    text2: &str,
    fragments: &[LineFragment],
    policy: ComparisonPolicy,
    highlight: HighlightPolicy,
    context: &str,
) {
    check_line_consistency(fragments, text1, text2, context);
    let mut last: Option<(usize, usize)> = None;
    for fragment in fragments {
        if let Some((end1, end2)) = last {
            assert!(
                end1 != fragment.lines.start1 || end2 != fragment.lines.start2,
                "adjoining fragments survived squash ({context})"
            );
        }
        last = Some((fragment.lines.end1, fragment.lines.end2));
        if let Some(inner) = &fragment.inner {
            assert_eq!(highlight, HighlightPolicy::Words, "{context}");
            let sequence1 = &text1[fragment.offsets.start1..fragment.offsets.end1];
            let sequence2 = &text2[fragment.offsets.start2..fragment.offsets.end2];
            check_diff_consistency(inner, context);
            check_valid_ranges(sequence1, sequence2, inner, policy, false, context);
        }
    }
    let line_fragments: Vec<DiffFragment> = fragments
        .iter()
        .map(|fragment| DiffFragment {
            start1: fragment.offsets.start1,
            end1: fragment.offsets.end1,
            start2: fragment.offsets.start2,
            end2: fragment.offsets.end2,
        })
        .collect();
    check_valid_ranges(text1, text2, &line_fragments, policy, true, context);
    check_cant_trim_lines(text1, text2, fragments, policy, context);
}

fn check_valid_ranges(
    text1: &str,
    text2: &str,
    fragments: &[DiffFragment],
    policy: ComparisonPolicy,
    skip_newline: bool,
    context: &str,
) {
    let unchanged_policy = if policy == ComparisonPolicy::Default {
        ComparisonPolicy::Default
    } else {
        ComparisonPolicy::IgnoreWhitespaces
    };
    let changed_policy = if policy == ComparisonPolicy::IgnoreWhitespaces {
        ComparisonPolicy::IgnoreWhitespaces
    } else {
        ComparisonPolicy::Default
    };
    let mut last1 = 0;
    let mut last2 = 0;
    for fragment in fragments {
        let chunk1 = &text1[last1..fragment.start1];
        let chunk2 = &text2[last2..fragment.start2];
        assert!(
            equals_maybe_newline(chunk1, chunk2, unchanged_policy, skip_newline),
            "unchanged parts differ: {chunk1:?} vs {chunk2:?} ({context})"
        );
        if !skip_newline {
            let content1 = &text1[fragment.start1..fragment.end1];
            let content2 = &text2[fragment.start2..fragment.end2];
            assert!(
                !is_equals(content1, content2, changed_policy),
                "changed parts are equal: {content1:?} vs {content2:?} ({context})"
            );
        }
        assert!(text1.is_char_boundary(fragment.start1) && text1.is_char_boundary(fragment.end1));
        assert!(text2.is_char_boundary(fragment.start2) && text2.is_char_boundary(fragment.end2));
        last1 = fragment.end1;
        last2 = fragment.end2;
    }
    let chunk1 = &text1[last1..];
    let chunk2 = &text2[last2..];
    assert!(
        equals_maybe_newline(chunk1, chunk2, unchanged_policy, skip_newline),
        "unchanged tails differ: {chunk1:?} vs {chunk2:?} ({context})"
    );
}

fn equals_maybe_newline(
    chunk1: &str,
    chunk2: &str,
    policy: ComparisonPolicy,
    skip_newline: bool,
) -> bool {
    if is_equals(chunk1, chunk2, policy) {
        return true;
    }
    if !skip_newline || policy != ComparisonPolicy::Default {
        return false;
    }
    chunk1.strip_suffix('\n') == Some(chunk2) || chunk2.strip_suffix('\n') == Some(chunk1)
}

fn check_cant_trim_lines(
    text1: &str,
    text2: &str,
    fragments: &[LineFragment],
    policy: ComparisonPolicy,
    context: &str,
) {
    let offsets1 = LineOffsets::new(text1);
    let offsets2 = LineOffsets::new(text2);
    for fragment in fragments {
        let Some((first1, last1)) =
            first_last_lines(text1, &offsets1, fragment.lines.start1, fragment.lines.end1)
        else {
            continue;
        };
        let Some((first2, last2)) =
            first_last_lines(text2, &offsets2, fragment.lines.start2, fragment.lines.end2)
        else {
            continue;
        };
        check_non_equals_if_long_enough(first1, first2, policy, context);
        check_non_equals_if_long_enough(last1, last2, policy, context);
    }
}

fn check_non_equals_if_long_enough(
    line1: &str,
    line2: &str,
    policy: ComparisonPolicy,
    context: &str,
) {
    if policy == ComparisonPolicy::IgnoreWhitespaces
        && (count_non_space_chars(line1) <= UNIMPORTANT_LINE_CHAR_COUNT
            || count_non_space_chars(line2) <= UNIMPORTANT_LINE_CHAR_COUNT)
    {
        return;
    }
    assert!(
        !is_equals(line1, line2, policy),
        "boundary lines are equal: {line1:?} vs {line2:?} ({context})"
    );
}

fn first_last_lines<'a>(
    text: &'a str,
    offsets: &LineOffsets,
    start: usize,
    end: usize,
) -> Option<(&'a str, &'a str)> {
    if start == end {
        return None;
    }
    let first = &text[offsets.line_start(start)..offsets.line_end_with_newline(start)];
    let last = &text[offsets.line_start(end - 1)..offsets.line_end_with_newline(end - 1)];
    Some((first, last))
}
