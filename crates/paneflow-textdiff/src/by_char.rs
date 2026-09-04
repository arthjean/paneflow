use crate::iterable::{lcs, lcs_bounded, ChangeBuilder, Changes, DiffTooBig, Range};
use crate::text::{expand_whitespaces_forward, is_punctuation_byte, is_whitespace};
use crate::ComparisonPolicy;

pub(crate) fn compare(text1: &str, text2: &str) -> Changes {
    let code_points1: Vec<char> = text1.chars().collect();
    let code_points2: Vec<char> = text2.chars().collect();
    let iterable = lcs(code_points1.iter().copied(), code_points2.iter().copied());

    let mut offset1 = 0;
    let mut offset2 = 0;
    let mut builder = ChangeBuilder::new(text1.len(), text2.len());
    for (range, equals) in iterable.iterate_all() {
        let end1 = offset1 + byte_len(&code_points1[range.start1..range.end1]);
        let end2 = offset2 + byte_len(&code_points2[range.start2..range.end2]);
        if equals {
            builder.mark_equal(offset1, offset2, end1, end2);
        }
        offset1 = end1;
        offset2 = end2;
    }
    debug_assert!(offset1 == text1.len() && offset2 == text2.len());
    builder.finish()
}

fn byte_len(code_points: &[char]) -> usize {
    code_points.iter().map(|c| c.len_utf8()).sum()
}

pub(crate) fn compare_two_step(text1: &str, text2: &str) -> Result<Changes, DiffTooBig> {
    let code_points1 = non_space_code_points(text1);
    let code_points2 = non_space_code_points(text2);
    let non_space_changes = lcs_bounded(
        code_points1.code_points.iter().copied(),
        code_points2.code_points.iter().copied(),
    )?;
    Ok(match_adjustment_spaces(
        &code_points1,
        &code_points2,
        text1,
        text2,
        &non_space_changes,
    ))
}

pub(crate) fn compare_trim_whitespaces(text1: &str, text2: &str) -> Result<Changes, DiffTooBig> {
    let iterable = compare_two_step(text1, text2)?;
    Ok(crate::by_word::trim_spaces_correction(
        &iterable, text1, text2,
    ))
}

pub(crate) fn compare_ignore_whitespaces(text1: &str, text2: &str) -> Result<Changes, DiffTooBig> {
    let code_points1 = non_space_code_points(text1);
    let code_points2 = non_space_code_points(text2);
    let changes = lcs_bounded(
        code_points1.code_points.iter().copied(),
        code_points2.code_points.iter().copied(),
    )?;
    Ok(match_adjustment_spaces_iw(
        &code_points1,
        &code_points2,
        text1,
        text2,
        &changes,
    ))
}

pub(crate) fn compare_punctuation(text1: &str, text2: &str) -> Changes {
    let chars1 = punctuation_chars(text1);
    let chars2 = punctuation_chars(text2);
    let changes = lcs(
        chars1.code_points.iter().copied(),
        chars2.code_points.iter().copied(),
    );
    let mut builder = ChangeBuilder::new(text1.len(), text2.len());
    for range in changes.unchanged() {
        let count = range.end1 - range.start1;
        for i in 0..count {
            let offset1 = chars1.offsets[range.start1 + i];
            let offset2 = chars2.offsets[range.start2 + i];
            builder.mark_equal_one(offset1, offset2);
        }
    }
    builder.finish()
}

fn match_adjustment_spaces(
    code_points1: &CodePointsOffsets,
    code_points2: &CodePointsOffsets,
    text1: &str,
    text2: &str,
    changes: &Changes,
) -> Changes {
    let mut builder = ChangeBuilder::new(text1.len(), text2.len());
    let mut last1 = 0;
    let mut last2 = 0;
    for range in changes.unchanged() {
        let count = range.end1 - range.start1;
        for i in 0..count {
            let start1 = code_points1.char_offset(range.start1 + i);
            let end1 = code_points1.char_offset_after(range.start1 + i);
            let start2 = code_points2.char_offset(range.start2 + i);
            let end2 = code_points2.char_offset_after(range.start2 + i);
            match_char_gap(&mut builder, text1, text2, last1, start1, last2, start2);
            builder.mark_equal(start1, start2, end1, end2);
            last1 = end1;
            last2 = end2;
        }
    }
    match_char_gap(
        &mut builder,
        text1,
        text2,
        last1,
        text1.len(),
        last2,
        text2.len(),
    );
    builder.finish()
}

fn match_char_gap(
    builder: &mut ChangeBuilder,
    text1: &str,
    text2: &str,
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
) {
    let inner_changes = compare(&text1[start1..end1], &text2[start2..end2]);
    for chunk in inner_changes.unchanged() {
        builder.mark_equal_count(
            start1 + chunk.start1,
            start2 + chunk.start2,
            chunk.end1 - chunk.start1,
        );
    }
}

fn match_adjustment_spaces_iw(
    code_points1: &CodePointsOffsets,
    code_points2: &CodePointsOffsets,
    text1: &str,
    text2: &str,
    changes: &Changes,
) -> Changes {
    let mut ranges = Vec::new();
    for change in changes.changed() {
        let (start1, end1) = if change.start1 == change.end1 {
            let end = expand_forward_w(code_points1, code_points2, text1, text2, *change, true);
            (end, end)
        } else {
            (
                code_points1.char_offset(change.start1),
                code_points1.char_offset_after(change.end1 - 1),
            )
        };
        let (start2, end2) = if change.start2 == change.end2 {
            let end = expand_forward_w(code_points1, code_points2, text1, text2, *change, false);
            (end, end)
        } else {
            (
                code_points2.char_offset(change.start2),
                code_points2.char_offset_after(change.end2 - 1),
            )
        };
        ranges.push(Range::new(start1, end1, start2, end2));
    }
    Changes::new(ranges, text1.len(), text2.len())
}

fn expand_forward_w(
    code_points1: &CodePointsOffsets,
    code_points2: &CodePointsOffsets,
    text1: &str,
    text2: &str,
    change: Range,
    left: bool,
) -> usize {
    let offset1 = if change.start1 == 0 {
        0
    } else {
        code_points1.char_offset_after(change.start1 - 1)
    };
    let offset2 = if change.start2 == 0 {
        0
    } else {
        code_points2.char_offset_after(change.start2 - 1)
    };
    let start = if left { offset1 } else { offset2 };
    start + expand_whitespaces_forward(text1, text2, offset1, offset2, text1.len(), text2.len())
}

struct CodePointsOffsets {
    code_points: Vec<char>,
    offsets: Vec<usize>,
}

impl CodePointsOffsets {
    fn char_offset(&self, index: usize) -> usize {
        self.offsets[index]
    }

    fn char_offset_after(&self, index: usize) -> usize {
        self.offsets[index] + self.code_points[index].len_utf8()
    }
}

fn non_space_code_points(text: &str) -> CodePointsOffsets {
    let mut code_points = Vec::with_capacity(text.len());
    let mut offsets = Vec::with_capacity(text.len());
    for (offset, c) in text.char_indices() {
        if !is_whitespace(c) {
            code_points.push(c);
            offsets.push(offset);
        }
    }
    CodePointsOffsets {
        code_points,
        offsets,
    }
}

fn punctuation_chars(text: &str) -> CodePointsOffsets {
    let mut code_points = Vec::new();
    let mut offsets = Vec::new();
    for (offset, byte) in text.bytes().enumerate() {
        if is_punctuation_byte(byte) {
            code_points.push(char::from(byte));
            offsets.push(offset);
        }
    }
    CodePointsOffsets {
        code_points,
        offsets,
    }
}

pub(crate) fn compare_chars(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> Result<Changes, DiffTooBig> {
    match policy {
        ComparisonPolicy::Default => compare_two_step(text1, text2),
        ComparisonPolicy::TrimWhitespaces => compare_trim_whitespaces(text1, text2),
        ComparisonPolicy::IgnoreWhitespaces => compare_ignore_whitespaces(text1, text2),
    }
}
