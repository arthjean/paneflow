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
        mouse_shape: mouse_shape_from_ghostty(content.mouse_shape),
        overscan_rows: overscan_rows_from_ghostty(&content.overscan_rows),
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
    back_shift: isize,
    back_valid: bool,
    last_dirty: Vec<bool>,
    last_shift: isize,
    front_address: usize,
    published_address: usize,
    row_versions: Vec<u64>,
    version_scratch: Vec<u64>,
    revision: Option<u64>,
    cols: usize,
    overscan_source: Arc<[ghostty::OverscanRow]>,
    overscan_rows: Arc<[OverscanRow]>,
}

impl CellMirror {
    pub(in crate::terminal) fn publish(&mut self, snapshot: ghostty::Content) -> Content {
        let cols = snapshot.cols;
        let rows = snapshot.rows;
        let generation = next_content_generation();
        let continuous = self
            .revision
            .is_some_and(|revision| revision.wrapping_add(1) == snapshot.revision)
            && self.cols == cols
            && self.row_versions.len() == rows
            && snapshot.dirty_rows.len() == rows;
        self.revision = Some(snapshot.revision);
        self.cols = cols;
        let shift = if continuous { snapshot.row_shift } else { 0 };
        let source_of = |row: usize| {
            row.checked_add_signed(shift)
                .filter(|source| *source < rows)
        };
        let mut dirty = std::mem::take(&mut self.last_dirty);
        dirty.clear();
        dirty.extend(
            (0..rows)
                .map(|row| !continuous || snapshot.dirty_rows[row] || source_of(row).is_none()),
        );
        self.version_scratch.clear();
        for (row, stale) in dirty.iter().enumerate() {
            let version = match source_of(row).filter(|_| !stale) {
                Some(source) => self.row_versions[source],
                None => next_content_generation(),
            };
            self.version_scratch.push(version);
        }
        std::mem::swap(&mut self.row_versions, &mut self.version_scratch);
        let back_offset = shift.checked_add(self.back_shift).filter(|offset| {
            offset.unsigned_abs() < rows
                && self.back_valid
                && continuous
                && !self.back.is_empty()
                && self.back.len() == snapshot.cells.len()
                && self.back_stale.len() == rows
        });
        let cells = match back_offset
            .and_then(|offset| Arc::get_mut(&mut self.back).map(|buffer| (offset, buffer)))
        {
            Some((offset, buffer)) => {
                ghostty::rotate_cells(buffer, offset * cols as isize);
                for (row, row_dirty) in dirty.iter().enumerate() {
                    let range = row * cols..(row + 1) * cols;
                    let reusable = !row_dirty
                        && source_of(row).is_some_and(|source| !self.back_stale[source])
                        && row
                            .checked_add_signed(offset)
                            .is_some_and(|back_row| back_row < rows);
                    if reusable {
                        if offset != 0 {
                            let line = Line(row as i32);
                            for cell in &mut buffer[range] {
                                cell.point.line = line;
                            }
                        }
                        continue;
                    }
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
        self.last_dirty = dirty;
        self.last_shift = shift;
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
            mouse_shape: mouse_shape_from_ghostty(snapshot.mouse_shape),
            overscan_rows: self.overscan_rows(snapshot.overscan_rows),
        }
    }

    fn overscan_rows(&mut self, source: Arc<[ghostty::OverscanRow]>) -> Arc<[OverscanRow]> {
        if !Arc::ptr_eq(&self.overscan_source, &source) {
            self.overscan_rows = if source.is_empty() && self.overscan_rows.is_empty() {
                self.overscan_rows.clone()
            } else {
                overscan_rows_from_ghostty(&source)
            };
            self.overscan_source = source;
        }
        self.overscan_rows.clone()
    }

    pub(in crate::terminal) fn recycle(&mut self, previous: Content) {
        let own =
            !previous.cells.is_empty() && previous.cells.as_ptr().addr() == self.front_address;
        self.back = previous.cells;
        self.back_valid = own;
        std::mem::swap(&mut self.back_stale, &mut self.last_dirty);
        self.back_shift = self.last_shift;
        self.front_address = self.published_address;
    }
}

fn overscan_rows_from_ghostty(rows: &[ghostty::OverscanRow]) -> Arc<[OverscanRow]> {
    rows.iter()
        .map(|row| OverscanRow {
            viewport_y: row.identity.viewport_y,
            cells: cells_from_ghostty(&row.cells),
        })
        .collect()
}

fn mouse_shape_from_ghostty(shape: ghostty::MouseShape) -> MouseShape {
    match shape {
        ghostty::MouseShape::Default => MouseShape::Default,
        ghostty::MouseShape::ContextMenu => MouseShape::ContextMenu,
        ghostty::MouseShape::Help => MouseShape::Help,
        ghostty::MouseShape::Pointer => MouseShape::Pointer,
        ghostty::MouseShape::Progress => MouseShape::Progress,
        ghostty::MouseShape::Wait => MouseShape::Wait,
        ghostty::MouseShape::Cell => MouseShape::Cell,
        ghostty::MouseShape::Crosshair => MouseShape::Crosshair,
        ghostty::MouseShape::Text => MouseShape::Text,
        ghostty::MouseShape::VerticalText => MouseShape::VerticalText,
        ghostty::MouseShape::Alias => MouseShape::Alias,
        ghostty::MouseShape::Copy => MouseShape::Copy,
        ghostty::MouseShape::Move => MouseShape::Move,
        ghostty::MouseShape::NoDrop => MouseShape::NoDrop,
        ghostty::MouseShape::NotAllowed => MouseShape::NotAllowed,
        ghostty::MouseShape::Grab => MouseShape::Grab,
        ghostty::MouseShape::Grabbing => MouseShape::Grabbing,
        ghostty::MouseShape::AllScroll => MouseShape::AllScroll,
        ghostty::MouseShape::ColResize => MouseShape::ColResize,
        ghostty::MouseShape::RowResize => MouseShape::RowResize,
        ghostty::MouseShape::NResize => MouseShape::NResize,
        ghostty::MouseShape::EResize => MouseShape::EResize,
        ghostty::MouseShape::SResize => MouseShape::SResize,
        ghostty::MouseShape::WResize => MouseShape::WResize,
        ghostty::MouseShape::NeResize => MouseShape::NeResize,
        ghostty::MouseShape::NwResize => MouseShape::NwResize,
        ghostty::MouseShape::SeResize => MouseShape::SeResize,
        ghostty::MouseShape::SwResize => MouseShape::SwResize,
        ghostty::MouseShape::EwResize => MouseShape::EwResize,
        ghostty::MouseShape::NsResize => MouseShape::NsResize,
        ghostty::MouseShape::NeswResize => MouseShape::NeswResize,
        ghostty::MouseShape::NwseResize => MouseShape::NwseResize,
        ghostty::MouseShape::ZoomIn => MouseShape::ZoomIn,
        ghostty::MouseShape::ZoomOut => MouseShape::ZoomOut,
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
        mouse_shape: MouseShape::Text,
        overscan_rows: Arc::default(),
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
            mouse_shape: ghostty::MouseShape::Text,
            row_identities: Vec::new().into(),
            row_shift: 0,
            revision: 0,
            overscan: ghostty::Overscan::default(),
            overscan_rows: Vec::new().into(),
        });

        assert_eq!((content.cols, content.rows), (80, 24));
    }

    #[test]
    fn overscan_rows_are_published_apart_from_the_viewport_grid() {
        let size = ghostty::WindowSize::new(12, 4, 8, 16).expect("valid grid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("libghostty initializes");
        for line in 0..30 {
            terminal
                .feed(format!("row {line:02}\r\n").as_bytes())
                .expect("output parses");
        }
        let mut mirror = CellMirror::default();
        let plain = mirror.publish(terminal.snapshot().expect("snapshot"));
        assert!(plain.overscan_rows.is_empty());
        let empty = plain.overscan_rows.clone();
        mirror.recycle(plain);
        let again = mirror.publish(terminal.snapshot().expect("snapshot"));
        assert!(
            Arc::ptr_eq(&empty, &again.overscan_rows),
            "no overscan must not allocate a new row list per frame"
        );
        mirror.recycle(again);

        terminal.scroll(ghostty::Scroll::Delta(6));
        terminal.set_overscan(1, 1).expect("overscan request");
        let content = mirror.publish(terminal.snapshot().expect("snapshot"));

        assert_eq!(content.cells.len(), 12 * 4);
        assert!(
            content
                .cells
                .iter()
                .all(|cell| (0..4).contains(&cell.point.line.0))
        );
        let rows = content
            .overscan_rows
            .iter()
            .map(|row| {
                (
                    row.viewport_y,
                    row.cells.iter().map(|cell| cell.c).collect::<String>(),
                )
            })
            .collect::<Vec<_>>();
        let top = content.cells[..12]
            .iter()
            .map(|cell| cell.c)
            .collect::<String>();
        let top_line: i32 = top.trim()["row ".len()..].parse().expect("numbered row");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].0, -1);
        assert_eq!(rows[0].1.trim(), format!("row {:02}", top_line - 1));
        assert_eq!(rows[1].0, 4);
        assert_eq!(rows[1].1.trim(), format!("row {:02}", top_line + 4));
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

    fn row_text(content: &Content, row: usize) -> String {
        content.cells[row * content.cols..(row + 1) * content.cols]
            .iter()
            .map(|cell| {
                format!(
                    "{:?}",
                    (cell.c, cell.fg, cell.bg, cell.flags, &cell.zerowidth)
                )
            })
            .collect()
    }

    #[test]
    fn the_cell_mirror_reuses_shifted_rows_without_changing_what_is_shown() {
        let size = ghostty::WindowSize::new(40, 8, 8, 16).expect("valid grid");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("libghostty initializes");
        for line in 0..30 {
            terminal
                .feed(format!("\x1b[3{}mrow {line:02}\x1b[0m words\r\n", line % 8).as_bytes())
                .expect("history parses");
        }
        let mut mirror = CellMirror::default();
        let mut front = mirror.publish(terminal.snapshot().expect("snapshot"));
        let mut kept_versions = 0usize;
        type Step<'a> = (&'a str, &'a dyn Fn(&mut ghostty::DisplayTerminal));
        let cases: [Step<'_>; 9] = [
            ("one line", &|t| t.feed(b"one more\r\n").expect("parses")),
            ("several lines", &|t| {
                t.feed(b"a\r\nb\r\nc\r\n").expect("parses")
            }),
            ("scroll back", &|t| t.scroll(ghostty::Scroll::Delta(-3))),
            ("output while scrolled back", &|t| {
                t.feed(b"behind\r\n").expect("parses")
            }),
            ("back to the bottom", &|t| t.scroll(ghostty::Scroll::Bottom)),
            ("alternate screen", &|t| {
                t.feed(b"\x1b[?1049hfull screen app\r\n").expect("parses")
            }),
            ("primary screen", &|t| {
                t.feed(b"\x1b[?1049lback\r\n").expect("parses")
            }),
            ("clear", &|t| {
                t.feed(b"\x1b[2J\x1b[Hcleared\r\nnext\r\n").expect("parses")
            }),
            ("resize during output", &|t| {
                t.feed(b"before resize\r\n").expect("parses");
                t.resize(ghostty::WindowSize::new(36, 7, 8, 16).expect("valid grid"))
                    .expect("resize applies");
                t.feed(b"after resize\r\n").expect("parses");
            }),
        ];
        for (name, step) in cases {
            step(&mut terminal);
            let snapshot = terminal.snapshot().expect("snapshot");
            let expected = content_from_ghostty(snapshot.clone());
            let published = mirror.publish(snapshot);
            assert_eq!(
                format!("{:?}", published.cells),
                format!("{:?}", expected.cells),
                "{name}"
            );
            if published.rows == front.rows && published.cols == front.cols {
                for row in 0..published.rows {
                    if let Some(source) = front
                        .row_versions
                        .iter()
                        .position(|version| *version == published.row_versions[row])
                    {
                        kept_versions += 1;
                        assert_eq!(
                            row_text(&published, row),
                            row_text(&front, source),
                            "{name} row {row}"
                        );
                    }
                }
            }
            let previous = std::mem::replace(&mut front, published);
            mirror.recycle(previous);
        }
        assert!(kept_versions > 0, "no row kept its version across a shift");
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
