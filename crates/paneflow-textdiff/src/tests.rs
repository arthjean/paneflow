#![allow(
    clippy::panic,
    clippy::unwrap_used,
    reason = "story acceptance tests want short, explicit failure sites"
)]

use std::time::{Duration, Instant};

use crate::manager::{squash, MAX_BAD_LINES};
use crate::{
    compare_chars, compare_lines, compare_lines_inner, compare_words, split_lines,
    ComparisonPolicy, DiffFragment, DiffTooBig, HighlightPolicy, LineFragment, Range,
};

pub(crate) fn budget(local: Duration) -> Duration {
    if std::env::var_os("CI").is_some() {
        local * 4
    } else {
        local
    }
}

const POLICIES: [ComparisonPolicy; 3] = [
    ComparisonPolicy::Default,
    ComparisonPolicy::TrimWhitespaces,
    ComparisonPolicy::IgnoreWhitespaces,
];

fn assert_fair(fragments: &[DiffFragment], text1: &str, text2: &str) {
    let mut last = (0, 0);
    for fragment in fragments {
        assert!(fragment.start1 <= fragment.end1 && fragment.start2 <= fragment.end2);
        assert!(fragment.start1 != fragment.end1 || fragment.start2 != fragment.end2);
        assert!(last.0 <= fragment.start1 && last.1 <= fragment.start2);
        assert!(text1.is_char_boundary(fragment.start1) && text1.is_char_boundary(fragment.end1));
        assert!(text2.is_char_boundary(fragment.start2) && text2.is_char_boundary(fragment.end2));
        last = (fragment.end1, fragment.end2);
    }
    assert!(last.0 <= text1.len() && last.1 <= text2.len());
}

#[test]
fn empty_texts_produce_no_fragments() {
    assert!(compare_lines(&[], &[], ComparisonPolicy::Default).is_empty());
    for policy in POLICIES {
        for highlight in [
            HighlightPolicy::Words,
            HighlightPolicy::Lines,
            HighlightPolicy::None,
        ] {
            assert!(compare_lines_inner("", "", policy, highlight).is_empty());
        }
        assert_eq!(compare_words("", "", policy), Ok(Vec::new()));
        assert_eq!(compare_chars("", "", policy), Ok(Vec::new()));
    }
}

#[test]
fn one_empty_side_is_a_single_range() {
    let lines = ["fn main() {", "}"];
    assert_eq!(
        compare_lines(&lines, &[], ComparisonPolicy::Default),
        vec![Range::new(0, 2, 0, 0)]
    );
    assert_eq!(
        compare_lines(&[], &lines, ComparisonPolicy::IgnoreWhitespaces),
        vec![Range::new(0, 0, 0, 2)]
    );
    let fragments = compare_lines_inner(
        "",
        "a b\n",
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].lines, Range::new(0, 0, 0, 1));
    assert_eq!(fragments[0].offsets, Range::new(0, 0, 0, 4));
    assert_eq!(fragments[0].inner, None);
    assert_eq!(
        compare_words("", "a b", ComparisonPolicy::Default),
        Ok(vec![DiffFragment {
            start1: 0,
            end1: 0,
            start2: 0,
            end2: 3
        }])
    );
}

#[test]
fn a_closing_brace_stays_attached_to_the_inserted_block() {
    let lines1 = ["{", "}"];
    let lines2 = ["{", " {", " }", "}", "x"];
    assert_eq!(
        compare_lines(&lines1, &lines2, ComparisonPolicy::Default),
        vec![Range::new(1, 1, 1, 3), Range::new(2, 2, 4, 5)]
    );
}

#[test]
fn short_lines_are_matched_in_the_gaps_between_important_lines() {
    let lines1 = ["}", "}", "}", "important line"];
    let lines2 = ["important line", "}", "}", "}"];
    assert_eq!(
        compare_lines(&lines1, &lines2, ComparisonPolicy::Default),
        vec![Range::new(0, 3, 0, 0), Range::new(4, 4, 1, 4)]
    );
}

#[test]
fn ten_thousand_different_lines_compare_under_two_hundred_milliseconds() {
    let text1: Vec<String> = (0..10_000).map(|index| format!("left {index}")).collect();
    let text2: Vec<String> = (0..10_000).map(|index| format!("right {index}")).collect();
    let lines1: Vec<&str> = text1.iter().map(String::as_str).collect();
    let lines2: Vec<&str> = text2.iter().map(String::as_str).collect();
    let started = Instant::now();
    let ranges = compare_lines(&lines1, &lines2, ComparisonPolicy::Default);
    let elapsed = started.elapsed();
    assert_eq!(ranges, vec![Range::new(0, 10_000, 0, 10_000)]);
    assert!(
        elapsed < budget(Duration::from_millis(200)),
        "took {elapsed:?}"
    );
}

#[test]
fn carriage_returns_are_whitespace_and_the_last_line_counts() {
    let crlf = split_lines("a\r\nb\r\n");
    let lf = split_lines("a\nb\n");
    assert_eq!(crlf, vec!["a\r", "b\r", ""]);
    assert_eq!(
        compare_lines(&crlf, &lf, ComparisonPolicy::Default),
        vec![Range::new(0, 2, 0, 2)]
    );
    assert!(compare_lines(&crlf, &lf, ComparisonPolicy::TrimWhitespaces).is_empty());
    assert!(compare_lines(&crlf, &lf, ComparisonPolicy::IgnoreWhitespaces).is_empty());
    assert!(compare_lines_inner(
        "a\r\nb\r\n",
        "a\nb\n",
        ComparisonPolicy::TrimWhitespaces,
        HighlightPolicy::Words
    )
    .is_empty());

    let terminated = split_lines("a\nb\n");
    let unterminated = split_lines("a\nb");
    assert_eq!(unterminated.len(), 2);
    assert_eq!(
        compare_lines(&terminated, &unterminated, ComparisonPolicy::Default),
        vec![Range::new(2, 3, 2, 2)]
    );
    let fragments = compare_lines_inner(
        "a b\r\n",
        "a c\r\n",
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(
        fragments[0].inner,
        Some(vec![DiffFragment {
            start1: 2,
            end1: 3,
            start2: 2,
            end2: 3
        }])
    );
}

#[test]
fn a_side_over_twenty_thousand_chunks_is_too_big() {
    let big: String = std::iter::repeat_n("w ", 20_001).collect();
    let limit: String = std::iter::repeat_n("w ", 20_000).collect();
    for policy in POLICIES {
        assert_eq!(compare_words(&big, "x", policy), Err(DiffTooBig));
        assert_eq!(compare_words("x", &big, policy), Err(DiffTooBig));
        assert!(compare_words(&limit, "x", policy).is_ok());
        assert_eq!(compare_chars(&big, "x", policy), Err(DiffTooBig));
    }
    let started = Instant::now();
    for _ in 0..100 {
        assert_eq!(
            compare_words(&big, "x", ComparisonPolicy::Default),
            Err(DiffTooBig)
        );
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed < budget(Duration::from_millis(200)),
        "the guard must fail fast, took {elapsed:?}"
    );
}

#[test]
fn whitespace_only_and_punctuation_only_texts_tokenize() {
    for policy in POLICIES {
        let fragments = compare_words("   ", "\t\t", policy).unwrap();
        assert_fair(&fragments, "   ", "\t\t");
        if policy == ComparisonPolicy::Default {
            assert_eq!(fragments.len(), 1);
        } else {
            assert!(fragments.is_empty(), "{policy:?}");
        }
        let fragments = compare_words("...", ",,,", policy).unwrap();
        assert_fair(&fragments, "...", ",,,");
        assert_eq!(fragments.len(), 1);
        let fragments = compare_words("a.b", "a,b", policy).unwrap();
        assert_eq!(
            fragments,
            vec![DiffFragment {
                start1: 1,
                end1: 2,
                start2: 1,
                end2: 2
            }]
        );
    }
}

#[test]
fn ten_thousand_cjk_chars_compare_under_fifty_milliseconds() {
    let text1: String = (0..10_000u32)
        .map(|index| char::from_u32(0x4E00 + index % 0x1000).unwrap())
        .collect();
    let mut chars: Vec<char> = text1.chars().collect();
    chars[5_000] = '漢';
    chars.insert(2_500, '字');
    let text2: String = chars.into_iter().collect();
    let started = Instant::now();
    let fragments = compare_words(&text1, &text2, ComparisonPolicy::Default).unwrap();
    let elapsed = started.elapsed();
    assert_fair(&fragments, &text1, &text2);
    assert_eq!(fragments.len(), 2);
    assert!(
        elapsed < budget(Duration::from_millis(50)),
        "took {elapsed:?}"
    );
}

#[test]
fn ignore_whitespaces_matches_the_space_before_an_inserted_char() {
    for policy in POLICIES {
        assert_eq!(
            compare_chars("x y", "x zy", policy),
            Ok(vec![DiffFragment {
                start1: 2,
                end1: 2,
                start2: 2,
                end2: 3
            }]),
            "{policy:?}"
        );
    }
    assert_eq!(
        compare_words("x y", "x zy", ComparisonPolicy::Default),
        Ok(vec![DiffFragment {
            start1: 2,
            end1: 3,
            start2: 2,
            end2: 4
        }])
    );
}

#[test]
fn punctuation_only_lines_stay_fair() {
    for policy in POLICIES {
        let fragments = compare_chars("...;;", ",,,;;", policy).unwrap();
        assert_fair(&fragments, "...;;", ",,,;;");
        assert_eq!(
            fragments,
            vec![DiffFragment {
                start1: 0,
                end1: 3,
                start2: 0,
                end2: 3
            }]
        );
        let text1 = "{\n...\n}\n";
        let text2 = "{\n,,,\n}\n";
        let fragments = compare_lines_inner(text1, text2, policy, HighlightPolicy::Words);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].lines, Range::new(1, 2, 1, 2));
        let inner = fragments[0].inner.as_ref().unwrap();
        assert_fair(
            inner,
            &text1[fragments[0].offsets.start1..fragments[0].offsets.end1],
            &text2[fragments[0].offsets.start2..fragments[0].offsets.end2],
        );
        assert_eq!(inner.len(), 1);
    }
}

fn too_big_line(word: &str) -> String {
    std::iter::repeat_n(format!("{word} "), 20_001).collect()
}

#[test]
fn three_too_big_blocks_disable_inner_fragments_for_the_rest() {
    let separator = "same\n";
    let big1 = too_big_line("a");
    let big2 = too_big_line("b");
    let text1 = format!("{big1}\n{separator}{big1}\n{separator}{big1}\n{separator}small x\n");
    let text2 = format!("{big2}\n{separator}{big2}\n{separator}{big2}\n{separator}small y\n");
    let fragments = compare_lines_inner(
        &text1,
        &text2,
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), MAX_BAD_LINES + 1);
    assert!(fragments.iter().all(|fragment| fragment.inner.is_none()));

    let text1 = format!("{big1}\n{separator}{big1}\n{separator}small x\n");
    let text2 = format!("{big2}\n{separator}{big2}\n{separator}small y\n");
    let fragments = compare_lines_inner(
        &text1,
        &text2,
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), MAX_BAD_LINES);
    assert!(fragments[0].inner.is_none() && fragments[1].inner.is_none());
    assert_eq!(
        fragments[2].inner,
        Some(vec![DiffFragment {
            start1: 6,
            end1: 7,
            start2: 6,
            end2: 7
        }])
    );
}

#[test]
fn squash_merges_adjoining_fragments_and_shifts_inner_offsets() {
    let deletion = LineFragment {
        lines: Range::new(0, 1, 0, 0),
        offsets: Range::new(0, 4, 0, 0),
        inner: None,
    };
    let modification = LineFragment {
        lines: Range::new(1, 2, 0, 1),
        offsets: Range::new(4, 8, 0, 4),
        inner: Some(vec![DiffFragment {
            start1: 1,
            end1: 2,
            start2: 1,
            end2: 2,
        }]),
    };
    let apart = LineFragment {
        lines: Range::new(5, 6, 4, 5),
        offsets: Range::new(20, 24, 16, 20),
        inner: Some(vec![]),
    };
    let squashed = squash(vec![deletion, modification, apart.clone()]);
    assert_eq!(squashed.len(), 2);
    assert_eq!(squashed[0].lines, Range::new(0, 2, 0, 1));
    assert_eq!(squashed[0].offsets, Range::new(0, 8, 0, 4));
    assert_eq!(
        squashed[0].inner,
        Some(vec![
            DiffFragment {
                start1: 0,
                end1: 4,
                start2: 0,
                end2: 0
            },
            DiffFragment {
                start1: 5,
                end1: 6,
                start2: 1,
                end2: 2
            }
        ])
    );
    assert_eq!(squashed[1], apart);

    let fragments = compare_lines_inner(
        "a\nb c\n",
        "b d\n",
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].lines, Range::new(0, 2, 0, 1));
    assert_eq!(
        fragments[0].inner,
        Some(vec![
            DiffFragment {
                start1: 0,
                end1: 2,
                start2: 0,
                end2: 0
            },
            DiffFragment {
                start1: 4,
                end1: 5,
                start2: 2,
                end2: 3
            }
        ])
    );
}

#[test]
fn lines_and_none_highlighting_skip_the_word_pass() {
    for highlight in [HighlightPolicy::Lines, HighlightPolicy::None] {
        let fragments = compare_lines_inner("a b\n", "a c\n", ComparisonPolicy::Default, highlight);
        assert_eq!(fragments.len(), 1);
        assert_eq!(fragments[0].inner, None);
    }
}

#[test]
fn identical_texts_short_circuit() {
    let text: String = (0..50_000).map(|index| format!("line {index}\n")).collect();
    let started = Instant::now();
    assert!(compare_lines_inner(
        &text,
        &text,
        ComparisonPolicy::Default,
        HighlightPolicy::Words
    )
    .is_empty());
    let lines = split_lines(&text);
    assert!(compare_lines(&lines, &lines, ComparisonPolicy::IgnoreWhitespaces).is_empty());
    let elapsed = started.elapsed();
    assert!(
        elapsed < budget(Duration::from_millis(100)),
        "took {elapsed:?}"
    );
}

#[test]
fn an_insertion_equal_under_the_policy_gets_empty_inner_fragments() {
    let fragments = compare_lines_inner(
        "a\nb\n",
        "a\n \nb\n",
        ComparisonPolicy::IgnoreWhitespaces,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].lines, Range::new(1, 1, 1, 2));
    assert_eq!(fragments[0].inner, Some(vec![]));
    let fragments = compare_lines_inner(
        "a\nb\n",
        "a\n \nb\n",
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].inner, None);
    let fragments = compare_lines_inner(
        "a\nb\n",
        "a\n",
        ComparisonPolicy::Default,
        HighlightPolicy::Words,
    );
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].inner, None);
}
