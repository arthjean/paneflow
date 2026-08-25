//! Line-level diff engine (US-004, prd-multi-worktree-diff-2026-Q3.md).
//!
//! A from-scratch port of Zed's `buffer_diff::compute_hunks`
//! (`crates/buffer_diff/src/buffer_diff.rs:1125`), reduced to what a read-only
//! viewer needs. Zed anchors hunks into a live CRDT `text::Buffer` (a
//! `Range<Anchor>` over a `SumTree`); Paneflow has no editor, so hunks carry
//! plain `u32` row ranges. That deliberately drops the entire
//! `text`/`language`/`rope`/`clock`/`sum_tree` dependency closure; the only
//! Zed-derived dependency kept is `imara-diff` (the same 0.1.8 Zed pins),
//! driving the identical `Histogram` algorithm over `lines_with_terminator`.
//!
//! Highlighting stays at the line level: the diff view shows changed lines as
//! whole rows, the way Cursor and VS Code do. Word-level intra-line marking
//! (Zed's `word_diff_ranges`, shipped here as US-010) was removed - on
//! rewritten lines it painted a second, louder wash over the row tint and read
//! as noise rather than precision.

use std::ops::Range;

use imara_diff::intern::InternedInput;
use imara_diff::sources::lines_with_terminator;
use imara_diff::{Algorithm, Sink};

/// What a hunk does to the base text, derived exactly as Zed's
/// `DiffHunk::status` does: an empty *new* range means the base lines were
/// deleted; an empty *base* range means new lines were added; both non-empty
/// means the lines were modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffHunkStatus {
    Added,
    Modified,
    Deleted,
}

/// A single contiguous change between the base text and the new text, expressed
/// as half-open line ranges into each side (0-based rows).
///
/// - `base_row_range`: lines in the base text this hunk replaces/removes.
/// - `new_row_range`: lines in the new text this hunk adds/replaces.
///
/// An `Added` hunk has an empty `base_row_range`; a `Deleted` hunk has an empty
/// `new_row_range`. EP-003 layout math (US-008) consumes these ranges to align
/// the side-by-side view with phantom rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub base_row_range: Range<u32>,
    pub new_row_range: Range<u32>,
    pub status: DiffHunkStatus,
}

/// Compute line-level hunks turning `base` into `new`.
///
/// Returns an empty vec when the two texts are identical. Pure and synchronous
/// callers run it off the GPUI main thread (US-007). Mirrors Zed's use of
/// `imara_diff::diff(Algorithm::Histogram, …)` over `lines_with_terminator`.
pub fn compute_hunks(base: &str, new: &str) -> Vec<DiffHunk> {
    if base == new {
        return Vec::new();
    }
    let input = InternedInput::new(lines_with_terminator(base), lines_with_terminator(new));
    imara_diff::diff(Algorithm::Histogram, &input, HunkCollector::default())
}

/// `imara-diff` sink that turns each `process_change(before, after)` callback -
/// where `before`/`after` are base/new line ranges - into a [`DiffHunk`].
#[derive(Default)]
struct HunkCollector {
    hunks: Vec<DiffHunk>,
}

impl Sink for HunkCollector {
    type Out = Vec<DiffHunk>;

    fn process_change(&mut self, before: Range<u32>, after: Range<u32>) {
        let status = if after.start == after.end {
            DiffHunkStatus::Deleted
        } else if before.start == before.end {
            DiffHunkStatus::Added
        } else {
            DiffHunkStatus::Modified
        };
        self.hunks.push(DiffHunk {
            base_row_range: before,
            new_row_range: after,
            status,
        });
    }

    fn finish(self) -> Self::Out {
        self.hunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_has_no_hunks() {
        assert!(compute_hunks("a\nb\nc\n", "a\nb\nc\n").is_empty());
        assert!(compute_hunks("", "").is_empty());
    }

    #[test]
    fn pure_addition() {
        // Append a line at the end: base has 2 lines, new has 3.
        let hunks = compute_hunks("a\nb\n", "a\nb\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Added);
        assert!(h.base_row_range.start == h.base_row_range.end); // empty base side
        assert_eq!(h.new_row_range, 2..3);
    }

    #[test]
    fn pure_deletion() {
        // Remove the middle line: base 3 lines, new 2.
        let hunks = compute_hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Deleted);
        assert_eq!(h.base_row_range, 1..2);
        assert!(h.new_row_range.start == h.new_row_range.end); // empty new side
    }

    #[test]
    fn modification() {
        // Change the middle line in place: equal line counts on both sides.
        let hunks = compute_hunks("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Modified);
        assert_eq!(h.base_row_range, 1..2);
        assert_eq!(h.new_row_range, 1..2);
    }

    #[test]
    fn multiple_disjoint_hunks() {
        let hunks = compute_hunks("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].status, DiffHunkStatus::Modified);
        assert_eq!(hunks[0].new_row_range, 0..1);
        assert_eq!(hunks[1].status, DiffHunkStatus::Modified);
        assert_eq!(hunks[1].new_row_range, 4..5);
    }

    #[test]
    fn added_from_empty_base() {
        let hunks = compute_hunks("", "a\nb\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
    }
}
