//! Color conversion + APCA contrast enforcement for the terminal renderer.
//!
//! Two responsibilities, related only because both translate terminal colors
//! into final `Hsla` values for painting:
//!
//! 1. **APCA contrast** (`ensure_minimum_contrast`): fixes foreground/background
//!    pairs that fail the APCA (Accessible Perceptual Contrast Algorithm) Lc
//!    threshold. Polarity-aware and perceptually uniform - more accurate than
//!    WCAG 2.0 on dark backgrounds. Matches Zed's algorithm.
//! 2. **Color resolution** (`convert_color`, `named_color`, `indexed_color`):
//!    translates the neutral [`crate::terminal::types::Color`] (Named / Spec /
//!    Indexed) into themed `Hsla`, covering the xterm-256color palette.
//!
//! Extracted from `terminal_element.rs` per US-009 of the src-app refactor PRD.

use gpui::{Hsla, Rgba};

use crate::terminal::types::{Color, NamedColor};
use crate::theme::TerminalTheme;

// ---------------------------------------------------------------------------
// Minimum contrast (APCA - Accessible Perceptual Contrast Algorithm)
// ---------------------------------------------------------------------------

/// APCA constants (0.0.98G-4g W3 compatible).
/// https://github.com/Myndex/apca-w3
struct ApcaConstants {
    main_trc: f32,
    s_rco: f32,
    s_gco: f32,
    s_bco: f32,
    norm_bg: f32,
    norm_txt: f32,
    rev_txt: f32,
    rev_bg: f32,
    blk_thrs: f32,
    blk_clmp: f32,
    scale_bow: f32,
    scale_wob: f32,
    lo_bow_offset: f32,
    lo_wob_offset: f32,
    delta_y_min: f32,
    lo_clip: f32,
}

const APCA: ApcaConstants = ApcaConstants {
    main_trc: 2.4,
    s_rco: 0.2126729,
    s_gco: 0.7151522,
    s_bco: 0.0721750,
    norm_bg: 0.56,
    norm_txt: 0.57,
    rev_txt: 0.62,
    rev_bg: 0.65,
    blk_thrs: 0.022,
    blk_clmp: 1.414,
    scale_bow: 1.14,
    scale_wob: 1.14,
    lo_bow_offset: 0.027,
    lo_wob_offset: 0.027,
    delta_y_min: 0.0005,
    lo_clip: 0.1,
};

fn srgb_to_y(color: Hsla) -> f32 {
    let rgba = Rgba::from(color);
    let r_linear = rgba.r.powf(APCA.main_trc);
    let g_linear = rgba.g.powf(APCA.main_trc);
    let b_linear = rgba.b.powf(APCA.main_trc);
    APCA.s_rco * r_linear + APCA.s_gco * g_linear + APCA.s_bco * b_linear
}

/// APCA Lightness Contrast (`Lc`) between `text` foreground and `bg`. Sign
/// indicates polarity (positive = light text on dark bg, negative = dark
/// text on light bg). Tests and theme-load code assert on `.abs() >= 45.0`.
///
/// Visibility: `pub(crate)` (was private pre-US-007) so theme tests can
/// verify the `selection_foreground` invariant directly.
pub(crate) fn apca_contrast(text: Hsla, bg: Hsla) -> f32 {
    let text_y = srgb_to_y(text);
    let bg_y = srgb_to_y(bg);

    let text_y = if text_y > APCA.blk_thrs {
        text_y
    } else {
        text_y + (APCA.blk_thrs - text_y).powf(APCA.blk_clmp)
    };
    let bg_y = if bg_y > APCA.blk_thrs {
        bg_y
    } else {
        bg_y + (APCA.blk_thrs - bg_y).powf(APCA.blk_clmp)
    };

    if (bg_y - text_y).abs() < APCA.delta_y_min {
        return 0.0;
    }

    let (sapc, offset) = if bg_y > text_y {
        let s = (bg_y.powf(APCA.norm_bg) - text_y.powf(APCA.norm_txt)) * APCA.scale_bow;
        (s, APCA.lo_bow_offset)
    } else {
        let s = (bg_y.powf(APCA.rev_bg) - text_y.powf(APCA.rev_txt)) * APCA.scale_wob;
        (s, -APCA.lo_wob_offset)
    };

    if sapc.abs() < APCA.lo_clip {
        0.0
    } else {
        (sapc - offset) * 100.0
    }
}

/// Adjust `fg` lightness using APCA so that perceptual contrast against `bg`
/// meets `min_lc`. Returns `fg` unchanged if contrast is already sufficient.
///
/// Three-stage fallback matching Zed's approach:
/// 1. Adjust lightness only (preserves hue + saturation)
/// 2. Reduce saturation + adjust lightness
/// 3. Fall back to black or white
///
/// Visibility: `pub(crate)` (was `pub(super)` pre-US-007) so theme code can
/// derive a contrast-validated `selection_foreground` at theme-load time.
pub(crate) fn ensure_minimum_contrast(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    if min_lc <= 0.0 {
        return fg;
    }

    if apca_contrast(fg, bg).abs() >= min_lc {
        return fg;
    }

    // Stage 1: adjust lightness only
    let adjusted = adjust_lightness_for_apca(fg, bg, min_lc);
    if apca_contrast(adjusted, bg).abs() >= min_lc {
        return adjusted;
    }

    // Stage 2: reduce saturation + adjust lightness
    for &sat_mult in &[0.8, 0.6, 0.4, 0.2, 0.0] {
        let desat = Hsla {
            s: fg.s * sat_mult,
            ..fg
        };
        let adjusted = adjust_lightness_for_apca(desat, bg, min_lc);
        if apca_contrast(adjusted, bg).abs() >= min_lc {
            return adjusted;
        }
    }

    // Stage 3: black or white
    let black = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: fg.a,
    };
    let white = Hsla {
        h: 0.0,
        s: 0.0,
        l: 1.0,
        a: fg.a,
    };
    if apca_contrast(white, bg).abs() > apca_contrast(black, bg).abs() {
        white
    } else {
        black
    }
}

fn adjust_lightness_for_apca(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    let bg_lum = srgb_to_y(bg);
    let should_darken = bg_lum > 0.5;

    let (mut lo, mut hi) = if should_darken {
        (0.0, fg.l)
    } else {
        (fg.l, 1.0)
    };
    let mut best_l = fg.l;

    for _ in 0..20 {
        let mid = (lo + hi) * 0.5;
        let test = Hsla { l: mid, ..fg };
        let contrast = apca_contrast(test, bg).abs();

        if contrast >= min_lc {
            best_l = mid;
            if should_darken {
                lo = mid;
            } else {
                hi = mid;
            }
        } else if should_darken {
            hi = mid;
        } else {
            lo = mid;
        }

        if (contrast - min_lc).abs() < 1.0 {
            best_l = mid;
            break;
        }
    }

    Hsla { l: best_l, ..fg }
}

// ---------------------------------------------------------------------------
// Color conversion
// ---------------------------------------------------------------------------

pub(super) fn convert_color(color: Color, theme: &TerminalTheme) -> Hsla {
    match color {
        Color::Named(name) => named_color(name, theme),
        // Truecolor RGB values are kept as-is. A previous special case mapped
        // `Spec(0,0,0)` to `theme.black`, but that silently hijacked apps that
        // chose a literal `#000000` (intentional pure black) and replaced it
        // with the slightly-lighter ANSI "black" slot from the theme.
        Color::Spec(rgb) => rgb_to_hsla(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => indexed_color(i, theme),
    }
}

fn named_color(name: NamedColor, theme: &TerminalTheme) -> Hsla {
    match name {
        NamedColor::Black => theme.black,
        NamedColor::Red => theme.red,
        NamedColor::Green => theme.green,
        NamedColor::Yellow => theme.yellow,
        NamedColor::Blue => theme.blue,
        NamedColor::Magenta => theme.magenta,
        NamedColor::Cyan => theme.cyan,
        NamedColor::White => theme.white,
        NamedColor::BrightBlack => theme.bright_black,
        NamedColor::BrightRed => theme.bright_red,
        NamedColor::BrightGreen => theme.bright_green,
        NamedColor::BrightYellow => theme.bright_yellow,
        NamedColor::BrightBlue => theme.bright_blue,
        NamedColor::BrightMagenta => theme.bright_magenta,
        NamedColor::BrightCyan => theme.bright_cyan,
        NamedColor::BrightWhite => theme.bright_white,
        NamedColor::Foreground => theme.foreground,
        NamedColor::Background => theme.ansi_background,
    }
}

/// Convert the xterm-256color indexed palette to HSLA.
fn indexed_color(i: u8, theme: &TerminalTheme) -> Hsla {
    if i < 16 {
        // Standard 16 colors - map to named
        return named_color(
            match i {
                0 => NamedColor::Black,
                1 => NamedColor::Red,
                2 => NamedColor::Green,
                3 => NamedColor::Yellow,
                4 => NamedColor::Blue,
                5 => NamedColor::Magenta,
                6 => NamedColor::Cyan,
                7 => NamedColor::White,
                8 => NamedColor::BrightBlack,
                9 => NamedColor::BrightRed,
                10 => NamedColor::BrightGreen,
                11 => NamedColor::BrightYellow,
                12 => NamedColor::BrightBlue,
                13 => NamedColor::BrightMagenta,
                14 => NamedColor::BrightCyan,
                15 => NamedColor::BrightWhite,
                _ => unreachable!(),
            },
            theme,
        );
    }

    if i < 232 {
        // 6x6x6 color cube (indices 16-231)
        let idx = i - 16;
        let r_idx = idx / 36;
        let g_idx = (idx % 36) / 6;
        let b_idx = idx % 6;
        let r = if r_idx == 0 { 0 } else { 55 + 40 * r_idx };
        let g = if g_idx == 0 { 0 } else { 55 + 40 * g_idx };
        let b = if b_idx == 0 { 0 } else { 55 + 40 * b_idx };
        return rgb_to_hsla(r, g, b);
    }

    // Grayscale ramp (indices 232-255)
    let gray = 8 + 10 * (i - 232);
    rgb_to_hsla(gray, gray, gray)
}

pub(super) fn rgb_to_hsla(r: u8, g: u8, b: u8) -> Hsla {
    Hsla::from(Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ground_colors_use_the_terminal_theme_slots() {
        let theme = crate::theme::paneflow_dark();

        assert_eq!(
            convert_color(Color::Named(NamedColor::Foreground), &theme),
            theme.foreground
        );
        assert_eq!(
            convert_color(Color::Named(NamedColor::Background), &theme),
            theme.ansi_background
        );
    }
}
