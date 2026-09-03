use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::snapshot_ffi::ghostty_point;
use crate::{GhosttyError, Point, Result, SelectionRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionAdjust {
    Left,
    Right,
    Up,
    Down,
    BeginningOfLine,
    EndOfLine,
    Home,
    End,
    PageUp,
    PageDown,
}

impl SelectionAdjust {
    fn raw(self) -> sys::GhosttySelectionAdjust {
        use sys as s;
        match self {
            Self::Left => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_LEFT,
            Self::Right => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_RIGHT,
            Self::Up => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_UP,
            Self::Down => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_DOWN,
            Self::BeginningOfLine => {
                s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_BEGINNING_OF_LINE
            }
            Self::EndOfLine => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_END_OF_LINE,
            Self::Home => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_HOME,
            Self::End => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_END,
            Self::PageUp => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_PAGE_UP,
            Self::PageDown => s::GhosttySelectionAdjust_GHOSTTY_SELECTION_ADJUST_PAGE_DOWN,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOrder {
    Forward,
    Reverse,
    MirroredForward,
    MirroredReverse,
}

impl SelectionOrder {
    fn raw(self) -> sys::GhosttySelectionOrder {
        use sys as s;
        match self {
            Self::Forward => s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_FORWARD,
            Self::Reverse => s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_REVERSE,
            Self::MirroredForward => {
                s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_FORWARD
            }
            Self::MirroredReverse => {
                s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_REVERSE
            }
        }
    }

    fn from_raw(value: sys::GhosttySelectionOrder) -> Result<Self> {
        use sys as s;
        match value {
            s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_FORWARD => Ok(Self::Forward),
            s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_REVERSE => Ok(Self::Reverse),
            s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_FORWARD => {
                Ok(Self::MirroredForward)
            }
            s::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_MIRRORED_REVERSE => {
                Ok(Self::MirroredReverse)
            }
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty selection order {other}"
            ))),
        }
    }
}

impl DisplayTerminal {
    pub fn select_all(&mut self) -> Result<bool> {
        let mut selection = empty_selection();
        let result =
            unsafe { sys::ghostty_terminal_select_all(self.terminal.raw(), &mut selection) };
        self.install_optional_selection(result, &selection)
    }

    pub fn select_output(&mut self, point: Point) -> Result<bool> {
        let reference = self.grid_ref(point)?;
        let mut selection = empty_selection();
        let result = unsafe {
            sys::ghostty_terminal_select_output(self.terminal.raw(), reference, &mut selection)
        };
        self.install_optional_selection(result, &selection)
    }

    pub fn nearest_word_between(
        &self,
        start: Point,
        end: Point,
        boundaries: &[char],
    ) -> Result<Option<SelectionRange>> {
        let codepoints: Vec<u32> = boundaries.iter().copied().map(u32::from).collect();
        let options = sys::GhosttyTerminalSelectWordBetweenOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectWordBetweenOptions>(),
            start: self.grid_ref(start)?,
            end: self.grid_ref(end)?,
            boundary_codepoints: pointer_or_null(&codepoints),
            boundary_codepoints_len: codepoints.len(),
        };
        let mut selection = empty_selection();
        let result = unsafe {
            sys::ghostty_terminal_select_word_between(
                self.terminal.raw(),
                &options,
                &mut selection,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("select_word_between", result)?;
        self.selection_range_of(&selection).map(Some)
    }

    pub fn select_words_between(
        &mut self,
        start: Point,
        end: Point,
        boundaries: &[char],
    ) -> Result<bool> {
        let forward = self.nearest_word_between(start, end, boundaries)?;
        let backward = self.nearest_word_between(end, start, boundaries)?;
        let (Some(forward), Some(backward)) = (forward, backward) else {
            return Ok(false);
        };
        let mut points = [forward.start, forward.end, backward.start, backward.end];
        points.sort_by_key(|point| (point.line, point.column));
        self.set_selection(SelectionRange {
            start: points[0],
            end: points[3],
            rectangle: false,
        })?;
        Ok(true)
    }

    pub(crate) fn selection_range_of(
        &self,
        selection: &sys::GhosttySelection,
    ) -> Result<SelectionRange> {
        Ok(SelectionRange {
            start: self.point_from_grid_ref(&selection.start)?,
            end: self.point_from_grid_ref(&selection.end)?,
            rectangle: selection.rectangle,
        })
    }

    pub(crate) fn point_from_grid_ref(&self, reference: &sys::GhosttyGridRef) -> Result<Point> {
        let mut coordinate = sys::GhosttyPointCoordinate { x: 0, y: 0 };
        let result = unsafe {
            sys::ghostty_terminal_point_from_grid_ref(
                self.terminal.raw(),
                reference,
                sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
                &mut coordinate,
            )
        };
        check("point_from_grid_ref", result)?;
        let scrollback = i64::try_from(self.scrollback_rows()?)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i64".into()))?;
        let line = i64::from(coordinate.y)
            .checked_sub(scrollback)
            .and_then(|line| i32::try_from(line).ok())
            .ok_or_else(|| GhosttyError::AbiMismatch("grid point does not fit i32".into()))?;
        Ok(Point::new(line, usize::from(coordinate.x)))
    }

    pub fn adjust_selection(&mut self, adjustment: SelectionAdjust) -> Result<bool> {
        let Some(mut selection) = self.current_selection()? else {
            return Ok(false);
        };
        let result = unsafe {
            sys::ghostty_terminal_selection_adjust(
                self.terminal.raw(),
                &mut selection,
                adjustment.raw(),
            )
        };
        check("selection_adjust", result)?;
        self.install_selection(&selection)?;
        Ok(true)
    }

    pub fn selection_order(&self) -> Result<Option<SelectionOrder>> {
        let Some(selection) = self.current_selection()? else {
            return Ok(None);
        };
        let mut order = sys::GhosttySelectionOrder_GHOSTTY_SELECTION_ORDER_FORWARD;
        let result = unsafe {
            sys::ghostty_terminal_selection_order(self.terminal.raw(), &selection, &mut order)
        };
        check("selection_order", result)?;
        SelectionOrder::from_raw(order).map(Some)
    }

    pub fn order_selection(&mut self, desired: SelectionOrder) -> Result<bool> {
        let Some(selection) = self.current_selection()? else {
            return Ok(false);
        };
        let mut ordered = empty_selection();
        let result = unsafe {
            sys::ghostty_terminal_selection_ordered(
                self.terminal.raw(),
                &selection,
                desired.raw(),
                &mut ordered,
            )
        };
        check("selection_ordered", result)?;
        self.install_selection(&ordered)?;
        Ok(true)
    }

    pub fn selection_contains(&self, point: Point) -> Result<bool> {
        let Some(selection) = self.current_selection()? else {
            return Ok(false);
        };
        let scrollback = i64::try_from(self.scrollback_rows()?)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i64".into()))?;
        let screen_y = i64::from(point.line)
            .checked_add(scrollback)
            .ok_or_else(|| GhosttyError::AbiMismatch("selection point overflow".into()))?;
        if screen_y < 0 {
            return Ok(false);
        }
        let point = ghostty_point(
            sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
            usize::try_from(screen_y)
                .map_err(|_| GhosttyError::AbiMismatch("negative selection point".into()))?,
            point.column,
        )?;
        let mut contains = false;
        let result = unsafe {
            sys::ghostty_terminal_selection_contains(
                self.terminal.raw(),
                &selection,
                point,
                &mut contains,
            )
        };
        check("selection_contains", result)?;
        Ok(contains)
    }

    pub fn selection_equals(&self, range: &SelectionRange) -> Result<bool> {
        let Some(current) = self.current_selection()? else {
            return Ok(false);
        };
        let other = sys::GhosttySelection {
            size: std::mem::size_of::<sys::GhosttySelection>(),
            start: self.grid_ref(range.start)?,
            end: self.grid_ref(range.end)?,
            rectangle: range.rectangle,
        };
        let mut equal = false;
        let result = unsafe {
            sys::ghostty_terminal_selection_equal(
                self.terminal.raw(),
                &current,
                &other,
                &mut equal,
            )
        };
        check("selection_equal", result)?;
        Ok(equal)
    }

    pub fn selection_text_into(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        let options = sys::GhosttyTerminalSelectionFormatOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectionFormatOptions>(),
            emit: sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: true,
            trim: true,
            selection: std::ptr::null(),
        };
        let mut written = 0usize;
        let result = unsafe {
            sys::ghostty_terminal_selection_format_buf(
                self.terminal.raw(),
                options,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("selection_format_buf", result)?;
        if written > buffer.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "selection_format_buf reported {written} bytes for a {}-byte buffer",
                buffer.len()
            )));
        }
        Ok(Some(written))
    }

    pub(crate) fn current_selection(&self) -> Result<Option<sys::GhosttySelection>> {
        let mut selection = empty_selection();
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION,
                (&raw mut selection).cast(),
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("terminal_get_selection", result)?;
        Ok(Some(selection))
    }
}

pub(crate) fn empty_selection() -> sys::GhosttySelection {
    let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
    selection.size = std::mem::size_of::<sys::GhosttySelection>();
    selection
}

fn pointer_or_null(codepoints: &[u32]) -> *const u32 {
    if codepoints.is_empty() {
        std::ptr::null()
    } else {
        codepoints.as_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn select_all_covers_the_written_text() {
        let mut terminal = terminal(10, 3);
        terminal.feed(b"one\r\ntwo").expect("output must parse");
        assert!(terminal.select_all().expect("select all"));
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert!(text.contains("one"));
        assert!(text.contains("two"));
    }

    #[test]
    fn select_output_uses_the_shell_integration_marks() {
        let mut terminal = terminal(20, 6);
        terminal
            .feed(b"\x1b]133;A\x07$ \x1b]133;B\x07ls\x1b]133;C\x07\r\nfirst\r\nsecond\r\n\x1b]133;D;0\x07")
            .expect("output must parse");

        assert!(
            terminal
                .select_output(Point::new(1, 0))
                .expect("select output")
        );
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert!(text.contains("first"), "got {text:?}");
        assert!(text.contains("second"), "got {text:?}");
        assert!(!text.contains("$ ls"), "prompt must stay out: {text:?}");
    }

    #[test]
    fn select_output_reports_no_selection_without_marks() {
        let mut terminal = terminal(20, 4);
        terminal.feed(b"plain output").expect("output must parse");
        assert!(
            !terminal
                .select_output(Point::new(0, 0))
                .expect("select output")
        );
    }

    #[test]
    fn the_nearest_word_search_stops_at_the_first_word_it_finds() {
        let terminal = {
            let mut terminal = terminal(30, 2);
            terminal
                .feed(b"alpha beta gamma")
                .expect("output must parse");
            terminal
        };
        let forward = terminal
            .nearest_word_between(Point::new(0, 7), Point::new(0, 12), &[])
            .expect("forward probe")
            .expect("a word exists");
        assert_eq!(forward.start.column, 6);
        assert_eq!(forward.end.column, 9);

        let from_space = terminal
            .nearest_word_between(Point::new(0, 10), Point::new(0, 15), &[])
            .expect("probe from a space")
            .expect("a word exists");
        assert_eq!((from_space.start.column, from_space.end.column), (10, 10));
    }

    #[test]
    fn a_double_click_drag_unions_the_words_at_both_ends() {
        let mut terminal = terminal(30, 2);
        terminal
            .feed(b"alpha beta gamma")
            .expect("output must parse");
        assert!(
            terminal
                .select_words_between(Point::new(0, 7), Point::new(0, 12), &[])
                .expect("drag selection")
        );
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text.trim(), "beta gamma");
    }

    #[test]
    fn custom_boundaries_change_what_counts_as_a_word() {
        let mut terminal = terminal(30, 2);
        terminal.feed(b"src/main.rs").expect("output must parse");
        assert!(
            terminal
                .select_words_between(Point::new(0, 5), Point::new(0, 5), &['/'])
                .expect("drag selection")
        );
        let text = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert_eq!(text.trim(), "main.rs");
    }

    #[test]
    fn a_drag_across_blank_space_selects_nothing() {
        let mut terminal = terminal(30, 2);
        terminal.feed(b"word").expect("output must parse");
        assert!(
            !terminal
                .select_words_between(Point::new(0, 10), Point::new(0, 20), &[])
                .expect("drag selection")
        );
    }

    #[test]
    fn adjusting_a_selection_extends_it() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello world").expect("output must parse");
        assert!(terminal.select_word(Point::new(0, 0)).expect("select word"));
        let before = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");

        assert!(
            terminal
                .adjust_selection(SelectionAdjust::EndOfLine)
                .expect("adjust")
        );
        let after = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");
        assert!(after.len() > before.len(), "{before:?} -> {after:?}");
    }

    #[test]
    fn adjusting_without_a_selection_is_not_an_error() {
        let mut terminal = terminal(10, 2);
        terminal.feed(b"text").expect("output must parse");
        assert!(
            !terminal
                .adjust_selection(SelectionAdjust::Right)
                .expect("adjust")
        );
        assert!(terminal.selection_order().expect("order").is_none());
        assert!(
            !terminal
                .selection_contains(Point::new(0, 0))
                .expect("contains")
        );
    }

    #[test]
    fn order_reports_and_normalizes_a_reversed_selection() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello world").expect("output must parse");
        terminal
            .set_selection(SelectionRange {
                start: Point::new(0, 8),
                end: Point::new(0, 2),
                rectangle: false,
            })
            .expect("selection must install");
        assert_eq!(
            terminal.selection_order().expect("order"),
            Some(SelectionOrder::Reverse)
        );

        assert!(
            terminal
                .order_selection(SelectionOrder::Forward)
                .expect("reorder")
        );
        assert_eq!(
            terminal.selection_order().expect("order"),
            Some(SelectionOrder::Forward)
        );
    }

    #[test]
    fn containment_and_equality_use_the_terminal_geometry() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello world").expect("output must parse");
        let range = SelectionRange {
            start: Point::new(0, 0),
            end: Point::new(0, 4),
            rectangle: false,
        };
        terminal
            .set_selection(range.clone())
            .expect("selection must install");

        assert!(
            terminal
                .selection_contains(Point::new(0, 2))
                .expect("inside")
        );
        assert!(
            !terminal
                .selection_contains(Point::new(0, 9))
                .expect("outside")
        );
        assert!(terminal.selection_equals(&range).expect("same range"));
        assert!(
            !terminal
                .selection_equals(&SelectionRange {
                    start: Point::new(0, 0),
                    end: Point::new(0, 2),
                    rectangle: false,
                })
                .expect("different range")
        );
    }

    #[test]
    fn buffered_selection_text_matches_the_allocating_path() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"hello world").expect("output must parse");
        assert!(terminal.select_all().expect("select all"));
        let allocated = terminal
            .selection_text()
            .expect("selection text")
            .expect("a selection exists");

        let mut buffer = vec![0u8; allocated.len() + 32];
        let written = terminal
            .selection_text_into(&mut buffer)
            .expect("buffered path")
            .expect("a selection exists");
        assert_eq!(&buffer[..written], allocated.as_bytes());

        terminal.clear_selection().expect("clear");
        assert!(
            terminal
                .selection_text_into(&mut buffer)
                .expect("buffered path")
                .is_none()
        );
    }
}
