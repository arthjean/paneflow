use std::ops::Range;

pub(crate) fn byte_offset_from_utf16(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for ch in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += ch.len_utf16();
        utf8_offset += ch.len_utf8();
    }
    utf8_offset
}

pub(crate) fn byte_range_from_utf16(text: &str, range_utf16: &Range<usize>) -> Range<usize> {
    byte_offset_from_utf16(text, range_utf16.start)..byte_offset_from_utf16(text, range_utf16.end)
}

pub(crate) fn utf16_offset_from_byte(text: &str, offset: usize) -> usize {
    let mut utf16_offset = 0;
    let mut utf8_count = 0;
    for ch in text.chars() {
        if utf8_count >= offset {
            break;
        }
        utf8_count += ch.len_utf8();
        utf16_offset += ch.len_utf16();
    }
    utf16_offset
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_range_conversion_handles_surrogate_pairs() {
        assert_eq!(byte_range_from_utf16("a😀b", &(1..3)), 1..5);
    }

    #[test]
    fn utf16_range_conversion_clamps_to_text_end() {
        assert_eq!(byte_range_from_utf16("é", &(0..99)), 0..2);
    }

    #[test]
    fn a_caret_after_an_emoji_or_cjk_character_round_trips_to_the_same_byte() {
        for text in ["😀x", "漢字x", "a😀漢b"] {
            let caret = text.len() - 1;
            let caret_utf16 = utf16_offset_from_byte(text, caret);
            assert_eq!(byte_offset_from_utf16(text, caret_utf16), caret);
            let mut inserted = text.to_string();
            inserted.insert(byte_offset_from_utf16(text, caret_utf16), 'あ');
            assert!(inserted.ends_with("あx") || inserted.ends_with("あb"));
        }
        assert_eq!(utf16_offset_from_byte("😀x", 4), 2);
        assert_eq!(utf16_offset_from_byte("漢字x", 6), 2);
    }
}
