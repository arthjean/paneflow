use std::os::fd::OwnedFd;

use paneflow_browser_proto::surface::{BYTES_PER_PIXEL, Rect, View, frame_bytes};
use paneflow_browser_proto::{Event, FrameDescriptor, PixelFormat, TabId};

use crate::shm::{Layer, Slots};

pub enum Out {
    Event(Event),
    Surface(Event, Vec<OwnedFd>),
}

#[derive(Default)]
struct PopupLayer {
    shown: bool,
    requested: Rect,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

pub struct Presentation {
    tab: TabId,
    view: View,
    displayed: bool,
    slots: Option<Slots>,
    next_generation: u64,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    popup: PopupLayer,
    stale: bool,
    superseded: Option<(u32, u32)>,
    failed: Option<(u32, u32)>,
    frames_sent: u64,
}

impl Presentation {
    pub fn new(tab: TabId) -> Self {
        Self {
            tab,
            view: View::default(),
            displayed: false,
            slots: None,
            next_generation: 1,
            pixels: Vec::new(),
            width: 0,
            height: 0,
            popup: PopupLayer::default(),
            stale: false,
            superseded: None,
            failed: None,
            frames_sent: 0,
        }
    }

    #[cfg(test)]
    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    pub fn present(&mut self, view: Option<View>) -> (bool, Vec<Out>) {
        match view {
            Some(view) => {
                let resized = view != self.view;
                if resized && differs(view.pixel_size(), self.view.pixel_size()) {
                    self.superseded = Some(self.view.pixel_size());
                }
                self.view = view;
                let shown = !self.displayed;
                self.displayed = true;
                self.failed = None;
                let mut out = Vec::new();
                if !resized && self.width > 0 {
                    out.extend(self.compose());
                }
                (resized || shown, out)
            }
            None => {
                self.displayed = false;
                self.slots = None;
                self.stale = false;
                (false, Vec::new())
            }
        }
    }

    pub fn paint_view(&mut self, buffer: &[u8], width: u32, height: u32) -> Vec<Out> {
        let Some(len) = frame_bytes(width, height) else {
            return Vec::new();
        };
        if buffer.len() < len {
            return Vec::new();
        }
        if self.superseded == Some((width, height)) {
            return Vec::new();
        }
        self.superseded = None;
        self.pixels.clear();
        self.pixels.extend_from_slice(&buffer[..len]);
        self.width = width;
        self.height = height;
        self.compose()
    }

    pub fn paint_popup(&mut self, buffer: &[u8], width: u32, height: u32) -> Vec<Out> {
        let Some(len) = frame_bytes(width, height) else {
            return Vec::new();
        };
        if buffer.len() < len {
            return Vec::new();
        }
        self.popup.pixels.clear();
        self.popup.pixels.extend_from_slice(&buffer[..len]);
        self.popup.width = width;
        self.popup.height = height;
        self.compose()
    }

    pub fn popup_shown(&mut self, shown: bool) -> Vec<Out> {
        self.popup.shown = shown;
        if !shown {
            self.popup.pixels.clear();
            self.popup.width = 0;
            self.popup.height = 0;
        }
        self.compose()
    }

    pub fn popup_moved(&mut self, rect: Rect) {
        self.popup.requested = rect;
    }

    fn popup_rect_in_pixels(&self) -> Option<Rect> {
        if !self.popup.shown || self.popup.width == 0 || self.popup.height == 0 {
            return None;
        }
        let scale = self.view.scale;
        let scaled = |value: i32| (value as f32 * scale).round() as i32;
        let requested = Rect {
            x: scaled(self.popup.requested.x),
            y: scaled(self.popup.requested.y),
            width: self.popup.width as i32,
            height: self.popup.height as i32,
        };
        Some(requested.clamped_into(self.width as i32, self.height as i32))
    }

    pub fn popup_offset(&self) -> Option<(Rect, i32, i32)> {
        if !self.popup.shown {
            return None;
        }
        let requested = self.popup.requested;
        let placed = requested.clamped_into(self.view.width as i32, self.view.height as i32);
        Some((placed, requested.x - placed.x, requested.y - placed.y))
    }

    pub fn release(&mut self, generation: u64, slot: u32) -> Vec<Out> {
        let Some(slots) = self.slots.as_mut() else {
            return Vec::new();
        };
        if slots.generation != generation {
            return Vec::new();
        }
        slots.release(slot);
        if self.stale {
            self.compose()
        } else {
            Vec::new()
        }
    }

    fn compose(&mut self) -> Vec<Out> {
        if !self.displayed
            || self.width == 0
            || self.height == 0
            || self.failed == Some((self.width, self.height))
        {
            return Vec::new();
        }
        let mut out = Vec::new();
        let current = self
            .slots
            .as_ref()
            .is_some_and(|slots| slots.width == self.width && slots.height == self.height);
        if !current {
            self.slots = None;
            let generation = self.next_generation;
            self.next_generation += 1;
            let created = Slots::create(generation, self.width, self.height).and_then(|slots| {
                let fds = slots
                    .fds()
                    .map_err(|error| format!("cannot share the frame slots: {error}"))?;
                Ok((slots, fds))
            });
            match created {
                Ok((slots, fds)) => {
                    out.push(Out::Surface(
                        Event::Surface {
                            tab: self.tab,
                            generation,
                            width: slots.width,
                            height: slots.height,
                            stride: slots.stride,
                            slots: slots.slot_count(),
                        },
                        fds,
                    ));
                    self.slots = Some(slots);
                }
                Err(reason) => {
                    self.failed = Some((self.width, self.height));
                    out.push(Out::Event(Event::PresentationFailed {
                        tab: self.tab,
                        reason,
                    }));
                    return out;
                }
            }
        }
        let popup_rect = self.popup_rect_in_pixels();
        let Some(slots) = self.slots.as_mut() else {
            return out;
        };
        let Some(slot) = slots.free_slot() else {
            self.stale = true;
            return out;
        };
        let popup = popup_rect.map(|at| Layer {
            pixels: &self.popup.pixels,
            width: self.popup.width,
            height: self.popup.height,
            at,
        });
        if !slots.write(slot, &self.pixels, popup) {
            return out;
        }
        self.stale = false;
        self.frames_sent += 1;
        out.push(Out::Event(Event::Frame(FrameDescriptor::SharedMemory {
            tab: self.tab,
            slot,
            generation: slots.generation,
            width: slots.width,
            height: slots.height,
            stride: slots.width * BYTES_PER_PIXEL,
            format: PixelFormat::Bgra8,
        })));
        out
    }
}

fn differs(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0.abs_diff(b.0) > 1 || a.1.abs_diff(b.1) > 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(out: &[Out]) -> Vec<(u64, u32)> {
        out.iter()
            .filter_map(|out| match out {
                Out::Event(Event::Frame(FrameDescriptor::SharedMemory {
                    generation,
                    slot,
                    ..
                })) => Some((*generation, *slot)),
                _ => None,
            })
            .collect()
    }

    fn surfaces(out: &[Out]) -> Vec<(u64, u32, u32)> {
        out.iter()
            .filter_map(|out| match out {
                Out::Surface(
                    Event::Surface {
                        generation,
                        width,
                        height,
                        ..
                    },
                    fds,
                ) => {
                    assert_eq!(fds.len(), 3);
                    Some((*generation, *width, *height))
                }
                _ => None,
            })
            .collect()
    }

    fn paint(presentation: &mut Presentation, width: u32, height: u32) -> Vec<Out> {
        presentation.paint_view(&vec![1u8; (width * height * 4) as usize], width, height)
    }

    #[test]
    fn a_tab_that_is_not_displayed_sends_no_frame() {
        let mut presentation = Presentation::new(TabId(1));
        assert!(paint(&mut presentation, 4, 4).is_empty());
        assert_eq!(presentation.frames_sent(), 0);
        presentation.present(Some(View::new(4, 4, 1.0)));
        assert_eq!(frames(&paint(&mut presentation, 4, 4)), [(1, 0)]);
        presentation.present(None);
        assert!(paint(&mut presentation, 4, 4).is_empty());
        assert_eq!(presentation.frames_sent(), 1);
    }

    #[test]
    fn a_resize_opens_a_new_generation_at_the_new_size() {
        let mut presentation = Presentation::new(TabId(1));
        presentation.present(Some(View::new(4, 4, 1.0)));
        let first = paint(&mut presentation, 4, 4);
        assert_eq!(surfaces(&first), [(1, 4, 4)]);
        let (resized, _) = presentation.present(Some(View::new(8, 2, 1.0)));
        assert!(resized);
        let second = paint(&mut presentation, 8, 2);
        assert_eq!(surfaces(&second), [(2, 8, 2)]);
        assert_eq!(frames(&second), [(2, 0)]);
    }

    #[test]
    fn a_paint_at_the_size_before_a_resize_is_dropped_until_the_new_size_arrives() {
        let mut presentation = Presentation::new(TabId(1));
        presentation.present(Some(View::new(320, 240, 1.0)));
        assert!(paint(&mut presentation, 1280, 800).is_empty());
        assert_eq!(presentation.frames_sent(), 0);
        assert_eq!(
            surfaces(&paint(&mut presentation, 320, 240)),
            [(1, 320, 240)]
        );
        presentation.present(Some(View::new(160, 120, 2.0)));
        assert_eq!(frames(&paint(&mut presentation, 320, 240)), [(1, 1)]);
        presentation.present(Some(View::new(100, 50, 1.0)));
        assert!(paint(&mut presentation, 320, 240).is_empty());
        assert_eq!(surfaces(&paint(&mut presentation, 100, 50)), [(2, 100, 50)]);
        assert_eq!(frames(&paint(&mut presentation, 320, 240)), [(3, 0)]);
    }

    #[test]
    fn with_every_slot_held_only_the_latest_frame_is_sent_on_release() {
        let mut presentation = Presentation::new(TabId(1));
        presentation.present(Some(View::new(2, 2, 1.0)));
        for _ in 0..3 {
            assert_eq!(frames(&paint(&mut presentation, 2, 2)).len(), 1);
        }
        for _ in 0..5 {
            assert!(frames(&paint(&mut presentation, 2, 2)).is_empty());
        }
        assert_eq!(frames(&presentation.release(1, 1)), [(1, 1)]);
        assert!(presentation.release(1, 0).is_empty());
        assert!(presentation.release(99, 2).is_empty());
        assert_eq!(presentation.frames_sent(), 4);
    }

    #[test]
    fn a_failed_segment_is_reported_once_per_size_and_retried_at_another_size_or_present() {
        let mut presentation = Presentation::new(TabId(1));
        presentation.present(Some(View::new(8192, 8192, 4.0)));
        let failed = presentation.paint_view(&[], 0, 0);
        assert!(failed.is_empty());
        presentation.width = 32_768;
        presentation.height = 32_768;
        let out = presentation.compose();
        assert!(matches!(
            out.as_slice(),
            [Out::Event(Event::PresentationFailed { .. })]
        ));
        assert!(presentation.compose().is_empty());
        assert_eq!(frames(&paint(&mut presentation, 4, 4)), [(2, 0)]);
        presentation.width = 32_768;
        presentation.height = 32_768;
        let (_, retried) = presentation.present(Some(View::new(8192, 8192, 4.0)));
        assert!(matches!(
            retried.as_slice(),
            [Out::Event(Event::PresentationFailed { .. })]
        ));
    }

    #[test]
    fn a_popup_near_the_bottom_is_placed_inside_and_its_offset_is_kept() {
        let mut presentation = Presentation::new(TabId(1));
        presentation.present(Some(View::new(100, 100, 1.0)));
        presentation.popup_moved(Rect {
            x: 10,
            y: 90,
            width: 50,
            height: 40,
        });
        presentation.popup_shown(true);
        let (placed, dx, dy) = presentation.popup_offset().unwrap();
        assert_eq!((placed.y, dx, dy), (60, 0, 30));
        presentation.popup_shown(false);
        assert!(presentation.popup_offset().is_none());
    }
}
