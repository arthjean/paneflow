//! Off-thread HEAD-relative diff build for the Agents dock.
//!
//! Shells the shared git pipeline ([`crate::diff::compute_head_diff`]) and turns
//! the result into the shared row models (unified + split) with syntax
//! highlighting, off the GPUI main thread. The product of this module is an
//! [`DiffDockBuilt`] that [`super::model::DiffDockData::apply_built`] wraps in
//! `Rc`s back on the main thread.

use std::path::Path;

use crate::diff::{
    DiffSyntax, DisplayRow, FileDiff, FileRowCache, RowKind, SplitRow,
    build_display_rows_with_caches, build_file_row_caches, build_split_rows_with_caches,
    compute_head_diff,
};
use crate::workspace::GitDiffStats;

/// Off-thread build result: the full (uncollapsed) display rows + anchors for
/// both view modes, plus the per-panel summary. Built in `smol::unblock` and
/// moved back to the main thread to seed an [`super::model::DiffDockData`].
pub(super) struct DiffDockBuilt {
    pub(super) unified: Vec<DisplayRow>,
    pub(super) anchors_unified: Vec<(String, usize)>,
    pub(super) split: Vec<SplitRow>,
    pub(super) anchors_split: Vec<(String, usize)>,
    pub(super) paths: Vec<String>,
    pub(super) file_count: usize,
    pub(super) added: u32,
    pub(super) removed: u32,
    pub(super) files_full: Vec<FileDiff>,
    pub(super) row_caches: Vec<FileRowCache>,
    pub(super) theme_generation: u64,
    pub(super) fingerprint: u64,
}

/// Off-thread builder: shell the HEAD-relative diff and turn it into both shared
/// row models with syntax highlighting. Mirrors the Review view's warm inactive
/// mode, but eagerly returns both modes so the Agents toggle is instant.
pub(super) fn build_diff_dock(
    cwd: &str,
    theme: crate::theme::TerminalTheme,
    theme_generation: u64,
) -> Result<DiffDockBuilt, String> {
    let diff = compute_head_diff(Path::new(cwd));
    if let Some(e) = diff.error {
        return Err(e);
    }
    let syntax = DiffSyntax::from_theme(&theme);
    let row_caches = build_file_row_caches(&diff.files, Some(&syntax));
    // File path → header row index, in file order, so a body click can resolve
    // which file's header was hit (collapse toggle). Header rows are emitted one
    // per file in `diff.files` order, so zipping realigns them.
    let (unified, _) = build_display_rows_with_caches(&diff.files, &row_caches);
    let anchors_unified: Vec<(String, usize)> = diff
        .files
        .iter()
        .map(|f| f.path.clone())
        .zip(
            unified
                .iter()
                .enumerate()
                .filter(|(_, r)| r.kind == RowKind::FileHeader)
                .map(|(i, _)| i),
        )
        .collect();
    let (split, _) = build_split_rows_with_caches(&diff.files, &row_caches);
    let anchors_split: Vec<(String, usize)> = diff
        .files
        .iter()
        .map(|f| f.path.clone())
        .zip(
            split
                .iter()
                .enumerate()
                .filter(|(_, r)| matches!(r, SplitRow::Header(_)))
                .map(|(i, _)| i),
        )
        .collect();
    let paths: Vec<String> = diff.files.iter().map(|f| f.path.clone()).collect();
    let fingerprint = diff_dock_snapshot_fingerprint(&diff.files);
    let (hunk_added, hunk_removed) = diff.files.iter().fold((0u32, 0u32), |(a, r), f| {
        let (fa, fr) = f.line_counts();
        (a + fa, r + fr)
    });
    let git_stats = GitDiffStats::from_cwd(cwd);
    let (file_count, added, removed) = if git_stats.is_empty() && !diff.files.is_empty() {
        (diff.files.len(), hunk_added, hunk_removed)
    } else {
        (
            git_stats.files_changed,
            u32::try_from(git_stats.insertions).unwrap_or(u32::MAX),
            u32::try_from(git_stats.deletions).unwrap_or(u32::MAX),
        )
    };
    Ok(DiffDockBuilt {
        unified,
        anchors_unified,
        split,
        anchors_split,
        file_count,
        paths,
        added,
        removed,
        files_full: diff.files,
        row_caches,
        theme_generation,
        fingerprint,
    })
}

fn diff_dock_snapshot_fingerprint(files: &[FileDiff]) -> u64 {
    use std::hash::{Hash as _, Hasher as _};

    let mut h = std::collections::hash_map::DefaultHasher::new();
    files.len().hash(&mut h);
    for file in files {
        file.path.hash(&mut h);
        file.change.hash(&mut h);
        file.old_path.hash(&mut h);
        file.base_text.hash(&mut h);
        file.new_text.hash(&mut h);
        file.is_binary.hash(&mut h);
    }
    h.finish()
}
