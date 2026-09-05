use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, BorderStyle, Context, Corners, DispatchPhase, Edges, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    ScrollHandle, StatefulInteractiveElement, Styled, canvas, div, point, px, quad,
};

use super::scrollbar::geometry::{SCROLLBAR_SIZE, Track, scrollbar_track};

#[derive(Clone, Copy)]
struct Drag {
    mouse_y: Pixels,
    offset: f32,
    units_per_pixel: f32,
}

#[derive(Clone, Default)]
pub(crate) struct EditorScrollbar {
    track: Rc<Cell<Track>>,
    drag: Rc<Cell<Option<Drag>>>,
    hovered: Rc<Cell<bool>>,
}

impl EditorScrollbar {
    pub(crate) fn render<T: 'static>(
        &self,
        scroll: &ScrollHandle,
        cx: &mut Context<T>,
    ) -> AnyElement {
        let down = self.clone();
        let handle = scroll.clone();
        let paint = self.clone();
        let paint_handle = scroll.clone();
        let owner = cx.entity().downgrade();
        let ui = crate::theme::ui_colors();
        let thumb_color = crate::theme::active_theme().scrollbar_thumb;
        div()
            .id("editor-vertical-scrollbar")
            .w(px(SCROLLBAR_SIZE))
            .h_full()
            .flex_none()
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let track = down.track.get();
                    let Some(thumb) = track.thumb else {
                        return;
                    };
                    let max = f32::from(handle.max_offset().y).max(0.0);
                    let units_per_pixel =
                        max / f32::from(track.bounds.size.height - thumb.size.height).max(1.0);
                    let mut offset = f32::from(-handle.offset().y).clamp(0.0, max);
                    if !thumb.contains(&ev.position) {
                        offset = (f32::from(
                            ev.position.y - track.bounds.top() - thumb.size.height / 2.0,
                        ) * units_per_pixel)
                            .clamp(0.0, max);
                        handle.set_offset(point(handle.offset().x, px(-offset)));
                    }
                    down.drag.set(Some(Drag {
                        mouse_y: ev.position.y,
                        offset,
                        units_per_pixel,
                    }));
                    cx.notify();
                }),
            )
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, _| {
                        let visible = f64::from(f32::from(paint_handle.bounds().size.height));
                        let max = f64::from(f32::from(paint_handle.max_offset().y).max(0.0));
                        let track = scrollbar_track(
                            bounds,
                            visible,
                            visible + max,
                            f64::from(f32::from(-paint_handle.offset().y)),
                            false,
                        );
                        paint.track.set(track);
                        let edges = Edges {
                            left: px(1.),
                            ..Default::default()
                        };
                        window.paint_quad(quad(
                            bounds,
                            Corners::default(),
                            gpui::transparent_black(),
                            edges,
                            ui.border,
                            BorderStyle::Solid,
                        ));
                        if let Some(thumb) = track.thumb {
                            let color = if paint.drag.get().is_some() {
                                thumb_color.blend(ui.text.opacity(0.2))
                            } else if paint.hovered.get() {
                                thumb_color.blend(ui.text.opacity(0.1))
                            } else {
                                thumb_color
                            };
                            window.paint_quad(quad(
                                thumb,
                                Corners::default(),
                                color,
                                edges,
                                ui.border,
                                BorderStyle::Solid,
                            ));
                        }
                        let moving = paint.clone();
                        let handle = paint_handle.clone();
                        let moving_owner = owner.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let hovered = moving
                                .track
                                .get()
                                .thumb
                                .is_some_and(|thumb| thumb.contains(&ev.position));
                            let mut changed = moving.hovered.replace(hovered) != hovered;
                            if ev.pressed_button != Some(MouseButton::Left) {
                                changed |= moving.drag.take().is_some();
                            } else if let Some(drag) = moving.drag.get() {
                                let max = f32::from(handle.max_offset().y).max(0.0);
                                let offset = (drag.offset
                                    + f32::from(ev.position.y - drag.mouse_y)
                                        * drag.units_per_pixel)
                                    .clamp(0.0, max);
                                handle.set_offset(point(handle.offset().x, px(-offset)));
                                changed = true;
                                cx.stop_propagation();
                            }
                            if changed {
                                let _ = moving_owner.update(cx, |_, cx| cx.notify());
                            }
                        });
                        let released = paint.clone();
                        let released_owner = owner.clone();
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && ev.button == MouseButton::Left
                                && released.drag.take().is_some()
                            {
                                let _ = released_owner.update(cx, |_, cx| cx.notify());
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element()
    }
}
