use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Point, Result, SelectionRange};

impl DisplayTerminal {
    pub fn select_all(&mut self) -> Result<bool> {
        let mut selection = empty_selection();
        let result =
            unsafe { sys::ghostty_terminal_select_all(self.terminal.raw(), &mut selection) };
        self.install_optional_selection(result, &selection)
    }

    pub fn selection_range(&self) -> Result<Option<SelectionRange>> {
        self.current_selection()?
            .as_ref()
            .map(|selection| self.selection_range_of(selection))
            .transpose()
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
}
