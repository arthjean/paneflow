use gpui::{
    AnyElement, Bounds, ClickEvent, CursorStyle, Div, ElementId, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, SharedString, Stateful,
    StatefulInteractiveElement, Styled, canvas, deferred, div, img, prelude::*, px, rgb, svg,
};

use std::time::Duration;

use crate::ui_primitives::action_menu;
use crate::ui_primitives::interface_style::{
    CLASSIC_CONTROL, SURFACE_ROW, circular, classic, corner, frame_corner, pick, pick_surface,
    with_surface_shape,
};
use crate::ui_primitives::{
    AccessibleControlExt, AnimatedHover, AnimatedHoverExt, ROW_RADIUS, ease_out_cubic,
    highlight_matches, lerp_color, reduce_motion, squircle,
    squircle::{squircle_border, squircle_fill, squircle_fill_blend},
    squircle_skin,
};

pub(crate) const SETTINGS_CONTROL_CORNER_RADIUS: Pixels = px(8.);
const INPUT_FOCUS_RING_WIDTH: f32 = 3.;
const INPUT_FOCUS_DURATION: Duration = Duration::from_millis(160);

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

pub(crate) const KEYCAP_HEIGHT: Pixels = px(20.);
pub(crate) const KEYCAP_RADIUS: Pixels = px(5.);

pub(crate) fn keycap_tones(ui: crate::theme::UiColors) -> (Hsla, Hsla, Hsla) {
    (ui.subtle, lerp_color(ui.subtle, ui.text, 0.08), ui.text)
}

pub(crate) fn keycap(label: impl Into<SharedString>, fill: Hsla, edge: Hsla, ink: Hsla) -> Div {
    div()
        .h(KEYCAP_HEIGHT)
        .min_w(KEYCAP_HEIGHT)
        .px(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(KEYCAP_RADIUS)
        .bg(fill)
        .border_1()
        .border_color(edge)
        .text_size(crate::ui_primitives::LABEL_SM)
        .font_weight(FontWeight::MEDIUM)
        .text_color(ink)
        .child(label.into())
}

pub fn settings_page_color(
    ui: crate::theme::UiColors,
    backdrop: crate::window_chrome::veil::Backdrop,
) -> Hsla {
    if backdrop.is_blended() {
        return crate::pane::pane_card_background(&crate::theme::active_theme(), backdrop, true);
    }
    if ui.surface.l > 0.5 {
        lerp_color(ui.card, ui.text, 0.05)
    } else {
        ui.base
    }
}

pub fn card_color() -> Hsla {
    crate::theme::ui_colors().card
}

const TOGGLE_FOCUS_RADIUS: Pixels = px(11.);
const FOCUS_HALO_ALPHA: f32 = 0.24;
const FOCUS_HALO_SPREAD: Pixels = px(4.);
const FOCUS_EDGE_SPREAD: Pixels = px(1.);
const FOCUS_INSET_WIDTH: Pixels = px(2.);

fn keyboard_focus_halo(accent: Hsla) -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.), px(0.), with_alpha(accent, FOCUS_HALO_ALPHA))
            .spread_radius(FOCUS_HALO_SPREAD),
        gpui::BoxShadow::new(px(0.), px(0.), accent).spread_radius(FOCUS_EDGE_SPREAD),
    ]
}

pub(crate) fn keyboard_focus_inset(accent: Hsla) -> Vec<gpui::BoxShadow> {
    vec![
        gpui::BoxShadow::new(px(0.), px(0.), accent)
            .spread_radius(FOCUS_INSET_WIDTH)
            .inset(),
    ]
}

pub(crate) trait KeyboardStopExt: InteractiveElement + Sized {
    fn keyboard_stop(self, radius: Pixels) -> Self {
        let ring = keyboard_focus_inset(crate::theme::ui_colors().accent);
        self.tab_index(0)
            .focus_visible(move |style| style.rounded(radius).shadow(ring))
    }

    fn opaque_keyboard_stop(self, radius: Pixels) -> Self {
        let ring = keyboard_focus_halo(crate::theme::ui_colors().accent);
        self.tab_index(0)
            .focus_visible(move |style| style.rounded(radius).shadow(ring))
    }
}

impl<E: InteractiveElement> KeyboardStopExt for E {}

pub fn toggle_switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    div()
        .id(id.clone())
        .flex_shrink_0()
        .focusable()
        .accessible_control(gpui::accesskit::Role::Switch, label)
        .keyboard_stop(TOGGLE_FOCUS_RADIUS)
        .aria_toggled(if on {
            gpui::accesskit::Toggled::True
        } else {
            gpui::accesskit::Toggled::False
        })
        .child(toggle_pill((id, "pill").into(), on, ui))
}

const TOGGLE_TRACK_WIDTH: f32 = 36.;
const TOGGLE_TRACK_HEIGHT: f32 = 22.;
const TOGGLE_KNOB_SIZE: f32 = 18.;
const TOGGLE_KNOB_INSET: f32 = 2.;

fn toggle_pill(id: ElementId, on: bool, ui: crate::theme::UiColors) -> impl IntoElement {
    let track_on = switch_blue();
    let track_off = with_alpha(ui.muted, 0.30);
    let travel = TOGGLE_TRACK_WIDTH - TOGGLE_KNOB_SIZE - 2. * TOGGLE_KNOB_INSET;

    let track = div()
        .id(id)
        .relative()
        .w(px(TOGGLE_TRACK_WIDTH))
        .h(px(TOGGLE_TRACK_HEIGHT))
        .rounded_full()
        .animated_toggle(on, move |track, delta| {
            track.style().bg(lerp_color(track_off, track_on, delta));
            track.extend([div()
                .absolute()
                .top(px(TOGGLE_KNOB_INSET))
                .left(px(TOGGLE_KNOB_INSET + travel * delta))
                .size(px(TOGGLE_KNOB_SIZE))
                .rounded_full()
                .bg(gpui::white())
                .into_any_element()]);
        });

    div().flex_shrink_0().child(track)
}

pub fn setting_text(
    ui: crate::theme::UiColors,
    title: &'static str,
    description: &'static str,
) -> impl IntoElement {
    let query = crate::settings::search::active_query();
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(crate::ui_primitives::BODY)
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(highlight_matches(title.to_string(), &query)),
        )
        .child(setting_description(
            ui,
            highlight_matches(description.to_string(), &query),
        ))
}

pub(crate) fn setting_description(ui: crate::theme::UiColors, text: gpui::StyledText) -> gpui::Div {
    div()
        .text_size(crate::ui_primitives::LABEL_SM)
        .text_color(ui.muted)
        .child(text)
}

pub(crate) fn input_surface<E: ParentElement + Styled>(
    element: E,
    ui: crate::theme::UiColors,
) -> E {
    let edge = with_alpha(ui.text, 0.14);
    element
        .relative()
        .child(squircle::squircle_fill_beneath_border(
            SETTINGS_CONTROL_CORNER_RADIUS,
            with_alpha(ui.text, 0.03),
            px(1.),
            edge,
        ))
        .child(squircle_border(
            SETTINGS_CONTROL_CORNER_RADIUS,
            px(1.),
            edge,
        ))
}

pub(crate) fn input_frame<E: ParentElement + Styled>(
    element: E,
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    invalid: bool,
    ui: crate::theme::UiColors,
) -> E {
    let input = input.clone();
    let ring = px(INPUT_FOCUS_RING_WIDTH);
    input_surface(element, ui).child(
        div()
            .absolute()
            .top(-ring)
            .right(-ring)
            .bottom(-ring)
            .left(-ring)
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| {
                        paint_input_focus(bounds, &input, invalid, ui, window, cx);
                    },
                )
                .size_full(),
            ),
    )
}

fn focus_ring_stroke_under_border(width: Pixels, scale_factor: f32) -> Pixels {
    width + squircle::layer_clearance(scale_factor)
}

fn paint_input_focus(
    bounds: Bounds<Pixels>,
    input: &gpui::Entity<crate::widgets::text_input::TextInput>,
    invalid: bool,
    ui: crate::theme::UiColors,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    let input = input.read(cx);
    let Some(since) = input.focused_since() else {
        return;
    };
    if !input.focus_handle.is_focused(window) {
        return;
    }
    let linear = if reduce_motion() {
        1.
    } else {
        (since.elapsed().as_secs_f32() / INPUT_FOCUS_DURATION.as_secs_f32()).min(1.)
    };
    if linear < 1. {
        window.request_animation_frame();
    }
    let progress = ease_out_cubic(linear);
    let (border, ring) = if invalid {
        (
            with_alpha(ui.agent_error, 0.64),
            with_alpha(ui.agent_error, 0.2),
        )
    } else {
        (ui.accent, with_alpha(ui.accent, 0.24))
    };
    let full = px(INPUT_FOCUS_RING_WIDTH);
    let width = full * progress;
    if circular() {
        let frame = bounds.inset(full);
        let radius = corner(
            SETTINGS_CONTROL_CORNER_RADIUS,
            frame.size.width.min(frame.size.height),
        );
        if width > px(0.) {
            window.paint_quad(gpui::quad(
                bounds.inset(full - width),
                radius + width,
                gpui::transparent_black(),
                width,
                ring.opacity(progress),
                gpui::BorderStyle::default(),
            ));
        }
        window.paint_quad(gpui::quad(
            frame,
            radius,
            gpui::transparent_black(),
            px(1.),
            border.opacity(progress),
            gpui::BorderStyle::default(),
        ));
        return;
    }
    if width > px(0.)
        && let Some(path) = squircle::squircle_stroke_path(
            bounds.inset(full - width),
            SETTINGS_CONTROL_CORNER_RADIUS + width,
            focus_ring_stroke_under_border(width, window.scale_factor()),
        )
    {
        window.paint_path(path, ring.opacity(progress));
    }
    if let Some(path) =
        squircle::squircle_stroke_path(bounds.inset(full), SETTINGS_CONTROL_CORNER_RADIUS, px(1.))
    {
        window.paint_path(path, border.opacity(progress));
    }
}

pub fn text_field(
    input: gpui::Entity<crate::widgets::text_input::TextInput>,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    input_frame(
        div()
            .flex_1()
            .min_w(px(180.))
            .max_w(px(320.))
            .px(px(10.))
            .py(px(6.)),
        &input,
        false,
        ui,
    )
    .text_size(px(12.))
    .text_color(ui.text)
    .child(input)
}

pub fn secondary_button(
    id: impl Into<SharedString>,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let hover_bg = lerp_color(ui.subtle, ui.text, CONTROL_HOVER_TOWARD_TEXT);
    let id = id.into();

    squircle_skin(
        div()
            .id(id.clone())
            .px(px(10.))
            .py(px(4.))
            .cursor(CursorStyle::PointingHand)
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(ui.text),
        format!("{id}-squircle"),
        ROW_RADIUS,
        Some(ui.subtle),
        Some(hover_bg),
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .keyboard_stop(ROW_RADIUS)
    .child(label)
    .on_click(on_click)
}

pub fn raised_button(
    id: impl Into<SharedString>,
    label: &'static str,
    fill: Hsla,
    ink: Hsla,
) -> AnimatedHover {
    let id: SharedString = id.into();
    raised_skin(
        div()
            .id(id.clone())
            .px(px(10.))
            .py(px(4.))
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(ink),
        format!("{id}-squircle"),
        ROW_RADIUS,
        fill,
        true,
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .opaque_keyboard_stop(ROW_RADIUS)
    .child(label)
}

pub(crate) fn inverse_fill(ui: crate::theme::UiColors) -> (Hsla, Hsla) {
    if ui.surface.l > 0.5 {
        (gpui::black(), gpui::white())
    } else {
        (gpui::white(), gpui::black())
    }
}

const RAISED_BUTTON_HEIGHT: Pixels = px(24.);

pub(crate) fn raised_skin(
    element: Stateful<Div>,
    group: impl Into<SharedString>,
    radius: Pixels,
    fill: Hsla,
    enabled: bool,
) -> AnimatedHover {
    let group: SharedString = group.into();
    let element = element
        .relative()
        .rounded(corner(radius, RAISED_BUTTON_HEIGHT));
    if !enabled {
        return element
            .opacity(0.64)
            .child(raised_layer(
                radius,
                fill,
                lerp_color(fill, gpui::white(), 0.16),
            ))
            .animated_hover(|_, _| {});
    }
    let hovered = lerp_color(fill, card_color(), 0.1);
    let lift = gpui::BoxShadow::new(px(0.), px(1.), fill.opacity(0.24)).blur_radius(px(2.));
    let progress = std::rc::Rc::new(std::cell::Cell::new(0.));
    element
        .group(group.clone())
        .shadow(vec![lift])
        .active(|style| style.shadow(Vec::new()))
        .cursor(CursorStyle::PointingHand)
        .child(
            div()
                .absolute()
                .inset_0()
                .child(squircle_fill_blend(
                    radius,
                    lerp_color(fill, gpui::white(), 0.16),
                    lerp_color(hovered, gpui::white(), 0.16),
                    progress.clone(),
                ))
                .child(
                    div()
                        .absolute()
                        .top(px(1.))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .child(squircle_fill_blend(radius, fill, hovered, progress.clone())),
                ),
        )
        .child(pressed_layer(
            &group,
            raised_layer(radius, hovered, lerp_color(hovered, gpui::black(), 0.08)),
        ))
        .animated_hover(move |_, delta| progress.set(delta))
}

fn raised_layer(radius: Pixels, fill: Hsla, top_edge: Hsla) -> Div {
    div()
        .absolute()
        .inset_0()
        .child(squircle_fill(radius, top_edge))
        .child(
            div()
                .absolute()
                .top(px(1.))
                .left_0()
                .right_0()
                .bottom_0()
                .child(squircle_fill(radius, fill)),
        )
}

fn pressed_layer(group: &SharedString, layer: impl IntoElement) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("{group}-pressed")))
        .absolute()
        .inset_0()
        .opacity(0.)
        .group_active(group.clone(), |style| style.opacity(1.))
        .child(layer)
}

pub(crate) const CONTROL_HOVER_TOWARD_TEXT: f32 = 0.06;

pub fn destructive_color() -> Hsla {
    Hsla::from(gpui::rgb(0xff453a))
}

pub fn destructive_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    solid_button(id, label, destructive_color())
}

pub fn solid_button(
    id: &'static str,
    label: &'static str,
    resting: Hsla,
) -> gpui::Stateful<gpui::Div> {
    let hovered = Hsla {
        l: (resting.l - 0.05).max(0.0),
        ..resting
    };

    squircle_skin(
        div()
            .id(id)
            .px(px(10.))
            .py(px(4.))
            .cursor(CursorStyle::PointingHand)
            .text_size(px(12.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(gpui::white()),
        format!("{id}-squircle"),
        ROW_RADIUS,
        Some(resting),
        Some(hovered),
    )
    .accessible_control(gpui::accesskit::Role::Button, label)
    .opaque_keyboard_stop(ROW_RADIUS)
    .child(label)
}

pub type Logo = (&'static str, bool);

pub fn render_logo(
    path: impl Into<SharedString>,
    multicolor: bool,
    size: Pixels,
    tint: Hsla,
) -> AnyElement {
    let path = path.into();
    if multicolor {
        img(path).size(size).flex_none().into_any_element()
    } else {
        svg()
            .size(size)
            .flex_none()
            .path(path)
            .text_color(tint)
            .into_any_element()
    }
}

pub(crate) const MODAL_PADDING: Pixels = px(20.);

pub(crate) enum ModalKey {
    Dismiss,
    Confirm,
}

pub(crate) fn modal_key(event: &gpui::KeyDownEvent) -> Option<ModalKey> {
    match event.keystroke.key.as_str() {
        "escape" => Some(ModalKey::Dismiss),
        "enter" => Some(ModalKey::Confirm),
        _ => None,
    }
}

pub(crate) fn modal_backdrop(
    id: impl Into<ElementId>,
    child: impl IntoElement,
    on_dismiss: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let outline = crate::window_chrome::csd::active_window_outline();
    deferred(
        div()
            .id(id)
            .absolute()
            .top(-outline.border.top)
            .right(-outline.border.right)
            .bottom(-outline.border.bottom)
            .left(-outline.border.left)
            .rounded_tl(outline.radii.top_left)
            .rounded_tr(outline.radii.top_right)
            .rounded_br(outline.radii.bottom_right)
            .rounded_bl(outline.radii.bottom_left)
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::hsla(0., 0., 0., 0.55))
            .on_mouse_down(gpui::MouseButton::Left, on_dismiss)
            .child(child),
    )
    .with_priority(10)
    .into_any_element()
}

pub(crate) fn modal_card(
    id: impl Into<ElementId>,
    width: Pixels,
    radius: Pixels,
    ui: crate::theme::UiColors,
    content: Div,
) -> Stateful<Div> {
    let framed = radius.min(crate::app::constants::SETTINGS_CARD_RADIUS);
    let radius = pick(radius, framed);
    div()
        .id(id)
        .occlude()
        .relative()
        .w(width)
        .rounded(frame_corner(radius))
        .shadow_lg()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(squircle::squircle_fill_beneath_border(
            radius,
            card_color(),
            px(1.),
            surface_edge(ui),
        ))
        .child(content.relative().flex().flex_col())
        .child(squircle::squircle_border(radius, px(1.), surface_edge(ui)))
}

pub(crate) fn modal_header(
    ui: crate::theme::UiColors,
    title: impl Into<SharedString>,
    summary: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(MODAL_PADDING)
        .pt(px(16.))
        .pb(px(12.))
        .child(
            div()
                .text_size(crate::ui_primitives::TITLE)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child(title.into()),
        )
        .child(
            div()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .child(summary.into()),
        )
}

pub(crate) const CONFIRMATION_TITLE_MAX_SHARE: f32 = 0.6;

fn confirmation_row(ui: crate::theme::UiColors, title: SharedString, detail: SharedString) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .child(
            div()
                .flex_none()
                .min_w_0()
                .max_w(gpui::relative(CONFIRMATION_TITLE_MAX_SHARE))
                .truncate()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .debug_selector(|| "confirmation-title".into())
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_right()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .debug_selector(|| "confirmation-detail".into())
                .child(detail),
        )
}

pub(crate) fn confirmation_list(
    ui: crate::theme::UiColors,
    rows: impl IntoIterator<Item = (SharedString, SharedString)>,
    max_listed: usize,
) -> Div {
    let mut rows = rows.into_iter().peekable();
    if rows.peek().is_none() {
        return div();
    }
    let mut list = div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .mx(MODAL_PADDING)
        .px(px(12.))
        .py(px(10.))
        .rounded(px(8.))
        .bg(with_alpha(ui.subtle, 0.5));
    let mut hidden = 0usize;
    for (index, (title, detail)) in rows.enumerate() {
        if index >= max_listed {
            hidden += 1;
            continue;
        }
        list = list.child(confirmation_row(ui, title, detail));
    }
    if hidden > 0 {
        list = list.child(
            div()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .child(format!("and {hidden} more")),
        );
    }
    list
}

pub(crate) fn confirmation_warning(
    ui: crate::theme::UiColors,
    text: impl Into<SharedString>,
) -> Div {
    div()
        .px(MODAL_PADDING)
        .pt(px(12.))
        .pb(px(2.))
        .text_size(crate::ui_primitives::BODY)
        .line_height(px(18.))
        .text_color(ui.muted)
        .child(text.into())
}

pub(crate) fn modal_footer() -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .px(MODAL_PADDING)
        .pt(px(18.))
        .pb(px(16.))
}

pub(crate) const SELECT_CHEVRON_SIZE: Pixels = px(12.);
pub(crate) const SELECT_CHEVRON_ASSET: &str = "icons/settings/unfold-more.svg";

pub fn select_chevron(ui: crate::theme::UiColors) -> impl IntoElement {
    svg()
        .size(SELECT_CHEVRON_SIZE)
        .flex_none()
        .path(SELECT_CHEVRON_ASSET)
        .text_color(with_alpha(ui.text, 0.8))
}

pub fn select_trigger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    expanded: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    let light_theme = ui.surface.l > 0.5;
    let fill = if light_theme {
        ui.card
    } else {
        with_alpha(ui.border, 0.32)
    };
    let lift =
        gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.05)).blur_radius(px(2.));
    let edge = if light_theme {
        gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.04))
    } else {
        gpui::BoxShadow::new(px(0.), px(-1.), gpui::white().opacity(0.06))
    };
    div()
        .id(id.into())
        .accessible_control(gpui::accesskit::Role::ComboBox, label)
        .keyboard_stop(SETTINGS_CONTROL_CORNER_RADIUS)
        .aria_expanded(expanded)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .px(px(9.))
        .py(px(5.))
        .min_w(px(190.))
        .max_w(px(260.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .border(SELECT_TRIGGER_BORDER)
        .border_color(ui.border)
        .bg(fill)
        .shadow(if expanded {
            Vec::new()
        } else {
            vec![lift, edge]
        })
        .active(|style| style.shadow(Vec::new()))
}

pub fn select_menu_surface(ui: crate::theme::UiColors) -> Hsla {
    if ui.surface.l > 0.5 {
        ui.overlay
    } else {
        Hsla {
            l: (ui.surface.l + 0.035).min(1.0),
            ..ui.surface
        }
    }
}

pub fn menu_divider_color(ui: crate::theme::UiColors) -> Hsla {
    with_alpha(ui.text, action_menu::SEPARATOR_ALPHA)
}

pub(crate) const MENU_RADIUS: Pixels = px(18.);

pub fn menu_surface<E: Styled + ParentElement>(el: E, ui: crate::theme::UiColors) -> E {
    menu_surface_filled(el, select_menu_surface(ui), ui)
}

pub fn menu_surface_filled<E: Styled + ParentElement>(
    el: E,
    fill: Hsla,
    ui: crate::theme::UiColors,
) -> E {
    with_surface_shape(|| {
        el.relative()
            .child(squircle::squircle_fill_beneath_border(
                MENU_RADIUS,
                fill,
                px(1.),
                surface_edge(ui),
            ))
            .child(squircle::squircle_border(
                MENU_RADIUS,
                px(1.),
                surface_edge(ui),
            ))
    })
}

pub(crate) const MENU_MAX_HEIGHT: Pixels = px(400.);

const CHROME_EDGE_ALPHA: f32 = 0.6;
const SURFACE_EDGE_ALPHA: f32 = 0.12;
const CLASSIC_EDGE_ALPHA: f32 = 0.07;

pub(crate) fn menu_padding() -> Pixels {
    action_menu::PANEL_PADDING
}

pub(crate) fn menu_item_radius() -> Pixels {
    pick_surface(ROW_RADIUS, SURFACE_ROW, CLASSIC_CONTROL)
}

const MENU_TEXT: Pixels = px(12.);
const SURFACE_MENU_TEXT: Pixels = px(13.);
const CLASSIC_MENU_TEXT: Pixels = px(13.);

pub(crate) fn menu_text_size() -> Pixels {
    pick_surface(MENU_TEXT, SURFACE_MENU_TEXT, CLASSIC_MENU_TEXT)
}

pub(crate) fn menu_text_weight() -> FontWeight {
    pick(FontWeight::NORMAL, FontWeight::MEDIUM)
}

pub(crate) fn menu_row_inset() -> Pixels {
    action_menu::ROW_INSET
}

pub(crate) fn menu_divider_inset(_resting: Pixels) -> Pixels {
    action_menu::ROW_INSET
}

pub(crate) fn surface_edge(ui: crate::theme::UiColors) -> Hsla {
    pick_surface(
        with_alpha(ui.border, CHROME_EDGE_ALPHA),
        with_alpha(ui.text, SURFACE_EDGE_ALPHA),
        with_alpha(ui.text, CLASSIC_EDGE_ALPHA),
    )
}

pub(crate) fn floating_shadow() -> Vec<gpui::BoxShadow> {
    if classic() {
        return vec![
            gpui::BoxShadow::new(px(0.), px(4.), gpui::black().opacity(0.16)).blur_radius(px(16.)),
            gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.12)).blur_radius(px(4.)),
        ];
    }
    vec![
        gpui::BoxShadow::new(px(0.), px(6.), gpui::black().opacity(0.10)).blur_radius(px(20.)),
        gpui::BoxShadow::new(px(0.), px(1.), gpui::black().opacity(0.10)).blur_radius(px(3.)),
    ]
}

pub fn menu_panel<E: Styled + ParentElement>(el: E, ui: crate::theme::UiColors) -> E {
    el.relative()
        .flex()
        .flex_col()
        .p(action_menu::PANEL_PADDING)
        .rounded(action_menu::PANEL_RADIUS)
        .border(action_menu::PANEL_BORDER)
        .border_color(with_alpha(ui.text, action_menu::EDGE_ALPHA))
        .bg(select_menu_surface(ui))
        .font_family(action_menu::FONT_FAMILY)
}

pub fn menu_row(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    with_surface_shape(|| {
        select_item_shaped(
            id,
            selected,
            ui,
            with_alpha(ui.text, action_menu::HIGHLIGHT_ALPHA),
            action_menu::ROW_RADIUS,
        )
        .h(action_menu::ROW_HEIGHT)
        .text_size(action_menu::LABEL_TEXT)
        .cursor(CursorStyle::Arrow)
    })
}

pub fn select_menu(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> SelectMenu {
    with_surface_shape(|| styled_select_menu(id.into(), ui))
}

fn styled_select_menu(id: ElementId, ui: crate::theme::UiColors) -> SelectMenu {
    let list_id: ElementId = (id.clone(), "list").into();
    let reveal_id: ElementId = (id.clone(), "reveal").into();
    SelectMenu {
        state_id: (id.clone(), "keyboard").into(),
        reveal_id,
        highlight: with_alpha(ui.text, action_menu::HIGHLIGHT_ALPHA),
        shell: div()
            .id(id)
            .relative()
            .flex()
            .flex_col()
            .min_w(px(200.))
            .max_w(px(280.))
            .max_h(MENU_MAX_HEIGHT)
            .rounded(action_menu::PANEL_RADIUS)
            .border(action_menu::PANEL_BORDER)
            .border_color(with_alpha(ui.text, action_menu::EDGE_ALPHA))
            .bg(select_menu_surface(ui))
            .shadow(action_menu::panel_shadow(ui))
            .font_family(action_menu::FONT_FAMILY)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        list: div()
            .id(list_id)
            .flex()
            .flex_col()
            .p(action_menu::PANEL_PADDING)
            .min_h_0()
            .overflow_y_scroll(),
        entries: Vec::new(),
    }
}

pub fn select_option(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    with_surface_shape(|| styled_select_option(id.into(), selected, ui))
}

fn styled_select_option(
    id: ElementId,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    squircle_skin(
        div()
            .id(id.clone())
            .flex_none()
            .h(action_menu::ROW_HEIGHT)
            .pl(action_menu::ROW_INSET)
            .pr(px(32.))
            .flex()
            .flex_row()
            .items_center()
            .gap(action_menu::ROW_GAP)
            .cursor(CursorStyle::Arrow)
            .text_size(action_menu::LABEL_TEXT)
            .font_weight(FontWeight::NORMAL),
        format!("{id}-squircle"),
        action_menu::ROW_RADIUS,
        None,
        Some(with_alpha(ui.text, action_menu::HIGHLIGHT_ALPHA)),
    )
    .when(selected, |row| {
        row.child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .right(action_menu::ROW_INSET)
                .flex()
                .items_center()
                .child(
                    svg()
                        .size(action_menu::CHECK_SIZE)
                        .path("icons/menu/check.svg")
                        .text_color(ui.text),
                ),
        )
    })
}

type SelectHandler = std::rc::Rc<dyn Fn(&(), &mut gpui::Window, &mut gpui::App)>;

enum SelectEntry {
    Element(AnyElement),
    Option(Stateful<Div>, SelectHandler),
}

#[derive(IntoElement)]
pub struct SelectMenu {
    state_id: ElementId,
    reveal_id: ElementId,
    highlight: Hsla,
    shell: Stateful<Div>,
    list: Stateful<Div>,
    entries: Vec<SelectEntry>,
}

impl SelectMenu {
    pub fn option(
        mut self,
        row: Stateful<Div>,
        on_select: impl Fn(&(), &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> Self {
        self.entries
            .push(SelectEntry::Option(row, std::rc::Rc::new(on_select)));
        self
    }
}

impl Styled for SelectMenu {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.shell.style()
    }
}

impl InteractiveElement for SelectMenu {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.shell.interactivity()
    }
}

impl StatefulInteractiveElement for SelectMenu {}

impl ParentElement for SelectMenu {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.entries
            .extend(elements.into_iter().map(SelectEntry::Element));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectKey {
    Highlight(usize),
    Choose(usize),
    Ignored,
}

pub(crate) fn select_key(key: &str, highlighted: Option<usize>, options: usize) -> SelectKey {
    if options == 0 {
        return SelectKey::Ignored;
    }
    let last = options - 1;
    let current = highlighted.filter(|index| *index <= last);
    match key {
        "down" => SelectKey::Highlight(current.map_or(0, |index| (index + 1) % options)),
        "up" => SelectKey::Highlight(current.map_or(last, |index| (index + last) % options)),
        "home" => SelectKey::Highlight(0),
        "end" => SelectKey::Highlight(last),
        "enter" | "space" => current.map_or(SelectKey::Ignored, SelectKey::Choose),
        _ => SelectKey::Ignored,
    }
}

struct SelectKeyboard {
    focus: gpui::FocusHandle,
    restore: Option<gpui::FocusHandle>,
    highlighted: Option<usize>,
    scroll: gpui::ScrollHandle,
}

impl SelectKeyboard {
    fn open(window: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let restore = window.focused(cx);
        let target = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&target, cx));
        Self {
            focus,
            restore,
            highlighted: None,
            scroll: gpui::ScrollHandle::new(),
        }
    }
}

fn restore_select_focus(
    state: &gpui::Entity<SelectKeyboard>,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) {
    if let Some(handle) = state.read(cx).restore.clone() {
        window.focus(&handle, cx);
    }
}

impl RenderOnce for SelectMenu {
    fn render(self, window: &mut gpui::Window, cx: &mut gpui::App) -> impl IntoElement {
        let SelectMenu {
            state_id,
            highlight,
            shell,
            list,
            entries,
            ..
        } = self;
        if !entries
            .iter()
            .any(|entry| matches!(entry, SelectEntry::Option(..)))
        {
            return shell
                .child(
                    list.children(entries.into_iter().filter_map(|entry| match entry {
                        SelectEntry::Element(element) => Some(element),
                        SelectEntry::Option(..) => None,
                    })),
                )
                .into_any_element();
        }

        let state = window.use_keyed_state(state_id, cx, SelectKeyboard::open);
        let (focus, highlighted, scroll) = {
            let state = state.read(cx);
            (state.focus.clone(), state.highlighted, state.scroll.clone())
        };
        let mut handlers: Vec<SelectHandler> = Vec::new();
        let mut option_children: Vec<usize> = Vec::new();
        let mut children: Vec<AnyElement> = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry {
                SelectEntry::Element(element) => children.push(element),
                SelectEntry::Option(row, handler) => {
                    let index = handlers.len();
                    let click_state = state.clone();
                    let click_handler = handler.clone();
                    option_children.push(children.len());
                    handlers.push(handler);
                    children.push(
                        row.when(highlighted == Some(index), |row| {
                            row.bg(highlight).rounded(action_menu::ROW_RADIUS)
                        })
                        .on_click(move |_, window, cx| {
                            restore_select_focus(&click_state, window, cx);
                            click_handler(&(), window, cx);
                        })
                        .into_any_element(),
                    );
                }
            }
        }

        let key_state = state.clone();
        let move_state = state.clone();
        let out_state = state;
        shell
            .track_focus(&focus)
            .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                let modifiers = event.keystroke.modifiers;
                if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
                    return;
                }
                let key = event.keystroke.key.as_str();
                let current = key_state.read(cx).highlighted;
                match select_key(key, current, handlers.len()) {
                    SelectKey::Highlight(index) => {
                        let child = option_children[index];
                        key_state.update(cx, |state, cx| {
                            state.highlighted = Some(index);
                            state.scroll.scroll_to_item(child);
                            cx.notify();
                        });
                        cx.stop_propagation();
                    }
                    SelectKey::Choose(index) => {
                        restore_select_focus(&key_state, window, cx);
                        handlers[index](&(), window, cx);
                        cx.stop_propagation();
                    }
                    SelectKey::Ignored => {
                        if key == "escape" {
                            restore_select_focus(&key_state, window, cx);
                        }
                    }
                }
            })
            .on_mouse_move(move |_, _, cx| {
                if move_state.read(cx).highlighted.is_some() {
                    move_state.update(cx, |state, cx| {
                        state.highlighted = None;
                        cx.notify();
                    });
                }
            })
            .on_mouse_down_out(move |_, window, cx| restore_select_focus(&out_state, window, cx))
            .child(list.track_scroll(&scroll).children(children))
            .into_any_element()
    }
}

pub fn select_item(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    select_item_tinted(id, selected, ui, with_alpha(ui.text, 0.10))
}

pub fn select_item_tinted(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
    selected_bg: Hsla,
) -> Stateful<Div> {
    select_item_shaped(id, selected, ui, selected_bg, menu_item_radius())
}

pub fn select_item_shaped(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
    selected_bg: Hsla,
    radius: Pixels,
) -> Stateful<Div> {
    let resting_bg = if selected {
        selected_bg
    } else {
        with_alpha(ui.text, 0.0)
    };
    let hover_bg = if selected {
        selected_bg
    } else {
        with_alpha(ui.text, 0.05)
    };

    let id: ElementId = id.into();
    let group = SharedString::from(format!("{id}-squircle"));
    squircle_skin(
        div()
            .id(id)
            .flex_none()
            .h(px(28.))
            .px(menu_row_inset())
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor(CursorStyle::PointingHand)
            .text_size(menu_text_size())
            .font_weight(menu_text_weight()),
        group,
        radius,
        (resting_bg.a > f32::EPSILON).then_some(resting_bg),
        (hover_bg.a > f32::EPSILON).then_some(hover_bg),
    )
}

pub(crate) const SELECT_TRIGGER_BORDER: Pixels = px(1.);
pub(crate) const SELECT_MENU_GAP: Pixels = px(4.);

pub fn deferred_select_menu(menu: SelectMenu) -> AnyElement {
    deferred_select_menu_at(menu, 1)
}

pub(crate) fn deferred_select_menu_at(menu: SelectMenu, priority: usize) -> AnyElement {
    let reveal_id = menu.reveal_id.clone();
    let trigger: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>> = std::rc::Rc::default();
    let probe = trigger.clone();
    div()
        .absolute()
        .top(-SELECT_TRIGGER_BORDER)
        .left(-SELECT_TRIGGER_BORDER)
        .right(-SELECT_TRIGGER_BORDER)
        .bottom(-SELECT_TRIGGER_BORDER)
        .child(
            canvas(move |bounds, _, _| probe.set(Some(bounds)), |_, _, _, _| {})
                .absolute()
                .size_full(),
        )
        .child(
            deferred(AttachedBelowTrigger {
                trigger,
                popup: Some(crate::ui_primitives::menu_reveal(
                    reveal_id,
                    div().occlude().child(menu),
                )),
                placed: None,
            })
            .with_priority(priority),
        )
        .into_any_element()
}

struct AttachedBelowTrigger {
    trigger: std::rc::Rc<std::cell::Cell<Option<Bounds<Pixels>>>>,
    popup: Option<AnyElement>,
    placed: Option<AnyElement>,
}

impl gpui::Element for AttachedBelowTrigger {
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
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        let style = gpui::Style {
            position: gpui::Position::Absolute,
            ..gpui::Style::default()
        };
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        let (Some(trigger), Some(popup)) = (self.trigger.get(), self.popup.take()) else {
            return;
        };
        let mut placed = gpui::anchored()
            .position_mode(gpui::AnchoredPositionMode::Window)
            .attach(trigger, gpui::Anchor::BottomRight)
            .anchor(gpui::Anchor::TopRight)
            .offset(gpui::point(px(0.), SELECT_MENU_GAP))
            .child(popup)
            .into_any_element();
        placed.layout_as_root(gpui::AvailableSpace::min_size(), window, cx);
        placed.prepaint_at(bounds.origin, window, cx);
        self.placed = Some(placed);
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        if let Some(placed) = self.placed.as_mut() {
            placed.paint(window, cx);
        }
    }
}

pub(crate) fn switch_blue() -> Hsla {
    Hsla::from(rgb(0x339cff))
}

pub(crate) const PAGE_TITLE_SIZE: f32 = 26.;

pub(crate) fn settings_breadcrumb(
    id: &'static str,
    parent: &'static str,
    back_label: &'static str,
    name: impl Into<SharedString>,
    ui: crate::theme::UiColors,
    on_back: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .child(
            div()
                .id(id)
                .flex_none()
                .cursor(CursorStyle::PointingHand)
                .text_size(px(PAGE_TITLE_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.muted)
                .hover(|style| style.text_color(ui.text))
                .role(gpui::accesskit::Role::Button)
                .aria_label(back_label)
                .keyboard_stop(px(6.))
                .on_click(on_back)
                .child(parent),
        )
        .child(
            svg()
                .size(px(18.))
                .flex_none()
                .path("icons/settings/arrow-right-01.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_size(px(PAGE_TITLE_SIZE))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child(name.into()),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_rows_nest_concentrically_inside_their_menu_surface() {
        use crate::ui_primitives::action_menu::{PANEL_PADDING, PANEL_RADIUS, ROW_RADIUS};
        use crate::ui_primitives::interface_style::{Shape, scoped};

        for shape in [Shape::Rounded, Shape::Surface, Shape::Classic] {
            scoped(shape, || {
                assert_eq!((action_menu::ROW_HEIGHT, menu_padding()), (px(28.), px(4.)));
                assert_eq!(PANEL_PADDING + ROW_RADIUS, PANEL_RADIUS);
            });
        }
        with_surface_shape(|| {
            assert_eq!(
                frame_corner(crate::app::constants::SETTINGS_CARD_RADIUS),
                px(12.)
            );
        });
    }

    #[test]
    fn menu_rows_carry_the_label_and_divider_metrics_of_every_shape() {
        use crate::ui_primitives::interface_style::{Shape, scoped};

        for shape in [Shape::Rounded, Shape::Surface, Shape::Classic] {
            scoped(shape, || {
                assert_eq!(menu_row_inset(), px(8.));
                assert_eq!(menu_divider_inset(px(6.)), menu_row_inset());
            });
        }
        assert_eq!(menu_text_size(), px(12.));
        assert_eq!(menu_text_weight(), FontWeight::NORMAL);
        scoped(Shape::Classic, || {
            assert_eq!(menu_text_size(), px(13.));
            assert_eq!(menu_text_weight(), FontWeight::MEDIUM);
        });
    }

    #[test]
    fn select_keys_walk_the_options_and_wrap() {
        assert_eq!(select_key("down", None, 3), SelectKey::Highlight(0));
        assert_eq!(select_key("down", Some(0), 3), SelectKey::Highlight(1));
        assert_eq!(select_key("down", Some(2), 3), SelectKey::Highlight(0));
        assert_eq!(select_key("up", None, 3), SelectKey::Highlight(2));
        assert_eq!(select_key("up", Some(0), 3), SelectKey::Highlight(2));
        assert_eq!(select_key("home", Some(2), 3), SelectKey::Highlight(0));
        assert_eq!(select_key("end", None, 3), SelectKey::Highlight(2));
        assert_eq!(select_key("enter", Some(1), 3), SelectKey::Choose(1));
        assert_eq!(select_key("space", Some(2), 3), SelectKey::Choose(2));
        assert_eq!(select_key("enter", None, 3), SelectKey::Ignored);
        assert_eq!(select_key("escape", Some(1), 3), SelectKey::Ignored);
        assert_eq!(select_key("down", None, 0), SelectKey::Ignored);
    }

    struct SelectHarness {
        trigger_top: f32,
        picks: std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
    }

    impl gpui::Render for SelectHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors();
            let mut menu =
                select_menu("harness-select", ui).debug_selector(|| "select-menu".into());
            for index in 0..3usize {
                let picks = self.picks.clone();
                menu = menu.option(
                    select_option(("harness-option", index), index == 0, ui)
                        .child(format!("Option {index}")),
                    move |_, _, _| picks.borrow_mut().push(index),
                );
            }
            div().size_full().relative().child(
                div()
                    .absolute()
                    .top(px(self.trigger_top))
                    .right(px(20.))
                    .child(
                        select_trigger("harness-trigger", "Harness", true, ui)
                            .debug_selector(|| "select-trigger".into())
                            .child("Option 0")
                            .child(deferred_select_menu(menu)),
                    ),
            )
        }
    }

    fn open_select(
        cx: &mut gpui::TestAppContext,
        trigger_top: f32,
    ) -> (
        &mut gpui::VisualTestContext,
        std::rc::Rc<std::cell::RefCell<Vec<usize>>>,
    ) {
        let picks = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let held = picks.clone();
        let (_view, cx) = cx.add_window_view(move |_, _| SelectHarness {
            trigger_top,
            picks: held,
        });
        cx.simulate_resize(gpui::size(px(600.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (cx, picks)
    }

    #[gpui::test]
    fn a_select_popup_opens_under_its_trigger_when_there_is_room(cx: &mut gpui::TestAppContext) {
        let (cx, _) = open_select(cx, 20.);
        let trigger = cx.debug_bounds("select-trigger").expect("trigger painted");
        let menu = cx.debug_bounds("select-menu").expect("menu painted");
        assert!(
            menu.top() >= trigger.bottom(),
            "the menu at {menu:?} starts under the trigger at {trigger:?}"
        );
        assert!((menu.right() - trigger.right()).abs() <= px(0.5));
    }

    #[gpui::test]
    fn a_select_popup_near_the_window_bottom_opens_above_its_trigger(
        cx: &mut gpui::TestAppContext,
    ) {
        let (cx, _) = open_select(cx, 352.);
        let trigger = cx.debug_bounds("select-trigger").expect("trigger painted");
        let menu = cx.debug_bounds("select-menu").expect("menu painted");
        assert!(
            menu.bottom() <= px(400.),
            "the menu at {menu:?} runs past the 400 px window"
        );
        assert!(
            menu.bottom() <= trigger.top(),
            "the menu at {menu:?} covers the trigger at {trigger:?}"
        );
    }

    #[gpui::test]
    fn a_select_popup_answers_the_arrow_keys_and_enter(cx: &mut gpui::TestAppContext) {
        let (cx, picks) = open_select(cx, 20.);
        cx.simulate_keystrokes("down down enter");
        assert_eq!(*picks.borrow(), vec![1]);
        cx.simulate_keystrokes("up enter");
        assert_eq!(*picks.borrow(), vec![1, 0]);
    }

    #[test]
    fn the_select_chevron_draws_two_solid_chevrons_with_a_clear_row_between_at_its_size() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(SELECT_CHEVRON_ASSET);
        let data = std::fs::read(&path).expect("chevron asset");
        let tree = resvg::usvg::Tree::from_data(&data, &Default::default()).expect("valid svg");
        let side = f32::from(SELECT_CHEVRON_SIZE) as u32;
        let oversample = 2;
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(side * oversample, side * oversample).expect("pixmap");
        let scale = (side * oversample) as f32 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        let device_pixel = |x: u32, y: u32| -> u32 {
            let mut sum = 0;
            for dy in 0..oversample {
                for dx in 0..oversample {
                    sum += pixmap
                        .pixel(x * oversample + dx, y * oversample + dy)
                        .map_or(0, |pixel| u32::from(pixel.alpha()));
                }
            }
            sum / (oversample * oversample)
        };
        let peaks: Vec<u32> = (0..side)
            .map(|y| (0..side).map(|x| device_pixel(x, y)).max().unwrap_or(0))
            .collect();
        let (upper, lower) = peaks.split_at(side as usize / 2);
        let solid = 230;
        assert!(
            upper.iter().any(|alpha| *alpha >= solid) && lower.iter().any(|alpha| *alpha >= solid),
            "each chevron reaches full ink at {side} px, row peaks {peaks:?}"
        );
        let first = peaks.iter().position(|alpha| *alpha >= solid).unwrap_or(0);
        let last = peaks.iter().rposition(|alpha| *alpha >= solid).unwrap_or(0);
        assert!(
            peaks[first..=last].iter().any(|alpha| *alpha < 24),
            "the two chevrons are kept apart by a clear row, row peaks {peaks:?}"
        );
    }

    struct ConfirmationHarness;

    impl gpui::Render for ConfirmationHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ui = crate::theme::ui_colors();
            div().w(px(460.)).debug_selector(|| "card".into()).child(confirmation_list(
                ui,
                [(
                    SharedString::from("masks.rs"),
                    SharedString::from(
                        "/home/user/dev/paneflow/src-app/src/terminal/element/paint/a/very/deeply/nested/folder",
                    ),
                )],
                8,
            ))
        }
    }

    struct EmptyConfirmationHarness;

    impl gpui::Render for EmptyConfirmationHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let rows: [(SharedString, SharedString); 0] = [];
            div().flex().flex_col().child(
                div()
                    .w(px(460.))
                    .debug_selector(|| "card".into())
                    .child(confirmation_list(crate::theme::ui_colors(), rows, 8)),
            )
        }
    }

    #[gpui::test]
    fn an_empty_confirmation_list_draws_no_band(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| EmptyConfirmationHarness);
        cx.simulate_resize(gpui::size(px(800.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let card = cx.debug_bounds("card").expect("card painted");
        assert_eq!(
            card.size.height,
            px(0.),
            "a list with nothing to list takes no room and paints no band"
        );
    }

    #[gpui::test]
    fn a_long_folder_wraps_inside_the_card_and_leaves_the_file_name_visible(
        cx: &mut gpui::TestAppContext,
    ) {
        let (_view, cx) = cx.add_window_view(|_, _| ConfirmationHarness);
        cx.simulate_resize(gpui::size(px(800.), px(400.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let card = cx.debug_bounds("card").expect("card painted");
        let title = cx
            .debug_bounds("confirmation-title")
            .expect("title painted");
        let detail = cx
            .debug_bounds("confirmation-detail")
            .expect("detail painted");
        assert!(
            detail.right() <= card.right(),
            "the folder ends at {:?}, past the card edge {:?}",
            detail.right(),
            card.right()
        );
        assert!(
            title.size.width > px(30.),
            "the file name keeps its width, got {:?}",
            title.size.width
        );
        assert!(
            detail.size.height > title.size.height,
            "the folder wraps onto more lines than the file name, {:?} vs {:?}",
            detail.size.height,
            title.size.height
        );
    }
}
