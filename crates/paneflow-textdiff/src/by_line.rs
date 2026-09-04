use std::hash::{Hash, Hasher};

use crate::chunk_optimizer::{optimize_chunks, select, BoundaryShift};
use crate::iterable::{lcs, ChangeBuilder, Changes, ExpandChangeBuilder, Range};
use crate::text::{count_non_space_chars, expand, hash, is_equals, UNIMPORTANT_LINE_CHAR_COUNT};
use crate::ComparisonPolicy;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Line<'a> {
    content: &'a str,
    policy: ComparisonPolicy,
    hash: u64,
    non_space_chars: usize,
}

impl<'a> Line<'a> {
    fn new(content: &'a str, policy: ComparisonPolicy) -> Self {
        Self {
            content,
            policy,
            hash: hash(content, policy),
            non_space_chars: count_non_space_chars(content),
        }
    }

    fn with_policy(self, policy: ComparisonPolicy) -> Self {
        if self.policy == policy {
            self
        } else {
            Line::new(self.content, policy)
        }
    }
}

impl PartialEq for Line<'_> {
    fn eq(&self, other: &Self) -> bool {
        debug_assert_eq!(self.policy, other.policy);
        self.hash == other.hash && is_equals(self.content, other.content, self.policy)
    }
}

impl Eq for Line<'_> {}

impl Hash for Line<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.hash);
    }
}

pub(crate) fn compare(lines1: &[&str], lines2: &[&str], policy: ComparisonPolicy) -> Changes {
    let lines1: Vec<Line> = lines1.iter().map(|line| Line::new(line, policy)).collect();
    let lines2: Vec<Line> = lines2.iter().map(|line| Line::new(line, policy)).collect();
    do_compare(&lines1, &lines2, policy)
}

fn do_compare(lines1: &[Line], lines2: &[Line], policy: ComparisonPolicy) -> Changes {
    if policy == ComparisonPolicy::IgnoreWhitespaces {
        let changes = compare_smart(lines1, lines2);
        let changes = optimize_line_chunks(lines1, lines2, &changes);
        expand_ranges(lines1, lines2, &changes)
    } else {
        let iw_lines1: Vec<Line> = lines1
            .iter()
            .map(|line| line.with_policy(ComparisonPolicy::IgnoreWhitespaces))
            .collect();
        let iw_lines2: Vec<Line> = lines2
            .iter()
            .map(|line| line.with_policy(ComparisonPolicy::IgnoreWhitespaces))
            .collect();
        let iw_changes = compare_smart(&iw_lines1, &iw_lines2);
        let iw_changes = optimize_line_chunks(lines1, lines2, &iw_changes);
        correct_changes_second_step(lines1, lines2, &iw_changes)
    }
}

fn compare_smart(lines1: &[Line], lines2: &[Line]) -> Changes {
    let threshold = UNIMPORTANT_LINE_CHAR_COUNT;
    let (big_lines1, indexes1) = big_lines(lines1, threshold);
    let (big_lines2, indexes2) = big_lines(lines2, threshold);
    let changes = lcs(big_lines1.into_iter(), big_lines2.into_iter());
    smart_line_change_correction(&indexes1, &indexes2, lines1, lines2, &changes)
}

fn big_lines<'a>(lines: &[Line<'a>], threshold: usize) -> (Vec<Line<'a>>, Vec<usize>) {
    let mut big = Vec::with_capacity(lines.len());
    let mut indexes = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        if line.non_space_chars > threshold {
            big.push(*line);
            indexes.push(index);
        }
    }
    (big, indexes)
}

fn smart_line_change_correction(
    indexes1: &[usize],
    indexes2: &[usize],
    lines1: &[Line],
    lines2: &[Line],
    changes: &Changes,
) -> Changes {
    let mut builder = ChangeBuilder::new(lines1.len(), lines2.len());
    let mut last1 = 0;
    let mut last2 = 0;
    for range in changes.unchanged() {
        let count = range.end1 - range.start1;
        for i in 0..count {
            let start1 = indexes1[range.start1 + i];
            let start2 = indexes2[range.start2 + i];
            let end1 = start1 + 1;
            let end2 = start2 + 1;
            match_line_gap(&mut builder, lines1, lines2, last1, start1, last2, start2);
            builder.mark_equal(start1, start2, end1, end2);
            last1 = end1;
            last2 = end2;
        }
    }
    match_line_gap(
        &mut builder,
        lines1,
        lines2,
        last1,
        lines1.len(),
        last2,
        lines2.len(),
    );
    builder.finish()
}

fn match_line_gap(
    builder: &mut ChangeBuilder,
    lines1: &[Line],
    lines2: &[Line],
    start1: usize,
    end1: usize,
    start2: usize,
    end2: usize,
) {
    let expanded = expand(lines1, lines2, start1, start2, end1, end2);
    let inner1 = &lines1[expanded.start1..expanded.end1];
    let inner2 = &lines2[expanded.start2..expanded.end2];
    let inner_changes = lcs(inner1.iter().copied(), inner2.iter().copied());

    builder.mark_equal(start1, start2, expanded.start1, expanded.start2);
    for chunk in inner_changes.unchanged() {
        builder.mark_equal_count(
            expanded.start1 + chunk.start1,
            expanded.start2 + chunk.start2,
            chunk.end1 - chunk.start1,
        );
    }
    builder.mark_equal(expanded.end1, expanded.end2, end1, end2);
}

fn optimize_line_chunks(lines1: &[Line], lines2: &[Line], changes: &Changes) -> Changes {
    optimize_chunks(
        lines1,
        lines2,
        changes,
        &LineBoundaryShift { lines1, lines2 },
    )
}

struct LineBoundaryShift<'l, 'a> {
    lines1: &'l [Line<'a>],
    lines2: &'l [Line<'a>],
}

impl BoundaryShift for LineBoundaryShift<'_, '_> {
    fn shift(
        &self,
        touch_left: bool,
        equal_forward: usize,
        equal_backward: usize,
        range1: Range,
        range2: Range,
    ) -> isize {
        for threshold in [0, UNIMPORTANT_LINE_CHAR_COUNT] {
            if let Some(shift) = self.unchanged_boundary_shift(
                touch_left,
                equal_forward,
                equal_backward,
                range2,
                threshold,
            ) {
                return shift;
            }
            if let Some(shift) = self.changed_boundary_shift(
                touch_left,
                equal_forward,
                equal_backward,
                range1,
                range2,
                threshold,
            ) {
                return shift;
            }
        }
        0
    }
}

impl LineBoundaryShift<'_, '_> {
    fn unchanged_boundary_shift(
        &self,
        touch_left: bool,
        equal_forward: usize,
        equal_backward: usize,
        range2: Range,
        threshold: usize,
    ) -> Option<isize> {
        let touch_lines = select(touch_left, self.lines1, self.lines2);
        let touch_start = select(touch_left, range2.start1, range2.start2);
        let forward = next_unimportant_line(touch_lines, touch_start, equal_forward + 1, threshold);
        let backward =
            prev_unimportant_line(touch_lines, touch_start - 1, equal_backward + 1, threshold);
        combine_shift(forward, backward)
    }

    fn changed_boundary_shift(
        &self,
        touch_left: bool,
        equal_forward: usize,
        equal_backward: usize,
        range1: Range,
        range2: Range,
        threshold: usize,
    ) -> Option<isize> {
        let non_touch_lines = select(touch_left, self.lines2, self.lines1);
        let change_start = select(touch_left, range1.end2, range1.end1);
        let change_end = select(touch_left, range2.start2, range2.start1);
        let forward =
            next_unimportant_line(non_touch_lines, change_start, equal_forward + 1, threshold);
        let backward = prev_unimportant_line(
            non_touch_lines,
            change_end - 1,
            equal_backward + 1,
            threshold,
        );
        combine_shift(forward, backward)
    }
}

fn next_unimportant_line(
    lines: &[Line],
    offset: usize,
    count: usize,
    threshold: usize,
) -> Option<usize> {
    (0..count).find(|i| lines[offset + i].non_space_chars <= threshold)
}

fn prev_unimportant_line(
    lines: &[Line],
    offset: usize,
    count: usize,
    threshold: usize,
) -> Option<usize> {
    (0..count).find(|i| lines[offset - i].non_space_chars <= threshold)
}

fn combine_shift(forward: Option<usize>, backward: Option<usize>) -> Option<isize> {
    match (forward, backward) {
        (None, None) => None,
        (Some(0), _) | (_, Some(0)) => Some(0),
        (Some(forward), _) => Some(forward as isize),
        (None, Some(backward)) => Some(-(backward as isize)),
    }
}

fn expand_ranges(lines1: &[Line], lines2: &[Line], changes: &Changes) -> Changes {
    let expanded: Vec<Range> = changes
        .changed()
        .iter()
        .map(|range| {
            expand(
                lines1,
                lines2,
                range.start1,
                range.start2,
                range.end1,
                range.end2,
            )
        })
        .filter(|range| !range.is_empty())
        .collect();
    Changes::new(expanded, lines1.len(), lines2.len())
}

struct SecondStep<'l, 'a> {
    lines1: &'l [Line<'a>],
    lines2: &'l [Line<'a>],
    builder: ExpandChangeBuilder<'l, Line<'a>>,
    sample: Option<&'a str>,
    last1: usize,
    last2: usize,
}

fn correct_changes_second_step(lines1: &[Line], lines2: &[Line], changes: &Changes) -> Changes {
    let mut step = SecondStep {
        lines1,
        lines2,
        builder: ExpandChangeBuilder::new(lines1, lines2),
        sample: None,
        last1: 0,
        last2: 0,
    };
    for range in changes.unchanged() {
        let count = range.end1 - range.start1;
        for i in 0..count {
            let index1 = range.start1 + i;
            let index2 = range.start2 + i;
            let line1 = lines1[index1];
            let line2 = lines2[index2];
            let same_as_sample = step.sample.is_some_and(|sample| {
                is_equals(sample, line1.content, ComparisonPolicy::IgnoreWhitespaces)
            });
            if !same_as_sample {
                if line1 == line2 {
                    step.flush(index1, index2);
                    step.builder.mark_equal_one(index1, index2);
                } else {
                    step.flush(index1, index2);
                    step.sample = Some(line1.content);
                }
            }
        }
    }
    step.flush(changes.len1(), changes.len2());
    step.builder.finish()
}

impl SecondStep<'_, '_> {
    fn flush(&mut self, line1: usize, line2: usize) {
        let Some(sample) = self.sample else {
            return;
        };
        let start1 = self.last1.max(self.builder.index1());
        let start2 = self.last2.max(self.builder.index2());

        let mut sub_lines1 = Vec::new();
        let mut sub_lines2 = Vec::new();
        for i in start1..line1 {
            if is_equals(
                sample,
                self.lines1[i].content,
                ComparisonPolicy::IgnoreWhitespaces,
            ) {
                sub_lines1.push(i);
                self.last1 = i + 1;
            }
        }
        for i in start2..line2 {
            if is_equals(
                sample,
                self.lines2[i].content,
                ComparisonPolicy::IgnoreWhitespaces,
            ) {
                sub_lines2.push(i);
                self.last2 = i + 1;
            }
        }

        debug_assert!(!sub_lines1.is_empty() && !sub_lines2.is_empty());
        self.align_exact_matching(&sub_lines1, &sub_lines2);
        self.sample = None;
    }

    fn align_exact_matching(&mut self, sub_lines1: &[usize], sub_lines2: &[usize]) {
        let n = sub_lines1.len().max(sub_lines2.len());
        let skip_aligning = n > 10 || sub_lines1.len() == sub_lines2.len();

        if skip_aligning {
            let count = sub_lines1.len().min(sub_lines2.len());
            for i in 0..count {
                let index1 = sub_lines1[i];
                let index2 = sub_lines2[i];
                if self.lines1[index1] == self.lines2[index2] {
                    self.builder.mark_equal_one(index1, index2);
                }
            }
            return;
        }

        if sub_lines1.len() < sub_lines2.len() {
            let matching =
                best_matching_alignment(sub_lines1, sub_lines2, self.lines1, self.lines2);
            for i in 0..sub_lines1.len() {
                let index1 = sub_lines1[i];
                let index2 = sub_lines2[matching[i]];
                if self.lines1[index1] == self.lines2[index2] {
                    self.builder.mark_equal_one(index1, index2);
                }
            }
        } else {
            let matching =
                best_matching_alignment(sub_lines2, sub_lines1, self.lines2, self.lines1);
            for i in 0..sub_lines2.len() {
                let index1 = sub_lines1[matching[i]];
                let index2 = sub_lines2[i];
                if self.lines1[index1] == self.lines2[index2] {
                    self.builder.mark_equal_one(index1, index2);
                }
            }
        }
    }
}

fn best_matching_alignment(
    sub_lines1: &[usize],
    sub_lines2: &[usize],
    lines1: &[Line],
    lines2: &[Line],
) -> Vec<usize> {
    debug_assert!(sub_lines1.len() < sub_lines2.len());
    let size = sub_lines1.len();
    let mut comb = vec![0usize; size];
    let mut best: Vec<usize> = (0..size).collect();
    let mut best_weight = 0;
    combinations(
        0,
        sub_lines2.len() - 1,
        0,
        &mut comb,
        &mut |comb: &[usize]| {
            let weight = (0..size)
                .filter(|&i| lines1[sub_lines1[i]] == lines2[sub_lines2[comb[i]]])
                .count();
            if weight > best_weight {
                best_weight = weight;
                best.copy_from_slice(comb);
            }
        },
    );
    best
}

fn combinations(
    start: usize,
    n: usize,
    k: usize,
    comb: &mut Vec<usize>,
    process: &mut impl FnMut(&[usize]),
) {
    if k == comb.len() {
        process(comb);
        return;
    }
    for i in start..=n {
        comb[k] = i;
        combinations(i + 1, n, k + 1, comb, process);
    }
}
