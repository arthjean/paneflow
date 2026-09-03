use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::MAX_GRID_ROWS;
use crate::snapshot_ffi::{RenderDirty, render_get, render_row_data, render_row_iterator};
use crate::{Cell, GhosttyError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyState {
    Clean,
    Partial,
    Full,
}

impl DisplayTerminal {
    pub fn refresh_render_state(&mut self) -> Result<()> {
        let result = unsafe {
            sys::ghostty_render_state_begin_update(self.render_state.raw(), self.terminal.raw())
        };
        check("render_state_begin_update", result)?;
        let result = unsafe { sys::ghostty_render_state_end_update(self.render_state.raw()) };
        check("render_state_end_update", result)
    }

    pub fn dirty_state(&self) -> Result<DirtyState> {
        match render_get::<RenderDirty>(self.render_state.raw())? {
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE => Ok(DirtyState::Clean),
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                Ok(DirtyState::Partial)
            }
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => Ok(DirtyState::Full),
            value => Err(GhosttyError::AbiMismatch(format!(
                "render state reported unknown dirty value {value}"
            ))),
        }
    }

    pub fn dirty_rows(&mut self) -> Result<Vec<u16>> {
        let iterator = render_row_iterator(self.render_state.raw(), self.row_iterator.raw())?;
        let mut rows = Vec::new();
        let mut y = 0u16;
        while unsafe { sys::ghostty_render_state_row_iterator_next_dirty(iterator, &mut y) } {
            if rows.len() > MAX_GRID_ROWS {
                return Err(GhosttyError::LimitExceeded {
                    resource: "dirty rows",
                    limit: MAX_GRID_ROWS,
                });
            }
            rows.push(y);
        }
        Ok(rows)
    }

    pub fn mark_frame_clean(&mut self) -> Result<()> {
        let result = unsafe { sys::ghostty_render_state_clean(self.render_state.raw()) };
        check("render_state_clean", result)
    }

    pub fn render_cell(&mut self, row: u16, column: u16) -> Result<Cell> {
        let iterator = render_row_iterator(self.render_state.raw(), self.row_iterator.raw())?;
        let mut y = 0u16;
        loop {
            if !unsafe { sys::ghostty_render_state_row_iterator_next(iterator) } {
                return Err(GhosttyError::Ffi {
                    operation: "render_cell_row_out_of_bounds",
                    code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
                });
            }
            if y == row {
                break;
            }
            y += 1;
        }
        let data = render_row_data(iterator, self.row_cells.raw())?;
        let result = unsafe { sys::ghostty_render_state_row_cells_select(data.cells, column) };
        check("render_state_row_cells_select", result)?;
        self.copy_cell(data.cells, usize::from(row), usize::from(column), false)
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
    fn only_the_rows_that_changed_are_reported_dirty() {
        let mut terminal = terminal(20, 4);
        terminal.snapshot().expect("first frame");

        terminal
            .feed(b"\x1b[3;1Hthird row")
            .expect("output must parse");
        terminal.refresh_render_state().expect("refresh");

        assert_eq!(
            terminal.dirty_state().expect("dirty state"),
            DirtyState::Partial
        );
        let dirty = terminal.dirty_rows().expect("dirty rows");
        assert!(dirty.contains(&2), "got {dirty:?}");
        assert!(!dirty.contains(&3), "got {dirty:?}");
    }

    #[test]
    fn a_clean_state_reports_no_dirty_rows() {
        let mut terminal = terminal(20, 4);
        terminal.feed(b"content").expect("output must parse");
        terminal.snapshot().expect("frame consumes the dirty state");

        terminal.refresh_render_state().expect("refresh");
        assert_eq!(terminal.dirty_state().expect("dirty state"), DirtyState::Clean);
        assert!(terminal.dirty_rows().expect("dirty rows").is_empty());
    }

    #[test]
    fn marking_a_frame_clean_consumes_every_dirty_row() {
        let mut terminal = terminal(20, 4);
        terminal.feed(b"one\r\ntwo\r\nthree").expect("output must parse");
        terminal.refresh_render_state().expect("refresh");
        assert!(!terminal.dirty_rows().expect("dirty rows").is_empty());

        terminal.mark_frame_clean().expect("clean");
        assert_eq!(terminal.dirty_state().expect("dirty state"), DirtyState::Clean);
        assert!(terminal.dirty_rows().expect("dirty rows").is_empty());
    }

    #[test]
    fn a_point_read_matches_the_same_cell_in_a_snapshot() {
        let mut terminal = terminal(20, 3);
        terminal
            .feed(b"ab\x1b[1mc\x1b[0m")
            .expect("output must parse");
        let snapshot = terminal.snapshot().expect("frame");

        let direct = terminal.render_cell(0, 2).expect("point read");
        let from_snapshot = &snapshot.cells[2];
        assert_eq!(direct.character, from_snapshot.character);
        assert_eq!(direct.character, 'c');
        assert_eq!(direct.flags, from_snapshot.flags);
        assert!(direct.flags.bold);
    }

    #[test]
    fn a_point_read_past_the_last_row_is_an_error() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"text").expect("output must parse");
        terminal.snapshot().expect("frame");
        assert!(terminal.render_cell(99, 0).is_err());
    }
}
