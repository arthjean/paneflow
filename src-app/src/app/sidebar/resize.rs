use std::rc::Rc;
use std::time::Instant;

use gpui::{
    App, CursorStyle, DispatchPhase, MouseMoveEvent, MouseUpEvent, Orientation, Pixels, canvas,
};

use super::*;
use crate::SidebarWidthAnimation;
use crate::app::constants::SIDEBAR_MAX_WIDTH;
use crate::ui_primitives::resize_edge::{
    ResizeEdge, ResizeEdgeBounce, ResizeSettle, resize_drag_sample,
};

pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 240.;
const SIDEBAR_MAIN_PANEL_FLOOR: f32 = 200.;
const SIDEBAR_DETENT_REACH: f32 = 12.;
const SIDEBAR_RESIZE_HIT_WIDTH: f32 = 7.;
const SIDEBAR_RESIZE_HIT_INSIDE: f32 = 2.;
const SIDEBAR_RESIZE_RULE_ALPHA: f32 = 0.16;
const SIDEBAR_RESIZE_GROUP: &str = "sidebar-resize-group";
const _: () = assert!(SIDEBAR_WIDTH + SIDEBAR_DETENT_REACH < SIDEBAR_MAX_WIDTH);
const _: () = assert!(SIDEBAR_WIDTH - SIDEBAR_DETENT_REACH > SIDEBAR_MIN_WIDTH);

type ResizeSidebar = Rc<dyn Fn(SidebarResizeInput, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SidebarResizeInput {
    Press(f32),
    Drag(f32),
    Release,
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SidebarResizeDrag {
    press_x: f32,
    press_width: f32,
    edge: Option<ResizeEdge>,
    on_detent: bool,
}

pub(crate) struct SidebarResize {
    chosen: f32,
    ceiling: f32,
    shown: f32,
    drag: Option<SidebarResizeDrag>,
    bounce: Option<ResizeEdgeBounce>,
    settle: Option<ResizeSettle>,
    glide: Option<SidebarWidthAnimation>,
}

impl SidebarResize {
    pub(crate) fn restored(persisted: Option<f32>) -> Self {
        let chosen = persisted
            .filter(|width| width.is_finite())
            .map_or(SIDEBAR_WIDTH, |width| {
                width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
            });
        Self {
            chosen,
            ceiling: SIDEBAR_MAX_WIDTH,
            shown: chosen,
            drag: None,
            bounce: None,
            settle: None,
            glide: None,
        }
    }

    pub(crate) fn persisted(&self) -> Option<f32> {
        (self.chosen != SIDEBAR_WIDTH).then_some(self.chosen)
    }

    pub(crate) fn ceiling_for(viewport_width: f32, right_rail_width: f32) -> f32 {
        (viewport_width - right_rail_width - SIDEBAR_MAIN_PANEL_FLOOR)
            .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
    }

    pub(crate) fn width(&self) -> f32 {
        self.shown
    }

    pub(crate) fn ceiling(&self) -> f32 {
        self.ceiling
    }

    pub(crate) fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    fn held_at_an_edge(&self) -> bool {
        self.drag.is_some_and(|drag| drag.edge.is_some())
    }

    fn held_on_the_detent(&self) -> bool {
        self.drag.is_some_and(|drag| drag.on_detent)
    }

    pub(crate) fn begin_frame(&mut self, now: Instant, ceiling: f32, animate: bool) -> bool {
        self.ceiling = ceiling;
        if !animate {
            self.glide = None;
            self.bounce = None;
            self.settle = None;
        }
        if self.glide.is_some_and(|glide| glide.is_finished(now)) {
            self.glide = None;
        }
        let nudge = self.bounce.and_then(|bounce| bounce.offset_at(now));
        if nudge.is_none() {
            self.bounce = None;
        }
        let settling = self.settle.and_then(|settle| settle.offset_at(now));
        if settling.is_none() {
            self.settle = None;
        }
        let resting = self
            .glide
            .map_or(self.chosen, |glide| glide.width_at(now))
            .min(ceiling);
        self.shown = resting + nudge.unwrap_or(0.) + settling.unwrap_or(0.);
        self.glide.is_some() || self.bounce.is_some() || self.settle.is_some()
    }

    fn resize(&mut self, input: SidebarResizeInput, now: Instant, animate: bool) -> bool {
        match input {
            SidebarResizeInput::Press(x) => {
                self.glide = None;
                let press_width = self.chosen.min(self.ceiling);
                self.drag = Some(SidebarResizeDrag {
                    press_x: x,
                    press_width,
                    edge: None,
                    on_detent: press_width == SIDEBAR_WIDTH,
                });
                false
            }
            SidebarResizeInput::Drag(x) => {
                let Some(drag) = self.drag.as_mut() else {
                    return false;
                };
                let sample = resize_drag_sample(
                    drag.press_width + x - drag.press_x,
                    SIDEBAR_MIN_WIDTH,
                    self.ceiling,
                    drag.edge,
                    animate,
                );
                let on_detent = sample.edge.is_none()
                    && SIDEBAR_WIDTH <= self.ceiling
                    && (sample.size - SIDEBAR_WIDTH).abs() <= SIDEBAR_DETENT_REACH;
                let size = if on_detent {
                    SIDEBAR_WIDTH
                } else {
                    sample.size
                };
                if animate && on_detent != drag.on_detent {
                    let carried = self
                        .settle
                        .and_then(|settle| settle.offset_at(now))
                        .unwrap_or(0.);
                    let jump = if on_detent {
                        sample.size - SIDEBAR_WIDTH
                    } else {
                        (SIDEBAR_WIDTH - size).clamp(-SIDEBAR_DETENT_REACH, SIDEBAR_DETENT_REACH)
                    };
                    self.settle = Some(ResizeSettle::new(carried + jump, now));
                }
                drag.edge = sample.edge;
                drag.on_detent = on_detent;
                self.chosen = size;
                if sample.starts_bounce {
                    self.bounce = sample.edge.map(|edge| ResizeEdgeBounce::new(edge, now));
                } else if sample.edge.is_none() {
                    self.bounce = None;
                }
                false
            }
            SidebarResizeInput::Release => self.drag.take().is_some(),
            SidebarResizeInput::Reset => {
                let from_width = self.shown;
                self.drag = None;
                self.bounce = None;
                self.settle = None;
                self.chosen = SIDEBAR_WIDTH;
                let to_width = SIDEBAR_WIDTH.min(self.ceiling);
                self.glide = (animate
                    && (from_width - to_width).abs() > crate::PRIMARY_SIDEBAR_MIN_ANIMATION_DELTA)
                    .then_some(SidebarWidthAnimation {
                        from_width,
                        to_width,
                        started_at: now,
                    });
                true
            }
        }
    }
}

fn fading_rule(color: gpui::Hsla) -> (gpui::Background, gpui::Background) {
    let clear = with_alpha(color, 0.);
    (
        gpui::linear_gradient(
            180.,
            gpui::linear_color_stop(clear, 0.),
            gpui::linear_color_stop(color, 1.),
        ),
        gpui::linear_gradient(
            180.,
            gpui::linear_color_stop(color, 0.),
            gpui::linear_color_stop(clear, 1.),
        ),
    )
}

impl PaneFlowApp {
    pub(crate) fn sidebar_width(&self) -> f32 {
        self.sidebar_resize.width()
    }

    fn resize_primary_sidebar(&mut self, input: SidebarResizeInput, cx: &mut Context<Self>) {
        let persist = self.sidebar_resize.resize(
            input,
            Instant::now(),
            !crate::ui_primitives::reduce_motion(),
        );
        if persist {
            crate::window_state::record_sidebar_width(self.sidebar_resize.persisted());
            cx.background_spawn(async { crate::window_state::save() })
                .detach();
        }
        cx.notify();
    }

    fn sidebar_resizer(cx: &mut Context<Self>) -> ResizeSidebar {
        let app = cx.weak_entity();
        Rc::new(move |input, _window, cx| {
            let _ = app.update(cx, |app, cx| app.resize_primary_sidebar(input, cx));
        })
    }

    pub(crate) fn render_primary_sidebar_resize_handle(
        &self,
        seam: f32,
        top: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let on_resize = Self::sidebar_resizer(cx);
        let dragging = self.sidebar_resize.dragging();
        let (upper, lower) = if self.sidebar_resize.held_on_the_detent() {
            let caught = gpui::Background::from(ui.accent);
            (caught, caught)
        } else {
            fading_rule(with_alpha(ui.text, SIDEBAR_RESIZE_RULE_ALPHA))
        };
        let lit = dragging && !self.sidebar_resize.held_at_an_edge();
        let half = |fill: gpui::Background| {
            let half = div().flex_1().when(lit, |half| half.bg(fill));
            if dragging {
                half
            } else {
                half.group_hover(SIDEBAR_RESIZE_GROUP, move |style| style.bg(fill))
            }
        };
        let press = on_resize.clone();
        div()
            .id("sidebar-resize")
            .group(SIDEBAR_RESIZE_GROUP)
            .absolute()
            .left(px(seam - SIDEBAR_RESIZE_HIT_INSIDE))
            .top(top)
            .bottom_0()
            .w(px(SIDEBAR_RESIZE_HIT_WIDTH))
            .occlude()
            .cursor(CursorStyle::ResizeLeftRight)
            .role(Role::Splitter)
            .aria_label("Resize the Workspaces sidebar")
            .aria_orientation(Orientation::Vertical)
            .aria_numeric_value(f64::from(self.sidebar_resize.chosen))
            .aria_min_numeric_value(f64::from(SIDEBAR_MIN_WIDTH))
            .aria_max_numeric_value(f64::from(self.sidebar_resize.ceiling()))
            .debug_selector(|| "sidebar-resize".into())
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
                let input = if event.click_count >= 2 {
                    SidebarResizeInput::Reset
                } else {
                    SidebarResizeInput::Press(f32::from(event.position.x))
                };
                press(input, window, cx);
            })
            .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                on_resize(SidebarResizeInput::Release, window, cx);
            })
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(SIDEBAR_RESIZE_HIT_INSIDE - 1.))
                    .w(px(1.))
                    .flex()
                    .flex_col()
                    .debug_selector(|| "sidebar-resize-rule".into())
                    .child(half(upper))
                    .child(half(lower)),
            )
            .into_any_element()
    }

    pub(crate) fn render_primary_sidebar_resize_veil(&self, cx: &mut Context<Self>) -> AnyElement {
        let on_resize = Self::sidebar_resizer(cx);
        div()
            .absolute()
            .inset_0()
            .occlude()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moved = on_resize.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let input = if event.pressed_button == Some(MouseButton::Left) {
                                SidebarResizeInput::Drag(f32::from(event.position.x))
                            } else {
                                SidebarResizeInput::Release
                            };
                            moved(input, window, cx);
                        });
                        let released = on_resize.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                released(SidebarResizeInput::Release, window, cx);
                            }
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::ui_primitives::resize_edge::{RESIZE_EDGE_BOUNCE, RESIZE_EDGE_NUDGE, RESIZE_SETTLE};

    const OFF_DEFAULT: f32 = 360.;

    fn at_rest(width: Option<f32>, now: Instant) -> SidebarResize {
        let mut sidebar = SidebarResize::restored(width);
        sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, true);
        sidebar
    }

    fn pressed(width: Option<f32>, now: Instant) -> SidebarResize {
        let mut sidebar = at_rest(width, now);
        let grabbed = sidebar.width();
        sidebar.resize(SidebarResizeInput::Press(grabbed), now, true);
        sidebar
    }

    #[test]
    fn the_default_width_is_ninety_percent_of_the_widest() {
        assert_eq!(SIDEBAR_WIDTH, 432.);
        assert_eq!(SIDEBAR_WIDTH / SIDEBAR_MAX_WIDTH, 0.9);
        assert!((SIDEBAR_MIN_WIDTH..SIDEBAR_MAX_WIDTH).contains(&SIDEBAR_WIDTH));
    }

    #[test]
    fn a_missing_or_unusable_width_restores_the_default() {
        assert_eq!(SidebarResize::restored(None).width(), SIDEBAR_WIDTH);
        assert_eq!(
            SidebarResize::restored(Some(f32::NAN)).width(),
            SIDEBAR_WIDTH
        );
        assert_eq!(
            SidebarResize::restored(Some(12.)).width(),
            SIDEBAR_MIN_WIDTH
        );
        assert_eq!(
            SidebarResize::restored(Some(9000.)).width(),
            SIDEBAR_MAX_WIDTH
        );
    }

    #[test]
    fn only_a_chosen_width_is_persisted() {
        let now = Instant::now();
        let mut sidebar = pressed(None, now);
        assert_eq!(sidebar.persisted(), None);

        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH - 60.), now, true);
        assert!(sidebar.resize(SidebarResizeInput::Release, now, true));
        assert_eq!(sidebar.persisted(), Some(SIDEBAR_WIDTH - 60.));
        assert!(!sidebar.resize(SidebarResizeInput::Release, now, true));
    }

    #[test]
    fn the_width_follows_the_pointer_from_where_it_was_pressed() {
        let now = Instant::now();
        let mut sidebar = at_rest(Some(OFF_DEFAULT), now);
        sidebar.resize(SidebarResizeInput::Press(OFF_DEFAULT + 3.), now, true);
        sidebar.resize(SidebarResizeInput::Drag(OFF_DEFAULT + 43.), now, true);
        sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, true);
        assert_eq!(sidebar.width(), OFF_DEFAULT + 40.);

        sidebar.resize(SidebarResizeInput::Release, now, true);
        sidebar.resize(SidebarResizeInput::Drag(250.), now, true);
        sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, true);
        assert_eq!(sidebar.width(), OFF_DEFAULT + 40.);
    }

    #[test]
    fn a_pointer_held_past_the_floor_nudges_once_and_settles_on_the_floor() {
        let started = Instant::now();
        let mut sidebar = pressed(Some(SIDEBAR_MIN_WIDTH + 20.), started);
        sidebar.resize(SidebarResizeInput::Drag(100.), started, true);
        assert!(sidebar.held_at_an_edge());

        let peak = started + RESIZE_EDGE_BOUNCE.mul_f32(0.32);
        assert!(sidebar.begin_frame(peak, SIDEBAR_MAX_WIDTH, true));
        assert!((sidebar.width() - (SIDEBAR_MIN_WIDTH - RESIZE_EDGE_NUDGE)).abs() < 0.01);

        sidebar.resize(SidebarResizeInput::Drag(60.), peak, true);
        assert!(sidebar.begin_frame(peak, SIDEBAR_MAX_WIDTH, true));
        assert!((sidebar.width() - (SIDEBAR_MIN_WIDTH - RESIZE_EDGE_NUDGE)).abs() < 0.01);

        let settled = started + RESIZE_EDGE_BOUNCE;
        assert!(!sidebar.begin_frame(settled, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_MIN_WIDTH);
        assert_eq!(sidebar.persisted(), Some(SIDEBAR_MIN_WIDTH));
    }

    #[test]
    fn a_pointer_held_past_the_ceiling_nudges_outward() {
        let started = Instant::now();
        let mut sidebar = pressed(Some(SIDEBAR_MAX_WIDTH - 20.), started);
        sidebar.resize(SidebarResizeInput::Drag(900.), started, true);

        let peak = started + RESIZE_EDGE_BOUNCE.mul_f32(0.32);
        assert!(sidebar.begin_frame(peak, SIDEBAR_MAX_WIDTH, true));
        assert!((sidebar.width() - (SIDEBAR_MAX_WIDTH + RESIZE_EDGE_NUDGE)).abs() < 0.01);
    }

    #[test]
    fn leaving_the_edge_drops_the_nudge_and_returning_starts_a_new_one() {
        let started = Instant::now();
        let mut sidebar = pressed(Some(SIDEBAR_MIN_WIDTH + 20.), started);
        sidebar.resize(SidebarResizeInput::Drag(100.), started, true);

        let midway = started + RESIZE_EDGE_BOUNCE.mul_f32(0.2);
        sidebar.resize(SidebarResizeInput::Drag(270.), midway, true);
        assert!(!sidebar.begin_frame(midway, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), 270.);

        sidebar.resize(SidebarResizeInput::Drag(100.), midway, true);
        assert!(sidebar.begin_frame(midway, SIDEBAR_MAX_WIDTH, true));
    }

    #[test]
    fn the_default_width_holds_the_edge_while_the_pointer_crosses_its_detent() {
        let now = Instant::now();
        let mut sidebar = pressed(Some(OFF_DEFAULT), now);
        let pointer_for = |width: f32| SidebarResizeInput::Drag(width);

        sidebar.resize(
            pointer_for(SIDEBAR_WIDTH + SIDEBAR_DETENT_REACH + 1.),
            now,
            true,
        );
        assert!(!sidebar.held_on_the_detent());
        assert_eq!(sidebar.chosen, SIDEBAR_WIDTH + SIDEBAR_DETENT_REACH + 1.);

        for inside in [
            SIDEBAR_WIDTH + SIDEBAR_DETENT_REACH,
            SIDEBAR_WIDTH + 2.,
            SIDEBAR_WIDTH,
            SIDEBAR_WIDTH - 3.,
            SIDEBAR_WIDTH - SIDEBAR_DETENT_REACH,
        ] {
            sidebar.resize(pointer_for(inside), now, true);
            assert!(sidebar.held_on_the_detent(), "{inside}");
            assert_eq!(sidebar.chosen, SIDEBAR_WIDTH, "{inside}");
        }

        sidebar.resize(
            pointer_for(SIDEBAR_WIDTH - SIDEBAR_DETENT_REACH - 1.),
            now,
            true,
        );
        assert!(!sidebar.held_on_the_detent());
        assert_eq!(sidebar.chosen, SIDEBAR_WIDTH - SIDEBAR_DETENT_REACH - 1.);
    }

    #[test]
    fn a_release_on_the_detent_forgets_the_chosen_width() {
        let now = Instant::now();
        let mut sidebar = pressed(Some(OFF_DEFAULT), now);
        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH + 4.), now, true);
        assert!(sidebar.resize(SidebarResizeInput::Release, now, true));
        assert_eq!(sidebar.persisted(), None);
    }

    #[test]
    fn the_edge_eases_onto_the_detent_and_off_it_instead_of_jumping() {
        let started = Instant::now();
        let mut sidebar = pressed(Some(OFF_DEFAULT), started);
        sidebar.resize(
            SidebarResizeInput::Drag(SIDEBAR_WIDTH + SIDEBAR_DETENT_REACH + 4.),
            started,
            true,
        );
        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH + 5.), started, true);
        assert!(sidebar.begin_frame(started, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH + 5.);

        let midway = started + RESIZE_SETTLE.mul_f32(0.5);
        assert!(sidebar.begin_frame(midway, SIDEBAR_MAX_WIDTH, true));
        assert!(sidebar.width() > SIDEBAR_WIDTH && sidebar.width() < SIDEBAR_WIDTH + 5.);

        let caught = started + RESIZE_SETTLE;
        assert!(!sidebar.begin_frame(caught, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH);

        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH - 40.), caught, true);
        assert!(sidebar.begin_frame(caught, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(
            sidebar.width(),
            SIDEBAR_WIDTH - 40. + SIDEBAR_DETENT_REACH,
            "a fast pointer is trailed by the detent reach at most"
        );
        assert!(!sidebar.begin_frame(caught + RESIZE_SETTLE, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH - 40.);
    }

    #[test]
    fn a_drag_from_the_default_starts_on_the_detent() {
        let now = Instant::now();
        let mut sidebar = pressed(None, now);
        assert!(sidebar.held_on_the_detent());
        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH + 3.), now, true);
        assert!(!sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH);
    }

    #[test]
    fn a_window_too_narrow_for_the_default_has_no_detent() {
        let now = Instant::now();
        let mut sidebar = at_rest(Some(SIDEBAR_MIN_WIDTH), now);
        let ceiling = SIDEBAR_WIDTH - 2.;
        sidebar.begin_frame(now, ceiling, true);
        sidebar.resize(SidebarResizeInput::Press(SIDEBAR_MIN_WIDTH), now, true);
        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH - 4.), now, true);
        assert!(!sidebar.held_on_the_detent());
        assert_eq!(sidebar.chosen, SIDEBAR_WIDTH - 4.);
    }

    #[test]
    fn reduced_motion_clamps_without_a_nudge_a_settle_or_a_glide() {
        let now = Instant::now();
        let mut sidebar = at_rest(Some(420.), now);
        sidebar.resize(SidebarResizeInput::Press(420.), now, false);
        sidebar.resize(SidebarResizeInput::Drag(900.), now, false);
        assert!(!sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, false));
        assert_eq!(sidebar.width(), SIDEBAR_MAX_WIDTH);

        sidebar.resize(SidebarResizeInput::Drag(SIDEBAR_WIDTH + 2.), now, false);
        assert!(!sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, false));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH);

        sidebar.resize(SidebarResizeInput::Drag(900.), now, false);
        assert!(sidebar.resize(SidebarResizeInput::Reset, now, false));
        assert!(!sidebar.begin_frame(now, SIDEBAR_MAX_WIDTH, false));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH);
    }

    #[test]
    fn a_reset_glides_back_to_the_default_and_forgets_the_chosen_width() {
        let started = Instant::now();
        let mut sidebar = at_rest(Some(420.), started);
        assert!(sidebar.resize(SidebarResizeInput::Reset, started, true));
        assert_eq!(sidebar.persisted(), None);

        assert!(sidebar.begin_frame(started, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), 420.);

        let landed = SidebarWidthAnimation {
            from_width: 420.,
            to_width: SIDEBAR_WIDTH,
            started_at: started,
        }
        .finishes_at();
        assert!(!sidebar.begin_frame(landed, SIDEBAR_MAX_WIDTH, true));
        assert_eq!(sidebar.width(), SIDEBAR_WIDTH);
    }

    #[test]
    fn a_narrow_window_caps_the_shown_width_and_keeps_the_chosen_one() {
        let now = Instant::now();
        let mut sidebar = at_rest(Some(460.), now);
        let ceiling = SidebarResize::ceiling_for(800., 300.);
        assert_eq!(ceiling, 300.);
        sidebar.begin_frame(now, ceiling, true);
        assert_eq!(sidebar.width(), 300.);
        assert_eq!(sidebar.persisted(), Some(460.));

        sidebar.begin_frame(now, SidebarResize::ceiling_for(1600., 0.), true);
        assert_eq!(sidebar.width(), 460.);
    }

    #[test]
    fn the_ceiling_stays_within_the_resize_range() {
        assert_eq!(SidebarResize::ceiling_for(300., 300.), SIDEBAR_MIN_WIDTH);
        assert_eq!(SidebarResize::ceiling_for(4000., 0.), SIDEBAR_MAX_WIDTH);
    }

    #[test]
    fn a_drag_in_a_narrow_window_stops_at_the_window_ceiling() {
        let started = Instant::now();
        let mut sidebar = at_rest(Some(310.), started);
        sidebar.begin_frame(started, 320., true);
        sidebar.resize(SidebarResizeInput::Press(310.), started, true);
        sidebar.resize(SidebarResizeInput::Drag(500.), started, true);
        assert!(sidebar.held_at_an_edge());
        assert!(!sidebar.begin_frame(started + RESIZE_EDGE_BOUNCE, 320., true));
        assert_eq!(sidebar.width(), 320.);
    }
}
