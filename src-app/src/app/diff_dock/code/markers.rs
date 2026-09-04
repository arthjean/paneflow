use paneflow_textdiff::{Block, BlockKind};

pub(crate) const MARKER_COLUMN_W: f32 = 6.0;
pub(crate) const MARKER_BAR_W: f32 = 4.0;
pub(crate) const MARKER_BAR_RADIUS: f32 = 2.0;
pub(crate) const MARKER_BAR_INSET: f32 = 1.0;
pub(crate) const MARKER_DELETED_H: f32 = 8.0;
pub(crate) const MARKER_HOVER_GROW: f32 = 3.0;
const MARKER_DELETED_HIT_ROWS: f32 = 1.0 / 3.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MarkerRect {
    pub(crate) index: usize,
    pub(crate) kind: BlockKind,
    pub(crate) y: f32,
    pub(crate) h: f32,
    pub(crate) hit_y: f32,
    pub(crate) hit_h: f32,
}

pub(crate) fn marker_rects(
    blocks: &[Block],
    first_row: usize,
    row_h: f32,
    viewport_h: f32,
) -> Vec<MarkerRect> {
    if viewport_h <= 0.0 || row_h <= 0.0 {
        return Vec::new();
    }
    let mut rects = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let kind = block.kind();
        let (top, bottom, hit_top, hit_bottom) = match kind {
            BlockKind::Deleted => {
                let center = if block.lines.start == 0 {
                    row_h / 2.0
                } else {
                    (block.lines.start as f32 - first_row as f32) * row_h
                };
                let grow = row_h * MARKER_DELETED_HIT_ROWS;
                (
                    center - MARKER_DELETED_H / 2.0,
                    center + MARKER_DELETED_H / 2.0,
                    center - MARKER_DELETED_H / 2.0 - grow,
                    center + MARKER_DELETED_H / 2.0 + grow,
                )
            }
            BlockKind::Added | BlockKind::Modified => {
                let top = (block.lines.start as f32 - first_row as f32) * row_h + MARKER_BAR_INSET;
                let bottom = (block.lines.end as f32 - first_row as f32) * row_h - MARKER_BAR_INSET;
                (top, bottom, top, bottom)
            }
        };
        if bottom <= 0.0 || top >= viewport_h {
            continue;
        }
        let y = top.max(0.0);
        let h = bottom.min(viewport_h) - y;
        let hit_y = hit_top.max(0.0);
        let hit_h = hit_bottom.min(viewport_h) - hit_y;
        if h <= 0.0 {
            continue;
        }
        rects.push(MarkerRect {
            index,
            kind,
            y,
            h,
            hit_y,
            hit_h: hit_h.max(0.0),
        });
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(lines: std::ops::Range<u32>, base_lines: std::ops::Range<u32>) -> Block {
        Block {
            lines,
            base_lines,
            dirty: false,
            too_big: false,
        }
    }

    #[test]
    fn bars_cover_their_lines_with_a_one_pixel_inset() {
        let blocks = [block(2..4, 2..3), block(6..7, 5..5)];
        let rects = marker_rects(&blocks, 0, 18.0, 400.0);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].kind, BlockKind::Modified);
        assert_eq!(rects[0].y, 2.0 * 18.0 + 1.0);
        assert_eq!(rects[0].h, 2.0 * 18.0 - 2.0);
        assert_eq!(rects[0].hit_y, rects[0].y);
        assert_eq!(rects[1].kind, BlockKind::Added);
        assert_eq!(rects[1].index, 1);
    }

    #[test]
    fn a_deletion_is_an_eight_pixel_pill_centered_on_its_boundary() {
        let rects = marker_rects(&[block(5..5, 4..7)], 0, 18.0, 400.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].kind, BlockKind::Deleted);
        assert_eq!(rects[0].y, 5.0 * 18.0 - 4.0);
        assert_eq!(rects[0].h, 8.0);
        assert_eq!(rects[0].hit_y, 5.0 * 18.0 - 4.0 - 6.0);
        assert_eq!(rects[0].hit_h, 8.0 + 12.0);

        let at_top = marker_rects(&[block(0..0, 0..2)], 0, 18.0, 400.0);
        assert_eq!(at_top[0].y, 9.0 - 4.0, "brought back into the first row");
        assert_eq!(
            at_top[0].hit_y, 0.0,
            "the hit zone never leaves the viewport"
        );
    }

    #[test]
    fn only_the_visible_part_of_a_block_is_emitted() {
        let blocks = [
            block(0..10, 0..10),
            block(50..60, 50..60),
            block(90..95, 90..95),
        ];
        let rects = marker_rects(&blocks, 5, 18.0, 5.0 * 18.0);
        assert_eq!(rects.len(), 1, "blocks outside the viewport emit nothing");
        assert_eq!(rects[0].index, 0);
        assert_eq!(rects[0].y, 0.0, "clipped at the viewport top");
        assert_eq!(rects[0].h, 5.0 * 18.0 - 1.0);

        let partial = marker_rects(&blocks, 48, 18.0, 5.0 * 18.0);
        assert_eq!(partial.len(), 1);
        assert_eq!(partial[0].index, 1);
        assert_eq!(partial[0].y, 2.0 * 18.0 + 1.0);
        assert_eq!(
            partial[0].y + partial[0].h,
            5.0 * 18.0,
            "clipped at the bottom"
        );
        assert!(marker_rects(&blocks, 0, 18.0, 0.0).is_empty());
    }
}
