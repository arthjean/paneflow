#![allow(
    clippy::panic,
    reason = "the oracle harness reports fixture mismatches with contextual diagnostics"
)]

mod auto;
mod chars;
mod lines;
mod split;
mod trim;
mod words;

use crate::by_char;
use crate::by_word;
use crate::manager::{self, LineOffsets};
use crate::text::is_equals;
use crate::{
    compare_chars, compare_lines_inner, compare_words, ComparisonPolicy, DiffFragment,
    HighlightPolicy, LineFragment, Range,
};

pub(crate) const CH_SMILE: &str = "\u{1F602}";
pub(crate) const CH_GUN: &str = "\u{1F52B}";
pub(crate) const CH_MAN: &str = "\u{1F9D2}";
pub(crate) const CH_FACE: &str = "\u{1F92B}";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Line,
    LineInner,
    Word,
    Char,
    CharSmart,
    CharRaw,
    Splitter { squash: bool, trim: bool },
}

#[derive(Default)]
struct PolicyData<T> {
    default: Option<T>,
    trim: Option<T>,
    ignore: Option<T>,
}

impl<T> PolicyData<T> {
    fn get(&self, policy: ComparisonPolicy) -> Option<&T> {
        match policy {
            ComparisonPolicy::IgnoreWhitespaces => self
                .ignore
                .as_ref()
                .or(self.trim.as_ref())
                .or(self.default.as_ref()),
            ComparisonPolicy::TrimWhitespaces => self.trim.as_ref().or(self.default.as_ref()),
            ComparisonPolicy::Default => self.default.as_ref(),
        }
    }
}

type Matching = (Vec<bool>, Vec<bool>);

pub(crate) struct TestBuilder {
    kind: Kind,
    before: Option<String>,
    after: Option<String>,
    matchings: PolicyData<Matching>,
    changes: PolicyData<Vec<Range>>,
    executed: bool,
}

pub(crate) fn mod_(line1: usize, line2: usize, count1: usize, count2: usize) -> Range {
    assert!(count1 != 0 && count2 != 0);
    Range::new(line1, line1 + count1, line2, line2 + count2)
}

pub(crate) fn del(line1: usize, line2: usize, count1: usize) -> Range {
    assert!(count1 != 0);
    Range::new(line1, line1 + count1, line2, line2)
}

pub(crate) fn ins(line1: usize, line2: usize, count2: usize) -> Range {
    assert!(count2 != 0);
    Range::new(line1, line1, line2, line2 + count2)
}

pub(crate) fn parse_source(source: &str) -> String {
    source.replace('_', "\n")
}

fn run(kind: Kind, f: impl FnOnce(&mut TestBuilder)) {
    let mut builder = TestBuilder {
        kind,
        before: None,
        after: None,
        matchings: PolicyData::default(),
        changes: PolicyData::default(),
        executed: false,
    };
    f(&mut builder);
    assert!(builder.executed, "fixture declared no policy run");
}

pub(crate) fn lines(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::Line, f);
}

pub(crate) fn lines_inner(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::LineInner, f);
}

pub(crate) fn words(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::Word, f);
}

pub(crate) fn chars(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::Char, f);
}

pub(crate) fn chars_raw(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::CharRaw, f);
}

pub(crate) fn chars_smart(f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::CharSmart, f);
}

pub(crate) fn splitter(f: impl FnOnce(&mut TestBuilder)) {
    run(
        Kind::Splitter {
            squash: false,
            trim: false,
        },
        f,
    );
}

pub(crate) fn splitter_with(squash: bool, trim: bool, f: impl FnOnce(&mut TestBuilder)) {
    run(Kind::Splitter { squash, trim }, f);
}

impl TestBuilder {
    pub(crate) fn text(&mut self, before: &str, after: &str) -> &mut Self {
        self.before = Some(parse_source(before));
        self.after = Some(parse_source(after));
        self
    }

    pub(crate) fn plain_text(&mut self, before: &str, after: &str) -> &mut Self {
        self.before = Some(before.to_string());
        self.after = Some(after.to_string());
        self
    }

    fn parse_matching(&self, before: &str, after: &str) -> Matching {
        let text_before = self.before.as_deref().expect("text set before matching");
        let text_after = self.after.as_deref().expect("text set before matching");
        if self.kind == Kind::Line {
            (
                parse_line_matching(before, text_before),
                parse_line_matching(after, text_after),
            )
        } else {
            (
                parse_char_matching(before, text_before),
                parse_char_matching(after, text_after),
            )
        }
    }

    pub(crate) fn matching_default(&mut self, before: &str, after: &str) -> &mut Self {
        assert!(self.matchings.default.is_none());
        self.matchings.default = Some(self.parse_matching(before, after));
        self
    }

    pub(crate) fn matching_trim(&mut self, before: &str, after: &str) -> &mut Self {
        assert!(self.matchings.trim.is_none());
        self.matchings.trim = Some(self.parse_matching(before, after));
        self
    }

    pub(crate) fn matching_ignore(&mut self, before: &str, after: &str) -> &mut Self {
        assert!(self.matchings.ignore.is_none());
        self.matchings.ignore = Some(self.parse_matching(before, after));
        self
    }

    pub(crate) fn default(&mut self, expected: &[Range]) -> &mut Self {
        assert!(self.changes.default.is_none());
        self.changes.default = Some(expected.to_vec());
        self
    }

    pub(crate) fn trim(&mut self, expected: &[Range]) -> &mut Self {
        assert!(self.changes.trim.is_none());
        self.changes.trim = Some(expected.to_vec());
        self
    }

    pub(crate) fn ignore(&mut self, expected: &[Range]) -> &mut Self {
        assert!(self.changes.ignore.is_none());
        self.changes.ignore = Some(expected.to_vec());
        self
    }

    pub(crate) fn test_all(&mut self) {
        self.test_default();
        self.test_trim();
        self.test_ignore();
    }

    pub(crate) fn test_default(&mut self) {
        self.run_policy(ComparisonPolicy::Default);
    }

    pub(crate) fn test_trim(&mut self) {
        self.run_policy(ComparisonPolicy::TrimWhitespaces);
    }

    pub(crate) fn test_ignore(&mut self) {
        self.run_policy(ComparisonPolicy::IgnoreWhitespaces);
    }

    fn run_policy(&mut self, policy: ComparisonPolicy) {
        self.executed = true;
        let before = self.before.clone().expect("fixture text");
        let after = self.after.clone().expect("fixture text");
        let matchings = self.matchings.get(policy);
        let changes = self.changes.get(policy);
        assert!(
            matchings.is_some() || changes.is_some(),
            "fixture declares no expectation for {policy:?}"
        );
        let context = format!("policy {policy:?}, before {before:?}, after {after:?}");
        match self.kind {
            Kind::Line => line_test(&before, &after, matchings, changes, policy, &context),
            Kind::LineInner => {
                line_inner_test(&before, &after, matchings, changes, policy, &context);
                word_test(&before, &after, matchings, changes, policy, &context);
            }
            Kind::Word => word_test(&before, &after, matchings, changes, policy, &context),
            Kind::Char => {
                char_test(&before, &after, matchings, changes, policy, &context);
                if policy == ComparisonPolicy::Default {
                    char_raw_test(&before, &after, matchings, changes, &context);
                }
            }
            Kind::CharSmart => char_test(&before, &after, matchings, changes, policy, &context),
            Kind::CharRaw => {
                if policy == ComparisonPolicy::Default {
                    char_raw_test(&before, &after, matchings, changes, &context);
                }
            }
            Kind::Splitter { squash, trim } => {
                assert!(matchings.is_none());
                splitter_test(&before, &after, squash, trim, changes, policy, &context);
            }
        }
    }
}

fn line_test(
    before: &str,
    after: &str,
    matchings: Option<&Matching>,
    changes: Option<&Vec<Range>>,
    policy: ComparisonPolicy,
    context: &str,
) {
    let fragments = compare_lines_inner(before, after, policy, HighlightPolicy::Lines);
    check_line_consistency(&fragments, before, after, context);
    if let Some(matchings) = matchings {
        check_line_matching(&fragments, matchings, before, after, context);
    }
    if let Some(changes) = changes {
        check_line_changes(&fragments, changes, context);
    }
}

fn line_inner_test(
    before: &str,
    after: &str,
    matchings: Option<&Matching>,
    changes: Option<&Vec<Range>>,
    policy: ComparisonPolicy,
    context: &str,
) {
    let fragments = compare_lines_inner(before, after, policy, HighlightPolicy::Words);
    assert_eq!(
        fragments.len(),
        1,
        "one squashed fragment expected, {context}"
    );
    let fragment = &fragments[0];
    assert_eq!(
        fragment.offsets,
        Range::new(0, before.len(), 0, after.len()),
        "{context}"
    );
    let inner = fragment
        .inner
        .as_ref()
        .unwrap_or_else(|| panic!("inner fragments expected, {context}"));
    check_line_consistency(&fragments, before, after, context);
    if let Some(matchings) = matchings {
        check_diff_matching(inner, matchings, before, after, context);
    }
    if let Some(changes) = changes {
        check_diff_changes(inner, changes, context);
    }
}

fn word_test(
    before: &str,
    after: &str,
    matchings: Option<&Matching>,
    changes: Option<&Vec<Range>>,
    policy: ComparisonPolicy,
    context: &str,
) {
    let fragments = compare_words(before, after, policy).expect("small fixture");
    check_diff_consistency(&fragments, context);
    if let Some(matchings) = matchings {
        check_diff_matching(&fragments, matchings, before, after, context);
    }
    if let Some(changes) = changes {
        check_diff_changes(&fragments, changes, context);
    }
}

fn char_test(
    before: &str,
    after: &str,
    matchings: Option<&Matching>,
    changes: Option<&Vec<Range>>,
    policy: ComparisonPolicy,
    context: &str,
) {
    let fragments = compare_chars(before, after, policy).expect("small fixture");
    check_diff_consistency(&fragments, context);
    if let Some(matchings) = matchings {
        check_diff_matching(&fragments, matchings, before, after, context);
    }
    if let Some(changes) = changes {
        check_diff_changes(&fragments, changes, context);
    }
}

fn char_raw_test(
    before: &str,
    after: &str,
    matchings: Option<&Matching>,
    changes: Option<&Vec<Range>>,
    context: &str,
) {
    let context = format!("raw, {context}");
    let fragments = by_word::convert_into_fragments(&by_char::compare(before, after));
    check_diff_consistency(&fragments, &context);
    if let Some(matchings) = matchings {
        check_diff_matching(&fragments, matchings, before, after, &context);
    }
    if let Some(changes) = changes {
        check_diff_changes(&fragments, changes, &context);
    }
}

fn splitter_test(
    before: &str,
    after: &str,
    squash: bool,
    trim: bool,
    changes: Option<&Vec<Range>>,
    policy: ComparisonPolicy,
    context: &str,
) {
    let fragments =
        manager::compare_lines_inner_unsquashed(before, after, policy, HighlightPolicy::Words)
            .fragments;
    check_line_consistency(&fragments, before, after, context);
    let fragments = process_blocks(fragments, before, after, policy, squash, trim);
    check_line_consistency(&fragments, before, after, context);
    if let Some(changes) = changes {
        check_line_changes(&fragments, changes, context);
    }
}

fn process_blocks(
    fragments: Vec<LineFragment>,
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    squash: bool,
    trim: bool,
) -> Vec<LineFragment> {
    if !squash && !trim {
        return fragments;
    }
    let mut result = Vec::new();
    let mut group: Vec<LineFragment> = Vec::new();
    for fragment in fragments {
        if let Some(previous) = group.last() {
            let adjoining = previous.lines.end1 == fragment.lines.start1
                && previous.lines.end2 == fragment.lines.start2
                && previous.offsets.end1 == fragment.offsets.start1
                && previous.offsets.end2 == fragment.offsets.start2;
            if !adjoining {
                result.extend(trim_ignored_edges(
                    std::mem::take(&mut group),
                    text1,
                    text2,
                    policy,
                    trim,
                ));
            }
        }
        group.push(fragment);
    }
    if !group.is_empty() {
        result.extend(trim_ignored_edges(group, text1, text2, policy, trim));
    }
    if squash {
        manager::squash(result)
    } else {
        result
    }
}

fn trim_ignored_edges(
    fragments: Vec<LineFragment>,
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    trim: bool,
) -> Vec<LineFragment> {
    if !trim || policy != ComparisonPolicy::IgnoreWhitespaces {
        return fragments;
    }
    let significant = |fragment: &LineFragment| {
        let sequence1 = &text1[fragment.offsets.start1..fragment.offsets.end1];
        let sequence2 = &text2[fragment.offsets.start2..fragment.offsets.end2];
        fragment
            .inner
            .as_ref()
            .is_none_or(|inner| !inner.is_empty())
            && !is_equals(sequence1, sequence2, ComparisonPolicy::IgnoreWhitespaces)
    };
    let mut start = 0;
    let mut end = fragments.len();
    while start < end && !significant(&fragments[start]) {
        start += 1;
    }
    while start < end && !significant(&fragments[end - 1]) {
        end -= 1;
    }
    fragments[start..end].to_vec()
}

fn parse_char_matching(matching: &str, text: &str) -> Vec<bool> {
    let pattern: Vec<char> = matching.chars().filter(|c| *c != '.').collect();
    let units: usize = text.chars().map(|c| c.len_utf16()).sum();
    assert_eq!(
        pattern.len(),
        units,
        "matching {matching:?} does not cover {text:?}"
    );
    let mut flags = vec![false; text.len()];
    let mut unit = 0;
    for (offset, c) in text.char_indices() {
        let width = c.len_utf16();
        let marked = pattern[unit..unit + width].iter().any(|p| *p != ' ');
        for flag in &mut flags[offset..offset + c.len_utf8()] {
            *flag = marked;
        }
        unit += width;
    }
    flags
}

fn parse_line_matching(matching: &str, text: &str) -> Vec<bool> {
    assert_eq!(
        matching.len(),
        text.len(),
        "matching {matching:?} vs {text:?}"
    );
    let pattern_lines: Vec<&str> = matching.split(['_', '*']).collect();
    let text_lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(pattern_lines.len(), text_lines.len());
    for (index, (pattern, line)) in pattern_lines.iter().zip(&text_lines).enumerate() {
        assert_eq!(pattern.len(), line.len(), "line {index}");
    }
    let mut flags = Vec::new();
    let mut index = 0;
    let bytes = matching.as_bytes();
    while index < bytes.len() {
        let end = bytes[index..]
            .iter()
            .position(|b| *b == b'_' || *b == b'*')
            .map_or(bytes.len(), |position| index + position + 1);
        let segment = &matching[index..end];
        let marked = segment.chars().any(|c| c != ' ' && c != '_');
        if marked {
            assert!(!segment.contains(' '));
        }
        flags.push(marked);
        index = end;
    }
    if matching.ends_with(['_', '*']) {
        flags.push(false);
    }
    while flags.len() < text_lines.len() {
        flags.push(false);
    }
    flags.truncate(text_lines.len());
    flags
}

fn check_line_matching(
    fragments: &[LineFragment],
    expected: &Matching,
    before: &str,
    after: &str,
    context: &str,
) {
    let mut actual1 = vec![false; expected.0.len()];
    let mut actual2 = vec![false; expected.1.len()];
    for fragment in fragments {
        for flag in &mut actual1[fragment.lines.start1..fragment.lines.end1] {
            *flag = true;
        }
        for flag in &mut actual2[fragment.lines.start2..fragment.lines.end2] {
            *flag = true;
        }
    }
    assert_sets_equal(
        &expected.0,
        &actual1,
        &render_line_pattern(&expected.0, before),
        &render_line_pattern(&actual1, before),
        "Before",
        context,
    );
    assert_sets_equal(
        &expected.1,
        &actual2,
        &render_line_pattern(&expected.1, after),
        &render_line_pattern(&actual2, after),
        "After",
        context,
    );
}

fn check_diff_matching(
    fragments: &[DiffFragment],
    expected: &Matching,
    before: &str,
    after: &str,
    context: &str,
) {
    let mut actual1 = vec![false; before.len()];
    let mut actual2 = vec![false; after.len()];
    for fragment in fragments {
        for flag in &mut actual1[fragment.start1..fragment.end1] {
            *flag = true;
        }
        for flag in &mut actual2[fragment.start2..fragment.end2] {
            *flag = true;
        }
    }
    assert_sets_equal(
        &expected.0,
        &actual1,
        &render_char_pattern(&expected.0, before),
        &render_char_pattern(&actual1, before),
        "Before",
        context,
    );
    assert_sets_equal(
        &expected.1,
        &actual2,
        &render_char_pattern(&expected.1, after),
        &render_char_pattern(&actual2, after),
        "After",
        context,
    );
}

fn assert_sets_equal(
    expected: &[bool],
    actual: &[bool],
    expected_pattern: &str,
    actual_pattern: &str,
    side: &str,
    context: &str,
) {
    assert!(
        expected == actual,
        "{side} matching differs ({context})\n  expected: \"{expected_pattern}\"\n  actual:   \"{actual_pattern}\""
    );
}

fn render_char_pattern(flags: &[bool], text: &str) -> String {
    let mut out = String::new();
    for (offset, c) in text.char_indices() {
        let mark = if flags[offset] { '-' } else { ' ' };
        for _ in 0..c.len_utf16() {
            out.push(mark);
        }
    }
    out
}

fn render_line_pattern(flags: &[bool], text: &str) -> String {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| {
            let mark = if flags.get(index).copied().unwrap_or(false) {
                '-'
            } else {
                ' '
            };
            std::iter::repeat_n(mark, line.len()).collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("_")
}

fn check_line_changes(fragments: &[LineFragment], expected: &[Range], context: &str) {
    let actual: Vec<Range> = fragments.iter().map(|fragment| fragment.lines).collect();
    assert_eq!(actual, expected, "line changes differ ({context})");
}

fn check_diff_changes(fragments: &[DiffFragment], expected: &[Range], context: &str) {
    let actual: Vec<Range> = fragments
        .iter()
        .map(|fragment| {
            Range::new(
                fragment.start1,
                fragment.end1,
                fragment.start2,
                fragment.end2,
            )
        })
        .collect();
    assert_eq!(actual, expected, "changes differ ({context})");
}

pub(crate) fn check_diff_consistency(fragments: &[DiffFragment], context: &str) {
    let mut last: Option<(usize, usize)> = None;
    for fragment in fragments {
        assert!(fragment.start1 <= fragment.end1, "{context}");
        assert!(fragment.start2 <= fragment.end2, "{context}");
        assert!(
            fragment.start1 != fragment.end1 || fragment.start2 != fragment.end2,
            "empty inner fragment ({context})"
        );
        if let Some((end1, end2)) = last {
            assert!(
                end1 <= fragment.start1 && end2 <= fragment.start2,
                "{context}"
            );
            assert!(
                end1 != fragment.start1 || end2 != fragment.start2,
                "adjacent inner fragments were not merged ({context})"
            );
        }
        last = Some((fragment.end1, fragment.end2));
    }
}

pub(crate) fn check_line_consistency(
    fragments: &[LineFragment],
    before: &str,
    after: &str,
    context: &str,
) {
    let offsets1 = LineOffsets::new(before);
    let offsets2 = LineOffsets::new(after);
    let mut last: Option<(usize, usize)> = None;
    for fragment in fragments {
        let lines = fragment.lines;
        let offsets = fragment.offsets;
        assert!(
            lines.start1 <= lines.end1 && lines.start2 <= lines.end2,
            "{context}"
        );
        assert!(
            lines.start1 != lines.end1 || lines.start2 != lines.end2,
            "empty line fragment ({context})"
        );
        assert!(lines.end1 <= offsets1.line_count(), "{context}");
        assert!(lines.end2 <= offsets2.line_count(), "{context}");
        assert!(
            offsets.start1 <= offsets.end1 && offsets.start2 <= offsets.end2,
            "{context}"
        );
        assert!(
            offsets.end1 <= before.len() && offsets.end2 <= after.len(),
            "{context}"
        );
        if let Some((end1, end2)) = last {
            assert!(end1 <= lines.start1 && end2 <= lines.start2, "{context}");
        }
        check_line_offsets(
            &offsets1,
            lines.start1,
            lines.end1,
            offsets.start1,
            offsets.end1,
            context,
        );
        check_line_offsets(
            &offsets2,
            lines.start2,
            lines.end2,
            offsets.start2,
            offsets.end2,
            context,
        );
        if let Some(inner) = &fragment.inner {
            check_diff_consistency(inner, context);
            for piece in inner {
                assert!(piece.end1 <= offsets.end1 - offsets.start1, "{context}");
                assert!(piece.end2 <= offsets.end2 - offsets.start2, "{context}");
            }
        }
        last = Some((lines.end1, lines.end2));
    }
}

fn check_line_offsets(
    line_offsets: &LineOffsets,
    start_line: usize,
    end_line: usize,
    start_offset: usize,
    end_offset: usize,
    context: &str,
) {
    if start_line != end_line {
        assert_eq!(
            line_offsets.line_start(start_line),
            start_offset,
            "{context}"
        );
        assert_eq!(
            line_offsets.line_end_with_newline(end_line - 1),
            end_offset,
            "{context}"
        );
    } else {
        let offset = if start_line == line_offsets.line_count() {
            line_offsets.line_end_with_newline(start_line - 1)
        } else {
            line_offsets.line_start(start_line)
        };
        assert_eq!(offset, start_offset, "{context}");
        assert_eq!(offset, end_offset, "{context}");
    }
}

#[test]
fn a_mismatching_fixture_reports_expected_and_actual_side_by_side() {
    let result = std::panic::catch_unwind(|| {
        lines_inner(|t| {
            t.text("x z", "y z");
            t.matching_default("-  ", "-  ");
            t.test_default();
        });
        lines(|t| {
            t.text("x_z", "y_z");
            t.matching_default("-_ ", "-_ ");
            t.test_default();
        });
        lines_inner(|t| {
            t.text("x z", "y z");
            t.matching_default("- -", "-  ");
            t.test_default();
        });
    });
    let message = result
        .err()
        .and_then(|payload| {
            payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        })
        .expect("the wrong matching must fail");
    assert!(message.contains("expected: \"- -\""), "{message}");
    assert!(message.contains("actual:   \"-  \""), "{message}");
}
