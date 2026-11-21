use gpui::{
    AnyElement, Div, ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Stateful, StatefulInteractiveElement, Styled, div, prelude::*, px, rgb, svg,
};

use crate::agent_launcher::TerminalAgent;
use crate::settings::components::{
    keyboard_focus_inset, render_logo, select_menu_surface, switch_blue, with_alpha,
};
use crate::ui_primitives::{AccessibleControlExt, AnimatedHoverExt, highlight_matches, lerp_color};

pub(crate) const TEXT_INSET: f32 = 17.;
pub(crate) const CARD_GAP: f32 = 16.;
pub(crate) const SECTION_GAP: f32 = 32.;
pub(crate) const LEAD: f32 = 32.;
pub(crate) const DIALOG_WIDTH: f32 = 460.;

const ROW_PADDING_X: f32 = 16.;
const ROW_PADDING_Y: f32 = 12.;
const ROW_GAP: f32 = 16.;
const LEAD_GAP: f32 = 12.;
pub(crate) const ROW_INSET: f32 = ROW_PADDING_X;
pub(crate) const COLUMN_INSET: f32 = ROW_PADDING_X + LEAD + LEAD_GAP;
const HAIR: f32 = 2.;
const MIN_TEXT: f32 = 180.;
const CONTROL_MAX: f32 = 240.;
pub(crate) const CARD_RADIUS: f32 = 12.;
pub(crate) const CARD_BORDER: f32 = 1.;
const CHEVRON: f32 = 14.;
pub(crate) const SECTION_HEAD_GAP: f32 = 8.;
const DISC_GLYPH: f32 = 16.;
const MARK_RADIUS_SHARE: f32 = 0.25;
const MARK_GLYPH_SHARE: f32 = 0.56;
const MARK_INK: u32 = 0x0b0b0c;
const ICON_BUTTON: f32 = 24.;
const ICON_BUTTON_RADIUS: f32 = 8.;
const ICON_GLYPH: f32 = 16.;
const SWITCH_WIDTH: f32 = 40.;
const SWITCH_HEIGHT: f32 = 24.;
const SWITCH_KNOB: f32 = 18.;
const SWITCH_INSET: f32 = 3.;
const DIALOG_RADIUS: f32 = 16.;
const DIALOG_PADDING: f32 = 20.;
const DIALOG_GAP: f32 = 20.;
const DIALOG_HEAD_GAP: f32 = 6.;
const FIELD_GAP: f32 = 6.;
const DISABLED_OPACITY: f32 = 0.4;
const STEP_BUTTON: f32 = 24.;
const STEP_BUTTON_RADIUS: f32 = 4.;
const STEP_GLYPH: f32 = 14.;
const STEPPER_INSET: f32 = 2.;
const STEPPER_VALUE: f32 = 36.;
const BADGE_HEIGHT: f32 = 16.;
const BADGE_PADDING_X: f32 = 4.;
const BADGE_RADIUS: f32 = 4.;
const METER_LABEL: f32 = 52.;
const METER_BAR: f32 = 88.;
const METER_PERCENT: f32 = 34.;
const METER_THICKNESS: f32 = 3.;
const METER_GAP: f32 = 8.;

pub(crate) const SIZE_XS: f32 = 11.;
pub(crate) const LINE_XS: f32 = 14.;
pub(crate) const SIZE_SM: f32 = 12.;
pub(crate) const LINE_SM: f32 = 16.;
pub(crate) const SIZE_MD: f32 = 13.;
pub(crate) const LINE_MD: f32 = 18.;
const SIZE_LG: f32 = 15.;
const LINE_LG: f32 = 20.;
const LINE_BODY: f32 = 20.;

const AVATAR_HUES: &[(u32, u32)] = &[
    (0xfca5a5, 0xdc2626),
    (0xfdba74, 0xea580c),
    (0xfcd34d, 0xd97706),
    (0xfde047, 0xca8a04),
    (0xbef264, 0x65a30d),
    (0x86efac, 0x16a34a),
    (0x6ee7b7, 0x059669),
    (0x5eead4, 0x0d9488),
    (0x67e8f9, 0x0891b2),
    (0x7dd3fc, 0x0284c7),
    (0x93c5fd, 0x2563eb),
    (0xa5b4fc, 0x4f46e5),
    (0xc4b5fd, 0x7c3aed),
    (0xd8b4fe, 0x9333ea),
    (0xf0abfc, 0xc026d3),
    (0xf9a8d4, 0xdb2777),
    (0xfda4af, 0xe11d48),
];

#[derive(Clone, Copy)]
pub(crate) struct Ink {
    pub(crate) primary: Hsla,
    pub(crate) secondary: Hsla,
    pub(crate) tertiary: Hsla,
    pub(crate) danger: Hsla,
    pub(crate) warning: Hsla,
    pub(crate) accent: Hsla,
    accent_wash: Hsla,
    pub(crate) card: Hsla,
    pub(crate) edge: Hsla,
    pub(crate) hover: Hsla,
    pub(crate) pressed: Hsla,
    pub(crate) well: Hsla,
    pub(crate) well_hover: Hsla,
    pub(crate) well_pressed: Hsla,
    track: Hsla,
    track_on: Hsla,
    knob_shadow: Hsla,
    mark_edge: Hsla,
    mark_shadow: Hsla,
    dialog: Hsla,
    dialog_edge: Hsla,
    dialog_shadow: Hsla,
    pub(crate) scrim: Hsla,
    fill: Hsla,
    fill_hover: Hsla,
    fill_pressed: Hsla,
    fill_ink: Hsla,
}

impl Ink {
    pub(crate) fn of(ui: crate::theme::UiColors) -> Self {
        let light = ui.surface.l > 0.5;
        let text = ui.text;
        let pick =
            |dark: f32, light_alpha: f32| with_alpha(text, if light { light_alpha } else { dark });
        let fill = text;
        Self {
            primary: text,
            secondary: pick(0.6, 0.64),
            tertiary: pick(0.42, 0.45),
            danger: ui.vc_deleted,
            warning: Hsla::from(rgb(if light { 0xb45309 } else { 0xfcd34d })),
            accent: switch_blue(),
            accent_wash: with_alpha(switch_blue(), if light { 0.1 } else { 0.16 }),
            card: if light {
                with_alpha(gpui::white(), 0.4)
            } else {
                with_alpha(gpui::white(), 0.031)
            },
            edge: pick(0.04, 0.09),
            hover: pick(0.05, 0.04),
            pressed: pick(0.09, 0.07),
            well: pick(0.08, 0.05),
            well_hover: pick(0.12, 0.08),
            well_pressed: pick(0.16, 0.11),
            track: pick(0.14, 0.09),
            track_on: with_alpha(switch_blue(), if light { 0.82 } else { 0.78 }),
            knob_shadow: gpui::black().opacity(if light { 0.14 } else { 0.35 }),
            mark_edge: if light {
                gpui::black().opacity(0.11)
            } else {
                gpui::white().opacity(0.8)
            },
            mark_shadow: gpui::black().opacity(if light { 0.06 } else { 0.35 }),
            dialog: select_menu_surface(ui),
            dialog_edge: pick(0.07, 0.14),
            dialog_shadow: gpui::black().opacity(if light { 0.12 } else { 0.5 }),
            scrim: gpui::black().opacity(if light { 0.18 } else { 0.35 }),
            fill,
            fill_hover: if light {
                lerp_color(fill, gpui::white(), 0.08)
            } else {
                gpui::white()
            },
            fill_pressed: lerp_color(fill, gpui::black(), if light { 1. } else { 0.1 }),
            fill_ink: if light {
                gpui::white()
            } else {
                Hsla::from(rgb(MARK_INK))
            },
        }
    }
}

fn query() -> String {
    crate::settings::search::active_query()
}

pub(crate) fn rule(ink: Ink) -> Div {
    div().flex_none().h(px(1.)).w_full().bg(ink.edge)
}

pub(crate) fn group(ink: Ink) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .rounded(px(CARD_RADIUS))
        .bg(ink.card)
        .border_1()
        .border_color(ink.edge)
        .overflow_hidden()
}

pub(crate) fn separated(ink: Ink, rows: impl IntoIterator<Item = AnyElement>) -> Div {
    let mut card = group(ink);
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            card = card.child(rule(ink));
        }
        card = card.child(row);
    }
    card
}

pub(crate) fn section(
    ink: Ink,
    title: impl Into<SharedString>,
    hint: Option<&'static str>,
    body: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(SECTION_HEAD_GAP))
        .child(section_head(ink, title, hint))
        .child(body)
}

pub(crate) fn section_with(
    ink: Ink,
    title: impl Into<SharedString>,
    hint: Option<&'static str>,
    action: impl IntoElement,
    body: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .gap(px(SECTION_HEAD_GAP))
        .child(section_head_with(ink, title, hint, action))
        .child(body)
}

pub(crate) fn section_head_with(
    ink: Ink,
    title: impl Into<SharedString>,
    hint: Option<&'static str>,
    action: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_end()
        .gap(px(12.))
        .child(section_head(ink, title, hint).flex_1().min_w_0())
        .child(
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .child(action),
        )
}

pub(crate) fn section_head(
    ink: Ink,
    title: impl Into<SharedString>,
    hint: Option<&'static str>,
) -> Div {
    let title: SharedString = title.into();
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .px(px(TEXT_INSET))
        .child(
            div()
                .text_size(px(SIZE_XS))
                .line_height(px(LINE_XS))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.tertiary)
                .child(highlight_matches(title.to_string(), &query())),
        )
        .when_some(hint, |head, hint| {
            head.child(
                div()
                    .text_size(px(SIZE_SM))
                    .line_height(px(LINE_SM))
                    .text_color(ink.tertiary)
                    .child(hint),
            )
        })
}

pub(crate) fn headline(ink: Ink, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(SIZE_LG))
        .line_height(px(LINE_LG))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(ink.primary)
        .child(text.into())
}

pub(crate) fn page_description(ink: Ink, text: &'static str) -> Div {
    div()
        .text_size(px(SIZE_MD))
        .line_height(px(LINE_BODY))
        .text_color(ink.secondary)
        .child(text)
}

pub(crate) struct Text {
    label: SharedString,
    label_ink: Option<Hsla>,
    label_element: Option<AnyElement>,
    badge: Option<AnyElement>,
    lines: Vec<AnyElement>,
}

impl Text {
    pub(crate) fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            label_ink: None,
            label_element: None,
            badge: None,
            lines: Vec::new(),
        }
    }

    pub(crate) fn danger(mut self, ink: Ink) -> Self {
        self.label_ink = Some(ink.danger);
        self
    }

    pub(crate) fn drawn_as(mut self, label: impl IntoElement) -> Self {
        self.label_element = Some(label.into_any_element());
        self
    }

    pub(crate) fn line(mut self, line: impl IntoElement) -> Self {
        self.lines.push(line.into_any_element());
        self
    }

    pub(crate) fn badge(mut self, badge: impl IntoElement) -> Self {
        self.badge = Some(badge.into_any_element());
        self
    }

    pub(crate) fn maybe_line(self, line: Option<AnyElement>) -> Self {
        match line {
            Some(line) => self.line(line),
            None => self,
        }
    }

    pub(crate) fn render(self, ink: Ink) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(HAIR))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .text_size(px(SIZE_MD))
                            .line_height(px(LINE_MD))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(self.label_ink.unwrap_or(ink.primary))
                            .child(match self.label_element {
                                Some(label) => label,
                                None => highlight_matches(self.label.to_string(), &query())
                                    .into_any_element(),
                            }),
                    )
                    .children(self.badge),
            )
            .children(self.lines)
    }
}

pub(crate) fn line(ink: Ink, text: impl Into<SharedString>) -> Div {
    line_in(ink.tertiary, text)
}

pub(crate) fn line_in(color: Hsla, text: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .text_size(px(SIZE_SM))
        .line_height(px(LINE_SM))
        .text_color(color)
        .child(text.into())
}

pub(crate) fn mono_line(ink: Ink, text: impl Into<SharedString>) -> Div {
    line(ink, text).font_family(mono_family()).truncate()
}

pub(crate) fn mono_family() -> SharedString {
    crate::terminal::element::resolve_font_family(None).into()
}

pub(crate) fn badge(ink: Ink, text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .h(px(BADGE_HEIGHT))
        .px(px(BADGE_PADDING_X))
        .flex()
        .items_center()
        .rounded(px(BADGE_RADIUS))
        .bg(ink.accent_wash)
        .text_size(px(SIZE_XS))
        .line_height(px(LINE_XS))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(ink.accent)
        .child(text.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MeterTone {
    Normal,
    Warning,
    Danger,
}

pub(crate) fn meter(
    ink: Ink,
    label: &'static str,
    fraction: f64,
    percent: impl Into<SharedString>,
    tone: MeterTone,
) -> Div {
    let fill = match tone {
        MeterTone::Normal => ink.accent,
        MeterTone::Warning => ink.warning,
        MeterTone::Danger => ink.danger,
    };
    let percent_ink = if tone == MeterTone::Normal {
        ink.tertiary
    } else {
        fill
    };
    let small = |text: SharedString, width: f32, color: Hsla| {
        div()
            .flex_none()
            .w(px(width))
            .text_size(px(SIZE_XS))
            .line_height(px(LINE_XS))
            .text_color(color)
            .child(text)
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(METER_GAP))
        .flex_none()
        .child(small(label.into(), METER_LABEL, ink.tertiary))
        .child(
            div()
                .flex_none()
                .w(px(METER_BAR))
                .h(px(METER_THICKNESS))
                .rounded_full()
                .bg(ink.track)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .rounded_full()
                        .bg(fill)
                        .w(px(METER_BAR * fraction.clamp(0., 1.) as f32)),
                ),
        )
        .child(small(percent.into(), METER_PERCENT, percent_ink).text_right())
}

pub(crate) fn meters(rows: impl IntoIterator<Item = Div>) -> Div {
    div().flex().flex_col().gap(px(HAIR)).children(rows)
}

fn row_frame() -> Div {
    div()
        .w_full()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(ROW_GAP))
        .px(px(ROW_PADDING_X))
        .py(px(ROW_PADDING_Y))
}

fn lead_and_text(lead: Option<AnyElement>, text: Div, wraps: bool) -> Div {
    div()
        .flex_grow(1.)
        .flex_basis(px(0.))
        .min_w(px(if wraps { MIN_TEXT } else { 0. }))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(LEAD_GAP))
        .children(lead)
        .child(text)
}

pub(crate) fn row(
    ink: Ink,
    lead: Option<AnyElement>,
    text: Text,
    control: Option<AnyElement>,
) -> Div {
    row_capped(ink, lead, text, control, Some(CONTROL_MAX))
}

pub(crate) fn wide_row(
    ink: Ink,
    lead: Option<AnyElement>,
    text: Text,
    control: Option<AnyElement>,
) -> Div {
    row_capped(ink, lead, text, control, None)
}

fn row_capped(
    ink: Ink,
    lead: Option<AnyElement>,
    text: Text,
    control: Option<AnyElement>,
    cap: Option<f32>,
) -> Div {
    row_frame()
        .child(lead_and_text(lead, text.render(ink), true))
        .when_some(control, |row, control| {
            row.child(
                div()
                    .flex_none()
                    .when_some(cap, |slot, cap| slot.max_w(px(cap)))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .child(control),
            )
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Only,
    First,
    Middle,
    Last,
}

impl Place {
    pub(crate) fn at(index: usize, count: usize) -> Self {
        match (index == 0, index + 1 >= count) {
            (true, true) => Self::Only,
            (true, false) => Self::First,
            (false, true) => Self::Last,
            (false, false) => Self::Middle,
        }
    }

    fn rounds_top(self) -> bool {
        matches!(self, Self::Only | Self::First)
    }

    fn rounds_bottom(self) -> bool {
        matches!(self, Self::Only | Self::Last)
    }
}

pub(crate) fn link_row(
    id: impl Into<ElementId>,
    ink: Ink,
    lead: Option<AnyElement>,
    text: Text,
    place: Place,
) -> Stateful<Div> {
    let label = text.label.clone();
    let inner = px(CARD_RADIUS - CARD_BORDER);
    let ring = keyboard_focus_inset(ink.accent);
    row_frame()
        .flex_nowrap()
        .id(id.into())
        .cursor(gpui::CursorStyle::PointingHand)
        .when(place.rounds_top(), |row| row.rounded_t(inner))
        .when(place.rounds_bottom(), |row| row.rounded_b(inner))
        .tab_index(0)
        .focus_visible(move |style| style.shadow(ring.clone()))
        .hover(move |style| style.bg(ink.hover))
        .active(move |style| style.bg(ink.pressed))
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .child(lead_and_text(lead, text.render(ink), false))
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .mr(px(-(CHEVRON * (1. - 15. / 24.))))
                .child(
                    svg()
                        .size(px(CHEVRON))
                        .path("icons/settings/arrow-right-01.svg")
                        .text_color(ink.tertiary),
                ),
        )
}

pub(crate) fn disc(ink: Ink, icon: &'static str) -> AnyElement {
    disc_holding(
        ink.well,
        svg()
            .size(px(DISC_GLYPH))
            .path(icon)
            .text_color(ink.secondary)
            .into_any_element(),
    )
}

pub(crate) fn disc_holding(fill: Hsla, inside: AnyElement) -> AnyElement {
    div()
        .flex_none()
        .size(px(LEAD))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(fill)
        .child(inside)
        .into_any_element()
}

pub(crate) fn mark_tile(agent: TerminalAgent, size: f32, ink: Ink) -> AnyElement {
    let glyph = (size * MARK_GLYPH_SHARE).round();
    let tint = agent
        .accent()
        .map(|color| Hsla::from(rgb(color)))
        .unwrap_or_else(|| Hsla::from(rgb(MARK_INK)));
    div()
        .flex_none()
        .size(px(size))
        .rounded(px(size * MARK_RADIUS_SHARE))
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui::white())
        .border_1()
        .border_color(ink.mark_edge)
        .shadow(vec![
            gpui::BoxShadow::new(px(0.), px(1.), ink.mark_shadow).blur_radius(px(2.)),
        ])
        .child(render_logo(
            agent.icon_path(),
            agent.icon_multicolor(),
            px(glyph),
            tint,
        ))
        .into_any_element()
}

fn seed_of(text: &str) -> u32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

pub(crate) fn avatar_hues(seed: &str) -> ((u32, u32), f32) {
    let hash = seed_of(seed);
    let hue = AVATAR_HUES[(hash as usize) % AVATAR_HUES.len()];
    let angle = (hash >> 8) % 360;
    (hue, angle as f32)
}

pub(crate) fn avatar(seed: &str) -> AnyElement {
    let ((soft, deep), angle) = avatar_hues(seed);
    div()
        .flex_none()
        .size(px(LEAD))
        .rounded_full()
        .bg(gpui::linear_gradient(
            angle,
            gpui::linear_color_stop(Hsla::from(rgb(soft)), 0.),
            gpui::linear_color_stop(Hsla::from(rgb(deep)), 1.),
        ))
        .into_any_element()
}

pub(crate) fn switch(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    on: bool,
    ink: Ink,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let travel = SWITCH_WIDTH - SWITCH_KNOB - 2. * SWITCH_INSET;
    let (track_off, track_on, knob_shadow) = (ink.track, ink.track_on, ink.knob_shadow);
    let track = div()
        .id((id.clone(), "track"))
        .relative()
        .w(px(SWITCH_WIDTH))
        .h(px(SWITCH_HEIGHT))
        .rounded_full()
        .animated_toggle(on, move |track, delta| {
            track.style().bg(lerp_color(track_off, track_on, delta));
            track.extend([div()
                .absolute()
                .top(px(SWITCH_INSET))
                .left(px(SWITCH_INSET + travel * delta))
                .size(px(SWITCH_KNOB))
                .rounded_full()
                .bg(gpui::white())
                .shadow(vec![
                    gpui::BoxShadow::new(px(0.), px(1.), knob_shadow).blur_radius(px(2.)),
                ])
                .into_any_element()]);
        });
    div()
        .id(id)
        .flex_none()
        .focusable()
        .cursor(gpui::CursorStyle::PointingHand)
        .role(gpui::accesskit::Role::Switch)
        .aria_label(label.into())
        .aria_toggled(if on {
            gpui::accesskit::Toggled::True
        } else {
            gpui::accesskit::Toggled::False
        })
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(track)
}

pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ink: Ink,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let group = SharedString::from(format!("{id}-glyph"));
    div()
        .id(id)
        .group(group.clone())
        .flex_none()
        .size(px(ICON_BUTTON))
        .rounded(px(ICON_BUTTON_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .cursor(gpui::CursorStyle::PointingHand)
        .hover(move |style| style.bg(ink.hover))
        .active(move |style| style.bg(ink.pressed))
        .accessible_control(gpui::accesskit::Role::Button, label)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            svg()
                .size(px(ICON_GLYPH))
                .path(icon)
                .text_color(ink.tertiary)
                .group_hover(group, move |style| style.text_color(ink.primary)),
        )
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tone {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Size {
    Small,
    Medium,
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    size: Size,
    ink: Ink,
) -> Stateful<Div> {
    button_with(id, label, tone, size, false, true, ink)
}

pub(crate) fn button_with(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: Tone,
    size: Size,
    busy: bool,
    enabled: bool,
    ink: Ink,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    let (rest, hover, pressed, text, text_hover) = match tone {
        Tone::Primary => (
            ink.fill,
            ink.fill_hover,
            ink.fill_pressed,
            ink.fill_ink,
            ink.fill_ink,
        ),
        Tone::Secondary => (
            ink.well,
            ink.well_hover,
            ink.well_pressed,
            ink.primary,
            ink.primary,
        ),
        Tone::Ghost => (
            with_alpha(ink.well, 0.),
            ink.well,
            ink.well_hover,
            ink.secondary,
            ink.primary,
        ),
        Tone::Danger => (
            with_alpha(ink.danger, 0.),
            with_alpha(ink.danger, 0.14),
            with_alpha(ink.danger, 0.22),
            ink.danger,
            ink.danger,
        ),
    };
    let (height, padding, radius, text_size) = match size {
        Size::Small => (28., 10., 6., SIZE_SM),
        Size::Medium => (32., 12., 8., SIZE_MD),
    };
    let interactive = enabled && !busy;
    let spinner_ink = text;
    div()
        .id(id.into())
        .relative()
        .flex_none()
        .h(px(height))
        .px(px(padding))
        .rounded(px(radius))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(8.))
        .bg(rest)
        .text_size(px(text_size))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(text)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label.clone())
        .when(!enabled, |button| button.opacity(DISABLED_OPACITY))
        .when(interactive, move |button| {
            button
                .cursor(gpui::CursorStyle::PointingHand)
                .hover(move |style| style.bg(hover).text_color(text_hover))
                .active(move |style| style.bg(pressed).text_color(text_hover))
        })
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(div().when(busy, |label| label.opacity(0.)).child(label))
        .when(busy, move |button| {
            button.child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::ui_primitives::comet_spinner(px(12.), spinner_ink)),
            )
        })
}

pub(crate) fn spinner(ink: Ink) -> AnyElement {
    crate::ui_primitives::comet_spinner(px(16.), ink.secondary)
}

pub(crate) fn backdrop(
    id: impl Into<ElementId>,
    ink: Ink,
    card: impl IntoElement,
    on_dismiss: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let outline = crate::window_chrome::csd::active_window_outline();
    gpui::deferred(
        div()
            .id(id.into())
            .occlude()
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
            .bg(ink.scrim)
            .on_mouse_down(gpui::MouseButton::Left, on_dismiss)
            .child(card),
    )
    .with_priority(DIALOG_PRIORITY)
    .into_any_element()
}

pub(crate) const DIALOG_PRIORITY: usize = 10;
pub(crate) const DIALOG_MENU_PRIORITY: usize = DIALOG_PRIORITY + 1;

pub(crate) fn dialog(
    id: impl Into<ElementId>,
    ink: Ink,
    title: impl Into<SharedString>,
    body: impl Into<SharedString>,
) -> Stateful<Div> {
    let title: SharedString = title.into();
    div()
        .id(id.into())
        .occlude()
        .w(px(DIALOG_WIDTH))
        .max_w_full()
        .flex()
        .flex_col()
        .gap(px(DIALOG_GAP))
        .p(px(DIALOG_PADDING))
        .rounded(px(DIALOG_RADIUS))
        .bg(ink.dialog)
        .border_1()
        .border_color(ink.dialog_edge)
        .shadow(vec![
            gpui::BoxShadow::new(px(0.), px(12.), ink.dialog_shadow).blur_radius(px(32.)),
        ])
        .role(gpui::accesskit::Role::Dialog)
        .aria_label(title.clone())
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(DIALOG_HEAD_GAP))
                .child(headline(ink, title))
                .child(
                    div()
                        .text_size(px(SIZE_MD))
                        .line_height(px(LINE_BODY))
                        .text_color(ink.secondary)
                        .child(body.into()),
                ),
        )
}

pub(crate) fn dialog_footer(aside: Option<AnyElement>, actions: Vec<AnyElement>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(8.))
        .when_some(aside, |footer, aside| {
            footer.child(div().flex_1().min_w_0().flex().flex_row().child(aside))
        })
        .children(actions)
}

pub(crate) fn field(
    ink: Ink,
    label: &'static str,
    control: impl IntoElement,
    hint: Option<AnyElement>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(FIELD_GAP))
        .w_full()
        .child(
            div()
                .text_size(px(SIZE_SM))
                .line_height(px(LINE_SM))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.secondary)
                .child(label),
        )
        .child(control)
        .children(hint)
}

pub(crate) fn search_group(ink: Ink, rows: Vec<(f32, AnyElement)>) -> Div {
    let mut card = group(ink);
    let mut previous: Option<f32> = None;
    for (amount, row) in rows {
        if let Some(above) = previous {
            let visible = above.min(amount);
            card = card.child(
                div()
                    .w_full()
                    .h(px(visible))
                    .overflow_hidden()
                    .opacity(visible)
                    .child(rule(ink)),
            );
        }
        card = card.child(row);
        previous = Some(amount);
    }
    card
}

pub(crate) fn section_gap(first: &mut bool) -> f32 {
    if std::mem::take(first) {
        0.
    } else {
        SECTION_GAP
    }
}

pub(crate) fn step_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    enabled: bool,
    ink: Ink,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .size(px(STEP_BUTTON))
        .rounded(px(STEP_BUTTON_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .accessible_control(gpui::accesskit::Role::Button, label)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .when(!enabled, |button| button.opacity(DISABLED_OPACITY))
        .when(enabled, move |button| {
            button
                .cursor(gpui::CursorStyle::PointingHand)
                .hover(move |style| style.bg(ink.well_hover))
                .active(move |style| style.bg(ink.well_pressed))
        })
        .child(
            svg()
                .size(px(STEP_GLYPH))
                .path(icon)
                .text_color(ink.primary),
        )
}

pub(crate) fn stepper(
    ink: Ink,
    value: impl Into<SharedString>,
    decrease: Stateful<Div>,
    increase: Stateful<Div>,
) -> Div {
    div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .p(px(STEPPER_INSET))
        .rounded(px(STEP_BUTTON_RADIUS + STEPPER_INSET))
        .bg(ink.well)
        .child(decrease)
        .child(
            div()
                .flex_none()
                .w(px(STEPPER_VALUE))
                .text_center()
                .text_size(px(SIZE_SM))
                .line_height(px(LINE_SM))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(ink.primary)
                .child(value.into()),
        )
        .child(increase)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct RowHarness;

    impl gpui::Render for RowHarness {
        fn render(
            &mut self,
            _window: &mut gpui::Window,
            _cx: &mut gpui::Context<Self>,
        ) -> impl IntoElement {
            let ink = Ink::of(crate::theme::ui_colors());
            div().w(px(600.)).child(row(
                ink,
                None,
                Text::new("Native OS notifications")
                    .line(line(ink, "Alert you when an agent needs attention.")),
                Some(
                    div()
                        .debug_selector(|| "row-control".into())
                        .flex_none()
                        .w(px(40.))
                        .h(px(24.))
                        .into_any_element(),
                ),
            ))
        }
    }

    #[gpui::test]
    fn a_settings_row_insets_its_control_by_the_row_padding(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| RowHarness);
        cx.simulate_resize(gpui::size(px(800.), px(200.)));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let control = cx.debug_bounds("row-control").expect("control painted");
        assert_eq!(control.right(), px(600. - ROW_PADDING_X));
    }

    #[test]
    fn a_row_rounds_the_card_corners_it_touches() {
        assert_eq!(Place::at(0, 1), Place::Only);
        assert_eq!(Place::at(0, 3), Place::First);
        assert_eq!(Place::at(1, 3), Place::Middle);
        assert_eq!(Place::at(2, 3), Place::Last);
        assert!(Place::First.rounds_top() && !Place::First.rounds_bottom());
        assert!(Place::Last.rounds_bottom() && !Place::Last.rounds_top());
        assert!(Place::Only.rounds_top() && Place::Only.rounds_bottom());
        assert!(!Place::Middle.rounds_top() && !Place::Middle.rounds_bottom());
    }

    #[test]
    fn an_avatar_keeps_its_colors_for_a_name_and_changes_them_across_names() {
        assert_eq!(avatar_hues("work"), avatar_hues("work"));
        let distinct: std::collections::HashSet<(u32, u32)> = ["work", "perso", "client", "beta"]
            .iter()
            .map(|name| avatar_hues(name).0)
            .collect();
        assert!(distinct.len() > 1);
        let (_, angle) = avatar_hues("work");
        assert!((0. ..360.).contains(&angle));
    }
}
