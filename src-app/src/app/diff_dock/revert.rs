use std::path::Path;

use gpui::{Context, Pixels, Point, point, px};

use super::code::save::{FileStamp, save_blocking};
use super::model::{DiffDockTab, DiffHover};
use crate::PaneFlowApp;
use crate::diff::{
    CellKind, DiffHunk, DisplayRow, FileChange, FileDiff, RowKind, SplitRow, hunk_for_base_line,
    hunk_for_new_line, revert_chip_bounds, row_at_offset,
};

pub(super) const DIRTY_TAB_MESSAGE: &str = "Save or discard the editor changes first";
pub(super) const STALE_FILE_MESSAGE: &str = "File changed on disk, refresh first";

fn line_contents(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\n' => {
                lines.push(&text[start..index]);
                index += 1;
                start = index;
            }
            b'\r' => {
                lines.push(&text[start..index]);
                index += if bytes.get(index + 1) == Some(&b'\n') {
                    2
                } else {
                    1
                };
                start = index;
            }
            _ => index += 1,
        }
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
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
    let new_lines = line_contents(new_text);
    let base_lines = line_contents(base_text);
    let start = (hunk.new_row_range.start as usize).min(new_lines.len());
    let end = (hunk.new_row_range.end as usize).clamp(start, new_lines.len());
    let base_start = (hunk.base_row_range.start as usize).min(base_lines.len());
    let base_end = (hunk.base_row_range.end as usize).clamp(base_start, base_lines.len());

    let mut lines = Vec::with_capacity(new_lines.len() + base_end - base_start);
    lines.extend_from_slice(&new_lines[..start]);
    lines.extend_from_slice(&base_lines[base_start..base_end]);
    lines.extend_from_slice(&new_lines[end..]);
    let terminated = if end == new_lines.len() {
        ends_with_newline(base_text)
    } else {
        ends_with_newline(new_text)
    };
    let mut out = lines.join(terminator);
    if terminated && !lines.is_empty() {
        out.push_str(terminator);
    }
    out
}

pub(super) fn revert_hunk_blocking(
    path: &Path,
    base_text: &str,
    hunk: &DiffHunk,
    recorded: Option<FileStamp>,
) -> Result<FileStamp, String> {
    match (recorded, FileStamp::read(path)) {
        (Some(recorded), Some(current)) if !recorded.differs(&current) => {}
        _ => return Err(STALE_FILE_MESSAGE.to_string()),
    }
    let bytes = std::fs::read(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let new_text =
        String::from_utf8(bytes).map_err(|_| format!("{}: not UTF-8 text", path.display()))?;
    let text = splice_base_lines(&new_text, base_text, hunk);
    save_blocking(path, &text)
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
    (file.change == FileChange::Modified && !file.is_binary).then_some(index)
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
        let base_text = file.base_text.clone();
        let recorded = data.stamps.get(&file.path).copied();
        let cwd = data.cwd.clone();
        if self.dirty_file_tab_open(&path, cx) {
            self.show_diff_dock_error(DIRTY_TAB_MESSAGE, cx);
            return;
        }
        cx.spawn(
            async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                let result =
                    smol::unblock(move || revert_hunk_blocking(&path, &base_text, &hunk, recorded))
                        .await;
                let _ = cx.update(|cx| {
                    this.update(cx, |app, cx| match result {
                        Ok(_) => app.refresh_diff_dock(cwd, cx),
                        Err(err) => app.show_diff_dock_error(&err, cx),
                    })
                });
            },
        )
        .detach();
    }

    fn dirty_file_tab_open(&self, path: &Path, cx: &Context<Self>) -> bool {
        self.diff_dock.diff_tabs.iter().any(|tab| match tab {
            DiffDockTab::File(view) => {
                let view = view.read(cx);
                view.path() == path && view.is_dirty()
            }
            _ => false,
        })
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
        let stamp = FileStamp::read(&on_disk);

        let stale = FileStamp::from_metadata(&std::fs::metadata(&on_disk).expect("meta"));
        let wrong_len = {
            let mut copy = tempfile::NamedTempFile::new_in(dir.path()).expect("temp");
            use std::io::Write as _;
            copy.write_all(b"x").expect("write");
            FileStamp::read(copy.path()).expect("stamp")
        };
        assert!(stale.differs(&wrong_len));
        assert_eq!(
            revert_hunk_blocking(&on_disk, &file.base_text, &file.hunks[0], Some(wrong_len)),
            Err(STALE_FILE_MESSAGE.to_string())
        );
        assert_eq!(
            revert_hunk_blocking(&on_disk, &file.base_text, &file.hunks[0], None),
            Err(STALE_FILE_MESSAGE.to_string())
        );
        assert_eq!(std::fs::read_to_string(&on_disk).expect("read"), edited);

        revert_hunk_blocking(&on_disk, &file.base_text, &file.hunks[0], stamp).expect("revert");
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
}
