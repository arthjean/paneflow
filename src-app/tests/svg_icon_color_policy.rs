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
    let end = rest.find(';').unwrap_or(rest.len());
    &rest[..end]
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
