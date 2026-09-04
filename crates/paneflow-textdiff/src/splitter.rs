use crate::by_word::InlineChunk;
use crate::iterable::{Changes, Range};
use crate::text::is_equals_range;
use crate::ComparisonPolicy;

#[derive(Clone, Copy, Debug)]
pub(crate) struct WordBlock {
    pub(crate) words: Range,
    pub(crate) offsets: Range,
}

#[derive(Clone, Copy)]
struct PendingChunk {
    block: WordBlock,
    has_equal_words: bool,
    has_words_inside: bool,
    is_equal_ignore_whitespaces: bool,
}

struct Splitter<'a> {
    text1: &'a str,
    text2: &'a str,
    words1: &'a [InlineChunk],
    words2: &'a [InlineChunk],
    result: Vec<WordBlock>,
    last1: Option<usize>,
    last2: Option<usize>,
    pending: Option<PendingChunk>,
}

pub(crate) fn split_line_blocks(
    text1: &str,
    text2: &str,
    words1: &[InlineChunk],
    words2: &[InlineChunk],
    iterable: &Changes,
) -> Vec<WordBlock> {
    let mut splitter = Splitter {
        text1,
        text2,
        words1,
        words2,
        result: Vec::new(),
        last1: None,
        last2: None,
        pending: None,
    };
    let mut has_equal_words = false;
    for range in iterable.unchanged() {
        let count = range.end1 - range.start1;
        for i in 0..count {
            let index1 = range.start1 + i;
            let index2 = range.start2 + i;
            if words1[index1].is_newline() && words2[index2].is_newline() {
                splitter.add_line_chunk(Some(index1), Some(index2), has_equal_words);
                has_equal_words = false;
            } else {
                if is_first_in_line(words1, index1) && is_first_in_line(words2, index2) {
                    splitter.add_line_chunk(
                        index1.checked_sub(1),
                        index2.checked_sub(1),
                        has_equal_words,
                    );
                }
                has_equal_words = true;
            }
        }
    }
    splitter.add_line_chunk(Some(words1.len()), Some(words2.len()), has_equal_words);
    if let Some(pending) = splitter.pending {
        splitter.result.push(pending.block);
    }
    splitter.result
}

impl Splitter<'_> {
    fn add_line_chunk(&mut self, end1: Option<usize>, end2: Option<usize>, has_equal_words: bool) {
        if after(self.last1, end1) || after(self.last2, end2) {
            return;
        }
        let chunk = self.create_chunk(self.last1, self.last2, end1, end2, has_equal_words);
        if chunk.block.offsets.is_empty() {
            return;
        }
        self.pending = Some(match self.pending {
            Some(previous) => {
                if should_merge_chunks(&previous, &chunk) {
                    merge_chunks(&previous, &chunk)
                } else {
                    self.result.push(previous.block);
                    chunk
                }
            }
            None => chunk,
        });
        self.last1 = end1;
        self.last2 = end2;
    }

    fn create_chunk(
        &self,
        start1: Option<usize>,
        start2: Option<usize>,
        end1: Option<usize>,
        end2: Option<usize>,
        has_equal_words: bool,
    ) -> PendingChunk {
        let start_offset1 = offset_at(self.words1, self.text1, start1);
        let start_offset2 = offset_at(self.words2, self.text2, start2);
        let end_offset1 = offset_at(self.words1, self.text1, end1);
        let end_offset2 = offset_at(self.words2, self.text2, end2);

        let words = Range::new(
            start1.map_or(0, |index| index + 1),
            end1.map_or(0, |index| (index + 1).min(self.words1.len())),
            start2.map_or(0, |index| index + 1),
            end2.map_or(0, |index| (index + 1).min(self.words2.len())),
        );
        let offsets = Range::new(start_offset1, end_offset1, start_offset2, end_offset2);
        let block = WordBlock { words, offsets };
        PendingChunk {
            block,
            has_equal_words,
            has_words_inside: self.has_words_inside(&block),
            is_equal_ignore_whitespaces: is_equals_range(
                self.text1,
                self.text2,
                offsets,
                ComparisonPolicy::IgnoreWhitespaces,
            ),
        }
    }

    fn has_words_inside(&self, block: &WordBlock) -> bool {
        self.words1[block.words.start1..block.words.end1]
            .iter()
            .chain(self.words2[block.words.start2..block.words.end2].iter())
            .any(|chunk| !chunk.is_newline())
    }
}

fn after(last: Option<usize>, end: Option<usize>) -> bool {
    match (last, end) {
        (Some(last), Some(end)) => last > end,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

fn offset_at(words: &[InlineChunk], text: &str, index: Option<usize>) -> usize {
    match index {
        None => 0,
        Some(index) if index == words.len() => text.len(),
        Some(index) => {
            debug_assert!(words[index].is_newline());
            words[index].end()
        }
    }
}

fn is_first_in_line(words: &[InlineChunk], index: usize) -> bool {
    index == 0 || words[index - 1].is_newline()
}

fn should_merge_chunks(chunk1: &PendingChunk, chunk2: &PendingChunk) -> bool {
    if !chunk1.has_equal_words && !chunk2.has_equal_words {
        return true;
    }
    if chunk1.is_equal_ignore_whitespaces && chunk2.is_equal_ignore_whitespaces {
        return true;
    }
    if !chunk1.has_words_inside || !chunk2.has_words_inside {
        return true;
    }
    false
}

fn merge_chunks(chunk1: &PendingChunk, chunk2: &PendingChunk) -> PendingChunk {
    let block1 = chunk1.block;
    let block2 = chunk2.block;
    PendingChunk {
        block: WordBlock {
            words: Range::new(
                block1.words.start1,
                block2.words.end1,
                block1.words.start2,
                block2.words.end2,
            ),
            offsets: Range::new(
                block1.offsets.start1,
                block2.offsets.end1,
                block1.offsets.start2,
                block2.offsets.end2,
            ),
        },
        has_equal_words: chunk1.has_equal_words || chunk2.has_equal_words,
        has_words_inside: chunk1.has_words_inside || chunk2.has_words_inside,
        is_equal_ignore_whitespaces: chunk1.is_equal_ignore_whitespaces
            && chunk2.is_equal_ignore_whitespaces,
    }
}
