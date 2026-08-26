//! Multi-worktree diff viewer (prd-multi-worktree-diff-2026-Q3.md).
//!
//! EP-001 scaffold (US-003): stands up the `DiffView` GPUI entity and its tab
//! plumbing only - it renders an empty/placeholder state seeded with the
//! sibling worktrees of one repo. The diff engine (EP-002), side-by-side
//! render (EP-003), and N-column live view + base selector (EP-004) fill in
//! `DiffView` with real hunk data on top of this host.
//!
//! `DiffView` is the exact structural analog of `markdown::MarkdownView`: an
//! `Entity` implementing `Render + Focusable`, hosted in a pane via the new
//! `PaneSurface::Diff` variant. It is ephemeral - never persisted to
//! `session.json` (like markdown tabs, dropped by `layout/serde.rs`).

mod align;
mod arrange;
mod element;
mod engine;
mod extract;
mod git;
mod highlighter;
mod hit_test;
mod hscroll;
mod multi_view;
mod review_terminal;
mod rows;
mod scope;
mod scope_header;
mod syntax;
mod view;

// Only the host view + its seed type are consumed outside this module
// (`pane::PaneSurface::Diff`, `event_handlers::open_multi_diff_for_repo`). The
// engine / git / rows types stay crate-internal, reached via `super::` paths.
pub use git::{FileChange, list_repo_worktrees};
pub use multi_view::MultiRepoDiffView;
pub use scope::{DiffScope, RepoGroup};
pub use view::{
    DiffView, DiffViewEvent, DiffWorktree, FileEntry, FileListState, aggregate_file_lists,
};

// EP-001 (review redesign, US-001/US-002): the Agents diff dock
// (`crate::app::diff_dock`) renders through the SAME `DiffElement` + git
// pipeline + row model as the Review view, so these are exposed crate-internally
// rather than re-implemented. Kept `pub(crate)` (not `pub`) so the unification
// surface stays inside the binary.
pub(crate) use element::{DiffBody, DiffElement};
pub(crate) use git::FileDiff;
pub(crate) use git::compute_head_diff;
pub(crate) use highlighter::{
    Grammar, MAX_HIGHLIGHT_BYTES, grammar_for_ext, markdown_inline_grammar, resolve_runs,
};
// The editor drives its own incremental parse; it calls the diff's one-shot
// entry point only to assert the two produce identical runs
// (prd-file-editor-2026-Q3, US-004 parity test).
#[cfg(test)]
pub(crate) use highlighter::highlight_lines;
pub(crate) use hit_test::row_at_offset;
pub(crate) use hscroll::{
    H_SCROLLBAR_TRACK_HEIGHT, HScrollbarSegment, file_at_row, h_offset_index, h_offset_len,
    h_scrollbar_click_offset, h_scrollbar_segments, set_file_side_offset, split_right_side_at_x,
};
pub(crate) use rows::{
    DisplayRow, FileRowCache, FileSpan, ROW_HEIGHT, RowKind, RowPalette, SplitRow,
    apply_collapse_split, apply_collapse_unified, apply_expanded_split_with_sources,
    apply_expanded_unified_with_sources, build_display_rows_with_caches, build_file_row_caches,
    build_split_rows_with_caches, discard_expanded_folds_for_path, file_ext, palette,
    split_file_spans, split_max_line_no, split_offsets, unified_file_spans, unified_max_line_no,
    unified_offsets,
};
pub(crate) use syntax::DiffSyntax;
