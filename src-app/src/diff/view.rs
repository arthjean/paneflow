//! `DiffView` - GPUI entity hosting the multi-worktree diff viewer.
//!
//! Pipeline: each sibling worktree becomes a column whose diff
//! (`merge-base..working-tree`) is computed off the main thread
//! (`smol::unblock`) and applied back via `this.update` from a spawned task -
//! never mutated inside `render`. A per-view `generation` counter discards
//! stale results when a refresh is superseded (US-007 last-write-wins).
//!
//! EP-004: N columns render side by side in one tab (US-012) with a shared
//! base-branch selector (US-013), per-column hide/show (US-014), and live
//! refresh on working-tree / HEAD / index / base-ref changes via an
//! entity-owned `notify` watcher (US-015). Rendering is virtualized by the
//! custom `DiffElement` in both unified and split modes (US-006/US-009).

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
use crate::pane_drag::{DropEdge, SPLIT_EDGE_BAND, TabDragPreview, compute_drop_edge};
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

/// Drag payload for a diff branch pane dragged by its column header (inc 5).
/// Carries the source column index; dropping on another pane's edge restructures
/// the [`Arrange`] tree to split toward that edge. Cloned cheaply by GPUI for
/// the duration of the drag.
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

/// When jumping to a hunk, leave this much room above its first changed line so
/// the pinned sticky file header (24px) does not cover it.
const HUNK_JUMP_MARGIN: f32 = 28.0;

/// Column-header bar height. The per-branch Review popover anchors just below it;
/// named so a header-height change has a single place to update (mirrors how
/// `HUNK_JUMP_MARGIN` centralizes the sticky-header offset).
const COL_HEADER_HEIGHT: f32 = 30.0;

/// Bottom of the toolbar base-ref chip; the base-branch popover anchors just
/// under it.
const TOOLBAR_CHIP_BOTTOM: f32 = 31.0;

/// Below this estimated per-column width the split view auto-falls back to
/// unified (mirroring Zed's `too_narrow_for_split`).
const MIN_SPLIT_COLUMN_PX: f32 = 360.0;

/// Live-refresh debounce. Long enough to coalesce a build's file churn into one
/// re-diff per window, short enough to feel live (US-015).
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(500);

/// After a reload, ignore further watcher events for this long. Prevents a
/// reload's own churn (or a concurrent build) from immediately re-triggering and
/// starving the in-flight load (the perpetual "Computing diff…" loop).
const REFRESH_COOLDOWN: Duration = Duration::from_millis(1000);

/// Syntax highlighting (prd-diff-syntax-highlight-2026-Q3.md). ON.
///
/// History: a full-file `syntect` pass cost 0.3-2.8 s/file (×4 builders ≈
/// ~30 s/column), so it shipped gated. Replaced by tree-sitter
/// ([`super::highlighter`]) - the same engine Zed uses - whose parse is
/// ms-scale, so the eager full-file highlight is now cheap enough to run at
/// build time off-thread. `super::syntax::DiffSyntax` supplies theme-derived
/// (ANSI) colors; unknown grammars fall back to monochrome.
const SYNTAX_HIGHLIGHT_ENABLED: bool = true;

/// Embedded review-terminal region height (px): default + drag clamp bounds.
/// Opens roughly half the view so the CLI/shell has real room (drag to resize).
const REVIEW_DEFAULT_HEIGHT: f32 = 520.0;
const REVIEW_MIN_HEIGHT: f32 = 120.0;
const REVIEW_MAX_HEIGHT: f32 = 1000.0;

/// Inline (unified) vs side-by-side. Unified is the default - it mirrors Zed's
/// git-panel Diff view (single gutter, one merged line number, colored hunk
/// bar). The toggle flips to Split; a too-narrow column also falls back to
/// Unified (US-011).
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

/// Async lifecycle of a single column's diff. Loaded keeps the raw per-file
/// diffs plus the row models that have already been prepared. The first load
/// installs the visible mode immediately; the inactive mode warms afterward.
enum ColumnState {
    Loading,
    Loaded {
        unified: Option<Rc<Vec<DisplayRow>>>,
        split: Option<Rc<Vec<SplitRow>>>,
        file_count: usize,
        /// US-008: per-file summary for the git panel (shared `Rc` so the
        /// sidebar reads it without cloning the whole list each frame).
        files: Rc<Vec<FileEntry>>,
        /// `(file path, header row index)` for the unified / split row sets, so
        /// a sidebar file click can scroll the body to that file
        /// ([`DiffView::jump_to_file`]). Built once per load, off-thread.
        anchors_unified: Option<Rc<Vec<(String, usize)>>>,
        anchors_split: Option<Rc<Vec<(String, usize)>>>,
        /// US-001/US-002 (prd-ai-in-diff-2026-Q3.md): the raw per-file diffs
        /// retained so "copy hunk/file" (US-003) and the agent review payload
        /// (US-005) serialize an exact unified diff at action time (no stable
        /// hunk ID - hunks are resolved from these on demand). Bounded by the
        /// same per-file caps as the rows; shared `Arc` so lazy mode builds can
        /// move the raw diff payload off-thread without cloning base/new text.
        files_full: Arc<Vec<super::git::FileDiff>>,
        /// Per-file syntax/word caches for the retained raw diff. Shared by lazy
        /// inactive-mode builds and fold expansion so those paths do not repeat
        /// the expensive highlighting work.
        row_caches: Arc<Vec<FileRowCache>>,
        /// Theme generation used to produce `row_caches`. Syntax runs store
        /// concrete colors, so a theme change requires rebuilding these rows.
        theme_generation: u64,
    },
    Failed(String),
}

/// Off-thread build result for one column. `Send` (only owned data) so it can
/// cross the `smol::unblock` boundary; the non-`Send` `Rc` wrapping happens back
/// on the main thread when the result is applied.
enum Built {
    Failed(String),
    Loaded {
        rows: BuiltModeRows,
        file_count: usize,
        files: Vec<FileEntry>,
        /// US-001/US-002: raw per-file diffs retained for copy/review, moved out
        /// of the off-thread `diff.files` after the rows are built from it.
        files_full: Vec<super::git::FileDiff>,
        /// Per-file syntax/word caches built once from the same `FileDiff`s.
        row_caches: Vec<FileRowCache>,
        /// Theme generation used to build the syntax caches.
        theme_generation: u64,
        /// US-016: captured in the same off-thread pass as the diff, so a later
        /// `revalidate` compares against it without re-shelling at harvest time.
        /// Boxed to keep this (transient, immediately-consumed) builder variant
        /// off the `large_enum_variant` threshold once `attribution` joined it.
        fingerprint: Box<super::git::ColumnFingerprint>,
        /// EP-004 US-014: agent sessions matched to this column, computed in the
        /// SAME off-thread pass (no second async round-trip) and applied onto the
        /// `Column` when the diff lands.
        attribution: Vec<SessionMeta>,
    },
}

struct Column {
    branch: String,
    path: PathBuf,
    /// The open workspace this column's worktree belongs to (seed
    /// [`DiffWorktree::workspace_id`]), used to tag the embedded review terminal.
    workspace_id: Option<u64>,
    state: ColumnState,
    /// Scroll handle for the custom [`DiffElement`] path (hosted in an
    /// `overflow_y_scroll` div). Also the offset source/target for cross-column
    /// scroll sync ([`DiffView::sync_scroll`]) and for `jump_to_file`.
    el_scroll: ScrollHandle,
    /// US-014: hidden columns are skipped in render and in refresh (no wasted
    /// diffing); re-showing reloads them.
    visible: bool,
    /// File paths whose hunks are collapsed (header-only) in the body. Persists
    /// across live-refresh reloads. Toggled per-file by clicking a file header
    /// in the body, or in bulk by the toolbar collapse/expand-all chip.
    collapsed: std::collections::HashSet<String>,
    /// Stable fold keys for collapsed unchanged regions that the user opened.
    /// The fold marker remains visible, so toggling the same key closes it.
    expanded_folds: std::collections::HashSet<String>,
    /// Collapse-filtered row sets + their file-header anchors, derived from
    /// `state` + `collapsed` by [`Column::recompute_display`] only on load /
    /// toggle (never per frame), so collapse is O(1) at paint and the body +
    /// `jump_to_file` index against what is actually shown.
    disp_unified: Rc<Vec<DisplayRow>>,
    disp_split: Rc<Vec<SplitRow>>,
    disp_anchors_unified: Rc<Vec<(String, usize)>>,
    disp_anchors_split: Rc<Vec<(String, usize)>>,
    /// Precomputed cumulative row offsets (`len + 1`) + widest line number for
    /// each display set, derived once in [`Column::recompute_display`] and shared
    /// with `DiffElement` so it never re-walks every row during `request_layout`
    /// / `prepaint`. Kept in lockstep with `disp_unified` / `disp_split`.
    disp_unified_offsets: Rc<Vec<f32>>,
    disp_split_offsets: Rc<Vec<f32>>,
    disp_unified_max_no: u32,
    disp_split_max_no: u32,
    /// Per-file horizontal-scroll spans (widest code line per file), lockstep
    /// with the display rows; `DiffElement` bounds each file's horizontal offset
    /// against `max_chars` instead of re-measuring rows per frame.
    disp_unified_spans: Rc<Vec<FileSpan>>,
    disp_split_spans: Rc<Vec<FileSpan>>,
    /// US-046: cumulative top offsets of each hunk's first changed row, cached
    /// in lockstep with the row sets (recomputed only in `recompute_display`).
    /// The toolbar's hunk counter renders every frame, so deriving these per
    /// frame was an O(rows) walk + allocation each repaint.
    disp_hunk_tops_unified: Rc<Vec<f32>>,
    disp_hunk_tops_split: Rc<Vec<f32>>,
    /// US-016 warm-resume: the git fingerprint (HEAD + base + status hash) this
    /// column's rows were built against, captured off-thread at load time.
    /// `DiffView::revalidate` compares a fresh fingerprint on diff-mode re-entry
    /// to re-diff ONLY columns that actually changed. `None` until first load
    /// (and on a failed load), so such a column always reloads on resume.
    fingerprint: Option<super::git::ColumnFingerprint>,
    /// Per-column comparison base override. `None` ⇒ this column diffs against
    /// the view's shared `base_ref` (e.g. `develop`); `Some(ref)` ⇒ it diffs
    /// against that ref instead - the per-commit toggle sets `Some("HEAD~1")` so
    /// one branch column can show "just my latest commit's work" while its
    /// siblings keep the whole-branch-vs-develop view.
    base_override: Option<String>,
    /// Per-column last-write-wins guard (US-007). Bumped each time THIS column
    /// is (re)loaded; the spawned task captures it and discards its result if a
    /// newer load for the same column superseded it. Per-column (not a single
    /// view-wide counter) so a subset reload - e.g. `revalidate` reloading only
    /// the columns whose fingerprint moved - never discards an in-flight full
    /// reload of the OTHER columns.
    generation: u64,
    /// The view mode currently being materialized from retained raw diffs. Kept
    /// separate from `generation`: a mode build can be superseded by another
    /// mode toggle without forcing a fresh git diff.
    loading_mode: Option<ViewMode>,
    /// Theme generation currently being rebuilt for this column. Prevents render
    /// from spawning duplicate theme-refresh jobs while an old snapshot remains
    /// visible until the new one lands.
    loading_theme_generation: Option<u64>,
    /// Review CLIs launched on this column's branch, rendered as real terminals
    /// under the diff body (prd-ai-in-diff-2026-Q3.md). Empty until the user runs
    /// Review; replaced on a re-run; closed by explicit terminal-close actions.
    review_terminals: Vec<ReviewTerminal>,
    /// Active terminal tab in the Review dock. Stored as an index because the
    /// list is column-local and recreated on every review run.
    active_review_terminal: usize,
    /// User-resizable height (px) of this column's embedded review region.
    review_height: f32,
    /// EP-004: local agent sessions matched to this worktree (cwd + branch),
    /// most-relevant first. Computed off-thread in the same task as the diff
    /// (US-014, folded into the column-load) and re-fetched only on re-diff, so
    /// per-frame render reads it O(1). Empty = no matching session (the
    /// attribution slot collapses to zero width, US-015).
    attribution: Vec<SessionMeta>,
    /// Horizontal scroll offsets (px), indexed by file in unified mode and by
    /// file+side in split mode. Consumed by the shared `DiffElement`; the Review
    /// body's wheel + scrollbar handlers mutate these slots directly.
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

    /// Rebuild the collapse-filtered views from any loaded row sets +
    /// `collapsed`. Both modes are normally loaded together so mode switches do
    /// not show a transient "Preparing diff" state.
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

    /// Cached hunk-start offsets for `mode` (US-046). Lockstep with the display
    /// rows - see [`Column::recompute_display`].
    fn hunk_tops(&self, mode: ViewMode) -> &Rc<Vec<f32>> {
        match mode {
            ViewMode::Unified => &self.disp_hunk_tops_unified,
            ViewMode::Split => &self.disp_hunk_tops_split,
        }
    }
}

/// Multi-worktree diff viewer pane.
pub struct DiffView {
    repo_root: PathBuf,
    /// Resolved base ref every column diffs against (US-013 selector).
    base_ref: String,
    /// Local branches offered by the base selector.
    branches: Vec<String>,
    /// Lowercased mirror of `branches`, precomputed once when `branches` is set so
    /// the base-popover filter never `to_lowercase()`es every branch on every
    /// keystroke / frame while the popover is open.
    branches_lc: Vec<String>,
    base_picker_open: bool,
    /// Live type-to-filter field inside the base-branch popover. Owned so the
    /// popover can be a real searchable list (the DiffView observes it to
    /// recompute matches on every keystroke).
    base_filter: Entity<TextInput>,
    columns: Vec<Column>,
    focus_handle: FocusHandle,
    element_id: SharedString,
    /// US-016 watcher epoch. Bumped by [`Self::suspend`]; [`Self::start_watchers`]
    /// captures it at spawn and (a) refuses to install the freshly-built watcher
    /// if the epoch advanced while it was building off-thread, and (b) stops the
    /// debounce loop once the epoch advances. Closes the build-race where a
    /// `suspend` between the watcher-build spawn and its completion would
    /// otherwise leave a live watcher (and event loop) on a hidden/cached entity,
    /// or a double watcher after `resume`.
    watch_epoch: u64,
    mode: ViewMode,
    /// Last mode that actually painted after responsive fallbacks. Reloads build
    /// this mode first, so a narrow viewport that forces unified never spends the
    /// initial off-thread pass on hidden split rows.
    last_effective_mode: ViewMode,
    /// When true (default), all visible columns scroll in lockstep: the
    /// vertical offset of `scroll_driver` is broadcast to the rest each render,
    /// turning N parked viewers into one comparison surface (the whole point of
    /// the side-by-side worktree view). Toggleable from the toolbar.
    sync_scroll: bool,
    /// Index of the column the user last scrolled - the offset source the sync
    /// broadcast follows. Set by each column's `on_scroll_wheel`. Sourcing only
    /// from the explicit driver (never from clamped followers) keeps the sync
    /// drift-free across columns of differing height.
    scroll_driver: usize,
    /// Column whose changed-file list feeds the sidebar and whose body
    /// `jump_to_file` scrolls. Set by clicking a column header.
    selected_column: usize,
    /// Entity-owned filesystem watchers (US-015). Dropped on tab close, which
    /// unregisters the OS handles and ends the debounce loop.
    _watchers: Vec<RecommendedWatcher>,
    /// US-016 warm-resume: `true` while the diff surface is hidden (CLI/Agents
    /// mode, or cached and not displayed). [`Self::suspend`] sets it and releases
    /// the watchers; [`Self::resume`] clears it, re-arms the watcher, and
    /// revalidates. While set, the deferred `bootstrap` completion does NOT arm a
    /// watcher (it would leak one for an invisible repo).
    suspended: bool,
    /// US-016: `true` once `bootstrap` has resolved the base + branches. Guards
    /// `resume`: if bootstrap is still in flight, clearing `suspended` is enough
    /// (bootstrap will arm the watcher + load itself); otherwise resume arms +
    /// revalidates directly.
    bootstrapped: bool,
    /// Inc 5: how the visible columns are arranged on screen - a splittable
    /// tree over column indices (side-by-side / stacked / nested), driven by
    /// drag-and-drop. Reconciled against the live columns each render, so the
    /// `Vec<Column>` and all its index-based logic stay untouched.
    arrange: Arrange,
    /// Transient drag state: `(hovered column idx, resolved drop edge)` while a
    /// `DiffColumnDrag` is in flight, so the hovered pane's overlay can preview
    /// the split. `None` edge = center (move/reorder). Cleared on drop.
    drag_target: Option<(usize, Option<DropEdge>)>,
    /// US-002/US-003 (prd-ai-in-diff-2026-Q3.md): open body context menu
    /// (right-click), carrying the resolved scope + window-space anchor. `None`
    /// when closed.
    body_menu: Option<DiffBodyMenu>,
    /// US-003: last pointer position over a column body `(col idx, window point)`,
    /// so the `Ctrl+Shift+C` action resolves the hunk under the cursor without a
    /// continuous row recompute on every move.
    last_body_pos: Option<(usize, Point<Pixels>)>,
    /// US-003: transient "copied" confirmation pill, auto-cleared by a spawned
    /// timer. Self-hosted so the diff view needs no PaneFlowApp toast handle.
    flash: Option<SharedString>,
    /// Which branch column's Review CLI multi-select popover is open (by column
    /// index), or `None` when closed. Anchored to that branch's header.
    review_menu_open: Option<usize>,
    /// Per-CLI "include in the review" toggles, aligned to [`ReviewCli::all`]
    /// order. Re-synced (default all-on) when the menu opens.
    review_picks: Vec<bool>,
    /// Active review-region resize drag: `(col_idx, start_pointer_y_px,
    /// start_height_px)`. `None` when not dragging.
    review_resizing: Option<(usize, f32, f32)>,
    /// Active horizontal-scrollbar thumb drag inside a diff body.
    h_scroll_drag: Option<DiffHScrollDrag>,
    /// When true, the column-header `×` emits [`DiffViewEvent::CloseColumn`] (the
    /// host deselects the branch from the scope) instead of locally hiding the
    /// column. Set for the Worktree scope, where a branch is either shown or not -
    /// no in-between "hidden but tracked" state with a "N hidden" pill.
    close_removes: bool,
    /// Scope breadcrumb fragment (scope › project › branches) PUSHED by
    /// `render_diff_main` every frame and consumed (`take`) by the next
    /// `render` - same push-only contract as `TitleBar`. The DiffView mounts
    /// it as the left side of its single toolbar row so the whole Diff mode
    /// has exactly one row of chrome.
    pub scope_slot: Option<AnyElement>,
}

/// Events a [`DiffView`] raises to its host (`PaneFlowApp`). Today: the user
/// asked to drop a branch column from the Worktree scope via its header `×`.
pub enum DiffViewEvent {
    CloseColumn { path: PathBuf },
}

/// US-002/US-003: an open right-click menu on the diff body, anchored at the
/// click point and pre-resolved to the file (+ optional hunk + clicked line)
/// under the cursor.
struct DiffBodyMenu {
    position: Point<Pixels>,
    col_idx: usize,
    scope: DiffBodyScope,
    mode: ViewMode,
}

/// Which file (+ optional hunk) a body point resolves to. Indices are into the
/// column's `files_full` and that file's `hunks`, resolved at action time from
/// the live rows (no stable hunk ID).
#[derive(Clone, Copy)]
struct DiffBodyScope {
    file_idx: usize,
    hunk_idx: Option<usize>,
}

impl DiffView {
    /// Build a diff view seeded with a repo's sibling worktrees, kick off the
    /// per-worktree diffs off the main thread, and start the live-refresh watch.
    pub fn new(repo_root: PathBuf, worktrees: Vec<DiffWorktree>, cx: &mut Context<Self>) -> Self {
        Self::with_base(repo_root, worktrees, None, cx)
    }

    /// Like [`Self::new`] but seeds the base ref. The multi-project host passes
    /// the last-chosen base so switching repos keeps the comparison base; when
    /// `base` is `None`, `bootstrap` resolves the default (develop→main→master).
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
        // Initial arrangement: every column side by side (mirrors the old fixed
        // flex row). Drag-and-drop reshapes this; `reconcile` keeps it in sync
        // with hide/show/reload.
        let arrange = Arrange::row(&(0..columns.len()).collect::<Vec<_>>());
        // Searchable base-branch filter. Observe it so each keystroke re-renders
        // the DiffView (and thus recomputes the filtered branch list) - the
        // TextInput only notifies itself otherwise.
        let base_filter = cx.new(|cx| TextInput::new("", "Filter branches…", cx));
        cx.observe(&base_filter, |_, _, cx| cx.notify()).detach();
        let mut view = Self {
            repo_root,
            // Seeded base (multi-project shared base) or empty until `bootstrap`
            // resolves the default off-thread - the git subprocesses must not
            // block the GPUI main thread at tab open. An empty base renders a
            // "pick a base" prompt rather than spinning on a bogus ref.
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

    /// Host opt-in: make the column-header `×` deselect the branch (emit
    /// [`DiffViewEvent::CloseColumn`]) rather than hide it in place. Used by the
    /// Worktree scope.
    pub fn set_close_removes(&mut self, v: bool) {
        self.close_removes = v;
    }

    /// Working-tree paths of the currently visible columns, in column order.
    /// Lets the host materialize the "currently shown" branch set (including
    /// on-disk-discovered columns) when deselecting one.
    pub fn column_paths(&self) -> Vec<PathBuf> {
        self.columns
            .iter()
            .filter(|c| c.visible)
            .map(|c| c.path.clone())
            .collect()
    }

    /// Resolve the base ref + branch list off the main thread, then kick off the
    /// per-column diffs and the live-refresh watcher. Doing the git subprocesses
    /// AND the (recursive, ~20k-dir) inotify registration walk off the GPUI
    /// thread is what prevents the multi-second "not responding" freeze that
    /// `new()` used to cause at tab open.
    fn bootstrap(&mut self, cx: &mut Context<Self>) {
        let first = self.columns.first().map(|c| c.path.clone());
        let n = self.columns.len();
        // Honor a seeded base (multi-project shared base); else resolve default.
        let preset = self.base_ref.clone();
        cx.spawn(async move |this, cx| {
            log::debug!("diff: bootstrap START ({n} columns); resolving base off-thread");
            let t = Instant::now();
            let (base, branches) = match first {
                Some(p) => {
                    smol::unblock(move || {
                        // Honor a seeded base (multi-project shared base) only if
                        // it actually exists in THIS repo - else fall back to the
                        // repo's own default (develop→main→master). Empty when
                        // nothing resolves, so the toolbar prompts for a base
                        // instead of failing every column on a non-existent ref.
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
                    // US-016: if the surface was hidden (parked to CLI) before
                    // bootstrap resolved, do NOT arm a watcher for an invisible
                    // repo - `resume` arms it when the user returns.
                    if !view.suspended {
                        view.start_watchers(cx);
                    }
                })
            });
        })
        .detach();
    }

    /// Tab-strip title, e.g. `Diff: paneflow`.
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

    /// Toggle a column between the shared base (e.g. `develop`) and "just my
    /// latest commit" (`HEAD~1`), reloading ONLY that column. One branch can show
    /// its last-commit delta while its siblings keep the whole-branch-vs-base
    /// view - the 80/20 of commit-granular review without a full commit walk.
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

    /// US-014: hide a column (drop its data, skip future refreshes).
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
                col.drop_loaded_data(); // dropped data; reloads on re-show
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
        // The hidden column must not remain the sync source or the selection: the
        // scroll broadcast would otherwise re-run its first-visible fallback every
        // frame, and the header selection marker would point at nothing. Re-anchor
        // both to the first still-visible column.
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

    /// US-014: re-show every hidden column and reload them.
    fn show_all_columns(&mut self, cx: &mut Context<Self>) {
        for col in &mut self.columns {
            col.visible = true;
        }
        self.start_loading(cx);
        self.restart_watchers(cx);
        cx.notify();
    }

    /// The base ref currently diffed against (read by the multi-project host to
    /// seed sibling repos with the same base).
    pub fn base_ref(&self) -> &str {
        &self.base_ref
    }

    /// Append worktree columns not already present (dedup by normalized path),
    /// load them, and re-arm the watcher to cover the new trees. Used by
    /// Worktree-scope on-disk discovery and live workspace-add so a new branch
    /// shows up without re-mounting the whole view (which would flash every
    /// column back to Loading).
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
            // start_loading keeps already-loaded columns' content until their
            // fresh diff swaps in, so only the new columns visibly start from
            // Loading. Re-arm the watcher off-thread to include the new trees.
            self.start_loading(cx);
            self.restart_watchers(cx);
        }
    }

    /// The selected column if visible, else the first visible column - the one
    /// the toolbar's diffstat / hunk-nav act on.
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

    /// True when every visible, loaded column has all of its files collapsed -
    /// the live source for the toolbar collapse/expand-all chip. Replaces a cached
    /// bool that drifted whenever per-file collapse (body click) or a live-refresh
    /// reload changed the real state without updating it.
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

    /// Toolbar: collapse every file in every visible column, or expand all.
    fn toggle_collapse_all(&mut self, cx: &mut Context<Self>) {
        // Decide from the live state, not a cached flag: if everything is already
        // collapsed, expand; otherwise collapse all.
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
    /// Floating, searchable base-branch popover anchored under the toolbar chip
    /// (US-013). Replaces the old wrapping chip-row: it floats above the diff
    /// body (no reflow), filters live as you type, marks the active base with a
    /// check, and supports keyboard (Esc to close, Enter to pick the top match).
    fn render_base_popover(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();

        let filter = self.base_filter.read(cx).value().to_lowercase();
        // Filter against the precomputed lowercase mirror so an open popover does
        // not re-`to_lowercase()` every branch on each keystroke / frame.
        let matches = base_branch::matching_indices(&self.branches_lc, &filter);

        // Header: a search field. The leading glyph + the real cursor-aware
        // `TextInput` make the popover a command-palette-style picker.
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

        // Scrollable result list with a bounded height so a 100-branch repo
        // doesn't grow the popover off-screen.
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
                            // No listed branch/tag matches - try the typed text as
                            // an arbitrary ref / SHA (validated off-thread).
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

/// Normalize a worktree path for dedup (canonicalize, lowercase on
/// case-insensitive filesystems) so the same worktree seeded from an open
/// workspace and discovered via `git worktree list` collapses to one column.
fn norm_key(p: &std::path::Path) -> String {
    let resolved = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let s = resolved.to_string_lossy().into_owned();
    if cfg!(target_os = "macos") || cfg!(target_os = "windows") {
        s.to_lowercase()
    } else {
        s
    }
}
