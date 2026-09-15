use gpui::{BorderStyle, Bounds, Corners, Edges, Hsla, Pixels, Point, Window, fill, px, quad};

use super::super::LayoutState;
use super::super::geometry::CellGeometry;
use crate::terminal::scrollbar_reveal::ScrollbarPresence;

fn grid_height(bounds: Bounds<Pixels>) -> Pixels {
    (bounds.size.height - px(crate::app::constants::PANE_CONTENT_INSET_Y) * 2.).max(px(0.0))
}

const TICK_PX: f32 = 2.0;
const SCROLLBAR_INSET: f32 = 2.0;
const SCROLLBAR_GUTTER: f32 = 10.0;
const SCROLLBAR_THUMB_REST: f32 = 6.0;
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
const SCROLLBAR_THUMB_RADIUS: f32 = 3.0;
const SCROLLBAR_HOVER_BOOST: f32 = 1.6;

fn gutter_left(bounds: Bounds<Pixels>) -> Pixels {
    bounds.origin.x + bounds.size.width - px(SCROLLBAR_INSET + SCROLLBAR_GUTTER)
}

fn with_alpha(color: Hsla, factor: f32) -> Hsla {
    Hsla {
        a: (color.a * factor).clamp(0.0, 1.0),
        ..color
    }
}

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
    let grid_height = grid_height(bounds);
    let visible_rows = (grid_height / geom.line_height).floor().max(1.0) as usize;
    let total_lines = layout.history_size + visible_rows;
    let track_height = grid_height.as_f32();
    let strip_width = px(SCROLLBAR_THUMB_REST);
    let strip_left = bounds.origin.x + bounds.size.width - px(SCROLLBAR_INSET) - strip_width;
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

#[derive(Clone, Copy, Debug)]
pub(crate) struct ScrollbarMetrics {
    pub(crate) strip_left: Pixels,
    pub(crate) strip_width: Pixels,
    pub(crate) track_top: Pixels,
    pub(crate) track_height: Pixels,
    pub(crate) thumb_top: Pixels,
    pub(crate) thumb_height: Pixels,
    pub(crate) display_offset: usize,
    pub(crate) history_size: usize,
}

impl ScrollbarMetrics {
    pub(crate) fn thumb_travel(&self) -> Pixels {
        (self.track_height - self.thumb_height).max(px(0.0))
    }

    pub(crate) fn offset_for_y(&self, abs_y: Pixels) -> usize {
        let thumb_travel = self.thumb_travel();
        if thumb_travel.as_f32() <= 0.0 || self.history_size == 0 {
            return 0;
        }
        let rel = ((abs_y - self.track_top) / thumb_travel).clamp(0.0, 1.0);
        let ratio = 1.0 - rel;
        (ratio * self.history_size as f32).round() as usize
    }

    pub(crate) fn strip_contains_x(&self, x: Pixels, slop: Pixels) -> bool {
        x >= self.strip_left - slop && x <= self.strip_left + self.strip_width + slop
    }

    pub(crate) fn track_contains(&self, position: Point<Pixels>, slop: Pixels) -> bool {
        self.strip_contains_x(position.x, slop)
            && position.y >= self.track_top
            && position.y <= self.track_top + self.track_height
    }

    pub(crate) fn y_on_thumb(&self, abs_y: Pixels) -> bool {
        abs_y >= self.thumb_top && abs_y <= self.thumb_top + self.thumb_height
    }

    pub(crate) fn track_bounds(&self) -> Bounds<Pixels> {
        Bounds::new(
            Point {
                x: self.strip_left,
                y: self.track_top,
            },
            gpui::Size {
                width: self.strip_width,
                height: self.track_height,
            },
        )
    }

    pub(crate) fn thumb_bounds(&self, expansion: f32) -> Bounds<Pixels> {
        let expansion = expansion.clamp(0.0, 1.0);
        let width = SCROLLBAR_THUMB_REST + (SCROLLBAR_GUTTER - SCROLLBAR_THUMB_REST) * expansion;
        Bounds::new(
            Point {
                x: self.strip_left + self.strip_width - px(width),
                y: self.thumb_top,
            },
            gpui::Size {
                width: px(width),
                height: self.thumb_height,
            },
        )
    }
}

pub(crate) fn scrollbar_metrics(
    history_size: usize,
    display_offset: usize,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
) -> Option<ScrollbarMetrics> {
    let line_height = geom.line_height;
    let visible_rows = (grid_height(bounds) / line_height).floor().max(1.0) as usize;
    let total_lines = history_size + visible_rows;
    if history_size == 0 || total_lines == 0 {
        return None;
    }
    let strip_width = px(SCROLLBAR_GUTTER);
    let strip_left = gutter_left(bounds);
    let track_height = grid_height(bounds);
    let visible_ratio = visible_rows as f32 / total_lines as f32;
    let thumb_height = (track_height * visible_ratio)
        .max(px(SCROLLBAR_MIN_THUMB))
        .min(track_height);
    let scroll_ratio = display_offset as f32 / history_size as f32;
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

pub fn paint_scrollbar(
    layout: &LayoutState,
    presence: ScrollbarPresence,
    geom: &CellGeometry,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    if !presence.is_visible() || layout.history_size == 0 {
        return;
    }
    let Some(metrics) = scrollbar_metrics(layout.history_size, layout.display_offset, geom, bounds)
    else {
        return;
    };
    if presence.expansion > 0.0 {
        window.paint_quad(quad(
            metrics.track_bounds(),
            Corners::all(px(SCROLLBAR_GUTTER / 2.0)),
            with_alpha(layout.scrollbar_track, presence.alpha * presence.expansion),
            Edges::default(),
            gpui::transparent_black(),
            BorderStyle::default(),
        ));
    }
    let thumb_boost = 1.0 + (SCROLLBAR_HOVER_BOOST - 1.0) * presence.expansion;
    window.paint_quad(quad(
        metrics.thumb_bounds(presence.expansion),
        Corners::all(px(SCROLLBAR_THUMB_RADIUS)),
        with_alpha(layout.scrollbar_thumb, presence.alpha * thumb_boost),
        Edges::default(),
        gpui::transparent_black(),
        BorderStyle::default(),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(track_top: f32, track_height: f32, history: usize) -> ScrollbarMetrics {
        ScrollbarMetrics {
            strip_left: px(788.0),
            strip_width: px(SCROLLBAR_GUTTER),
            track_top: px(track_top),
            track_height: px(track_height),
            thumb_top: px(track_top),
            thumb_height: px(SCROLLBAR_MIN_THUMB),
            display_offset: 0,
            history_size: history,
        }
    }

    #[test]
    fn thumb_rests_flush_right_and_fills_the_gutter_when_expanded() {
        let m = metrics(0.0, 400.0, 100);
        let rest = m.thumb_bounds(0.0);
        assert_eq!(
            rest.origin.x,
            px(788.0 + SCROLLBAR_GUTTER - SCROLLBAR_THUMB_REST)
        );
        assert_eq!(rest.size.width, px(SCROLLBAR_THUMB_REST));
        let expanded = m.thumb_bounds(1.0);
        assert_eq!(expanded.origin.x, px(788.0));
        assert_eq!(expanded.size.width, px(SCROLLBAR_GUTTER));
        let half = m.thumb_bounds(0.5);
        assert_eq!(half.size.width, px(8.0));
        assert_eq!(m.track_bounds().size.height, px(400.0));
    }

    #[test]
    fn track_contains_covers_the_gutter_with_slop_and_stops_at_the_track_ends() {
        let m = metrics(10.0, 400.0, 100);
        let slop = px(2.0);
        assert!(m.track_contains(
            Point {
                x: px(786.0),
                y: px(10.0)
            },
            slop
        ));
        assert!(m.track_contains(
            Point {
                x: px(800.0),
                y: px(410.0)
            },
            slop
        ));
        assert!(!m.track_contains(
            Point {
                x: px(785.0),
                y: px(200.0)
            },
            slop
        ));
        assert!(!m.track_contains(
            Point {
                x: px(790.0),
                y: px(9.0)
            },
            slop
        ));
        assert!(!m.track_contains(
            Point {
                x: px(790.0),
                y: px(411.0)
            },
            slop
        ));
    }

    #[test]
    fn ticks_project_proportionally_top_to_bottom() {
        let ticks = match_tick_offsets([999usize, 0], 1000, 400.0);
        assert_eq!(ticks.len(), 2);
        assert_eq!(ticks[0], 0.0);
        assert!(ticks[1] >= 396.0, "live-edge match lands at the bottom");
    }

    #[test]
    fn ticks_are_decimated_to_one_per_bucket() {
        let ticks = match_tick_offsets(0..10_000usize, 10_000, 400.0);
        assert!(ticks.len() <= 200, "got {} ticks", ticks.len());
        assert!(!ticks.is_empty());
    }

    #[test]
    fn ticks_adjacent_matches_share_a_bucket() {
        let ticks = match_tick_offsets([5000usize, 5001], 100_000, 400.0);
        assert_eq!(ticks.len(), 1);
    }

    #[test]
    fn ticks_empty_and_degenerate_inputs() {
        assert!(match_tick_offsets(std::iter::empty(), 1000, 400.0).is_empty());
        assert!(match_tick_offsets([5usize], 0, 400.0).is_empty());
        assert!(match_tick_offsets([5usize], 1000, 0.0).is_empty());
        assert_eq!(match_tick_offsets([usize::MAX], 100, 400.0).len(), 1);
    }

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
        let offset = m.offset_for_y(px(188.0));
        assert!(
            (offset as i64 - 500).abs() <= 1,
            "got {offset}, expected ~500"
        );
    }

    #[test]
    fn offset_for_y_respects_track_top() {
        let m = metrics(100.0, 400.0, 1000);
        assert_eq!(m.offset_for_y(px(100.0)), 1000);
        assert_eq!(m.offset_for_y(px(500.0)), 0);
    }

    #[test]
    fn offset_for_y_clamps_out_of_range() {
        let m = metrics(0.0, 400.0, 500);
        assert_eq!(m.offset_for_y(px(-50.0)), 500);
        assert_eq!(m.offset_for_y(px(9999.0)), 0);
    }

    #[test]
    fn offset_for_y_zero_history_is_zero() {
        let m = metrics(0.0, 400.0, 0);
        assert_eq!(m.offset_for_y(px(0.0)), 0);
    }

    #[test]
    fn strip_contains_x_widened_hit_zone() {
        let m = metrics(0.0, 400.0, 1000);
        assert!(m.strip_contains_x(px(798.0), px(6.0)));
        assert!(m.strip_contains_x(px(791.0), px(6.0)));
        assert!(!m.strip_contains_x(px(780.0), px(6.0)));
    }

    #[test]
    fn y_on_thumb_discriminates_track_from_thumb() {
        let mut m = metrics(0.0, 400.0, 1000);
        m.thumb_top = px(100.0);
        m.thumb_height = px(40.0);
        assert!(m.y_on_thumb(px(120.0)));
        assert!(!m.y_on_thumb(px(50.0)));
        assert!(!m.y_on_thumb(px(300.0)));
    }
}
