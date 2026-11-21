use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, DispatchPhase, HitboxBehavior, IntoElement, Pixels, ScrollEvent, Styled, Window, canvas,
    point, px,
};

use super::scrollbar::ScrollableHandle;

const GLIDE_SECONDS: f32 = 0.06;
const SETTLED_PX: f32 = 0.5;
const WHEEL_LINE: Pixels = px(20.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Input {
    Glide,
    Track,
    Jump,
}

impl Input {
    fn of(event: &ScrollEvent, reduce_motion: bool) -> Self {
        if reduce_motion {
            Self::Jump
        } else if event.delta.precise() {
            Self::Track
        } else {
            Self::Glide
        }
    }
}

#[derive(Debug, Default)]
struct Glide {
    position: f32,
    target: f32,
    clock: Option<Instant>,
}

impl Glide {
    fn is_moving(&self) -> bool {
        self.clock.is_some()
    }

    fn take_hold(&mut self, current: f32) {
        if !self.is_moving() {
            self.position = current;
            self.target = current;
        }
    }

    fn nudge(&mut self, current: f32, delta: f32, max: f32, now: Instant, input: Input) -> f32 {
        self.take_hold(current);
        self.target = (self.target + delta).clamp(0., max);
        match input {
            Input::Glide => {
                self.clock.get_or_insert(now);
            }
            Input::Track => self.position = (self.position + delta).clamp(0., max),
            Input::Jump => {
                self.position = self.target;
                self.clock = None;
            }
        }
        self.position
    }

    fn head_for(&mut self, current: f32, destination: f32, max: f32, now: Instant, smooth: bool) {
        self.take_hold(current);
        self.target = destination.clamp(0., max);
        if smooth {
            self.clock.get_or_insert(now);
        } else {
            self.position = self.target;
            self.clock = None;
        }
    }

    fn advance(&mut self, now: Instant, max: f32) -> Option<f32> {
        let last = self.clock?;
        let elapsed = now.saturating_duration_since(last).as_secs_f32();
        self.clock = Some(now);
        self.target = self.target.clamp(0., max);
        self.position = self.position.clamp(0., max);
        let pull = 1. - (-elapsed / GLIDE_SECONDS).exp();
        self.position += (self.target - self.position) * pull;
        if (self.target - self.position).abs() <= SETTLED_PX {
            self.position = self.target;
            self.clock = None;
        }
        Some(self.position)
    }
}

#[derive(Debug, Default)]
struct State {
    glide: Glide,
    stale: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SmoothScroll(Rc<RefCell<State>>);

fn scrolled<H: ScrollableHandle>(handle: &H) -> f32 {
    -f32::from(handle.offset().y)
}

fn furthest<H: ScrollableHandle>(handle: &H) -> f32 {
    f32::from(handle.max_offset().y).max(0.)
}

fn place<H: ScrollableHandle>(handle: &H, position: f32) {
    handle.set_offset(point(px(0.), px(-position)));
}

impl SmoothScroll {
    pub(crate) fn content_replaced(&self) {
        let mut state = self.0.borrow_mut();
        state.glide.clock = None;
        state.stale = true;
    }

    pub(crate) fn settle<H: ScrollableHandle>(&self, handle: &H, window: &Window) {
        self.settle_at(handle, Instant::now(), window);
    }

    fn settle_at<H: ScrollableHandle>(&self, handle: &H, now: Instant, window: &Window) {
        let (position, moving) = {
            let mut state = self.0.borrow_mut();
            let position = state.glide.advance(now, furthest(handle));
            (position, state.glide.is_moving())
        };
        if let Some(position) = position {
            place(handle, position);
        }
        if moving {
            window.request_animation_frame();
        }
    }

    pub(crate) fn ease<H: ScrollableHandle>(&self, handle: &H, jump: impl FnOnce()) {
        let (stale, flight) = {
            let state = self.0.borrow();
            let flight = state
                .glide
                .is_moving()
                .then_some((state.glide.position, state.glide.target));
            (state.stale, flight)
        };
        if stale {
            jump();
            return;
        }
        let current = flight.map_or_else(|| scrolled(handle), |(position, _)| position);
        if let Some((_, aim)) = flight {
            place(handle, aim);
        }
        let aim = scrolled(handle);
        jump();
        let to = scrolled(handle);
        if to == aim {
            if flight.is_some() {
                place(handle, current);
            }
            return;
        }
        let smooth = !crate::ui_primitives::reduce_motion();
        if smooth {
            place(handle, current);
        }
        self.0
            .borrow_mut()
            .glide
            .head_for(current, to, furthest(handle), Instant::now(), smooth);
    }

    pub(crate) fn wheel_layer<H: ScrollableHandle + 'static>(&self, handle: H) -> impl IntoElement {
        let this = self.clone();
        canvas(
            |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
            move |_, hitbox, window, _| {
                this.0.borrow_mut().stale = false;
                let view = window.current_view();
                window.on_mouse_event(move |event: &ScrollEvent, phase, window, cx: &mut App| {
                    if phase != DispatchPhase::Capture || !hitbox.should_handle_scroll(window) {
                        return;
                    }
                    let delta = event.delta.pixel_delta(WHEEL_LINE).y;
                    if delta == px(0.) {
                        return;
                    }
                    cx.stop_propagation();
                    let input = Input::of(event, crate::ui_primitives::reduce_motion());
                    let position = {
                        let mut state = this.0.borrow_mut();
                        if state.stale {
                            return;
                        }
                        state.glide.nudge(
                            scrolled(&handle),
                            -f32::from(delta),
                            furthest(&handle),
                            Instant::now(),
                            input,
                        )
                    };
                    if input != Input::Glide {
                        place(&handle, position);
                    }
                    cx.notify(view);
                });
            },
        )
        .absolute()
        .top_0()
        .right_0()
        .bottom_0()
        .left_0()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;
    use std::time::Duration;

    const NOTCH: f32 = 60.;
    const MAX: f32 = 1_000.;

    fn after(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_wheel_notch_glides_to_its_target_and_settles() {
        let start = Instant::now();
        let mut glide = Glide::default();
        assert_eq!(glide.nudge(0., NOTCH, MAX, start, Input::Glide), 0.);
        let first = glide.advance(after(start, 16), MAX).expect("gliding");
        assert!(first > 0. && first < NOTCH / 2., "{first}");
        let second = glide.advance(after(start, 32), MAX).expect("gliding");
        assert!(second > first && second < NOTCH);
        assert_eq!(glide.advance(after(start, 2_000), MAX), Some(NOTCH));
        assert!(!glide.is_moving());
        assert_eq!(glide.advance(after(start, 2_016), MAX), None);
    }

    #[test]
    fn notches_pile_up_on_the_target_while_the_view_catches_up() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.nudge(0., NOTCH, MAX, start, Input::Glide);
        glide.advance(after(start, 16), MAX);
        let lagging = glide.position;
        glide.nudge(123., NOTCH, MAX, after(start, 20), Input::Glide);
        assert_eq!(glide.target, NOTCH * 2.);
        assert_eq!(
            glide.position, lagging,
            "a notch mid-glide continues from the glide, not from the list"
        );
    }

    #[test]
    fn the_glide_is_the_same_at_any_frame_rate() {
        let start = Instant::now();
        let mut coarse = Glide::default();
        let mut fine = Glide::default();
        coarse.nudge(0., NOTCH, MAX, start, Input::Glide);
        fine.nudge(0., NOTCH, MAX, start, Input::Glide);
        coarse.advance(after(start, 32), MAX);
        fine.advance(after(start, 8), MAX);
        fine.advance(after(start, 16), MAX);
        fine.advance(after(start, 24), MAX);
        fine.advance(after(start, 32), MAX);
        assert!((coarse.position - fine.position).abs() < 0.01);
    }

    #[test]
    fn the_target_stays_within_the_content() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.nudge(0., -NOTCH, MAX, start, Input::Glide);
        assert_eq!(glide.target, 0.);
        glide.nudge(0., MAX * 3., MAX, start, Input::Glide);
        assert_eq!(glide.target, MAX);
        glide.advance(after(start, 16), MAX / 2.);
        assert_eq!(
            glide.target,
            MAX / 2.,
            "content that shrinks pulls the glide in"
        );
    }

    #[test]
    fn a_touchpad_moves_the_view_at_once_and_keeps_a_glide_in_flight() {
        let start = Instant::now();
        let mut glide = Glide::default();
        assert_eq!(glide.nudge(40., 7., MAX, start, Input::Track), 47.);
        assert!(!glide.is_moving());
        glide.nudge(47., NOTCH, MAX, start, Input::Glide);
        glide.advance(after(start, 16), MAX);
        let remaining = glide.target - glide.position;
        glide.nudge(0., 5., MAX, after(start, 20), Input::Track);
        assert!((glide.target - glide.position - remaining).abs() < 1e-3);
    }

    #[test]
    fn reduced_motion_lands_at_once() {
        let start = Instant::now();
        let mut glide = Glide::default();
        assert_eq!(
            glide.nudge(10., NOTCH, MAX, start, Input::Jump),
            10. + NOTCH
        );
        assert!(!glide.is_moving());
        glide.head_for(70., 300., MAX, start, false);
        assert_eq!((glide.position, glide.is_moving()), (300., false));
    }

    #[test]
    fn a_reveal_glides_from_where_the_view_stands() {
        let start = Instant::now();
        let mut glide = Glide::default();
        glide.head_for(100., 400., MAX, start, true);
        assert_eq!(glide.position, 100.);
        assert_eq!(glide.advance(after(start, 2_000), MAX), Some(400.));
    }

    struct Wheeled {
        list: gpui::ListState,
        scroll: SmoothScroll,
    }

    impl gpui::Render for Wheeled {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            use gpui::ParentElement as _;
            gpui::div()
                .relative()
                .size_full()
                .child(
                    gpui::list(self.list.clone(), |_, _, _| {
                        gpui::div().h(px(100.)).w_full().into_any_element()
                    })
                    .size_full(),
                )
                .child(self.scroll.wheel_layer(self.list.clone()))
        }
    }

    fn wheeled(
        cx: &mut gpui::TestAppContext,
    ) -> (gpui::ListState, SmoothScroll, &mut gpui::VisualTestContext) {
        let cx = cx.add_empty_window();
        let list = gpui::ListState::new(10, gpui::ListAlignment::Top, px(0.)).measure_all();
        let scroll = SmoothScroll::default();
        let (state, glide) = (list.clone(), scroll.clone());
        cx.draw(
            point(px(0.), px(0.)),
            gpui::size(px(200.), px(200.)),
            |_, cx| {
                cx.new(|_| Wheeled {
                    list: state,
                    scroll: glide,
                })
                .into_any_element()
            },
        );
        (list, scroll, cx)
    }

    fn wheel(cx: &mut gpui::VisualTestContext, delta: gpui::ScrollDelta) {
        cx.simulate_event(ScrollEvent {
            position: point(px(50.), px(50.)),
            delta,
            ..Default::default()
        });
    }

    #[gpui::test]
    fn the_layer_takes_the_wheel_from_the_list(cx: &mut gpui::TestAppContext) {
        let (list, scroll, cx) = wheeled(cx);
        wheel(cx, gpui::ScrollDelta::Lines(point(0., -3.)));
        assert_eq!(
            scrolled(&list),
            0.,
            "a notch must not jump: the list's own handler never sees it"
        );
        cx.update(|window, _| {
            scroll.settle_at(&list, Instant::now() + Duration::from_secs(5), window)
        });
        assert_eq!(scrolled(&list), 3. * f32::from(WHEEL_LINE));
    }

    #[gpui::test]
    fn a_touchpad_scrolls_once_and_at_once(cx: &mut gpui::TestAppContext) {
        let (list, _scroll, cx) = wheeled(cx);
        wheel(cx, gpui::ScrollDelta::Pixels(point(px(0.), px(-30.))));
        assert_eq!(scrolled(&list), 30.);
        wheel(cx, gpui::ScrollDelta::Pixels(point(px(0.), px(-5_000.))));
        assert_eq!(scrolled(&list), furthest(&list));
    }
}
