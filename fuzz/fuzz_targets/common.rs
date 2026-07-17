use alacritty_terminal::Term;
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Point};
use alacritty_terminal::term::Config;
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};
use paneflow_terminal_ghostty::{Content, DisplayTerminal, WideCell, WindowSize};

const MAX_FUZZ_BYTES: usize = 4_096;

struct TerminalDimensions {
    cols: usize,
    rows: usize,
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub fn differential_replay(data: &[u8], cols: usize, rows: usize, snapshot_each_chunk: bool) {
    let dimensions = TerminalDimensions { cols, rows };
    let mut alacritty = Term::new(Config::default(), &dimensions, VoidListener);
    let mut processor = Processor::<StdSyncHandler>::new();
    let size = WindowSize::new(cols, rows, 8, 16).expect("bounded fuzz dimensions are valid");
    let mut ghostty = DisplayTerminal::new(size, 1_000).expect("libghostty initializes");
    let input = bounded_vt_input(data, cols, rows);

    for chunk in input.chunks(64) {
        processor.advance(&mut alacritty, chunk);
        ghostty
            .feed(chunk)
            .expect("libghostty accepts bounded input");
        if snapshot_each_chunk {
            assert_shared_invariants(&alacritty, &mut ghostty, cols, rows);
        }
    }
    assert_shared_invariants(&alacritty, &mut ghostty, cols, rows);
}

fn bounded_vt_input(data: &[u8], cols: usize, rows: usize) -> Vec<u8> {
    let limit = MAX_FUZZ_BYTES.min(cols.saturating_mul(rows).saturating_div(2).max(1));
    let mut input = Vec::with_capacity(limit.saturating_mul(4));
    for byte in data.iter().copied().take(limit) {
        match byte % 8 {
            0 => input.extend_from_slice(b"\x1b[0m"),
            1 => input.extend_from_slice(b"\x1b[1m"),
            2 => input.extend_from_slice(b"\x1b[4m"),
            3 => input.extend_from_slice(b"\r\n"),
            _ => input.push(b' ' + (byte % 95)),
        }
    }
    input
}

fn assert_shared_invariants(
    alacritty: &Term<VoidListener>,
    ghostty: &mut DisplayTerminal,
    cols: usize,
    rows: usize,
) {
    let content = ghostty.snapshot().expect("libghostty snapshot succeeds");
    assert_eq!(content.cols, cols);
    assert_eq!(content.rows, rows);
    assert_cursor_in_bounds(&content);
    assert_eq!(
        normalized_alacritty_text(alacritty),
        normalized_ghostty_text(&content)
    );
}

fn assert_cursor_in_bounds(content: &Content) {
    assert!(content.cursor.point.column < content.cols);
    assert!(content.cursor.point.line >= 0);
    assert!((content.cursor.point.line as usize) < content.rows);
}

fn normalized_alacritty_text(term: &Term<VoidListener>) -> String {
    let start = Point::new(alacritty_terminal::index::Line(0), Column(0));
    let end = Point::new(term.bottommost_line(), term.last_column());
    normalize_text(term.bounds_to_string(start, end))
}

fn normalized_ghostty_text(content: &Content) -> String {
    let mut lines = vec![String::new(); content.rows];
    for cell in content.cells.iter() {
        let Ok(line) = usize::try_from(cell.point.line) else {
            continue;
        };
        if line >= lines.len() || cell.point.column >= content.cols {
            continue;
        }
        if matches!(cell.wide, WideCell::SpacerHead | WideCell::SpacerTail) {
            continue;
        }
        let line = &mut lines[line];
        if line.len() < cell.point.column {
            line.extend(std::iter::repeat_n(' ', cell.point.column - line.len()));
        }
        line.push(cell.character);
        if let Some(zerowidth) = &cell.zerowidth {
            line.extend(zerowidth.iter().copied());
        }
    }
    normalize_text(lines.join("\n"))
}

fn normalize_text(text: String) -> String {
    let mut lines: Vec<_> = text.lines().map(str::trim_end).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}
