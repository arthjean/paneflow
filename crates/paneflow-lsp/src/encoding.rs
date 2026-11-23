use ropey::Rope;

use crate::protocol::{Position, PositionEncoding};

pub fn position_at(text: &Rope, byte: usize, encoding: PositionEncoding) -> Position {
    let byte = byte.min(text.len_bytes());
    let line = text.byte_to_line(byte);
    let line_start = text.line_to_byte(line);
    let character = match encoding {
        PositionEncoding::Utf8 => byte - line_start,
        PositionEncoding::Utf16 => {
            let start_char = text.byte_to_char(line_start);
            let end_char = text.byte_to_char(byte);
            text.slice(start_char..end_char)
                .chars()
                .map(char::len_utf16)
                .sum()
        }
    };
    Position {
        line: u32::try_from(line).unwrap_or(u32::MAX),
        character: u32::try_from(character).unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_counts_a_character_outside_the_bmp_as_two_units() {
        let text = Rope::from_str("first\nlet 🦀 = x;\n");
        let after_crab = "first\nlet 🦀".len();
        assert_eq!(
            position_at(&text, after_crab, PositionEncoding::Utf16),
            Position {
                line: 1,
                character: 6
            }
        );
        assert_eq!(
            position_at(&text, after_crab, PositionEncoding::Utf8),
            Position {
                line: 1,
                character: 8
            }
        );
    }

    #[test]
    fn positions_past_the_end_clamp_to_the_last_line() {
        let text = Rope::from_str("ab\ncd");
        assert_eq!(
            position_at(&text, 99, PositionEncoding::Utf8),
            Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(
            position_at(&text, 3, PositionEncoding::Utf16),
            Position {
                line: 1,
                character: 0
            }
        );
    }
}
