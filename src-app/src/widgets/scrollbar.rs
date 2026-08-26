//! Reusable visible scrollbar overlay for `overflow_y_scroll` div lists.
//!
//! GPUI's `overflow_y_scroll` enables wheel scrolling but doesn't render a
//! visible thumb (Zed's `crates/ui/src/components/scrollbar.rs` is a custom
//! `Element` that paints quads - out of scope for PaneFlow's hand-rolled
//! popover-style menus). This module provides a thin overlay built from
//! plain divs, plus the math + listener helpers each call site needs.
//!
//! ## Usage
//!
//! ```ignore
//! // 1. Add fields on the consuming Entity:
//! my_scroll: ScrollHandle,
//! my_drag: Option<ScrollDragState>,
//!
//! // 2. Attach to the scrollable list:
//! let list = div().overflow_y_scroll().track_scroll(&self.my_scroll);
//!
//! // 3. Render the overlay as a sibling inside a `relative()` wrapper:
//! let scrollbar = scrollbar::render(
//!     &self.my_scroll,
//!     ui,
//!     Some((estimated_content_h, max_viewport_h)),
//!     "my-track",
//!     "my-thumb",
//!     cx.listener(|this, ev: &MouseDownEvent, _, cx| {
//!         if let Some(off) = scrollbar::track_click_offset(&this.my_scroll, ev.position.y) {
//!             this.my_scroll.set_offset(Point::new(px(0.), px(off)));
//!             cx.notify();
//!         }
//!     }),
//!     cx.listener(|this, ev: &MouseDownEvent, _, cx| {
//!         this.my_drag = Some(scrollbar::begin_drag(&this.my_scroll, ev.position.y));
//!         cx.stop_propagation();
//!     }),
//! );
//!
//! // 4. Wire move/up/wheel on the wrapper or popover root:
//! div().relative()
//!     .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
//!     .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
//!         if let Some(drag) = this.my_drag
//!             && let Some(off) = scrollbar::drag_offset(&this.my_scroll, &drag, ev.position.y)
//!         {
//!             this.my_scroll.set_offset(Point::new(px(0.), px(off)));
//!             cx.notify();
//!         }
//!     }))
//!     .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| {
//!         if this.my_drag.take().is_some() { cx.notify(); }
//!     }))
//!     .child(list)
//!     .when_some(scrollbar, |d, sb| d.child(sb))
//! ```
//!
//! ## Sign convention (load-bearing!)
//!
//! GPUI clamps `ScrollHandle::max_offset()` to the **non-negative**
//! magnitude of the scrollable range (`div.rs:1948-1950`); the live
//! `offset()` is always `<= 0` (zero at the top, `-max_offset` when
//! scrolled to the bottom). Earlier code in this repo treated `max_offset`
//! as negative and silently broke the moment real bounds replaced the
//! first-frame fallback estimate.

use gpui::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    ParentElement, Pixels, ScrollHandle, Styled, Window, div, px,
};

use crate::theme::UiColors;
use crate::ui_primitives::AnimatedHoverExt;

/// Track + thumb width. Anything thinner is hard to grab.
pub const SCROLLBAR_WIDTH: Pixels = px(6.);
/// Padding the parent should reserve on the right edge so list content
/// doesn't run under the scrollbar.
pub const SCROLLBAR_GUTTER: Pixels = px(10.);
/// Minimum thumb height - below this the thumb is too small to grab even
/// when the content is huge relative to the viewport.
const SCROLLBAR_MIN_THUMB: f32 = 24.0;
/// `max_offset` magnitudes below this are treated as "no overflow" - the
/// content effectively fits, and showing a scrollbar would be visual
/// noise. Mirrors Zed's behaviour for tiny rounding artefacts.
const NO_OVERFLOW_EPSILON: f32 = 0.5;

/// Drag-to-scroll state. Captured at `mouse_down` on the thumb; mouse-move
/// computes the new offset from `(current_mouse_y - start_mouse_y)`.
#[derive(Debug, Clone, Copy)]
pub struct ScrollDragState {
    pub start_mouse_y: Pixels,
    pub start_offset_y: Pixels,
}

/// Capture the drag-start pose. Call from the thumb's `on_mouse_down`.
pub fn begin_drag(handle: &ScrollHandle, mouse_y: Pixels) -> ScrollDragState {
    ScrollDragState {
        start_mouse_y: mouse_y,
        start_offset_y: handle.offset().y,
    }
}

/// Resolved thumb geometry for one scroll position.
///
/// Every consumer - the painted thumb, a track click, a thumb drag - derives
/// its numbers from here, so the thumb the user sees and the thumb the user
/// grabs can never disagree. Previously the `thumb_h` formula was written out
/// three times in this file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarMetrics {
    /// Height of the scrollable viewport, which is also the track height.
    pub viewport_h: f32,
    /// Non-negative scrollable range (see the sign convention above).
    pub max_off_y: f32,
    /// Thumb height, floored at [`SCROLLBAR_MIN_THUMB`].
    pub thumb_h: f32,
    /// Thumb top, relative to the track top.
    pub thumb_top: f32,
}

/// Resolve the metrics from raw numbers. `None` when there is no laid-out
/// viewport or the content fits.
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

/// Thumb geometry for a live scroll handle, or `None` when the content fits or
/// the host has not been laid out yet.
pub fn metrics(handle: &ScrollHandle) -> Option<ScrollbarMetrics> {
    metrics_from(
        f32::from(handle.bounds().size.height),
        f32::from(handle.max_offset().y),
        f32::from(handle.offset().y),
    )
}

/// Compute the new offset.y when the user clicks anywhere on the track.
/// Returns the target offset (negative or zero) or `None` if there's no
/// overflow / no laid-out viewport yet. Centres the thumb on the click.
pub fn track_click_offset(handle: &ScrollHandle, mouse_y: Pixels) -> Option<f32> {
    let track_top = f32::from(handle.bounds().origin.y);
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

/// Compute the new offset.y while the user drags the thumb. Returns the
/// target offset or `None` if there's nothing to scroll.
pub fn drag_offset(handle: &ScrollHandle, drag: &ScrollDragState, mouse_y: Pixels) -> Option<f32> {
    let ScrollbarMetrics {
        viewport_h,
        max_off_y,
        thumb_h,
        ..
    } = metrics(handle)?;
    let track_range = (viewport_h - thumb_h).max(1.0);
    let delta_mouse = f32::from(mouse_y - drag.start_mouse_y);
    // Dragging the thumb DOWN (positive delta_mouse) makes the offset
    // MORE negative - content scrolls up, revealing rows below.
    let delta_offset = -delta_mouse * max_off_y / track_range;
    let start_off = f32::from(drag.start_offset_y);
    Some((start_off + delta_offset).clamp(-max_off_y, 0.0))
}

/// Render the scrollbar overlay (track + thumb). Caller wraps the
/// scrollable list and this element in a `relative()` container.
///
/// `estimate` is `(content_h, max_viewport_h)` and is used as a fallback
/// for the first frame, before `track_scroll` has populated the handle's
/// real bounds. Pass `None` to skip the fallback - the scrollbar then
/// only appears once real bounds exist (1+ frame after open).
///
/// Returns `None` when the content fits the viewport (no scrolling
/// possible) - caller should `when_some` the result onto its child list.
pub fn render(
    handle: &ScrollHandle,
    ui: UiColors,
    estimate: Option<(f32, f32)>,
    track_id: impl Into<ElementId>,
    thumb_id: impl Into<ElementId>,
    on_track_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    on_thumb_down: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> Option<AnyElement> {
    let real_viewport_h = f32::from(handle.bounds().size.height);
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

    /// Pure-math sanity check on the offset math. Construct a fake
    /// "scenario" by abusing what we know about the helpers: they work
    /// purely on the scroll handle's reported bounds/offset/max_offset,
    /// so we can exercise edge cases without spinning up a GPUI window.
    #[test]
    fn no_overflow_returns_none() {
        let handle = ScrollHandle::new();
        // Fresh handle: bounds and max_offset are zero - no overflow.
        assert!(track_click_offset(&handle, px(50.)).is_none());
    }

    /// The three consumers (paint, track click, thumb drag) now read one
    /// formula. Pin the shape so a future edit cannot silently reintroduce a
    /// second one.
    #[test]
    fn metrics_are_proportional_and_clamped() {
        // 400px viewport over 1600px of content: 1200px of scrollable range.
        let top = metrics_from(400.0, 1200.0, 0.0).expect("overflow");
        assert_eq!(top.thumb_h, 400.0 * 400.0 / 1600.0);
        assert_eq!(top.thumb_top, 0.0);

        let bottom = metrics_from(400.0, 1200.0, -1200.0).expect("overflow");
        assert_eq!(bottom.thumb_h, top.thumb_h);
        assert_eq!(bottom.thumb_top, 400.0 - top.thumb_h);

        // Overscroll in either direction stays on the track.
        let over = metrics_from(400.0, 1200.0, -5000.0).expect("overflow");
        assert_eq!(over.thumb_top, bottom.thumb_top);

        // A huge document still leaves a grabbable thumb.
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
