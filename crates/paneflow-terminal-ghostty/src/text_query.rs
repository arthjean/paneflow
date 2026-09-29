use crate::Result;
use crate::engine::DisplayTerminal;
use crate::grid::GridLine;
use crate::search::MAX_SEARCH_CELLS;

const SUPERSEDED_CHECK_ROWS: usize = 256;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowWindow {
    pub lines: Vec<String>,
    pub total_lines: usize,
    pub eof: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowMatches {
    pub matches: Vec<(i32, String)>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowSearch {
    Complete(RowMatches),
    Superseded,
}

impl DisplayTerminal {
    pub fn row_window(&self, lines: usize, offset: usize) -> Result<RowWindow> {
        let geometry = self.grid_geometry()?;
        let mut line = GridLine::default();
        let mut grapheme = Vec::new();
        let mut total_lines = 0;
        for y in (0..geometry.total_rows).rev() {
            self.fill_grid_line(y, &geometry, &mut line, &mut grapheme)?;
            if !line.text.trim_end().is_empty() {
                total_lines = y + 1;
                break;
            }
        }
        let end = total_lines.saturating_sub(offset);
        let start = end.saturating_sub(lines);
        let mut window = Vec::with_capacity(end - start);
        for y in start..end {
            self.fill_grid_line(y, &geometry, &mut line, &mut grapheme)?;
            window.push(line.text.trim_end().to_owned());
        }
        Ok(RowWindow {
            lines: window,
            total_lines,
            eof: start == 0,
        })
    }

    pub fn search_rows(
        &self,
        query: &str,
        max_rows: usize,
        superseded: &dyn Fn() -> bool,
    ) -> Result<RowSearch> {
        let needle = query.to_lowercase();
        if needle.is_empty() || max_rows == 0 {
            return Ok(RowSearch::Complete(RowMatches::default()));
        }
        let geometry = self.grid_geometry()?;
        let row_budget = MAX_SEARCH_CELLS / geometry.cols;
        let mut line = GridLine::default();
        let mut grapheme = Vec::new();
        let mut matches = Vec::new();
        for y in 0..geometry.total_rows {
            if y % SUPERSEDED_CHECK_ROWS == 0 && superseded() {
                return Ok(RowSearch::Superseded);
            }
            if y >= row_budget {
                return Ok(RowSearch::Complete(RowMatches {
                    matches,
                    truncated: true,
                }));
            }
            self.fill_grid_line(y, &geometry, &mut line, &mut grapheme)?;
            if !line.text.to_lowercase().contains(&needle) {
                continue;
            }
            if matches.len() == max_rows {
                return Ok(RowSearch::Complete(RowMatches {
                    matches,
                    truncated: true,
                }));
            }
            matches.push((line.line, line.text.trim_end().to_owned()));
        }
        Ok(RowSearch::Complete(RowMatches {
            matches,
            truncated: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 1_000, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    fn feed_lines(terminal: &mut DisplayTerminal, count: usize) {
        for index in 0..count {
            terminal
                .feed(format!("line {index}\r\n").as_bytes())
                .expect("feed");
        }
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn row_window_returns_only_the_requested_tail_rows() {
        let mut terminal = terminal(20, 5);
        feed_lines(&mut terminal, 30);

        let window = terminal.row_window(3, 0).expect("window");

        assert_eq!(window.total_lines, 30);
        assert_eq!(window.lines, vec!["line 27", "line 28", "line 29"]);
        assert!(!window.eof);
    }

    #[test]
    fn row_window_offset_walks_back_and_reports_the_top() {
        let mut terminal = terminal(20, 5);
        feed_lines(&mut terminal, 30);

        let window = terminal.row_window(10, 25).expect("window");

        assert_eq!(window.lines, vec!["line 0", "line 1", "line 2", "line 3", "line 4"]);
        assert!(window.eof);
        let past = terminal.row_window(10, 40).expect("window past top");
        assert!(past.lines.is_empty());
        assert_eq!(past.total_lines, 30);
    }

    #[test]
    fn row_window_of_a_blank_terminal_is_empty_at_eof() {
        let terminal = terminal(20, 5);

        let window = terminal.row_window(10, 0).expect("window");

        assert_eq!(window, RowWindow {
            lines: Vec::new(),
            total_lines: 0,
            eof: true,
        });
    }

    #[test]
    fn search_rows_is_truncated_only_when_more_rows_match() {
        let mut terminal = terminal(20, 5);
        feed_lines(&mut terminal, 12);

        let exact = terminal.search_rows("LINE 1", 3, &never).expect("search");
        let RowSearch::Complete(exact) = exact else {
            unreachable!("search was superseded");
        };
        let texts: Vec<&str> = exact.matches.iter().map(|(_, text)| text.as_str()).collect();
        assert_eq!(texts, vec!["line 1", "line 10", "line 11"]);
        assert_eq!(exact.matches[1].0 - exact.matches[0].0, 9);
        assert!(!exact.truncated);

        let capped = terminal.search_rows("line 1", 2, &never).expect("search");
        let RowSearch::Complete(capped) = capped else {
            unreachable!("search was superseded");
        };
        assert_eq!(capped.matches.len(), 2);
        assert!(capped.truncated);
    }

    #[test]
    fn search_rows_reports_a_superseded_scan() {
        let mut terminal = terminal(20, 5);
        feed_lines(&mut terminal, 12);

        let result = terminal.search_rows("line", 50, &|| true).expect("search");

        assert_eq!(result, RowSearch::Superseded);
    }
}
