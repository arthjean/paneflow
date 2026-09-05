use std::ops::{ControlFlow, Range};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(test)]
use gpui::AppContext;
use gpui::{AsyncApp, Context, Hsla, WeakEntity};
use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Node, ParseOptions, ParseState, Parser, Point as TsPoint, QueryCursor, TextProvider,
    Tree,
};

use crate::diff::{
    DiffSyntax, Grammar, MAX_CAPTURES_PER_ROW, grammar_for_ext, highlight_cap, is_markdown,
    markdown_inline_grammar, resolve_runs,
};

use super::document::{CodeDocument, CodeEdit};

pub(crate) const SYNC_PARSE_BUDGET: Duration = Duration::from_millis(1);
pub(crate) const INITIAL_PARSE_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const HIGHLIGHT_FRAME_BUDGET: Duration = Duration::from_millis(2);
pub(crate) const MAX_QUERY_ROWS: usize = 128;

pub(crate) type LineRuns = Vec<(Range<usize>, Hsla)>;

#[derive(Clone, Copy)]
struct IndexedCapture {
    pass: u16,
    capture: u16,
}

type IndexedRun = (u32, u32, IndexedCapture);
type IndexedLineRuns = Vec<IndexedRun>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowState {
    Fresh,
    Stale,
}

struct GrammarPass {
    grammar: &'static Grammar,
    parser: Parser,
    tree: Option<Tree>,
    colors: Vec<Option<Hsla>>,
    cursor: QueryCursor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StaleFill {
    pub(crate) stale_rows: usize,
}

impl StaleFill {
    pub(crate) fn any_stale(self) -> bool {
        self.stale_rows > 0
    }
}

pub(crate) enum HighlightOutcome {
    Synced,
    Deferred(DeferredParse),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct UnorderedBatch;

pub(crate) struct DeferredParse {
    generation: u64,
    rope: Rope,
    passes: Vec<(&'static Grammar, Option<Tree>)>,
    cancel: Arc<AtomicBool>,
    timeout: Option<Duration>,
}

#[derive(Default)]
struct ParsedPass {
    tree: Option<Tree>,
    changed: Option<Vec<Range<usize>>>,
}

pub(crate) struct ParsedTrees {
    generation: u64,
    len_bytes: usize,
    timed_out: bool,
    passes: Vec<ParsedPass>,
    parse_cost: Duration,
    cancelled: bool,
}

#[cfg(test)]
impl ParsedTrees {
    pub(crate) fn was_cancelled(&self) -> bool {
        self.cancelled
    }
}

impl DeferredParse {
    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    #[cfg(test)]
    pub(crate) fn with_timeout_for_test(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub(crate) fn run(self) -> ParsedTrees {
        let DeferredParse {
            generation,
            rope,
            passes,
            cancel,
            timeout,
        } = self;
        let deadline = timeout.map(|timeout| Instant::now() + timeout);
        let len_bytes = rope.len_bytes();
        let mut parsed = Vec::with_capacity(passes.len());
        let mut parse_cost = Duration::ZERO;
        let mut timed_out = false;
        for (grammar, old) in passes {
            if timed_out || cancel.load(Ordering::Relaxed) {
                parsed.push(ParsedPass::default());
                continue;
            }
            let mut parser = Parser::new();
            if parser.set_language(&grammar.language).is_err() {
                parsed.push(ParsedPass::default());
                continue;
            }
            let started = Instant::now();
            let Some(tree) = parse_rope(
                &mut parser,
                &rope,
                old.as_ref(),
                deadline,
                Some(cancel.as_ref()),
            ) else {
                timed_out = deadline.is_some_and(|deadline| Instant::now() >= deadline);
                parsed.push(ParsedPass::default());
                continue;
            };
            let changed = old.as_ref().map(|old| {
                old.changed_ranges(&tree)
                    .map(|range| range.start_byte..range.end_byte)
                    .collect()
            });
            parse_cost += started.elapsed();
            parsed.push(ParsedPass {
                tree: Some(tree),
                changed,
            });
        }
        ParsedTrees {
            generation,
            len_bytes,
            timed_out,
            passes: parsed,
            parse_cost,
            cancelled: cancel.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct CodeHighlighter {
    syntax: DiffSyntax,
    passes: Vec<GrammarPass>,
    rows: Vec<IndexedLineRuns>,
    row_states: Vec<RowState>,
    enabled: bool,
    too_complex: bool,
    generation: u64,
    last_parse_cost: Duration,
    deferred_cancel: Option<Arc<AtomicBool>>,
}

impl CodeHighlighter {
    pub(crate) fn new(doc: &CodeDocument, syntax: DiffSyntax) -> Self {
        let mut passes = Vec::new();
        if doc.len_bytes() <= highlight_cap(doc.ext())
            && let Some(grammar) = grammar_for_ext(doc.ext())
        {
            passes.push(grammar);
            if is_markdown(doc.ext())
                && let Some(inline) = markdown_inline_grammar()
            {
                passes.push(inline);
            }
        }
        let passes = passes
            .into_iter()
            .filter_map(|grammar| {
                let mut parser = Parser::new();
                parser.set_language(&grammar.language).ok()?;
                Some(GrammarPass {
                    grammar,
                    parser,
                    tree: None,
                    colors: capture_colors(grammar, &syntax),
                    cursor: QueryCursor::new(),
                })
            })
            .collect::<Vec<_>>();
        let enabled = !passes.is_empty();
        let tracked_lines = if enabled { doc.line_count() } else { 0 };

        Self {
            syntax,
            passes,
            rows: vec![Vec::new(); tracked_lines],
            row_states: vec![RowState::Stale; tracked_lines],
            enabled,
            too_complex: false,
            generation: 0,
            last_parse_cost: Duration::ZERO,
            deferred_cancel: None,
        }
    }

    pub(crate) fn initial_parse(&mut self, doc: &CodeDocument) -> Option<DeferredParse> {
        if !self.enabled || self.has_tree() {
            return None;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.deferred_cancel = Some(cancel.clone());
        Some(DeferredParse {
            generation: self.generation,
            rope: doc.text().clone(),
            passes: self
                .passes
                .iter()
                .map(|pass| (pass.grammar, None))
                .collect(),
            cancel,
            timeout: Some(INITIAL_PARSE_TIMEOUT),
        })
    }

    pub(crate) fn has_tree(&self) -> bool {
        self.passes.iter().any(|pass| pass.tree.is_some())
    }

    pub(crate) fn is_too_complex(&self) -> bool {
        self.too_complex
    }

    #[cfg(test)]
    pub(crate) fn last_parse_cost(&self) -> Duration {
        self.last_parse_cost
    }

    #[cfg(test)]
    pub(crate) fn set_last_parse_cost(&mut self, cost: Duration) {
        self.last_parse_cost = cost;
    }

    #[allow(dead_code)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn runs_into(&self, row: usize, out: &mut LineRuns) {
        out.clear();
        if self.row_states.get(row) != Some(&RowState::Fresh) {
            return;
        }
        let Some(runs) = self.rows.get(row) else {
            return;
        };
        for &(start, end, indexed) in runs.iter() {
            let Some(pass) = self.passes.get(indexed.pass as usize) else {
                continue;
            };
            let Some(color) = pass.colors.get(indexed.capture as usize).copied().flatten() else {
                continue;
            };
            out.push((start as usize..end as usize, color));
        }
    }

    #[cfg(test)]
    pub(crate) fn runs(&self, row: usize) -> LineRuns {
        let mut out = LineRuns::new();
        self.runs_into(row, &mut out);
        out
    }

    pub(crate) fn set_syntax(&mut self, _doc: &CodeDocument, syntax: DiffSyntax) {
        self.syntax = syntax;
        for pass in &mut self.passes {
            pass.colors = capture_colors(pass.grammar, &self.syntax);
        }
    }

    pub(crate) fn edit(&mut self, doc: &CodeDocument, edit: &CodeEdit) -> HighlightOutcome {
        self.edit_with_budget(doc, edit, SYNC_PARSE_BUDGET)
    }

    pub(crate) fn edit_with_budget(
        &mut self,
        doc: &CodeDocument,
        edit: &CodeEdit,
        budget: Duration,
    ) -> HighlightOutcome {
        self.edit_batch(doc, std::slice::from_ref(edit), budget)
            .unwrap_or(HighlightOutcome::Synced)
    }

    pub(crate) fn edit_batch(
        &mut self,
        doc: &CodeDocument,
        edits: &[CodeEdit],
        budget: Duration,
    ) -> Result<HighlightOutcome, UnorderedBatch> {
        if !descends_without_overlap(edits) {
            return Err(UnorderedBatch);
        }
        if !self.enabled || edits.is_empty() {
            return Ok(HighlightOutcome::Synced);
        }
        self.cancel_deferred();
        self.generation = self.generation.wrapping_add(1);
        self.interpolate(doc, edits);

        for edit in edits {
            let input = InputEdit {
                start_byte: edit.start_byte,
                old_end_byte: edit.old_end_byte,
                new_end_byte: edit.new_end_byte,
                start_position: point(edit.start_point.row, edit.start_point.column),
                old_end_position: point(edit.old_end_point.row, edit.old_end_point.column),
                new_end_position: point(edit.new_end_point.row, edit.new_end_point.column),
            };
            for pass in &mut self.passes {
                if let Some(tree) = pass.tree.as_mut() {
                    tree.edit(&input);
                }
            }
        }

        let mut changed: Vec<Range<usize>> = Vec::new();
        let mut without_old_tree = false;
        let mut deferred = self.last_parse_cost >= budget;
        if !deferred {
            let deadline = Instant::now() + budget;
            let mut parse_cost = Duration::ZERO;
            for pass in &mut self.passes {
                let old = pass.tree.clone();
                let started = Instant::now();
                match parse_rope(
                    &mut pass.parser,
                    doc.text(),
                    old.as_ref(),
                    Some(deadline),
                    None,
                ) {
                    Some(new_tree) => {
                        match old.as_ref() {
                            Some(old) => changed.extend(
                                old.changed_ranges(&new_tree)
                                    .map(|range| range.start_byte..range.end_byte),
                            ),
                            None => without_old_tree = true,
                        }
                        parse_cost += started.elapsed();
                        pass.tree = Some(new_tree);
                    }
                    None => {
                        pass.parser.reset();
                        deferred = true;
                    }
                }
            }
            self.last_parse_cost = if deferred { budget } else { parse_cost };
        }

        if without_old_tree {
            self.mark_stale(0..doc.line_count());
        } else {
            for range in &changed {
                let rows = self.dirty_rows(doc, range);
                self.mark_stale(rows);
            }
        }

        if deferred {
            let cancel = Arc::new(AtomicBool::new(false));
            self.deferred_cancel = Some(cancel.clone());
            return Ok(HighlightOutcome::Deferred(DeferredParse {
                generation: self.generation,
                rope: doc.text().clone(),
                passes: self
                    .passes
                    .iter()
                    .map(|p| (p.grammar, p.tree.clone()))
                    .collect(),
                cancel,
                timeout: None,
            }));
        }

        Ok(HighlightOutcome::Synced)
    }

    pub(crate) fn apply_parsed(&mut self, doc: &CodeDocument, parsed: ParsedTrees) -> bool {
        if parsed.cancelled
            || parsed.generation != self.generation
            || parsed.len_bytes != doc.len_bytes()
        {
            return false;
        }
        if parsed.passes.len() != self.passes.len() {
            return false;
        }
        if parsed.timed_out {
            log::warn!(
                "coloring gave up on {}: its parse ran past {:?}",
                doc.path().display(),
                INITIAL_PARSE_TIMEOUT
            );
            self.deferred_cancel = None;
            self.enabled = false;
            self.too_complex = true;
            self.passes = Vec::new();
            self.rows = Vec::new();
            self.row_states = Vec::new();
            return true;
        }
        let parse_cost = parsed.parse_cost;
        let mut changed = Vec::new();
        let mut without_old_tree = false;
        for (pass, parsed) in self.passes.iter_mut().zip(parsed.passes) {
            let Some(new_tree) = parsed.tree else {
                continue;
            };
            match parsed.changed {
                Some(ranges) => changed.extend(ranges),
                None => without_old_tree = true,
            }
            pass.tree = Some(new_tree);
        }
        self.deferred_cancel = None;
        if !without_old_tree {
            self.last_parse_cost = parse_cost;
        }
        if without_old_tree {
            self.mark_stale(0..doc.line_count());
        } else {
            for range in changed {
                let rows = self.dirty_rows(doc, &range);
                self.mark_stale(rows);
            }
        }
        true
    }

    fn interpolate(&mut self, doc: &CodeDocument, edits: &[CodeEdit]) {
        match edits {
            [edit] => self.interpolate_in_place(doc, edit),
            _ => self.interpolate_batch(doc, edits),
        }
    }

    fn interpolate_in_place(&mut self, doc: &CodeDocument, edit: &CodeEdit) {
        let line_count = doc.line_count();
        let start_row = edit.start_point.row;
        let old_end_row = edit.old_end_point.row.max(start_row);
        let new_end_row = edit.new_end_point.row.max(start_row);
        let prefix = head_runs(self.rows.get(start_row), edit.start_point.column);
        let suffix = tail_runs(
            self.rows.get(old_end_row),
            edit.old_end_point.column,
            edit.new_end_point.column,
        );
        let mut replacement = Vec::with_capacity(new_end_row - start_row + 1);
        if new_end_row == start_row {
            let mut merged = prefix;
            merged.extend(suffix);
            replacement.push(merged);
        } else {
            replacement.push(prefix);
            replacement.resize(new_end_row - start_row, Vec::new());
            replacement.push(suffix);
        }
        let removed_end = (old_end_row + 1).min(self.rows.len());
        self.rows
            .splice(start_row.min(removed_end)..removed_end, replacement);
        self.rows.resize(line_count, Vec::new());

        let stale = vec![RowState::Stale; new_end_row - start_row + 1];
        let removed_end = (old_end_row + 1).min(self.row_states.len());
        self.row_states
            .splice(start_row.min(removed_end)..removed_end, stale);
        self.row_states.resize(line_count, RowState::Stale);
    }

    fn interpolate_batch(&mut self, doc: &CodeDocument, edits: &[CodeEdit]) {
        let line_count = doc.line_count();
        let mut old_rows = std::mem::take(&mut self.rows);
        let old_states = std::mem::take(&mut self.row_states);
        let mut rows = Vec::with_capacity(line_count);
        let mut states = Vec::with_capacity(line_count);
        let mut cursor = 0usize;
        for edit in edits.iter().rev() {
            let start_row = edit.start_point.row;
            let old_end_row = edit.old_end_point.row.max(start_row);
            let new_end_row = edit.new_end_point.row.max(start_row);
            while cursor < start_row {
                rows.push(taken_row(&mut old_rows, cursor));
                states.push(old_states.get(cursor).copied().unwrap_or(RowState::Stale));
                cursor += 1;
            }
            let prefix = head_runs(old_rows.get(start_row), edit.start_point.column);
            let suffix = tail_runs(
                old_rows.get(old_end_row),
                edit.old_end_point.column,
                edit.new_end_point.column,
            );
            if new_end_row == start_row {
                let mut merged = prefix;
                merged.extend(suffix);
                rows.push(merged);
                states.push(RowState::Stale);
            } else {
                rows.push(prefix);
                rows.resize(rows.len() + new_end_row - start_row - 1, Vec::new());
                rows.push(suffix);
                states.resize(states.len() + new_end_row - start_row + 1, RowState::Stale);
            }
            cursor = old_end_row + 1;
        }
        while cursor < old_rows.len() {
            rows.push(taken_row(&mut old_rows, cursor));
            states.push(old_states.get(cursor).copied().unwrap_or(RowState::Stale));
            cursor += 1;
        }
        rows.resize(line_count, Vec::new());
        states.resize(line_count, RowState::Stale);
        self.rows = rows;
        self.row_states = states;
    }

    fn dirty_rows(&self, doc: &CodeDocument, bytes: &Range<usize>) -> Range<usize> {
        let lines = doc.line_count();
        let first = doc.byte_to_line(bytes.start);
        let last = doc.byte_to_line(bytes.end.max(bytes.start));
        first..(last + 1).min(lines)
    }

    pub(crate) fn requery_rows(&mut self, doc: &CodeDocument, rows: Range<usize>) {
        if !self.enabled {
            return;
        }
        let lines = doc.line_count();
        if self.rows.len() != lines {
            self.rows.resize(lines, Vec::new());
        }
        if self.row_states.len() != lines {
            self.row_states.resize(lines, RowState::Stale);
        }
        if !self.has_tree() {
            return;
        }
        let rows = rows.start.min(lines)..rows.end.min(lines);
        if rows.is_empty() {
            return;
        }
        let start_byte = doc.line_to_byte(rows.start);
        let end_byte = if rows.end < lines {
            doc.line_to_byte(rows.end)
        } else {
            doc.len_bytes()
        };
        let line_ranges = rows
            .clone()
            .filter_map(|row| doc.line_byte_range(row))
            .collect::<Vec<_>>();
        let mut capture_counts = vec![0usize; line_ranges.len()];
        for row in rows.clone() {
            self.rows[row].clear();
        }

        for (pass_index, pass) in self.passes.iter_mut().enumerate() {
            let GrammarPass {
                grammar,
                tree,
                colors,
                cursor,
                ..
            } = pass;
            let Some(tree) = tree.as_ref() else {
                continue;
            };
            let Ok(pass_index) = u16::try_from(pass_index) else {
                continue;
            };
            let grammar: &'static Grammar = grammar;
            cursor.set_byte_range(start_byte..end_byte);
            let mut caps = cursor.captures(&grammar.query, tree.root_node(), RopeText(doc.text()));
            while let Some((mat, idx)) = caps.next() {
                let cap = mat.captures[*idx];
                let Ok(capture) = u16::try_from(cap.index) else {
                    continue;
                };
                if colors.get(capture as usize).copied().flatten().is_none() {
                    continue;
                }
                bucket_capture(
                    cap.node.start_byte(),
                    cap.node.end_byte(),
                    IndexedCapture {
                        pass: pass_index,
                        capture,
                    },
                    &rows,
                    &line_ranges,
                    &mut capture_counts,
                    &mut self.rows,
                );
            }
        }

        for row in rows {
            resolve_indexed_runs(&mut self.rows[row]);
            self.row_states[row] = RowState::Fresh;
        }
    }

    pub(crate) fn fill_stale_rows(
        &mut self,
        doc: &CodeDocument,
        rows: Range<usize>,
        budget: Duration,
    ) -> StaleFill {
        if !self.enabled || !self.has_tree() {
            return StaleFill::default();
        }
        let lines = doc.line_count();
        let rows = rows.start.min(lines)..rows.end.min(lines);
        let deadline = Instant::now() + budget;
        let mut queried = false;
        let mut row = rows.start;
        'fill: while row < rows.end {
            if self.row_states.get(row) != Some(&RowState::Stale) {
                row += 1;
                continue;
            }
            let span_start = row;
            while row < rows.end && self.row_states.get(row) == Some(&RowState::Stale) {
                row += 1;
            }
            let mut slice_start = span_start;
            while slice_start < row {
                if queried && Instant::now() >= deadline {
                    break 'fill;
                }
                let slice_end = (slice_start + MAX_QUERY_ROWS).min(row);
                self.requery_rows(doc, slice_start..slice_end);
                queried = true;
                slice_start = slice_end;
            }
        }
        let stale_rows = rows
            .filter(|row| self.row_states.get(*row) == Some(&RowState::Stale))
            .count();
        StaleFill { stale_rows }
    }

    fn mark_stale(&mut self, rows: Range<usize>) {
        let end = rows.end.min(self.row_states.len());
        for state in &mut self.row_states[rows.start.min(end)..end] {
            *state = RowState::Stale;
        }
    }

    fn cancel_deferred(&mut self) {
        if let Some(cancel) = self.deferred_cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for CodeHighlighter {
    fn drop(&mut self) {
        self.cancel_deferred();
    }
}

#[cfg(test)]
impl CodeHighlighter {
    pub(crate) fn parse_initial_blocking(&mut self, doc: &CodeDocument) -> bool {
        let Some(parse) = self.initial_parse(doc) else {
            return false;
        };
        let parsed = parse.run();
        self.apply_parsed(doc, parsed)
    }

    fn root_child_ids(&self) -> Vec<usize> {
        self.passes
            .first()
            .and_then(|p| p.tree.as_ref())
            .map(|tree| {
                let root = tree.root_node();
                let mut cursor = root.walk();
                root.children(&mut cursor).map(|n| n.id()).collect()
            })
            .unwrap_or_default()
    }

    fn all_rows_stale(&self) -> bool {
        self.row_states
            .iter()
            .all(|state| *state == RowState::Stale)
    }

    fn has_stale_rows(&self) -> bool {
        self.row_states.contains(&RowState::Stale)
    }

    pub(crate) fn is_row_stale(&self, row: usize) -> bool {
        self.row_states.get(row) == Some(&RowState::Stale)
    }

    pub(crate) fn stale_rows_in(&self, rows: Range<usize>) -> usize {
        rows.filter(|row| self.is_row_stale(*row)).count()
    }

    fn per_row_capacity(&self) -> (usize, usize) {
        (self.rows.capacity(), self.row_states.capacity())
    }
}

fn capture_colors(grammar: &Grammar, syntax: &DiffSyntax) -> Vec<Option<Hsla>> {
    grammar
        .query
        .capture_names()
        .iter()
        .map(|name| syntax.color_for_capture(name))
        .collect()
}

fn descends_without_overlap(edits: &[CodeEdit]) -> bool {
    edits.windows(2).all(|pair| {
        pair[1].old_end_point.row < pair[0].start_point.row
            && pair[1].old_end_byte <= pair[0].start_byte
    })
}

fn taken_row(rows: &mut [IndexedLineRuns], row: usize) -> IndexedLineRuns {
    rows.get_mut(row).map(std::mem::take).unwrap_or_default()
}

fn head_runs(runs: Option<&IndexedLineRuns>, start_col: usize) -> IndexedLineRuns {
    let Some(runs) = runs else {
        return IndexedLineRuns::new();
    };
    runs.iter()
        .filter_map(|&(start, end, capture)| {
            let start = start as usize;
            let end = (end as usize).min(start_col);
            (start < start_col && start < end).then_some((start as u32, end as u32, capture))
        })
        .collect()
}

fn tail_runs(
    runs: Option<&IndexedLineRuns>,
    old_end_col: usize,
    new_end_col: usize,
) -> IndexedLineRuns {
    let Some(runs) = runs else {
        return IndexedLineRuns::new();
    };
    runs.iter()
        .filter_map(|&(start, end, capture)| {
            let start = start as usize;
            let end = end as usize;
            if end <= old_end_col {
                return None;
            }
            let shifted_start = start.max(old_end_col) - old_end_col + new_end_col;
            let shifted_end = end - old_end_col + new_end_col;
            Some((
                u32::try_from(shifted_start).ok()?,
                u32::try_from(shifted_end).ok()?,
                capture,
            ))
        })
        .collect()
}

fn bucket_capture(
    cstart: usize,
    cend: usize,
    capture: IndexedCapture,
    rows: &Range<usize>,
    line_ranges: &[Range<usize>],
    capture_counts: &mut [usize],
    out: &mut [IndexedLineRuns],
) {
    if cend <= cstart {
        return;
    }
    let mut local_row = line_ranges.partition_point(|range| range.end <= cstart);
    while let Some(lr) = line_ranges.get(local_row) {
        if lr.start >= cend {
            break;
        }
        let s = cstart.max(lr.start).saturating_sub(lr.start);
        let e = cend.min(lr.end).saturating_sub(lr.start);
        if e > s
            && capture_counts[local_row] < MAX_CAPTURES_PER_ROW
            && let (Ok(s), Ok(e)) = (u32::try_from(s), u32::try_from(e))
        {
            out[rows.start + local_row].push((s, e, capture));
            capture_counts[local_row] += 1;
        }
        local_row += 1;
    }
}

fn resolve_indexed_runs(runs: &mut IndexedLineRuns) {
    let mut expanded = runs
        .drain(..)
        .map(|(start, end, capture)| (start as usize..end as usize, capture))
        .collect::<Vec<_>>();
    resolve_runs(&mut expanded);
    runs.extend(expanded.into_iter().filter_map(|(range, capture)| {
        Some((
            u32::try_from(range.start).ok()?,
            u32::try_from(range.end).ok()?,
            capture,
        ))
    }));
}

struct RopeText<'a>(&'a Rope);

type ChunkBytes<'a> = std::iter::Map<ropey::iter::Chunks<'a>, fn(&'a str) -> &'a [u8]>;

impl<'a> TextProvider<&'a [u8]> for RopeText<'a> {
    type I = ChunkBytes<'a>;

    fn text(&mut self, node: Node) -> Self::I {
        let len = self.0.len_bytes();
        let range = node.byte_range();
        let end = range.end.min(len);
        let start = range.start.min(end);
        self.0
            .byte_slice(start..end)
            .chunks()
            .map(str::as_bytes as fn(&str) -> &[u8])
    }
}

fn parse_rope(
    parser: &mut Parser,
    rope: &Rope,
    old: Option<&Tree>,
    deadline: Option<Instant>,
    cancel: Option<&AtomicBool>,
) -> Option<Tree> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline)
        || cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
    {
        return None;
    }
    let len = rope.len_bytes();
    let mut read = |byte: usize, _pos: TsPoint| -> &[u8] {
        if byte >= len {
            return &[];
        }
        let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
        &chunk.as_bytes()[byte - chunk_start..]
    };
    let mut progress = |_state: &ParseState| -> ControlFlow<()> {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline)
            || cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    };
    let options = ParseOptions::new().progress_callback(&mut progress);
    parser.parse_with_options(&mut read, old, Some(options))
}

const fn point(row: usize, column: usize) -> TsPoint {
    TsPoint { row, column }
}

pub(crate) fn spawn_deferred_parse<V, F>(deferred: DeferredParse, cx: &mut Context<V>, apply: F)
where
    V: 'static,
    F: FnOnce(&mut V, ParsedTrees, &mut Context<V>) + 'static,
{
    cx.spawn(async move |this: WeakEntity<V>, cx: &mut AsyncApp| {
        #[cfg(not(test))]
        let parsed = smol::unblock(move || deferred.run()).await;
        #[cfg(test)]
        let parsed = cx.background_spawn(async move { deferred.run() }).await;
        cx.update(|cx| {
            let _ = this.update(cx, |view: &mut V, cx: &mut Context<V>| {
                apply(view, parsed, cx);
            });
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::diff::{MAX_HIGHLIGHT_BYTES, MAX_MARKDOWN_HIGHLIGHT_BYTES, highlight_lines};
    use crate::theme::paneflow_dark;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&paneflow_dark())
    }

    fn doc(name: &str, text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from(format!("/tmp/{name}")), text)
    }

    fn parsed(doc: &CodeDocument) -> CodeHighlighter {
        let mut highlighter = CodeHighlighter::new(doc, syntax());
        highlighter.parse_initial_blocking(doc);
        highlighter
    }

    fn rows_of_code(rows: usize) -> String {
        (0..rows).map(|row| format!("fn f{row}() {{}}\n")).collect()
    }

    fn corpus() -> Vec<(&'static str, &'static str)> {
        crate::diff::parity_tests::CORPUS.to_vec()
    }

    fn expected_rows(text: &str, ext: &str, lines: usize) -> Vec<LineRuns> {
        let mut rows = highlight_lines(text, ext, &syntax());
        assert!(
            rows.len() <= lines,
            "diff produced more rows ({}) than the document has lines ({lines})",
            rows.len()
        );
        rows.resize(lines, Vec::new());
        rows
    }

    fn fill_all(highlighter: &mut CodeHighlighter, document: &CodeDocument) {
        highlighter.requery_rows(document, 0..document.line_count());
    }

    fn assert_parity(name: &str, text: &str) {
        let d = doc(name, text);
        let mut h = parsed(&d);
        assert!(h.is_enabled(), "{name} resolved no grammar");
        assert!(h.all_rows_stale(), "{name} queried during construction");
        fill_all(&mut h, &d);
        let expected = expected_rows(text, d.ext(), d.line_count());
        for (row, want) in expected.iter().enumerate() {
            assert_eq!(
                h.runs(row),
                want.as_slice(),
                "{name} row {row} diverges from the diff: {:?}",
                d.line_string(row)
            );
        }
    }

    #[test]
    fn parity_matches_the_diff_highlighter_on_every_grammar() {
        for (name, text) in corpus() {
            assert_parity(name, text);
        }
    }

    #[test]
    fn parity_holds_after_an_incremental_edit_on_every_grammar() {
        for (name, text) in corpus() {
            let mut d = doc(name, text);
            let h = &mut parsed(&d);
            fill_all(h, &d);
            let at = d.line_to_byte(1);
            let edit = d.insert(at, "\n ").expect("insert");
            let outcome = h.edit_with_budget(&d, &edit, Duration::from_secs(5));
            assert!(
                matches!(outcome, HighlightOutcome::Synced),
                "{name} deferred"
            );

            let after = d.to_disk_string();
            fill_all(h, &d);
            let expected = expected_rows(&after, d.ext(), d.line_count());
            for (row, want) in expected.iter().enumerate() {
                assert_eq!(
                    h.runs(row),
                    want.as_slice(),
                    "{name} row {row} diverges after an edit: {:?}",
                    d.line_string(row)
                );
            }
        }
    }

    #[test]
    fn a_keystroke_reuses_the_existing_tree_instead_of_reparsing_the_file() {
        let mut text = String::new();
        for i in 0..400 {
            text.push_str(&format!(
                "pub fn f{i}(a: i32) -> i32 {{\n    a + {i}\n}}\n\n"
            ));
        }
        let mut d = doc("big.rs", &text);
        let mut h = parsed(&d);
        let before = h.root_child_ids();
        assert!(before.len() >= 400);

        let at = d.line_to_byte(1) + 4;
        let edit = d.insert(at, "1").expect("insert");
        assert!(matches!(
            h.edit_with_budget(&d, &edit, Duration::from_secs(5)),
            HighlightOutcome::Synced
        ));

        let after = h.root_child_ids();
        assert_eq!(after.len(), before.len());
        let reused = before.iter().zip(&after).filter(|(a, b)| a == b).count();
        let fresh = parsed(&d).root_child_ids();
        let coincidental = after.iter().zip(&fresh).filter(|(a, b)| a == b).count();
        assert!(
            reused * 10 >= before.len() * 9,
            "only {reused}/{} subtrees were reused - the parse was not incremental",
            before.len()
        );
        assert!(
            coincidental * 10 < before.len(),
            "the from-scratch control shared {coincidental}/{} subtrees, so id identity \
             proves nothing here",
            before.len()
        );
    }

    #[test]
    fn a_blown_budget_defers_the_parse_until_visible_rows_are_refilled() {
        let text = "fn main() {\n    let s = \"hello\";\n    println!(\"{s}\");\n}\n";
        let mut d = doc("deferred.rs", text);
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let colored_before = h.runs(1).to_vec();
        assert!(!colored_before.is_empty());

        let at = d.line_to_byte(1);
        let edit = d.insert(at, "    // note\n").expect("insert");
        let HighlightOutcome::Deferred(deferred) = h.edit_with_budget(&d, &edit, Duration::ZERO)
        else {
            panic!("a zero budget must defer");
        };
        assert_eq!(deferred.generation(), h.generation());
        assert!(h.runs(1).is_empty());
        assert!(h.runs(2).is_empty());

        assert!(h.apply_parsed(&d, deferred.run()));
        assert!(
            h.has_stale_rows(),
            "the applied tree must leave the edited rows to requery"
        );
        fill_all(&mut h, &d);
        let after = d.to_disk_string();
        let expected = expected_rows(&after, d.ext(), d.line_count());
        for (row, want) in expected.iter().enumerate() {
            assert_eq!(h.runs(row), want.as_slice(), "row {row}");
        }
    }

    #[test]
    fn a_deferred_parse_from_a_superseded_generation_is_dropped() {
        let text = "fn main() {\n    let s = \"hello\";\n}\n";
        let mut d = doc("stale.rs", text);
        let mut h = parsed(&d);
        fill_all(&mut h, &d);

        let first = d.insert(0, "//x\n").expect("insert");
        let HighlightOutcome::Deferred(stale) = h.edit_with_budget(&d, &first, Duration::ZERO)
        else {
            panic!("a zero budget must defer");
        };

        let second = d.insert(0, "//y\n").expect("insert");
        let _ = h.edit(&d, &second);
        let snapshot: Vec<_> = (0..d.line_count()).map(|r| h.runs(r)).collect();

        let parsed = stale.run();
        assert!(parsed.cancelled);
        assert!(!h.apply_parsed(&d, parsed));
        let after: Vec<_> = (0..d.line_count()).map(|r| h.runs(r)).collect();
        assert_eq!(snapshot, after);
    }

    #[test]
    fn only_the_latest_deferred_parse_survives_an_edit_burst() {
        let mut d = doc("burst.rs", "fn main() { let value = 1; }\n");
        let mut h = parsed(&d);
        let first_edit = d.insert(0, "x").expect("first insert");
        let HighlightOutcome::Deferred(first) = h.edit_with_budget(&d, &first_edit, Duration::ZERO)
        else {
            panic!("first parse must defer");
        };
        let second_edit = d.insert(0, "y").expect("second insert");
        let HighlightOutcome::Deferred(second) =
            h.edit_with_budget(&d, &second_edit, Duration::ZERO)
        else {
            panic!("second parse must defer");
        };

        assert!(first.run().was_cancelled());
        let latest = second.run();
        assert!(!latest.was_cancelled());
        assert!(h.apply_parsed(&d, latest));
    }

    #[test]
    fn dropping_the_highlighter_cancels_its_deferred_parse() {
        let mut d = doc("drop.rs", "fn main() {}\n");
        let mut h = parsed(&d);
        let edit = d.insert(0, "x").expect("insert");
        let HighlightOutcome::Deferred(parse) = h.edit_with_budget(&d, &edit, Duration::ZERO)
        else {
            panic!("parse must defer");
        };
        drop(h);
        assert!(parse.run().was_cancelled());
    }

    #[test]
    fn dropping_the_highlighter_cancels_its_initial_parse() {
        let d = doc("closed.rs", "fn main() {}\n");
        let mut h = CodeHighlighter::new(&d, syntax());
        let parse = h.initial_parse(&d).expect("the initial parse is deferred");
        drop(h);
        let parsed = parse.run();
        assert!(parsed.was_cancelled());
    }

    #[test]
    fn a_keystroke_before_the_first_tree_cancels_it_and_reparses_from_nothing() {
        let mut d = doc("typed.rs", &rows_of_code(400));
        let mut h = CodeHighlighter::new(&d, syntax());
        let initial = h.initial_parse(&d).expect("the initial parse is deferred");

        let edit = d.insert(0, "//\n").expect("insert");
        let HighlightOutcome::Deferred(after_edit) = h.edit_with_budget(&d, &edit, Duration::ZERO)
        else {
            panic!("a treeless reparse must defer");
        };
        assert_eq!(h.generation(), 1, "the keystroke advanced the generation");
        assert!(
            initial.run().was_cancelled(),
            "the initial parse was cancelled by the keystroke"
        );
        assert_eq!(
            after_edit.generation(),
            1,
            "the replacement parse carries the new generation"
        );

        assert!(h.apply_parsed(&d, after_edit.run()));
        assert!(h.has_tree(), "the replacement parse installed the tree");
        assert!(h.all_rows_stale(), "a first tree leaves every row stale");
        fill_all(&mut h, &d);
        let expected = expected_rows(&d.to_disk_string(), d.ext(), d.line_count());
        for (row, want) in expected.iter().enumerate() {
            assert_eq!(h.runs(row), want.as_slice(), "row {row} diverges");
        }
    }

    #[test]
    fn an_initial_parse_past_its_timeout_gives_up_and_greys_the_file() {
        let d = doc("slow.rs", &rows_of_code(4_000));
        let mut h = CodeHighlighter::new(&d, syntax());
        let parse = h
            .initial_parse(&d)
            .expect("the initial parse is deferred")
            .with_timeout_for_test(Duration::ZERO);

        let parsed = parse.run();
        assert!(!parsed.was_cancelled(), "a timeout is not a cancellation");
        assert!(h.apply_parsed(&d, parsed), "the timeout is applied");

        assert!(h.is_too_complex(), "the tab reports the give-up");
        assert!(!h.is_enabled(), "the file stays grey");
        assert!(!h.has_tree());
        assert_eq!(h.per_row_capacity(), (0, 0), "no per-row storage is kept");
        assert!(h.runs(0).is_empty());
    }

    #[test]
    fn a_contiguous_stale_span_within_the_cap_is_filled_by_a_single_query() {
        let text = rows_of_code(60);
        let d = doc("span.rs", &text);
        let mut h = parsed(&d);

        assert!(!h.fill_stale_rows(&d, 0..60, Duration::ZERO).any_stale());
        for row in 0..60 {
            assert!(!h.is_row_stale(row), "row {row} stayed stale");
            assert!(!h.runs(row).is_empty(), "row {row} kept no runs");
        }
    }

    #[test]
    fn a_starved_fill_stops_between_slices_and_leaves_whole_rows_stale() {
        let text = rows_of_code(300);
        let d = doc("sliced.rs", &text);
        let mut h = parsed(&d);

        let first = h.fill_stale_rows(&d, 0..300, Duration::ZERO);
        assert_eq!(first.stale_rows, 300 - MAX_QUERY_ROWS);
        assert!(!h.runs(MAX_QUERY_ROWS - 1).is_empty());
        assert!(h.is_row_stale(MAX_QUERY_ROWS));
        assert!(h.runs(MAX_QUERY_ROWS).is_empty());

        let second = h.fill_stale_rows(&d, 0..300, Duration::ZERO);
        assert_eq!(second.stale_rows, 300 - 2 * MAX_QUERY_ROWS);

        let third = h.fill_stale_rows(&d, 0..300, Duration::ZERO);
        assert_eq!(third.stale_rows, 0);
    }

    #[test]
    fn a_starved_fill_leaves_a_later_span_entirely_stale() {
        let text = rows_of_code(100);
        let d = doc("spans.rs", &text);
        let mut h = parsed(&d);
        h.requery_rows(&d, 0..d.line_count());
        h.mark_stale(0..5);
        h.mark_stale(50..60);

        let fill = h.fill_stale_rows(&d, 0..100, Duration::ZERO);
        assert_eq!(fill.stale_rows, 10);
        assert_eq!(h.stale_rows_in(0..5), 0);
        assert_eq!(h.stale_rows_in(50..60), 10);
    }

    #[test]
    fn a_reused_cursor_matches_a_first_query_on_the_same_range() {
        let text = rows_of_code(200);
        let d = doc("cursor.rs", &text);
        let mut reused = parsed(&d);
        reused.requery_rows(&d, 0..40);
        reused.requery_rows(&d, 120..160);

        let mut fresh = parsed(&d);
        fresh.requery_rows(&d, 120..160);

        for row in 120..160 {
            assert_eq!(reused.runs(row), fresh.runs(row), "row {row}");
        }
    }

    #[test]
    fn a_row_over_the_capture_cap_does_not_break_its_span() {
        let pairs = (0..MAX_CAPTURES_PER_ROW)
            .map(|index| format!("\"k{index}\": {index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!("{{\n  \"row\": {{{pairs}}},\n  \"tail\": 1\n}}\n");
        let d = doc("capped.json", &text);
        let mut h = parsed(&d);
        assert!(h.is_enabled());

        h.requery_rows(&d, 0..d.line_count());
        assert_eq!(h.stale_rows_in(0..d.line_count()), 0);
        assert!(
            !h.runs(2).is_empty(),
            "the row after the capped one lost its runs"
        );
    }

    #[test]
    fn a_viewport_past_the_end_of_the_document_is_clamped_and_counts_only_real_rows() {
        let text = rows_of_code(300);
        let d = doc("clamped.rs", &text);
        let mut h = parsed(&d);
        assert_eq!(d.line_count(), 301);

        let starved = h.fill_stale_rows(&d, 0..10_000, Duration::ZERO);
        assert!(starved.any_stale());
        assert_eq!(starved.stale_rows, d.line_count() - MAX_QUERY_ROWS);

        let filled = h.fill_stale_rows(&d, 0..10_000, Duration::from_secs(1));
        assert!(!filled.any_stale());
        assert_eq!(filled.stale_rows, 0);
    }

    #[test]
    fn a_deferred_parse_only_marks_its_changed_ranges_stale() {
        let text = rows_of_code(10_000);
        let mut d = doc("deferred.rs", &text);
        assert!(d.len_bytes() <= MAX_HIGHLIGHT_BYTES);
        let mut h = parsed(&d);
        h.requery_rows(&d, 0..200);
        let before = h.runs(100);
        assert!(!before.is_empty());

        let offset = d.line_to_byte(5_000);
        let edit = d.insert(offset, "x").expect("insert");
        let HighlightOutcome::Deferred(parse) = h.edit_with_budget(&d, &edit, Duration::ZERO)
        else {
            panic!("a zero budget must defer the parse");
        };
        let parsed = parse.run();
        assert!(h.apply_parsed(&d, parsed));

        assert!(!h.is_row_stale(100), "an untouched row must stay fresh");
        assert_eq!(h.runs(100), before);
        assert!(h.is_row_stale(5_000), "the edited row must be stale");
        assert_eq!(
            h.stale_rows_in(0..200),
            0,
            "a distant edit must not invalidate the top of the file"
        );
    }

    #[test]
    fn a_parse_known_to_exceed_the_budget_is_deferred_without_a_new_attempt() {
        let mut d = doc("known.rs", "fn main() { let value = 1; }\n");
        let mut h = parsed(&d);
        assert_eq!(h.last_parse_cost(), Duration::ZERO);
        h.set_last_parse_cost(Duration::from_secs(1));

        let edit = d.insert(0, "x").expect("insert");
        let HighlightOutcome::Deferred(parse) =
            h.edit_with_budget(&d, &edit, HIGHLIGHT_FRAME_BUDGET)
        else {
            panic!("a parse measured past the budget must not be retried on the render thread");
        };
        assert!(h.apply_parsed(&d, parse.run()));
        assert!(
            h.last_parse_cost() < HIGHLIGHT_FRAME_BUDGET,
            "a completed parse must replace the estimate that caused the skip"
        );

        let edit = d.insert(0, "y").expect("insert");
        assert!(
            matches!(
                h.edit_with_budget(&d, &edit, HIGHLIGHT_FRAME_BUDGET),
                HighlightOutcome::Synced
            ),
            "a parse that fits the budget must bring the synchronous path back"
        );
    }

    #[test]
    fn a_blown_sync_budget_is_remembered_as_the_budget_it_blew() {
        let text = rows_of_code(4_000);
        let mut d = doc("blown.rs", &text);
        let mut h = parsed(&d);
        let budget = Duration::from_micros(50);

        let edit = d.insert(d.line_to_byte(2_000), "x").expect("insert");
        let HighlightOutcome::Deferred(parse) = h.edit_with_budget(&d, &edit, budget) else {
            panic!("a 50 us budget must not cover a 4 000 row reparse");
        };
        assert_eq!(h.last_parse_cost(), budget);

        assert!(h.apply_parsed(&d, parse.run()));
        assert!(
            h.last_parse_cost() > Duration::ZERO,
            "the background parse must publish what the reparse really costs"
        );
    }

    #[test]
    fn a_deferred_parse_holds_parity_on_every_grammar() {
        for (name, text) in corpus() {
            let mut d = doc(name, text);
            let h = &mut parsed(&d);
            fill_all(h, &d);
            let at = d.line_to_byte(1);
            let edit = d.insert(at, "\n ").expect("insert");
            let HighlightOutcome::Deferred(deferred) =
                h.edit_with_budget(&d, &edit, Duration::ZERO)
            else {
                panic!("{name} did not defer on a zero budget");
            };
            assert!(
                h.apply_parsed(&d, deferred.run()),
                "{name} rejected its own deferred parse"
            );

            let after = d.to_disk_string();
            fill_all(h, &d);
            let expected = expected_rows(&after, d.ext(), d.line_count());
            for (row, want) in expected.iter().enumerate() {
                assert_eq!(
                    h.runs(row),
                    want.as_slice(),
                    "{name} row {row} diverges after a deferred parse: {:?}",
                    d.line_string(row)
                );
            }
        }
    }

    #[test]
    fn a_markdown_file_past_its_own_cap_stays_editable_and_plain() {
        let mut text = String::with_capacity(MAX_MARKDOWN_HIGHLIGHT_BYTES + 64);
        while text.len() <= MAX_MARKDOWN_HIGHLIGHT_BYTES {
            text.push_str("# Heading with `code`, *emphasis* and [a link](https://paneflow.dev)\n");
        }
        assert!(
            text.len() < MAX_HIGHLIGHT_BYTES,
            "the markdown cap must be the lower of the two, or this test proves nothing"
        );

        let d = doc("huge.md", &text);
        let mut h = CodeHighlighter::new(&d, syntax());
        assert!(
            !h.is_enabled(),
            "markdown past its two-pass cap is left plain"
        );
        assert!(h.initial_parse(&d).is_none(), "and asks for no parse");
        assert_eq!(h.per_row_capacity(), (0, 0));

        let small = doc("small.md", "# Title\n\nSome `code` here.\n");
        let mut colored = parsed(&small);
        assert!(colored.is_enabled(), "markdown under the cap still colors");
        fill_all(&mut colored, &small);
        assert!(!colored.runs(0).is_empty());
    }

    #[test]
    fn a_file_past_the_highlight_cap_stays_editable_and_plain() {
        let mut text = String::with_capacity(MAX_HIGHLIGHT_BYTES + 64);
        while text.len() <= MAX_HIGHLIGHT_BYTES {
            text.push_str("pub fn f() -> i32 { 1 }\n");
        }
        let mut d = doc("huge.rs", &text);
        let mut h = parsed(&d);
        assert!(!h.is_enabled());
        assert!(h.runs(0).is_empty());
        assert_eq!(h.per_row_capacity(), (0, 0));

        let edit = d.insert(0, "// still editable\n").expect("insert");
        assert!(matches!(h.edit(&d, &edit), HighlightOutcome::Synced));
        assert!(!h.has_stale_rows());
        assert_eq!(h.per_row_capacity(), (0, 0));
        h.requery_rows(&d, 0..d.line_count());
        assert_eq!(h.per_row_capacity(), (0, 0));
        assert!(
            !h.fill_stale_rows(&d, 0..d.line_count(), Duration::ZERO)
                .any_stale()
        );
        assert_eq!(h.per_row_capacity(), (0, 0));
        assert!(h.runs(0).is_empty());
        assert!(h.runs(d.line_count() - 1).is_empty());
        assert_eq!(d.line_string(0).as_deref(), Some("// still editable"));
    }

    #[test]
    fn an_unknown_extension_stays_editable_and_plain() {
        let mut d = doc("notes.unknownext", "anything at all\nsecond line\n");
        let mut h = parsed(&d);
        assert!(!h.is_enabled());
        assert_eq!(h.per_row_capacity(), (0, 0));
        let edit = d.insert(0, "x").expect("insert");
        assert!(matches!(h.edit(&d, &edit), HighlightOutcome::Synced));
        assert_eq!(h.per_row_capacity(), (0, 0));
        assert!(h.runs(0).is_empty());
        assert!(highlight_lines("anything at all\n", "unknownext", &syntax())[0].is_empty());
    }

    #[test]
    fn deleting_a_row_keeps_the_row_map_aligned_with_the_document() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut d = doc("del.rs", text);
        let mut h = parsed(&d);
        fill_all(&mut h, &d);

        let start = d.line_to_byte(1);
        let end = d.line_to_byte(2);
        let edit = d.remove(start..end).expect("remove");
        assert!(matches!(
            h.edit_with_budget(&d, &edit, Duration::from_secs(5)),
            HighlightOutcome::Synced
        ));

        let after = d.to_disk_string();
        fill_all(&mut h, &d);
        assert_eq!(after, "fn a() {}\nfn c() {}\n");
        let expected = expected_rows(&after, "rs", d.line_count());
        for (row, want) in expected.iter().enumerate() {
            assert_eq!(h.runs(row), want.as_slice(), "row {row}");
        }
    }

    #[test]
    fn a_theme_change_recolors_without_touching_the_trees() {
        let text = "fn main() { let s = \"x\"; }\n";
        let d = doc("theme.rs", text);
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let before = h.root_child_ids();
        let colors_before: Vec<_> = h.runs(0).iter().map(|(_, c)| *c).collect();

        let other = crate::theme::THEMES
            .iter()
            .find(|(name, _)| *name != crate::theme::DEFAULT_THEME)
            .map(|(_, build)| build())
            .expect("a second bundled theme");
        h.set_syntax(&d, DiffSyntax::from_theme(&other));

        assert_eq!(h.root_child_ids(), before, "the trees were rebuilt");
        let colors_after: Vec<_> = h.runs(0).iter().map(|(_, c)| *c).collect();
        assert_eq!(colors_after.len(), colors_before.len());
        assert_ne!(colors_after, colors_before);
    }

    #[test]
    fn indexed_runs_keep_the_twelve_byte_storage_contract() {
        assert_eq!(std::mem::size_of::<IndexedRun>(), 12);
    }

    #[test]
    fn an_edit_marks_changed_rows_stale_without_querying_them() {
        let mut d = doc("stale.rs", "fn one() {}\nfn two() {}\n");
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let edit = d.insert(3, "x").expect("insert");
        assert!(matches!(
            h.edit_with_budget(&d, &edit, Duration::from_secs(5)),
            HighlightOutcome::Synced
        ));
        assert!(h.runs(0).is_empty());
        assert!(!h.runs(1).is_empty());
        assert!(
            !h.fill_stale_rows(&d, 0..1, Duration::from_secs(1))
                .any_stale()
        );
        assert!(!h.runs(0).is_empty());
    }

    #[test]
    fn opening_defers_its_first_parse_and_leaves_every_row_stale() {
        let d = doc("lazy.rs", "fn main() {}\n");
        let mut h = CodeHighlighter::new(&d, syntax());
        assert!(h.is_enabled(), "the grammar resolved");
        assert!(!h.has_tree(), "the initial parse did not run inside new");
        assert!(h.all_rows_stale());
        assert!(h.runs(0).is_empty());
        assert_eq!(
            h.fill_stale_rows(&d, 0..d.line_count(), Duration::from_secs(1)),
            StaleFill::default(),
            "a treeless fill asks for no follow-up frame"
        );

        let parse = h.initial_parse(&d).expect("the initial parse is deferred");
        assert_eq!(parse.generation(), 0, "the initial parse is generation 0");
        assert!(h.apply_parsed(&d, parse.run()));

        assert!(h.has_tree(), "the deferred parse installed the tree");
        assert!(h.all_rows_stale());
        fill_all(&mut h, &d);
        assert!(!h.runs(0).is_empty(), "and the rows color from it");
        assert!(
            h.initial_parse(&d).is_none(),
            "a parsed highlighter asks for no second initial parse"
        );
    }

    #[test]
    fn a_batch_of_hunks_advances_one_generation_and_defers_one_parse() {
        let mut d = doc("batch.rs", &rows_of_code(400));
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let before = h.generation();

        let mut edits = Vec::with_capacity(10);
        for hunk in (0..10).rev() {
            let at = d.line_to_byte(hunk * 30 + 5);
            edits.push(d.insert(at, "// agent\n").expect("insert"));
        }

        let HighlightOutcome::Deferred(deferred) = h
            .edit_batch(&d, &edits, Duration::ZERO)
            .expect("descending hunks are a valid batch")
        else {
            panic!("a zero budget must defer");
        };
        assert_eq!(
            h.generation(),
            before + 1,
            "ten hunks are one batch, so one generation"
        );
        assert_eq!(deferred.generation(), h.generation());
        assert!(h.apply_parsed(&d, deferred.run()));
        fill_all(&mut h, &d);
        let expected = expected_rows(&d.to_disk_string(), d.ext(), d.line_count());
        for (row, want) in expected.iter().enumerate() {
            assert_eq!(h.runs(row), want.as_slice(), "row {row} after a batch");
        }
    }

    #[test]
    fn edit_batch_refuses_hunks_that_do_not_descend() {
        let mut d = doc("batch.rs", &rows_of_code(40));
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let generation = h.generation();
        let low = d.insert(d.line_to_byte(5), "// a\n").expect("insert");
        let high = d.insert(d.line_to_byte(20), "// b\n").expect("insert");
        let snapshot: Vec<_> = (0..d.line_count()).map(|r| h.runs(r)).collect();

        assert!(
            h.edit_batch(&d, &[low, high], Duration::from_secs(1))
                .is_err(),
            "an ascending batch is refused"
        );
        assert_eq!(
            h.generation(),
            generation,
            "a refused batch changes nothing"
        );
        let after: Vec<_> = (0..d.line_count()).map(|r| h.runs(r)).collect();
        assert_eq!(snapshot, after);
    }

    #[test]
    fn edit_batch_refuses_hunks_that_share_a_row() {
        let mut d = doc("batch.rs", &rows_of_code(40));
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let generation = h.generation();
        let second = d.insert(d.line_to_byte(10), "// b\n").expect("insert");
        let first = d.insert(d.line_to_byte(10), "// a\n").expect("insert");

        assert!(
            h.edit_batch(&d, &[second, first], Duration::from_secs(1))
                .is_err(),
            "two hunks on one row overlap and are refused"
        );
        assert_eq!(
            h.generation(),
            generation,
            "a refused batch changes nothing"
        );
    }

    #[test]
    fn a_batch_leaves_the_rows_between_its_hunks_alone() {
        let mut d = doc("batch.rs", &rows_of_code(200));
        let mut h = parsed(&d);
        fill_all(&mut h, &d);
        let quiet = h.runs(100).to_vec();
        assert!(!quiet.is_empty(), "the untouched row starts colored");

        let high = d.insert(d.line_to_byte(150), "// b\n").expect("insert");
        let low = d.insert(d.line_to_byte(50), "// a\n").expect("insert");
        assert!(h.edit_batch(&d, &[high, low], Duration::ZERO).is_ok());

        assert!(h.is_row_stale(50), "the lower hunk is stale");
        assert!(
            h.is_row_stale(151),
            "the upper hunk is stale after the shift"
        );
        assert!(
            !h.is_row_stale(101),
            "a row between two hunks keeps its color through one interpolation"
        );
        assert_eq!(
            h.runs(101),
            quiet.as_slice(),
            "and it kept the runs it already had"
        );
    }

    #[test]
    fn a_batch_cancels_the_deferred_parse_it_finds_in_flight() {
        let mut d = doc("batch.rs", &rows_of_code(120));
        let mut h = parsed(&d);
        fill_all(&mut h, &d);

        let first = d.insert(d.line_to_byte(10), "// a\n").expect("insert");
        let HighlightOutcome::Deferred(in_flight) = h.edit_with_budget(&d, &first, Duration::ZERO)
        else {
            panic!("a zero budget must defer");
        };

        let high = d.insert(d.line_to_byte(90), "// c\n").expect("insert");
        let low = d.insert(d.line_to_byte(30), "// b\n").expect("insert");
        let HighlightOutcome::Deferred(batched) = h
            .edit_batch(&d, &[high, low], Duration::ZERO)
            .expect("descending hunks are a valid batch")
        else {
            panic!("a zero budget must defer");
        };

        let parsed = in_flight.run();
        assert!(
            parsed.cancelled,
            "the batch cancelled the parse it found in flight"
        );
        assert!(!h.apply_parsed(&d, parsed));
        assert_eq!(
            batched.generation(),
            h.generation(),
            "the batch left exactly one live deferred parse"
        );
        assert!(h.apply_parsed(&d, batched.run()));
    }
}
