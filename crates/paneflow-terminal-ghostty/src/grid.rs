use paneflow_libghostty_sys as sys;

use crate::engine::{DisplayTerminal, get_terminal};
use crate::handles::check;
use crate::limits::{MAX_GRID_CELLS, MAX_GRID_ROWS, MAX_SCROLLBACK_ROWS};
use crate::snapshot_ffi::{ghostty_point, raw_cell_get};
use crate::{GhosttyError, Point, Result};

const MAX_GRAPHEME_CODEPOINTS: usize = 1024;

pub(crate) struct GridLine {
    pub(crate) line: i32,
    pub(crate) text: String,
    pub(crate) char_to_column: Vec<usize>,
}

impl DisplayTerminal {
    pub(crate) fn grid_ref(&self, point: Point) -> Result<sys::GhosttyGridRef> {
        let scrollback = i64::try_from(self.scrollback_rows()?)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i64".into()))?;
        let screen_y = i64::from(point.line)
            .checked_add(scrollback)
            .ok_or_else(|| GhosttyError::AbiMismatch("grid point overflow".into()))?;
        if screen_y < 0 {
            return Err(GhosttyError::Ffi {
                operation: "grid_point_out_of_bounds",
                code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        let point = ghostty_point(
            sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN,
            usize::try_from(screen_y)
                .map_err(|_| GhosttyError::AbiMismatch("negative grid point".into()))?,
            point.column,
        )?;
        let mut reference: sys::GhosttyGridRef = unsafe { std::mem::zeroed() };
        reference.size = std::mem::size_of::<sys::GhosttyGridRef>();
        let result =
            unsafe { sys::ghostty_terminal_grid_ref(self.terminal.raw(), point, &mut reference) };
        check("terminal_grid_ref", result)?;
        Ok(reference)
    }

    /// Read multiple logical grid lines from the live terminal in one call.
    /// Logical line zero is the first viewport row and negative lines address
    /// scrollback, matching the coordinates returned by [`Self::search`].
    pub fn line_texts(&self, lines: &[i32]) -> Result<Vec<(i32, String)>> {
        if lines.is_empty() {
            return Ok(Vec::new());
        }
        let total_rows = self.total_rows()?;
        let scrollback = self.scrollback_rows()?;
        let cols = self.cols()?;
        check_grid_cell_count(lines.len(), cols)?;
        let total_rows = i64::try_from(total_rows)
            .map_err(|_| GhosttyError::AbiMismatch("total rows do not fit i64".into()))?;
        let scrollback_i64 = i64::try_from(scrollback)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i64".into()))?;
        let scrollback = i32::try_from(scrollback)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i32".into()))?;
        let mut result = Vec::with_capacity(lines.len());
        for &line in lines {
            let screen_y = i64::from(line)
                .checked_add(scrollback_i64)
                .ok_or_else(|| GhosttyError::AbiMismatch("grid line overflow".into()))?;
            if screen_y < 0 || screen_y >= total_rows {
                return Err(GhosttyError::Ffi {
                    operation: "grid_line_out_of_bounds",
                    code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
                });
            }
            let grid_line = self.grid_line_at_screen_row(
                usize::try_from(screen_y)
                    .map_err(|_| GhosttyError::AbiMismatch("negative grid line".into()))?,
                scrollback,
                cols,
            )?;
            result.push((grid_line.line, grid_line.text));
        }
        Ok(result)
    }

    pub(crate) fn grid_lines(
        &self,
        range: Option<std::ops::Range<usize>>,
    ) -> Result<Vec<GridLine>> {
        let total_rows = self.total_rows()?;
        let scrollback = self.scrollback_rows()?;
        let cols = self.cols()?;
        let range = range.unwrap_or(0..total_rows);
        if range.start > range.end || range.end > total_rows {
            return Err(GhosttyError::Ffi {
                operation: "grid_range_out_of_bounds",
                code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        check_grid_cell_count(range.len(), cols)?;
        let scrollback = i32::try_from(scrollback)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i32".into()))?;
        let mut lines = Vec::with_capacity(range.len());
        for y in range {
            lines.push(self.grid_line_at_screen_row(y, scrollback, cols)?);
        }
        Ok(lines)
    }

    fn grid_line_at_screen_row(&self, y: usize, scrollback: i32, cols: usize) -> Result<GridLine> {
        let mut text = String::with_capacity(cols);
        let mut char_to_column = Vec::with_capacity(cols);
        for column in 0..cols {
            let point = ghostty_point(sys::GhosttyPointTag_GHOSTTY_POINT_TAG_SCREEN, y, column)?;
            let mut reference: sys::GhosttyGridRef = unsafe { std::mem::zeroed() };
            reference.size = std::mem::size_of::<sys::GhosttyGridRef>();
            let result = unsafe {
                sys::ghostty_terminal_grid_ref(self.terminal.raw(), point, &mut reference)
            };
            check("terminal_grid_ref", result)?;
            let mut cell = 0u64;
            let result = unsafe { sys::ghostty_grid_ref_cell(&reference, &mut cell) };
            check("grid_ref_cell", result)?;
            let mut wide = sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_NARROW;
            // SAFETY: `cell` was returned by `ghostty_grid_ref_cell` for this
            // live reference, and WIDE writes a GhosttyCellWide.
            unsafe { raw_cell_get(cell, sys::GhosttyCellData_GHOSTTY_CELL_DATA_WIDE, &mut wide) }?;
            if wide == sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL {
                continue;
            }
            let grapheme = grid_ref_grapheme(&reference)?;
            let grapheme = if grapheme.is_empty() {
                " ".to_owned()
            } else {
                grapheme
            };
            for character in grapheme.chars() {
                char_to_column.push(column);
                text.push(character);
            }
        }
        let line = i32::try_from(y)
            .ok()
            .and_then(|y| y.checked_sub(scrollback))
            .ok_or_else(|| GhosttyError::AbiMismatch("grid line overflow".into()))?;
        Ok(GridLine {
            line,
            text,
            char_to_column,
        })
    }

    pub(crate) fn total_rows(&self) -> Result<usize> {
        let mut value = 0usize;
        // SAFETY: the owned terminal handle is live, and TOTAL_ROWS writes a
        // usize into `value` for the duration of this call.
        unsafe {
            get_terminal(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
                &mut value,
            )
        }?;
        if value > MAX_GRID_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "total grid rows",
                limit: MAX_GRID_ROWS,
            });
        }
        Ok(value)
    }

    pub(crate) fn scrollback_rows(&self) -> Result<usize> {
        let mut value = 0usize;
        // SAFETY: the owned terminal handle is live, and SCROLLBACK_ROWS
        // writes a usize into `value` for the duration of this call.
        unsafe {
            get_terminal(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                &mut value,
            )
        }?;
        if value > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        Ok(value)
    }

    fn cols(&self) -> Result<usize> {
        let mut value = 0u16;
        // SAFETY: the owned terminal handle is live, and COLS writes a u16
        // into `value` for the duration of this call.
        unsafe {
            get_terminal(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS,
                &mut value,
            )
        }?;
        if value == 0 {
            return Err(GhosttyError::AbiMismatch(
                "terminal reported zero columns".into(),
            ));
        }
        Ok(usize::from(value))
    }
}

fn check_grid_cell_count(rows: usize, cols: usize) -> Result<()> {
    let cell_count = rows
        .checked_mul(cols)
        .ok_or_else(|| GhosttyError::AbiMismatch("grid cell count overflow".into()))?;
    if cell_count > MAX_GRID_CELLS {
        return Err(GhosttyError::LimitExceeded {
            resource: "grid cells per call",
            limit: MAX_GRID_CELLS,
        });
    }
    Ok(())
}

fn grid_ref_grapheme(reference: &sys::GhosttyGridRef) -> Result<String> {
    let mut required = 0usize;
    let result = unsafe {
        sys::ghostty_grid_ref_graphemes(reference, std::ptr::null_mut(), 0, &mut required)
    };
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS && required == 0 {
        return Ok(String::new());
    }
    if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        check("grid_ref_graphemes_size", result)?;
    }
    if required > MAX_GRAPHEME_CODEPOINTS {
        return Err(GhosttyError::LimitExceeded {
            resource: "cell grapheme",
            limit: MAX_GRAPHEME_CODEPOINTS,
        });
    }
    let mut codepoints = vec![0u32; required];
    let result = unsafe {
        sys::ghostty_grid_ref_graphemes(
            reference,
            codepoints.as_mut_ptr(),
            codepoints.len(),
            &mut required,
        )
    };
    check("grid_ref_graphemes", result)?;
    if required > codepoints.len() {
        return Err(GhosttyError::AbiMismatch(format!(
            "grid_ref_graphemes reported {required} codepoints after receiving a {}-codepoint buffer",
            codepoints.len()
        )));
    }
    codepoints.truncate(required);
    Ok(codepoints
        .into_iter()
        .map(|value| char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect())
}
