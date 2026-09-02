use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use super::cursor::CodeSelection;
use super::document::{CodeDocument, CodeEdit, normalize_newlines};

pub(crate) const UNDO_GROUP_INTERVAL: Duration = Duration::from_millis(300);

pub(crate) const MAX_UNDO_TRANSACTIONS: usize = 1000;

const INDENT_WIDTHS: Range<usize> = 2..9;

const INDENT_SCAN_LINES: usize = 5_000;

const DEFAULT_INDENT_SPACES: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppliedEdit {
    pub(crate) start: usize,
    pub(crate) removed: String,
    pub(crate) inserted: String,
}

impl AppliedEdit {
    fn inserted_range(&self) -> Range<usize> {
        self.start..self.start + self.inserted.len()
    }

    fn removed_range(&self) -> Range<usize> {
        self.start..self.start + self.removed.len()
    }
}

pub(crate) struct Splice {
    pub(crate) edits: Vec<CodeEdit>,
    pub(crate) record: AppliedEdit,
}

pub(crate) fn splice(doc: &mut CodeDocument, range: Range<usize>, text: &str) -> Option<Splice> {
    if doc.is_read_only() {
        return None;
    }
    let start = doc.snap_to_boundary(range.start);
    let end = doc.snap_to_boundary(range.end.max(range.start));
    let removed = doc.slice_string(start..end);
    let inserted = normalize_newlines(text).into_owned();
    if removed.is_empty() && inserted.is_empty() {
        return None;
    }
    let mut edits = Vec::with_capacity(2);
    if end > start
        && let Some(edit) = doc.remove(start..end)
    {
        edits.push(edit);
    }
    if !inserted.is_empty()
        && let Some(edit) = doc.insert(start, &inserted)
    {
        edits.push(edit);
    }
    Some(Splice {
        edits,
        record: AppliedEdit {
            start,
            removed,
            inserted,
        },
    })
}

fn apply_forward(doc: &mut CodeDocument, record: &AppliedEdit) -> Vec<CodeEdit> {
    raw_splice(doc, record.removed_range(), &record.inserted)
}

fn apply_reverse(doc: &mut CodeDocument, record: &AppliedEdit) -> Vec<CodeEdit> {
    raw_splice(doc, record.inserted_range(), &record.removed)
}

fn raw_splice(doc: &mut CodeDocument, range: Range<usize>, text: &str) -> Vec<CodeEdit> {
    let mut edits = Vec::with_capacity(2);
    if range.end > range.start
        && let Some(edit) = doc.remove(range.clone())
    {
        edits.push(edit);
    }
    if !text.is_empty()
        && let Some(edit) = doc.insert(range.start, text)
    {
        edits.push(edit);
    }
    edits
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditGroup {
    Typing,
    Atomic,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum HistoryMark {
    #[default]
    Baseline,
    Transaction(u64),
}

#[derive(Clone, Debug)]
struct Transaction {
    id: u64,
    edits: Vec<AppliedEdit>,
    before: CodeSelection,
    after: CodeSelection,
}

pub(crate) struct HistoryStep {
    pub(crate) edits: Vec<CodeEdit>,
    pub(crate) selection: CodeSelection,
}

#[derive(Default)]
pub(crate) struct UndoHistory {
    undo: VecDeque<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    open: bool,
    last_edit_at: Option<Instant>,
}

impl UndoHistory {
    pub(crate) fn push(
        &mut self,
        edits: Vec<AppliedEdit>,
        before: CodeSelection,
        after: CodeSelection,
        group: EditGroup,
        now: Instant,
    ) {
        if edits.is_empty() {
            return;
        }
        self.redo.clear();

        let joinable = group == EditGroup::Typing
            && self.open
            && self
                .last_edit_at
                .is_some_and(|last| now.saturating_duration_since(last) <= UNDO_GROUP_INTERVAL);
        if joinable && let Some(top) = self.undo.back_mut() {
            top.edits.extend(edits);
            top.after = after;
            self.last_edit_at = Some(now);
            return;
        }

        let id = self.next_id;
        self.next_id += 1;
        self.undo.push_back(Transaction {
            id,
            edits,
            before,
            after,
        });
        if self.undo.len() > MAX_UNDO_TRANSACTIONS {
            self.undo.pop_front();
        }
        self.open = group == EditGroup::Typing;
        self.last_edit_at = Some(now);
    }

    pub(crate) fn close_group(&mut self) {
        self.open = false;
    }

    pub(crate) fn undo(&mut self, doc: &mut CodeDocument) -> Option<HistoryStep> {
        self.open = false;
        let transaction = self.undo.pop_back()?;
        let mut edits = Vec::new();
        for record in transaction.edits.iter().rev() {
            edits.extend(apply_reverse(doc, record));
        }
        let selection = transaction.before;
        self.redo.push(transaction);
        Some(HistoryStep { edits, selection })
    }

    pub(crate) fn redo(&mut self, doc: &mut CodeDocument) -> Option<HistoryStep> {
        self.open = false;
        let transaction = self.redo.pop()?;
        let mut edits = Vec::new();
        for record in &transaction.edits {
            edits.extend(apply_forward(doc, record));
        }
        let selection = transaction.after;
        self.undo.push_back(transaction);
        Some(HistoryStep { edits, selection })
    }

    pub(crate) fn mark(&self) -> HistoryMark {
        match self.undo.back() {
            Some(transaction) => HistoryMark::Transaction(transaction.id),
            None => HistoryMark::Baseline,
        }
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.open = false;
        self.last_edit_at = None;
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.undo.len()
    }

    #[cfg(test)]
    fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IndentUnit {
    Tab,
    Spaces(usize),
}

impl IndentUnit {
    pub(crate) fn as_str(self) -> Cow<'static, str> {
        match self {
            Self::Tab => Cow::Borrowed("\t"),
            Self::Spaces(n) => Cow::Owned(" ".repeat(n)),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn width(self) -> usize {
        match self {
            Self::Tab => 1,
            Self::Spaces(n) => n.max(1),
        }
    }

    pub(crate) fn detect(doc: &CodeDocument) -> Self {
        let rows = doc.line_count().min(INDENT_SCAN_LINES);
        let mut tabs = 0usize;
        let mut spaces = 0usize;
        let mut steps = [0usize; INDENT_WIDTHS.end];
        let mut previous: Option<usize> = None;

        for row in 0..rows {
            let Some(line) = doc.line_string(row) else {
                continue;
            };
            let indent = leading_indent(&line);
            if indent.len() == line.trim_end_matches('\n').len() {
                continue;
            }
            if indent.starts_with('\t') {
                tabs += 1;
                previous = None;
                continue;
            }
            let width = indent.len();
            if width > 0 {
                spaces += 1;
            }
            if let Some(previous) = previous {
                let step = width.abs_diff(previous);
                if INDENT_WIDTHS.contains(&step) {
                    steps[step] += 1;
                }
            }
            previous = Some(width);
        }

        if tabs > spaces && tabs > 0 {
            return Self::Tab;
        }
        let best = INDENT_WIDTHS
            .rev()
            .max_by_key(|width| steps[*width])
            .filter(|width| steps[*width] > 0);
        match best {
            Some(width) => Self::Spaces(width),
            None => Self::Spaces(DEFAULT_INDENT_SPACES),
        }
    }
}

pub(crate) fn leading_indent(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

pub(crate) fn dedent_width(line: &str, unit: IndentUnit) -> usize {
    let indent = leading_indent(line);
    if indent.is_empty() {
        return 0;
    }
    match unit {
        IndentUnit::Tab => {
            if indent.starts_with('\t') {
                1
            } else {
                indent.len().min(DEFAULT_INDENT_SPACES)
            }
        }
        IndentUnit::Spaces(n) => {
            let n = n.max(1);
            let mut removed = 0;
            for byte in indent.bytes().take(n) {
                if byte == b'\t' {
                    if removed == 0 {
                        removed = 1;
                    }
                    break;
                }
                removed += 1;
            }
            removed
        }
    }
}

pub(crate) fn sanitize_paste(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push('\n');
            }
            '\n' | '\t' => out.push(c),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    crate::markdown::strip_bidi_zero_width(out)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn doc(text: &str) -> CodeDocument {
        CodeDocument::new(PathBuf::from("/nonexistent/edit.rs"), text)
    }

    #[test]
    fn a_splice_reverses_exactly() {
        let mut d = doc("hello world");
        let record = splice(&mut d, 6..11, "there")
            .expect("splice applies")
            .record;
        assert_eq!(d.text().to_string(), "hello there");
        assert_eq!(record.removed, "world");
        assert_eq!(record.inserted, "there");

        apply_reverse(&mut d, &record);
        assert_eq!(d.text().to_string(), "hello world");
        apply_forward(&mut d, &record);
        assert_eq!(d.text().to_string(), "hello there");
    }

    #[test]
    fn a_read_only_document_refuses_a_splice() {
        let mut d = doc("locked");
        d.set_read_only(Some(super::super::document::ReadOnlyReason::Permissions));
        assert!(splice(&mut d, 0..6, "nope").is_none());
        assert_eq!(d.text().to_string(), "locked");
    }

    #[test]
    fn keystrokes_group_until_the_interval_lapses() {
        let mut d = doc("");
        let mut history = UndoHistory::default();
        let t0 = Instant::now();
        let sel = CodeSelection::at(0);

        for (i, ch) in ["a", "b", "c"].iter().enumerate() {
            let record = splice(&mut d, i..i, ch).expect("insert").record;
            history.push(
                vec![record],
                sel,
                sel,
                EditGroup::Typing,
                t0 + Duration::from_millis(i as u64 * 50),
            );
        }
        assert_eq!(history.len(), 1, "three keystrokes, one transaction");

        let record = splice(&mut d, 3..3, "d").expect("insert").record;
        history.push(
            vec![record],
            sel,
            sel,
            EditGroup::Typing,
            t0 + UNDO_GROUP_INTERVAL + Duration::from_millis(400),
        );
        assert_eq!(history.len(), 2, "the pause closed the group");

        let record = splice(&mut d, 4..4, "XY").expect("insert").record;
        history.push(
            vec![record],
            sel,
            sel,
            EditGroup::Atomic,
            t0 + UNDO_GROUP_INTERVAL + Duration::from_millis(410),
        );
        assert_eq!(history.len(), 3, "an atomic edit never joins");
        assert_eq!(d.text().to_string(), "abcdXY");

        history.undo(&mut d);
        assert_eq!(d.text().to_string(), "abcd");
        history.undo(&mut d);
        assert_eq!(d.text().to_string(), "abc");
        history.undo(&mut d);
        assert_eq!(d.text().to_string(), "", "the whole group came back out");
    }

    #[test]
    fn undo_restores_the_caret_and_a_new_edit_drops_the_redo_branch() {
        let mut d = doc("one\ntwo");
        let mut history = UndoHistory::default();
        let before = CodeSelection::at(3);
        let record = splice(&mut d, 3..3, "\nalpha\nbeta").expect("paste").record;
        let after = CodeSelection::at(record.start + record.inserted.len());
        history.push(
            vec![record],
            before,
            after,
            EditGroup::Atomic,
            Instant::now(),
        );
        assert_eq!(d.text().to_string(), "one\nalpha\nbeta\ntwo");

        let step = history.undo(&mut d).expect("one transaction");
        assert_eq!(d.text().to_string(), "one\ntwo", "one undo, whole paste");
        assert_eq!(step.selection, before, "the caret came back too");
        assert_eq!(history.redo_len(), 1);

        let step = history.redo(&mut d).expect("redoable");
        assert_eq!(d.text().to_string(), "one\nalpha\nbeta\ntwo");
        assert_eq!(step.selection, after);

        history.undo(&mut d);
        let record = splice(&mut d, 0..0, "x").expect("insert").record;
        history.push(
            vec![record],
            CodeSelection::at(0),
            CodeSelection::at(1),
            EditGroup::Typing,
            Instant::now(),
        );
        assert_eq!(history.redo_len(), 0, "a new edit clears the redo branch");
    }

    #[test]
    fn the_history_is_capped_and_the_mark_tracks_the_saved_state() {
        let mut d = doc("");
        let mut history = UndoHistory::default();
        let sel = CodeSelection::at(0);
        assert_eq!(history.mark(), HistoryMark::Baseline);

        for i in 0..(MAX_UNDO_TRANSACTIONS + 25) {
            let record = splice(&mut d, i..i, "z").expect("insert").record;
            history.push(vec![record], sel, sel, EditGroup::Atomic, Instant::now());
        }
        assert_eq!(history.len(), MAX_UNDO_TRANSACTIONS);

        let saved = history.mark();
        let record = splice(&mut d, 0..0, "!").expect("insert").record;
        history.push(vec![record], sel, sel, EditGroup::Typing, Instant::now());
        assert_ne!(history.mark(), saved, "an edit past the save is dirty");
        history.undo(&mut d);
        assert_eq!(history.mark(), saved, "undoing back to it is clean again");
    }

    #[test]
    fn the_indent_unit_comes_from_the_file() {
        assert_eq!(
            IndentUnit::detect(&doc("fn main() {\n\tlet a = 1;\n\tlet b = 2;\n}\n")),
            IndentUnit::Tab
        );
        assert_eq!(
            IndentUnit::detect(&doc("a:\n  b:\n    c: 1\n  d: 2\n")),
            IndentUnit::Spaces(2)
        );
        assert_eq!(
            IndentUnit::detect(&doc("fn f() {\n    if x {\n        y();\n    }\n}\n")),
            IndentUnit::Spaces(4)
        );
        assert_eq!(
            IndentUnit::detect(&doc("one line, no indent\n")),
            IndentUnit::Spaces(DEFAULT_INDENT_SPACES)
        );
        assert_eq!(
            IndentUnit::detect(&doc("")),
            IndentUnit::Spaces(DEFAULT_INDENT_SPACES)
        );
    }

    #[test]
    fn dedent_never_eats_a_real_character() {
        assert_eq!(dedent_width("        deep", IndentUnit::Spaces(4)), 4);
        assert_eq!(dedent_width("  shallow", IndentUnit::Spaces(4)), 2);
        assert_eq!(dedent_width("flush", IndentUnit::Spaces(4)), 0);
        assert_eq!(dedent_width("\ttabbed", IndentUnit::Tab), 1);
        assert_eq!(dedent_width("nope", IndentUnit::Tab), 0);
        assert_eq!(dedent_width("\tmixed", IndentUnit::Spaces(4)), 1);
    }

    #[test]
    fn a_paste_is_sanitized() {
        assert_eq!(sanitize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(sanitize_paste("keep\there"), "keep\there");
        assert_eq!(sanitize_paste("bell\u{7}esc\u{1b}"), "bellesc");
        assert_eq!(sanitize_paste("safe\u{202e}evil\u{200b}"), "safeevil");
    }
}
