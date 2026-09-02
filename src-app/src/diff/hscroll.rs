use super::rows::FileSpan;

pub(crate) const DIFF_CHAR_WIDTH: f32 = 7.5;
pub(crate) const H_SCROLLBAR_TRACK_HEIGHT: f32 = 6.0;
pub(crate) const H_SCROLLBAR_PAD_X: f32 = 12.0;
pub(crate) const H_SCROLLBAR_BOTTOM_INSET: f32 = 3.0;
pub(crate) const H_SCROLLBAR_MIN_THUMB: f32 = 28.0;
pub(crate) const H_SCROLLBAR_EPSILON: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct HScrollbarSegment {
    pub offset_idx: usize,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub thumb_x: f32,
    pub thumb_width: f32,
    pub max_scroll: f32,
    pub offset: f32,
}

pub(crate) fn h_text_viewport(split: bool, panel_width: f32) -> f32 {
    if split {
        (panel_width - 1.0) / 2.0 - 55.0
    } else {
        panel_width - 92.0
    }
}

pub(crate) fn max_h_scroll(max_chars: usize, split: bool, panel_width: f32) -> f32 {
    let content_w = max_chars as f32 * DIFF_CHAR_WIDTH + 12.0;
    (content_w - h_text_viewport(split, panel_width)).max(0.0)
}

pub(crate) fn file_at_row(spans: &[FileSpan], row: usize) -> Option<usize> {
    if spans.is_empty() {
        return None;
    }
    Some(
        spans
            .partition_point(|s| s.header_row <= row)
            .saturating_sub(1),
    )
}

pub(crate) fn h_offset_len(file_count: usize, split: bool) -> usize {
    if split {
        file_count.saturating_mul(3)
    } else {
        file_count
    }
}

pub(crate) fn h_offset_index(
    file_count: usize,
    file_idx: usize,
    split: bool,
    right: bool,
) -> usize {
    if split {
        file_count + file_idx.saturating_mul(2) + usize::from(right)
    } else {
        file_idx
    }
}

fn side_max_chars(span: &FileSpan, split: bool, right: bool) -> usize {
    if split && right {
        span.right_max_chars.unwrap_or(0)
    } else {
        span.max_chars
    }
}

pub(crate) fn split_right_side_at_x(x: f32, panel_width: f32) -> bool {
    let divider_w = 3.0;
    let half_w = ((panel_width - divider_w) / 2.0).max(0.0);
    x >= half_w + divider_w
}

pub(crate) fn file_side_offset(
    spans: &[FileSpan],
    offsets: &[f32],
    idx: usize,
    right: bool,
    split: bool,
    panel_width: f32,
) -> f32 {
    let Some(span) = spans.get(idx) else {
        return 0.0;
    };
    let max = max_h_scroll(side_max_chars(span, split, right), split, panel_width);
    let offset_idx = h_offset_index(spans.len(), idx, split, right);
    offsets
        .get(offset_idx)
        .copied()
        .unwrap_or(0.0)
        .clamp(0.0, max)
}

pub(crate) fn set_file_side_offset(
    offsets: &mut Vec<f32>,
    spans: &[FileSpan],
    idx: usize,
    right: bool,
    value: f32,
    split: bool,
    panel_width: f32,
) {
    let needed = h_offset_len(spans.len(), split);
    if offsets.len() < needed {
        offsets.resize(needed, 0.0);
    }
    let max_chars = spans
        .get(idx)
        .map(|span| side_max_chars(span, split, right))
        .unwrap_or(0);
    let offset_idx = h_offset_index(spans.len(), idx, split, right);
    if let Some(slot) = offsets.get_mut(offset_idx) {
        *slot = value.clamp(0.0, max_h_scroll(max_chars, split, panel_width));
    }
}

#[allow(clippy::too_many_arguments)]
fn push_segment(
    out: &mut Vec<HScrollbarSegment>,
    offset_idx: usize,
    x: f32,
    y: f32,
    width: f32,
    text_viewport_w: f32,
    max_scroll: f32,
    offset: f32,
) {
    if width <= 0.0 || text_viewport_w <= 0.0 || max_scroll < H_SCROLLBAR_EPSILON {
        return;
    }

    let content_w = text_viewport_w + max_scroll;
    let thumb_width = (width * text_viewport_w / content_w)
        .max(H_SCROLLBAR_MIN_THUMB)
        .min(width);
    let progress = (offset / max_scroll).clamp(0.0, 1.0);
    let thumb_x = progress * (width - thumb_width).max(0.0);

    out.push(HScrollbarSegment {
        offset_idx,
        x,
        y,
        width,
        thumb_x,
        thumb_width,
        max_scroll,
        offset,
    });
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn h_scrollbar_segments(
    spans: &[FileSpan],
    row_offsets: &[f32],
    h_offsets: &[f32],
    split: bool,
    panel_width: f32,
    visible_top: f32,
    visible_bottom: f32,
) -> Vec<HScrollbarSegment> {
    if spans.is_empty() || row_offsets.len() < 2 || panel_width <= 0.0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    let text_viewport_w = h_text_viewport(split, panel_width).max(1.0);
    let track_y_inset = H_SCROLLBAR_BOTTOM_INSET + H_SCROLLBAR_TRACK_HEIGHT;
    let row_count = row_offsets.len().saturating_sub(1);
    if row_count == 0 || visible_bottom <= visible_top {
        return out;
    }

    let first_row = row_offsets
        .partition_point(|&o| o <= visible_top)
        .saturating_sub(1)
        .min(row_count.saturating_sub(1));
    let last_row_exclusive = row_offsets
        .partition_point(|&o| o < visible_bottom)
        .min(row_count);
    if first_row >= last_row_exclusive {
        return out;
    }
    let last_row = last_row_exclusive.saturating_sub(1);
    let first_file = spans
        .partition_point(|span| span.header_row <= first_row)
        .saturating_sub(1)
        .min(spans.len().saturating_sub(1));
    let last_file_exclusive = spans
        .partition_point(|span| span.header_row <= last_row)
        .min(spans.len());
    out.reserve(
        last_file_exclusive
            .saturating_sub(first_file)
            .saturating_mul(if split { 2 } else { 1 }),
    );

    for file_idx in first_file..last_file_exclusive {
        let span = &spans[file_idx];
        let start_row = span.header_row.min(row_offsets.len().saturating_sub(1));
        let end_row = spans
            .get(file_idx + 1)
            .map(|next| next.header_row)
            .unwrap_or_else(|| row_offsets.len().saturating_sub(1))
            .min(row_offsets.len().saturating_sub(1));
        let file_top = row_offsets.get(start_row).copied().unwrap_or(0.0);
        let file_bottom = row_offsets.get(end_row).copied().unwrap_or(file_top);
        let visible_file_top = file_top.max(visible_top);
        let visible_file_bottom = file_bottom.min(visible_bottom);
        if visible_file_bottom - visible_file_top <= track_y_inset {
            continue;
        }

        let y = visible_file_bottom - track_y_inset;

        if split {
            let divider_w = 3.0;
            let half_w = ((panel_width - divider_w) / 2.0).max(0.0);
            let track_w = (half_w - H_SCROLLBAR_PAD_X * 2.0).max(0.0);
            for (right, x) in [
                (false, H_SCROLLBAR_PAD_X),
                (true, half_w + divider_w + H_SCROLLBAR_PAD_X),
            ] {
                let max_scroll = max_h_scroll(side_max_chars(span, true, right), true, panel_width);
                let offset_idx = h_offset_index(spans.len(), file_idx, true, right);
                let offset = h_offsets
                    .get(offset_idx)
                    .copied()
                    .unwrap_or(0.0)
                    .clamp(0.0, max_scroll);
                push_segment(
                    &mut out,
                    offset_idx,
                    x,
                    y,
                    track_w,
                    text_viewport_w,
                    max_scroll,
                    offset,
                );
            }
        } else {
            let max_scroll = max_h_scroll(span.max_chars, false, panel_width);
            let offset_idx = h_offset_index(spans.len(), file_idx, false, false);
            let offset = h_offsets
                .get(offset_idx)
                .copied()
                .unwrap_or(0.0)
                .clamp(0.0, max_scroll);
            push_segment(
                &mut out,
                offset_idx,
                H_SCROLLBAR_PAD_X,
                y,
                (panel_width - H_SCROLLBAR_PAD_X * 2.0).max(0.0),
                text_viewport_w,
                max_scroll,
                offset,
            );
        }
    }

    out
}

pub(crate) fn h_scrollbar_click_offset(
    segments: &[HScrollbarSegment],
    x: f32,
    y: f32,
) -> Option<(usize, f32)> {
    let segment = segments.iter().find(|segment| {
        x >= segment.x
            && x <= segment.x + segment.width
            && y >= segment.y
            && y <= segment.y + H_SCROLLBAR_TRACK_HEIGHT
    })?;
    let travel = (segment.width - segment.thumb_width).max(0.0);
    let target_thumb_x = (x - segment.x - segment.thumb_width / 2.0).clamp(0.0, travel);
    let progress = if travel > 0.0 {
        target_thumb_x / travel
    } else {
        0.0
    };
    Some((segment.offset_idx, progress * segment.max_scroll))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(header_row: usize, max_chars: usize) -> FileSpan {
        FileSpan {
            header_row,
            max_chars,
            right_max_chars: None,
        }
    }

    fn split_span(header_row: usize, left_max_chars: usize, right_max_chars: usize) -> FileSpan {
        FileSpan {
            header_row,
            max_chars: left_max_chars,
            right_max_chars: Some(right_max_chars),
        }
    }

    #[test]
    fn max_h_scroll_zero_when_fits_then_grows() {
        assert_eq!(max_h_scroll(10, false, 600.0), 0.0);
        assert!(max_h_scroll(400, false, 600.0) > 0.0);
        assert!(max_h_scroll(120, false, 300.0) > max_h_scroll(120, false, 600.0));
    }

    #[test]
    fn file_at_row_maps_rows_to_files() {
        let spans = [span(0, 50), span(8, 50), span(20, 50)];
        assert_eq!(file_at_row(&spans, 0), Some(0));
        assert_eq!(file_at_row(&spans, 7), Some(0));
        assert_eq!(file_at_row(&spans, 8), Some(1));
        assert_eq!(file_at_row(&spans, 25), Some(2));
        assert_eq!(file_at_row(&[], 3), None);
    }

    #[test]
    fn set_file_side_offset_clamps_and_resizes() {
        let spans = [span(0, 400), span(10, 5)];
        let mut offsets = Vec::new();
        set_file_side_offset(&mut offsets, &spans, 0, false, 1_000_000.0, false, 600.0);
        assert_eq!(offsets.len(), 2);
        assert!(offsets[0] > 0.0 && offsets[0] <= max_h_scroll(400, false, 600.0));
        set_file_side_offset(&mut offsets, &spans, 1, false, 500.0, false, 600.0);
        assert_eq!(offsets[1], 0.0);
        set_file_side_offset(&mut offsets, &spans, 0, false, -50.0, false, 600.0);
        assert_eq!(offsets[0], 0.0);
    }

    #[test]
    fn split_side_offsets_are_independent() {
        let spans = [split_span(0, 400, 300)];
        let mut offsets = Vec::new();

        set_file_side_offset(&mut offsets, &spans, 0, false, 120.0, true, 600.0);
        let left_idx = h_offset_index(spans.len(), 0, true, false);
        let right_idx = h_offset_index(spans.len(), 0, true, true);
        assert_eq!(offsets.len(), h_offset_len(spans.len(), true));
        assert_eq!(offsets[left_idx], 120.0);
        assert_eq!(offsets[right_idx], 0.0);

        set_file_side_offset(&mut offsets, &spans, 0, true, 80.0, true, 600.0);
        assert_eq!(offsets[left_idx], 120.0);
        assert_eq!(offsets[right_idx], 80.0);
    }

    #[test]
    fn h_scrollbar_segments_only_considers_visible_files() {
        let spans = [span(0, 400), span(10, 400), span(20, 400)];
        let row_offsets: Vec<f32> = (0..=30).map(|row| row as f32 * 18.0).collect();
        let offsets = vec![0.0; spans.len()];

        let segments = h_scrollbar_segments(
            &spans,
            &row_offsets,
            &offsets,
            false,
            300.0,
            12.0,
            14.0 * 18.0,
        );

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.offset_idx)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    #[test]
    fn h_scrollbar_segments_includes_owner_when_view_starts_mid_file() {
        let spans = [span(0, 400), span(20, 400)];
        let row_offsets: Vec<f32> = (0..=40).map(|row| row as f32 * 18.0).collect();
        let offsets = vec![0.0; spans.len()];

        let segments = h_scrollbar_segments(
            &spans,
            &row_offsets,
            &offsets,
            false,
            300.0,
            8.0 * 18.0,
            11.0 * 18.0,
        );

        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].offset_idx, 0);
    }
}
