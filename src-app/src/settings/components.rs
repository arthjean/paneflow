//! Cockpit-style UI primitives shared across every settings tab.
//!
//! Visual recipes mirror the cockpit rails:
//! - **section_header** - lowercase eyebrow (11px, NORMAL, `ui.muted`), no
//!   border below.
//! - **section_header_with_action** - same eyebrow with a right-aligned
//!   secondary button (used by Shortcuts/Appearance "Reset to defaults").
//! - **setting_card** - borderless theme-aware panel (white in light, `#232323`
//!   in dark) with the CLI pane card's squircle corner. Wraps row groups so
//!   each section reads as a card the way the pane grid does.
//! - **card_tint** - a selection/emphasis fill painted over a card, in the same
//!   squircle, because a plain `.bg()` would square its corners.
//! - **hairline** - 1px row separator (border at ~50% alpha), used inside
//!   cards to split rows.
//! - **toggle_pill** - Codex/iOS switch: a 36x22 pill, solid `#339cff` track
//!   when on / soft neutral when off, with a white thumb.
//! - **secondary_button** - filled, agents cancel-button style
//!   (`ui.subtle` bg, no border).
//!
//! - **toggle_row** - a full setting row (optional leading icon, title +
//!   description, trailing switch) that owns its own persist listener. Shared
//!   by the General and AI Agent pages.
//!
//! All helpers return `impl IntoElement` or `Div`. Apart from `secondary_button`
//! and `toggle_row`, they take no listeners - parent rows wire their own
//! `.id()` and `.on_click()`.

use gpui::{
    AnyElement, ClickEvent, CursorStyle, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, SharedString, Stateful, StatefulInteractiveElement, Styled, deferred,
    div, img, prelude::*, px, svg,
};

use crate::ui_primitives::{
    AnimatedHover, AnimatedHoverExt, ROW_RADIUS, lerp_color, squircle, squircle_skin,
};

pub(crate) const SETTINGS_CONTROL_CORNER_RADIUS: Pixels = px(8.);

/// Apply an alpha override to an `Hsla` color. GPUI's `Hsla` has no
/// dedicated builder method for alpha, so we update the field manually.
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// Lowercase eyebrow section label (11px, NORMAL, muted). No border below.
pub fn section_header(ui: crate::theme::UiColors, label: &'static str) -> impl IntoElement {
    div().pb(px(8.)).child(
        div()
            .text_size(crate::ui_primitives::LABEL_SM)
            .font_weight(gpui::FontWeight::NORMAL)
            .text_color(ui.muted)
            .child(label),
    )
}

/// Eyebrow header with a right-aligned action element (typically a
/// `secondary_button`). Same typography as `section_header`.
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

/// The card fill shared by every settings card, chosen by the active theme's
/// lightness: white on a light theme, `#232323` on a dark one. Cards are
/// borderless (see [`setting_card`]), so this is a single color, not a pair.
pub fn card_color() -> Hsla {
    if crate::theme::active_theme().background.l > 0.5 {
        Hsla::from(gpui::rgb(0xffffff))
    } else {
        Hsla::from(gpui::rgb(0x232323))
    }
}

/// Borderless card in the CLI pane-card language: no outline at all, and the
/// squircle corner from `pane.rs` (`PANE_CARD_RADIUS`) instead of GPUI's
/// circular `rounded()`. Same recipe as the pane card minus the hairline - the
/// superellipse fill is painted as an absolute first child, so the host must
/// stay `relative()` and must not set its own `bg`.
///
/// Because the fill is an overlay rather than a clip, it cannot round its
/// children: only wrap rows that leave the card's own background visible at the
/// top and bottom edges (padded rows with no opaque fill of their own).
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

/// A tint painted over a [`setting_card`]'s fill (selection, emphasis).
/// Chain it right after the card, before its content: the card's own fill is
/// already its first child, and GPUI paints children in order.
///
/// A plain `.bg()` on the card would paint a square quad over the superellipse
/// and hand back the corners the squircle just traced.
pub fn card_tint(color: Hsla) -> impl IntoElement {
    squircle::squircle_fill(crate::app::constants::PANE_CARD_RADIUS, color)
}

/// 1px hairline divider used between rows inside a `setting_card`.
/// Half-alpha border so it reads as a separator without competing with
/// the card outline.
pub fn hairline(ui: crate::theme::UiColors) -> impl IntoElement {
    div().h(px(1.)).w_full().bg(with_alpha(ui.border, 0.5))
}

/// A full boolean setting row: optional leading icon, title + description, and
/// a trailing [`toggle_pill`] that writes `config_key` on click. Only the switch
/// is interactive - the row itself neither hovers nor toggles, so a stray click
/// on a long description cannot flip a security-sensitive setting.
///
/// Unlike the other helpers here this one owns its listener, because every call
/// site wires the exact same `persist_setting` write (cache-mutate + notify +
/// off-thread file write).
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

/// [`toggle_row`] with a caller-supplied trailing control. Use it when the write
/// is not a plain top-level bool - a nested key, or a value other than a
/// boolean - so the row geometry still has one definition.
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

/// Pure-visual pill toggle, Codex / iOS style: a solid `#339cff` track when on
/// (fixed in both themes, per design), a soft neutral gray when off, and a white
/// thumb sliding between the ends - borderless for the clean filled look. The
/// parent row owns the `id` + `on_click`.
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

/// Standard "title + description" left column used by every setting row.
/// Grows (`flex_1`) and shrinks below content width (`min_w_0`) so the
/// description text wraps inside the row's bounds.
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

/// Filled secondary button (agents cancel-button style): `ui.subtle` bg,
/// no border, whisper-soft text wash on hover. Used for "Reset to defaults"
/// and similar inline actions inside section headers.
pub fn secondary_button(
    id: &'static str,
    label: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let hover_bg = lerp_color(ui.subtle, ui.text, 0.06);

    // Same silhouette as a menu row and a CLI pane card: `ROW_RADIUS` traced as
    // a superellipse rather than GPUI's circular `rounded()`. The skin paints
    // both fills as absolute children, so the label is chained *after* it -
    // GPUI paints children in order, and a fill added last would cover the text.
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

/// A [`secondary_button`] in destructive red, for an action that cannot be
/// undone. Red fill with a white label rather than theme tokens: `accent` and
/// `text` are independent per theme and their pairing is not guaranteed to be
/// legible, while red-on-white reads the same everywhere and carries the
/// warning by itself.
///
/// Returns the element unclicked so the caller attaches its own listener; a
/// destructive action is never generic enough to bake in here.
pub fn destructive_button(
    id: &'static str,
    label: &'static str,
    _ui: crate::theme::UiColors,
) -> gpui::Stateful<gpui::Div> {
    let resting = Hsla::from(gpui::rgb(0xff453a));
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

// ── Codex-style select / dropdown primitives ─────────────────────────────
//
// Shared by the General, Themes (font picker) and Terminal settings pages so
// every settings dropdown is identical: a subtle-gray trigger pill with an
// up/down selector glyph, opening an elevated, hairline-bordered
// menu with whisper-soft row highlights and click-outside-to-close. Callers own
// the "which dropdown is open" state and wire the handlers; these only style.

/// A leading logo for a select option: `(asset path, multicolor)`. Multicolor
/// brand logos render via `img()` (resvg keeps every fill); monochrome
/// `currentColor` SVGs render via a `text_color`-tinted `svg()` mask so they
/// follow the light/dark theme.
pub type Logo = (&'static str, bool);

/// Render a 14px leading logo (see [`Logo`]).
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

/// The up/down selector glyph shown at the right edge of a select trigger.
pub fn select_chevron(ui: crate::theme::UiColors) -> impl IntoElement {
    svg()
        .size(px(12.))
        .flex_none()
        .path("icons/selector.svg")
        .text_color(with_alpha(ui.muted, 0.7))
}

/// The subtle-gray select trigger pill (Codex style). Returns a `relative` `Div`
/// so a deferred menu can anchor to it; the caller adds the value cluster,
/// [`select_chevron`], an open/close `on_mouse_down`, and (when open) the menu.
pub fn select_trigger(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> AnimatedHover {
    // Same hover as `secondary_button`: a tint of `ui.text` over `ui.subtle`,
    // not a fixed lightness cut. The cut darkened in both themes, so a light
    // theme's trigger and its neighbouring Reset button reacted in opposite
    // directions; the lerp lifts on dark and deepens on light, one recipe for
    // every subtle-gray control on the settings pages.
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

/// The elevated surface color used by [`select_menu`]: white-ish lift in light,
/// a touch lighter than the card in dark. Exposed so menus that cannot reuse the
/// fixed-width [`select_menu`] container (e.g. a stretch-to-width sidebar
/// popover) can still match its surface exactly.
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

/// Hairline color for dividers *inside* a menu (between item groups). A whisper
/// of `ui.text`, NOT `ui.border`: the menu sits on the elevated surface (see
/// [`select_menu_surface`]), which in dark themes is lighter than `ui.border`
/// (`0x2a2a2a` vs `0x252525`), so a `ui.border` divider has near-zero contrast
/// and vanishes. The structural app borders (sidebar/terminal divider, title
/// bar) read as `ui.border` only because they sit on the near-black terminal -
/// same color, far darker backdrop. A text-tint lifts off the menu surface in
/// either theme; at 0.12 it lands on ~`ui.border` over a light theme's white
/// menu (no regression there) while staying clearly visible on the dark menu.
pub fn menu_divider_color(ui: crate::theme::UiColors) -> Hsla {
    with_alpha(ui.text, 0.12)
}

/// Corner of a floating menu surface.
///
/// `ROW_RADIUS` (14) plus the menu's own 4 px inset, so the surface curve and the
/// row curve inside it stay concentric: nesting a corner of radius r inside a
/// padding of p needs an outer radius of `r + p`, or the inner curve reads
/// pinched against the outer one.
pub(crate) const MENU_RADIUS: Pixels = px(18.);

/// Apply the elevated floating-menu *skin* - continuous corner, lifted surface,
/// and a hairline border at 0.6 alpha - to any element. The single source of
/// truth for the Settings "Shell" select look, shared by [`select_menu`] (the
/// fixed-width container) and by every variable-width app menu/popover that
/// anchors to its own trigger (context menus, the diff scope/base pickers, the
/// sidebar Settings popover). Layout (flex, gap, padding, width, interactivity)
/// stays with the caller; this only paints the surface.
///
/// The silhouette is a pair of `squircle` paths rather than `bg()` + `rounded()`
/// because GPUI resolves `corner_radii` with a circular-arc SDF and exposes no
/// corner smoothing - the rest of the app's chrome is drawn as a superellipse
/// (rail rows, dock chrome), and a menu answering that with a quarter circle
/// reads as a different material. Both layers are added *before* the caller's
/// content: GPUI paints children in declaration order and does not clip them to
/// a parent radius, so a fill added afterwards would paint over the rows.
///
/// The host must not be a scroll container - GPUI pushes the scroll offset onto
/// every child, absolute ones included, so the surface would scroll out from
/// under its own rows. [`select_menu`] is the scrolling variant and keeps the
/// scroll on an inner host for exactly this reason.
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

/// The elevated floating menu container: the [`menu_surface`] skin plus tight
/// geometry and a fixed 200-280px width clamp. A press inside is swallowed
/// (stop_propagation); the caller adds the rows and an `on_mouse_down_out` to
/// close. Menus that must size to their own content use [`menu_surface`]
/// directly instead (the width clamp here would fight a stretch/auto width).
///
/// Two elements, not one: the rows scroll past 320px, and GPUI applies a scroll
/// container's offset to *every* child - absolute ones included - so a surface
/// painted as an absolute path on the scroll host would slide out from under
/// its own rows. The shell paints and clamps; an inner host scrolls. Callers see
/// one element: [`SelectMenu`] routes styling and listeners to the shell and
/// children to the list.
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
        // Width stays `auto` on purpose. The shell is sized by its content -
        // `min_w`/`max_w` only clamp, they do not make it definite - so a
        // `w_full()` percentage here has no definite parent to resolve against
        // and collapses back to the content width, leaving every row short of
        // the menu's right edge. `auto` lets flex cross-axis stretch fill the
        // shell instead, and the rows then stretch to the list.
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

/// A [`select_menu`] under construction: a non-scrolling shell carrying the
/// surface, and the scrolling row host inside it.
///
/// The split is invisible to callers - `Styled` and the interaction traits
/// address the shell (so `.absolute()`, `.w()`, `.occlude()`,
/// `.on_mouse_down_out()` land where they did before the split), while
/// `ParentElement` appends to the list (so `.child()` still adds a row).
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

/// One menu row with whisper highlights (selected slightly stronger than hover).
/// The caller adds the leading logo + label children and the `on_click`.
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
    // The rail's row silhouette, so a menu row and a sidebar row are the same
    // control: `ROW_RADIUS` traced as a superellipse rather than GPUI's circular
    // `rounded()`. The hover fill is a visibility toggle rather than an
    // interpolated color for the rail's reason - a long menu would otherwise ask
    // GPUI for an animation frame per hovered row for the whole transition.
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

/// Wrap a built menu in the deferred, occluding popover anchored just under the
/// trigger's right edge. Use as the trigger's last child while it is open.
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
