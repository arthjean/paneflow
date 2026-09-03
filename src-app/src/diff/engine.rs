use std::ops::Range;

use imara_diff::intern::InternedInput;
use imara_diff::sources::lines_with_terminator;
use imara_diff::{Algorithm, Sink};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffHunkStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub base_row_range: Range<u32>,
    pub new_row_range: Range<u32>,
    pub status: DiffHunkStatus,
}

pub fn compute_hunks(base: &str, new: &str) -> Vec<DiffHunk> {
    if base == new {
        return Vec::new();
    }
    let input = InternedInput::new(lines_with_terminator(base), lines_with_terminator(new));
    imara_diff::diff(Algorithm::Histogram, &input, HunkCollector::default())
}

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
        let hunks = compute_hunks("a\nb\n", "a\nb\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Added);
        assert!(h.base_row_range.start == h.base_row_range.end);
        assert_eq!(h.new_row_range, 2..3);
    }

    #[test]
    fn pure_deletion() {
        let hunks = compute_hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Deleted);
        assert_eq!(h.base_row_range, 1..2);
        assert!(h.new_row_range.start == h.new_row_range.end);
    }

    #[test]
    fn modification() {
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
