use crate::engine::DisplayTerminal;
use crate::grid::GridLine;
use crate::{GhosttyError, Point, Result, SearchMatch, SearchResult};

const MAX_MATCHES: usize = 10_000;
const MAX_QUERY_BYTES: usize = 512;
const SEARCH_CHUNK_LINES: usize = 128;

impl DisplayTerminal {
    pub fn search(&self, query: &str, regex_mode: bool) -> Result<SearchResult> {
        if query.len() > MAX_QUERY_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "search query",
                limit: MAX_QUERY_BYTES,
            });
        }
        if query.is_empty() {
            return Ok(SearchResult::default());
        }
        let regex = if regex_mode {
            match regex::RegexBuilder::new(query)
                .case_insensitive(true)
                .build()
            {
                Ok(regex) => Some(regex),
                Err(error) => {
                    return Ok(SearchResult {
                        matches: Vec::new(),
                        regex_error: Some(error.to_string()),
                    });
                }
            }
        } else {
            None
        };
        let folded_query: Vec<String> = if regex_mode {
            Vec::new()
        } else {
            query.chars().map(fold_char).collect()
        };
        let mut matches = Vec::new();
        let total_rows = self.total_rows()?;
        for start_row in (0..total_rows).step_by(SEARCH_CHUNK_LINES) {
            let end_row = start_row.saturating_add(SEARCH_CHUNK_LINES).min(total_rows);
            for line in self.grid_lines(Some(start_row..end_row))? {
                if let Some(regex) = &regex {
                    for found in regex.find_iter(&line.text) {
                        let start = line.text[..found.start()].chars().count();
                        let count = line.text[found.start()..found.end()].chars().count();
                        push_match(&line, start, count, &mut matches);
                        if matches.len() == MAX_MATCHES {
                            return Ok(SearchResult {
                                matches,
                                regex_error: None,
                            });
                        }
                    }
                } else {
                    let folded_line: Vec<String> = line.text.chars().map(fold_char).collect();
                    if folded_line.len() < folded_query.len() {
                        continue;
                    }
                    for start in 0..=folded_line.len() - folded_query.len() {
                        if folded_line[start..start + folded_query.len()] == folded_query {
                            push_match(&line, start, folded_query.len(), &mut matches);
                            if matches.len() == MAX_MATCHES {
                                return Ok(SearchResult {
                                    matches,
                                    regex_error: None,
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(SearchResult {
            matches,
            regex_error: None,
        })
    }
}

fn push_match(line: &GridLine, start: usize, count: usize, matches: &mut Vec<SearchMatch>) {
    if count == 0 {
        return;
    }
    if let (Some(&start_column), Some(&end_column)) = (
        line.char_to_column.get(start),
        line.char_to_column.get(start + count - 1),
    ) {
        matches.push(SearchMatch {
            start: Point::new(line.line, start_column),
            end: Point::new(line.line, end_column),
        });
    }
}

fn fold_char(character: char) -> String {
    character.to_lowercase().collect()
}
