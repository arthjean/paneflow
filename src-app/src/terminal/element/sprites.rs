//! Glyphs the renderer draws itself instead of asking the font.
//!
//! Box drawing, block shades, braille, and the geometric Powerline symbols
//! must meet at exact cell boundaries and share one stroke thickness, which
//! font outlines cannot guarantee across fallback faces and fractional
//! scales. This table decides which codepoints the renderer owns and how;
//! `paint/sprites.rs` draws them. Coverage and geometry follow Ghostty's
//! sprite font (`src/font/sprite/draw/`, MIT licensed, Mitchell Hashimoto
//! and contributors).

/// Stroke style of one arm of a box-drawing character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arm {
    None,
    Light,
    Heavy,
    Double,
}

impl Arm {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            1 => Arm::Light,
            2 => Arm::Heavy,
            3 => Arm::Double,
            _ => Arm::None,
        }
    }
}

/// The four arms of a straight box-drawing character, each from the cell
/// edge to the center.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Lines {
    pub up: Arm,
    pub right: Arm,
    pub down: Arm,
    pub left: Arm,
}

/// Which corner an arc character (`╭ ╮ ╯ ╰`) opens toward, named by the
/// cell corner the two straight segments point away from, as in Ghostty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Gap requested between the dashes of a dashed line, before the width cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DashGap {
    /// At least four pixels, or the light thickness if that is larger.
    AtLeastFour,
    Light,
    Heavy,
}

/// Density of a shade block, as a fraction of the foreground alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shade {
    Light,
    Medium,
    Dark,
}

impl Shade {
    pub fn alpha(self) -> f32 {
        match self {
            Shade::Light => 0.25,
            Shade::Medium => 0.5,
            Shade::Dark => 0.75,
        }
    }
}

/// Powerline symbols with a geometric definition.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Powerline {
    /// U+E0B0: right-pointing filled triangle.
    RightTriangle,
    /// U+E0B1: right-pointing chevron.
    RightChevron,
    /// U+E0B2: left-pointing filled triangle.
    LeftTriangle,
    /// U+E0B3: left-pointing chevron.
    LeftChevron,
    /// U+E0B4: right half circle, filled.
    RightHalfCircle,
    /// U+E0B5: right half circle, outline.
    RightHalfCircleOutline,
    /// U+E0B6: left half circle, filled.
    LeftHalfCircle,
    /// U+E0B7: left half circle, outline.
    LeftHalfCircleOutline,
    /// U+E0B8: lower-left filled triangle.
    LowerLeftTriangle,
    /// U+E0BA: lower-right filled triangle.
    LowerRightTriangle,
    /// U+E0BC: upper-left filled triangle.
    UpperLeftTriangle,
    /// U+E0BE: upper-right filled triangle.
    UpperRightTriangle,
    /// U+E0D2: left-pointing trapezoid pair.
    LeftTrapezoid,
    /// U+E0D4: right-pointing trapezoid pair.
    RightTrapezoid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sprite {
    Lines(Lines),
    DashHorizontal {
        count: u8,
        heavy: bool,
        gap: DashGap,
    },
    DashVertical {
        count: u8,
        heavy: bool,
        gap: DashGap,
    },
    Arc(Corner),
    /// U+2571 `╱`.
    DiagonalUpperRightToLowerLeft,
    /// U+2572 `╲`.
    DiagonalUpperLeftToLowerRight,
    /// U+2573 `╳`.
    DiagonalCross,
    /// `░ ▒ ▓`: the full cell at a fraction of the foreground alpha.
    Shade(Shade),
    /// U+2800..U+28FF, the low byte of the codepoint.
    Braille(u8),
    Powerline(Powerline),
}

/// Arms of U+2500..U+257F, packed two bits per arm (`up | right << 2 |
/// down << 4 | left << 6`; 0 none, 1 light, 2 heavy, 3 double). Generated
/// from the `linesChar` calls in Ghostty's `box.zig`; the zero entries are
/// dashes, arcs, and diagonals, which the match below handles.
const BOX_ARMS: [u8; 128] = [
    0x44, 0x88, 0x11, 0x22, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x18, 0x24,
    0x28, // U+2500
    0x50, 0x90, 0x60, 0xa0, 0x05, 0x09, 0x06, 0x0a, 0x41, 0x81, 0x42, 0x82, 0x15, 0x19, 0x16,
    0x25, // U+2510
    0x26, 0x1a, 0x29, 0x2a, 0x51, 0x91, 0x52, 0x61, 0x62, 0x92, 0xa1, 0xa2, 0x54, 0x94, 0x58,
    0x98, // U+2520
    0x64, 0xa4, 0x68, 0xa8, 0x45, 0x85, 0x49, 0x89, 0x46, 0x86, 0x4a, 0x8a, 0x55, 0x95, 0x59,
    0x99, // U+2530
    0x56, 0x65, 0x66, 0x96, 0x5a, 0xa5, 0x69, 0x9a, 0xa9, 0xa6, 0x6a, 0xaa, 0x00, 0x00, 0x00,
    0x00, // U+2540
    0xcc, 0x33, 0x1c, 0x34, 0x3c, 0xd0, 0x70, 0xf0, 0x0d, 0x07, 0x0f, 0xc1, 0x43, 0xc3, 0x1d,
    0x37, // U+2550
    0x3f, 0xd1, 0x73, 0xf3, 0xdc, 0x74, 0xfc, 0xcd, 0x47, 0xcf, 0xdd, 0x77, 0xff, 0x00, 0x00,
    0x00, // U+2560
    0x00, 0x00, 0x00, 0x00, 0x40, 0x01, 0x04, 0x10, 0x80, 0x02, 0x08, 0x20, 0x48, 0x21, 0x84,
    0x12, // U+2570
];

fn lines_from_bits(bits: u8) -> Lines {
    Lines {
        up: Arm::from_bits(bits),
        right: Arm::from_bits(bits >> 2),
        down: Arm::from_bits(bits >> 4),
        left: Arm::from_bits(bits >> 6),
    }
}

/// The sprite drawn for `c`, or `None` when the font owns the glyph.
pub(super) fn sprite_for(c: char) -> Option<Sprite> {
    let cp = c as u32;
    let dash = |vertical: bool, count: u8, heavy: bool, gap: DashGap| {
        Some(if vertical {
            Sprite::DashVertical { count, heavy, gap }
        } else {
            Sprite::DashHorizontal { count, heavy, gap }
        })
    };
    match cp {
        0x2504 => dash(false, 3, false, DashGap::AtLeastFour),
        0x2505 => dash(false, 3, true, DashGap::AtLeastFour),
        0x2506 => dash(true, 3, false, DashGap::AtLeastFour),
        0x2507 => dash(true, 3, true, DashGap::AtLeastFour),
        0x2508 => dash(false, 4, false, DashGap::AtLeastFour),
        0x2509 => dash(false, 4, true, DashGap::AtLeastFour),
        0x250a => dash(true, 4, false, DashGap::AtLeastFour),
        0x250b => dash(true, 4, true, DashGap::AtLeastFour),
        0x254c => dash(false, 2, false, DashGap::Light),
        0x254d => dash(false, 2, true, DashGap::Heavy),
        0x254e => dash(true, 2, false, DashGap::Heavy),
        0x254f => dash(true, 2, true, DashGap::Heavy),
        0x256d => Some(Sprite::Arc(Corner::BottomRight)),
        0x256e => Some(Sprite::Arc(Corner::BottomLeft)),
        0x256f => Some(Sprite::Arc(Corner::TopLeft)),
        0x2570 => Some(Sprite::Arc(Corner::TopRight)),
        0x2571 => Some(Sprite::DiagonalUpperRightToLowerLeft),
        0x2572 => Some(Sprite::DiagonalUpperLeftToLowerRight),
        0x2573 => Some(Sprite::DiagonalCross),
        0x2500..=0x257f => {
            let bits = BOX_ARMS[(cp - 0x2500) as usize];
            (bits != 0).then(|| Sprite::Lines(lines_from_bits(bits)))
        }
        0x2591 => Some(Sprite::Shade(Shade::Light)),
        0x2592 => Some(Sprite::Shade(Shade::Medium)),
        0x2593 => Some(Sprite::Shade(Shade::Dark)),
        0x2800..=0x28ff => Some(Sprite::Braille((cp & 0xff) as u8)),
        0xe0b0 => Some(Sprite::Powerline(Powerline::RightTriangle)),
        0xe0b1 => Some(Sprite::Powerline(Powerline::RightChevron)),
        0xe0b2 => Some(Sprite::Powerline(Powerline::LeftTriangle)),
        0xe0b3 => Some(Sprite::Powerline(Powerline::LeftChevron)),
        0xe0b4 => Some(Sprite::Powerline(Powerline::RightHalfCircle)),
        0xe0b5 => Some(Sprite::Powerline(Powerline::RightHalfCircleOutline)),
        0xe0b6 => Some(Sprite::Powerline(Powerline::LeftHalfCircle)),
        0xe0b7 => Some(Sprite::Powerline(Powerline::LeftHalfCircleOutline)),
        0xe0b8 => Some(Sprite::Powerline(Powerline::LowerLeftTriangle)),
        0xe0b9 => Some(Sprite::DiagonalUpperLeftToLowerRight),
        0xe0ba => Some(Sprite::Powerline(Powerline::LowerRightTriangle)),
        0xe0bb => Some(Sprite::DiagonalUpperRightToLowerLeft),
        0xe0bc => Some(Sprite::Powerline(Powerline::UpperLeftTriangle)),
        0xe0bd => Some(Sprite::DiagonalUpperRightToLowerLeft),
        0xe0be => Some(Sprite::Powerline(Powerline::UpperRightTriangle)),
        0xe0bf => Some(Sprite::DiagonalUpperLeftToLowerRight),
        0xe0d2 => Some(Sprite::Powerline(Powerline::LeftTrapezoid)),
        0xe0d4 => Some(Sprite::Powerline(Powerline::RightTrapezoid)),
        _ => None,
    }
}

/// Private Use Area codepoints, where Nerd Fonts put their icons. These get
/// the icon constraint (`paint/text.rs`) instead of the font's raw placement.
pub(super) fn is_private_use(c: char) -> bool {
    matches!(c as u32, 0xe000..=0xf8ff | 0xf0000..=0xffffd | 0x100000..=0x10fffd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_box_drawing_codepoint_is_covered() {
        for cp in 0x2500u32..=0x257f {
            let c = char::from_u32(cp).unwrap();
            assert!(sprite_for(c).is_some(), "U+{cp:04X} has no sprite");
        }
    }

    #[test]
    fn box_arms_match_the_unicode_names() {
        let lines = |c: char| match sprite_for(c) {
            Some(Sprite::Lines(lines)) => lines,
            other => panic!("{c} should be straight lines, got {other:?}"),
        };
        assert_eq!(
            lines('─'),
            Lines {
                up: Arm::None,
                right: Arm::Light,
                down: Arm::None,
                left: Arm::Light
            }
        );
        assert_eq!(
            lines('┃'),
            Lines {
                up: Arm::Heavy,
                right: Arm::None,
                down: Arm::Heavy,
                left: Arm::None
            }
        );
        assert_eq!(
            lines('╋'),
            Lines {
                up: Arm::Heavy,
                right: Arm::Heavy,
                down: Arm::Heavy,
                left: Arm::Heavy
            }
        );
        assert_eq!(
            lines('╬'),
            Lines {
                up: Arm::Double,
                right: Arm::Double,
                down: Arm::Double,
                left: Arm::Double
            }
        );
        // '╒' BOX DRAWINGS DOWN SINGLE AND RIGHT DOUBLE
        assert_eq!(
            lines('╒'),
            Lines {
                up: Arm::None,
                right: Arm::Double,
                down: Arm::Light,
                left: Arm::None
            }
        );
        // '╿' BOX DRAWINGS HEAVY UP AND LIGHT DOWN
        assert_eq!(
            lines('╿'),
            Lines {
                up: Arm::Heavy,
                right: Arm::None,
                down: Arm::Light,
                left: Arm::None
            }
        );
    }

    #[test]
    fn dashes_arcs_and_diagonals_have_their_own_sprites() {
        assert_eq!(
            sprite_for('┄'),
            Some(Sprite::DashHorizontal {
                count: 3,
                heavy: false,
                gap: DashGap::AtLeastFour
            })
        );
        assert_eq!(
            sprite_for('╏'),
            Some(Sprite::DashVertical {
                count: 2,
                heavy: true,
                gap: DashGap::Heavy
            })
        );
        assert_eq!(sprite_for('╭'), Some(Sprite::Arc(Corner::BottomRight)));
        assert_eq!(sprite_for('╯'), Some(Sprite::Arc(Corner::TopLeft)));
        assert_eq!(sprite_for('╳'), Some(Sprite::DiagonalCross));
    }

    #[test]
    fn shades_braille_and_powerline_are_sprites() {
        assert_eq!(sprite_for('░'), Some(Sprite::Shade(Shade::Light)));
        assert_eq!(sprite_for('▓'), Some(Sprite::Shade(Shade::Dark)));
        assert_eq!(Shade::Medium.alpha(), 0.5);
        assert_eq!(sprite_for('⣿'), Some(Sprite::Braille(0xff)));
        assert_eq!(sprite_for('⠁'), Some(Sprite::Braille(0x01)));
        assert_eq!(
            sprite_for('\u{e0b0}'),
            Some(Sprite::Powerline(Powerline::RightTriangle))
        );
        assert_eq!(
            sprite_for('\u{e0b7}'),
            Some(Sprite::Powerline(Powerline::LeftHalfCircleOutline))
        );
    }

    #[test]
    fn text_and_block_elements_stay_with_their_own_paths() {
        for c in ['a', '█', '▀', '▖', '■', '●', '\u{e0c0}', '\u{f09b}'] {
            assert!(sprite_for(c).is_none(), "{c} must not be a sprite");
        }
    }

    #[test]
    fn private_use_covers_the_nerd_font_planes() {
        assert!(is_private_use('\u{e62b}'));
        assert!(is_private_use('\u{f09b}'));
        assert!(is_private_use('\u{f0001}'));
        assert!(!is_private_use('a'));
        assert!(!is_private_use('█'));
    }
}
