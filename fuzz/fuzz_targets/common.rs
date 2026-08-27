use paneflow_terminal_ghostty::{
    Content, DisplayTerminal, TerminalAppearance, WideCell, WindowSize,
};

const MAX_FUZZ_BYTES: usize = 4_096;

/// Feed bounded VT input to libghostty and assert the snapshot invariants the
/// renderer relies on. The engine is the only implementation now, so there is
/// no second parser to diff against: the oracle is the contract the terminal
/// element assumes when it paints a `Content`.
pub fn replay(data: &[u8], cols: usize, rows: usize, snapshot_each_chunk: bool) {
    let size = WindowSize::new(cols, rows, 8, 16).expect("bounded fuzz dimensions are valid");
    let mut ghostty = DisplayTerminal::new(size, 1_000, TerminalAppearance::default())
        .expect("libghostty initializes");
    let input = bounded_vt_input(data, cols, rows);

    for chunk in input.chunks(64) {
        ghostty
            .feed(chunk)
            .expect("libghostty accepts bounded input");
        if snapshot_each_chunk {
            assert_snapshot_invariants(&mut ghostty, cols, rows);
        }
    }
    assert_snapshot_invariants(&mut ghostty, cols, rows);
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

fn assert_snapshot_invariants(ghostty: &mut DisplayTerminal, cols: usize, rows: usize) {
    let content = ghostty.snapshot().expect("libghostty snapshot succeeds");
    assert_eq!(content.cols, cols);
    assert_eq!(content.rows, rows);
    assert_cursor_in_bounds(&content);
    assert_cells_in_bounds(&content);
    // Rebuilding the visible text must not panic on any cell the engine emits:
    // this is the exact walk the renderer and `extract_scrollback` perform.
    let _ = visible_text(&content);
}

fn assert_cursor_in_bounds(content: &Content) {
    assert!(content.cursor.point.column < content.cols);
    assert!(content.cursor.point.line >= 0);
    assert!((content.cursor.point.line as usize) < content.rows);
}

fn assert_cells_in_bounds(content: &Content) {
    for cell in content.cells.iter() {
        assert!(cell.point.column < content.cols);
        let line = usize::try_from(cell.point.line).expect("viewport lines are non-negative");
        assert!(line < content.rows);
    }
}

fn visible_text(content: &Content) -> String {
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
