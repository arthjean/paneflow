use std::path::Path;

use gpui::{Context, Pixels, Point, point, px};

use super::code::save::{FileStamp, SaveFailure, save_blocking};
use super::model::{DiffDockTab, DiffHover};
use crate::PaneFlowApp;
use crate::diff::{
    CellKind, DiffHunk, DisplayRow, FileChange, FileDiff, RowKind, SplitRow, classify_git_bytes,
    hunk_for_base_line, hunk_for_new_line, revert_chip_bounds, row_at_offset,
};

pub(super) const DIRTY_TAB_MESSAGE: &str = "Save or discard the editor changes first";
pub(super) const STALE_FILE_MESSAGE: &str = "File changed on disk, refresh first";

fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        let terminator_len = match bytes[index] {
            b'\n' => 1,
            b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
            b'\r' => 1,
            _ => {
                index += 1;
                continue;
            }
        };
        lines.push((&text[start..index], &text[index..index + terminator_len]));
        index += terminator_len;
        start = index;
    }
    if start < bytes.len() {
        lines.push((&text[start..], ""));
    }
    lines
}

fn ends_with_newline(text: &str) -> bool {
    text.ends_with('\n') || text.ends_with('\r')
}

fn dominant_terminator(text: &str) -> &'static str {
    let crlf = text.matches("\r\n").count();
    let lf = text.matches('\n').count() - crlf;
    if crlf > lf { "\r\n" } else { "\n" }
}

pub(crate) fn splice_base_lines(new_text: &str, base_text: &str, hunk: &DiffHunk) -> String {
    let terminator = dominant_terminator(new_text);
    let new_lines = split_lines(new_text);
    let base_lines = split_lines(base_text);
    let start = (hunk.new_row_range.start as usize).min(new_lines.len());
    let end = (hunk.new_row_range.end as usize).clamp(start, new_lines.len());
    let base_start = (hunk.base_row_range.start as usize).min(base_lines.len());
    let base_end = (hunk.base_row_range.end as usize).clamp(base_start, base_lines.len());

    let mut lines = Vec::with_capacity(new_lines.len() + base_end - base_start);
    lines.extend_from_slice(&new_lines[..start]);
    let replaced = &new_lines[start..end];
    lines.extend(base_lines[base_start..base_end].iter().enumerate().map(
        |(offset, (content, _))| {
            let ending = replaced
                .get(offset)
                .map(|(_, ending)| *ending)
                .filter(|ending| matches!(*ending, "\n" | "\r\n"))
                .unwrap_or(terminator);
            (*content, ending)
        },
    ));
    lines.extend_from_slice(&new_lines[end..]);
    if end == new_lines.len()
        && let Some(last) = lines.last_mut()
    {
        last.1 = match (ends_with_newline(base_text), last.1) {
            (false, _) => "",
            (true, "") => terminator,
            (true, own) => own,
        };
    }
    lines
        .iter()
        .flat_map(|(content, ending)| [*content, *ending])
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum RevertFailure {
    Stale,
    Refused(String),
}

pub(super) struct RevertRequest<'a> {
    pub(super) path: &'a Path,
    pub(super) expected_text: &'a str,
    pub(super) base_text: &'a str,
    pub(super) hunk: &'a DiffHunk,
    pub(super) recorded: Option<FileStamp>,
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

pub(super) fn type_change_message(path: &Path) -> String {
    format!(
        "{} changed between a file and a symlink; Revert cannot restore it",
        display_name(path)
    )
}

fn read_revert_source(path: &Path) -> Result<(Vec<u8>, FileStamp), RevertFailure> {
    use std::io::Read as _;
    let refused =
        |err: std::io::Error| RevertFailure::Refused(format!("{}: {err}", path.display()));
    let link = std::fs::symlink_metadata(path).map_err(refused)?;
    if link.file_type().is_symlink() {
        return Err(RevertFailure::Refused(format!(
            "{} is a symlink; Revert only edits regular files",
            display_name(path)
        )));
    }
    let (mut file, metadata) = paneflow_home::open_regular_for_reading(path).map_err(refused)?;
    if cfg!(windows) && metadata.permissions().readonly() {
        return Err(RevertFailure::Refused(format!(
            "{} is read-only; Revert left it untouched",
            display_name(path)
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(refused)?;
    Ok((bytes, FileStamp::from_metadata(&metadata)))
}

pub(super) fn revert_hunk_blocking(request: RevertRequest<'_>) -> Result<FileStamp, RevertFailure> {
    let (bytes, stamp) = read_revert_source(request.path)?;
    if request
        .recorded
        .is_none_or(|recorded| recorded.differs(&stamp))
    {
        return Err(RevertFailure::Stale);
    }
    let (normalized, binary) = classify_git_bytes(bytes.clone());
    if binary || normalized != request.expected_text {
        return Err(RevertFailure::Stale);
    }
    let current = String::from_utf8(bytes).map_err(|_| {
        RevertFailure::Refused(format!("{}: not UTF-8 text", request.path.display()))
    })?;
    let text = splice_base_lines(&current, request.base_text, request.hunk);
    save_blocking(request.path, &text, Some(stamp)).map_err(|failure| match failure {
        SaveFailure::ChangedOnDisk(_) => RevertFailure::Stale,
        SaveFailure::Write(message) => RevertFailure::Refused(message),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RevertTarget {
    pub(super) file: usize,
    pub(super) hunk: usize,
    pub(super) chip_row: usize,
}

fn revertable_file(anchors: &[(String, usize)], files: &[FileDiff], row: usize) -> Option<usize> {
    let (path, _) = anchors.iter().rev().find(|(_, header)| *header <= row)?;
    let index = files.iter().position(|file| file.path == *path)?;
    let file = &files[index];
    let revertable = matches!(file.change, FileChange::Modified | FileChange::TypeChanged);
    (revertable && !file.is_binary).then_some(index)
}

fn hunk_index(hunks: &[DiffHunk], hunk: &DiffHunk) -> Option<usize> {
    hunks
        .iter()
        .position(|candidate| std::ptr::eq(candidate, hunk))
}

fn hunk_for_new_no(hunks: &[DiffHunk], no: Option<u32>) -> Option<usize> {
    let line = no?.checked_sub(1)?;
    hunk_index(hunks, hunk_for_new_line(hunks, line)?)
}

fn hunk_for_base_no(hunks: &[DiffHunk], no: Option<u32>) -> Option<usize> {
    let line = no?.checked_sub(1)?;
    hunk_index(hunks, hunk_for_base_line(hunks, line)?)
}

fn first_row_of_run(row: usize, is_changed: impl Fn(usize) -> bool) -> Option<usize> {
    (0..=row).rev().take_while(|r| is_changed(*r)).last()
}

pub(super) fn unified_revert_target(
    rows: &[DisplayRow],
    anchors: &[(String, usize)],
    files: &[FileDiff],
    row: usize,
) -> Option<RevertTarget> {
    let current = rows.get(row)?;
    let file = revertable_file(anchors, files, row)?;
    let hunks = &files[file].hunks;
    let hunk = match current.kind {
        RowKind::Added => hunk_for_new_no(hunks, current.new_no),
        RowKind::Removed => hunk_for_base_no(hunks, current.old_no),
        _ => None,
    }?;
    let chip_row = first_row_of_run(row, |r| {
        matches!(rows[r].kind, RowKind::Added | RowKind::Removed)
    })?;
    Some(RevertTarget {
        file,
        hunk,
        chip_row,
    })
}

fn is_changed_pair(row: &SplitRow) -> bool {
    matches!(
        row,
        SplitRow::Pair { left, right }
            if left.kind != CellKind::Context || right.kind != CellKind::Context
    )
}

pub(super) fn split_revert_target(
    rows: &[SplitRow],
    anchors: &[(String, usize)],
    files: &[FileDiff],
    row: usize,
) -> Option<RevertTarget> {
    let SplitRow::Pair { left, right } = rows.get(row)? else {
        return None;
    };
    let file = revertable_file(anchors, files, row)?;
    let hunks = &files[file].hunks;
    let hunk = match (left.kind, right.kind) {
        (_, CellKind::Added) => hunk_for_new_no(hunks, right.no),
        (CellKind::Removed, _) => hunk_for_base_no(hunks, left.no),
        _ => None,
    }?;
    let chip_row = first_row_of_run(row, |r| is_changed_pair(&rows[r]))?;
    Some(RevertTarget {
        file,
        hunk,
        chip_row,
    })
}

impl PaneFlowApp {
    fn diff_dock_revert_target_at(&self, position: Point<Pixels>) -> Option<DiffHover> {
        let active = self.diff_dock.diff_tabs.get(self.diff_dock.diff_active_tab);
        if !matches!(active, Some(DiffDockTab::Changes)) {
            return None;
        }
        let data = self.diff_dock.data.as_ref()?;
        let bounds = self.diff_dock.scroll.bounds();
        if !bounds.contains(&position) {
            return None;
        }
        let content_y =
            f32::from(position.y - bounds.top() - self.diff_dock.scroll.offset().y).max(0.0);
        let split = self.diff_dock.split;
        let target = if split {
            let row = row_at_offset(&data.disp_split_offsets, content_y)?;
            split_revert_target(
                &data.disp_split,
                &data.disp_anchors_split,
                &data.files_full,
                row,
            )
        } else {
            let row = row_at_offset(&data.disp_unified_offsets, content_y)?;
            unified_revert_target(
                &data.disp_unified,
                &data.disp_anchors_unified,
                &data.files_full,
                row,
            )
        }?;
        Some(DiffHover {
            split,
            path: data.files_full[target.file].path.clone(),
            hunk: target.hunk,
            chip_row: target.chip_row,
        })
    }

    pub(crate) fn update_diff_dock_hover(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let next = self.diff_dock_revert_target_at(position);
        if next != self.diff_dock.hover {
            self.diff_dock.hover = next;
            cx.notify();
        }
    }

    pub(super) fn handle_diff_dock_revert_click(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(target) = self.diff_dock_revert_target_at(position) else {
            return false;
        };
        let inside_chip = {
            let Some(data) = self.diff_dock.data.as_ref() else {
                return false;
            };
            let offsets = if target.split {
                &data.disp_split_offsets
            } else {
                &data.disp_unified_offsets
            };
            let (Some(top), Some(bottom)) = (
                offsets.get(target.chip_row),
                offsets.get(target.chip_row + 1),
            ) else {
                return false;
            };
            let bounds = self.diff_dock.scroll.bounds();
            let origin = point(
                bounds.left(),
                bounds.top() + self.diff_dock.scroll.offset().y + px(*top),
            );
            revert_chip_bounds(origin, bounds.size.width, px(bottom - top)).contains(&position)
        };
        if !inside_chip {
            return false;
        }
        self.revert_diff_dock_hunk(target, cx);
        true
    }

    fn revert_diff_dock_hunk(&mut self, target: DiffHover, cx: &mut Context<Self>) {
        let Some(data) = self.diff_dock.data.as_ref() else {
            return;
        };
        let Some(toplevel) = data.toplevel.clone() else {
            return;
        };
        let Some(file) = data.files_full.iter().find(|file| file.path == target.path) else {
            return;
        };
        let Some(hunk) = file.hunks.get(target.hunk).cloned() else {
            return;
        };
        let path = toplevel.join(&file.path);
        let type_changed = file.change == FileChange::TypeChanged;
        let expected_text = file.new_text.clone();
        let base_text = file.base_text.clone();
        let recorded = data.stamps.get(&file.path).copied();
        let cwd = data.cwd.clone();
        let origin = self.diff_dock.owner;
        if type_changed {
            self.show_diff_dock_error(&type_change_message(&path), cx);
            return;
        }
        if self.dirty_view_of(&path, cx) {
            self.show_diff_dock_error(DIRTY_TAB_MESSAGE, cx);
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result = smol::unblock(move || {
                    revert_hunk_blocking(RevertRequest {
                        path: &path,
                        expected_text: &expected_text,
                        base_text: &base_text,
                        hunk: &hunk,
                        recorded,
                    })
                })
                .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| match result {
                        Ok(_) => app.refresh_diff_dock_of_tab(origin, cwd, cx),
                        Err(RevertFailure::Stale) => {
                            app.refresh_diff_dock_of_tab(origin, cwd, cx);
                            app.show_toast(STALE_FILE_MESSAGE, cx);
                        }
                        Err(RevertFailure::Refused(err)) => app.show_diff_dock_error(&err, cx),
                    })
                });
            },
        )
        .detach();
    }

    fn dirty_view_of(&self, path: &Path, cx: &Context<Self>) -> bool {
        self.all_unsaved_views(cx)
            .iter()
            .any(|view| view.read(cx).path() == path)
    }

    fn refresh_diff_dock_of_tab(
        &mut self,
        origin: Option<u64>,
        cwd: String,
        cx: &mut Context<Self>,
    ) {
        if self.diff_dock.owner == origin {
            self.refresh_diff_dock(cwd, cx);
        } else if let Some(data) = origin.and_then(|id| self.parked_diff_dock_data(id)) {
            data.mark_stale();
        }
    }

    fn show_diff_dock_error(&mut self, message: &str, cx: &mut Context<Self>) {
        if let Some(data) = self.diff_dock.data.as_mut() {
            data.error = Some(message.to_string());
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::diff::{
        DiffOptions, build_display_rows_with_caches, build_file_row_caches,
        build_split_rows_with_caches, compute_head_diff,
    };

    fn hunks(base: &str, new: &str) -> Vec<DiffHunk> {
        crate::diff::compute_hunks(base, new)
    }

    fn revert(new: &str, base: &str, hunk: usize) -> String {
        let hunks = hunks(base, new);
        splice_base_lines(new, base, &hunks[hunk])
    }

    #[test]
    fn a_block_at_the_start_middle_or_end_is_replaced_by_the_base_lines() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        assert_eq!(
            revert("ONE\ntwo\nthree\nfour\nfive\n", base, 0),
            base,
            "start"
        );
        assert_eq!(
            revert("one\ntwo\nTHREE\nfour\nfive\n", base, 0),
            base,
            "middle"
        );
        assert_eq!(
            revert("one\ntwo\nthree\nfour\nFIVE\n", base, 0),
            base,
            "end"
        );
        assert_eq!(
            revert("one\ntwo\nthree\nfour\nfive\nsix\n", base, 0),
            base,
            "appended lines"
        );
        assert_eq!(revert("one\nfive\n", base, 0), base, "deleted lines");
    }

    #[test]
    fn reverting_one_of_two_blocks_leaves_the_other_change_in_place() {
        let base = "a\nb\nc\nd\ne\nf\n";
        let new = "A\nb\nc\nd\nE\nf\n";
        assert_eq!(revert(new, base, 0), "a\nb\nc\nd\nE\nf\n");
        assert_eq!(revert(new, base, 1), "A\nb\nc\nd\ne\nf\n");
    }

    #[test]
    fn crlf_files_keep_crlf_and_a_missing_final_newline_stays_missing() {
        let base = "a\nb\nc\n";
        let modified = hunks(base, "a\nB\nc\n");
        assert_eq!(
            splice_base_lines("a\r\nB\r\nc\r\n", base, &modified[0]),
            "a\r\nb\r\nc\r\n"
        );
        let unterminated = hunks("a\nb\nc", "a\nB\nc");
        assert_eq!(
            splice_base_lines("a\r\nB\r\nc", "a\nb\nc", &unterminated[0]),
            "a\r\nb\r\nc"
        );
        let last_line = hunks("a\nb\nc", "a\nb\nC");
        assert_eq!(
            splice_base_lines("a\nb\nC", "a\nb\nc", &last_line[0]),
            "a\nb\nc"
        );
    }

    #[test]
    fn a_block_at_the_end_takes_the_final_newline_from_the_base() {
        let lost_newline = hunks("a\nb\n", "a\nb");
        assert_eq!(
            splice_base_lines("a\nb", "a\nb\n", &lost_newline[0]),
            "a\nb\n"
        );
        let gained_newline = hunks("a\nb", "a\nb\n");
        assert_eq!(
            splice_base_lines("a\nb\n", "a\nb", &gained_newline[0]),
            "a\nb"
        );
        let tail = hunks("a\nb\nc\n", "a\nb\nX");
        assert_eq!(
            splice_base_lines("a\nb\nX", "a\nb\nc\n", &tail[0]),
            "a\nb\nc\n"
        );
    }

    fn file(path: &str, change: FileChange, base: &str, new: &str) -> FileDiff {
        FileDiff {
            path: path.to_string(),
            change,
            old_path: None,
            base_text: base.to_string(),
            new_text: new.to_string(),
            hunks: hunks(base, new),
            is_binary: false,
        }
    }

    fn two_files() -> Vec<FileDiff> {
        vec![
            file(
                "src/a.rs",
                FileChange::Modified,
                "a\nb\nc\nd\ne\nf\ng\n",
                "a\nB\nc\nd\ne\nF\nG\n",
            ),
            file("src/new.rs", FileChange::Added, "", "x\ny\n"),
        ]
    }

    fn unified_anchors(rows: &[DisplayRow], files: &[FileDiff]) -> Vec<(String, usize)> {
        files
            .iter()
            .map(|f| f.path.clone())
            .zip(
                rows.iter()
                    .enumerate()
                    .filter(|(_, r)| r.kind == RowKind::FileHeader)
                    .map(|(i, _)| i),
            )
            .collect()
    }

    fn split_anchors(rows: &[SplitRow], files: &[FileDiff]) -> Vec<(String, usize)> {
        files
            .iter()
            .map(|f| f.path.clone())
            .zip(
                rows.iter()
                    .enumerate()
                    .filter(|(_, r)| matches!(r, SplitRow::Header(_)))
                    .map(|(i, _)| i),
            )
            .collect()
    }

    #[test]
    fn unified_rows_resolve_the_hunk_and_its_first_row_for_modified_files_only() {
        let files = two_files();
        let caches = build_file_row_caches(&files, None);
        let (rows, _) = build_display_rows_with_caches(&files, &caches);
        let anchors = unified_anchors(&rows, &files);

        let changed: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| matches!(r.kind, RowKind::Added | RowKind::Removed))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            changed.len(),
            2 + 4 + 2,
            "a two row block, a four row block, then the added file"
        );
        let first = unified_revert_target(&rows, &anchors, &files, changed[1]).expect("hunk 0");
        assert_eq!(
            first,
            RevertTarget {
                file: 0,
                hunk: 0,
                chip_row: changed[0]
            }
        );
        for row in [changed[2], changed[3], changed[5]] {
            let second = unified_revert_target(&rows, &anchors, &files, row).expect("hunk 1");
            assert_eq!(second.hunk, 1, "row {row}");
            assert_eq!(second.chip_row, changed[2], "row {row}");
        }
        assert!(
            unified_revert_target(&rows, &anchors, &files, changed[0] - 1).is_none(),
            "a context row shows no chip"
        );
        assert!(
            unified_revert_target(&rows, &anchors, &files, changed[6]).is_none(),
            "an added file shows no chip"
        );
    }

    #[test]
    fn split_rows_resolve_the_hunk_from_either_side() {
        let files = two_files();
        let caches = build_file_row_caches(&files, None);
        let (rows, _) = build_split_rows_with_caches(&files, &caches);
        let anchors = split_anchors(&rows, &files);
        let changed: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| is_changed_pair(r))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            changed.len(),
            1 + 2 + 2,
            "one paired row, two paired rows, the added file"
        );
        let first = split_revert_target(&rows, &anchors, &files, changed[0]).expect("hunk 0");
        assert_eq!(first.hunk, 0);
        assert_eq!(first.chip_row, changed[0]);
        let second = split_revert_target(&rows, &anchors, &files, changed[2]).expect("hunk 1");
        assert_eq!(second.hunk, 1);
        assert_eq!(second.chip_row, changed[1]);
        assert!(split_revert_target(&rows, &anchors, &files, changed[3]).is_none());
    }

    fn git(cwd: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }

    fn commit(root: &Path, message: &str) -> bool {
        git(root, &["add", "-A"])
            && git(
                root,
                &[
                    "-c",
                    "user.email=paneflow@example.com",
                    "-c",
                    "user.name=Paneflow",
                    "commit",
                    "-q",
                    "-m",
                    message,
                ],
            )
    }

    fn request<'a>(
        path: &'a Path,
        file: &'a FileDiff,
        hunk: usize,
        recorded: Option<FileStamp>,
    ) -> RevertRequest<'a> {
        RevertRequest {
            path,
            expected_text: &file.new_text,
            base_text: &file.base_text,
            hunk: &file.hunks[hunk],
            recorded,
        }
    }

    fn recorded_stamp(
        metadata: &std::collections::HashMap<String, std::fs::Metadata>,
        path: &str,
    ) -> Option<FileStamp> {
        metadata.get(path).map(FileStamp::from_metadata)
    }

    fn link_to(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(target, link);
        made.is_ok()
    }

    fn repo() -> Option<tempfile::TempDir> {
        let dir = tempfile::tempdir().expect("tempdir");
        if !git(dir.path(), &["init", "-q"]) {
            return None;
        }
        assert!(git(dir.path(), &["config", "core.autocrlf", "false"]));
        Some(dir)
    }

    #[test]
    fn reverting_the_first_of_two_blocks_writes_the_base_for_it_and_keeps_the_second() {
        let Some(dir) = repo() else {
            return;
        };
        let path = dir.path().join("notes.txt");
        let base = "one\ntwo\nthree\nfour\nfive\nsix\n";
        std::fs::write(&path, base).expect("write base");
        assert!(commit(dir.path(), "base"));
        let edited = "ONE\ntwo\nthree\nfour\nfive\nSIX\n";
        std::fs::write(&path, edited).expect("write edit");

        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        assert!(diff.error.is_none(), "{:?}", diff.error);
        assert!(diff.head_sha.is_some());
        let toplevel = diff.toplevel.clone().expect("toplevel");
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "notes.txt")
            .expect("notes.txt");
        assert_eq!(file.change, FileChange::Modified);
        assert_eq!(file.hunks.len(), 2);
        let on_disk = toplevel.join(&file.path);
        let stamp = recorded_stamp(&diff.working_metadata, &file.path);

        let stale = FileStamp::from_metadata(&std::fs::metadata(&on_disk).expect("meta"));
        let wrong_len = {
            let mut copy = tempfile::NamedTempFile::new_in(dir.path()).expect("temp");
            use std::io::Write as _;
            copy.write_all(b"x").expect("write");
            FileStamp::read(copy.path()).expect("stamp")
        };
        assert!(stale.differs(&wrong_len));
        assert_eq!(
            revert_hunk_blocking(request(&on_disk, file, 0, Some(wrong_len))),
            Err(RevertFailure::Stale)
        );
        assert_eq!(
            revert_hunk_blocking(request(&on_disk, file, 0, None)),
            Err(RevertFailure::Stale)
        );
        assert_eq!(std::fs::read_to_string(&on_disk).expect("read"), edited);

        revert_hunk_blocking(request(&on_disk, file, 0, stamp)).expect("revert");
        assert_eq!(
            std::fs::read_to_string(&on_disk).expect("read"),
            "one\ntwo\nthree\nfour\nfive\nSIX\n"
        );
        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "notes.txt")
            .expect("notes.txt");
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].new_row_range, 5..6);
    }

    #[test]
    fn a_mixed_line_ending_file_only_changes_inside_the_hunk() {
        let working = "a\r\nb\nc\rd\r\ne\n";
        let base = "a\nb\nc\nD\ne\n";
        let normalized = "a\nb\nc\nd\ne\n";
        let block = hunks(base, normalized);
        assert_eq!(block.len(), 1);

        assert_eq!(
            splice_base_lines(working, base, &block[0]),
            "a\r\nb\nc\rD\r\ne\n"
        );
    }

    #[test]
    fn an_agent_write_between_the_build_and_the_click_leaves_the_disk_unchanged() {
        let Some(dir) = repo() else {
            return;
        };
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("write base");
        assert!(commit(dir.path(), "base"));
        std::fs::write(&path, "ONE\ntwo\nthree\n").expect("write edit");
        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "notes.txt")
            .expect("notes.txt");
        let on_disk = diff.toplevel.clone().expect("toplevel").join(&file.path);
        let stamp = recorded_stamp(&diff.working_metadata, &file.path);
        let modified = std::fs::metadata(&on_disk)
            .and_then(|meta| meta.modified())
            .expect("mtime");

        let agent = "ONE\ntwo\nTHREE\n";
        std::fs::write(&on_disk, agent).expect("agent write");
        std::fs::File::options()
            .write(true)
            .open(&on_disk)
            .and_then(|handle| handle.set_modified(modified))
            .expect("restore mtime");
        assert_eq!(
            FileStamp::read(&on_disk),
            stamp,
            "the agent write is invisible to the stamp"
        );

        assert_eq!(
            revert_hunk_blocking(request(&on_disk, file, 0, stamp)),
            Err(RevertFailure::Stale)
        );
        assert_eq!(std::fs::read_to_string(&on_disk).expect("read"), agent);
    }

    #[test]
    fn a_symlink_is_refused_by_name_and_stays_a_symlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "a\nB\n").expect("target");
        let link = dir.path().join("link.txt");
        if !link_to(&target, &link) {
            return;
        }
        let file = file("link.txt", FileChange::Modified, "a\nb\n", "a\nB\n");
        let stamp = FileStamp::read(&link);

        let refused = revert_hunk_blocking(request(&link, &file, 0, stamp));

        assert_eq!(
            refused,
            Err(RevertFailure::Refused(
                "link.txt is a symlink; Revert only edits regular files".to_string()
            ))
        );
        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link")
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&target).expect("read"), "a\nB\n");
    }

    #[test]
    fn a_type_change_is_parsed_as_such_and_named_in_the_refusal() {
        let Some(dir) = repo() else {
            return;
        };
        assert!(git(dir.path(), &["config", "core.symlinks", "true"]));
        std::fs::write(dir.path().join("target.txt"), "x\n").expect("target");
        let entry = dir.path().join("entry.txt");
        std::fs::write(&entry, "entry\n").expect("entry");
        assert!(commit(dir.path(), "base"));
        std::fs::remove_file(&entry).expect("remove");
        if !link_to(Path::new("target.txt"), &entry) {
            return;
        }

        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "entry.txt")
            .expect("entry.txt");

        assert_eq!(file.change, FileChange::TypeChanged);
        assert_eq!(
            type_change_message(&entry),
            "entry.txt changed between a file and a symlink; Revert cannot restore it"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_mode_survives_a_revert() {
        use std::os::unix::fs::PermissionsExt;
        let Some(dir) = repo() else {
            return;
        };
        for mode in [0o444, 0o555] {
            let name = format!("file-{mode:o}.txt");
            let path = dir.path().join(&name);
            std::fs::write(&path, "one\ntwo\n").expect("base");
            assert!(commit(dir.path(), "base"));
            std::fs::write(&path, "ONE\ntwo\n").expect("edit");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
            let diff = compute_head_diff(dir.path(), DiffOptions::default());
            let file = diff.files.iter().find(|f| f.path == name).expect("file");
            let on_disk = diff.toplevel.clone().expect("toplevel").join(&file.path);

            revert_hunk_blocking(request(
                &on_disk,
                file,
                0,
                recorded_stamp(&diff.working_metadata, &name),
            ))
            .expect("revert");

            let meta = std::fs::metadata(&on_disk).expect("meta");
            assert_eq!(meta.permissions().mode() & 0o777, mode);
            assert_eq!(
                std::fs::read_to_string(&on_disk).expect("read"),
                "one\ntwo\n"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_read_only_file_is_refused_and_keeps_its_attribute() {
        let Some(dir) = repo() else {
            return;
        };
        let path = dir.path().join("locked.txt");
        std::fs::write(&path, "one\ntwo\n").expect("base");
        assert!(commit(dir.path(), "base"));
        std::fs::write(&path, "ONE\ntwo\n").expect("edit");
        let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&path, permissions).expect("read-only");
        let diff = compute_head_diff(dir.path(), DiffOptions::default());
        let file = diff
            .files
            .iter()
            .find(|f| f.path == "locked.txt")
            .expect("file");
        let on_disk = diff.toplevel.clone().expect("toplevel").join(&file.path);

        let refused = revert_hunk_blocking(request(
            &on_disk,
            file,
            0,
            recorded_stamp(&diff.working_metadata, "locked.txt"),
        ));

        assert_eq!(
            refused,
            Err(RevertFailure::Refused(
                "locked.txt is read-only; Revert left it untouched".to_string()
            ))
        );
        let mut permissions = std::fs::metadata(&on_disk).expect("meta").permissions();
        assert!(permissions.readonly());
        assert_eq!(
            std::fs::read_to_string(&on_disk).expect("read"),
            "ONE\ntwo\n"
        );
        #[allow(clippy::permissions_set_readonly_false, reason = "test cleanup")]
        permissions.set_readonly(false);
        std::fs::set_permissions(&on_disk, permissions).expect("writable");
    }
}
