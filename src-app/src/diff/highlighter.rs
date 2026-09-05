use std::ops::Range;
use std::sync::OnceLock;

use gpui::Hsla;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use super::syntax::DiffSyntax;

pub(crate) const MAX_HIGHLIGHT_BYTES: usize = 2_000_000;

pub(crate) const MAX_MARKDOWN_HIGHLIGHT_BYTES: usize = 1_000_000;

pub(crate) const MAX_CAPTURES_PER_ROW: usize = 4_096;

pub(crate) fn is_markdown(ext: &str) -> bool {
    matches!(ext, "md" | "markdown" | "mdx")
}

pub(crate) fn highlight_cap(ext: &str) -> usize {
    if is_markdown(ext) {
        MAX_MARKDOWN_HIGHLIGHT_BYTES
    } else {
        MAX_HIGHLIGHT_BYTES
    }
}

pub(crate) struct Grammar {
    pub(crate) language: Language,
    pub(crate) query: Query,
}

pub(crate) fn grammar_for_ext(ext: &str) -> Option<&'static Grammar> {
    macro_rules! grammar {
        ($cell:ident, $lang:expr, $query:expr) => {{
            static $cell: OnceLock<Option<Grammar>> = OnceLock::new();
            $cell
                .get_or_init(|| {
                    let language: Language = $lang.into();
                    let query = Query::new(&language, $query).ok()?;
                    Some(Grammar { language, query })
                })
                .as_ref()
        }};
    }
    match ext {
        "rs" => grammar!(
            RUST,
            tree_sitter_rust::LANGUAGE,
            include_str!("queries/rust/highlights.scm")
        ),
        "json" => grammar!(
            JSON,
            tree_sitter_json::LANGUAGE,
            include_str!("queries/json/highlights.scm")
        ),
        "jsonc" => grammar!(
            JSONC,
            tree_sitter_json::LANGUAGE,
            include_str!("queries/jsonc/highlights.scm")
        ),
        "sh" | "bash" | "zsh" => grammar!(
            BASH,
            tree_sitter_bash::LANGUAGE,
            include_str!("queries/bash/highlights.scm")
        ),
        "py" | "pyi" => grammar!(
            PY,
            tree_sitter_python::LANGUAGE,
            include_str!("queries/python/highlights.scm")
        ),
        "ts" | "mts" | "cts" => grammar!(
            TS,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            include_str!("queries/typescript/highlights.scm")
        ),
        "tsx" => grammar!(
            TSX,
            tree_sitter_typescript::LANGUAGE_TSX,
            include_str!("queries/tsx/highlights.scm")
        ),
        "jsx" | "js" | "mjs" | "cjs" => grammar!(
            JS,
            tree_sitter_typescript::LANGUAGE_TSX,
            include_str!("queries/javascript/highlights.scm")
        ),
        "toml" => grammar!(
            TOML,
            tree_sitter_toml_ng::LANGUAGE,
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY
        ),
        "md" | "markdown" | "mdx" => grammar!(
            MD,
            tree_sitter_md::LANGUAGE,
            include_str!("queries/markdown/highlights.scm")
        ),
        "go" => grammar!(
            GO,
            tree_sitter_go::LANGUAGE,
            include_str!("queries/go/highlights.scm")
        ),
        "yaml" | "yml" => grammar!(
            YAML,
            tree_sitter_yaml::LANGUAGE,
            include_str!("queries/yaml/highlights.scm")
        ),
        "css" => grammar!(
            CSS,
            tree_sitter_css::LANGUAGE,
            include_str!("queries/css/highlights.scm")
        ),
        "html" | "htm" => grammar!(
            HTML,
            tree_sitter_html::LANGUAGE,
            tree_sitter_html::HIGHLIGHTS_QUERY
        ),
        "c" | "h" => grammar!(
            C,
            tree_sitter_c::LANGUAGE,
            include_str!("queries/c/highlights.scm")
        ),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => grammar!(
            CPP,
            tree_sitter_cpp::LANGUAGE,
            include_str!("queries/cpp/highlights.scm")
        ),
        "java" => grammar!(
            JAVA,
            tree_sitter_java::LANGUAGE,
            tree_sitter_java::HIGHLIGHTS_QUERY
        ),
        "rb" => grammar!(
            RUBY,
            tree_sitter_ruby::LANGUAGE,
            tree_sitter_ruby::HIGHLIGHTS_QUERY
        ),
        _ => None,
    }
}

pub(crate) fn markdown_inline_grammar() -> Option<&'static Grammar> {
    static MD_INLINE: OnceLock<Option<Grammar>> = OnceLock::new();
    MD_INLINE
        .get_or_init(|| {
            let language: Language = tree_sitter_md::INLINE_LANGUAGE.into();
            let query = Query::new(
                &language,
                include_str!("queries/markdown-inline/highlights.scm"),
            )
            .ok()?;
            Some(Grammar { language, query })
        })
        .as_ref()
}

pub fn highlight_lines(
    text: &str,
    ext: &str,
    syntax: &DiffSyntax,
) -> Vec<Vec<(Range<usize>, Hsla)>> {
    if text.len() > highlight_cap(ext) {
        return text.lines().map(|_| Vec::new()).collect();
    }

    let line_ranges: Vec<Range<usize>> = text
        .lines()
        .map(|l| {
            let start = l.as_ptr() as usize - text.as_ptr() as usize;
            start..start + l.len()
        })
        .collect();
    let mut out: Vec<Vec<(Range<usize>, Hsla)>> = vec![Vec::new(); line_ranges.len()];

    let Some(grammar) = grammar_for_ext(ext) else {
        return out;
    };
    apply_grammar(grammar, text, syntax, &line_ranges, &mut out);

    if is_markdown(ext)
        && let Some(inline) = markdown_inline_grammar()
    {
        apply_grammar(inline, text, syntax, &line_ranges, &mut out);
    }

    for runs in &mut out {
        resolve_runs(runs);
    }
    out
}

fn apply_grammar(
    grammar: &Grammar,
    text: &str,
    syntax: &DiffSyntax,
    line_ranges: &[Range<usize>],
    out: &mut [Vec<(Range<usize>, Hsla)>],
) {
    let mut parser = Parser::new();
    if parser.set_language(&grammar.language).is_err() {
        return;
    }
    let Some(tree) = parser.parse(text, None) else {
        return;
    };

    let names = grammar.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut caps = cursor.captures(&grammar.query, tree.root_node(), text.as_bytes());
    while let Some((mat, idx)) = caps.next() {
        let cap = mat.captures[*idx];
        let name = names[cap.index as usize];
        let Some(color) = syntax.color_for_capture(name) else {
            continue;
        };
        bucket_capture(
            cap.node.start_byte(),
            cap.node.end_byte(),
            color,
            line_ranges,
            out,
        );
    }
}

fn bucket_capture(
    cstart: usize,
    cend: usize,
    color: Hsla,
    line_ranges: &[Range<usize>],
    out: &mut [Vec<(Range<usize>, Hsla)>],
) {
    if cend <= cstart || line_ranges.is_empty() {
        return;
    }
    let mut li = line_ranges.partition_point(|r| r.end <= cstart);
    while li < line_ranges.len() {
        let lr = &line_ranges[li];
        if lr.start >= cend {
            break;
        }
        let s = cstart.max(lr.start) - lr.start;
        let e = cend.min(lr.end) - lr.start;
        if e > s {
            out[li].push((s..e, color));
        }
        li += 1;
    }
}

pub(crate) fn resolve_runs<T: Copy>(runs: &mut Vec<(Range<usize>, T)>) {
    let mut captures: Vec<_> = runs
        .drain(..)
        .take(MAX_CAPTURES_PER_ROW)
        .filter(|(range, _)| range.start < range.end)
        .collect();
    captures.sort_by_key(|(range, _)| range.start);
    let mut stack: Vec<usize> = Vec::with_capacity(captures.len());
    let mut next = 0;
    let mut offset = captures.first().map_or(0, |(range, _)| range.start);
    let mut last_capture = None;
    while next < captures.len() || !stack.is_empty() {
        while stack
            .last()
            .is_some_and(|&index| captures[index].0.end <= offset)
        {
            stack.pop();
        }
        while next < captures.len() && captures[next].0.start <= offset {
            stack.push(next);
            next += 1;
        }
        let next_start = captures
            .get(next)
            .map_or(usize::MAX, |(range, _)| range.start);
        if let Some(&index) = stack.last() {
            let end = captures[index].0.end.min(next_start);
            if last_capture == Some(index)
                && let Some((range, _)) = runs.last_mut()
            {
                range.end = end;
            } else {
                runs.push((offset..end, captures[index].1));
            }
            last_capture = Some(index);
            offset = end;
        } else {
            offset = next_start;
            last_capture = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::theme::paneflow_dark;

    #[test]
    fn highlights_rust_keyword() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let lines = highlight_lines("fn main() {}", "rs", &syn);
        assert_eq!(lines.len(), 1, "one run-list per input line");
        assert!(
            !lines[0].is_empty(),
            "expected colored runs for recognized rust code"
        );
        for w in lines[0].windows(2) {
            assert!(w[0].0.end <= w[1].0.start);
        }
    }

    #[test]
    fn the_diff_view_caps_markdown_at_its_own_two_pass_budget() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let mut text = String::with_capacity(MAX_MARKDOWN_HIGHLIGHT_BYTES + 128);
        while text.len() <= MAX_MARKDOWN_HIGHLIGHT_BYTES {
            text.push_str("# Heading with `code`, *emphasis* and [a link](https://paneflow.dev)\n");
        }
        assert!(
            text.len() < MAX_HIGHLIGHT_BYTES,
            "the markdown cap must be the lower of the two, or this test proves nothing"
        );

        let lines = highlight_lines(&text, "md", &syn);
        assert_eq!(lines.len(), text.lines().count(), "one run-list per line");
        assert!(
            lines.iter().all(Vec::is_empty),
            "a markdown side past its own cap must not build two trees for the diff view"
        );

        let under = "# Title\n\nSome `code` here.\n";
        assert!(
            highlight_lines(under, "md", &syn)
                .iter()
                .any(|runs| !runs.is_empty()),
            "markdown under the cap still colors"
        );
    }

    #[test]
    fn line_count_matches_input() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let lines = highlight_lines("let a = 1;\nlet b = 2;\n", "rs", &syn);
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn unknown_extension_returns_empty_runs_without_panic() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let lines = highlight_lines("plain text line\nsecond", "xyz", &syn);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|r| r.is_empty()));
    }

    fn has_color(lines: &[Vec<(Range<usize>, Hsla)>]) -> bool {
        lines.iter().any(|r| !r.is_empty())
    }

    fn distinct_colors(lines: &[Vec<(Range<usize>, Hsla)>]) -> usize {
        let mut seen: Vec<Hsla> = Vec::new();
        for line in lines {
            for (_, c) in line {
                if !seen.contains(c) {
                    seen.push(*c);
                }
            }
        }
        seen.len()
    }

    #[test]
    fn new_p1_grammars_produce_colored_runs() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let cases: &[(&str, &str)] = &[
            (
                "go",
                "package main\n\nfunc main() {\n\tvar x string = \"hi\"\n\t_ = x\n}\n",
            ),
            ("yaml", "name: paneflow\nport: 8080\nenabled: true\n"),
            ("css", ".btn {\n  color: #ffffff;\n  margin: 0;\n}\n"),
            ("html", "<div class=\"x\">\n  <p>hello</p>\n</div>\n"),
        ];
        for (ext, src) in cases {
            let lines = highlight_lines(src, ext, &syn);
            assert!(has_color(&lines), "expected colored runs for {ext} snippet");
        }
    }

    #[test]
    fn new_p2_grammars_produce_colored_runs() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let cases: &[(&str, &str)] = &[
            (
                "c",
                "#include <stdio.h>\nint main(void) {\n  return 0;\n}\n",
            ),
            (
                "cpp",
                "#include <vector>\nint main() {\n  std::vector<int> v;\n  return 0;\n}\n",
            ),
            (
                "java",
                "class A {\n  void f() {\n    String s = \"x\";\n  }\n}\n",
            ),
            ("rb", "def foo\n  x = \"bar\"\n  puts x\nend\n"),
        ];
        for (ext, src) in cases {
            let lines = highlight_lines(src, ext, &syn);
            assert!(has_color(&lines), "expected colored runs for {ext} snippet");
        }
    }

    #[test]
    fn markdown_block_and_inline_passes_color_richly() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let doc = "# Heading\n\nSome **bold** text and a [link](https://paneflow.dev).\n\n- first item\n- second item\n\n```rust\nfn x() {}\n```\n";
        let lines = highlight_lines(doc, "md", &syn);
        assert!(has_color(&lines), "expected colored markdown runs");
        assert!(
            distinct_colors(&lines) >= 3,
            "expected ≥3 distinct markdown colors (heading/code/link/marker), got {}",
            distinct_colors(&lines)
        );
        for line in &lines {
            for w in line.windows(2) {
                assert!(
                    w[0].0.end <= w[1].0.start,
                    "merged markdown runs must be non-overlapping"
                );
            }
        }
    }

    #[test]
    fn resolve_runs_preserves_nested_specific_captures() {
        let palette = paneflow_dark().syntax;
        let mut runs = vec![
            (0..10, palette.text_literal),
            (2..5, palette.emphasis_strong),
            (7..9, palette.link_text),
        ];

        resolve_runs(&mut runs);

        let ranges: Vec<Range<usize>> = runs.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, vec![0..2, 2..5, 5..7, 7..9, 9..10]);
        assert_eq!(runs[1].1, palette.emphasis_strong);
        assert_eq!(runs[3].1, palette.link_text);
    }

    #[test]
    fn resolve_runs_uses_capture_order_even_when_the_last_capture_is_wider() {
        let mut runs = vec![(0..3, 1), (0..8, 2), (2..5, 3), (2..5, 4)];
        resolve_runs(&mut runs);
        assert_eq!(runs, vec![(0..2, 2), (2..5, 4), (5..8, 2)]);
    }

    #[test]
    fn resolve_runs_ignores_captures_past_the_row_cap() {
        let color = paneflow_dark().syntax.text_literal;
        let mut runs: Vec<_> = (0..MAX_CAPTURES_PER_ROW + 512)
            .map(|index| (index * 2..index * 2 + 1, color))
            .collect();
        let started = Instant::now();
        resolve_runs(&mut runs);
        assert_eq!(runs.len(), MAX_CAPTURES_PER_ROW);
        assert_eq!(runs.last().map(|run| run.0.clone()), Some(8190..8191));
        assert!(started.elapsed() < Duration::from_millis(5));
    }

    #[test]
    fn resolve_runs_drops_empty_and_reversed_ranges() {
        let color = paneflow_dark().syntax.text_literal;
        let mut runs = vec![0..0, Range { start: 4, end: 2 }, 1..3]
            .into_iter()
            .map(|range| (range, color))
            .collect();
        resolve_runs(&mut runs);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, 1..3);
    }

    #[test]
    fn malformed_and_empty_inputs_never_panic() {
        let syn = DiffSyntax::from_theme(&paneflow_dark());
        let exts = [
            "go", "yaml", "yml", "css", "html", "c", "cpp", "java", "rb", "md",
        ];
        for ext in exts {
            let _ = highlight_lines("", ext, &syn);
            let _ = highlight_lines(">>>;;;@@@ \0 not valid {[(", ext, &syn);
            let _ = highlight_lines("\n\n\n", ext, &syn);
        }
    }

    #[test]
    fn malformed_query_compiles_to_none_not_panic() {
        let language: Language = tree_sitter_rust::LANGUAGE.into();
        let bad = Query::new(&language, "(this is not a valid query");
        assert!(
            bad.is_err(),
            "a malformed query must Err so `.ok()?` degrades to monochrome"
        );
    }
}
