use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::code::save::FileStamp;
use crate::diff::{
    DiffOptions, DiffSyntax, DisplayRow, FileDiff, FileRowCache, RowKind, SplitRow,
    build_display_rows_with_caches, build_split_rows_with_caches, cached_file_row_caches,
    compute_head_diff, highlighted_file_row_caches,
};
use crate::workspace::GitDiffStats;

pub(super) struct DiffDockBuilt {
    pub(super) unified: Vec<DisplayRow>,
    pub(super) anchors_unified: Vec<(String, usize)>,
    pub(super) split: Vec<SplitRow>,
    pub(super) anchors_split: Vec<(String, usize)>,
    pub(super) paths: Vec<String>,
    pub(super) file_count: usize,
    pub(super) file_count_capped: bool,
    pub(super) added: u32,
    pub(super) removed: u32,
    pub(super) files_full: Arc<Vec<FileDiff>>,
    pub(super) row_caches: Vec<FileRowCache>,
    pub(super) shown_files: usize,
    pub(super) syntax_pending: bool,
    pub(super) theme_generation: u64,
    pub(super) fingerprint: u64,
    pub(super) options: DiffOptions,
    pub(super) toplevel: Option<PathBuf>,
    pub(super) head_sha: Option<String>,
    pub(super) stamps: HashMap<String, FileStamp>,
}

fn file_stamps(
    files: &[FileDiff],
    metadata: &HashMap<String, std::fs::Metadata>,
) -> HashMap<String, FileStamp> {
    files
        .iter()
        .filter(|file| file.change == crate::diff::FileChange::Modified && !file.is_binary)
        .filter_map(|file| {
            let stamp = FileStamp::from_metadata(metadata.get(&file.path)?);
            Some((file.path.clone(), stamp))
        })
        .collect()
}

pub(super) struct DiffDockColored {
    pub(super) unified: Vec<DisplayRow>,
    pub(super) split: Vec<SplitRow>,
    pub(super) row_caches: Vec<FileRowCache>,
}

const INLINE_SYNTAX_BYTES: usize = 256 * 1024;

struct DockRows {
    unified: Vec<DisplayRow>,
    anchors_unified: Vec<(String, usize)>,
    split: Vec<SplitRow>,
    anchors_split: Vec<(String, usize)>,
}

fn dock_rows(files: &[FileDiff], row_caches: &[FileRowCache]) -> DockRows {
    let (unified, _) = build_display_rows_with_caches(files, row_caches);
    let anchors_unified: Vec<(String, usize)> = files
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
    let (split, _) = build_split_rows_with_caches(files, row_caches);
    let anchors_split: Vec<(String, usize)> = files
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
    DockRows {
        unified,
        anchors_unified,
        split,
        anchors_split,
    }
}

pub(super) fn colorize_diff_dock(
    files: &[FileDiff],
    shown_files: usize,
    theme: &crate::theme::TerminalTheme,
) -> DiffDockColored {
    let syntax = DiffSyntax::from_theme(theme);
    let (row_caches, _) = highlighted_file_row_caches(files, shown_files, &syntax);
    let rows = dock_rows(files, &row_caches);
    DiffDockColored {
        unified: rows.unified,
        split: rows.split,
        row_caches,
    }
}

pub(super) fn build_diff_dock(
    cwd: &str,
    theme: crate::theme::TerminalTheme,
    theme_generation: u64,
    options: DiffOptions,
) -> Result<DiffDockBuilt, String> {
    let options = options.for_cached_rows();
    let (diff, git_stats) = std::thread::scope(|scope| {
        let stats = scope.spawn(|| GitDiffStats::from_cwd(cwd));
        let diff = compute_head_diff(Path::new(cwd), options);
        (diff, stats.join().unwrap_or_default())
    });
    if let Some(e) = diff.error {
        return Err(e);
    }
    let stamps = file_stamps(&diff.files, &diff.working_metadata);
    let syntax = DiffSyntax::from_theme(&theme);
    let cached = cached_file_row_caches(&diff.files, &syntax);
    let mut row_caches = cached.caches;
    let mut rows = dock_rows(&diff.files, &row_caches);
    let shown_files = rows.anchors_unified.len().max(rows.anchors_split.len());
    let missing_bytes: usize = cached.missing_bytes.iter().take(shown_files).sum();
    let mut syntax_pending = missing_bytes > 0;
    if syntax_pending && missing_bytes <= INLINE_SYNTAX_BYTES {
        row_caches = highlighted_file_row_caches(&diff.files, shown_files, &syntax).0;
        rows = dock_rows(&diff.files, &row_caches);
        syntax_pending = false;
    }
    let DockRows {
        unified,
        anchors_unified,
        split,
        anchors_split,
    } = rows;
    let paths: Vec<String> = diff.files.iter().map(|f| f.path.clone()).collect();
    let fingerprint = diff_dock_snapshot_fingerprint(&diff.files);
    let (hunk_added, hunk_removed) = diff.files.iter().fold((0u32, 0u32), |(a, r), f| {
        let (fa, fr) = f.line_counts();
        (a + fa, r + fr)
    });
    let (file_count, added, removed) = if git_stats.is_empty() && !diff.files.is_empty() {
        (diff.files.len(), hunk_added, hunk_removed)
    } else {
        (
            git_stats.files_changed,
            u32::try_from(git_stats.insertions).unwrap_or(u32::MAX),
            u32::try_from(git_stats.deletions).unwrap_or(u32::MAX),
        )
    };
    let file_count_capped = git_stats.count_capped;
    Ok(DiffDockBuilt {
        unified,
        anchors_unified,
        split,
        anchors_split,
        file_count,
        file_count_capped,
        paths,
        added,
        removed,
        files_full: Arc::new(diff.files),
        row_caches,
        shown_files,
        syntax_pending,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture as fixture;

    fn rust_functions(marker: &str, count: usize) -> String {
        (0..count)
            .map(|index| format!("pub fn {marker}_{index}() -> usize {{ {index} }}\n"))
            .collect()
    }

    fn any_colored(rows: &[DisplayRow]) -> bool {
        rows.iter().any(|row| !row.syntax_runs.is_empty())
    }

    #[test]
    fn a_cold_large_diff_arrives_plain_and_takes_its_colors_on_the_same_rows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = rust_functions("cold_large_diff", 7_000);
        assert!(before.len() > INLINE_SYNTAX_BYTES);
        fixture::committed_repo(root, &[("src/big.rs", &before)]);
        let after = before.replacen("-> usize { 3 }", "-> usize { 33 }", 1);
        std::fs::write(root.join("src/big.rs"), after).unwrap();
        let cwd = root.to_string_lossy().into_owned();
        let theme = crate::theme::active_theme();

        let plain = build_diff_dock(&cwd, theme, 0, DiffOptions::default()).unwrap();
        assert!(plain.syntax_pending);
        assert_eq!(plain.shown_files, 1);
        assert!(!any_colored(&plain.unified));

        let colored = colorize_diff_dock(&plain.files_full, plain.shown_files, &theme);
        assert!(any_colored(&colored.unified));
        assert_eq!(colored.unified.len(), plain.unified.len());
        assert_eq!(colored.split.len(), plain.split.len());
        assert!(
            colored
                .unified
                .iter()
                .zip(&plain.unified)
                .all(|(colored, plain)| colored.kind == plain.kind && colored.text == plain.text)
        );

        let warm = build_diff_dock(&cwd, theme, 0, DiffOptions::default()).unwrap();
        assert!(!warm.syntax_pending);
        assert!(any_colored(&warm.unified));
    }

    #[test]
    fn a_small_diff_is_colored_in_its_first_pass() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let before = rust_functions("small_diff_first_pass", 20);
        fixture::committed_repo(root, &[("src/small.rs", &before)]);
        let after = before.replacen("-> usize { 3 }", "-> usize { 33 }", 1);
        std::fs::write(root.join("src/small.rs"), after).unwrap();
        let cwd = root.to_string_lossy().into_owned();

        let built = build_diff_dock(
            &cwd,
            crate::theme::active_theme(),
            0,
            DiffOptions::default(),
        )
        .unwrap();
        assert!(!built.syntax_pending);
        assert!(any_colored(&built.unified));
    }
}
