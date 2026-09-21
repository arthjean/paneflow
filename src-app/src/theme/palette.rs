use std::sync::Arc;

use gpui::Hsla;
use paneflow_terminal_ghostty as ghostty;
use parking_lot::Mutex;

use super::model::{TerminalTheme, is_light_theme};
use super::watcher::{active_theme, theme_generation};

pub const PALETTE_LEN: usize = ghostty::PALETTE_LEN;

const ANSI_SLOTS: usize = 16;

pub(crate) fn ghostty_rgb(color: Hsla) -> ghostty::Rgb {
    let rgba = gpui::Rgba::from(color);
    ghostty::Rgb {
        r: (rgba.r.clamp(0.0, 1.0) * 255.0) as u8,
        g: (rgba.g.clamp(0.0, 1.0) * 255.0) as u8,
        b: (rgba.b.clamp(0.0, 1.0) * 255.0) as u8,
    }
}

fn hsla_from_ghostty(color: ghostty::Rgb) -> Hsla {
    Hsla::from(gpui::Rgba {
        r: f32::from(color.r) / 255.0,
        g: f32::from(color.g) / 255.0,
        b: f32::from(color.b) / 255.0,
        a: 1.0,
    })
}

fn ansi_slots(theme: &TerminalTheme) -> [Hsla; ANSI_SLOTS] {
    [
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
    ]
}

#[must_use]
pub fn palette_is_harmonious(theme: &TerminalTheme) -> bool {
    is_light_theme(theme)
}

#[must_use]
pub fn generated_terminal_palette(theme: &TerminalTheme) -> [ghostty::Rgb; PALETTE_LEN] {
    let mut base = ghostty::default_palette();
    for (slot, color) in ansi_slots(theme).into_iter().enumerate() {
        base[slot] = ghostty_rgb(color);
    }
    ghostty::generate_palette(
        Some(&base),
        &ghostty::PaletteMask::default(),
        ghostty_rgb(theme.ansi_background),
        ghostty_rgb(theme.foreground),
        palette_is_harmonious(theme),
    )
}

pub struct ThemePalette {
    generation: u64,
    entries: [Hsla; PALETTE_LEN],
}

impl ThemePalette {
    #[must_use]
    pub fn from_theme(theme: &TerminalTheme) -> Self {
        Self::with_generation(theme, theme_generation())
    }

    #[must_use]
    pub fn with_generation(theme: &TerminalTheme, generation: u64) -> Self {
        let generated = generated_terminal_palette(theme);
        let mut entries = generated.map(hsla_from_ghostty);
        for (entry, color) in entries.iter_mut().zip(ansi_slots(theme)) {
            *entry = color;
        }
        Self {
            generation,
            entries,
        }
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    #[must_use]
    pub fn color(&self, index: u8) -> Hsla {
        self.entries[usize::from(index)]
    }
}

struct CachedPalette {
    theme: TerminalTheme,
    palette: Arc<ThemePalette>,
}

static PALETTE_CACHE: Mutex<Option<CachedPalette>> = Mutex::new(None);

fn install_into(
    cache: &mut Option<CachedPalette>,
    theme: &TerminalTheme,
    generation: u64,
) -> Arc<ThemePalette> {
    if let Some(cached) = cache.as_ref()
        && cached.palette.generation == generation
        && cached.theme == *theme
    {
        return Arc::clone(&cached.palette);
    }
    let palette = Arc::new(ThemePalette::with_generation(theme, generation));
    *cache = Some(CachedPalette {
        theme: *theme,
        palette: Arc::clone(&palette),
    });
    palette
}

pub(super) fn install_palette(theme: &TerminalTheme, generation: u64) -> Arc<ThemePalette> {
    install_into(&mut PALETTE_CACHE.lock(), theme, generation)
}

#[must_use]
pub fn active_palette() -> Arc<ThemePalette> {
    let generation = theme_generation();
    {
        let cache = PALETTE_CACHE.lock();
        if let Some(cached) = cache.as_ref()
            && cached.palette.generation == generation
        {
            return Arc::clone(&cached.palette);
        }
    }
    let theme = active_theme();
    install_palette(&theme, theme_generation())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{PRESETS, theme_by_name};

    fn quantized(color: Hsla) -> (u8, u8, u8) {
        let rgb = ghostty_rgb(color);
        (rgb.r, rgb.g, rgb.b)
    }

    fn preset_theme(name: &str) -> TerminalTheme {
        theme_by_name(name).unwrap_or_else(|| panic!("preset {name} must exist"))
    }

    fn cube_anchors(theme: &TerminalTheme) -> [(u8, &'static str, Hsla); 8] {
        [
            (16, "ansi_background", theme.ansi_background),
            (21, "blue", theme.blue),
            (46, "green", theme.green),
            (51, "cyan", theme.cyan),
            (196, "red", theme.red),
            (201, "magenta", theme.magenta),
            (226, "yellow", theme.yellow),
            (231, "foreground", theme.foreground),
        ]
    }

    #[test]
    fn the_sixteen_ansi_slots_are_carried_verbatim() {
        for preset in PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = preset_theme(name);
                let palette = ThemePalette::from_theme(&theme);
                for (index, slot) in ansi_slots(&theme).into_iter().enumerate() {
                    let index = u8::try_from(index).expect("ansi slot fits a u8");
                    assert_eq!(
                        palette.color(index),
                        slot,
                        "{name}: palette slot {index} must equal the theme ANSI color"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cube_corners_follow_the_generated_palette() {
        for preset in PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = preset_theme(name);
                let generated = generated_terminal_palette(&theme);
                let palette = ThemePalette::from_theme(&theme);
                for (index, source) in generated.iter().enumerate().skip(ANSI_SLOTS) {
                    let slot = u8::try_from(index).expect("palette index fits a u8");
                    let stored = quantized(palette.color(slot));
                    let source = (source.r, source.g, source.b);
                    let drift = [
                        i16::from(stored.0) - i16::from(source.0),
                        i16::from(stored.1) - i16::from(source.1),
                        i16::from(stored.2) - i16::from(source.2),
                    ];
                    assert!(
                        drift.iter().all(|delta| delta.abs() <= 1),
                        "{name}: palette slot {index} must come from generate_palette, stored {stored:?} against {source:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_cube_anchor_carries_its_theme_seed() {
        for preset in PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = preset_theme(name);
                let palette = ThemePalette::from_theme(&theme);
                for (index, slot, seed) in cube_anchors(&theme) {
                    let stored = quantized(palette.color(index));
                    let expected = quantized(seed);
                    let drift = [
                        i16::from(stored.0) - i16::from(expected.0),
                        i16::from(stored.1) - i16::from(expected.1),
                        i16::from(stored.2) - i16::from(expected.2),
                    ];
                    assert!(
                        drift.iter().all(|delta| delta.abs() <= 1),
                        "{name}: cube anchor {index} must carry the theme {slot}, stored {stored:?} against {expected:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_grey_ramp_runs_monotonically_from_background_to_foreground() {
        for preset in PRESETS {
            for name in [preset.light, preset.dark] {
                let theme = preset_theme(name);
                let palette = ThemePalette::from_theme(&theme);
                let descending = theme.foreground.l < theme.ansi_background.l;
                for index in 232u8..255 {
                    let current = palette.color(index).l;
                    let next = palette.color(index + 1).l;
                    let against = if descending {
                        next > current
                    } else {
                        next < current
                    };
                    assert!(
                        !against,
                        "{name}: grey ramp {index} to {} must move toward the foreground, {current} then {next}",
                        index + 1
                    );
                }
                let span = (palette.color(255).l - palette.color(232).l).abs();
                assert!(
                    span > 0.05,
                    "{name}: the grey ramp must span a visible lightness range, got {span}"
                );
            }
        }
    }

    #[test]
    fn light_presets_run_the_cube_from_background_to_foreground() {
        for preset in PRESETS {
            let theme = preset_theme(preset.light);
            assert!(
                palette_is_harmonious(&theme),
                "{}: light presets must generate a harmonious palette",
                preset.light
            );
            let palette = ThemePalette::from_theme(&theme);
            assert_eq!(
                quantized(palette.color(16)),
                quantized(theme.ansi_background),
                "{}: cube origin must be the theme background",
                preset.light
            );
            assert_eq!(
                quantized(palette.color(231)),
                quantized(theme.foreground),
                "{}: cube opposite corner must be the theme foreground",
                preset.light
            );
            assert!(
                palette.color(232).l > palette.color(255).l,
                "{}: the grey ramp must run from the light background to the dark foreground",
                preset.light
            );
        }
    }

    #[test]
    fn dark_presets_keep_the_dark_to_light_cube_orientation() {
        for preset in PRESETS {
            let theme = preset_theme(preset.dark);
            assert!(
                !palette_is_harmonious(&theme),
                "{}: dark presets must not harmonize the cube",
                preset.dark
            );
            let palette = ThemePalette::from_theme(&theme);
            assert!(
                palette.color(16).l < palette.color(231).l,
                "{}: cube origin must stay darker than its opposite corner",
                preset.dark
            );
            assert!(
                palette.color(232).l < palette.color(255).l,
                "{}: the grey ramp must run dark to light",
                preset.dark
            );
        }
    }

    #[test]
    fn a_degenerate_theme_builds_a_constant_grey_ramp() {
        let mut theme = preset_theme("Paneflow Dark");
        theme.foreground = theme.ansi_background;
        let palette = ThemePalette::from_theme(&theme);
        let first = quantized(palette.color(232));
        for index in 232u8..=255 {
            assert_eq!(
                quantized(palette.color(index)),
                first,
                "a theme whose foreground equals its background must ramp to a constant"
            );
        }
        assert_eq!(quantized(palette.color(16)), quantized(palette.color(231)));
    }

    #[test]
    fn a_generation_is_built_once_and_a_new_one_rebuilds() {
        let light = preset_theme("Paneflow Light");
        let dark = preset_theme("Paneflow Dark");
        let mut cache = None;

        let first = install_into(&mut cache, &light, 7);
        let second = install_into(&mut cache, &light, 7);
        assert!(
            Arc::ptr_eq(&first, &second),
            "the same generation must be served from the cache"
        );
        assert_eq!(second.generation(), 7);

        let next = install_into(&mut cache, &light, 8);
        assert!(
            !Arc::ptr_eq(&second, &next),
            "a new generation must rebuild the palette"
        );

        let swapped = install_into(&mut cache, &dark, 8);
        assert!(
            !Arc::ptr_eq(&next, &swapped),
            "a different theme at the same generation must rebuild the palette"
        );
    }

    #[test]
    fn the_active_palette_is_installed_for_the_active_theme() {
        let palette = active_palette();
        let theme = active_theme();
        if palette.generation() == theme_generation() {
            for (index, slot) in ansi_slots(&theme).into_iter().enumerate() {
                let index = u8::try_from(index).expect("ansi slot fits a u8");
                assert_eq!(palette.color(index), slot);
            }
        }
        assert_ne!(
            quantized(palette.color(16)),
            quantized(palette.color(231)),
            "a bundled theme must not generate a degenerate cube"
        );
    }
}
