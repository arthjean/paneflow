use std::collections::{BTreeMap, HashMap};
use std::ops::Range;
use std::path::{Path, PathBuf};

use gpui::SharedString;

use super::model::RowRef;
use super::search::SearchHit;
use crate::app::files_git::{GitStatuses, GitSummary};
use crate::app::files_tree::{self, FileNode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DirState {
    Ready,
    Loading,
    Failed,
}

#[derive(Clone, Debug)]
pub(super) struct FileRow {
    pub node: FileNode,
    pub depth: usize,
    pub expanded: bool,
    pub label: SharedString,
    pub highlights: Vec<Range<usize>>,
    pub icon_token: &'static str,
    pub id: SharedString,
    pub group: SharedString,
    pub status: GitSummary,
    pub state: DirState,
}

impl FileRow {
    fn new(
        node: &FileNode,
        depth: usize,
        expanded: bool,
        highlights: Vec<Range<usize>>,
        state: DirState,
        git: &GitStatuses,
    ) -> Self {
        let label = files_tree::node_name(node);
        Self {
            node: node.clone(),
            depth,
            expanded,
            icon_token: crate::file_icons::file_icon_token(&label),
            label: label.into(),
            highlights,
            status: git.summary(&node.path),
            id: format!("files-row-{}", node.path.display()).into(),
            group: format!("files-row-group-{}", node.path.display()).into(),
            state,
        }
    }

    pub(super) fn from_ref(row: RowRef<'_>, git: &GitStatuses) -> Self {
        Self::new(
            row.node,
            row.depth,
            row.expanded,
            Vec::new(),
            row.state,
            git,
        )
    }
}

#[derive(Default)]
pub(super) struct SearchRows {
    pub rows: Vec<FileRow>,
    indices: HashMap<PathBuf, usize>,
}

#[derive(Default)]
struct Level<'a> {
    dirs: BTreeMap<(String, &'a str), Level<'a>>,
    files: Vec<(String, &'a str, &'a SearchHit)>,
}

impl SearchRows {
    pub fn build(root: &Path, hits: &[SearchHit], git: &GitStatuses) -> Self {
        let rows = search_rows(root, hits, git);
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

fn search_rows(root: &Path, hits: &[SearchHit], git: &GitStatuses) -> Vec<FileRow> {
    let mut top = Level::default();
    for hit in hits {
        let mut parts: Vec<&str> = hit
            .rel
            .split(std::path::is_separator)
            .filter(|part| !part.is_empty())
            .collect();
        let Some(name) = parts.pop() else {
            continue;
        };
        let mut level = &mut top;
        for part in parts {
            level = level
                .dirs
                .entry((part.to_ascii_lowercase(), part))
                .or_default();
        }
        level.files.push((name.to_ascii_lowercase(), name, hit));
    }
    let mut rows = Vec::new();
    emit_level(root.to_path_buf(), &mut top, 0, git, &mut rows);
    rows
}

fn emit_level(
    dir: PathBuf,
    level: &mut Level<'_>,
    depth: usize,
    git: &GitStatuses,
    rows: &mut Vec<FileRow>,
) {
    for ((_, name), child) in level.dirs.iter_mut() {
        let path = dir.join(name);
        let node = FileNode {
            path: path.clone(),
            is_dir: true,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        };
        rows.push(FileRow::new(
            &node,
            depth,
            true,
            Vec::new(),
            DirState::Ready,
            git,
        ));
        emit_level(path, child, depth + 1, git, rows);
    }
    level.files.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, name, hit) in &level.files {
        let node = FileNode {
            path: dir.join(name),
            is_dir: false,
            is_ignored: false,
            is_hidden: false,
            size: 0,
        };
        let start = hit.rel.len() - name.len();
        let highlights = hit
            .highlights
            .iter()
            .filter(|range| range.end > start)
            .map(|range| range.start.max(start) - start..range.end - start)
            .collect();
        rows.push(FileRow::new(
            &node,
            depth,
            false,
            highlights,
            DirState::Ready,
            git,
        ));
    }
}
