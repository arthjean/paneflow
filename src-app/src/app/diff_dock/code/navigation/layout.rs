use gpui::{Bounds, Pixels, Point, point, px, size};

use super::super::element::CodeScroll;

pub(crate) use crate::widgets::scrollbar::geometry::{
    MIN_THUMB, SCROLLBAR_SIZE, Track, scrollbar_track,
};
pub(crate) const MINIMAP_FONT_SIZE: f32 = 2.0;
pub(crate) const MINIMAP_LINE_HEIGHT: f32 = MINIMAP_FONT_SIZE * 1.618;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigationPart {
    Vertical,
    Horizontal,
    Minimap,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct NavigationLayout {
    pub(crate) vertical: Option<Track>,
    pub(crate) horizontal: Option<Track>,
    pub(crate) minimap: Option<Track>,
    pub(crate) minimap_top: f64,
}

impl NavigationLayout {
    pub(crate) fn track(self, part: NavigationPart) -> Option<Track> {
        match part {
            NavigationPart::Vertical => self.vertical,
            NavigationPart::Horizontal => self.horizontal,
            NavigationPart::Minimap => self.minimap,
        }
    }

    pub(crate) fn part_at(self, position: Point<Pixels>) -> Option<NavigationPart> {
        [
            NavigationPart::Vertical,
            NavigationPart::Horizontal,
            NavigationPart::Minimap,
        ]
        .into_iter()
        .find(|part| {
            self.track(*part)
                .is_some_and(|track| track.bounds.contains(&position))
        })
    }
}

pub(crate) fn minimap_width(text_width: f32, char_width: f32, enabled: bool) -> f32 {
    let width = (text_width * 0.15).min(char_width * 80.0);
    if enabled && width >= char_width * 20.0 {
        width
    } else {
        0.0
    }
}

pub(crate) fn minimap_top(line_count: usize, scroll: &CodeScroll) -> f64 {
    let progress = if scroll.max_rows() > 0.0 {
        scroll.rows() / scroll.max_rows()
    } else {
        0.0
    };
    progress
        * (line_count as f64 - f64::from(scroll.viewport_height() / MINIMAP_LINE_HEIGHT)).max(0.0)
}

pub(crate) fn minimap_track(
    bounds: Bounds<Pixels>,
    line_count: usize,
    scroll: &CodeScroll,
) -> Track {
    let height = f32::from(bounds.size.height);
    let (top_line, visible_lines) = {
        let map = scroll.map();
        let top_line = map.line_position(scroll.rows());
        let bottom_line = map.line_position(scroll.rows() + scroll.visible_rows());
        (top_line, bottom_line - top_line)
    };
    let thumb_height = (visible_lines as f32 * MINIMAP_LINE_HEIGHT)
        .max(MIN_THUMB)
        .min(height);
    let top =
        ((top_line - minimap_top(line_count, scroll)) * f64::from(MINIMAP_LINE_HEIGHT)) as f32;
    Track {
        bounds,
        thumb: (height > 0.0 && scroll.map().total_rows() as f64 > scroll.visible_rows()).then(
            || {
                Bounds::new(
                    point(
                        bounds.origin.x,
                        bounds.origin.y + px(top.clamp(0.0, (height - thumb_height).max(0.0))),
                    ),
                    size(bounds.size.width, px(thumb_height)),
                )
            },
        ),
    }
}
