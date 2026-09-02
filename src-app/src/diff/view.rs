use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, ScrollHandle,
    ScrollWheelEvent, SharedString, StatefulInteractiveElement, Styled, Window, anchored, deferred,
    div, point, prelude::*, px, relative,
};
use notify::RecommendedWatcher;

use crate::agent_sessions::SessionMeta;
use crate::pane_drag::{DragPreview, DropEdge, SPLIT_EDGE_BAND, compute_drop_edge};
use crate::settings::components::{menu_divider_color, menu_surface, select_item, with_alpha};
use crate::widgets::text_input::TextInput;

use super::arrange::{Arrange, Axis};
use super::element::{DiffBody, DiffElement};
use super::hit_test;
use super::hscroll::HScrollbarSegment;
use super::review_terminal::ReviewTerminal;

mod attribution;
mod base_branch;
mod interaction;
mod loader;
mod model;
mod render;
mod review;
mod scroller;
mod watcher;

pub use model::{DiffWorktree, FileEntry, FileListState, aggregate_file_lists};

#[derive(Clone)]
pub struct DiffColumnDrag {
    pub source_idx: usize,
}

#[derive(Clone, Copy)]
struct DiffHScrollDrag {
    col_idx: usize,
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

const COL_HEADER_HEIGHT: f32 = 30.0;

const TOOLBAR_CHIP_BOTTOM: f32 = 31.0;

const MIN_SPLIT_COLUMN_PX: f32 = 360.0;

const REFRESH_DEBOUNCE: Duration = Duration::from_millis(500);

const REFRESH_COOLDOWN: Duration = Duration::from_millis(1000);

const SYNTAX_HIGHLIGHT_ENABLED: bool = true;

const REVIEW_DEFAULT_HEIGHT: f32 = 520.0;
const REVIEW_MIN_HEIGHT: f32 = 120.0;
const REVIEW_MAX_HEIGHT: f32 = 1000.0;

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
    visible: bool,
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
    base_override: Option<String>,
    generation: u64,
    loading_mode: Option<ViewMode>,
    loading_theme_generation: Option<u64>,
    review_terminals: Vec<ReviewTerminal>,
    active_review_terminal: usize,
    review_height: f32,
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
            visible: true,
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
            base_override: None,
            generation: 0,
            loading_mode: None,
            loading_theme_generation: None,
            review_terminals: Vec::new(),
            active_review_terminal: 0,
            review_height: REVIEW_DEFAULT_HEIGHT,
            attribution: Vec::new(),
            h_offsets: Rc::new(Vec::new()),
        }
    }

    fn reset_display_caches(&mut self) {
        self.clear_display_mode(ViewMode::Unified);
        self.clear_display_mode(ViewMode::Split);
        self.h_offsets = Rc::new(Vec::new());
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

    fn drop_loaded_data(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.loading_mode = None;
        self.loading_theme_generation = None;
        self.state = ColumnState::Loading;
        self.collapsed.clear();
        self.expanded_folds.clear();
        self.fingerprint = None;
        self.attribution.clear();
        self.reset_display_caches();
    }

    fn has_running_review_terminal(&self, cx: &mut Context<DiffView>) -> bool {
        self.review_terminals
            .iter()
            .any(|term| term.terminal.read(cx).terminal.exited.is_none())
    }

    fn drop_exited_review_terminals(&mut self, cx: &mut Context<DiffView>) {
        self.review_terminals
            .retain(|term| term.terminal.read(cx).terminal.exited.is_none());
        if self.review_terminals.is_empty() {
            self.active_review_terminal = 0;
        } else if self.active_review_terminal >= self.review_terminals.len() {
            self.active_review_terminal = self.review_terminals.len() - 1;
        }
    }

    fn drop_review_terminals(&mut self) {
        self.review_terminals.clear();
        self.active_review_terminal = 0;
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
}

pub struct DiffView {
    repo_root: PathBuf,
    base_ref: String,
    branches: Vec<String>,
    branches_lc: Vec<String>,
    base_picker_open: bool,
    base_filter: Entity<TextInput>,
    columns: Vec<Column>,
    focus_handle: FocusHandle,
    element_id: SharedString,
    watch_epoch: u64,
    mode: ViewMode,
    last_effective_mode: ViewMode,
    sync_scroll: bool,
    scroll_driver: usize,
    selected_column: usize,
    _watchers: Vec<RecommendedWatcher>,
    suspended: bool,
    bootstrapped: bool,
    arrange: Arrange,
    drag_target: Option<(usize, Option<DropEdge>)>,
    body_menu: Option<DiffBodyMenu>,
    last_body_pos: Option<(usize, Point<Pixels>)>,
    flash: Option<SharedString>,
    review_menu_open: Option<usize>,
    review_picks: Vec<bool>,
    review_resizing: Option<(usize, f32, f32)>,
    h_scroll_drag: Option<DiffHScrollDrag>,
    close_removes: bool,
    pub scope_slot: Option<AnyElement>,
}

pub enum DiffViewEvent {
    CloseColumn { path: PathBuf },
}

struct DiffBodyMenu {
    position: Point<Pixels>,
    col_idx: usize,
    scope: DiffBodyScope,
    mode: ViewMode,
}

#[derive(Clone, Copy)]
struct DiffBodyScope {
    file_idx: usize,
    hunk_idx: Option<usize>,
}

impl DiffView {
    pub fn new(repo_root: PathBuf, worktrees: Vec<DiffWorktree>, cx: &mut Context<Self>) -> Self {
        Self::with_base(repo_root, worktrees, None, cx)
    }

    pub fn with_base(
        repo_root: PathBuf,
        worktrees: Vec<DiffWorktree>,
        base: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let element_id = SharedString::from(format!("diff-view-{}", repo_root.display()));
        let columns: Vec<Column> = worktrees
            .into_iter()
            .map(|w| Column::new_loading(w.branch, w.path, w.workspace_id))
            .collect();
        let arrange = Arrange::row(&(0..columns.len()).collect::<Vec<_>>());
        let base_filter = cx.new(|cx| TextInput::new("", "Filter branches…", cx));
        cx.observe(&base_filter, |_, _, cx| cx.notify()).detach();
        let mut view = Self {
            repo_root,
            base_ref: base.unwrap_or_default(),
            branches: Vec::new(),
            branches_lc: Vec::new(),
            base_picker_open: false,
            base_filter,
            columns,
            focus_handle: cx.focus_handle(),
            element_id,
            watch_epoch: 0,
            mode: ViewMode::Unified,
            last_effective_mode: ViewMode::Unified,
            sync_scroll: true,
            scroll_driver: 0,
            selected_column: 0,
            _watchers: Vec::new(),
            suspended: false,
            bootstrapped: false,
            arrange,
            drag_target: None,
            body_menu: None,
            last_body_pos: None,
            flash: None,
            review_menu_open: None,
            review_picks: Vec::new(),
            review_resizing: None,
            h_scroll_drag: None,
            close_removes: false,
            scope_slot: None,
        };
        view.bootstrap(cx);
        view
    }

    pub fn set_close_removes(&mut self, v: bool) {
        self.close_removes = v;
    }

    pub fn column_paths(&self) -> Vec<PathBuf> {
        self.columns
            .iter()
            .filter(|c| c.visible)
            .map(|c| c.path.clone())
            .collect()
    }

    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let first = self.columns.first().map(|c| c.path.clone());
        let n = self.columns.len();
        let preset = self.base_ref.clone();
        cx.spawn(async move |this, cx| {
            log::debug!("diff: bootstrap START ({n} columns); resolving base off-thread");
            let t = Instant::now();
            let (base, branches) = match first {
                Some(p) => {
                    smol::unblock(move || {
                        let base = if !preset.is_empty() && super::git::ref_exists(&p, &preset) {
                            preset
                        } else {
                            super::git::default_base_ref(&p).unwrap_or_default()
                        };
                        let branches = super::git::list_base_ref_candidates(&p);
                        (base, branches)
                    })
                    .await
                }
                None => (preset, Vec::new()),
            };
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
        let name = self
            .repo_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.repo_root.display().to_string());
        format!("Diff: {name}")
    }

    fn effective_mode(&self, window: &Window) -> ViewMode {
        if self.mode == ViewMode::Unified {
            return ViewMode::Unified;
        }
        let cols = self.visible_count().max(1) as f32;
        let per_col = f32::from(window.viewport_size().width) / cols;
        if per_col < MIN_SPLIT_COLUMN_PX {
            ViewMode::Unified
        } else {
            ViewMode::Split
        }
    }

    fn visible_count(&self) -> usize {
        self.columns.iter().filter(|c| c.visible).count()
    }

    fn toggle_column_base(&mut self, idx: usize, cx: &mut Context<Self>) {
        match self.columns.get_mut(idx) {
            Some(col) => {
                col.base_override = match col.base_override {
                    None => Some("HEAD~1".to_string()),
                    Some(_) => None,
                };
            }
            None => return,
        }
        self.start_loading_columns(&[idx], cx);
    }

    fn hide_column(&mut self, idx: usize, cx: &mut Context<Self>) {
        let blocked_by_running_review = {
            let Some(col) = self.columns.get_mut(idx) else {
                return;
            };
            if col.has_running_review_terminal(cx) {
                col.drop_exited_review_terminals(cx);
                true
            } else {
                col.visible = false;
                col.drop_review_terminals();
                col.drop_loaded_data();
                false
            }
        };
        if blocked_by_running_review {
            self.set_flash(
                "Close Review terminals before hiding this column".into(),
                cx,
            );
            return;
        }
        if self
            .body_menu
            .as_ref()
            .is_some_and(|menu| menu.col_idx == idx)
        {
            self.body_menu = None;
        }
        if self.review_menu_open == Some(idx) {
            self.review_menu_open = None;
        }
        if self
            .review_resizing
            .is_some_and(|(col_idx, _, _)| col_idx == idx)
        {
            self.review_resizing = None;
        }
        if self.h_scroll_drag.is_some_and(|drag| drag.col_idx == idx) {
            self.h_scroll_drag = None;
        }
        if self
            .last_body_pos
            .is_some_and(|(col_idx, _)| col_idx == idx)
        {
            self.last_body_pos = None;
        }
        if self.scroll_driver == idx || self.selected_column == idx {
            let first_visible = self.columns.iter().position(|c| c.visible).unwrap_or(0);
            if self.scroll_driver == idx {
                self.scroll_driver = first_visible;
            }
            if self.selected_column == idx {
                self.selected_column = first_visible;
            }
        }
        self.restart_watchers(cx);
        cx.notify();
    }

    fn show_all_columns(&mut self, cx: &mut Context<Self>) {
        for col in &mut self.columns {
            col.visible = true;
        }
        self.start_loading(cx);
        self.restart_watchers(cx);
        cx.notify();
    }

    pub fn base_ref(&self) -> &str {
        &self.base_ref
    }

    pub fn add_columns(&mut self, worktrees: Vec<DiffWorktree>, cx: &mut Context<Self>) {
        let existing: std::collections::HashSet<String> =
            self.columns.iter().map(|c| norm_key(&c.path)).collect();
        let mut added = false;
        for w in worktrees {
            if existing.contains(&norm_key(&w.path)) {
                continue;
            }
            self.columns
                .push(Column::new_loading(w.branch, w.path, w.workspace_id));
            added = true;
        }
        if added {
            self.start_loading(cx);
            self.restart_watchers(cx);
        }
    }

    fn selected_or_first_visible(&self) -> Option<usize> {
        if self
            .columns
            .get(self.selected_column)
            .is_some_and(|c| c.visible)
        {
            Some(self.selected_column)
        } else {
            self.columns.iter().position(|c| c.visible)
        }
    }

    fn reload_visible_columns_if_theme_changed(&mut self, cx: &mut Context<Self>) {
        let current_theme_generation = crate::theme::theme_generation();
        let stale: Vec<usize> = self
            .columns
            .iter()
            .enumerate()
            .filter_map(|(idx, col)| {
                if !col.visible || col.loading_theme_generation == Some(current_theme_generation) {
                    return None;
                }
                let loaded_generation = col.loaded_theme_generation()?;
                (loaded_generation != current_theme_generation).then_some(idx)
            })
            .collect();
        if !stale.is_empty() {
            self.start_loading_columns(&stale, cx);
        }
    }

    fn all_visible_collapsed(&self) -> bool {
        let mut any_loaded = false;
        for col in &self.columns {
            if !col.visible {
                continue;
            }
            if let ColumnState::Loaded { files_full, .. } = &col.state {
                any_loaded = true;
                if !files_full
                    .iter()
                    .all(|file| col.collapsed.contains(&file.path))
                {
                    return false;
                }
            }
        }
        any_loaded
    }

    fn toggle_collapse_all(&mut self, cx: &mut Context<Self>) {
        let collapse = !self.all_visible_collapsed();
        for col in &mut self.columns {
            if !col.visible {
                continue;
            }
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
        }
        cx.notify();
    }
}

impl DiffView {
    fn render_base_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();

        let filter = self.base_filter.read(cx).value().to_lowercase();
        let matches = base_branch::matching_indices(&self.branches_lc, &filter);

        let search = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(7.))
            .px(px(10.))
            .py(px(7.))
            .border_b_1()
            .border_color(menu_divider_color(ui))
            .child(
                gpui::svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/tool_search.svg")
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.text)
                    .child(self.base_filter.clone()),
            )
            .when(!self.branches.is_empty(), |d| {
                d.child(
                    div()
                        .flex_none()
                        .text_size(crate::ui_primitives::LABEL_SM)
                        .text_color(ui.muted)
                        .child(format!("{}", matches.len())),
                )
            });

        let mut list = div()
            .id("diff-base-list")
            .flex()
            .flex_col()
            .gap(px(1.))
            .max_h(px(280.))
            .overflow_y_scroll()
            .p(px(4.));

        if self.branches.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.muted)
                    .child("No local branches found"),
            );
        } else if matches.is_empty() {
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .text_size(crate::ui_primitives::BODY)
                    .text_color(ui.muted)
                    .child("No branch matches your filter"),
            );
        } else {
            for bi in matches {
                let Some(branch) = self.branches.get(bi) else {
                    continue;
                };
                let is_current = *branch == self.base_ref;
                let branch_owned = branch.clone();
                list = list.child(
                    select_item(
                        SharedString::from(format!("diff-base-opt-{bi}")),
                        is_current,
                        ui,
                    )
                    .cursor(CursorStyle::Arrow)
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.set_base(branch_owned.clone(), cx);
                        window.focus(&this.focus_handle, cx);
                    }))
                    .child(
                        gpui::svg()
                            .size(px(13.))
                            .flex_none()
                            .path("icons/git-branch.svg")
                            .text_color(if is_current { ui.accent } else { ui.muted }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(ui.text)
                            .child(branch.clone()),
                    )
                    .when(is_current, |d| {
                        d.child(
                            gpui::svg()
                                .size(px(13.))
                                .flex_none()
                                .path("icons/check.svg")
                                .text_color(ui.accent),
                        )
                    }),
                );
            }
        }

        menu_surface(div().id("diff-base-popover"), ui)
            .occlude()
            .absolute()
            .left(px(8.))
            .top(px(TOOLBAR_CHIP_BOTTOM))
            .w(px(288.))
            .flex()
            .flex_col()
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.close_base_picker(window, cx);
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        this.close_base_picker(window, cx);
                        cx.stop_propagation();
                    }
                    "enter" => {
                        let raw = this.base_filter.read(cx).value().to_string();
                        let filter = raw.to_lowercase();
                        if let Some(branch) =
                            base_branch::first_matching_index(&this.branches_lc, &filter)
                                .and_then(|index| this.branches.get(index))
                                .cloned()
                        {
                            this.set_base(branch, cx);
                            window.focus(&this.focus_handle, cx);
                        } else if !raw.trim().is_empty() {
                            this.resolve_and_set_base(raw, cx);
                            window.focus(&this.focus_handle, cx);
                        }
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(search)
            .child(list)
            .into_any_element()
    }
}

#[cfg(test)]
#[allow(
    clippy::items_after_test_module,
    reason = "path normalization remains beside the view implementation"
)]
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

    #[test]
    fn hidden_column_cleanup_drops_loaded_data_and_display_caches() {
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
        assert!(col.review_terminals.is_empty());
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
}

fn norm_key(p: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}
