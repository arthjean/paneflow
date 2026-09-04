mod by_char;
mod by_line;
mod by_word;
mod chunk_optimizer;
mod iterable;
mod manager;
mod splitter;
mod text;
mod tracker;

pub use iterable::{DiffTooBig, Range};
pub use manager::split_lines;
pub use tracker::{Block, BlockKind, BlockTracker, TrackerStats, TOO_BIG_BLOCK_LINES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ComparisonPolicy {
    #[default]
    Default,
    TrimWhitespaces,
    IgnoreWhitespaces,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum HighlightPolicy {
    Lines,
    #[default]
    Words,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DiffFragment {
    pub start1: usize,
    pub end1: usize,
    pub start2: usize,
    pub end2: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineFragment {
    pub lines: Range,
    pub offsets: Range,
    pub inner: Option<Vec<DiffFragment>>,
}

pub fn compare_lines(lines1: &[&str], lines2: &[&str], policy: ComparisonPolicy) -> Vec<Range> {
    if lines1 == lines2 {
        return Vec::new();
    }
    by_line::compare(lines1, lines2, policy).changed().to_vec()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LineDiffReport {
    pub fragments: Vec<LineFragment>,
    pub too_big_blocks: usize,
}

pub fn compare_lines_inner_report(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    highlight: HighlightPolicy,
) -> LineDiffReport {
    let report = manager::compare_lines_inner_unsquashed(text1, text2, policy, highlight);
    LineDiffReport {
        fragments: manager::squash(report.fragments),
        too_big_blocks: report.too_big_blocks,
    }
}

pub fn compare_lines_inner(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
    highlight: HighlightPolicy,
) -> Vec<LineFragment> {
    compare_lines_inner_report(text1, text2, policy, highlight).fragments
}

pub fn compare_words(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> Result<Vec<DiffFragment>, DiffTooBig> {
    by_word::compare(text1, text2, policy)
}

pub fn compare_chars(
    text1: &str,
    text2: &str,
    policy: ComparisonPolicy,
) -> Result<Vec<DiffFragment>, DiffTooBig> {
    let changes = by_char::compare_chars(text1, text2, policy)?;
    Ok(by_word::convert_into_fragments(&changes))
}

#[cfg(test)]
mod oracle;
#[cfg(test)]
mod tests;
