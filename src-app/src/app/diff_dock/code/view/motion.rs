use super::*;

impl CodeView {
    #[cfg(test)]
    pub(crate) fn set_cursor_row(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let row = row.min(doc.line_count().saturating_sub(1));
        let offset = doc.line_to_byte(row);
        self.place_caret(offset, false, cx);
    }

    fn place_caret(&mut self, offset: usize, extend: bool, cx: &mut Context<Self>) {
        self.end_typing_group();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = cursor::clamp(doc, offset);
        let goal = cursor::goal_column(doc, offset);
        self.goal_column = goal;
        self.selection.apply(offset, extend);
        self.after_motion(cx);
    }

    fn move_rows(&mut self, delta: isize, extend: bool, cx: &mut Context<Self>) {
        self.end_typing_group();
        let goal = self.goal_column;
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = cursor::vertical(doc, self.selection.cursor(), goal, delta);
        self.selection.apply(offset, extend);
        self.after_motion(cx);
    }

    pub(super) fn after_motion(&mut self, cx: &mut Context<Self>) {
        self.last_motion = Instant::now();
        self.blink_visible = true;
        self.reveal_cursor();
        cx.notify();
    }

    fn page_rows(&self) -> usize {
        cursor::page_rows(self.scroll.viewport_height(), CODE_ROW_HEIGHT)
    }

    pub(crate) fn reveal_cursor(&mut self) {
        let viewport_h = self.scroll.viewport_height();
        let geometry = self.geometry.get();
        let h_offset = self.h_offset;
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = self.selection.cursor();
        let row = doc.byte_to_line(offset);
        let column = cursor::goal_column(doc, offset);
        self.scroll.set_line_count(doc.line_count());

        let target = reveal_rows(row, viewport_h, self.scroll.max_rows(), self.scroll.rows());
        self.scroll.set_rows(target);
        let caret_x = column as f32 * geometry.char_w;
        self.h_offset = reveal_h_offset(
            caret_x,
            geometry.text_viewport_w,
            geometry.max_h_offset,
            h_offset,
        );
    }

    pub(super) fn left(&mut self, _: &CeLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(-1, false, cx);
    }

    pub(super) fn right(&mut self, _: &CeRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.horizontal(1, false, cx);
    }

    pub(super) fn select_left(
        &mut self,
        _: &CeSelectLeft,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.horizontal(-1, true, cx);
    }

    pub(super) fn select_right(
        &mut self,
        _: &CeSelectRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.horizontal(1, true, cx);
    }

    pub(super) fn up(&mut self, _: &CeUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, false, cx);
    }

    pub(super) fn down(&mut self, _: &CeDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(1, false, cx);
    }

    pub(super) fn select_up(&mut self, _: &CeSelectUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.move_rows(-1, true, cx);
    }

    pub(super) fn select_down(
        &mut self,
        _: &CeSelectDown,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_rows(1, true, cx);
    }

    pub(super) fn word_left(&mut self, _: &CeWordLeft, _w: &mut Window, cx: &mut Context<Self>) {
        self.word(-1, false, cx);
    }

    pub(super) fn word_right(&mut self, _: &CeWordRight, _w: &mut Window, cx: &mut Context<Self>) {
        self.word(1, false, cx);
    }

    pub(super) fn select_word_left(
        &mut self,
        _: &CeSelectWordLeft,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.word(-1, true, cx);
    }

    pub(super) fn select_word_right(
        &mut self,
        _: &CeSelectWordRight,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.word(1, true, cx);
    }

    pub(super) fn home(&mut self, _: &CeHome, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(false, false, cx);
    }

    pub(super) fn end(&mut self, _: &CeEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, false, cx);
    }

    pub(super) fn select_home(
        &mut self,
        _: &CeSelectHome,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.line_edge(false, true, cx);
    }

    pub(super) fn select_end(&mut self, _: &CeSelectEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.line_edge(true, true, cx);
    }

    pub(super) fn page_up(&mut self, _: &CePageUp, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(-1, false, cx);
    }

    pub(super) fn page_down(&mut self, _: &CePageDown, _w: &mut Window, cx: &mut Context<Self>) {
        self.page(1, false, cx);
    }

    pub(super) fn select_page_up(
        &mut self,
        _: &CeSelectPageUp,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page(-1, true, cx);
    }

    pub(super) fn select_page_down(
        &mut self,
        _: &CeSelectPageDown,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.page(1, true, cx);
    }

    pub(super) fn doc_start(&mut self, _: &CeDocStart, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(false, false, cx);
    }

    pub(super) fn doc_end(&mut self, _: &CeDocEnd, _w: &mut Window, cx: &mut Context<Self>) {
        self.doc_edge(true, false, cx);
    }

    pub(super) fn select_doc_start(
        &mut self,
        _: &CeSelectDocStart,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.doc_edge(false, true, cx);
    }

    pub(super) fn select_doc_end(
        &mut self,
        _: &CeSelectDocEnd,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.doc_edge(true, true, cx);
    }

    pub(super) fn select_all(&mut self, _: &CeSelectAll, _w: &mut Window, cx: &mut Context<Self>) {
        self.take_whole_document(cx);
    }

    fn horizontal(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let from = match (extend, self.selection.is_empty(), direction < 0) {
            (false, false, true) => self.selection.range().start,
            (false, false, false) => self.selection.range().end,
            _ => self.selection.cursor(),
        };
        let collapsing = !extend && !self.selection.is_empty();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if collapsing {
            from
        } else if direction < 0 {
            cursor::grapheme_left(doc, from)
        } else {
            cursor::grapheme_right(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn word(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let from = self.selection.cursor();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if direction < 0 {
            cursor::word_left(doc, from)
        } else {
            cursor::word_right(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let from = self.selection.cursor();
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if end {
            cursor::line_end(doc, from)
        } else {
            cursor::line_home(doc, from)
        };
        self.place_caret(offset, extend, cx);
    }

    fn page(&mut self, direction: isize, extend: bool, cx: &mut Context<Self>) {
        let rows = self.page_rows() as isize;
        self.move_rows(direction * rows, extend, cx);
    }

    fn doc_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(doc) = self.state.document() else {
            return;
        };
        let offset = if end { cursor::doc_end(doc) } else { 0 };
        self.place_caret(offset, extend, cx);
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::super::tests::*;
    use super::*;

    #[gpui::test]
    fn the_header_reads_the_caret_as_one_based_line_and_column(cx: &mut TestAppContext) {
        let (editor, cx) = view(cx, "let foo = 1;\nbb\nlast line");

        editor.update_in(cx, |view, window, cx| {
            assert_eq!(view.cursor_line_column(), (1, 1));

            view.right(&CeRight, window, cx);
            view.right(&CeRight, window, cx);
            view.right(&CeRight, window, cx);
            assert_eq!(view.cursor_line_column(), (1, 4));

            view.down(&CeDown, window, cx);
            assert_eq!(
                view.cursor_line_column().0,
                2,
                "the caret moved a line down"
            );

            view.doc_end(&CeDocEnd, window, cx);
            assert_eq!(view.cursor_line_column(), (3, 10), "end of `last line`");
        });

        let (loading, cx) = view(cx, "");
        loading.update(cx, |view, _cx| {
            assert!(view.document().is_none());
            assert_eq!(view.cursor_line_column(), (1, 1));
        });
    }

    #[gpui::test]
    fn the_navigation_actions_walk_the_document(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "let foo = 1;\nbb\nlast line");

        view.update_in(cx, |view, window, cx| {
            view.right(&CeRight, window, cx);
            assert_eq!(view.cursor(), 1);
            view.word_right(&CeWordRight, window, cx);
            assert_eq!(view.cursor(), 3, "end of `let`");
            view.end(&CeEnd, window, cx);
            assert_eq!(view.cursor(), 12);
            view.right(&CeRight, window, cx);
            assert_eq!(
                view.cursor(),
                13,
                "right at a row end steps to the next row"
            );
            view.home(&CeHome, window, cx);
            assert_eq!(view.cursor(), 13);
            view.doc_end(&CeDocEnd, window, cx);
            assert_eq!(view.cursor(), view.document().unwrap().len_bytes());
            view.doc_start(&CeDocStart, window, cx);
            assert_eq!(view.cursor(), 0);
            assert!(view.selection().is_empty(), "plain motion never selects");
        });
    }

    #[gpui::test]
    fn shift_extends_and_select_all_takes_the_document(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "abc\ndef");

        view.update_in(cx, |view, window, cx| {
            view.select_right(&CeSelectRight, window, cx);
            view.select_right(&CeSelectRight, window, cx);
            assert_eq!(view.selection(), 0..2);
            assert_eq!(view.cursor(), 2);

            view.left(&CeLeft, window, cx);
            assert_eq!(view.cursor(), 0, "collapses onto the near edge");
            assert!(view.selection().is_empty());

            view.select_all(&CeSelectAll, window, cx);
            assert_eq!(view.selection(), 0..7);
        });
    }

    #[gpui::test]
    fn vertical_motion_restores_the_goal_column(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "aaaaaaa\nbb\ncccccccc");

        view.update_in(cx, |view, window, cx| {
            view.place_caret(5, false, cx);
            view.down(&CeDown, window, cx);
            assert_eq!(view.cursor(), 10, "clamped to the short row");
            view.down(&CeDown, window, cx);
            assert_eq!(view.cursor(), 16, "the goal column comes back");
        });
    }

    #[gpui::test]
    fn the_caret_clamps_and_a_new_caret_clears_the_selection(cx: &mut TestAppContext) {
        let (view, cx) = view(cx, "one\ntwo");

        view.update(cx, |view, cx| {
            view.place_caret(9_999, false, cx);
            assert_eq!(view.cursor(), 7);

            view.take_whole_document(cx);
            assert_eq!(view.selection(), 0..7);
            let before = view.document().unwrap().len_bytes();
            view.place_caret(2, false, cx);
            assert!(view.selection().is_empty(), "the selection is gone");
            assert_eq!(
                view.document().unwrap().len_bytes(),
                before,
                "and the content is untouched"
            );
            assert_eq!(view.cursor_row(), 0, "the row follows the byte offset");
        });
    }
}
