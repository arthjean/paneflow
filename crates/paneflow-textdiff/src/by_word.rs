use crate::by_char;
use crate::chunk_optimizer::{optimize_chunks, select, BoundaryShift};
use crate::iterable::{lcs_bounded, ChangeBuilder, Changes, DiffTooBig, Range};
use crate::splitter::{split_line_blocks, WordBlock};
use crate::text::{
    expand_whitespaces, expand_whitespaces_backward, expand_whitespaces_forward, is_alpha,
    is_continuous_script, is_equals_range, is_whitespace_byte, trim_end, trim_range, trim_start,
};
use crate::{ComparisonPolicy, DiffFragment};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InlineChunk {
    Word { start: usize, end: usize },
    Newline { offset: usize },
}

impl InlineChunk {
    pub(crate) fn start(&self) -> usize {
        match self {
            InlineChunk::Word { start, .. } => *start,
            InlineChunk::Newline { offset } => *offset,
        }
    }

    pub(crate) fn end(&self) -> usize {
        match self {
            InlineChunk::Word { end, .. } => *end,
            InlineChunk::Newline { offset } => *offset + 1,
        }
    }

    pub(crate) fn is_newline(&self) -> bool {
        matches!(self, InlineChunk::Newline { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ChunkToken<'a> {
    Word(&'a str),
    Newline,
}

fn token<'a>(text: &'a str, chunk: &InlineChunk) -> ChunkToken<'a> {
    match chunk {
        InlineChunk::Word { start, end } => ChunkToken::Word(&text[*start..*end]),
        InlineChunk::Newline { .. } => ChunkToken::Newline,
    }
}

#[derive(Clone, Copy)]
struct ChunkView<'a> {
    text: &'a str,
    chunk: InlineChunk,
}

impl ChunkView<'_> {
    fn token(&self) -> ChunkToken<'_> {
        token(self.text, &self.chunk)
    }
}

impl PartialEq for ChunkView<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.token() == other.token()
    }
}

fn views<'a>(text: &'a str, chunks: &[InlineChunk]) -> Vec<ChunkView<'a>> {
    chunks
        .iter()
        .map(|chunk| ChunkView {
            text,
            chunk: *chunk,
        })
        .collect()
}

pub(crate) fn inline_chunks(text: &str) -> Vec<InlineChunk> {
    let mut chunks = Vec::new();
    let mut word_start: Option<usize> = None;
    for (offset, c) in text.char_indices() {
        let alpha = is_alpha(c);
        let word_part = alpha && !is_continuous_script(c);
        if word_part {
            if word_start.is_none() {
                word_start = Some(offset);
            }
            continue;
        }
        if let Some(start) = word_start.take() {
            chunks.push(InlineChunk::Word { start, end: offset });
        }
        if alpha {
            chunks.push(InlineChunk::Word {
                start: offset,
                end: offset + c.len_utf8(),
            });
        } else if c == '\n' {
            chunks.push(InlineChunk::Newline { offset });
        }
    }
    if let Some(start) = word_start {
        chunks.push(InlineChunk::Word {
            start,
            end: text.len(),
        });
    }
    chunks
}

fn diff_chunks(
    text1: &str,
    words1: &[InlineChunk],
    text2: &str,
    words2: &[InlineChunk],
) -> Result<Changes, DiffTooBig> {
    lcs_bounded(
        words1.iter().map(|chunk| token(text1, chunk)),
        words2.iter().map(|chunk| token(text2, chunk)),
    )
}

pub(crate) fn compare(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> Result<Vec<DiffFragment>, DiffTooBig> {
    let words1 = inline_chunks(text1);
    let words2 = inline_chunks(text2);
    let changes = diff_chunks(text1, &words1, text2, &words2)?;
    let changes = optimize_word_chunks(text1, text2, &words1, &words2, &changes);
    let delimiters = match_adjustment_delimiters(text1, text2, &words1, &words2, &changes, 0, 0);
    let iterable = match_adjustment_whitespaces(text1, text2, &delimiters, policy);
    Ok(convert_into_fragments(&iterable))
}

pub(crate) struct LineBlock {
    pub(crate) fragments: Vec<DiffFragment>,
    pub(crate) offsets: Range,
    pub(crate) newlines1: usize,
    pub(crate) newlines2: usize,
}

pub(crate) fn compare_and_split(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> Result<Vec<LineBlock>, DiffTooBig> {
    let words1 = inline_chunks(text1);
    let words2 = inline_chunks(text2);
    let changes = diff_chunks(text1, &words1, text2, &words2)?;
    let changes = optimize_word_chunks(text1, text2, &words1, &words2, &changes);

    let word_blocks = split_line_blocks(text1, text2, &words1, &words2, &changes);
    let sub_iterables = collect_word_block_sub_iterables(&changes, &word_blocks);

    let mut line_blocks = Vec::with_capacity(word_blocks.len());
    for (block, sub_iterable) in word_blocks.iter().zip(sub_iterables) {
        let offsets = block.offsets;
        let words = block.words;
        let subtext1 = &text1[offsets.start1..offsets.end1];
        let subtext2 = &text2[offsets.start2..offsets.end2];
        let subwords1 = &words1[words.start1..words.end1];
        let subwords2 = &words2[words.start2..words.end2];

        let delimiters = match_adjustment_delimiters(
            subtext1,
            subtext2,
            subwords1,
            subwords2,
            &sub_iterable,
            offsets.start1,
            offsets.start2,
        );
        let iterable = match_adjustment_whitespaces(subtext1, subtext2, &delimiters, policy);
        let fragments = convert_into_fragments(&iterable);
        line_blocks.push(LineBlock {
            fragments,
            offsets,
            newlines1: subwords1.iter().filter(|chunk| chunk.is_newline()).count(),
            newlines2: subwords2.iter().filter(|chunk| chunk.is_newline()).count(),
        });
    }
    Ok(line_blocks)
}

fn collect_word_block_sub_iterables(changes: &Changes, word_blocks: &[WordBlock]) -> Vec<Changes> {
    let changed = changes.changed();
    let mut index = 0;
    let mut sub_iterables = Vec::with_capacity(word_blocks.len());
    for block in word_blocks {
        let words = block.words;
        while index < changed.len() {
            let range = changed[index];
            if range.end1 < words.start1 || range.end2 < words.start2 {
                index += 1;
                continue;
            }
            break;
        }
        sub_iterables.push(sub_iterable(changed, words, index));
    }
    sub_iterables
}

fn sub_iterable(changed: &[Range], words: Range, first_index: usize) -> Changes {
    let mut ranges = Vec::new();
    for range in &changed[first_index..] {
        if range.end1 < words.start1 || range.end2 < words.start2 {
            continue;
        }
        if range.start1 > words.end1 || range.start2 > words.end2 {
            break;
        }
        let clipped = Range::new(
            range.start1.max(words.start1) - words.start1,
            range.end1.min(words.end1) - words.start1,
            range.start2.max(words.start2) - words.start2,
            range.end2.min(words.end2) - words.start2,
        );
        if clipped.is_empty() {
            continue;
        }
        ranges.push(clipped);
    }
    Changes::new(ranges, words.end1 - words.start1, words.end2 - words.start2)
}

fn optimize_word_chunks(
    text1: &str,
    text2: &str,
    words1: &[InlineChunk],
    words2: &[InlineChunk],
    changes: &Changes,
) -> Changes {
    let data1 = views(text1, words1);
    let data2 = views(text2, words2);
    optimize_chunks(
        &data1,
        &data2,
        changes,
        &WordBoundaryShift {
            text1,
            text2,
            words1,
            words2,
        },
    )
}

struct WordBoundaryShift<'a> {
    text1: &'a str,
    text2: &'a str,
    words1: &'a [InlineChunk],
    words2: &'a [InlineChunk],
}

impl BoundaryShift for WordBoundaryShift<'_> {
    fn shift(
        &self,
        touch_left: bool,
        equal_forward: usize,
        equal_backward: usize,
        _range1: Range,
        range2: Range,
    ) -> isize {
        let touch_words = select(touch_left, self.words1, self.words2);
        let touch_text = select(touch_left, self.text1, self.text2);
        let touch_start = select(touch_left, range2.start1, range2.start2);

        if is_separated_with_whitespace(
            touch_text,
            &touch_words[touch_start - 1],
            &touch_words[touch_start],
        ) {
            return 0;
        }

        if let Some(left_shift) =
            sequence_edge_shift(touch_text, touch_words, touch_start, equal_forward, true)
        {
            return left_shift as isize;
        }
        if let Some(right_shift) = sequence_edge_shift(
            touch_text,
            touch_words,
            touch_start - 1,
            equal_backward,
            false,
        ) {
            return -(right_shift as isize);
        }
        0
    }
}

fn sequence_edge_shift(
    text: &str,
    words: &[InlineChunk],
    offset: usize,
    count: usize,
    left_to_right: bool,
) -> Option<usize> {
    for i in 0..count {
        let (word1, word2) = if left_to_right {
            (&words[offset + i], &words[offset + i + 1])
        } else {
            (&words[offset - i - 1], &words[offset - i])
        };
        if is_separated_with_whitespace(text, word1, word2) {
            return Some(i + 1);
        }
    }
    None
}

fn is_separated_with_whitespace(text: &str, word1: &InlineChunk, word2: &InlineChunk) -> bool {
    if word1.is_newline() || word2.is_newline() {
        return true;
    }
    text.as_bytes()[word1.end()..word2.start()]
        .iter()
        .any(|byte| is_whitespace_byte(*byte))
}

#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the IntelliJ matcher constructor"
)]
fn match_adjustment_delimiters(
    text1: &str,
    text2: &str,
    words1: &[InlineChunk],
    words2: &[InlineChunk],
    changes: &Changes,
    start_shift1: usize,
    start_shift2: usize,
) -> Changes {
    PunctuationMatcher {
        text1,
        text2,
        words1,
        words2,
        start_shift1,
        start_shift2,
        builder: ChangeBuilder::new(text1.len(), text2.len()),
        last: None,
    }
    .build(changes)
}

struct PunctuationMatcher<'a> {
    text1: &'a str,
    text2: &'a str,
    words1: &'a [InlineChunk],
    words2: &'a [InlineChunk],
    start_shift1: usize,
    start_shift2: usize,
    builder: ChangeBuilder,
    last: Option<Range>,
}

impl PunctuationMatcher<'_> {
    fn build(mut self, changes: &Changes) -> Changes {
        self.match_forward_at(None, None);
        for range in changes.unchanged() {
            let count = range.end1 - range.start1;
            for i in 0..count {
                let index1 = range.start1 + i;
                let index2 = range.start2 + i;
                let start1 = self.start_offset1(index1);
                let start2 = self.start_offset2(index2);
                let end1 = self.end_offset1(index1);
                let end2 = self.end_offset2(index2);

                self.match_backward_at(index1, index2);
                self.builder.mark_equal(start1, start2, end1, end2);
                self.match_forward_at(Some(index1), Some(index2));
            }
        }
        self.match_backward_at(self.words1.len(), self.words2.len());
        self.builder.finish()
    }

    fn match_backward_at(&mut self, index1: usize, index2: usize) {
        let start1 = if index1 == 0 {
            0
        } else {
            self.end_offset1(index1 - 1)
        };
        let start2 = if index2 == 0 {
            0
        } else {
            self.end_offset2(index2 - 1)
        };
        let end1 = if index1 == self.words1.len() {
            self.text1.len()
        } else {
            self.start_offset1(index1)
        };
        let end2 = if index2 == self.words2.len() {
            self.text2.len()
        } else {
            self.start_offset2(index2)
        };
        self.match_backward(Range::new(start1, end1, start2, end2));
        self.last = None;
    }

    fn match_forward_at(&mut self, index1: Option<usize>, index2: Option<usize>) {
        let start1 = index1.map_or(0, |index| self.end_offset1(index));
        let start2 = index2.map_or(0, |index| self.end_offset2(index));
        let next1 = index1.map_or(0, |index| index + 1);
        let next2 = index2.map_or(0, |index| index + 1);
        let end1 = if next1 == self.words1.len() {
            self.text1.len()
        } else {
            self.start_offset1(next1)
        };
        let end2 = if next2 == self.words2.len() {
            self.text2.len()
        } else {
            self.start_offset2(next2)
        };
        debug_assert!(self.last.is_none());
        self.last = Some(Range::new(start1, end1, start2, end2));
    }

    fn match_backward(&mut self, range: Range) {
        let Some(last) = self.last else {
            debug_assert!(false, "match_forward must precede match_backward");
            return;
        };
        if last.start1 == range.start1 && last.start2 == range.start2 {
            self.match_range(range);
            return;
        }
        if last.start1 < range.start1 && last.start2 < range.start2 {
            self.match_range(last);
            self.match_range(range);
            return;
        }
        self.match_complex_range(last, range);
    }

    fn match_range(&mut self, range: Range) {
        if range.is_empty() {
            return;
        }
        let sequence1 = &self.text1[range.start1..range.end1];
        let sequence2 = &self.text2[range.start2..range.end2];
        let changes = by_char::compare_punctuation(sequence1, sequence2);
        for ch in changes.unchanged() {
            self.builder.mark_equal(
                range.start1 + ch.start1,
                range.start2 + ch.start2,
                range.start1 + ch.end1,
                range.start2 + ch.end2,
            );
        }
    }

    fn match_complex_range(&mut self, first: Range, second: Range) {
        if first.start1 == second.start1 && first.end1 == second.end1 {
            self.match_complex_range_left(
                first.start1,
                first.end1,
                first.start2,
                first.end2,
                second.start2,
                second.end2,
            );
        } else if first.start2 == second.start2 && first.end2 == second.end2 {
            self.match_complex_range_right(
                first.start2,
                first.end2,
                first.start1,
                first.end1,
                second.start1,
                second.end1,
            );
        } else {
            debug_assert!(false, "adjustment ranges must share one side");
        }
    }

    fn match_complex_range_left(
        &mut self,
        start1: usize,
        end1: usize,
        start12: usize,
        end12: usize,
        start22: usize,
        end22: usize,
    ) {
        let sequence1 = &self.text1[start1..end1];
        let sequence21 = &self.text2[start12..end12];
        let sequence22 = &self.text2[start22..end22];
        let (first, second) = compare_punctuation_two_side(sequence1, sequence21, sequence22);
        for ch in first {
            self.builder.mark_equal(
                start1 + ch.start1,
                start12 + ch.start2,
                start1 + ch.end1,
                start12 + ch.end2,
            );
        }
        for ch in second {
            self.builder.mark_equal(
                start1 + ch.start1,
                start22 + ch.start2,
                start1 + ch.end1,
                start22 + ch.end2,
            );
        }
    }

    fn match_complex_range_right(
        &mut self,
        start2: usize,
        end2: usize,
        start11: usize,
        end11: usize,
        start21: usize,
        end21: usize,
    ) {
        let sequence11 = &self.text1[start11..end11];
        let sequence12 = &self.text1[start21..end21];
        let sequence2 = &self.text2[start2..end2];
        let (first, second) = compare_punctuation_two_side(sequence2, sequence11, sequence12);
        for ch in first {
            self.builder.mark_equal(
                start11 + ch.start2,
                start2 + ch.start1,
                start11 + ch.end2,
                start2 + ch.end1,
            );
        }
        for ch in second {
            self.builder.mark_equal(
                start21 + ch.start2,
                start2 + ch.start1,
                start21 + ch.end2,
                start2 + ch.end1,
            );
        }
    }

    fn start_offset1(&self, index: usize) -> usize {
        self.words1[index].start() - self.start_shift1
    }

    fn start_offset2(&self, index: usize) -> usize {
        self.words2[index].start() - self.start_shift2
    }

    fn end_offset1(&self, index: usize) -> usize {
        self.words1[index].end() - self.start_shift1
    }

    fn end_offset2(&self, index: usize) -> usize {
        self.words2[index].end() - self.start_shift2
    }
}

fn compare_punctuation_two_side(
    text1: &str,
    text21: &str,
    text22: &str,
) -> (Vec<Range>, Vec<Range>) {
    let mut merged = String::with_capacity(text21.len() + text22.len());
    merged.push_str(text21);
    merged.push_str(text22);
    let changes = by_char::compare_punctuation(text1, &merged);
    let offset = text21.len();
    let mut ranges1 = Vec::new();
    let mut ranges2 = Vec::new();
    for ch in changes.unchanged() {
        if ch.end2 <= offset {
            ranges1.push(ch);
        } else if ch.start2 >= offset {
            ranges2.push(Range::new(
                ch.start1,
                ch.end1,
                ch.start2 - offset,
                ch.end2 - offset,
            ));
        } else {
            let len2 = offset - ch.start2;
            ranges1.push(Range::new(ch.start1, ch.start1 + len2, ch.start2, offset));
            ranges2.push(Range::new(ch.start1 + len2, ch.end1, 0, ch.end2 - offset));
        }
    }
    (ranges1, ranges2)
}

fn match_adjustment_whitespaces(
    text1: &str,
    text2: &str,
    iterable: &Changes,
    policy: ComparisonPolicy,
) -> Changes {
    match policy {
        ComparisonPolicy::Default => default_correction(iterable, text1, text2),
        ComparisonPolicy::TrimWhitespaces => {
            let default = default_correction(iterable, text1, text2);
            trim_spaces_correction(&default, text1, text2)
        }
        ComparisonPolicy::IgnoreWhitespaces => ignore_spaces_correction(iterable, text1, text2),
    }
}

fn default_correction(iterable: &Changes, text1: &str, text2: &str) -> Changes {
    let mut changes = Vec::new();
    for range in iterable.changed() {
        let end_cut = expand_whitespaces_backward(
            text1,
            text2,
            range.start1,
            range.start2,
            range.end1,
            range.end2,
        );
        let start_cut = expand_whitespaces_forward(
            text1,
            text2,
            range.start1,
            range.start2,
            range.end1 - end_cut,
            range.end2 - end_cut,
        );
        let expanded = Range::new(
            range.start1 + start_cut,
            range.end1 - end_cut,
            range.start2 + start_cut,
            range.end2 - end_cut,
        );
        if !expanded.is_empty() {
            changes.push(expanded);
        }
    }
    Changes::new(changes, text1.len(), text2.len())
}

fn ignore_spaces_correction(iterable: &Changes, text1: &str, text2: &str) -> Changes {
    let mut changes = Vec::new();
    for range in iterable.changed() {
        let expanded = expand_whitespaces(text1, text2, *range);
        let trimmed = trim_range(text1, text2, expanded);
        if !trimmed.is_empty()
            && !is_equals_range(text1, text2, trimmed, ComparisonPolicy::IgnoreWhitespaces)
        {
            changes.push(trimmed);
        }
    }
    Changes::new(changes, text1.len(), text2.len())
}

pub(crate) fn trim_spaces_correction(iterable: &Changes, text1: &str, text2: &str) -> Changes {
    let mut changes = Vec::new();
    for range in iterable.changed() {
        let mut start1 = range.start1;
        let mut start2 = range.start2;
        let mut end1 = range.end1;
        let mut end2 = range.end2;

        if is_leading_trailing_space(text1, Some(start1)) {
            start1 = trim_start(text1, start1, end1);
        }
        if is_leading_trailing_space(text1, end1.checked_sub(1)) {
            end1 = trim_end(text1, start1, end1);
        }
        if is_leading_trailing_space(text2, Some(start2)) {
            start2 = trim_start(text2, start2, end2);
        }
        if is_leading_trailing_space(text2, end2.checked_sub(1)) {
            end2 = trim_end(text2, start2, end2);
        }

        let trimmed = Range::new(start1, end1, start2, end2);
        if !trimmed.is_empty() && !is_equals_range(text1, text2, trimmed, ComparisonPolicy::Default)
        {
            changes.push(trimmed);
        }
    }
    Changes::new(changes, text1.len(), text2.len())
}

fn is_leading_trailing_space(text: &str, offset: Option<usize>) -> bool {
    let Some(offset) = offset else {
        return false;
    };
    is_leading_space(text, offset) || is_trailing_space(text, offset)
}

fn is_leading_space(text: &str, start: usize) -> bool {
    let bytes = text.as_bytes();
    if start >= bytes.len() || !is_whitespace_byte(bytes[start]) {
        return false;
    }
    let mut index = start;
    while index > 0 {
        index -= 1;
        let byte = bytes[index];
        if byte == b'\n' {
            return true;
        }
        if !is_whitespace_byte(byte) {
            return false;
        }
    }
    true
}

fn is_trailing_space(text: &str, end: usize) -> bool {
    let bytes = text.as_bytes();
    if end >= bytes.len() || !is_whitespace_byte(bytes[end]) {
        return false;
    }
    let mut index = end;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' {
            return true;
        }
        if !is_whitespace_byte(byte) {
            return false;
        }
        index += 1;
    }
    true
}

pub(crate) fn convert_into_fragments(changes: &Changes) -> Vec<DiffFragment> {
    changes
        .changed()
        .iter()
        .map(|range| DiffFragment {
            start1: range.start1,
            end1: range.end1,
            start2: range.start2,
            end2: range.end2,
        })
        .collect()
}
