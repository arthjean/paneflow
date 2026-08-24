use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::limits::{MAX_GRID_CELLS, MAX_GRID_ROWS, MAX_SCROLLBACK_ROWS};
use crate::snapshot_ffi::{
    TerminalCols, TerminalScrollbackRows, TerminalTotalRows, ghostty_point, raw_cell_wide,
    terminal_get,
};
use crate::{GhosttyError, Point, Result};

const MAX_GRAPHEME_CODEPOINTS: usize = 1024;

#[derive(Default)]
pub(crate) struct GridLine {
    pub(crate) line: i32,
    pub(crate) text: String,
    pub(crate) char_to_column: Vec<usize>,
}

pub(crate) struct GridGeometry {
    pub(crate) total_rows: usize,
    pub(crate) scrollback: i32,
    pub(crate) cols: usize,
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
        let geometry = self.grid_geometry()?;
        check_grid_cell_count(lines.len(), geometry.cols)?;
        let total_rows = i64::try_from(geometry.total_rows)
            .map_err(|_| GhosttyError::AbiMismatch("total rows do not fit i64".into()))?;
        let scrollback_i64 = i64::from(geometry.scrollback);
        let mut result = Vec::with_capacity(lines.len());
        let mut grid_line = GridLine::default();
        let mut grapheme = Vec::new();
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
            self.fill_grid_line(
                usize::try_from(screen_y)
                    .map_err(|_| GhosttyError::AbiMismatch("negative grid line".into()))?,
                &geometry,
                &mut grid_line,
                &mut grapheme,
            )?;
            result.push((grid_line.line, std::mem::take(&mut grid_line.text)));
        }
        Ok(result)
    }

    pub(crate) fn grid_lines(
        &self,
        range: Option<std::ops::Range<usize>>,
    ) -> Result<Vec<GridLine>> {
        let geometry = self.grid_geometry()?;
        let range = range.unwrap_or(0..geometry.total_rows);
        if range.start > range.end || range.end > geometry.total_rows {
            return Err(GhosttyError::Ffi {
                operation: "grid_range_out_of_bounds",
                code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        check_grid_cell_count(range.len(), geometry.cols)?;
        let mut lines = Vec::with_capacity(range.len());
        let mut grapheme = Vec::new();
        for y in range {
            let mut line = GridLine::default();
            self.fill_grid_line(y, &geometry, &mut line, &mut grapheme)?;
            lines.push(line);
        }
        Ok(lines)
    }

    pub(crate) fn grid_geometry(&self) -> Result<GridGeometry> {
        let scrollback = i32::try_from(self.scrollback_rows()?)
            .map_err(|_| GhosttyError::AbiMismatch("scrollback does not fit i32".into()))?;
        Ok(GridGeometry {
            total_rows: self.total_rows()?,
            scrollback,
            cols: self.cols()?,
        })
    }

    pub(crate) fn fill_grid_line(
        &self,
        y: usize,
        geometry: &GridGeometry,
        line: &mut GridLine,
        grapheme: &mut Vec<u32>,
    ) -> Result<()> {
        if y >= geometry.total_rows {
            return Err(GhosttyError::Ffi {
                operation: "grid_line_out_of_bounds",
                code: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
        line.text.clear();
        line.char_to_column.clear();
        line.text.reserve(geometry.cols);
        line.char_to_column.reserve(geometry.cols);
        for column in 0..geometry.cols {
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
            let wide = raw_cell_wide(cell)?;
            if wide == sys::GhosttyCellWide_GHOSTTY_CELL_WIDE_SPACER_TAIL {
                continue;
            }
            append_grid_ref_grapheme(
                &reference,
                column,
                &mut line.text,
                &mut line.char_to_column,
                grapheme,
            )?;
        }
        line.line = i32::try_from(y)
            .ok()
            .and_then(|y| y.checked_sub(geometry.scrollback))
            .ok_or_else(|| GhosttyError::AbiMismatch("grid line overflow".into()))?;
        Ok(())
    }

    pub(crate) fn total_rows(&self) -> Result<usize> {
        let value = terminal_get::<TerminalTotalRows>(self.terminal.raw())?;
        if value > MAX_GRID_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "total grid rows",
                limit: MAX_GRID_ROWS,
            });
        }
        Ok(value)
    }

    pub(crate) fn scrollback_rows(&self) -> Result<usize> {
        let value = terminal_get::<TerminalScrollbackRows>(self.terminal.raw())?;
        if value > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        Ok(value)
    }

    fn cols(&self) -> Result<usize> {
        let value = terminal_get::<TerminalCols>(self.terminal.raw())?;
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

fn append_grid_ref_grapheme(
    reference: &sys::GhosttyGridRef,
    column: usize,
    text: &mut String,
    char_to_column: &mut Vec<usize>,
    codepoints: &mut Vec<u32>,
) -> Result<()> {
    let mut required = 0usize;
    let result = unsafe {
        sys::ghostty_grid_ref_graphemes(reference, std::ptr::null_mut(), 0, &mut required)
    };
    if result == sys::GhosttyResult_GHOSTTY_SUCCESS && required == 0 {
        char_to_column.push(column);
        text.push(' ');
        return Ok(());
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
    codepoints.clear();
    codepoints.resize(required, 0);
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
    if codepoints.is_empty() {
        char_to_column.push(column);
        text.push(' ');
    } else {
        for value in codepoints.iter().copied() {
            char_to_column.push(column);
            text.push(char::from_u32(value).unwrap_or(char::REPLACEMENT_CHARACTER));
        }
    }
    Ok(())
}
