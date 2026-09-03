use gpui::{Hsla, Rgba};

use crate::terminal::types::{Color, NamedColor};
use crate::theme::TerminalTheme;

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

pub(crate) fn ensure_minimum_contrast(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    if min_lc <= 0.0 {
        return fg;
    }
    contrast_cache_get_or_insert(fg, bg, min_lc)
}

const CONTRAST_CACHE_SLOTS: usize = 128;

#[derive(Clone, Copy)]
struct ContrastEntry {
    key: [u32; 9],
    value: Hsla,
}

thread_local! {
    static CONTRAST_CACHE: std::cell::RefCell<[Option<ContrastEntry>; CONTRAST_CACHE_SLOTS]> =
        const { std::cell::RefCell::new([None; CONTRAST_CACHE_SLOTS]) };
}

fn contrast_key(fg: Hsla, bg: Hsla, min_lc: f32) -> [u32; 9] {
    [
        fg.h.to_bits(),
        fg.s.to_bits(),
        fg.l.to_bits(),
        fg.a.to_bits(),
        bg.h.to_bits(),
        bg.s.to_bits(),
        bg.l.to_bits(),
        bg.a.to_bits(),
        min_lc.to_bits(),
    ]
}

fn contrast_slot(key: &[u32; 9]) -> usize {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for word in key {
        hash ^= u64::from(*word);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash as usize) % CONTRAST_CACHE_SLOTS
}

fn contrast_cache_get_or_insert(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    let key = contrast_key(fg, bg, min_lc);
    let slot = contrast_slot(&key);
    CONTRAST_CACHE.with(|cache| {
        if let Ok(cache) = cache.try_borrow()
            && let Some(entry) = cache[slot].as_ref()
            && entry.key == key
        {
            return entry.value;
        }
        let value = compute_minimum_contrast(fg, bg, min_lc);
        if let Ok(mut cache) = cache.try_borrow_mut() {
            cache[slot] = Some(ContrastEntry { key, value });
        }
        value
    })
}

fn compute_minimum_contrast(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    if apca_contrast(fg, bg).abs() >= min_lc {
        return fg;
    }

    let adjusted = adjust_lightness_for_apca(fg, bg, min_lc);
    if apca_contrast(adjusted, bg).abs() >= min_lc {
        return adjusted;
    }

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

pub(super) fn convert_color(color: Color, theme: &TerminalTheme) -> Hsla {
    match color {
        Color::Named(name) => named_color(name, theme),
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

fn indexed_color(i: u8, theme: &TerminalTheme) -> Hsla {
    if i < 16 {
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
        let idx = i - 16;
        let r_idx = idx / 36;
        let g_idx = (idx % 36) / 6;
        let b_idx = idx % 6;
        let r = if r_idx == 0 { 0 } else { 55 + 40 * r_idx };
        let g = if g_idx == 0 { 0 } else { 55 + 40 * g_idx };
        let b = if b_idx == 0 { 0 } else { 55 + 40 * b_idx };
        return rgb_to_hsla(r, g, b);
    }

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
    fn the_contrast_cache_never_changes_the_answer() {
        let min_lc = 45.0;
        let mut pairs = Vec::new();
        for step in 0..(CONTRAST_CACHE_SLOTS * 3) {
            let t = step as f32 / (CONTRAST_CACHE_SLOTS * 3) as f32;
            let fg = Hsla {
                h: t,
                s: 0.6,
                l: 0.5 + t * 0.4,
                a: 1.0,
            };
            let bg = Hsla {
                h: 1.0 - t,
                s: 0.3,
                l: 0.5 - t * 0.4,
                a: 1.0,
            };
            pairs.push((fg, bg));
        }

        for (fg, bg) in &pairs {
            let cached = ensure_minimum_contrast(*fg, *bg, min_lc);
            let direct = compute_minimum_contrast(*fg, *bg, min_lc);
            assert_eq!(
                (cached.h, cached.s, cached.l, cached.a),
                (direct.h, direct.s, direct.l, direct.a),
                "cache diverged for fg={fg:?} bg={bg:?}"
            );
        }
        for (fg, bg) in &pairs {
            let cached = ensure_minimum_contrast(*fg, *bg, min_lc);
            let direct = compute_minimum_contrast(*fg, *bg, min_lc);
            assert_eq!(
                (cached.h, cached.s, cached.l),
                (direct.h, direct.s, direct.l)
            );
        }
    }

    #[test]
    fn the_contrast_cache_keys_on_the_threshold_too() {
        let fg = Hsla {
            h: 0.1,
            s: 0.5,
            l: 0.52,
            a: 1.0,
        };
        let bg = Hsla {
            h: 0.1,
            s: 0.5,
            l: 0.48,
            a: 1.0,
        };
        let lenient = ensure_minimum_contrast(fg, bg, 15.0);
        let strict = ensure_minimum_contrast(fg, bg, 75.0);
        assert_ne!(
            (lenient.l, lenient.s),
            (strict.l, strict.s),
            "a stricter threshold must move the foreground further"
        );
        assert_eq!(strict.l, compute_minimum_contrast(fg, bg, 75.0).l);
    }

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
