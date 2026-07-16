//! Cross-surface UI primitives shared by the Agents view and the Review (Git
//! Diff) view (review redesign, EP-001 US-003).
//!
//! Before this module, the Review and Agents surfaces re-coded the same recipes
//! inline: a byte-for-byte tooltip struct in each (`DiffHeaderTooltip` ==
//! `HoverActionTooltip`), two near-identical filter fields, two `centered`
//! empty-state helpers, and a dozen ad-hoc icon buttons / pills. Every visual
//! change had to be made twice and the two surfaces had already drifted. This is
//! the single home for those recipes so a later visual change is made once.
//!
//! Layout that depends on view-specific state (the `TextInput` entity, the
//! `cx.listener` handlers, which popover is open) stays with the caller; these
//! helpers only paint the shared skin and accept the dynamic bits as params,
//! mirroring the established pattern in [`crate::settings::components`].

use gpui::{
    AnimationExt, AnyView, App, ClickEvent, Div, ElementId, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, Render, SharedString, Stateful, Styled, Window, div,
    prelude::*, px, svg,
};

use crate::settings::components::with_alpha;
use crate::theme::UiColors;

// ── Type scale ────────────────────────────────────────────────────────────
//
// The named typographic scale for Review/Agents product UI. These are the
// values the Agents view already used as scattered literals; naming them lets
// new code reference a role instead of a magic number (EP-002 US-007 migrates
// the remaining literals onto these).

/// Micro numeric labels and badges (diffstat chips, counts).
pub(crate) const LABEL_XS: Pixels = px(10.);
/// Eyebrows, tooltips, secondary metadata.
pub(crate) const LABEL_SM: Pixels = px(11.);
/// Default body text.
pub(crate) const BODY: Pixels = px(12.);
/// Emphasized body (row titles).
pub(crate) const BODY_EMPHASIS: Pixels = px(13.);
/// Section / panel titles.
pub(crate) const TITLE: Pixels = px(14.);

// ── Tooltip ───────────────────────────────────────────────────────────────

/// The shared hover-tooltip body. Replaces the formerly-duplicated
/// `DiffHeaderTooltip` (diff view) and `HoverActionTooltip` (agents sidebar),
/// which were byte-for-byte identical.
pub(crate) struct PaneflowTooltip {
    pub(crate) label: SharedString,
}

impl Render for PaneflowTooltip {
    fn render(&mut self, _w: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = crate::theme::active_theme();
        let ui = crate::theme::ui_colors();
        div()
            .px(px(8.))
            .py(px(6.))
            .rounded(px(6.))
            .bg(theme.title_bar_background)
            .border_1()
            .border_color(ui.border)
            .text_color(ui.text)
            .text_size(LABEL_SM)
            .child(self.label.clone())
    }
}

/// Convenience builder for `.tooltip(text_tooltip("…"))` - a plain text tooltip
/// using [`PaneflowTooltip`].
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

// ── Icon buttons ────────────────────────────────────────────────────────────

fn icon_button(
    id: impl Into<ElementId>,
    outer: Pixels,
    icon: &'static str,
    icon_size: Pixels,
    icon_color: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(outer)
        .rounded(px(4.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover_bg))
        .child(
            svg()
                .size(icon_size)
                .flex_none()
                .path(icon)
                .text_color(icon_color),
        )
}

/// 20×20 icon button (12px glyph). The caller chains `.on_click` / `.tooltip`
/// and any resting-state `.bg(..)`.
pub(crate) fn icon_button_sm(
    id: impl Into<ElementId>,
    icon: &'static str,
    icon_color: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    icon_button(id, px(20.), icon, px(12.), icon_color, hover_bg)
}

/// 24×24 icon button (13px glyph). The caller chains `.on_click` / `.tooltip`
/// and any resting-state `.bg(..)`.
pub(crate) fn icon_button_md(
    id: impl Into<ElementId>,
    icon: &'static str,
    icon_color: Hsla,
    hover_bg: Hsla,
) -> Stateful<Div> {
    icon_button(id, px(24.), icon, px(13.), icon_color, hover_bg)
}

// ── Toolbar pill ─────────────────────────────────────────────────────────────

/// An icon+label toolbar control (24px tall, subtle-gray resting/hover fill).
/// `active` paints the resting highlight (open popover / toggle on). The caller
/// chains `.on_click` and the icon/label children.
pub(crate) fn toolbar_pill(id: impl Into<ElementId>, ui: UiColors, active: bool) -> Stateful<Div> {
    div()
        .id(id.into())
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(5.))
        .h(px(24.))
        .px(px(8.))
        .rounded(px(6.))
        .when(active, |d| d.bg(ui.subtle))
        .cursor_pointer()
        .text_size(BODY)
        .text_color(ui.text)
        .hover(move |s| s.bg(ui.subtle))
}

// ── Filter pill ──────────────────────────────────────────────────────────────

/// A search/filter field as a filled `ui.subtle` pill (the canonical Agents
/// look). Builds the shared anatomy - leading magnifier, the caller's
/// `TextInput` child, and an optional trailing clear (×) - and returns the
/// stateful container so the caller can layer its own `.on_key_down`
/// (Escape/Enter) and `.on_mouse_down_out` (blur) handlers.
pub(crate) fn filter_pill(
    id: impl Into<ElementId>,
    clear_id: impl Into<ElementId>,
    ui: UiColors,
    input: impl IntoElement,
    show_clear: bool,
    on_clear: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let clear_id = clear_id.into();
    let mut field = div()
        .id(id.into())
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
                .cursor_pointer()
                .text_color(ui.muted)
                .hover(move |s| s.bg(with_alpha(ui.text, 0.10)).text_color(ui.text))
                .on_click(on_clear)
                .child(
                    svg()
                        .size(px(10.))
                        .flex_none()
                        .path("icons/close.svg")
                        .text_color(ui.muted),
                ),
        );
    }
    field
}

// ── Section eyebrow ──────────────────────────────────────────────────────────

/// A section eyebrow label (11px SEMIBOLD muted). Returned as a bare `Div` so
/// the caller can chain layout (`.flex_1().min_w_0().truncate()` in a sidebar
/// list, `.flex_none()` next to a spacer).
pub(crate) fn section_eyebrow(label: impl Into<SharedString>, ui: UiColors) -> Div {
    div()
        .text_size(LABEL_SM)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ui.muted)
        .child(label.into())
}

// ── Empty / loading state ────────────────────────────────────────────────────

/// A centered panel empty/loading/onboarding state: an optional leading icon
/// (the animated `loader-circle.svg` when `animate`), an optional `title`
/// (14px), and a muted body `message` (12px). Replaces the ad-hoc `centered`
/// helpers duplicated across the diff sidebar and diff view.
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
        col = col.child(if animate {
            glyph
                .with_animation(
                    "panel-empty-spin",
                    gpui::Animation::new(std::time::Duration::from_secs(1)).repeat(),
                    |s, delta| {
                        s.with_transformation(gpui::Transformation::rotate(gpui::percentage(delta)))
                    },
                )
                .into_any_element()
        } else {
            glyph.into_any_element()
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
