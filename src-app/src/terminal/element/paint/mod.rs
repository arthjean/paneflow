//! Paint sub-passes for `TerminalElement`.
//!
//! Each sub-module owns a specific visual layer. `paint()` in
//! `terminal/element/mod.rs` orchestrates them in the fixed order:
//!
//! 1. `background`  - terminal background, per-cell bg rects, block quads
//! 2. `selection`   - selection highlight rects
//! 3. `overlay::search_highlights` - search match rects
//! 4. `sprites`     - box drawing, shades, braille, Powerline on the device grid
//! 5. `kitty`      - graphics placements with a negative z-index
//! 6. `decorations` - underlines and strikethroughs from the cell metrics
//! 7. `text`        - batched `shape_line` glyph runs, then constrained icons
//! 8. `kitty`      - graphics placements with a zero or positive z-index
//! 9. `overlay::hyperlink` - Ctrl+hover underline
//! 10. `cursor`      - primary cursor + copy-mode anchor cursor
//! 11. `scrollbar`   - right-edge thumb
//! 12. `overlay::ime` - IME handler registration + preedit overlay
//! 13. `overlay::exit` - process-exited centered message
//!
//! Every function here is a `pub fn` inside a `pub(super)` module - the
//! parent module boundary gates access to `element`, and every function
//! takes explicit args (no hidden state).
//!
//! Extracted from `terminal_element.rs` per US-015 of the src-app refactor PRD.

use gpui::{Font, FontWeight};

pub(super) mod background;
pub(super) mod cursor;
pub(super) mod decorations;
pub(super) mod kitty;
pub(super) mod overlay;
pub(super) mod scrollbar;
pub(super) mod selection;
pub(super) mod sprites;
pub(super) mod text;

/// Convert terminal intensity into a distinct display weight.
///
/// SGR 1 must remain visibly distinct from regular terminal text. A single
/// 100-point step turns the default 400 face into Medium 500, which is too
/// subtle at terminal sizes. Keep at least a 200-point separation, use the
/// bundled SemiBold face as the floor, and never reduce an already-heavy base.
fn display_font_for_intensity(font: &Font, base_weight: FontWeight) -> Font {
    let mut display_font = font.clone();
    if font.weight == FontWeight::BOLD {
        display_font.weight = if base_weight.0 >= FontWeight::BOLD.0 {
            base_weight
        } else {
            FontWeight((base_weight.0 + 200.0).clamp(FontWeight::SEMIBOLD.0, FontWeight::BOLD.0))
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
    fn ansi_bold_uses_semibold_from_normal() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::NORMAL);
        assert_eq!(display.weight, FontWeight::SEMIBOLD);
    }

    #[test]
    fn ansi_bold_uses_bold_from_medium() {
        let display = display_font_for_intensity(&font(FontWeight::BOLD), FontWeight::MEDIUM);
        assert_eq!(display.weight, FontWeight::BOLD);
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
