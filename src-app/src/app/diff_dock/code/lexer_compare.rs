use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Parser, QueryCursor};

use super::bench_corpus::lexer_fixture as fixture;
use paneflow_highlights::{LexDocument, language_for_key, token_types};

use crate::diff::{DiffSyntax, Grammar, grammar_for_ext, markdown_inline_grammar};

const ROLES: &[(&str, &[&str])] = &[
    ("comment.doc", &["comment.doc"]),
    ("comment", &["comment"]),
    ("string.escape", &["string.escape", "escape"]),
    ("string.special", &["string.special", "string.regex"]),
    ("string", &["string"]),
    ("text.literal", &["text.literal"]),
    ("title", &["title"]),
    ("link_uri", &["link_uri"]),
    ("link_text", &["link_text"]),
    ("emphasis.strong", &["emphasis.strong"]),
    ("emphasis", &["emphasis"]),
    ("boolean", &["boolean"]),
    ("number", &["number"]),
    ("constant.builtin", &["constant.builtin"]),
    ("constant", &["constant", "selector.class", "selector.id"]),
    ("keyword", &["keyword", "preproc"]),
    ("constructor", &["constructor"]),
    ("type", &["type"]),
    ("function", &["function"]),
    ("attribute", &["attribute", "selector.pseudo"]),
    ("tag", &["tag"]),
    ("property", &["property", "variable.parameter"]),
    ("label", &["label"]),
    ("namespace", &["namespace", "module"]),
    (
        "variable.builtin",
        &["variable.builtin", "variable.special"],
    ),
    ("variable", &["variable"]),
    ("operator", &["operator"]),
    (
        "punctuation.special",
        &["punctuation.special", "punctuation.list_marker"],
    ),
    ("punctuation", &["punctuation"]),
];

fn has_prefix(name: &str, prefix: &str) -> bool {
    name == prefix || (name.starts_with(prefix) && name.as_bytes().get(prefix.len()) == Some(&b'.'))
}

fn role(name: &str) -> Option<&'static str> {
    ROLES
        .iter()
        .find(|(_, prefixes)| prefixes.iter().any(|prefix| has_prefix(name, prefix)))
        .map(|(role, _)| *role)
}

const NONE: &str = "-";

fn captures(grammar: &Grammar, text: &str) -> Vec<(Range<usize>, &'static str)> {
    let mut parser = Parser::new();
    parser.set_language(&grammar.language).expect("a grammar");
    let tree = parser.parse(text, None).expect("a tree");
    let names = grammar.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut out = Vec::new();
    let mut matches = cursor.captures(&grammar.query, tree.root_node(), text.as_bytes());
    while let Some((found, index)) = matches.next() {
        let capture = found.captures[*index];
        let name = names[capture.index as usize];
        if let Some(role) = role(name) {
            out.push((capture.node.byte_range(), role));
        }
    }
    out
}

fn first_wins(captures: &[(Range<usize>, &'static str)], len: usize) -> Vec<&'static str> {
    let mut ordered: Vec<_> = captures
        .iter()
        .enumerate()
        .filter(|(_, (span, _))| span.start < span.end)
        .collect();
    ordered.sort_by_key(|(order, (span, _))| (span.start, *order));
    let mut pending = ordered.into_iter().peekable();
    let mut stack: Vec<(usize, &'static str)> = Vec::new();
    (0..len)
        .map(|byte| {
            while stack.last().is_some_and(|(end, _)| *end <= byte) {
                stack.pop();
            }
            while let Some((_, (span, class))) =
                pending.next_if(|(_, (span, _))| span.start <= byte)
            {
                stack.push((span.end, *class));
            }
            stack.last().map_or(NONE, |(_, class)| *class)
        })
        .collect()
}

fn tree_sitter_roles(ext: &str, text: &str) -> Option<Vec<&'static str>> {
    let grammar = grammar_for_ext(ext)?;
    let mut raw = captures(grammar, text);
    if matches!(ext, "md" | "markdown")
        && let Some(inline) = markdown_inline_grammar()
    {
        raw.extend(captures(inline, text));
    }
    Some(first_wins(&raw, text.len()))
}

fn lexer_roles(language: &str, text: &str) -> Vec<&'static str> {
    let language = language_for_key(language).expect("a lexer language");
    let doc = LexDocument::lex(text, language).expect("a lexed document");
    let mut out = vec![NONE; text.len()];
    for line in 0..doc.line_count() {
        let start = doc.line_start(line).expect("a line");
        for token in doc.line_tokens(line).expect("line tokens") {
            let class = role(token.id.name()).unwrap_or(NONE);
            out[start + token.range.start..start + token.range.end].fill(class);
        }
    }
    out
}

#[derive(Default)]
struct Tally {
    bytes: usize,
    agree: usize,
    confused_with: BTreeMap<&'static str, usize>,
}

struct Comparison {
    label: &'static str,
    significant: usize,
    agree: usize,
    by_role: BTreeMap<&'static str, Tally>,
    lexer_only: usize,
}

fn compare(
    label: &'static str,
    tree: &[&'static str],
    lexed: &[&'static str],
    text: &str,
) -> Comparison {
    let mut comparison = Comparison {
        label,
        significant: 0,
        agree: 0,
        by_role: BTreeMap::new(),
        lexer_only: 0,
    };
    for (byte, (tree, lexed)) in tree.iter().zip(lexed).enumerate() {
        if text.as_bytes()[byte].is_ascii_whitespace() {
            continue;
        }
        if *tree == NONE && *lexed == NONE {
            continue;
        }
        comparison.significant += 1;
        if *tree == NONE {
            comparison.lexer_only += 1;
        }
        let tally = comparison.by_role.entry(tree).or_default();
        tally.bytes += 1;
        if tree == lexed {
            tally.agree += 1;
            comparison.agree += 1;
        } else {
            *tally.confused_with.entry(lexed).or_default() += 1;
        }
    }
    comparison
}

fn percent(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        return 100.0;
    }
    part as f64 * 100.0 / whole as f64
}

fn render(comparison: &Comparison, out: &mut String) {
    let _ = writeln!(
        out,
        "\n### {}\n\nAgreement on {} colored bytes: {:.1}%. Bytes only the lexer colors: {}.\n",
        comparison.label,
        comparison.significant,
        percent(comparison.agree, comparison.significant),
        comparison.lexer_only,
    );
    let _ = writeln!(
        out,
        "| tree-sitter role | bytes | agree | lexer instead (top 3) |\n|---|---|---|---|"
    );
    let mut rows: Vec<_> = comparison.by_role.iter().collect();
    rows.sort_by_key(|(_, tally)| std::cmp::Reverse(tally.bytes - tally.agree));
    for (role, tally) in rows {
        let mut confused: Vec<_> = tally.confused_with.iter().collect();
        confused.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
        let top = confused
            .iter()
            .take(3)
            .map(|(name, count)| format!("{name} {count}"))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(
            out,
            "| {role} | {} | {:.0}% | {top} |",
            tally.bytes,
            percent(tally.agree, tally.bytes),
        );
    }
}

struct Probe {
    language: &'static str,
    construct: &'static str,
    text: &'static str,
    needle: &'static str,
    occurrence: usize,
}

const PROBES: &[Probe] = &[
    Probe {
        language: "rs",
        construct: "Rust macro call",
        text: "fn main() {\n    println!(\"{}\", 1);\n    let v = vec![1, 2];\n}\n",
        needle: "println!",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust vec! macro",
        text: "fn main() {\n    println!(\"{}\", 1);\n    let v = vec![1, 2];\n}\n",
        needle: "vec!",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust macro_rules! definition",
        text: "macro_rules! square {\n    ($x:expr) => { $x * $x };\n}\n",
        needle: "square",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust lifetime in generics",
        text: "struct Holder<'a> {\n    name: &'a str,\n}\n",
        needle: "'a",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust lifetime in a reference",
        text: "struct Holder<'a> {\n    name: &'a str,\n}\n",
        needle: "'a",
        occurrence: 1,
    },
    Probe {
        language: "rs",
        construct: "Rust char literal",
        text: "fn main() {\n    let c = 'x';\n}\n",
        needle: "'x'",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust attribute",
        text: "#[derive(Debug, Clone)]\nstruct Point;\n",
        needle: "derive",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust multi-line string, second line",
        text: "fn main() {\n    let s = \"first line\nsecond line\";\n}\n",
        needle: "second line",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust code after a multi-line string",
        text: "fn main() {\n    let s = \"first line\nsecond line\";\n    let after = 1;\n}\n",
        needle: "after",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust keyword after a multi-line string",
        text: "fn main() {\n    let s = \"first line\nsecond line\";\n    let after = 1;\n}\n",
        needle: "let",
        occurrence: 1,
    },
    Probe {
        language: "rs",
        construct: "Rust raw string, second line",
        text: "fn main() {\n    let s = r#\"first \"quoted\"\nsecond line\"#;\n}\n",
        needle: "second line",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust type in generic argument",
        text: "fn main() {\n    let m: HashMap<String, Vec<u8>> = HashMap::new();\n}\n",
        needle: "Vec",
        occurrence: 0,
    },
    Probe {
        language: "rs",
        construct: "Rust enum variant path",
        text: "fn main() {\n    let o = Option::Some(1);\n}\n",
        needle: "Some",
        occurrence: 0,
    },
    Probe {
        language: "ts",
        construct: "TypeScript generic type argument",
        text: "const cache: Map<string, Array<number>> = new Map();\n",
        needle: "Array",
        occurrence: 0,
    },
    Probe {
        language: "ts",
        construct: "TypeScript generic call",
        text: "const ids = parse<UserId>(raw);\n",
        needle: "UserId",
        occurrence: 0,
    },
    Probe {
        language: "ts",
        construct: "TypeScript comparison read as generic",
        text: "const ok = a < b && c > d;\n",
        needle: "b",
        occurrence: 0,
    },
    Probe {
        language: "ts",
        construct: "TypeScript template literal, second line",
        text: "const html = `first ${name}\nsecond line`;\n",
        needle: "second line",
        occurrence: 0,
    },
    Probe {
        language: "ts",
        construct: "TypeScript interface property",
        text: "interface User {\n  name: string;\n}\n",
        needle: "name",
        occurrence: 0,
    },
    Probe {
        language: "py",
        construct: "Python triple-quoted string, second line",
        text: "def f():\n    \"\"\"First line.\n    second line\n    \"\"\"\n    return 1\n",
        needle: "second line",
        occurrence: 0,
    },
    Probe {
        language: "py",
        construct: "Python decorator",
        text: "@cache\ndef f():\n    return 1\n",
        needle: "cache",
        occurrence: 0,
    },
    Probe {
        language: "py",
        construct: "Python f-string interpolation",
        text: "name = 1\nprint(f\"hi {name}\")\n",
        needle: "name",
        occurrence: 1,
    },
    Probe {
        language: "md",
        construct: "Markdown fenced Rust code (nested language)",
        text: "# Title\n\n```rust\nfn main() {}\n```\n",
        needle: "fn",
        occurrence: 0,
    },
    Probe {
        language: "md",
        construct: "Markdown inline code",
        text: "Use `cargo test` here.\n",
        needle: "cargo test",
        occurrence: 0,
    },
    Probe {
        language: "html",
        construct: "HTML script body (nested language)",
        text: "<html>\n<script>\nconst answer = 42;\n</script>\n</html>\n",
        needle: "const",
        occurrence: 0,
    },
    Probe {
        language: "html",
        construct: "HTML style body (nested language)",
        text: "<style>\nbody { color: red; }\n</style>\n",
        needle: "color",
        occurrence: 0,
    },
    Probe {
        language: "html",
        construct: "HTML attribute name",
        text: "<a href=\"/docs\">Docs</a>\n",
        needle: "href",
        occurrence: 0,
    },
];

fn locate(text: &str, needle: &str, occurrence: usize) -> Range<usize> {
    let start = text
        .match_indices(needle)
        .nth(occurrence)
        .map(|(at, _)| at)
        .expect("the probe needle");
    start..start + needle.len()
}

fn summarize(roles: &[&'static str], range: Range<usize>) -> String {
    let mut seen: Vec<&'static str> = Vec::new();
    for role in &roles[range] {
        if seen.last() != Some(role) {
            seen.push(role);
        }
    }
    seen.join(" + ")
}

#[test]
fn every_role_label_matches_the_editor_palette() {
    let syntax = DiffSyntax::from_theme(&crate::theme::paneflow_dark());
    let mut names: Vec<&str> = token_types().to_vec();
    for ext in ["rs", "ts", "py", "md", "html", "json", "css", "go"] {
        if let Some(grammar) = grammar_for_ext(ext) {
            names.extend(grammar.query.capture_names());
        }
    }
    for name in names {
        assert_eq!(
            role(name).is_some(),
            syntax.color_for_capture(name).is_some(),
            "{name}"
        );
    }
}

#[test]
#[ignore = "lexer correctness report: cargo test -p paneflow-app --bin paneflow app::diff_dock::code::lexer_compare::lexer_correctness_report -- --ignored --exact --nocapture"]
fn lexer_correctness_report() {
    let mut out = String::from("## Lexer correctness report\n");
    let files: [(&str, &'static str, &str); 5] = [
        ("rs", "Rust, the 300 KB bench corpus", "rs"),
        ("ts", "TypeScript, scripts/generate-file-icons.ts", "ts"),
        ("py", "Python fixture", "py"),
        ("md", "Markdown with code blocks, the bench corpus", "md"),
        ("html", "HTML with script and style", "html"),
    ];
    for (ext, label, language) in files {
        let text = fixture(ext);
        let tree = tree_sitter_roles(ext, &text).expect("a tree-sitter grammar");
        let lexed = lexer_roles(language, &text);
        render(&compare(label, &tree, &lexed, &text), &mut out);
    }
    let _ = writeln!(
        out,
        "\n### Constructs\n\n| language | construct | text | tree-sitter | lexer |\n|---|---|---|---|---|"
    );
    for probe in PROBES {
        let range = locate(probe.text, probe.needle, probe.occurrence);
        let tree = tree_sitter_roles(probe.language, probe.text).expect("a grammar");
        let lexed = lexer_roles(probe.language, probe.text);
        let _ = writeln!(
            out,
            "| {} | {} | `{}` | {} | {} |",
            probe.language,
            probe.construct,
            probe.needle,
            summarize(&tree, range.clone()),
            summarize(&lexed, range),
        );
    }
    println!("{out}");
}
