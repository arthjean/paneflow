pub(crate) mod action_menu;
pub(crate) mod animation_clock;
pub(crate) mod interface_style;
pub(crate) mod quick_pick;
pub(crate) mod resize_edge;
pub(crate) mod ring;
pub(crate) mod shimmer;
pub(crate) mod squircle;
pub(crate) mod stroke;

use gpui::{
    AnimationExt, AnyElement, AnyView, App, Bounds, ClickEvent, CursorStyle, Div, Element,
    ElementId, FontWeight, GlobalElementId, Hsla, InspectorElementId, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Render, Rgba, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, prelude::*, px, svg,
};
use std::time::{Duration, Instant};

use crate::settings::components::with_alpha;
use crate::theme::UiColors;

const HOVER_ANIMATION_DURATION: Duration = Duration::from_millis(120);
pub(crate) const TOGGLE_ANIMATION_DURATION: Duration = Duration::from_millis(180);

#[derive(Clone, Debug)]
struct HoverAnimationState {
    from: f32,
    target: f32,
    started_at: Instant,
    duration: Duration,
    hitbox: Option<gpui::Hitbox>,
}

impl HoverAnimationState {
    fn resting_at(on: bool) -> Self {
        let rest = if on { 1.0 } else { 0.0 };
        Self {
            from: rest,
            target: rest,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            hitbox: None,
        }
    }

    fn progress_at(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return self.target;
        }

        let elapsed = now.duration_since(self.started_at).as_secs_f32();
        let linear = (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0);
        self.from + (self.target - self.from) * ease_out_quint(linear)
    }

    fn retarget(&mut self, hovered: bool, now: Instant, span: Duration) -> bool {
        let target = if hovered { 1.0 } else { 0.0 };
        if target == self.target {
            return false;
        }

        let current = self.progress_at(now);
        self.from = current;
        self.target = target;
        self.started_at = now;
        self.duration = span.mul_f32((target - current).abs());
        true
    }

    fn aims_at(&self, hovered: bool) -> bool {
        self.target == if hovered { 1.0 } else { 0.0 }
    }

    fn is_animating(&self, now: Instant) -> bool {
        !self.duration.is_zero() && now.duration_since(self.started_at) < self.duration
    }
}

fn ease_out_quint(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(5)
}

pub(crate) fn lerp_color(from: Hsla, to: Hsla, delta: f32) -> Hsla {
    let from = Rgba::from(from);
    let to = Rgba::from(to);
    let delta = delta.clamp(0.0, 1.0);
    Hsla::from(Rgba {
        r: from.r + (to.r - from.r) * delta,
        g: from.g + (to.g - from.g) * delta,
        b: from.b + (to.b - from.b) * delta,
        a: from.a + (to.a - from.a) * delta,
    })
}

static REDUCE_MOTION: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_reduce_motion(enabled: bool, cx: &mut App) {
    REDUCE_MOTION.store(enabled, std::sync::atomic::Ordering::Relaxed);
    cx.set_reduce_motion(enabled);
}

pub(crate) fn reduce_motion() -> bool {
    REDUCE_MOTION.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) const MENU_REVEAL_MS: u64 = 140;
const MENU_REVEAL_DROP: f32 = 4.;

pub(crate) fn ease_out_cubic(delta: f32) -> f32 {
    1. - (1. - delta).powi(3)
}

pub(crate) fn menu_reveal<E>(id: impl Into<ElementId>, element: E) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    if reduce_motion() {
        return element.into_any_element();
    }
    element
        .with_animation(
            id,
            gpui::Animation::new(Duration::from_millis(MENU_REVEAL_MS)).with_easing(ease_out_cubic),
            |element, delta| {
                element
                    .opacity(delta)
                    .mt(px(-MENU_REVEAL_DROP * (1. - delta)))
            },
        )
        .into_any_element()
}

pub(crate) type ClipLine = std::rc::Rc<std::cell::Cell<Option<Pixels>>>;

pub(crate) struct ClipBelowLine {
    child: AnyElement,
    line: ClipLine,
}

pub(crate) fn clip_below_line(child: impl IntoElement, line: ClipLine) -> ClipBelowLine {
    ClipBelowLine {
        child: child.into_any_element(),
        line,
    }
}

impl Element for ClipBelowLine {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(line) = self.line.get() else {
            self.child.paint(window, cx);
            return;
        };
        let height = (line - bounds.origin.y).clamp(px(0.), bounds.size.height);
        let mask = gpui::ContentMask {
            bounds: Bounds {
                origin: bounds.origin,
                size: gpui::size(bounds.size.width, height),
            },
        };
        window.with_content_mask(Some(mask), |window| self.child.paint(window, cx));
    }
}

type StyleAnimator = dyn for<'a> Fn(&mut AnimatedStyle<'a>, f32);
type ElementAnimator = dyn for<'a> FnOnce(&mut AnimatedElement<'a>, f32);

pub(crate) struct AnimatedHover {
    element: Stateful<Div>,
    style_animator: Option<Box<StyleAnimator>>,
    element_animator: Option<Box<ElementAnimator>>,
    driven_by: Option<bool>,
    span: Duration,
}

pub(crate) struct AnimatedStyle<'a>(&'a mut StyleRefinement);

impl AnimatedStyle<'_> {
    pub(crate) fn bg(&mut self, fill: impl Into<gpui::Fill>) -> &mut Self {
        *self.0 = std::mem::take(self.0).bg(fill);
        self
    }

    pub(crate) fn text_color(&mut self, color: impl Into<Hsla>) -> &mut Self {
        *self.0 = std::mem::take(self.0).text_color(color);
        self
    }

    pub(crate) fn border_color(&mut self, color: impl Into<Hsla>) -> &mut Self {
        *self.0 = std::mem::take(self.0).border_color(color);
        self
    }

    pub(crate) fn opacity(&mut self, opacity: f32) -> &mut Self {
        *self.0 = std::mem::take(self.0).opacity(opacity);
        self
    }
}

pub(crate) struct AnimatedElement<'a>(&'a mut Stateful<Div>);

impl AnimatedElement<'_> {
    pub(crate) fn style(&mut self) -> AnimatedStyle<'_> {
        AnimatedStyle(self.0.style())
    }

    pub(crate) fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.0.extend(elements);
    }
}

pub(crate) trait AnimatedHoverExt {
    fn animated_hover(
        self,
        animator: impl for<'a> Fn(&mut AnimatedStyle<'a>, f32) + 'static,
    ) -> AnimatedHover;

    fn animated_hover_element(
        self,
        animator: impl for<'a> FnOnce(&mut AnimatedElement<'a>, f32) + 'static,
    ) -> AnimatedHover;

    fn animated_hover_bg(self, resting: Hsla, hovered: Hsla) -> AnimatedHover
    where
        Self: Sized;

    fn animated_toggle(
        self,
        on: bool,
        animator: impl for<'a> FnOnce(&mut AnimatedElement<'a>, f32) + 'static,
    ) -> AnimatedHover;
}

impl AnimatedHoverExt for Stateful<Div> {
    fn animated_hover(
        self,
        animator: impl for<'a> Fn(&mut AnimatedStyle<'a>, f32) + 'static,
    ) -> AnimatedHover {
        AnimatedHover {
            element: self.hover(|style| style),
            style_animator: Some(Box::new(animator)),
            element_animator: None,
            driven_by: None,
            span: HOVER_ANIMATION_DURATION,
        }
    }

    fn animated_hover_element(
        self,
        animator: impl for<'a> FnOnce(&mut AnimatedElement<'a>, f32) + 'static,
    ) -> AnimatedHover {
        AnimatedHover {
            element: self.hover(|style| style),
            style_animator: None,
            element_animator: Some(Box::new(animator)),
            driven_by: None,
            span: HOVER_ANIMATION_DURATION,
        }
    }

    fn animated_toggle(
        self,
        on: bool,
        animator: impl for<'a> FnOnce(&mut AnimatedElement<'a>, f32) + 'static,
    ) -> AnimatedHover {
        AnimatedHover {
            element: self,
            style_animator: None,
            element_animator: Some(Box::new(animator)),
            driven_by: Some(on),
            span: TOGGLE_ANIMATION_DURATION,
        }
    }

    fn animated_hover_bg(self, resting: Hsla, hovered: Hsla) -> AnimatedHover {
        self.animated_hover(move |style, delta| {
            style.bg(lerp_color(resting, hovered, delta));
        })
    }
}

impl Styled for AnimatedHover {
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl InteractiveElement for AnimatedHover {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.element.interactivity()
    }
}

impl StatefulInteractiveElement for AnimatedHover {}

impl ParentElement for AnimatedHover {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements)
    }
}

impl Element for AnimatedHover {
    type RequestLayoutState = <Stateful<Div> as Element>::RequestLayoutState;
    type PrepaintState = <Stateful<Div> as Element>::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        <Stateful<Div> as Element>::id(&self.element)
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.element.source_location()
    }

    fn a11y_role(&self) -> Option<gpui::accesskit::Role> {
        self.element.a11y_role()
    }

    fn write_a11y_info(&self, node: &mut gpui::accesskit::Node) {
        self.element.write_a11y_info(node);
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut gpui::A11ySubtreeBuilder,
    ) {
        <Stateful<Div> as Element>::a11y_synthetic_children(&mut self.element, prepaint, builder);
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (gpui::LayoutId, Self::RequestLayoutState) {
        let Some(global_id) = global_id else {
            return self.element.request_layout(None, inspector_id, window, cx);
        };
        let now = Instant::now();
        let reduce_motion = reduce_motion();
        let driven_by = self.driven_by;
        let span = self.span;
        let (progress, is_animating) =
            window.with_element_state(global_id, |state: Option<HoverAnimationState>, window| {
                let mut state = state
                    .unwrap_or_else(|| HoverAnimationState::resting_at(driven_by.unwrap_or(false)));
                let hovered = driven_by.unwrap_or_else(|| {
                    !cx.has_active_drag()
                        && state
                            .hitbox
                            .as_ref()
                            .is_some_and(|hitbox| hitbox.is_hovered(window))
                });
                state.retarget(hovered, now, span);
                let (progress, is_animating) = if reduce_motion {
                    (if hovered { 1.0 } else { 0.0 }, false)
                } else {
                    (state.progress_at(now), state.is_animating(now))
                };
                ((progress, is_animating), state)
            });

        if is_animating {
            window.request_animation_frame();
        }

        if let Some(animator) = self.style_animator.as_ref() {
            animator(&mut AnimatedStyle(self.element.style()), progress);
        }
        if let Some(animator) = self.element_animator.take() {
            animator(&mut AnimatedElement(&mut self.element), progress);
        }
        self.element
            .request_layout(Some(global_id), inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let Some(global_id) = global_id else {
            return self
                .element
                .prepaint(None, inspector_id, bounds, request_layout, window, cx);
        };
        let prepaint = self.element.prepaint(
            Some(global_id),
            inspector_id,
            bounds,
            request_layout,
            window,
            cx,
        );

        let hitbox = prepaint.clone();
        window.with_element_state(global_id, |state: Option<HoverAnimationState>, _window| {
            let mut state = state.unwrap_or_else(|| HoverAnimationState::resting_at(false));
            state.hitbox = hitbox;
            ((), state)
        });

        prepaint
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.driven_by.is_none()
            && let Some(global_id) = global_id
        {
            let has_drag = cx.has_active_drag();
            let stale = window.with_element_state(
                global_id,
                |state: Option<HoverAnimationState>, window| {
                    let state = state.unwrap_or_else(|| HoverAnimationState::resting_at(false));
                    let hovered = !has_drag
                        && state
                            .hitbox
                            .as_ref()
                            .is_some_and(|hitbox| hitbox.is_hovered(window));
                    (!state.aims_at(hovered), state)
                },
            );
            if stale {
                window.request_animation_frame();
            }
        }
        self.element.paint(
            global_id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        )
    }
}

pub(crate) const FOCUS_BLUE: u32 = 0x007aff;

pub(crate) const LABEL_XS: Pixels = px(10.);
pub(crate) const LABEL_SM: Pixels = px(11.);
pub(crate) const BODY: Pixels = px(12.);
pub(crate) const BODY_EMPHASIS: Pixels = px(13.);
pub(crate) const TITLE: Pixels = px(14.);

pub(crate) const ROW_RADIUS: Pixels = px(14.);

pub(crate) fn squircle_skin(
    element: Stateful<Div>,
    group: impl Into<SharedString>,
    radius: Pixels,
    resting: Option<Hsla>,
    hovered: Option<Hsla>,
) -> Stateful<Div> {
    let group: SharedString = group.into();
    let mut element = element.relative().group(group.clone());
    if let Some(resting) = resting {
        element = element.child(squircle::squircle_fill(radius, resting));
    }
    if let Some(hovered) = hovered {
        element = element.child(
            div()
                .absolute()
                .inset_0()
                .invisible()
                .group_hover(group, |style| style.visible())
                .child(squircle::squircle_fill(radius, hovered)),
        );
    }
    element
}

pub(crate) fn animated_squircle_skin(
    element: Stateful<Div>,
    group: impl Into<SharedString>,
    radius: Pixels,
    resting: Hsla,
    hovered: Hsla,
) -> AnimatedHover {
    let progress = std::rc::Rc::new(std::cell::Cell::new(0.));
    let fill = squircle::squircle_fill_blend(radius, resting, hovered, progress.clone());
    element
        .relative()
        .group(group.into())
        .child(fill)
        .animated_hover(move |_, delta| progress.set(delta))
}

const COMET_TURN: Duration = Duration::from_millis(700);
const EMPTY_STATE_TURN: Duration = Duration::from_secs(1);

fn spinner_rotation(turn: Duration, reduce_motion: bool) -> Option<gpui::TimedTransformation> {
    (!reduce_motion)
        .then(|| gpui::TimedTransformation::rotation(turn, gpui::Transformation::default()))
}

fn spin(glyph: gpui::Svg, turn: Duration) -> gpui::Svg {
    match spinner_rotation(turn, reduce_motion()) {
        Some(rotation) => glyph.with_timed_transformation(rotation),
        None => glyph,
    }
}

pub(crate) fn comet_spinner(size: Pixels, color: Hsla) -> AnyElement {
    spin(
        svg()
            .size(size)
            .flex_none()
            .path("icons/comet.svg")
            .text_color(color),
        COMET_TURN,
    )
    .into_any_element()
}

pub(crate) const DISMISS_BUTTON_SIZE: Pixels = px(24.);

pub(crate) fn dismiss_button(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    radius: Pixels,
    ink: Hsla,
    hovered_ink: Hsla,
    wash: Hsla,
) -> Stateful<Div> {
    let id: SharedString = id.into();
    let group = SharedString::from(format!("{id}-hover"));
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .size(DISMISS_BUTTON_SIZE)
            .flex()
            .items_center()
            .justify_center(),
        group.clone(),
        radius,
        None,
        Some(wash),
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
    .child(
        svg()
            .size(px(10.))
            .flex_none()
            .path("icons/close.svg")
            .text_color(ink)
            .group_hover(group, move |style| style.text_color(hovered_ink)),
    )
}

pub(crate) const TOOLTIP_SHOW_DELAY: Duration = Duration::from_millis(800);

pub(crate) trait TooltipDelayExt: Sized {
    fn delayed_tooltip(
        self,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self;
}

impl<E: StatefulInteractiveElement> TooltipDelayExt for E {
    fn delayed_tooltip(
        self,
        build_tooltip: impl Fn(&mut Window, &mut App) -> AnyView + 'static,
    ) -> Self {
        self.capture_any_mouse_down(|event, _window, _cx| {
            LAST_PRESS.set(Some(event.position));
        })
        .tooltip(move |window, cx| {
            if pointer_still_since_press(LAST_PRESS.get(), window.mouse_position()) {
                cx.new(|_| SuppressedTooltip).into()
            } else {
                build_tooltip(window, cx)
            }
        })
        .tooltip_show_delay(TOOLTIP_SHOW_DELAY)
    }
}

thread_local! {
    static LAST_PRESS: std::cell::Cell<Option<gpui::Point<Pixels>>> =
        const { std::cell::Cell::new(None) };
}

const PRESS_STILLNESS: Pixels = px(3.);

fn pointer_still_since_press(
    press: Option<gpui::Point<Pixels>>,
    pointer: gpui::Point<Pixels>,
) -> bool {
    press.is_some_and(|press| {
        (press.x - pointer.x).abs() <= PRESS_STILLNESS
            && (press.y - pointer.y).abs() <= PRESS_STILLNESS
    })
}

struct SuppressedTooltip;

impl Render for SuppressedTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub(crate) struct Press;

pub(crate) trait AccessibleControlExt: StatefulInteractiveElement + Sized {
    fn accessible_control(
        self,
        role: gpui::accesskit::Role,
        label: impl Into<SharedString>,
    ) -> Self {
        let label: SharedString = label.into();
        debug_assert!(
            !label.trim().is_empty(),
            "an interactive control needs an accessible label"
        );
        self.role(role)
            .aria_label(label.clone())
            .delayed_tooltip(text_tooltip(label))
    }

    fn on_pointer_press(self, handler: impl Fn(&Press, &mut Window, &mut App) + 'static) -> Self {
        let handler = std::rc::Rc::new(handler);
        let pointer = handler.clone();
        self.on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            window.prevent_default();
            pointer(&Press, window, cx);
        })
        .on_a11y_action(gpui::accesskit::Action::Click, move |_, window, cx| {
            handler(&Press, window, cx)
        })
    }

    fn on_press(self, handler: impl Fn(&Press, &mut Window, &mut App) + 'static) -> Self {
        let handler = std::rc::Rc::new(handler);
        let keyboard = handler.clone();
        self.focusable()
            .on_pointer_press(move |press, window, cx| handler(press, window, cx))
            .on_click(move |event, window, cx| {
                if event.is_keyboard() {
                    keyboard(&Press, window, cx);
                }
            })
    }
}

impl<E: StatefulInteractiveElement> AccessibleControlExt for E {}

pub(crate) const TOOLTIP_RADIUS: Pixels = px(14.);
pub(crate) const TOOLTIP_MAX_WIDTH: Pixels = px(400.);

pub(crate) fn tooltip_shell() -> Div {
    let theme = crate::theme::active_theme();
    let ui = crate::theme::ui_colors();
    div()
        .relative()
        .max_w(TOOLTIP_MAX_WIDTH)
        .px(px(8.))
        .py(px(6.))
        .text_color(ui.text)
        .text_sm()
        .child(squircle::squircle_fill_beneath_border(
            TOOLTIP_RADIUS,
            theme.title_bar_background,
            px(1.),
            ui.border,
        ))
        .child(squircle::squircle_border(TOOLTIP_RADIUS, px(1.), ui.border))
}

pub(crate) const TOOLTIP_POINTER_CLEARANCE_X: Pixels = px(8.);
pub(crate) const TOOLTIP_POINTER_CLEARANCE_Y: Pixels = px(12.);

fn clear_of_the_pointer(shell: Div) -> Div {
    div()
        .px(TOOLTIP_POINTER_CLEARANCE_X)
        .py(TOOLTIP_POINTER_CLEARANCE_Y)
        .child(shell)
}

pub(crate) struct PaneflowTooltip {
    pub(crate) label: SharedString,
}

impl Render for PaneflowTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        clear_of_the_pointer(
            tooltip_shell()
                .debug_selector(|| "paneflow-tooltip".into())
                .child(self.label.clone()),
        )
    }
}

pub(crate) fn text_tooltip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label: SharedString = label.into();
    move |_w, cx| {
        cx.new(|_| PaneflowTooltip {
            label: label.clone(),
        })
        .into()
    }
}

pub(crate) fn filter_pill(
    id: impl Into<ElementId>,
    clear_id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    ui: UiColors,
    input: impl IntoElement,
    show_clear: bool,
    on_clear: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let clear_id = clear_id.into();
    let label: SharedString = label.into();
    let clear_label = SharedString::from(format!("Clear {}", label.to_lowercase()));
    let mut field = div()
        .id(id.into())
        .accessible_control(gpui::accesskit::Role::Search, label)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(6.))
        .rounded(crate::app::constants::SIDEBAR_TAB_CORNER_RADIUS)
        .bg(ui.subtle)
        .cursor_text()
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path("icons/tool_search.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(BODY)
                .text_color(ui.text)
                .child(input),
        );
    if show_clear {
        field = field.child(
            div()
                .id(clear_id)
                .flex_none()
                .w(px(16.))
                .h(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .cursor(CursorStyle::Arrow)
                .text_color(ui.muted)
                .accessible_control(gpui::accesskit::Role::Button, clear_label)
                .animated_hover_element(move |button, delta| {
                    let icon_color = lerp_color(ui.muted, ui.text, delta);
                    button
                        .style()
                        .bg(lerp_color(
                            with_alpha(ui.text, 0.0),
                            with_alpha(ui.text, 0.10),
                            delta,
                        ))
                        .text_color(icon_color);
                    button.extend([svg()
                        .size(px(10.))
                        .flex_none()
                        .path("icons/close.svg")
                        .text_color(icon_color)
                        .into_any_element()]);
                })
                .on_click(on_clear),
        );
    }
    field
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum FilterFieldGlyph {
    Filter,
    Search,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum FilterFieldShape {
    Capsule,
    Well(Pixels),
}

const CLASSIC_FIELD_REST_ALPHA: f32 = 0.05;

#[derive(Clone, Copy)]
pub(crate) struct FilterFieldStyle {
    pub(crate) glyph: FilterFieldGlyph,
    pub(crate) shape: FilterFieldShape,
    pub(crate) glyph_size: Pixels,
    pub(crate) height: Pixels,
    pub(crate) padding: Pixels,
    pub(crate) text_size: Pixels,
}

impl FilterFieldStyle {
    pub(crate) fn sidebar(glyph: FilterFieldGlyph) -> Self {
        Self {
            glyph,
            shape: FilterFieldShape::Capsule,
            glyph_size: glyph.size(),
            height: px(36.),
            padding: px(10.),
            text_size: px(15.),
        }
    }

    pub(crate) fn palette() -> Self {
        Self {
            glyph: FilterFieldGlyph::Search,
            shape: FilterFieldShape::Well(px(9.)),
            glyph_size: px(14.),
            height: px(34.),
            padding: px(11.),
            text_size: px(14.),
        }
    }
}

impl FilterFieldGlyph {
    fn size(self) -> Pixels {
        match self {
            Self::Filter => px(20.),
            Self::Search => px(17.),
        }
    }

    fn path(self, focused: bool) -> &'static str {
        match (self, focused) {
            (Self::Filter, true) => "icons/filter-circle.svg",
            (Self::Filter, false) => "icons/filter-circle-outline.svg",
            (Self::Search, _) => "icons/tool_search.svg",
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn filter_field(
    id: impl Into<ElementId>,
    clear_id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    ui: UiColors,
    style: FilterFieldStyle,
    focused: bool,
    has_query: bool,
    input_visible: bool,
    prefix: Option<AnyElement>,
    input: impl IntoElement,
    on_clear: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let active_bg = crate::app::constants::sidebar_tab_active_background();
    let hover_bg = crate::app::constants::sidebar_tab_hover_background();
    let clear_id = clear_id.into();
    let label: SharedString = label.into();
    let clear_label = SharedString::from(format!("Clear {}", label.to_lowercase()));
    let id: ElementId = id.into();
    let base = div()
        .id(id.clone())
        .accessible_control(gpui::accesskit::Role::Search, label)
        .min_w_0()
        .h(style.height)
        .px(style.padding)
        .flex()
        .items_center()
        .gap(px(8.));
    let filled = focused || has_query;
    let shaped = match style.shape {
        FilterFieldShape::Capsule if interface_style::classic() => base
            .rounded(interface_style::CLASSIC_CONTROL)
            .when(filled, |field| field.bg(active_bg))
            .when(input_visible && !filled, |field| {
                field.bg(with_alpha(ui.text, CLASSIC_FIELD_REST_ALPHA))
            })
            .hover(move |hovered| hovered.bg(if filled { active_bg } else { hover_bg })),
        FilterFieldShape::Capsule => base
            .rounded_full()
            .when(filled, |field| field.bg(active_bg))
            .hover(move |hovered| hovered.bg(if filled { active_bg } else { hover_bg })),
        FilterFieldShape::Well(radius) => base
            .relative()
            .child(squircle::squircle_fill(radius, with_alpha(ui.text, 0.07))),
    };
    let glyph = style.glyph;
    let glyph_lit = focused && style.shape == FilterFieldShape::Capsule;
    shaped
        .cursor_text()
        .child(match prefix {
            Some(prefix) => prefix,
            None => svg()
                .relative()
                .size(style.glyph_size)
                .flex_none()
                .path(glyph.path(focused))
                .text_color(if glyph_lit {
                    crate::app::constants::sidebar_filter_icon_color()
                } else {
                    ui.muted
                })
                .into_any_element(),
        })
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .text_size(style.text_size)
                .line_height(style.text_size + px(5.))
                .text_color(ui.text)
                .when(!input_visible, |field| field.invisible())
                .child(input),
        )
        .when(has_query, |field| {
            field.child(
                div()
                    .id(clear_id)
                    .size(px(18.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(ui.muted.opacity(0.2))
                    .cursor_pointer()
                    .accessible_control(gpui::accesskit::Role::Button, clear_label)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(on_clear)
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/close.svg")
                            .text_color(ui.text),
                    ),
            )
        })
}

pub(crate) fn single_line_label() -> Div {
    div()
        .flex_1()
        .min_w_0()
        .overflow_x_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
}

pub(crate) fn highlight_matches(text: String, query: &str) -> gpui::StyledText {
    let mut ranges = Vec::new();
    if !query.is_empty() {
        let mut normalized = String::new();
        let mut offsets = Vec::new();
        for (start, ch) in text.char_indices() {
            let lowered = ch.to_lowercase().to_string();
            offsets.extend(std::iter::repeat_n(
                start..start + ch.len_utf8(),
                lowered.len(),
            ));
            normalized.push_str(&lowered);
        }
        for (start, matched) in normalized.match_indices(query) {
            let range = offsets[start].start..offsets[start + matched.len() - 1].end;
            if ranges
                .last()
                .is_none_or(|previous: &std::ops::Range<usize>| previous.end <= range.start)
            {
                ranges.push(range);
            }
        }
    }
    gpui::StyledText::new(text).with_highlights(ranges.into_iter().map(|range| {
        (
            range,
            gpui::HighlightStyle {
                color: Some(crate::app::constants::filter_match_color()),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
        )
    }))
}

pub(crate) fn section_eyebrow(label: impl Into<SharedString>, ui: UiColors) -> Div {
    div()
        .text_size(LABEL_SM)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ui.muted)
        .child(label.into())
}

const EMPTY_STATE_PROGRESS_LABEL: &str = "Loading";

fn empty_state_spins(animate: bool, reduce_motion: bool) -> bool {
    animate && !reduce_motion
}

pub(crate) fn panel_empty_state(
    ui: UiColors,
    icon: Option<&'static str>,
    title: Option<SharedString>,
    message: impl Into<SharedString>,
    animate: bool,
) -> Div {
    let mut col = div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .p(px(12.));
    if let Some(path) = icon {
        let glyph = svg()
            .size(px(18.))
            .flex_none()
            .path(path)
            .text_color(with_alpha(ui.muted, 0.8));
        let glyph = if empty_state_spins(animate, reduce_motion()) {
            spin(glyph, EMPTY_STATE_TURN).into_any_element()
        } else {
            glyph.into_any_element()
        };
        col = col.child(if animate {
            div()
                .id("panel-empty-progress")
                .role(gpui::accesskit::Role::ProgressIndicator)
                .aria_label(EMPTY_STATE_PROGRESS_LABEL)
                .child(glyph)
                .into_any_element()
        } else {
            glyph
        });
    }
    if let Some(title) = title {
        col = col.child(
            div()
                .text_size(TITLE)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child(title),
        );
    }
    col.child(
        div()
            .text_size(BODY)
            .text_color(ui.muted)
            .child(message.into()),
    )
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc, thread};

    #[test]
    fn a_toggle_mounted_on_rests_without_animating() {
        let now = Instant::now();
        let mut state = HoverAnimationState::resting_at(true);
        assert!(!state.retarget(true, now, TOGGLE_ANIMATION_DURATION));
        assert_eq!(state.progress_at(now), 1.0);
        assert!(!state.is_animating(now));
    }

    #[test]
    fn a_retarget_midway_spans_only_the_distance_left() {
        let start = Instant::now();
        let mut state = HoverAnimationState::resting_at(false);
        assert!(state.retarget(true, start, TOGGLE_ANIMATION_DURATION));
        let midway = start + TOGGLE_ANIMATION_DURATION / 2;
        let reached = state.progress_at(midway);
        assert!(state.retarget(false, midway, TOGGLE_ANIMATION_DURATION));
        assert_eq!(state.duration, TOGGLE_ANIMATION_DURATION.mul_f32(reached));
        assert_eq!(state.progress_at(midway), reached);
    }

    use gpui::{InputEvent, Modifiers, MouseMoveEvent, TestAppContext, point, size};

    use super::*;

    struct HoverHarness {
        progress: Rc<Cell<f32>>,
    }

    struct SquircleHoverHarness {
        renders: Rc<Cell<usize>>,
        sibling: gpui::Entity<HoverSibling>,
    }

    struct HoverSibling {
        renders: Rc<Cell<usize>>,
    }

    impl Render for HoverSibling {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().size_full()
        }
    }

    impl Render for SquircleHoverHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
                .flex()
                .flex_col()
                .size_full()
                .children((0_usize..2).map(|ix| {
                    squircle_skin(
                        div().id(("files-hover-row", ix)),
                        SharedString::from(format!("files-hover-group-{ix}")),
                        ROW_RADIUS,
                        None,
                        Some(gpui::white()),
                    )
                    .w(px(100.))
                    .h(px(28.))
                    .flex_none()
                }))
                .child(
                    self.sibling
                        .clone()
                        .cached(StyleRefinement::default().w(px(100.)).h(px(20.))),
                )
        }
    }

    fn labeled_control<E: Element>(element: &E) -> (gpui::accesskit::Role, Option<String>) {
        let role = element
            .a11y_role()
            .expect("a shared interactive helper exposes a role");
        let mut node = gpui::accesskit::Node::new(role);
        element.write_a11y_info(&mut node);
        (role, node.label().map(str::to_owned))
    }

    type HelperCheck = (
        &'static str,
        fn(&'static str) -> (gpui::accesskit::Role, Option<String>),
    );

    fn shared_interactive_helpers() -> Vec<HelperCheck> {
        use crate::settings::components as c;
        vec![
            ("toggle_switch", |label| {
                labeled_control(&c::toggle_switch(
                    "t",
                    label,
                    true,
                    crate::theme::ui_colors(),
                ))
            }),
            ("select_trigger", |label| {
                labeled_control(&c::select_trigger(
                    "s",
                    label,
                    false,
                    crate::theme::ui_colors(),
                ))
            }),
            ("raised_icon_button", |label| {
                labeled_control(&c::raised_icon_button(
                    "i",
                    label,
                    "icons/plus.svg",
                    gpui::black(),
                    gpui::white(),
                    true,
                ))
            }),
            ("soft_icon_button", |label| {
                labeled_control(&c::soft_icon_button(
                    "v",
                    label,
                    "icons/check.svg",
                    crate::theme::ui_colors(),
                ))
            }),
            ("ghost_icon_button", |label| {
                labeled_control(&c::ghost_icon_button(
                    "g",
                    label,
                    "icons/plus.svg",
                    crate::theme::ui_colors(),
                ))
            }),
            ("raised_button", |label| {
                labeled_control(&c::raised_button("p", label, gpui::black(), gpui::white()))
            }),
            ("soft_button", |label| {
                labeled_control(&c::soft_button(
                    "t",
                    label,
                    crate::theme::ui_colors(),
                    |_, _, _| {},
                ))
            }),
            ("secondary_button", |label| {
                labeled_control(&c::secondary_button(
                    "b",
                    label,
                    crate::theme::ui_colors(),
                    |_, _, _| {},
                ))
            }),
            ("solid_button", |label| {
                labeled_control(&c::solid_button("o", label, gpui::black()))
            }),
            ("destructive_button", |label| {
                labeled_control(&c::destructive_button("r", label))
            }),
            ("dismiss_button", |label| {
                labeled_control(&dismiss_button(
                    "x",
                    label,
                    ROW_RADIUS,
                    gpui::black(),
                    gpui::black(),
                    gpui::white(),
                ))
            }),
            ("filter_pill", |label| {
                labeled_control(&filter_pill(
                    "f",
                    "fc",
                    label,
                    crate::theme::ui_colors(),
                    div(),
                    true,
                    |_, _, _| {},
                ))
            }),
            ("filter_field", |label| {
                labeled_control(&filter_field(
                    "ff",
                    "ffc",
                    label,
                    crate::theme::ui_colors(),
                    FilterFieldStyle::palette(),
                    false,
                    true,
                    true,
                    None,
                    div(),
                    |_, _, _| {},
                ))
            }),
            ("render_window_button", |label| {
                let _ = label;
                labeled_control(&crate::window_chrome::csd::render_window_button(
                    "right",
                    gpui::WindowButton::Close,
                    false,
                    px(32.),
                    |_, _| {},
                ))
            }),
            ("render_diff_header_icon_button", |label| {
                labeled_control(&crate::app::diff_dock::render_diff_header_icon_button(
                    "h",
                    label,
                    "icons/close.svg",
                    |_, _, _| {},
                    gpui::black(),
                ))
            }),
        ]
    }

    #[test]
    fn every_shared_interactive_helper_exposes_its_label_as_role_and_name() {
        for (name, build) in shared_interactive_helpers() {
            let (role, label) = build("Do the thing");
            assert_ne!(role, gpui::accesskit::Role::GenericContainer, "{name}");
            let expected = if name == "render_window_button" {
                "Close"
            } else {
                "Do the thing"
            };
            assert_eq!(label.as_deref(), Some(expected), "{name}");
        }
    }

    #[test]
    fn no_shared_interactive_helper_can_be_built_without_a_label() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let unlabeled: Vec<&str> = shared_interactive_helpers()
            .into_iter()
            .filter(|(name, _)| *name != "render_window_button")
            .filter(|(_, build)| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build("  "))).is_ok()
            })
            .map(|(name, _)| name)
            .collect();
        std::panic::set_hook(previous);
        assert!(
            unlabeled.is_empty(),
            "these helpers build without an accessible label: {unlabeled:?}"
        );
        for button in [
            gpui::WindowButton::Minimize,
            gpui::WindowButton::Maximize,
            gpui::WindowButton::Close,
        ] {
            for maximized in [false, true] {
                let label = crate::window_chrome::csd::window_button_label(button, maximized);
                assert!(!label.trim().is_empty());
            }
        }
    }

    #[test]
    fn the_empty_state_spinner_is_static_under_reduce_motion() {
        assert!(empty_state_spins(true, false));
        assert!(!empty_state_spins(true, true));
        assert!(!empty_state_spins(false, false));
        assert!(!EMPTY_STATE_PROGRESS_LABEL.is_empty());
    }

    #[test]
    fn spinners_turn_once_per_period_on_the_shared_clock_and_rest_under_reduce_motion() {
        let turn =
            |period| gpui::TimedTransformation::rotation(period, gpui::Transformation::default());
        assert_eq!(spinner_rotation(COMET_TURN, false), Some(turn(COMET_TURN)));
        assert_eq!(COMET_TURN, Duration::from_millis(700));
        assert_eq!(
            spinner_rotation(EMPTY_STATE_TURN, false),
            Some(turn(EMPTY_STATE_TURN))
        );
        assert_eq!(EMPTY_STATE_TURN, Duration::from_secs(1));
        assert_eq!(spinner_rotation(COMET_TURN, true), None);
        assert_eq!(spinner_rotation(EMPTY_STATE_TURN, true), None);
        assert_eq!(
            spinner_rotation(COMET_TURN, false),
            spinner_rotation(COMET_TURN, false),
            "two comet spinners carry the same rotation, so the shared clock gives them one phase"
        );
    }

    struct SpinnersOnly {
        renders: Rc<Cell<usize>>,
    }

    impl Render for SpinnersOnly {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div()
                .size_full()
                .child(comet_spinner(px(14.), gpui::white()))
                .child(comet_spinner(px(12.), gpui::white()))
                .child(panel_empty_state(
                    crate::theme::ui_colors(),
                    Some("icons/comet.svg"),
                    None,
                    "Loading",
                    true,
                ))
        }
    }

    #[gpui::test]
    fn spinners_turn_for_sixty_frames_without_rendering_their_view_again(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let (_view, cx) = cx.add_window_view({
            let renders = renders.clone();
            move |_, _| SpinnersOnly { renders }
        });
        cx.run_until_parked();
        assert_eq!(renders.get(), 1);
        for _ in 0..60 {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
            });
            cx.run_until_parked();
        }
        assert_eq!(
            renders.get(),
            1,
            "a spinner must turn in the retained scene, not by rendering its view each frame"
        );
    }

    #[gpui::test]
    fn squircle_skin_hover_repaints_on_pointer_transitions(cx: &mut TestAppContext) {
        let renders = Rc::new(Cell::new(0));
        let renders_for_view = renders.clone();
        let sibling_renders = Rc::new(Cell::new(0));
        let sibling_renders_for_view = sibling_renders.clone();
        let (_view, cx) = cx.add_window_view(move |_, cx| SquircleHoverHarness {
            renders: renders_for_view,
            sibling: cx.new(|_| HoverSibling {
                renders: sibling_renders_for_view,
            }),
        });
        cx.simulate_resize(size(px(200.), px(100.)));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.simulate_mouse_move(point(px(150.), px(80.)), cx);
        });

        for (position, should_repaint) in [
            (point(px(25.), px(14.)), true),
            (point(px(30.), px(14.)), false),
            (point(px(25.), px(42.)), true),
            (point(px(30.), px(42.)), false),
            (point(px(150.), px(80.)), true),
        ] {
            let before = renders.get();
            let sibling_before = sibling_renders.get();
            cx.update(|window, cx| window.simulate_mouse_move(position, cx));
            assert_eq!(
                renders.get() - before,
                usize::from(should_repaint),
                "row hover must repaint on entry and exit without waiting for another UI event; position: {position:?}"
            );
            assert_eq!(
                sibling_renders.get(),
                sibling_before,
                "row hover must not invalidate a cached sibling"
            );
        }
    }

    struct StyledSurfaces;

    impl Render for StyledSurfaces {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div()
                .size_full()
                .relative()
                .child(squircle::squircle_fill(px(24.), gpui::white()))
                .child(squircle::squircle_fill_beneath_border(
                    px(24.),
                    gpui::white(),
                    px(1.),
                    gpui::black(),
                ))
                .child(squircle::squircle_border(px(24.), px(1.), gpui::black()))
                .child(squircle::squircle_fill_blend(
                    px(14.),
                    gpui::white(),
                    gpui::black(),
                    Rc::new(Cell::new(0.5)),
                ))
        }
    }

    #[gpui::test]
    fn circular_fills_and_borders_paint_quads_without_building_a_path(cx: &mut TestAppContext) {
        use interface_style::Shape;

        let (_view, cx) = cx.add_window_view(|_, _| StyledSurfaces);
        cx.simulate_resize(size(px(200.), px(100.)));
        let before = squircle::squircle_builds();
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        });
        let continuous = squircle::squircle_builds();
        assert!(
            continuous > before,
            "the Rounded style traces its continuous corners as paths"
        );
        for shape in [Shape::Surface, Shape::Classic] {
            interface_style::scoped(shape, || {
                cx.update(|window, cx| {
                    window.refresh();
                    window.draw(cx).clear(cx);
                });
            });
        }
        assert_eq!(
            squircle::squircle_builds(),
            continuous,
            "the surface and Classic shapes paint their fills and borders as quads"
        );
    }

    impl Render for HoverHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let progress = self.progress.clone();
            div()
                .id("animated-hover-regression")
                .w(px(50.))
                .h(px(50.))
                .animated_hover(move |style, delta| {
                    progress.set(delta);
                    style.opacity(0.5 + delta * 0.5);
                })
        }
    }

    #[gpui::test]
    fn animated_hover_progresses_after_pointer_entry(cx: &mut TestAppContext) {
        let progress = Rc::new(Cell::new(0.0));
        let progress_for_view = progress.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| HoverHarness {
            progress: progress_for_view,
        });
        cx.simulate_resize(size(px(100.), px(100.)));

        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.dispatch_event(
                MouseMoveEvent {
                    position: point(px(25.), px(25.)),
                    modifiers: Modifiers::default(),
                    pressed_button: None,
                }
                .to_platform_input(),
                cx,
            );
            window.draw(cx).clear(cx);
        });

        thread::sleep(Duration::from_millis(10));
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        assert!(
            progress.get() > 0.0,
            "hover progress stayed at zero after pointer entry"
        );
    }

    #[test]
    fn a_pointer_that_has_not_moved_since_a_press_holds_the_tooltip_back() {
        let press = Some(point(px(40.), px(20.)));
        assert!(pointer_still_since_press(press, point(px(40.), px(20.))));
        assert!(pointer_still_since_press(press, point(px(42.), px(18.))));
        assert!(!pointer_still_since_press(press, point(px(48.), px(20.))));
        assert!(!pointer_still_since_press(None, point(px(40.), px(20.))));
    }

    struct TooltipHarness {
        built: Rc<Cell<usize>>,
    }

    impl Render for TooltipHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let built = self.built.clone();
            div().size_full().child(
                div()
                    .id("tooltip-trigger")
                    .w(px(100.))
                    .h(px(30.))
                    .on_click(|_, _, _| {})
                    .delayed_tooltip(move |_window, cx| {
                        built.set(built.get() + 1);
                        cx.new(|_| PaneflowTooltip {
                            label: "Customize sidebar".into(),
                        })
                        .into()
                    }),
            )
        }
    }

    #[gpui::test]
    fn a_click_holds_its_trigger_tooltip_back_until_the_pointer_comes_back(
        cx: &mut TestAppContext,
    ) {
        let built = Rc::new(Cell::new(0));
        let built_for_view = built.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| TooltipHarness {
            built: built_for_view,
        });
        cx.simulate_resize(size(px(300.), px(200.)));
        let wait_for_tooltip = |cx: &mut gpui::VisualTestContext| {
            cx.executor().advance_clock(TOOLTIP_SHOW_DELAY * 2);
            cx.run_until_parked();
            cx.update(|window, cx| window.draw(cx).clear(cx));
        };
        cx.update(|window, cx| window.draw(cx).clear(cx));

        cx.simulate_mouse_move(point(px(50.), px(15.)), None, Modifiers::default());
        wait_for_tooltip(cx);
        assert_eq!(built.get(), 1, "a hovered trigger shows its tooltip");

        cx.simulate_mouse_move(point(px(250.), px(150.)), None, Modifiers::default());
        wait_for_tooltip(cx);
        cx.simulate_mouse_move(point(px(60.), px(15.)), None, Modifiers::default());
        cx.simulate_click(point(px(60.), px(15.)), Modifiers::default());
        wait_for_tooltip(cx);
        assert_eq!(
            built.get(),
            1,
            "a tooltip that comes due after a click on its trigger, with the pointer still, would cover the menu the click opened"
        );

        cx.simulate_mouse_move(point(px(250.), px(150.)), None, Modifiers::default());
        wait_for_tooltip(cx);
        cx.simulate_mouse_move(point(px(40.), px(15.)), None, Modifiers::default());
        wait_for_tooltip(cx);
        assert_eq!(built.get(), 2, "the tooltip comes back on the next hover");
    }

    struct LongTooltipHarness;

    impl Render for LongTooltipHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().child(
                tooltip_shell()
                    .debug_selector(|| "tooltip".into())
                    .child("arthur@dune:/tmp/claude-1000/-home-arthur-dev-paneflow/defde28f-5ff0-423e-a803-98654a37048c/scratchpad/ep7/work/repo"),
            )
        }
    }

    #[gpui::test]
    fn a_long_path_tooltip_wraps_within_its_maximum_width(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| LongTooltipHarness);
        cx.simulate_resize(size(px(1200.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = cx.debug_bounds("tooltip").expect("tooltip painted");
        assert!(
            bounds.size.width <= TOOLTIP_MAX_WIDTH,
            "the tooltip is {:?} wide, past its {:?} maximum",
            bounds.size.width,
            TOOLTIP_MAX_WIDTH
        );
        assert!(
            bounds.size.height > px(40.),
            "a path longer than the maximum width wraps onto a second line, got {:?}",
            bounds.size.height
        );
    }

    struct PointerTooltipHarness {
        trigger: Bounds<Pixels>,
    }

    impl Render for PointerTooltipHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            div().size_full().relative().child(
                div()
                    .id("pointer-tooltip-trigger")
                    .absolute()
                    .left(self.trigger.origin.x)
                    .top(self.trigger.origin.y)
                    .w(self.trigger.size.width)
                    .h(self.trigger.size.height)
                    .on_click(|_, _, _| {})
                    .delayed_tooltip(text_tooltip("Split right")),
            )
        }
    }

    fn tooltip_shown_at(
        cx: &mut TestAppContext,
        trigger: Bounds<Pixels>,
        pointer: gpui::Point<Pixels>,
    ) -> Bounds<Pixels> {
        let (_view, cx) = cx.add_window_view(move |_, _| PointerTooltipHarness { trigger });
        cx.simulate_resize(size(px(400.), px(300.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_move(pointer, None, Modifiers::default());
        cx.executor().advance_clock(TOOLTIP_SHOW_DELAY * 2);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("paneflow-tooltip")
            .expect("tooltip painted")
    }

    #[gpui::test]
    fn a_tooltip_opens_below_and_beside_the_pointer_without_touching_it(cx: &mut TestAppContext) {
        let pointer = point(px(60.), px(52.));
        let tooltip = tooltip_shown_at(
            cx,
            Bounds::new(point(px(40.), px(40.)), size(px(24.), px(24.))),
            pointer,
        );
        assert!(
            tooltip.top() >= pointer.y + TOOLTIP_POINTER_CLEARANCE_Y,
            "the tooltip at {tooltip:?} starts too close under the pointer {pointer:?}"
        );
        assert!(
            tooltip.left() >= pointer.x + TOOLTIP_POINTER_CLEARANCE_X,
            "the tooltip at {tooltip:?} starts too close beside the pointer {pointer:?}"
        );
    }

    #[gpui::test]
    fn a_tooltip_that_flips_above_the_pointer_keeps_the_same_clearance(cx: &mut TestAppContext) {
        let pointer = point(px(380.), px(288.));
        let tooltip = tooltip_shown_at(
            cx,
            Bounds::new(point(px(360.), px(276.)), size(px(24.), px(24.))),
            pointer,
        );
        assert!(
            tooltip.bottom() <= pointer.y - TOOLTIP_POINTER_CLEARANCE_Y,
            "the flipped tooltip at {tooltip:?} covers the pointer row {pointer:?}"
        );
        assert!(
            tooltip.right() <= pointer.x - TOOLTIP_POINTER_CLEARANCE_X,
            "the flipped tooltip at {tooltip:?} covers the pointer {pointer:?}"
        );
    }

    struct ScrimHarness {
        progress: Rc<Cell<f32>>,
        covered: bool,
    }

    impl Render for ScrimHarness {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let progress = self.progress.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .id("row-under-a-modal")
                        .w(px(100.))
                        .h(px(30.))
                        .animated_hover(move |_, delta| progress.set(delta)),
                )
                .when(self.covered, |root| {
                    root.child(div().id("modal-scrim").absolute().inset_0().occlude())
                })
        }
    }

    #[gpui::test]
    fn a_row_covered_by_a_modal_schedules_the_frame_that_drops_its_hover(cx: &mut TestAppContext) {
        let progress = Rc::new(Cell::new(0.0));
        let held = progress.clone();
        let (view, cx) = cx.add_window_view(move |_, _| ScrimHarness {
            progress: held,
            covered: false,
        });
        cx.simulate_resize(size(px(200.), px(100.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.simulate_mouse_move(point(px(20.), px(15.)), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        thread::sleep(HOVER_ANIMATION_DURATION * 2);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            progress.get() > 0.99,
            "the row is hovered: {}",
            progress.get()
        );
        cx.update(|window, cx| window.simulate_next_frame(cx));

        view.update(cx, |harness, cx| {
            harness.covered = true;
            cx.notify();
        });
        cx.run_until_parked();
        for _ in 0..4 {
            thread::sleep(HOVER_ANIMATION_DURATION);
            cx.update(|window, cx| window.simulate_next_frame(cx));
            cx.run_until_parked();
        }
        assert!(
            progress.get() < 0.01,
            "a modal that now covers the hovered row drops its hover without a pointer move: {}",
            progress.get()
        );
    }

    #[cfg(target_os = "linux")]
    struct InkedAtlasAssets;

    #[cfg(target_os = "linux")]
    const INKED_TILE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><rect width="24" height="24" fill="currentColor"/></svg>"#;

    #[cfg(target_os = "linux")]
    impl gpui::AssetSource for InkedAtlasAssets {
        fn load(&self, path: &str) -> gpui::Result<Option<std::borrow::Cow<'static, [u8]>>> {
            if path.starts_with("inked/") {
                return Ok(Some(std::borrow::Cow::Borrowed(INKED_TILE_SVG.as_bytes())));
            }
            gpui::AssetSource::load(&crate::assets::Assets, path)
        }

        fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
            gpui::AssetSource::list(&crate::assets::Assets, path)
        }
    }

    #[cfg(target_os = "linux")]
    const TURNING_COMET_ORIGIN: f32 = 100.;
    #[cfg(target_os = "linux")]
    const TURNING_COMET_SIZE: f32 = 6.;
    #[cfg(target_os = "linux")]
    const TURNING_COMET_BACKGROUND: u32 = 0x202020;

    #[cfg(target_os = "linux")]
    struct TurningCometBetweenInkedTiles {
        angle: f32,
    }

    #[cfg(target_os = "linux")]
    impl Render for TurningCometBetweenInkedTiles {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            let inked = |index: usize| {
                svg()
                    .path(SharedString::from(format!("inked/{index}.svg")))
                    .size(px(TURNING_COMET_SIZE))
                    .flex_none()
                    .text_color(gpui::white())
            };
            div()
                .size_full()
                .bg(gpui::rgb(TURNING_COMET_BACKGROUND))
                .child(div().flex().gap_2().children((0..4).map(inked)))
                .child(
                    div()
                        .absolute()
                        .left(px(TURNING_COMET_ORIGIN))
                        .top(px(TURNING_COMET_ORIGIN))
                        .child(
                            svg()
                                .path("icons/comet.svg")
                                .size(px(TURNING_COMET_SIZE))
                                .flex_none()
                                .text_color(gpui::white())
                                .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                    self.angle,
                                ))),
                        ),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .bottom_0()
                        .flex()
                        .gap_2()
                        .children((4..8).map(inked)),
                )
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_turning_comet_samples_only_its_own_atlas_tile() {
        let mut cx = gpui::HeadlessAppContext::with_platform(
            std::sync::Arc::new(gpui_wgpu::CosmicTextSystem::new("Lilex")),
            std::sync::Arc::new(InkedAtlasAssets),
            || {
                let renderer = gpui_wgpu::WgpuHeadlessRenderer::new()?;
                Ok(Some(
                    Box::new(renderer) as Box<dyn gpui::PlatformHeadlessRenderer>
                ))
            },
        );
        let window =
            gpui::HeadlessAppContext::open_window(&mut cx, size(px(200.), px(200.)), |_, cx| {
                cx.new(|_| TurningCometBetweenInkedTiles { angle: 0. })
            })
            .expect("a headless window");
        let background = (TURNING_COMET_BACKGROUND >> 16) as i32;
        let mut strays = Vec::new();
        for degrees in (0..360).step_by(5) {
            cx.update_window(window.into(), |root, _, cx| {
                let root = root
                    .downcast::<TurningCometBetweenInkedTiles>()
                    .expect("the comet root");
                root.update(cx, |root, cx| {
                    root.angle = (degrees as f32).to_radians();
                    cx.notify();
                });
            })
            .expect("an open window");
            cx.run_until_parked();
            let image = cx
                .capture_screenshot(window.into())
                .expect("a headless frame");
            let scale = image.width() as f32 / 200.;
            let centre = (TURNING_COMET_ORIGIN + TURNING_COMET_SIZE / 2.) * scale;
            let ring = TURNING_COMET_SIZE / 2. * scale + 1.;
            let reach = (TURNING_COMET_SIZE * scale) as u32;
            let first = centre as u32 - reach;
            for y in first..first + 2 * reach {
                for x in first..first + 2 * reach {
                    let distance = (x as f32 + 0.5 - centre).hypot(y as f32 + 0.5 - centre);
                    let ink = image.get_pixel(x, y)[0] as i32 - background;
                    if distance > ring && ink.abs() > 6 {
                        strays.push((degrees, x, y, ink));
                    }
                }
            }
        }
        assert!(
            strays.is_empty(),
            "a rotated comet painted outside its ring (degrees, x, y, ink): {strays:?}"
        );
    }
}
