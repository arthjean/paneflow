use paneflow_libghostty_sys as sys;

use crate::engine::{DisplayTerminal, get_terminal};
use crate::handles::check;
use crate::snapshot_ffi::{cursor_shape, render_get};
use crate::{Cursor, GhosttyError, Point, Result};

impl DisplayTerminal {
    pub(crate) fn render_dimensions(&self) -> Result<(usize, usize)> {
        let mut cols = 0u16;
        let mut rows = 0u16;
        // SAFETY: `self.render_state` owns a live handle, and the COLS and ROWS
        // selectors each write a u16 into their distinct matching outputs.
        unsafe {
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
                &mut cols,
            )?;
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
                &mut rows,
            )?;
        }
        if cols == 0 || rows == 0 {
            return Err(GhosttyError::AbiMismatch(format!(
                "render state reported invalid dimensions {cols}x{rows}"
            )));
        }
        Ok((usize::from(cols), usize::from(rows)))
    }

    pub(crate) fn cursor(&self, display_offset: usize) -> Result<Cursor> {
        let display_offset = i32::try_from(display_offset)
            .map_err(|_| GhosttyError::AbiMismatch("cursor display offset overflow".into()))?;
        let mut visible = false;
        let mut blinking = false;
        let mut in_viewport = false;
        let mut wide_tail = false;
        let mut x = 0u16;
        let mut y = 0u16;
        let mut shape =
            sys::GhosttyRenderStateCursorVisualStyle_GHOSTTY_RENDER_STATE_CURSOR_VISUAL_STYLE_BLOCK;
        // SAFETY: the render-state handle is live; the first three selectors
        // write bool values and CURSOR_VISUAL_STYLE writes its generated enum
        // type into the corresponding distinct outputs.
        unsafe {
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISIBLE,
                &mut visible,
            )?;
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_BLINKING,
                &mut blinking,
            )?;
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE,
                &mut in_viewport,
            )?;
            render_get(
                self.render_state.raw(),
                sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VISUAL_STYLE,
                &mut shape,
            )?;
        }
        if in_viewport {
            // SAFETY: the render-state handle is live; viewport X/Y write u16
            // and WIDE_TAIL writes bool into their matching distinct outputs.
            unsafe {
                render_get(
                    self.render_state.raw(),
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_X,
                    &mut x,
                )?;
                render_get(
                    self.render_state.raw(),
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_Y,
                    &mut y,
                )?;
                render_get(
                    self.render_state.raw(),
                    sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_CURSOR_VIEWPORT_WIDE_TAIL,
                    &mut wide_tail,
                )?;
            }
        } else {
            // SAFETY: `self.terminal` owns a live handle, and the CURSOR_X and
            // CURSOR_Y selectors each write u16 into distinct matching outputs.
            unsafe {
                get_terminal(
                    self.terminal.raw(),
                    sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_X,
                    &mut x,
                )?;
                get_terminal(
                    self.terminal.raw(),
                    sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_CURSOR_Y,
                    &mut y,
                )?;
            }
        }
        Ok(Cursor {
            point: Point::new(
                i32::from(y) - if in_viewport { display_offset } else { 0 },
                usize::from(x),
            ),
            shape: cursor_shape(shape)?,
            visible,
            blinking,
            wide_tail,
        })
    }

    pub(crate) fn selection_rectangle(&self) -> Result<Option<bool>> {
        let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
        selection.size = std::mem::size_of::<sys::GhosttySelection>();
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION,
                (&mut selection as *mut sys::GhosttySelection).cast(),
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            Ok(None)
        } else {
            check("terminal_get_selection", result)?;
            Ok(Some(selection.rectangle))
        }
    }
}
