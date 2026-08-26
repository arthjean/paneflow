//! Editing primitives for the file editor: splices, the undo history, the
//! indent unit, and the paste sanitizer.
//!
//! prd-file-editor-2026-Q3, EP-004 (US-012 through US-014). Nothing here knows
//! about GPUI: [`super::view::CodeView`] turns an action or an IME callback
//! into one call on this module, then routes the returned [`CodeEdit`]s into
//! the incremental highlighter. That is the same split `super::cursor` already
//! uses for motion, and it is what makes the grouping rule, the 1000-entry cap
//! and the dedent-never-eats-text invariant provable without a window.
//!
//! ## Why records, not snapshots
//!
//! A transaction stores the bytes a splice removed and the bytes it inserted,
//! not a copy of the document. Undo replays the inverse splice, which keeps the
//! history proportional to what the user typed rather than to the file. It also
//! produces a real [`CodeEdit`] per step, which is the part US-013 actually
//! needs: `Tree::edit` sees an undo exactly the way it saw the original edit.
//!
//! ## Grouping
//!
//! Zed's `text.rs:229` ships a 300 ms `group_interval`, and `text.rs:294`
//! merges a transaction into the previous one when the gap between them is
//! inside it. [`UNDO_GROUP_INTERVAL`] is the same number and
//! [`UndoHistory::push`] is the same rule, narrowed to keystrokes: a paste, an
//! indent or a reload is always its own transaction, and a caret move closes
//! whatever group was open.

use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use super::cursor::CodeSelection;
use super::document::{CodeDocument, CodeEdit, normalize_newlines};

/// How long a group of keystrokes stays open. Zed's production
/// `group_interval` (`zed/crates/text/src/text.rs:229`).
pub(crate) const UNDO_GROUP_INTERVAL: Duration = Duration::from_millis(300);

/// Transactions kept before the oldest is dropped (US-013).
pub(crate) const MAX_UNDO_TRANSACTIONS: usize = 1000;

/// Indent widths the detector will accept. Anything outside this is noise
/// (a wrapped argument list, an ASCII-art comment) rather than a unit.
const INDENT_WIDTHS: Range<usize> = 2..9;

/// Lines the indent detector reads before it settles. A file whose first few
/// thousand lines are ambiguous is not going to get clearer further down.
const INDENT_SCAN_LINES: usize = 5_000;

/// Fallback indent when the file offers no evidence: empty, all-blank, or a
/// single unindented line (PRD open question 1). Four spaces, no setting.
const DEFAULT_INDENT_SPACES: usize = 4;

/// One applied splice, stored with enough text to be replayed in either
/// direction. `start` is the byte offset the splice began at *in the document
/// state that preceded it*, which is why a transaction's records are replayed
/// forward in order and backward in reverse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AppliedEdit {
    pub(crate) start: usize,
    pub(crate) removed: String,
    pub(crate) inserted: String,
}

impl AppliedEdit {
    /// Byte range the inserted text occupies once the splice is applied.
    fn inserted_range(&self) -> Range<usize> {
        self.start..self.start + self.inserted.len()
    }

    /// Byte range the removed text occupied before the splice.
    fn removed_range(&self) -> Range<usize> {
        self.start..self.start + self.removed.len()
    }
}

/// What [`splice`] produced: the document edits to feed the highlighter, and
/// the record to hand the history.
pub(crate) struct Splice {
    pub(crate) edits: Vec<CodeEdit>,
    pub(crate) record: AppliedEdit,
}

/// Replace `range` with `text`.
///
/// Returns `None` when the document is read-only or when the splice would be a
/// no-op (empty range, empty replacement) - in both cases nothing was written
/// and nothing should reach the history. Line endings in `text` are normalized
/// to the rope's LF form first, so [`AppliedEdit::inserted`] is what the
/// document really holds and the inverse splice is exact.
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

/// Replay `record` in the direction it was originally applied.
fn apply_forward(doc: &mut CodeDocument, record: &AppliedEdit) -> Vec<CodeEdit> {
    raw_splice(doc, record.removed_range(), &record.inserted)
}

/// Replay `record` backwards: put the removed bytes back where the inserted
/// ones are.
fn apply_reverse(doc: &mut CodeDocument, record: &AppliedEdit) -> Vec<CodeEdit> {
    raw_splice(doc, record.inserted_range(), &record.removed)
}

/// The splice a replay performs: no record, no normalization (both texts came
/// out of the rope and are already LF), no read-only check - a history replay
/// cannot be the thing that discovers the document turned read-only, because
/// the document it is replaying into is the one it edited.
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

/// How a new edit relates to the group before it (US-013).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum EditGroup {
    /// A keystroke. Joins the open group when it lands inside
    /// [`UNDO_GROUP_INTERVAL`], and leaves the group open behind it.
    Typing,
    /// A paste, an indent, a reload. Always its own transaction, and it closes
    /// the group behind it so the next keystroke starts a fresh one.
    Atomic,
}

/// Where the history currently stands. Comparing this against the mark the
/// last save left behind is what makes "undo back to the saved state clears
/// the dirty dot" (US-013 / US-015) true without diffing the file.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum HistoryMark {
    /// Nothing on the undo stack: the document is exactly what was loaded.
    #[default]
    Baseline,
    /// The identified transaction is the newest applied one.
    Transaction(u64),
}

/// One undoable step: the records it applied, plus the selection to restore on
/// either side of it (US-013 AC: undo restores text *and* caret).
#[derive(Clone, Debug)]
struct Transaction {
    id: u64,
    edits: Vec<AppliedEdit>,
    before: CodeSelection,
    after: CodeSelection,
}

/// What an undo or a redo produced, for the caller to route onward.
pub(crate) struct HistoryStep {
    /// Document edits, in the order they were applied, to feed `Tree::edit`.
    pub(crate) edits: Vec<CodeEdit>,
    /// Selection to restore.
    pub(crate) selection: CodeSelection,
}

/// Bounded undo / redo stack (US-013).
#[derive(Default)]
pub(crate) struct UndoHistory {
    undo: VecDeque<Transaction>,
    redo: Vec<Transaction>,
    next_id: u64,
    /// Whether the newest undo entry still accepts more keystrokes.
    open: bool,
    /// When the newest entry last grew.
    last_edit_at: Option<Instant>,
}

impl UndoHistory {
    /// Record `edits` as one step. `now` is injected so the grouping rule is
    /// testable without sleeping.
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
        // Any new edit abandons the redo branch: the future it described no
        // longer starts from this document.
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

    /// Close the open group. A caret move, a save, a paste and an undo all do
    /// this, which is what stops a keystroke after any of them from being
    /// folded into what came before.
    pub(crate) fn close_group(&mut self) {
        self.open = false;
    }

    /// Undo the newest transaction, applying its inverse to `doc`.
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

    /// Redo the newest undone transaction.
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

    /// Where the history stands right now.
    pub(crate) fn mark(&self) -> HistoryMark {
        match self.undo.back() {
            Some(transaction) => HistoryMark::Transaction(transaction.id),
            None => HistoryMark::Baseline,
        }
    }

    /// Drop everything. Used when the view is pointed at another file.
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

/// The indentation one Tab inserts (US-014).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum IndentUnit {
    Tab,
    Spaces(usize),
}

impl IndentUnit {
    /// The text one indent level inserts.
    pub(crate) fn as_str(self) -> Cow<'static, str> {
        match self {
            Self::Tab => Cow::Borrowed("\t"),
            Self::Spaces(n) => Cow::Owned(" ".repeat(n)),
        }
    }

    /// Columns one level is worth, for dedent.
    #[allow(dead_code)] // EP-004 accessor: indentation is applied through `IndentUnit::text`; the column width has no caller yet.
    pub(crate) fn width(self) -> usize {
        match self {
            Self::Tab => 1,
            Self::Spaces(n) => n.max(1),
        }
    }

    /// Detect the file's own indentation.
    ///
    /// Tabs win outright when more indented lines start with one, which is the
    /// only signal a tab-indented file gives. Otherwise the unit is the most
    /// frequent step between the indent widths of consecutive indented lines,
    /// which is what distinguishes a 2-space file from a 4-space file whose
    /// first block happens to be nested twice. With no evidence at all, the
    /// answer is [`DEFAULT_INDENT_SPACES`].
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
                continue; // blank or whitespace-only: carries no structure
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
        // `max_by_key` keeps the last maximum; iterate in reverse so a tie
        // resolves to the narrower unit, which is the safer guess.
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

/// The leading run of spaces and tabs in `line`.
pub(crate) fn leading_indent(line: &str) -> &str {
    let end = line
        .find(|c: char| c != ' ' && c != '\t')
        .unwrap_or(line.len());
    &line[..end]
}

/// Bytes [`dedent`] should remove from the front of `line`: at most one indent
/// level, and never past the first non-whitespace character.
///
/// This is the invariant US-014 states outright - Shift+Tab must not be able to
/// delete a real character - so it is enforced here, on the text, rather than
/// in the caller's arithmetic.
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
                    // A tab inside a space-indented file is one level on its
                    // own: stop rather than mixing the two.
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

/// Clean text arriving from the clipboard (US-014).
///
/// Three passes, in order: every line-break convention becomes `\n` (a lone
/// `\r` from a classic-Mac source would otherwise be dropped outright by the
/// rope's normalizer and silently join two lines); control characters other
/// than tab and newline are removed; and the bidi and zero-width set goes
/// through the markdown viewer's [`crate::markdown::strip_bidi_zero_width`],
/// so a pasted `U+202E` cannot make the file read differently from what it
/// contains.
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

    /// US-012: a splice removes and inserts in one step, and the record it
    /// leaves reverses exactly.
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

    /// US-012 AC: a read-only document mutates nothing.
    #[test]
    fn a_read_only_document_refuses_a_splice() {
        let mut d = doc("locked");
        d.set_read_only(Some(super::super::document::ReadOnlyReason::Permissions));
        assert!(splice(&mut d, 0..6, "nope").is_none());
        assert_eq!(d.text().to_string(), "locked");
    }

    /// US-013 AC: consecutive keystrokes group; a 300 ms pause closes the
    /// group; an atomic edit is always alone.
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

    /// US-013 AC: undo of a multi-line paste is one step, undo restores the
    /// caret, and a new edit clears the redo branch.
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

    /// US-013 AC: the history is capped, and undoing back to the mark a save
    /// left behind reports the document clean again.
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

    /// US-014 AC: the unit comes from the file, and an ambiguous file falls
    /// back to four spaces.
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

    /// US-014 AC: dedent never removes a character that is not indentation.
    #[test]
    fn dedent_never_eats_a_real_character() {
        assert_eq!(dedent_width("        deep", IndentUnit::Spaces(4)), 4);
        assert_eq!(dedent_width("  shallow", IndentUnit::Spaces(4)), 2);
        assert_eq!(dedent_width("flush", IndentUnit::Spaces(4)), 0);
        assert_eq!(dedent_width("\ttabbed", IndentUnit::Tab), 1);
        assert_eq!(dedent_width("nope", IndentUnit::Tab), 0);
        assert_eq!(dedent_width("\tmixed", IndentUnit::Spaces(4)), 1);
    }

    /// US-014 AC: pasted control characters and bidi markers are neutralized,
    /// and every line-break convention survives as a real break.
    #[test]
    fn a_paste_is_sanitized() {
        assert_eq!(sanitize_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(sanitize_paste("keep\there"), "keep\there");
        assert_eq!(sanitize_paste("bell\u{7}esc\u{1b}"), "bellesc");
        assert_eq!(sanitize_paste("safe\u{202e}evil\u{200b}"), "safeevil");
    }
}
