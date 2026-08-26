//! `CodeDocument` - the editable text behind a file tab of the diff dock.
//!
//! prd-file-editor-2026-Q3, US-001. The text lives in a [`ropey::Rope`] rather
//! than a flat `String` + line index: an insertion in the middle of a 20 000
//! line file costs O(log n) instead of an O(n) memmove plus an O(lines) index
//! rebuild, and the rope already carries the line counters, so every byte <->
//! line conversion is a tree descent rather than a rescan.
//!
//! **Line-break alphabet.** `ropey` is pinned with `default-features = false`
//! (see `src-app/Cargo.toml`), which drops its `unicode_lines` and `cr_lines`
//! features: only `\n` ends a line here, exactly as in `str::lines()`, which is
//! how [`crate::diff::highlight_lines`] segments the diff. A lone `\r`, `\u{2028}`
//! or `\u{0085}` therefore cannot split a line in the editor while leaving the
//! diff's rows unsplit - the divergence US-004 forbids.
//!
//! **CRLF.** The rope always holds LF text. The document remembers the file's
//! [`LineEnding`] at load and re-applies it in [`CodeDocument::to_disk_string`],
//! so a CRLF file stays CRLF and an LF file stays LF across a round-trip
//! without a single `\r` reaching the layout or the parser. Text inserted at
//! runtime (a CRLF paste, say) is normalized the same way.
//!
//! **Longest line.** [`CodeDocument::longest_line_chars`] backs the horizontal
//! scroll extent of US-008. It is measured over every line exactly once, at
//! construction; afterwards each edit only measures the rows it actually
//! touched. That makes it grow-only between loads: deleting the longest line
//! leaves an over-estimate rather than paying a full rescan per keystroke. The
//! direction is deliberate - an over-estimate only offers scroll room nobody
//! uses, while an under-estimate would clip real text.

use std::borrow::Cow;
use std::ops::Range;
use std::path::{Path, PathBuf};

use ropey::{Rope, RopeSlice};

/// The line terminator a file uses on disk. Detected once at load from the
/// first `\n`, and re-applied on save.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    /// CRLF when the first `\n` in `text` is preceded by a `\r`, LF otherwise
    /// (which covers a file with no line break at all). Mixed files follow
    /// their first break; [`CodeDocument::to_disk_string`] then normalizes the
    /// rest to it, which is what every mainstream editor does.
    pub(crate) fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(i) if i > 0 && text.as_bytes()[i - 1] == b'\r' => Self::Crlf,
            _ => Self::Lf,
        }
    }

    #[allow(dead_code)] // EP-001 accessor: the writer round-trips the stored `LineEnding`; no caller needs its literal yet.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }
}

/// A `(row, column)` position, with `column` counted in **bytes from the start
/// of the row**. Same shape and units as `tree_sitter::Point`, which
/// `code::highlight` converts it into; kept as a local type so this module
/// stays free of any highlighting dependency.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct DocPoint {
    pub(crate) row: usize,
    pub(crate) column: usize,
}

/// One applied mutation, described in both byte offsets and points, in the form
/// tree-sitter's `InputEdit` needs (US-004). `old_*` describe the text *before*
/// the mutation, `new_*` the text after it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct CodeEdit {
    pub(crate) start_byte: usize,
    pub(crate) old_end_byte: usize,
    pub(crate) new_end_byte: usize,
    pub(crate) start_point: DocPoint,
    pub(crate) old_end_point: DocPoint,
    pub(crate) new_end_point: DocPoint,
}

/// Why a loaded document refuses edits. `None` on the document means editable.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ReadOnlyReason {
    /// The file is not writable by this process (POSIX mode bits, or the
    /// Windows read-only attribute).
    Permissions,
    /// A line longer than the editor's cap (US-003). Rendering it is fine;
    /// editing it is what breaks the vertical virtualization budget.
    GiantLine { chars: usize, limit: usize },
}

impl ReadOnlyReason {
    /// The written banner shown above the file (FR-7: never a raw OS error).
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

/// An editable file: its text, where it came from, and how it must be written
/// back. Holds no cursor, no selection and no highlighting - those belong to
/// EP-003 / EP-004 and to `code::highlight`.
pub(crate) struct CodeDocument {
    path: PathBuf,
    ext: String,
    text: Rope,
    line_ending: LineEnding,
    read_only: Option<ReadOnlyReason>,
    longest_line_chars: usize,
}

/// Deliberately hand-written rather than derived: a derived `Debug` would dump
/// the whole rope into any assertion message or log line, which is unusable for
/// a multi-megabyte file.
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
    /// Build a document from text just read off disk. `raw` may use either line
    /// ending; the rope keeps the LF form and [`Self::line_ending`] remembers
    /// the original. This is the only place that measures every line.
    pub(crate) fn new(path: PathBuf, raw: &str) -> Self {
        let line_ending = LineEnding::detect(raw);
        // Same derivation the diff uses, called through the same function, so
        // a path can never resolve to one grammar in the diff and another in
        // the editor (US-004).
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

    #[allow(dead_code)] // EP-001 accessor: the view holds the path it opened, so nothing reads it back off the document yet.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Lowercased extension, derived exactly like [`crate::diff::file_ext`] so
    /// the editor and the diff resolve the same grammar for the same path.
    pub(crate) fn ext(&self) -> &str {
        &self.ext
    }

    #[allow(dead_code)] // EP-001 accessor: the save path reads the field directly; no caller needs it off the document yet.
    pub(crate) fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub(crate) fn text(&self) -> &Rope {
        &self.text
    }

    pub(crate) fn len_bytes(&self) -> usize {
        self.text.len_bytes()
    }

    /// Number of lines, editor-style: an empty file is one empty line, and a
    /// file whose last byte is `\n` carries a final empty line (which is what
    /// the trailing newline means). A file with no trailing newline gains no
    /// phantom line.
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

    /// Widest line measured so far, in characters. Grow-only between loads -
    /// see the module header for why that direction is the safe one.
    pub(crate) fn longest_line_chars(&self) -> usize {
        self.longest_line_chars
    }

    /// Byte range of row `row`'s **content**, with the trailing `\n` (and a
    /// `\r` before it, should a degenerate mixed file have left one) excluded.
    /// Matches the slices `str::lines()` yields, which is what keeps per-line
    /// highlight runs interchangeable with the diff's.
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

    /// Row `row` without its line terminator, or `None` past the last row.
    pub(crate) fn line(&self, row: usize) -> Option<RopeSlice<'_>> {
        let range = self.line_byte_range(row)?;
        Some(self.text.byte_slice(range))
    }

    /// Row `row` materialized as a `String`. Convenience for callers that need
    /// an owned line (tests, clipboard); rendering reads the `RopeSlice`.
    pub(crate) fn line_string(&self, row: usize) -> Option<String> {
        self.line(row).map(|s| s.to_string())
    }

    /// Row containing `byte`, clamped into range. O(log n).
    pub(crate) fn byte_to_line(&self, byte: usize) -> usize {
        self.text.byte_to_line(byte.min(self.text.len_bytes()))
    }

    /// First byte of row `row`, clamped into range. O(log n).
    pub(crate) fn line_to_byte(&self, row: usize) -> usize {
        self.text
            .line_to_byte(row.min(self.text.len_lines().saturating_sub(1)))
    }

    /// `(row, byte column)` of `byte`, clamped into range.
    pub(crate) fn point_at(&self, byte: usize) -> DocPoint {
        let byte = byte.min(self.text.len_bytes());
        let row = self.text.byte_to_line(byte);
        DocPoint {
            row,
            column: byte - self.text.line_to_byte(row),
        }
    }

    /// Largest byte offset that is a char boundary and `<= byte`.
    pub(crate) fn snap_to_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.text.len_bytes());
        self.text.char_to_byte(self.text.byte_to_char(byte))
    }

    /// Insert `text` at `byte_offset`, returning the applied edit, or `None`
    /// when the document is read-only or `text` normalizes to nothing. Line
    /// endings in `text` are normalized to the rope's LF form, so the returned
    /// `new_end_byte` is authoritative and may be shorter than `text.len()`.
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

    /// Delete `range` (byte offsets, snapped to char boundaries), returning the
    /// applied edit, or `None` when the document is read-only or the range is
    /// empty.
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

    /// The text of `range` as an owned `String`, byte offsets snapped to char
    /// boundaries. The undo history (US-013) stores exactly this: what a splice
    /// removed, so it can be put back byte for byte.
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

    /// UTF-16 code-unit index of byte `offset` (US-012).
    ///
    /// GPUI's [`gpui::EntityInputHandler`] speaks UTF-16 because that is what
    /// every platform IME speaks; the document speaks bytes. `ropey` 1.6 ships
    /// no UTF-16 conversion, so this walks the rope's chunks and takes an
    /// `is_ascii` fast path per chunk - the vectorized check turns the common
    /// case (a source file that is ASCII up to the caret) into a length sum
    /// instead of a per-character loop.
    pub(crate) fn byte_to_utf16(&self, offset: usize) -> usize {
        let offset = self.snap_to_boundary(offset);
        let char_idx = self.text.byte_to_char(offset);
        self.text
            .slice(..char_idx)
            .chunks()
            .map(utf16_len)
            .sum::<usize>()
    }

    /// Inverse of [`Self::byte_to_utf16`]. An index that lands inside a
    /// surrogate pair resolves to the start of that character, which keeps the
    /// result a legal caret slot.
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

    /// The bytes to write to disk: the rope's LF text, with every `\n` turned
    /// back into the file's original terminator. A file that had no trailing
    /// newline still has none.
    pub(crate) fn to_disk_string(&self) -> String {
        let text = self.text.to_string();
        match self.line_ending {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }

    /// Raw byte span of row `row`, terminator included.
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

    /// Character width of row `row`, terminator excluded.
    fn line_chars(&self, row: usize) -> usize {
        self.line(row).map_or(0, |l| l.len_chars())
    }

    /// Full scan, load-time only (US-001 AC: a complete recompute is allowed
    /// exactly here).
    fn measure_all_lines(&self) -> usize {
        (0..self.text.len_lines())
            .map(|row| self.line_chars(row))
            .max()
            .unwrap_or(0)
    }

    /// Measure only the rows an edit touched and keep the running maximum.
    /// Bounded by the edit's own row span, never by `line_count`.
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

/// UTF-16 code units in `chunk`. ASCII is one unit per byte, which is the
/// whole point of the fast path.
fn utf16_len(chunk: &str) -> usize {
    if chunk.is_ascii() {
        chunk.len()
    } else {
        chunk.chars().map(char::len_utf16).sum()
    }
}

/// Collapse `\r\n` to `\n`. Borrows when there is nothing to do, which is the
/// common case on Linux and macOS. Only ASCII bytes are dropped, so the result
/// is still valid UTF-8; the fallback keeps the function total rather than
/// asserting that.
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
        // Clamping conversions stay in range for absurd inputs.
        assert_eq!(d.byte_to_line(usize::MAX), d.line_count() - 1);
        assert_eq!(d.line_to_byte(usize::MAX), d.len_bytes());
    }

    #[test]
    fn crlf_is_preserved_across_a_round_trip() {
        let d = doc("one\r\ntwo\r\n");
        assert_eq!(d.line_ending(), LineEnding::Crlf);
        // The rope holds LF, so no `\r` reaches layout or the parser...
        assert_eq!(d.line_string(0).as_deref(), Some("one"));
        assert_eq!(d.line_count(), 3);
        // ...but the file written back is byte-identical to the original.
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
        // `str::lines()` does not split on a bare `\r`, and neither may the
        // editor - that is the parity the ropey feature flags buy (US-004).
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
        // The edit reports the bytes actually inserted, not the input length.
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
        // Built by hand: a `3..1` literal is a clippy::reversed_empty_ranges
        // error, and a reversed range is exactly what this asserts is inert.
        let reversed = std::ops::Range { start: 3, end: 1 };
        assert!(d.remove(reversed).is_none());
        assert!(d.insert(0, "").is_none());
        assert_eq!(d.to_disk_string(), "abc");
    }

    #[test]
    fn edits_snap_to_char_boundaries_instead_of_panicking() {
        let mut d = doc("héllo");
        // Byte 2 is inside the two-byte `é`; the edit lands before it.
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
        // Only the edited rows are re-measured, so the maximum is grow-only:
        // deleting the widest line leaves the over-estimate rather than paying
        // an O(lines) rescan on a keystroke.
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
        // The rope's counters answer conversions; nothing rescanned.
        assert_eq!(d.byte_to_line(d.line_to_byte(99_999)), 99_999);
    }

    #[test]
    fn normalize_newlines_borrows_when_there_is_nothing_to_do() {
        assert!(matches!(
            normalize_newlines("plain\ntext"),
            Cow::Borrowed(_)
        ));
        assert_eq!(normalize_newlines("a\r\nb"), "a\nb");
        // A bare `\r` is content, not a terminator, so it survives.
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
