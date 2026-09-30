#![allow(
    clippy::panic,
    reason = "integration test setup failures need contextual diagnostics"
)]

use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        panic!("failed to read source dir {}", dir.display());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    out
}

fn builder_chain(src: &str, start: usize) -> &str {
    let rest = &src[start..];
    let mut depth = 0usize;
    for (index, ch) in rest.char_indices() {
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return &rest[..index],
            ')' | ']' | '}' => depth -= 1,
            ',' | ';' if depth == 0 => return &rest[..index],
            _ => {}
        }
    }
    rest
}

#[test]
fn the_chain_stops_at_the_end_of_the_svg_expression() {
    let src = "div().child(svg().path(\"a.svg\").size(px(12.))).child(div().text_color(ink))";
    let start = src
        .find("svg()")
        .unwrap_or_else(|| panic!("fixture has an svg"));
    let chain = builder_chain(src, start);
    assert_eq!(chain, "svg().path(\"a.svg\").size(px(12.))");
    assert!(!chain.contains(".text_color("));

    let src = "let icon = svg()\n    .path(p)\n    .when(on, |s| s.text_color(x));";
    let start = src
        .find("svg()")
        .unwrap_or_else(|| panic!("fixture has an svg"));
    assert!(builder_chain(src, start).contains(".text_color("));
}

#[test]
fn every_svg_icon_sets_its_own_text_color() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut checked = 0usize;

    for file in rust_sources(&src_dir) {
        let Ok(source) = std::fs::read_to_string(&file) else {
            panic!("failed to read {}", file.display());
        };
        for (offset, _) in source.match_indices("svg()") {
            let chain = builder_chain(&source, offset);
            if !chain.contains(".path(") {
                continue;
            }
            checked += 1;
            if chain.contains(".text_color(") {
                continue;
            }
            let line = source[..offset].matches('\n').count() + 1;
            offenders.push(format!("{}:{line}", file.display()));
        }
    }

    assert!(
        checked > 0,
        "found no `svg().path(..)` call sites at all - the scan is broken, \
         not the code"
    );
    assert!(
        offenders.is_empty(),
        "these `svg()` icons set no `text_color` and will paint as blank \
         space:\n  {}\n\nGPUI paints an svg mask in its own style's colour and \
         never inherits the parent's. Set `.text_color(..)` on the `svg()` \
         itself - see the delete button in `agents_sidebar/mod.rs` for the \
         hover-animated form.",
        offenders.join("\n  ")
    );
}
