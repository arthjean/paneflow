use std::ops::Range;

pub use paneflow_textdiff::{ComparisonPolicy, HighlightPolicy};
use paneflow_textdiff::{DiffFragment, LineDiffReport, compare_lines_inner_report};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct DiffOptions {
    pub highlight: HighlightPolicy,
    pub whitespace: ComparisonPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffHunkStatus {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChangeTone {
    #[default]
    Full,
    Muted,
    Plain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub base_row_range: Range<u32>,
    pub new_row_range: Range<u32>,
    pub status: DiffHunkStatus,
    pub tone: ChangeTone,
    pub base_runs: Vec<Vec<Range<usize>>>,
    pub new_runs: Vec<Vec<Range<usize>>>,
}

impl DiffHunk {
    #[cfg(test)]
    pub fn plain(base_row_range: Range<u32>, new_row_range: Range<u32>) -> Self {
        let status = hunk_status(&base_row_range, &new_row_range);
        Self {
            base_row_range,
            new_row_range,
            status,
            tone: ChangeTone::Full,
            base_runs: Vec::new(),
            new_runs: Vec::new(),
        }
    }

    pub fn base_line_runs(&self, row: u32) -> &[Range<usize>] {
        line_runs_at(&self.base_runs, &self.base_row_range, row)
    }

    pub fn new_line_runs(&self, row: u32) -> &[Range<usize>] {
        line_runs_at(&self.new_runs, &self.new_row_range, row)
    }
}

fn line_runs_at<'a>(
    runs: &'a [Vec<Range<usize>>],
    rows: &Range<u32>,
    row: u32,
) -> &'a [Range<usize>] {
    row.checked_sub(rows.start)
        .and_then(|offset| runs.get(offset as usize))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn hunk_status(base_rows: &Range<u32>, new_rows: &Range<u32>) -> DiffHunkStatus {
    if new_rows.is_empty() {
        DiffHunkStatus::Deleted
    } else if base_rows.is_empty() {
        DiffHunkStatus::Added
    } else {
        DiffHunkStatus::Modified
    }
}

#[cfg(test)]
pub fn compute_hunks(base: &str, new: &str) -> Vec<DiffHunk> {
    compute_hunks_with(base, new, DiffOptions::default())
}

pub struct HunkReport {
    pub hunks: Vec<DiffHunk>,
    pub too_big_blocks: usize,
}

#[cfg(test)]
pub fn compute_hunks_with(base: &str, new: &str, options: DiffOptions) -> Vec<DiffHunk> {
    compute_hunk_report(base, new, options).hunks
}

pub fn compute_hunk_report(base: &str, new: &str, options: DiffOptions) -> HunkReport {
    if base == new {
        return HunkReport {
            hunks: Vec::new(),
            too_big_blocks: 0,
        };
    }
    let LineDiffReport {
        fragments,
        too_big_blocks,
    } = compare_lines_inner_report(base, new, options.whitespace, options.highlight);
    let base_spans = line_spans(base);
    let new_spans = line_spans(new);
    let mut hunks: Vec<DiffHunk> = Vec::with_capacity(fragments.len());
    for fragment in &fragments {
        let base_rows = real_rows(
            fragment.lines.start1,
            fragment.lines.end1,
            &base_spans,
            base,
        );
        let new_rows = real_rows(fragment.lines.start2, fragment.lines.end2, &new_spans, new);
        let phantom_only = base_rows.is_empty() && new_rows.is_empty();
        let (mut base_rows, mut new_rows) = if phantom_only {
            (last_row(base_spans.len()), last_row(new_spans.len()))
        } else {
            (base_rows, new_rows)
        };
        if base_rows.is_empty() && new_rows.is_empty() {
            continue;
        }
        let (previous_base_end, previous_new_end) = hunks
            .last()
            .map(|h: &DiffHunk| (h.base_row_range.end, h.new_row_range.end))
            .unwrap_or((0, 0));
        if base_rows.start < previous_base_end || new_rows.start < previous_new_end {
            continue;
        }
        let context_lines =
            (base_rows.start - previous_base_end).min(new_rows.start - previous_new_end);
        base_rows.start = previous_base_end + context_lines;
        new_rows.start = previous_new_end + context_lines;
        let status = hunk_status(&base_rows, &new_rows);
        let tone = tone_for(options.highlight, fragment.inner.as_deref());
        let (base_runs, new_runs) = match (&fragment.inner, phantom_only) {
            (Some(inner), false) if !inner.is_empty() => (
                side_runs(
                    &base_spans,
                    &base_rows,
                    fragment.offsets.start1,
                    inner.iter().map(|piece| (piece.start1, piece.end1)),
                ),
                side_runs(
                    &new_spans,
                    &new_rows,
                    fragment.offsets.start2,
                    inner.iter().map(|piece| (piece.start2, piece.end2)),
                ),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        hunks.push(DiffHunk {
            base_row_range: base_rows,
            new_row_range: new_rows,
            status,
            tone,
            base_runs,
            new_runs,
        });
    }
    push_unpaired_tail(
        &mut hunks,
        base_spans.len() as u32,
        new_spans.len() as u32,
        options.highlight,
    );
    HunkReport {
        hunks,
        too_big_blocks,
    }
}

fn push_unpaired_tail(
    hunks: &mut Vec<DiffHunk>,
    base_lines: u32,
    new_lines: u32,
    highlight: HighlightPolicy,
) {
    let (base_end, new_end) = hunks
        .last()
        .map(|h| (h.base_row_range.end, h.new_row_range.end))
        .unwrap_or((0, 0));
    let paired = (base_lines - base_end).min(new_lines - new_end);
    let base_rows = base_end + paired..base_lines;
    let new_rows = new_end + paired..new_lines;
    if base_rows.is_empty() && new_rows.is_empty() {
        return;
    }
    hunks.push(DiffHunk {
        status: hunk_status(&base_rows, &new_rows),
        base_row_range: base_rows,
        new_row_range: new_rows,
        tone: tone_for(highlight, None),
        base_runs: Vec::new(),
        new_runs: Vec::new(),
    });
}

fn tone_for(highlight: HighlightPolicy, inner: Option<&[DiffFragment]>) -> ChangeTone {
    match (highlight, inner) {
        (HighlightPolicy::None, _) => ChangeTone::Plain,
        (_, Some([])) => ChangeTone::Muted,
        _ => ChangeTone::Full,
    }
}

fn real_rows(start: usize, end: usize, spans: &[Range<usize>], text: &str) -> Range<u32> {
    let real = spans.len();
    let has_phantom = text.is_empty() || text.ends_with('\n');
    let phantom_only = has_phantom && start + 1 == real + 1 && end == real + 1;
    if phantom_only
        && let Some(last) = start.checked_sub(1)
        && spans.get(last).is_some_and(Range::is_empty)
    {
        return last as u32..start as u32;
    }
    let end = end.min(real);
    let start = start.min(end);
    start as u32..end as u32
}

fn last_row(line_count: usize) -> Range<u32> {
    let end = line_count as u32;
    end.saturating_sub(1)..end
}

fn line_spans(text: &str) -> Vec<Range<usize>> {
    let mut spans = Vec::new();
    let mut start = 0usize;
    for segment in text.split_inclusive('\n') {
        let content = segment
            .strip_suffix('\n')
            .map(|s| s.strip_suffix('\r').unwrap_or(s))
            .unwrap_or(segment);
        spans.push(start..start + content.len());
        start += segment.len();
    }
    spans
}

fn side_runs(
    spans: &[Range<usize>],
    rows: &Range<u32>,
    block_start: usize,
    pieces: impl Iterator<Item = (usize, usize)>,
) -> Vec<Vec<Range<usize>>> {
    let pieces: Vec<(usize, usize)> = pieces
        .filter(|(start, end)| start < end)
        .map(|(start, end)| (block_start + start, block_start + end))
        .collect();
    let mut result = Vec::with_capacity(rows.len());
    let mut next_piece = 0usize;
    for row in rows.clone() {
        let Some(span) = spans.get(row as usize) else {
            result.push(Vec::new());
            continue;
        };
        while next_piece < pieces.len() && pieces[next_piece].1 <= span.start {
            next_piece += 1;
        }
        let mut runs = Vec::new();
        let mut index = next_piece;
        while index < pieces.len() && pieces[index].0 < span.end {
            let (start, end) = pieces[index];
            let clipped = start.max(span.start)..end.min(span.end);
            if clipped.start < clipped.end {
                runs.push(clipped.start - span.start..clipped.end - span.start);
            }
            index += 1;
        }
        result.push(runs);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_has_no_hunks() {
        assert!(compute_hunks("a\nb\nc\n", "a\nb\nc\n").is_empty());
        assert!(compute_hunks("", "").is_empty());
    }

    #[test]
    fn pure_addition() {
        let hunks = compute_hunks("a\nb\n", "a\nb\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Added);
        assert!(h.base_row_range.start == h.base_row_range.end);
        assert_eq!(h.new_row_range, 2..3);
    }

    #[test]
    fn pure_deletion() {
        let hunks = compute_hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Deleted);
        assert_eq!(h.base_row_range, 1..2);
        assert!(h.new_row_range.start == h.new_row_range.end);
    }

    #[test]
    fn modification() {
        let hunks = compute_hunks("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Modified);
        assert_eq!(h.base_row_range, 1..2);
        assert_eq!(h.new_row_range, 1..2);
    }

    #[test]
    fn multiple_disjoint_hunks() {
        let hunks = compute_hunks("a\nb\nc\nd\ne\n", "A\nb\nc\nd\nE\n");
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].status, DiffHunkStatus::Modified);
        assert_eq!(hunks[0].new_row_range, 0..1);
        assert_eq!(hunks[1].status, DiffHunkStatus::Modified);
        assert_eq!(hunks[1].new_row_range, 4..5);
    }

    #[test]
    fn added_from_empty_base() {
        let hunks = compute_hunks("", "a\nb\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
    }

    #[test]
    fn default_options_highlight_words_with_default_whitespace() {
        assert_eq!(
            DiffOptions::default(),
            DiffOptions {
                highlight: HighlightPolicy::Words,
                whitespace: ComparisonPolicy::Default,
            }
        );
    }

    #[test]
    fn modified_block_carries_word_runs_on_both_sides() {
        let hunks = compute_hunks("let x = old_value;\n", "let x = new_value;\n");
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.status, DiffHunkStatus::Modified);
        assert_eq!(h.tone, ChangeTone::Full);
        assert_eq!(h.base_line_runs(0), std::slice::from_ref(&(8..17)));
        assert_eq!(h.new_line_runs(0), std::slice::from_ref(&(8..17)));
    }

    #[test]
    fn runs_are_split_per_line_and_never_exceed_line_text() {
        let base = "alpha beta\ngamma delta\n";
        let new = "alpha BETA\ngamma DELTA\n";
        let hunks = compute_hunks(base, new);
        assert_eq!(hunks.len(), 1);
        let h = &hunks[0];
        assert_eq!(h.base_row_range, 0..2);
        assert_eq!(h.base_line_runs(0), std::slice::from_ref(&(6..10)));
        assert_eq!(h.base_line_runs(1), std::slice::from_ref(&(6..11)));
        assert_eq!(h.new_line_runs(0), std::slice::from_ref(&(6..10)));
        assert_eq!(h.new_line_runs(1), std::slice::from_ref(&(6..11)));
        assert!(h.base_line_runs(2).is_empty());
    }

    #[test]
    fn insertion_has_no_runs_and_full_tone() {
        let hunks = compute_hunks("a\nc\n", "a\nb\nc\n");
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
        assert_eq!(hunks[0].tone, ChangeTone::Full);
        assert!(hunks[0].new_line_runs(1).is_empty());
    }

    #[test]
    fn a_trailing_blank_line_is_an_insertion_of_that_line() {
        let hunks = compute_hunks("a\nb\n", "a\nb\n\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
        assert_eq!(hunks[0].base_row_range, 2..2);
        assert_eq!(hunks[0].new_row_range, 2..3);
        let hunks = compute_hunks("a\nb\n\n", "a\nb\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Deleted);
        assert_eq!(hunks[0].base_row_range, 2..3);
        assert_eq!(hunks[0].new_row_range, 2..2);
    }

    #[test]
    fn blank_line_insertion_is_muted_under_ignore_and_full_otherwise() {
        let ignore = DiffOptions {
            highlight: HighlightPolicy::Words,
            whitespace: ComparisonPolicy::IgnoreWhitespaces,
        };
        let hunks = compute_hunks_with("a\nb\n", "a\n   \nb\n", ignore);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
        assert_eq!(hunks[0].new_row_range, 1..2);
        assert_eq!(hunks[0].tone, ChangeTone::Muted);
        assert!(hunks[0].new_line_runs(1).is_empty());
        let trim = DiffOptions {
            highlight: HighlightPolicy::Words,
            whitespace: ComparisonPolicy::TrimWhitespaces,
        };
        assert_eq!(
            compute_hunks_with("a\nb\n", "a\n   \nb\n", trim)[0].tone,
            ChangeTone::Full
        );
        assert_eq!(
            compute_hunks("a\nb\n", "a\n   \nb\n")[0].tone,
            ChangeTone::Full
        );
    }

    #[test]
    fn indentation_change_is_a_full_block_with_a_whitespace_run_under_default_policy() {
        let hunks = compute_hunks("fn main() {\n", "  fn main() {\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].tone, ChangeTone::Full);
        assert_eq!(hunks[0].new_line_runs(0), std::slice::from_ref(&(0..2)));
    }

    #[test]
    fn inner_whitespace_change_stays_full_under_trim_policy() {
        let options = DiffOptions {
            highlight: HighlightPolicy::Words,
            whitespace: ComparisonPolicy::TrimWhitespaces,
        };
        let hunks = compute_hunks_with("let a = b;\n", "let a  =  b;\n", options);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].tone, ChangeTone::Full);
        assert!(!hunks[0].new_line_runs(0).is_empty());
    }

    #[test]
    fn ignore_whitespace_drops_indentation_only_blocks() {
        let options = DiffOptions {
            highlight: HighlightPolicy::Words,
            whitespace: ComparisonPolicy::IgnoreWhitespaces,
        };
        assert!(compute_hunks_with("fn main() {\n", "  fn main() {\n", options).is_empty());
        let hunks = compute_hunks_with("a\n  b\nc\n", "a\nb\nC\n", options);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].new_row_range, 2..3);
    }

    #[test]
    fn lines_highlight_yields_full_tone_without_runs() {
        let options = DiffOptions {
            highlight: HighlightPolicy::Lines,
            whitespace: ComparisonPolicy::Default,
        };
        let hunks = compute_hunks_with("let x = 1;\n", "let x = 2;\n", options);
        assert_eq!(hunks[0].tone, ChangeTone::Full);
        assert!(hunks[0].base_line_runs(0).is_empty());
    }

    #[test]
    fn none_highlight_yields_plain_tone() {
        let options = DiffOptions {
            highlight: HighlightPolicy::None,
            whitespace: ComparisonPolicy::Default,
        };
        let hunks = compute_hunks_with("let x = 1;\n", "let x = 2;\n", options);
        assert_eq!(hunks[0].tone, ChangeTone::Plain);
        assert!(hunks[0].new_line_runs(0).is_empty());
    }

    #[test]
    fn crlf_runs_stop_before_the_carriage_return() {
        let hunks = compute_hunks("let x = 1;\r\nend\r\n", "let x = 2;\r\nend\r\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].base_row_range, 0..1);
        assert!(!hunks[0].new_line_runs(0).is_empty());
        for run in hunks[0].new_line_runs(0) {
            assert!(run.end <= "let x = 2;".len());
        }
    }

    #[test]
    fn missing_trailing_newline_marks_the_last_line() {
        let hunks = compute_hunks("a\nb", "a\nb\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].base_row_range, 1..2);
        assert_eq!(hunks[0].new_row_range, 1..2);
        let hunks = compute_hunks("", "\n");
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].status, DiffHunkStatus::Added);
        assert_eq!(hunks[0].new_row_range, 0..1);
    }

    #[test]
    fn a_giant_line_reports_too_big_blocks_and_keeps_line_hunks() {
        let base: String = (0..25_000).map(|i| format!("w{i} ")).collect();
        let new: String = (0..25_000).map(|i| format!("v{i} ")).collect();
        let report = compute_hunk_report(&base, &new, DiffOptions::default());
        assert_eq!(report.too_big_blocks, 1);
        assert_eq!(report.hunks.len(), 1);
        assert_eq!(report.hunks[0].tone, ChangeTone::Full);
        assert!(report.hunks[0].new_line_runs(0).is_empty());
    }

    #[test]
    fn a_10k_character_line_keeps_runs_inside_the_text() {
        let base = format!("{}\n", "x".repeat(10_000));
        let new = format!("{}y\n", "x".repeat(10_000));
        let hunks = compute_hunks(&base, &new);
        for run in hunks[0].new_line_runs(0) {
            assert!(run.end <= 10_001);
        }
        for run in hunks[0].base_line_runs(0) {
            assert!(run.end <= 10_000);
        }
    }
    fn next_random(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *state >> 33
    }

    fn random_text(state: &mut u64) -> String {
        let alphabet = ["a", "b", "c", " ", "  ", ";", ",", "\t", "x1", "yy"];
        let lines = (next_random(state) % 9) as usize;
        let mut out = String::new();
        for _ in 0..lines {
            for _ in 0..(next_random(state) % 6) {
                out.push_str(alphabet[(next_random(state) % alphabet.len() as u64) as usize]);
            }
            if !next_random(state).is_multiple_of(8) {
                out.push('\n');
            }
        }
        out
    }

    fn content_lines(text: &str) -> Vec<&str> {
        text.split_inclusive('\n')
            .map(|line| {
                line.strip_suffix('\n')
                    .map(|l| l.strip_suffix('\r').unwrap_or(l))
                    .unwrap_or(line)
            })
            .collect()
    }

    #[test]
    fn random_texts_keep_every_line_paired_or_inside_a_hunk() {
        let mut state = 0x5eed_1234_u64;
        for _ in 0..600 {
            let base = random_text(&mut state);
            let new = random_text(&mut state);
            let base_lines = content_lines(&base);
            let new_lines = content_lines(&new);
            for whitespace in [
                ComparisonPolicy::Default,
                ComparisonPolicy::TrimWhitespaces,
                ComparisonPolicy::IgnoreWhitespaces,
            ] {
                for highlight in [
                    HighlightPolicy::Words,
                    HighlightPolicy::Lines,
                    HighlightPolicy::None,
                ] {
                    let options = DiffOptions {
                        highlight,
                        whitespace,
                    };
                    let hunks = compute_hunk_report(&base, &new, options).hunks;
                    let case = || format!("{options:?} base={base:?} new={new:?} hunks={hunks:?}");
                    let mut bc = 0u32;
                    let mut nc = 0u32;
                    for hunk in &hunks {
                        assert!(
                            hunk.base_row_range.start >= bc && hunk.new_row_range.start >= nc,
                            "hunks overlap: {}",
                            case()
                        );
                        assert_eq!(
                            hunk.base_row_range.start - bc,
                            hunk.new_row_range.start - nc,
                            "context gap is not paired one to one: {}",
                            case()
                        );
                        assert!(
                            hunk.base_row_range.end as usize <= base_lines.len()
                                && hunk.new_row_range.end as usize <= new_lines.len(),
                            "hunk runs past the file: {}",
                            case()
                        );
                        assert!(
                            !(hunk.base_row_range.is_empty() && hunk.new_row_range.is_empty()),
                            "empty hunk: {}",
                            case()
                        );
                        if whitespace == ComparisonPolicy::Default {
                            for k in 0..(hunk.base_row_range.start - bc) {
                                assert_eq!(
                                    base_lines[(bc + k) as usize],
                                    new_lines[(nc + k) as usize],
                                    "context line differs: {}",
                                    case()
                                );
                            }
                        }
                        for row in hunk.base_row_range.clone() {
                            let len = base_lines[row as usize].len();
                            for run in hunk.base_line_runs(row) {
                                assert!(
                                    run.start < run.end && run.end <= len,
                                    "base run outside its line: {}",
                                    case()
                                );
                            }
                        }
                        for row in hunk.new_row_range.clone() {
                            let len = new_lines[row as usize].len();
                            for run in hunk.new_line_runs(row) {
                                assert!(
                                    run.start < run.end && run.end <= len,
                                    "new run outside its line: {}",
                                    case()
                                );
                            }
                        }
                        bc = hunk.base_row_range.end;
                        nc = hunk.new_row_range.end;
                    }
                    assert_eq!(
                        base_lines.len() as u32 - bc,
                        new_lines.len() as u32 - nc,
                        "trailing context is not paired one to one: {}",
                        case()
                    );
                    if whitespace == ComparisonPolicy::Default {
                        for k in 0..(base_lines.len() as u32 - bc) {
                            assert_eq!(
                                base_lines[(bc + k) as usize],
                                new_lines[(nc + k) as usize],
                                "trailing context line differs: {}",
                                case()
                            );
                        }
                    }
                }
            }
        }
    }
}
