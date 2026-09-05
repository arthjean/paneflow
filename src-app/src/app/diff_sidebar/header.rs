use crate::PaneFlowApp;
use crate::diff::DiffView;
use crate::theme::UiColors;
use crate::ui_primitives::TooltipDelayExt;
use crate::ui_primitives::{AnimatedHover, AnimatedHoverExt};
use gpui::{
    AnyElement, ClickEvent, Context, Entity, Focusable, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, Role, SharedString, StatefulInteractiveElement,
    Styled, Window, div, px, svg,
};

use super::REVIEW_SIDEBAR_ROW_RADIUS;
use crate::settings::components::{menu_surface, select_item};

const CONTROLS_ROW_HEIGHT: f32 = 32.0;
const BASE_PICKER_MAX_ROWS: usize = 12;

fn header_icon_button(
    id: &'static str,
    icon: &'static str,
    tooltip: &'static str,
    icon_color: Hsla,
    row_background: Hsla,
) -> AnimatedHover {
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(tooltip)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(28.))
        .rounded(px(REVIEW_SIDEBAR_ROW_RADIUS))
        .animated_hover_bg(row_background.opacity(0.0), row_background)
        .delayed_tooltip(crate::ui_primitives::text_tooltip(tooltip))
        .child(
            svg()
                .size(px(13.))
                .flex_none()
                .path(icon)
                .text_color(icon_color),
        )
}

impl PaneFlowApp {
    pub(super) fn render_diff_files_header(
        &self,
        ui: UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row_background = crate::app::constants::sidebar_tab_active_background();
        let tree_tooltip = if self.review.files_tree {
            "Show flat list"
        } else {
            "Show file tree"
        };
        let tree_icon = if self.review.files_tree {
            "icons/list.svg"
        } else {
            "icons/file_tree.svg"
        };

        div()
            .id("diff-files-header")
            .flex_none()
            .h(px(36.))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .child(
                div()
                    .pl(px(8.))
                    .text_size(px(13.))
                    .text_color(ui.muted)
                    .child("Changes"),
            )
            .child(
                header_icon_button(
                    "diff-files-tree-toggle",
                    tree_icon,
                    tree_tooltip,
                    ui.muted,
                    row_background,
                )
                .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.review.files_tree = !this.review.files_tree;
                    cx.stop_propagation();
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    pub(super) fn render_diff_controls(
        &self,
        view: &Entity<DiffView>,
        ui: UiColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base = view.read(cx).base_ref().to_string();
        let base_open = self.review.base_picker_open;
        let base_label = if base.is_empty() {
            "Choose base".to_string()
        } else {
            format!("vs {base}")
        };

        let base_chip = crate::ui_primitives::toolbar_pill("diff-base-chip", ui, base_open)
            .delayed_tooltip(crate::ui_primitives::text_tooltip(
                "Base branch the diff is computed against",
            ))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                let open = !this.review.base_picker_open;
                this.review.dismiss_popovers();
                this.review.base_picker_open = open;
                if open {
                    this.review
                        .base_filter
                        .update(cx, |input, cx| input.clear(cx));
                    let focus = this.review.base_filter.read(cx).focus_handle.clone();
                    window.focus(&focus, cx);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/git-pull-request.svg")
                    .text_color(ui.muted),
            )
            .child(div().min_w_0().max_w(px(150.)).truncate().child(base_label))
            .child(
                svg()
                    .size(px(10.))
                    .flex_none()
                    .path("icons/chevron-down.svg")
                    .text_color(ui.muted),
            );

        let mut row = div()
            .id("diff-controls-row")
            .relative()
            .flex_none()
            .h(px(CONTROLS_ROW_HEIGHT))
            .px(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .child(base_chip);

        if base_open {
            row = row.child(self.render_base_picker(view, &base, ui, window, cx));
        }
        row.into_any_element()
    }

    fn render_base_picker(
        &self,
        view: &Entity<DiffView>,
        current: &str,
        ui: UiColors,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter_lc = self.review.base_filter.read(cx).value().to_lowercase();
        let (matches, total): (Vec<String>, usize) = {
            let view_ref = view.read(cx);
            let indices = view_ref.matching_branches(&filter_lc);
            let total = indices.len();
            let matches = indices
                .into_iter()
                .take(BASE_PICKER_MAX_ROWS)
                .filter_map(|index| view_ref.branches().get(index).cloned())
                .collect();
            (matches, total)
        };
        let submit_view = view.clone();
        let filter_field = crate::ui_primitives::filter_pill_with_arrow_clear(
            "diff-base-filter",
            "diff-base-filter-clear",
            ui,
            self.review.base_filter.clone(),
            !filter_lc.is_empty(),
            cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.review.base_filter.update(cx, |inp, cx| inp.clear(cx));
            }),
        )
        .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| {
            match ev.keystroke.key.as_str() {
                "escape" => {
                    this.review.base_picker_open = false;
                    if let Some(view) = this.review_focused_view(cx) {
                        view.read(cx).focus_handle(cx).focus(window, cx);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
                "enter" => {
                    let raw = this.review.base_filter.read(cx).value();
                    let raw = raw.trim().to_string();
                    if raw.is_empty() {
                        return;
                    }
                    let picked = submit_view
                        .read(cx)
                        .first_matching_branch(&raw.to_lowercase())
                        .unwrap_or(raw);
                    submit_view.update(cx, |view, cx| view.resolve_and_set_base(picked, cx));
                    this.review.base_picker_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }
                _ => {}
            }
        }));

        let mut list = div()
            .id("diff-base-list")
            .flex()
            .flex_col()
            .gap(px(1.))
            .max_h(px(280.))
            .overflow_y_scroll();
        if matches.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(crate::ui_primitives::LABEL_SM)
                    .text_color(ui.muted)
                    .child(if filter_lc.is_empty() {
                        "No branches found. Type a ref and press Enter."
                    } else {
                        "No match. Press Enter to use it as a ref."
                    }),
            );
        }
        for (index, branch) in matches.into_iter().enumerate() {
            let selected = branch == current;
            let pick_view = view.clone();
            let picked = branch.clone();
            list = list.child(
                select_item(
                    SharedString::from(format!("diff-base-option-{index}")),
                    selected,
                    ui,
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    pick_view.update(cx, |view, cx| view.set_base(picked.clone(), cx));
                    this.review.base_picker_open = false;
                    cx.stop_propagation();
                    cx.notify();
                }))
                .child(
                    svg()
                        .size(px(11.))
                        .flex_none()
                        .path("icons/git-branch-sidebar.svg")
                        .text_color(if selected { ui.accent } else { ui.muted }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ui.text)
                        .child(branch),
                ),
            );
        }
        if total > BASE_PICKER_MAX_ROWS {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(4.))
                    .text_size(crate::ui_primitives::LABEL_XS)
                    .text_color(ui.muted)
                    .child(format!(
                        "{} more, keep typing to narrow",
                        total - BASE_PICKER_MAX_ROWS
                    )),
            );
        }

        menu_surface(div().id("diff-base-picker"), ui)
            .occlude()
            .absolute()
            .top(px(CONTROLS_ROW_HEIGHT))
            .left(px(8.))
            .w(px(268.))
            .flex()
            .flex_col()
            .gap(px(4.))
            .p(px(6.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.review.base_picker_open = false;
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(filter_field)
            .child(list)
            .into_any_element()
    }
}
