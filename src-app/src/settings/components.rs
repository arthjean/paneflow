use gpui::{
    AnyElement, ClickEvent, CursorStyle, Div, ElementId, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled,
    deferred, div, img, prelude::*, px, rgb, svg,
};

use crate::ui_primitives::{
    AnimatedHover, AnimatedHoverExt, ROW_RADIUS, highlight_matches, lerp_color, squircle,
    squircle_skin,
};

pub(crate) const SETTINGS_CONTROL_CORNER_RADIUS: Pixels = px(8.);

pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

pub fn section_header(ui: crate::theme::UiColors, label: &'static str) -> impl IntoElement {
    let query = crate::settings::search::active_query();
    div().pb(px(8.)).child(
        div()
            .text_size(crate::ui_primitives::LABEL_SM)
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(ui.muted)
            .child(highlight_matches(label.to_string(), &query)),
    )
}

pub fn section_header_with_action(
    ui: crate::theme::UiColors,
    label: &'static str,
    action: impl IntoElement,
) -> impl IntoElement {
    let query = crate::settings::search::active_query();
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
                .child(highlight_matches(label.to_string(), &query)),
        )
        .child(action)
}

pub fn card_color() -> Hsla {
    crate::theme::ui_colors().card
}

pub fn setting_card(_ui: crate::theme::UiColors) -> Div {
    let bg = card_color();
    div()
        .relative()
        .flex()
        .flex_col()
        .child(squircle::squircle_fill(
            crate::app::constants::SETTINGS_CARD_RADIUS,
            bg,
        ))
}

pub fn card_tint(color: Hsla) -> impl IntoElement {
    squircle::squircle_fill(crate::app::constants::SETTINGS_CARD_RADIUS, color)
}

pub fn hairline(ui: crate::theme::UiColors) -> impl IntoElement {
    div().h(px(1.)).w_full().bg(with_alpha(ui.border, 0.5))
}

pub const CARD_PADDING_X: f32 = 16.;
pub const CARD_PADDING_Y: f32 = 6.;
pub const ROW_HAIRLINE_INSET: f32 = CARD_PADDING_X + 18. + 12.;

pub fn hairline_inset(ui: crate::theme::UiColors) -> impl IntoElement {
    div()
        .h(px(1.))
        .w_full()
        .pl(px(ROW_HAIRLINE_INSET))
        .child(div().h_full().w_full().bg(with_alpha(ui.border, 0.5)))
}

pub fn section_title(
    ui: crate::theme::UiColors,
    label: &'static str,
    description: Option<&'static str>,
) -> impl IntoElement {
    let query = crate::settings::search::active_query();
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(2.))
        .pb(px(8.))
        .child(
            div()
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child(highlight_matches(label.to_string(), &query)),
        )
        .when_some(description, |d, description| {
            d.child(
                div()
                    .max_w(px(520.))
                    .text_size(crate::ui_primitives::LABEL_SM)
                    .text_color(ui.muted)
                    .child(description),
            )
        })
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
        .px(px(CARD_PADDING_X))
        .py(px(12.))
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
        .child(
            div()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .child(highlight_matches(description.to_string(), &query)),
        )
}

pub fn text_field(
    input: gpui::Entity<crate::widgets::text_input::TextInput>,
    ui: crate::theme::UiColors,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(180.))
        .max_w(px(320.))
        .px(px(10.))
        .py(px(6.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(ui.subtle)
        .text_size(px(12.))
        .text_color(ui.text)
        .child(input)
}

pub fn secondary_button(
    id: impl Into<SharedString>,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let hover_bg = lerp_color(ui.subtle, ui.text, 0.06);
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
    .role(gpui::accesskit::Role::Button)
    .aria_label(label)
    .child(label)
    .on_click(on_click)
}

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
    .role(gpui::accesskit::Role::Button)
    .aria_label(label)
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
    deferred(
        div()
            .id(id)
            .absolute()
            .top_0()
            .left_0()
            .size_full()
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
    div()
        .id(id)
        .occlude()
        .relative()
        .w(width)
        .rounded(radius)
        .shadow_lg()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(squircle::squircle_fill(radius, card_color()))
        .child(content.relative().flex().flex_col())
        .child(squircle::squircle_border(
            radius,
            px(1.),
            with_alpha(ui.border, 0.6),
        ))
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

pub(crate) fn confirmation_list(
    ui: crate::theme::UiColors,
    rows: impl IntoIterator<Item = (SharedString, SharedString)>,
    max_listed: usize,
) -> Div {
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
    for (index, (title, detail)) in rows.into_iter().enumerate() {
        if index >= max_listed {
            hidden += 1;
            continue;
        }
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(crate::ui_primitives::BODY)
                        .text_color(ui.text)
                        .child(title),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .text_color(ui.muted)
                        .child(detail),
                ),
        );
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

pub(crate) const MENU_ROW_HEIGHT: Pixels = px(34.);
pub(crate) const MENU_ROW_GAP: Pixels = px(1.);
pub(crate) const MENU_PADDING: Pixels = px(7.);
pub(crate) const MENU_MAX_HEIGHT: Pixels = px(400.);

pub fn menu_panel<E: Styled + ParentElement>(el: E, ui: crate::theme::UiColors) -> E {
    menu_surface(el, ui)
        .flex()
        .flex_col()
        .gap(MENU_ROW_GAP)
        .p(MENU_PADDING)
}

pub fn menu_row(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    select_item(id, selected, ui).h(MENU_ROW_HEIGHT)
}

pub fn menu_height(rows: f32, extra: f32) -> Pixels {
    px(f32::from(MENU_PADDING) * 2.
        + rows * (f32::from(MENU_ROW_HEIGHT) + f32::from(MENU_ROW_GAP))
        + extra)
}

pub fn select_menu(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> SelectMenu {
    let id: ElementId = id.into();
    let list_id: ElementId = (id.clone(), "list").into();
    let reveal_id: ElementId = (id.clone(), "reveal").into();
    SelectMenu {
        reveal_id,
        shell: menu_surface(div().id(id), ui)
            .flex()
            .flex_col()
            .min_w(px(200.))
            .max_w(px(280.))
            .max_h(MENU_MAX_HEIGHT)
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        list: div()
            .id(list_id)
            .flex()
            .flex_col()
            .gap(MENU_ROW_GAP)
            .p(MENU_PADDING)
            .min_h_0()
            .overflow_y_scroll(),
    }
}

pub struct SelectMenu {
    reveal_id: ElementId,
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
    select_item_tinted(id, selected, ui, with_alpha(ui.text, 0.10))
}

pub fn select_item_tinted(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
    selected_bg: Hsla,
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
    let reveal_id = menu.reveal_id.clone();
    deferred(crate::ui_primitives::menu_reveal(
        reveal_id,
        div()
            .absolute()
            .top(px(36.))
            .right(px(0.))
            .occlude()
            .child(menu),
    ))
    .with_priority(1)
    .into_any_element()
}

pub(crate) fn settings_label(
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
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(ui.muted)
                .child(description),
        )
}

pub(crate) fn switch_blue() -> Hsla {
    Hsla::from(rgb(0x339cff))
}

pub(crate) fn apple_red() -> Hsla {
    Hsla::from(rgb(0xff453a))
}

pub(crate) fn quiet_card() -> gpui::Div {
    let bg = card_color();
    div()
        .flex()
        .flex_col()
        .bg(bg)
        .rounded(px(8.))
        .overflow_hidden()
}

pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    primary: bool,
    enabled: bool,
) -> AnimatedHover {
    let bg = if primary { switch_blue() } else { ui.subtle };
    let fg = if primary { gpui::white() } else { ui.text };
    let disabled_bg = ui.subtle;
    let disabled_fg = ui.muted;
    let resting_background = if enabled { bg } else { disabled_bg };
    let hover_background = if !enabled {
        resting_background
    } else if primary {
        with_alpha(bg, 0.86)
    } else {
        with_alpha(ui.text, 0.06)
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(resting_background)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { disabled_fg })
        .animated_hover_bg(resting_background, hover_background)
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { disabled_fg }),
        )
        .child(label.into())
}

pub(crate) fn destructive_icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    enabled: bool,
) -> AnimatedHover {
    let bg = apple_red();
    let enabled_hover_background = Hsla {
        l: (bg.l - 0.05).max(0.0),
        ..bg
    };
    let fg = gpui::white();
    let resting_background = if enabled { bg } else { ui.subtle };
    let hover_background = if enabled {
        enabled_hover_background
    } else {
        resting_background
    };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(resting_background)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { ui.muted })
        .animated_hover_bg(resting_background, hover_background)
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { ui.muted }),
        )
        .child(label.into())
}

pub(crate) fn save_icon_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: &'static str,
    ui: crate::theme::UiColors,
    enabled: bool,
) -> AnimatedHover {
    let light_theme = ui.surface.l > 0.5;
    let bg: Hsla = if light_theme {
        rgb(0x000000).into()
    } else {
        gpui::white()
    };
    let fg: Hsla = if light_theme {
        gpui::white()
    } else {
        rgb(0x000000).into()
    };
    let resting_background = if enabled { bg } else { ui.subtle };
    let hover_background = if enabled {
        Hsla { a: 0.86, ..bg }
    } else {
        resting_background
    };

    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(5.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .bg(resting_background)
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(if enabled { fg } else { ui.muted })
        .animated_hover_bg(resting_background, hover_background)
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if enabled { fg } else { ui.muted }),
        )
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_rows_nest_concentrically_inside_the_menu_surface() {
        let painted = f32::from(squircle::corner_for_height(MENU_ROW_HEIGHT, ROW_RADIUS));
        let nested = f32::from(MENU_PADDING) + painted;
        let surface = f32::from(MENU_RADIUS);
        assert!(
            (nested - surface).abs() <= 0.5,
            "a menu row corner must sit concentrically inside the {surface} surface corner: {} padding plus a {painted} painted corner is {nested}",
            f32::from(MENU_PADDING)
        );
    }
}
