use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, Context, CursorStyle, FocusHandle, Focusable, IntoElement,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point,
    Render, ScrollHandle, ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled,
    Window, anchored, deferred, div, point, prelude::*, px,
};
use notify::RecommendedWatcher;

use crate::agent_sessions::SessionMeta;
use crate::diff::DiffOptions;
use crate::settings::components::{menu_surface, select_item, with_alpha};

use super::element::{DiffBody, DiffElement};
use super::hit_test;
use super::hscroll::HScrollbarSegment;

mod attribution;
mod base_branch;
mod interaction;
mod loader;
mod model;
mod render;
mod scroller;
mod watcher;

pub use model::{DiffWorktree, FileEntry, FileListState, ReviewSubject};

#[derive(Clone, Copy)]
struct DiffHScrollDrag {
    offset_idx: usize,
    start_mouse_x: Pixels,
    start_offset: f32,
    max_scroll: f32,
    track_width: f32,
    thumb_width: f32,
}
use super::rows::{
    DisplayRow, FileRowCache, FileSpan, RowKind, SplitRow, apply_collapse_split,
    apply_collapse_unified, apply_expanded_split_with_sources, apply_expanded_unified_with_sources,
    build_display_rows_with_caches, build_file_row_caches, build_split_rows_with_caches,
    discard_expanded_folds_for_path, palette, split_file_spans, split_hunk_tops, split_max_line_no,
    split_offsets, unified_file_spans, unified_hunk_tops, unified_max_line_no, unified_offsets,
};

const HUNK_JUMP_MARGIN: f32 = 28.0;

const MIN_SPLIT_COLUMN_PX: f32 = 360.0;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(500);

const REFRESH_COOLDOWN: Duration = Duration::from_millis(1000);

const SYNTAX_HIGHLIGHT_ENABLED: bool = true;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Split,
    Unified,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            ViewMode::Split => "split",
            ViewMode::Unified => "unified",
        }
    }

    fn other(self) -> Self {
        match self {
            ViewMode::Split => ViewMode::Unified,
            ViewMode::Unified => ViewMode::Split,
        }
    }
}

enum BuiltModeRows {
    Unified {
        rows: Vec<DisplayRow>,
        anchors: Vec<(String, usize)>,
    },
    Split {
        rows: Vec<SplitRow>,
        anchors: Vec<(String, usize)>,
    },
}

#[cfg(test)]
fn build_rows_for_mode(
    files: &[super::git::FileDiff],
    mode: ViewMode,
    syntax: Option<&super::syntax::DiffSyntax>,
) -> BuiltModeRows {
    let caches = build_file_row_caches(files, syntax);
    build_rows_for_mode_with_caches(files, mode, &caches)
}

fn build_rows_for_mode_with_caches(
    files: &[super::git::FileDiff],
    mode: ViewMode,
    caches: &[FileRowCache],
) -> BuiltModeRows {
    match mode {
        ViewMode::Unified => {
            let (rows, _) = build_display_rows_with_caches(files, caches);
            let anchors = files
                .iter()
                .map(|f| f.path.clone())
                .zip(
                    rows.iter()
                        .enumerate()
                        .filter(|(_, r)| r.kind == RowKind::FileHeader)
                        .map(|(i, _)| i),
                )
                .collect();
            BuiltModeRows::Unified { rows, anchors }
        }
        ViewMode::Split => {
            let (rows, _) = build_split_rows_with_caches(files, caches);
            let anchors = files
                .iter()
                .map(|f| f.path.clone())
                .zip(
                    rows.iter()
                        .enumerate()
                        .filter(|(_, r)| matches!(r, SplitRow::Header(_)))
                        .map(|(i, _)| i),
                )
                .collect();
            BuiltModeRows::Split { rows, anchors }
        }
    }
}

enum ColumnState {
    Loading,
    Loaded {
        unified: Option<Rc<Vec<DisplayRow>>>,
        split: Option<Rc<Vec<SplitRow>>>,
        file_count: usize,
        files: Rc<Vec<FileEntry>>,
        anchors_unified: Option<Rc<Vec<(String, usize)>>>,
        anchors_split: Option<Rc<Vec<(String, usize)>>>,
        files_full: Arc<Vec<super::git::FileDiff>>,
        row_caches: Arc<Vec<FileRowCache>>,
        theme_generation: u64,
    },
    Failed(String),
}

enum Built {
    Failed(String),
    Loaded {
        rows: BuiltModeRows,
        file_count: usize,
        files: Vec<FileEntry>,
        files_full: Vec<super::git::FileDiff>,
        row_caches: Vec<FileRowCache>,
        theme_generation: u64,
        fingerprint: Box<super::git::ColumnFingerprint>,
        attribution: Vec<SessionMeta>,
    },
}

struct Column {
    branch: String,
    path: PathBuf,
    workspace_id: Option<u64>,
    state: ColumnState,
    el_scroll: ScrollHandle,
    collapsed: std::collections::HashSet<String>,
    expanded_folds: std::collections::HashSet<String>,
    disp_unified: Rc<Vec<DisplayRow>>,
    disp_split: Rc<Vec<SplitRow>>,
    disp_anchors_unified: Rc<Vec<(String, usize)>>,
    disp_anchors_split: Rc<Vec<(String, usize)>>,
    disp_unified_offsets: Rc<Vec<f32>>,
    disp_split_offsets: Rc<Vec<f32>>,
    disp_unified_max_no: u32,
    disp_split_max_no: u32,
    disp_unified_spans: Rc<Vec<FileSpan>>,
    disp_split_spans: Rc<Vec<FileSpan>>,
    disp_hunk_tops_unified: Rc<Vec<f32>>,
    disp_hunk_tops_split: Rc<Vec<f32>>,
    fingerprint: Option<super::git::ColumnFingerprint>,
    generation: u64,
    loading_mode: Option<ViewMode>,
    loading_theme_generation: Option<u64>,
    attribution: Vec<SessionMeta>,
    h_offsets: Rc<Vec<f32>>,
}

impl Column {
    fn new_loading(branch: String, path: PathBuf, workspace_id: Option<u64>) -> Self {
        Self {
            branch,
            path,
            workspace_id,
            state: ColumnState::Loading,
            el_scroll: ScrollHandle::new(),
            collapsed: std::collections::HashSet::new(),
            expanded_folds: std::collections::HashSet::new(),
            disp_unified: Rc::new(Vec::new()),
            disp_split: Rc::new(Vec::new()),
            disp_anchors_unified: Rc::new(Vec::new()),
            disp_anchors_split: Rc::new(Vec::new()),
            disp_unified_offsets: Rc::new(vec![0.0]),
            disp_split_offsets: Rc::new(vec![0.0]),
            disp_unified_max_no: 0,
            disp_split_max_no: 0,
            disp_unified_spans: Rc::new(Vec::new()),
            disp_split_spans: Rc::new(Vec::new()),
            disp_hunk_tops_unified: Rc::new(Vec::new()),
            disp_hunk_tops_split: Rc::new(Vec::new()),
            fingerprint: None,
            generation: 0,
            loading_mode: None,
            loading_theme_generation: None,
            attribution: Vec::new(),
            h_offsets: Rc::new(Vec::new()),
        }
    }

    fn clear_display_mode(&mut self, mode: ViewMode) {
        match mode {
            ViewMode::Unified => {
                self.disp_unified = Rc::new(Vec::new());
                self.disp_anchors_unified = Rc::new(Vec::new());
                self.disp_unified_offsets = Rc::new(vec![0.0]);
                self.disp_unified_max_no = 0;
                self.disp_unified_spans = Rc::new(Vec::new());
                self.disp_hunk_tops_unified = Rc::new(Vec::new());
            }
            ViewMode::Split => {
                self.disp_split = Rc::new(Vec::new());
                self.disp_anchors_split = Rc::new(Vec::new());
                self.disp_split_offsets = Rc::new(vec![0.0]);
                self.disp_split_max_no = 0;
                self.disp_split_spans = Rc::new(Vec::new());
                self.disp_hunk_tops_split = Rc::new(Vec::new());
            }
        }
    }

    #[cfg(test)]
    fn drop_loaded_data(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loading_mode = None;
        self.loading_theme_generation = None;
        self.state = ColumnState::Loading;
        self.collapsed.clear();
        self.expanded_folds.clear();
        self.fingerprint = None;
        self.attribution.clear();
        self.clear_display_mode(ViewMode::Unified);
        self.clear_display_mode(ViewMode::Split);
        self.h_offsets = Rc::new(Vec::new());
    }

    fn has_rows_for_mode(&self, mode: ViewMode) -> bool {
        match &self.state {
            ColumnState::Loaded { unified, split, .. } => match mode {
                ViewMode::Unified => unified.is_some(),
                ViewMode::Split => split.is_some(),
            },
            _ => false,
        }
    }

    fn has_display_for_mode(&self, mode: ViewMode) -> bool {
        match mode {
            ViewMode::Unified => !self.disp_unified.is_empty(),
            ViewMode::Split => !self.disp_split.is_empty(),
        }
    }

    fn loaded_theme_generation(&self) -> Option<u64> {
        match &self.state {
            ColumnState::Loaded {
                theme_generation, ..
            } => Some(*theme_generation),
            _ => None,
        }
    }

    fn insert_mode_rows(&mut self, rows: BuiltModeRows) {
        let ColumnState::Loaded {
            unified,
            split,
            anchors_unified,
            anchors_split,
            ..
        } = &mut self.state
        else {
            return;
        };
        match rows {
            BuiltModeRows::Unified { rows, anchors } => {
                *unified = Some(Rc::new(rows));
                *anchors_unified = Some(Rc::new(anchors));
            }
            BuiltModeRows::Split { rows, anchors } => {
                *split = Some(Rc::new(rows));
                *anchors_split = Some(Rc::new(anchors));
            }
        }
    }

    fn recompute_display(&mut self) {
        self.recompute_display_for(ViewMode::Unified);
        self.recompute_display_for(ViewMode::Split);
    }

    fn recompute_display_for(&mut self, mode: ViewMode) {
        match mode {
            ViewMode::Unified => self.recompute_unified_display(),
            ViewMode::Split => self.recompute_split_display(),
        }
    }

    fn recompute_unified_display(&mut self) {
        let computed = match &self.state {
            ColumnState::Loaded {
                unified,
                anchors_unified,
                files_full,
                row_caches,
                ..
            } => match (unified, anchors_unified) {
                (Some(unified), Some(anchors_unified)) => {
                    if self.collapsed.is_empty() && self.expanded_folds.is_empty() {
                        Some((unified.clone(), anchors_unified.clone()))
                    } else {
                        let (du, au) = if self.collapsed.is_empty() {
                            (unified.as_ref().clone(), anchors_unified.as_ref().clone())
                        } else {
                            apply_collapse_unified(unified, anchors_unified, &self.collapsed)
                        };
                        let (du, au) = if self.expanded_folds.is_empty() {
                            (du, au)
                        } else {
                            apply_expanded_unified_with_sources(
                                &du,
                                &au,
                                &self.expanded_folds,
                                files_full,
                                row_caches,
                            )
                        };
                        Some((Rc::new(du), Rc::new(au)))
                    }
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((u, au)) = computed {
            self.disp_unified = u;
            self.disp_anchors_unified = au;
            self.disp_unified_offsets = Rc::new(unified_offsets(&self.disp_unified));
            self.disp_unified_max_no = unified_max_line_no(&self.disp_unified);
            self.disp_unified_spans = Rc::new(unified_file_spans(&self.disp_unified));
            let file_count = self.disp_unified_spans.len();
            let needed = super::hscroll::h_offset_len(file_count, false);
            if self.h_offsets.len() < needed {
                Rc::make_mut(&mut self.h_offsets).resize(needed, 0.0);
            }
            self.disp_hunk_tops_unified = Rc::new(unified_hunk_tops(&self.disp_unified));
        } else {
            self.clear_display_mode(ViewMode::Unified);
        }
    }

    fn recompute_split_display(&mut self) {
        let computed = match &self.state {
            ColumnState::Loaded {
                split,
                anchors_split,
                files_full,
                row_caches,
                ..
            } => match (split, anchors_split) {
                (Some(split), Some(anchors_split)) => {
                    if self.collapsed.is_empty() && self.expanded_folds.is_empty() {
                        Some((split.clone(), anchors_split.clone()))
                    } else {
                        let (ds, as_) = if self.collapsed.is_empty() {
                            (split.as_ref().clone(), anchors_split.as_ref().clone())
                        } else {
                            apply_collapse_split(split, anchors_split, &self.collapsed)
                        };
                        let (ds, as_) = if self.expanded_folds.is_empty() {
                            (ds, as_)
                        } else {
                            apply_expanded_split_with_sources(
                                &ds,
                                &as_,
                                &self.expanded_folds,
                                files_full,
                                row_caches,
                            )
                        };
                        Some((Rc::new(ds), Rc::new(as_)))
                    }
                }
                _ => None,
            },
            _ => None,
        };
        if let Some((s, as_)) = computed {
            self.disp_split = s;
            self.disp_anchors_split = as_;
            self.disp_split_offsets = Rc::new(split_offsets(&self.disp_split));
            self.disp_split_max_no = split_max_line_no(&self.disp_split);
            self.disp_split_spans = Rc::new(split_file_spans(&self.disp_split));
            let file_count = self.disp_split_spans.len();
            let needed = super::hscroll::h_offset_len(file_count, true);
            if self.h_offsets.len() != needed {
                Rc::make_mut(&mut self.h_offsets).resize(needed, 0.0);
            }
            self.disp_hunk_tops_split = Rc::new(split_hunk_tops(&self.disp_split));
        } else {
            self.clear_display_mode(ViewMode::Split);
        }
    }

    fn hunk_tops(&self, mode: ViewMode) -> &Rc<Vec<f32>> {
        match mode {
            ViewMode::Unified => &self.disp_hunk_tops_unified,
            ViewMode::Split => &self.disp_hunk_tops_split,
        }
    }

    fn file_list(&self) -> FileListState {
        match &self.state {
            ColumnState::Loading => FileListState::Loading,
            ColumnState::Failed(e) => FileListState::Failed(e.clone()),
            ColumnState::Loaded { files, .. } => FileListState::Loaded(files.clone()),
        }
    }
}

pub struct DiffView {
    repo_root: PathBuf,
    base_ref: String,
    branches: Vec<String>,
    branches_lc: Vec<String>,
    column: Column,
    options: DiffOptions,
    focus_handle: FocusHandle,
    element_id: SharedString,
    watch_epoch: u64,
    mode: ViewMode,
    last_effective_mode: ViewMode,
    _watchers: Vec<RecommendedWatcher>,
    suspended: bool,
    bootstrapped: bool,
    body_menu: Option<DiffBodyMenu>,
    last_body_pos: Option<Point<Pixels>>,
    flash: Option<SharedString>,
    h_scroll_drag: Option<DiffHScrollDrag>,
    vertical_scrollbar: crate::widgets::editor_scrollbar::EditorScrollbar,
}

struct DiffBodyMenu {
    position: Point<Pixels>,
    scope: DiffBodyScope,
    mode: ViewMode,
}

#[derive(Clone, Copy)]
struct DiffBodyScope {
    file_idx: usize,
    hunk_idx: Option<usize>,
}

impl DiffView {
    pub fn new(subject: ReviewSubject, cx: &mut Context<Self>) -> Self {
        Self::with_base(subject, None, cx)
    }

    pub fn with_base(subject: ReviewSubject, base: Option<String>, cx: &mut Context<Self>) -> Self {
        let element_id =
            SharedString::from(format!("diff-view-{}", subject.worktree.path.display()));
        let column = Column::new_loading(
            subject.worktree.branch,
            subject.worktree.path,
            subject.worktree.workspace_id,
        );
        let mut view = Self {
            repo_root: subject.repo_root,
            base_ref: base.unwrap_or_default(),
            branches: Vec::new(),
            branches_lc: Vec::new(),
            column,
            options: DiffOptions::default(),
            focus_handle: cx.focus_handle(),
            element_id,
            watch_epoch: 0,
            mode: ViewMode::Unified,
            last_effective_mode: ViewMode::Unified,
            _watchers: Vec::new(),
            suspended: false,
            bootstrapped: false,
            body_menu: None,
            last_body_pos: None,
            flash: None,
            h_scroll_drag: None,
            vertical_scrollbar: Default::default(),
        };
        view.bootstrap(cx);
        view
    }

    pub fn subject(&self) -> ReviewSubject {
        ReviewSubject {
            repo_root: self.repo_root.clone(),
            worktree: DiffWorktree {
                path: self.column.path.clone(),
                branch: self.column.branch.clone(),
                workspace_id: self.column.workspace_id,
            },
        }
    }

    pub fn worktree_path(&self) -> &PathBuf {
        &self.column.path
    }

    pub fn file_list(&self) -> FileListState {
        self.column.file_list()
    }

    pub fn has_changes(&self) -> bool {
        matches!(&self.column.state, ColumnState::Loaded { file_count, .. } if *file_count > 0)
    }

    pub fn base_ref(&self) -> &str {
        &self.base_ref
    }

    pub fn branches(&self) -> &[String] {
        &self.branches
    }

    pub fn matching_branches(&self, filter_lc: &str) -> Vec<usize> {
        base_branch::matching_indices(&self.branches_lc, filter_lc)
    }

    pub fn first_matching_branch(&self, filter_lc: &str) -> Option<String> {
        base_branch::first_matching_index(&self.branches_lc, filter_lc)
            .and_then(|index| self.branches.get(index))
            .cloned()
    }

    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let probe = self.column.path.clone();
        let preset = self.base_ref.clone();
        cx.spawn(async move |this, cx| {
            log::debug!("diff: bootstrap START; resolving base off-thread");
            let t = Instant::now();
            let (base, branches) = smol::unblock(move || {
                let base = if !preset.is_empty() && super::git::ref_exists(&probe, &preset) {
                    preset
                } else {
                    super::git::default_base_ref(&probe).unwrap_or_default()
                };
                let branches = super::git::list_base_ref_candidates(&probe);
                (base, branches)
            })
            .await;
            log::debug!(
                "diff: bootstrap resolved base={base:?}, {} branches in {:?}; -> start_loading + start_watchers",
                branches.len(),
                t.elapsed()
            );
            let _ = cx.update(|cx| {
                this.update(cx, |view: &mut Self, cx| {
                    view.base_ref = base;
                    view.branches_lc = branches.iter().map(|b| b.to_lowercase()).collect();
                    view.branches = branches;
                    view.bootstrapped = true;
                    view.start_loading(cx);
                    if !view.suspended {
                        view.start_watchers(cx);
                    }
                })
            });
        })
        .detach();
    }

    pub fn title(&self) -> String {
        self.subject().label()
    }

    fn effective_mode(&self, window: &Window) -> ViewMode {
        if self.mode == ViewMode::Unified {
            return ViewMode::Unified;
        }
        let measured = f32::from(self.column.el_scroll.bounds().size.width);
        let width = if measured > 0.0 {
            measured
        } else {
            f32::from(window.viewport_size().width)
        };
        if width < MIN_SPLIT_COLUMN_PX {
            ViewMode::Unified
        } else {
            ViewMode::Split
        }
    }

    fn reload_if_theme_changed(&mut self, cx: &mut Context<Self>) {
        let current = crate::theme::theme_generation();
        if self.column.loading_theme_generation == Some(current) {
            return;
        }
        let Some(loaded) = self.column.loaded_theme_generation() else {
            return;
        };
        if loaded != current {
            self.start_loading(cx);
        }
    }

    pub fn options(&self) -> DiffOptions {
        self.options
    }

    pub fn set_options(&mut self, options: DiffOptions, cx: &mut Context<Self>) {
        if self.options == options {
            return;
        }
        let comparison_changed = self.options.whitespace != options.whitespace;
        self.options = options;
        if comparison_changed {
            self.start_loading(cx);
        } else {
            cx.notify();
        }
    }

    pub fn is_split(&self) -> bool {
        self.mode == ViewMode::Split
    }

    pub fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        let mode = if split {
            ViewMode::Split
        } else {
            ViewMode::Unified
        };
        if self.mode != mode {
            self.mode = mode;
            cx.notify();
        }
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.start_loading(cx);
    }

    pub fn all_collapsed(&self) -> bool {
        match &self.column.state {
            ColumnState::Loaded { files_full, .. } => {
                !files_full.is_empty()
                    && files_full
                        .iter()
                        .all(|file| self.column.collapsed.contains(&file.path))
            }
            _ => false,
        }
    }

    pub fn set_all_collapsed(&mut self, collapse: bool, cx: &mut Context<Self>) {
        let col = &mut self.column;
        col.collapsed.clear();
        if collapse {
            let paths: Vec<String> = match &col.state {
                ColumnState::Loaded { files_full, .. } => {
                    files_full.iter().map(|file| file.path.clone()).collect()
                }
                _ => Vec::new(),
            };
            col.collapsed.extend(paths);
            col.expanded_folds.clear();
        }
        col.recompute_display();
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file() -> super::super::git::FileDiff {
        let base = "alpha\nold\nomega\n".to_string();
        let new = "alpha\nnew\nomega\n".to_string();
        super::super::git::FileDiff {
            path: "src/lib.rs".into(),
            change: super::super::git::FileChange::Modified,
            old_path: None,
            hunks: super::super::engine::compute_hunks(&base, &new),
            base_text: base,
            new_text: new,
            is_binary: false,
        }
    }

    fn file_entry(file: &super::super::git::FileDiff) -> FileEntry {
        let (added, removed) = file.line_counts();
        FileEntry {
            path: file.path.clone(),
            change: file.change,
            old_path: file.old_path.clone(),
            added,
            removed,
            is_binary: file.is_binary,
        }
    }

    fn loaded_column_with_both_modes() -> Column {
        let file = sample_file();
        let files = vec![file.clone()];
        let row_caches = build_file_row_caches(&files, None);
        let (unified, anchors_unified) = match build_rows_for_mode(&files, ViewMode::Unified, None)
        {
            BuiltModeRows::Unified { rows, anchors } => (rows, anchors),
            BuiltModeRows::Split { .. } => unreachable!("requested unified rows"),
        };
        let (split, anchors_split) = match build_rows_for_mode(&files, ViewMode::Split, None) {
            BuiltModeRows::Split { rows, anchors } => (rows, anchors),
            BuiltModeRows::Unified { .. } => unreachable!("requested split rows"),
        };
        let mut col = Column::new_loading("feature".into(), PathBuf::from("."), None);
        col.state = ColumnState::Loaded {
            unified: Some(Rc::new(unified)),
            split: Some(Rc::new(split)),
            file_count: 1,
            files: Rc::new(vec![file_entry(&file)]),
            anchors_unified: Some(Rc::new(anchors_unified)),
            anchors_split: Some(Rc::new(anchors_split)),
            files_full: Arc::new(files),
            row_caches: Arc::new(row_caches),
            theme_generation: crate::theme::theme_generation(),
        };
        col.collapsed.insert("src/lib.rs".into());
        col.h_offsets = Rc::new(vec![12.0]);
        col.recompute_display();
        col
    }

    #[gpui::test]
    fn highlight_changes_reuse_loaded_rows_without_waiting_for_a_reload(
        cx: &mut gpui::TestAppContext,
    ) {
        let view = cx.new(|cx| DiffView {
            repo_root: PathBuf::new(),
            base_ref: "HEAD".into(),
            branches: Vec::new(),
            branches_lc: Vec::new(),
            column: loaded_column_with_both_modes(),
            options: DiffOptions::default(),
            focus_handle: cx.focus_handle(),
            element_id: "highlight-test".into(),
            watch_epoch: 0,
            mode: ViewMode::Unified,
            last_effective_mode: ViewMode::Unified,
            _watchers: Vec::new(),
            suspended: true,
            bootstrapped: true,
            body_menu: None,
            last_body_pos: None,
            flash: None,
            h_scroll_drag: None,
            vertical_scrollbar: Default::default(),
        });
        view.update(cx, |view, cx| {
            let generation = view.column.generation;
            let unified = view.column.disp_unified.clone();
            let split = view.column.disp_split.clone();
            let offsets = view.column.h_offsets.clone();
            for highlight in [
                crate::diff::HighlightPolicy::Lines,
                crate::diff::HighlightPolicy::None,
                crate::diff::HighlightPolicy::Words,
            ] {
                view.set_options(
                    DiffOptions {
                        highlight,
                        ..view.options
                    },
                    cx,
                );
                assert_eq!(view.options.highlight, highlight);
                assert_eq!(
                    view.column.generation, generation,
                    "Highlight queued a full reload"
                );
                assert!(Rc::ptr_eq(&unified, &view.column.disp_unified));
                assert!(Rc::ptr_eq(&split, &view.column.disp_split));
                assert!(Rc::ptr_eq(&offsets, &view.column.h_offsets));
                assert!(view.column.loading_theme_generation.is_none());
            }
        });
    }

    #[test]
    fn dropping_loaded_data_clears_display_caches() {
        let mut col = loaded_column_with_both_modes();
        assert!(col.has_rows_for_mode(ViewMode::Unified));
        assert!(col.has_rows_for_mode(ViewMode::Split));
        assert!(!col.disp_unified.is_empty());
        assert!(!col.disp_split.is_empty());

        let generation = col.generation;
        col.drop_loaded_data();

        assert!(matches!(col.state, ColumnState::Loading));
        assert_eq!(col.generation, generation.wrapping_add(1));
        assert!(col.collapsed.is_empty());
        assert!(col.attribution.is_empty());
        assert!(col.h_offsets.is_empty());
        assert!(col.disp_unified.is_empty());
        assert!(col.disp_split.is_empty());
        assert!(col.disp_anchors_unified.is_empty());
        assert!(col.disp_anchors_split.is_empty());
        assert_eq!(col.disp_unified_offsets.as_ref(), &[0.0]);
        assert_eq!(col.disp_split_offsets.as_ref(), &[0.0]);
        assert!(col.disp_unified_spans.is_empty());
        assert!(col.disp_split_spans.is_empty());
        assert!(col.disp_hunk_tops_unified.is_empty());
        assert!(col.disp_hunk_tops_split.is_empty());
    }

    #[test]
    fn loaded_diff_mode_rows_are_retained() {
        let mut col = loaded_column_with_both_modes();

        col.recompute_display_for(ViewMode::Unified);
        assert!(col.has_rows_for_mode(ViewMode::Unified));
        assert!(col.has_rows_for_mode(ViewMode::Split));
        assert!(!col.disp_unified.is_empty());
        assert!(!col.disp_split.is_empty());

        let files = vec![sample_file()];
        col.insert_mode_rows(build_rows_for_mode(&files, ViewMode::Split, None));
        col.recompute_display_for(ViewMode::Split);
        assert!(col.has_rows_for_mode(ViewMode::Unified));
        assert!(col.has_rows_for_mode(ViewMode::Split));
        assert!(!col.disp_unified.is_empty());
        assert!(!col.disp_split.is_empty());
    }

    #[test]
    fn subject_label_joins_repo_and_branch() {
        let subject = ReviewSubject {
            repo_root: PathBuf::from("/home/dev/paneflow"),
            worktree: DiffWorktree {
                path: PathBuf::from("/home/dev/paneflow"),
                branch: "main".into(),
                workspace_id: Some(1),
            },
        };
        assert_eq!(subject.label(), "paneflow · main");
    }
}
