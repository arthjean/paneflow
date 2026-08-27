//! Terminal cell styles as libghostty reports them.
//!
//! [`Style`] keeps the raw `GhosttyStyle` so `ghostty_style_is_default` stays
//! authoritative about what "default" means, while the accessors hand out the
//! crate's neutral colors and flags.

use paneflow_libghostty_sys as sys;

use crate::snapshot_ffi::underline;
use crate::{CellFlags, Color, GhosttyError, Result};

/// A terminal cell style: colors plus text decoration flags.
#[derive(Clone, Copy)]
pub struct Style(sys::GhosttyStyle);

impl Style {
    /// Wrap a style libghostty produced, for example through a grid ref.
    #[must_use]
    pub(crate) fn from_raw(raw: sys::GhosttyStyle) -> Self {
        Self(raw)
    }

    /// Whether every color is unset and every flag is off.
    #[must_use]
    pub fn is_default(&self) -> bool {
        // SAFETY: `self.0` is a live, fully initialized style: either
        // libghostty filled it or `Style::default` did.
        unsafe { sys::ghostty_style_is_default(&raw const self.0) }
    }

    /// The foreground color, or [`Color::Default`] when unset.
    pub fn foreground(&self) -> Result<Color> {
        style_color(self.0.fg_color)
    }

    /// The background color, or [`Color::Default`] when unset.
    pub fn background(&self) -> Result<Color> {
        style_color(self.0.bg_color)
    }

    /// The explicit underline color, or [`Color::Default`] when the underline
    /// follows the foreground.
    pub fn underline_color(&self) -> Result<Color> {
        style_color(self.0.underline_color)
    }

    /// The decoration flags, including the underline shape.
    pub fn flags(&self) -> Result<CellFlags> {
        Ok(CellFlags {
            bold: self.0.bold,
            dim: self.0.faint,
            italic: self.0.italic,
            inverse: self.0.inverse,
            invisible: self.0.invisible,
            strikethrough: self.0.strikethrough,
            overline: self.0.overline,
            underline: underline(self.0.underline)?,
        })
    }

    /// Whether the style requests blinking text.
    #[must_use]
    pub fn blink(&self) -> bool {
        self.0.blink
    }
}

impl Default for Style {
    fn default() -> Self {
        let mut raw = std::mem::MaybeUninit::<sys::GhosttyStyle>::uninit();
        // SAFETY: `ghostty_style_default` fully initializes the struct it is
        // handed, including the leading `size` field.
        let raw = unsafe {
            sys::ghostty_style_default(raw.as_mut_ptr());
            raw.assume_init()
        };
        Self(raw)
    }
}

impl std::fmt::Debug for Style {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Style")
            .field("foreground", &self.foreground().ok())
            .field("background", &self.background().ok())
            .field("underline_color", &self.underline_color().ok())
            .field("flags", &self.flags().ok())
            .field("blink", &self.blink())
            .finish()
    }
}

impl PartialEq for Style {
    fn eq(&self, other: &Self) -> bool {
        self.foreground().ok() == other.foreground().ok()
            && self.background().ok() == other.background().ok()
            && self.underline_color().ok() == other.underline_color().ok()
            && self.flags().ok() == other.flags().ok()
            && self.blink() == other.blink()
    }
}

impl Eq for Style {}

/// Map a `GhosttyStyleColor` tagged union onto the crate's neutral color.
///
/// Palette entries stay indexed so Paneflow's own theme resolves them.
pub(crate) fn style_color(color: sys::GhosttyStyleColor) -> Result<Color> {
    match color.tag {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => Ok(Color::Default),
        // SAFETY: the tag selects the active union field.
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            Ok(Color::Palette(unsafe { color.value.palette }))
        }
        // SAFETY: the tag selects the active union field.
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_RGB => {
            Ok(Color::Rgb(unsafe { color.value.rgb }.into()))
        }
        _ => Err(GhosttyError::AbiMismatch(
            "unknown Ghostty style color tag".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::UnderlineStyle;

    #[test]
    fn the_default_style_is_reported_as_default() {
        let style = Style::default();
        assert!(style.is_default());
        assert_eq!(style.foreground().expect("fg"), Color::Default);
        assert_eq!(style.background().expect("bg"), Color::Default);
        assert_eq!(style.underline_color().expect("underline"), Color::Default);
        assert_eq!(
            style.flags().expect("flags").underline,
            UnderlineStyle::None
        );
        assert!(!style.blink());
        assert_eq!(style, Style::default());
    }

    #[test]
    fn a_bold_style_is_not_the_default_style() {
        let mut style = Style::default();
        style.0.bold = true;
        assert!(!style.is_default());
        assert!(style.flags().expect("flags").bold);
        assert_ne!(style, Style::default());
    }
}
