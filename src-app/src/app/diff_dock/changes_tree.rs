use std::collections::{BTreeMap, HashSet};

use gpui::{
    AnyElement, ClickEvent, Context, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, SharedString, StatefulInteractiveElement, Styled, div, point, px, svg,
};

use super::model::DiffDockData;
use super::render::{file_icon_element, render_diff_header_icon_button};
use crate::PaneFlowApp;
use crate::app::menu_icons as icon;
use crate::diff::FileChange;
use crate::settings::components::select_item;
use crate::ui_primitives::{BODY, LABEL_SM};

const TREE_WIDTH_FRACTION: f32 = 0.4;
const TREE_MIN_WIDTH: f32 = 160.0;
const TREE_MAX_WIDTH: f32 = 256.0;
const TREE_HEADER_HEIGHT: f32 = 28.0;
const TREE_ROW_HEIGHT: f32 = 24.0;
const TREE_ROW_INSET: f32 = 6.0;
const TREE_INDENT: f32 = 12.0;
const TREE_ICON: f32 = 14.0;
const TREE_CHEVRON: f32 = 12.0;
const FOLDER_OPEN_ICON: &str = "icons/dock/arrow-down-01.svg";
const FOLDER_FOLDED_ICON: &str = "icons/dock/arrow-right-01.svg";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TreeRowKind {
    Folder { folded: bool },
    File { change: FileChange },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TreeRow {
    pub(super) depth: usize,
    pub(super) path: String,
    pub(super) label: String,
    pub(super) kind: TreeRowKind,
}

#[derive(Default)]
struct TreeNode {
    folders: BTreeMap<String, TreeNode>,
    files: BTreeMap<String, (String, FileChange)>,
}

pub(super) fn changes_tree_width(panel_width: f32) -> f32 {
    (panel_width * TREE_WIDTH_FRACTION).clamp(TREE_MIN_WIDTH, TREE_MAX_WIDTH)
}

pub(super) fn tree_rows(files: &[(String, FileChange)], folded: &HashSet<String>) -> Vec<TreeRow> {
    let mut root = TreeNode::default();
    for (path, change) in files {
        let mut segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let Some(name) = segments.pop() else {
            continue;
        };
        let mut node = &mut root;
        for segment in segments {
            node = node.folders.entry(segment.to_string()).or_default();
        }
        node.files.insert(name.to_string(), (path.clone(), *change));
    }
    let mut rows = Vec::new();
    push_tree_rows(&root, "", 0, folded, &mut rows);
    rows
}

fn push_tree_rows(
    node: &TreeNode,
    prefix: &str,
    depth: usize,
    folded: &HashSet<String>,
    rows: &mut Vec<TreeRow>,
) {
    for (name, child) in &node.folders {
        let mut label = name.clone();
        let mut folder = child;
        while folder.files.is_empty() && folder.folders.len() == 1 {
            let Some((next_name, next)) = folder.folders.iter().next() else {
                break;
            };
            label = format!("{label}/{next_name}");
            folder = next;
        }
        let path = if prefix.is_empty() {
            label.clone()
        } else {
            format!("{prefix}/{label}")
        };
        let is_folded = folded.contains(&path);
        rows.push(TreeRow {
            depth,
            path: path.clone(),
            label,
            kind: TreeRowKind::Folder { folded: is_folded },
        });
        if !is_folded {
            push_tree_rows(folder, &path, depth + 1, folded, rows);
        }
    }
    for (name, (path, change)) in &node.files {
        rows.push(TreeRow {
            depth,
            path: path.clone(),
            label: name.clone(),
            kind: TreeRowKind::File { change: *change },
        });
    }
}

pub(super) fn folder_paths(files: &[(String, FileChange)]) -> Vec<String> {
    tree_rows(files, &HashSet::new())
        .into_iter()
        .filter(|row| matches!(row.kind, TreeRowKind::Folder { .. }))
        .map(|row| row.path)
        .collect()
}

fn change_mark(change: FileChange, ui: crate::theme::UiColors) -> (&'static str, Hsla) {
    match change {
        FileChange::Added => ("A", ui.vc_added),
        FileChange::Modified => ("M", ui.vc_modified),
        FileChange::Deleted => ("D", ui.vc_deleted),
        FileChange::Renamed => ("R", ui.vc_modified),
        FileChange::TypeChanged => ("T", ui.vc_modified),
    }
}

fn change_name(change: FileChange) -> &'static str {
    match change {
        FileChange::Added => "added",
        FileChange::Modified => "modified",
        FileChange::Deleted => "deleted",
        FileChange::Renamed => "renamed",
        FileChange::TypeChanged => "type changed",
    }
}

impl PaneFlowApp {
    pub(crate) fn set_diff_changes_tree_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.diff_dock.changes_tree_open != open {
            self.diff_dock.changes_tree_open = open;
            cx.notify();
        }
    }

    fn toggle_diff_changes_tree_folder(&mut self, path: String, cx: &mut Context<Self>) {
        if !self.diff_dock.changes_tree_folded.remove(&path) {
            self.diff_dock.changes_tree_folded.insert(path);
        }
        cx.notify();
    }

    fn set_diff_changes_tree_folded(
        &mut self,
        folders: Vec<String>,
        folded: bool,
        cx: &mut Context<Self>,
    ) {
        self.diff_dock.changes_tree_folded.clear();
        if folded {
            self.diff_dock.changes_tree_folded.extend(folders);
        }
        cx.notify();
    }

    pub(super) fn reveal_diff_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self.diff_dock.collapsed.remove(&path) {
            self.recompute_diff_dock_display();
        }
        let split = self.diff_dock.split;
        let offset = self.diff_dock.data.as_ref().and_then(|data| {
            let (anchors, offsets) = if split {
                (&data.disp_anchors_split, &data.disp_split_offsets)
            } else {
                (&data.disp_anchors_unified, &data.disp_unified_offsets)
            };
            anchors
                .iter()
                .find(|(anchor, _)| *anchor == path)
                .and_then(|(_, row)| offsets.get(*row).copied())
        });
        if let Some(offset) = offset {
            self.diff_dock.scroll.set_offset(point(px(0.), -px(offset)));
        }
        self.diff_dock.changes_tree_selected = Some(path);
        cx.notify();
    }

    pub(super) fn render_diff_changes_tree(
        &self,
        data: &Option<DiffDockData>,
        panel_width: f32,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.diff_dock.changes_tree_open {
            return None;
        }
        let data = data.as_ref().filter(|data| data.has_rows())?;
        let files: Vec<(String, FileChange)> = data
            .files_full
            .iter()
            .map(|file| (file.path.clone(), file.change))
            .collect();
        if files.is_empty() {
            return None;
        }
        let rows = tree_rows(&files, &self.diff_dock.changes_tree_folded);
        let folders = folder_paths(&files);
        let all_folded = !folders.is_empty()
            && folders
                .iter()
                .all(|folder| self.diff_dock.changes_tree_folded.contains(folder));
        let selected = self.diff_dock.changes_tree_selected.as_deref();

        let mut header = div()
            .flex_none()
            .h(px(TREE_HEADER_HEIGHT))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.))
            .pl(px(10.))
            .pr(px(2.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(BODY)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child("Files"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(LABEL_SM)
                    .text_color(ui.muted)
                    .child(files.len().to_string()),
            );
        if folders.is_empty() {
            header = header.pr(px(10.));
        } else {
            header = header.child(render_diff_header_icon_button(
                "diff-changes-tree-fold-all",
                if all_folded {
                    "Expand all folders"
                } else {
                    "Collapse all folders"
                },
                if all_folded {
                    icon::EXPAND_ALL
                } else {
                    icon::COLLAPSE_ALL
                },
                cx.listener(move |this, _: &ClickEvent, _w, cx| {
                    this.set_diff_changes_tree_folded(folders.clone(), !all_folded, cx);
                }),
                ui.muted,
            ));
        }

        let row_elements = rows
            .into_iter()
            .enumerate()
            .map(|(index, row)| {
                let indent = TREE_ROW_INSET + row.depth as f32 * TREE_INDENT;
                let is_selected = matches!(row.kind, TreeRowKind::File { .. })
                    && selected == Some(row.path.as_str());
                let base = select_item(
                    SharedString::from(format!("diff-changes-tree-row-{index}")),
                    is_selected,
                    ui,
                )
                .w_full()
                .h(px(TREE_ROW_HEIGHT))
                .gap(px(6.))
                .pl(px(indent))
                .pr(px(TREE_ROW_INSET))
                .text_size(BODY)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
                let path = row.path.clone();
                match row.kind {
                    TreeRowKind::Folder { folded } => base
                        .role(gpui::accesskit::Role::Button)
                        .aria_label(format!("Folder {}", row.label))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                            this.toggle_diff_changes_tree_folder(path.clone(), cx);
                        }))
                        .child(
                            svg()
                                .size(px(TREE_CHEVRON))
                                .flex_none()
                                .path(if folded {
                                    FOLDER_FOLDED_ICON
                                } else {
                                    FOLDER_OPEN_ICON
                                })
                                .text_color(ui.muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(ui.muted)
                                .child(row.label),
                        ),
                    TreeRowKind::File { change } => {
                        let (mark, mark_color) = change_mark(change, ui);
                        base.role(gpui::accesskit::Role::Button)
                            .aria_label(format!("{}, {}", row.label, change_name(change)))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                                this.reveal_diff_file(path.clone(), cx);
                            }))
                            .child(file_icon_element(
                                crate::file_icons::language_icon_path(&row.label),
                                px(TREE_ICON),
                                ui.muted,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(ui.text)
                                    .child(row.label),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(LABEL_SM)
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(mark_color)
                                    .child(mark),
                            )
                    }
                }
            })
            .collect::<Vec<_>>();

        Some(
            div()
                .flex_none()
                .w(px(changes_tree_width(panel_width)))
                .h_full()
                .flex()
                .flex_col()
                .border_l_1()
                .border_color(ui.border)
                .child(header)
                .child(
                    div()
                        .id("diff-changes-tree")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&self.diff_dock.changes_tree_scroll)
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .px(px(4.))
                        .pb(px(4.))
                        .children(row_elements),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(paths: &[&str]) -> Vec<(String, FileChange)> {
        paths
            .iter()
            .map(|path| (path.to_string(), FileChange::Modified))
            .collect()
    }

    fn labels(rows: &[TreeRow]) -> Vec<(usize, &str)> {
        rows.iter()
            .map(|row| (row.depth, row.label.as_str()))
            .collect()
    }

    #[test]
    fn folders_come_before_the_files_beside_them() {
        let rows = tree_rows(
            &files(&["README.md", "src/main.rs", "src/app/mod.rs", "src/lib.rs"]),
            &HashSet::new(),
        );
        assert_eq!(
            labels(&rows),
            vec![
                (0, "src"),
                (1, "app"),
                (2, "mod.rs"),
                (1, "lib.rs"),
                (1, "main.rs"),
                (0, "README.md"),
            ]
        );
    }

    #[test]
    fn a_chain_of_single_folders_is_one_row() {
        let rows = tree_rows(
            &files(&["src-app/src/app/diff_dock/mod.rs", "Cargo.toml"]),
            &HashSet::new(),
        );
        assert_eq!(
            labels(&rows),
            vec![
                (0, "src-app/src/app/diff_dock"),
                (1, "mod.rs"),
                (0, "Cargo.toml"),
            ]
        );
        assert_eq!(rows[0].path, "src-app/src/app/diff_dock");
        assert_eq!(rows[1].path, "src-app/src/app/diff_dock/mod.rs");
    }

    #[test]
    fn a_folded_folder_hides_its_rows_and_keeps_its_own() {
        let listed = files(&["src/a.rs", "src/deep/b.rs", "top.rs"]);
        let folded = HashSet::from(["src".to_string()]);
        let rows = tree_rows(&listed, &folded);
        assert_eq!(labels(&rows), vec![(0, "src"), (0, "top.rs")]);
        assert_eq!(rows[0].kind, TreeRowKind::Folder { folded: true });
        assert_eq!(folder_paths(&listed), vec!["src", "src/deep"]);
    }

    #[test]
    fn a_file_row_keeps_its_change_kind() {
        let rows = tree_rows(
            &[
                ("new.rs".to_string(), FileChange::Added),
                ("old.rs".to_string(), FileChange::Deleted),
            ],
            &HashSet::new(),
        );
        assert_eq!(
            rows[0].kind,
            TreeRowKind::File {
                change: FileChange::Added
            }
        );
        assert_eq!(
            rows[1].kind,
            TreeRowKind::File {
                change: FileChange::Deleted
            }
        );
    }

    #[test]
    fn the_tree_takes_two_fifths_of_the_dock_within_its_limits() {
        assert_eq!(changes_tree_width(540.), 216.);
        assert_eq!(changes_tree_width(360.), TREE_MIN_WIDTH);
        assert_eq!(changes_tree_width(1400.), TREE_MAX_WIDTH);
    }
}
