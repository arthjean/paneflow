use std::ops::Range;

use crate::ffi::{self, RawLine};
use crate::language::{Language, TokenId};
use crate::lexer::Lexer;
use crate::{status, LexError};

const RUN_EVERYTHING: u32 = i32::MAX as u32;
const CHANGE_RECORD_WORDS: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub range: Range<usize>,
    pub id: TokenId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineChange {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditOutcome {
    pub changes: Vec<LineChange>,
    pub line_count: usize,
}

impl EditOutcome {
    pub fn changed_lines(&self) -> Option<Range<usize>> {
        let start = self.changes.iter().map(|change| change.new.start).min()?;
        let end = self.changes.iter().map(|change| change.new.end).max()?;
        Some(start..end.max(start))
    }
}

pub struct LexDocument {
    lexer: Lexer,
    language: Language,
    line_starts: Vec<usize>,
    len: usize,
}

struct LineView<'a> {
    text: &'a [u8],
    utf16_len: usize,
    terminator: usize,
    lone_cr: bool,
    records: &'a [u32],
    wide: bool,
}

fn to_u32(value: usize) -> Result<u32, LexError> {
    u32::try_from(value).map_err(|_| LexError::TooLarge(value))
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn line_starts_of(text: &str) -> Vec<usize> {
    let bytes = text.as_bytes();
    let mut starts = Vec::with_capacity(bytes.len() / 32 + 1);
    starts.push(0);
    let mut skip_lf_at = None;
    for at in memchr::memchr2_iter(b'\n', b'\r', bytes) {
        if skip_lf_at == Some(at) {
            continue;
        }
        if bytes[at] == b'\r' && bytes.get(at + 1) == Some(&b'\n') {
            skip_lf_at = Some(at + 1);
            starts.push(at + 2);
        } else {
            starts.push(at + 1);
        }
    }
    starts
}

fn utf16_units(prefix: &[u8]) -> Option<usize> {
    Some(
        std::str::from_utf8(prefix)
            .ok()?
            .chars()
            .map(char::len_utf16)
            .sum(),
    )
}

fn line_text<'a>(view: &LineView<'a>) -> Result<&'a str, LexError> {
    std::str::from_utf8(view.text).map_err(|_| LexError::OutOfBounds)
}

fn convert_tokens(view: &LineView<'_>, text: &str, out: &mut Vec<Token>) {
    let ascii = view.text.len() == view.utf16_len;
    let mut chars = text.char_indices().peekable();
    let mut units = 0usize;
    let mut start = 0usize;
    let mut start_units = 0usize;
    let stride = if view.wide { 2 } else { 1 };
    for record in view.records.chunks_exact(stride) {
        let (end_units, id) = if view.wide {
            (to_usize(record[0]), record[1])
        } else {
            (to_usize(record[0] & ffi::PACKED_END_MASK), record[0] >> 24)
        };
        let end_units = end_units.min(view.utf16_len);
        if end_units <= start_units {
            continue;
        }
        let end = if ascii {
            end_units
        } else {
            while units < end_units {
                match chars.next() {
                    Some((_, ch)) => units += ch.len_utf16(),
                    None => break,
                }
            }
            chars.peek().map_or(view.text.len(), |(at, _)| *at)
        };
        out.push(Token {
            range: start..end,
            id: TokenId::from_raw(id),
        });
        start = end;
        start_units = end_units;
    }
}

impl LexDocument {
    pub(crate) fn new(lexer: Lexer, text: &str, language: Language) -> Result<Self, LexError> {
        let len = to_u32(text.len())?;
        let code = unsafe {
            ffi::paneflow_hl_init(lexer.raw(), text.as_ptr(), len, u32::from(language.id()))
        };
        status(code, text.len(), lexer.memory_limit())?;
        let document = Self {
            lexer,
            language,
            line_starts: line_starts_of(text),
            len: text.len(),
        };
        let lines = document.raw_line_count()?;
        if lines != document.line_starts.len() {
            return Err(LexError::OutOfBounds);
        }
        Ok(document)
    }

    pub fn lex(text: &str, language: Language) -> Result<Self, LexError> {
        let mut document = Lexer::new()?.document(text, language)?;
        document.settle()?;
        Ok(document)
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> Option<usize> {
        self.line_starts.get(line).copied()
    }

    pub fn memory_bytes(&self) -> usize {
        self.lexer.memory_bytes()
    }

    pub fn settle(&mut self) -> Result<(), LexError> {
        self.run(RUN_EVERYTHING)
    }

    pub fn run(&mut self, line_budget: u32) -> Result<(), LexError> {
        let code = unsafe { ffi::paneflow_hl_run(self.lexer.raw(), line_budget, 0, 0) };
        status(code, 0, self.lexer.memory_limit())
    }

    pub fn run_before(&mut self, line_budget: u32, end_line: usize) -> Result<(), LexError> {
        if end_line == 0 {
            return Ok(());
        }
        let end = to_u32(end_line)?;
        let code = unsafe { ffi::paneflow_hl_run(self.lexer.raw(), line_budget, end, 0) };
        status(code, 0, self.lexer.memory_limit())
    }

    pub fn pending_lines(&self) -> Result<usize, LexError> {
        self.stat(ffi::STAT_PENDING_LINES)
    }

    pub fn next_pending_line(&self) -> Result<Option<usize>, LexError> {
        if self.pending_lines()? == 0 {
            return Ok(None);
        }
        Ok(Some(self.stat(ffi::STAT_NEXT_PENDING_LINE)?))
    }

    fn stat(&self, index: u32) -> Result<usize, LexError> {
        let mut value = 0;
        let code = unsafe { ffi::paneflow_hl_stat(self.lexer.raw(), index, &mut value) };
        status(code, 0, self.lexer.memory_limit())?;
        Ok(to_usize(value))
    }

    fn raw_line_count(&self) -> Result<usize, LexError> {
        let mut count = 0;
        let code = unsafe { ffi::paneflow_hl_line_count(self.lexer.raw(), &mut count) };
        status(code, 0, self.lexer.memory_limit())?;
        Ok(to_usize(count))
    }

    fn view(&self, line: usize) -> Result<LineView<'_>, LexError> {
        let mut raw = RawLine::empty();
        let code = unsafe { ffi::paneflow_hl_read_line(self.lexer.raw(), to_u32(line)?, &mut raw) };
        status(code, 0, self.lexer.memory_limit())?;
        Ok(self.line_view(&raw))
    }

    fn views(&self, lines: Range<usize>, raws: &mut Vec<RawLine>) -> Result<(), LexError> {
        raws.clear();
        raws.resize(lines.len(), RawLine::empty());
        let code = unsafe {
            ffi::paneflow_hl_read_lines(
                self.lexer.raw(),
                to_u32(lines.start)?,
                to_u32(lines.len())?,
                raws.as_mut_ptr(),
            )
        };
        status(code, 0, self.lexer.memory_limit())
    }

    fn line_view(&self, raw: &RawLine) -> LineView<'_> {
        let wide = raw.flags & ffi::WIDE_RECORDS_FLAG != 0;
        let words = to_usize(raw.record_count) * if wide { 2 } else { 1 };
        let text = if raw.byte_len == 0 || raw.text.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(raw.text, to_usize(raw.byte_len)) }
        };
        let records = if words == 0 || raw.records.is_null() {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(raw.records, words) }
        };
        LineView {
            text,
            utf16_len: to_usize(raw.utf16_len),
            terminator: to_usize(raw.flags & ffi::TERMINATOR_MASK),
            lone_cr: raw.flags & ffi::LONE_CR_FLAG != 0,
            records,
            wide,
        }
    }

    pub fn text(&self) -> Result<String, LexError> {
        let mut text = String::with_capacity(self.len);
        let mut raws = Vec::new();
        self.views(0..self.line_count(), &mut raws)?;
        for raw in &raws {
            let view = self.line_view(raw);
            text.push_str(std::str::from_utf8(view.text).map_err(|_| LexError::OutOfBounds)?);
            text.push_str(match (view.terminator, view.lone_cr) {
                (0, _) => "",
                (2, _) => "\r\n",
                (_, true) => "\r",
                _ => "\n",
            });
        }
        Ok(text)
    }

    pub fn line_tokens(&self, line: usize) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        self.line_tokens_into(line, &mut tokens)?;
        Ok(tokens)
    }

    pub fn line_tokens_into(&self, line: usize, out: &mut Vec<Token>) -> Result<(), LexError> {
        out.clear();
        if line >= self.line_count() {
            return Ok(());
        }
        let view = self.view(line)?;
        convert_tokens(&view, line_text(&view)?, out);
        Ok(())
    }

    pub fn for_each_line_tokens(
        &self,
        lines: Range<usize>,
        mut visit: impl FnMut(usize, &[Token]),
    ) -> Result<(), LexError> {
        self.for_each_line(lines, |line, _, tokens| visit(line, tokens))
    }

    pub fn for_each_line(
        &self,
        lines: Range<usize>,
        mut visit: impl FnMut(usize, &str, &[Token]),
    ) -> Result<(), LexError> {
        const BATCH: usize = 64;
        let end = lines.end.min(self.line_count());
        let mut raws = Vec::with_capacity(BATCH);
        let mut tokens = Vec::new();
        let mut first = lines.start.min(end);
        while first < end {
            let batch = first..(first + BATCH).min(end);
            self.views(batch.clone(), &mut raws)?;
            for (line, raw) in batch.zip(&raws) {
                let view = self.line_view(raw);
                let text = line_text(&view)?;
                tokens.clear();
                convert_tokens(&view, text, &mut tokens);
                visit(line, text, &tokens);
            }
            first += raws.len();
        }
        Ok(())
    }

    pub fn line_at(&self, offset: usize) -> usize {
        self.line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    fn position(&self, offset: usize) -> Result<(usize, usize, u32), LexError> {
        let line = self.line_at(offset);
        let column = offset - self.line_starts[line];
        let view = self.view(line)?;
        let units =
            view.text
                .get(..column)
                .and_then(utf16_units)
                .ok_or(LexError::InvalidRange {
                    start: offset,
                    end: offset,
                })?;
        Ok((line, column, to_u32(units)?))
    }

    pub fn apply_edit(&mut self, range: Range<usize>, text: &str) -> Result<(), LexError> {
        let invalid = LexError::InvalidRange {
            start: range.start,
            end: range.end,
        };
        if range.start > range.end || range.end > self.len {
            return Err(invalid);
        }
        if range.is_empty() && text.is_empty() {
            return Ok(());
        }
        let widen = |err: LexError| match err {
            LexError::InvalidRange { .. } => invalid.clone(),
            other => other,
        };
        let (start_line, _, start_column) = self.position(range.start).map_err(widen)?;
        let (end_line, _, end_column) = self.position(range.end).map_err(widen)?;
        let staged = 28 + text.len();
        let code = unsafe {
            ffi::paneflow_hl_edit(
                self.lexer.raw(),
                to_u32(start_line)?,
                start_column,
                to_u32(end_line)?,
                end_column,
                text.as_ptr(),
                to_u32(text.len())?,
            )
        };
        status(code, staged, self.lexer.memory_limit())?;
        self.len = self.len - (range.end - range.start) + text.len();
        self.remap_lines(start_line, end_line, range.end - range.start, text.len())
    }

    fn remap_lines(
        &mut self,
        start_line: usize,
        end_line: usize,
        removed: usize,
        inserted: usize,
    ) -> Result<(), LexError> {
        let old_count = self.line_starts.len();
        let new_count = self.raw_line_count()?;
        let first = start_line.saturating_sub(1);
        let old_end = (end_line + 2).min(old_count);
        let new_end = (old_end + new_count)
            .checked_sub(old_count)
            .filter(|end| *end >= first)
            .ok_or(LexError::OutOfBounds)?;
        let mut rebuilt = Vec::with_capacity(new_end - first);
        let mut at = self.line_starts[first];
        let mut raws = Vec::new();
        self.views(first..new_end, &mut raws)?;
        for raw in &raws {
            rebuilt.push(at);
            let view = self.line_view(raw);
            at += view.text.len() + view.terminator;
        }
        let tail: Vec<usize> = self.line_starts[old_end..]
            .iter()
            .map(|start| start + inserted - removed)
            .collect();
        if tail.first().is_some_and(|start| *start != at) {
            return Err(LexError::OutOfBounds);
        }
        self.line_starts.truncate(first);
        self.line_starts.extend(rebuilt);
        self.line_starts.extend(tail);
        Ok(())
    }

    pub fn changes(&self) -> Result<Vec<LineChange>, LexError> {
        let mut records = std::ptr::null();
        let mut count = 0;
        let code = unsafe { ffi::paneflow_hl_changes(self.lexer.raw(), &mut records, &mut count) };
        status(code, 0, self.lexer.memory_limit())?;
        if count == 0 || records.is_null() {
            return Ok(Vec::new());
        }
        let words =
            unsafe { std::slice::from_raw_parts(records, to_usize(count) * CHANGE_RECORD_WORDS) };
        Ok(words
            .as_chunks::<CHANGE_RECORD_WORDS>()
            .0
            .iter()
            .map(|change| LineChange {
                old: to_usize(change[0])..to_usize(change[1]),
                new: to_usize(change[2])..to_usize(change[3]),
            })
            .collect())
    }

    pub fn edit(&mut self, range: Range<usize>, text: &str) -> Result<EditOutcome, LexError> {
        self.apply_edit(range, text)?;
        self.settle()?;
        Ok(EditOutcome {
            changes: self.changes()?,
            line_count: self.line_count(),
        })
    }

    #[cfg(test)]
    pub(crate) fn probe_trap(&mut self) -> Result<(), LexError> {
        let code = unsafe { ffi::paneflow_hl_probe_trap(self.lexer.raw()) };
        status(code, 0, self.lexer.memory_limit())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::language_for_key;
    use crate::lexer::HOST_CALLS;
    use crate::MAX_MEMORY_BYTES;

    trait AmbiguousIfSync<A> {
        fn probe() {}
    }
    impl<T: ?Sized> AmbiguousIfSync<()> for T {}
    impl<T: ?Sized + Sync> AmbiguousIfSync<u8> for T {}

    const _: fn() = || {
        fn assert_send<T: Send>() {}
        assert_send::<LexDocument>();
        assert_send::<Lexer>();
        let _ = <LexDocument as AmbiguousIfSync<_>>::probe;
    };

    const RUST: &str = "fn main() {\n    let a = 1;\n    let b = 2;\n}\nfn other() {}\n";

    fn rust() -> Language {
        language_for_key("rs").expect("rust")
    }

    fn named(document: &LexDocument, line: usize) -> Vec<(String, &'static str)> {
        let start = document.line_start(line).expect("a line");
        let tokens = document.line_tokens(line).expect("tokens");
        let text = document_text(document);
        tokens
            .iter()
            .map(|token| {
                (
                    text[start + token.range.start..start + token.range.end].to_owned(),
                    token.id.name(),
                )
            })
            .collect()
    }

    fn document_text(document: &LexDocument) -> String {
        document.text().expect("the document text")
    }

    fn all_tokens(document: &LexDocument) -> Vec<Vec<Token>> {
        (0..document.line_count())
            .map(|line| document.line_tokens(line).expect("tokens"))
            .collect()
    }

    #[test]
    fn a_settled_document_returns_token_ids_with_byte_ranges_into_the_line() {
        let line = "let caf\u{e9} = \"\u{e9}\u{1f600}\";";
        let document = LexDocument::lex(line, rust()).expect("a document");
        let tokens = document.line_tokens(0).expect("tokens");
        assert_eq!(tokens.last().map(|token| token.range.end), Some(line.len()));
        assert!(line.len() > line.encode_utf16().count());
        for token in &tokens {
            assert!(
                line.is_char_boundary(token.range.start) && line.is_char_boundary(token.range.end)
            );
        }
        let names = named(&document, 0);
        assert!(
            names
                .iter()
                .any(|(text, name)| text.trim() == "caf\u{e9}" && *name == "variable"),
            "{names:?}"
        );
        assert!(
            names
                .iter()
                .any(|(text, name)| text.contains("\u{1f600}") && name.starts_with("string")),
            "{names:?}"
        );
        assert!(
            names[0].0.starts_with("let") && names[0].1.starts_with("keyword"),
            "{names:?}"
        );
    }

    #[test]
    fn an_edit_relexes_until_the_state_matches_and_reports_the_changed_lines() {
        let mut document = LexDocument::lex(RUST, rust()).expect("a document");
        let original = all_tokens(&document);
        let open = document.edit(16..16, "/* ").expect("an edit");
        assert_eq!(open.changed_lines(), Some(1..6));
        assert_eq!(open.line_count, 6);
        for line in 1..4 {
            assert!(
                named(&document, line)
                    .iter()
                    .any(|(_, name)| name.starts_with("comment")),
                "line {line}: {:?}",
                named(&document, line)
            );
        }
        let close = document.edit(16..19, "").expect("an edit");
        assert_eq!(close.changed_lines(), Some(1..6));
        assert_eq!(all_tokens(&document), original);
        let rename = document.edit(20..21, "z").expect("an edit");
        assert_eq!(rename.changed_lines(), Some(1..2));
        assert_eq!(document_text(&document), RUST.replacen("let a", "let z", 1));
    }

    #[test]
    fn a_batched_line_read_equals_the_per_line_reads() {
        let text = RUST.repeat(40);
        let document = LexDocument::lex(&text, rust()).expect("a document");
        assert!(document.line_count() > 130);
        let mut batched = Vec::new();
        document
            .for_each_line_tokens(3..document.line_count() + 10, |line, tokens| {
                batched.push((line, tokens.to_vec()));
            })
            .expect("batched tokens");
        let single: Vec<(usize, Vec<Token>)> = (3..document.line_count())
            .map(|line| (line, document.line_tokens(line).expect("tokens")))
            .collect();
        assert_eq!(batched, single);
        let mut none = 0;
        document
            .for_each_line_tokens(500..600, |_, _| none += 1)
            .expect("an empty range");
        assert_eq!(none, 0);
    }

    #[test]
    fn line_starts_break_after_lf_crlf_and_a_lone_cr_like_the_lexer() {
        for (text, starts) in [
            ("", vec![0]),
            ("a", vec![0]),
            ("a\nb", vec![0, 2]),
            ("a\r\nb", vec![0, 3]),
            ("a\rb", vec![0, 2]),
            ("\r\r\n\n\r", vec![0, 1, 3, 4, 5]),
            ("x\n\r\ny\r", vec![0, 2, 4, 6]),
        ] {
            assert_eq!(line_starts_of(text), starts, "{text:?}");
            let document = LexDocument::lex(text, rust()).expect("a document");
            assert_eq!(document.line_count(), starts.len(), "{text:?}");
        }
    }

    #[test]
    fn a_bounded_run_stops_before_its_end_line_and_reports_where_work_resumes() {
        let text: String = (0..20)
            .map(|line| format!("let a{line} = {line};\n"))
            .collect();
        let mut document = LexDocument::lex(&text, rust()).expect("a document");
        assert_eq!(document.next_pending_line().expect("stats"), None);
        let at = text.find("let a3").expect("line 3");
        document.apply_edit(at..at, "/*").expect("an edit");
        assert_eq!(document.next_pending_line().expect("stats"), Some(3));
        document.run_before(u32::MAX >> 1, 9).expect("a run");
        assert_eq!(
            document.next_pending_line().expect("stats"),
            Some(9),
            "the scan stops before line 9 although the comment runs on"
        );
        document.run_before(2, 20).expect("a run");
        assert_eq!(document.next_pending_line().expect("stats"), Some(11));
        document.run_before(u32::MAX >> 1, 0).expect("an empty run");
        assert_eq!(document.next_pending_line().expect("stats"), Some(11));
        document.settle().expect("settle");
        assert_eq!(document.next_pending_line().expect("stats"), None);
        let fresh = LexDocument::lex(&document.text().expect("text"), rust()).expect("fresh");
        for line in 0..document.line_count() {
            assert_eq!(
                document.line_tokens(line).expect("tokens"),
                fresh.line_tokens(line).expect("tokens"),
                "line {line}"
            );
        }
    }

    #[test]
    fn each_line_is_visited_with_its_text_and_any_offset_maps_to_its_line() {
        let text = "fn a() {}\r\nlet b = 1;\rlet c;\n";
        let document = LexDocument::lex(text, rust()).expect("a document");
        let mut seen = Vec::new();
        document
            .for_each_line(0..document.line_count(), |line, line_text, tokens| {
                assert_eq!(
                    tokens.last().map_or(0, |token| token.range.end),
                    line_text.len()
                );
                seen.push((line, line_text.to_owned()));
            })
            .expect("the lines");
        assert_eq!(
            seen,
            vec![
                (0, "fn a() {}".to_owned()),
                (1, "let b = 1;".to_owned()),
                (2, "let c;".to_owned()),
                (3, String::new()),
            ]
        );
        assert_eq!(document.line_at(0), 0);
        assert_eq!(document.line_at(10), 0);
        assert_eq!(document.line_at(11), 1);
        assert_eq!(document.line_at(22), 2);
        assert_eq!(document.line_at(text.len()), 3);
        assert_eq!(document.line_at(text.len() + 5), 3);
    }

    #[test]
    fn a_line_budget_settles_the_document_one_slice_at_a_time() {
        let mut document = Lexer::new()
            .expect("a lexer")
            .document(RUST, rust())
            .expect("a document");
        let mut slices = 0;
        while document.pending_lines().expect("stats") > 0 {
            document.run(1).expect("a slice");
            slices += 1;
            assert!(
                slices <= document.line_count(),
                "the document never settled"
            );
        }
        assert!(slices > 1);
        assert_eq!(
            all_tokens(&document),
            all_tokens(&LexDocument::lex(RUST, rust()).expect("a document"))
        );
    }

    #[test]
    fn edits_keep_lines_and_tokens_equal_to_a_fresh_lex_of_the_same_text() {
        let pieces = [
            "",
            "\n",
            "\r",
            "\r\n",
            "/*",
            "*/",
            "\"",
            "let \u{e9} = 1;",
            "fn x() {",
            "}",
            "\u{1f600}",
        ];
        let mut text = String::from("fn main() {\r\n    let a = \"x\";\r}\nfn b() {}\n");
        let mut document = LexDocument::lex(&text, rust()).expect("a document");
        let mut seed: u64 = 0x5eed_1234;
        let mut next = |bound: usize| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            usize::try_from(seed >> 33).unwrap_or(0) % bound.max(1)
        };
        for step in 0..400 {
            let boundaries: Vec<usize> = (0..=text.len())
                .filter(|at| text.is_char_boundary(*at))
                .collect();
            let a = boundaries[next(boundaries.len())];
            let b = boundaries[next(boundaries.len())];
            let range = a.min(b)..a.max(b).min(a.min(b) + 8);
            let range = range.start
                ..boundaries
                    .iter()
                    .copied()
                    .filter(|at| *at <= range.end)
                    .max()
                    .unwrap_or(range.start);
            let insert = pieces[next(pieces.len())];
            let splits_crlf = |at: usize| {
                at > 0
                    && text.as_bytes().get(at - 1) == Some(&b'\r')
                    && text.as_bytes().get(at) == Some(&b'\n')
            };
            match document.edit(range.clone(), insert) {
                Ok(_) => {
                    text.replace_range(range.clone(), insert);
                }
                Err(LexError::InvalidRange { .. }) => {
                    assert!(
                        splits_crlf(range.start) || splits_crlf(range.end),
                        "step {step}: {range:?} rejected"
                    );
                    continue;
                }
                Err(err) => {
                    assert_eq!(Err::<(), _>(err), Ok(()), "step {step}");
                    continue;
                }
            }
            let fresh = LexDocument::lex(&text, rust()).expect("a fresh document");
            assert_eq!(document.len(), text.len(), "step {step}");
            assert_eq!(document_text(&document), text, "step {step}");
            assert_eq!(
                document.line_starts, fresh.line_starts,
                "step {step}: {text:?}"
            );
            assert_eq!(
                all_tokens(&document),
                all_tokens(&fresh),
                "step {step}: {text:?}"
            );
        }
    }

    #[test]
    fn a_range_that_splits_a_character_or_passes_the_end_is_refused() {
        let mut document = LexDocument::lex("caf\u{e9}\r\nx", rust()).expect("a document");
        assert!(matches!(
            document.edit(4..4, "a"),
            Err(LexError::InvalidRange { .. })
        ));
        assert!(matches!(
            document.edit(6..6, "a"),
            Err(LexError::InvalidRange { .. })
        ));
        assert!(matches!(
            document.edit(3..99, ""),
            Err(LexError::InvalidRange { .. })
        ));
        assert!(document.edit(5..5, "!").is_ok());
    }

    #[test]
    fn two_documents_lexed_on_two_threads_equal_their_single_threaded_results() {
        let texts = [
            RUST.repeat(200),
            "const x = `a ${b} c`;\n// note\n".repeat(300),
        ];
        let languages = [rust(), language_for_key("ts").expect("ts")];
        let expected: Vec<Vec<Vec<Token>>> = texts
            .iter()
            .zip(languages)
            .map(|(text, language)| {
                all_tokens(&LexDocument::lex(text, language).expect("a document"))
            })
            .collect();
        for _ in 0..4 {
            let handles: Vec<_> = texts
                .iter()
                .cloned()
                .zip(languages)
                .map(|(text, language)| {
                    std::thread::spawn(move || {
                        let mut document = LexDocument::lex(&text, language).expect("a document");
                        document.edit(0..0, " ").expect("an edit");
                        document.edit(0..1, "").expect("an edit");
                        all_tokens(&document)
                    })
                })
                .collect();
            let results: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().expect("a thread"))
                .collect();
            assert_eq!(results, expected);
        }
    }

    #[test]
    fn a_unicode_identifier_is_classified_through_the_host_imports() {
        let typescript = language_for_key("ts").expect("ts");
        for (sample, identifier, expected) in [
            ("let \u{3b1}\u{3b2} = 1;", "\u{3b1}\u{3b2}", "variable"),
            ("let x\u{200d}y = 1;", "x\u{200d}y", "variable"),
            ("let \u{1f600} = 1;", "\u{1f600}", "none"),
        ] {
            let before = HOST_CALLS.with(std::cell::Cell::get);
            let document = LexDocument::lex(sample, typescript).expect("a document");
            assert!(
                HOST_CALLS.with(std::cell::Cell::get) > before,
                "{sample:?} never reached the host"
            );
            let names = named(&document, 0);
            assert!(
                names
                    .iter()
                    .any(|(text, name)| text.trim() == identifier && *name == expected),
                "{sample:?}: {names:?}"
            );
        }
    }

    #[test]
    fn a_trap_is_an_error_and_a_new_lexer_afterwards_matches_a_clean_run() {
        let clean = all_tokens(&LexDocument::lex(RUST, rust()).expect("a document"));
        let mut document = LexDocument::lex(RUST, rust()).expect("a document");
        assert_eq!(document.probe_trap(), Err(LexError::Trap));
        assert_eq!(document.line_tokens(0), Err(LexError::Poisoned));
        assert_eq!(
            document.edit(0..0, "x").map(|_| ()),
            Err(LexError::Poisoned)
        );
        drop(document);
        let after = all_tokens(&LexDocument::lex(RUST, rust()).expect("a document"));
        assert_eq!(after, clean);
    }

    #[test]
    fn staging_past_the_memory_ceiling_is_refused_without_growing_memory() {
        assert_eq!(MAX_MEMORY_BYTES, 8 * crate::MAX_FILE_BYTES);
        let mut document = LexDocument::lex("fn main() {}", rust()).expect("a document");
        let before = document.memory_bytes();
        let tokens = all_tokens(&document);
        let huge = " ".repeat(MAX_MEMORY_BYTES);
        assert!(matches!(
            document.edit(0..0, &huge),
            Err(LexError::MemoryLimit { limit, .. }) if limit == MAX_MEMORY_BYTES
        ));
        assert_eq!(document.memory_bytes(), before);
        assert_eq!(all_tokens(&document), tokens);
        assert_eq!(document.len(), "fn main() {}".len());
        let small = Lexer::with_memory_limit(2 * 1024 * 1024).expect("a lexer");
        let staged = small.document(&" ".repeat(3 * 1024 * 1024), rust());
        assert!(
            matches!(staged, Err(LexError::MemoryLimit { .. })),
            "{:?}",
            staged.err()
        );
    }

    #[test]
    fn lexing_that_would_grow_past_the_ceiling_fails_without_passing_it() {
        let limit = 4 * 1024 * 1024;
        let text = RUST.repeat(20_000);
        let lexer = Lexer::with_memory_limit(limit).expect("a lexer");
        let result = lexer.document(&text, rust()).and_then(|mut document| {
            let settled = document.settle();
            assert!(document.memory_bytes() <= limit);
            settled
        });
        assert!(
            result.is_err(),
            "a {} byte document lexed inside {limit} bytes",
            text.len()
        );
    }
}
