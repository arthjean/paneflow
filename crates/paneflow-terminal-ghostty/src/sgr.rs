//! Standalone SGR (Select Graphic Rendition) attribute parsing.
//!
//! The terminal parses its own SGR sequences, so this is for the cases where
//! Paneflow holds styling text that never reaches a terminal: coloring a
//! captured log line, interpreting an agent's ANSI output, or validating a
//! theme snippet with the exact same parser the terminal uses.

use std::ffi::c_char;

use paneflow_libghostty_sys as sys;

use crate::handles::{OwnedHandle, check};
use crate::snapshot_ffi::underline;
use crate::{GhosttyError, Result, Rgb, UnderlineStyle};

/// Cap on a single parameter list, well past the longest real SGR sequence
/// (`38:2::R:G:B` style direct colors run to a dozen parameters).
const MAX_SGR_PARAMS: usize = 256;

/// The separator that follows a parameter in the original sequence.
///
/// Colons matter: `4:3` is a curly underline while `4;3` is an underline
/// followed by italic. The last parameter of a list is terminated by the
/// sequence itself, which libghostty reads as a semicolon.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SgrSeparator {
    /// A `;` separator, or the end of the list.
    #[default]
    Semicolon,
    /// A `:` separator, which binds this parameter to the next one.
    Colon,
}

impl SgrSeparator {
    fn byte(self) -> c_char {
        match self {
            Self::Semicolon => b';' as c_char,
            Self::Colon => b':' as c_char,
        }
    }
}

/// One parsed SGR attribute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SgrAttribute {
    /// An explicit reset to the default style (`SGR 0`).
    Unset,
    /// A parameter run libghostty could not interpret. `full` is the whole
    /// list, `partial` the prefix that failed.
    Unknown {
        /// The complete parameter list.
        full: Vec<u16>,
        /// The prefix where parsing stopped.
        partial: Vec<u16>,
    },
    /// Bold on.
    Bold,
    /// Bold off.
    ResetBold,
    /// Italic on.
    Italic,
    /// Italic off.
    ResetItalic,
    /// Faint (dim) on.
    Faint,
    /// Underline with the given shape.
    Underline(UnderlineStyle),
    /// Direct-color underline.
    UnderlineColor(Rgb),
    /// Palette-indexed underline color.
    UnderlineColor256(u8),
    /// Underline color back to the foreground.
    ResetUnderlineColor,
    /// Overline on.
    Overline,
    /// Overline off.
    ResetOverline,
    /// Blink on.
    Blink,
    /// Blink off.
    ResetBlink,
    /// Reverse video on.
    Inverse,
    /// Reverse video off.
    ResetInverse,
    /// Invisible text on.
    Invisible,
    /// Invisible text off.
    ResetInvisible,
    /// Strikethrough on.
    Strikethrough,
    /// Strikethrough off.
    ResetStrikethrough,
    /// 24-bit foreground.
    DirectColorFg(Rgb),
    /// 24-bit background.
    DirectColorBg(Rgb),
    /// One of the eight base background colors.
    Bg8(u8),
    /// One of the eight base foreground colors.
    Fg8(u8),
    /// Foreground back to the default.
    ResetFg,
    /// Background back to the default.
    ResetBg,
    /// One of the eight bright background colors.
    BrightBg8(u8),
    /// One of the eight bright foreground colors.
    BrightFg8(u8),
    /// Palette-indexed background.
    Bg256(u8),
    /// Palette-indexed foreground.
    Fg256(u8),
}

/// A reusable SGR parameter parser.
pub struct SgrParser {
    handle: OwnedHandle<sys::GhosttySgrParser>,
}

impl SgrParser {
    /// Create a parser on libghostty's default allocator.
    pub fn new() -> Result<Self> {
        // SAFETY: the null allocator selects libghostty's default, and
        // `ghostty_sgr_free` is the matching destructor for the handle
        // `ghostty_sgr_new` produces.
        let handle = unsafe {
            crate::handles::create(
                "sgr_new",
                std::ptr::null(),
                sys::ghostty_sgr_new,
                sys::ghostty_sgr_free,
            )?
        };
        Ok(Self { handle })
    }

    /// Rewind iteration to the start of the current parameter list.
    pub fn reset(&mut self) {
        // SAFETY: the handle is live for as long as `self` is.
        unsafe { sys::ghostty_sgr_reset(self.handle.raw()) };
    }

    /// Load a parameter list. libghostty copies the data, so the slices are
    /// free immediately afterwards.
    pub fn set_params(&mut self, params: &[u16], separators: &[SgrSeparator]) -> Result<()> {
        if params.len() > MAX_SGR_PARAMS {
            return Err(GhosttyError::LimitExceeded {
                resource: "SGR parameters",
                limit: MAX_SGR_PARAMS,
            });
        }
        if !separators.is_empty() && separators.len() != params.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "SGR separators must match the {} parameters, got {}",
                params.len(),
                separators.len()
            )));
        }
        let separator_bytes: Vec<c_char> =
            separators.iter().copied().map(SgrSeparator::byte).collect();
        let separator_pointer = if separator_bytes.is_empty() {
            std::ptr::null()
        } else {
            separator_bytes.as_ptr()
        };
        // SAFETY: both pointers address live slices of `params.len()` items
        // for the duration of the call, and the callee copies them.
        let result = unsafe {
            sys::ghostty_sgr_set_params(
                self.handle.raw(),
                params.as_ptr(),
                separator_pointer,
                params.len(),
            )
        };
        check("sgr_set_params", result)
    }

    /// The next attribute, or `None` once the list is exhausted.
    pub fn next_attribute(&mut self) -> Result<Option<SgrAttribute>> {
        let mut raw = sys::GhosttySgrAttribute {
            tag: sys::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNSET,
            value: sys::GhosttySgrAttributeValue { _padding: [0; 8] },
        };
        // SAFETY: the handle is live and `raw` is valid writable storage.
        if !unsafe { sys::ghostty_sgr_next(self.handle.raw(), &mut raw) } {
            return Ok(None);
        }
        attribute(&mut raw).map(Some)
    }

    /// Parse a full SGR parameter string such as `1;38:2::255:0:0`.
    ///
    /// Empty parameters default to zero, matching how a terminal reads an
    /// omitted CSI parameter.
    pub fn parse(&mut self, text: &str) -> Result<Vec<SgrAttribute>> {
        let mut params = Vec::new();
        let mut separators = Vec::new();
        let mut current = String::new();
        for character in text.chars() {
            match character {
                ';' | ':' => {
                    params.push(current.parse::<u16>().unwrap_or(0));
                    separators.push(if character == ':' {
                        SgrSeparator::Colon
                    } else {
                        SgrSeparator::Semicolon
                    });
                    current.clear();
                }
                _ => current.push(character),
            }
        }
        params.push(current.parse::<u16>().unwrap_or(0));
        separators.push(SgrSeparator::Semicolon);

        self.set_params(&params, &separators)?;
        let mut attributes = Vec::new();
        while let Some(attribute) = self.next_attribute()? {
            attributes.push(attribute);
        }
        Ok(attributes)
    }
}

fn attribute(raw: &mut sys::GhosttySgrAttribute) -> Result<SgrAttribute> {
    // Read the tag and value through the accessors rather than the struct
    // fields: they are the documented entry points and keep this code honest
    // if the layout ever moves.
    // SAFETY: `raw` was filled by `ghostty_sgr_next` and is live here.
    let tag = unsafe { sys::ghostty_sgr_attribute_tag(*raw) };
    // SAFETY: the pointer borrows `raw` for this statement only, and the tag
    // selects which union field is active below.
    let value = unsafe { &*sys::ghostty_sgr_attribute_value(raw) };
    use sys as s;
    Ok(match tag {
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNSET => SgrAttribute::Unset,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNKNOWN => {
            // SAFETY: the tag says the `unknown` field is active.
            let unknown = unsafe { value.unknown };
            SgrAttribute::Unknown {
                full: unknown_params(unknown, sys::ghostty_sgr_unknown_full)?,
                partial: unknown_params(unknown, sys::ghostty_sgr_unknown_partial)?,
            }
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BOLD => SgrAttribute::Bold,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_BOLD => SgrAttribute::ResetBold,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_ITALIC => SgrAttribute::Italic,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_ITALIC => SgrAttribute::ResetItalic,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_FAINT => SgrAttribute::Faint,
        // SAFETY: the tag says the `underline` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNDERLINE => {
            SgrAttribute::Underline(underline(unsafe { value.underline })?)
        }
        // SAFETY: the tag says the `underline_color` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNDERLINE_COLOR => {
            SgrAttribute::UnderlineColor(unsafe { value.underline_color }.into())
        }
        // SAFETY: the tag says the `underline_color_256` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNDERLINE_COLOR_256 => {
            SgrAttribute::UnderlineColor256(unsafe { value.underline_color_256 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_UNDERLINE_COLOR => {
            SgrAttribute::ResetUnderlineColor
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_OVERLINE => SgrAttribute::Overline,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_OVERLINE => SgrAttribute::ResetOverline,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BLINK => SgrAttribute::Blink,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_BLINK => SgrAttribute::ResetBlink,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_INVERSE => SgrAttribute::Inverse,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_INVERSE => SgrAttribute::ResetInverse,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_INVISIBLE => SgrAttribute::Invisible,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_INVISIBLE => SgrAttribute::ResetInvisible,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_STRIKETHROUGH => SgrAttribute::Strikethrough,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_STRIKETHROUGH => {
            SgrAttribute::ResetStrikethrough
        }
        // SAFETY: the tag says the `direct_color_fg` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_DIRECT_COLOR_FG => {
            SgrAttribute::DirectColorFg(unsafe { value.direct_color_fg }.into())
        }
        // SAFETY: the tag says the `direct_color_bg` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_DIRECT_COLOR_BG => {
            SgrAttribute::DirectColorBg(unsafe { value.direct_color_bg }.into())
        }
        // SAFETY: the tag says the `bg_8` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BG_8 => {
            SgrAttribute::Bg8(unsafe { value.bg_8 })
        }
        // SAFETY: the tag says the `fg_8` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_FG_8 => {
            SgrAttribute::Fg8(unsafe { value.fg_8 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_FG => SgrAttribute::ResetFg,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_BG => SgrAttribute::ResetBg,
        // SAFETY: the tag says the `bright_bg_8` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BRIGHT_BG_8 => {
            SgrAttribute::BrightBg8(unsafe { value.bright_bg_8 })
        }
        // SAFETY: the tag says the `bright_fg_8` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BRIGHT_FG_8 => {
            SgrAttribute::BrightFg8(unsafe { value.bright_fg_8 })
        }
        // SAFETY: the tag says the `bg_256` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BG_256 => {
            SgrAttribute::Bg256(unsafe { value.bg_256 })
        }
        // SAFETY: the tag says the `fg_256` field is active.
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_FG_256 => {
            SgrAttribute::Fg256(unsafe { value.fg_256 })
        }
        other => {
            return Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty SGR attribute tag {other}"
            )));
        }
    })
}

fn unknown_params(
    unknown: sys::GhosttySgrUnknown,
    read: unsafe extern "C" fn(sys::GhosttySgrUnknown, *mut *const u16) -> usize,
) -> Result<Vec<u16>> {
    let mut pointer: *const u16 = std::ptr::null();
    // SAFETY: `unknown` is a live descriptor and `pointer` is valid writable
    // storage for the out-parameter.
    let len = unsafe { read(unknown, &mut pointer) };
    if len == 0 {
        return Ok(Vec::new());
    }
    if pointer.is_null() {
        return Err(GhosttyError::AbiMismatch(
            "SGR unknown parameters reported a length with a null pointer".into(),
        ));
    }
    if len > MAX_SGR_PARAMS {
        return Err(GhosttyError::LimitExceeded {
            resource: "SGR parameters",
            limit: MAX_SGR_PARAMS,
        });
    }
    // SAFETY: the library reported `len` readable `u16` values at `pointer`,
    // owned by the parser and valid until the next call on it.
    Ok(unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<SgrAttribute> {
        let mut parser = SgrParser::new().expect("parser must initialize");
        parser.parse(text).expect("parameters must parse")
    }

    #[test]
    fn semicolon_separated_attributes_parse_in_order() {
        assert_eq!(
            parse("1;3;31"),
            vec![SgrAttribute::Bold, SgrAttribute::Italic, SgrAttribute::Fg8(1)]
        );
        assert_eq!(parse("0"), vec![SgrAttribute::Unset]);
    }

    #[test]
    fn colon_separators_bind_underline_shape_and_direct_color() {
        assert_eq!(
            parse("4:3"),
            vec![SgrAttribute::Underline(UnderlineStyle::Curly)]
        );
        assert_eq!(
            parse("4;3"),
            vec![
                SgrAttribute::Underline(UnderlineStyle::Single),
                SgrAttribute::Italic
            ]
        );
        assert_eq!(
            parse("38;2;255;0;0"),
            vec![SgrAttribute::DirectColorFg(Rgb { r: 255, g: 0, b: 0 })]
        );
        assert_eq!(parse("48;5;12"), vec![SgrAttribute::Bg256(12)]);
    }

    #[test]
    fn reset_forms_and_bright_colors_are_distinguished() {
        assert_eq!(
            parse("22;39;49;92;102"),
            vec![
                SgrAttribute::ResetBold,
                SgrAttribute::ResetFg,
                SgrAttribute::ResetBg,
                // Bright colors report their resolved palette index, not
                // the 0-7 offset within the bright range.
                SgrAttribute::BrightFg8(10),
                SgrAttribute::BrightBg8(10),
            ]
        );
    }

    #[test]
    fn an_unparseable_run_reports_its_parameters() {
        let attributes = parse("38;2");
        let unknown = attributes.iter().find_map(|attribute| match attribute {
            SgrAttribute::Unknown { full, partial } => Some((full, partial)),
            _ => None,
        });
        let (full, partial) = unknown.unwrap_or_else(|| {
            unreachable!("an incomplete direct color must parse as unknown: {attributes:?}")
        });
        assert_eq!(full, &[38, 2]);
        assert!(!partial.is_empty());
    }

    #[test]
    fn reset_replays_the_same_parameter_list() {
        let mut parser = SgrParser::new().expect("parser must initialize");
        parser.set_params(&[1], &[]).expect("params must load");
        assert_eq!(
            parser.next_attribute().expect("first pass"),
            Some(SgrAttribute::Bold)
        );
        assert_eq!(parser.next_attribute().expect("exhausted"), None);
        parser.reset();
        assert_eq!(
            parser.next_attribute().expect("second pass"),
            Some(SgrAttribute::Bold)
        );
    }

    #[test]
    fn a_mismatched_separator_list_is_rejected() {
        let mut parser = SgrParser::new().expect("parser must initialize");
        let error = parser
            .set_params(&[1, 2], &[SgrSeparator::Colon])
            .expect_err("length mismatch must be rejected");
        assert!(matches!(error, GhosttyError::AbiMismatch(_)));
    }
}
