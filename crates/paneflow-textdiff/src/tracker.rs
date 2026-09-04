use std::ops::Range;

use crate::text::is_equals;
use crate::{compare_lines, ComparisonPolicy};

pub const TOO_BIG_BLOCK_LINES: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockKind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub lines: Range<u32>,
    pub base_lines: Range<u32>,
    pub dirty: bool,
    pub too_big: bool,
}

impl Block {
    fn new(lines: Range<u32>, base_lines: Range<u32>, dirty: bool, too_big: bool) -> Self {
        Self {
            lines,
            base_lines,
            dirty,
            too_big,
        }
    }

    pub fn kind(&self) -> BlockKind {
        if self.lines.is_empty() {
            BlockKind::Deleted
        } else if self.base_lines.is_empty() {
            BlockKind::Added
        } else {
            BlockKind::Modified
        }
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.base_lines.is_empty()
    }

    fn touches(&self, next: &Block) -> bool {
        next.lines.start <= self.lines.end || next.base_lines.start <= self.base_lines.end
    }

    pub fn covers_line(&self, line: u32) -> bool {
        match self.kind() {
            BlockKind::Deleted => line == self.lines.start || line + 1 == self.lines.start,
            _ => self.lines.contains(&line),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackerStats {
    pub rediffs: u32,
    pub full_rediffs: u32,
    pub diffed_lines: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockTracker {
    active: bool,
    blocks: Vec<Block>,
    stats: TrackerStats,
}

impl BlockTracker {
    pub fn inactive() -> Self {
        Self::default()
    }

    pub fn fresh(doc_lines: u32, base_lines: u32) -> Self {
        let mut tracker = Self {
            active: true,
            blocks: Vec::new(),
            stats: TrackerStats::default(),
        };
        tracker.reset(doc_lines, base_lines);
        tracker
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn is_dirty(&self) -> bool {
        self.blocks.iter().any(|block| block.dirty)
    }

    pub fn stats(&self) -> TrackerStats {
        self.stats
    }

    pub fn block_at(&self, line: u32) -> Option<(usize, &Block)> {
        self.blocks
            .iter()
            .enumerate()
            .find(|(_, block)| block.covers_line(line))
    }

    pub fn reset(&mut self, doc_lines: u32, base_lines: u32) {
        if !self.active {
            return;
        }
        self.blocks.clear();
        self.blocks
            .push(Block::new(0..doc_lines, 0..base_lines, true, false));
    }

    pub fn range_changed(&mut self, start_line: u32, before_len: u32, after_len: u32) {
        if !self.active {
            return;
        }
        let end_line = start_line + before_len;
        let delta = i64::from(after_len) - i64::from(before_len);
        let blocks = std::mem::take(&mut self.blocks);
        let mut next = Vec::with_capacity(blocks.len() + 1);
        let mut affected: Option<(Block, Block)> = None;
        let mut shift_before = 0i64;
        let mut affected_any_too_big = false;
        let mut after = Vec::new();
        for block in blocks {
            if block.lines.end < start_line {
                shift_before = i64::from(block.base_lines.end) - i64::from(block.lines.end);
                next.push(block);
            } else if block.lines.start > end_line {
                after.push(block);
            } else {
                affected_any_too_big |= block.too_big;
                affected = Some(match affected {
                    None => (block.clone(), block),
                    Some((first, _)) => (first, block),
                });
            }
        }
        let (range_start, range_end, base_start, base_end) = match &affected {
            None => (
                i64::from(start_line),
                i64::from(end_line) + delta,
                i64::from(start_line) + shift_before,
                i64::from(end_line) + shift_before,
            ),
            Some((first, last)) => {
                let (range_start, base_start) = if first.lines.start <= start_line {
                    (
                        i64::from(first.lines.start),
                        i64::from(first.base_lines.start),
                    )
                } else {
                    (
                        i64::from(start_line),
                        i64::from(start_line) + i64::from(first.base_lines.start)
                            - i64::from(first.lines.start),
                    )
                };
                let (range_end, base_end) = if last.lines.end >= end_line {
                    (
                        i64::from(last.lines.end) + delta,
                        i64::from(last.base_lines.end),
                    )
                } else {
                    (
                        i64::from(end_line) + delta,
                        i64::from(end_line) + i64::from(last.base_lines.end)
                            - i64::from(last.lines.end),
                    )
                };
                (range_start, range_end, base_start, base_end)
            }
        };
        next.push(Block::new(
            clamp_line(range_start)..clamp_line(range_end.max(range_start)),
            clamp_line(base_start)..clamp_line(base_end.max(base_start)),
            true,
            affected_any_too_big,
        ));
        for mut block in after {
            block.lines = shift_range(&block.lines, delta);
            next.push(block);
        }
        self.blocks = next;
    }

    pub fn refresh_dirty(
        &mut self,
        doc_lines: &[&str],
        base_lines: &[&str],
        policy: ComparisonPolicy,
    ) -> bool {
        if !self.active || !self.is_dirty() {
            return false;
        }
        let all_dirty = self.blocks.iter().all(|block| block.dirty);
        if all_dirty && doc_lines.len() == base_lines.len() && doc_lines == base_lines {
            self.blocks.clear();
            return true;
        }
        let blocks = std::mem::take(&mut self.blocks);
        let mut refreshed = Vec::with_capacity(blocks.len());
        let mut group: Vec<Block> = Vec::new();
        for block in blocks {
            if group.last().is_some_and(|last| !last.touches(&block)) {
                self.flush_group(&mut group, &mut refreshed, doc_lines, base_lines, policy);
            }
            group.push(block);
        }
        self.flush_group(&mut group, &mut refreshed, doc_lines, base_lines, policy);
        self.blocks = refreshed;
        true
    }

    fn flush_group(
        &mut self,
        group: &mut Vec<Block>,
        out: &mut Vec<Block>,
        doc_lines: &[&str],
        base_lines: &[&str],
        policy: ComparisonPolicy,
    ) {
        if group.is_empty() {
            return;
        }
        if !group.iter().any(|block| block.dirty) {
            out.append(group);
            return;
        }
        let first = &group[0];
        let last = &group[group.len() - 1];
        let merged = Block::new(
            first.lines.start..last.lines.end.max(first.lines.start),
            first.base_lines.start..last.base_lines.end.max(first.base_lines.start),
            true,
            group.iter().any(|block| block.too_big),
        );
        group.clear();
        if merged.is_empty() {
            return;
        }
        let doc_range = clamp_range(&merged.lines, doc_lines.len());
        let base_range = clamp_range(&merged.base_lines, base_lines.len());
        let doc_slice = &doc_lines[doc_range.clone()];
        let base_slice = &base_lines[base_range.clone()];
        if merged.too_big
            || doc_slice.len() > TOO_BIG_BLOCK_LINES
            || base_slice.len() > TOO_BIG_BLOCK_LINES
        {
            if let Some(block) =
                trimmed_block(doc_slice, base_slice, &doc_range, &base_range, policy)
            {
                out.push(block);
            }
            return;
        }
        self.stats.rediffs += 1;
        if doc_range.len() == doc_lines.len() && base_range.len() == base_lines.len() {
            self.stats.full_rediffs += 1;
        }
        self.stats.diffed_lines += (doc_slice.len() + base_slice.len()) as u64;
        for range in compare_lines(base_slice, doc_slice, policy) {
            out.push(Block::new(
                (doc_range.start + range.start2) as u32..(doc_range.start + range.end2) as u32,
                (base_range.start + range.start1) as u32..(base_range.start + range.end1) as u32,
                false,
                false,
            ));
        }
    }
}

fn trimmed_block(
    doc_slice: &[&str],
    base_slice: &[&str],
    doc_range: &Range<usize>,
    base_range: &Range<usize>,
    policy: ComparisonPolicy,
) -> Option<Block> {
    let mut prefix = 0usize;
    while prefix < doc_slice.len()
        && prefix < base_slice.len()
        && is_equals(doc_slice[prefix], base_slice[prefix], policy)
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < doc_slice.len() - prefix
        && suffix < base_slice.len() - prefix
        && is_equals(
            doc_slice[doc_slice.len() - suffix - 1],
            base_slice[base_slice.len() - suffix - 1],
            policy,
        )
    {
        suffix += 1;
    }
    let block = Block::new(
        (doc_range.start + prefix) as u32..(doc_range.end - suffix) as u32,
        (base_range.start + prefix) as u32..(base_range.end - suffix) as u32,
        false,
        true,
    );
    (!block.is_empty()).then_some(block)
}

fn clamp_line(value: i64) -> u32 {
    u32::try_from(value.max(0)).unwrap_or(u32::MAX)
}

fn clamp_range(range: &Range<u32>, len: usize) -> Range<usize> {
    let end = (range.end as usize).min(len);
    (range.start as usize).min(end)..end
}

fn shift_range(range: &Range<u32>, delta: i64) -> Range<u32> {
    clamp_line(i64::from(range.start) + delta)..clamp_line(i64::from(range.end) + delta)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::unwrap_used,
        reason = "tracker acceptance tests want short, explicit failure sites"
    )]

    use std::time::{Duration, Instant};

    use super::*;
    use crate::split_lines;
    use crate::tests::budget;

    const POLICIES: [ComparisonPolicy; 3] = [
        ComparisonPolicy::Default,
        ComparisonPolicy::TrimWhitespaces,
        ComparisonPolicy::IgnoreWhitespaces,
    ];

    fn lines(text: &str) -> Vec<String> {
        split_lines(text).into_iter().map(str::to_string).collect()
    }

    fn refs(lines: &[String]) -> Vec<&str> {
        lines.iter().map(String::as_str).collect()
    }

    fn tracked(doc: &[String], base: &[String], policy: ComparisonPolicy) -> BlockTracker {
        let mut tracker = BlockTracker::fresh(doc.len() as u32, base.len() as u32);
        tracker.refresh_dirty(&refs(doc), &refs(base), policy);
        tracker
    }

    fn full_blocks(doc: &[String], base: &[String], policy: ComparisonPolicy) -> Vec<Block> {
        compare_lines(&refs(base), &refs(doc), policy)
            .into_iter()
            .map(|range| {
                Block::new(
                    range.start2 as u32..range.end2 as u32,
                    range.start1 as u32..range.end1 as u32,
                    false,
                    false,
                )
            })
            .collect()
    }

    fn assert_valid(
        tracker: &BlockTracker,
        doc: &[String],
        base: &[String],
        policy: ComparisonPolicy,
    ) {
        let blocks = tracker.blocks();
        let mut last_doc = 0u32;
        let mut last_base = 0u32;
        let mut previous: Option<&Block> = None;
        for block in blocks {
            assert!(
                !block.dirty,
                "a refreshed tracker holds no dirty block: {block:?}"
            );
            assert!(block.lines.start >= last_doc && block.base_lines.start >= last_base);
            assert!(block.lines.end as usize <= doc.len());
            assert!(block.base_lines.end as usize <= base.len());
            assert!(
                !block.is_empty(),
                "an empty block is not a change: {block:?}"
            );
            if let Some(previous) = previous {
                assert!(
                    block.lines.start > previous.lines.end
                        && block.base_lines.start > previous.base_lines.end,
                    "adjacent blocks must be merged: {previous:?} then {block:?}"
                );
            }
            for (doc_line, base_line) in
                (last_doc..block.lines.start).zip(last_base..block.base_lines.start)
            {
                assert!(
                    is_equals(&doc[doc_line as usize], &base[base_line as usize], policy),
                    "unchanged line {doc_line} differs from base line {base_line}"
                );
            }
            assert_eq!(
                block.lines.start - last_doc,
                block.base_lines.start - last_base,
                "unchanged regions advance both sides in lockstep"
            );
            if block.kind() == BlockKind::Modified {
                let first_doc = &doc[block.lines.start as usize];
                let first_base = &base[block.base_lines.start as usize];
                let last_doc_line = &doc[block.lines.end as usize - 1];
                let last_base_line = &base[block.base_lines.end as usize - 1];
                assert!(
                    !is_equals(first_doc, first_base, policy),
                    "a modified block never starts with an equal pair: {block:?}"
                );
                assert!(
                    !is_equals(last_doc_line, last_base_line, policy),
                    "a modified block never ends with an equal pair: {block:?}"
                );
            }
            last_doc = block.lines.end;
            last_base = block.base_lines.end;
            previous = Some(block);
        }
        assert_eq!(
            doc.len() as u32 - last_doc,
            base.len() as u32 - last_base,
            "the tail after the last block is equal on both sides"
        );
        for (doc_line, base_line) in (last_doc..doc.len() as u32).zip(last_base..base.len() as u32)
        {
            assert!(is_equals(
                &doc[doc_line as usize],
                &base[base_line as usize],
                policy
            ));
        }
    }

    #[test]
    fn a_fresh_tracker_is_one_dirty_block_and_the_kind_follows_the_sides() {
        let tracker = BlockTracker::fresh(12, 7);
        assert!(tracker.is_active());
        assert!(tracker.is_dirty());
        assert_eq!(tracker.blocks(), &[Block::new(0..12, 0..7, true, false)]);
        assert_eq!(
            Block::new(3..5, 3..3, false, false).kind(),
            BlockKind::Added
        );
        assert_eq!(
            Block::new(3..3, 3..5, false, false).kind(),
            BlockKind::Deleted
        );
        assert_eq!(
            Block::new(3..5, 3..4, false, false).kind(),
            BlockKind::Modified
        );
    }

    #[test]
    fn an_inactive_tracker_ignores_every_call_without_allocating() {
        let mut tracker = BlockTracker::inactive();
        tracker.range_changed(0, 3, 9);
        tracker.reset(40, 40);
        assert!(!tracker.refresh_dirty(&["a"], &["b"], ComparisonPolicy::Default));
        assert!(tracker.blocks().is_empty());
        assert_eq!(tracker.blocks.capacity(), 0);
        assert!(!tracker.is_active());
        assert!(tracker.block_at(0).is_none());
    }

    #[test]
    fn range_changed_partitions_merges_and_shifts_without_a_diff() {
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = vec![
            Block::new(2..3, 2..3, false, false),
            Block::new(6..8, 6..7, false, false),
            Block::new(12..12, 11..13, false, false),
            Block::new(20..21, 21..22, false, false),
        ];
        tracker.range_changed(7, 2, 5);
        assert_eq!(
            tracker.blocks(),
            &[
                Block::new(2..3, 2..3, false, false),
                Block::new(6..12, 6..8, true, false),
                Block::new(15..15, 11..13, false, false),
                Block::new(23..24, 21..22, false, false),
            ]
        );
        assert_eq!(tracker.stats().rediffs, 0);
    }

    #[test]
    fn a_change_between_blocks_creates_one_dirty_block_using_the_shift_before_it() {
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = vec![Block::new(2..5, 2..3, false, false)];
        tracker.range_changed(9, 1, 3);
        assert_eq!(
            tracker.blocks(),
            &[
                Block::new(2..5, 2..3, false, false),
                Block::new(9..12, 7..8, true, false),
            ]
        );
    }

    #[test]
    fn an_edit_after_the_last_block_creates_a_dirty_block_and_leaves_the_others() {
        let base = lines("a\nb\nc\nd\ne\n");
        let mut doc = lines("a\nB\nc\nd\ne\n");
        let mut tracker = tracked(&doc, &base, ComparisonPolicy::Default);
        assert_eq!(tracker.blocks(), &[Block::new(1..2, 1..2, false, false)]);

        doc[4] = "E".to_string();
        tracker.range_changed(4, 1, 1);
        assert_eq!(
            tracker.blocks(),
            &[
                Block::new(1..2, 1..2, false, false),
                Block::new(4..5, 4..5, true, false),
            ]
        );
        let before = tracker.stats();
        assert!(tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default));
        assert_eq!(tracker.stats().rediffs, before.rediffs + 1);
        assert_eq!(
            tracker.stats().full_rediffs,
            before.full_rediffs,
            "the edit after the last block is diffed on its own window"
        );
        assert_eq!(
            tracker.blocks(),
            &[
                Block::new(1..2, 1..2, false, false),
                Block::new(4..5, 4..5, false, false),
            ]
        );
    }

    #[test]
    fn deleting_the_whole_document_leaves_one_deleted_block_over_the_base() {
        let base = lines("a\nb\nc\n");
        let mut tracker = tracked(&base, &base, ComparisonPolicy::Default);
        assert!(tracker.blocks().is_empty());

        tracker.range_changed(0, 4, 1);
        assert_eq!(tracker.blocks(), &[Block::new(0..1, 0..4, true, false)]);
        let doc = lines("");
        assert!(tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default));
        assert_eq!(tracker.blocks().len(), 1);
        let block = &tracker.blocks()[0];
        assert_eq!(block.kind(), BlockKind::Deleted);
        assert_eq!(block.base_lines, 0..3);
        assert!(block.lines.is_empty());
    }

    #[test]
    fn a_document_equal_to_its_base_clears_the_blocks_without_a_diff() {
        let base = lines("x\ny\n");
        let mut tracker = BlockTracker::fresh(3, 3);
        assert!(tracker.refresh_dirty(&refs(&base), &refs(&base), ComparisonPolicy::Default));
        assert!(tracker.blocks().is_empty());
        assert_eq!(tracker.stats().rediffs, 0);
        assert!(!tracker.refresh_dirty(&refs(&base), &refs(&base), ComparisonPolicy::Default));
    }

    #[test]
    fn adjacent_dirty_blocks_are_merged_before_the_rediff() {
        let base = lines("a\nb\nc\nd\n");
        let doc = lines("a\nB\nC\nd\n");
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = vec![
            Block::new(1..2, 1..2, true, false),
            Block::new(2..3, 2..3, true, false),
        ];
        tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default);
        assert_eq!(tracker.blocks(), &[Block::new(1..3, 1..3, false, false)]);
        assert_eq!(tracker.stats().rediffs, 1);
    }

    #[test]
    fn a_too_big_block_is_kept_as_it_is() {
        let base = lines("a\nb\nc\n");
        let doc = lines("a\nX\nc\n");
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = vec![Block::new(0..4, 0..4, true, true)];
        tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default);
        assert_eq!(tracker.blocks(), &[Block::new(1..2, 1..2, false, true)]);
        assert_eq!(
            tracker.stats().rediffs,
            0,
            "a too big block never reaches the diff"
        );

        tracker.range_changed(1, 1, 1);
        assert!(tracker.blocks()[0].too_big, "the flag survives an edit");
        tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default);
        assert_eq!(tracker.blocks(), &[Block::new(1..2, 1..2, false, true)]);
    }

    #[test]
    fn reset_reinstalls_a_single_dirty_block_that_the_next_refresh_recomputes() {
        let base = lines("a\nb\nc\n");
        let doc = lines("a\nX\nc\nd\n");
        let mut tracker = tracked(&doc, &base, ComparisonPolicy::Default);
        assert_eq!(tracker.blocks().len(), 2);
        tracker.reset(doc.len() as u32, base.len() as u32);
        assert_eq!(tracker.blocks(), &[Block::new(0..5, 0..4, true, false)]);
        tracker.refresh_dirty(&refs(&doc), &refs(&base), ComparisonPolicy::Default);
        assert_eq!(
            tracker.blocks(),
            &full_blocks(&doc, &base, ComparisonPolicy::Default)
        );
        assert_eq!(tracker.stats().full_rediffs, 2);
    }

    #[test]
    fn block_at_finds_the_block_of_a_line_and_the_boundary_of_a_deletion() {
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = vec![
            Block::new(1..3, 1..2, false, false),
            Block::new(6..6, 5..7, false, false),
        ];
        assert_eq!(tracker.block_at(0), None);
        assert_eq!(tracker.block_at(2).map(|(index, _)| index), Some(0));
        assert_eq!(tracker.block_at(3), None);
        assert_eq!(tracker.block_at(5).map(|(index, _)| index), Some(1));
        assert_eq!(tracker.block_at(6).map(|(index, _)| index), Some(1));
        assert_eq!(tracker.block_at(7), None);
    }

    #[test]
    fn single_edits_match_the_full_diff_of_the_document() {
        let base =
            lines("fn main() {\n    let a = 1;\n    let b = 2;\n    println!(\"{a}{b}\");\n}\n");
        let policy = ComparisonPolicy::Default;
        let mut doc = base.clone();
        let mut tracker = tracked(&doc, &base, policy);

        doc[1] = "    let a = 10;".to_string();
        tracker.range_changed(1, 1, 1);
        tracker.refresh_dirty(&refs(&doc), &refs(&base), policy);
        assert_eq!(tracker.blocks(), &full_blocks(&doc, &base, policy));

        doc.insert(3, "    let c = 3;".to_string());
        tracker.range_changed(3, 0, 1);
        tracker.refresh_dirty(&refs(&doc), &refs(&base), policy);
        assert_eq!(tracker.blocks(), &full_blocks(&doc, &base, policy));

        doc.remove(4);
        tracker.range_changed(4, 1, 0);
        tracker.refresh_dirty(&refs(&doc), &refs(&base), policy);
        assert_eq!(tracker.blocks(), &full_blocks(&doc, &base, policy));
        assert_eq!(tracker.stats().rediffs, 3, "one bounded diff per edit");
        assert_eq!(
            tracker.stats().full_rediffs,
            0,
            "an equal document short-circuits and no edit diffed everything"
        );
    }

    #[test]
    fn two_hundred_keystrokes_never_rediff_the_whole_document() {
        let base: Vec<String> = (0..400).map(|index| format!("line {index}")).collect();
        let mut doc = base.clone();
        let policy = ComparisonPolicy::Default;
        let mut tracker = tracked(&doc, &base, policy);
        let mut rng = Lcg(11);
        for keystroke in 0..200 {
            let row = 5 + rng.below(390);
            doc[row].push(char::from(b'a' + (keystroke % 26) as u8));
            tracker.range_changed(row as u32, 1, 1);
            tracker.refresh_dirty(&refs(&doc), &refs(&base), policy);
            assert_valid(&tracker, &doc, &base, policy);
        }
        assert_eq!(tracker.stats().rediffs, 200);
        assert_eq!(tracker.stats().full_rediffs, 0);
        assert!(
            tracker.stats().diffed_lines < 200 * 20,
            "each keystroke diffs a handful of lines, not the file: {}",
            tracker.stats().diffed_lines
        );
    }

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 33
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound.max(1) as u64) as usize
        }

        fn line(&mut self) -> String {
            const ALPHABET: [char; 10] = ['a', 'b', 'c', 'd', 'e', 'f', ' ', '\t', ';', '('];
            let len = self.below(13);
            (0..len)
                .map(|_| ALPHABET[self.below(ALPHABET.len())])
                .collect()
        }

        fn text(&mut self, max_lines: usize) -> Vec<String> {
            let count = self.below(max_lines + 1);
            (0..=count).map(|_| self.line()).collect()
        }
    }

    #[test]
    fn five_hundred_random_edit_sequences_keep_the_tracker_valid_and_incremental() {
        let mut rng = Lcg(0x5041_4e45_464c_4f57);
        let started = Instant::now();
        let mut full_matches = 0usize;
        let mut comparisons = 0usize;
        for iteration in 0..500 {
            let policy = POLICIES[iteration % POLICIES.len()];
            let base = rng.text(40);
            let mut doc = rng.text(40);
            let mut tracker = tracked(&doc, &base, policy);
            assert_valid(&tracker, &doc, &base, policy);
            let edits = 1 + rng.below(12);
            for _ in 0..edits {
                let kind = rng.below(3);
                let start = rng.below(doc.len());
                let before_len = if kind == 0 {
                    0
                } else {
                    (1 + rng.below(20)).min(doc.len() - start)
                };
                let after_len = if kind == 1 { 0 } else { 1 + rng.below(20) };
                let replacement: Vec<String> = (0..after_len).map(|_| rng.line()).collect();
                doc.splice(start..start + before_len, replacement);
                if doc.is_empty() {
                    doc.push(String::new());
                    tracker.range_changed(start as u32, before_len as u32, 1);
                } else {
                    tracker.range_changed(start as u32, before_len as u32, after_len as u32);
                }
                let rediffs = tracker.stats().rediffs;
                tracker.refresh_dirty(&refs(&doc), &refs(&base), policy);
                assert!(
                    tracker.stats().rediffs <= rediffs + 1,
                    "one edit costs at most one bounded diff"
                );
                assert_valid(&tracker, &doc, &base, policy);
                comparisons += 1;
                if tracker.blocks() == full_blocks(&doc, &base, policy).as_slice() {
                    full_matches += 1;
                }
            }
            let mut recomputed = tracker.clone();
            recomputed.reset(doc.len() as u32, base.len() as u32);
            recomputed.refresh_dirty(&refs(&doc), &refs(&base), policy);
            assert_eq!(
                recomputed.blocks(),
                full_blocks(&doc, &base, policy).as_slice(),
                "iteration {iteration}: a reset converges to the full diff"
            );
        }
        assert!(
            full_matches * 10 >= comparisons * 6,
            "incremental blocks agree with the full diff on most steps, the rest are tie-breaks through runs of equal lines, got {full_matches}/{comparisons}"
        );
        assert!(
            started.elapsed().as_secs() < 10,
            "took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn range_changed_over_a_thousand_blocks_stays_under_fifty_microseconds() {
        let mut tracker = BlockTracker::fresh(0, 0);
        tracker.blocks = (0..1_000u32)
            .map(|index| {
                Block::new(
                    index * 4..index * 4 + 1,
                    index * 4..index * 4 + 2,
                    false,
                    false,
                )
            })
            .collect();
        let mut rng = Lcg(7);
        let batches = 20u32;
        let calls_per_batch = 10u32;
        let mut fastest_batch = Duration::MAX;
        for _ in 0..batches {
            let started = Instant::now();
            for _ in 0..calls_per_batch {
                let start = rng.below(4_000) as u32;
                tracker.range_changed(start, 1, 1);
                std::hint::black_box(tracker.blocks());
            }
            fastest_batch = fastest_batch.min(started.elapsed() / calls_per_batch);
        }
        assert!(
            fastest_batch < budget(Duration::from_micros(50)),
            "range_changed took {fastest_batch:?} per call over 1 000 blocks in its fastest batch"
        );
        assert_eq!(tracker.stats().rediffs, 0, "range_changed never diffs");
    }

    #[test]
    fn refreshing_a_hundred_line_block_in_a_large_document_stays_under_five_milliseconds() {
        let base: Vec<String> = (0..110_000)
            .map(|index| format!("    let value_{index} = compute({index});"))
            .collect();
        let mut doc = base.clone();
        for line in doc.iter_mut().skip(50_000).take(100) {
            line.push_str(" // edited");
        }
        let mut tracker = tracked(&base, &base, ComparisonPolicy::Default);
        tracker.range_changed(50_000, 100, 100);
        let doc_refs = refs(&doc);
        let base_refs = refs(&base);
        let started = Instant::now();
        tracker.refresh_dirty(&doc_refs, &base_refs, ComparisonPolicy::Default);
        let elapsed = started.elapsed();
        assert_eq!(
            tracker.blocks(),
            &[Block::new(50_000..50_100, 50_000..50_100, false, false)]
        );
        assert!(
            elapsed < budget(Duration::from_millis(5)),
            "refresh_dirty took {elapsed:?}"
        );
    }
}
