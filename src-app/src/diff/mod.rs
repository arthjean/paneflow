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

pub use git::{FileChange, list_repo_worktrees};
pub use multi_view::MultiRepoDiffView;
pub use scope::{DiffScope, RepoGroup};
pub use view::{
    DiffView, DiffViewEvent, DiffWorktree, FileEntry, FileListState, aggregate_file_lists,
};

pub(crate) use align::CellKind;
pub(crate) use element::{DiffBody, DiffElement, revert_chip_bounds};
pub(crate) use engine::DiffHunk;
#[cfg(test)]
pub(crate) use engine::compute_hunks;
pub(crate) use engine::{ComparisonPolicy, DiffOptions, HighlightPolicy};
pub(crate) use git::FileDiff;
pub(crate) use git::{
    HeadFile, MAX_FILE_BYTES as MAX_DIFF_FILE_BYTES, classify as classify_git_bytes,
    compute_head_diff, head_sha, show_head_file, try_worktree_toplevel,
};
pub(crate) use highlighter::{
    Grammar, MAX_CAPTURES_PER_ROW, MAX_HIGHLIGHT_BYTES, grammar_for_ext, highlight_lines,
    markdown_inline_grammar, resolve_runs,
};
pub(crate) use hit_test::row_at_offset;
pub(crate) use hscroll::{
    H_SCROLLBAR_TRACK_HEIGHT, HScrollbarSegment, file_at_row, h_offset_index, h_offset_len,
    h_scrollbar_click_offset, h_scrollbar_segments, set_file_side_offset, split_right_side_at_x,
};
pub(crate) use rows::{
    DisplayRow, FileRowCache, FileSpan, ROW_HEIGHT, RowKind, RowPalette, SplitRow,
    apply_collapse_split, apply_collapse_unified, apply_expanded_split_with_sources,
    apply_expanded_unified_with_sources, build_display_rows_with_caches, build_file_row_caches,
    build_split_rows_with_caches, discard_expanded_folds_for_path, file_ext, hunk_for_base_line,
    hunk_for_new_line, palette, split_file_spans, split_max_line_no, split_offsets,
    unified_file_spans, unified_max_line_no, unified_offsets,
};
pub(crate) use syntax::DiffSyntax;
