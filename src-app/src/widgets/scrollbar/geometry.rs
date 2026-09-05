use gpui::{Bounds, Pixels, point, px, size};

pub(crate) const SCROLLBAR_SIZE: f32 = 15.0;
pub(crate) const MIN_THUMB: f32 = 25.0;

#[derive(Clone, Copy, Default)]
pub(crate) struct Track {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) thumb: Option<Bounds<Pixels>>,
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
