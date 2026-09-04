use super::engine::DiffHunk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Context,
    Added,
    Removed,
    Phantom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    pub kind: CellKind,
    pub line: Option<u32>,
}

impl Cell {
    const PHANTOM: Cell = Cell {
        kind: CellKind::Phantom,
        line: None,
    };

    fn context(line: u32) -> Cell {
        Cell {
            kind: CellKind::Context,
            line: Some(line),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlignedRow {
    pub left: Cell,
    pub right: Cell,
}

pub fn align_rows(
    hunks: &[DiffHunk],
    base_line_count: u32,
    new_line_count: u32,
) -> Vec<AlignedRow> {
    let mut rows = Vec::new();
    let mut bc = 0u32;
    let mut nc = 0u32;

    for h in hunks {
        while nc < h.new_row_range.start && bc < h.base_row_range.start {
            rows.push(AlignedRow {
                left: Cell::context(bc),
                right: Cell::context(nc),
            });
            bc += 1;
            nc += 1;
        }

        let rem_start = h.base_row_range.start;
        let add_start = h.new_row_range.start;
        let rem_len = h.base_row_range.end - rem_start;
        let add_len = h.new_row_range.end - add_start;
        let pairs = rem_len.max(add_len);
        for k in 0..pairs {
            let left = if k < rem_len {
                Cell {
                    kind: CellKind::Removed,
                    line: Some(rem_start + k),
                }
            } else {
                Cell::PHANTOM
            };
            let right = if k < add_len {
                Cell {
                    kind: CellKind::Added,
                    line: Some(add_start + k),
                }
            } else {
                Cell::PHANTOM
            };
            rows.push(AlignedRow { left, right });
        }

        bc = h.base_row_range.end;
        nc = h.new_row_range.end;
    }

    while nc < new_line_count && bc < base_line_count {
        rows.push(AlignedRow {
            left: Cell::context(bc),
            right: Cell::context(nc),
        });
        bc += 1;
        nc += 1;
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::super::engine::DiffHunkStatus;
    use super::*;

    fn hunk(b: std::ops::Range<u32>, n: std::ops::Range<u32>, status: DiffHunkStatus) -> DiffHunk {
        DiffHunk {
            base_row_range: b,
            new_row_range: n,
            status,
            tone: super::super::engine::ChangeTone::Full,
            base_runs: Vec::new(),
            new_runs: Vec::new(),
        }
    }

    #[test]
    fn no_hunks_all_context() {
        let rows = align_rows(&[], 3, 3);
        assert_eq!(rows.len(), 3);
        assert!(
            rows.iter()
                .all(|r| r.left.kind == CellKind::Context && r.right.kind == CellKind::Context)
        );
        assert_eq!(rows[0].left.line, Some(0));
        assert_eq!(rows[0].right.line, Some(0));
    }

    #[test]
    fn pure_addition_pads_left_with_phantoms() {
        let rows = align_rows(&[hunk(1..1, 1..3, DiffHunkStatus::Added)], 2, 4);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].left.kind, CellKind::Context);
        assert_eq!(rows[1].left.kind, CellKind::Phantom);
        assert_eq!(rows[1].right.kind, CellKind::Added);
        assert_eq!(rows[2].left.kind, CellKind::Phantom);
        assert_eq!(rows[2].right.kind, CellKind::Added);
        assert_eq!(rows[3].left.kind, CellKind::Context);
    }

    #[test]
    fn pure_deletion_pads_right_with_phantoms() {
        let rows = align_rows(&[hunk(1..3, 1..1, DiffHunkStatus::Deleted)], 4, 2);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].left.kind, CellKind::Removed);
        assert_eq!(rows[1].right.kind, CellKind::Phantom);
        assert_eq!(rows[2].left.kind, CellKind::Removed);
        assert_eq!(rows[2].right.kind, CellKind::Phantom);
    }

    #[test]
    fn modification_pairs_removed_with_added() {
        let rows = align_rows(&[hunk(1..2, 1..2, DiffHunkStatus::Modified)], 3, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].left.kind, CellKind::Removed);
        assert_eq!(rows[1].left.line, Some(1));
        assert_eq!(rows[1].right.kind, CellKind::Added);
        assert_eq!(rows[1].right.line, Some(1));
    }

    #[test]
    fn uneven_modification_pads_shorter_side() {
        let rows = align_rows(&[hunk(0..1, 0..3, DiffHunkStatus::Modified)], 1, 3);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].left.kind, CellKind::Removed);
        assert_eq!(rows[0].right.kind, CellKind::Added);
        assert_eq!(rows[1].left.kind, CellKind::Phantom);
        assert_eq!(rows[1].right.kind, CellKind::Added);
        assert_eq!(rows[2].left.kind, CellKind::Phantom);
        assert_eq!(rows[2].right.kind, CellKind::Added);
    }
}
