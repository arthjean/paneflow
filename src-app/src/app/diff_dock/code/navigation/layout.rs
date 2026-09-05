use gpui::{Bounds, Pixels, Point, point, px, size};

use super::super::element::{CODE_ROW_HEIGHT, CodeScroll};

pub(crate) const SCROLLBAR_SIZE: f32 = 15.0;
pub(crate) const MIN_THUMB: f32 = 25.0;
pub(crate) const MINIMAP_FONT_SIZE: f32 = 2.0;
pub(crate) const MINIMAP_LINE_HEIGHT: f32 = MINIMAP_FONT_SIZE * 1.618;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NavigationPart {
    Vertical,
    Horizontal,
    Minimap,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Track {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) thumb: Option<Bounds<Pixels>>,
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

pub(crate) fn scrollbar_track(
    bounds: Bounds<Pixels>,
    visible: f64,
    total: f64,
    offset: f64,
    horizontal: bool,
) -> Track {
    let length = f32::from(if horizontal {
        bounds.size.width
    } else {
        bounds.size.height
    });
    let thumb = if length > 0.0 && total > visible && visible > 0.0 {
        let thumb_length = (length * (visible / total) as f32)
            .max(MIN_THUMB)
            .min(length);
        let start = ((offset / (total - visible)).clamp(0.0, 1.0) as f32) * (length - thumb_length);
        Some(if horizontal {
            Bounds::new(
                point(bounds.origin.x + px(start), bounds.origin.y),
                size(px(thumb_length), bounds.size.height),
            )
        } else {
            Bounds::new(
                point(bounds.origin.x, bounds.origin.y + px(start)),
                size(bounds.size.width, px(thumb_length)),
            )
        })
    } else {
        None
    };
    Track { bounds, thumb }
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
    let thumb_height = (scroll.viewport_height() / CODE_ROW_HEIGHT * MINIMAP_LINE_HEIGHT)
        .max(MIN_THUMB)
        .min(height);
    let top =
        ((scroll.rows() - minimap_top(line_count, scroll)) * f64::from(MINIMAP_LINE_HEIGHT)) as f32;
    Track {
        bounds,
        thumb: (height > 0.0 && line_count as f64 > scroll.visible_rows()).then(|| {
            Bounds::new(
                point(
                    bounds.origin.x,
                    bounds.origin.y + px(top.clamp(0.0, (height - thumb_height).max(0.0))),
                ),
                size(bounds.size.width, px(thumb_height)),
            )
        }),
    }
}
