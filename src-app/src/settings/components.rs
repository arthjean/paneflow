use gpui::{
    AnyElement, ClickEvent, CursorStyle, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled, deferred,
    div, img, prelude::*, px, svg,
};

use crate::ui_primitives::{
    AnimatedHover, AnimatedHoverExt, ROW_RADIUS, lerp_color, squircle, squircle_skin,
};

pub(crate) const SETTINGS_CONTROL_CORNER_RADIUS: Pixels = px(8.);

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

pub fn section_header(ui: crate::theme::UiColors, label: &'static str) -> impl IntoElement {
    div().pb(px(8.)).child(
        div()
            .text_size(crate::ui_primitives::LABEL_SM)
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(ui.muted)
            .child(label),
    )
}

pub fn section_header_with_action(
    ui: crate::theme::UiColors,
    label: &'static str,
    action: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .pb(px(8.))
        .child(
            div()
                .text_size(crate::ui_primitives::LABEL_SM)
                .font_weight(gpui::FontWeight::NORMAL)
                .text_color(ui.muted)
                .child(label),
        )
        .child(action)
}

pub fn card_color() -> Hsla {
    if crate::theme::active_theme().background.l > 0.5 {
        Hsla::from(gpui::rgb(0xffffff))
    } else {
        Hsla::from(gpui::rgb(0x232323))
    }
}

pub fn setting_card(_ui: crate::theme::UiColors) -> Div {
    let bg = card_color();
    div()
        .relative()
        .flex()
        .flex_col()
        .child(squircle::squircle_fill(
            crate::app::constants::PANE_CARD_RADIUS,
            bg,
        ))
}

pub fn card_tint(color: Hsla) -> impl IntoElement {
    squircle::squircle_fill(crate::app::constants::PANE_CARD_RADIUS, color)
}

pub fn hairline(ui: crate::theme::UiColors) -> impl IntoElement {
    div().h(px(1.)).w_full().bg(with_alpha(ui.border, 0.5))
}

#[allow(clippy::too_many_arguments)]
pub fn toggle_row(
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: Option<AnyElement>,
    current: bool,
    config_key: &'static str,
    ui: crate::theme::UiColors,
    cx: &mut gpui::Context<crate::PaneFlowApp>,
) -> impl IntoElement {
    let target_value = !current;
    toggle_row_with(
        title,
        description,
        icon,
        ui,
        div()
            .id(SharedString::from(id))
            .flex_shrink_0()
            .on_click(cx.listener(move |this, _: &ClickEvent, _window, cx| {
                this.persist_setting(false, config_key, serde_json::Value::Bool(target_value), cx);
            }))
            .child(toggle_pill(current, ui)),
    )
}

pub fn toggle_row_with(
    title: &'static str,
    description: &'static str,
    icon: Option<AnyElement>,
    ui: crate::theme::UiColors,
    control: impl IntoElement,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(16.))
        .px(px(12.))
        .py(px(10.))
        .when_some(icon, |d, icon| d.child(icon))
        .child(setting_text(ui, title, description))
        .child(control)
}

pub fn toggle_pill(on: bool, ui: crate::theme::UiColors) -> impl IntoElement {
    let track_bg = if on {
        Hsla::from(gpui::rgb(0x339cff))
    } else {
        with_alpha(ui.muted, 0.30)
    };

    let track = div()
        .flex()
        .flex_row()
        .items_center()
        .w(px(36.))
        .h(px(22.))
        .rounded_full()
        .px(px(2.))
        .bg(track_bg)
        .when(on, |s| s.justify_end())
        .when(!on, |s| s.justify_start())
        .child(div().w(px(18.)).h(px(18.)).rounded_full().bg(gpui::white()));

    div().flex_shrink_0().child(track)
}

pub fn setting_text(
    ui: crate::theme::UiColors,
    title: &'static str,
    description: &'static str,
) -> impl IntoElement {
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
                .child(title),
        )
        .child(
            div()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .child(description),
        )
}

pub fn secondary_button(
    id: &'static str,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let hover_bg = lerp_color(ui.subtle, ui.text, 0.06);

    squircle_skin(
        div()
            .id(id)
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
    .child(label)
    .on_click(on_click)
}

pub fn destructive_color() -> Hsla {
    Hsla::from(gpui::rgb(0xff453a))
}

pub fn destructive_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    let resting = destructive_color();
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
    .child(label)
}

pub type Logo = (&'static str, bool);

pub fn render_logo(logo: Logo, ui: crate::theme::UiColors) -> AnyElement {
    let (path, multicolor) = logo;
    if multicolor {
        img(path).size(px(14.)).flex_none().into_any_element()
    } else {
        svg()
            .size(px(14.))
            .flex_none()
            .path(path)
            .text_color(ui.text)
            .into_any_element()
    }
}

pub fn select_chevron(ui: crate::theme::UiColors) -> impl IntoElement {
    svg()
        .size(px(12.))
        .flex_none()
        .path("icons/selector.svg")
        .text_color(with_alpha(ui.muted, 0.7))
}

pub fn select_trigger(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> AnimatedHover {
    select_trigger_with_hover(id, ui, lerp_color(ui.subtle, ui.text, 0.06))
}

pub fn select_trigger_with_hover(
    id: impl Into<ElementId>,
    ui: crate::theme::UiColors,
    hover_bg: Hsla,
) -> AnimatedHover {
    div()
        .id(id.into())
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .min_w(px(190.))
        .max_w(px(260.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .animated_hover_bg(ui.subtle, hover_bg)
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
    with_alpha(ui.text, 0.12)
}

pub(crate) const MENU_RADIUS: Pixels = px(18.);

pub fn menu_surface<E: Styled + ParentElement>(el: E, ui: crate::theme::UiColors) -> E {
    el.relative()
        .child(squircle::squircle_fill(
            MENU_RADIUS,
            select_menu_surface(ui),
        ))
        .child(squircle::squircle_border(
            MENU_RADIUS,
            px(1.),
            with_alpha(ui.border, 0.6),
        ))
}

pub fn select_menu(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> SelectMenu {
    let id: ElementId = id.into();
    let list_id: ElementId = (id.clone(), "list").into();
    SelectMenu {
        shell: menu_surface(div().id(id), ui)
            .flex()
            .flex_col()
            .min_w(px(200.))
            .max_w(px(280.))
            .max_h(px(320.))
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        list: div()
            .id(list_id)
            .flex()
            .flex_col()
            .gap(px(1.))
            .p(px(4.))
            .min_h_0()
            .overflow_y_scroll(),
    }
}

pub struct SelectMenu {
    shell: Stateful<Div>,
    list: Stateful<Div>,
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
        self.list.extend(elements);
    }
}

impl IntoElement for SelectMenu {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        self.shell.child(self.list)
    }
}

pub fn select_item(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    let selected_bg = with_alpha(ui.text, 0.10);
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
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .cursor(CursorStyle::PointingHand)
            .text_size(px(12.)),
        group,
        ROW_RADIUS,
        (resting_bg.a > f32::EPSILON).then_some(resting_bg),
        (hover_bg.a > f32::EPSILON).then_some(hover_bg),
    )
}

pub fn deferred_select_menu(menu: SelectMenu) -> AnyElement {
    deferred(
        div()
            .absolute()
            .top(px(36.))
            .right(px(0.))
            .occlude()
            .child(menu),
    )
    .with_priority(1)
    .into_any_element()
}
