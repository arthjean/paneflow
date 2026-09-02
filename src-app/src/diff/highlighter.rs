use std::ops::Range;
use std::sync::OnceLock;

use gpui::Hsla;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Parser, Query, QueryCursor};

use super::syntax::DiffSyntax;

pub(crate) const MAX_HIGHLIGHT_BYTES: usize = 300_000;

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
            tree_sitter_rust::HIGHLIGHTS_QUERY
        ),
        "json" | "jsonc" => grammar!(
            JSON,
            tree_sitter_json::LANGUAGE,
            tree_sitter_json::HIGHLIGHTS_QUERY
        ),
        "sh" | "bash" | "zsh" => grammar!(
            BASH,
            tree_sitter_bash::LANGUAGE,
            tree_sitter_bash::HIGHLIGHT_QUERY
        ),
        "py" | "pyi" => grammar!(
            PY,
            tree_sitter_python::LANGUAGE,
            tree_sitter_python::HIGHLIGHTS_QUERY
        ),
        "ts" | "mts" | "cts" => grammar!(
            TS,
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            tree_sitter_typescript::HIGHLIGHTS_QUERY
        ),
        "tsx" | "jsx" | "js" | "mjs" | "cjs" => grammar!(
            TSX,
            tree_sitter_typescript::LANGUAGE_TSX,
            tree_sitter_typescript::HIGHLIGHTS_QUERY
        ),
        "toml" => grammar!(
            TOML,
            tree_sitter_toml_ng::LANGUAGE,
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY
        ),
        "md" | "markdown" | "mdx" => grammar!(
            MD,
            tree_sitter_md::LANGUAGE,
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK
        ),
        "go" => grammar!(
            GO,
            tree_sitter_go::LANGUAGE,
            tree_sitter_go::HIGHLIGHTS_QUERY
        ),
        "yaml" | "yml" => grammar!(
            YAML,
            tree_sitter_yaml::LANGUAGE,
            tree_sitter_yaml::HIGHLIGHTS_QUERY
        ),
        "css" => grammar!(
            CSS,
            tree_sitter_css::LANGUAGE,
            tree_sitter_css::HIGHLIGHTS_QUERY
        ),
        "html" | "htm" => grammar!(
            HTML,
            tree_sitter_html::LANGUAGE,
            tree_sitter_html::HIGHLIGHTS_QUERY
        ),
        "c" | "h" => grammar!(C, tree_sitter_c::LANGUAGE, tree_sitter_c::HIGHLIGHT_QUERY),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" => grammar!(
            CPP,
            tree_sitter_cpp::LANGUAGE,
            &format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                tree_sitter_cpp::HIGHLIGHT_QUERY
            )
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
            let query = Query::new(&language, tree_sitter_md::HIGHLIGHT_QUERY_INLINE).ok()?;
            Some(Grammar { language, query })
        })
        .as_ref()
}

pub fn highlight_lines(
    text: &str,
    ext: &str,
    syntax: &DiffSyntax,
) -> Vec<Vec<(Range<usize>, Hsla)>> {
    if text.len() > MAX_HIGHLIGHT_BYTES {
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

    if matches!(ext, "md" | "markdown" | "mdx")
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

pub(crate) fn resolve_runs(runs: &mut Vec<(Range<usize>, Hsla)>) {
    if runs.len() < 2 {
        return;
    }
    let mut candidates: Vec<_> = runs
        .drain(..)
        .enumerate()
        .map(|(order, (range, color))| (range, color, order))
        .collect();
    candidates.sort_by(|a, b| {
        let a_len = a.0.end.saturating_sub(a.0.start);
        let b_len = b.0.end.saturating_sub(b.0.start);
        a_len
            .cmp(&b_len)
            .then(a.0.start.cmp(&b.0.start))
            .then(a.0.end.cmp(&b.0.end))
            .then(a.2.cmp(&b.2))
    });

    let mut kept: Vec<(Range<usize>, Hsla)> = Vec::with_capacity(candidates.len());
    let mut covered: Vec<Range<usize>> = Vec::with_capacity(candidates.len());
    for (range, color, _) in candidates {
        if range.start >= range.end {
            continue;
        }
        let mut fragments = vec![range];
        for cover in &covered {
            let mut next = Vec::new();
            for fragment in fragments {
                if cover.end <= fragment.start || cover.start >= fragment.end {
                    next.push(fragment);
                    continue;
                }
                if fragment.start < cover.start {
                    next.push(fragment.start..cover.start);
                }
                if cover.end < fragment.end {
                    next.push(cover.end..fragment.end);
                }
            }
            fragments = next;
            if fragments.is_empty() {
                break;
            }
        }
        for fragment in fragments {
            covered.push(fragment.clone());
            kept.push((fragment, color));
        }
        covered.sort_by(|a, b| a.start.cmp(&b.start).then(a.end.cmp(&b.end)));
    }
    kept.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.end.cmp(&b.0.end)));
    *runs = kept;
}

#[cfg(test)]
mod tests {
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
