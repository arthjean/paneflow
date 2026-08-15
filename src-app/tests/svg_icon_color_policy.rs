#![allow(
    clippy::panic,
    reason = "integration test setup failures need contextual diagnostics"
)]

//! Every `svg()` icon must set its own `text_color`.
//!
//! GPUI's `svg()` rasterizes to a monochrome alpha mask painted in the colour
//! taken from that element's *own* style. It does not inherit the parent's
//! text colour the way a text glyph does, so an `svg()` with no `text_color`
//! of its own paints nothing at all.
//!
//! That failure is invisible to every other kind of test, and nearly invisible
//! in review: the button still lays out, still hit-tests, still shows its
//! tooltip and still runs its click handler. Only the glyph is missing, which
//! is exactly how the sidebar's delete icon and the text fields' clear icon
//! both shipped as blank squares - a button users could not see but could
//! still press.
//!
//! Enforcing it here rather than by convention keeps the whole class from
//! coming back, since nothing else in the suite can observe a painted pixel.

use std::path::{Path, PathBuf};

/// Walk `dir` and return every `.rs` file under it.
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

/// The builder chain that starts at `start` in `src`, ending at the statement
/// or argument terminator. Good enough to tell "this `svg()` sets a colour"
/// from "this one does not": both `.path(..)` and `.text_color(..)` sit on the
/// same chain, and a chain never spans a `;`.
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
            // Only chains that actually name an asset paint an icon; a bare
            // `svg()` handed to a helper is coloured by that helper.
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
