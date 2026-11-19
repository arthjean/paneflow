use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Styled, StyledText, div, img, prelude::*, px, svg,
};

use super::panel::{FilesEvent, FilesSidebar};
use super::projection::{DirState, FileRow};
use super::{DIMMED_OPACITY, INDENT_STEP, ROW_GAP, ROW_HEIGHT, ROW_SLOT};
use crate::app::files_git;
use crate::ui_primitives::{LABEL_SM, squircle_skin};

const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
const SIDEBAR_ROW_LINE_HEIGHT: f32 = 18.0;
const LOADING_ICON: &str = "icons/dock/loader-circle.svg";
pub(crate) const FOLDER_FAILED: &str = "Unable to load folder.";

pub(super) fn is_dimmed(node: &crate::app::files_tree::FileNode) -> bool {
    node.is_ignored || node.is_hidden
}

pub(super) fn row_label_color(
    status: files_git::GitSummary,
    ui: crate::theme::UiColors,
) -> gpui::Hsla {
    files_git::label_color(status, ui).unwrap_or(ui.text)
}

impl FilesSidebar {
    pub(super) fn files_row(
        &self,
        row: &FileRow,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let node = &row.node;
        let dimmed = is_dimmed(node);
        let text_color = row_label_color(row.status, ui);
        let indicator = files_git::status_indicator(row.status, ui);
        let indent = px(SIDEBAR_ROW_PADDING_X + row.depth as f32 * INDENT_STEP);
        let path = node.path.clone();
        let is_dir = node.is_dir;
        let group = row.group.clone();
        let (resting, hovered) = if selected {
            (
                Some(crate::app::constants::sidebar_tab_active_background()),
                None,
            )
        } else {
            (
                None,
                Some(crate::app::constants::sidebar_tab_hover_background()),
            )
        };

        let loading = row.state == DirState::Loading;
        let failed = row.state == DirState::Failed;
        let slot = if loading {
            let spinner = crate::ui_primitives::spinning_icon(LOADING_ICON, px(ROW_SLOT), ui.muted);
            #[cfg(test)]
            let spinner = div()
                .flex_none()
                .debug_selector(|| format!("files-row-loading-{}", row.node.path.display()))
                .child(spinner)
                .into_any_element();
            spinner
        } else if is_dir {
            svg()
                .size(px(ROW_SLOT))
                .flex_none()
                .path(if row.expanded {
                    "icons/dock/arrow-down-01.svg"
                } else {
                    "icons/dock/arrow-right-01.svg"
                })
                .text_color(ui.muted)
                .into_any_element()
        } else {
            img(crate::file_icons::icon_path_for(
                row.icon_token,
                crate::file_icons::IconVariant::for_ui(ui),
            ))
            .size(px(ROW_SLOT))
            .flex_none()
            .into_any_element()
        };

        let guide_color = ui.text.opacity(0.08);
        let guides = (0..row.depth)
            .map(|level| {
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(SIDEBAR_ROW_PADDING_X
                        + level as f32 * INDENT_STEP
                        + (ROW_SLOT / 2.).floor()))
                    .w(px(1.))
                    .bg(guide_color)
                    .into_any_element()
            })
            .collect::<Vec<_>>();

        let mut el = squircle_skin(div().id(row.id.clone()), group, px(6.), resting, hovered)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(ROW_GAP))
            .h(ROW_HEIGHT)
            .w_full()
            .flex_none()
            .overflow_x_hidden()
            .pl(indent)
            .pr(px(SIDEBAR_ROW_PADDING_X))
            .when(dimmed, |s| s.opacity(DIMMED_OPACITY))
            .children(guides);

        #[cfg(test)]
        {
            el = el.debug_selector(|| row.id.to_string());
        }

        let menu_path = path.clone();
        el = el.on_aux_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
            if this.active
                && e.is_right_click()
                && let Some(position) = e.mouse_position()
            {
                this.focus.focus(window, cx);
                this.select_path(&menu_path, cx);
                cx.emit(FilesEvent::ContextMenu(crate::FilesContextMenu {
                    root: this.tree.root.clone(),
                    path: menu_path.clone(),
                    position,
                    scope: crate::app::window::FilesMenuScope::Full,
                }));
                cx.stop_propagation();
                cx.notify();
            }
        }));

        let drag = crate::app::drag::FileTreeDrag {
            path: path.clone(),
            root: self.tree.root.clone(),
            label: row.label.clone(),
            icon: (!is_dir).then(|| {
                crate::file_icons::icon_path_for(
                    row.icon_token,
                    crate::file_icons::IconVariant::for_ui(ui),
                )
                .into()
            }),
        };
        el = el.on_drag(drag, |drag, _offset, _window, cx| cx.new(|_| drag.clone()));

        let click_path = path.clone();
        el = el.on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.focus.focus(window, cx);
            this.activate_path(&click_path, is_dir, window, cx);
            cx.stop_propagation();
        }));

        let name = if row.highlights.is_empty() {
            row.label.clone().into_any_element()
        } else {
            let style = HighlightStyle {
                color: Some(ui.accent),
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            };
            StyledText::new(row.label.clone())
                .with_highlights(row.highlights.iter().map(|range| (range.clone(), style)))
                .into_any_element()
        };

        let el = el
            .child(slot)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(13.))
                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                    .text_color(text_color)
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(name),
            )
            .when(failed, |el| {
                let notice = div()
                    .flex_none()
                    .text_size(LABEL_SM)
                    .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                    .text_color(ui.muted)
                    .child(FOLDER_FAILED);
                #[cfg(test)]
                let notice = notice
                    .debug_selector(|| format!("files-row-failed-{}", row.node.path.display()));
                el.child(notice)
            })
            .when_some(indicator.filter(|_| !failed), |el, (letter, color)| {
                el.child(
                    div()
                        .flex_none()
                        .w(px(ROW_SLOT))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(if is_dir {
                            div()
                                .size(px(6.))
                                .rounded_full()
                                .bg(color.opacity(0.5))
                                .into_any_element()
                        } else {
                            div()
                                .text_size(LABEL_SM)
                                .line_height(px(SIDEBAR_ROW_LINE_HEIGHT))
                                .font_weight(FontWeight::BOLD)
                                .text_color(color)
                                .child(letter)
                                .into_any_element()
                        }),
                )
            });

        el.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::files_tree::FileNode;

    #[test]
    fn a_retired_binary_extension_row_is_neither_dimmed_nor_muted() {
        let ui = crate::theme::ui_colors_with(&crate::theme::paneflow_dark());
        for name in ["logo.png", "app.wasm", "dump.zip", "font.woff2", "huge.log"] {
            let node = FileNode {
                path: std::path::PathBuf::from(format!("/w/{name}")),
                is_dir: false,
                is_ignored: false,
                is_hidden: false,
                size: 512 * 1024 * 1024,
            };
            assert!(!is_dimmed(&node), "{name}");
            assert_eq!(
                row_label_color(files_git::GitSummary::default(), ui),
                ui.text,
                "{name}"
            );
        }
    }
}
