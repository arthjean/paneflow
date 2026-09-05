use std::cell::Cell;

use gpui::{
    DispatchPhase, Entity, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, Window,
};

use super::element::CodeScroll;
use super::view::CodeView;

mod layout;

pub(crate) use layout::{
    MINIMAP_FONT_SIZE, MINIMAP_LINE_HEIGHT, NavigationLayout, NavigationPart, SCROLLBAR_SIZE,
    Track, minimap_top, minimap_track, minimap_width, scrollbar_track,
};

#[derive(Clone, Copy)]
pub(crate) struct NavigationDrag {
    part: NavigationPart,
    mouse: Point<Pixels>,
    offset: f64,
    units_per_pixel: f64,
}

#[derive(Default)]
pub(crate) struct NavigationState {
    pub(crate) layout: Cell<NavigationLayout>,
    pub(crate) hovered: Option<NavigationPart>,
    pub(crate) drag: Option<NavigationDrag>,
}

impl NavigationState {
    pub(crate) fn dragging(&self, part: NavigationPart) -> bool {
        self.drag.is_some_and(|drag| drag.part == part)
    }

    pub(crate) fn mouse_down(
        &mut self,
        position: Point<Pixels>,
        scroll: &CodeScroll,
        h_offset: &mut f32,
        max_h: f32,
    ) -> bool {
        let layout = self.layout.get();
        let Some(part) = layout.part_at(position) else {
            return false;
        };
        let Some(track) = layout.track(part) else {
            return false;
        };
        let Some(thumb) = track.thumb else {
            return true;
        };
        let horizontal = part == NavigationPart::Horizontal;
        let (track_length, thumb_length, mouse, origin, max) = if horizontal {
            (
                track.bounds.size.width,
                thumb.size.width,
                position.x,
                track.bounds.origin.x,
                f64::from(max_h),
            )
        } else {
            (
                track.bounds.size.height,
                thumb.size.height,
                position.y,
                track.bounds.origin.y,
                scroll.max_rows(),
            )
        };
        let units_per_pixel = if part == NavigationPart::Minimap {
            let total_rows = scroll.max_rows() + scroll.visible_rows();
            1.0 / (f64::from(f32::from(track_length)) / total_rows)
                .min(f64::from(MINIMAP_LINE_HEIGHT))
                .max(f64::EPSILON)
        } else {
            max / f64::from(f32::from(track_length - thumb_length).max(1.0))
        };
        let mut offset = (if horizontal {
            f64::from(*h_offset)
        } else {
            scroll.rows()
        })
        .clamp(0.0, max);
        if !thumb.contains(&position) {
            offset = if part == NavigationPart::Minimap {
                layout.minimap_top
                    + f64::from(f32::from(mouse - origin)) / f64::from(MINIMAP_LINE_HEIGHT)
                    - scroll.visible_rows() / 2.0
            } else {
                f64::from(f32::from(mouse - origin - thumb_length / 2.0)) * units_per_pixel
            }
            .clamp(0.0, max);
            if horizontal {
                *h_offset = offset as f32;
            } else {
                scroll.set_rows(offset);
            }
        }
        self.drag = Some(NavigationDrag {
            part,
            mouse: position,
            offset,
            units_per_pixel,
        });
        true
    }

    pub(crate) fn mouse_move(
        &mut self,
        position: Point<Pixels>,
        pressed: bool,
        scroll: &CodeScroll,
        h_offset: &mut f32,
        max_h: f32,
    ) -> bool {
        let layout = self.layout.get();
        let hovered = layout.part_at(position).filter(|part| {
            layout
                .track(*part)
                .and_then(|track| track.thumb)
                .is_some_and(|thumb| thumb.contains(&position))
        });
        let mut changed = hovered != self.hovered;
        self.hovered = hovered;
        if !pressed {
            return self.drag.take().is_some() || changed;
        }
        if let Some(drag) = self.drag {
            let horizontal = drag.part == NavigationPart::Horizontal;
            let delta = if horizontal {
                position.x - drag.mouse.x
            } else {
                position.y - drag.mouse.y
            };
            let offset = drag.offset + f64::from(f32::from(delta)) * drag.units_per_pixel;
            if horizontal {
                let next = (offset as f32).clamp(0.0, max_h);
                changed |= *h_offset != next;
                *h_offset = next;
            } else {
                changed |= scroll.set_rows(offset);
            }
        }
        changed
    }
}

pub(crate) fn bind_drag(view: &Entity<CodeView>, window: &mut Window) {
    let moving = view.downgrade();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
        if phase == DispatchPhase::Capture {
            let _ = moving.update(cx, |view, cx| {
                if view.navigation.drag.is_some() {
                    view.on_scrollbar_move(event, cx);
                    cx.stop_propagation();
                }
            });
        }
    });
    let released = view.downgrade();
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
        if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
            let _ = released.update(cx, |view, cx| {
                if view.navigation.drag.is_some() {
                    view.on_scrollbar_up(event, cx);
                    cx.stop_propagation();
                }
            });
        }
    });
}

#[cfg(test)]
mod tests;
