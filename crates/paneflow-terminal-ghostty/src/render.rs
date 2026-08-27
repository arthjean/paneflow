//! Partial-frame rendering: which rows changed, and direct access to one
//! cell.
//!
//! [`DisplayTerminal::snapshot`] rebuilds a whole frame and is what the GPUI
//! renderer consumes. These are the primitives underneath it, for callers
//! that want to redraw only what moved: a damage-tracking renderer, a
//! diagnostic overlay, or a test asserting that a write dirtied exactly the
//! rows it should have.

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::MAX_GRID_ROWS;
use crate::snapshot_ffi::{RenderDirty, render_get, render_row_data, render_row_iterator};
use crate::{Cell, GhosttyError, Result};

/// How much of the render state changed since it was last consumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirtyState {
    /// Nothing changed.
    Clean,
    /// Some rows changed; their per-row dirty flags say which.
    Partial,
    /// Everything must be redrawn.
    Full,
}

impl DisplayTerminal {
    /// Refresh the render state from the terminal.
    ///
    /// [`Self::snapshot`] does this itself; call it directly only when
    /// driving [`Self::dirty_rows`] or [`Self::render_cell`] without taking a
    /// snapshot.
    pub fn refresh_render_state(&mut self) -> Result<()> {
        // SAFETY: both handles are owned by `self`.
        let result = unsafe {
            sys::ghostty_render_state_begin_update(self.render_state.raw(), self.terminal.raw())
        };
        check("render_state_begin_update", result)?;
        // SAFETY: the render state is owned by `self` and a begin phase just
        // completed.
        let result = unsafe { sys::ghostty_render_state_end_update(self.render_state.raw()) };
        check("render_state_end_update", result)
    }

    /// How much of the current render state needs redrawing.
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

    /// The viewport rows that need a redraw, in ascending order.
    ///
    /// Empty when the state is clean; every row when it is fully dirty. This
    /// reads the flags without clearing them, so a caller that redraws from
    /// the result must follow with [`Self::mark_frame_clean`].
    pub fn dirty_rows(&mut self) -> Result<Vec<u16>> {
        let iterator = render_row_iterator(self.render_state.raw(), self.row_iterator.raw())?;
        let mut rows = Vec::new();
        let mut y = 0u16;
        // SAFETY: `iterator` is owned by `self` and positioned by the call
        // above; `y` is valid writable storage.
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

    /// Mark every dirty bit consumed, globally and per row.
    ///
    /// Call this only after a complete frame has been rendered. A partial
    /// consumer should clear the rows it drew instead, which is what
    /// [`Self::snapshot`] does on its incremental path.
    pub fn mark_frame_clean(&mut self) -> Result<()> {
        // SAFETY: the render state is owned by `self`.
        let result = unsafe { sys::ghostty_render_state_clean(self.render_state.raw()) };
        check("render_state_clean", result)
    }

    /// Read one cell of the current render state directly.
    ///
    /// Jumps the row-cells iterator straight to `column` rather than walking
    /// to it, which is what makes a point query cheap.
    pub fn render_cell(&mut self, row: u16, column: u16) -> Result<Cell> {
        let iterator = render_row_iterator(self.render_state.raw(), self.row_iterator.raw())?;
        let mut y = 0u16;
        loop {
            // SAFETY: `iterator` is owned by `self`.
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
        // SAFETY: `data.cells` is the row-cells handle libghostty just bound
        // to this row.
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
        // Row 2 took the text; row 0 is dirty too because the cursor left it.
        // What matters is that untouched rows stay out of the set.
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
