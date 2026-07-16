//! Scrollbar thumb + search match-rail paint pass.

use gpui::{Bounds, Hsla, Pixels, Point, Window, fill, px};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;

/// EP-006 US-017: match-rail tick height AND bucket size. One tick per
/// occupied 2 px bucket bounds the paint cost by the track height, never
/// by the match count (which is capped at 10 000 upstream anyway).
const TICK_PX: f32 = 2.0;

/// EP-006 US-017: project match positions (lines from the grid bottom)
/// onto the scrollbar track, decimated by [`TICK_PX`] buckets. Document
/// space is `total_lines` (history + visible screen); top of track = top
/// of scrollback. Returns each occupied bucket's tick-top Y relative to
/// the track top. Pure - unit-tested.
pub(crate) fn match_tick_offsets(
    lines_from_bottom: impl IntoIterator<Item = usize>,
    total_lines: usize,
    track_height: f32,
) -> Vec<f32> {
    if total_lines == 0 || track_height <= TICK_PX {
        return Vec::new();
    }
    let bucket_count = (track_height / TICK_PX).ceil() as usize;
    let mut occupied = vec![false; bucket_count];
    for l in lines_from_bottom {
        let doc_pos = (total_lines - 1).saturating_sub(l.min(total_lines - 1));
        let y = (doc_pos as f32 / total_lines as f32) * track_height;
        let idx = ((y / TICK_PX) as usize).min(bucket_count - 1);
        occupied[idx] = true;
    }
    occupied
        .iter()
        .enumerate()
        .filter_map(|(i, o)| o.then_some(i as f32 * TICK_PX))
        .collect()
}

/// EP-006 US-017: paint the search match rail - one [`TICK_PX`]-high quad
/// per occupied bucket, on the same 4 px right-edge strip as the thumb.
/// Painted whenever a search has matches, independent of the thumb (which
/// only shows while scrolled); rail click-to-jump is the EXISTING
/// proportional track click (`ScrollbarMetrics::offset_for_y` in the
/// view's mouse handler) - no per-tick hit-test needed.
pub fn paint_match_ticks(
    lines_from_bottom: &[usize],
    color: Hsla,
    layout: &LayoutState,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    if lines_from_bottom.is_empty() {
        return;
    }
    // Keep this visible_rows/total_lines derivation IDENTICAL to
    // `scrollbar_metrics` above - the rail and the thumb must agree on the
    // document space or ticks drift off the offsets the track click jumps to.
    let visible_rows = (bounds.size.height / geom.line_height).floor().max(1.0) as usize;
    let total_lines = layout.history_size + visible_rows;
    let track_height = bounds.size.height.as_f32();
    let strip_width = px(4.0);
    let strip_left = bounds.origin.x + bounds.size.width - strip_width;
    for y in match_tick_offsets(lines_from_bottom.iter().copied(), total_lines, track_height) {
        window.paint_quad(fill(
            Bounds::new(
                Point {
                    x: strip_left,
                    y: geom.origin.y + px(y),
                },
                gpui::Size {
                    width: strip_width,
                    height: px(TICK_PX),
                },
            ),
            color,
        ));
    }
}

/// US-015: resolved pixel geometry of the scrollbar strip + thumb for a single
/// frame. Computed in [`scrollbar_metrics`] (the single source of truth shared
/// with [`paint_scrollbar`]) and stashed by `TerminalElement::paint` into a
/// shared cell so the view's mouse handlers can hit-test clicks/drags against
/// the exact same geometry that was painted - no formula duplication.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollbarMetrics {
    /// Absolute window-space X of the strip's left edge.
    pub(crate) strip_left: Pixels,
    /// Visual strip width (the painted thumb width).
    pub(crate) strip_width: Pixels,
    /// Absolute window-space Y of the track top.
    pub(crate) track_top: Pixels,
    /// Pixel height of the full track.
    pub(crate) track_height: Pixels,
    /// Absolute window-space Y of the thumb top (valid even when the thumb is
    /// not painted because `display_offset == 0`).
    pub(crate) thumb_top: Pixels,
    /// Pixel height of the thumb.
    pub(crate) thumb_height: Pixels,
    /// Display offset represented by this painted frame.
    pub(crate) display_offset: usize,
    /// Scrollback line count, for pixel ↔ `display_offset` mapping.
    pub(crate) history_size: usize,
}

impl ScrollbarMetrics {
    /// Pixel course available to the thumb's top edge.
    pub(crate) fn thumb_travel(&self) -> Pixels {
        (self.track_height - self.thumb_height).max(px(0.0))
    }

    /// Map an absolute pixel Y on the track to a scrollback `display_offset`.
    /// Track top → fully scrolled back (`history_size`); track bottom → `0`.
    /// Inverse of the thumb-position formula in [`scrollbar_metrics`].
    pub(crate) fn offset_for_y(&self, abs_y: Pixels) -> usize {
        let thumb_travel = self.thumb_travel();
        if thumb_travel.as_f32() <= 0.0 || self.history_size == 0 {
            return 0;
        }
        let rel = ((abs_y - self.track_top) / thumb_travel).clamp(0.0, 1.0);
        let ratio = 1.0 - rel;
        (ratio * self.history_size as f32).round() as usize
    }

    /// Whether an absolute pixel X falls on the clickable strip. The hit zone
    /// is widened by `slop` pixels each side of the 4px visual strip so the
    /// thin strip is easy to grab without a pixel-perfect click.
    pub(crate) fn strip_contains_x(&self, x: Pixels, slop: Pixels) -> bool {
        x >= self.strip_left - slop && x <= self.strip_left + self.strip_width + slop
    }

    /// Whether an absolute pixel Y is on the thumb (a drag-grab) rather than
    /// the bare track (a click-to-jump).
    pub(crate) fn y_on_thumb(&self, abs_y: Pixels) -> bool {
        abs_y >= self.thumb_top && abs_y <= self.thumb_top + self.thumb_height
    }
}

/// Compute the scrollbar geometry for the current frame, or `None` when there
/// is no scrollback to scroll (`history_size == 0`). Single source of truth for
/// both the painted thumb ([`paint_scrollbar`]) and the view's mouse hit-test.
pub(crate) fn scrollbar_metrics(
    history_size: usize,
    display_offset: usize,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
) -> Option<ScrollbarMetrics> {
    let line_height = geom.line_height;
    let visible_rows = (bounds.size.height / line_height).floor().max(1.0) as usize;
    let total_lines = history_size + visible_rows;
    if history_size == 0 || total_lines == 0 {
        return None;
    }
    let strip_width = px(4.0);
    // Sit against the element's right edge. Use `bounds.origin.x` (NOT
    // `geom.origin.x`, which is shifted right by the 1-cell left gutter) so the
    // strip lands `strip_width` inside the right edge instead of `cell_width`
    // past it - past the edge it gets scissored away by the `bounds` content
    // mask, making both the painted thumb and the hit zone invisible.
    let strip_left = bounds.origin.x + bounds.size.width - strip_width;
    let track_height = bounds.size.height;
    let visible_ratio = visible_rows as f32 / total_lines as f32;
    let thumb_height = (track_height * visible_ratio)
        .max(px(16.0))
        .min(track_height);
    let scroll_ratio = display_offset as f32 / history_size as f32;
    // display_offset = max → scrolled to top → thumb at top.
    let thumb_y = track_height - thumb_height - (track_height - thumb_height) * scroll_ratio;
    Some(ScrollbarMetrics {
        strip_left,
        strip_width,
        track_top: geom.origin.y,
        track_height,
        thumb_top: geom.origin.y + thumb_y,
        thumb_height,
        display_offset,
        history_size,
    })
}

/// Paint the 4px right-edge scrollbar thumb. The thumb only shows while
/// scrolled away from the bottom; it is purely a position indicator.
pub fn paint_scrollbar(
    layout: &LayoutState,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    // Thumb - short-circuit when scrolled to the bottom and there is no
    // history (legacy behaviour preserved; the thumb is purely a position
    // indicator). Geometry comes from `scrollbar_metrics` so the painted
    // thumb and the view's interactive hit-test never diverge (US-015).
    if layout.display_offset == 0 || layout.history_size == 0 {
        return;
    }
    let Some(metrics) = scrollbar_metrics(layout.history_size, layout.display_offset, geom, bounds)
    else {
        return;
    };
    let scrollbar_bounds = Bounds::new(
        Point {
            x: metrics.strip_left,
            y: metrics.thumb_top,
        },
        gpui::Size {
            width: metrics.strip_width,
            height: metrics.thumb_height,
        },
    );
    window.paint_quad(fill(scrollbar_bounds, layout.scrollbar_thumb));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(track_top: f32, track_height: f32, history: usize) -> ScrollbarMetrics {
        ScrollbarMetrics {
            strip_left: px(796.0),
            strip_width: px(4.0),
            track_top: px(track_top),
            track_height: px(track_height),
            thumb_top: px(track_top),
            thumb_height: px(16.0),
            display_offset: 0,
            history_size: history,
        }
    }

    // EP-006 US-017 - match-rail projection + decimation.

    #[test]
    fn ticks_project_proportionally_top_to_bottom() {
        // total 1000 lines, 400px track. A match at the very top of the
        // scrollback (999 from bottom) → y≈0; at the live edge (0 from
        // bottom) → y near the track bottom.
        let ticks = match_tick_offsets([999usize, 0], 1000, 400.0);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0], 0.0);
        assert!(ticks[1] >= 396.0, "live-edge match lands at the bottom");
    }

    #[test]
    fn ticks_are_decimated_to_one_per_bucket() {
        // 10 000 matches over a 400px track can paint at most
        // track_height / TICK_PX = 200 ticks (US-017 AC: paint bounded by
        // the track, never the match count).
        let ticks = match_tick_offsets(0..10_000usize, 10_000, 400.0);
        assert!(ticks.len() <= 200, "got {} ticks", ticks.len());
        assert!(!ticks.is_empty());
    }

    #[test]
    fn ticks_adjacent_matches_share_a_bucket() {
        // Two matches one line apart in a huge document share one 2px
        // bucket → one tick.
        let ticks = match_tick_offsets([5000usize, 5001], 100_000, 400.0);
        assert_eq!(ticks.len(), 1);
    }

    #[test]
    fn ticks_empty_and_degenerate_inputs() {
        assert!(match_tick_offsets(std::iter::empty(), 1000, 400.0).is_empty());
        assert!(match_tick_offsets([5usize], 0, 400.0).is_empty());
        assert!(match_tick_offsets([5usize], 1000, 0.0).is_empty());
        // Out-of-range line (hostile/stale) is clamped, never panics.
        assert_eq!(match_tick_offsets([usize::MAX], 100, 400.0).len(), 1);
    }

    // US-015: top of track = fully scrolled back; bottom = at the live edge.
    #[test]
    fn offset_for_y_top_is_full_history() {
        let m = metrics(0.0, 400.0, 1000);
        assert_eq!(m.offset_for_y(px(0.0)), 1000);
    }

    #[test]
    fn offset_for_y_bottom_is_zero() {
        let m = metrics(0.0, 400.0, 1000);
        assert_eq!(m.offset_for_y(px(400.0)), 0);
    }

    #[test]
    fn offset_for_y_thumb_travel_midpoint_is_half_history() {
        let m = metrics(0.0, 400.0, 1000);
        let offset = m.offset_for_y(px(192.0));
        assert!(
            (offset as i64 - 500).abs() <= 1,
            "got {offset}, expected ~500"
        );
    }

    // Origin offset (non-zero track_top) is honoured.
    #[test]
    fn offset_for_y_respects_track_top() {
        let m = metrics(100.0, 400.0, 1000);
        assert_eq!(m.offset_for_y(px(100.0)), 1000); // at track top
        assert_eq!(m.offset_for_y(px(500.0)), 0); // at track bottom
    }

    // Out-of-range Y is clamped, never panics, never exceeds history.
    #[test]
    fn offset_for_y_clamps_out_of_range() {
        let m = metrics(0.0, 400.0, 500);
        assert_eq!(m.offset_for_y(px(-50.0)), 500); // above top → clamped
        assert_eq!(m.offset_for_y(px(9999.0)), 0); // below bottom → clamped
    }

    #[test]
    fn offset_for_y_zero_history_is_zero() {
        let m = metrics(0.0, 400.0, 0);
        assert_eq!(m.offset_for_y(px(0.0)), 0);
    }

    // Widened hit zone straddles the 4px visual strip.
    #[test]
    fn strip_contains_x_widened_hit_zone() {
        let m = metrics(0.0, 400.0, 1000); // strip_left=796, width=4 → [796,800]
        assert!(m.strip_contains_x(px(798.0), px(6.0))); // on the strip
        assert!(m.strip_contains_x(px(791.0), px(6.0))); // within left slop
        assert!(!m.strip_contains_x(px(780.0), px(6.0))); // outside
    }

    #[test]
    fn y_on_thumb_discriminates_track_from_thumb() {
        let mut m = metrics(0.0, 400.0, 1000);
        m.thumb_top = px(100.0);
        m.thumb_height = px(40.0); // thumb spans [100,140]
        assert!(m.y_on_thumb(px(120.0))); // on thumb → drag-grab
        assert!(!m.y_on_thumb(px(50.0))); // above thumb → track jump
        assert!(!m.y_on_thumb(px(300.0))); // below thumb → track jump
    }
}
