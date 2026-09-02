use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::{Rope, RopeSlice};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub(crate) fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => Self::Crlf,
            _ => Self::Lf,
        }
    }

    #[allow(dead_code)]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct DocPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CodeEdit {
    pub(crate) start_byte: usize,
    pub(crate) old_end_byte: usize,
    pub(crate) new_end_byte: usize,
    pub(crate) start_point: DocPoint,
    pub(crate) old_end_point: DocPoint,
    pub(crate) new_end_point: DocPoint,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReadOnlyReason {
    Permissions,
    GiantLine { chars: usize, limit: usize },
}

impl ReadOnlyReason {
    pub(crate) fn banner(self) -> String {
        match self {
            Self::Permissions => {
                "This file is read-only on disk, so editing is disabled.".to_string()
            }
            Self::GiantLine { chars, limit } => format!(
                "This file has a {chars}-character line, past the {limit}-character editing limit, \
                 so it opens read-only."
            ),
        }
    }
}

pub(crate) struct CodeDocument {
    path: PathBuf,
    ext: String,
    text: Rope,
    line_ending: LineEnding,
    read_only: Option<ReadOnlyReason>,
    longest_line_chars: usize,
}

impl std::fmt::Debug for CodeDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeDocument")
            .field("path", &self.path)
            .field("bytes", &self.text.len_bytes())
            .field("lines", &self.text.len_lines())
            .field("line_ending", &self.line_ending)
            .field("read_only", &self.read_only)
            .finish()
    }
}

impl CodeDocument {
    pub(crate) fn new(path: PathBuf, raw: &str) -> Self {
        let line_ending = LineEnding::detect(raw);
        let ext = crate::diff::file_ext(&path.to_string_lossy());
        let mut doc = Self {
            path,
            ext,
            text: Rope::from_str(&normalize_newlines(raw)),
            line_ending,
            read_only: None,
            longest_line_chars: 0,
        };
        doc.longest_line_chars = doc.measure_all_lines();
        doc
    }

    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn ext(&self) -> &str {
        &self.ext
    }

    #[allow(dead_code)]
    pub(crate) fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }

    pub(crate) fn line_count(&self) -> usize {
        self.text.len_lines()
    }

    pub(crate) fn read_only_reason(&self) -> Option<ReadOnlyReason> {
        self.read_only
    }

    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only.is_some()
    }

    pub(crate) fn set_read_only(&mut self, reason: Option<ReadOnlyReason>) {
        self.read_only = reason;
    }

    pub(crate) fn longest_line_chars(&self) -> usize {
        self.longest_line_chars
    }

    pub(crate) fn line_byte_range(&self, row: usize) -> Option<Range<usize>> {
        let span = self.line_span(row)?;
        let mut end = span.end;
        if end > span.start && self.text.byte(end - 1) == b'\n' {
            end -= 1;
        }
        if end > span.start && self.text.byte(end - 1) == b'\r' {
            end -= 1;
        }
        Some(span.start..end)
    }

    pub(crate) fn line(&self, row: usize) -> Option<RopeSlice<'_>> {
        let range = self.line_byte_range(row)?;
        Some(self.text.byte_slice(range))
    }

    pub(crate) fn line_string(&self, row: usize) -> Option<String> {
        self.line(row).map(|s| s.to_string())
    }

    pub(crate) fn byte_to_line(&self, byte: usize) -> usize {
        self.text.byte_to_line(byte.min(self.text.len_bytes()))
    }

    pub(crate) fn line_to_byte(&self, row: usize) -> usize {
        self.text
            .line_to_byte(row.min(self.text.len_lines().saturating_sub(1)))
    }

    pub(crate) fn point_at(&self, byte: usize) -> DocPoint {
        let byte = byte.min(self.text.len_bytes());
        let row = self.text.byte_to_line(byte);
        DocPoint {
            row,
            column: byte - self.text.line_to_byte(row),
        }
    }

    pub(crate) fn snap_to_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len_bytes());
        self.text.char_to_byte(self.text.byte_to_char(byte))
    }

    pub(crate) fn insert(&mut self, byte_offset: usize, text: &str) -> Option<CodeEdit> {
        if self.is_read_only() {
            return None;
        }
        let normalized = normalize_newlines(text);
        if normalized.is_empty() {
            return None;
        }
        let start_byte = self.snap_to_boundary(byte_offset);
        let start_point = self.point_at(start_byte);
        let char_idx = self.text.byte_to_char(start_byte);
        self.text.insert(char_idx, &normalized);

        let new_end_byte = start_byte + normalized.len();
        let new_end_point = self.point_at(new_end_byte);
        self.remeasure_rows(start_point.row, new_end_point.row);
        Some(CodeEdit {
            start_byte,
            old_end_byte: start_byte,
            new_end_byte,
            start_point,
            old_end_point: start_point,
            new_end_point,
        })
    }

    pub(crate) fn remove(&mut self, range: Range<usize>) -> Option<CodeEdit> {
        if self.is_read_only() {
            return None;
        }
        let start_byte = self.snap_to_boundary(range.start);
        let old_end_byte = self.snap_to_boundary(range.end);
        if old_end_byte <= start_byte {
            return None;
        }
        let start_point = self.point_at(start_byte);
        let old_end_point = self.point_at(old_end_byte);
        let start_char = self.text.byte_to_char(start_byte);
        let end_char = self.text.byte_to_char(old_end_byte);
        self.text.remove(start_char..end_char);

        self.remeasure_rows(start_point.row, start_point.row);
        Some(CodeEdit {
            start_byte,
            old_end_byte,
            new_end_byte: start_byte,
            start_point,
            old_end_point,
            new_end_point: start_point,
        })
    }

    pub(crate) fn slice_string(&self, range: Range<usize>) -> String {
        let start = self.snap_to_boundary(range.start);
        let end = self.snap_to_boundary(range.end.max(range.start));
        if end <= start {
            return String::new();
        }
        let start_char = self.text.byte_to_char(start);
        let end_char = self.text.byte_to_char(end);
        self.text.slice(start_char..end_char).to_string()
    }

    pub(crate) fn byte_to_utf16(&self, offset: usize) -> usize {
        let offset = self.snap_to_boundary(offset);
        let char_idx = self.text.byte_to_char(offset);
        self.text
            .slice(..char_idx)
            .chunks()
            .map(utf16_len)
            .sum::<usize>()
    }

    pub(crate) fn utf16_to_byte(&self, target: usize) -> usize {
        let mut units = 0usize;
        let mut byte = 0usize;
        for chunk in self.text.chunks() {
            let chunk_units = utf16_len(chunk);
            if units + chunk_units < target {
                units += chunk_units;
                byte += chunk.len();
                continue;
            }
            for ch in chunk.chars() {
                if units >= target {
                    return byte;
                }
                units += ch.len_utf16();
                byte += ch.len_utf8();
            }
            return byte;
        }
        byte
    }

    pub(crate) fn to_disk_string(&self) -> String {
        let text = self.text.to_string();
        match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }

    fn line_span(&self, row: usize) -> Option<Range<usize>> {
        let lines = self.text.len_lines();
        if row >= lines {
            return None;
        }
        let start = self.text.line_to_byte(row);
        let end = if row + 1 < lines {
            self.text.line_to_byte(row + 1)
        } else {
            self.text.len_bytes()
        };
        Some(start..end)
    }

    fn line_chars(&self, row: usize) -> usize {
        self.line(row).map_or(0, |l| l.len_chars())
    }

    fn measure_all_lines(&self) -> usize {
        (0..self.text.len_lines())
            .map(|row| self.line_chars(row))
            .max()
            .unwrap_or(0)
    }

    fn remeasure_rows(&mut self, first_row: usize, last_row: usize) {
        let last = last_row.min(self.text.len_lines().saturating_sub(1));
        for row in first_row..=last {
            let chars = self.line_chars(row);
            if chars > self.longest_line_chars {
                self.longest_line_chars = chars;
            }
        }
    }
}

fn utf16_len(chunk: &str) -> usize {
    if chunk.is_ascii() {
        chunk.len()
    } else {
        chunk.chars().map(char::len_utf16).sum()
    }
}

pub(crate) fn normalize_newlines(text: &str) -> Cow<'_, str> {
    if !text.contains('\r') {
        return Cow::Borrowed(text);
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    match String::from_utf8(out) {
        Ok(s) => Cow::Owned(s),
        Err(_) => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from("/tmp/sample.rs"), text)
    }

    #[test]
    fn empty_file_is_one_empty_line() {
        let d = doc("");
        assert_eq!(d.line_count(), 1);
        assert_eq!(d.line_string(0).as_deref(), Some(""));
        assert_eq!(d.longest_line_chars(), 0);
    }

    #[test]
    fn single_line_without_trailing_newline_gains_no_phantom_line() {
        let d = doc("fn main() {}");
        assert_eq!(d.line_count(), 1);
        assert_eq!(d.line_string(0).as_deref(), Some("fn main() {}"));
        assert_eq!(d.line(1), None);
    }

    #[test]
    fn trailing_newline_yields_the_final_empty_line() {
        let d = doc("a\nb\n");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.line_string(2).as_deref(), Some(""));
        assert_eq!(d.line(3), None);
    }

    #[test]
    fn line_is_returned_without_its_terminator() {
        let d = doc("alpha\nbeta\n");
        assert_eq!(d.line_string(0).as_deref(), Some("alpha"));
        assert_eq!(d.line_string(1).as_deref(), Some("beta"));
    }

    #[test]
    fn out_of_bounds_index_returns_none_rather_than_panicking() {
        let d = doc("only\n");
        assert_eq!(d.line(usize::MAX), None);
        assert_eq!(d.line_byte_range(9_999), None);
        assert_eq!(d.byte_to_line(usize::MAX), d.line_count() - 1);
        assert_eq!(d.line_to_byte(usize::MAX), d.len_bytes());
    }

    #[test]
    fn crlf_is_preserved_across_a_round_trip() {
        let d = doc("one\r\ntwo\r\n");
        assert_eq!(d.line_ending(), LineEnding::Crlf);
        assert_eq!(d.line_string(0).as_deref(), Some("one"));
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.to_disk_string(), "one\r\ntwo\r\n");
    }

    #[test]
    fn lf_stays_lf_across_a_round_trip() {
        let d = doc("one\ntwo\n");
        assert_eq!(d.line_ending(), LineEnding::Lf);
        assert_eq!(d.to_disk_string(), "one\ntwo\n");
    }

    #[test]
    fn a_lone_cr_is_not_a_line_break() {
        let d = doc("a\rb\n");
        assert_eq!(d.line_count(), 2);
        assert_eq!(d.line_string(0).as_deref(), Some("a\rb"));
        assert_eq!("a\rb\n".lines().count(), 1);
    }

    #[test]
    fn crlf_document_reemits_every_line_break_it_holds() {
        let mut d = doc("one\r\ntwo");
        d.insert(d.len_bytes(), "\nthree").expect("insert");
        assert_eq!(d.to_disk_string(), "one\r\ntwo\r\nthree");
    }

    #[test]
    fn inserted_crlf_is_normalized_to_a_single_break() {
        let mut d = doc("a\n");
        let edit = d.insert(0, "x\r\ny").expect("insert");
        assert_eq!(d.line_count(), 3);
        assert_eq!(d.line_string(0).as_deref(), Some("x"));
        assert_eq!(edit.new_end_byte, 3);
    }

    #[test]
    fn insert_reports_a_tree_sitter_shaped_edit() {
        let mut d = doc("alpha\nbeta\n");
        let edit = d.insert(6, "XY").expect("insert");
        assert_eq!(edit.start_byte, 6);
        assert_eq!(edit.old_end_byte, 6);
        assert_eq!(edit.new_end_byte, 8);
        assert_eq!(edit.start_point, DocPoint { row: 1, column: 0 });
        assert_eq!(edit.new_end_point, DocPoint { row: 1, column: 2 });
        assert_eq!(d.line_string(1).as_deref(), Some("XYbeta"));
    }

    #[test]
    fn remove_across_rows_reports_the_collapsed_point() {
        let mut d = doc("alpha\nbeta\ngamma\n");
        let edit = d.remove(3..8).expect("remove");
        assert_eq!(edit.start_point, DocPoint { row: 0, column: 3 });
        assert_eq!(edit.old_end_point, DocPoint { row: 1, column: 2 });
        assert_eq!(edit.new_end_point, edit.start_point);
        assert_eq!(d.line_string(0).as_deref(), Some("alpta"));
        assert_eq!(d.line_count(), 3);
    }

    #[test]
    fn an_empty_or_reversed_range_is_a_no_op() {
        let mut d = doc("abc");
        assert!(d.remove(2..2).is_none());
        let reversed = std::ops::Range { start: 3, end: 1 };
        assert!(d.remove(reversed).is_none());
        assert!(d.insert(0, "").is_none());
        assert_eq!(d.to_disk_string(), "abc");
    }

    #[test]
    fn edits_snap_to_char_boundaries_instead_of_panicking() {
        let mut d = doc("héllo");
        let edit = d.insert(2, "X").expect("insert");
        assert_eq!(edit.start_byte, 1);
        assert_eq!(d.line_string(0).as_deref(), Some("hXéllo"));
    }

    #[test]
    fn a_read_only_document_refuses_every_edit() {
        let mut d = doc("locked\n");
        d.set_read_only(Some(ReadOnlyReason::Permissions));
        assert!(d.insert(0, "x").is_none());
        assert!(d.remove(0..3).is_none());
        assert_eq!(d.to_disk_string(), "locked\n");
        assert!(
            d.read_only_reason()
                .expect("reason")
                .banner()
                .contains("read-only")
        );
    }

    #[test]
    fn longest_line_is_maintained_without_a_full_rescan() {
        let mut d = doc("ab\nabcd\nabc\n");
        assert_eq!(d.longest_line_chars(), 4);
        d.insert(0, "ZZZZZZZZ").expect("insert");
        assert_eq!(d.longest_line_chars(), 10);
        d.remove(0..8).expect("remove");
        assert_eq!(d.longest_line_chars(), 10);
    }

    #[test]
    fn longest_line_counts_characters_not_bytes() {
        let d = doc("ééé\nab\n");
        assert_eq!(d.longest_line_chars(), 3);
    }

    #[test]
    fn insert_in_the_middle_of_a_hundred_thousand_lines() {
        let mut text = String::with_capacity(100_000 * 8);
        for i in 0..100_000 {
            text.push_str(&format!("line {i}\n"));
        }
        let mut d = doc(&text);
        assert_eq!(d.line_count(), 100_001);

        let mid = d.line_to_byte(50_000);
        let edit = d.insert(mid, "// inserted\n").expect("insert");
        assert_eq!(edit.start_point.row, 50_000);
        assert_eq!(d.line_count(), 100_002);
        assert_eq!(d.line_string(50_000).as_deref(), Some("// inserted"));
        assert_eq!(d.line_string(50_001).as_deref(), Some("line 50000"));
        assert_eq!(d.byte_to_line(d.line_to_byte(99_999)), 99_999);
    }

    #[test]
    fn normalize_newlines_borrows_when_there_is_nothing_to_do() {
        assert!(matches!(
            normalize_newlines("plain\ntext"),
            Cow::Borrowed(_)
        ));
        assert_eq!(normalize_newlines("a\r\nb"), "a\nb");
        assert_eq!(normalize_newlines("a\rb"), "a\rb");
    }

    #[test]
    fn extension_is_lowercased_like_the_diff() {
        let d = CodeDocument::new(PathBuf::from("/tmp/Main.RS"), "");
        assert_eq!(d.ext(), "rs");
        let none = CodeDocument::new(PathBuf::from("/tmp/Makefile"), "");
        assert_eq!(none.ext(), "");
    }
}
