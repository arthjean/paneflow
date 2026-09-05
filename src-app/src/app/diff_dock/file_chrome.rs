use std::path::{Component, Path};

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, MouseButton, ParentElement,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder, px, svg,
};

use crate::PaneFlowApp;
use crate::theme::UiColors;
use crate::ui_primitives::AnimatedHoverExt;

fn project_name(root: &str) -> String {
    Path::new(root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string())
}

pub(super) fn render_tree_toggle(
    open: bool,
    ui: UiColors,
    cx: &mut Context<PaneFlowApp>,
) -> AnyElement {
    let hover = crate::app::constants::sidebar_tab_hover_background();
    div()
        .id("diff-dock-files-toggle")
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .animated_hover_bg(
            if open {
                hover
            } else {
                gpui::transparent_black()
            },
            hover,
        )
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.handle_toggle_files_sidebar(&crate::ToggleFilesSidebar, window, cx);
            cx.stop_propagation();
        }))
        .child(
            svg()
                .path("icons/folders.svg")
                .size(px(16.))
                .text_color(if open { ui.text } else { ui.muted }),
        )
        .into_any_element()
}

pub(super) fn render_file_breadcrumbs(root: &str, path: &str, ui: UiColors) -> AnyElement {
    let mut parts = Vec::new();
    if !root.is_empty() && !Path::new(path).is_absolute() {
        parts.push(project_name(root));
    }
    parts.extend(
        Path::new(path)
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            }),
    );
    let count = parts.len();
    div()
        .flex_1()
        .min_w_0()
        .h(px(40.))
        .flex()
        .items_center()
        .px(px(16.))
        .gap(px(4.))
        .overflow_hidden()
        .text_size(px(12.))
        .text_color(ui.muted)
        .children(parts.into_iter().enumerate().map(|(index, part)| {
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(4.))
                .when(index > 0, |crumb| {
                    crumb.child(
                        svg()
                            .path("icons/chevron-right.svg")
                            .size(px(12.))
                            .flex_none(),
                    )
                })
                .child(
                    div()
                        .truncate()
                        .when(index + 1 == count, |label| label.text_color(ui.text))
                        .child(part),
                )
        }))
        .into_any_element()
}
