use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::{Hsla, hsla};
use paneflow_highlights::language_for_path;
use serde::Deserialize;

use super::document::CodeDocument;
use super::first_frame::override_policy_for_test;
use super::highlight::{CodeHighlighter, LineRuns};
use crate::diff::parity_tests::CORPUS;
use crate::diff::{DiffSyntax, grammar_for_ext};
use crate::theme::SyntaxPalette;

const PLAIN: &str = "plain";

const PAIRED_LANGUAGES: usize = 19;

pub(crate) const SWAP_ROWS: usize = 60;

const ROLE_NAMES: [&str; 29] = [
    "comment",
    "comment.doc",
    "keyword",
    "function",
    "type",
    "constructor",
    "string",
    "string.escape",
    "string.special",
    "number",
    "boolean",
    "constant",
    "constant.builtin",
    "property",
    "variable",
    "variable.builtin",
    "operator",
    "punctuation",
    "punctuation.special",
    "attribute",
    "tag",
    "label",
    "namespace",
    "title",
    "text.literal",
    "link_uri",
    "link_text",
    "emphasis",
    "emphasis.strong",
];

fn role_color(index: usize) -> Hsla {
    hsla(index as f32 / ROLE_NAMES.len() as f32, 0.5, 0.5, 1.0)
}

pub(crate) fn role_syntax() -> DiffSyntax {
    let c = role_color;
    DiffSyntax::from_palette(SyntaxPalette {
        comment: c(0),
        comment_doc: c(1),
        keyword: c(2),
        function: c(3),
        r#type: c(4),
        constructor: c(5),
        string: c(6),
        string_escape: c(7),
        string_special: c(8),
        number: c(9),
        boolean: c(10),
        constant: c(11),
        constant_builtin: c(12),
        property: c(13),
        variable: c(14),
        variable_builtin: c(15),
        operator: c(16),
        punctuation: c(17),
        punctuation_special: c(18),
        attribute: c(19),
        tag: c(20),
        label: c(21),
        namespace: c(22),
        title: c(23),
        text_literal: c(24),
        link_uri: c(25),
        link_text: c(26),
        emphasis: c(27),
        emphasis_strong: c(28),
    })
}

fn role_of(color: Hsla) -> &'static str {
    (0..ROLE_NAMES.len())
        .find(|index| role_color(*index) == color)
        .map_or("unknown", |index| ROLE_NAMES[index])
}

fn row_roles(runs: &LineRuns, len: usize) -> Vec<&'static str> {
    let mut roles = vec![PLAIN; len];
    for (range, color) in runs {
        let end = range.end.min(len);
        if range.start < end {
            roles[range.start..end].fill(role_of(*color));
        }
    }
    roles
}

#[derive(Clone, Copy)]
pub(crate) struct Pair {
    pub(crate) ext: &'static str,
    pub(crate) sample: &'static str,
    pub(crate) lexer: &'static str,
    pub(crate) text: &'static str,
}

impl Pair {
    pub(crate) fn document(&self) -> CodeDocument {
        CodeDocument::new(PathBuf::from(self.sample), self.text)
    }
}

pub(crate) fn paired_samples() -> Vec<Pair> {
    CORPUS
        .iter()
        .filter_map(|(sample, text)| {
            let ext = Path::new(sample).extension()?.to_str()?;
            grammar_for_ext(ext)?;
            let lexer = language_for_path(Path::new(sample))
                .unwrap_or_else(|| panic!("{sample} has a grammar but no lexer language"));
            Some(Pair {
                ext,
                sample,
                lexer: lexer.name(),
                text,
            })
        })
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct Agreement {
    compared: usize,
    agreed: usize,
    disagreeing: BTreeMap<&'static str, usize>,
}

impl Agreement {
    fn percent(&self) -> f64 {
        if self.compared == 0 {
            return 100.0;
        }
        self.agreed as f64 * 100.0 / self.compared as f64
    }

    fn tenths(&self) -> u64 {
        if self.compared == 0 {
            return 1000;
        }
        (self.agreed as u64 * 1000) / self.compared as u64
    }

    fn meets(&self, floor_tenths: u64) -> bool {
        self.agreed as u64 * 1000 >= floor_tenths * self.compared as u64
    }

    fn worst_roles(&self) -> String {
        let mut roles: Vec<_> = self.disagreeing.iter().collect();
        roles.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        roles
            .iter()
            .take(3)
            .map(|(role, bytes)| format!("{role} {bytes} B"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn outside(ranges: &[Range<usize>], byte: usize) -> bool {
    !ranges.iter().any(|range| range.contains(&byte))
}

pub(crate) fn agreement(pair: &Pair) -> Agreement {
    let doc = pair.document();
    let mut lexical = CodeHighlighter::new(&doc, role_syntax());
    assert!(
        lexical.has_lexical(),
        "{} opened without layer 1",
        pair.sample
    );
    lexical.settle_lexical_for_test(&doc);
    let mut tree = CodeHighlighter::layer2_only(&doc, role_syntax());
    assert!(
        tree.parse_initial_blocking(&doc),
        "{} produced no layer-2 tree",
        pair.sample
    );
    tree.requery_rows(&doc, 0..doc.line_count());

    let mut out = Agreement::default();
    for row in 0..doc.line_count() {
        let Some(line) = doc.line_string(row) else {
            continue;
        };
        let lexed = row_roles(&lexical.lexical_runs(row), line.len());
        let shown = row_roles(&tree.runs(row), line.len());
        let nested = tree.nested_ranges(row);
        for (byte, value) in line.bytes().enumerate() {
            if value.is_ascii_whitespace() || !outside(&nested, byte) {
                continue;
            }
            if lexed[byte] == PLAIN && shown[byte] == PLAIN {
                continue;
            }
            out.compared += 1;
            if lexed[byte] == shown[byte] {
                out.agreed += 1;
            } else {
                *out.disagreeing.entry(shown[byte]).or_default() += 1;
            }
        }
    }
    out
}

#[derive(Deserialize)]
struct FloorsFile {
    floors: BTreeMap<String, f64>,
}

const FLOORS_TOML: &str = include_str!("oracle-floors.toml");

fn floors_in_tenths(source: &str) -> BTreeMap<String, u64> {
    let file: FloorsFile = toml::from_str(source).expect("oracle-floors.toml parses");
    file.floors
        .into_iter()
        .map(|(language, floor)| {
            assert!(
                (0.0..=100.0).contains(&floor),
                "{language}: floor {floor} is not a percentage"
            );
            (language, (floor * 10.0).round() as u64)
        })
        .collect()
}

fn floor_failures(measured: &[(Pair, Agreement)], floors: &BTreeMap<String, u64>) -> Vec<String> {
    let mut failures = Vec::new();
    for (pair, agreement) in measured {
        let Some(&floor) = floors.get(pair.ext) else {
            failures.push(format!(
                "{} (lexer {}, {}): no floor in oracle-floors.toml",
                pair.ext, pair.lexer, pair.sample
            ));
            continue;
        };
        if !agreement.meets(floor) {
            failures.push(format!(
                "{} (lexer {}, {}): agreement {:.2} % is below its floor {:.1} %; most disagreeing roles: {}",
                pair.ext,
                pair.lexer,
                pair.sample,
                agreement.percent(),
                floor as f64 / 10.0,
                agreement.worst_roles(),
            ));
        }
    }
    for language in floors.keys() {
        if !measured.iter().any(|(pair, _)| pair.ext == language) {
            failures.push(format!(
                "{language}: oracle-floors.toml has a floor for a language the oracle does not pair"
            ));
        }
    }
    failures
}

fn measure_all() -> Vec<(Pair, Agreement)> {
    paired_samples()
        .into_iter()
        .map(|pair| {
            let agreement = agreement(&pair);
            (pair, agreement)
        })
        .collect()
}

#[derive(Debug, Default)]
pub(crate) struct Swap {
    colored: usize,
    changed: usize,
    transitions: BTreeMap<(&'static str, &'static str), usize>,
}

impl Swap {
    fn percent(&self) -> f64 {
        if self.colored == 0 {
            return 0.0;
        }
        self.changed as f64 * 100.0 / self.colored as f64
    }

    fn top_transitions(&self) -> String {
        let mut transitions: Vec<_> = self.transitions.iter().collect();
        transitions.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        transitions
            .iter()
            .take(3)
            .map(|((from, to), bytes)| format!("{from} to {to} {bytes} B"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub(crate) fn swap(pair: &Pair) -> Swap {
    let _lexical = override_policy_for_test(&[]);
    let doc = pair.document();
    let mut highlighter = CodeHighlighter::new(&doc, role_syntax());
    let rows = 0..doc.line_count().min(SWAP_ROWS);
    let before: Vec<LineRuns> = rows.clone().map(|row| highlighter.runs(row)).collect();
    assert!(
        highlighter.parse_initial_blocking(&doc),
        "{} produced no layer-2 tree",
        pair.sample
    );
    highlighter.requery_rows(&doc, rows.clone());

    let mut out = Swap::default();
    for (row, before) in rows.zip(before) {
        let Some(line) = doc.line_string(row) else {
            continue;
        };
        let first = row_roles(&before, line.len());
        let swapped = row_roles(&highlighter.runs(row), line.len());
        for (byte, value) in line.bytes().enumerate() {
            if value.is_ascii_whitespace() || (first[byte] == PLAIN && swapped[byte] == PLAIN) {
                continue;
            }
            out.colored += 1;
            if first[byte] != swapped[byte] {
                out.changed += 1;
                *out.transitions
                    .entry((first[byte], swapped[byte]))
                    .or_default() += 1;
            }
        }
    }
    out
}

#[test]
fn the_swap_report_covers_every_paired_language() {
    let mut out = format!(
        "| language | lexer | colored bytes (first {SWAP_ROWS} rows) | role changes at the swap | share | largest changes |\n|---|---|---|---|---|---|\n"
    );
    let pairs = paired_samples();
    assert_eq!(pairs.len(), PAIRED_LANGUAGES);
    for pair in &pairs {
        let swap = swap(pair);
        assert!(swap.colored > 0, "{} has no colored byte", pair.sample);
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.1} % | {} |",
            pair.ext,
            pair.lexer,
            swap.colored,
            swap.changed,
            swap.percent(),
            swap.top_transitions(),
        );
    }
    println!("{out}");
}

#[test]
fn the_swap_measures_layer_1_against_layer_2_and_not_against_itself() {
    let pair = paired_samples()
        .into_iter()
        .find(|pair| pair.ext == "rs")
        .expect("the Rust sample");
    let swap = swap(&pair);
    assert!(swap.changed > 0, "Rust changes some bytes at the swap");
    assert!(swap.changed < swap.colored);
    let markdown = paired_samples()
        .into_iter()
        .find(|pair| pair.ext == "md")
        .expect("the Markdown sample");
    let fence = swap_on_rows(&markdown, "fn main() {}");
    assert_eq!(fence, 0, "the fence body stays on layer 1 through the swap");
}

fn swap_on_rows(pair: &Pair, row_text: &str) -> usize {
    let _lexical = override_policy_for_test(&[]);
    let doc = pair.document();
    let row = (0..doc.line_count())
        .find(|row| doc.line_string(*row).as_deref() == Some(row_text))
        .expect("the row");
    let mut highlighter = CodeHighlighter::new(&doc, role_syntax());
    let before = highlighter.runs(row);
    assert!(!before.is_empty());
    assert!(highlighter.parse_initial_blocking(&doc));
    highlighter.requery_rows(&doc, 0..doc.line_count());
    assert!(highlighter.row_has_nested_region(row));
    let line = doc.line_string(row).expect("the row");
    let first = row_roles(&before, line.len());
    let swapped = row_roles(&highlighter.runs(row), line.len());
    first.iter().zip(&swapped).filter(|(a, b)| a != b).count()
}

#[test]
fn every_role_resolves_to_its_own_color() {
    let syntax = role_syntax();
    for (index, name) in ROLE_NAMES.iter().enumerate() {
        assert_eq!(
            syntax.color_for_capture(name),
            Some(role_color(index)),
            "{name}"
        );
        assert_eq!(role_of(role_color(index)), *name);
    }
}

#[test]
fn the_oracle_pairs_every_grammar_of_the_corpus_through_the_editor_lookup() {
    let pairs = paired_samples();
    assert_eq!(pairs.len(), PAIRED_LANGUAGES);
    for pair in &pairs {
        let doc = pair.document();
        assert_eq!(
            language_for_path(doc.path()).map(|language| language.name()),
            Some(pair.lexer),
            "{}",
            pair.sample
        );
        assert!(grammar_for_ext(doc.ext()).is_some(), "{}", pair.sample);
    }
}

#[test]
fn every_paired_language_meets_its_agreement_floor() {
    let measured = measure_all();
    assert_eq!(measured.len(), PAIRED_LANGUAGES);
    let failures = floor_failures(&measured, &floors_in_tenths(FLOORS_TOML));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_language_below_its_floor_fails_with_its_floor_measure_and_worst_roles() {
    let pair = paired_samples()
        .into_iter()
        .find(|pair| pair.ext == "rs")
        .expect("the Rust sample");
    let measured = agreement(&pair);
    assert!(measured.tenths() < 1000, "Rust must disagree somewhere");
    let floors = BTreeMap::from([("rs".to_owned(), 1000)]);
    let failures = floor_failures(&[(pair, measured)], &floors);
    assert_eq!(failures.len(), 1, "{failures:?}");
    let failure = &failures[0];
    assert!(
        failure.starts_with("rs (lexer rs, sample.rs): agreement "),
        "{failure}"
    );
    assert!(failure.contains("below its floor 100.0 %"), "{failure}");
    let worst = failure
        .split("most disagreeing roles: ")
        .nth(1)
        .expect("the worst roles");
    assert!((1..=3).contains(&worst.split(", ").count()), "{failure}");
    assert!(worst.contains(" B"), "{failure}");
}

#[test]
fn a_paired_language_absent_from_the_floors_file_fails_naming_it() {
    let pair = paired_samples()
        .into_iter()
        .find(|pair| pair.ext == "zig")
        .expect("the Zig sample");
    let measured = agreement(&pair);
    let failures = floor_failures(&[(pair, measured)], &BTreeMap::new());
    assert_eq!(
        failures,
        vec!["zig (lexer zig, sample.zig): no floor in oracle-floors.toml".to_owned()]
    );
    let stale = floor_failures(&[], &BTreeMap::from([("kotlin".to_owned(), 500)]));
    assert_eq!(stale.len(), 1);
    assert!(stale[0].starts_with("kotlin: "), "{stale:?}");
}

#[test]
#[ignore = "oracle floors report: cargo test -p paneflow-app --bin paneflow app::diff_dock::code::oracle::oracle_floor_report -- --ignored --exact --nocapture"]
fn oracle_floor_report() {
    let mut out = String::from(
        "| language | lexer | sample | compared bytes | agreement | most disagreeing roles |\n|---|---|---|---|---|---|\n",
    );
    let mut floors = String::from("[floors]\n");
    for (pair, agreement) in measure_all() {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} | {:.2} % | {} |",
            pair.ext,
            pair.lexer,
            pair.sample,
            agreement.compared,
            agreement.percent(),
            agreement.worst_roles(),
        );
        let tenths = agreement.tenths();
        let _ = writeln!(floors, "{} = {}.{}", pair.ext, tenths / 10, tenths % 10);
    }
    println!("{out}\n{floors}");
}
