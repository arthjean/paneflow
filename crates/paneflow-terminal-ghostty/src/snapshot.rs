use std::sync::Arc;

use paneflow_libghostty_sys as sys;

use crate::abi::RawCellLayout;
use crate::callbacks::RenderSlot;
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::{MAX_OVERSCAN_ROWS, MAX_SNAPSHOT_CELLS};
use crate::snapshot_ffi::{
    RenderDirty, TerminalMouseShape, mouse_shape, render_get, render_overscan, render_row_data,
    render_row_identity, render_row_iterator, render_row_raw_cells, select_row_cell, terminal_get,
    terminal_scrollbar,
};
use crate::{
    Cell, Content, GhosttyError, Overscan, OverscanRow, Point, Result, RowIdentity, Scroll,
    SelectionRange,
};

#[derive(Default)]
pub(crate) struct SnapshotCache {
    cells: Arc<[Cell]>,
    raw: Vec<sys::GhosttyCell>,
    raw_layout: Option<RawCellLayout>,
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
    row_shift: isize,
    revision: u64,
}

impl SnapshotCache {
    pub(crate) fn new() -> Self {
        Self {
            raw_layout: crate::abi::raw_cell_layout(),
            ..Self::default()
        }
    }

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
        self.snapshot_cache.row_shift = 0;
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
        self.snapshot_cache.revision = self.snapshot_cache.revision.wrapping_add(1);

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
            row_shift: self.snapshot_cache.row_shift,
            revision: self.snapshot_cache.revision,
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
        let mut rebuilt_raw = rebuilt_cells
            .as_ref()
            .map(|_| Vec::<sys::GhosttyCell>::with_capacity(cell_count));
        let reuse = self
            .snapshot_cache
            .raw_layout
            .filter(|_| {
                in_place
                    && self.snapshot_cache.valid
                    && self.snapshot_cache.raw.len() == cell_count
                    && self.snapshot_cache.row_identities.len() == rows
            })
            .map(|layout| {
                self.viewport_row_shift(render_state, rows)
                    .map(|shift| (layout, shift))
            })
            .transpose()?;
        let previous_rows = reuse.map(|_| self.snapshot_cache.row_identities.clone());
        if let Some((_, shift)) = reuse.filter(|(_, shift)| *shift != 0) {
            self.shift_cached_rows(shift, cols)?;
        }
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
            let reused_from = reuse.filter(|_| !overscan_row).and_then(|(layout, shift)| {
                row_index
                    .checked_add_signed(shift)
                    .and_then(|source| previous_rows.as_ref()?.get(source))
                    .filter(|previous| previous.id == row.identity.id)
                    .map(|_| layout)
            });
            if let Some(layout) = reused_from {
                self.snapshot_cache.dirty_rows[row_index] =
                    self.reuse_row(iterator, row.cells, row_index, cols, row_selection, layout)?;
            } else if overscan_row || full_refresh || row.dirty {
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
                if !overscan_row && self.snapshot_cache.raw_layout.is_some() {
                    let fresh = render_row_raw_cells(iterator)?;
                    if let Some(raws) = rebuilt_raw.as_mut() {
                        raws.extend_from_slice(fresh);
                    } else if let Some(slot) = self.snapshot_cache.raw.get_mut(row_index * cols..)
                        && let Some(slot) = slot.get_mut(..fresh.len())
                    {
                        slot.copy_from_slice(fresh);
                    }
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
            self.snapshot_cache.cells = cells.into();
            self.snapshot_cache.raw = rebuilt_raw.unwrap_or_default();
            self.snapshot_cache.selection = selection;
            self.snapshot_cache.row_identities = row_identities;
            self.snapshot_cache.row_identity_scratch = row_identity_scratch;
            self.snapshot_cache.overscan = overscan;
            self.snapshot_cache.overscan_rows = overscan_rows;
            self.snapshot_cache.cols = cols;
            self.snapshot_cache.rows = rows;
            self.snapshot_cache.source = source;
            self.snapshot_cache.valid = true;
        } else {
            self.snapshot_cache.selection = selection;
            self.snapshot_cache.row_identities = row_identities;
            self.snapshot_cache.row_identity_scratch = row_identity_scratch;
            self.snapshot_cache.overscan = overscan;
            self.snapshot_cache.overscan_rows = overscan_rows;
            self.snapshot_cache.source = source;
            self.snapshot_cache.valid = true;
        }
        self.snapshot_cache.row_shift = reuse.map_or(0, |(_, shift)| shift);
        Ok(())
    }

    fn viewport_row_shift(
        &mut self,
        render_state: sys::GhosttyRenderState,
        rows: usize,
    ) -> Result<isize> {
        let iterator = render_row_iterator(render_state, self.row_iterator.raw())?;
        let previous = &self.snapshot_cache.row_identities;
        while unsafe { sys::ghostty_render_state_row_iterator_next(iterator) } {
            let identity = render_row_identity(iterator)?;
            let Some(row) = usize::try_from(identity.viewport_y)
                .ok()
                .filter(|row| *row < rows)
            else {
                continue;
            };
            if let Some(source) = previous.iter().position(|old| old.id == identity.id) {
                return isize::try_from(source)
                    .ok()
                    .zip(isize::try_from(row).ok())
                    .map(|(source, row)| source - row)
                    .ok_or_else(|| GhosttyError::AbiMismatch("row shift overflow".into()));
            }
        }
        Ok(0)
    }

    fn shift_cached_rows(&mut self, shift: isize, cols: usize) -> Result<()> {
        self.snapshot_cache.valid = false;
        let offset = shift.unsigned_abs().saturating_mul(cols);
        let cells = Arc::get_mut(&mut self.snapshot_cache.cells).ok_or_else(|| {
            GhosttyError::AbiMismatch("shifted rows need an unshared snapshot cache".into())
        })?;
        let raw = self.snapshot_cache.raw.as_mut_slice();
        if offset >= cells.len() || offset >= raw.len() {
            return Err(GhosttyError::AbiMismatch(
                "row shift exceeded the snapshot cache".into(),
            ));
        }
        let by = if shift > 0 {
            offset as isize
        } else {
            -(offset as isize)
        };
        rotate_cells(cells, by);
        rotate_cells(raw, by);
        Ok(())
    }

    fn reuse_row(
        &mut self,
        iterator: sys::GhosttyRenderStateRowIterator,
        row_cells: sys::GhosttyRenderStateRowCells,
        row: usize,
        cols: usize,
        selection: Option<(usize, usize)>,
        layout: RawCellLayout,
    ) -> Result<bool> {
        let fresh = render_row_raw_cells(iterator)?;
        if fresh.len() != cols {
            return Err(GhosttyError::AbiMismatch(format!(
                "render row returned {} raw cells, expected {cols}",
                fresh.len()
            )));
        }
        let line = i32::try_from(row)
            .map_err(|_| GhosttyError::AbiMismatch("snapshot row overflow".into()))?;
        let mut cells = std::mem::take(&mut self.snapshot_cache.cells);
        let mut raws = std::mem::take(&mut self.snapshot_cache.raw);
        let outcome = (|| {
            let start = row * cols;
            let (Some(cached), Some(cached_raw)) = (
                Arc::get_mut(&mut cells).and_then(|cells| cells.get_mut(start..start + cols)),
                raws.get_mut(start..start + cols),
            ) else {
                return Err(GhosttyError::AbiMismatch(
                    "reused row exceeded the snapshot cache".into(),
                ));
            };
            let mut changed = false;
            if selection.is_none() && layout.unchanged_and_plain(cached_raw, fresh) {
                for cell in cached.iter_mut() {
                    cell.point.line = line;
                    changed |= std::mem::take(&mut cell.selected);
                }
                return Ok(changed);
            }
            for (column, ((cell, previous_raw), &raw)) in
                cached.iter_mut().zip(cached_raw.iter_mut()).zip(fresh).enumerate()
            {
                let selected = selection.is_some_and(|(start, end)| (start..=end).contains(&column));
                cell.point = Point::new(line, column);
                if *previous_raw == raw && layout.is_plain(raw) {
                    if cell.selected != selected {
                        cell.selected = selected;
                        changed = true;
                    }
                    continue;
                }
                select_row_cell(row_cells, column)?;
                let copied = self.copy_cell(row_cells, row, column, selected)?;
                *previous_raw = raw;
                if *cell != copied {
                    *cell = copied;
                    changed = true;
                }
            }
            Ok(changed)
        })();
        self.snapshot_cache.cells = cells;
        self.snapshot_cache.raw = raws;
        outcome
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

pub fn rotate_cells<T>(cells: &mut [T], by: isize) {
    let len = cells.len();
    let count = by.unsigned_abs();
    if count == 0 || len == 0 {
        return;
    }
    if count >= len {
        let count = count % len;
        if by > 0 {
            cells.rotate_left(count);
        } else {
            cells.rotate_right(count);
        }
        return;
    }
    let mut displaced: Vec<T> = Vec::with_capacity(count);
    let base = cells.as_mut_ptr();
    unsafe {
        if by > 0 {
            std::ptr::copy_nonoverlapping(base, displaced.as_mut_ptr(), count);
            std::ptr::copy(base.add(count), base, len - count);
            std::ptr::copy_nonoverlapping(displaced.as_ptr(), base.add(len - count), count);
        } else {
            std::ptr::copy_nonoverlapping(base.add(len - count), displaced.as_mut_ptr(), count);
            std::ptr::copy(base, base.add(count), len - count);
            std::ptr::copy_nonoverlapping(displaced.as_ptr(), base, count);
        }
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

    enum Step {
        Feed(&'static [u8]),
        Scroll(crate::Scroll),
        Resize(usize, usize),
        SelectAll,
    }

    fn numbered_lines(count: usize) -> Vec<u8> {
        (0..count)
            .flat_map(|line| {
                format!("\x1b[38;5;{}mline {line:03}\x1b[0m plain words after the color\r\n", line % 256)
                    .into_bytes()
            })
            .collect()
    }

    fn apply(terminal: &mut DisplayTerminal, step: &Step) {
        match step {
            Step::Feed(bytes) => terminal.feed(bytes).expect("output must parse"),
            Step::Scroll(scroll) => terminal.scroll(*scroll),
            Step::Resize(cols, rows) => terminal
                .resize(WindowSize::new(*cols, *rows, 8, 16).expect("valid terminal size"))
                .expect("resize must apply"),
            Step::SelectAll => {
                terminal.select_all().expect("selection must apply");
            }
        }
    }

    #[test]
    fn shifted_rows_publish_the_same_content_as_a_full_copy() {
        let mut reused = terminal(40, 8);
        let mut copied = terminal(40, 8);
        let history = numbered_lines(30);
        let steps = [
            Step::Feed(b"\x1b[31mfirst\x1b[0m"),
            Step::Feed(b" tail\r\n"),
            Step::Feed(b"one more line\r\n"),
            Step::Feed(b"a\r\nb\r\nc\r\n"),
            Step::Feed(b"\x1b[7;1H\x1b[32mrecolored\x1b[0m\r\n\r\n"),
            Step::Feed(b"\x1b[1Ae\xcc\x81 combining\r\n"),
            Step::Feed(b"\x1b[33mX\x1b[0m\r\x1b[34mX\x1b[0m\r\n"),
            Step::Feed(b"\x1b[3;6r\x1b[6;1Hregion\r\nscrolls\r\n\x1b[r"),
            Step::Scroll(crate::Scroll::Delta(-3)),
            Step::Feed(b"while scrolled back\r\n"),
            Step::Scroll(crate::Scroll::Delta(1)),
            Step::Scroll(crate::Scroll::Delta(5)),
            Step::Scroll(crate::Scroll::Bottom),
            Step::SelectAll,
            Step::Feed(b"selected output\r\n"),
            Step::Feed(b"\x1b[?1049halternate\r\nscreen\r\n"),
            Step::Feed(b"\x1b[?1049l"),
            Step::Feed(b"\x1b[2J\x1b[Hcleared\r\n"),
            Step::Resize(30, 6),
            Step::Feed(b"after resize\r\nmore\r\n"),
            Step::Feed(b"\xe4\xb8\xad\xe6\x96\x87 wide\r\n"),
        ];
        for terminal in [&mut reused, &mut copied] {
            terminal.feed(&history).expect("history must parse");
            terminal.snapshot().expect("first frame");
        }
        let mut shifted = 0usize;
        for (index, step) in steps.iter().enumerate() {
            apply(&mut reused, step);
            apply(&mut copied, step);
            let frame = reused.snapshot().expect("reused frame");
            copied.snapshot_cache.invalidate();
            let expected = copied.snapshot().expect("copied frame");
            assert_eq!(frame.cells, expected.cells, "step {index}");
            assert_eq!(frame.cursor, expected.cursor, "step {index}");
            assert_eq!(frame.selection, expected.selection, "step {index}");
            if frame.row_shift != 0 {
                shifted += 1;
                assert!(
                    frame.dirty_rows.iter().any(|dirty| !dirty),
                    "step {index} shifted by {} but reported every row dirty",
                    frame.row_shift
                );
            }
        }
        assert!(shifted >= 4, "only {shifted} steps reused shifted rows");
    }

    #[test]
    fn a_one_line_scroll_reports_the_shift_and_only_the_new_rows() {
        let mut terminal = terminal(220, 60);
        terminal.feed(&numbered_lines(200)).expect("history must parse");
        let before = terminal.snapshot().expect("first frame");
        let (revision, second_row) = (before.revision, before.row_identities[1].id);
        let (history_size, display_offset) = (before.history_size, before.display_offset);
        drop(before);

        terminal.feed(b"next line\r\n").expect("output must parse");
        let after = terminal.snapshot().expect("second frame");

        assert_eq!(after.row_shift, 1);
        assert_eq!(after.revision, revision + 1);
        assert_eq!(
            (after.history_size, after.display_offset),
            (history_size + 1, display_offset)
        );
        assert_eq!(display_offset, 0);
        let dirty: Vec<usize> = (0..60).filter(|row| after.dirty_rows[*row]).collect();
        assert_eq!(dirty, [58, 59]);
        assert_eq!(after.row_identities[0].id, second_row);
    }

    #[test]
    fn a_pruned_scrollback_page_still_reports_a_one_line_shift() {
        let mut terminal = terminal(220, 60);
        terminal.feed(&numbered_lines(200)).expect("history must parse");
        let mut history_size = terminal.snapshot().expect("first frame").history_size;
        let pruned = (0..1_000).any(|_| {
            terminal.feed(b"next line\r\n").expect("output must parse");
            let frame = terminal.snapshot().expect("scrolled frame");
            assert_eq!(frame.row_shift, 1, "history {history_size}");
            let pruned = frame.history_size < history_size;
            history_size = frame.history_size;
            pruned
        });
        assert!(pruned, "the scrollback never pruned a page");
    }

    #[test]
    fn rotating_cells_matches_the_standard_rotation() {
        for len in [0usize, 1, 5, 12] {
            for by in -14isize..=14 {
                let mut moved: Vec<String> = (0..len).map(|i| i.to_string()).collect();
                let mut expected = moved.clone();
                if len > 0 {
                    let count = by.unsigned_abs() % len;
                    if by > 0 {
                        expected.rotate_left(count);
                    } else {
                        expected.rotate_right(count);
                    }
                }
                super::rotate_cells(&mut moved, by);
                assert_eq!(moved, expected, "len {len} by {by}");
            }
        }
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
