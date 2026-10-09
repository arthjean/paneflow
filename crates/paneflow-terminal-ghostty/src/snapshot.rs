use std::sync::Arc;

use paneflow_libghostty_sys as sys;

use crate::callbacks::RenderSlot;
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::{MAX_OVERSCAN_ROWS, MAX_SNAPSHOT_CELLS};
use crate::snapshot_ffi::{
    RenderDirty, TerminalMouseShape, mouse_shape, render_get, render_overscan, render_row_data,
    render_row_iterator, terminal_get, terminal_scrollbar,
};
use crate::{
    Cell, Content, GhosttyError, Overscan, OverscanRow, Point, Result, RowIdentity, Scroll,
    SelectionRange,
};

#[derive(Default)]
pub(crate) struct SnapshotCache {
    cells: Arc<[Cell]>,
    dirty_rows: Vec<bool>,
    selection: Option<SelectionRange>,
    row_identities: Arc<[RowIdentity]>,
    row_identity_scratch: Vec<RowIdentity>,
    overscan: Overscan,
    overscan_rows: Arc<[OverscanRow]>,
    cols: usize,
    rows: usize,
    source: RenderSlot,
    valid: bool,
}

impl SnapshotCache {
    pub(crate) fn invalidate(&mut self) {
        self.valid = false;
    }

    fn matches(&self, source: RenderSlot, cols: usize, rows: usize, cell_count: usize) -> bool {
        self.valid
            && self.source == source
            && self.cols == cols
            && self.rows == rows
            && self.cells.len() == cell_count
    }
}

impl DisplayTerminal {
    pub fn snapshot(&mut self) -> Result<Content> {
        self.snapshot_frame(RenderSlot::Main, self.callbacks.render_hold().is_none())
    }

    pub fn snapshot_live(&mut self) -> Result<Content> {
        if self.callbacks.render_hold().is_some() {
            self.snapshot_frame(RenderSlot::LiveDuringHold, true)
        } else {
            self.snapshot_frame(RenderSlot::Main, true)
        }
    }

    pub fn set_overscan(&mut self, above: u16, below: u16) -> Result<()> {
        if above > MAX_OVERSCAN_ROWS || below > MAX_OVERSCAN_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "overscan rows",
                limit: usize::from(MAX_OVERSCAN_ROWS),
            });
        }
        self.callbacks
            .set_overscan_request(Overscan { above, below })?;
        self.snapshot_cache.invalidate();
        Ok(())
    }

    pub fn overscan_request(&self) -> Result<Overscan> {
        render_overscan(
            self.callbacks.render_state(),
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_OVERSCAN_REQUEST,
        )
    }

    fn snapshot_frame(&mut self, source: RenderSlot, update: bool) -> Result<Content> {
        let (render_state, scrollbar) = match self.callbacks.rendered_scrollbar() {
            Some(scrollbar) if !update => (self.callbacks.render_state(), scrollbar),
            _ => {
                let render_state = self.callbacks.claim_terminal_dirt(source)?;
                let result = unsafe {
                    sys::ghostty_render_state_begin_update(render_state, self.terminal.raw())
                };
                check("render_state_begin_update", result)?;
                let scrollbar = self.scrollbar()?;
                let result = unsafe { sys::ghostty_render_state_end_update(render_state) };
                check("render_state_end_update", result)?;
                if source == RenderSlot::Main {
                    self.callbacks.record_rendered_scrollbar(scrollbar);
                }
                (render_state, scrollbar)
            }
        };
        let (history_size, display_offset) = scrollbar_position(scrollbar)?;
        let display_offset_i32 = i32::try_from(display_offset)
            .map_err(|_| crate::GhosttyError::AbiMismatch("display offset overflow".into()))?;
        let (cols, rows) = self.render_dimensions(render_state)?;
        let cell_count = cols.checked_mul(rows).ok_or_else(|| {
            crate::GhosttyError::AbiMismatch("snapshot cell count overflow".into())
        })?;
        if cell_count > MAX_SNAPSHOT_CELLS {
            return Err(crate::GhosttyError::LimitExceeded {
                resource: "snapshot cells",
                limit: MAX_SNAPSHOT_CELLS,
            });
        }

        let dirty = render_get::<RenderDirty>(render_state)?;
        let full_refresh = match dirty {
            sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE
            | sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_PARTIAL => {
                !self.snapshot_cache.matches(source, cols, rows, cell_count)
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
            self.refresh_snapshot_cache(render_state, source, cols, rows, cell_count, full_refresh)?;
        } else {
            self.snapshot_cache.dirty_rows.fill(false);
        }
        if !self.snapshot_cache.matches(source, cols, rows, cell_count) {
            return Err(GhosttyError::AbiMismatch(
                "render state was clean before the snapshot cache was initialized".into(),
            ));
        }
        if full_refresh {
            let result = unsafe { sys::ghostty_render_state_clean(render_state) };
            check("render_state_clean", result)?;
        } else {
            self.clear_render_dirty(render_state)?;
        }

        let cells = self.snapshot_cache.cells.clone();
        let dirty_rows: Arc<[bool]> = self.snapshot_cache.dirty_rows.as_slice().into();
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
            dirty_rows,
            cursor: self.cursor(render_state, display_offset)?,
            selection,
            cols,
            rows,
            display_offset,
            history_size,
            mouse_shape: mouse_shape(terminal_get::<TerminalMouseShape>(self.terminal.raw())?),
            row_identities: self.snapshot_cache.row_identities.clone(),
            overscan: self.snapshot_cache.overscan,
            overscan_rows: self.snapshot_cache.overscan_rows.clone(),
        })
    }

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
        render_state: sys::GhosttyRenderState,
        source: RenderSlot,
        cols: usize,
        rows: usize,
        cell_count: usize,
        full_refresh: bool,
    ) -> Result<()> {
        let in_place = full_refresh
            && self.snapshot_cache.cols == cols
            && self.snapshot_cache.rows == rows
            && self.snapshot_cache.cells.len() == cell_count
            && Arc::get_mut(&mut self.snapshot_cache.cells).is_some();
        let mut rebuilt_cells = (full_refresh && !in_place).then(|| {
            self.snapshot_cache.valid = false;
            Vec::with_capacity(cell_count)
        });
        self.snapshot_cache.dirty_rows.clear();
        self.snapshot_cache.dirty_rows.resize(rows, false);
        let overscan = render_overscan(
            render_state,
            sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_OVERSCAN,
        )?;
        let mut row_identities = std::mem::take(&mut self.snapshot_cache.row_identity_scratch);
        row_identities.clear();
        let mut overscan_rows = Vec::with_capacity(usize::from(overscan.above + overscan.below));

        let iterator = render_row_iterator(render_state, self.row_iterator.raw())?;

        let mut row_index = 0usize;
        let mut previous_y = None;
        let mut selection_start = None;
        let mut selection_end = None;
        while unsafe { sys::ghostty_render_state_row_iterator_next(iterator) } {
            let row = render_row_data(iterator, self.row_cells.raw())?;
            let viewport_y = row.identity.viewport_y;
            if previous_y.is_some_and(|previous: i32| previous.checked_add(1) != Some(viewport_y)) {
                return Err(GhosttyError::AbiMismatch(format!(
                    "render iterator jumped from row {previous_y:?} to {viewport_y}"
                )));
            }
            previous_y = Some(viewport_y);
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
            let overscan_row = usize::try_from(viewport_y)
                .ok()
                .filter(|y| *y < rows)
                .is_none();
            if !overscan_row {
                if usize::try_from(viewport_y).ok() != Some(row_index) {
                    return Err(GhosttyError::AbiMismatch(format!(
                        "render iterator returned viewport row {viewport_y}, expected {row_index}"
                    )));
                }
                row_identities.push(row.identity);
                if let Some((start, end)) = row_selection {
                    selection_start.get_or_insert(Point::new(viewport_y, start));
                    selection_end = Some(Point::new(viewport_y, end));
                }
            }
            if overscan_row || full_refresh || row.dirty {
                let mut overscan_cells = overscan_row.then(|| Vec::with_capacity(cols));
                let cell_row = if overscan_row { 0 } else { row_index };
                if !overscan_row {
                    self.snapshot_cache.dirty_rows[row_index] = true;
                }
                let mut column = 0usize;
                while unsafe { sys::ghostty_render_state_row_cells_next(row.cells) } {
                    if column >= cols {
                        return Err(GhosttyError::AbiMismatch(
                            "render iterator returned too many columns".into(),
                        ));
                    }
                    let selected =
                        row_selection.is_some_and(|(start, end)| (start..=end).contains(&column));
                    let mut cell = self.copy_cell(row.cells, cell_row, column, selected)?;
                    if let Some(cells) = overscan_cells.as_mut() {
                        cell.point.line = viewport_y;
                        cells.push(cell);
                    } else if let Some(cells) = rebuilt_cells.as_mut() {
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
                if let Some(cells) = overscan_cells {
                    overscan_rows.push(OverscanRow {
                        identity: row.identity,
                        cells: cells.into(),
                    });
                }
            }

            if !full_refresh && row.dirty {
                self.clear_row_dirty(iterator)?;
            }
            if !overscan_row {
                row_index += 1;
            }
        }
        if row_index != rows {
            return Err(GhosttyError::AbiMismatch(format!(
                "render iterator returned {row_index} rows, expected {rows}"
            )));
        }
        if overscan_rows.len() != usize::from(overscan.above + overscan.below) {
            return Err(GhosttyError::AbiMismatch(format!(
                "render iterator returned {} overscan rows, the update reported {overscan:?}",
                overscan_rows.len()
            )));
        }
        let row_identity_scratch = row_identities;
        let row_identities =
            share_row_identities(&mut self.snapshot_cache.row_identities, &row_identity_scratch);
        let overscan_rows = if overscan_rows.is_empty() && self.snapshot_cache.overscan_rows.is_empty()
        {
            self.snapshot_cache.overscan_rows.clone()
        } else {
            overscan_rows.into()
        };

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
                dirty_rows: std::mem::take(&mut self.snapshot_cache.dirty_rows),
                selection,
                row_identities,
                row_identity_scratch,
                overscan,
                overscan_rows,
                cols,
                rows,
                source,
                valid: true,
            };
        } else {
            self.snapshot_cache.selection = selection;
            self.snapshot_cache.row_identities = row_identities;
            self.snapshot_cache.row_identity_scratch = row_identity_scratch;
            self.snapshot_cache.overscan = overscan;
            self.snapshot_cache.overscan_rows = overscan_rows;
            self.snapshot_cache.source = source;
            self.snapshot_cache.valid = true;
        }
        Ok(())
    }

    fn clear_row_dirty(&self, iterator: sys::GhosttyRenderStateRowIterator) -> Result<()> {
        let clean = false;
        let result = unsafe {
            sys::ghostty_render_state_row_set(
                iterator,
                sys::GhosttyRenderStateRowOption_GHOSTTY_RENDER_STATE_ROW_OPTION_DIRTY,
                (&raw const clean).cast(),
            )
        };
        check("render_state_row_set", result)
    }

    fn clear_render_dirty(&self, render_state: sys::GhosttyRenderState) -> Result<()> {
        let clean = sys::GhosttyRenderStateDirty_GHOSTTY_RENDER_STATE_DIRTY_FALSE;
        let result = unsafe {
            sys::ghostty_render_state_set(
                render_state,
                sys::GhosttyRenderStateOption_GHOSTTY_RENDER_STATE_OPTION_DIRTY,
                (&clean as *const sys::GhosttyRenderStateDirty).cast(),
            )
        };
        check("render_state_set", result)
    }

    fn scrollbar(&self) -> Result<sys::GhosttyTerminalScrollbar> {
        terminal_scrollbar(self.terminal.raw())
    }

    pub(crate) fn scrollbar_position(&self) -> Result<(usize, usize)> {
        scrollbar_position(self.scrollbar()?)
    }
}

fn share_row_identities(
    cached: &mut Arc<[RowIdentity]>,
    fresh: &[RowIdentity],
) -> Arc<[RowIdentity]> {
    if let Some(slot) = Arc::get_mut(cached).filter(|slot| slot.len() == fresh.len()) {
        slot.copy_from_slice(fresh);
        return cached.clone();
    }
    if **cached == *fresh {
        return cached.clone();
    }
    fresh.into()
}

fn scrollbar_position(scrollbar: sys::GhosttyTerminalScrollbar) -> Result<(usize, usize)> {
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

#[cfg(test)]
mod tests {
    use crate::{DisplayTerminal, TerminalAppearance, WindowSize};

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
        let frame = terminal.snapshot().expect("second frame");

        assert!(frame.dirty_rows[2], "got {:?}", frame.dirty_rows);
        assert!(!frame.dirty_rows[3], "got {:?}", frame.dirty_rows);
    }

    #[test]
    fn a_snapshot_consumes_every_dirty_row() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"one\r\ntwo\r\nthree")
            .expect("output must parse");
        let first = terminal.snapshot().expect("first frame");
        assert!(first.dirty_rows.iter().any(|dirty| *dirty));

        let second = terminal.snapshot().expect("second frame");
        assert!(second.dirty_rows.iter().all(|dirty| !dirty));
    }
}
