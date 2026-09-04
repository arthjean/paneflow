use crate::iterable::Range;
use crate::ComparisonPolicy;

pub(crate) const UNIMPORTANT_LINE_CHAR_COUNT: usize = 3;

pub(crate) fn is_whitespace_byte(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

pub(crate) fn is_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

pub(crate) fn is_punctuation_byte(byte: u8) -> bool {
    if byte == b'_' {
        return false;
    }
    (33..=47).contains(&byte)
        || (58..=64).contains(&byte)
        || (91..=96).contains(&byte)
        || (123..=126).contains(&byte)
}

pub(crate) fn is_punctuation(c: char) -> bool {
    c.is_ascii() && is_punctuation_byte(c as u8)
}

pub(crate) fn is_alpha(c: char) -> bool {
    !is_whitespace(c) && !is_punctuation(c)
}

pub(crate) fn is_continuous_script(c: char) -> bool {
    let code = c as u32;
    if code < 128 {
        return false;
    }
    if in_ranges(code, &DECIMAL_DIGIT_STARTS, &DECIMAL_DIGIT_ENDS) {
        return false;
    }
    if code > 0xFFFF {
        return true;
    }
    if in_ranges(code, &IDEOGRAPHIC_STARTS, &IDEOGRAPHIC_ENDS) {
        return true;
    }
    if !c.is_alphabetic() {
        return true;
    }
    in_ranges(code, &HIRAGANA_STARTS, &HIRAGANA_ENDS)
        || in_ranges(code, &KATAKANA_STARTS, &KATAKANA_ENDS)
        || in_ranges(code, &THAI_STARTS, &THAI_ENDS)
        || in_ranges(code, &JAVANESE_STARTS, &JAVANESE_ENDS)
}

fn in_ranges(code: u32, starts: &[u32], ends: &[u32]) -> bool {
    match starts.binary_search(&code) {
        Ok(_) => true,
        Err(0) => false,
        Err(insertion) => code <= ends[insertion - 1],
    }
}

pub(crate) fn is_equals(text1: &str, text2: &str, policy: ComparisonPolicy) -> bool {
    match policy {
        ComparisonPolicy::Default => text1 == text2,
        ComparisonPolicy::TrimWhitespaces => equals_trim_whitespaces(text1, text2),
        ComparisonPolicy::IgnoreWhitespaces => equals_ignore_whitespaces(text1, text2),
    }
}

fn equals_trim_whitespaces(text1: &str, text2: &str) -> bool {
    let mut lines1 = text1.split('\n');
    let mut lines2 = text2.split('\n');
    loop {
        match (lines1.next(), lines2.next()) {
            (Some(line1), Some(line2)) => {
                if trim_line(line1) != trim_line(line2) {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

fn trim_line(line: &str) -> &str {
    line.trim_matches(is_whitespace)
}

fn equals_ignore_whitespaces(text1: &str, text2: &str) -> bool {
    let mut bytes1 = text1.bytes().filter(|byte| !is_whitespace_byte(*byte));
    let mut bytes2 = text2.bytes().filter(|byte| !is_whitespace_byte(*byte));
    loop {
        match (bytes1.next(), bytes2.next()) {
            (Some(a), Some(b)) => {
                if a != b {
                    return false;
                }
            }
            (None, None) => return true,
            _ => return false,
        }
    }
}

pub(crate) fn hash(text: &str, policy: ComparisonPolicy) -> u64 {
    let mut state = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |byte: u8| {
        state ^= u64::from(byte);
        state = state.wrapping_mul(0x0100_0000_01b3);
    };
    match policy {
        ComparisonPolicy::Default => text.bytes().for_each(&mut feed),
        ComparisonPolicy::TrimWhitespaces => trim_line(text).bytes().for_each(&mut feed),
        ComparisonPolicy::IgnoreWhitespaces => text
            .bytes()
            .filter(|byte| !is_whitespace_byte(*byte))
            .for_each(&mut feed),
    }
    state
}

pub(crate) fn count_non_space_chars(text: &str) -> usize {
    text.chars().filter(|c| !is_whitespace(*c)).count()
}

pub(crate) fn trim_start(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut start = start;
    while start < end && is_whitespace_byte(bytes[start]) {
        start += 1;
    }
    start
}

pub(crate) fn trim_end(text: &str, start: usize, end: usize) -> usize {
    let bytes = text.as_bytes();
    let mut end = end;
    while start < end && is_whitespace_byte(bytes[end - 1]) {
        end -= 1;
    }
    end
}

pub(crate) fn trim_range(text1: &str, text2: &str, range: Range) -> Range {
    let start1 = trim_start(text1, range.start1, range.end1);
    let end1 = trim_end(text1, start1, range.end1);
    let start2 = trim_start(text2, range.start2, range.end2);
    let end2 = trim_end(text2, start2, range.end2);
    Range::new(start1, end1, start2, end2)
}

pub(crate) fn expand_whitespaces_forward(
    text1: &str,
    text2: &str,
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let bytes1 = text1.as_bytes();
    let bytes2 = text2.as_bytes();
    let mut count = 0;
    while start1 + count < end1
        && start2 + count < end2
        && bytes1[start1 + count] == bytes2[start2 + count]
        && is_whitespace_byte(bytes1[start1 + count])
    {
        count += 1;
    }
    count
}

pub(crate) fn expand_whitespaces_backward(
    text1: &str,
    text2: &str,
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let bytes1 = text1.as_bytes();
    let bytes2 = text2.as_bytes();
    let mut count = 0;
    while start1 + count < end1
        && start2 + count < end2
        && bytes1[end1 - count - 1] == bytes2[end2 - count - 1]
        && is_whitespace_byte(bytes1[end1 - count - 1])
    {
        count += 1;
    }
    count
}

pub(crate) fn expand_whitespaces(text1: &str, text2: &str, range: Range) -> Range {
    let forward = expand_whitespaces_forward(
        text1,
        text2,
        range.start1,
        range.start2,
        range.end1,
        range.end2,
    );
    let start1 = range.start1 + forward;
    let start2 = range.start2 + forward;
    let backward =
        expand_whitespaces_backward(text1, text2, start1, start2, range.end1, range.end2);
    Range::new(start1, range.end1 - backward, start2, range.end2 - backward)
}

pub(crate) fn is_equals_range(
    text1: &str,
    text2: &str,
    range: Range,
    policy: ComparisonPolicy,
) -> bool {
    is_equals(
        &text1[range.start1..range.end1],
        &text2[range.start2..range.end2],
        policy,
    )
}

pub(crate) fn expand_forward<T: PartialEq>(
    items1: &[T],
    items2: &[T],
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let mut count = 0;
    while start1 + count < end1
        && start2 + count < end2
        && items1[start1 + count] == items2[start2 + count]
    {
        count += 1;
    }
    count
}

pub(crate) fn expand_backward<T: PartialEq>(
    items1: &[T],
    items2: &[T],
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> usize {
    let mut count = 0;
    while start1 + count < end1
        && start2 + count < end2
        && items1[end1 - count - 1] == items2[end2 - count - 1]
    {
        count += 1;
    }
    count
}

pub(crate) fn expand<T: PartialEq>(
    items1: &[T],
    items2: &[T],
    start1: usize,
    start2: usize,
    end1: usize,
    end2: usize,
) -> Range {
    let forward = expand_forward(items1, items2, start1, start2, end1, end2);
    let start1 = start1 + forward;
    let start2 = start2 + forward;
    let backward = expand_backward(items1, items2, start1, start2, end1, end2);
    Range::new(start1, end1 - backward, start2, end2 - backward)
}

const IDEOGRAPHIC_STARTS: [u32; 22] = [
    0x3006, 0x3007, 0x3021, 0x3038, 0x3400, 0x4e00, 0xf900, 0xfa70, 0x16fe4, 0x17000, 0x18800,
    0x18cff, 0x1b170, 0x20000, 0x2a700, 0x2b740, 0x2b820, 0x2ceb0, 0x2ebf0, 0x2f800, 0x30000,
    0x31350,
];

const IDEOGRAPHIC_ENDS: [u32; 22] = [
    0x3006, 0x3007, 0x3029, 0x303a, 0x4dbf, 0x9fff, 0xfa6d, 0xfad9, 0x16fe4, 0x187f7, 0x18cd5,
    0x18d08, 0x1b2fb, 0x2a6df, 0x2b739, 0x2b81d, 0x2cea1, 0x2ebe0, 0x2ee5d, 0x2fa1d, 0x3134a,
    0x323af,
];

const HIRAGANA_STARTS: [u32; 7] = [0x3041, 0x309d, 0x309f, 0x1b001, 0x1b132, 0x1b150, 0x1f200];

const HIRAGANA_ENDS: [u32; 7] = [0x3096, 0x309e, 0x309f, 0x1b11f, 0x1b132, 0x1b152, 0x1f200];

const KATAKANA_STARTS: [u32; 15] = [
    0x30a1, 0x30fd, 0x30ff, 0x31f0, 0x32d0, 0x3300, 0xff66, 0xff71, 0x1aff0, 0x1aff5, 0x1affd,
    0x1b000, 0x1b120, 0x1b155, 0x1b164,
];

const KATAKANA_ENDS: [u32; 15] = [
    0x30fa, 0x30fe, 0x30ff, 0x31ff, 0x32fe, 0x3357, 0xff6f, 0xff9d, 0x1aff3, 0x1affb, 0x1affe,
    0x1b000, 0x1b122, 0x1b155, 0x1b167,
];

const THAI_STARTS: [u32; 10] = [
    0x0e01, 0x0e31, 0x0e32, 0x0e34, 0x0e40, 0x0e46, 0x0e47, 0x0e4f, 0x0e50, 0x0e5a,
];

const THAI_ENDS: [u32; 10] = [
    0x0e30, 0x0e31, 0x0e33, 0x0e3a, 0x0e45, 0x0e46, 0x0e4e, 0x0e4f, 0x0e59, 0x0e5b,
];

const JAVANESE_STARTS: [u32; 12] = [
    0xa980, 0xa983, 0xa984, 0xa9b3, 0xa9b4, 0xa9b6, 0xa9ba, 0xa9bc, 0xa9be, 0xa9c1, 0xa9d0, 0xa9de,
];

const JAVANESE_ENDS: [u32; 12] = [
    0xa982, 0xa983, 0xa9b2, 0xa9b3, 0xa9b5, 0xa9b9, 0xa9bb, 0xa9bd, 0xa9c0, 0xa9cd, 0xa9d9, 0xa9df,
];

const DECIMAL_DIGIT_STARTS: [u32; 70] = [
    0x0030, 0x0660, 0x06F0, 0x07C0, 0x0966, 0x09E6, 0x0A66, 0x0AE6, 0x0B66, 0x0BE6, 0x0C66, 0x0CE6,
    0x0D66, 0x0DE6, 0x0E50, 0x0ED0, 0x0F20, 0x1040, 0x1090, 0x17E0, 0x1810, 0x1946, 0x19D0, 0x1A80,
    0x1A90, 0x1B50, 0x1BB0, 0x1C40, 0x1C50, 0xA620, 0xA8D0, 0xA900, 0xA9D0, 0xA9F0, 0xAA50, 0xABF0,
    0xFF10, 0x104A0, 0x10D30, 0x10D40, 0x11066, 0x110F0, 0x11136, 0x111D0, 0x112F0, 0x11450,
    0x114D0, 0x11650, 0x116C0, 0x116D0, 0x11730, 0x118E0, 0x11950, 0x11BF0, 0x11C50, 0x11D50,
    0x11DA0, 0x11F50, 0x16130, 0x16A60, 0x16AC0, 0x16B50, 0x16D70, 0x1CCF0, 0x1D7CE, 0x1E140,
    0x1E2F0, 0x1E4F0, 0x1E5F1, 0x1E950,
];

const DECIMAL_DIGIT_ENDS: [u32; 70] = [
    0x0039, 0x0669, 0x06F9, 0x07C9, 0x096F, 0x09EF, 0x0A6F, 0x0AEF, 0x0B6F, 0x0BEF, 0x0C6F, 0x0CEF,
    0x0D6F, 0x0DEF, 0x0E59, 0x0ED9, 0x0F29, 0x1049, 0x1099, 0x17E9, 0x1819, 0x194F, 0x19D9, 0x1A89,
    0x1A99, 0x1B59, 0x1BB9, 0x1C49, 0x1C59, 0xA629, 0xA8D9, 0xA909, 0xA9D9, 0xA9F9, 0xAA59, 0xABF9,
    0xFF19, 0x104A9, 0x10D39, 0x10D49, 0x1106F, 0x110F9, 0x1113F, 0x111D9, 0x112F9, 0x11459,
    0x114D9, 0x11659, 0x116C9, 0x116E3, 0x11739, 0x118E9, 0x11959, 0x11BF9, 0x11C59, 0x11D59,
    0x11DA9, 0x11F59, 0x16139, 0x16A69, 0x16AC9, 0x16B59, 0x16D79, 0x1CCF9, 0x1D7FF, 0x1E149,
    0x1E2F9, 0x1E4F9, 0x1E5FA, 0x1E959,
];
