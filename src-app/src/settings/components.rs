//! Agents-style UI primitives shared across every settings tab.
//!
//! Visual recipes mirror `agents_view::view` + `app::agents_sidebar`:
//! - **section_header** - lowercase eyebrow (11px, NORMAL, `ui.muted`), no
//!   border below. Matches `threads_section_header` in the agents sidebar.
//! - **section_header_with_action** - same eyebrow with a right-aligned
//!   secondary button (used by Shortcuts/Appearance "Reset to defaults").
//! - **setting_card** - explicit theme-aware panel (white/`#e5e5ed` in light,
//!   `#232323`/`#303030` in dark) with a 1px border and a generous Apple-
//!   approximating radius. Wraps row groups so each section reads as a card the
//!   way Agents content cards do.
//! - **hairline** - 1px row separator (border at ~50% alpha), used inside
//!   cards to split rows without competing with the card border.
//! - **toggle_pill** - Codex/iOS switch: a 36x22 pill, solid `#339cff` track
//!   when on / soft neutral when off, with a white thumb.
//! - **secondary_button** - filled, agents cancel-button style
//!   (`ui.subtle` bg, no border).
//!
//! All helpers return `impl IntoElement` or `Div`. They take no listeners
//! beyond the explicit on_click on `secondary_button` - parent rows wire
//! their own `.id()` and `.on_click()`.

use gpui::{
    AnyElement, ClickEvent, CursorStyle, Div, ElementId, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Stateful, Styled, deferred, div, img, prelude::*, px, svg,
};

pub(crate) const SETTINGS_CONTROL_CORNER_RADIUS: Pixels = px(8.);
const SETTINGS_CARD_CORNER_RADIUS: Pixels = px(8.);

/// Apply an alpha override to an `Hsla` color. GPUI's `Hsla` has no
/// dedicated builder method for alpha, so we update the field manually.
pub fn with_alpha(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// Lowercase eyebrow section label (11px, NORMAL, muted). No border below.
/// Mirrors `app::agents_sidebar::threads_section_header`.
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

/// Theme-dependent card fill + border shared by every settings card. Codex
/// polish: white `#ffffff` bg + `#e5e5ed` border in a light theme, `#232323` bg
/// + `#303030` border in a dark theme (chosen by the active theme's lightness).
///
/// Exposed so a bespoke card can match `setting_card` exactly.
pub fn card_colors() -> (Hsla, Hsla) {
    if crate::theme::active_theme().background.l > 0.5 {
        (
            Hsla::from(gpui::rgb(0xffffff)),
            Hsla::from(gpui::rgb(0xe5e5ed)),
        )
    } else {
        (
            Hsla::from(gpui::rgb(0x232323)),
            Hsla::from(gpui::rgb(0x303030)),
        )
    }
}

/// Card container for grouped setting rows. Codex-style polish: an explicit
/// theme-aware fill/border (see [`card_colors`]) - not the theme `ui.surface` -
/// plus a generous Apple-approximating corner radius. `_ui` is retained for
/// call-site compatibility (every tab already has it in scope).
///
/// Returns a `Div` so callers can chain `.child()` to add rows.
pub fn setting_card(_ui: crate::theme::UiColors) -> Div {
    let (bg, border) = card_colors();
    div()
        .flex()
        .flex_col()
        .bg(bg)
        .border_1()
        .border_color(border)
        .rounded(SETTINGS_CARD_CORNER_RADIUS)
        .overflow_hidden()
}

/// 1px hairline divider used between rows inside a `setting_card`.
/// Half-alpha border so it reads as a separator without competing with
/// the card outline.
pub fn hairline(ui: crate::theme::UiColors) -> impl IntoElement {
    div().h(px(1.)).w_full().bg(with_alpha(ui.border, 0.5))
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
    div()
        .id(id)
        .px(px(10.))
        .py(px(4.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .cursor(CursorStyle::PointingHand)
        .bg(ui.subtle)
        .text_size(px(12.))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(ui.text)
        .hover(move |s| s.bg(with_alpha(ui.text, 0.06)))
        .child(label)
        .on_click(on_click)
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
pub fn select_trigger(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> Stateful<Div> {
    let hover_bg = Hsla {
        l: (ui.subtle.l - 0.04).max(0.0),
        ..ui.subtle
    };
    select_trigger_with_hover(id, ui, hover_bg)
}

pub fn select_trigger_with_hover(
    id: impl Into<ElementId>,
    ui: crate::theme::UiColors,
    hover_bg: Hsla,
) -> Stateful<Div> {
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
        .cursor(CursorStyle::PointingHand)
        .hover(move |s| s.bg(hover_bg))
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

/// Apply the elevated floating-menu *skin* - radius, lifted surface, and a
/// hairline border at 0.6 alpha - to any element. The single source of
/// truth for the Settings "Shell" select look, shared by [`select_menu`] (the
/// fixed-width container) and by every variable-width app menu/popover that
/// anchors to its own trigger (context menus, the diff scope/base pickers, the
/// sidebar Settings popover). Layout (flex, gap, padding, width, interactivity)
/// stays with the caller; this only paints the surface.
pub fn menu_surface<E: Styled>(el: E, ui: crate::theme::UiColors) -> E {
    el.rounded(px(10.))
        .bg(select_menu_surface(ui))
        .border_1()
        .border_color(with_alpha(ui.border, 0.6))
}

/// The elevated floating menu container: the [`menu_surface`] skin plus tight
/// geometry and a fixed 200-280px width clamp. A press inside is swallowed
/// (stop_propagation); the caller adds the rows and an `on_mouse_down_out` to
/// close. Menus that must size to their own content use [`menu_surface`]
/// directly instead (the width clamp here would fight a stretch/auto width).
pub fn select_menu(id: impl Into<ElementId>, ui: crate::theme::UiColors) -> Stateful<Div> {
    menu_surface(div().id(id.into()), ui)
        .flex()
        .flex_col()
        .gap(px(1.))
        .p(px(4.))
        .min_w(px(200.))
        .max_w(px(280.))
        .max_h(px(320.))
        .overflow_y_scroll()
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// One menu row with whisper highlights (selected slightly stronger than hover).
/// The caller adds the leading logo + label children and the `on_click`.
pub fn select_item(
    id: impl Into<ElementId>,
    selected: bool,
    ui: crate::theme::UiColors,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .h(px(28.))
        .px(px(8.))
        .rounded(SETTINGS_CONTROL_CORNER_RADIUS)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .cursor(CursorStyle::PointingHand)
        .text_size(px(12.))
        .when(selected, |d| d.bg(with_alpha(ui.text, 0.10)))
        .when(!selected, |d| {
            d.hover(move |s| s.bg(with_alpha(ui.text, 0.05)))
        })
}

/// Wrap a built menu in the deferred, occluding popover anchored just under the
/// trigger's right edge. Use as the trigger's last child while it is open.
pub fn deferred_select_menu(menu: Stateful<Div>) -> AnyElement {
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
