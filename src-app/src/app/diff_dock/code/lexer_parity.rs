use std::ops::Range;
use std::path::Path;

use paneflow_highlights::{Language, LexDocument, language_for_path};

use super::bench_corpus::lexer_fixture;

pub(crate) type LineTokens = Vec<(Range<usize>, u8)>;

pub(crate) const PARITY_CORPORA: &[(&str, &str, &str)] = &[
    ("Rust, the 300 KB bench corpus", "rs", "corpus.rs"),
    ("Markdown, the bench corpus", "md", "corpus.md"),
    (
        "TypeScript, scripts/generate-file-icons.ts",
        "ts",
        "generate-file-icons.ts",
    ),
    ("Python fixture", "py", "fixture.py"),
    ("HTML with script and style", "html", "fixture.html"),
    ("Zig, sample.zig of the parity corpus", "zig", "sample.zig"),
];

const RECORDED_WASMTIME_FINGERPRINTS: &[(&str, usize, usize, u64)] = &[
    (
        "Rust, the 300 KB bench corpus",
        9246,
        67175,
        0xa907_4b7f_cba0_473b,
    ),
    (
        "Markdown, the bench corpus",
        2733,
        7606,
        0x865c_aea3_d109_3f74,
    ),
    (
        "TypeScript, scripts/generate-file-icons.ts",
        202,
        1909,
        0x8deb_65c6_5d05_6c7d,
    ),
    ("Python fixture", 43, 244, 0xa191_b6b9_fe83_acaa),
    ("HTML with script and style", 26, 200, 0x917c_da70_7268_936d),
    (
        "Zig, sample.zig of the parity corpus",
        22,
        164,
        0x17f9_2b7b_9a34_3a18,
    ),
];

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Difference {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) native: u8,
    pub(crate) other: u8,
}

pub(crate) fn corpus_text(fixture: &str) -> String {
    if fixture == "zig" {
        return crate::diff::parity_tests::CORPUS
            .iter()
            .find(|(name, _)| *name == "sample.zig")
            .map(|(_, text)| (*text).to_owned())
            .unwrap_or_default();
    }
    lexer_fixture(fixture)
}

pub(crate) fn corpus_language(file: &str) -> Language {
    language_for_path(Path::new(file)).expect("every parity corpus has a lexer language")
}

pub(crate) fn native_lines(text: &str, language: Language) -> Vec<LineTokens> {
    let document = LexDocument::lex(text, language).expect("a lexed document");
    (0..document.line_count())
        .map(|line| {
            document
                .line_tokens(line)
                .expect("line tokens")
                .into_iter()
                .map(|token| (token.range, token.id.get()))
                .collect()
        })
        .collect()
}

fn id_at(tokens: &LineTokens, byte: usize) -> u8 {
    tokens
        .iter()
        .find(|(range, _)| range.contains(&byte))
        .map_or(0, |(_, id)| *id)
}

pub(crate) fn first_difference(native: &[LineTokens], other: &[LineTokens]) -> Option<Difference> {
    let lines = native.len().max(other.len());
    let empty = LineTokens::new();
    (0..lines).find_map(|line| {
        let ours = native.get(line).unwrap_or(&empty);
        let theirs = other.get(line).unwrap_or(&empty);
        if ours == theirs {
            return None;
        }
        let end = ours
            .iter()
            .chain(theirs)
            .map(|(range, _)| range.end)
            .max()
            .unwrap_or(0);
        let column = (0..end)
            .find(|byte| id_at(ours, *byte) != id_at(theirs, *byte))
            .or_else(|| {
                ours.iter()
                    .zip(theirs)
                    .find(|(a, b)| a != b)
                    .map(|(a, b)| a.0.start.min(b.0.start))
            })
            .unwrap_or_else(|| ours.len().min(theirs.len()));
        Some(Difference {
            line,
            column,
            native: id_at(ours, column),
            other: id_at(theirs, column),
        })
    })
}

pub(crate) fn fingerprint(lines: &[LineTokens]) -> (usize, usize, u64) {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |value: u64| {
        for byte in value.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    let mut tokens = 0;
    for (line, records) in lines.iter().enumerate() {
        for (range, id) in records {
            tokens += 1;
            mix(line as u64);
            mix(range.start as u64);
            mix(range.end as u64);
            mix(u64::from(*id));
        }
    }
    (lines.len(), tokens, hash)
}

#[test]
fn a_difference_names_the_first_differing_line_byte_column_and_both_token_ids() {
    let native = vec![vec![(0..3, 17), (3..8, 58)], vec![(0..2, 24), (2..6, 44)]];
    let mut other = native.clone();
    assert_eq!(first_difference(&native, &other), None);
    other[1][1] = (2..6, 4);
    assert_eq!(
        first_difference(&native, &other),
        Some(Difference {
            line: 1,
            column: 2,
            native: 44,
            other: 4,
        })
    );
    let mut split = native.clone();
    split[0] = vec![(0..1, 17), (1..3, 17), (3..8, 58)];
    assert_eq!(
        first_difference(&native, &split),
        Some(Difference {
            line: 0,
            column: 0,
            native: 17,
            other: 17,
        })
    );
}

#[test]
fn native_tokens_match_the_fingerprints_recorded_from_the_wasmtime_prototype() {
    assert_eq!(RECORDED_WASMTIME_FINGERPRINTS.len(), PARITY_CORPORA.len());
    for ((label, fixture, file), (recorded, lines, tokens, hash)) in
        PARITY_CORPORA.iter().zip(RECORDED_WASMTIME_FINGERPRINTS)
    {
        assert_eq!(label, recorded);
        let native = native_lines(&corpus_text(fixture), corpus_language(file));
        assert_eq!(
            fingerprint(&native),
            (*lines, *tokens, *hash),
            "{label}: the native tokens no longer match the wasmtime run recorded in this table"
        );
    }
}
