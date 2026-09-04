use std::hash::Hash;

use imara_diff::{Algorithm, Diff, Interner, Token};

use crate::text::expand;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Range {
    pub start1: usize,
    pub end1: usize,
    pub start2: usize,
    pub end2: usize,
}

impl Range {
    pub fn new(start1: usize, end1: usize, start2: usize, end2: usize) -> Self {
        debug_assert!(
            start1 <= end1 && start2 <= end2,
            "[{start1}, {end1}, {start2}, {end2}]"
        );
        Self {
            start1,
            end1,
            start2,
            end2,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.start1 == self.end1 && self.start2 == self.end2
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiffTooBig;

impl std::fmt::Display for DiffTooBig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a block exceeds the fine comparison size threshold")
    }
}

impl std::error::Error for DiffTooBig {}

pub(crate) const DELTA_THRESHOLD_SIZE: usize = 20_000;

#[derive(Clone, Debug, Default)]
pub(crate) struct Changes {
    changed: Vec<Range>,
    len1: usize,
    len2: usize,
}

impl Changes {
    pub(crate) fn new(changed: Vec<Range>, len1: usize, len2: usize) -> Self {
        Self {
            changed,
            len1,
            len2,
        }
    }

    pub(crate) fn from_unchanged(unchanged: &[Range], len1: usize, len2: usize) -> Self {
        Self::new(complement(unchanged, len1, len2), len1, len2)
    }

    pub(crate) fn len1(&self) -> usize {
        self.len1
    }

    pub(crate) fn len2(&self) -> usize {
        self.len2
    }

    pub(crate) fn changed(&self) -> &[Range] {
        &self.changed
    }

    pub(crate) fn unchanged(&self) -> Vec<Range> {
        complement(&self.changed, self.len1, self.len2)
    }

    pub(crate) fn iterate_all(&self) -> Vec<(Range, bool)> {
        let mut all = Vec::with_capacity(self.changed.len() * 2 + 1);
        let mut last1 = 0;
        let mut last2 = 0;
        for change in &self.changed {
            let gap = Range::new(last1, change.start1, last2, change.start2);
            if !gap.is_empty() {
                all.push((gap, true));
            }
            all.push((*change, false));
            last1 = change.end1;
            last2 = change.end2;
        }
        let tail = Range::new(last1, self.len1, last2, self.len2);
        if !tail.is_empty() {
            all.push((tail, true));
        }
        all
    }
}

fn complement(ranges: &[Range], len1: usize, len2: usize) -> Vec<Range> {
    let mut result = Vec::with_capacity(ranges.len() + 1);
    let mut last1 = 0;
    let mut last2 = 0;
    for range in ranges {
        let gap = Range::new(last1, range.start1, last2, range.start2);
        if !gap.is_empty() {
            result.push(gap);
        }
        last1 = range.end1;
        last2 = range.end2;
    }
    let tail = Range::new(last1, len1, last2, len2);
    if !tail.is_empty() {
        result.push(tail);
    }
    result
}

pub(crate) struct ChangeBuilder {
    len1: usize,
    len2: usize,
    index1: usize,
    index2: usize,
    changes: Vec<Range>,
}

impl ChangeBuilder {
    pub(crate) fn new(len1: usize, len2: usize) -> Self {
        Self {
            len1,
            len2,
            index1: 0,
            index2: 0,
            changes: Vec::new(),
        }
    }

    pub(crate) fn mark_equal_one(&mut self, index1: usize, index2: usize) {
        self.mark_equal_count(index1, index2, 1);
    }

    pub(crate) fn mark_equal_count(&mut self, index1: usize, index2: usize, count: usize) {
        self.mark_equal(index1, index2, index1 + count, index2 + count);
    }

    pub(crate) fn mark_equal(&mut self, index1: usize, index2: usize, end1: usize, end2: usize) {
        if index1 == end1 && index2 == end2 {
            return;
        }
        debug_assert!(self.index1 <= index1 && self.index2 <= index2);
        debug_assert!(index1 <= end1 && index2 <= end2);
        if self.index1 != index1 || self.index2 != index2 {
            self.changes
                .push(Range::new(self.index1, index1, self.index2, index2));
        }
        self.index1 = end1;
        self.index2 = end2;
    }

    pub(crate) fn finish(mut self) -> Changes {
        debug_assert!(self.index1 <= self.len1 && self.index2 <= self.len2);
        if self.len1 != self.index1 || self.len2 != self.index2 {
            self.changes
                .push(Range::new(self.index1, self.len1, self.index2, self.len2));
        }
        Changes::new(self.changes, self.len1, self.len2)
    }
}

pub(crate) struct ExpandChangeBuilder<'a, T> {
    items1: &'a [T],
    items2: &'a [T],
    index1: usize,
    index2: usize,
    changes: Vec<Range>,
}

impl<'a, T: PartialEq> ExpandChangeBuilder<'a, T> {
    pub(crate) fn new(items1: &'a [T], items2: &'a [T]) -> Self {
        Self {
            items1,
            items2,
            index1: 0,
            index2: 0,
            changes: Vec::new(),
        }
    }

    pub(crate) fn index1(&self) -> usize {
        self.index1
    }

    pub(crate) fn index2(&self) -> usize {
        self.index2
    }

    pub(crate) fn mark_equal_one(&mut self, index1: usize, index2: usize) {
        debug_assert!(self.index1 <= index1 && self.index2 <= index2);
        if self.index1 != index1 || self.index2 != index2 {
            self.add_change(self.index1, self.index2, index1, index2);
        }
        self.index1 = index1 + 1;
        self.index2 = index2 + 1;
    }

    fn add_change(&mut self, start1: usize, start2: usize, end1: usize, end2: usize) {
        let range = expand(self.items1, self.items2, start1, start2, end1, end2);
        if !range.is_empty() {
            self.changes.push(range);
        }
    }

    pub(crate) fn finish(mut self) -> Changes {
        let len1 = self.items1.len();
        let len2 = self.items2.len();
        if len1 != self.index1 || len2 != self.index2 {
            self.add_change(self.index1, self.index2, len1, len2);
        }
        Changes::new(self.changes, len1, len2)
    }
}

pub(crate) fn lcs<T: Hash + Eq>(
    items1: impl ExactSizeIterator<Item = T>,
    items2: impl ExactSizeIterator<Item = T>,
) -> Changes {
    let mut interner = Interner::new(items1.len() + items2.len());
    let before: Vec<Token> = items1.map(|item| interner.intern(item)).collect();
    let after: Vec<Token> = items2.map(|item| interner.intern(item)).collect();
    let len1 = before.len();
    let len2 = after.len();
    if len1 == 0 || len2 == 0 {
        let whole = Range::new(0, len1, 0, len2);
        let changed = if whole.is_empty() {
            Vec::new()
        } else {
            vec![whole]
        };
        return Changes::new(changed, len1, len2);
    }
    let mut diff = Diff::default();
    diff.compute_with(Algorithm::Myers, &before, &after, interner.num_tokens());
    let changed = diff
        .hunks()
        .map(|hunk| {
            Range::new(
                hunk.before.start as usize,
                hunk.before.end as usize,
                hunk.after.start as usize,
                hunk.after.end as usize,
            )
        })
        .collect();
    Changes::new(changed, len1, len2)
}

pub(crate) fn lcs_bounded<T: Hash + Eq>(
    items1: impl ExactSizeIterator<Item = T>,
    items2: impl ExactSizeIterator<Item = T>,
) -> Result<Changes, DiffTooBig> {
    if items1.len() > DELTA_THRESHOLD_SIZE || items2.len() > DELTA_THRESHOLD_SIZE {
        return Err(DiffTooBig);
    }
    Ok(lcs(items1, items2))
}
