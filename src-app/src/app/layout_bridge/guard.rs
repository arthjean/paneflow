use std::path::{Path, PathBuf};

const PROJECTION_MODULE: &str = "app/layout_bridge/";

fn rust_sources(root: &Path) -> Vec<(String, String)> {
    let mut sources = Vec::new();
    let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            sources.push((rel, std::fs::read_to_string(&path).unwrap()));
        }
    }
    sources
}

fn layout_writes(text: &str) -> Vec<usize> {
    let patterns = [
        r"\.(root|saved_layout)\s*=[^=]",
        r"\.(root|saved_layout)\s*\.\s*(take|replace|insert|get_or_insert|get_or_insert_with|as_mut|as_deref_mut)\s*\(",
        r"&mut\s+[A-Za-z_][A-Za-z0-9_.()\[\]]*\.(root|saved_layout)\b",
        r"mem::(take|replace|swap)\s*\([^)]*\.(root|saved_layout)\b",
    ];
    let patterns: Vec<regex::Regex> = patterns
        .iter()
        .map(|pattern| regex::Regex::new(pattern).unwrap())
        .collect();
    text.lines()
        .enumerate()
        .filter(|(_, line)| patterns.iter().any(|pattern| pattern.is_match(line)))
        .map(|(idx, _)| idx + 1)
        .collect()
}

fn function_bodies(text: &str) -> Vec<(usize, &str)> {
    let signature = regex::Regex::new(r"\bfn\s+[a-z_0-9]+\s*(<[^>]*>)?\s*\(").unwrap();
    let mut bodies = Vec::new();
    for found in signature.find_iter(text) {
        let rest = &text[found.end()..];
        let Some(open) = rest.find('{') else {
            continue;
        };
        if rest.find(';').is_some_and(|semi| semi < open) {
            continue;
        }
        let start = found.end() + open;
        let mut depth = 0usize;
        let mut end = text.len();
        for (offset, ch) in text[start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let line = text[..found.start()].matches('\n').count() + 1;
        bodies.push((line, &text[start..end]));
    }
    bodies
}

fn saves_after_an_op(text: &str) -> Vec<usize> {
    let op = regex::Regex::new(r"_through_model\s*\(|apply_layout_op\s*\(").unwrap();
    function_bodies(text)
        .into_iter()
        .filter(|(_, body)| body.contains("save_session(") && op.is_match(body))
        .map(|(line, _)| line)
        .collect()
}

#[test]
fn a_structural_change_is_saved_only_by_the_model_revision() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let violations: Vec<String> = rust_sources(&root)
        .into_iter()
        .filter(|(rel, _)| !rel.starts_with(PROJECTION_MODULE))
        .flat_map(|(rel, text)| {
            saves_after_an_op(&text)
                .into_iter()
                .map(move |line| format!("{rel}:{line}"))
        })
        .collect();
    assert!(
        violations.is_empty(),
        "a function applies a layout op and also calls save_session: {violations:?}"
    );
    let sample = "fn close(&mut self) {\n    self.close_pane_through_model(&pane, cx);\n    self.save_session(cx);\n}\nfn other(&mut self) {\n    self.save_session(cx);\n}\n";
    assert_eq!(saves_after_an_op(sample), [1]);
}

#[test]
fn only_the_projection_module_writes_a_tab_layout() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let violations: Vec<String> = rust_sources(&root)
        .into_iter()
        .filter(|(rel, _)| !rel.starts_with(PROJECTION_MODULE))
        .flat_map(|(rel, text)| {
            layout_writes(&text)
                .into_iter()
                .map(move |line| format!("{rel}:{line}"))
        })
        .collect();
    assert!(
        violations.is_empty(),
        "a tab's root or saved_layout is written outside {PROJECTION_MODULE}: {violations:?}"
    );
}

#[test]
fn the_guard_catches_every_form_of_layout_write() {
    for line in [
        "tab.root = Some(tree);",
        "ws.active_tab_mut().saved_layout = None;",
        "let tree = tab.root.take();",
        "tab.saved_layout.replace(full);",
        "if let Some(root) = tab.root.as_mut() {",
        "let slot = &mut tab.root;",
        "std::mem::swap(&mut a, &mut tab.saved_layout);",
    ] {
        assert_eq!(layout_writes(line), [1], "{line}");
    }
    for line in [
        "let root = tab.root.as_ref();",
        "if tab.root == other {",
        "root.insert(key, value);",
        "let saved = tab.saved_layout.is_some();",
    ] {
        assert!(layout_writes(line).is_empty(), "{line}");
    }
}
