use std::ops::Range;

use ropey::{Rope, RopeSlice};

use super::rows::RowSpan;
use crate::tab_stops;

pub(crate) fn wrap_columns(text_viewport_w: f32, char_w: f32) -> usize {
    if char_w <= 0.0 || !text_viewport_w.is_finite() {
        return 1;
    }
    ((text_viewport_w / char_w).floor() as usize).max(1)
}

fn trimmed(line: RopeSlice<'_>) -> RopeSlice<'_> {
    let mut end = line.len_bytes();
    if end > 0 && line.byte(end - 1) == b'\n' {
        end -= 1;
    }
    if end > 0 && line.byte(end - 1) == b'\r' {
        end -= 1;
    }
    line.byte_slice(..end)
}

fn columns_between(line: RopeSlice<'_>, range: Range<usize>) -> usize {
    line.byte_slice(range).chars().fold(0, tab_stops::advance)
}

fn may_wrap(line: RopeSlice<'_>, cols: usize) -> bool {
    let bytes = line.len_bytes();
    if bytes <= cols {
        return line.chunks().any(|chunk| chunk.contains('\t'));
    }
    true
}

struct Breaker<'a, F: FnMut(Range<usize>)> {
    line: RopeSlice<'a>,
    cols: usize,
    start: usize,
    column: usize,
    last_break: Option<usize>,
    byte: usize,
    emit: F,
}

impl<F: FnMut(Range<usize>)> Breaker<'_, F> {
    fn step(&mut self, advance: impl Fn(usize) -> usize, len: usize, space: bool) {
        let mut next = advance(self.column);
        if next > self.cols && self.column > 0 {
            let byte = self.byte;
            match self.last_break.filter(|at| *at > self.start && *at <= byte) {
                Some(at) => {
                    (self.emit)(self.start..at);
                    self.start = at;
                    self.column = columns_between(self.line, at..byte);
                    if self.column >= self.cols {
                        (self.emit)(self.start..byte);
                        self.start = byte;
                        self.column = 0;
                    }
                }
                None => {
                    (self.emit)(self.start..byte);
                    self.start = byte;
                    self.column = 0;
                }
            }
            self.last_break = None;
            next = advance(self.column);
        }
        self.column = next;
        self.byte += len;
        if space {
            self.last_break = Some(self.byte);
        }
    }
}

fn ascii_advance(byte: u8) -> impl Fn(usize) -> usize {
    move |column| match byte {
        b'\t' => column + tab_stops::tab_len(column),
        byte if tab_stops::is_one_column_ascii(byte) => column + 1,
        _ => column,
    }
}

pub(crate) fn for_each_segment(
    line: RopeSlice<'_>,
    cols: usize,
    mut emit: impl FnMut(Range<usize>),
) {
    let line = trimmed(line);
    let len = line.len_bytes();
    let cols = cols.max(1);
    if !may_wrap(line, cols) {
        emit(0..len);
        return;
    }
    let mut breaker = Breaker {
        line,
        cols,
        start: 0,
        column: 0,
        last_break: None,
        byte: 0,
        emit: &mut emit,
    };
    for chunk in line.chunks() {
        if chunk.is_ascii() {
            for &byte in chunk.as_bytes() {
                breaker.step(ascii_advance(byte), 1, byte == b' ' || byte == b'\t');
            }
        } else {
            for ch in chunk.chars() {
                breaker.step(
                    |column| tab_stops::advance(column, ch),
                    ch.len_utf8(),
                    ch == ' ' || ch == '\t',
                );
            }
        }
    }
    let start = breaker.start;
    emit(start..len);
}

pub(crate) fn segments(line: RopeSlice<'_>, cols: usize) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    for_each_segment(line, cols, |segment| out.push(segment));
    out
}

#[cfg(test)]
pub(crate) fn extra_rows(line: RopeSlice<'_>, cols: usize) -> u32 {
    let mut count = 0u32;
    for_each_segment(line, cols, |_| count = count.saturating_add(1));
    count.saturating_sub(1)
}

pub(crate) const SEEDED_SEGMENT_ROWS: u32 = 64;

#[derive(Debug, Default)]
pub(crate) struct WrapLayout {
    pub(crate) spans: Vec<RowSpan>,
    pub(crate) seeded: Vec<(usize, Vec<Range<usize>>)>,
}

pub(crate) fn wrap_layout(text: &Rope, cols: usize) -> WrapLayout {
    let mut layout = WrapLayout::default();
    let mut parts = Vec::new();
    for (line, slice) in text.lines().enumerate() {
        parts.clear();
        for_each_segment(slice, cols, |segment| parts.push(segment));
        let wrap = (parts.len().saturating_sub(1)).min(u32::MAX as usize) as u32;
        if wrap == 0 {
            continue;
        }
        layout.spans.push(RowSpan {
            line,
            wrap,
            below: 0,
        });
        if wrap >= SEEDED_SEGMENT_ROWS {
            layout.seeded.push((line, std::mem::take(&mut parts)));
        }
    }
    layout
}

#[cfg(test)]
pub(crate) fn wrap_spans(text: &Rope, cols: usize) -> Vec<RowSpan> {
    wrap_layout(text, cols).spans
}

pub(crate) fn segment_index(segments: &[Range<usize>], local: usize) -> usize {
    let last = segments.len().saturating_sub(1);
    segments
        .iter()
        .position(|segment| local < segment.end)
        .unwrap_or(last)
        .min(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(text: &str, cols: usize) -> Vec<String> {
        let rope = Rope::from_str(text);
        segments(rope.line(0), cols)
            .into_iter()
            .map(|range| text[range].to_string())
            .collect()
    }

    #[test]
    fn a_short_line_is_one_segment() {
        assert_eq!(split("let a = 1;\n", 80), vec!["let a = 1;"]);
        assert_eq!(split("", 80), vec![""]);
        assert_eq!(extra_rows(Rope::from_str("abc").line(0), 3), 0);
    }

    #[test]
    fn a_long_line_breaks_after_the_last_space_that_fits() {
        assert_eq!(
            split("alpha beta gamma delta", 11),
            vec!["alpha beta ", "gamma delta"]
        );
        assert_eq!(
            split("alpha beta gamma delta", 8),
            vec!["alpha ", "beta ", "gamma ", "delta"]
        );
    }

    #[test]
    fn a_word_longer_than_the_row_breaks_mid_word() {
        assert_eq!(split("abcdefghij", 4), vec!["abcd", "efgh", "ij"]);
        assert_eq!(split("ab cdefghijkl", 4), vec!["ab ", "cdef", "ghij", "kl"]);
    }

    #[test]
    fn segments_cover_the_line_without_gaps() {
        let text = "fn main() { let \tvalue = \"a long string with café and 字 wide\"; }";
        for cols in 1..40 {
            let rope = Rope::from_str(text);
            let parts = segments(rope.line(0), cols);
            assert_eq!(parts.first().map(|r| r.start), Some(0));
            assert_eq!(parts.last().map(|r| r.end), Some(text.len()));
            for pair in parts.windows(2) {
                assert_eq!(pair[0].end, pair[1].start, "cols {cols}");
            }
            for part in &parts {
                assert!(text.is_char_boundary(part.start) && text.is_char_boundary(part.end));
            }
            assert_eq!(extra_rows(rope.line(0), cols) as usize, parts.len() - 1);
        }
    }

    #[test]
    fn the_segments_end_where_the_document_ends_the_line() {
        let rope = Rope::from_str("ab\r\r\nnext\n");
        assert_eq!(segments(rope.line(0), 1).last().map(|r| r.end), Some(3));
        assert_eq!(split("crlf\r\n", 80), vec!["crlf"]);
    }

    #[test]
    fn a_tab_counts_up_to_its_stop() {
        assert_eq!(split("\t\tab", 8), vec!["\t\t", "ab"]);
    }

    #[test]
    fn a_ten_thousand_character_line_wraps_into_rows() {
        let line = "x".repeat(10_001);
        let rope = Rope::from_str(&line);
        assert_eq!(extra_rows(rope.line(0), 100), 100);
        let spans = wrap_spans(&Rope::from_str(&format!("a\n{line}\nb\n")), 100);
        assert_eq!(
            spans,
            vec![RowSpan {
                line: 1,
                wrap: 100,
                below: 0
            }]
        );
    }

    #[test]
    fn the_segment_of_an_offset_is_the_first_that_ends_after_it() {
        let parts = vec![0..4, 4..8, 8..10];
        assert_eq!(segment_index(&parts, 0), 0);
        assert_eq!(segment_index(&parts, 3), 0);
        assert_eq!(segment_index(&parts, 4), 1);
        assert_eq!(segment_index(&parts, 10), 2);
        assert_eq!(segment_index(&parts, 99), 2);
        assert_eq!(segment_index(&[], 3), 0);
    }

    #[test]
    fn the_wrap_width_never_reaches_zero_columns() {
        assert_eq!(wrap_columns(0.0, 7.0), 1);
        assert_eq!(wrap_columns(700.0, 7.0), 100);
        assert_eq!(wrap_columns(700.0, 0.0), 1);
    }
}
