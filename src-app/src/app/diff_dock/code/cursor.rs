use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::document::CodeDocument;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CodeSelection {
    pub(crate) anchor: usize,
    pub(crate) head: usize,
}

impl CodeSelection {
    pub(crate) fn at(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub(crate) fn cursor(self) -> usize {
        self.head
    }

    pub(crate) fn range(self) -> Range<usize> {
        if self.anchor <= self.head {
            self.anchor..self.head
        } else {
            self.head..self.anchor
        }
    }

    pub(crate) fn is_empty(self) -> bool {
        self.anchor == self.head
    }

    pub(crate) fn collapse_to(&mut self, offset: usize) {
        self.anchor = offset;
        self.head = offset;
    }

    pub(crate) fn extend_to(&mut self, offset: usize) {
        self.head = offset;
    }

    pub(crate) fn apply(&mut self, offset: usize, extend: bool) {
        if extend {
            self.extend_to(offset);
        } else {
            self.collapse_to(offset);
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn row_of(doc: &CodeDocument, offset: usize) -> (usize, Range<usize>) {
    let row = doc.byte_to_line(offset);
    let range = doc
        .line_byte_range(row)
        .unwrap_or_else(|| doc.len_bytes()..doc.len_bytes());
    (row, range)
}

fn snap_local_to_grapheme(line: &str, local: usize) -> usize {
    let local = local.min(line.len());
    let mut last = 0;
    for (idx, _) in line.grapheme_indices(true) {
        if idx > local {
            return last;
        }
        last = idx;
    }
    if local >= line.len() {
        line.len()
    } else {
        last
    }
}

pub(crate) fn clamp(doc: &CodeDocument, offset: usize) -> usize {
    let offset = doc.snap_to_boundary(offset);
    let (row, range) = row_of(doc, offset);
    if offset <= range.start {
        return range.start;
    }
    if offset >= range.end {
        return range.end;
    }
    let Some(line) = doc.line_string(row) else {
        return range.end;
    };
    range.start + snap_local_to_grapheme(&line, offset - range.start)
}

pub(crate) fn offset_at_column(doc: &CodeDocument, row: usize, col: usize) -> usize {
    let row = row.min(doc.line_count().saturating_sub(1));
    let Some(range) = doc.line_byte_range(row) else {
        return doc.len_bytes();
    };
    let Some(slice) = doc.line(row) else {
        return range.start;
    };
    let col = col.min(slice.len_chars());
    let local = slice.char_to_byte(col);
    clamp(doc, range.start + local)
}

pub(crate) fn goal_column(doc: &CodeDocument, offset: usize) -> usize {
    let (row, range) = row_of(doc, offset);
    let Some(slice) = doc.line(row) else {
        return 0;
    };
    let local = offset.saturating_sub(range.start).min(slice.len_bytes());
    slice.byte_to_char(local)
}

pub(crate) fn grapheme_left(doc: &CodeDocument, offset: usize) -> usize {
    let (row, range) = row_of(doc, offset);
    if offset <= range.start {
        if row == 0 {
            return 0;
        }
        return doc
            .line_byte_range(row - 1)
            .map(|r| r.end)
            .unwrap_or(range.start);
    }
    let Some(line) = doc.line_string(row) else {
        return range.start;
    };
    let local = (offset - range.start).min(line.len());
    let prev = line[..local]
        .grapheme_indices(true)
        .next_back()
        .map(|(i, _)| i)
        .unwrap_or(0);
    range.start + prev
}

pub(crate) fn grapheme_right(doc: &CodeDocument, offset: usize) -> usize {
    let (row, range) = row_of(doc, offset);
    if offset >= range.end {
        if row + 1 >= doc.line_count() {
            return range.end;
        }
        return doc.line_to_byte(row + 1);
    }
    let Some(line) = doc.line_string(row) else {
        return range.end;
    };
    let local = (offset - range.start).min(line.len());
    let next = line[local..]
        .graphemes(true)
        .next()
        .map(|g| local + g.len())
        .unwrap_or(line.len());
    range.start + next
}

pub(crate) fn word_left(doc: &CodeDocument, offset: usize) -> usize {
    let (row, range) = row_of(doc, offset);
    if offset <= range.start {
        return grapheme_left(doc, offset);
    }
    let Some(line) = doc.line_string(row) else {
        return range.start;
    };
    let local = (offset - range.start).min(line.len());
    let head: Vec<(usize, char)> = line[..local].char_indices().collect();
    let mut i = head.len();
    while i > 0 && head[i - 1].1.is_whitespace() {
        i -= 1;
    }
    if i > 0 {
        let word = is_word_char(head[i - 1].1);
        while i > 0 && !head[i - 1].1.is_whitespace() && is_word_char(head[i - 1].1) == word {
            i -= 1;
        }
    }
    let local = head.get(i).map(|(b, _)| *b).unwrap_or(local);
    range.start + local
}

pub(crate) fn word_right(doc: &CodeDocument, offset: usize) -> usize {
    let (row, range) = row_of(doc, offset);
    if offset >= range.end {
        return grapheme_right(doc, offset);
    }
    let Some(line) = doc.line_string(row) else {
        return range.end;
    };
    let local = (offset - range.start).min(line.len());
    let tail: Vec<(usize, char)> = line[local..].char_indices().collect();
    let mut i = 0;
    while i < tail.len() && tail[i].1.is_whitespace() {
        i += 1;
    }
    if i < tail.len() {
        let word = is_word_char(tail[i].1);
        while i < tail.len() && !tail[i].1.is_whitespace() && is_word_char(tail[i].1) == word {
            i += 1;
        }
    }
    let end = tail.get(i).map(|(b, _)| local + *b).unwrap_or(line.len());
    range.start + end
}

pub(crate) fn line_home(doc: &CodeDocument, offset: usize) -> usize {
    row_of(doc, offset).1.start
}

pub(crate) fn line_end(doc: &CodeDocument, offset: usize) -> usize {
    row_of(doc, offset).1.end
}

pub(crate) fn doc_end(doc: &CodeDocument) -> usize {
    doc.len_bytes()
}

pub(crate) fn vertical(doc: &CodeDocument, offset: usize, goal: usize, delta: isize) -> usize {
    let row = doc.byte_to_line(offset) as isize;
    let last = doc.line_count().saturating_sub(1) as isize;
    let target = (row + delta).clamp(0, last) as usize;
    offset_at_column(doc, target, goal)
}

pub(crate) fn page_rows(viewport_h: f32, row_height: f32) -> usize {
    if row_height <= 0. {
        return 1;
    }
    let rows = (viewport_h / row_height).floor() as isize;
    (rows - 1).max(1) as usize
}

pub(crate) fn word_range_at(doc: &CodeDocument, offset: usize) -> Range<usize> {
    let (row, range) = row_of(doc, offset);
    let Some(line) = doc.line_string(row) else {
        return range;
    };
    if line.is_empty() {
        return range.start..range.end;
    }
    let local = (offset - range.start).min(line.len());
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut at = chars.partition_point(|(b, _)| *b < local);
    if at >= chars.len() {
        at = chars.len() - 1;
    } else if chars[at].0 > local {
        at -= 1;
    }
    if at > 0 && chars[at].1.is_whitespace() && is_word_char(chars[at - 1].1) {
        at -= 1;
    }
    let pivot = chars[at].1;
    let class = |c: char| {
        if c.is_whitespace() {
            0
        } else if is_word_char(c) {
            1
        } else {
            2
        }
    };
    let want = class(pivot);
    if want == 2 {
        let end = chars.get(at + 1).map(|(b, _)| *b).unwrap_or(line.len());
        return range.start + chars[at].0..range.start + end;
    }
    let mut start = at;
    while start > 0 && class(chars[start - 1].1) == want {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && class(chars[end].1) == want {
        end += 1;
    }
    let end_byte = chars.get(end).map(|(b, _)| *b).unwrap_or(line.len());
    range.start + chars[start].0..range.start + end_byte
}

pub(crate) fn line_range_at(doc: &CodeDocument, offset: usize) -> Range<usize> {
    let (row, range) = row_of(doc, offset);
    let end = if row + 1 < doc.line_count() {
        doc.line_to_byte(row + 1)
    } else {
        range.end
    };
    range.start..end
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn doc(text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from("t.rs"), text)
    }

    #[test]
    fn a_selection_orders_its_range_whichever_way_it_was_dragged() {
        let mut sel = CodeSelection::at(10);
        assert!(sel.is_empty());
        sel.extend_to(4);
        assert_eq!(sel.range(), 4..10);
        assert_eq!(sel.cursor(), 4);
        sel.collapse_to(7);
        assert!(sel.is_empty());
        assert_eq!(sel.range(), 7..7);
    }

    #[test]
    fn the_caret_never_lands_inside_a_grapheme_cluster() {
        let d = doc("a\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}b");
        let cluster_end = d.len_bytes() - 1;
        for byte in 0..=d.len_bytes() {
            let snapped = clamp(&d, byte);
            assert!(
                snapped == 0 || snapped == 1 || snapped == cluster_end || snapped == d.len_bytes(),
                "byte {byte} snapped to {snapped}, which splits the cluster"
            );
        }
        assert_eq!(grapheme_right(&d, 1), cluster_end);
        assert_eq!(grapheme_left(&d, cluster_end), 1);
    }

    #[test]
    fn horizontal_motion_steps_over_the_line_terminator() {
        let d = doc("ab\ncd");
        assert_eq!(
            grapheme_right(&d, 2),
            3,
            "end of row 0 jumps to start of row 1"
        );
        assert_eq!(
            grapheme_left(&d, 3),
            2,
            "start of row 1 jumps back to end of row 0"
        );
        assert_eq!(grapheme_left(&d, 0), 0, "document start is a wall");
        assert_eq!(grapheme_right(&d, 5), 5, "document end is a wall");
    }

    #[test]
    fn a_click_past_the_last_line_lands_on_the_end_of_the_file() {
        let d = doc("one\ntwo\n");
        assert_eq!(d.line_count(), 3);
        assert_eq!(clamp(&d, 9_999), d.len_bytes());
        assert_eq!(offset_at_column(&d, 99, 40), d.len_bytes());
    }

    #[test]
    fn vertical_motion_keeps_the_goal_column_across_a_shorter_line() {
        let d = doc("aaaaaaa\nbb\ncccccccc");
        let start = 5;
        let goal = goal_column(&d, start);
        assert_eq!(goal, 5);
        let mid = vertical(&d, start, goal, 1);
        assert_eq!(mid, 10, "clamped to the end of the short row");
        let low = vertical(&d, mid, goal, 1);
        assert_eq!(low, 11 + 5, "the goal column comes back on the long row");
    }

    #[test]
    fn the_goal_column_counts_chars_not_bytes() {
        let d = doc("éé\nabcd");
        let end_of_first = line_end(&d, 0);
        assert_eq!(end_of_first, 4, "two 2-byte chars");
        assert_eq!(goal_column(&d, end_of_first), 2);
        assert_eq!(vertical(&d, end_of_first, 2, 1), 5 + 2);
    }

    #[test]
    fn word_motion_skips_whitespace_then_one_class_of_character() {
        let d = doc("let foo_bar = 1;");
        assert_eq!(word_right(&d, 0), 3, "end of `let`");
        assert_eq!(word_right(&d, 3), 11, "whitespace then `foo_bar`");
        assert_eq!(word_left(&d, 11), 4, "back to the start of `foo_bar`");
        assert_eq!(word_left(&d, 0), 0);
        let d = doc("ab\ncd");
        assert_eq!(word_right(&d, 2), 3);
        assert_eq!(word_left(&d, 3), 2);
    }

    #[test]
    fn double_click_picks_the_word_and_triple_click_the_line() {
        let d = doc("let foo_bar = 1;\nnext");
        assert_eq!(word_range_at(&d, 6), 4..11);
        assert_eq!(word_range_at(&d, 4), 4..11);
        assert_eq!(
            word_range_at(&d, 11),
            4..11,
            "the trailing edge still counts"
        );
        assert_eq!(word_range_at(&d, 12), 12..13, "punctuation selects itself");
        assert_eq!(word_range_at(&d, 13), 13..14, "a lone space selects itself");
        assert_eq!(line_range_at(&d, 6), 0..17, "terminator included");
        assert_eq!(line_range_at(&d, 18), 17..21, "last row has no terminator");
    }

    #[test]
    fn home_and_end_stay_inside_their_own_row() {
        let d = doc("  indented\nnext");
        assert_eq!(line_home(&d, 5), 0);
        assert_eq!(line_end(&d, 5), 10);
        assert_eq!(line_home(&d, 12), 11);
        assert_eq!(line_end(&d, 12), 15);
        assert_eq!(doc_end(&d), 15);
    }

    #[test]
    fn a_page_keeps_one_row_of_overlap_and_never_stalls() {
        assert_eq!(page_rows(180., 18.), 9);
        assert_eq!(page_rows(18., 18.), 1, "a one-row viewport still moves");
        assert_eq!(page_rows(0., 18.), 1);
        assert_eq!(page_rows(180., 0.), 1, "no division by zero");
    }

    #[test]
    fn an_empty_document_has_exactly_one_caret_slot() {
        let d = doc("");
        assert_eq!(d.line_count(), 1);
        assert_eq!(clamp(&d, 12), 0);
        assert_eq!(grapheme_left(&d, 0), 0);
        assert_eq!(grapheme_right(&d, 0), 0);
        assert_eq!(vertical(&d, 0, 4, -1), 0);
        assert_eq!(word_range_at(&d, 0), 0..0);
    }
}
