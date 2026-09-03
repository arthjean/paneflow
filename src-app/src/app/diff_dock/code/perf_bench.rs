use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

use gpui::{Hsla, hsla};

use crate::bench_harness::{
    Metric, SegmentTimer, live_bytes, measure, measure_segments, process_cpu_time, publish,
    refuse_debug_profile,
};
use crate::diff::{DiffSyntax, resolve_runs};

use super::bench_corpus::{
    EDITOR_CORPUS_SEED, HIGHLIGHTED_RUST_BYTES, LARGE_RUST_BYTES, MINIFIED_JSON_CHARS,
    RELOAD_RUST_BYTES, json_token_ranges, markdown_source, minified_json_line, rust_source,
};
use super::cursor::CodeSelection;
use super::document::CodeDocument;
use super::edit::{EditGroup, UndoHistory, splice};
use super::highlight::{CodeHighlighter, HighlightOutcome};

const VIEWPORT_ROWS: usize = 60;

const RESOLVE_RUNS_CAPTURES: usize = 3_750;

const RELOADS: usize = 200;

struct Corpora {
    highlighted_rust: String,
    large_rust: String,
    reload_rust: String,
    minified_json: String,
    markdown: String,
}

impl Corpora {
    fn build() -> Self {
        Self {
            highlighted_rust: rust_source(HIGHLIGHTED_RUST_BYTES),
            large_rust: rust_source(LARGE_RUST_BYTES),
            reload_rust: rust_source(RELOAD_RUST_BYTES),
            minified_json: minified_json_line(MINIFIED_JSON_CHARS),
            markdown: markdown_source(64_000),
        }
    }
}

fn dark() -> DiffSyntax {
    DiffSyntax::from_theme(&crate::theme::paneflow_dark())
}

fn light() -> DiffSyntax {
    DiffSyntax::from_theme(&crate::theme::paneflow_light())
}

fn document(name: &str, text: &str) -> CodeDocument {
    CodeDocument::new(PathBuf::from(name), text)
}

fn stride(index: usize, span: usize) -> usize {
    if span == 0 {
        return 0;
    }
    (index.wrapping_mul(2_654_435_761)) % span
}

fn apply_ui_edit(
    doc: &mut CodeDocument,
    highlighter: &mut CodeHighlighter,
    range: Range<usize>,
    text: &str,
    timer: &mut SegmentTimer,
) {
    let Some(applied) = timer.time(|| splice(doc, range, text)) else {
        return;
    };
    let mut deferred = None;
    for change in &applied.edits {
        if let HighlightOutcome::Deferred(parse) = timer.time(|| highlighter.edit(doc, change)) {
            deferred = Some(parse);
        }
    }
    if let Some(parse) = deferred {
        let parsed = parse.run();
        timer.time(|| highlighter.apply_parsed(doc, parsed));
    }
}

fn open_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    metrics.push(measure(
        "open_300kb_highlighted",
        "a 300 KB Rust file opened: rope build, longest-line measure, tree-sitter parse and an explicit query of the first 60-row viewport",
        1,
        10,
        || {
            let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
            let mut highlighter = CodeHighlighter::new(&doc, dark());
            highlighter.requery_rows(&doc, 0..VIEWPORT_ROWS.min(doc.line_count()));
            std::hint::black_box((doc, highlighter));
        },
    ));
    metrics.push(measure(
        "open_3_7mb",
        "a 3.7 MB Rust file opened: rope build plus the longest-line measure, past the 300 KB highlight cap",
        1,
        5,
        || {
            let doc = document("bench-3-7mb.rs", &corpora.large_rust);
            let highlighter = CodeHighlighter::new(&doc, dark());
            std::hint::black_box((doc, highlighter));
        },
    ));
}

fn keystroke_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let mut doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let mut index = 0usize;
    metrics.push(measure_segments(
        "keystroke_to_runs",
        "render-thread work of one inserted character at a pseudo-random row on 300 KB of Rust: splice, incremental parse and the highlight requery, deferred parses excluded from the timer but their applied trees included",
        5,
        200,
        |timer| {
            index += 1;
            let row = stride(index, doc.line_count());
            let offset = doc.line_to_byte(row);
            apply_ui_edit(&mut doc, &mut highlighter, offset..offset, "x", timer);
        },
    ));
}

fn viewport_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let span = doc.line_count().saturating_sub(VIEWPORT_ROWS).max(1);
    let mut index = 0usize;
    metrics.push(measure(
        "viewport_query_60_rows",
        "the highlight query for one 60-row viewport of 300 KB of Rust, the work a viewport-bounded requery would do per frame",
        5,
        200,
        || {
            index += 1;
            let first = stride(index, span);
            highlighter.requery_rows(&doc, first..first + VIEWPORT_ROWS);
        },
    ));
}

fn unclosed_comment_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let text = format!("/*\n{}", corpora.highlighted_rust);
    metrics.push(measure_segments(
        "unclosed_comment_close_ui",
        "render-thread work of closing an unterminated block comment at the top of 300 KB of Rust, which re-tokenizes the whole file",
        1,
        10,
        |timer| {
            let mut doc = document("bench-300kb.rs", &text);
            let mut highlighter = CodeHighlighter::new(&doc, dark());
            apply_ui_edit(&mut doc, &mut highlighter, 2..2, "*/", timer);
            std::hint::black_box((doc, highlighter));
        },
    ));
}

fn resolve_runs_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let palette = [
        hsla(0.08, 0.6, 0.6, 1.0),
        hsla(0.33, 0.5, 0.6, 1.0),
        hsla(0.58, 0.7, 0.7, 1.0),
        hsla(0.85, 0.4, 0.65, 1.0),
    ];
    let runs: Vec<(Range<usize>, Hsla)> =
        json_token_ranges(&corpora.minified_json, RESOLVE_RUNS_CAPTURES)
            .into_iter()
            .enumerate()
            .map(|(index, range)| (range, palette[index % palette.len()]))
            .collect();
    metrics.push(measure_segments(
        "resolve_runs_3750",
        "resolve_runs over 3 750 captures taken from a 10 000-character minified JSON line, the shape the diff view shares",
        1,
        10,
        |timer| {
            let mut input = runs.clone();
            timer.time(|| resolve_runs(&mut input));
            std::hint::black_box(input);
        },
    ));
}

fn document_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let doc = document("bench-3-7mb.rs", &corpora.large_rust);
    let end = doc.len_bytes();
    metrics.push(measure(
        "byte_to_utf16_eof",
        "one byte offset converted to a UTF-16 offset at the end of a 3.7 MB document, two to four times per keystroke through EntityInputHandler",
        20,
        200,
        || {
            std::hint::black_box(doc.byte_to_utf16(end));
        },
    ));
    metrics.push(measure(
        "to_disk_string_3_7mb",
        "the whole 3.7 MB document rendered to the string a save writes",
        1,
        10,
        || {
            std::hint::black_box(doc.to_disk_string());
        },
    ));
}

fn theme_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let mut index = 0usize;
    metrics.push(measure(
        "theme_switch",
        "a theme change on 300 KB of Rust: today the whole document is requeried on the render thread",
        1,
        20,
        || {
            index += 1;
            let syntax = if index.is_multiple_of(2) { dark() } else { light() };
            highlighter.set_syntax(&doc, syntax);
        },
    ));
}

fn markdown_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    metrics.push(measure(
        "open_markdown_injected",
        "a 64 KB Markdown file opened, the only corpus with a second grammar pass through the inline injection",
        1,
        10,
        || {
            let doc = document("bench-inline.md", &corpora.markdown);
            let highlighter = CodeHighlighter::new(&doc, dark());
            std::hint::black_box((doc, highlighter));
        },
    ));
}

fn reload_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let before = live_bytes();
    let mut doc = document("bench-2mb.rs", &corpora.reload_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let mut history = UndoHistory::default();
    for round in 0..RELOADS {
        let mut text = String::with_capacity(corpora.reload_rust.len() + 64);
        text.push_str(&corpora.reload_rust);
        text.push_str(&format!("const AGENT_ROUND_{round}: usize = {round};\n"));
        let len = doc.len_bytes();
        let Some(applied) = splice(&mut doc, 0..len, &text) else {
            continue;
        };
        for change in &applied.edits {
            if let HighlightOutcome::Deferred(parse) = highlighter.edit(&doc, change) {
                let parsed = parse.run();
                highlighter.apply_parsed(&doc, parsed);
            }
        }
        history.push(
            vec![applied.record],
            CodeSelection::at(0),
            CodeSelection::at(0),
            EditGroup::Atomic,
            Instant::now(),
        );
    }
    let retained = (live_bytes() - before).max(0) as f64;
    metrics.push(Metric::count(
        "reload_200_retained_bytes",
        "bytes",
        retained,
        "live allocated bytes a tab still holds after 200 external reloads of a 2 MB file: document, highlighter and undo history",
    ));
    std::hint::black_box((doc, highlighter, history));
}

#[test]
#[ignore = "editor performance benchmark: run through scripts/bench-editor"]
fn editor_pipeline_benchmark() {
    refuse_debug_profile();

    let corpora = Corpora::build();
    let mut metrics = Vec::new();
    let timed_started = Instant::now();
    let cpu_before = process_cpu_time();
    open_scenarios(&mut metrics, &corpora);
    markdown_scenario(&mut metrics, &corpora);
    keystroke_scenario(&mut metrics, &corpora);
    viewport_scenario(&mut metrics, &corpora);
    unclosed_comment_scenario(&mut metrics, &corpora);
    resolve_runs_scenario(&mut metrics, &corpora);
    document_scenarios(&mut metrics, &corpora);
    theme_scenario(&mut metrics, &corpora);
    let cpu_share = (process_cpu_time() - cpu_before).as_secs_f64()
        / timed_started.elapsed().as_secs_f64().max(f64::EPSILON);
    println!("PANEFLOW_BENCH_NOTE cpu share over the timed scenarios: {cpu_share:.2}");
    if cpu_share < 0.9 {
        println!(
            "PANEFLOW_BENCH_WARNING the process only got {:.0}% of a core while timing: another workload was competing, treat the timings as inflated",
            cpu_share * 100.0
        );
    }
    reload_scenario(&mut metrics, &corpora);

    publish(
        "paneflow-editor-bench",
        EDITOR_CORPUS_SEED,
        &metrics,
        cpu_share,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_corpus_reaches_the_path_the_bench_claims() {
        let rust = rust_source(8_192);
        let doc = document("probe.rs", &rust);
        let highlighter = CodeHighlighter::new(&doc, dark());
        assert!(highlighter.is_enabled(), "the rust corpus must highlight");
        assert!(
            (0..doc.line_count()).any(|row| !highlighter.runs(row).is_empty()),
            "the rust corpus must produce colored runs"
        );

        let markdown = markdown_source(8_192);
        let doc = document("probe.md", &markdown);
        let highlighter = CodeHighlighter::new(&doc, dark());
        assert!(
            highlighter.is_enabled(),
            "the markdown corpus must highlight"
        );

        let json = minified_json_line(2_048);
        let doc = document("probe.json", &json);
        let highlighter = CodeHighlighter::new(&doc, dark());
        assert!(highlighter.is_enabled(), "the json corpus must highlight");
        assert_eq!(
            doc.line_count(),
            2,
            "the json corpus is one line plus its terminator"
        );
    }

    #[test]
    fn a_viewport_requery_colors_the_rows_it_covers() {
        let rust = rust_source(32_768);
        let doc = document("probe.rs", &rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        let rows = 0..VIEWPORT_ROWS.min(doc.line_count());
        highlighter.requery_rows(&doc, rows.clone());
        assert!(
            rows.clone().any(|row| !highlighter.runs(row).is_empty()),
            "a viewport requery must leave colored runs behind"
        );
    }

    #[test]
    fn a_ui_edit_keeps_the_document_and_the_runs_in_step() {
        let rust = rust_source(16_384);
        let mut doc = document("probe.rs", &rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        let before = doc.len_bytes();
        let mut timer = SegmentTimer::default();
        apply_ui_edit(&mut doc, &mut highlighter, 0..0, "x", &mut timer);
        assert_eq!(doc.len_bytes(), before + 1);
        assert!(
            (0..doc.line_count()).any(|row| !highlighter.runs(row).is_empty()),
            "the highlighter must still carry runs after a UI edit"
        );
        assert!(
            highlighter.runs(doc.line_count()).is_empty(),
            "a row past the end must resolve to no runs"
        );
    }
}
