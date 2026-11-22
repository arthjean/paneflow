use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tree_sitter::{Language, Query};

use super::highlighter::{Grammar, grammar_for_ext, markdown_inline_grammar};

#[derive(Debug, Clone, PartialEq, Eq)]
struct UnevaluatedPredicate {
    pattern: usize,
    operator: String,
}

impl fmt::Display for UnevaluatedPredicate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pattern {} uses #{}, which the tree-sitter runtime never evaluates",
            self.pattern, self.operator
        )
    }
}

fn unevaluated_predicates(query: &Query) -> Vec<UnevaluatedPredicate> {
    (0..query.pattern_count())
        .flat_map(|pattern| {
            query
                .general_predicates(pattern)
                .iter()
                .map(move |predicate| UnevaluatedPredicate {
                    pattern,
                    operator: predicate.operator.to_string(),
                })
        })
        .collect()
}

const PREDICATES_ZED_IGNORES_TOO: &[(&str, usize, &str)] = &[("cpp", 13, "has-ancestor?")];

fn registered_extensions() -> Vec<&'static str> {
    let source = include_str!("highlighter.rs");
    let start = source.find("match ext {").unwrap();
    let end = start + source[start..].find("_ => None").unwrap();
    source[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|literal| !literal.contains('/'))
        .collect()
}

fn shipped_queries() -> Vec<(&'static str, &'static Grammar)> {
    let mut queries: Vec<(&'static str, &'static Grammar)> = Vec::new();
    for ext in registered_extensions() {
        let grammar = grammar_for_ext(ext)
            .unwrap_or_else(|| panic!("{ext} is registered but its query does not compile"));
        if !queries
            .iter()
            .any(|(_, known)| std::ptr::eq(*known, grammar))
        {
            queries.push((ext, grammar));
        }
    }
    queries.push(("markdown-inline", markdown_inline_grammar().unwrap()));
    queries
}

#[test]
fn the_extension_scan_sees_every_registered_grammar() {
    let extensions = registered_extensions();
    for ext in ["rs", "cjs", "hxx", "rb", "zig", "zon"] {
        assert!(
            extensions.contains(&ext),
            "{ext} missing from {extensions:?}"
        );
    }
    let unique: BTreeSet<_> = extensions.iter().collect();
    assert_eq!(unique.len(), extensions.len(), "{extensions:?}");
    assert_eq!(shipped_queries().len(), 20);
}

#[test]
fn no_shipped_query_carries_a_predicate_the_runtime_ignores() {
    let mut found = Vec::new();
    for (name, grammar) in shipped_queries() {
        for predicate in unevaluated_predicates(&grammar.query) {
            found.push((name, predicate.pattern, predicate.operator));
        }
    }
    let accepted: Vec<_> = PREDICATES_ZED_IGNORES_TOO
        .iter()
        .map(|(name, pattern, operator)| (*name, *pattern, (*operator).to_owned()))
        .collect();
    let unexpected: Vec<_> = found
        .iter()
        .filter(|entry| !accepted.contains(entry))
        .map(|(name, pattern, operator)| {
            format!(
                "{name}: {}",
                UnevaluatedPredicate {
                    pattern: *pattern,
                    operator: operator.clone()
                }
            )
        })
        .collect();
    assert!(unexpected.is_empty(), "{}", unexpected.join("\n"));
    let stale: Vec<_> = accepted
        .iter()
        .filter(|entry| !found.contains(entry))
        .collect();
    assert!(stale.is_empty(), "accepted but absent: {stale:?}");
}

#[test]
fn a_lua_match_predicate_is_reported_with_its_operator_and_pattern() {
    let language: Language = tree_sitter_zig::LANGUAGE.into();
    let query = Query::new(
        &language,
        "(identifier) @variable\n((identifier) @type (#lua-match? @type \"^[A-Z]\"))",
    )
    .unwrap();
    let failures = unevaluated_predicates(&query);
    assert_eq!(
        failures,
        vec![UnevaluatedPredicate {
            pattern: 1,
            operator: "lua-match?".to_owned()
        }]
    );
    assert_eq!(
        failures[0].to_string(),
        "pattern 1 uses #lua-match?, which the tree-sitter runtime never evaluates"
    );
}

#[test]
fn the_zig_crate_query_is_rejected_for_its_lua_match_predicates() {
    let language: Language = tree_sitter_zig::LANGUAGE.into();
    let query = Query::new(&language, tree_sitter_zig::HIGHLIGHTS_QUERY).unwrap();
    let failures = unevaluated_predicates(&query);
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(
        failures
            .iter()
            .all(|failure| failure.operator == "lua-match?")
    );
}

#[test]
fn property_settings_and_evaluated_predicates_pass() {
    let language: Language = tree_sitter_zig::LANGUAGE.into();
    let query = Query::new(
        &language,
        "((string) @string (#set! \"priority\" 95))\n((identifier) @type (#match? @type \"^[A-Z]\"))\n((identifier) @variable.builtin (#eq? @variable.builtin \"_\"))\n((builtin_identifier) @keyword.import (#any-of? @keyword.import \"@import\" \"@cImport\"))",
    )
    .unwrap();
    assert_eq!(unevaluated_predicates(&query), Vec::new());
}

fn queries_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/diff/queries")
}

const SOURCE_FIELDS: [&str; 4] = ["repository", "commit", "license", "license_evidence"];

fn manifest_failures(root: &Path) -> Vec<String> {
    let manifest = match std::fs::read_to_string(root.join("MANIFEST.toml")) {
        Ok(text) => text,
        Err(error) => return vec![format!("MANIFEST.toml: {error}")],
    };
    let manifest: toml::Table = match manifest.parse() {
        Ok(table) => table,
        Err(error) => return vec![format!("MANIFEST.toml: {error}")],
    };
    let mut failures = Vec::new();
    for field in SOURCE_FIELDS {
        if manifest.get(field).and_then(toml::Value::as_str).is_none() {
            failures.push(format!("MANIFEST.toml: missing top-level {field}"));
        }
    }
    let entries = manifest
        .get("queries")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut listed = BTreeSet::new();
    for entry in &entries {
        let Some(name) = entry.get("name").and_then(toml::Value::as_str) else {
            failures.push("MANIFEST.toml: a query entry has no name".to_owned());
            continue;
        };
        let file = format!("{name}/highlights.scm");
        if !listed.insert(name.to_owned()) {
            failures.push(format!("{file}: listed twice"));
        }
        let overrides = SOURCE_FIELDS
            .iter()
            .filter(|field| entry.get(**field).is_some())
            .count();
        if overrides != 0 && overrides != SOURCE_FIELDS.len() {
            failures.push(format!(
                "{file}: an entry overrides all of {SOURCE_FIELDS:?} or none"
            ));
        }
        if entry.get("source").and_then(toml::Value::as_str).is_none() {
            failures.push(format!("{file}: no source path"));
        }
        let Some(recorded) = entry.get("sha256").and_then(toml::Value::as_str) else {
            failures.push(format!("{file}: no sha256"));
            continue;
        };
        match std::fs::read(root.join(name).join("highlights.scm")) {
            Ok(bytes) => {
                let actual: String = Sha256::digest(&bytes)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                if actual != recorded {
                    failures.push(format!("{file}: sha256 {actual}, manifest says {recorded}"));
                }
            }
            Err(error) => failures.push(format!("{file}: {error}")),
        }
    }
    match std::fs::read_dir(root) {
        Ok(children) => {
            let mut unlisted: Vec<_> = children
                .filter_map(Result::ok)
                .filter(|child| child.path().join("highlights.scm").is_file())
                .filter_map(|child| child.file_name().into_string().ok())
                .filter(|name| !listed.contains(name))
                .collect();
            unlisted.sort();
            for name in unlisted {
                failures.push(format!("{name}/highlights.scm: absent from MANIFEST.toml"));
            }
        }
        Err(error) => failures.push(format!("{}: {error}", root.display())),
    }
    failures
}

fn copy_of_the_queries() -> tempfile::TempDir {
    let copy = tempfile::tempdir().unwrap();
    let root = queries_root();
    std::fs::copy(
        root.join("MANIFEST.toml"),
        copy.path().join("MANIFEST.toml"),
    )
    .unwrap();
    for child in std::fs::read_dir(&root).unwrap() {
        let child = child.unwrap();
        let query = child.path().join("highlights.scm");
        if query.is_file() {
            let target = copy.path().join(child.file_name());
            std::fs::create_dir(&target).unwrap();
            std::fs::copy(query, target.join("highlights.scm")).unwrap();
        }
    }
    copy
}

fn listed_query_names(root: &Path) -> Vec<String> {
    let manifest: toml::Table = std::fs::read_to_string(root.join("MANIFEST.toml"))
        .unwrap()
        .parse()
        .unwrap();
    manifest["queries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn every_vendored_query_matches_its_manifest_hash() {
    assert_eq!(manifest_failures(&queries_root()), Vec::<String>::new());
    let names = listed_query_names(&queries_root());
    assert_eq!(names.len(), 16, "{names:?}");
    assert!(names.iter().any(|name| name == "zig"));
}

#[test]
fn the_zig_entry_carries_its_own_source_and_the_zed_fields_stay() {
    let manifest: toml::Table = std::fs::read_to_string(queries_root().join("MANIFEST.toml"))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(
        manifest["repository"].as_str(),
        Some("https://github.com/zed-industries/zed")
    );
    assert_eq!(
        manifest["commit"].as_str(),
        Some("ee3b5558c581429633937e458fad8d109f29e9ee")
    );
    assert_eq!(manifest["license"].as_str(), Some("GPL-3.0-or-later"));
    let entries = manifest["queries"].as_array().unwrap();
    let zig = entries
        .iter()
        .find(|entry| entry["name"].as_str() == Some("zig"))
        .unwrap();
    for (field, value) in [
        ("repository", "https://github.com/zed-extensions/zig"),
        ("commit", "996432e66cb5bb2eed5a30ad2afaaa90349a3817"),
        ("source", "languages/zig/highlights.scm"),
        (
            "sha256",
            "8f7f115b4a48a3336609ee8a087e634290131816d18d00fc11ef840be880df8e",
        ),
        ("license", "Apache-2.0"),
    ] {
        assert_eq!(zig[field].as_str(), Some(value), "{field}");
    }
    for entry in entries
        .iter()
        .filter(|entry| entry["name"].as_str() != Some("zig"))
    {
        for field in SOURCE_FIELDS {
            assert!(entry.get(field).is_none(), "{entry:?}");
        }
    }
    let license = std::fs::read_to_string(queries_root().join("zig/LICENSE")).unwrap();
    assert!(license.contains("Apache License") && license.contains("Version 2.0"));
    let notice = std::fs::read_to_string(queries_root().join("NOTICE")).unwrap();
    assert!(notice.contains("https://github.com/zed-extensions/zig"));
    assert!(notice.contains("Apache-2.0"));
}

#[test]
fn one_changed_byte_in_any_vendored_query_names_that_file() {
    let copy = copy_of_the_queries();
    assert_eq!(manifest_failures(copy.path()), Vec::<String>::new());
    for name in listed_query_names(copy.path()) {
        let path = copy.path().join(&name).join("highlights.scm");
        let original = std::fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        std::fs::write(&path, changed).unwrap();
        let failures = manifest_failures(copy.path());
        assert_eq!(failures.len(), 1, "{name}: {failures:?}");
        assert!(
            failures[0].starts_with(&format!("{name}/highlights.scm: sha256 ")),
            "{failures:?}"
        );
        std::fs::write(&path, original).unwrap();
    }
}

#[test]
fn a_query_absent_from_the_manifest_is_named() {
    let copy = copy_of_the_queries();
    std::fs::create_dir(copy.path().join("kotlin")).unwrap();
    std::fs::write(
        copy.path().join("kotlin/highlights.scm"),
        "(comment) @comment\n",
    )
    .unwrap();
    assert_eq!(
        manifest_failures(copy.path()),
        vec!["kotlin/highlights.scm: absent from MANIFEST.toml".to_owned()]
    );
}

#[test]
fn a_listed_query_missing_on_disk_is_named() {
    let copy = copy_of_the_queries();
    std::fs::remove_file(copy.path().join("zig/highlights.scm")).unwrap();
    let failures = manifest_failures(copy.path());
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].starts_with("zig/highlights.scm: "),
        "{failures:?}"
    );
}
