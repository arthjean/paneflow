use std::ops::{ControlFlow, Range};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Context, Hsla, WeakEntity};
use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Node, ParseOptions, ParseState, Parser, Point as TsPoint, QueryCursor, TextProvider,
    Tree,
};

use crate::diff::{
    DiffSyntax, Grammar, MAX_CAPTURES_PER_ROW, MAX_HIGHLIGHT_BYTES, grammar_for_ext,
    markdown_inline_grammar, resolve_runs,
};

use super::document::{CodeDocument, CodeEdit};

pub(crate) const SYNC_PARSE_BUDGET: Duration = Duration::from_millis(1);
pub(crate) const HIGHLIGHT_FRAME_BUDGET: Duration = Duration::from_millis(2);

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

pub(crate) struct DeferredParse {
    generation: u64,
    rope: Rope,
    passes: Vec<(&'static Grammar, Option<Tree>)>,
    cancel: Arc<AtomicBool>,
}

pub(crate) struct ParsedTrees {
    generation: u64,
    len_bytes: usize,
    trees: Vec<Option<Tree>>,
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

    pub(crate) fn run(self) -> ParsedTrees {
        let DeferredParse {
            generation,
            rope,
            passes,
            cancel,
        } = self;
        let len_bytes = rope.len_bytes();
        let trees = passes
            .into_iter()
            .map(|(grammar, old)| {
                if cancel.load(Ordering::Relaxed) {
                    return None;
                }
                let mut parser = Parser::new();
                if parser.set_language(&grammar.language).is_err() {
                    return None;
                }
                parse_rope(
                    &mut parser,
                    &rope,
                    old.as_ref(),
                    None,
                    Some(cancel.as_ref()),
                )
            })
            .collect();
        ParsedTrees {
            generation,
            len_bytes,
            trees,
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
    generation: u64,
    deferred_cancel: Option<Arc<AtomicBool>>,
}

impl CodeHighlighter {
    pub(crate) fn new(doc: &CodeDocument, syntax: DiffSyntax) -> Self {
        let mut passes = Vec::new();
        if doc.len_bytes() <= MAX_HIGHLIGHT_BYTES
            && let Some(grammar) = grammar_for_ext(doc.ext())
        {
            passes.push(grammar);
            if matches!(doc.ext(), "md" | "markdown" | "mdx")
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
                let tree = parse_rope(&mut parser, doc.text(), None, None, None);
                Some(GrammarPass {
                    grammar,
                    parser,
                    tree,
                    colors: capture_colors(grammar, &syntax),
                })
            })
            .collect::<Vec<_>>();
        let enabled = !passes.is_empty();

        Self {
            syntax,
            passes,
            rows: vec![Vec::new(); doc.line_count()],
            row_states: vec![
                if enabled {
                    RowState::Stale
                } else {
                    RowState::Fresh
                };
                doc.line_count()
            ],
            enabled,
            generation: 0,
            deferred_cancel: None,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn runs(&self, row: usize) -> LineRuns {
        if self.row_states.get(row) != Some(&RowState::Fresh) {
            return Vec::new();
        }
        self.rows
            .get(row)
            .map(|runs| {
                runs.iter()
                    .filter_map(|&(start, end, indexed)| {
                        let pass = self.passes.get(indexed.pass as usize)?;
                        let color = pass
                            .colors
                            .get(indexed.capture as usize)
                            .copied()
                            .flatten()?;
                        Some((start as usize..end as usize, color))
                    })
                    .collect()
            })
            .unwrap_or_default()
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
        self.cancel_deferred();
        self.generation = self.generation.wrapping_add(1);
        self.interpolate(doc, edit);
        if !self.enabled {
            self.row_states.fill(RowState::Fresh);
            return HighlightOutcome::Synced;
        }

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

        let deadline = Instant::now() + budget;
        let mut dirty = edit.start_byte..edit.new_end_byte.max(edit.start_byte);
        let mut deferred = false;
        for pass in &mut self.passes {
            let old = pass.tree.clone();
            match parse_rope(
                &mut pass.parser,
                doc.text(),
                old.as_ref(),
                Some(deadline),
                None,
            ) {
                Some(new_tree) => {
                    if let Some(old) = old.as_ref() {
                        for range in old.changed_ranges(&new_tree) {
                            dirty.start = dirty.start.min(range.start_byte);
                            dirty.end = dirty.end.max(range.end_byte);
                        }
                    } else {
                        dirty = 0..doc.len_bytes();
                    }
                    pass.tree = Some(new_tree);
                }
                None => {
                    pass.parser.reset();
                    deferred = true;
                }
            }
        }

        let rows = self.dirty_rows(doc, &dirty);
        self.mark_stale(rows);

        if deferred {
            let cancel = Arc::new(AtomicBool::new(false));
            self.deferred_cancel = Some(cancel.clone());
            return HighlightOutcome::Deferred(DeferredParse {
                generation: self.generation,
                rope: doc.text().clone(),
                passes: self
                    .passes
                    .iter()
                    .map(|p| (p.grammar, p.tree.clone()))
                    .collect(),
                cancel,
            });
        }

        HighlightOutcome::Synced
    }

    pub(crate) fn apply_parsed(&mut self, doc: &CodeDocument, parsed: ParsedTrees) -> bool {
        if parsed.cancelled
            || parsed.generation != self.generation
            || parsed.len_bytes != doc.len_bytes()
        {
            return false;
        }
        if parsed.trees.len() != self.passes.len() {
            return false;
        }
        for (pass, tree) in self.passes.iter_mut().zip(parsed.trees) {
            if tree.is_some() {
                pass.tree = tree;
            }
        }
        self.deferred_cancel = None;
        self.mark_stale(0..doc.line_count());
        true
    }

    fn interpolate(&mut self, doc: &CodeDocument, edit: &CodeEdit) {
        let start_row = edit.start_point.row;
        let old_end_row = edit.old_end_point.row;
        let new_end_row = edit.new_end_point.row;
        interpolate_rows(&mut self.rows, doc.line_count(), edit);
        if start_row >= self.row_states.len() {
            self.row_states.resize(doc.line_count(), RowState::Stale);
            return;
        }
        let removed_end = (old_end_row + 1).min(self.row_states.len());
        let replacement = vec![RowState::Stale; new_end_row - start_row + 1];
        self.row_states.splice(start_row..removed_end, replacement);
        self.row_states.resize(doc.line_count(), RowState::Stale);
    }

    fn dirty_rows(&self, doc: &CodeDocument, bytes: &Range<usize>) -> Range<usize> {
        let lines = doc.line_count();
        let first = doc.byte_to_line(bytes.start);
        let last = doc.byte_to_line(bytes.end.max(bytes.start));
        first..(last + 1).min(lines)
    }

    pub(crate) fn requery_rows(&mut self, doc: &CodeDocument, rows: Range<usize>) {
        let lines = doc.line_count();
        if self.rows.len() != lines {
            self.rows.resize(lines, Vec::new());
        }
        if self.row_states.len() != lines {
            self.row_states.resize(lines, RowState::Stale);
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
            let Some(tree) = pass.tree.as_ref() else {
                continue;
            };
            let Ok(pass_index) = u16::try_from(pass_index) else {
                continue;
            };
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(start_byte..end_byte);
            let mut caps =
                cursor.captures(&pass.grammar.query, tree.root_node(), RopeText(doc.text()));
            while let Some((mat, idx)) = caps.next() {
                let cap = mat.captures[*idx];
                let Ok(capture) = u16::try_from(cap.index) else {
                    continue;
                };
                if pass
                    .colors
                    .get(capture as usize)
                    .copied()
                    .flatten()
                    .is_none()
                {
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
        let lines = doc.line_count();
        let rows = rows.start.min(lines)..rows.end.min(lines);
        let deadline = Instant::now() + budget;
        for row in rows.clone() {
            if self.row_states.get(row) != Some(&RowState::Stale) {
                continue;
            }
            self.requery_rows(doc, row..row + 1);
            if Instant::now() >= deadline {
                break;
            }
        }
        let mut stale_rows = 0usize;
        for row in rows {
            if self.row_states.get(row) == Some(&RowState::Stale) {
                stale_rows += 1;
            }
        }
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

    pub(crate) fn has_tree(&self) -> bool {
        self.passes.iter().any(|pass| pass.tree.is_some())
    }

    fn all_rows_stale(&self) -> bool {
        self.row_states
            .iter()
            .all(|state| *state == RowState::Stale)
    }

    fn has_stale_rows(&self) -> bool {
        self.row_states.contains(&RowState::Stale)
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

fn interpolate_rows(rows: &mut Vec<IndexedLineRuns>, line_count: usize, edit: &CodeEdit) {
    let start_row = edit.start_point.row;
    let old_end_row = edit.old_end_point.row;
    let new_end_row = edit.new_end_point.row;
    if start_row >= rows.len() {
        rows.resize(line_count, Vec::new());
        return;
    }

    let start_col = edit.start_point.column;
    let old_end_col = edit.old_end_point.column;
    let new_end_col = edit.new_end_point.column;
    let prefix = rows[start_row]
        .iter()
        .filter_map(|&(start, end, capture)| {
            let start = start as usize;
            let end = (end as usize).min(start_col);
            (start < start_col && start < end).then_some((start as u32, end as u32, capture))
        })
        .collect::<IndexedLineRuns>();
    let suffix = rows
        .get(old_end_row)
        .map(|runs| {
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
                .collect::<IndexedLineRuns>()
        })
        .unwrap_or_default();

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

    let removed_end = (old_end_row + 1).min(rows.len());
    rows.splice(start_row..removed_end, replacement);
    rows.resize(line_count, Vec::new());
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
        let parsed = smol::unblock(move || deferred.run()).await;
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
    use crate::diff::highlight_lines;
    use crate::theme::paneflow_dark;

    fn syntax() -> DiffSyntax {
        DiffSyntax::from_theme(&paneflow_dark())
    }

    fn doc(name: &str, text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from(format!("/tmp/{name}")), text)
    }

    fn corpus() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "sample.rs",
                "use std::fmt;\n\n/// Doc.\npub fn add(a: i32, b: i32) -> i32 {\n    let s = \"text\";\n    a + b // sum\n}\n",
            ),
            (
                "sample.json",
                "{\n  \"name\": \"paneflow\",\n  \"count\": 3,\n  \"ok\": true,\n  \"tags\": [\"a\", \"b\"]\n}\n",
            ),
            (
                "sample.sh",
                "#!/usr/bin/env bash\nset -euo pipefail\nname=\"world\"\nif [ -n \"$name\" ]; then\n  echo \"hello $name\"\nfi\n",
            ),
            (
                "sample.py",
                "import os\n\n\nclass Greeter:\n    \"\"\"Docstring.\"\"\"\n\n    def greet(self, name: str) -> str:\n        return f\"hi {name}\"  # comment\n",
            ),
            (
                "sample.ts",
                "import { readFile } from 'fs';\n\nexport interface User { id: number; name: string }\n\nexport const greet = (u: User): string => `hi ${u.name}`;\n",
            ),
            (
                "sample.tsx",
                "import React from 'react';\n\nexport function App({ title }: { title: string }) {\n  return <div className=\"app\">{title}</div>;\n}\n",
            ),
            (
                "sample.toml",
                "[package]\nname = \"paneflow\"\nversion = \"0.1.0\"\n\n[dependencies]\nropey = { version = \"1.6\", features = [\"simd\"] }\n",
            ),
            (
                "sample.md",
                "# Title\n\nSome **bold** and `code` text.\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n",
            ),
            (
                "sample.go",
                "package main\n\nimport \"fmt\"\n\n// Main entry.\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n",
            ),
            (
                "sample.yaml",
                "name: build\non:\n  push:\n    branches: [main]\njobs:\n  test:\n    runs-on: ubuntu-latest\n",
            ),
            (
                "sample.css",
                ".panel {\n  display: flex;\n  color: #181825; /* dark */\n  padding: 4px 8px;\n}\n",
            ),
            (
                "sample.html",
                "<!doctype html>\n<html>\n  <body>\n    <p class=\"lead\">hello</p>\n  </body>\n</html>\n",
            ),
            (
                "sample.c",
                "#include <stdio.h>\n\nint main(void) {\n    /* comment */\n    printf(\"hi\\n\");\n    return 0;\n}\n",
            ),
            (
                "sample.cpp",
                "#include <string>\n\nnamespace app {\nstd::string greet(const std::string &n) { return \"hi \" + n; }\n}\n",
            ),
            (
                "sample.java",
                "package app;\n\npublic class Main {\n    public static void main(String[] args) {\n        System.out.println(\"hi\");\n    }\n}\n",
            ),
            (
                "sample.rb",
                "# frozen_string_literal: true\n\nclass Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\nend\n",
            ),
        ]
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
            let h = &mut CodeHighlighter::new(&d, syntax());
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
        let fresh = CodeHighlighter::new(&d, syntax()).root_child_ids();
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
        assert!(h.all_rows_stale());
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
        let mut h = CodeHighlighter::new(&d, syntax());
        let edit = d.insert(0, "x").expect("insert");
        let HighlightOutcome::Deferred(parse) = h.edit_with_budget(&d, &edit, Duration::ZERO)
        else {
            panic!("parse must defer");
        };
        drop(h);
        assert!(parse.run().was_cancelled());
    }

    #[test]
    fn unfinished_visible_rows_are_left_stale_for_the_next_frame() {
        let d = doc(
            "progressive.rs",
            "fn one() {}\nfn two() {}\nfn three() {}\n",
        );
        let mut h = CodeHighlighter::new(&d, syntax());
        assert_eq!(h.fill_stale_rows(&d, 0..3, Duration::ZERO).stale_rows, 2);
        assert!(!h.runs(0).is_empty());
        assert!(h.runs(1).is_empty());
        assert!(
            !h.fill_stale_rows(&d, 0..3, Duration::from_secs(1))
                .any_stale()
        );
        assert!(!h.runs(1).is_empty());
        assert!(!h.runs(2).is_empty());
    }

    #[test]
    fn a_viewport_past_the_end_of_the_document_is_clamped_and_counts_only_real_rows() {
        let d = doc("clamped.rs", "fn one() {}\nfn two() {}\nfn three() {}\n");
        let mut h = CodeHighlighter::new(&d, syntax());
        assert_eq!(d.line_count(), 4);

        let starved = h.fill_stale_rows(&d, 0..10_000, Duration::ZERO);
        assert!(starved.any_stale());
        assert_eq!(starved.stale_rows, d.line_count() - 1);

        let filled = h.fill_stale_rows(&d, 0..10_000, Duration::from_secs(1));
        assert!(!filled.any_stale());
        assert_eq!(filled.stale_rows, 0);
    }

    #[test]
    fn a_file_past_the_highlight_cap_stays_editable_and_plain() {
        let mut text = String::with_capacity(MAX_HIGHLIGHT_BYTES + 64);
        while text.len() <= MAX_HIGHLIGHT_BYTES {
            text.push_str("pub fn f() -> i32 { 1 }\n");
        }
        let mut d = doc("huge.rs", &text);
        let mut h = CodeHighlighter::new(&d, syntax());
        assert!(!h.is_enabled());
        assert!(h.runs(0).is_empty());

        let edit = d.insert(0, "// still editable\n").expect("insert");
        assert!(matches!(h.edit(&d, &edit), HighlightOutcome::Synced));
        assert!(!h.has_stale_rows());
        assert!(
            !h.fill_stale_rows(&d, 0..d.line_count(), Duration::ZERO)
                .any_stale()
        );
        assert!(h.runs(0).is_empty());
        assert_eq!(d.line_string(0).as_deref(), Some("// still editable"));
    }

    #[test]
    fn an_unknown_extension_stays_editable_and_plain() {
        let mut d = doc("notes.unknownext", "anything at all\nsecond line\n");
        let mut h = CodeHighlighter::new(&d, syntax());
        assert!(!h.is_enabled());
        let edit = d.insert(0, "x").expect("insert");
        assert!(matches!(h.edit(&d, &edit), HighlightOutcome::Synced));
        assert!(h.runs(0).is_empty());
        assert!(highlight_lines("anything at all\n", "unknownext", &syntax())[0].is_empty());
    }

    #[test]
    fn deleting_a_row_keeps_the_row_map_aligned_with_the_document() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let mut d = doc("del.rs", text);
        let mut h = CodeHighlighter::new(&d, syntax());
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
        let mut h = CodeHighlighter::new(&d, syntax());
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
    fn opening_builds_trees_but_leaves_every_row_stale() {
        let d = doc("lazy.rs", "fn main() {}\n");
        let h = CodeHighlighter::new(&d, syntax());
        assert!(h.has_tree());
        assert!(h.all_rows_stale());
        assert!(h.runs(0).is_empty());
    }
}
