use std::ffi::c_char;

use paneflow_libghostty_sys as sys;

use crate::handles::{OwnedHandle, check};
use crate::snapshot_ffi::underline;
use crate::{GhosttyError, Result, Rgb, UnderlineStyle};

const MAX_SGR_PARAMS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SgrSeparator {
    #[default]
    Semicolon,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SgrAttribute {
    Unset,
    Unknown {
        full: Vec<u16>,
        partial: Vec<u16>,
    },
    Bold,
    ResetBold,
    Italic,
    ResetItalic,
    Faint,
    Underline(UnderlineStyle),
    UnderlineColor(Rgb),
    UnderlineColor256(u8),
    ResetUnderlineColor,
    Overline,
    ResetOverline,
    Blink,
    ResetBlink,
    Inverse,
    ResetInverse,
    Invisible,
    ResetInvisible,
    Strikethrough,
    ResetStrikethrough,
    DirectColorFg(Rgb),
    DirectColorBg(Rgb),
    Bg8(u8),
    Fg8(u8),
    ResetFg,
    ResetBg,
    BrightBg8(u8),
    BrightFg8(u8),
    Bg256(u8),
    Fg256(u8),
}

pub struct SgrParser {
    handle: OwnedHandle<sys::GhosttySgrParser>,
}

impl SgrParser {
    pub fn new() -> Result<Self> {
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

    pub fn reset(&mut self) {
        unsafe { sys::ghostty_sgr_reset(self.handle.raw()) };
    }

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

    pub fn next_attribute(&mut self) -> Result<Option<SgrAttribute>> {
        let mut raw = sys::GhosttySgrAttribute {
            tag: sys::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNSET,
            value: sys::GhosttySgrAttributeValue { _padding: [0; 8] },
        };
        if !unsafe { sys::ghostty_sgr_next(self.handle.raw(), &mut raw) } {
            return Ok(None);
        }
        attribute(&mut raw).map(Some)
    }

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
    let tag = unsafe { sys::ghostty_sgr_attribute_tag(*raw) };
    let value = unsafe { &*sys::ghostty_sgr_attribute_value(raw) };
    use sys as s;
    Ok(match tag {
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNSET => SgrAttribute::Unset,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNKNOWN => {
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
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNDERLINE => {
            SgrAttribute::Underline(underline(unsafe { value.underline })?)
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_UNDERLINE_COLOR => {
            SgrAttribute::UnderlineColor(unsafe { value.underline_color }.into())
        }
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
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_DIRECT_COLOR_FG => {
            SgrAttribute::DirectColorFg(unsafe { value.direct_color_fg }.into())
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_DIRECT_COLOR_BG => {
            SgrAttribute::DirectColorBg(unsafe { value.direct_color_bg }.into())
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BG_8 => {
            SgrAttribute::Bg8(unsafe { value.bg_8 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_FG_8 => {
            SgrAttribute::Fg8(unsafe { value.fg_8 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_FG => SgrAttribute::ResetFg,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_RESET_BG => SgrAttribute::ResetBg,
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BRIGHT_BG_8 => {
            SgrAttribute::BrightBg8(unsafe { value.bright_bg_8 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BRIGHT_FG_8 => {
            SgrAttribute::BrightFg8(unsafe { value.bright_fg_8 })
        }
        s::GhosttySgrAttributeTag_GHOSTTY_SGR_ATTR_BG_256 => {
            SgrAttribute::Bg256(unsafe { value.bg_256 })
        }
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
