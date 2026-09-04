use std::borrow::Cow;
use std::collections::VecDeque;
use std::ops::Range;
use std::time::{Duration, Instant};

use imara_diff::intern::InternedInput;
use imara_diff::sources::lines_with_terminator;
use imara_diff::{Algorithm, Sink};
use ropey::Rope;

use super::cursor::CodeSelection;
use super::document::{CodeDocument, CodeEdit, normalize_newlines};

pub(crate) const UNDO_GROUP_INTERVAL: Duration = Duration::from_millis(300);

pub(crate) const MAX_UNDO_TRANSACTIONS: usize = 1000;

pub(crate) const MAX_UNDO_BYTES: usize = 32 * 1024 * 1024;

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
    pub(crate) edit: CodeEdit,
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
    let removal = (end > start).then(|| doc.remove(start..end)).flatten();
    let insertion = (!inserted.is_empty())
        .then(|| doc.insert(start, &inserted))
        .flatten();
    let edit = replacement_edit(removal, insertion)?;
    Some(Splice {
        edit,
        record: AppliedEdit {
            start,
            removed,
            inserted,
        },
    })
}

fn replacement_edit(removal: Option<CodeEdit>, insertion: Option<CodeEdit>) -> Option<CodeEdit> {
    match (removal, insertion) {
        (Some(removal), Some(insertion)) => Some(CodeEdit {
            start_byte: removal.start_byte,
            old_end_byte: removal.old_end_byte,
            new_end_byte: insertion.new_end_byte,
            start_point: removal.start_point,
            old_end_point: removal.old_end_point,
            new_end_point: insertion.new_end_point,
        }),
        (Some(edit), None) | (None, Some(edit)) => Some(edit),
        (None, None) => None,
    }
}

#[derive(Default)]
struct DiskHunkCollector {
    hunks: Vec<(Range<u32>, Range<u32>)>,
}

impl Sink for DiskHunkCollector {
    type Out = Vec<(Range<u32>, Range<u32>)>;

    fn process_change(&mut self, before: Range<u32>, after: Range<u32>) {
        self.hunks.push((before, after));
    }

    fn finish(self) -> Self::Out {
        self.hunks
    }
}

pub(crate) fn disk_splices(current: &Rope, incoming: &str) -> Vec<(Range<usize>, String)> {
    let incoming = normalize_newlines(incoming);
    let incoming_lines: Vec<&str> = lines_with_terminator(&incoming).collect();
    let current_lines = terminated_line_count(current);

    let mut prefix = 0usize;
    while prefix < current_lines
        && prefix < incoming_lines.len()
        && current.line(prefix) == incoming_lines[prefix]
    {
        prefix += 1;
    }

    let mut suffix = 0usize;
    while suffix < current_lines.saturating_sub(prefix)
        && suffix < incoming_lines.len().saturating_sub(prefix)
        && current.line(current_lines - suffix - 1)
            == incoming_lines[incoming_lines.len() - suffix - 1]
    {
        suffix += 1;
    }

    let current_start = current.line_to_byte(prefix);
    let current_end = current.line_to_byte(current_lines - suffix);
    let incoming_start = incoming_lines[..prefix]
        .iter()
        .map(|line| line.len())
        .sum::<usize>();
    let incoming_end = incoming.len()
        - incoming_lines[incoming_lines.len() - suffix..]
            .iter()
            .map(|line| line.len())
            .sum::<usize>();
    if current_start == current_end && incoming_start == incoming_end {
        return Vec::new();
    }
    let current_middle = current
        .byte_slice(current_start..current_end)
        .chunks()
        .collect::<String>();
    let current_middle = current_middle.as_str();
    let incoming_middle = &incoming[incoming_start..incoming_end];
    let current_offsets = line_offsets(current_middle);
    let incoming_offsets = line_offsets(incoming_middle);
    let input = InternedInput::new(
        lines_with_terminator(current_middle),
        lines_with_terminator(incoming_middle),
    );
    let hunks = imara_diff::diff(Algorithm::Histogram, &input, DiskHunkCollector::default());
    let mut splices = Vec::with_capacity(hunks.len());
    for (before, after) in hunks {
        let before_start = current_start + current_offsets[before.start as usize];
        let before_end = current_start + current_offsets[before.end as usize];
        let after_start = incoming_offsets[after.start as usize];
        let after_end = incoming_offsets[after.end as usize];
        splices.push((
            before_start..before_end,
            incoming_middle[after_start..after_end].to_string(),
        ));
    }
    splices.reverse();
    splices
}

pub(crate) fn shift_selection_for_splices(
    selection: CodeSelection,
    splices_descending: &[(Range<usize>, String)],
) -> CodeSelection {
    CodeSelection {
        anchor: shift_offset_for_splices(selection.anchor, splices_descending),
        head: shift_offset_for_splices(selection.head, splices_descending),
    }
}

fn terminated_line_count(text: &Rope) -> usize {
    let lines = text.len_lines();
    match lines.checked_sub(1) {
        Some(last) if text.line(last).len_bytes() == 0 => last,
        _ => lines,
    }
}

fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = Vec::new();
    offsets.push(0);
    let mut total = 0usize;
    for line in lines_with_terminator(text) {
        total += line.len();
        offsets.push(total);
    }
    offsets
}

fn shift_offset_for_splices(offset: usize, splices_descending: &[(Range<usize>, String)]) -> usize {
    let mut delta = 0isize;
    for (range, inserted) in splices_descending.iter().rev() {
        if range.is_empty() {
            if offset >= range.start {
                delta += inserted.len() as isize;
            }
            continue;
        }
        if offset <= range.start {
            break;
        }
        if offset >= range.end {
            delta += inserted.len() as isize - range.len() as isize;
            continue;
        }
        return (range.start as isize + delta + (offset - range.start).min(inserted.len()) as isize)
            .max(0) as usize;
    }
    (offset as isize + delta).max(0) as usize
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

impl Transaction {
    fn bytes(&self) -> usize {
        self.edits.iter().fold(0usize, |total, edit| {
            total.saturating_add(edit.removed.len().saturating_add(edit.inserted.len()))
        })
    }
}

pub(crate) struct HistoryStep {
    pub(crate) edits: Vec<CodeEdit>,
    pub(crate) selection: CodeSelection,
}

pub(crate) struct UndoHistory {
    undo: VecDeque<Transaction>,
    redo: Vec<Transaction>,
    undo_bytes: usize,
    redo_bytes: usize,
    base_mark: HistoryMark,
    next_id: u64,
    open: bool,
    last_edit_at: Option<Instant>,
    max_bytes: usize,
}

impl Default for UndoHistory {
    fn default() -> Self {
        Self {
            undo: VecDeque::new(),
            redo: Vec::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            base_mark: HistoryMark::Baseline,
            next_id: 0,
            open: false,
            last_edit_at: None,
            max_bytes: MAX_UNDO_BYTES,
        }
    }
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
        self.redo_bytes = 0;

        let joinable = group == EditGroup::Typing
            && self.open
            && self
                .last_edit_at
                .is_some_and(|last| now.saturating_duration_since(last) <= UNDO_GROUP_INTERVAL);
        if joinable && let Some(top) = self.undo.back_mut() {
            let added_bytes = edits.iter().fold(0usize, |total, edit| {
                total.saturating_add(edit.removed.len().saturating_add(edit.inserted.len()))
            });
            top.edits.extend(edits);
            top.after = after;
            self.undo_bytes = self.undo_bytes.saturating_add(added_bytes);
            self.trim_undo();
            self.last_edit_at = Some(now);
            return;
        }

        let id = self.next_id;
        self.next_id += 1;
        let transaction = Transaction {
            id,
            edits,
            before,
            after,
        };
        self.undo_bytes = self.undo_bytes.saturating_add(transaction.bytes());
        self.undo.push_back(transaction);
        self.trim_undo();
        self.open = group == EditGroup::Typing;
        self.last_edit_at = Some(now);
    }

    pub(crate) fn close_group(&mut self) {
        self.open = false;
    }

    pub(crate) fn undo(&mut self, doc: &mut CodeDocument) -> Option<HistoryStep> {
        self.open = false;
        let transaction = self.undo.pop_back()?;
        let bytes = transaction.bytes();
        self.undo_bytes = self.undo_bytes.saturating_sub(bytes);
        let mut edits = Vec::new();
        for record in transaction.edits.iter().rev() {
            edits.extend(apply_reverse(doc, record));
        }
        let selection = transaction.before;
        self.redo.push(transaction);
        self.redo_bytes = self.redo_bytes.saturating_add(bytes);
        Some(HistoryStep { edits, selection })
    }

    pub(crate) fn redo(&mut self, doc: &mut CodeDocument) -> Option<HistoryStep> {
        self.open = false;
        let transaction = self.redo.pop()?;
        let bytes = transaction.bytes();
        self.redo_bytes = self.redo_bytes.saturating_sub(bytes);
        let mut edits = Vec::new();
        for record in &transaction.edits {
            edits.extend(apply_forward(doc, record));
        }
        let selection = transaction.after;
        self.undo.push_back(transaction);
        self.undo_bytes = self.undo_bytes.saturating_add(bytes);
        Some(HistoryStep { edits, selection })
    }

    pub(crate) fn mark(&self) -> HistoryMark {
        match self.undo.back() {
            Some(transaction) => HistoryMark::Transaction(transaction.id),
            None => self.base_mark,
        }
    }

    fn trim_undo(&mut self) {
        while self.undo.len() > MAX_UNDO_TRANSACTIONS
            || (self.undo_bytes > self.max_bytes && self.undo.len() > 1)
        {
            let Some(evicted) = self.undo.pop_front() else {
                break;
            };
            self.undo_bytes = self.undo_bytes.saturating_sub(evicted.bytes());
            self.base_mark = HistoryMark::Transaction(evicted.id);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.undo_bytes = 0;
        self.redo_bytes = 0;
        self.base_mark = HistoryMark::Baseline;
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

    #[cfg(test)]
    fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn retained_bytes(&self) -> usize {
        self.undo_bytes.saturating_add(self.redo_bytes)
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
    fn disk_splices_apply_disjoint_line_hunks_in_one_plan() {
        let current = "keep\none\nmiddle\ntwo\ntail\n";
        let incoming = "keep\nONE!\nmiddle\nTWO!\ntail\n";
        let ops = disk_splices(&Rope::from_str(current), incoming);
        assert_eq!(ops.len(), 2, "one operation per disjoint hunk");
        assert!(ops[0].0.start > ops[1].0.start, "offsets descend");

        let mut d = doc(current);
        for (range, inserted) in ops {
            splice(&mut d, range, &inserted).expect("disk hunk applies");
        }
        assert_eq!(d.text().to_string(), incoming);
    }

    #[test]
    fn disk_splices_normalize_crlf_and_skip_identical_text() {
        let current = Rope::from_str("one\ntwo\n");
        assert!(disk_splices(&current, "one\r\ntwo\r\n").is_empty());
        let ops = disk_splices(&current, "one\r\nTWO\r\n");
        assert_eq!(ops, vec![(4..8, "TWO\n".to_string())]);
    }

    #[test]
    fn disk_splices_limit_a_ten_line_reload_to_ten_lines() {
        let current = (0..9_000)
            .map(|row| format!("line {row:04}\n"))
            .collect::<String>();
        let mut incoming_lines = lines_with_terminator(&current)
            .map(str::to_string)
            .collect::<Vec<_>>();
        for (row, line) in incoming_lines
            .iter_mut()
            .enumerate()
            .take(4_010)
            .skip(4_000)
        {
            *line = format!("changed {row:04}\n");
        }
        let incoming = incoming_lines.concat();
        let ops = disk_splices(&Rope::from_str(&current), &incoming);
        assert_eq!(ops.len(), 1);
        let changed = &current[ops[0].0.clone()];
        assert_eq!(lines_with_terminator(changed).count(), 10);

        let caret = current.find("line 8000").expect("caret line") + 5;
        let shifted = shift_selection_for_splices(CodeSelection::at(caret), &ops);
        assert_eq!(
            shifted.cursor(),
            incoming.find("line 8000").expect("shifted caret line") + 5
        );
    }

    #[test]
    fn a_single_oversized_transaction_survives_until_the_next_push() {
        let mut d = doc("");
        let mut history = UndoHistory::with_max_bytes(8);
        let sel = CodeSelection::at(0);
        let large = splice(&mut d, 0..0, "0123456789")
            .expect("large insert")
            .record;
        history.push(vec![large], sel, sel, EditGroup::Atomic, Instant::now());
        assert_eq!(history.len(), 1);
        assert_eq!(history.retained_bytes(), 10);

        let end = d.len_bytes();
        let next = splice(&mut d, end..end, "x").expect("next insert").record;
        history.push(vec![next], sel, sel, EditGroup::Atomic, Instant::now());
        assert_eq!(history.len(), 1);
        assert_eq!(history.retained_bytes(), 1);
    }

    #[test]
    fn an_evicted_saved_mark_is_reached_after_newer_edits_are_undone() {
        let mut d = doc("");
        let mut history = UndoHistory::with_max_bytes(4);
        let sel = CodeSelection::at(0);
        let saved_edit = splice(&mut d, 0..0, "save").expect("saved insert").record;
        history.push(
            vec![saved_edit],
            sel,
            sel,
            EditGroup::Atomic,
            Instant::now(),
        );
        let saved_mark = history.mark();
        let newer = splice(&mut d, 4..4, "!").expect("newer insert").record;
        history.push(vec![newer], sel, sel, EditGroup::Atomic, Instant::now());
        assert_ne!(history.mark(), saved_mark);
        assert_eq!(history.len(), 1, "the saved transaction was evicted");

        history.undo(&mut d).expect("newer edit is undoable");
        assert_eq!(d.text().to_string(), "save");
        assert_eq!(history.mark(), saved_mark);
    }

    #[test]
    fn a_new_transaction_drops_redo_bytes_before_enforcing_the_budget() {
        let mut d = doc("");
        let mut history = UndoHistory::with_max_bytes(16);
        let sel = CodeSelection::at(0);
        for text in ["aaaa", "bbbb"] {
            let end = d.len_bytes();
            let record = splice(&mut d, end..end, text).expect("insert").record;
            history.push(vec![record], sel, sel, EditGroup::Atomic, Instant::now());
        }
        history.undo(&mut d).expect("undo");
        assert_eq!(history.retained_bytes(), 8);
        assert_eq!(history.redo_len(), 1);

        let end = d.len_bytes();
        let record = splice(&mut d, end..end, "c").expect("branch insert").record;
        history.push(vec![record], sel, sel, EditGroup::Atomic, Instant::now());
        assert_eq!(history.redo_len(), 0);
        assert_eq!(history.retained_bytes(), 5);
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
