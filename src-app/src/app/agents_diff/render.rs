//! Free render helpers for the Agents diff dock chrome: the resize handle, the
//! toolbar toggle button, the panel header, the files toolbar, and the
//! empty/loading/error placeholder. The body (the shared `DiffElement`) and the
//! panel orchestration live on `PaneFlowApp` in [`super`].

use std::collections::HashSet;

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Entity, FontWeight, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, svg,
};

use super::model::AgentsDiffData;
use crate::PaneFlowApp;
use crate::settings::components::with_alpha;

/// The thin, column-resize hit target straddling the panel's left border.
/// Captures the drag anchor `(cursor_x, width_at_grab)`; the actual resize math
/// runs in the Agents main area's `on_mouse_move` (a wide capture surface, so
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
        .hover(move |d| d.bg(with_alpha(ui.text, 0.06)))
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

/// Toolbar button (sibling to the environment-panel toggle) that opens the diff
/// dock. Visually identical to [`super::super::agents_view_actions`]'s list
/// toggle: a bare glyph at rest, a whisper fill on hover or while the dock is
/// open.
pub(crate) fn render_agents_diff_toggle_button(
    open: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let fill = with_alpha(ui.text, if open { 0.08 } else { 0.0 });
    let hover = with_alpha(ui.text, 0.08);
    div()
        .id("agents-env-toolbar-diff")
        .flex_none()
        .h(px(28.))
        .w(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .bg(fill)
        .hover(move |d| d.bg(hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.toggle_agents_diff_panel(event, window, cx);
        }))
        .child(
            svg()
                .size(px(16.))
                .flex_none()
                .path("icons/layout-sidebar-right.svg")
                .text_color(with_alpha(ui.text, 0.7)),
        )
        .into_any_element()
}

pub(super) fn render_diff_panel_header(
    data: &Option<AgentsDiffData>,
    folder: &str,
    cwd: String,
    split: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let loaded = data
        .as_ref()
        .is_some_and(|d| !d.loading && d.error.is_none());
    let (added, removed) = data
        .as_ref()
        .map(|d| (d.added, d.removed))
        .unwrap_or((0, 0));
    let diff = ui.diff_colors();

    let mut title_row = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_none()
                .text_size(crate::ui_primitives::BODY_EMPHASIS)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ui.text)
                .child("Changes"),
        );
    if !folder.is_empty() {
        title_row = title_row.child(
            div()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(crate::ui_primitives::BODY)
                .text_color(ui.muted)
                .child(SharedString::from(folder.to_string())),
        );
    }

    let mut right = div()
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(2.));
    if loaded {
        right = right.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .mr(px(4.))
                .text_size(crate::ui_primitives::BODY)
                .child(div().text_color(diff.added).child(format!("+{added}")))
                .child(div().text_color(diff.deleted).child(format!("-{removed}"))),
        );
        right = right.child(render_diff_split_button(split, ui, cx));
    }
    right = right
        .child(render_diff_header_icon_button(
            "agents-diff-refresh",
            "icons/refresh.svg",
            ui,
            cx.listener(move |this, _: &ClickEvent, _w, cx| {
                this.refresh_agents_diff(cwd.clone(), cx);
            }),
            ui.muted,
        ))
        .child(render_diff_header_icon_button(
            "agents-diff-close",
            "icons/close.svg",
            ui,
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_agents_diff_panel(cx);
            }),
            ui.muted,
        ));

    div()
        .h(px(48.))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(8.))
        .px(px(14.))
        .border_b_1()
        .border_color(ui.border)
        .child(title_row)
        .child(right)
        .into_any_element()
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
        .hover(move |d| d.bg(with_alpha(ui.text, 0.08)))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(on_click)
        .child(svg().size(px(15.)).flex_none().path(icon).text_color(color))
        .into_any_element()
}

/// Single toggle for the split (side-by-side) view, shown in the header once a
/// diff is loaded. The glyph is a fixed red/green two-pane image, rendered via
/// `img` because `svg` would flatten it to a monochrome mask. While split is on
/// the button wears the hover wash as a resting fill; clicking flips the mode.
fn render_diff_split_button(
    split: bool,
    ui: crate::theme::UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let rest = with_alpha(ui.text, if split { 0.08 } else { 0.0 });
    let hover = with_alpha(ui.text, 0.08);
    let icon = if split {
        "icons/diff-split.svg"
    } else {
        "icons/diff-unified.svg"
    };
    div()
        .id("agents-diff-view-split")
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .bg(rest)
        .hover(move |d| d.bg(hover))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            this.set_agents_diff_split(!split, cx);
        }))
        .child(gpui::img(icon).size(px(16.)).flex_none())
        .into_any_element()
}

/// The top "files toolbar": a muted file count on the left, a collapse-all /
/// expand-all toggle on the right. Its label + glyph flip based on whether
/// every file is already folded.
pub(super) fn render_diff_files_toolbar(
    data: &AgentsDiffData,
    collapsed: &HashSet<String>,
    ui: crate::theme::UiColors,
    entity: &Entity<PaneFlowApp>,
) -> AnyElement {
    let count = data.file_count;
    let all_collapsed = data.all_collapsed(collapsed);
    let (label, icon, next_collapse) = if all_collapsed {
        ("Expand all", "icons/chevron_down.svg", false)
    } else {
        ("Collapse all", "icons/chevron_up.svg", true)
    };
    let paths = data.paths();
    let entity = entity.clone();

    div()
        .flex_none()
        .h(px(34.))
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .border_b_1()
        .border_color(ui.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(crate::ui_primitives::LABEL_SM)
                .text_color(ui.muted)
                .child(format!("{count} file{}", if count == 1 { "" } else { "s" })),
        )
        .child(
            div()
                .id("agents-diff-collapse-all")
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(5.))
                .h(px(24.))
                .px(px(8.))
                .rounded(px(6.))
                .hover(move |d| d.bg(with_alpha(ui.text, 0.08)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(
                    move |_e: &ClickEvent, _w: &mut Window, cx: &mut gpui::App| {
                        let paths = paths.clone();
                        entity.update(cx, |this, cx| {
                            this.set_all_diff_collapsed(&paths, next_collapse, cx);
                        });
                    },
                )
                .child(
                    svg()
                        .size(px(13.))
                        .flex_none()
                        .path(icon)
                        .text_color(ui.muted),
                )
                .child(
                    div()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .text_color(ui.muted)
                        .child(label),
                ),
        )
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
