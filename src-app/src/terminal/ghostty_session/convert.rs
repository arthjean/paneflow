use super::*;

pub(super) fn search_result_from_ghostty(
    result: ghostty::SearchResult,
) -> crate::search::SearchResult {
    crate::search::SearchResult {
        matches: result
            .matches
            .into_iter()
            .map(|found| crate::search::SearchMatch {
                start: point_from_ghostty(found.start),
                end: point_from_ghostty(found.end),
            })
            .collect(),
        regex_error: result.regex_error,
        truncated: result.truncated,
    }
}

#[cfg(test)]
pub(super) fn pty_size(size: TerminalWindowSize) -> PtySize {
    PtySize {
        rows: size.rows.clamp(1, u16::MAX as usize) as u16,
        cols: size.cols.clamp(1, u16::MAX as usize) as u16,
        pixel_width: size
            .cols
            .saturating_mul(usize::from(size.cell_width))
            .min(u16::MAX as usize) as u16,
        pixel_height: size
            .rows
            .saturating_mul(usize::from(size.cell_height))
            .min(u16::MAX as usize) as u16,
    }
}

pub(super) fn normalized_window_size(size: TerminalWindowSize) -> TerminalWindowSize {
    TerminalWindowSize::new(
        size.cols.clamp(1, u16::MAX as usize),
        size.rows.clamp(1, u16::MAX as usize),
        size.cell_width,
        size.cell_height,
    )
}

pub(super) fn window_size(size: TerminalWindowSize) -> ghostty::Result<ghostty::WindowSize> {
    ghostty::WindowSize::new(
        size.cols,
        size.rows,
        u32::from(size.cell_width),
        u32::from(size.cell_height),
    )
}

pub(super) fn pixel_position(position: (f32, f32)) -> (f64, f64) {
    (f64::from(position.0), f64::from(position.1))
}

pub(super) fn ghostty_point(point: Point) -> ghostty::Point {
    ghostty::Point::new(point.line.0, point.column.0)
}

pub(super) fn point_from_ghostty(point: ghostty::Point) -> Point {
    Point::new(point.line, point.column)
}

pub(super) fn selection_range_from_ghostty(selection: ghostty::SelectionRange) -> SelectionRange {
    SelectionRange {
        start: point_from_ghostty(selection.start),
        end: point_from_ghostty(selection.end),
        is_block: selection.rectangle,
    }
}

pub(super) fn filter_copyable_selection_text(
    kind: Option<SelectionKind>,
    range: Option<SelectionRange>,
    text: Option<String>,
) -> Option<String> {
    let is_focus_click = matches!(kind, Some(SelectionKind::Simple))
        && range.is_some_and(|range| range.start == range.end);
    (!is_focus_click).then_some(text).flatten()
}

pub(in crate::terminal) fn modes_from_ghostty(modes: ghostty::Modes) -> Modes {
    let mut result = Modes::empty();
    if modes.alternate_screen {
        result = result | Modes::ALT_SCREEN;
    }
    if modes.application_cursor {
        result = result | Modes::APP_CURSOR;
    }
    if modes.application_keypad {
        result = result | Modes::APP_KEYPAD;
    }
    if modes.bracketed_paste {
        result = result | Modes::BRACKETED_PASTE;
    }
    if modes.focus_reporting {
        result = result | Modes::FOCUS_IN_OUT;
    }
    if modes.alternate_scroll {
        result = result | Modes::ALTERNATE_SCROLL;
    }
    if modes.sgr_mouse {
        result = result | Modes::SGR_MOUSE;
    }
    if modes.utf8_mouse {
        result = result | Modes::UTF8_MOUSE;
    }
    if modes.mouse_report_click {
        result = result | Modes::MOUSE_REPORT_CLICK;
    }
    if modes.mouse_drag {
        result = result | Modes::MOUSE_DRAG;
    }
    if modes.mouse_motion {
        result = result | Modes::MOUSE_MOTION;
    }
    if modes.kitty_keyboard {
        result = result | Modes::KITTY_KEYBOARD;
    }
    result
}

#[cfg(test)]
pub(in crate::terminal) fn content_from_ghostty(content: ghostty::Content) -> Content {
    let cells = cells_from_ghostty(&content.cells);
    let cursor = cursor_from_ghostty(&content);
    Content {
        generation: next_content_generation(),
        row_versions: Arc::default(),
        cols: content.cols,
        rows: content.rows,
        cells,
        cursor,
        selection: content.selection.map(selection_range_from_ghostty),
        display_offset: content.display_offset,
        history_size: content.history_size,
    }
}

fn cells_from_ghostty(cells: &[ghostty::Cell]) -> Arc<[Cell]> {
    cells.iter().map(cell_from_ghostty).collect()
}

fn cell_from_ghostty(cell: &ghostty::Cell) -> Cell {
    Cell {
        point: point_from_ghostty(cell.point),
        c: cell.character,
        fg: color_from_ghostty(cell.foreground, NamedColor::Foreground),
        bg: color_from_ghostty(cell.background, NamedColor::Background),
        flags: ghostty_cell_flags(cell),
        zerowidth: cell.zerowidth.as_deref().map(<[_]>::to_vec),
        hyperlink: cell.hyperlink,
    }
}

fn cursor_from_ghostty(content: &ghostty::Content) -> RenderableCursor {
    let cursor_viewport_line = content.cursor.point.line + content.display_offset as i32;
    let column = content.cursor.point.column;
    let cursor_cell = usize::try_from(cursor_viewport_line)
        .ok()
        .filter(|row| *row < content.rows && column < content.cols)
        .and_then(|row| content.cells.get(row * content.cols + column))
        .filter(|cell| cell.point.line == cursor_viewport_line && cell.point.column == column);
    let cursor_flags = cursor_cell.map_or(CellFlags::empty(), ghostty_cell_flags);
    RenderableCursor {
        point: point_from_ghostty(content.cursor.point),
        shape: if content.cursor.visible {
            match content.cursor.shape {
                ghostty::CursorShape::Bar => CursorShape::Beam,
                ghostty::CursorShape::Block => CursorShape::Block,
                ghostty::CursorShape::Underline => CursorShape::Underline,
                ghostty::CursorShape::HollowBlock => CursorShape::HollowBlock,
            }
        } else {
            CursorShape::Hidden
        },
        fg: cursor_cell.map_or(Color::Named(NamedColor::Foreground), |cell| {
            color_from_ghostty(cell.foreground, NamedColor::Foreground)
        }),
        bg: cursor_cell.map_or(Color::Named(NamedColor::Background), |cell| {
            color_from_ghostty(cell.background, NamedColor::Background)
        }),
        flags: cursor_flags,
        wide: cursor_cell.is_some_and(|cell| matches!(cell.wide, ghostty::WideCell::Wide)),
        text: cursor_cell.map_or(' ', |cell| cell.character),
        bold: cursor_flags.contains(CellFlags::BOLD),
        italic: cursor_flags.contains(CellFlags::ITALIC),
    }
}

#[derive(Default)]
pub(in crate::terminal) struct CellMirror {
    back: Arc<[Cell]>,
    back_stale: Vec<bool>,
    back_valid: bool,
    last_dirty: Vec<bool>,
    front_address: usize,
    published_address: usize,
    row_versions: Vec<u64>,
    cols: usize,
}

impl CellMirror {
    pub(in crate::terminal) fn publish(&mut self, snapshot: ghostty::Content) -> Content {
        let cols = snapshot.cols;
        let rows = snapshot.rows;
        let generation = next_content_generation();
        if self.cols != cols || self.row_versions.len() != rows {
            self.row_versions = vec![generation; rows];
            self.cols = cols;
        } else {
            for (row, version) in self.row_versions.iter_mut().enumerate() {
                if snapshot.dirty_rows.get(row).copied().unwrap_or(true) {
                    *version = generation;
                }
            }
        }
        let reusable = self.back_valid
            && !self.back.is_empty()
            && self.back.len() == snapshot.cells.len()
            && self.back_stale.len() == rows
            && snapshot.dirty_rows.len() == rows;
        let cells = match reusable.then(|| Arc::get_mut(&mut self.back)).flatten() {
            Some(buffer) => {
                for row in 0..rows {
                    if !(snapshot.dirty_rows[row] || self.back_stale[row]) {
                        continue;
                    }
                    let range = row * cols..(row + 1) * cols;
                    for (target, source) in
                        buffer[range.clone()].iter_mut().zip(&snapshot.cells[range])
                    {
                        *target = cell_from_ghostty(source);
                    }
                }
                std::mem::take(&mut self.back)
            }
            None => cells_from_ghostty(&snapshot.cells),
        };
        self.back_valid = false;
        self.last_dirty.clear();
        self.last_dirty.extend_from_slice(&snapshot.dirty_rows);
        self.published_address = cells.as_ptr().addr();
        Content {
            generation,
            row_versions: self.row_versions.as_slice().into(),
            cols,
            rows,
            cells,
            cursor: cursor_from_ghostty(&snapshot),
            selection: snapshot.selection.map(selection_range_from_ghostty),
            display_offset: snapshot.display_offset,
            history_size: snapshot.history_size,
        }
    }

    pub(in crate::terminal) fn recycle(&mut self, previous: Content) {
        let own =
            !previous.cells.is_empty() && previous.cells.as_ptr().addr() == self.front_address;
        self.back = previous.cells;
        self.back_valid = own;
        std::mem::swap(&mut self.back_stale, &mut self.last_dirty);
        self.front_address = self.published_address;
    }
}

fn ghostty_cell_flags(cell: &ghostty::Cell) -> CellFlags {
    let mut flags = CellFlags::empty();
    if cell.flags.inverse {
        flags |= CellFlags::INVERSE;
    }
    if cell.flags.bold {
        flags |= CellFlags::BOLD;
    }
    if cell.flags.italic {
        flags |= CellFlags::ITALIC;
    }
    if cell.flags.dim {
        flags |= CellFlags::DIM;
    }
    if cell.flags.strikethrough {
        flags |= CellFlags::STRIKEOUT;
    }
    match cell.flags.underline {
        ghostty::UnderlineStyle::None => {}
        ghostty::UnderlineStyle::Single => flags |= CellFlags::UNDERLINE,
        ghostty::UnderlineStyle::Double => flags |= CellFlags::DOUBLE_UNDERLINE,
        ghostty::UnderlineStyle::Curly => flags |= CellFlags::UNDERCURL,
        ghostty::UnderlineStyle::Dotted => flags |= CellFlags::DOTTED_UNDERLINE,
        ghostty::UnderlineStyle::Dashed => flags |= CellFlags::DASHED_UNDERLINE,
    }
    match cell.wide {
        ghostty::WideCell::Wide | ghostty::WideCell::SpacerHead => {
            flags |= CellFlags::WIDE_CHAR;
        }
        ghostty::WideCell::SpacerTail => flags |= CellFlags::WIDE_CHAR_SPACER,
        ghostty::WideCell::Narrow => {}
    }
    flags
}

fn color_from_ghostty(color: ghostty::Color, default: NamedColor) -> Color {
    match color {
        ghostty::Color::Default => Color::Named(default),
        ghostty::Color::Palette(index) => match index {
            0 => Color::Named(NamedColor::Black),
            1 => Color::Named(NamedColor::Red),
            2 => Color::Named(NamedColor::Green),
            3 => Color::Named(NamedColor::Yellow),
            4 => Color::Named(NamedColor::Blue),
            5 => Color::Named(NamedColor::Magenta),
            6 => Color::Named(NamedColor::Cyan),
            7 => Color::Named(NamedColor::White),
            8 => Color::Named(NamedColor::BrightBlack),
            9 => Color::Named(NamedColor::BrightRed),
            10 => Color::Named(NamedColor::BrightGreen),
            11 => Color::Named(NamedColor::BrightYellow),
            12 => Color::Named(NamedColor::BrightBlue),
            13 => Color::Named(NamedColor::BrightMagenta),
            14 => Color::Named(NamedColor::BrightCyan),
            15 => Color::Named(NamedColor::BrightWhite),
            _ => Color::Indexed(index),
        },
        ghostty::Color::Rgb(rgb) => Color::Spec(Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
    }
}

pub(in crate::terminal) fn blank_content(cols: usize, rows: usize) -> Content {
    let cells: Arc<[Cell]> = (0..rows)
        .flat_map(|row| {
            (0..cols).map(move |column| Cell {
                point: Point::new(row as i32, column),
                c: ' ',
                fg: Color::Named(NamedColor::Foreground),
                bg: Color::Named(NamedColor::Background),
                flags: CellFlags::empty(),
                zerowidth: None,
                hyperlink: false,
            })
        })
        .collect::<Vec<_>>()
        .into();
    Content {
        generation: next_content_generation(),
        row_versions: Arc::default(),
        cols,
        rows,
        cells,
        cursor: RenderableCursor {
            point: Point::new(0, 0),
            shape: CursorShape::Block,
            fg: Color::Spec(Rgb::default()),
            bg: Color::Spec(Rgb::default()),
            flags: CellFlags::empty(),
            wide: false,
            text: ' ',
            bold: false,
            italic: false,
        },
        selection: None,
        display_offset: 0,
        history_size: 0,
    }
}

pub(super) fn initial_grid_metrics(cols: usize, rows: usize) -> GridMetrics {
    GridMetrics {
        columns: cols,
        screen_lines: rows,
        display_offset: 0,
        topmost_line: Line(0),
        bottommost_line: Line(i32::try_from(rows.saturating_sub(1)).unwrap_or(i32::MAX)),
        cursor: Point::new(0, 0),
    }
}

pub(super) fn grid_metrics_from_ghostty(content: &ghostty::Content) -> GridMetrics {
    GridMetrics {
        columns: content.cols,
        screen_lines: content.rows,
        display_offset: content.display_offset,
        topmost_line: Line(-i32::try_from(content.history_size).unwrap_or(i32::MAX)),
        bottommost_line: Line(i32::try_from(content.rows.saturating_sub(1)).unwrap_or(i32::MAX)),
        cursor: point_from_ghostty(content.cursor.point),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_size_reports_cells_and_total_pixels() {
        assert_eq!(
            pty_size(TerminalWindowSize::new(80, 24, 8, 16)),
            PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 640,
                pixel_height: 384,
            }
        );
    }

    #[test]
    fn content_conversion_preserves_snapshot_grid_dimensions() {
        let content = content_from_ghostty(ghostty::Content {
            cells: Vec::<ghostty::Cell>::new().into(),
            dirty_rows: Vec::new().into(),
            cursor: ghostty::Cursor {
                point: ghostty::Point::new(0, 0),
                shape: ghostty::CursorShape::Block,
                visible: true,
                blinking: false,
                wide_tail: false,
            },
            selection: None,
            cols: 80,
            rows: 24,
            display_offset: 0,
            history_size: 0,
        });

        assert_eq!((content.cols, content.rows), (80, 24));
    }

    #[test]
    fn the_cell_mirror_matches_a_full_conversion_across_partial_frames() {
        let size = ghostty::WindowSize::new(40, 8, 8, 16).expect("valid grid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("libghostty initializes");
        let mut mirror = CellMirror::default();
        let mut front = blank_content(40, 8);
        let mut addresses = Vec::new();
        let frames: [&[u8]; 7] = [
            b"\x1b[31mfirst\x1b[0m line\r\n",
            b"second line\r\n",
            b"\x1b[3;1Hedited row three",
            b"\x1b[7;5H\x1b[1mbold\x1b[0m",
            b"\x1b[1;1H\x1b[2Jcleared",
            b"\x1b[8;1H\r\n\r\n\r\nscrolled",
            b"\xf0\x9f\x98\x80\xe2\x80\x8d\xf0\x9f\x92\xbb tail",
        ];
        for (index, bytes) in frames.iter().enumerate() {
            terminal.feed(bytes).expect("frame parses");
            let snapshot = terminal.snapshot().expect("snapshot");
            let expected = content_from_ghostty(snapshot.clone());
            let published = mirror.publish(snapshot);
            assert_eq!(
                format!("{:?}", published.cells),
                format!("{:?}", expected.cells),
                "frame {index}"
            );
            assert_eq!(
                published.cursor.point, expected.cursor.point,
                "frame {index}"
            );
            assert_eq!(published.cursor.text, expected.cursor.text, "frame {index}");
            addresses.push(published.cells.as_ptr().addr());
            let previous = std::mem::replace(&mut front, published);
            mirror.recycle(previous);
        }
        assert_eq!(addresses[2], addresses[0]);
        assert_eq!(addresses[3], addresses[1]);
        assert_eq!(addresses[6], addresses[4]);
    }

    #[test]
    fn the_cell_mirror_falls_back_to_a_fresh_buffer_while_the_renderer_holds_the_old_one() {
        let size = ghostty::WindowSize::new(20, 4, 8, 16).expect("valid grid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("libghostty initializes");
        let mut mirror = CellMirror::default();
        let mut front = blank_content(20, 4);
        let mut publish = |terminal: &mut ghostty::DisplayTerminal, front: &mut Content| {
            terminal.feed(b"x").expect("frame parses");
            let published = mirror.publish(terminal.snapshot().expect("snapshot"));
            let previous = std::mem::replace(front, published);
            mirror.recycle(previous);
            front.cells.clone()
        };
        let first = publish(&mut terminal, &mut front);
        let second_address = publish(&mut terminal, &mut front).as_ptr().addr();
        let third = publish(&mut terminal, &mut front);
        assert!(!Arc::ptr_eq(&first, &third));
        let third_address = third.as_ptr().addr();
        drop(first);
        drop(third);
        let fourth_address = publish(&mut terminal, &mut front).as_ptr().addr();
        assert_eq!(fourth_address, second_address);
        let fifth_address = publish(&mut terminal, &mut front).as_ptr().addr();
        assert_eq!(fifth_address, third_address);
    }
}
