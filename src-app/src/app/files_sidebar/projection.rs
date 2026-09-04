use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::SharedString;

use crate::app::files_tree::{self, FileNode, FilesTreeState};

#[derive(Clone, Debug)]
pub(super) struct FileRow {
    pub node: FileNode,
    pub depth: usize,
    pub expanded: bool,
    pub label: SharedString,
    pub highlight: Option<Range<usize>>,
    pub icon: &'static str,
    pub id: SharedString,
    pub group: SharedString,
}

impl FileRow {
    fn new(
        node: &FileNode,
        depth: usize,
        expanded: bool,
        label: String,
        highlight: Option<Range<usize>>,
    ) -> Self {
        Self {
            node: node.clone(),
            depth,
            expanded,
            label: label.into(),
            highlight,
            icon: crate::file_icons::language_icon_path(&files_tree::node_name(node)),
            id: format!("files-row-{}", node.path.display()).into(),
            group: format!("files-row-group-{}", node.path.display()).into(),
        }
    }
}

#[derive(Default)]
pub(super) struct FilesProjection {
    pub rows: Vec<FileRow>,
    indices: HashMap<PathBuf, usize>,
}

impl FilesProjection {
    pub fn build(tree: &FilesTreeState, expanded: &HashSet<PathBuf>, query: &str) -> Self {
        let rows: Vec<FileRow> = if query.is_empty() {
            files_tree::flatten_visible_refs(&tree.root, expanded, &tree.children)
                .into_iter()
                .map(|row| {
                    FileRow::new(
                        row.node,
                        row.depth,
                        row.expanded,
                        files_tree::node_name(row.node),
                        None,
                    )
                })
                .collect()
        } else {
            super::filter::filter_rows(&tree.root, &tree.children, query)
                .into_iter()
                .map(|row| FileRow::new(row.node, 0, false, row.rel, row.highlight))
                .collect()
        };
        let indices = rows
            .iter()
            .enumerate()
            .map(|(ix, row)| (row.node.path.clone(), ix))
            .collect();
        Self { rows, indices }
    }

    pub fn index(&self, path: &Path) -> Option<usize> {
        self.indices.get(path).copied()
    }

    pub fn reconcile_selection(
        &self,
        selected: Option<&Path>,
        previous_index: usize,
    ) -> Option<PathBuf> {
        if let Some(path) = selected {
            for candidate in path.ancestors() {
                if self.indices.contains_key(candidate) {
                    return Some(candidate.to_path_buf());
                }
            }
        }
        self.rows
            .get(previous_index.min(self.rows.len().saturating_sub(1)))
            .map(|row| row.node.path.clone())
    }
}
