use paneflow_libghostty_sys as sys;

use crate::Rgb;

pub const PALETTE_LEN: usize = 256;

impl From<Rgb> for sys::GhosttyColorRgb {
    fn from(value: Rgb) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

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
    #[must_use]
    pub const fn new() -> Self {
        Self(sys::GhosttyColorPaletteMask { bits: [0; 4] })
    }

    #[cfg(test)]
    pub fn set(&mut self, index: u8) {
        self.0.bits[usize::from(index) >> 6] |= 1u64 << (index & 63);
    }

    #[cfg(test)]
    pub fn unset(&mut self, index: u8) {
        self.0.bits[usize::from(index) >> 6] &= !(1u64 << (index & 63));
    }

    #[cfg(test)]
    #[must_use]
    pub fn contains(&self, index: u8) -> bool {
        self.0.bits[usize::from(index) >> 6] & (1u64 << (index & 63)) != 0
    }
}

#[must_use]
pub fn default_palette() -> [Rgb; PALETTE_LEN] {
    let mut raw = [sys::GhosttyColorRgb { r: 0, g: 0, b: 0 }; PALETTE_LEN];
    unsafe { sys::ghostty_color_palette_default(raw.as_mut_ptr()) };
    raw.map(Rgb::from)
}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
