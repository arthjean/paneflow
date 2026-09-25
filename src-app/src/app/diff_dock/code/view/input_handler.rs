use super::*;

impl EntityInputHandler for CodeView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let doc = self.state.document()?;
        let start = doc.utf16_to_byte(range_utf16.start);
        let end = doc.utf16_to_byte(range_utf16.end).max(start);
        *adjusted_range = Some(doc.byte_to_utf16(start)..doc.byte_to_utf16(end));
        Some(doc.slice_string(start..end))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let doc = self.state.document()?;
        let range = self.selection.range();
        Some(UTF16Selection {
            range: doc.byte_to_utf16(range.start)..doc.byte_to_utf16(range.end),
            reversed: self.selection.head < self.selection.anchor,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let doc = self.state.document()?;
        let marked = self.marked.clone()?;
        Some(doc.byte_to_utf16(marked.start)..doc.byte_to_utf16(marked.end))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            self.history.close_group();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.resolve_replacement(range_utf16) else {
            return;
        };
        self.marked = None;
        let inserted = normalize_newlines(text).into_owned();
        let caret = CodeSelection::at(range.start + inserted.len());
        self.splice_all(&[(range, inserted)], caret, EditGroup::Typing, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.resolve_replacement(range_utf16) else {
            return;
        };
        let inserted = normalize_newlines(new_text).into_owned();
        let start = range.start;
        let end = start + inserted.len();
        let caret = CodeSelection::at(end);
        if !self.splice_all(&[(range, inserted)], caret, EditGroup::Typing, cx) {
            return;
        }
        self.marked = if start == end { None } else { Some(start..end) };
        if let Some(selected) = new_selected_range_utf16
            && let Some(doc) = self.state.document()
        {
            let base = doc.byte_to_utf16(start);
            let head = doc.utf16_to_byte(base + selected.end);
            let anchor = doc.utf16_to_byte(base + selected.start);
            self.selection = CodeSelection { anchor, head };
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let geometry = self.geometry.get();
        let doc = self.state.document()?;
        let start = doc.utf16_to_byte(range_utf16.start);
        let row = doc.byte_to_line(start);
        let column = cursor::goal_column(doc, start);
        let hits = self.hits.borrow();
        let (x, y) = if hits.lines.is_empty() {
            (
                f32::from(element_bounds.origin.x),
                f32::from(element_bounds.origin.y),
            )
        } else {
            (
                hits.text_x + column as f32 * geometry.char_w,
                hits.top_y + row.saturating_sub(hits.first_row) as f32 * CODE_ROW_HEIGHT,
            )
        };
        Some(Bounds {
            origin: Point::new(px(x), px(y)),
            size: size(px(geometry.char_w.max(1.0)), px(CODE_ROW_HEIGHT)),
        })
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let doc = self.state.document()?;
        let offset = self.hits.borrow().offset_at(doc, point);
        Some(doc.byte_to_utf16(offset))
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::super::tests::*;
    use super::*;

    #[gpui::test]
    fn the_ime_reads_the_caret_from_the_painted_rows(cx: &mut TestAppContext) {
        let (view, cx) = scrolled(cx, "/nonexistent/ime.rs", "let alpha = 1;\nlet beta = 2;\n");

        let (caret, index) = view.update_in(cx, |view, window, cx| {
            let element_bounds = view.scroll.bounds();
            let caret = view
                .bounds_for_range(4..9, element_bounds, window, cx)
                .expect("a laid out row");
            let index = view
                .character_index_for_point(caret.origin, window, cx)
                .expect("a laid out row");
            (caret, index)
        });
        assert_eq!(index, 4, "the IME round trips the caret it was given");
        assert_eq!(f32::from(caret.size.height), CODE_ROW_HEIGHT);
        assert_eq!(
            f32::from(caret.origin.y),
            view.read_with(cx, |view, _| view.row_top(0)),
            "the IME caret sits on the painted row"
        );
    }
}
