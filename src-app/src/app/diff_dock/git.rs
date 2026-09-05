use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::code::save::FileStamp;
use crate::diff::{
    DiffOptions, DiffSyntax, DisplayRow, FileDiff, FileRowCache, RowKind, SplitRow,
    build_display_rows_with_caches, build_file_row_caches, build_split_rows_with_caches,
    compute_head_diff,
};
use crate::workspace::GitDiffStats;

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
    pub(super) options: DiffOptions,
    pub(super) toplevel: Option<PathBuf>,
    pub(super) head_sha: Option<String>,
    pub(super) stamps: HashMap<String, FileStamp>,
}

fn file_stamps(toplevel: &Path, files: &[FileDiff]) -> HashMap<String, FileStamp> {
    files
        .iter()
        .filter(|file| file.change == crate::diff::FileChange::Modified && !file.is_binary)
        .filter_map(|file| {
            FileStamp::read(&toplevel.join(&file.path)).map(|stamp| (file.path.clone(), stamp))
        })
        .collect()
}

pub(super) fn build_diff_dock(
    cwd: &str,
    theme: crate::theme::TerminalTheme,
    theme_generation: u64,
    options: DiffOptions,
) -> Result<DiffDockBuilt, String> {
    let options = options.for_cached_rows();
    let diff = compute_head_diff(Path::new(cwd), options);
    if let Some(e) = diff.error {
        return Err(e);
    }
    let stamps = diff
        .toplevel
        .as_deref()
        .map(|toplevel| file_stamps(toplevel, &diff.files))
        .unwrap_or_default();
    let syntax = DiffSyntax::from_theme(&theme);
    let row_caches = build_file_row_caches(&diff.files, Some(&syntax));
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
        options,
        toplevel: diff.toplevel,
        head_sha: diff.head_sha,
        stamps,
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
