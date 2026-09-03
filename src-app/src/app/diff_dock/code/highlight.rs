use std::ops::{ControlFlow, Range};
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Context, Hsla, WeakEntity};
use ropey::Rope;
use streaming_iterator::StreamingIterator;
use tree_sitter::{
    InputEdit, Node, ParseOptions, ParseState, Parser, Point as TsPoint, QueryCursor, TextProvider,
    Tree,
};

use crate::diff::{
    DiffSyntax, Grammar, MAX_HIGHLIGHT_BYTES, grammar_for_ext, markdown_inline_grammar,
    resolve_runs,
};

use super::document::{CodeDocument, CodeEdit};

pub(crate) const SYNC_PARSE_BUDGET: Duration = Duration::from_millis(1);

pub(crate) type LineRuns = Vec<(Range<usize>, Hsla)>;

struct GrammarPass {
    grammar: &'static Grammar,
    parser: Parser,
    tree: Option<Tree>,
}

pub(crate) enum HighlightOutcome {
    Synced,
    Deferred(DeferredParse),
}

pub(crate) struct DeferredParse {
    generation: u64,
    rope: Rope,
    passes: Vec<(&'static Grammar, Option<Tree>)>,
}

pub(crate) struct ParsedTrees {
    generation: u64,
    len_bytes: usize,
    trees: Vec<Option<Tree>>,
}

impl DeferredParse {
    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn run(self) -> ParsedTrees {
        let len_bytes = self.rope.len_bytes();
        let trees = self
            .passes
            .into_iter()
            .map(|(grammar, old)| {
                let mut parser = Parser::new();
                if parser.set_language(&grammar.language).is_err() {
                    return None;
                }
                parse_rope(&mut parser, &self.rope, old.as_ref(), None)
            })
            .collect();
        ParsedTrees {
            generation: self.generation,
            len_bytes,
            trees,
        }
    }
}

pub(crate) struct CodeHighlighter {
    syntax: DiffSyntax,
    passes: Vec<GrammarPass>,
    rows: Vec<LineRuns>,
    enabled: bool,
    generation: u64,
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
        let enabled = !passes.is_empty();
        let passes = passes
            .into_iter()
            .filter_map(|grammar| {
                let mut parser = Parser::new();
                parser.set_language(&grammar.language).ok()?;
                let tree = parse_rope(&mut parser, doc.text(), None, None);
                Some(GrammarPass {
                    grammar,
                    parser,
                    tree,
                })
            })
            .collect();

        let mut this = Self {
            syntax,
            passes,
            rows: vec![Vec::new(); doc.line_count()],
            enabled,
            generation: 0,
        };
        if this.enabled {
            this.requery_rows(doc, 0..doc.line_count());
        }
        this
    }

    #[allow(dead_code)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[allow(dead_code)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn runs(&self, row: usize) -> &[(Range<usize>, Hsla)] {
        self.rows.get(row).map_or(&[], Vec::as_slice)
    }

    pub(crate) fn set_syntax(&mut self, doc: &CodeDocument, syntax: DiffSyntax) {
        self.syntax = syntax;
        if self.enabled {
            self.requery_rows(doc, 0..doc.line_count());
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
        self.generation = self.generation.wrapping_add(1);
        self.interpolate(doc, edit);
        if !self.enabled {
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
            match parse_rope(&mut pass.parser, doc.text(), old.as_ref(), Some(deadline)) {
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

        if deferred {
            return HighlightOutcome::Deferred(DeferredParse {
                generation: self.generation,
                rope: doc.text().clone(),
                passes: self
                    .passes
                    .iter()
                    .map(|p| (p.grammar, p.tree.clone()))
                    .collect(),
            });
        }

        let rows = self.dirty_rows(doc, &dirty);
        self.requery_rows(doc, rows);
        HighlightOutcome::Synced
    }

    pub(crate) fn apply_parsed(&mut self, doc: &CodeDocument, parsed: ParsedTrees) -> bool {
        if parsed.generation != self.generation || parsed.len_bytes != doc.len_bytes() {
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
        self.requery_rows(doc, 0..doc.line_count());
        true
    }

    fn interpolate(&mut self, doc: &CodeDocument, edit: &CodeEdit) {
        let start_row = edit.start_point.row;
        let old_end_row = edit.old_end_point.row;
        let new_end_row = edit.new_end_point.row;
        if start_row >= self.rows.len() {
            self.rows.resize(doc.line_count(), Vec::new());
            return;
        }

        let start_col = edit.start_point.column;
        let old_end_col = edit.old_end_point.column;
        let new_end_col = edit.new_end_point.column;

        let prefix: LineRuns = self.rows[start_row]
            .iter()
            .filter(|(r, _)| r.start < start_col)
            .map(|(r, c)| (r.start..r.end.min(start_col), *c))
            .filter(|(r, _)| r.start < r.end)
            .collect();
        let suffix: LineRuns = self
            .rows
            .get(old_end_row)
            .map(|runs| {
                runs.iter()
                    .filter(|(r, _)| r.end > old_end_col)
                    .map(|(r, c)| {
                        let s = r.start.max(old_end_col) - old_end_col + new_end_col;
                        let e = r.end - old_end_col + new_end_col;
                        (s..e, *c)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut replacement: Vec<LineRuns> = Vec::with_capacity(new_end_row - start_row + 1);
        if new_end_row == start_row {
            let mut merged = prefix;
            merged.extend(suffix);
            resolve_runs(&mut merged);
            replacement.push(merged);
        } else {
            replacement.push(prefix);
            replacement.resize(new_end_row - start_row, Vec::new());
            replacement.push(suffix);
        }

        let removed_end = (old_end_row + 1).min(self.rows.len());
        self.rows.splice(start_row..removed_end, replacement);
        self.rows.resize(doc.line_count(), Vec::new());
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
        for row in rows.clone() {
            self.rows[row].clear();
        }

        for pass in &self.passes {
            let Some(tree) = pass.tree.as_ref() else {
                continue;
            };
            let names = pass.grammar.query.capture_names();
            let mut cursor = QueryCursor::new();
            cursor.set_byte_range(start_byte..end_byte);
            let mut caps =
                cursor.captures(&pass.grammar.query, tree.root_node(), RopeText(doc.text()));
            while let Some((mat, idx)) = caps.next() {
                let cap = mat.captures[*idx];
                let name = names[cap.index as usize];
                let Some(color) = self.syntax.color_for_capture(name) else {
                    continue;
                };
                bucket_capture(
                    doc,
                    cap.node.start_byte(),
                    cap.node.end_byte(),
                    color,
                    &rows,
                    &mut self.rows,
                );
            }
        }

        for row in rows {
            resolve_runs(&mut self.rows[row]);
        }
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
}

fn bucket_capture(
    doc: &CodeDocument,
    cstart: usize,
    cend: usize,
    color: Hsla,
    rows: &Range<usize>,
    out: &mut [LineRuns],
) {
    if cend <= cstart {
        return;
    }
    let mut row = doc.byte_to_line(cstart).max(rows.start);
    while row < rows.end {
        let Some(lr) = doc.line_byte_range(row) else {
            break;
        };
        if lr.start >= cend {
            break;
        }
        let s = cstart.max(lr.start).saturating_sub(lr.start);
        let e = cend.min(lr.end).saturating_sub(lr.start);
        if e > s {
            out[row].push((s..e, color));
        }
        row += 1;
    }
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
) -> Option<Tree> {
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
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
        match deadline {
            Some(deadline) if Instant::now() >= deadline => ControlFlow::Break(()),
            _ => ControlFlow::Continue(()),
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

    fn assert_parity(name: &str, text: &str) {
        let d = doc(name, text);
        let h = CodeHighlighter::new(&d, syntax());
        assert!(h.is_enabled(), "{name} resolved no grammar");
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
            let at = d.line_to_byte(1);
            let edit = d.insert(at, "\n ").expect("insert");
            let outcome = h.edit_with_budget(&d, &edit, Duration::from_secs(5));
            assert!(
                matches!(outcome, HighlightOutcome::Synced),
                "{name} deferred"
            );

            let after = d.to_disk_string();
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
    fn a_blown_budget_defers_the_parse_and_keeps_the_text_colored() {
        let text = "fn main() {\n    let s = \"hello\";\n    println!(\"{s}\");\n}\n";
        let mut d = doc("deferred.rs", text);
        let mut h = CodeHighlighter::new(&d, syntax());
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
        assert_eq!(h.runs(2), colored_before.as_slice());

        assert!(h.apply_parsed(&d, deferred.run()));
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

        let first = d.insert(0, "//x\n").expect("insert");
        let HighlightOutcome::Deferred(stale) = h.edit_with_budget(&d, &first, Duration::ZERO)
        else {
            panic!("a zero budget must defer");
        };

        let second = d.insert(0, "//y\n").expect("insert");
        let _ = h.edit(&d, &second);
        let snapshot: Vec<_> = (0..d.line_count()).map(|r| h.runs(r).to_vec()).collect();

        assert!(!h.apply_parsed(&d, stale.run()));
        let after: Vec<_> = (0..d.line_count()).map(|r| h.runs(r).to_vec()).collect();
        assert_eq!(snapshot, after);
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

        let start = d.line_to_byte(1);
        let end = d.line_to_byte(2);
        let edit = d.remove(start..end).expect("remove");
        assert!(matches!(
            h.edit_with_budget(&d, &edit, Duration::from_secs(5)),
            HighlightOutcome::Synced
        ));

        let after = d.to_disk_string();
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
}
