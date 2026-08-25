//! Free render helpers for the Agents diff dock chrome: the resize handle, the
//! toolbar toggle button, the tab strip, the files toolbar, and the
//! empty/loading/error placeholder. The body (the shared `DiffElement`) and the
//! panel orchestration live on `PaneFlowApp` in [`super`].

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px, svg,
};

use super::model::{DiffChrome, DiffDockTab};
use super::new_tab_menu::render_diff_new_tab_menu;
use super::options_menu::render_diff_options_button;
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;
use crate::ui_primitives::AnimatedHoverExt;

/// The thin, column-resize hit target straddling the panel's left border.
/// Captures the drag anchor `(cursor_x, width_at_grab)`; the actual resize math
/// runs in the CLI dock wrapper's `on_mouse_move` (a wide capture surface, so
/// the drag survives the cursor leaving the dock).
pub(super) fn render_diff_resize_handle(
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    div()
        .id("agents-diff-resize")
        .absolute()
        .left(px(-3.))
        .top_0()
        .bottom_0()
        .w(px(7.))
        .cursor(CursorStyle::ResizeLeftRight)
        .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.06))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &MouseDownEvent, _w, cx| {
                let w = this.agents_view.agents_diff_width;
                this.agents_view.agents_diff_resize = Some((f32::from(event.position.x), w));
                cx.notify();
            }),
        )
        .into_any_element()
}

/// The dock's tab strip: the permanent "Changes" diff tab, then one tab per
/// terminal opened from the trailing `+` (which opens the surface picker in
/// [`super::new_tab_menu`]). The dock's own close button is pinned right, so it
/// stays reachable from every tab.
pub(super) fn render_diff_tab_strip(
    tabs: &[DiffDockTab],
    active: usize,
    new_tab_menu_open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let mut strip = div()
        .h(px(40.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.))
        .px(px(8.))
        .border_b_1()
        .border_color(ui.border);

    for (index, tab) in tabs.iter().enumerate() {
        strip = strip.child(render_diff_tab(tab, index, index == active, ui, cx));
    }

    // Toggle off the render-time snapshot, not the live flag: the open menu's
    // `on_mouse_up_out` fires on this same release and has already cleared it,
    // so a live toggle would re-open the menu on every second press.
    let open = new_tab_menu_open;
    let new_tab_rest = with_alpha(ui.text, if open { 0.08 } else { 0.0 });

    strip
        .child(
            div()
                .id("agents-diff-tab-new")
                .relative()
                .flex_none()
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor(CursorStyle::PointingHand)
                .bg(new_tab_rest)
                .animated_hover_bg(new_tab_rest, with_alpha(ui.text, 0.08))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.toggle_diff_new_tab_menu(!open, cx);
                }))
                .child(
                    svg()
                        .size(px(14.))
                        .flex_none()
                        .path("icons/plus.svg")
                        .text_color(ui.muted),
                )
                .when(open, |trigger| {
                    trigger.child(render_diff_new_tab_menu(ui, cx))
                }),
        )
        .child(div().flex_1().min_w_0())
        .child(render_diff_header_icon_button(
            "agents-diff-close",
            "icons/close.svg",
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_agents_diff_panel(cx);
            }),
            ui.muted,
        ))
        .into_any_element()
}

/// One tab chip. The active one carries the raised fill and hairline; the rest
/// stay flat until hovered. Terminal tabs get a trailing close button; the
/// `Changes` tab is permanent and has none.
fn render_diff_tab(
    tab: &DiffDockTab,
    index: usize,
    active: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let (icon, label) = match tab {
        DiffDockTab::Changes => ("icons/plus-minus.svg", "Changes"),
        DiffDockTab::Terminal(_) => ("icons/terminal.svg", "Terminal"),
    };
    let rest = with_alpha(ui.text, if active { 0.05 } else { 0.0 });
    let hover = with_alpha(ui.text, if active { 0.05 } else { 0.04 });
    let text = if active { ui.text } else { ui.muted };

    let mut chip = div()
        .id(SharedString::from(format!("agents-diff-tab-{index}")))
        .flex_none()
        .h(px(26.))
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(9.))
        .rounded(px(7.))
        .cursor(CursorStyle::PointingHand)
        .bg(rest)
        .border_1()
        .border_color(if active {
            ui.border
        } else {
            gpui::transparent_black()
        })
        .animated_hover_bg(rest, hover)
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.select_diff_tab(index, cx);
        }))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(if active { ui.muted } else { text }),
        )
        .child(
            div()
                .flex_none()
                .whitespace_nowrap()
                .text_size(crate::ui_primitives::BODY)
                .font_weight(FontWeight::MEDIUM)
                .text_color(text)
                .child(label),
        );

    if matches!(tab, DiffDockTab::Terminal(_)) {
        chip = chip.child(
            div()
                .id(SharedString::from(format!("agents-diff-tab-close-{index}")))
                .flex_none()
                .size(px(16.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.10))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.close_diff_tab(index, cx);
                }))
                .child(
                    svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/close.svg")
                        .text_color(ui.muted),
                ),
        );
    }

    chip.into_any_element()
}

fn render_diff_header_icon_button(
    id: &'static str,
    icon: &'static str,
    ui: crate::theme::UiColors,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    color: Hsla,
) -> AnyElement {
    div()
        .id(id)
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .animated_hover_bg(with_alpha(ui.text, 0.0), with_alpha(ui.text, 0.08))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(svg().size(px(15.)).flex_none().path(icon).text_color(color))
        .into_any_element()
}

/// The summary row under the tab strip, shown with the `Changes` tab: the scope
/// ("Uncommitted" plus its +/- totals), the branch chip, then the overflow menu
/// pushed to the right edge.
pub(super) fn render_diff_files_toolbar(
    chrome: &DiffChrome<'_>,
    branch_chip: Option<AnyElement>,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = chrome
        .data
        .as_ref()
        .filter(|d| !d.loading && d.error.is_none());
    let diff = ui.diff_colors();

    let mut row = div()
        .flex_none()
        .h(px(36.))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .border_b_1()
        .border_color(ui.border)
        .child(
            svg()
                .size(px(14.))
                .flex_none()
                .path("icons/file-text.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_none()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.text)
                .child("Uncommitted"),
        );

    if let Some(data) = loaded {
        row = row
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(diff.added)
                    .child(format!("+{}", data.added)),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(diff.deleted)
                    .child(format!("-{}", data.removed)),
            );
    }

    if let Some(chip) = branch_chip {
        row = row.child(chip);
    }

    row.child(div().flex_1().min_w_0())
        .child(render_diff_options_button(chrome, ui, cx))
        .into_any_element()
}

pub(super) fn diff_panel_centered(
    icon: &'static str,
    label: impl Into<String>,
    ui: crate::theme::UiColors,
) -> AnyElement {
    crate::ui_primitives::panel_empty_state(
        ui,
        Some(icon),
        None,
        label.into(),
        icon == "icons/loader-circle.svg",
    )
    .into_any_element()
}
