use regex::Regex;

use crate::{GhosttyError, Point, Result, SearchMatch, SearchResult};

pub const MAX_QUERY_LEN: usize = 512;
const MAX_MATCHES: usize = 10_000;
pub const MAX_SEARCH_CELLS: usize = 12_000_000;
pub const SEARCH_CHUNK_CELLS: usize = 64 * 1024;

pub struct SearchLine {
    pub line: i32,
    pub text: String,
    pub char_to_column: Vec<usize>,
}

pub struct SearchChunk {
    pub lines: Vec<SearchLine>,
    pub next_row: usize,
    pub total_rows: usize,
    pub cols: usize,
}

pub struct SearchEngine {
    regex: Option<Regex>,
    plain_query: Vec<char>,
    result: SearchResult,
    done: bool,
}

impl SearchEngine {
    pub fn new(query: &str, regex_mode: bool) -> Result<Self> {
        if query.len() > MAX_QUERY_LEN {
            return Err(GhosttyError::LimitExceeded {
                resource: "search query",
                limit: MAX_QUERY_LEN,
            });
        }
        if query.is_empty() {
            return Ok(Self {
                regex: None,
                plain_query: Vec::new(),
                result: SearchResult::default(),
                done: true,
            });
        }
        let (regex, regex_error) = if regex_mode {
            match regex::RegexBuilder::new(query)
                .case_insensitive(true)
                .build()
            {
                Ok(regex) => (Some(regex), None),
                Err(error) => (None, Some(error.to_string())),
            }
        } else {
            (None, None)
        };
        let done = regex_error.is_some();
        Ok(Self {
            regex,
            plain_query: if regex_mode {
                Vec::new()
            } else {
                query.chars().collect()
            },
            result: SearchResult {
                matches: Vec::new(),
                regex_error,
                truncated: false,
            },
            done,
        })
    }

    pub fn push_line(&mut self, line: i32, text: &str, char_to_column: &[usize]) -> bool {
        if self.done {
            return false;
        }
        if let Some(regex) = &self.regex {
            for found in regex.find_iter(text) {
                let start = text[..found.start()].chars().count();
                let count = text[found.start()..found.end()].chars().count();
                push_match(line, char_to_column, start, count, &mut self.result.matches);
                if self.result.matches.len() == MAX_MATCHES {
                    self.result.truncated = true;
                    self.done = true;
                    return false;
                }
            }
        } else {
            let line_chars: Vec<char> = text.chars().collect();
            if line_chars.len() >= self.plain_query.len() {
                for start in 0..=line_chars.len() - self.plain_query.len() {
                    if line_chars[start..start + self.plain_query.len()]
                        .iter()
                        .zip(&self.plain_query)
                        .all(|(&actual, &expected)| {
                            actual.to_lowercase().eq(expected.to_lowercase())
                        })
                    {
                        push_match(
                            line,
                            char_to_column,
                            start,
                            self.plain_query.len(),
                            &mut self.result.matches,
                        );
                        if self.result.matches.len() == MAX_MATCHES {
                            self.result.truncated = true;
                            self.done = true;
                            return false;
                        }
                    }
                }
            }
        }
        true
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn finish(mut self, truncated: bool) -> SearchResult {
        self.result.truncated |= truncated;
        self.result
    }
}

fn push_match(
    line: i32,
    char_to_column: &[usize],
    start: usize,
    count: usize,
    matches: &mut Vec<SearchMatch>,
) {
    if count == 0 {
        return;
    }
    if let (Some(&start_column), Some(&end_column)) = (
        char_to_column.get(start),
        char_to_column.get(start + count - 1),
    ) {
        matches.push(SearchMatch {
            start: Point::new(line, start_column),
            end: Point::new(line, end_column),
        });
    }
}

#[cfg(ghostty_native)]
impl crate::engine::DisplayTerminal {
    pub fn search_chunk(&self, start_row: usize, max_cells: usize) -> Result<SearchChunk> {
        let geometry = self.grid_geometry()?;
        if start_row > geometry.total_rows {
            return Err(GhosttyError::AbiMismatch(
                "search start row is outside the grid".into(),
            ));
        }
        let rows = max_cells
            .min(MAX_SEARCH_CELLS)
            .checked_div(geometry.cols)
            .unwrap_or(0)
            .min(geometry.total_rows.saturating_sub(start_row));
        let next_row = start_row.saturating_add(rows);
        let lines = self
            .grid_lines(Some(start_row..next_row))?
            .into_iter()
            .map(|line| SearchLine {
                line: line.line,
                text: line.text,
                char_to_column: line.char_to_column,
            })
            .collect();
        Ok(SearchChunk {
            lines,
            next_row,
            total_rows: geometry.total_rows,
            cols: geometry.cols,
        })
    }

    pub fn search(&self, query: &str, regex_mode: bool) -> Result<SearchResult> {
        let mut search = SearchEngine::new(query, regex_mode)?;
        if search.is_done() {
            return Ok(search.finish(false));
        }
        let mut next_row = 0;
        let mut scanned_cells = 0usize;
        loop {
            let remaining = MAX_SEARCH_CELLS.saturating_sub(scanned_cells);
            if remaining == 0 {
                return Ok(search.finish(true));
            }
            let chunk = self.search_chunk(next_row, remaining.min(SEARCH_CHUNK_CELLS))?;
            if chunk.next_row == next_row && chunk.next_row < chunk.total_rows {
                return Ok(search.finish(true));
            }
            scanned_cells = scanned_cells.saturating_add(chunk.lines.len() * chunk.cols);
            for line in chunk.lines {
                if !search.push_line(line.line, &line.text, &line.char_to_column) {
                    return Ok(search.finish(false));
                }
            }
            if chunk.next_row >= chunk.total_rows {
                return Ok(search.finish(false));
            }
            next_row = chunk.next_row;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_engine_maps_wide_and_lowercase_expansion_columns() {
        let mut search = SearchEngine::new("abc", false).unwrap();
        assert!(search.push_line(7, "中İabc", &[0, 2, 3, 4, 5]));
        let result = search.finish(false);

        assert_eq!(result.matches.len(), 1);
        assert_eq!(result.matches[0].start, Point::new(7, 3));
        assert_eq!(result.matches[0].end, Point::new(7, 5));
        assert!(!result.truncated);
    }

    #[test]
    fn invalid_regex_is_a_result_not_a_scan_failure() {
        let search = SearchEngine::new("(", true).unwrap();
        assert!(search.is_done());
        assert!(search.finish(false).regex_error.is_some());
    }
}
