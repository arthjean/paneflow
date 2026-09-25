use super::*;

fn ops_descend_by_row(doc: &CodeDocument, ops: &[(Range<usize>, String)]) -> bool {
    ops.windows(2)
        .all(|pair| doc.byte_to_line(pair[1].0.end) < doc.byte_to_line(pair[0].0.start))
}

const READ_ONLY_FLASH: Duration = Duration::from_millis(600);

impl CodeView {
    pub(super) fn take_whole_document(&mut self, cx: &mut Context<Self>) {
        self.end_typing_group();
        let Some(doc) = self.state.document() else {
            return;
        };
        let end = cursor::doc_end(doc);
        let goal = cursor::goal_column(doc, end);
        self.selection = CodeSelection {
            anchor: 0,
            head: end,
        };
        self.goal_column = goal;
        self.after_motion(cx);
    }

    pub(super) fn end_typing_group(&mut self) {
        self.history.close_group();
        self.marked = None;
    }

    pub(super) fn splice_all(
        &mut self,
        ops: &[(Range<usize>, String)],
        after: CodeSelection,
        group: EditGroup,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            self.flash_read_only(cx);
            return false;
        }
        let before = self.selection;
        let now = Instant::now();
        let batched = self
            .state
            .document()
            .is_some_and(|doc| ops_descend_by_row(doc, ops));
        let mut records = Vec::with_capacity(ops.len());
        let mut edits = Vec::with_capacity(ops.len());
        let mut deferred: Option<DeferredParse> = None;
        let mut changes = Vec::with_capacity(ops.len() * 2);
        if let Some((doc, hl)) = self.state.editable() {
            for (range, text) in ops {
                let Some(applied) = edit::splice(doc, range.clone(), text) else {
                    continue;
                };
                if batched {
                    edits.push(applied.edit);
                } else if let HighlightOutcome::Deferred(parse) = hl.edit(doc, &applied.edit) {
                    deferred = Some(parse);
                }
                changes.extend(applied.windows);
                records.push(applied.record);
            }
            if batched
                && let Ok(HighlightOutcome::Deferred(parse)) =
                    hl.edit_batch(doc, &edits, SYNC_PARSE_BUDGET)
            {
                deferred = Some(parse);
            }
        }
        if records.is_empty() {
            return false;
        }
        self.history.push(records, before, after, group, now);
        self.note_changes(&changes, cx);
        self.finish_edit(after, deferred, cx);
        true
    }

    fn finish_edit(
        &mut self,
        after: CodeSelection,
        deferred: Option<DeferredParse>,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self.state.document() {
            self.selection = CodeSelection {
                anchor: cursor::clamp(doc, after.anchor),
                head: cursor::clamp(doc, after.head),
            };
            self.goal_column = cursor::goal_column(doc, self.selection.cursor());
        }
        if let Some(parse) = deferred {
            spawn_deferred_parse(parse, cx, |view: &mut Self, parsed, cx| {
                if let Some((doc, hl)) = view.state.editable()
                    && hl.apply_parsed(doc, parsed)
                {
                    cx.notify();
                }
            });
        }
        self.refresh_longest_line(cx);
        self.after_motion(cx);
    }

    fn refresh_longest_line(&mut self, cx: &mut Context<Self>) {
        let Some((text, revision)) = self
            .state
            .document()
            .and_then(CodeDocument::longest_line_snapshot)
        else {
            return;
        };
        let load_generation = self.slot.current();
        super::spawn_blocking_then(
            cx,
            move || CodeDocument::measure_longest_line(&text),
            move |view: &mut Self, longest, cx| {
                if !view.slot.accept(load_generation) {
                    return;
                }
                let Some(doc) = view.state.document_mut() else {
                    return;
                };
                if doc.apply_longest_line_measurement(revision, longest) {
                    cx.notify();
                }
            },
        );
    }

    pub(super) fn flash_read_only(&mut self, cx: &mut Context<Self>) {
        self.read_only_flash = Some(Instant::now());
        cx.notify();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(READ_ONLY_FLASH).await;
            cx.update(|cx| {
                let _ = this.update(cx, |view: &mut Self, cx: &mut Context<Self>| {
                    if view
                        .read_only_flash
                        .is_some_and(|at| at.elapsed() >= READ_ONLY_FLASH)
                    {
                        view.read_only_flash = None;
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn insert_text(&mut self, text: &str, group: EditGroup, cx: &mut Context<Self>) -> bool {
        let range = self.replacement_range();
        let inserted = normalize_newlines(text).into_owned();
        let caret = CodeSelection::at(range.start + inserted.len());
        self.splice_all(&[(range, inserted)], caret, group, cx)
    }

    fn replacement_range(&self) -> Range<usize> {
        match &self.marked {
            Some(marked) => marked.clone(),
            None => self.selection.range(),
        }
    }

    pub(super) fn resolve_replacement(
        &self,
        range_utf16: Option<Range<usize>>,
    ) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        Some(match range_utf16 {
            Some(range) => {
                let start = doc.utf16_to_byte(range.start);
                start..doc.utf16_to_byte(range.end).max(start)
            }
            None => self.replacement_range(),
        })
    }

    fn delete_grapheme(&mut self, forward: bool, cx: &mut Context<Self>) {
        let selection = self.selection.range();
        let range = if !selection.is_empty() {
            selection
        } else {
            let Some(doc) = self.state.document() else {
                return;
            };
            let at = self.selection.cursor();
            if forward {
                at..cursor::grapheme_right(doc, at)
            } else {
                cursor::grapheme_left(doc, at)..at
            }
        };
        if range.is_empty() {
            return;
        }
        let caret = CodeSelection::at(range.start);
        self.splice_all(&[(range, String::new())], caret, EditGroup::Typing, cx);
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        let mut text = String::from("\n");
        if let Some(doc) = self.state.document() {
            let at = self.selection.range().start;
            let row = doc.byte_to_line(at);
            let start = doc.line_to_byte(row);
            if let Some(line) = doc.line_string(row) {
                let indent = edit::leading_indent(&line);
                let column = at.saturating_sub(start);
                text.push_str(&indent[..indent.len().min(column)]);
            }
        }
        self.insert_text(&text, EditGroup::Atomic, cx);
    }

    fn selected_rows(&self) -> Option<(usize, usize)> {
        let doc = self.state.document()?;
        let range = self.selection.range();
        let first = doc.byte_to_line(range.start);
        let last_byte = if range.end > range.start {
            range.end - 1
        } else {
            range.end
        };
        Some((first, doc.byte_to_line(last_byte).max(first)))
    }

    fn shift_lines(&mut self, outdent: bool, cx: &mut Context<Self>) {
        let Some((first, last)) = self.selected_rows() else {
            return;
        };
        if !outdent && self.selection.is_empty() {
            let unit = self.indent.as_str().into_owned();
            self.insert_text(&unit, EditGroup::Atomic, cx);
            return;
        }
        let unit = self.indent;
        let mut ops: Vec<(Range<usize>, String)> = Vec::new();
        let mut deltas: Vec<(usize, isize)> = Vec::new();
        {
            let Some(doc) = self.state.document() else {
                return;
            };
            for row in (first..=last).rev() {
                let start = doc.line_to_byte(row);
                let Some(line) = doc.line_string(row) else {
                    continue;
                };
                if outdent {
                    let width = edit::dedent_width(&line, unit);
                    if width == 0 {
                        continue;
                    }
                    ops.push((start..start + width, String::new()));
                    deltas.push((start, -(width as isize)));
                } else {
                    if line.trim_end_matches('\n').is_empty() {
                        continue;
                    }
                    let text = unit.as_str().into_owned();
                    let width = text.len() as isize;
                    ops.push((start..start, text));
                    deltas.push((start, width));
                }
            }
        }
        if ops.is_empty() {
            return;
        }
        let after = CodeSelection {
            anchor: shift_offset(self.selection.anchor, &deltas),
            head: shift_offset(self.selection.head, &deltas),
        };
        self.splice_all(&ops, after, EditGroup::Atomic, cx);
    }

    fn clip_range(&self) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        let selection = self.selection.range();
        if selection.is_empty() {
            Some(cursor::line_range_at(doc, selection.start))
        } else {
            Some(selection)
        }
    }

    fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        let Some(range) = self.clip_range() else {
            return;
        };
        if range.is_empty() {
            return;
        }
        let Some(doc) = self.state.document() else {
            return;
        };
        let text = doc.slice_string(range.clone());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if cut {
            let caret = CodeSelection::at(range.start);
            self.splice_all(&[(range, String::new())], caret, EditGroup::Atomic, cx);
        }
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let Some(text) = item.text() else {
            return;
        };
        let text = edit::sanitize_paste(&text);
        if text.is_empty() {
            return;
        }
        self.end_typing_group();
        self.insert_text(&text, EditGroup::Atomic, cx);
    }

    fn time_travel(&mut self, redo: bool, cx: &mut Context<Self>) {
        if self.state.document().is_none_or(CodeDocument::is_read_only) {
            self.flash_read_only(cx);
            return;
        }
        self.marked = None;
        let mut deferred: Option<DeferredParse> = None;
        let mut restored = None;
        let mut changes = Vec::new();
        if let Some((doc, hl)) = self.state.editable() {
            let step = if redo {
                self.history.redo(doc)
            } else {
                self.history.undo(doc)
            };
            if let Some(step) = step {
                for change in &step.edits {
                    if let HighlightOutcome::Deferred(parse) = hl.edit(doc, &change.edit) {
                        deferred = Some(parse);
                    }
                }
                changes = step.edits.iter().map(|change| change.window).collect();
                restored = Some(step.selection);
            }
        }
        let Some(selection) = restored else {
            return;
        };
        self.note_changes(&changes, cx);
        self.finish_edit(selection, deferred, cx);
    }

    pub(super) fn backspace(&mut self, _: &CeBackspace, _w: &mut Window, cx: &mut Context<Self>) {
        self.delete_grapheme(false, cx);
    }

    pub(super) fn delete(&mut self, _: &CeDelete, _w: &mut Window, cx: &mut Context<Self>) {
        self.delete_grapheme(true, cx);
    }

    pub(super) fn newline(&mut self, _: &CeNewline, _w: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    pub(super) fn undo(&mut self, _: &CeUndo, _w: &mut Window, cx: &mut Context<Self>) {
        self.time_travel(false, cx);
    }

    pub(super) fn redo(&mut self, _: &CeRedo, _w: &mut Window, cx: &mut Context<Self>) {
        self.time_travel(true, cx);
    }

    pub(super) fn copy(&mut self, _: &CeCopy, _w: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(false, cx);
    }

    pub(super) fn cut(&mut self, _: &CeCut, _w: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(true, cx);
    }

    pub(super) fn paste_action(&mut self, _: &CePaste, _w: &mut Window, cx: &mut Context<Self>) {
        self.paste(cx);
    }

    pub(super) fn indent(&mut self, _: &CeIndent, _w: &mut Window, cx: &mut Context<Self>) {
        self.shift_lines(false, cx);
    }

    pub(super) fn outdent(&mut self, _: &CeOutdent, _w: &mut Window, cx: &mut Context<Self>) {
        self.shift_lines(true, cx);
    }

    pub(super) fn escape(&mut self, _: &CeEscape, _w: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            self.close_marker_popup(cx);
        } else {
            cx.propagate();
        }
    }
}

fn shift_offset(offset: usize, deltas: &[(usize, isize)]) -> usize {
    let mut out = offset as isize;
    for (start, delta) in deltas {
        if *delta > 0 {
            if *start <= offset {
                out += delta;
            }
        } else if *start < offset {
            let removed = delta.unsigned_abs();
            out -= removed.min(offset - start) as isize;
        }
    }
    out.max(0) as usize
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::super::tests::*;
    use super::*;
    use crate::app::diff_dock::code::highlight::CodeHighlighter;
    use crate::app::diff_dock::code::load::{LoadedCode, build_document};

    #[gpui::test]
    fn typing_replaces_the_live_selection(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "hello world\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 5 };
            view.replace_text_in_range(None, "bye", window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "bye world\n");
            assert_eq!(view.cursor(), 3, "the caret lands past what was inserted");
            assert!(view.is_dirty(), "an edit marks the document dirty");
        });
    }

    #[gpui::test]
    async fn shortening_the_longest_line_refreshes_horizontal_extent_off_thread(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = view(cx, "the longest line\nshort\n");
        cx.executor().allow_parking();

        view.update(cx, |view, cx| {
            assert!(view.splice_all(
                &[(0..16, "tiny".to_string())],
                CodeSelection::at(4),
                EditGroup::Atomic,
                cx,
            ));
            assert_eq!(view.document().expect("document").longest_line_chars(), 16);
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if view.update(cx, |view, _cx| {
                view.document().expect("document").longest_line_chars() == 5
            }) {
                break;
            }
            smol::Timer::after(Duration::from_millis(1)).await;
        }

        view.update(cx, |view, _cx| {
            assert_eq!(view.document().expect("document").longest_line_chars(), 5);
        });
    }

    #[gpui::test]
    fn backspace_removes_a_whole_composed_emoji(cx: &mut TestAppContext) {
        let emoji = "\u{1F44D}\u{1F3FD}";
        let (view, cx) = view(cx, &format!("ok{emoji}\n"));
        let end = 2 + emoji.len();

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(end);
            view.backspace(&CeBackspace, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(
                text_of(view),
                "ok\n",
                "the whole grapheme went in one press"
            );
        });
    }

    #[gpui::test]
    fn enter_repeats_the_row_indentation(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "fn main() {\n    let x = 1;\n}\n");
        let at = "fn main() {\n    let x = 1;".len();

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(at);
            view.newline(&CeNewline, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "fn main() {\n    let x = 1;\n    \n}\n");
            assert_eq!(view.cursor(), at + 5, "the caret sits past the new indent");
        });
    }

    #[gpui::test]
    fn a_keystroke_on_a_read_only_document_is_refused_visibly(cx: &mut TestAppContext) {
        let path = PathBuf::from("/nonexistent/paneflow-code.rs");
        let document = build_document(path.clone(), "locked\n", true);
        let mut highlighter = CodeHighlighter::new(
            &document,
            DiffSyntax::from_theme(&crate::theme::paneflow_dark()),
        );
        highlighter.parse_initial_blocking(&document);
        let state = CodeLoadState::Ready(Box::new(LoadedCode {
            document,
            highlighter,
            indent: IndentUnit::Spaces(4),
            stamp: None,
        }));
        let (view, cx) =
            cx.add_window_view(move |_window, cx| CodeView::with_state(path, state, None, cx));

        view.update_in(cx, |view, window, cx| {
            view.replace_text_in_range(None, "x", window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "locked\n", "nothing was written");
            assert!(
                !view.is_dirty(),
                "a refused keystroke leaves no transaction"
            );
            assert!(
                view.read_only_flash.is_some(),
                "the refusal lights the banner up"
            );
        });
    }

    #[gpui::test]
    fn undo_on_a_read_only_document_is_refused_visibly(cx: &mut TestAppContext) {
        let (dir, view, cx) = file_view(cx, "one\ntwo\n", false);
        let path = dir.path().join("main.rs");

        view.update(cx, |view, _cx| {
            view.state
                .document_mut()
                .expect("document")
                .set_read_only(Some(ReadOnlyReason::Permissions));
        });

        std::fs::write(&path, "one\ntwo\nthree\n").expect("external write");
        let stamp = FileStamp::read(&path);
        view.update(cx, |view, cx| {
            view.disk_changed(stamp, Some("one\ntwo\nthree\n".to_string()), cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n", "the reload landed");
            assert!(
                !view.is_dirty(),
                "a silent reload leaves the document clean"
            );
        });

        view.update_in(cx, |view, window, cx| {
            view.undo(&CeUndo, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "one\ntwo\nthree\n", "nothing was replayed");
            assert!(!view.is_dirty(), "and the document is still clean");
            assert!(view.read_only_flash.is_some(), "the refusal is visible");
        });
    }

    #[gpui::test]
    fn undo_keeps_the_highlighting_a_fresh_parse_would_give(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "fn main() {\n    let value = 1;\n}\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(16);
            view.replace_text_in_range(None, "xyz", window, cx);
            view.undo(&CeUndo, window, cx);
        });
        cx.run_until_parked();

        view.update(cx, |view, _cx| {
            let (doc, live) = view.state.editable().expect("document and highlighter");
            live.requery_rows(doc, 0..doc.line_count());
            let mut oracle =
                CodeHighlighter::new(doc, DiffSyntax::from_theme(&crate::theme::paneflow_dark()));
            oracle.parse_initial_blocking(doc);
            oracle.requery_rows(doc, 0..doc.line_count());
            assert!(live.is_enabled(), "the grammar is loaded");
            assert!(
                !oracle.runs(1).is_empty(),
                "the oracle colors something, so the comparison means something"
            );
            for row in 0..doc.line_count() {
                assert_eq!(
                    live.runs(row),
                    oracle.runs(row),
                    "row {row} kept its coloring across the undo"
                );
            }
        });
    }

    #[gpui::test]
    fn keystrokes_group_until_the_caret_moves(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "\n");

        view.update_in(cx, |view, window, cx| {
            for letter in ["a", "b", "c"] {
                view.replace_text_in_range(None, letter, window, cx);
            }
            view.left(&CeLeft, window, cx);
            view.right(&CeRight, window, cx);
            view.replace_text_in_range(None, "d", window, cx);
            assert_eq!(text_of(view), "abcd\n");

            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "abc\n",
                "the post-move keystroke undid alone"
            );
            view.undo(&CeUndo, window, cx);
            assert_eq!(
                text_of(view),
                "\n",
                "the three grouped keystrokes undid together"
            );

            view.redo(&CeRedo, window, cx);
            assert_eq!(text_of(view), "abc\n", "redo replays the same grouping");
        });
    }

    #[gpui::test]
    fn undo_restores_the_selection_the_edit_replaced(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "hello world\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection {
                anchor: 6,
                head: 11,
            };
            view.replace_text_in_range(None, "there", window, cx);
            view.undo(&CeUndo, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "hello world\n");
            assert_eq!(view.selection(), 6..11, "the replaced selection came back");
        });
    }

    #[gpui::test]
    fn a_multi_line_paste_is_one_undo_step(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "start\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(6);
            cx.write_to_clipboard(ClipboardItem::new_string("one\r\ntwo\r\nthree".to_string()));
            view.paste_action(&CePaste, window, cx);
            assert_eq!(text_of(view), "start\none\ntwo\nthree");
            assert_eq!(
                view.cursor(),
                text_of(view).len(),
                "the caret is at the end"
            );

            view.undo(&CeUndo, window, cx);
            assert_eq!(text_of(view), "start\n", "the whole paste undid at once");
        });
    }

    #[gpui::test]
    fn a_paste_is_sanitized_before_insertion(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "\n");

        view.update_in(cx, |view, window, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(
                "let x = 1;\u{202E}\u{0007}\u{200B}".to_string(),
            ));
            view.paste_action(&CePaste, window, cx);
        });

        view.update(cx, |view, _cx| {
            assert_eq!(text_of(view), "let x = 1;\n");
        });
    }

    #[gpui::test]
    fn copy_with_no_selection_takes_the_whole_row(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "first\nsecond\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection::at(8);
            view.copy(&CeCopy, window, cx);
            let clipped = cx
                .read_from_clipboard()
                .and_then(|item| item.text())
                .unwrap_or_default();
            assert_eq!(clipped, "second\n");
            assert_eq!(text_of(view), "first\nsecond\n", "copy never mutates");
        });
    }

    #[gpui::test]
    fn tab_and_shift_tab_shift_every_touched_row(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo\nthree\n");

        view.update_in(cx, |view, window, cx| {
            view.selection = CodeSelection { anchor: 0, head: 8 };
            view.indent(&CeIndent, window, cx);
            assert_eq!(text_of(view), "    one\n    two\nthree\n");

            view.outdent(&CeOutdent, window, cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n");

            view.outdent(&CeOutdent, window, cx);
            assert_eq!(text_of(view), "one\ntwo\nthree\n");
        });
    }
}
