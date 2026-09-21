use gpui::{Hsla, Rgba};

use super::oklab::{Oklab, Oklch, hsla_to_oklab, hsla_to_oklch, oklch_to_hsla_in_gamut};
use crate::terminal::types::{Color, NamedColor};
use crate::theme::{TerminalTheme, ThemePalette};

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

pub(crate) fn ensure_minimum_contrast(
    fg: Hsla,
    bg: Hsla,
    min_lc: f32,
    harmony: Option<&HarmonyTargets>,
) -> Hsla {
    #[cfg(test)]
    CORRECTION_CALLS.with(|calls| calls.set(calls.get() + 1));
    if min_lc <= 0.0 {
        return fg;
    }
    let key = ContrastKey::new(fg, bg);
    let threshold = ContrastThreshold::quantized(min_lc);
    let corrected = contrast_cache_get_or_insert(key, threshold, harmony);
    Hsla {
        a: fg.a,
        ..corrected
    }
}

#[cfg(test)]
pub(super) fn oklab_of(color: Hsla) -> Oklab {
    hsla_to_oklab(color)
}

#[cfg(test)]
pub(super) fn oklch_of(color: Hsla) -> Oklch {
    hsla_to_oklch(color)
}

#[cfg(test)]
pub(super) fn corrected_without_harmony(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    compute_minimum_contrast(fg, bg, min_lc, None)
}

#[cfg(test)]
fn uncached_minimum_contrast(fg: Hsla, bg: Hsla, min_lc: f32) -> Hsla {
    let key = ContrastKey::new(fg, bg);
    compute_minimum_contrast(
        key.foreground(),
        key.background(),
        ContrastThreshold::quantized(min_lc).lc(),
        None,
    )
}

#[cfg(test)]
pub(super) fn nearest_theme_color(theme: &TerminalTheme, color: Hsla) -> Hsla {
    HarmonyTargets::from_theme(theme).nearest(hsla_to_oklab(color))
}

const HARMONY_TARGET_COUNT: usize = 18;

pub(crate) struct HarmonyTargets {
    id: u64,
    targets: [(Hsla, Oklab); HARMONY_TARGET_COUNT],
}

impl HarmonyTargets {
    #[must_use]
    pub(crate) fn from_theme(theme: &TerminalTheme) -> Self {
        let colors = [
            theme.foreground,
            theme.dim_foreground,
            theme.black,
            theme.red,
            theme.green,
            theme.yellow,
            theme.blue,
            theme.magenta,
            theme.cyan,
            theme.white,
            theme.bright_black,
            theme.bright_red,
            theme.bright_green,
            theme.bright_yellow,
            theme.bright_blue,
            theme.bright_magenta,
            theme.bright_cyan,
            theme.bright_white,
        ];
        let mut id = 0xcbf2_9ce4_8422_2325_u64;
        let mut targets = [(colors[0], hsla_to_oklab(colors[0])); HARMONY_TARGET_COUNT];
        for (slot, color) in colors.into_iter().enumerate() {
            id ^= u64::from(packed_srgb(color));
            id = id.wrapping_mul(0x0000_0100_0000_01b3);
            targets[slot] = (color, hsla_to_oklab(color));
        }
        Self { id, targets }
    }

    fn nearest(&self, lab: Oklab) -> Hsla {
        let mut nearest = self.targets[0].0;
        let mut shortest = f32::INFINITY;
        for (color, target) in &self.targets {
            let distance = lab.distance(*target);
            if distance < shortest {
                shortest = distance;
                nearest = *color;
            }
        }
        nearest
    }
}

#[cfg(test)]
thread_local! {
    static CORRECTION_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_correction_calls() -> usize {
    CORRECTION_CALLS.with(|calls| calls.replace(0))
}

fn packed_srgb(color: Hsla) -> u32 {
    let rgba = Rgba::from(color);
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u32;
    (channel(rgba.r) << 16) | (channel(rgba.g) << 8) | channel(rgba.b)
}

fn unpacked_srgb(packed: u32) -> Hsla {
    rgb_to_hsla(
        ((packed >> 16) & 0xff) as u8,
        ((packed >> 8) & 0xff) as u8,
        (packed & 0xff) as u8,
    )
}

pub(super) fn is_same_visible_color(a: Hsla, b: Hsla) -> bool {
    packed_srgb(a) == packed_srgb(b)
}

const CONTRAST_CACHE_SLOTS: usize = 4096;
const THRESHOLD_SCALE: f32 = 512.0;
const THRESHOLD_CEILING: f32 = 127.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContrastKey(u64);

impl ContrastKey {
    fn new(fg: Hsla, bg: Hsla) -> Self {
        Self((u64::from(packed_srgb(fg)) << 32) | u64::from(packed_srgb(bg)))
    }

    fn foreground(self) -> Hsla {
        unpacked_srgb((self.0 >> 32) as u32)
    }

    fn background(self) -> Hsla {
        unpacked_srgb(self.0 as u32)
    }

    fn slot(self) -> usize {
        let mut hash = self.0 ^ 0xcbf2_9ce4_8422_2325;
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xff51_afd7_ed55_8ccd);
        hash ^= hash >> 33;
        hash = hash.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        hash ^= hash >> 33;
        (hash as usize) % CONTRAST_CACHE_SLOTS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContrastThreshold(u16);

impl ContrastThreshold {
    fn quantized(min_lc: f32) -> Self {
        let clamped = min_lc.clamp(0.0, THRESHOLD_CEILING);
        Self((clamped * THRESHOLD_SCALE).round() as u16)
    }

    fn lc(self) -> f32 {
        f32::from(self.0) / THRESHOLD_SCALE
    }
}

#[derive(Clone, Copy)]
struct ContrastEntry {
    key: ContrastKey,
    harmony: u64,
    value: Hsla,
    threshold: ContrastThreshold,
}

struct ContrastCache {
    generation: u64,
    entries: [Option<ContrastEntry>; CONTRAST_CACHE_SLOTS],
}

thread_local! {
    static CONTRAST_CACHE: std::cell::RefCell<ContrastCache> = const {
        std::cell::RefCell::new(ContrastCache {
            generation: 0,
            entries: [None; CONTRAST_CACHE_SLOTS],
        })
    };
}

#[cfg(test)]
thread_local! {
    static CONTRAST_HITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CONTRAST_MISSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn take_contrast_cache_stats() -> (usize, usize) {
    (
        CONTRAST_HITS.with(|hits| hits.replace(0)),
        CONTRAST_MISSES.with(|misses| misses.replace(0)),
    )
}

fn contrast_cache_get_or_insert(
    key: ContrastKey,
    threshold: ContrastThreshold,
    harmony: Option<&HarmonyTargets>,
) -> Hsla {
    let generation = crate::theme::theme_generation();
    let harmony_id = harmony.map_or(0, |harmony| harmony.id);
    let slot = key.slot();
    CONTRAST_CACHE.with(|cache| {
        if let Ok(mut cache) = cache.try_borrow_mut() {
            if cache.generation != generation {
                cache.entries.fill(None);
                cache.generation = generation;
            }
            if let Some(entry) = cache.entries[slot].as_ref()
                && entry.key == key
                && entry.threshold == threshold
                && entry.harmony == harmony_id
            {
                #[cfg(test)]
                CONTRAST_HITS.with(|hits| hits.set(hits.get() + 1));
                return entry.value;
            }
        }
        #[cfg(test)]
        CONTRAST_MISSES.with(|misses| misses.set(misses.get() + 1));
        let value =
            compute_minimum_contrast(key.foreground(), key.background(), threshold.lc(), harmony);
        if let Ok(mut cache) = cache.try_borrow_mut() {
            cache.entries[slot] = Some(ContrastEntry {
                key,
                harmony: harmony_id,
                value,
                threshold,
            });
        }
        value
    })
}

const MONOCHROME_BLACK: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 0.0,
    a: 1.0,
};
const MONOCHROME_WHITE: Hsla = Hsla {
    h: 0.0,
    s: 0.0,
    l: 1.0,
    a: 1.0,
};
const CHROMA_LADDER: [f32; 5] = [0.8, 0.6, 0.4, 0.2, 0.0];
const HARMONY_CHROMA_SCALE: f32 = 0.6;
const HARMONY_CHROMA_FLOOR: f32 = 0.02;
const LIGHTNESS_STEPS: usize = 20;

fn compute_minimum_contrast(
    fg: Hsla,
    bg: Hsla,
    min_lc: f32,
    harmony: Option<&HarmonyTargets>,
) -> Hsla {
    if apca_contrast(fg, bg).abs() >= min_lc {
        return fg;
    }

    let source = hsla_to_oklch(fg);

    for scale in std::iter::once(1.0).chain(CHROMA_LADDER) {
        let candidate = Oklch {
            c: source.c * scale,
            ..source
        };
        let Some(corrected) = bisect_lightness_either_way(candidate, bg, min_lc, fg.a) else {
            continue;
        };
        let Some(harmony) = harmony else {
            return corrected;
        };
        let realized = hsla_to_oklch(corrected).c;
        let drained =
            source.c >= HARMONY_CHROMA_FLOOR && realized < source.c * HARMONY_CHROMA_SCALE;
        let collapsed = source.c >= HARMONY_CHROMA_FLOOR && realized < HARMONY_CHROMA_FLOOR;
        if !drained && !collapsed {
            return corrected;
        }
        return pull_to_theme(corrected, bg, min_lc, fg.a, harmony);
    }

    monochrome_fallback(bg, fg.a)
}

fn resolve_lightness(color: Oklch, lightness: f32, alpha: f32) -> Hsla {
    oklch_to_hsla_in_gamut(
        Oklch {
            l: lightness,
            ..color
        },
        alpha,
    )
}

fn darkening_direction(bg: Hsla) -> bool {
    srgb_to_y(bg) > 0.5
}

fn bisect_lightness_either_way(color: Oklch, bg: Hsla, min_lc: f32, alpha: f32) -> Option<Hsla> {
    let darken = darkening_direction(bg);
    bisect_lightness(color, bg, min_lc, darken, alpha)
        .or_else(|| bisect_lightness(color, bg, min_lc, !darken, alpha))
}

fn bisect_lightness(color: Oklch, bg: Hsla, min_lc: f32, darken: bool, alpha: f32) -> Option<Hsla> {
    let limit = if darken { 0.0 } else { 1.0 };
    let mut best = resolve_lightness(color, limit, alpha);
    if apca_contrast(best, bg).abs() < min_lc {
        return None;
    }
    let mut near = color.l.clamp(0.0, 1.0);
    let mut far = limit;
    for _ in 0..LIGHTNESS_STEPS {
        let mid = (near + far) * 0.5;
        let candidate = resolve_lightness(color, mid, alpha);
        if apca_contrast(candidate, bg).abs() >= min_lc {
            best = candidate;
            far = mid;
        } else {
            near = mid;
        }
    }
    Some(best)
}

fn pull_to_theme(
    corrected: Hsla,
    bg: Hsla,
    min_lc: f32,
    alpha: f32,
    harmony: &HarmonyTargets,
) -> Hsla {
    let nearest = harmony.nearest(hsla_to_oklab(corrected));
    let target = Hsla {
        a: alpha,
        ..nearest
    };
    if apca_contrast(target, bg).abs() >= min_lc {
        return target;
    }
    let lch = hsla_to_oklch(target);
    bisect_lightness_either_way(lch, bg, min_lc, alpha).unwrap_or(corrected)
}

fn monochrome_fallback(bg: Hsla, alpha: f32) -> Hsla {
    let extreme =
        if apca_contrast(MONOCHROME_BLACK, bg).abs() >= apca_contrast(MONOCHROME_WHITE, bg).abs() {
            MONOCHROME_BLACK
        } else {
            MONOCHROME_WHITE
        };
    Hsla {
        a: alpha,
        ..extreme
    }
}

pub(super) fn convert_color(color: Color, theme: &TerminalTheme, palette: &ThemePalette) -> Hsla {
    match color {
        Color::Named(name) => named_color(name, theme),
        Color::Spec(rgb) => rgb_to_hsla(rgb.r, rgb.g, rgb.b),
        Color::Indexed(i) => indexed_color(i, theme, palette),
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

fn indexed_color(i: u8, theme: &TerminalTheme, palette: &ThemePalette) -> Hsla {
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

    palette.color(i)
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
    fn the_same_visible_color_ignores_alpha_and_hue_of_greys() {
        let opaque = Hsla {
            h: 0.3,
            s: 0.0,
            l: 0.5,
            a: 1.0,
        };
        let translucent = Hsla { a: 0.4, ..opaque };
        let other_hue = Hsla { h: 0.8, ..opaque };
        assert!(is_same_visible_color(opaque, translucent));
        assert!(is_same_visible_color(opaque, other_hue));
        assert!(!is_same_visible_color(opaque, Hsla { l: 0.9, ..opaque }));
    }

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
            let cached = ensure_minimum_contrast(*fg, *bg, min_lc, None);
            let direct = uncached_minimum_contrast(*fg, *bg, min_lc);
            assert_eq!(
                (cached.h, cached.s, cached.l, cached.a),
                (direct.h, direct.s, direct.l, fg.a),
                "cache diverged for fg={fg:?} bg={bg:?}"
            );
        }
        for (fg, bg) in &pairs {
            let cached = ensure_minimum_contrast(*fg, *bg, min_lc, None);
            let direct = uncached_minimum_contrast(*fg, *bg, min_lc);
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
        let lenient = ensure_minimum_contrast(fg, bg, 15.0, None);
        let strict = ensure_minimum_contrast(fg, bg, 75.0, None);
        assert_ne!(
            (lenient.l, lenient.s),
            (strict.l, strict.s),
            "a stricter threshold must move the foreground further"
        );
        assert_eq!(strict.l, uncached_minimum_contrast(fg, bg, 75.0).l);
    }

    #[test]
    fn the_contrast_key_packs_both_colors_into_one_word() {
        let fg = rgb_to_hsla(0x12, 0x34, 0x56);
        let bg = rgb_to_hsla(0xab, 0xcd, 0xef);
        let key = ContrastKey::new(fg, bg);
        assert_eq!(key.0, 0x0012_3456_00ab_cdef);
        assert_eq!(packed_srgb(key.foreground()), 0x0012_3456);
        assert_eq!(packed_srgb(key.background()), 0x00ab_cdef);
        let translucent = Hsla { a: 0.25, ..fg };
        assert_eq!(ContrastKey::new(translucent, bg), key);
    }

    #[test]
    fn the_threshold_field_round_trips_through_sixteen_bits() {
        for lc in [0.5_f32, 45.0, 60.0, 75.0, 90.0] {
            let threshold = ContrastThreshold::quantized(lc);
            assert!((threshold.lc() - lc).abs() <= 1.0 / THRESHOLD_SCALE);
        }
        assert_eq!(
            ContrastThreshold::quantized(400.0),
            ContrastThreshold::quantized(THRESHOLD_CEILING)
        );
    }

    #[test]
    fn a_cache_hit_returns_the_stored_color_without_recomputing() {
        let bg = rgb_to_hsla(0xf4, 0xf4, 0xf0);
        let mut attempt = 0u8;
        loop {
            let fg = rgb_to_hsla(0xc8, 0xc8, 0x40 + attempt);
            let generation = crate::theme::theme_generation();
            let _ = take_contrast_cache_stats();
            let first = ensure_minimum_contrast(fg, bg, 60.0, None);
            let (cold_hits, cold_misses) = take_contrast_cache_stats();
            let second = ensure_minimum_contrast(fg, bg, 60.0, None);
            let (warm_hits, warm_misses) = take_contrast_cache_stats();
            if crate::theme::theme_generation() == generation {
                assert_eq!(
                    (cold_hits, cold_misses),
                    (0, 1),
                    "the first lookup must compute once"
                );
                assert_eq!(
                    (warm_hits, warm_misses),
                    (1, 0),
                    "the second lookup must not recompute"
                );
                assert_eq!((first.h, first.s, first.l), (second.h, second.s, second.l));
                break;
            }
            attempt += 1;
            assert!(
                attempt < 5,
                "the theme generation kept moving during the measurement"
            );
        }
    }

    #[test]
    fn the_alpha_of_the_caller_survives_a_cache_hit() {
        let fg = rgb_to_hsla(0x80, 0x90, 0x30);
        let bg = rgb_to_hsla(0xfa, 0xfa, 0xfa);
        let opaque = ensure_minimum_contrast(fg, bg, 60.0, None);
        let dim = ensure_minimum_contrast(Hsla { a: 0.5, ..fg }, bg, 60.0, None);
        assert_eq!((opaque.a, dim.a), (1.0, 0.5));
        assert_eq!((opaque.h, opaque.s, opaque.l), (dim.h, dim.s, dim.l));
    }

    #[test]
    fn a_theme_generation_change_clears_the_cache_before_the_next_lookup() {
        let bg = rgb_to_hsla(0xfb, 0xfb, 0xfb);
        let mut attempt = 0u8;
        loop {
            let fg = rgb_to_hsla(0x30, 0x90, 0xc0 + attempt);
            let generation = crate::theme::theme_generation();
            let _ = take_contrast_cache_stats();
            let _ = ensure_minimum_contrast(fg, bg, 60.0, None);
            let _ = ensure_minimum_contrast(fg, bg, 60.0, None);
            let (hits, _) = take_contrast_cache_stats();
            if crate::theme::theme_generation() == generation {
                assert_eq!(
                    hits, 1,
                    "the pair must be cached before the generation moves"
                );
                crate::theme::invalidate_theme_cache();
                let _ = ensure_minimum_contrast(fg, bg, 60.0, None);
                let (hits, misses) = take_contrast_cache_stats();
                assert_eq!(
                    (hits, misses),
                    (0, 1),
                    "a new theme generation must clear the cache before serving anything"
                );
                break;
            }
            attempt += 1;
            assert!(
                attempt < 5,
                "the theme generation kept moving during the measurement"
            );
        }
    }

    #[test]
    fn the_cache_answers_from_a_worker_thread_without_a_global_lock() {
        let handle = std::thread::spawn(|| {
            let fg = rgb_to_hsla(0x99, 0x70, 0x20);
            let bg = rgb_to_hsla(0xf2, 0xf2, 0xf2);
            let _ = take_contrast_cache_stats();
            let direct = uncached_minimum_contrast(fg, bg, 60.0);
            let cached = ensure_minimum_contrast(fg, bg, 60.0, None);
            let (_, misses) = take_contrast_cache_stats();
            assert_eq!(misses, 1, "a fresh thread starts with an empty cache");
            assert_eq!(
                (cached.h, cached.s, cached.l),
                (direct.h, direct.s, direct.l)
            );
        });
        handle.join().expect("the worker thread must not panic");
    }

    fn hue_gap(a: f32, b: f32) -> f32 {
        let gap = (a - b).abs();
        gap.min(360.0 - gap)
    }

    #[test]
    fn the_correction_keeps_the_hue_when_the_gamut_leaves_the_chroma_alone() {
        let backgrounds = [
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.98,
                a: 1.0,
            },
            Hsla {
                h: 0.0,
                s: 0.0,
                l: 0.09,
                a: 1.0,
            },
        ];
        let mut checked = 0usize;
        for bg in backgrounds {
            for step in 0..24 {
                let fg = Hsla {
                    h: step as f32 / 24.0,
                    s: 0.45,
                    l: 0.5,
                    a: 1.0,
                };
                let source = hsla_to_oklch(fg);
                let corrected = compute_minimum_contrast(fg, bg, 60.0, None);
                let result = hsla_to_oklch(corrected);
                if result.c < source.c * 0.999 {
                    continue;
                }
                checked += 1;
                assert!(
                    apca_contrast(corrected, bg).abs() >= 60.0,
                    "the correction must reach the threshold for {fg:?}"
                );
                let drift = hue_gap(source.h, result.h);
                assert!(
                    drift <= 2.0,
                    "an untouched chroma must keep the hue, {fg:?} drifted {drift} degrees"
                );
            }
        }
        assert!(
            checked > 10,
            "the fixture must exercise corrections that keep their chroma, got {checked}"
        );
    }

    #[test]
    fn the_correction_stops_at_the_lightness_closest_to_the_original() {
        let bg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.98,
            a: 1.0,
        };
        let fg = Hsla {
            h: 0.13,
            s: 0.8,
            l: 0.62,
            a: 1.0,
        };
        let corrected = compute_minimum_contrast(fg, bg, 60.0, None);
        let source = hsla_to_oklch(fg);
        let result = hsla_to_oklch(corrected);
        assert!(
            result.l < source.l,
            "a light background must darken the foreground"
        );
        let short_of_it = oklch_to_hsla_in_gamut(
            Oklch {
                l: result.l + (source.l - result.l) * 0.25,
                ..result
            },
            fg.a,
        );
        assert!(
            apca_contrast(short_of_it, bg).abs() < 60.0,
            "a lightness closer to the original must miss the threshold"
        );
    }

    #[test]
    fn an_unreachable_threshold_falls_back_to_black_or_white() {
        let light_bg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        };
        let dark_bg = Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        };
        let fg = Hsla {
            h: 0.6,
            s: 0.7,
            l: 0.5,
            a: 1.0,
        };
        let on_light = compute_minimum_contrast(fg, light_bg, 200.0, None);
        let on_dark = compute_minimum_contrast(fg, dark_bg, 200.0, None);
        assert_eq!((on_light.s, on_light.l), (0.0, 0.0));
        assert_eq!((on_dark.s, on_dark.l), (0.0, 1.0));
    }

    #[test]
    fn a_mid_grey_background_keeps_the_hue_instead_of_collapsing_to_black() {
        for level in [180u8, 185, 191] {
            let bg = rgb_to_hsla(level, level, level);
            for (r, g, b) in [(120u8, 170u8, 255u8), (230, 120, 40), (90, 200, 120)] {
                let fg = rgb_to_hsla(r, g, b);
                let corrected = compute_minimum_contrast(fg, bg, 60.0, None);
                assert!(
                    apca_contrast(corrected, bg).abs() >= 60.0,
                    "grey {level}: rgb({r},{g},{b}) must reach the threshold"
                );
                let result = hsla_to_oklch(corrected);
                assert!(
                    result.c >= HARMONY_CHROMA_FLOOR,
                    "grey {level}: rgb({r},{g},{b}) must not collapse to the monochrome                      fallback, chroma {}",
                    result.c
                );
                let drift = hue_gap(hsla_to_oklch(fg).h, result.h);
                assert!(
                    drift <= 2.0,
                    "grey {level}: rgb({r},{g},{b}) drifted {drift} degrees"
                );
            }
        }
    }

    #[test]
    fn a_source_below_the_chroma_floor_is_never_pulled() {
        for name in ["Paneflow Light", "Paneflow Dark"] {
            let theme = crate::theme::theme_by_name(name).expect("the preset must exist");
            let harmony = HarmonyTargets::from_theme(&theme);
            let mut checked = 0usize;
            for bg in [theme.ansi_background, rgb_to_hsla(185, 185, 185)] {
                for level in [40u8, 110, 150, 200] {
                    for tint in [0u8, 4, 8] {
                        let fg = rgb_to_hsla(level, level.saturating_sub(tint), level);
                        if hsla_to_oklch(fg).c >= HARMONY_CHROMA_FLOOR {
                            continue;
                        }
                        checked += 1;
                        let plain = compute_minimum_contrast(fg, bg, 60.0, None);
                        let pulled = compute_minimum_contrast(fg, bg, 60.0, Some(&harmony));
                        assert_eq!(
                            (plain.h, plain.s, plain.l),
                            (pulled.h, pulled.s, pulled.l),
                            "{name}: rgb({level},{},{level}) started below the chroma floor and                              must never be tinted by the pull",
                            level.saturating_sub(tint)
                        );
                    }
                }
            }
            assert!(
                checked >= 8,
                "{name}: the fixture must exercise near-neutral sources, got {checked}"
            );
        }
    }

    #[test]
    fn a_drained_chroma_is_pulled_onto_a_theme_color() {
        let theme = crate::theme::theme_by_name("Paneflow Dark").expect("the preset must exist");
        let harmony = HarmonyTargets::from_theme(&theme);
        let bg = theme.ansi_background;
        let fg = rgb_to_hsla(255, 0, 255);
        let plain = compute_minimum_contrast(fg, bg, 60.0, None);
        let pulled = compute_minimum_contrast(fg, bg, 60.0, Some(&harmony));

        let source_chroma = hsla_to_oklch(fg).c;
        assert!(
            hsla_to_oklch(plain).c < source_chroma * HARMONY_CHROMA_SCALE,
            "the fixture must drain more than 40% of the chroma"
        );
        assert_ne!(
            (plain.h, plain.s, plain.l),
            (pulled.h, pulled.s, pulled.l),
            "a drained chroma must move onto the theme"
        );
        assert!(
            apca_contrast(pulled, bg).abs() >= 60.0,
            "the pulled color must still meet the threshold"
        );
        let target = harmony.nearest(hsla_to_oklab(plain));
        assert!(
            hue_gap(hsla_to_oklch(pulled).h, hsla_to_oklch(target).h) <= 2.0,
            "the pull must land on the hue of the nearest theme color"
        );
    }

    #[test]
    fn a_mild_chroma_reduction_is_never_pulled() {
        let theme = crate::theme::theme_by_name("Paneflow Light").expect("the preset must exist");
        let harmony = HarmonyTargets::from_theme(&theme);
        let bg = theme.ansi_background;
        let mut checked = 0usize;
        for (r, g, b) in [
            (255u8, 255u8, 0u8),
            (0, 255, 0),
            (120, 170, 255),
            (255, 170, 0),
        ] {
            let fg = rgb_to_hsla(r, g, b);
            let plain = compute_minimum_contrast(fg, bg, 60.0, None);
            if hsla_to_oklch(plain).c < hsla_to_oklch(fg).c * HARMONY_CHROMA_SCALE {
                continue;
            }
            checked += 1;
            let pulled = compute_minimum_contrast(fg, bg, 60.0, Some(&harmony));
            assert_eq!(
                (plain.h, plain.s, plain.l),
                (pulled.h, pulled.s, pulled.l),
                "rgb({r},{g},{b}) kept its chroma and must not be pulled"
            );
            let drift = hue_gap(hsla_to_oklch(fg).h, hsla_to_oklch(pulled).h);
            assert!(
                drift <= 2.0,
                "rgb({r},{g},{b}) drifted {drift} degrees without a chroma reduction"
            );
        }
        assert!(
            checked >= 3,
            "the fixture must exercise mild reductions, got {checked}"
        );
    }

    #[test]
    fn an_already_neutral_color_is_never_tinted_by_the_pull() {
        let theme = crate::theme::theme_by_name("Paneflow Light").expect("the preset must exist");
        let harmony = HarmonyTargets::from_theme(&theme);
        let bg = theme.ansi_background;
        for level in [150u8, 200, 255] {
            let fg = rgb_to_hsla(level, level, level);
            let pulled = compute_minimum_contrast(fg, bg, 60.0, Some(&harmony));
            assert!(
                hsla_to_oklch(pulled).c < HARMONY_CHROMA_FLOOR,
                "grey {level} must stay neutral, got chroma {}",
                hsla_to_oklch(pulled).c
            );
        }
    }

    #[test]
    fn the_cache_keys_on_the_harmony_targets() {
        let light = crate::theme::theme_by_name("Paneflow Light").expect("the preset must exist");
        let dark = crate::theme::theme_by_name("Paneflow Dark").expect("the preset must exist");
        let light_harmony = HarmonyTargets::from_theme(&light);
        let dark_harmony = HarmonyTargets::from_theme(&dark);
        assert_ne!(
            light_harmony.id, dark_harmony.id,
            "two presets must not share a harmony identity"
        );
        let fg = rgb_to_hsla(255, 0, 255);
        let bg = dark.ansi_background;
        let with_dark = ensure_minimum_contrast(fg, bg, 60.0, Some(&dark_harmony));
        let with_light = ensure_minimum_contrast(fg, bg, 60.0, Some(&light_harmony));
        assert_eq!(
            with_dark,
            compute_minimum_contrast(fg, bg, 60.0, Some(&dark_harmony)),
            "the cache must not serve another theme's answer"
        );
        assert_eq!(
            with_light,
            compute_minimum_contrast(fg, bg, 60.0, Some(&light_harmony)),
            "the cache must not serve another theme's answer"
        );
    }

    #[test]
    fn default_ground_colors_use_the_terminal_theme_slots() {
        let theme = crate::theme::paneflow_dark();
        let palette = ThemePalette::from_theme(&theme);

        assert_eq!(
            convert_color(Color::Named(NamedColor::Foreground), &theme, &palette),
            theme.foreground
        );
        assert_eq!(
            convert_color(Color::Named(NamedColor::Background), &theme, &palette),
            theme.ansi_background
        );
    }

    fn preset(name: &str) -> TerminalTheme {
        crate::theme::theme_by_name(name).unwrap_or_else(|| panic!("preset {name} must exist"))
    }

    #[test]
    fn the_cube_and_grey_ramp_resolve_from_the_theme_palette() {
        for name in crate::theme::THEMES {
            let theme = preset(name.0);
            let palette = ThemePalette::from_theme(&theme);
            for index in 16u8..=255 {
                assert_eq!(
                    convert_color(Color::Indexed(index), &theme, &palette),
                    palette.color(index),
                    "{}: index {index} must resolve from the render palette",
                    name.0
                );
            }
        }
    }

    #[test]
    fn the_first_sixteen_indices_stay_on_the_named_slots() {
        let theme = preset("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        let expected = [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
            NamedColor::BrightBlack,
            NamedColor::BrightRed,
            NamedColor::BrightGreen,
            NamedColor::BrightYellow,
            NamedColor::BrightBlue,
            NamedColor::BrightMagenta,
            NamedColor::BrightCyan,
            NamedColor::BrightWhite,
        ];
        for (index, name) in expected.into_iter().enumerate() {
            let index = u8::try_from(index).expect("ansi index fits a u8");
            assert_eq!(
                convert_color(Color::Indexed(index), &theme, &palette),
                named_color(name, &theme),
                "index {index} must keep the named ANSI slot"
            );
        }
    }

    #[test]
    fn the_light_cube_replaces_the_illegible_xterm_corners() {
        let theme = preset("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        for (index, xterm) in [(230u8, (255u8, 255u8, 215u8)), (187, (215, 215, 175))] {
            let baseline = rgb_to_hsla(xterm.0, xterm.1, xterm.2);
            let baseline_lc = apca_contrast(baseline, theme.ansi_background).abs();
            assert!(
                baseline_lc < 25.0,
                "the xterm cube value for index {index} is the illegible baseline, Lc {baseline_lc}"
            );
            let resolved = convert_color(Color::Indexed(index), &theme, &palette);
            let resolved_lc = apca_contrast(resolved, theme.ansi_background).abs();
            assert!(
                resolved_lc > 45.0,
                "index {index} must resolve to a legible theme color, Lc {resolved_lc}"
            );
        }
    }

    #[test]
    fn the_light_grey_ramp_keeps_its_steps_legible() {
        let theme = preset("Paneflow Light");
        let palette = ThemePalette::from_theme(&theme);
        for index in 240u8..=255 {
            let resolved = convert_color(Color::Indexed(index), &theme, &palette);
            let lc = apca_contrast(resolved, theme.ansi_background).abs();
            assert!(
                lc > 45.0,
                "grey ramp index {index} must stay legible on a light theme, Lc {lc}"
            );
        }
    }
}
