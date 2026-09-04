use crate::by_line;
use crate::by_word;
use crate::iterable::{Changes, DiffTooBig, Range};
use crate::text::is_equals;
use crate::{ComparisonPolicy, DiffFragment, HighlightPolicy, LineFragment};

pub(crate) const MAX_BAD_LINES: usize = 3;

pub fn split_lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

pub(crate) struct LineOffsets {
    starts: Vec<usize>,
    text_len: usize,
}

impl LineOffsets {
    pub(crate) fn new(text: &str) -> Self {
        let mut starts = vec![0];
        for (index, byte) in text.bytes().enumerate() {
            if byte == b'\n' {
                starts.push(index + 1);
            }
        }
        Self {
            starts,
            text_len: text.len(),
        }
    }

    pub(crate) fn line_count(&self) -> usize {
        self.starts.len()
    }

    pub(crate) fn line_start(&self, line: usize) -> usize {
        self.starts[line]
    }

    pub(crate) fn line_end_with_newline(&self, line: usize) -> usize {
        if line + 1 < self.starts.len() {
            self.starts[line + 1]
        } else {
            self.text_len
        }
    }

    fn offsets(&self, start: usize, end: usize) -> (usize, usize) {
        if start == end {
            let offset = if start < self.line_count() {
                self.line_start(start)
            } else {
                self.text_len
            };
            (offset, offset)
        } else {
            (self.line_start(start), self.line_end_with_newline(end - 1))
        }
    }
}

pub(crate) fn line_fragment(
    lines: Range,
    offsets1: &LineOffsets,
    offsets2: &LineOffsets,
) -> LineFragment {
    let (start1, end1) = offsets1.offsets(lines.start1, lines.end1);
    let (start2, end2) = offsets2.offsets(lines.start2, lines.end2);
    LineFragment {
        lines,
        offsets: Range::new(start1, end1, start2, end2),
        inner: None,
    }
}

pub(crate) struct UnsquashedReport {
    pub(crate) fragments: Vec<LineFragment>,
    pub(crate) too_big_blocks: usize,
}

pub(crate) fn compare_lines_inner_unsquashed(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    highlight: HighlightPolicy,
) -> UnsquashedReport {
    if text1 == text2 {
        return UnsquashedReport {
            fragments: Vec::new(),
            too_big_blocks: 0,
        };
    }
    let lines1 = split_lines(text1);
    let lines2 = split_lines(text2);
    let offsets1 = LineOffsets::new(text1);
    let offsets2 = LineOffsets::new(text2);
    let changes: Changes = by_line::compare(&lines1, &lines2, policy);
    let fragments: Vec<LineFragment> = changes
        .changed()
        .iter()
        .map(|range| line_fragment(*range, &offsets1, &offsets2))
        .collect();
    match highlight {
        HighlightPolicy::Words => create_inner_fragments(fragments, text1, text2, policy),
        HighlightPolicy::Lines | HighlightPolicy::None => UnsquashedReport {
            fragments,
            too_big_blocks: 0,
        },
    }
}

fn create_inner_fragments(
    fragments: Vec<LineFragment>,
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> UnsquashedReport {
    let mut result = Vec::with_capacity(fragments.len());
    let mut too_big_blocks = 0;
    for fragment in fragments {
        let try_compute = too_big_blocks < MAX_BAD_LINES;
        match create_inner(&fragment, text1, text2, policy, try_compute) {
            Ok(inner) => result.extend(inner),
            Err(DiffTooBig) => {
                result.push(fragment);
                too_big_blocks += 1;
            }
        }
    }
    UnsquashedReport {
        fragments: result,
        too_big_blocks,
    }
}

fn create_inner(
    fragment: &LineFragment,
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    try_compute: bool,
) -> Result<Vec<LineFragment>, DiffTooBig> {
    let offsets = fragment.offsets;
    let sub1 = &text1[offsets.start1..offsets.end1];
    let sub2 = &text2[offsets.start2..offsets.end2];

    if fragment.lines.start1 == fragment.lines.end1 || fragment.lines.start2 == fragment.lines.end2
    {
        let inner = if is_equals(sub1, sub2, policy) {
            Some(Vec::new())
        } else {
            None
        };
        return Ok(vec![LineFragment {
            lines: fragment.lines,
            offsets: fragment.offsets,
            inner,
        }]);
    }

    if !try_compute {
        return Ok(vec![fragment.clone()]);
    }

    let blocks = by_word::compare_and_split(sub1, sub2, policy)?;
    debug_assert!(!blocks.is_empty());

    let mut current_start1 = fragment.lines.start1;
    let mut current_start2 = fragment.lines.start2;
    let mut chunks = Vec::with_capacity(blocks.len());
    let last = blocks.len().saturating_sub(1);
    for (index, block) in blocks.into_iter().enumerate() {
        let (current_end1, current_end2) = if index == last {
            (fragment.lines.end1, fragment.lines.end2)
        } else {
            (
                current_start1 + block.newlines1,
                current_start2 + block.newlines2,
            )
        };
        let block_offsets = Range::new(
            block.offsets.start1 + offsets.start1,
            block.offsets.end1 + offsets.start1,
            block.offsets.start2 + offsets.start2,
            block.offsets.end2 + offsets.start2,
        );
        chunks.push(LineFragment::with_inner(
            Range::new(current_start1, current_end1, current_start2, current_end2),
            block_offsets,
            Some(block.fragments),
        ));
        current_start1 = current_end1;
        current_start2 = current_end2;
    }
    Ok(chunks)
}

impl LineFragment {
    pub(crate) fn with_inner(
        lines: Range,
        offsets: Range,
        inner: Option<Vec<DiffFragment>>,
    ) -> Self {
        let length1 = offsets.end1 - offsets.start1;
        let length2 = offsets.end2 - offsets.start2;
        let inner = match inner {
            Some(fragments)
                if fragments.len() == 1
                    && fragments[0].start1 == 0
                    && fragments[0].start2 == 0
                    && fragments[0].end1 == length1
                    && fragments[0].end2 == length2 =>
            {
                None
            }
            other => other,
        };
        Self {
            lines,
            offsets,
            inner,
        }
    }
}

pub(crate) fn squash(fragments: Vec<LineFragment>) -> Vec<LineFragment> {
    if fragments.is_empty() {
        return fragments;
    }
    let mut result = Vec::with_capacity(fragments.len());
    let mut group: Vec<LineFragment> = Vec::new();
    for fragment in fragments {
        if let Some(previous) = group.last() {
            if !is_adjoining(previous, &fragment) {
                result.push(squash_group(std::mem::take(&mut group)));
            }
        }
        group.push(fragment);
    }
    if !group.is_empty() {
        result.push(squash_group(group));
    }
    result
}

fn is_adjoining(before: &LineFragment, after: &LineFragment) -> bool {
    before.lines.end1 == after.lines.start1
        && before.lines.end2 == after.lines.start2
        && before.offsets.end1 == after.offsets.start1
        && before.offsets.end2 == after.offsets.start2
}

fn squash_group(mut fragments: Vec<LineFragment>) -> LineFragment {
    if fragments.len() == 1 {
        return fragments.remove(0);
    }
    let first = fragments[0].clone();
    let last = fragments[fragments.len() - 1].clone();

    let mut inner: Vec<DiffFragment> = Vec::new();
    for fragment in &fragments {
        let shift1 = fragment.offsets.start1 - first.offsets.start1;
        let shift2 = fragment.offsets.start2 - first.offsets.start2;
        for piece in extract_inner_fragments(fragment) {
            let shifted = DiffFragment {
                start1: piece.start1 + shift1,
                end1: piece.end1 + shift1,
                start2: piece.start2 + shift2,
                end2: piece.end2 + shift2,
            };
            match inner.last_mut() {
                Some(previous)
                    if previous.end1 == shifted.start1 && previous.end2 == shifted.start2 =>
                {
                    previous.end1 = shifted.end1;
                    previous.end2 = shifted.end2;
                }
                _ => inner.push(shifted),
            }
        }
    }

    LineFragment::with_inner(
        Range::new(
            first.lines.start1,
            last.lines.end1,
            first.lines.start2,
            last.lines.end2,
        ),
        Range::new(
            first.offsets.start1,
            last.offsets.end1,
            first.offsets.start2,
            last.offsets.end2,
        ),
        Some(inner),
    )
}

fn extract_inner_fragments(fragment: &LineFragment) -> Vec<DiffFragment> {
    match &fragment.inner {
        Some(inner) => inner.clone(),
        None => vec![DiffFragment {
            start1: 0,
            end1: fragment.offsets.end1 - fragment.offsets.start1,
            start2: 0,
            end2: fragment.offsets.end2 - fragment.offsets.start2,
        }],
    }
}
