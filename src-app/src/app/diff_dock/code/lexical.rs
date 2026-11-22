use std::borrow::Cow;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::Hsla;
use paneflow_highlights::{Language, LexDocument, LexError, Lexer, Token, language_for_path};
use ropey::Rope;

use crate::diff::{DiffSyntax, highlight_cap};

use super::document::{CodeDocument, CodeEdit};
use super::highlight::LineRuns;

pub(crate) const MAX_LEXICAL_BYTES: usize = super::load::MAX_FILE_BYTES;

const END_MASK: u32 = 0x00ff_ffff;
const ID_SHIFT: u32 = 24;
pub(crate) const INITIAL_LEXICAL_ROWS: usize = 128;
pub(crate) const IDLE_SLICE_BUDGET: Duration = Duration::from_micros(500);
const FIRST_SCAN_SLICE_LINES: usize = 8;
const SCAN_SLICE_LINES: usize = 256;
const BACKGROUND_SLICE_LINES: u32 = 4_096;
const EDIT_TAIL_ROWS: usize = 64;
const PLAIN_ID: u8 = 0;

type PackedRow = Box<[u32]>;

static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);

enum LexState {
    Ready(LexDocument),
    Pending,
}

pub(crate) struct LexicalLayer {
    language: Language,
    ext: String,
    state: LexState,
    rows: Vec<PackedRow>,
    colors: Vec<Option<Hsla>>,
    generation: Arc<AtomicU64>,
    in_flight: bool,
    instance: u64,
    cancel: Arc<AtomicBool>,
}

pub(crate) struct LexJob {
    instance: u64,
    generation: u64,
    current: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    rope: Rope,
    language: Language,
}

enum LexFailure {
    Superseded,
    Failed(LexError),
}

type LexedRows = (LexDocument, Vec<PackedRow>);

pub(crate) struct Lexed {
    instance: u64,
    generation: u64,
    outcome: Result<LexedRows, LexFailure>,
}

pub(crate) enum Applied {
    Installed,
    Ignored,
    Failed(LexError),
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
    id: u8,
}

fn rope_text(rope: &Rope) -> Cow<'_, str> {
    match rope.chunks().nth(1) {
        None => Cow::Borrowed(rope.chunks().next().unwrap_or("")),
        Some(_) => Cow::Owned(rope.to_string()),
    }
}

fn is_blank(text: &str, range: &Range<usize>) -> bool {
    text.as_bytes()
        .get(range.clone())
        .is_some_and(|bytes| bytes.iter().all(|byte| matches!(byte, b' ' | b'\t')))
}

fn push_span(out: &mut Vec<u32>, end: usize, id: u8) -> bool {
    let Ok(end) = u32::try_from(end) else {
        return false;
    };
    if end > END_MASK {
        return false;
    }
    let word = end | (u32::from(id) << ID_SHIFT);
    match out.last_mut() {
        Some(last) if (*last >> ID_SHIFT) as u8 == id => *last = word,
        _ => out.push(word),
    }
    true
}

fn pack(spans: impl Iterator<Item = (Span, bool)>) -> PackedRow {
    let mut out: Vec<u32> = Vec::new();
    let mut cursor = 0usize;
    for (span, blank) in spans {
        if span.end <= cursor {
            continue;
        }
        if span.start > cursor && !push_span(&mut out, span.start, PLAIN_ID) {
            return PackedRow::default();
        }
        let id = match out.last() {
            Some(last) if blank => (*last >> ID_SHIFT) as u8,
            _ => span.id,
        };
        if !push_span(&mut out, span.end, id) {
            return PackedRow::default();
        }
        cursor = span.end;
    }
    out.into_boxed_slice()
}

fn unpack(row: &[u32]) -> impl Iterator<Item = Span> + '_ {
    let mut start = 0usize;
    row.iter().map(move |word| {
        let end = (word & END_MASK) as usize;
        let span = Span {
            start,
            end,
            id: (word >> ID_SHIFT) as u8,
        };
        start = end;
        span
    })
}

fn token_spans<'a>(
    text: &'a str,
    tokens: &'a [Token],
    offset: usize,
) -> impl Iterator<Item = (Span, bool)> + 'a {
    tokens.iter().map(move |token| {
        (
            Span {
                start: token.range.start + offset,
                end: token.range.end + offset,
                id: token.id.get(),
            },
            is_blank(text, &token.range),
        )
    })
}

fn read_rows(
    lexed: &LexDocument,
    rope: &Rope,
    rows: Range<usize>,
    mut put: impl FnMut(usize, PackedRow),
) -> Result<(), LexError> {
    let line_count = rope.len_lines();
    let rows = rows.start.min(line_count)..rows.end.min(line_count);
    if rows.is_empty() {
        return Ok(());
    }
    if lexed.line_count() == line_count {
        return lexed.for_each_line(rows, |line, text, tokens| {
            put(line, pack(token_spans(text, tokens, 0)));
        });
    }
    let first_byte = rope.line_to_byte(rows.start);
    let end_byte = if rows.end < line_count {
        rope.line_to_byte(rows.end)
    } else {
        rope.len_bytes() + 1
    };
    let first_line = lexed.line_at(first_byte);
    let end_line = lexed.line_at(end_byte.saturating_sub(1)) + 1;
    let mut row = rows.start;
    let mut spans: Vec<(Span, bool)> = Vec::new();
    lexed.for_each_line(first_line..end_line, |line, text, tokens| {
        let start = lexed.line_start(line).unwrap_or(0);
        let line_row = rope.byte_to_line(start.min(rope.len_bytes()));
        if line_row != row {
            put(row, pack(spans.drain(..)));
            row = line_row;
        }
        let offset = start.saturating_sub(rope.line_to_byte(line_row));
        spans.extend(token_spans(text, tokens, offset));
    })?;
    put(row, pack(spans.drain(..)));
    Ok(())
}

fn identity_lines(lexed: &LexDocument, rope: &Rope) -> bool {
    lexed.line_count() == rope.len_lines()
}

fn row_of_line(lexed: &LexDocument, rope: &Rope, line: usize) -> usize {
    if identity_lines(lexed, rope) {
        return line;
    }
    let start = lexed.line_start(line).unwrap_or(lexed.len());
    rope.byte_to_line(start.min(rope.len_bytes()))
}

fn line_of_row(lexed: &LexDocument, rope: &Rope, row: usize) -> usize {
    if identity_lines(lexed, rope) {
        return row.min(lexed.line_count());
    }
    if row >= rope.len_lines() {
        return lexed.line_count();
    }
    lexed.line_at(rope.line_to_byte(row))
}

struct Scan {
    rewritten: Option<Range<usize>>,
    reached: bool,
}

fn cover(rows: &mut Option<Range<usize>>, more: Range<usize>) {
    *rows = Some(match rows.take() {
        Some(seen) => seen.start.min(more.start)..seen.end.max(more.end),
        None => more,
    });
}

fn scan_until(
    lexed: &mut LexDocument,
    rope: &Rope,
    rows: &mut [PackedRow],
    limit_row: usize,
    deadline: Option<Instant>,
    at_least_one_slice: bool,
) -> Result<Scan, LexError> {
    let limit = line_of_row(lexed, rope, limit_row.min(rope.len_lines()));
    let mut rewritten = None;
    let mut slice = FIRST_SCAN_SLICE_LINES;
    loop {
        let reached = match lexed.next_pending_line()? {
            None => true,
            Some(next) => next >= limit,
        };
        let may_skip = rewritten.is_some() || !at_least_one_slice;
        let out_of_time = may_skip && deadline.is_some_and(|deadline| Instant::now() >= deadline);
        if reached || out_of_time {
            return Ok(Scan { rewritten, reached });
        }
        let Some(next) = lexed.next_pending_line()? else {
            return Ok(Scan {
                rewritten,
                reached: true,
            });
        };
        let end = (next + slice).min(limit);
        lexed.run_before(slice as u32, end)?;
        slice = (slice * 2).min(SCAN_SLICE_LINES);
        let reached_line = lexed
            .next_pending_line()?
            .map_or(end, |after| after.min(end));
        if reached_line <= next {
            return Err(LexError::OutOfBounds);
        }
        let first_row = row_of_line(lexed, rope, next);
        let last_row = row_of_line(lexed, rope, reached_line - 1);
        read_rows(lexed, rope, first_row..last_row + 1, |row, packed| {
            if let Some(slot) = rows.get_mut(row) {
                *slot = packed;
            }
        })?;
        cover(&mut rewritten, first_row..last_row + 1);
    }
}

fn new_document(rope: &Rope, language: Language) -> Result<LexDocument, LexError> {
    #[cfg(test)]
    LEXES.with(|count| count.set(count.get() + 1));
    #[cfg(test)]
    if let Some(err) = injected_failure() {
        return Err(err);
    }
    let text = rope_text(rope);
    Lexer::new().and_then(|lexer| lexer.document(&text, language))
}

fn stage_edits(
    lexed: &mut LexDocument,
    doc: &CodeDocument,
    edits: &[CodeEdit],
) -> Result<usize, LexError> {
    let mut last_row = 0;
    for (index, edit) in edits.iter().enumerate() {
        let start = final_offset(edits, index, edit.start_byte);
        let inserted = edit.new_end_byte.saturating_sub(edit.start_byte);
        let text = if inserted == 0 {
            String::new()
        } else {
            doc.slice_string(start..start + inserted)
        };
        if text.len() != inserted {
            return Err(LexError::InvalidRange {
                start,
                end: start + inserted,
            });
        }
        lexed.apply_edit(edit.start_byte..edit.old_end_byte, &text)?;
        last_row = last_row.max(doc.byte_to_line(start + inserted));
    }
    Ok(last_row)
}

fn lex_all(
    rope: &Rope,
    language: Language,
    superseded: impl Fn() -> bool,
) -> Result<LexedRows, LexFailure> {
    let mut lexed = new_document(rope, language).map_err(LexFailure::Failed)?;
    while lexed.pending_lines().map_err(LexFailure::Failed)? > 0 {
        if superseded() {
            return Err(LexFailure::Superseded);
        }
        lexed
            .run(BACKGROUND_SLICE_LINES)
            .map_err(LexFailure::Failed)?;
    }
    let mut rows = Vec::with_capacity(rope.len_lines());
    read_rows(&lexed, rope, 0..rope.len_lines(), |_, row| rows.push(row))
        .map_err(LexFailure::Failed)?;
    if rows.len() != rope.len_lines() {
        return Err(LexFailure::Failed(LexError::OutOfBounds));
    }
    Ok((lexed, rows))
}

fn final_offset(edits: &[CodeEdit], index: usize, byte: usize) -> usize {
    edits[index + 1..].iter().fold(byte, |at, later| {
        if later.old_end_byte <= at {
            (at + later.new_end_byte).saturating_sub(later.old_end_byte)
        } else {
            at
        }
    })
}

fn shifted_head(row: Option<&PackedRow>, start_col: usize) -> Vec<Span> {
    row.map(|row| {
        unpack(row)
            .filter(|span| span.start < start_col)
            .map(|span| Span {
                end: span.end.min(start_col),
                ..span
            })
            .collect()
    })
    .unwrap_or_default()
}

fn shifted_tail(row: Option<&PackedRow>, old_end_col: usize, new_end_col: usize) -> Vec<Span> {
    row.map(|row| {
        unpack(row)
            .filter(|span| span.end > old_end_col)
            .map(|span| Span {
                start: span.start.max(old_end_col) - old_end_col + new_end_col,
                end: span.end - old_end_col + new_end_col,
                id: span.id,
            })
            .collect()
    })
    .unwrap_or_default()
}

fn repack(spans: Vec<Span>) -> PackedRow {
    pack(spans.into_iter().map(|span| (span, false)))
}

#[cfg(test)]
thread_local! {
    static DISABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static INJECTED: std::cell::RefCell<Option<LexError>> = const { std::cell::RefCell::new(None) };
    static LEXES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn lex_count_for_test() -> usize {
    LEXES.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) const DISABLE_ENV: &str = "PANEFLOW_BENCH_WITHOUT_LAYER1";

#[cfg(test)]
pub(crate) fn set_disabled_for_test(disabled: bool) {
    DISABLED.with(|cell| cell.set(disabled));
}

#[cfg(test)]
fn disabled_for_test() -> bool {
    DISABLED.with(std::cell::Cell::get) || std::env::var_os(DISABLE_ENV).is_some()
}

#[cfg(test)]
pub(crate) fn inject_failure_for_test(err: LexError) {
    INJECTED.with(|cell| *cell.borrow_mut() = Some(err));
}

#[cfg(test)]
fn injected_failure() -> Option<LexError> {
    INJECTED.with(|cell| cell.borrow_mut().take())
}

impl LexicalLayer {
    pub(crate) fn open(doc: &CodeDocument, syntax: &DiffSyntax) -> Option<Self> {
        #[cfg(test)]
        if disabled_for_test() {
            return None;
        }
        let language = language_for_path(doc.path())?;
        if doc.len_bytes() > MAX_LEXICAL_BYTES {
            return None;
        }
        let mut layer = Self {
            language,
            ext: doc.ext().to_string(),
            state: LexState::Pending,
            rows: Vec::new(),
            colors: syntax.lexical_colors(),
            generation: Arc::new(AtomicU64::new(0)),
            in_flight: false,
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if doc.len_bytes() <= highlight_cap(doc.ext()) {
            let rope = doc.text();
            let opened = new_document(rope, language).and_then(|mut lexed| {
                let mut rows = vec![PackedRow::default(); rope.len_lines()];
                scan_until(
                    &mut lexed,
                    rope,
                    &mut rows,
                    INITIAL_LEXICAL_ROWS,
                    None,
                    true,
                )?;
                Ok((lexed, rows))
            });
            match opened {
                Ok((lexed, rows)) => {
                    layer.state = LexState::Ready(lexed);
                    layer.rows = rows;
                }
                Err(err) => {
                    layer.warn(&err);
                    return None;
                }
            }
        }
        Some(layer)
    }

    pub(crate) fn warn(&self, err: &LexError) {
        tracing::warn!(
            target: "paneflow_app::code::lexical",
            "lexical coloring is off for this .{} file: {err}",
            self.ext
        );
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        match &self.state {
            LexState::Ready(lexed) => matches!(lexed.next_pending_line(), Ok(None)),
            LexState::Pending => false,
        }
    }

    pub(crate) fn has_pending_scan(&self) -> bool {
        match &self.state {
            LexState::Ready(lexed) => !matches!(lexed.next_pending_line(), Ok(None)),
            LexState::Pending => false,
        }
    }

    pub(crate) fn needs_job(&self) -> bool {
        matches!(self.state, LexState::Pending) && !self.in_flight
    }

    fn scan(
        &mut self,
        doc: &CodeDocument,
        limit_row: usize,
        deadline: Option<Instant>,
        at_least_one_slice: bool,
    ) -> Option<Scan> {
        let LexState::Ready(lexed) = &mut self.state else {
            return None;
        };
        self.rows.resize_with(doc.line_count(), PackedRow::default);
        match scan_until(
            lexed,
            doc.text(),
            &mut self.rows,
            limit_row,
            deadline,
            at_least_one_slice,
        ) {
            Ok(scan) => Some(scan),
            Err(err) => {
                tracing::debug!(
                    target: "paneflow_app::code::lexical",
                    "the lexical scan of this .{} file failed, relexing it: {err}",
                    self.ext
                );
                self.state = LexState::Pending;
                None
            }
        }
    }

    pub(crate) fn fill(
        &mut self,
        doc: &CodeDocument,
        rows: Range<usize>,
        deadline: Instant,
    ) -> bool {
        self.scan(doc, rows.end, Some(deadline), true)
            .is_some_and(|scan| scan.reached)
    }

    pub(crate) fn idle_slice(
        &mut self,
        doc: &CodeDocument,
        budget: Duration,
    ) -> Option<Range<usize>> {
        let deadline = Instant::now() + budget;
        self.scan(doc, doc.line_count(), Some(deadline), true)?
            .rewritten
    }

    pub(crate) fn set_syntax(&mut self, syntax: &DiffSyntax) {
        self.colors = syntax.lexical_colors();
    }

    pub(crate) fn push_runs(&self, row: usize, clip: Range<usize>, out: &mut LineRuns) {
        let Some(packed) = self.rows.get(row) else {
            return;
        };
        for span in unpack(packed) {
            let start = span.start.max(clip.start);
            let end = span.end.min(clip.end);
            if start >= end {
                continue;
            }
            let Some(color) = self.colors.get(usize::from(span.id)).copied().flatten() else {
                continue;
            };
            match out.last_mut() {
                Some((range, last)) if range.end == start && *last == color => range.end = end,
                _ => out.push((start..end, color)),
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn drop_document_for_test(&mut self) {
        self.state = LexState::Pending;
    }

    #[cfg(test)]
    pub(crate) fn linear_bytes(&self) -> usize {
        match &self.state {
            LexState::Ready(lexed) => lexed.memory_bytes(),
            LexState::Pending => 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        let document = match &self.state {
            LexState::Ready(lexed) => lexed.memory_bytes(),
            LexState::Pending => 0,
        };
        let rows = self.rows.capacity() * std::mem::size_of::<PackedRow>()
            + self
                .rows
                .iter()
                .map(|row| row.len() * std::mem::size_of::<u32>())
                .sum::<usize>();
        document + rows
    }

    fn bump_generation(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn edit(&mut self, doc: &CodeDocument, edits: &[CodeEdit], budget: Duration) {
        self.bump_generation();
        if !self.rows.is_empty() {
            self.shift_rows(doc, edits);
        }
        let LexState::Ready(lexed) = &mut self.state else {
            return;
        };
        let last_row = match stage_edits(lexed, doc, edits) {
            Ok(last_row) => last_row,
            Err(err) => {
                tracing::debug!(
                    target: "paneflow_app::code::lexical",
                    "an edit ended the lexical document of this .{} file, relexing it: {err}",
                    self.ext
                );
                self.state = LexState::Pending;
                return;
            }
        };
        let limit = (last_row + 1 + EDIT_TAIL_ROWS).min(doc.line_count());
        self.scan(doc, limit, Some(Instant::now() + budget), false);
    }

    fn shift_rows(&mut self, doc: &CodeDocument, edits: &[CodeEdit]) {
        for edit in edits {
            let start_row = edit.start_point.row;
            let old_end_row = edit.old_end_point.row.max(start_row);
            let new_end_row = edit.new_end_point.row.max(start_row);
            let prefix = shifted_head(self.rows.get(start_row), edit.start_point.column);
            let suffix = shifted_tail(
                self.rows.get(old_end_row),
                edit.old_end_point.column,
                edit.new_end_point.column,
            );
            let mut replacement = Vec::with_capacity(new_end_row - start_row + 1);
            if new_end_row == start_row {
                let mut merged = prefix;
                merged.extend(suffix);
                replacement.push(repack(merged));
            } else {
                replacement.push(repack(prefix));
                replacement.resize_with(new_end_row - start_row, PackedRow::default);
                replacement.push(repack(suffix));
            }
            let removed_end = (old_end_row + 1).min(self.rows.len());
            self.rows
                .splice(start_row.min(removed_end)..removed_end, replacement);
        }
        self.rows.resize_with(doc.line_count(), PackedRow::default);
    }

    pub(crate) fn job(&mut self, doc: &CodeDocument) -> Option<LexJob> {
        if !self.needs_job() {
            return None;
        }
        self.in_flight = true;
        Some(LexJob {
            instance: self.instance,
            generation: self.generation.load(Ordering::Relaxed),
            current: self.generation.clone(),
            cancel: self.cancel.clone(),
            rope: doc.text().clone(),
            language: self.language,
        })
    }

    pub(crate) fn apply(&mut self, doc: &CodeDocument, lexed: Lexed) -> Applied {
        if lexed.instance != self.instance {
            return Applied::Ignored;
        }
        self.in_flight = false;
        if lexed.generation != self.generation.load(Ordering::Relaxed)
            || matches!(self.state, LexState::Ready(_))
        {
            return Applied::Ignored;
        }
        match lexed.outcome {
            Ok((document, rows)) if rows.len() == doc.line_count() => {
                self.state = LexState::Ready(document);
                self.rows = rows;
                Applied::Installed
            }
            Ok(_) | Err(LexFailure::Superseded) => Applied::Ignored,
            Err(LexFailure::Failed(err)) => Applied::Failed(err),
        }
    }
}

impl Drop for LexicalLayer {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl LexJob {
    pub(crate) fn run(self) -> Lexed {
        let LexJob {
            instance,
            generation,
            current,
            cancel,
            rope,
            language,
        } = self;
        let superseded =
            || cancel.load(Ordering::Relaxed) || current.load(Ordering::Relaxed) != generation;
        let outcome = if superseded() {
            Err(LexFailure::Superseded)
        } else {
            lex_all(&rope, language, superseded)
        };
        Lexed {
            instance,
            generation,
            outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::theme::paneflow_dark;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&paneflow_dark())
    }

    fn doc(name: &str, text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from(format!("/tmp/{name}")), text)
    }

    fn rows_of(layer: &LexicalLayer) -> Vec<Vec<(usize, usize, u8)>> {
        layer
            .rows
            .iter()
            .map(|row| {
                unpack(row)
                    .map(|span| (span.start, span.end, span.id))
                    .collect()
            })
            .collect()
    }

    fn settle(layer: &mut LexicalLayer, doc: &CodeDocument) {
        while layer.has_pending_scan() {
            layer.idle_slice(doc, Duration::from_secs(60));
        }
    }

    fn fresh_rows(doc: &CodeDocument) -> Vec<Vec<(usize, usize, u8)>> {
        let mut layer = LexicalLayer::open(doc, &syntax()).expect("a lexical layer");
        settle(&mut layer, doc);
        rows_of(&layer)
    }

    #[test]
    fn packing_merges_blank_tokens_and_equal_neighbours_and_fills_gaps_as_plain() {
        let spans = [
            (
                Span {
                    start: 0,
                    end: 4,
                    id: 7,
                },
                true,
            ),
            (
                Span {
                    start: 4,
                    end: 7,
                    id: 9,
                },
                false,
            ),
            (
                Span {
                    start: 7,
                    end: 8,
                    id: 3,
                },
                true,
            ),
            (
                Span {
                    start: 8,
                    end: 10,
                    id: 9,
                },
                false,
            ),
            (
                Span {
                    start: 12,
                    end: 15,
                    id: 5,
                },
                false,
            ),
        ];
        let packed = pack(spans.into_iter());
        let unpacked: Vec<_> = unpack(&packed).map(|s| (s.start, s.end, s.id)).collect();
        assert_eq!(
            unpacked,
            vec![(0, 4, 7), (4, 10, 9), (10, 12, PLAIN_ID), (12, 15, 5)]
        );
    }

    #[test]
    fn a_row_past_the_packed_offset_range_is_left_plain() {
        let span = Span {
            start: 0,
            end: END_MASK as usize + 1,
            id: 4,
        };
        assert!(pack(std::iter::once((span, false))).is_empty());
    }

    #[test]
    fn a_lone_carriage_return_maps_lexer_lines_back_to_editor_rows() {
        let text = "let a = 1;\rlet b = 2;\nlet c = 3;\n";
        let d = doc("lone.rs", text);
        assert_eq!(d.line_count(), 3);
        let layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        let LexState::Ready(lexed) = &layer.state else {
            panic!("a small file is lexed at open");
        };
        assert_eq!(lexed.line_count(), 4, "the lexer breaks on the lone CR");
        let rows = rows_of(&layer);
        assert_eq!(rows.len(), 3);
        let first: Vec<_> = rows[0].iter().map(|(s, e, _)| (*s, *e)).collect();
        assert_eq!(first.last().map(|(_, end)| *end), Some(21));
        assert!(
            rows[0].iter().any(|(start, _, id)| *start >= 11
                && paneflow_highlights::token_types()[usize::from(*id)].starts_with("keyword")),
            "the second lexer line lands after the CR on the same row: {:?}",
            rows[0]
        );
        assert_eq!(rows[1], fresh_rows(&doc("c.rs", "let c = 3;\n"))[0]);
    }

    #[test]
    fn a_keystroke_inside_the_budget_relexes_on_the_spot_and_matches_a_fresh_lex() {
        let mut d = doc("edit.rs", "fn main() {\n    let a = 1;\n}\nfn b() {}\n");
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        for (at, text) in [(16, "/* "), (27, "x"), (16, "")] {
            let edit = if text.is_empty() {
                d.remove(16..19).expect("remove")
            } else {
                d.insert(at, text).expect("insert")
            };
            layer.edit(&d, std::slice::from_ref(&edit), Duration::from_secs(5));
            assert!(layer.is_ready(), "the relex fit the budget");
            assert_eq!(rows_of(&layer), fresh_rows(&d), "after {text:?} at {at}");
        }
    }

    #[test]
    fn a_descending_batch_lands_each_hunk_at_its_final_offset() {
        let text: String = (0..40)
            .map(|row| format!("let v{row} = {row};\n"))
            .collect();
        let mut d = doc("batch.rs", &text);
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        let high = d
            .insert(d.line_to_byte(30), "/* a */ \"b\"\n")
            .expect("insert");
        let middle = d
            .remove(d.line_to_byte(12)..d.line_to_byte(14))
            .expect("remove");
        let low = d.insert(d.line_to_byte(3) + 4, "mut ").expect("insert");
        layer.edit(&d, &[high, middle, low], Duration::from_secs(5));
        assert!(layer.is_ready());
        assert_eq!(rows_of(&layer), fresh_rows(&d));
    }

    #[test]
    fn a_zero_budget_keeps_the_old_runs_shifted_until_the_scan_reaches_them() {
        let mut d = doc("shift.rs", "let value = 1;\nlet other = 2;\n");
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        let before = rows_of(&layer);
        let edit = d.insert(0, "xx").expect("insert");
        layer.edit(&d, std::slice::from_ref(&edit), Duration::ZERO);
        assert!(!layer.is_ready(), "a zero budget scans nothing");
        assert!(layer.has_pending_scan());
        assert!(
            layer.job(&d).is_none(),
            "an unfinished scan never needs a relex"
        );
        let shifted = rows_of(&layer);
        let expected: Vec<_> = before[0]
            .iter()
            .map(|(start, end, id)| (start + 2, end + 2, *id))
            .collect();
        assert_eq!(
            shifted[0][0],
            (0, 2, PLAIN_ID),
            "the inserted bytes are plain"
        );
        assert_eq!(&shifted[0][1..], expected.as_slice());
        assert_eq!(shifted[1], before[1], "an untouched row keeps its runs");

        let rewritten = layer
            .idle_slice(&d, Duration::from_secs(5))
            .expect("the scan rewrote rows");
        assert_eq!(rewritten.start, 0, "starting at the edited row");
        assert!(layer.is_ready());
        assert_eq!(rows_of(&layer), fresh_rows(&d));
    }

    #[test]
    fn the_visible_fill_scans_only_up_to_the_last_shown_row() {
        let text: String = (0..2_000)
            .map(|row| format!("let v{row} = {row};\n"))
            .collect();
        let mut d = doc("visible.rs", &text);
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        let LexState::Ready(lexed) = &layer.state else {
            panic!("a small file opens with a document");
        };
        assert_eq!(
            lexed.next_pending_line().expect("stats"),
            Some(INITIAL_LEXICAL_ROWS),
            "the open scans the first rows and leaves the rest pending"
        );
        let opened = rows_of(&layer);
        assert!(
            opened[..INITIAL_LEXICAL_ROWS]
                .iter()
                .all(|row| !row.is_empty())
        );
        assert!(opened[INITIAL_LEXICAL_ROWS..].iter().all(Vec::is_empty));

        assert!(layer.fill(&d, 900..960, Instant::now() + Duration::from_secs(5)));
        let LexState::Ready(lexed) = &layer.state else {
            panic!("the fill keeps the document");
        };
        assert_eq!(lexed.next_pending_line().expect("stats"), Some(960));
        let filled = rows_of(&layer);
        assert!(filled[..960].iter().all(|row| !row.is_empty()));
        assert!(filled[960..].iter().all(Vec::is_empty));

        let edit = d.insert(0, "/* ").expect("insert");
        layer.edit(&d, std::slice::from_ref(&edit), Duration::from_secs(5));
        let LexState::Ready(lexed) = &layer.state else {
            panic!("the edit keeps the document");
        };
        assert_eq!(
            lexed.next_pending_line().expect("stats"),
            Some(1 + EDIT_TAIL_ROWS),
            "an edit that comments out the file scans only its tail on the spot"
        );
        settle(&mut layer, &d);
        assert_eq!(rows_of(&layer), fresh_rows(&d));
    }

    #[test]
    fn a_dropped_document_is_relexed_by_exactly_one_job() {
        let mut d = doc("relex.rs", "let value = 1;\n");
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        layer.drop_document_for_test();
        let edit = d.insert(0, "a").expect("insert");
        layer.edit(&d, std::slice::from_ref(&edit), Duration::from_secs(5));
        assert!(!layer.has_pending_scan(), "no document, no scan");
        let job = layer.job(&d).expect("a dropped document hands out one job");
        assert!(layer.job(&d).is_none(), "and only one at a time");
        assert!(matches!(layer.apply(&d, job.run()), Applied::Installed));
        assert!(layer.is_ready());
        assert_eq!(rows_of(&layer), fresh_rows(&d));
    }

    #[test]
    fn a_job_overtaken_by_an_edit_is_dropped_and_the_next_job_lexes_the_current_text() {
        let mut d = doc("race.rs", "let value = 1;\n");
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        layer.drop_document_for_test();
        let stale = layer.job(&d).expect("a job");
        let second = d.insert(0, "b").expect("insert");
        layer.edit(&d, std::slice::from_ref(&second), Duration::ZERO);
        assert!(matches!(layer.apply(&d, stale.run()), Applied::Ignored));
        let current = layer.job(&d).expect("the follow-up job");
        assert!(matches!(layer.apply(&d, current.run()), Applied::Installed));
        assert_eq!(rows_of(&layer), fresh_rows(&d));
    }

    #[test]
    fn a_result_from_another_layer_is_never_installed() {
        let d = doc("a.rs", "let a = 1;\n");
        let mut mine = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        let mut other = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        mine.drop_document_for_test();
        other.drop_document_for_test();
        let foreign = other.job(&d).expect("a job");
        assert!(matches!(mine.apply(&d, foreign.run()), Applied::Ignored));
    }

    #[test]
    fn dropping_the_layer_cancels_its_job() {
        let d = doc("drop.rs", "let value = 1;\n");
        let mut layer = LexicalLayer::open(&d, &syntax()).expect("a lexical layer");
        layer.drop_document_for_test();
        let job = layer.job(&d).expect("a job");
        drop(layer);
        assert!(matches!(job.run().outcome, Err(LexFailure::Superseded)));
    }

    #[test]
    fn plain_text_and_unknown_files_get_no_lexical_layer() {
        assert!(LexicalLayer::open(&doc("notes.txt", "hello\n"), &syntax()).is_none());
        assert!(LexicalLayer::open(&doc("notes.unknownext", "hello\n"), &syntax()).is_none());
        assert!(LexicalLayer::open(&doc("Dockerfile", "FROM rust\n"), &syntax()).is_some());
    }
}
