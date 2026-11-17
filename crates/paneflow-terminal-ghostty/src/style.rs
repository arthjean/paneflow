use paneflow_libghostty_sys as sys;

use crate::{CellAttribute, Color, GhosttyError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FlagField {
    pub(crate) name: &'static str,
    pub(crate) lsb: u32,
    pub(crate) width: u32,
}

impl FlagField {
    const fn bit(name: &'static str, lsb: u32) -> Self {
        Self {
            name,
            lsb,
            width: 1,
        }
    }

    const fn mask(self) -> sys::GhosttyStyleFlags {
        (((1u32 << self.width) - 1) << self.lsb) as sys::GhosttyStyleFlags
    }

    const fn read(self, flags: sys::GhosttyStyleFlags) -> u32 {
        (flags & self.mask()) as u32 >> self.lsb
    }
}

pub(crate) const BOLD: FlagField = FlagField::bit("bold", 0);
pub(crate) const ITALIC: FlagField = FlagField::bit("italic", 1);
pub(crate) const FAINT: FlagField = FlagField::bit("faint", 2);
pub(crate) const BLINK: FlagField = FlagField::bit("blink", 3);
pub(crate) const INVERSE: FlagField = FlagField::bit("inverse", 4);
pub(crate) const INVISIBLE: FlagField = FlagField::bit("invisible", 5);
pub(crate) const STRIKETHROUGH: FlagField = FlagField::bit("strikethrough", 6);
pub(crate) const OVERLINE: FlagField = FlagField::bit("overline", 7);
pub(crate) const UNDERLINE: FlagField = FlagField {
    name: "underline",
    lsb: sys::GHOSTTY_STYLE_UNDERLINE_SHIFT,
    width: 3,
};

const FLAG_FIELDS: [FlagField; 9] = [
    BOLD,
    ITALIC,
    FAINT,
    BLINK,
    INVERSE,
    INVISIBLE,
    STRIKETHROUGH,
    OVERLINE,
    UNDERLINE,
];

const ATTRIBUTE_FIELDS: [(FlagField, CellAttribute); 7] = [
    (BOLD, CellAttribute::Bold),
    (ITALIC, CellAttribute::Italic),
    (FAINT, CellAttribute::Dim),
    (INVERSE, CellAttribute::Inverse),
    (INVISIBLE, CellAttribute::Invisible),
    (STRIKETHROUGH, CellAttribute::Strikethrough),
    (OVERLINE, CellAttribute::Overline),
];

const ATTRIBUTE_MASK: sys::GhosttyStyleFlags = {
    let mut mask = 0;
    let mut index = 0;
    while index < ATTRIBUTE_FIELDS.len() {
        let (field, attribute) = ATTRIBUTE_FIELDS[index];
        assert!(field.width == 1 && field.lsb == attribute as u32);
        mask |= field.mask();
        index += 1;
    }
    mask
};

#[inline]
pub(crate) const fn attribute_bits(flags: sys::GhosttyStyleFlags) -> u8 {
    (flags & ATTRIBUTE_MASK) as u8
}

#[inline]
pub(crate) const fn underline_bits(flags: sys::GhosttyStyleFlags) -> sys::GhosttySgrUnderline {
    UNDERLINE.read(flags) as sys::GhosttySgrUnderline
}

pub(crate) fn validate_flag_bits(layouts: &serde_json::Value) -> Result<()> {
    let flags = layouts.get("GhosttyStyleFlags");
    let kind = flags.and_then(|value| value.get("kind")).and_then(serde_json::Value::as_str);
    let bits = flags.and_then(|value| value.get("bits"));
    let (Some("packed"), Some(bits)) = (kind, bits) else {
        return Err(GhosttyError::AbiMismatch(
            "GhosttyStyleFlags is not described as a packed bit set".into(),
        ));
    };
    let published = bits.as_object().map_or(0, serde_json::Map::len);
    if published != FLAG_FIELDS.len() {
        return Err(GhosttyError::AbiMismatch(format!(
            "GhosttyStyleFlags publishes {published} fields, expected {}",
            FLAG_FIELDS.len()
        )));
    }
    for field in FLAG_FIELDS {
        let entry = bits.get(field.name);
        let read = |key| {
            entry
                .and_then(|value| value.get(key))
                .and_then(serde_json::Value::as_u64)
        };
        let actual = (read("lsb"), read("width"));
        if actual != (Some(u64::from(field.lsb)), Some(u64::from(field.width))) {
            return Err(GhosttyError::AbiMismatch(format!(
                "GhosttyStyleFlags.{} lsb/width expected {}/{}, got {:?}/{:?}",
                field.name, field.lsb, field.width, actual.0, actual.1
            )));
        }
    }
    Ok(())
}

pub(crate) fn style_color(color: sys::GhosttyStyleColor) -> Result<Color> {
    match sys::GhosttyStyleColorTag::from(color.tag) {
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_NONE => Ok(Color::Default),
        sys::GhosttyStyleColorTag_GHOSTTY_STYLE_COLOR_PALETTE => {
            Ok(Color::Palette(unsafe { color.value.palette }))
        }
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

    const ATTRIBUTES: [CellAttribute; 7] = [
        CellAttribute::Bold,
        CellAttribute::Italic,
        CellAttribute::Dim,
        CellAttribute::Inverse,
        CellAttribute::Invisible,
        CellAttribute::Strikethrough,
        CellAttribute::Overline,
    ];

    fn attributes_of(flags: sys::GhosttyStyleFlags) -> Vec<CellAttribute> {
        let cell = crate::CellFlags::from_attribute_bits(
            attribute_bits(flags),
            crate::UnderlineStyle::None,
        );
        ATTRIBUTES
            .into_iter()
            .filter(|&attribute| cell.has(attribute))
            .collect()
    }

    #[test]
    fn every_attribute_reads_its_own_bit() {
        for (field, attribute) in ATTRIBUTE_FIELDS {
            assert_eq!(
                attributes_of(field.mask()),
                [attribute],
                "{} must set exactly one attribute",
                field.name
            );
            assert_eq!(underline_bits(field.mask()), 0);
        }
        assert!(attributes_of(BLINK.mask()).is_empty(), "blink is not drawn");
    }

    #[test]
    fn the_underline_style_reads_three_bits_above_the_attributes() {
        let curly = sys::GhosttySgrUnderline_GHOSTTY_SGR_UNDERLINE_CURLY;
        let flags = ((curly as sys::GhosttyStyleFlags) << sys::GHOSTTY_STYLE_UNDERLINE_SHIFT)
            | 0xff
            | 0xf800;
        assert_eq!(underline_bits(flags), curly);
        assert_eq!(attributes_of(flags), ATTRIBUTES);
    }

    #[test]
    fn a_moved_flag_bit_is_named_in_the_mismatch() {
        let json = unsafe { std::ffi::CStr::from_ptr(sys::ghostty_type_json()) }
            .to_str()
            .expect("layout JSON is UTF-8");
        let mut layouts: serde_json::Value =
            serde_json::from_str::<serde_json::Value>(json).expect("layout JSON must parse")["types"].clone();
        validate_flag_bits(&layouts).expect("pinned flag bits must validate");
        layouts["GhosttyStyleFlags"]["bits"]["strikethrough"]["lsb"] = serde_json::Value::from(9);
        let error = validate_flag_bits(&layouts).expect_err("a moved bit must not validate");
        assert!(
            matches!(&error, GhosttyError::AbiMismatch(message) if message.starts_with("GhosttyStyleFlags.strikethrough")),
            "{error:?}"
        );
    }
}
