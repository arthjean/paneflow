//! Color parsing, palette generation, and contrast math from libghostty.
//!
//! Paneflow keeps its own theme model, so nothing here replaces it. These are
//! the terminal-accurate primitives: the same X11 name table and the same
//! `#rgb`/`rgb:`/`0x` parsers a config file goes through, plus the palette
//! generator that fills the 216-color cube and grayscale ramp from a
//! foreground/background pair.

use std::ffi::CStr;
use std::sync::OnceLock;

use paneflow_libghostty_sys as sys;

use crate::encode::encode_with_buffer;
use crate::handles::check;
use crate::{ColorScheme, Result, Rgb};

/// Number of entries in a terminal color palette.
pub const PALETTE_LEN: usize = 256;

/// Upper bound for an encoded color scheme report (`CSI ? 997 ; N n`).
const MAX_COLOR_SCHEME_REPORT_BYTES: usize = 32;

impl From<Rgb> for sys::GhosttyColorRgb {
    fn from(value: Rgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

/// A 256-bit set of palette indices, used to pin entries that palette
/// generation must copy from the base palette instead of deriving.
#[derive(Clone, Copy, Debug)]
pub struct PaletteMask(sys::GhosttyColorPaletteMask);

impl Default for PaletteMask {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for PaletteMask {
    fn eq(&self, other: &Self) -> bool {
        self.0.bits == other.0.bits
    }
}

impl Eq for PaletteMask {}

impl PaletteMask {
    /// An empty mask: generation derives every index.
    #[must_use]
    pub const fn new() -> Self {
        Self(sys::GhosttyColorPaletteMask { bits: [0; 4] })
    }

    /// Pin `index` to its base-palette value.
    pub fn set(&mut self, index: u8) {
        self.0.bits[usize::from(index) >> 6] |= 1u64 << (index & 63);
    }

    /// Stop pinning `index`.
    pub fn unset(&mut self, index: u8) {
        self.0.bits[usize::from(index) >> 6] &= !(1u64 << (index & 63));
    }

    /// Whether `index` is pinned.
    #[must_use]
    pub fn contains(&self, index: u8) -> bool {
        self.0.bits[usize::from(index) >> 6] & (1u64 << (index & 63)) != 0
    }
}

/// Parse a color specification: `#RGB`, `#RRGGBB`, `rgb:R/G/B`, an X11 name,
/// or any other form Ghostty's config accepts.
pub fn parse(value: &str) -> Result<Rgb> {
    let mut out = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
    // SAFETY: the pointer and length describe `value`'s bytes, and `out` is
    // valid writable storage.
    let result =
        unsafe { sys::ghostty_color_parse(value.as_ptr().cast(), value.len(), &mut out) };
    check("color_parse", result)?;
    Ok(out.into())
}

/// Parse an X11 color name such as `cornflowerblue`.
pub fn parse_x11(name: &str) -> Result<Rgb> {
    let mut out = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
    // SAFETY: the pointer and length describe `name`'s bytes, and `out` is
    // valid writable storage.
    let result =
        unsafe { sys::ghostty_color_parse_x11(name.as_ptr().cast(), name.len(), &mut out) };
    check("color_parse_x11", result)?;
    Ok(out.into())
}

/// Parse a `index=color` palette override such as `0x10=#282c34`.
pub fn parse_palette_entry(value: &str) -> Result<(u8, Rgb)> {
    let mut index = 0u8;
    let mut rgb = sys::GhosttyColorRgb { r: 0, g: 0, b: 0 };
    // SAFETY: the pointer and length describe `value`'s bytes, and both
    // out-parameters are valid writable storage.
    let result = unsafe {
        sys::ghostty_color_parse_palette_entry(
            value.as_ptr().cast(),
            value.len(),
            &mut index,
            &mut rgb,
        )
    };
    check("color_parse_palette_entry", result)?;
    Ok((index, rgb.into()))
}

/// Ghostty's built-in 256-color palette.
#[must_use]
pub fn default_palette() -> [Rgb; PALETTE_LEN] {
    let mut raw = [sys::GhosttyColorRgb { r: 0, g: 0, b: 0 }; PALETTE_LEN];
    // SAFETY: the callee writes exactly `PALETTE_LEN` entries into the array.
    unsafe { sys::ghostty_color_palette_default(raw.as_mut_ptr()) };
    raw.map(Rgb::from)
}

/// Derive the 216-color cube and grayscale ramp from `background` and
/// `foreground`.
///
/// `base` supplies the ANSI 0-15 entries and anything pinned by `skip`;
/// `None` uses Ghostty's default palette. `harmonious` keeps the
/// background-to-foreground orientation for light themes instead of swapping
/// it so the ramp always runs dark-to-light.
#[must_use]
pub fn generate_palette(
    base: Option<&[Rgb; PALETTE_LEN]>,
    skip: &PaletteMask,
    background: Rgb,
    foreground: Rgb,
    harmonious: bool,
) -> [Rgb; PALETTE_LEN] {
    let base_raw = base.map(|palette| palette.map(sys::GhosttyColorRgb::from));
    let base_pointer = base_raw
        .as_ref()
        .map_or(std::ptr::null(), |palette| palette.as_ptr());
    let background = sys::GhosttyColorRgb::from(background);
    let foreground = sys::GhosttyColorRgb::from(foreground);
    let mut out = [sys::GhosttyColorRgb { r: 0, g: 0, b: 0 }; PALETTE_LEN];
    // SAFETY: `base_pointer` is null or points at `PALETTE_LEN` entries that
    // outlive the call, the mask and both colors are live, and `out` is
    // writable storage for exactly `PALETTE_LEN` entries.
    unsafe {
        sys::ghostty_color_palette_generate(
            base_pointer,
            &raw const skip.0,
            &raw const background,
            &raw const foreground,
            harmonious,
            out.as_mut_ptr(),
        );
    }
    out.map(Rgb::from)
}

/// Relative luminance, as used for WCAG contrast.
#[must_use]
pub fn luminance(color: Rgb) -> f64 {
    let color = sys::GhosttyColorRgb::from(color);
    // SAFETY: `color` is a live struct passed by pointer for the call only.
    unsafe { sys::ghostty_color_luminance(&raw const color) }
}

/// Perceived luminance, which weights the channels the way the eye does.
#[must_use]
pub fn perceived_luminance(color: Rgb) -> f64 {
    let color = sys::GhosttyColorRgb::from(color);
    // SAFETY: `color` is a live struct passed by pointer for the call only.
    unsafe { sys::ghostty_color_perceived_luminance(&raw const color) }
}

/// WCAG contrast ratio between two colors, from 1.0 to 21.0.
#[must_use]
pub fn contrast(a: Rgb, b: Rgb) -> f64 {
    let a = sys::GhosttyColorRgb::from(a);
    let b = sys::GhosttyColorRgb::from(b);
    // SAFETY: both structs are live and passed by pointer for the call only.
    unsafe { sys::ghostty_color_contrast(&raw const a, &raw const b) }
}

/// Ghostty's X11 color name table.
///
/// The table is static in the library, so it is walked once and cached.
#[must_use]
pub fn x11_names() -> &'static [(&'static str, Rgb)] {
    static NAMES: OnceLock<Vec<(&'static str, Rgb)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        // SAFETY: both accessors read immutable static data owned by the
        // library, so the table and its name strings live for the program.
        let (entries, count) =
            unsafe { (sys::ghostty_color_x11_names(), sys::ghostty_color_x11_name_count()) };
        if entries.is_null() {
            return Vec::new();
        }
        (0..count)
            .filter_map(|index| {
                // SAFETY: `index` is below the count the library reported for
                // this table, so the entry is in bounds.
                let entry = unsafe { *entries.add(index) };
                if entry.name.is_null() {
                    return None;
                }
                // SAFETY: the table's names are null-terminated statics.
                let name = unsafe { CStr::from_ptr(entry.name) }.to_str().ok()?;
                Some((name, Rgb::from(entry.color)))
            })
            .collect()
    })
}

/// Encode a color scheme report (`CSI ? 997 ; N n`) for mode 2031 subscribers.
pub fn encode_color_scheme_report(scheme: ColorScheme) -> Result<Vec<u8>> {
    let scheme = match scheme {
        ColorScheme::Light => sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_LIGHT,
        ColorScheme::Dark => sys::GhosttyColorScheme_GHOSTTY_COLOR_SCHEME_DARK,
    };
    encode_with_buffer(
        "color_scheme_report_encode",
        MAX_COLOR_SCHEME_REPORT_BYTES,
        |buffer, len, written| unsafe {
            sys::ghostty_color_scheme_report_encode(scheme, buffer, len, written)
        },
    )
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_x11_and_palette_entries_all_parse() {
        assert_eq!(parse("#282c34").expect("hex"), Rgb { r: 40, g: 44, b: 52 });
        assert_eq!(
            parse_x11("cornflowerblue").expect("x11 name"),
            Rgb {
                r: 100,
                g: 149,
                b: 237
            }
        );
        let (index, rgb) = parse_palette_entry("0x10=#282c34").expect("palette entry");
        assert_eq!(index, 16);
        assert_eq!(rgb, Rgb { r: 40, g: 44, b: 52 });
        assert!(parse("not a color").is_err());
    }

    #[test]
    fn generation_preserves_only_the_pinned_indices() {
        let default = default_palette();
        let mut base = default;
        base[20] = Rgb { r: 1, g: 2, b: 3 };
        base[21] = Rgb { r: 4, g: 5, b: 6 };

        let mut skip = PaletteMask::new();
        skip.set(20);
        assert!(skip.contains(20));
        assert!(!skip.contains(21));

        let generated = generate_palette(
            Some(&base),
            &skip,
            Rgb { r: 0, g: 0, b: 0 },
            Rgb {
                r: 255,
                g: 255,
                b: 255,
            },
            true,
        );
        assert_eq!(generated[20], Rgb { r: 1, g: 2, b: 3 });
        assert_ne!(generated[21], Rgb { r: 4, g: 5, b: 6 });

        skip.unset(20);
        assert!(!skip.contains(20));
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        let black = Rgb { r: 0, g: 0, b: 0 };
        let white = Rgb {
            r: 255,
            g: 255,
            b: 255,
        };
        assert!((contrast(black, white) - 21.0).abs() < 0.01);
        assert!((contrast(white, black) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.01);
        assert!(luminance(white) > luminance(black));
        assert!(perceived_luminance(white) > perceived_luminance(black));
    }

    #[test]
    fn the_x11_table_is_non_empty_and_agrees_with_the_parser() {
        let names = x11_names();
        assert!(names.len() > 100);
        let (name, color) = names[0];
        assert_eq!(parse_x11(name).expect("table name must parse"), color);
    }

    #[test]
    fn color_scheme_reports_encode_both_variants() {
        assert_eq!(
            encode_color_scheme_report(ColorScheme::Dark).expect("dark"),
            b"\x1b[?997;1n"
        );
        assert_eq!(
            encode_color_scheme_report(ColorScheme::Light).expect("light"),
            b"\x1b[?997;2n"
        );
    }
}
