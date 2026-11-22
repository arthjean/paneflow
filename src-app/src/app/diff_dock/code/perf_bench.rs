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
    Metric, SegmentTimer, count_tree_sitter_allocations_in_a_process_that_has_not_parsed_yet,
    instruction_case, live_bytes, measure, measure_segments, process_cpu_time, publish,
    refuse_debug_profile, tree_sitter_live_bytes,
};
use crate::diff::{
    DiffSyntax, MAX_HIGHLIGHT_BYTES, MAX_MARKDOWN_HIGHLIGHT_BYTES, grammar_for_ext, is_markdown,
    markdown_inline_grammar, resolve_runs,
};
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
use super::element::{CODE_FONT_SIZE, CODE_ROW_HEIGHT, CodeScroll, ShapeCache};
use super::highlight::{
    CodeHighlighter, HIGHLIGHT_FRAME_BUDGET, HighlightOutcome, LineRuns, SYNC_PARSE_BUDGET,
};
use super::lexical::LexicalLayer;
use super::rows::{RowMap, RowSlot};
use super::wrap;

const VIEWPORT_ROWS: usize = 60;

const RESOLVE_RUNS_CAPTURES: usize = 3_750;

const RELOADS: usize = 200;
const RELOAD_HUNKS: usize = 10;
const SHAPE_ROW_CHARS: usize = 100;

const FILL_WINDOWS_CAP: usize = 120;

const PAGEDOWN_JUMPS: usize = 20;
const PAGEDOWN_FRAME_LIMIT: usize = 64;

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

fn parsed(doc: &CodeDocument, syntax: DiffSyntax) -> CodeHighlighter {
    let mut highlighter = CodeHighlighter::new(doc, syntax);
    highlighter.parse_initial_blocking(doc);
    while highlighter.lexical_has_pending_scan() {
        highlighter.lexical_idle_slice(doc, UNBOUNDED_BUDGET);
    }
    highlighter
}

fn parsed_layer2(doc: &CodeDocument, syntax: DiffSyntax) -> CodeHighlighter {
    let mut highlighter = CodeHighlighter::layer2_only(doc, syntax);
    highlighter.parse_initial_blocking(doc);
    highlighter
}

fn fill_windows(doc: &CodeDocument) -> usize {
    (doc.line_count() / VIEWPORT_ROWS).clamp(1, FILL_WINDOWS_CAP)
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
    let change = applied.edit;
    let mut deferred = None;
    if let HighlightOutcome::Deferred(parse) = timer.time(|| highlighter.edit(doc, &change)) {
        deferred = Some(parse);
    }
    if let Some(parse) = deferred {
        let parsed = parse.run();
        timer.time(|| highlighter.apply_parsed(doc, parsed));
    }
}

fn open_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    metrics.push(measure(
        "open_300kb_highlighted",
        "a 300 KB Rust file opened: rope build, longest-line measure and an explicit query of the first 60-row viewport; since US-030 the initial parse is deferred, so this is the work between the read and the first visible text",
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
        "open_to_first_tree_300kb",
        "the same 300 KB Rust file from the read to apply_parsed: the deferred initial parse plus the first 60-row viewport query it makes possible, all of it off the render thread except the apply",
        1,
        10,
        || {
            let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
            let mut highlighter = CodeHighlighter::new(&doc, dark());
            let parse = highlighter
                .initial_parse(&doc)
                .expect("the 300 KB corpus defers an initial parse");
            highlighter.apply_parsed(&doc, parse.run());
            highlighter.fill_stale_rows(
                &doc,
                0..VIEWPORT_ROWS.min(doc.line_count()),
                Duration::from_millis(2),
            );
            std::hint::black_box((doc, highlighter));
        },
    ));
    metrics.push(measure(
        "open_to_first_color_300kb",
        "the same 300 KB Rust file from the read to a first 60-row viewport with colors: layer 1 lexes it inside the open, so the tree is waited for only when layer 1 is off (PANEFLOW_BENCH_WITHOUT_LAYER1), where this equals open_to_first_tree_300kb",
        1,
        10,
        || {
            let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
            let mut highlighter = CodeHighlighter::new(&doc, dark());
            let rows = 0..VIEWPORT_ROWS.min(doc.line_count());
            if !viewport_colored(&highlighter, rows.clone()) {
                if let Some(parse) = highlighter.initial_parse(&doc) {
                    highlighter.apply_parsed(&doc, parse.run());
                }
                highlighter.fill_stale_rows(&doc, rows.clone(), Duration::from_millis(2));
            }
            assert!(viewport_colored(&highlighter, rows), "the first viewport is colored");
            std::hint::black_box((doc, highlighter));
        },
    ));
    metrics.push(measure(
        "open_2mb_to_text",
        "a 2 MB Rust file from the read to visible text: rope build, longest-line measure and the first 60-row viewport query while the initial parse is still in flight; US-030 budgets this under 50 ms",
        1,
        10,
        || {
            let doc = document("bench-2mb.rs", &corpora.reload_rust);
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
        "a 3.7 MB Rust file opened: rope build plus the longest-line measure, past the 2 MB highlight cap",
        1,
        5,
        || {
            let doc = document("bench-3-7mb.rs", &corpora.large_rust);
            let highlighter = CodeHighlighter::new(&doc, dark());
            std::hint::black_box((doc, highlighter));
        },
    ));
}

fn viewport_colored(highlighter: &CodeHighlighter, rows: Range<usize>) -> bool {
    let mut runs = LineRuns::new();
    rows.into_iter().any(|row| {
        highlighter.runs_into(row, &mut runs);
        !runs.is_empty()
    })
}

fn settle(layer: &mut LexicalLayer, doc: &CodeDocument) {
    while layer.has_pending_scan() {
        layer.idle_slice(doc, UNBOUNDED_BUDGET);
    }
}

fn settled_layer(doc: &CodeDocument, syntax: &DiffSyntax) -> LexicalLayer {
    let mut layer = LexicalLayer::open(doc, syntax).expect("a lexical layer");
    settle(&mut layer, doc);
    layer
}

fn lexical_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let syntax = dark();
    let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    if LexicalLayer::open(&doc, &syntax).is_none() {
        println!(
            "PANEFLOW_BENCH_SKIP lexical scenarios: layer 1 is off ({})",
            super::lexical::DISABLE_ENV
        );
        return;
    }
    metrics.push(measure(
        "lexical_open_300kb",
        "layer 1 opens the 300 KB Rust file: the text staged into a fresh lexer and its first rows scanned and packed, the work open_blocking adds before the first frame",
        2,
        20,
        || {
            std::hint::black_box(LexicalLayer::open(&doc, &syntax).expect("a lexical layer"));
        },
    ));
    metrics.push(measure(
        "lexical_full_300kb",
        "layer 1 colors the whole 300 KB Rust file: the open, then idle slices until the scan reaches the end; the slices run on the render thread between frames, at most IDLE_SLICE_BUDGET each",
        2,
        20,
        || {
            std::hint::black_box(settled_layer(&doc, &syntax));
        },
    ));

    let heap_before = live_bytes();
    let layer = settled_layer(&doc, &syntax);
    let heap = (live_bytes() - heap_before).max(0) as f64;
    metrics.push(Metric::count(
        "lexical_memory_300kb",
        "bytes",
        heap + layer.linear_bytes() as f64,
        "bytes layer 1 holds for the 300 KB Rust file once its scan reached the end: the lexer's linear memory plus the Rust heap of its line index and of the packed runs of every row",
    ));

    let span = doc.line_count().saturating_sub(VIEWPORT_ROWS).max(1);
    let mut index = 0usize;
    let mut runs = LineRuns::new();
    metrics.push(measure(
        "lexical_viewport_60_rows",
        "the layer-1 runs of one 60-row viewport of the 300 KB Rust file resolved to colors at the same pseudo-random rows as viewport_query_60_rows: the paint-time cost of a row layer 2 has not covered",
        5,
        200,
        || {
            index += 1;
            let first = stride(index, span);
            let mut count = 0usize;
            for row in first..first + VIEWPORT_ROWS {
                runs.clear();
                layer.push_runs(row, 0..usize::MAX, &mut runs);
                count += runs.len();
            }
            std::hint::black_box(count);
        },
    ));
    drop(layer);

    let mut doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut layer = settled_layer(&doc, &syntax);
    let mut index = 0usize;
    metrics.push(measure_segments(
        "lexical_keystroke",
        "one inserted character at a pseudo-random row of the 300 KB Rust file in layer 1 of the editor: the runs shifted, the edit staged and the lexer rescanned from the edited row to 64 rows past it, the render-thread work layer 1 adds to keystroke_to_runs; the splice and the idle slices that finish any longer scan are excluded",
        5,
        200,
        |timer| {
            index += 1;
            let row = stride(index, doc.line_count());
            let offset = doc.line_to_byte(row);
            let Some(applied) = splice(&mut doc, offset..offset, "x") else {
                return;
            };
            timer.time(|| layer.edit(&doc, std::slice::from_ref(&applied.edit), SYNC_PARSE_BUDGET));
            settle(&mut layer, &doc);
        },
    ));

    let mut flip = false;
    metrics.push(measure_segments(
        "lexical_comment_out_300kb_ui",
        "an unterminated /* typed, then removed, at the top of the 300 KB Rust file in layer 1: the edit plus the fill of the first 60-row viewport, the render-thread work before the frame that shows it; the idle slices that recolor the rest of the file are excluded",
        2,
        40,
        |timer| {
            flip = !flip;
            let applied = if flip {
                splice(&mut doc, 0..0, "/*")
            } else {
                splice(&mut doc, 0..2, "")
            };
            let Some(applied) = applied else {
                return;
            };
            timer.time(|| {
                layer.edit(&doc, std::slice::from_ref(&applied.edit), SYNC_PARSE_BUDGET);
                layer.fill(
                    &doc,
                    0..VIEWPORT_ROWS,
                    Instant::now() + HIGHLIGHT_FRAME_BUDGET,
                );
            });
            settle(&mut layer, &doc);
        },
    ));

    let large = document("bench-10mib.rs", &rust_source(super::load::MAX_FILE_BYTES));
    let heap_before = live_bytes();
    let mut layer = LexicalLayer::open(&large, &syntax).expect("a lexical layer");
    let job = layer
        .job(&large)
        .expect("a file past the cap lexes in a job");
    let installed = matches!(
        layer.apply(&large, job.run()),
        super::lexical::Applied::Installed
    );
    let heap = (live_bytes() - heap_before).max(0) as f64;
    let retained = heap + layer.linear_bytes() as f64;
    assert!(installed, "the 10 MiB file lexed inside the memory ceiling");
    metrics.push(Metric::count(
        "lexical_memory_10mib",
        "bytes",
        retained,
        "bytes layer 1 holds for a 10 MiB Rust file, the editor cap, once its background lex is installed: the lexer's linear memory plus the Rust heap of its line index and of the packed runs of every row (US-015, assumption A4)",
    ));
    metrics.push(Metric::count(
        "lexical_memory_10mib_per_input_byte",
        "ratio",
        retained / large.len_bytes() as f64,
        "lexical_memory_10mib divided by the file size; US-015 caps layer 1 below the editor cap only if this passes 8",
    ));
}

fn keystroke_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let mut doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = parsed(&doc, dark());
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
    let mut highlighter = parsed(&doc, dark());
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

const WRAP_COLUMNS: usize = 100;

fn wrapped_frame(scroll: &CodeScroll, doc: &CodeDocument) -> usize {
    scroll.sync_wrap(doc, Some(WRAP_COLUMNS));
    let rows = {
        let map = scroll.map();
        map.visible_rows(scroll.rows(), scroll.viewport_height())
    };
    let mut bytes = 0usize;
    for row in rows {
        let slot = scroll.map().slot_at(row);
        if let RowSlot::Text { line, segment } = slot {
            bytes += scroll
                .segments(doc, line)
                .get(segment)
                .map_or(0, |range| range.len());
        }
    }
    bytes
}

fn giant_line_scroll(doc: &CodeDocument) -> CodeScroll {
    let scroll = CodeScroll::new();
    scroll.set_metrics(
        gpui::Bounds::new(
            gpui::point(px(0.), px(0.)),
            gpui::size(px(800.), px(VIEWPORT_ROWS as f32 * CODE_ROW_HEIGHT)),
        ),
        doc.line_count(),
    );
    scroll.sync_wrap(doc, Some(WRAP_COLUMNS));
    let middle = scroll.map().total_rows() / 2;
    scroll.set_rows(middle as f64);
    scroll
}

fn wrap_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let rust = document("bench-300kb.rs", &corpora.highlighted_rust);
    metrics.push(measure(
        "wrap_layout_300kb",
        "the row map rebuilt for word wrap at 100 columns on 300 KB of Rust: every line measured and the prefix sums built, the synchronous work a width change or an edit costs a wrapped tab",
        2,
        50,
        || {
            std::hint::black_box(RowMap::from_spans(
                rust.line_count(),
                wrap::wrap_layout(rust.text(), WRAP_COLUMNS).spans,
            ));
        },
    ));
    let giant_text = format!("{{\n{}\n}}\n", minified_json_line(RELOAD_RUST_BYTES));
    let giant = document("bench-giant.json", &giant_text);
    metrics.push(measure(
        "wrapped_giant_line_first_frame",
        "the first frame of a file whose 2 MB line wraps at 100 columns: the synchronous row map build, then the 60 visible rows in the middle of the line resolved to their segments, the render-thread work of a prepaint before shaping",
        1,
        10,
        || {
            let scroll = giant_line_scroll(&giant);
            std::hint::black_box(wrapped_frame(&scroll, &giant));
        },
    ));
    let warm = giant_line_scroll(&giant);
    wrapped_frame(&warm, &giant);
    metrics.push(measure(
        "wrapped_giant_line_warm_frame",
        "a later frame of the same wrapped 2 MB line: the map and the line's segments are cached, so a frame only resolves its 60 rows",
        5,
        200,
        || {
            std::hint::black_box(wrapped_frame(&warm, &giant));
        },
    ));
}

fn fill_stale_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = parsed(&doc, dark());
    let windows = fill_windows(&doc);
    let mut index = 0usize;
    metrics.push(measure_segments(
        "fill_60_stale_rows",
        "one budgeted fill of a never-queried 60-row viewport on 300 KB of Rust: the contiguous stale span is merged into a single ranged query instead of 60 one-row queries",
        0,
        windows,
        |timer| {
            let first = index * VIEWPORT_ROWS;
            index += 1;
            timer.time(|| {
                highlighter.fill_stale_rows(
                    &doc,
                    first..first + VIEWPORT_ROWS,
                    HIGHLIGHT_FRAME_BUDGET,
                )
            });
        },
    ));
    std::hint::black_box((doc, highlighter));
}

fn plain_keystroke_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let mut doc = document("bench-3-7mb.rs", &corpora.large_rust);
    let before = live_bytes();
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let retained = (live_bytes() - before).max(0) as f64;
    metrics.push(Metric::count(
        "plain_highlighter_retained_bytes",
        "bytes",
        retained,
        "live allocated bytes the highlighter of the 3.7 MB file holds past the highlight cap: no per-row runs and no per-row states",
    ));
    let mut index = 0usize;
    metrics.push(measure_segments(
        "keystroke_3_7mb_plain",
        "render-thread work of one inserted character at a pseudo-random row of the 3.7 MB file, past the highlight cap: the rope splice and nothing else, with no interpolation and no per-row table",
        5,
        200,
        |timer| {
            index += 1;
            let row = stride(index, doc.line_count());
            let offset = doc.line_to_byte(row);
            apply_ui_edit(&mut doc, &mut highlighter, offset..offset, "x", timer);
        },
    ));
    std::hint::black_box((doc, highlighter));
}

struct PageDownProbe {
    stale_median: usize,
    stale_max: usize,
    frames_to_fresh: usize,
}

fn pagedown_probe(
    doc: &CodeDocument,
    highlighter: &mut CodeHighlighter,
    budget: Duration,
) -> PageDownProbe {
    let span = doc.line_count().saturating_sub(VIEWPORT_ROWS).max(1);
    let mut stale = Vec::with_capacity(PAGEDOWN_JUMPS);
    let mut frames_to_fresh = 0usize;
    for jump in 0..PAGEDOWN_JUMPS {
        let first = stride(jump + 1, span);
        let rows = first..first + VIEWPORT_ROWS;
        let mut fill = highlighter.fill_stale_rows(doc, rows.clone(), budget);
        stale.push(fill.stale_rows);
        let mut frames = 1usize;
        while fill.any_stale() && frames < PAGEDOWN_FRAME_LIMIT {
            fill = highlighter.fill_stale_rows(doc, rows.clone(), budget);
            frames += 1;
        }
        frames_to_fresh = frames_to_fresh.max(frames);
    }
    stale.sort_unstable();
    PageDownProbe {
        stale_median: stale.get(stale.len() / 2).copied().unwrap_or(0),
        stale_max: stale.last().copied().unwrap_or(0),
        frames_to_fresh,
    }
}

fn pagedown_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = parsed(&doc, dark());
    let probe = pagedown_probe(&doc, &mut highlighter, HIGHLIGHT_FRAME_BUDGET);
    metrics.push(Metric::count(
        "pagedown_stale_rows",
        "rows",
        probe.stale_median as f64,
        "median rows of a 60-row viewport still uncolored after one 2 ms fill, over 20 pseudo-random jumps on a freshly opened 300 KB Rust file",
    ));
    metrics.push(Metric::count(
        "pagedown_stale_rows_max",
        "rows",
        probe.stale_max as f64,
        "worst jump of the same 20: rows the first frame after a PageDown leaves in plain text",
    ));
    metrics.push(Metric::count(
        "pagedown_frames_to_fresh",
        "frames",
        probe.frames_to_fresh as f64,
        "successive 2 ms fills the worst of those 20 jumps needs before no visible row is stale; a starved fill now schedules the next one through request_animation_frame",
    ));
    std::hint::black_box((doc, highlighter));
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
            let mut highlighter = parsed(&doc, dark());
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
    let mut highlighter = parsed(&doc, dark());
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
    let mut highlighter = parsed(&doc, dark());
    let mut index = 0usize;
    metrics.push(measure(
        "theme_switch",
        "a theme change on 300 KB of Rust: rebuild capture color tables without querying tree-sitter or rewriting row runs",
        1,
        20,
        || {
            index += 1;
            let syntax = if index.is_multiple_of(2) { dark() } else { light() };
            highlighter.set_syntax(syntax);
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

fn platform_text_system() -> Option<(Rc<dyn Platform>, Rc<WindowTextSystem>)> {
    let (platform, text_system) = std::panic::catch_unwind(|| {
        let platform = gpui_platform::current_platform(true);
        let text_system = Arc::new(TextSystem::new(platform.text_system()));
        (platform, text_system)
    })
    .ok()?;
    let fonts = crate::assets::Assets
        .embedded_fonts()
        .expect("the embedded fonts load from the binary");
    text_system.add_fonts(fonts).expect(
        "the platform text system registers the embedded fonts, as the app does at startup",
    );
    Some((platform, Rc::new(WindowTextSystem::new(text_system))))
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

fn ascii_document(salt: usize) -> CodeDocument {
    let mut text = String::with_capacity(VIEWPORT_ROWS * (SHAPE_ROW_CHARS + 1));
    for row in 0..VIEWPORT_ROWS {
        text.push_str(&ascii_row(salt, row));
        text.push('\n');
    }
    document("prepaint.txt", &text)
}

fn prepaint_rows(
    text_system: &WindowTextSystem,
    font: &Font,
    doc: &CodeDocument,
    runs: &mut Vec<TextRun>,
    shapes: &mut ShapeCache,
) -> f32 {
    let mut width = 0.0f32;
    for row in 0..VIEWPORT_ROWS {
        let Some(range) = doc.line_byte_range(row) else {
            continue;
        };
        let Some(slice) = doc.line(row) else {
            continue;
        };
        let len = range.end - range.start;
        runs.clear();
        runs.push(TextRun {
            len,
            font: font.clone(),
            color: hsla(0.0, 0.0, 0.9, 1.0),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
        let (shaped, _) = shapes.shape_rope_line(slice, px(CODE_FONT_SIZE), runs, text_system);
        width += f32::from(shaped.width());
    }
    shapes.end_frame();
    width
}

fn skip_shape(metrics: &mut Vec<Metric>, reason: &'static str) {
    println!("PANEFLOW_BENCH_SKIP shape_cold_60_rows: {reason}");
    println!("PANEFLOW_BENCH_SKIP shape_warm_60_rows: {reason}");
    println!("PANEFLOW_BENCH_SKIP prepaint_60_rows_warm: {reason}");
    metrics.push(Metric::unavailable("shape_cold_60_rows", "ns", reason));
    metrics.push(Metric::unavailable("shape_warm_60_rows", "ns", reason));
    metrics.push(Metric::unavailable("prepaint_60_rows_warm", "ns", reason));
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
    let warm_doc = ascii_document(0);
    let mut runs = Vec::with_capacity(64);
    let mut shapes = ShapeCache::default();
    prepaint_rows(&text_system, &font, &warm_doc, &mut runs, &mut shapes);
    metrics.push(measure_segments(
        "prepaint_60_rows_warm",
        "the same 60 rows re-shaped the way the editor prepaint does it, through the content-keyed shape cache with a reused run buffer: the warm-cache cost of one scrolled viewport",
        1,
        20,
        |timer| {
            timer.time(|| {
                std::hint::black_box(prepaint_rows(
                    &text_system,
                    &font,
                    &warm_doc,
                    &mut runs,
                    &mut shapes,
                ))
            });
        },
    ));
    drop(platform);
}

fn rewrite_hunks(source: &str) -> String {
    let line_starts = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(index, _)| index + 1))
        .collect::<Vec<_>>();
    let mut out = String::with_capacity(source.len() + 512);
    let mut cursor = 0usize;
    for hunk in 1..=RELOAD_HUNKS {
        let row = hunk * line_starts.len() / (RELOAD_HUNKS + 1);
        let Some(&start) = line_starts.get(row) else {
            break;
        };
        let end = line_starts.get(row + 1).copied().unwrap_or(source.len());
        out.push_str(&source[cursor..start]);
        out.push_str(&format!("const AGENT_HUNK_{hunk}: usize = {hunk};\n"));
        cursor = end;
    }
    out.push_str(&source[cursor..]);
    out
}

fn apply_reload(
    doc: &mut CodeDocument,
    highlighter: &mut CodeHighlighter,
    history: &mut UndoHistory,
    ops: &[(Range<usize>, String)],
) {
    let mut records = Vec::with_capacity(ops.len());
    let mut edits = Vec::with_capacity(ops.len());
    for (range, inserted) in ops {
        let Some(applied) = splice(doc, range.clone(), inserted) else {
            continue;
        };
        edits.push(applied.edit);
        records.push(applied.record);
    }
    let deferred = highlighter.edit_batch(doc, &edits, SYNC_PARSE_BUDGET);
    history.push(
        records,
        CodeSelection::at(0),
        CodeSelection::at(0),
        EditGroup::Atomic,
        Instant::now(),
    );
    highlighter.fill_stale_rows(
        doc,
        0..VIEWPORT_ROWS.min(doc.line_count()),
        HIGHLIGHT_FRAME_BUDGET,
    );
    std::hint::black_box(deferred.is_ok());
}

fn reload_hunk_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let rewritten = rewrite_hunks(&corpora.large_rust);
    let original = document("bench-3-7mb.rs", &corpora.large_rust);
    metrics.push(measure(
        "disk_splices_100k_lines",
        "the disk-to-document diff of 10 hunks over the 115 500 lines of the 3.7 MB corpus: the work the reload task does off the render thread, excluded from reload_10_hunks_100k_lines_ui",
        1,
        10,
        || {
            std::hint::black_box(disk_splices(original.text(), &rewritten));
        },
    ));

    assert!(
        (100_000..130_000).contains(&original.line_count()),
        "the reload corpus must hold the six figures its metrics claim, got {}",
        original.line_count()
    );
    let to_rewritten = disk_splices(original.text(), &rewritten);
    assert_eq!(
        to_rewritten.len(),
        RELOAD_HUNKS,
        "the reload corpus must land one splice per hunk"
    );
    let rewritten_doc = document("bench-3-7mb.rs", &rewritten);
    let to_original = disk_splices(rewritten_doc.text(), &corpora.large_rust);
    drop(rewritten_doc);
    drop(original);

    let mut doc = document("bench-3-7mb.rs", &corpora.large_rust);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    let mut history = UndoHistory::default();
    let mut round = 0usize;
    metrics.push(measure_segments(
        "reload_10_hunks_100k_lines_ui",
        "render-thread work of an external reload of the 115 500-line 3.7 MB file: 10 descending hunks spliced under one batched highlighter call, then the first viewport fill; the corpus sits past the 2 MB coloring cap, so both of those return at once and the number is the splice and history cost alone; the off-thread diff is excluded",
        1,
        20,
        |timer| {
            let ops = if round.is_multiple_of(2) {
                &to_rewritten
            } else {
                &to_original
            };
            round += 1;
            timer.time(|| apply_reload(&mut doc, &mut highlighter, &mut history, ops));
        },
    ));
    std::hint::black_box((doc, highlighter, history));
}

fn reload_scenario(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    let before = live_bytes();
    let mut doc = document("bench-2mb.rs", &corpora.reload_rust);
    let mut highlighter = parsed(&doc, dark());
    assert!(
        highlighter.has_tree(),
        "the 2 MB corpus must sit under the highlight cap for this metric to mean what it claims"
    );
    let mut history = UndoHistory::default();
    for round in 0..RELOADS {
        let mut text = String::with_capacity(corpora.reload_rust.len() + 64);
        text.push_str(&corpora.reload_rust);
        text.push_str(&format!("const AGENT_ROUND_{round}: usize = {round};\n"));
        let ops = disk_splices(doc.text(), &text);
        apply_reload(&mut doc, &mut highlighter, &mut history, &ops);
    }
    let retained = (live_bytes() - before).max(0) as f64;
    metrics.push(Metric::count(
        "reload_200_retained_bytes",
        "bytes",
        retained,
        "live allocated bytes a colored 2 MB tab still holds after 200 external reloads: document, per-row runs and undo history; the tree-sitter trees allocate through the C allocator and are counted by the tree memory probe instead",
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

const TREE_BUDGET_BYTES: i64 = 128 * 1024 * 1024;

fn parse_grammars(ext: &str) -> Vec<&'static crate::diff::Grammar> {
    let mut grammars = Vec::new();
    if let Some(grammar) = grammar_for_ext(ext) {
        grammars.push(grammar);
    }
    if is_markdown(ext)
        && let Some(inline) = markdown_inline_grammar()
    {
        grammars.push(inline);
    }
    grammars
}

fn tree_bytes_of(ext: &str, source: &str) -> (i64, i64) {
    let grammars = parse_grammars(ext);
    let before = tree_sitter_live_bytes();
    let trees = grammars
        .into_iter()
        .filter_map(|grammar| {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&grammar.language).ok()?;
            parser.parse(source, None)
        })
        .collect::<Vec<_>>();
    let held = tree_sitter_live_bytes() - before;
    drop(trees);
    (held, tree_sitter_live_bytes() - before)
}

const TREE_MEMORY_PROBE_CHILD_ENV: &str = "PANEFLOW_TREE_MEMORY_PROBE_CHILD";

const TREE_MEMORY_PROBE_RAN: &str = "PANEFLOW_TREE_MEMORY_PROBE_RAN";

fn tree_memory_probe_child_test_name() -> String {
    let module = module_path!()
        .split_once("::")
        .map_or(module_path!(), |(_, rest)| rest);
    format!("{module}::tree_memory_probe_in_a_fresh_process")
}

fn passed_test_count(stdout: &str) -> Option<usize> {
    stdout.lines().find_map(|line| {
        let rest = line.strip_prefix("test result: ")?;
        let (_, counts) = rest.split_once(". ")?;
        let passed: usize = counts.split_once(" passed")?.0.trim().parse().ok()?;
        let failed: usize = counts
            .split_once("; ")?
            .1
            .split_once(" failed")?
            .0
            .trim()
            .parse()
            .ok()?;
        Some(passed + failed)
    })
}

#[test]
#[ignore = "tree memory probe: cargo test --release -p paneflow-app --bin paneflow app::diff_dock::code::perf_bench::tree_memory_probe -- --ignored --exact --nocapture"]
fn tree_memory_probe() {
    let child = tree_memory_probe_child_test_name();
    let output = std::process::Command::new(std::env::current_exe().expect("test binary path"))
        .args([
            child.as_str(),
            "--ignored",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(TREE_MEMORY_PROBE_CHILD_ENV, "1")
        .output()
        .expect("the probe child process starts");
    let stdout = String::from_utf8_lossy(&output.stdout);
    print!("{stdout}");
    eprint!("{}", String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "the probe child failed");
    assert_eq!(
        passed_test_count(&stdout),
        Some(1),
        "the probe child must run exactly one test ({child})"
    );
    assert!(
        stdout.contains(TREE_MEMORY_PROBE_RAN),
        "the probe child ran {child} without measuring"
    );
}

#[test]
#[ignore = "run only by tree_memory_probe, in its own process"]
fn tree_memory_probe_in_a_fresh_process() {
    if std::env::var_os(TREE_MEMORY_PROBE_CHILD_ENV).is_none() {
        return;
    }
    unsafe { count_tree_sitter_allocations_in_a_process_that_has_not_parsed_yet() };
    println!("{TREE_MEMORY_PROBE_RAN}");
    let cases = [
        ("rust_300kb", "rs", rust_source(HIGHLIGHTED_RUST_BYTES)),
        ("rust_2mb", "rs", rust_source(RELOAD_RUST_BYTES)),
        ("rust_3_7mb", "rs", rust_source(LARGE_RUST_BYTES)),
        ("markdown_64kb", "md", markdown_source(64_000)),
        ("markdown_2mb", "md", markdown_source(RELOAD_RUST_BYTES)),
        (
            "json_295kb",
            "json",
            minified_json_line(HIGHLIGHTED_RUST_BYTES),
        ),
        ("json_2mb", "json", minified_json_line(RELOAD_RUST_BYTES)),
    ];

    for (name, ext, source) in cases {
        let (held, leaked) = tree_bytes_of(ext, &source);
        let ratio = held as f64 / source.len() as f64;
        println!(
            "PANEFLOW_BENCH_NOTE tree_bytes {name}: {} source bytes hold {held} tree bytes, {ratio:.2} per source byte, so the {} MiB budget allows a cap of {} bytes",
            source.len(),
            TREE_BUDGET_BYTES / (1024 * 1024),
            (TREE_BUDGET_BYTES as f64 / ratio) as u64
        );
        assert!(
            held > 0,
            "{name} produced no tree: the counter is not installed or the grammar is missing"
        );
        assert!(
            leaked.abs() < held / 8,
            "{name} kept {leaked} bytes after its trees were dropped"
        );
    }

    let source = rust_source(HIGHLIGHTED_RUST_BYTES);
    let mut doc = document("probe-300kb.rs", &source);
    let before = tree_sitter_live_bytes();
    let mut highlighter = parsed(&doc, dark());
    let one_tree = tree_sitter_live_bytes() - before;
    let edit = doc
        .insert(doc.line_to_byte(10), "//\n")
        .expect("the probe edit lands");
    let HighlightOutcome::Deferred(parse) =
        highlighter.edit_with_budget(&doc, &edit, Duration::ZERO)
    else {
        panic!("a zero budget must defer the reparse");
    };
    let trees = parse.run();
    let both_trees = tree_sitter_live_bytes() - before;
    assert!(
        both_trees > one_tree,
        "the deferred parse must hold a second tree while it is in flight: {one_tree} -> {both_trees}"
    );
    assert!(highlighter.apply_parsed(&doc, trees));
    let after_apply = tree_sitter_live_bytes() - before;
    println!(
        "PANEFLOW_BENCH_NOTE tree_bytes one_tree={one_tree} both_trees={both_trees} after_apply={after_apply}"
    );
    assert!(
        after_apply < one_tree * 3 / 2,
        "apply_parsed must drop the superseded tree: {both_trees} in flight, {after_apply} kept, {one_tree} for one tree"
    );
    drop(highlighter);
    drop(doc);

    for (label, ext, cap) in [
        ("rs", "rs", MAX_HIGHLIGHT_BYTES),
        ("md", "md", MAX_MARKDOWN_HIGHLIGHT_BYTES),
        ("json", "json", MAX_HIGHLIGHT_BYTES),
    ] {
        let source = match ext {
            "md" => markdown_source(cap),
            "json" => minified_json_line(cap),
            _ => rust_source(cap),
        };
        let (held, _) = tree_bytes_of(ext, &source);
        println!(
            "PANEFLOW_BENCH_NOTE tree_bytes at_cap_{label}: {} source bytes hold {held} tree bytes ({:.1} MiB)",
            source.len(),
            held as f64 / (1024.0 * 1024.0)
        );
        assert!(
            held < TREE_BUDGET_BYTES,
            "a {label} file at the {cap}-byte cap holds {held} tree bytes, past the {TREE_BUDGET_BYTES}-byte budget"
        );
    }
}

const ENGINE_CALLS: usize = 10_000;

fn mid_row(doc_lines: usize, index: usize) -> usize {
    doc_lines / 2 + index % 32
}

fn lexer_rows(lexed: &paneflow_highlights::LexDocument, rows: std::ops::Range<usize>) -> usize {
    let mut count = 0usize;
    lexed
        .for_each_line_tokens(rows, |_, tokens| count += tokens.len())
        .expect("the tokens of the rows");
    count
}

fn lexer_engine_scenarios(metrics: &mut Vec<Metric>, corpora: &Corpora) {
    use paneflow_highlights::{LexDocument, Lexer, language_for_key};

    let rust = language_for_key("rs").expect("the Rust lexer");
    let text = corpora.highlighted_rust.as_str();

    metrics.push(measure(
        "lexer_instantiate",
        "one fresh native lexer instance (paneflow-highlights) with its two Unicode host imports and an empty document, the fixed cost of a new tab",
        3,
        50,
        || {
            let mut document = Lexer::new()
                .and_then(|lexer| lexer.document("", rust))
                .expect("an empty document");
            document.settle().expect("a settled document");
            std::hint::black_box(document);
        },
    ));
    metrics.push(measure(
        "lexer_full_color_300kb",
        "the native lexer colors the whole 300 KB Rust file: a fresh instance, the text staged into its memory and every line tokenized",
        2,
        20,
        || {
            std::hint::black_box(LexDocument::lex(text, rust).expect("a colored document"));
        },
    ));
    let doc = document("bench-300kb.rs", text);
    metrics.push(measure(
        "tree_sitter_full_color_300kb",
        "tree-sitter colors the whole 300 KB Rust file: the blocking initial parse plus the highlight query over every row, the counterpart of lexer_full_color_300kb; layer 1 is left out so the number is tree-sitter's alone",
        2,
        20,
        || {
            let mut highlighter = parsed_layer2(&doc, dark());
            highlighter.requery_rows(&doc, 0..doc.line_count());
            std::hint::black_box(highlighter);
        },
    ));

    let heap_before = live_bytes();
    let mut lexed = LexDocument::lex(text, rust).expect("a colored document");
    let heap = (live_bytes() - heap_before).max(0) as f64;
    let linear = lexed.memory_bytes() as f64;
    metrics.push(Metric::count(
        "lexer_retained_bytes",
        "bytes",
        heap + linear,
        "bytes one colored 300 KB Rust document holds in the native lexer: the instance's linear memory (text, line table, tokens, lexer states) plus the Rust heap it allocated, its line index included",
    ));
    let heap_before = live_bytes();
    let mut highlighter = parsed_layer2(&doc, dark());
    highlighter.requery_rows(&doc, 0..doc.line_count());
    let runs = (live_bytes() - heap_before).max(0) as f64;
    metrics.push(Metric::count(
        "tree_sitter_runs_retained_bytes",
        "bytes",
        runs,
        "Rust heap bytes the tree-sitter highlighter of the 300 KB file holds once every row is colored; the syntax tree lives in the C allocator and is measured by tree_memory_probe",
    ));
    drop(highlighter);

    let mut index = 0usize;
    metrics.push(measure(
        "lexer_keystroke_mid_file",
        "one inserted character in the middle of the 300 KB Rust file in the native lexer: the edit staged, the lexer rerun until its state matches again, then the 60 rows below read back",
        5,
        200,
        || {
            index += 1;
            let line = mid_row(lexed.line_count(), index);
            let at = lexed.line_start(line).expect("a row");
            lexed.edit(at..at, "x").expect("an edit");
            std::hint::black_box(lexer_rows(&lexed, line..line + VIEWPORT_ROWS));
        },
    ));
    let mut doc = document("bench-300kb.rs", text);
    let mut highlighter = parsed_layer2(&doc, dark());
    let mut index = 0usize;
    metrics.push(measure_segments(
        "tree_sitter_keystroke_mid_file",
        "one inserted character in the middle of the 300 KB Rust file with tree-sitter: splice, incremental parse or deferred-tree apply and the fill of the 60 rows below, the counterpart of lexer_keystroke_mid_file; layer 1 is left out so the number is tree-sitter's alone",
        5,
        200,
        |timer| {
            index += 1;
            let row = mid_row(doc.line_count(), index);
            let offset = doc.line_to_byte(row);
            apply_ui_edit(&mut doc, &mut highlighter, offset..offset, "x", timer);
            let end = (row + VIEWPORT_ROWS).min(doc.line_count());
            timer.time(|| highlighter.fill_stale_rows(&doc, row..end, UNBOUNDED_BUDGET));
        },
    ));

    let lexed = LexDocument::lex(text, rust).expect("a colored document");
    let span = lexed.line_count().saturating_sub(VIEWPORT_ROWS).max(1);
    let mut index = 0usize;
    metrics.push(measure(
        "lexer_viewport_query_60_rows",
        "the tokens of one 60-row viewport read back from the native lexer at the same pseudo-random rows as viewport_query_60_rows, its tree-sitter counterpart in this run",
        5,
        200,
        || {
            index += 1;
            let first = stride(index, span);
            std::hint::black_box(lexer_rows(&lexed, first..first + VIEWPORT_ROWS));
        },
    ));

    let started = Instant::now();
    let mut pending = 0usize;
    for _ in 0..ENGINE_CALLS {
        pending = pending.wrapping_add(std::hint::black_box(
            lexed.pending_lines().expect("the pending line count"),
        ));
    }
    let per_call = started.elapsed().as_nanos() as f64 / ENGINE_CALLS as f64;
    std::hint::black_box(pending);
    metrics.push(Metric::count(
        "lexer_call_ns",
        "ns",
        per_call,
        "mean cost of one Rust-to-lexer call that does no work (liveStats through the trap-catching shim), over 10,000 calls: the floor every per-line read pays",
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
    plain_keystroke_scenario(&mut metrics, &corpora);
    viewport_scenario(&mut metrics, &corpora);
    lexer_engine_scenarios(&mut metrics, &corpora);
    wrap_scenarios(&mut metrics, &corpora);
    fill_stale_scenario(&mut metrics, &corpora);
    pagedown_scenario(&mut metrics, &corpora);
    unclosed_comment_scenario(&mut metrics, &corpora);
    deferred_parse_burst_scenario(&mut metrics, &corpora);
    resolve_runs_scenario(&mut metrics, &corpora);
    document_scenarios(&mut metrics, &corpora);
    theme_scenario(&mut metrics, &corpora);
    reload_hunk_scenarios(&mut metrics, &corpora);
    textdiff_scenarios(&mut metrics, &corpora);
    lexical_scenarios(&mut metrics, &corpora);
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
        "editor",
        EDITOR_CORPUS_SEED,
        &metrics,
        Some(cpu_share),
    );
}

const UNBOUNDED_BUDGET: Duration = Duration::from_secs(3_600);
const INSTRUCTION_KEYSTROKES: usize = 20;
const INSTRUCTION_VIEWPORTS: usize = 20;
const INSTRUCTION_UTF16_LOOKUPS: usize = 1_000;

fn apply_unbounded_edit(
    doc: &mut CodeDocument,
    highlighter: &mut CodeHighlighter,
    range: Range<usize>,
    text: &str,
) {
    let Some(applied) = splice(doc, range, text) else {
        return;
    };
    if let HighlightOutcome::Deferred(parse) =
        highlighter.edit_with_budget(doc, &applied.edit, UNBOUNDED_BUDGET)
    {
        let parsed = parse.run();
        highlighter.apply_parsed(doc, parsed);
    }
}

fn open_with_first_viewport(name: &str, text: &str) {
    let doc = document(name, text);
    let mut highlighter = CodeHighlighter::new(&doc, dark());
    highlighter.fill_stale_rows(
        &doc,
        0..VIEWPORT_ROWS.min(doc.line_count()),
        UNBOUNDED_BUDGET,
    );
    std::hint::black_box((doc, highlighter));
}

fn open_instruction_cases(corpora: &Corpora) {
    instruction_case("open_300kb_highlighted", || {
        open_with_first_viewport("bench-300kb.rs", &corpora.highlighted_rust);
    });
    instruction_case("open_2mb_to_text", || {
        open_with_first_viewport("bench-2mb.rs", &corpora.reload_rust);
    });
    instruction_case("open_3_7mb", || {
        let doc = document("bench-3-7mb.rs", &corpora.large_rust);
        let highlighter = CodeHighlighter::new(&doc, dark());
        std::hint::black_box((doc, highlighter));
    });
    instruction_case("open_markdown_injected", || {
        let doc = document("bench-inline.md", &corpora.markdown);
        let highlighter = CodeHighlighter::new(&doc, dark());
        std::hint::black_box((doc, highlighter));
    });
    instruction_case("open_to_first_color_300kb", || {
        let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        let rows = 0..VIEWPORT_ROWS.min(doc.line_count());
        if !viewport_colored(&highlighter, rows.clone())
            && let Some(parse) = highlighter.initial_parse(&doc)
        {
            highlighter.apply_parsed(&doc, parse.run());
            highlighter.fill_stale_rows(&doc, rows, UNBOUNDED_BUDGET);
        }
        std::hint::black_box((doc, highlighter));
    });
    instruction_case("open_to_first_tree_300kb", || {
        let doc = document("bench-300kb.rs", &corpora.highlighted_rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        if let Some(parse) = highlighter.initial_parse(&doc) {
            highlighter.apply_parsed(&doc, parse.run());
        }
        highlighter.fill_stale_rows(
            &doc,
            0..VIEWPORT_ROWS.min(doc.line_count()),
            UNBOUNDED_BUDGET,
        );
        std::hint::black_box((doc, highlighter));
    });
}

fn edit_instruction_cases(corpora: &Corpora) {
    let mut doc = document("bench-300kb.rs", &corpora.highlighted_rust);
    let mut highlighter = parsed(&doc, dark());
    let mut index = 0usize;
    instruction_case("keystroke_to_runs", || {
        for _ in 0..INSTRUCTION_KEYSTROKES {
            index += 1;
            let row = stride(index, doc.line_count());
            let offset = doc.line_to_byte(row);
            apply_unbounded_edit(&mut doc, &mut highlighter, offset..offset, "x");
            let end = (row + VIEWPORT_ROWS).min(doc.line_count());
            highlighter.fill_stale_rows(&doc, row..end, UNBOUNDED_BUDGET);
        }
    });
    let span = doc.line_count().saturating_sub(VIEWPORT_ROWS).max(1);
    instruction_case("viewport_query_60_rows", || {
        for _ in 0..INSTRUCTION_VIEWPORTS {
            index += 1;
            let first = stride(index, span);
            highlighter.requery_rows(&doc, first..first + VIEWPORT_ROWS);
        }
    });
    instruction_case("theme_switch", || {
        index += 1;
        let syntax = if index.is_multiple_of(2) {
            dark()
        } else {
            light()
        };
        highlighter.set_syntax(syntax);
    });
    std::hint::black_box((doc, highlighter));

    let mut plain = document("bench-3-7mb.rs", &corpora.large_rust);
    let mut plain_highlighter = CodeHighlighter::new(&plain, dark());
    instruction_case("keystroke_3_7mb_plain", || {
        for _ in 0..INSTRUCTION_KEYSTROKES {
            index += 1;
            let row = stride(index, plain.line_count());
            let offset = plain.line_to_byte(row);
            apply_unbounded_edit(&mut plain, &mut plain_highlighter, offset..offset, "x");
        }
    });
    let end = plain.len_bytes();
    instruction_case("byte_to_utf16_eof", || {
        for _ in 0..INSTRUCTION_UTF16_LOOKUPS {
            std::hint::black_box(plain.byte_to_utf16(end));
        }
    });
    instruction_case("to_disk_string_3_7mb", || {
        std::hint::black_box(plain.to_disk_string());
    });
    std::hint::black_box((plain, plain_highlighter));
}

fn text_instruction_cases(corpora: &Corpora) {
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
    instruction_case("resolve_runs_3750", || {
        let mut input = runs.clone();
        resolve_runs(&mut input);
        std::hint::black_box(input);
    });
    let rewritten = rewrite_hunks(&corpora.large_rust);
    let original = document("bench-3-7mb.rs", &corpora.large_rust);
    instruction_case("disk_splices_100k_lines", || {
        std::hint::black_box(disk_splices(original.text(), &rewritten));
    });
    let textdiff = &corpora.textdiff;
    let pairs = [
        (
            "textdiff_300kb_50_blocks",
            &textdiff.rust,
            &textdiff.rust_edited,
        ),
        (
            "textdiff_5k_lines_one_word_each",
            &textdiff.word_lines,
            &textdiff.word_lines_edited,
        ),
        (
            "textdiff_5k_lines_all_different",
            &textdiff.dense_before,
            &textdiff.dense_after,
        ),
    ];
    for (name, before, after) in pairs {
        instruction_case(name, || {
            std::hint::black_box(compare_lines_inner(
                before,
                after,
                ComparisonPolicy::Default,
                HighlightPolicy::Words,
            ));
        });
    }
}

fn shape_instruction_cases() {
    let Some((platform, text_system)) = platform_text_system() else {
        println!("PANEFLOW_BENCH_SKIP shape instruction cases: no platform text system");
        return;
    };
    let font = editor_font();
    let warm = ascii_rows(0);
    let mut salt = 0usize;
    instruction_case("shape_cold_60_rows", || {
        salt += 1;
        std::hint::black_box(shape_rows(&text_system, &font, &ascii_rows(salt)));
    });
    instruction_case("shape_warm_60_rows", || {
        std::hint::black_box(shape_rows(&text_system, &font, &warm));
    });
    let warm_doc = ascii_document(0);
    let mut runs = Vec::with_capacity(64);
    let mut shapes = ShapeCache::default();
    instruction_case("prepaint_60_rows_warm", || {
        std::hint::black_box(prepaint_rows(
            &text_system,
            &font,
            &warm_doc,
            &mut runs,
            &mut shapes,
        ));
    });
    drop(platform);
}

#[test]
#[ignore = "editor instruction counts: run under Callgrind through scripts/perf-ab"]
fn editor_instruction_benchmark() {
    refuse_debug_profile();
    let corpora = Corpora::build();
    open_instruction_cases(&corpora);
    edit_instruction_cases(&corpora);
    text_instruction_cases(&corpora);
    shape_instruction_cases();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_parent_counts_the_tests_its_child_ran() {
        assert_eq!(
            passed_test_count("running 1 test\ntest result: ok. 1 passed; 0 failed; 0 ignored"),
            Some(1)
        );
        assert_eq!(
            passed_test_count("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured"),
            Some(0)
        );
        assert_eq!(
            passed_test_count("test result: FAILED. 1 passed; 1 failed; 0 ignored"),
            Some(2)
        );
        assert_eq!(passed_test_count("no summary"), None);
        assert!(
            tree_memory_probe_child_test_name().starts_with("app::diff_dock::code::perf_bench::")
        );
    }

    #[test]
    fn every_corpus_reaches_the_path_the_bench_claims() {
        let rust = rust_source(8_192);
        let doc = document("probe.rs", &rust);
        let mut highlighter = parsed(&doc, dark());
        assert!(highlighter.is_enabled(), "the rust corpus must highlight");
        highlighter.requery_rows(&doc, 0..doc.line_count());
        assert!(
            (0..doc.line_count()).any(|row| !highlighter.runs(row).is_empty()),
            "the rust corpus must produce colored runs"
        );

        let markdown = markdown_source(8_192);
        let doc = document("probe.md", &markdown);
        let highlighter = parsed(&doc, dark());
        assert!(
            highlighter.is_enabled(),
            "the markdown corpus must highlight"
        );
        assert!(
            highlighter.has_tree(),
            "the markdown corpus must reach both of its grammar passes"
        );

        let json = minified_json_line(2_048);
        let doc = document("probe.json", &json);
        let highlighter = parsed(&doc, dark());
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
    fn the_reload_plan_descends_by_whole_rows_one_hunk_at_a_time() {
        let source = rust_source(200_000);
        let doc = document("probe.rs", &source);
        let rewritten = rewrite_hunks(&source);
        let ops = disk_splices(doc.text(), &rewritten);
        assert_eq!(
            ops.len(),
            RELOAD_HUNKS,
            "the rewrite must produce one hunk per replaced line"
        );
        for pair in ops.windows(2) {
            assert!(
                doc.byte_to_line(pair[1].0.end) < doc.byte_to_line(pair[0].0.start),
                "the reload plan must descend by whole rows so it batches"
            );
        }
    }

    #[test]
    fn a_viewport_requery_colors_the_rows_it_covers() {
        let rust = rust_source(32_768);
        let doc = document("probe.rs", &rust);
        let mut highlighter = parsed(&doc, dark());
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
        let mut highlighter = parsed(&doc, dark());
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
    fn the_pagedown_probe_leaves_no_stale_row_even_without_a_budget() {
        let rust = rust_source(64_000);
        let doc = document("probe.rs", &rust);
        let mut highlighter = parsed(&doc, dark());
        assert!(highlighter.is_enabled(), "the probe needs a colored file");

        let starved = pagedown_probe(&doc, &mut highlighter, Duration::ZERO);
        assert_eq!(
            starved.stale_max, 0,
            "a 60-row viewport is one ranged query, so even a zero budget colors it whole"
        );
        assert_eq!(starved.frames_to_fresh, 1);

        let mut fresh = parsed(&doc, dark());
        let generous = pagedown_probe(&doc, &mut fresh, Duration::from_secs(1));
        assert_eq!(generous.stale_max, 0);
        assert_eq!(generous.frames_to_fresh, 1);
    }

    #[test]
    fn the_fill_scenario_walks_disjoint_never_queried_viewports() {
        let rust = rust_source(64_000);
        let doc = document("probe.rs", &rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        let windows = fill_windows(&doc);
        assert!(windows > 1, "the corpus must hold several viewports");

        for index in 0..windows {
            let first = index * VIEWPORT_ROWS;
            let rows = first..first + VIEWPORT_ROWS;
            assert_eq!(
                highlighter.stale_rows_in(rows.clone()),
                VIEWPORT_ROWS,
                "viewport {index} was already queried"
            );
            let fill = highlighter.fill_stale_rows(&doc, rows, HIGHLIGHT_FRAME_BUDGET);
            assert_eq!(fill.stale_rows, 0, "viewport {index} stayed stale");
        }
    }

    #[test]
    fn the_pagedown_probe_spends_no_budget_past_the_highlight_cap() {
        let rust = rust_source(crate::diff::MAX_HIGHLIGHT_BYTES + 4_096);
        let doc = document("probe.rs", &rust);
        let mut highlighter = CodeHighlighter::new(&doc, dark());
        assert!(!highlighter.is_enabled(), "the file must be past the cap");

        let probe = pagedown_probe(&doc, &mut highlighter, Duration::ZERO);
        assert_eq!(probe.stale_median, 0);
        assert_eq!(probe.stale_max, 0);
        assert_eq!(probe.frames_to_fresh, 1);
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
        let font = editor_font();
        let width = shape_rows(&text_system, &font, &ascii_rows(0));
        assert!(
            width.is_finite() && width >= 0.0,
            "a shaped viewport must report a finite width, got {width}"
        );

        let doc = ascii_document(0);
        let mut runs = Vec::with_capacity(64);
        let mut shapes = ShapeCache::default();
        let cold = prepaint_rows(&text_system, &font, &doc, &mut runs, &mut shapes);
        let warm = prepaint_rows(&text_system, &font, &doc, &mut runs, &mut shapes);
        assert_eq!(
            cold, warm,
            "the shape cache must return the same layout it cached"
        );
        assert_eq!(
            runs.len(),
            1,
            "the prepaint probe must reuse one run buffer across every row"
        );
        drop(platform);
    }

    #[test]
    fn the_platform_text_system_shapes_unicode_line_and_paragraph_separators() {
        let Some((platform, text_system)) = platform_text_system() else {
            return;
        };
        let font = editor_font();
        let rows: Vec<SharedString> = ["a\u{2028}b", "\u{2029}", "agent \u{2028}\u{2029} output"]
            .into_iter()
            .map(SharedString::from)
            .collect();
        let width = shape_rows(&text_system, &font, &rows);
        assert!(
            width.is_finite() && width >= 0.0,
            "a line holding U+2028 or U+2029 must shape to a finite width, got {width}"
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
