use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    Font, FontFeatures, FontStyle, FontWeight, Hsla, Platform, SharedString, TextRun, TextSystem,
    WindowTextSystem, hsla, px,
};

use crate::bench_harness::{
    Metric, SegmentTimer, live_bytes, measure, measure_segments, process_cpu_time, publish,
    refuse_debug_profile,
};
use crate::diff::{DiffSyntax, resolve_runs};
use paneflow_textdiff::{ComparisonPolicy, HighlightPolicy, compare_lines_inner};

use super::bench_corpus::{
    EDITOR_CORPUS_SEED, HIGHLIGHTED_RUST_BYTES, LARGE_RUST_BYTES, MINIFIED_JSON_CHARS,
    RELOAD_RUST_BYTES, TEXTDIFF_DENSE_AFTER_SALT, TEXTDIFF_DENSE_BEFORE_SALT,
    TEXTDIFF_DENSE_BLOCKS, TEXTDIFF_DENSE_WORDS_PER_LINE, TEXTDIFF_EDITED_BLOCK_LINES,
    TEXTDIFF_EDITED_BLOCKS, TEXTDIFF_RUST_BYTES, TEXTDIFF_WORD_LINES, TEXTDIFF_WORD_SALT,
    TEXTDIFF_WORDS_PER_LINE, change_one_word_per_line, dense_word_blocks, edit_line_blocks,
    interleave_separators, json_token_ranges, markdown_source, minified_json_line, rust_source,
    word_lines,
};
use super::cursor::CodeSelection;
use super::document::CodeDocument;
use super::edit::{EditGroup, UndoHistory, disk_splices, splice};
use super::element::CODE_FONT_SIZE;
use super::highlight::{CodeHighlighter, HighlightOutcome};

const VIEWPORT_ROWS: usize = 60;

const RESOLVE_RUNS_CAPTURES: usize = 3_750;

const RELOADS: usize = 200;
const SHAPE_ROW_CHARS: usize = 100;

struct Corpora {
    highlighted_rust: String,
    large_rust: String,
    reload_rust: String,
    minified_json: String,
    markdown: String,
    textdiff: TextdiffCorpora,
}

struct TextdiffCorpora {
    rust: String,
    rust_edited: String,
    word_lines: String,
    word_lines_edited: String,
    dense_before: String,
    dense_after: String,
}

impl TextdiffCorpora {
    fn build() -> Self {
        let rust = rust_source(TEXTDIFF_RUST_BYTES);
        let rust_edited =
            edit_line_blocks(&rust, TEXTDIFF_EDITED_BLOCKS, TEXTDIFF_EDITED_BLOCK_LINES);
        let edited_lines = word_lines(
            TEXTDIFF_WORD_LINES,
            TEXTDIFF_WORDS_PER_LINE,
            TEXTDIFF_WORD_SALT,
        );
        let word_lines = interleave_separators(&edited_lines);
        let word_lines_edited = interleave_separators(&change_one_word_per_line(&edited_lines));
        let dense_before = dense_word_blocks(
            TEXTDIFF_WORD_LINES,
            TEXTDIFF_DENSE_WORDS_PER_LINE,
            TEXTDIFF_DENSE_BLOCKS,
            TEXTDIFF_DENSE_BEFORE_SALT,
        );
        let dense_after = dense_word_blocks(
            TEXTDIFF_WORD_LINES,
            TEXTDIFF_DENSE_WORDS_PER_LINE,
            TEXTDIFF_DENSE_BLOCKS,
            TEXTDIFF_DENSE_AFTER_SALT,
        );
        Self {
            rust,
            rust_edited,
            word_lines,
            word_lines_edited,
            dense_before,
            dense_after,
        }
    }
}

impl Corpora {
    fn build() -> Self {
        Self {
            highlighted_rust: rust_source(HIGHLIGHTED_RUST_BYTES),
            large_rust: rust_source(LARGE_RUST_BYTES),
            reload_rust: rust_source(RELOAD_RUST_BYTES),
            minified_json: minified_json_line(MINIFIED_JSON_CHARS),
            markdown: markdown_source(64_000),
            textdiff: TextdiffCorpora::build(),
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
        if let HighlightOutcome::Deferred(parse) =
            timer.time(|| highlighter.edit(doc, &change.edit))
        {
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
            highlighter.fill_stale_rows(
                &doc,
                0..VIEWPORT_ROWS.min(doc.line_count()),
                Duration::from_millis(2),
            );
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
        "render-thread work of one inserted character at a pseudo-random row on 300 KB of Rust: splice, incremental parse or deferred-tree apply, then a viewport-bounded highlight fill; background parsing is excluded",
        5,
        200,
        |timer| {
            index += 1;
            let row = stride(index, doc.line_count());
            let offset = doc.line_to_byte(row);
            apply_ui_edit(&mut doc, &mut highlighter, offset..offset, "x", timer);
            let end = (row + VIEWPORT_ROWS).min(doc.line_count());
            timer.time(|| {
                highlighter.fill_stale_rows(&doc, row..end, Duration::from_millis(2))
            });
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
        "render-thread work of closing an unterminated block comment at the top of 300 KB of Rust: edit, deferred-tree apply and first viewport fill; background parsing is excluded",
        1,
        10,
        |timer| {
            let mut doc = document("bench-300kb.rs", &text);
            let mut highlighter = CodeHighlighter::new(&doc, dark());
            apply_ui_edit(&mut doc, &mut highlighter, 2..2, "*/", timer);
            timer.time(|| {
                highlighter.fill_stale_rows(
                    &doc,
                    0..VIEWPORT_ROWS.min(doc.line_count()),
                    Duration::from_millis(2),
                )
            });
            std::hint::black_box((doc, highlighter));
        },
    ));
}

fn deferred_parse_burst_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let mut doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let mut parses = Vec::with_capacity(50);
    let burst_started = Instant::now();
    let cpu_before = process_cpu_time();
    for index in 0..50 {
        let row = stride(index + 1, doc.line_count());
        let offset = doc.line_to_byte(row);
        let Some(edit) = doc.insert(offset, "x") else {
            continue;
        };
        let HighlightOutcome::Deferred(parse) =
            highlighter.edit_with_budget(&doc, &edit, Duration::ZERO)
        else {
            continue;
        };
        parses.push(smol::spawn(smol::unblock(move || parse.run())));
    }
    let burst_elapsed = burst_started.elapsed();
    let mut completed = 0usize;
    let mut latest = None;
    for parse in parses {
        let parsed = smol::block_on(parse);
        completed += usize::from(!parsed.was_cancelled());
        latest = Some(parsed);
    }
    let cpu = process_cpu_time() - cpu_before;
    if let Some(parsed) = latest {
        std::hint::black_box(highlighter.apply_parsed(&doc, parsed));
    }
    metrics.push(Metric::count(
        "deferred_burst_completed_parses",
        "count",
        completed as f64,
        "complete background parses after 50 zero-budget edits; superseded generations must cancel so only the latest completes",
    ));
    metrics.push(Metric::count(
        "deferred_burst_cpu",
        "ns",
        cpu.as_nanos() as f64,
        "process CPU spent running every deferred parse from a 50-edit burst after cancellation; target below 120 ms",
    ));
    metrics.push(Metric::count(
        "deferred_burst_edit_wall",
        "ns",
        burst_elapsed.as_nanos() as f64,
        "wall time to enqueue 50 zero-budget edits and their background parses; target below 200 ms",
    ));
    println!(
        "PANEFLOW_BENCH_NOTE deferred edit burst built 50 generations in {:.3} ms",
        burst_elapsed.as_secs_f64() * 1_000.0
    );
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
        "a theme change on 300 KB of Rust: rebuild capture color tables without querying tree-sitter or rewriting row runs",
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

fn editor_font() -> Font {
    Font {
        family: crate::terminal::element::resolve_font_family(None).into(),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn ascii_row(salt: usize, row: usize) -> SharedString {
    let mut text = String::with_capacity(SHAPE_ROW_CHARS);
    text.push_str(&format!("{salt:04}:{row:04} "));
    let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-<>(){}[];:,.";
    let mut cursor = salt.wrapping_mul(31).wrapping_add(row);
    while text.len() < SHAPE_ROW_CHARS {
        cursor = cursor.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        text.push(alphabet[(cursor >> 16) % alphabet.len()] as char);
    }
    text.truncate(SHAPE_ROW_CHARS);
    text.into()
}

fn platform_text_system() -> Option<(Rc<dyn Platform>, Arc<WindowTextSystem>)> {
    std::panic::catch_unwind(|| {
        let platform = gpui_platform::current_platform(true);
        let text_system = Arc::new(TextSystem::new(platform.text_system()));
        (platform, Arc::new(WindowTextSystem::new(text_system)))
    })
    .ok()
}

fn ascii_rows(salt: usize) -> Vec<SharedString> {
    (0..VIEWPORT_ROWS).map(|row| ascii_row(salt, row)).collect()
}

fn shape_rows(text_system: &WindowTextSystem, font: &Font, rows: &[SharedString]) -> f32 {
    let mut width = 0.0f32;
    for text in rows {
        let text = text.clone();
        let runs = [TextRun {
            len: text.len(),
            font: font.clone(),
            color: hsla(0.0, 0.0, 0.9, 1.0),
            background_color: None,
            underline: None,
            strikethrough: None,
        }];
        let shaped = text_system.shape_line(text, px(CODE_FONT_SIZE), &runs, None);
        width += f32::from(shaped.width());
    }
    width
}

fn skip_shape(metrics: &mut Vec<Metric>, reason: &'static str) {
    println!("PANEFLOW_BENCH_SKIP shape_cold_60_rows: {reason}");
    println!("PANEFLOW_BENCH_SKIP shape_warm_60_rows: {reason}");
    metrics.push(Metric::unavailable("shape_cold_60_rows", "ns", reason));
    metrics.push(Metric::unavailable("shape_warm_60_rows", "ns", reason));
}

fn shape_scenarios(metrics: &mut Vec<Metric>) {
    if std::env::var_os("PANEFLOW_BENCH_SKIP_SHAPE").is_some() {
        skip_shape(metrics, "PANEFLOW_BENCH_SKIP_SHAPE is set");
        return;
    }
    let Some((platform, text_system)) = platform_text_system() else {
        skip_shape(
            metrics,
            "the platform text system could not be created in this process",
        );
        return;
    };
    let font = editor_font();
    let warm = ascii_rows(0);
    if shape_rows(&text_system, &font, &warm) <= 0.0 {
        skip_shape(
            metrics,
            "the platform text system shaped a zero-width line, so no real font is available",
        );
        return;
    }
    let mut salt = 0usize;
    metrics.push(measure_segments(
        "shape_cold_60_rows",
        "60 never-seen ASCII rows of 100 characters shaped with the editor monospace font, the cold-cache cost of one scrolled viewport",
        1,
        20,
        |timer| {
            salt += 1;
            let rows = ascii_rows(salt);
            timer.time(|| std::hint::black_box(shape_rows(&text_system, &font, &rows)));
        },
    ));
    metrics.push(measure_segments(
        "shape_warm_60_rows",
        "the same 60 rows shaped again, the warm-cache cost the line-layout cache serves on a second frame",
        1,
        20,
        |timer| {
            timer.time(|| std::hint::black_box(shape_rows(&text_system, &font, &warm)));
        },
    ));
    drop(platform);
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
        let current = doc.text().to_string();
        let ops = disk_splices(&current, &text);
        let mut records = Vec::with_capacity(ops.len());
        for (range, inserted) in ops {
            let Some(applied) = splice(&mut doc, range, &inserted) else {
                continue;
            };
            for change in &applied.edits {
                if let HighlightOutcome::Deferred(parse) = highlighter.edit(&doc, &change.edit) {
                    let parsed = parse.run();
                    highlighter.apply_parsed(&doc, parsed);
                }
            }
            records.push(applied.record);
        }
        history.push(
            records,
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

fn textdiff_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let corpora = &corpora.textdiff;
    metrics.push(measure(
        "textdiff_300kb_50_blocks",
        "compare_lines_inner with word highlighting over 300 KB of Rust against a copy with 50 rewritten 10-line blocks",
        2,
        20,
        || {
            std::hint::black_box(compare_lines_inner(
                &corpora.rust,
                &corpora.rust_edited,
                ComparisonPolicy::Default,
                HighlightPolicy::Words,
            ));
        },
    ));
    metrics.push(measure(
        "textdiff_5k_lines_one_word_each",
        "compare_lines_inner with word highlighting over 5 000 edited lines, each with one word changed and separated from the next by an identical line, so the word pass runs once per edited line",
        2,
        10,
        || {
            std::hint::black_box(compare_lines_inner(
                &corpora.word_lines,
                &corpora.word_lines_edited,
                ComparisonPolicy::Default,
                HighlightPolicy::Words,
            ));
        },
    ));
    metrics.push(measure(
        "textdiff_5k_lines_all_different",
        "compare_lines_inner over 5 000 all-different dense lines in four blocks: the first three blocks exceed the fine comparison threshold and trip the bad-lines guard",
        2,
        20,
        || {
            std::hint::black_box(compare_lines_inner(
                &corpora.dense_before,
                &corpora.dense_after,
                ComparisonPolicy::Default,
                HighlightPolicy::Words,
            ));
        },
    ));
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
    deferred_parse_burst_scenario(&mut metrics, &corpora);
    resolve_runs_scenario(&mut metrics, &corpora);
    document_scenarios(&mut metrics, &corpora);
    theme_scenario(&mut metrics, &corpora);
    textdiff_scenarios(&mut metrics, &corpora);
    let cpu_share = (process_cpu_time() - cpu_before).as_secs_f64()
        / timed_started.elapsed().as_secs_f64().max(f64::EPSILON);
    println!("PANEFLOW_BENCH_NOTE cpu share over the timed scenarios: {cpu_share:.2}");
    if cpu_share < 0.9 {
        println!(
            "PANEFLOW_BENCH_WARNING the process only got {:.0}% of a core while timing: another workload was competing, treat the timings as inflated",
            cpu_share * 100.0
        );
    }
    shape_scenarios(&mut metrics);
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
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        assert!(highlighter.is_enabled(), "the rust corpus must highlight");
        highlighter.requery_rows(&doc, 0..doc.line_count());
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
    fn the_textdiff_corpora_reach_the_paths_the_bench_claims() {
        let corpora = TextdiffCorpora::build();
        let fragments = compare_lines_inner(
            &corpora.rust,
            &corpora.rust_edited,
            ComparisonPolicy::Default,
            HighlightPolicy::Words,
        );
        assert_eq!(fragments.len(), TEXTDIFF_EDITED_BLOCKS);
        assert!(
            fragments.iter().all(|fragment| fragment.inner.is_some()),
            "every rewritten block gets word highlighting"
        );

        let fragments = compare_lines_inner(
            &corpora.word_lines,
            &corpora.word_lines_edited,
            ComparisonPolicy::Default,
            HighlightPolicy::Words,
        );
        assert_eq!(
            fragments.len(),
            TEXTDIFF_WORD_LINES,
            "one block per edited line"
        );
        assert!(
            fragments.iter().all(|fragment| fragment
                .inner
                .as_ref()
                .is_some_and(|inner| inner.len() == 1)),
            "one inner fragment per edited line"
        );

        let fragments = compare_lines_inner(
            &corpora.dense_before,
            &corpora.dense_after,
            ComparisonPolicy::Default,
            HighlightPolicy::Words,
        );
        assert_eq!(fragments.len(), TEXTDIFF_DENSE_BLOCKS);
        assert!(
            fragments.iter().all(|fragment| fragment.inner.is_none()),
            "three too-big blocks trip the guard and the fourth is skipped"
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
        highlighter.requery_rows(&doc, 0..VIEWPORT_ROWS.min(doc.line_count()));
        let before = doc.len_bytes();
        let mut timer = SegmentTimer::default();
        apply_ui_edit(&mut doc, &mut highlighter, 0..0, "x", &mut timer);
        highlighter.fill_stale_rows(
            &doc,
            0..VIEWPORT_ROWS.min(doc.line_count()),
            Duration::from_secs(1),
        );
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

    #[test]
    fn the_shape_probe_either_measures_or_reports_itself_unavailable() {
        let Some((platform, text_system)) = platform_text_system() else {
            let mut metrics = Vec::new();
            skip_shape(
                &mut metrics,
                "the platform text system could not be created in this process",
            );
            assert!(metrics.iter().all(|metric| !metric.available));
            return;
        };
        let width = shape_rows(&text_system, &editor_font(), &ascii_rows(0));
        assert!(
            width.is_finite() && width >= 0.0,
            "a shaped viewport must report a finite width, got {width}"
        );
        drop(platform);
    }

    #[test]
    fn ascii_rows_are_distinct_and_the_advertised_width() {
        let first = ascii_row(1, 0);
        let second = ascii_row(1, 1);
        assert_eq!(first.len(), SHAPE_ROW_CHARS);
        assert_eq!(second.len(), SHAPE_ROW_CHARS);
        assert_ne!(first, second);
        assert_ne!(first, ascii_row(2, 0));
        assert_eq!(first, ascii_row(1, 0));
        assert!(first.is_ascii());
    }
}
