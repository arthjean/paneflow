use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, Query, QueryCursor};

use super::highlighter::{
    Grammar, MAX_CAPTURES_PER_ROW, grammar_for_ext, highlight_lines, markdown_inline_grammar,
    resolve_runs,
};
use super::syntax::DiffSyntax;
use crate::theme::paneflow_dark;

pub(crate) const CORPUS: &[(&str, &str)] = &[
    (
        "sample.rs",
        r#"mod pane;
use gpui::{canvas, div};
const MAX_X: i32 = 42;
#[derive(Clone)]
struct Foo<'a> { field: &'a str }
impl<'a> Foo<'a> {
    fn bar(&self, param: i32) -> i32 {
        foo(); self.bar(param); let value = self.field; println!("{}", value);
        MAX_X + param
    }
}
"#,
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
        "# Title\n\nSome *italic* **bold** and `code` text, [link](https://example.org).\n\n> quote\n\n[id]: https://example.org\n\n- item one\n- item two\n\n```rust\nfn main() {}\n```\n",
    ),
    (
        "sample.go",
        "package main\n\nimport \"fmt\"\n\n// Main entry.\nfunc main() {\n\tfmt.Println(\"hello\")\n}\n",
    ),
    (
        "sample.yaml",
        "count: 3\nenabled: true\nmissing: null\n# workflow\nname: build\non:\n  push:\n    branches: [main]\njobs:\n  test:\n    runs-on: ubuntu-latest\n",
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
        r#"export module app.core;
import std;
#include <string>
namespace app {
template<typename T> concept Numeric = requires(T value) { value + 1; };
template<Numeric T> class Box { public: T value; T get() { return value; } };
std::string greet(const std::string &name) { return "hi " + name; }
}
"#,
    ),
    (
        "sample.java",
        "package app;\n\npublic class Main {\n    public static void main(String[] args) {\n        System.out.println(\"hi\");\n    }\n}\n",
    ),
    (
        "sample.rb",
        "# frozen_string_literal: true\n\nclass Greeter\n  def greet(name)\n    \"hi #{name}\"\n  end\nend\n",
    ),
    (
        "sample.jsonc",
        "{\n // comment\n \"name\": \"hi\\n\", \"count\": 3, \"ok\": true, \"empty\": null\n}\n",
    ),
    (
        "sample.js",
        "// comment\nexport function greet(name) { const count = 3; return <div title=\"hi\">{name + count}</div>; }\n",
    ),
];

fn captures(grammar: &'static Grammar, text: &str) -> Vec<(Range<usize>, &'static str)> {
    let mut parser = Parser::new();
    parser.set_language(&grammar.language).unwrap();
    let tree = parser.parse(text, None).unwrap();
    let mut cursor = QueryCursor::new();
    let mut result = Vec::new();
    let mut matches = cursor.captures(&grammar.query, tree.root_node(), text.as_bytes());
    while let Some((mat, index)) = matches.next() {
        let capture = mat.captures[*index];
        result.push((
            capture.node.byte_range(),
            grammar.query.capture_names()[capture.index as usize],
        ));
    }
    result
}

fn oracle<T: Copy>(captures: &[(Range<usize>, T)], len: usize) -> Vec<Option<T>> {
    let mut ordered: Vec<_> = captures
        .iter()
        .take(MAX_CAPTURES_PER_ROW)
        .enumerate()
        .filter(|(_, (span, _))| span.start < span.end)
        .collect();
    ordered.sort_by_key(|(order, (span, _))| (span.start, *order));
    let mut pending = ordered.into_iter().peekable();
    let mut stack = Vec::new();
    (0..len)
        .map(|byte| {
            while stack.last().is_some_and(|(end, _)| *end <= byte) {
                stack.pop();
            }
            while pending
                .peek()
                .is_some_and(|(_, (span, _))| span.start <= byte)
            {
                let (_, (span, class)) = pending.next().unwrap();
                stack.push((span.end, *class));
            }
            stack.last().map(|(_, class)| *class)
        })
        .collect()
}

fn assert_oracle(text: &str, ext: &str) {
    let syntax = DiffSyntax::from_theme(&paneflow_dark());
    let mut raw = captures(grammar_for_ext(ext).unwrap(), text);
    if matches!(ext, "md" | "markdown" | "mdx") {
        raw.extend(captures(markdown_inline_grammar().unwrap(), text));
    }
    raw.retain(|(_, name)| syntax.color_for_capture(name).is_some());
    let rendered = highlight_lines(text, ext, &syntax);
    for (row, line) in text.lines().enumerate() {
        let start = line.as_ptr() as usize - text.as_ptr() as usize;
        let end = start + line.len();
        let clipped: Vec<_> = raw
            .iter()
            .filter_map(|(span, class)| {
                let from = span.start.max(start);
                let to = span.end.min(end);
                (from < to)
                    .then_some((from.saturating_sub(start)..to.saturating_sub(start), *class))
            })
            .collect();
        let expected = oracle(&clipped, line.len());
        let mut runs = clipped;
        resolve_runs(&mut runs);
        let mut actual = vec![None; line.len()];
        for (span, class) in runs {
            actual[span].fill(Some(class));
        }
        assert_eq!(actual, expected, "class parity {ext} row {row}");
        let mut colors = vec![None; line.len()];
        for (span, color) in &rendered[row] {
            colors[span.clone()].fill(Some(*color));
        }
        let expected_colors: Vec<_> = expected
            .into_iter()
            .map(|name| name.and_then(|name| syntax.color_for_capture(name)))
            .collect();
        assert_eq!(colors, expected_colors, "diff entry point {ext} row {row}");
    }
}

#[test]
fn every_grammar_and_the_diff_entry_point_match_the_independent_byte_oracle() {
    for (name, text) in CORPUS {
        assert_oracle(text, name.rsplit('.').next().unwrap());
    }
}

#[test]
fn benchmark_corpora_match_the_independent_byte_oracle() {
    use crate::app::diff_dock::code::bench_corpus::{
        HIGHLIGHTED_RUST_BYTES, markdown_source, minified_json_line, rust_source,
    };
    for (ext, text) in [
        ("rs", rust_source(HIGHLIGHTED_RUST_BYTES)),
        ("json", minified_json_line(10_000)),
        ("md", markdown_source(100_000)),
    ] {
        assert_oracle(&text, ext);
    }
}

#[test]
fn ten_thousand_overlap_inputs_match_the_independent_stack() {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        seed ^= seed << 7;
        seed ^= seed >> 9;
        seed ^= seed << 8;
        seed as usize
    };
    for case in 0..10_000 {
        let raw: Vec<_> = (0..next() % 24 + 1)
            .map(|class| (next() % 80..next() % 80, class))
            .collect();
        let expected = oracle(&raw, 80);
        let mut runs = raw;
        resolve_runs(&mut runs);
        let mut actual = vec![None; 80];
        for (span, class) in runs {
            actual[span].fill(Some(class));
        }
        assert_eq!(actual, expected, "case {case}");
    }
}

macro_rules! compiles {
    ($test:ident, $ext:literal) => {
        #[test]
        fn $test() {
            assert!(
                grammar_for_ext($ext).is_some(),
                "query failed to compile: {}",
                $ext
            );
        }
    };
}
compiles!(rust_query_compiles, "rs");
compiles!(json_query_compiles, "json");
compiles!(jsonc_query_compiles, "jsonc");
compiles!(bash_query_compiles, "sh");
compiles!(python_query_compiles, "py");
compiles!(typescript_query_compiles, "ts");
compiles!(tsx_query_compiles, "tsx");
compiles!(javascript_query_compiles, "js");
compiles!(markdown_query_compiles, "md");
compiles!(go_query_compiles, "go");
compiles!(yaml_query_compiles, "yaml");
compiles!(css_query_compiles, "css");
compiles!(c_query_compiles, "c");
compiles!(cpp_query_compiles, "cpp");

#[test]
fn markdown_inline_query_compiles() {
    assert!(markdown_inline_grammar().is_some());
}

#[test]
fn extension_aliases_select_the_intended_query() {
    for group in [
        vec!["jsonc"],
        vec!["ts", "mts", "cts"],
        vec!["js", "jsx", "mjs", "cjs"],
        vec!["md", "markdown", "mdx"],
        vec!["c", "h"],
        vec!["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
    ] {
        let first = grammar_for_ext(group[0]).unwrap();
        for ext in group {
            assert!(std::ptr::eq(first, grammar_for_ext(ext).unwrap()));
        }
    }
    assert!(!std::ptr::eq(
        grammar_for_ext("json").unwrap(),
        grammar_for_ext("jsonc").unwrap()
    ));
    assert!(!std::ptr::eq(
        grammar_for_ext("tsx").unwrap(),
        grammar_for_ext("js").unwrap()
    ));
}

#[test]
fn stock_queries_report_every_same_node_priority_change() {
    let syntax = DiffSyntax::from_theme(&paneflow_dark());
    let mut report = Vec::new();
    for (name, text) in CORPUS
        .iter()
        .filter(|(name, _)| !matches!(*name, "sample.rs" | "sample.jsonc" | "sample.js"))
    {
        let ext = name.rsplit('.').next().unwrap();
        let source = match ext {
            "json" => tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
            "sh" => tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
            "py" => tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
            "ts" | "tsx" => tree_sitter_typescript::HIGHLIGHTS_QUERY.to_owned(),
            "toml" => tree_sitter_toml_ng::HIGHLIGHTS_QUERY.to_owned(),
            "md" => tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.to_owned(),
            "go" => tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
            "yaml" => tree_sitter_yaml::HIGHLIGHTS_QUERY.to_owned(),
            "css" => tree_sitter_css::HIGHLIGHTS_QUERY.to_owned(),
            "html" => tree_sitter_html::HIGHLIGHTS_QUERY.to_owned(),
            "c" => tree_sitter_c::HIGHLIGHT_QUERY.to_owned(),
            "cpp" => format!(
                "{}\n{}",
                tree_sitter_c::HIGHLIGHT_QUERY,
                include_str!("fixtures/cpp-stock-0.23.4.scm")
            ),
            "java" => tree_sitter_java::HIGHLIGHTS_QUERY.to_owned(),
            "rb" => tree_sitter_ruby::HIGHLIGHTS_QUERY.to_owned(),
            _ => unreachable!(),
        };
        let language = grammar_for_ext(ext).unwrap().language.clone();
        let query = Query::new(&language, &source).unwrap();
        let mut parser = Parser::new();
        parser.set_language(&language).unwrap();
        let tree = parser.parse(text, None).unwrap();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.captures(&query, tree.root_node(), text.as_bytes());
        let mut nodes: BTreeMap<(usize, usize, usize), Vec<&str>> = BTreeMap::new();
        let mut raw = Vec::new();
        while let Some((mat, index)) = matches.next() {
            let cap = mat.captures[*index];
            let class = query.capture_names()[cap.index as usize];
            if syntax.color_for_capture(class).is_some() {
                nodes
                    .entry((cap.node.start_byte(), cap.node.end_byte(), cap.node.id()))
                    .or_default()
                    .push(class);
                raw.push((cap.node.byte_range(), class));
            }
        }
        let mut conflicts = 0;
        for ((start, end, _), classes) in nodes {
            if classes.first() != classes.last() {
                conflicts += 1;
                report.push(format!(
                    "STOCK {ext} {start}..{end} {:?}: {} -> {}",
                    &text[start..end],
                    classes[0],
                    classes.last().unwrap()
                ));
            }
        }
        report.push(format!("STOCK {ext}: {conflicts} conflicts"));
        let stacked = oracle(&raw, text.len());
        let mut resolved = raw.clone();
        resolve_runs(&mut resolved);
        let mut resolved_bytes = vec![None; text.len()];
        for (span, class) in resolved {
            resolved_bytes[span].fill(Some(class));
        }
        assert_eq!(resolved_bytes, stacked, "stock resolver parity {ext}");
        let mut changes: Vec<(Range<usize>, Option<&str>, Option<&str>)> = Vec::new();
        for (byte, class) in stacked.into_iter().enumerate() {
            let previous = raw
                .iter()
                .enumerate()
                .filter(|(_, (span, _))| span.contains(&byte))
                .min_by_key(|(order, (span, _))| (span.len(), *order))
                .map(|(_, (_, class))| *class);
            if previous != class {
                if let Some((span, from, to)) = changes.last_mut()
                    && span.end == byte
                    && *from == previous
                    && *to == class
                {
                    span.end = byte + 1;
                } else {
                    changes.push((byte..byte + 1, previous, class));
                }
            }
        }
        for (span, from, to) in changes {
            report.push(format!(
                "BYTES {ext} {}..{} {:?}: {from:?} -> {to:?}",
                span.start,
                span.end,
                &text[span.clone()]
            ));
        }
    }
    assert_eq!(
        report.join("\n") + "\n",
        include_str!("fixtures/stock-priority-audit.txt").replace("\r\n", "\n"),
        "stock priority changes require explicit review"
    );
}

#[test]
fn every_vendored_query_is_exercised_by_at_least_five_classes() {
    for (name, text) in CORPUS.iter().filter(|(name, _)| {
        !matches!(
            *name,
            "sample.toml" | "sample.html" | "sample.java" | "sample.rb"
        )
    }) {
        let ext = name.rsplit('.').next().unwrap();
        let raw = captures(grammar_for_ext(ext).unwrap(), text);
        let classes: BTreeSet<_> = raw.iter().map(|(_, name)| *name).collect();
        assert!(classes.len() >= 5, "{name}: {classes:?}");
        println!("CLASSES {name}: {classes:?}");
    }
    let inline = captures(
        markdown_inline_grammar().unwrap(),
        "*italic* **bold** \u{60}code\u{60} [link](https://example.org) ~~strike~~",
    );
    assert!(
        inline
            .iter()
            .map(|(_, name)| *name)
            .collect::<BTreeSet<_>>()
            .len()
            >= 5
    );
}

#[test]
fn rust_tokens_keep_the_zed_classes_and_neutral_lifetimes() {
    let text = CORPUS[0].1;
    let raw = captures(grammar_for_ext("rs").unwrap(), text);
    let syntax = DiffSyntax::from_theme(&paneflow_dark());
    let colored: Vec<_> = raw
        .iter()
        .filter(|(_, name)| syntax.color_for_capture(name).is_some())
        .cloned()
        .collect();
    let bytes = oracle(&colored, text.len());
    for (token, class) in [
        ("pane", "variable"),
        ("canvas", "variable"),
        ("MAX_X", "constant"),
        ("Foo", "type"),
        ("foo()", "function"),
        ("bar(param)", "function.method"),
        ("field;", "property"),
        ("println!", "function.special"),
        ("derive", "attribute"),
        ("self", "variable.special"),
        ("param:", "variable.parameter"),
        ("= 42", "operator"),
        ("->", "operator"),
    ] {
        let start = text.find(token).unwrap();
        assert_eq!(bytes[start], Some(class), "{token}");
    }
    assert!(
        raw.iter()
            .any(|(span, name)| *name == "lifetime" && &text[span.clone()] == "'")
    );
    assert_eq!(syntax.color_for_capture("lifetime"), None);
}

#[test]
fn token_expectations_cover_every_vendored_language() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        (
            "json",
            &[
                ("name", "property.json_key"),
                ("paneflow", "string"),
                ("3", "number"),
                ("true", "boolean"),
                ("{", "punctuation.bracket"),
            ],
        ),
        (
            "jsonc",
            &[
                ("name", "property.json_key"),
                ("comment", "comment"),
                ("3", "number"),
                ("true", "boolean"),
                ("null", "constant.builtin"),
            ],
        ),
        (
            "sh",
            &[
                ("#!", "keyword.directive"),
                ("set", "function"),
                ("name", "variable"),
                ("if", "keyword.control"),
                ("world", "string"),
            ],
        ),
        (
            "py",
            &[
                ("import", "keyword"),
                ("Greeter", "type.class.definition"),
                ("greet", "function.definition"),
                ("self", "variable.special"),
                ("str", "type.builtin"),
            ],
        ),
        (
            "ts",
            &[
                ("import", "keyword.import"),
                ("User", "type"),
                ("id", "property"),
                ("number", "type.builtin"),
                ("fs", "string"),
            ],
        ),
        (
            "tsx",
            &[
                ("import", "keyword.import"),
                ("App", "function"),
                ("className", "attribute.jsx"),
                ("div", "tag.jsx"),
                ("string", "type.builtin"),
            ],
        ),
        (
            "js",
            &[
                ("comment", "comment"),
                ("greet", "function"),
                ("3", "number"),
                ("div", "tag.jsx"),
                ("title", "attribute.jsx"),
            ],
        ),
        (
            "md",
            &[
                ("# Title", "title.markup"),
                (">", "punctuation.markup"),
                ("[id]", "link_text.markup"),
                ("https://example.org", "link_uri.markup"),
                ("- ", "punctuation.list_marker.markup"),
            ],
        ),
        (
            "go",
            &[
                ("package", "keyword"),
                ("fmt", "string"),
                ("// Main", "comment"),
                ("main()", "function"),
                ("Println", "function.method.call"),
            ],
        ),
        (
            "yaml",
            &[
                ("count", "property"),
                ("3", "number"),
                ("true", "boolean"),
                ("null", "constant.builtin"),
                ("# workflow", "comment"),
            ],
        ),
        (
            "css",
            &[
                ("panel", "selector.class"),
                ("display", "property"),
                ("flex", "constant.builtin"),
                ("4", "number"),
                ("/* dark */", "comment"),
            ],
        ),
        (
            "c",
            &[
                ("#include", "keyword.preproc"),
                ("stdio.h", "string"),
                ("main", "function"),
                ("/* comment */", "comment"),
                ("0", "number"),
            ],
        ),
        (
            "cpp",
            &[
                ("module", "keyword"),
                ("app", "module"),
                ("Numeric", "concept"),
                ("Box", "type"),
                ("get", "function"),
                ("return", "keyword.control"),
                ("#include", "keyword.preproc"),
            ],
        ),
    ];
    for (ext, expected) in cases {
        let (_, text) = CORPUS
            .iter()
            .find(|(name, _)| name.rsplit('.').next() == Some(ext))
            .unwrap();
        let raw = captures(grammar_for_ext(ext).unwrap(), text);
        for (token, class) in *expected {
            assert!(
                raw.iter().any(|(span, actual)| actual == class
                    && text[span.clone()].contains(token.trim_end_matches("()"))),
                "{ext}: {token} should have {class}"
            );
        }
    }
    let text = "*italic* **bold** `code` [link](https://example.org) ~~strike~~";
    let raw = captures(markdown_inline_grammar().unwrap(), text);
    for (token, class) in [
        ("italic", "emphasis.markup"),
        ("bold", "emphasis.strong.markup"),
        ("code", "text.literal.markup"),
        ("link", "link_text.markup"),
        ("https://example.org", "link_uri.markup"),
        ("strike", "strikethrough.markup"),
    ] {
        assert!(
            raw.iter()
                .any(|(span, actual)| *actual == class && text[span.clone()].contains(token))
        );
    }
}
