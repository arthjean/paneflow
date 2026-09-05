pub(crate) mod geometry;

use gpui::{
    AnyElement, App, Bounds, ElementId, InteractiveElement, IntoElement, ListState, MouseButton,
    MouseDownEvent, ParentElement, Pixels, Point, ScrollHandle, Styled, Window, div, px,
};

use crate::theme::UiColors;
use crate::ui_primitives::AnimatedHoverExt;

pub trait ScrollableHandle {
    fn viewport(&self) -> Bounds<Pixels>;
    fn max_offset(&self) -> Point<Pixels>;
    fn offset(&self) -> Point<Pixels>;
    fn set_offset(&self, offset: Point<Pixels>);
    fn drag_started(&self) {}
    fn drag_ended(&self) {}
}

impl ScrollableHandle for ScrollHandle {
    fn viewport(&self) -> Bounds<Pixels> {
        ScrollHandle::bounds(self)
    }

    fn max_offset(&self) -> Point<Pixels> {
        ScrollHandle::max_offset(self)
    }

    fn offset(&self) -> Point<Pixels> {
        ScrollHandle::offset(self)
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        ScrollHandle::set_offset(self, offset);
    }
}

impl ScrollableHandle for ListState {
    fn viewport(&self) -> Bounds<Pixels> {
        self.viewport_bounds()
    }

    fn max_offset(&self) -> Point<Pixels> {
        self.max_offset_for_scrollbar()
    }

    fn offset(&self) -> Point<Pixels> {
        self.scroll_px_offset_for_scrollbar()
    }

    fn set_offset(&self, offset: Point<Pixels>) {
        self.set_offset_from_scrollbar(offset);
    }

    fn drag_started(&self) {
        self.scrollbar_drag_started();
    }

    fn drag_ended(&self) {
        self.scrollbar_drag_ended();
    }
}

pub const SCROLLBAR_WIDTH: Pixels = px(6.);
pub const SCROLLBAR_GUTTER: Pixels = px(10.);
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
const NO_OVERFLOW_EPSILON: f32 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct ScrollDragState {
    pub start_mouse_y: Pixels,
    pub start_offset_y: Pixels,
}

pub fn begin_drag<H: ScrollableHandle>(handle: &H, mouse_y: Pixels) -> ScrollDragState {
    handle.drag_started();
    ScrollDragState {
        start_mouse_y: mouse_y,
        start_offset_y: handle.offset().y,
    }
}

pub fn end_drag<H: ScrollableHandle>(handle: &H, drag: Option<ScrollDragState>) -> bool {
    if drag.is_some() {
        handle.drag_ended();
        true
    } else {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarMetrics {
    pub viewport_h: f32,
    pub max_off_y: f32,
    pub thumb_h: f32,
    pub thumb_top: f32,
}

fn metrics_from(viewport_h: f32, max_off_y: f32, off_y: f32) -> Option<ScrollbarMetrics> {
    if viewport_h <= 0.0 || max_off_y < NO_OVERFLOW_EPSILON {
        return None;
    }
    let content_h = viewport_h + max_off_y;
    let thumb_h = (viewport_h * viewport_h / content_h)
        .max(SCROLLBAR_MIN_THUMB)
        .min(viewport_h);
    let progress = (-off_y / max_off_y).clamp(0.0, 1.0);
    Some(ScrollbarMetrics {
        viewport_h,
        max_off_y,
        thumb_h,
        thumb_top: progress * (viewport_h - thumb_h),
    })
}

pub fn metrics<H: ScrollableHandle>(handle: &H) -> Option<ScrollbarMetrics> {
    metrics_from(
        f32::from(handle.viewport().size.height),
        f32::from(handle.max_offset().y),
        f32::from(handle.offset().y),
    )
}

pub fn track_click_offset<H: ScrollableHandle>(handle: &H, mouse_y: Pixels) -> Option<f32> {
    let track_top = f32::from(handle.viewport().origin.y);
    let ScrollbarMetrics {
        viewport_h: track_h,
        max_off_y,
        thumb_h,
        ..
    } = metrics(handle)?;
    let click_y = (f32::from(mouse_y) - track_top).clamp(0.0, track_h);
    let target_thumb_top = (click_y - thumb_h / 2.0).clamp(0.0, track_h - thumb_h);
    let progress = if track_h - thumb_h > 0.0 {
        target_thumb_top / (track_h - thumb_h)
    } else {
        0.0
    };
    Some(-progress * max_off_y)
}

pub fn drag_offset<H: ScrollableHandle>(
    handle: &H,
    drag: &ScrollDragState,
    mouse_y: Pixels,
) -> Option<f32> {
    let ScrollbarMetrics {
        viewport_h,
        max_off_y,
        thumb_h,
        ..
    } = metrics(handle)?;
    let track_range = (viewport_h - thumb_h).max(1.0);
    let delta_mouse = f32::from(mouse_y - drag.start_mouse_y);
    let delta_offset = -delta_mouse * max_off_y / track_range;
    let start_off = f32::from(drag.start_offset_y);
    Some((start_off + delta_offset).clamp(-max_off_y, 0.0))
}

pub fn render<H: ScrollableHandle>(
    handle: &H,
    ui: UiColors,
    estimate: Option<(f32, f32)>,
    track_id: impl Into<ElementId>,
    thumb_id: impl Into<ElementId>,
    on_track_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_thumb_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    let real_viewport_h = f32::from(handle.viewport().size.height);
    let real_max_off_y = f32::from(handle.max_offset().y);
    let off_y = f32::from(handle.offset().y);

    let (viewport_h, max_off_y) = if real_viewport_h > 0.0 {
        (real_viewport_h, real_max_off_y)
    } else if let Some((est_content, est_max_viewport)) = estimate {
        let est_viewport = est_content.min(est_max_viewport);
        (est_viewport, (est_content - est_viewport).max(0.0))
    } else {
        return None;
    };

    let ScrollbarMetrics {
        viewport_h,
        thumb_h,
        thumb_top,
        ..
    } = metrics_from(viewport_h, max_off_y, off_y)?;

    let thumb_bg = ui.muted;
    let thumb_hover_bg = ui.text;

    Some(
        div()
            .absolute()
            .top_0()
            .right_0()
            .h(px(viewport_h))
            .w(SCROLLBAR_GUTTER)
            .child(
                div()
                    .id(track_id.into())
                    .absolute()
                    .top_0()
                    .right(px(2.))
                    .w(SCROLLBAR_WIDTH)
                    .h(px(viewport_h))
                    .rounded(px(3.))
                    .on_mouse_down(MouseButton::Left, on_track_click),
            )
            .child(
                div()
                    .id(thumb_id.into())
                    .absolute()
                    .top(px(thumb_top))
                    .right(px(2.))
                    .w(SCROLLBAR_WIDTH)
                    .h(px(thumb_h))
                    .rounded(px(3.))
                    .bg(thumb_bg)
                    .animated_hover_bg(thumb_bg, thumb_hover_bg)
                    .on_mouse_down(MouseButton::Left, on_thumb_down),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_overflow_returns_none() {
        let handle = ScrollHandle::new();
        assert!(track_click_offset(&handle, px(50.)).is_none());
    }

    #[test]
    fn metrics_are_proportional_and_clamped() {
        let top = metrics_from(400.0, 1200.0, 0.0).expect("overflow");
        assert_eq!(top.thumb_h, 400.0 * 400.0 / 1600.0);
        assert_eq!(top.thumb_top, 0.0);

        let bottom = metrics_from(400.0, 1200.0, -1200.0).expect("overflow");
        assert_eq!(bottom.thumb_h, top.thumb_h);
        assert_eq!(bottom.thumb_top, 400.0 - top.thumb_h);

        let over = metrics_from(400.0, 1200.0, -5000.0).expect("overflow");
        assert_eq!(over.thumb_top, bottom.thumb_top);

        let tiny = metrics_from(400.0, 4_000_000.0, 0.0).expect("overflow");
        assert_eq!(tiny.thumb_h, SCROLLBAR_MIN_THUMB);
    }

    #[test]
    fn metrics_none_without_overflow_or_layout() {
        assert!(metrics_from(400.0, 0.0, 0.0).is_none());
        assert!(metrics_from(400.0, NO_OVERFLOW_EPSILON / 2.0, 0.0).is_none());
        assert!(metrics_from(0.0, 1200.0, 0.0).is_none());
        assert!(metrics(&ScrollHandle::new()).is_none());
    }

    #[test]
    fn drag_offset_no_overflow_returns_none() {
        let handle = ScrollHandle::new();
        let drag = ScrollDragState {
            start_mouse_y: px(100.),
            start_offset_y: px(0.),
        };
        assert!(drag_offset(&handle, &drag, px(120.)).is_none());
    }
}
