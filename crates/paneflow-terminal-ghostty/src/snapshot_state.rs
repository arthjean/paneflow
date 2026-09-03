use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::snapshot_ffi::{
    RenderCursorBlinking, RenderCursorViewportHasValue, RenderCursorViewportWideTail,
    RenderCursorViewportX, RenderCursorViewportY, RenderCursorVisible, RenderCursorVisualStyle,
    TerminalCursorX, TerminalCursorY, cursor_shape, render_get, terminal_get,
    terminal_selection_rectangle,
};
use crate::{Cursor, GhosttyError, Point, Result};

impl DisplayTerminal {
    pub(crate) fn render_dimensions(&self) -> Result<(usize, usize)> {
        let mut cols = 0u16;
        let mut rows = 0u16;
        unsafe {
            get_multi(
                "render_state_get_multi",
                self.render_state.raw(),
                sys::ghostty_render_state_get_multi,
                [
                    Slot::new(
                        sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_COLS,
                        &mut cols,
                    ),
                    Slot::new(
                        sys::GhosttyRenderStateData_GHOSTTY_RENDER_STATE_DATA_ROWS,
                        &mut rows,
                    ),
                ],
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
        let visible = render_get::<RenderCursorVisible>(self.render_state.raw())?;
        let blinking = render_get::<RenderCursorBlinking>(self.render_state.raw())?;
        let in_viewport = render_get::<RenderCursorViewportHasValue>(self.render_state.raw())?;
        let shape = render_get::<RenderCursorVisualStyle>(self.render_state.raw())?;
        let (x, y, wide_tail) = if in_viewport {
            (
                render_get::<RenderCursorViewportX>(self.render_state.raw())?,
                render_get::<RenderCursorViewportY>(self.render_state.raw())?,
                render_get::<RenderCursorViewportWideTail>(self.render_state.raw())?,
            )
        } else {
            (
                terminal_get::<TerminalCursorX>(self.terminal.raw())?,
                terminal_get::<TerminalCursorY>(self.terminal.raw())?,
                false,
            )
        };
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
        terminal_selection_rectangle(self.terminal.raw())
    }
}
