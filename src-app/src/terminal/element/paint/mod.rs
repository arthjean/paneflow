//! Paint sub-passes for `TerminalElement`.
//!
//! Each sub-module owns a specific visual layer. `paint()` in
//! `terminal/element/mod.rs` orchestrates them in the fixed order:
//!
//! 1. `background`  - terminal background, per-cell bg rects, block quads
//! 2. `selection`   - selection highlight rects
//! 3. `overlay::search_highlights` - search match rects
//! 4. `text`        - batched `shape_line` glyph runs
//! 5. `overlay::hyperlink` - Ctrl+hover underline
//! 6. `cursor`      - primary cursor + copy-mode anchor cursor
//! 7. `scrollbar`   - right-edge thumb
//! 8. `overlay::ime` - IME handler registration + preedit overlay
//! 9. `overlay::exit` - process-exited centered message
//!
//! Every function here is a `pub fn` inside a `pub(super)` module - the
//! parent module boundary gates access to `element`, and every function
//! takes explicit args (no hidden state).
//!
//! Extracted from `terminal_element.rs` per US-015 of the src-app refactor PRD.

use gpui::{Font, FontWeight};

pub(super) mod background;
pub(super) mod cursor;
pub(super) mod overlay;
pub(super) mod scrollbar;
pub(super) mod selection;
pub(super) mod text;

/// Convert terminal intensity into a restrained display weight.
///
/// SGR 1 means increased intensity, but assigning an absolute 700 weight makes
/// large colored surfaces such as fastfetch appear much heavier than Windows
/// Terminal. Advance one weight step from the configured base instead. Layout
/// state keeps the original bold attribute for batching and snapshots; only
/// glyph shaping receives the optical adjustment.
fn display_font_for_intensity(font: &Font, base_weight: FontWeight) -> Font {
    let mut display_font = font.clone();
    if font.weight == FontWeight::BOLD {
        display_font.weight = if base_weight.0 >= FontWeight::BOLD.0 {
            base_weight
        } else {
            FontWeight((base_weight.0 + 100.0).min(FontWeight::BOLD.0))
        };
    }
    display_font
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontFeatures, FontStyle};

    fn font(weight: FontWeight) -> Font {
        Font {
            family: "test-mono".into(),
            features: FontFeatures::default(),
            fallbacks: None,
            weight,
            style: FontStyle::Normal,
        }
    }

    #[test]
    fn ansi_bold_advances_one_weight_step_from_normal() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::NORMAL);
        assert_eq!(display.weight, FontWeight::MEDIUM);
    }

    #[test]
    fn regular_runs_keep_their_configured_weight() {
        let display = display_font_for_intensity(&font(FontWeight::NORMAL), FontWeight::NORMAL);
        assert_eq!(display.weight, FontWeight::NORMAL);
    }

    #[test]
    fn heavy_base_weight_is_never_reduced() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::EXTRA_BOLD);
        assert_eq!(display.weight, FontWeight::EXTRA_BOLD);
    }
}
