use std::sync::Arc;

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::MAX_SNAPSHOT_CELLS;
use crate::snapshot_ffi::{
    RenderDirty, render_get, render_row_data, render_row_iterator, terminal_scrollbar,
};
use crate::{Cell, Content, GhosttyError, Point, Result, Scroll, SelectionRange};

#[derive(Default)]
pub(crate) struct SnapshotCache {
    cells: Arc<[Cell]>,
    selection: Option<SelectionRange>,
    cols: usize,
    rows: usize,
    valid: bool,
}

impl SnapshotCache {
    pub(crate) fn invalidate(&mut self) {
        self.valid = false;
    }

    fn matches(&self, cols: usize, rows: usize, cell_count: usize) -> bool {
        self.valid && self.cols == cols && self.rows == rows && self.cells.len() == cell_count
    }
}

impl DisplayTerminal {
    pub fn snapshot(&mut self) -> Result<Content> {
        let result = unsafe {
            sys::ghostty_render_state_update(self.render_state.raw(), self.terminal.raw())
        };
        check("render_state_update", result)?;
        let (history_size, display_offset) = self.scrollbar_position()?;
        let display_offset_i32 = i32::try_from(display_offset)
            .map_err(|_| crate::GhosttyError::AbiMismatch("display offset overflow".into()))?;
        let (cols, rows) = self.render_dimensions()?;
        let cell_count = cols.checked_mul(rows).ok_or_else(|| {
            crate::GhosttyError::AbiMismatch("snapshot cell count overflow".into())
        })?;
        if cell_count > MAX_SNAPSHOT_CELLS {
            return Err(crate::GhosttyError::LimitExceeded {
                resource: "snapshot cells",
                limit: MAX_SNAPSHOT_CELLS,
            });
        }

        let dirty = render_get::<RenderDirty>(self.render_state.raw())?;
        let full_refresh = match dirty {
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE
            | sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                !self.snapshot_cache.matches(cols, rows, cell_count)
            }
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FULL => true,
            value => {
                return Err(GhosttyError::AbiMismatch(format!(
                    "render state reported unknown dirty value {value}"
                )));
            }
        };
        if full_refresh || dirty == sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL
        {
            self.refresh_snapshot_cache(cols, rows, cell_count, full_refresh)?;
        }
        if !self.snapshot_cache.matches(cols, rows, cell_count) {
            return Err(GhosttyError::AbiMismatch(
                "render state was clean before the snapshot cache was initialized".into(),
            ));
        }
        self.clear_render_dirty()?;

        let cells = self.snapshot_cache.cells.clone();
        let selection = self
            .snapshot_cache
            .selection
            .as_ref()
            .map(|selection| {
                let start_line = selection
                    .start
                    .line
                    .checked_sub(display_offset_i32)
                    .ok_or_else(|| GhosttyError::AbiMismatch("selection start overflow".into()))?;
                let end_line = selection
                    .end
                    .line
                    .checked_sub(display_offset_i32)
                    .ok_or_else(|| GhosttyError::AbiMismatch("selection end overflow".into()))?;
                Ok(SelectionRange {
                    start: Point::new(start_line, selection.start.column),
                    end: Point::new(end_line, selection.end.column),
                    rectangle: selection.rectangle,
                })
            })
            .transpose()?;
        Ok(Content {
            cells,
            cursor: self.cursor(display_offset)?,
            selection,
            cols,
            rows,
            display_offset,
            history_size,
        })
    }

    /// Move to a viewport row measured from the top of the scrollable area.
    /// This matches Ghostty's scrollbar offset space, so output that extends
    /// history cannot move an in-progress scrollbar gesture to newer content.
    pub fn scroll_to_viewport_row(&mut self, row: usize) -> Result<()> {
        let (history_size, current) = self.scrollbar_position()?;
        let target = history_size.saturating_sub(row.min(history_size));
        let current = i32::try_from(current)
            .map_err(|_| GhosttyError::AbiMismatch("display offset overflow".into()))?;
        let target = i32::try_from(target)
            .map_err(|_| GhosttyError::AbiMismatch("display offset target overflow".into()))?;
        let delta = target - current;
        if delta != 0 {
            self.scroll(Scroll::Delta(delta));
        }
        Ok(())
    }

    fn refresh_snapshot_cache(
        &mut self,
        cols: usize,
        rows: usize,
        cell_count: usize,
        full_refresh: bool,
    ) -> Result<()> {
        let mut rebuilt_cells = full_refresh.then(|| {
            self.snapshot_cache.valid = false;
            Vec::with_capacity(cell_count)
        });

        let iterator = render_row_iterator(self.render_state.raw(), self.row_iterator.raw())?;

        let mut row_index = 0usize;
        let mut selection_start = None;
        let mut selection_end = None;
        while unsafe { sys::ghostty_render_state_row_iterator_next(iterator) } {
            if row_index >= rows {
                return Err(GhosttyError::AbiMismatch(
                    "render iterator returned too many rows".into(),
                ));
            }

            let row = render_row_data(iterator, self.row_cells.raw())?;
            let row_selection = if let Some(selection) = row.selection {
                let start = usize::from(selection.start_x.min(selection.end_x));
                let end = usize::from(selection.start_x.max(selection.end_x));
                if end >= cols {
                    return Err(GhosttyError::AbiMismatch(
                        "render selection exceeded snapshot columns".into(),
                    ));
                }
                Some((start, end))
            } else {
                None
            };
            if let Some((start, end)) = row_selection {
                let line = i32::try_from(row_index)
                    .map_err(|_| GhosttyError::AbiMismatch("selection row overflow".into()))?;
                selection_start.get_or_insert(Point::new(line, start));
                selection_end = Some(Point::new(line, end));
            }
            if full_refresh || row.dirty {
                let mut column = 0usize;
                while unsafe { sys::ghostty_render_state_row_cells_next(row.cells) } {
                    if column >= cols {
                        return Err(GhosttyError::AbiMismatch(
                            "render iterator returned too many columns".into(),
                        ));
                    }
                    let selected =
                        row_selection.is_some_and(|(start, end)| (start..=end).contains(&column));
                    let cell = self.copy_cell(row.cells, row_index, column, selected)?;
                    if let Some(cells) = rebuilt_cells.as_mut() {
                        cells.push(cell);
                    } else {
                        let cell_index = row_index * cols + column;
                        let cached = Arc::make_mut(&mut self.snapshot_cache.cells)
                            .get_mut(cell_index)
                            .ok_or_else(|| {
                                GhosttyError::AbiMismatch(
                                    "partial render update exceeded the snapshot cache".into(),
                                )
                            })?;
                        *cached = cell;
                    }
                    column += 1;
                }
                if column != cols {
                    return Err(GhosttyError::AbiMismatch(format!(
                        "render row returned {column} columns, expected {cols}"
                    )));
                }
            }

            if full_refresh || row.dirty {
                let clean = false;
                let result = unsafe {
                    sys::ghostty_render_state_row_set(
                        iterator,
                        sys::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                        (&clean as *const bool).cast(),
                    )
                };
                check("render_state_row_set", result)?;
            }
            row_index += 1;
        }
        if row_index != rows {
            return Err(GhosttyError::AbiMismatch(format!(
                "render iterator returned {row_index} rows, expected {rows}"
            )));
        }

        let selection = match selection_start.zip(selection_end) {
            Some((start, end)) => Some(SelectionRange {
                start,
                end,
                rectangle: self.selection_rectangle()?.unwrap_or(false),
            }),
            None => None,
        };

        if let Some(cells) = rebuilt_cells {
            if cells.len() != cell_count {
                return Err(GhosttyError::AbiMismatch(format!(
                    "render iterator returned {} cells, expected {cell_count}",
                    cells.len()
                )));
            }
            self.snapshot_cache = SnapshotCache {
                cells: cells.into(),
                selection,
                cols,
                rows,
                valid: true,
            };
        } else {
            self.snapshot_cache.selection = selection;
        }
        Ok(())
    }

    fn clear_render_dirty(&self) -> Result<()> {
        let clean = sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        let result = unsafe {
            sys::ghostty_render_state_set(
                self.render_state.raw(),
                sys::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&clean as *const sys::GhosttyRenderStateDirty).cast(),
            )
        };
        check("render_state_set", result)
    }

    fn scrollbar(&self) -> Result<sys::GhosttyTerminalScrollbar> {
        terminal_scrollbar(self.terminal.raw())
    }

    fn scrollbar_position(&self) -> Result<(usize, usize)> {
        let scrollbar = self.scrollbar()?;
        let history_size = scrollbar
            .total
            .checked_sub(scrollbar.len)
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| GhosttyError::AbiMismatch("invalid scrollbar length".into()))?;
        let scrollbar_offset = usize::try_from(scrollbar.offset)
            .map_err(|_| GhosttyError::AbiMismatch("scrollbar offset overflow".into()))?;
        let display_offset = history_size
            .checked_sub(scrollbar_offset)
            .ok_or_else(|| GhosttyError::AbiMismatch("scrollbar offset exceeds history".into()))?;
        Ok((history_size, display_offset))
    }
}
