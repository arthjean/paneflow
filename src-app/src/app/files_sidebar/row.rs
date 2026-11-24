use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement,
    ParentElement, Styled, StyledText, div, img, prelude::*, px, svg,
};

use super::panel::{FilesEvent, FilesSidebar};
use super::projection::{DirState, FileRow};
use super::{DIMMED_OPACITY, INDENT_STEP, ROW_GAP, ROW_HEIGHT, ROW_SLOT};
use crate::app::files_git;
use crate::ui_primitives::AccessibleControlExt as _;
use crate::ui_primitives::{LABEL_SM, squircle_skin};

const SIDEBAR_ROW_PADDING_X: f32 = 8.0;
const SIDEBAR_ROW_LINE_HEIGHT: f32 = 18.0;
const LOADING_ICON: &str = "icons/dock/loader-circle.svg";
pub(crate) const FOLDER_FAILED: &str = "Unable to load folder.";
const FOLDER_LOADING: &str = "Loading";
const FOLDER_CHANGED: &str = "Contains changes";
pub(super) const FILES_TREE_LABEL: &str = "Files";

pub(super) fn is_dimmed(node: &crate::app::files_tree::FileNode) -> bool {
    node.is_ignored || node.is_hidden
}

pub(super) fn row_description(row: &FileRow) -> Option<&'static str> {
    match row.state {
        DirState::Failed => return Some(FOLDER_FAILED),
        DirState::Loading => return Some(FOLDER_LOADING),
        DirState::Ready => {}
    }
    let (letter, _) = files_git::status_indicator(row.status, crate::theme::ui_colors())?;
    if row.node.is_dir {
        return Some(FOLDER_CHANGED);
    }
    Some(match letter {
        "!" => "Conflicted",
        "U" => "Untracked",
        "D" => "Deleted",
        "M" => "Modified",
        "A" => "Added",
        _ => "Changed",
    })
}

pub(super) fn expose_file_row<E: StatefulInteractiveElement>(
    row_element: E,
    row: &FileRow,
    selected: bool,
) -> E {
    let element = row_element
        .role(gpui::Role::TreeItem)
        .aria_label(row.label.clone())
        .aria_level(row.depth + 1)
        .aria_selected(selected);
    let element = if row.node.is_dir {
        element.aria_expanded(row.expanded)
    } else {
        element
    };
    match row_description(row) {
        Some(description) => element.aria_description(description),
        None => element,
    }
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

        let skin = squircle_skin(div().id(row.id.clone()), group, px(6.), resting, hovered);
        let mut el = expose_file_row(skin, row, selected)
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
                    root: this.model.root().to_path_buf(),
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
            root: self.model.root().to_path_buf(),
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
        el = el.on_activate(cx.listener(move |this, _: &ClickEvent, window, cx| {
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

    fn file_row(path: &str, is_dir: bool, depth: usize, expanded: bool) -> FileRow {
        FileRow {
            node: FileNode {
                path: std::path::PathBuf::from(path),
                is_dir,
                is_ignored: false,
                is_hidden: false,
                size: 0,
            },
            depth,
            expanded,
            label: path.rsplit('/').next().unwrap_or(path).to_string().into(),
            highlights: Vec::new(),
            icon_token: "default",
            id: format!("files-row-{path}").into(),
            group: format!("files-row-group-{path}").into(),
            status: files_git::GitSummary::default(),
            state: DirState::Ready,
        }
    }

    fn exposed(row: &FileRow, selected: bool) -> gpui::accesskit::Node {
        use crate::ui_primitives::AccessibleControlExt as _;
        let element =
            expose_file_row(div().id(row.id.clone()), row, selected).on_activate(|_, _, _| {});
        let role = gpui::Element::a11y_role(&element).expect("a role");
        let mut node = gpui::accesskit::Node::new(role);
        gpui::Element::write_a11y_info(&element, &mut node);
        node
    }

    #[test]
    fn a_files_row_is_a_tree_item_named_leveled_and_expanded_when_a_folder() {
        use gpui::accesskit::{Action, Role};
        let folder = exposed(&file_row("/w/src", true, 0, true), false);
        assert_eq!(folder.role(), Role::TreeItem);
        assert_eq!(folder.label(), Some("src"));
        assert_eq!(folder.level(), Some(1));
        assert_eq!(folder.is_expanded(), Some(true));
        assert_eq!(folder.is_selected(), Some(false));
        assert!(folder.supports_action(Action::Click));
        let closed = exposed(&file_row("/w/src/app", true, 1, false), false);
        assert_eq!(closed.is_expanded(), Some(false));
        assert_eq!(closed.level(), Some(2));
        let file = exposed(&file_row("/w/src/main.rs", false, 1, false), true);
        assert_eq!(file.role(), Role::TreeItem);
        assert_eq!(file.label(), Some("main.rs"));
        assert_eq!(file.is_expanded(), None);
        assert_eq!(file.is_selected(), Some(true));
        assert_eq!(file.description(), None);
        let mut modified = file_row("/w/src/lib.rs", false, 1, false);
        modified.status.worktree.modified = 1;
        assert_eq!(exposed(&modified, false).description(), Some("Modified"));
        let mut failed = file_row("/w/locked", true, 0, false);
        failed.state = DirState::Failed;
        assert_eq!(exposed(&failed, false).description(), Some(FOLDER_FAILED));
        let tree = super::super::list::files_tree();
        let role = gpui::Element::a11y_role(&tree).expect("a role");
        let mut node = gpui::accesskit::Node::new(role);
        gpui::Element::write_a11y_info(&tree, &mut node);
        assert_eq!(role, Role::Tree);
        assert_eq!(node.label(), Some(FILES_TREE_LABEL));
    }

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
