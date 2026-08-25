//! Unified diff display-row model + per-row rendering (US-006,
//! prd-multi-worktree-diff-2026-Q3.md).
//!
//! `build_display_rows` reconstructs a standard unified diff (context / removed
//! / added lines, with file headers) as a flat row list consumed by the custom
//! virtualized `super::element::DiffElement`, which culls to the visible window
//! using the precomputed per-row offsets (file headers are taller; other rows
//! use the compact line height). Side-by-side rendering (LHS/RHS with phantom
//! rows) is EP-003 (US-008/US-009); this is the EP-002 unified view.

use std::collections::HashSet;
use std::ops::Range;

use gpui::{Hsla, SharedString};

use super::git::{FileChange, FileDiff};
use super::syntax::DiffSyntax;

const MAX_FULL_SPLIT_ALIGN_LINES: usize = 20_000;

/// Content row height (CSS px) - compact, one diff line.
pub const ROW_HEIGHT: f32 = 18.0;

/// File-header row height. Taller than a content line so each file reads as a
/// padded, breathing "card" header (Zed buffer-subheader feel) while content
/// lines stay compact. The custom element lays rows out at variable heights
/// keyed off [`display_row_height`] / [`split_row_height`].
pub const FILE_HEADER_HEIGHT: f32 = 32.0;

/// Collapsed unchanged-region row height. This row is a compact control, not a
/// normal code line, so it needs room for the rounded surface and fold icon.
pub const FOLD_ROW_HEIGHT: f32 = 32.0;

/// Laid-out height of one unified row: a padded card for file headers, the
/// compact line height for everything else.
pub fn display_row_height(row: &DisplayRow) -> f32 {
    match row.kind {
        RowKind::FileHeader => FILE_HEADER_HEIGHT,
        RowKind::Fold => FOLD_ROW_HEIGHT,
        _ => ROW_HEIGHT,
    }
}

/// Side-by-side analog of [`display_row_height`].
pub fn split_row_height(row: &SplitRow) -> f32 {
    match row {
        SplitRow::Header(_) => FILE_HEADER_HEIGHT,
        SplitRow::Fold(_) => FOLD_ROW_HEIGHT,
        _ => ROW_HEIGHT,
    }
}

/// Cumulative top offsets (px) for a unified row set: `offsets[i]` is the top of
/// row `i`, `offsets[len]` the total content height. Precomputed off the render
/// path (in `Column::recompute_display`) and shared with `DiffElement`, which
/// culls + lays out against it instead of re-walking every row each frame.
pub fn unified_offsets(rows: &[DisplayRow]) -> Vec<f32> {
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(rows.len() + 1);
    out.push(0.0);
    for r in rows {
        acc += display_row_height(r);
        out.push(acc);
    }
    out
}

/// Side-by-side analog of [`unified_offsets`].
pub fn split_offsets(rows: &[SplitRow]) -> Vec<f32> {
    let mut acc = 0.0;
    let mut out = Vec::with_capacity(rows.len() + 1);
    out.push(0.0);
    for r in rows {
        acc += split_row_height(r);
        out.push(acc);
    }
    out
}

/// Cumulative top offsets (px) of each hunk's first changed row in a unified row
/// set. A "hunk start" is a change row (Added/Removed) whose predecessor is not
/// a change, so consecutive removed+added lines count as one hunk. Precomputed
/// in `Column::recompute_display` (US-046) so the toolbar's hunk counter and
/// hunk-nav read it per frame instead of re-walking every row.
pub fn unified_hunk_tops(rows: &[DisplayRow]) -> Vec<f32> {
    let mut tops = Vec::new();
    let mut acc = 0.0f32;
    let mut prev_change = false;
    for r in rows {
        let is_change = matches!(r.kind, RowKind::Added | RowKind::Removed);
        if is_change && !prev_change {
            tops.push(acc);
        }
        prev_change = is_change;
        acc += display_row_height(r);
    }
    tops
}

/// Side-by-side analog of [`unified_hunk_tops`].
pub fn split_hunk_tops(rows: &[SplitRow]) -> Vec<f32> {
    let mut tops = Vec::new();
    let mut acc = 0.0f32;
    let mut prev_change = false;
    for r in rows {
        let is_change = matches!(
            r,
            SplitRow::Pair { left, right }
                if matches!(left.kind, CellKind::Added | CellKind::Removed)
                    || matches!(right.kind, CellKind::Added | CellKind::Removed)
        );
        if is_change && !prev_change {
            tops.push(acc);
        }
        prev_change = is_change;
        acc += split_row_height(r);
    }
    tops
}

/// Widest line number across a unified row set (both sides); `0` when empty.
/// Used to size the line-number gutter once at build time.
pub fn unified_max_line_no(rows: &[DisplayRow]) -> u32 {
    rows.iter()
        .map(|r| r.new_no.unwrap_or(0).max(r.old_no.unwrap_or(0)))
        .max()
        .unwrap_or(0)
}

/// Side-by-side analog of [`unified_max_line_no`].
pub fn split_max_line_no(rows: &[SplitRow]) -> u32 {
    rows.iter()
        .map(|r| match r {
            SplitRow::Pair { left, right } => left.no.unwrap_or(0).max(right.no.unwrap_or(0)),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

/// One file's extent in a display-row set: the index of its `FileHeader` row
/// and the width (in monospace cells) of its widest code line. The widest-line
/// width drives the file's horizontal-scroll bound; split rows additionally
/// store the right-side width. These spans are precomputed off the render path
/// (in `recompute_display` / `AgentsDiffData::recompute`) and shared with
/// `DiffElement`, which offsets each file side's code by its own scroll
/// position instead of re-measuring every row per frame. Widths count `char`s
/// (not bytes), matching the monospace-cell estimate the element scrolls by;
/// they are `0` for a collapsed file (header only) or a binary/fold-only file.
#[derive(Clone, Copy)]
pub struct FileSpan {
    pub header_row: usize,
    /// Unified width, or split-left width when `right_max_chars` is `Some`.
    pub max_chars: usize,
    /// Split-right width. `None` means the span belongs to unified mode.
    pub right_max_chars: Option<usize>,
}

/// Per-file spans for a unified row set, one entry per `FileHeader` in file
/// order (so `partition_point` on `header_row` maps any row back to its file).
pub fn unified_file_spans(rows: &[DisplayRow]) -> Vec<FileSpan> {
    let mut spans: Vec<FileSpan> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match r.kind {
            RowKind::FileHeader => spans.push(FileSpan {
                header_row: i,
                max_chars: 0,
                right_max_chars: None,
            }),
            RowKind::Context | RowKind::Added | RowKind::Removed => {
                if let Some(span) = spans.last_mut() {
                    span.max_chars = span.max_chars.max(r.text.chars().count());
                }
            }
            // Folds, binary notes and the truncation row never scroll.
            RowKind::Fold | RowKind::Binary | RowKind::Truncated => {}
        }
    }
    spans
}

/// Side-by-side analog of [`unified_file_spans`]. Each `Pair` row contributes
/// to the matching half's width, so split horizontal scroll can clamp left and
/// right independently.
pub fn split_file_spans(rows: &[SplitRow]) -> Vec<FileSpan> {
    let mut spans: Vec<FileSpan> = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match r {
            SplitRow::Header(_) => spans.push(FileSpan {
                header_row: i,
                max_chars: 0,
                right_max_chars: Some(0),
            }),
            SplitRow::Pair { left, right } => {
                if let Some(span) = spans.last_mut() {
                    span.max_chars = span.max_chars.max(left.text.chars().count());
                    let right_max = span.right_max_chars.get_or_insert(0);
                    *right_max = (*right_max).max(right.text.chars().count());
                }
            }
            SplitRow::Note(_) | SplitRow::Fold(_) => {}
        }
    }
    spans
}

/// Cap on rendered rows across a whole column. Beyond this the column shows a
/// truncation notice instead of freezing the frame on a pathological diff.
pub const MAX_DISPLAY_ROWS: usize = 10_000;

/// Unchanged lines kept on each side of a hunk before the middle is collapsed
/// into a [`RowKind::Fold`] marker (mirrors Zed's default diff context).
pub const CONTEXT_LINES: u32 = 3;

/// Per-file caches shared by unified/split row builders for one diff snapshot.
/// The Review view keeps these next to `files_full` so the inactive mode and
/// later fold expansions reuse the syntax pass instead of recomputing it.
#[derive(Clone, Default)]
pub struct FileRowCache {
    syn_old: Vec<Vec<(Range<usize>, Hsla)>>,
    syn_new: Vec<Vec<(Range<usize>, Hsla)>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    FileHeader,
    Context,
    Added,
    Removed,
    Binary,
    Truncated,
    /// Collapsed run of unchanged lines between/around hunks (Zed-style fold).
    Fold,
}

#[derive(Clone)]
pub struct DisplayRow {
    pub kind: RowKind,
    pub text: SharedString,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Per-token foreground syntax runs (US-017), computed off-thread; empty
    /// when syntax highlighting is disabled or the line is plain.
    pub syntax_runs: Vec<(Range<usize>, Hsla)>,
    /// EP-002 US-006: typed file-header segments for [`RowKind::FileHeader`]
    /// rows (`None` for every other kind). Decomposes the header into directory
    /// prefix + basename + diffstat so the element paints each as its own typed
    /// run instead of one fused monospace string.
    pub header: Option<HeaderParts>,
    /// Stable identity for a collapsed unchanged region. Present only on
    /// [`RowKind::Fold`] rows, and used by the Review view to toggle that region
    /// open without recomputing the git diff.
    pub fold_key: Option<SharedString>,
    /// Source range behind a fold marker. Hidden rows are rebuilt lazily from
    /// `FileDiff` + `FileRowCache` on expansion; the marker stays visible so
    /// clicking it again closes the region.
    pub fold_base_start: Option<u32>,
    pub fold_new_start: Option<u32>,
    pub fold_count: u32,
    /// Legacy eager payload. New builders leave this empty so initial Review
    /// paint does not materialize thousands of hidden context rows.
    pub folded_rows: Vec<DisplayRow>,
}

#[derive(Clone)]
pub struct FoldBlock<T> {
    pub key: SharedString,
    pub text: SharedString,
    pub base_start: u32,
    pub new_start: u32,
    pub count: u32,
    pub rows: Vec<T>,
}

/// EP-002 US-006: the file-header row, split into typed segments at build time
/// (off the render path) so [`super::element::DiffElement`] paints a structured
/// header - file-type icon, muted directory prefix, emphasized basename,
/// right-aligned green/red diffstat, and trailing actions - instead of one
/// undifferentiated mono string. Shared by the Review view and the Agents diff
/// dock.
#[derive(Clone)]
pub struct HeaderParts {
    /// Directory portion including the trailing `/`, or `""` at the repo root.
    /// The element truncates HERE under width pressure, never on the basename.
    pub dir_prefix: SharedString,
    /// File basename (a rename shows `old → new`). Never truncated.
    pub basename: SharedString,
    pub added: u32,
    pub removed: u32,
}

/// Split a display path into `(dir_prefix_with_trailing_slash, basename)`.
/// For a rename (`"old → new"`) the last `/` lands inside the new path, so the
/// new file's basename is emphasized and the `old → newdir/` lead falls into the
/// muted directory prefix - readable, allocation-cheap, never panics.
fn split_header_path(shown_path: &str) -> (String, String) {
    match shown_path.rfind('/') {
        Some(i) => (
            shown_path[..=i].to_string(),
            shown_path[i + 1..].to_string(),
        ),
        None => (String::new(), shown_path.to_string()),
    }
}

/// Diff colors, snapshotted once per render and copied into the (`'static`)
/// `super::element::DiffElement`, which owns its row data for the frame.
#[derive(Clone, Copy)]
pub struct RowPalette {
    pub text: Hsla,
    pub muted: Hsla,
    pub header_bg: Hsla,
    /// Background of the pinned sticky file header that tracks the viewport top
    /// while a file's hunks scroll under it. Opaque + slightly elevated so it
    /// reads as floating above the scrolling body.
    pub sticky_header_bg: Hsla,
    pub add_bg: Hsla,
    pub del_bg: Hsla,
    pub add_fg: Hsla,
    pub del_fg: Hsla,
    /// Gutter line-number tint for changed lines - added/removed numbers read in
    /// a status hue instead of flat `muted`, so the eye finds the changed lines
    /// from the gutter alone (GitHub / Zed behaviour). Context numbers stay
    /// `muted`.
    pub gutter_add: Hsla,
    pub gutter_del: Hsla,
    /// File-type icon accent used by the compact file-list headers.
    pub file_icon_hot: Hsla,
    /// Opaque hunk-indicator bar colors. Zed blends the raw `version_control_*`
    /// color with the editor background before painting the gutter strip so it
    /// reads solid; we pre-blend once in `view.rs::palette()`.
    pub add_bar: Hsla,
    pub del_bar: Hsla,
    /// Background for balancing phantom cells in the side-by-side view.
    pub phantom_bg: Hsla,
    pub phantom_hatch: Hsla,
    /// EP-002 US-007: faint document wash on context (unchanged) code so the
    /// diff body reads as a surface, not the bare window background.
    pub context_bg: Hsla,
    /// EP-002 US-007: persistent line-number-rail tint, painted over every
    /// content row's gutter region so the gutter reads as a structural column.
    pub gutter_bg: Hsla,
    pub add_gutter_bg: Hsla,
    pub del_gutter_bg: Hsla,
}

fn content_row(
    lines: &[&str],
    idx: u32,
    kind: RowKind,
    old_no: Option<u32>,
    new_no: Option<u32>,
    syntax_runs: Vec<(Range<usize>, Hsla)>,
) -> DisplayRow {
    DisplayRow {
        kind,
        text: lines
            .get(idx as usize)
            .copied()
            .unwrap_or("")
            .to_string()
            .into(),
        old_no,
        new_no,
        syntax_runs,
        header: None,
        fold_key: None,
        fold_base_start: None,
        fold_new_start: None,
        fold_count: 0,
        folded_rows: Vec::new(),
    }
}

fn fold_label(n: u32) -> SharedString {
    if n == 1 {
        "1 unmodified line".into()
    } else {
        format!("{n} unmodified lines").into()
    }
}

fn fold_key(path: &str, base_start: u32, new_start: u32, count: u32) -> SharedString {
    format!("{path}:{base_start}:{new_start}:{count}").into()
}

pub fn discard_expanded_folds_for_path(expanded: &mut HashSet<String>, path: &str) {
    let prefix = format!("{path}:");
    expanded.retain(|key| !key.starts_with(&prefix));
}

fn folded_display_row(path: &str, base_start: u32, new_start: u32, count: u32) -> DisplayRow {
    DisplayRow {
        kind: RowKind::Fold,
        text: fold_label(count),
        old_no: None,
        new_no: None,
        syntax_runs: Vec::new(),
        header: None,
        fold_key: Some(fold_key(path, base_start, new_start, count)),
        fold_base_start: Some(base_start),
        fold_new_start: Some(new_start),
        fold_count: count,
        folded_rows: Vec::new(),
    }
}

fn folded_split_row(path: &str, base_start: u32, new_start: u32, count: u32) -> SplitRow {
    SplitRow::Fold(FoldBlock {
        key: fold_key(path, base_start, new_start, count),
        text: fold_label(count),
        base_start,
        new_start,
        count,
        rows: Vec::new(),
    })
}

fn truncated_display_row(dropped: usize) -> DisplayRow {
    DisplayRow {
        kind: RowKind::Truncated,
        text: format!("diff truncated - {dropped} more lines not shown").into(),
        old_no: None,
        new_no: None,
        syntax_runs: Vec::new(),
        header: None,
        fold_key: None,
        fold_base_start: None,
        fold_new_start: None,
        fold_count: 0,
        folded_rows: Vec::new(),
    }
}

fn truncated_split_note(dropped: usize) -> SplitRow {
    SplitRow::Note(format!("diff truncated - {dropped} more lines not shown").into())
}

fn push_display_capped(row: DisplayRow, rows: &mut Vec<DisplayRow>, dropped: &mut usize) {
    if rows.len() >= MAX_DISPLAY_ROWS {
        *dropped += 1;
    } else {
        rows.push(row);
    }
}

fn push_split_capped(row: SplitRow, rows: &mut Vec<SplitRow>, dropped: &mut usize) {
    if rows.len() >= MAX_DISPLAY_ROWS {
        *dropped += 1;
    } else {
        rows.push(row);
    }
}

fn extend_display_capped<I>(iter: I, rows: &mut Vec<DisplayRow>, dropped: &mut usize)
where
    I: IntoIterator<Item = DisplayRow>,
{
    for row in iter {
        push_display_capped(row, rows, dropped);
    }
}

fn extend_split_capped<I>(iter: I, rows: &mut Vec<SplitRow>, dropped: &mut usize)
where
    I: IntoIterator<Item = SplitRow>,
{
    for row in iter {
        push_split_capped(row, rows, dropped);
    }
}

/// File extension (lowercased) used to pick a `syntect` grammar.
fn file_ext(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Per-line syntax runs for one side of a file, or an empty vec when syntax
/// highlighting is off / the file is binary. Indexed to match `str::lines()`.
fn side_syntax(
    syntax: Option<&DiffSyntax>,
    file: &FileDiff,
    text: &str,
) -> Vec<Vec<(Range<usize>, Hsla)>> {
    match syntax {
        Some(s) if !file.is_binary => {
            super::highlighter::highlight_lines(text, &file_ext(&file.path), s)
        }
        _ => Vec::new(),
    }
}

fn line_syntax(side: &[Vec<(Range<usize>, Hsla)>], idx: u32) -> Vec<(Range<usize>, Hsla)> {
    side.get(idx as usize).cloned().unwrap_or_default()
}

fn build_file_row_cache(file: &FileDiff, syntax: Option<&DiffSyntax>) -> FileRowCache {
    if file.is_binary {
        return FileRowCache::default();
    }
    FileRowCache {
        syn_old: side_syntax(syntax, file, &file.base_text),
        syn_new: side_syntax(syntax, file, &file.new_text),
    }
}

pub fn build_file_row_caches(files: &[FileDiff], syntax: Option<&DiffSyntax>) -> Vec<FileRowCache> {
    files
        .iter()
        .map(|file| build_file_row_cache(file, syntax))
        .collect()
}

/// Resolve a [`RowPalette`] from the active theme's UI colors. The single color
/// source for [`super::element::DiffElement`], shared by the Review view
/// ([`super::view`]) and the Agents diff dock ([`crate::app::agents_diff`]) so
/// both render with identical washes.
pub fn palette(ui: crate::theme::UiColors) -> RowPalette {
    let diff = ui.diff_colors();
    RowPalette {
        text: ui.text,
        muted: ui.muted,
        // EP-002 US-005: file card at the `surface` tier (one step off the
        // `base` body), so each file reads as an elevated card over the body.
        header_bg: ui.surface,
        // EP-002 US-005/US-008: the sticky bar is the file header pinned to the
        // viewport top; it must read as FLOATING above the inline card, not
        // identical to it. A faint text-tint lift shifts it off `surface` on
        // both themes (lighter on dark, defined on light); the bottom hairline
        // the element paints completes the "floating layer" read.
        sticky_header_bg: ui.surface.blend(ui.text.opacity(0.06)),
        add_bg: diff.added_background,
        del_bg: diff.deleted_background,
        add_fg: diff.added,
        del_fg: diff.deleted,
        // Gutter numbers for changed lines use the sampled colors directly to
        // match the reference diff gutters.
        gutter_add: diff.added,
        gutter_del: diff.deleted,
        file_icon_hot: ui.vc_conflict,
        // Zed paints the gutter hunk strip as `editor_background.blend(version_control_*)`
        // so it reads solid; pre-blend against the diff body surface (`ui.base`,
        // what context lines sit on) so the bar is opaque, not faint at the wash alpha.
        add_bar: ui.base.blend(diff.added),
        del_bar: ui.base.blend(diff.deleted),
        // Empty split-side cells: base fill plus subtle diagonal hatches.
        phantom_bg: ui.base,
        phantom_hatch: ui.surface,
        // EP-002 US-007: intra-line word emphasis. Light keeps the theme's 0.40
        // `vc_word_*` alpha; dark drops to 0.28 - 0.40 read too hot over the
        // Codex-sampled dark line wash.
        // Neutral rows and phantom gutters sit on the editor background; the
        // colored add/delete washes carry the visual hierarchy.
        context_bg: ui.base,
        gutter_bg: ui.base,
        add_gutter_bg: diff.added_gutter_background,
        del_gutter_bg: diff.deleted_gutter_background,
    }
}

/// Build the flat, virtualization-ready row list for a column's files. Returns
/// the rows plus the number of content lines dropped by the `MAX_DISPLAY_ROWS`
/// cap (0 when nothing was truncated).
#[cfg(test)]
pub fn build_display_rows(
    files: &[FileDiff],
    syntax: Option<&DiffSyntax>,
) -> (Vec<DisplayRow>, usize) {
    let caches = build_file_row_caches(files, syntax);
    build_display_rows_with_caches(files, &caches)
}

pub fn build_display_rows_with_caches(
    files: &[FileDiff],
    caches: &[FileRowCache],
) -> (Vec<DisplayRow>, usize) {
    let mut rows: Vec<DisplayRow> = Vec::new();
    let mut dropped = 0usize;

    for (file_idx, file) in files.iter().enumerate() {
        if rows.len() >= MAX_DISPLAY_ROWS {
            dropped += 1;
            continue;
        }
        let (added, removed) = file.line_counts();
        let shown_path = match (file.change, &file.old_path) {
            (FileChange::Renamed, Some(old)) => format!("{old} → {}", file.path),
            _ => file.path.clone(),
        };
        let (dir_prefix, basename) = split_header_path(&shown_path);
        rows.push(DisplayRow {
            kind: RowKind::FileHeader,
            text: format!("{shown_path}   +{added} -{removed}").into(),
            old_no: None,
            new_no: None,
            syntax_runs: Vec::new(),
            header: Some(HeaderParts {
                dir_prefix: dir_prefix.into(),
                basename: basename.into(),
                added,
                removed,
            }),
            fold_key: None,
            fold_base_start: None,
            fold_new_start: None,
            fold_count: 0,
            folded_rows: Vec::new(),
        });

        if file.is_binary {
            rows.push(DisplayRow {
                kind: RowKind::Binary,
                text: "Diff not shown (binary or large file)".into(),
                old_no: None,
                new_no: None,
                syntax_runs: Vec::new(),
                header: None,
                fold_key: None,
                fold_base_start: None,
                fold_new_start: None,
                fold_count: 0,
                folded_rows: Vec::new(),
            });
            continue;
        }

        let base_lines: Vec<&str> = file.base_text.lines().collect();
        let new_lines: Vec<&str> = file.new_text.lines().collect();
        let fallback_cache;
        let cache = match caches.get(file_idx) {
            Some(cache) => cache,
            None => {
                fallback_cache = FileRowCache::default();
                &fallback_cache
            }
        };
        let syn_old = &cache.syn_old;
        let syn_new = &cache.syn_new;
        let mut bc = 0u32; // base cursor (next unconsumed base row)
        let mut nc = 0u32; // new cursor (next unconsumed new row)

        let push = |row: DisplayRow, rows: &mut Vec<DisplayRow>, dropped: &mut usize| {
            if rows.len() >= MAX_DISPLAY_ROWS {
                *dropped += 1;
            } else {
                rows.push(row);
            }
        };

        // One equal/context line at (base `bi`, new `ni`).
        let ctx = |bi: u32, ni: u32| {
            content_row(
                &new_lines,
                ni,
                RowKind::Context,
                Some(bi + 1),
                Some(ni + 1),
                line_syntax(syn_new, ni),
            )
        };
        let fold = |base_start: u32, new_start: u32, n: u32| {
            folded_display_row(&file.path, base_start, new_start, n)
        };

        let mut first_gap = true;
        for h in &file.hunks {
            // Equal region before this hunk: keep CONTEXT_LINES bordering the
            // hunk and collapse the middle. The very first gap has no preceding
            // hunk, so it shows no lead context (Zed folds the top of the file).
            let gap = h.new_row_range.start - nc;
            let lead = if first_gap { 0 } else { CONTEXT_LINES.min(gap) };
            let trail = CONTEXT_LINES.min(gap - lead);
            let hidden = gap - lead - trail;
            for k in 0..lead {
                push(ctx(bc + k, nc + k), &mut rows, &mut dropped);
            }
            if hidden > 0 {
                push(fold(bc + lead, nc + lead, hidden), &mut rows, &mut dropped);
            }
            for k in 0..trail {
                let off = lead + hidden + k;
                push(ctx(bc + off, nc + off), &mut rows, &mut dropped);
            }
            for r in h.base_row_range.clone() {
                push(
                    content_row(
                        &base_lines,
                        r,
                        RowKind::Removed,
                        Some(r + 1),
                        None,
                        line_syntax(syn_old, r),
                    ),
                    &mut rows,
                    &mut dropped,
                );
            }
            bc = h.base_row_range.end;
            for r in h.new_row_range.clone() {
                push(
                    content_row(
                        &new_lines,
                        r,
                        RowKind::Added,
                        None,
                        Some(r + 1),
                        line_syntax(syn_new, r),
                    ),
                    &mut rows,
                    &mut dropped,
                );
            }
            nc = h.new_row_range.end;
            first_gap = false;
        }
        // Trailing equal region after the last hunk: keep CONTEXT_LINES, collapse
        // the rest down to EOF.
        let tail = new_lines.len() as u32 - nc;
        let lead = CONTEXT_LINES.min(tail);
        for k in 0..lead {
            push(ctx(bc + k, nc + k), &mut rows, &mut dropped);
        }
        let hidden = tail - lead;
        if hidden > 0 {
            push(fold(bc + lead, nc + lead, hidden), &mut rows, &mut dropped);
        }
    }

    if dropped > 0 {
        rows.push(truncated_display_row(dropped));
    }
    (rows, dropped)
}

// ── Side-by-side (split) rows (US-009) ──────────────────────────────────────

use super::align::{AlignedRow, Cell, CellKind, align_rows};

/// One resolved half (left=base or right=new) of a side-by-side row.
#[derive(Clone)]
pub struct HalfCell {
    pub kind: CellKind,
    pub no: Option<u32>,
    pub text: SharedString,
    pub syntax_runs: Vec<(Range<usize>, Hsla)>,
}

/// A row of the side-by-side view. `Pair` holds both halves so the two sides
/// share one row (and therefore one scroll offset - US-011 sync scroll is free).
#[derive(Clone)]
pub enum SplitRow {
    /// File-section header. EP-002 US-006: carries the same typed
    /// [`HeaderParts`] as the unified [`DisplayRow::header`] so split + unified
    /// paint identical structured headers.
    Header(HeaderParts),
    Note(SharedString),
    /// Collapsed run of unchanged lines (Zed-style fold), spanning both halves.
    Fold(FoldBlock<SplitRow>),
    Pair {
        left: HalfCell,
        right: HalfCell,
    },
}

fn resolve_half(cell: Cell, lines: &[&str], syntax: &[Vec<(Range<usize>, Hsla)>]) -> HalfCell {
    let (no, text, syntax_runs) = match cell.kind {
        CellKind::Phantom => (
            None,
            SharedString::default(),
            Vec::<(Range<usize>, Hsla)>::new(),
        ),
        _ => {
            let idx = cell.line.unwrap_or(0);
            (
                Some(idx + 1),
                lines
                    .get(idx as usize)
                    .copied()
                    .unwrap_or("")
                    .to_string()
                    .into(),
                line_syntax(syntax, idx),
            )
        }
    };
    HalfCell {
        kind: cell.kind,
        no,
        text,
        syntax_runs,
    }
}

fn split_pair_row(
    left: Cell,
    right: Cell,
    base_lines: &[&str],
    new_lines: &[&str],
    syn_old: &[Vec<(Range<usize>, Hsla)>],
    syn_new: &[Vec<(Range<usize>, Hsla)>],
) -> SplitRow {
    SplitRow::Pair {
        left: resolve_half(left, base_lines, syn_old),
        right: resolve_half(right, new_lines, syn_new),
    }
}

#[allow(clippy::too_many_arguments)]
fn build_split_rows_from_hunk_windows(
    file: &FileDiff,
    base_lines: &[&str],
    new_lines: &[&str],
    syn_old: &[Vec<(Range<usize>, Hsla)>],
    syn_new: &[Vec<(Range<usize>, Hsla)>],
    rows: &mut Vec<SplitRow>,
    dropped: &mut usize,
) {
    let emit_pair = |left: Cell, right: Cell, rows: &mut Vec<SplitRow>, dropped: &mut usize| {
        if rows.len() >= MAX_DISPLAY_ROWS {
            *dropped += 1;
        } else {
            rows.push(split_pair_row(
                left, right, base_lines, new_lines, syn_old, syn_new,
            ));
        }
    };
    let emit_fold =
        |base_start: u32, new_start: u32, n: u32, rows: &mut Vec<SplitRow>, dropped: &mut usize| {
            if n == 0 {
                return;
            }
            if rows.len() >= MAX_DISPLAY_ROWS {
                *dropped += 1;
            } else {
                rows.push(folded_split_row(&file.path, base_start, new_start, n));
            }
        };
    let emit_context_range = |base_start: u32,
                              base_end: u32,
                              new_start: u32,
                              rows: &mut Vec<SplitRow>,
                              dropped: &mut usize| {
        let count = base_end.saturating_sub(base_start);
        for k in 0..count {
            emit_pair(
                Cell {
                    kind: CellKind::Context,
                    line: Some(base_start + k),
                },
                Cell {
                    kind: CellKind::Context,
                    line: Some(new_start + k),
                },
                rows,
                dropped,
            );
        }
    };

    #[derive(Clone, Copy)]
    struct Window {
        first_hunk: usize,
        last_hunk_exclusive: usize,
        base_start: u32,
        base_end: u32,
        new_start: u32,
        new_end: u32,
    }

    let mut windows: Vec<Window> = Vec::new();
    for (hunk_index, h) in file.hunks.iter().enumerate() {
        let base_start = h.base_row_range.start.saturating_sub(CONTEXT_LINES);
        let base_end = (h.base_row_range.end + CONTEXT_LINES).min(base_lines.len() as u32);
        let new_start = h.new_row_range.start.saturating_sub(CONTEXT_LINES);
        let new_end = (h.new_row_range.end + CONTEXT_LINES).min(new_lines.len() as u32);
        if let Some(last) = windows.last_mut()
            && base_start <= last.base_end
            && new_start <= last.new_end
        {
            last.last_hunk_exclusive = hunk_index + 1;
            last.base_end = last.base_end.max(base_end);
            last.new_end = last.new_end.max(new_end);
        } else {
            windows.push(Window {
                first_hunk: hunk_index,
                last_hunk_exclusive: hunk_index + 1,
                base_start,
                base_end,
                new_start,
                new_end,
            });
        }
    }

    let mut bc = 0u32;
    let mut nc = 0u32;
    for window in windows {
        emit_fold(
            bc,
            nc,
            window
                .base_start
                .saturating_sub(bc)
                .min(window.new_start.saturating_sub(nc)),
            rows,
            dropped,
        );
        let mut context_base = window.base_start.max(bc);
        let mut context_new = window.new_start.max(nc);
        for h in &file.hunks[window.first_hunk..window.last_hunk_exclusive] {
            emit_context_range(
                context_base,
                h.base_row_range.start,
                context_new,
                rows,
                dropped,
            );

            let rem_start = h.base_row_range.start;
            let add_start = h.new_row_range.start;
            let rem_len = h.base_row_range.end - rem_start;
            let add_len = h.new_row_range.end - add_start;
            for k in 0..rem_len.max(add_len) {
                let left = if k < rem_len {
                    Cell {
                        kind: CellKind::Removed,
                        line: Some(rem_start + k),
                    }
                } else {
                    Cell {
                        kind: CellKind::Phantom,
                        line: None,
                    }
                };
                let right = if k < add_len {
                    Cell {
                        kind: CellKind::Added,
                        line: Some(add_start + k),
                    }
                } else {
                    Cell {
                        kind: CellKind::Phantom,
                        line: None,
                    }
                };
                emit_pair(left, right, rows, dropped);
            }
            context_base = h.base_row_range.end;
            context_new = h.new_row_range.end;
        }
        emit_context_range(context_base, window.base_end, context_new, rows, dropped);
        bc = window.base_end;
        nc = window.new_end;
    }

    let remaining = (base_lines.len() as u32)
        .saturating_sub(bc)
        .min((new_lines.len() as u32).saturating_sub(nc));
    emit_fold(bc, nc, remaining, rows, dropped);
}

/// Build the side-by-side row list for a column's files (US-009). Aligns each
/// file with [`align_rows`] and resolves cells to text. Honors the same
/// `MAX_DISPLAY_ROWS` cap as the unified builder.
#[cfg(test)]
pub fn build_split_rows(files: &[FileDiff], syntax: Option<&DiffSyntax>) -> (Vec<SplitRow>, usize) {
    let caches = build_file_row_caches(files, syntax);
    build_split_rows_with_caches(files, &caches)
}

pub fn build_split_rows_with_caches(
    files: &[FileDiff],
    caches: &[FileRowCache],
) -> (Vec<SplitRow>, usize) {
    let mut rows: Vec<SplitRow> = Vec::new();
    let mut dropped = 0usize;

    for (file_idx, file) in files.iter().enumerate() {
        if rows.len() >= MAX_DISPLAY_ROWS {
            dropped += 1;
            continue;
        }
        let (added, removed) = file.line_counts();
        let shown_path = match (file.change, &file.old_path) {
            (FileChange::Renamed, Some(old)) => format!("{old} → {}", file.path),
            _ => file.path.clone(),
        };
        let (dir_prefix, basename) = split_header_path(&shown_path);
        rows.push(SplitRow::Header(HeaderParts {
            dir_prefix: dir_prefix.into(),
            basename: basename.into(),
            added,
            removed,
        }));

        if file.is_binary {
            rows.push(SplitRow::Note(
                "Diff not shown (binary or large file)".into(),
            ));
            continue;
        }

        let base_lines: Vec<&str> = file.base_text.lines().collect();
        let new_lines: Vec<&str> = file.new_text.lines().collect();
        let fallback_cache;
        let cache = match caches.get(file_idx) {
            Some(cache) => cache,
            None => {
                fallback_cache = FileRowCache::default();
                &fallback_cache
            }
        };
        let syn_old = &cache.syn_old;
        let syn_new = &cache.syn_new;
        if base_lines.len() + new_lines.len() > MAX_FULL_SPLIT_ALIGN_LINES {
            build_split_rows_from_hunk_windows(
                file,
                &base_lines,
                &new_lines,
                syn_old,
                syn_new,
                &mut rows,
                &mut dropped,
            );
            continue;
        }
        let aligned = align_rows(&file.hunks, base_lines.len() as u32, new_lines.len() as u32);
        // Collapse runs of unchanged (context-on-both-sides) aligned rows the
        // same way the unified builder does: keep CONTEXT_LINES bordering each
        // change, fold the middle (and the file head/tail) into one marker.
        let emit_pair = |a: &AlignedRow, rows: &mut Vec<SplitRow>, dropped: &mut usize| {
            if rows.len() >= MAX_DISPLAY_ROWS {
                *dropped += 1;
            } else {
                rows.push(split_pair_row(
                    a.left,
                    a.right,
                    &base_lines,
                    &new_lines,
                    syn_old,
                    syn_new,
                ));
            }
        };
        let emit_fold =
            |hidden_rows: &[AlignedRow], rows: &mut Vec<SplitRow>, dropped: &mut usize| {
                if hidden_rows.is_empty() {
                    return;
                }
                if rows.len() >= MAX_DISPLAY_ROWS {
                    *dropped += 1;
                } else {
                    let first = hidden_rows[0];
                    let base_start = first.left.line.unwrap_or(0);
                    let new_start = first.right.line.unwrap_or(0);
                    rows.push(folded_split_row(
                        &file.path,
                        base_start,
                        new_start,
                        hidden_rows.len() as u32,
                    ));
                }
            };
        let is_ctx =
            |a: &AlignedRow| a.left.kind == CellKind::Context && a.right.kind == CellKind::Context;
        let total = aligned.len();
        let mut i = 0;
        while i < total {
            if is_ctx(&aligned[i]) {
                let mut j = i;
                while j < total && is_ctx(&aligned[j]) {
                    j += 1;
                }
                let run = (j - i) as u32;
                let lead = if i == 0 { 0 } else { CONTEXT_LINES.min(run) };
                let trail = if j == total {
                    0
                } else {
                    CONTEXT_LINES.min(run - lead)
                };
                let hidden = run - lead - trail;
                // US-058: lead/hidden/trail partition `run = j - i`, so every
                // `i + off` below is < j <= total. The `.get()` guards make that
                // fail-safe (no release panic) if the arithmetic ever drifts.
                for k in 0..lead as usize {
                    debug_assert!(i + k < total, "lead index out of bounds");
                    if let Some(a) = aligned.get(i + k) {
                        emit_pair(a, &mut rows, &mut dropped);
                    }
                }
                if hidden > 0 {
                    let start = i + lead as usize;
                    let end = start + hidden as usize;
                    emit_fold(&aligned[start..end], &mut rows, &mut dropped);
                }
                for k in 0..trail as usize {
                    let off = lead as usize + hidden as usize + k;
                    debug_assert!(i + off < total, "trail index out of bounds");
                    if let Some(a) = aligned.get(i + off) {
                        emit_pair(a, &mut rows, &mut dropped);
                    }
                }
                i = j;
            } else {
                emit_pair(&aligned[i], &mut rows, &mut dropped);
                i += 1;
            }
        }
    }

    if dropped > 0 {
        rows.push(truncated_split_note(dropped));
    }
    (rows, dropped)
}

/// Filter a unified row set by a per-file collapse set: a collapsed file keeps
/// only its header row, an expanded file keeps its full segment. `anchors` maps
/// each file path to its header row index (file order). Returns the filtered
/// rows plus rebuilt anchors (header index in the output). Shared by the Review
/// view ([`super::view`]) and the Agents diff dock ([`crate::app::agents_diff`]).
pub fn apply_collapse_unified(
    rows: &[DisplayRow],
    anchors: &[(String, usize)],
    collapsed: &HashSet<String>,
) -> (Vec<DisplayRow>, Vec<(String, usize)>) {
    let mut out = Vec::with_capacity(rows.len());
    let mut out_anchors = Vec::with_capacity(anchors.len());
    for (index, (path, start)) in anchors.iter().enumerate() {
        let Some(header) = rows.get(*start) else {
            continue;
        };
        let end = anchors
            .get(index + 1)
            .map(|(_, next_start)| *next_start)
            .unwrap_or(rows.len())
            .min(rows.len());
        out_anchors.push((path.clone(), out.len()));
        if collapsed.contains(path) {
            out.push(header.clone());
        } else if let Some(segment) = rows.get(*start..end) {
            out.extend_from_slice(segment);
        }
    }
    (out, out_anchors)
}

/// Split-view counterpart of [`apply_collapse_unified`].
pub fn apply_collapse_split(
    rows: &[SplitRow],
    anchors: &[(String, usize)],
    collapsed: &HashSet<String>,
) -> (Vec<SplitRow>, Vec<(String, usize)>) {
    let mut out = Vec::with_capacity(rows.len());
    let mut out_anchors = Vec::with_capacity(anchors.len());
    for (index, (path, start)) in anchors.iter().enumerate() {
        let end = anchors
            .get(index + 1)
            .map(|(_, next_start)| *next_start)
            .unwrap_or(rows.len())
            .min(rows.len());
        out_anchors.push((path.clone(), out.len()));
        if collapsed.contains(path) {
            match rows.get(*start) {
                Some(row @ SplitRow::Header(_)) => out.push(row.clone()),
                _ => {
                    out_anchors.pop();
                    continue;
                }
            }
        } else if let Some(segment) = rows.get(*start..end) {
            out.extend_from_slice(segment);
        }
    }
    (out, out_anchors)
}

fn source_file_index(files: &[FileDiff], path: &str) -> Option<usize> {
    files.iter().position(|file| file.path == path)
}

fn unified_fold_rows(
    file: &FileDiff,
    cache: &FileRowCache,
    base_start: u32,
    new_start: u32,
    count: u32,
) -> Vec<DisplayRow> {
    let new_lines: Vec<&str> = file.new_text.lines().collect();
    (0..count)
        .map(|k| {
            let bi = base_start + k;
            let ni = new_start + k;
            content_row(
                &new_lines,
                ni,
                RowKind::Context,
                Some(bi + 1),
                Some(ni + 1),
                line_syntax(&cache.syn_new, ni),
            )
        })
        .collect()
}

fn split_fold_rows(
    file: &FileDiff,
    cache: &FileRowCache,
    base_start: u32,
    new_start: u32,
    count: u32,
) -> Vec<SplitRow> {
    let base_lines: Vec<&str> = file.base_text.lines().collect();
    let new_lines: Vec<&str> = file.new_text.lines().collect();
    (0..count)
        .map(|k| {
            split_pair_row(
                Cell {
                    kind: CellKind::Context,
                    line: Some(base_start + k),
                },
                Cell {
                    kind: CellKind::Context,
                    line: Some(new_start + k),
                },
                &base_lines,
                &new_lines,
                &cache.syn_old,
                &cache.syn_new,
            )
        })
        .collect()
}

pub fn apply_expanded_unified_with_sources(
    rows: &[DisplayRow],
    anchors: &[(String, usize)],
    expanded: &HashSet<String>,
    files: &[FileDiff],
    caches: &[FileRowCache],
) -> (Vec<DisplayRow>, Vec<(String, usize)>) {
    let mut out = Vec::with_capacity(rows.len().min(MAX_DISPLAY_ROWS));
    let mut out_anchors = Vec::with_capacity(anchors.len());
    let mut next_anchor = 0usize;
    let mut current_file_idx = None;
    let mut dropped = 0usize;

    for (i, row) in rows.iter().enumerate() {
        while anchors
            .get(next_anchor)
            .is_some_and(|(_, header_row)| *header_row == i)
        {
            if let Some((path, _)) = anchors.get(next_anchor) {
                if out.len() < MAX_DISPLAY_ROWS {
                    out_anchors.push((path.clone(), out.len()));
                }
                current_file_idx = source_file_index(files, path);
            }
            next_anchor += 1;
        }

        push_display_capped(row.clone(), &mut out, &mut dropped);
        if row.kind == RowKind::Fold
            && row
                .fold_key
                .as_ref()
                .is_some_and(|key| expanded.contains(key.as_ref()))
        {
            if !row.folded_rows.is_empty() {
                extend_display_capped(row.folded_rows.iter().cloned(), &mut out, &mut dropped);
            } else if let (Some(file_idx), Some(base_start), Some(new_start)) =
                (current_file_idx, row.fold_base_start, row.fold_new_start)
                && let Some(file) = files.get(file_idx)
            {
                let fallback_cache;
                let cache = match caches.get(file_idx) {
                    Some(cache) => cache,
                    None => {
                        fallback_cache = FileRowCache::default();
                        &fallback_cache
                    }
                };
                extend_display_capped(
                    unified_fold_rows(file, cache, base_start, new_start, row.fold_count),
                    &mut out,
                    &mut dropped,
                );
            }
        }
    }

    if dropped > 0 {
        out.push(truncated_display_row(dropped));
    }
    (out, out_anchors)
}

pub fn apply_expanded_split_with_sources(
    rows: &[SplitRow],
    anchors: &[(String, usize)],
    expanded: &HashSet<String>,
    files: &[FileDiff],
    caches: &[FileRowCache],
) -> (Vec<SplitRow>, Vec<(String, usize)>) {
    let mut out = Vec::with_capacity(rows.len().min(MAX_DISPLAY_ROWS));
    let mut out_anchors = Vec::with_capacity(anchors.len());
    let mut next_anchor = 0usize;
    let mut current_file_idx = None;
    let mut dropped = 0usize;

    for (i, row) in rows.iter().enumerate() {
        while anchors
            .get(next_anchor)
            .is_some_and(|(_, header_row)| *header_row == i)
        {
            if let Some((path, _)) = anchors.get(next_anchor) {
                if out.len() < MAX_DISPLAY_ROWS {
                    out_anchors.push((path.clone(), out.len()));
                }
                current_file_idx = source_file_index(files, path);
            }
            next_anchor += 1;
        }

        push_split_capped(row.clone(), &mut out, &mut dropped);
        if let SplitRow::Fold(fold) = row
            && expanded.contains(fold.key.as_ref())
        {
            if !fold.rows.is_empty() {
                extend_split_capped(fold.rows.iter().cloned(), &mut out, &mut dropped);
            } else if let Some(file_idx) = current_file_idx
                && let Some(file) = files.get(file_idx)
            {
                let fallback_cache;
                let cache = match caches.get(file_idx) {
                    Some(cache) => cache,
                    None => {
                        fallback_cache = FileRowCache::default();
                        &fallback_cache
                    }
                };
                extend_split_capped(
                    split_fold_rows(file, cache, fold.base_start, fold.new_start, fold.count),
                    &mut out,
                    &mut dropped,
                );
            }
        }
    }

    if dropped > 0 {
        out.push(truncated_split_note(dropped));
    }
    (out, out_anchors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::engine::DiffHunkStatus;

    #[test]
    fn split_header_path_separates_dir_from_basename() {
        // EP-002 US-006: the directory prefix keeps its trailing slash and the
        // basename is the last segment - root files have an empty prefix.
        assert_eq!(
            split_header_path("src/app/view.rs"),
            ("src/app/".to_string(), "view.rs".to_string())
        );
        assert_eq!(
            split_header_path("Cargo.toml"),
            (String::new(), "Cargo.toml".to_string())
        );
        // A rename ("old → new"): the last `/` is in the new path, so the new
        // file's basename is emphasized and the arrow lead falls into the
        // (muted) directory prefix.
        assert_eq!(
            split_header_path("old/a.rs → new/b.rs"),
            ("old/a.rs → new/".to_string(), "b.rs".to_string())
        );
    }

    #[test]
    fn file_header_row_carries_typed_segments() {
        // EP-002 US-006: build_display_rows must populate the structured header
        // for the file-header row (split path + diffstat), so the element
        // paints typed segments instead of re-parsing a fused string.
        let file = FileDiff {
            path: "src/diff/rows.rs".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: "a\n".into(),
            new_text: "b\n".into(),
            hunks: crate::diff::engine::compute_hunks("a\n", "b\n"),
            is_binary: false,
        };
        let (added, removed) = file.line_counts();
        let (rows, _) = build_display_rows(std::slice::from_ref(&file), None);
        let header = rows
            .iter()
            .find(|r| r.kind == RowKind::FileHeader)
            .and_then(|r| r.header.as_ref())
            .expect("file-header row must carry HeaderParts");
        assert_eq!(header.dir_prefix.as_ref(), "src/diff/");
        assert_eq!(header.basename.as_ref(), "rows.rs");
        assert_eq!((header.added, header.removed), (added, removed));
    }

    #[test]
    fn unified_collapses_distant_context_into_fold() {
        // One change on line 0, then 29 unchanged lines. The trailing context
        // must collapse to CONTEXT_LINES kept + a single Fold marker, instead
        // of dumping the whole file (the pre-fold behaviour).
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..30 {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let hunks = crate::diff::engine::compute_hunks(&base, &new);
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks,
            is_binary: false,
        };
        let (rows, dropped) = build_display_rows(&[file], None);
        assert_eq!(dropped, 0);

        let folds: Vec<&DisplayRow> = rows.iter().filter(|r| r.kind == RowKind::Fold).collect();
        assert_eq!(folds.len(), 1, "exactly one collapsed region");
        assert!(
            folds[0].text.contains("26"),
            "fold hides 29 - 3 = 26 lines, got {:?}",
            folds[0].text
        );

        let ctx = rows.iter().filter(|r| r.kind == RowKind::Context).count();
        assert_eq!(ctx, CONTEXT_LINES as usize, "only bordering context kept");

        // 30-line file → folded output is a handful of rows, not 1 + 30.
        assert!(rows.len() < 10, "folded row count {} too large", rows.len());
    }

    #[test]
    fn unified_expands_fold_marker_in_place() {
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..30 {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base.clone(),
            new_text: new.clone(),
            hunks: crate::diff::engine::compute_hunks(&base, &new),
            is_binary: false,
        };
        let (rows, _) = build_display_rows(std::slice::from_ref(&file), None);
        let fold_idx = rows
            .iter()
            .position(|r| r.kind == RowKind::Fold)
            .expect("fixture should contain a fold");
        let key = rows[fold_idx]
            .fold_key
            .as_ref()
            .expect("fold has a stable key")
            .to_string();
        let hidden = rows[fold_idx].fold_count as usize;
        let mut expanded = HashSet::new();
        expanded.insert(key);
        let caches = build_file_row_caches(std::slice::from_ref(&file), None);

        let (open_rows, anchors) = apply_expanded_unified_with_sources(
            &rows,
            &[("a.txt".into(), 0)],
            &expanded,
            std::slice::from_ref(&file),
            &caches,
        );

        assert_eq!(anchors, vec![("a.txt".into(), 0)]);
        assert_eq!(open_rows.len(), rows.len() + hidden);
        assert!(matches!(open_rows[fold_idx].kind, RowKind::Fold));
        assert!(matches!(open_rows[fold_idx + 1].kind, RowKind::Context));
    }

    #[test]
    fn unified_hunk_tops_marks_each_hunk_start_at_its_row_offset() {
        // US-046: the cached hunk tops MUST equal the precomputed row offset of
        // every hunk-start row (a change row whose predecessor is not a change).
        // Guards against the offsets and hunk_tops computations drifting apart.
        let base = "a\nb\nc\nd\ne\n".to_string();
        let new = "a\nB\nc\nd\nE\n".to_string(); // two separate single-line edits
        let hunks = crate::diff::engine::compute_hunks(&base, &new);
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks,
            is_binary: false,
        };
        let (rows, _) = build_display_rows(&[file], None);
        let offsets = unified_offsets(&rows);

        let mut expected = Vec::new();
        let mut prev_change = false;
        for (i, r) in rows.iter().enumerate() {
            let is_change = matches!(r.kind, RowKind::Added | RowKind::Removed);
            if is_change && !prev_change {
                expected.push(offsets[i]);
            }
            prev_change = is_change;
        }

        assert_eq!(unified_hunk_tops(&rows), expected);
        assert_eq!(expected.len(), 2, "fixture has two distinct hunks");
    }

    #[test]
    fn split_hunk_tops_marks_each_hunk_start_at_its_row_offset() {
        // US-046 (EP-008 review): the split analog of the unified drift guard.
        // `split_hunk_tops` walks the rows with its own accumulator; it MUST
        // agree with the `split_offsets` prefix sum at every hunk-start row, or
        // side-by-side hunk-nav jumps to the wrong pixel. Catches a future
        // divergence between `split_row_height` and the hunk-tops walk that the
        // unified guard would NOT surface (the two heights are separate fns).
        let base = "a\nb\nc\nd\ne\n".to_string();
        let new = "a\nB\nc\nd\nE\n".to_string(); // two separate single-line edits
        let hunks = crate::diff::engine::compute_hunks(&base, &new);
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks,
            is_binary: false,
        };
        let (rows, _) = build_split_rows(std::slice::from_ref(&file), None);
        let offsets = split_offsets(&rows);

        let mut expected = Vec::new();
        let mut prev_change = false;
        for (i, r) in rows.iter().enumerate() {
            let is_change = matches!(
                r,
                SplitRow::Pair { left, right }
                    if matches!(left.kind, CellKind::Added | CellKind::Removed)
                        || matches!(right.kind, CellKind::Added | CellKind::Removed)
            );
            if is_change && !prev_change {
                expected.push(offsets[i]);
            }
            prev_change = is_change;
        }

        assert_eq!(split_hunk_tops(&rows), expected);
        assert!(
            !expected.is_empty(),
            "fixture must produce at least one split hunk for the guard to be meaningful"
        );
    }

    #[test]
    fn split_collapses_distant_context_into_fold() {
        // Same fixture as the unified test: side-by-side must collapse the
        // unchanged tail too, instead of dumping every aligned row.
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..30 {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let hunks = crate::diff::engine::compute_hunks(&base, &new);
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks,
            is_binary: false,
        };
        let (rows, dropped) = build_split_rows(&[file], None);
        assert_eq!(dropped, 0);

        let folds = rows
            .iter()
            .filter(|r| matches!(r, SplitRow::Fold(_)))
            .count();
        assert_eq!(folds, 1, "exactly one collapsed region");

        let pairs = rows
            .iter()
            .filter(|r| matches!(r, SplitRow::Pair { .. }))
            .count();
        // 1 changed pair + 3 trailing context pairs kept; the rest folded.
        assert_eq!(pairs, 1 + CONTEXT_LINES as usize);
        assert!(rows.len() < 10, "folded row count {} too large", rows.len());
    }

    #[test]
    fn split_expands_fold_marker_in_place() {
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..30 {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base.clone(),
            new_text: new.clone(),
            hunks: crate::diff::engine::compute_hunks(&base, &new),
            is_binary: false,
        };
        let (rows, _) = build_split_rows(std::slice::from_ref(&file), None);
        let fold_idx = rows
            .iter()
            .position(|r| matches!(r, SplitRow::Fold(_)))
            .expect("fixture should contain a fold");
        let (key, hidden) = match &rows[fold_idx] {
            SplitRow::Fold(fold) => (fold.key.to_string(), fold.count as usize),
            _ => unreachable!(),
        };
        let mut expanded = HashSet::new();
        expanded.insert(key);
        let caches = build_file_row_caches(std::slice::from_ref(&file), None);

        let (open_rows, anchors) = apply_expanded_split_with_sources(
            &rows,
            &[("a.txt".into(), 0)],
            &expanded,
            std::slice::from_ref(&file),
            &caches,
        );

        assert_eq!(anchors, vec![("a.txt".into(), 0)]);
        assert_eq!(open_rows.len(), rows.len() + hidden);
        assert!(matches!(open_rows[fold_idx], SplitRow::Fold(_)));
        assert!(matches!(open_rows[fold_idx + 1], SplitRow::Pair { .. }));
    }

    #[test]
    fn unified_fold_expansion_respects_display_row_cap() {
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..(MAX_DISPLAY_ROWS + 100) {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base.clone(),
            new_text: new.clone(),
            hunks: crate::diff::engine::compute_hunks(&base, &new),
            is_binary: false,
        };
        let (rows, _) = build_display_rows(std::slice::from_ref(&file), None);
        let key = rows
            .iter()
            .find(|row| row.kind == RowKind::Fold)
            .and_then(|row| row.fold_key.as_ref())
            .expect("fixture should contain a fold")
            .to_string();
        let mut expanded = HashSet::new();
        expanded.insert(key);
        let caches = build_file_row_caches(std::slice::from_ref(&file), None);

        let (open_rows, _) = apply_expanded_unified_with_sources(
            &rows,
            &[("a.txt".into(), 0)],
            &expanded,
            std::slice::from_ref(&file),
            &caches,
        );

        assert_eq!(open_rows.len(), MAX_DISPLAY_ROWS + 1);
        assert!(
            open_rows
                .last()
                .is_some_and(|row| row.kind == RowKind::Truncated)
        );
    }

    #[test]
    fn split_fold_expansion_respects_display_row_cap() {
        let mut base = String::from("OLD\n");
        let mut new = String::from("NEW\n");
        for i in 1..(MAX_DISPLAY_ROWS + 100) {
            base.push_str(&format!("ctx{i}\n"));
            new.push_str(&format!("ctx{i}\n"));
        }
        let file = FileDiff {
            path: "a.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base.clone(),
            new_text: new.clone(),
            hunks: crate::diff::engine::compute_hunks(&base, &new),
            is_binary: false,
        };
        let (rows, _) = build_split_rows(std::slice::from_ref(&file), None);
        let key = rows
            .iter()
            .find_map(|row| match row {
                SplitRow::Fold(fold) => Some(fold.key.to_string()),
                _ => None,
            })
            .expect("fixture should contain a fold");
        let mut expanded = HashSet::new();
        expanded.insert(key);
        let caches = build_file_row_caches(std::slice::from_ref(&file), None);

        let (open_rows, _) = apply_expanded_split_with_sources(
            &rows,
            &[("a.txt".into(), 0)],
            &expanded,
            std::slice::from_ref(&file),
            &caches,
        );

        assert_eq!(open_rows.len(), MAX_DISPLAY_ROWS + 1);
        assert!(matches!(open_rows.last(), Some(SplitRow::Note(_))));
    }

    #[test]
    fn split_large_files_use_hunk_windows_instead_of_full_alignment() {
        // A side-by-side diff over a huge file should keep only the hunk window
        // and folds. This guards the fast path that avoids aligning every
        // unchanged line before folding it away.
        let line_count = (MAX_FULL_SPLIT_ALIGN_LINES / 2) + 50;
        let changed = (line_count / 2) as u32;
        let mut base = String::new();
        let mut new = String::new();
        for i in 0..line_count {
            if i as u32 == changed {
                base.push_str("old\n");
                new.push_str("new\n");
            } else {
                base.push_str(&format!("ctx{i}\n"));
                new.push_str(&format!("ctx{i}\n"));
            }
        }
        let file = FileDiff {
            path: "large.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks: vec![crate::diff::engine::DiffHunk {
                base_row_range: changed..changed + 1,
                new_row_range: changed..changed + 1,
                status: DiffHunkStatus::Modified,
            }],
            is_binary: false,
        };

        let (rows, dropped) = build_split_rows(&[file], None);
        assert_eq!(dropped, 0);
        assert!(
            rows.len() < 20,
            "large split diff should be windowed, got {} rows",
            rows.len()
        );

        let folds = rows
            .iter()
            .filter(|row| matches!(row, SplitRow::Fold(_)))
            .count();
        assert_eq!(folds, 2, "head and tail context should be folded");

        let changed_pairs = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    SplitRow::Pair { left, right }
                        if left.kind == CellKind::Removed && right.kind == CellKind::Added
                )
            })
            .count();
        assert_eq!(changed_pairs, 1);
    }

    #[test]
    fn split_large_file_hunk_windows_merge_overlapping_context() {
        let line_count = (MAX_FULL_SPLIT_ALIGN_LINES / 2) + 50;
        let first_changed = 100u32;
        let second_changed = first_changed + 2;
        let mut base = String::new();
        let mut new = String::new();
        for i in 0..line_count {
            match i as u32 {
                n if n == first_changed => {
                    base.push_str("old-a\n");
                    new.push_str("new-a\n");
                }
                n if n == second_changed => {
                    base.push_str("old-b\n");
                    new.push_str("new-b\n");
                }
                _ => {
                    base.push_str(&format!("ctx{i}\n"));
                    new.push_str(&format!("ctx{i}\n"));
                }
            }
        }
        let file = FileDiff {
            path: "large.txt".into(),
            change: FileChange::Modified,
            old_path: None,
            base_text: base,
            new_text: new,
            hunks: vec![
                crate::diff::engine::DiffHunk {
                    base_row_range: first_changed..first_changed + 1,
                    new_row_range: first_changed..first_changed + 1,
                    status: DiffHunkStatus::Modified,
                },
                crate::diff::engine::DiffHunk {
                    base_row_range: second_changed..second_changed + 1,
                    new_row_range: second_changed..second_changed + 1,
                    status: DiffHunkStatus::Modified,
                },
            ],
            is_binary: false,
        };

        let (rows, dropped) = build_split_rows(std::slice::from_ref(&file), None);
        assert_eq!(dropped, 0);

        let changed_pairs = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    SplitRow::Pair { left, right }
                        if left.kind == CellKind::Removed && right.kind == CellKind::Added
                )
            })
            .count();
        assert_eq!(changed_pairs, 2);

        let duplicated_second_change_as_context = rows.iter().any(|row| {
            matches!(
                row,
                SplitRow::Pair { left, right }
                    if left.kind == CellKind::Context
                        && right.kind == CellKind::Context
                        && left.no == Some(second_changed + 1)
                        && right.no == Some(second_changed + 1)
            )
        });
        assert!(!duplicated_second_change_as_context);
    }
}
