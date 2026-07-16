use std::marker::PhantomData;
use std::rc::Rc;

use paneflow_libghostty_sys as sys;

use crate::callbacks::CallbackState;
use crate::color_query::ColorQueryResponder;
use crate::handles::{OwnedHandle, check};
use crate::osc52::Osc52Scanner;
use crate::snapshot::SnapshotCache;
use crate::{BackendEvent, GhosttyError, Modes, Result, Scroll, WindowSize};

const MAX_METADATA_BYTES: usize = 4096;
const CLEAR_SCREEN_AND_SCROLLBACK: &[u8] = b"\x1b[3J\x1b[2J\x1b[H";

pub struct DisplayTerminal {
    pub(crate) mouse_event: OwnedHandle<sys::GhosttyMouseEvent>,
    pub(crate) mouse_encoder: OwnedHandle<sys::GhosttyMouseEncoder>,
    pub(crate) key_event: OwnedHandle<sys::GhosttyKeyEvent>,
    pub(crate) key_encoder: OwnedHandle<sys::GhosttyKeyEncoder>,
    pub(crate) row_cells: OwnedHandle<sys::GhosttyRenderStateRowCells>,
    pub(crate) row_iterator: OwnedHandle<sys::GhosttyRenderStateRowIterator>,
    pub(crate) render_state: OwnedHandle<sys::GhosttyRenderState>,
    pub(crate) terminal: OwnedHandle<sys::GhosttyTerminal>,
    pub(crate) snapshot_cache: SnapshotCache,
    pub(crate) callbacks: Box<CallbackState>,
    pub(crate) color_queries: ColorQueryResponder,
    pub(crate) osc52: Osc52Scanner,
    pub(crate) last_pwd: Option<String>,
    pub(crate) _not_send_or_sync: PhantomData<Rc<()>>,
}

impl DisplayTerminal {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        let callbacks = &self.callbacks;
        let terminal = self.terminal.raw();
        let osc52 = &mut self.osc52;
        self.color_queries.feed(
            bytes,
            &mut |input| {
                if input.is_empty() {
                    return;
                }
                osc52.feed(input, &mut |text| {
                    callbacks.push(BackendEvent::ClipboardStore(text));
                });
                unsafe { sys::ghostty_terminal_vt_write(terminal, input.as_ptr(), input.len()) };
            },
            &mut |reply| callbacks.push(BackendEvent::WritePty(reply.to_vec())),
        );
        self.capture_pwd()
    }

    pub fn resize(&mut self, size: WindowSize) -> Result<()> {
        let size = size.validate()?;
        let current = self.callbacks.size();
        if size.cols < current.cols && size.rows < current.rows {
            // Ghostty before 7fa6fffb underflows while shrinking both axes
            // when the cursor was on the old bottom row. Shrinking rows first
            // reloads the cursor against the new bottom before column reflow.
            let rows_first = WindowSize {
                cols: current.cols,
                rows: size.rows,
                cell_width: size.cell_width,
                cell_height: size.cell_height,
            };
            resize_terminal(self.terminal.raw(), rows_first)?;
            self.callbacks.set_size(rows_first);
        }
        resize_terminal(self.terminal.raw(), size)?;
        self.snapshot_cache.invalidate();
        self.callbacks.set_size(size);
        Ok(())
    }

    pub fn reset(&mut self) {
        unsafe { sys::ghostty_terminal_reset(self.terminal.raw()) };
        self.snapshot_cache.invalidate();
    }

    /// Clear the viewport, scrollback, and cursor position without performing
    /// a full terminal reset, so negotiated modes remain intact.
    pub fn clear_screen_and_scrollback(&mut self) -> Result<()> {
        self.feed(CLEAR_SCREEN_AND_SCROLLBACK)?;
        self.snapshot_cache.invalidate();
        Ok(())
    }

    pub fn drain_events(&mut self) -> Vec<BackendEvent> {
        self.callbacks.drain()
    }

    pub fn modes(&self) -> Result<Modes> {
        Ok(Modes {
            alternate_screen: self.mode(47)? || self.mode(1047)? || self.mode(1049)?,
            application_cursor: self.mode(1)?,
            application_keypad: self.mode(66)?,
            bracketed_paste: self.mode(2004)?,
            focus_reporting: self.mode(1004)?,
            alternate_scroll: self.mode(1007)?,
            mouse_report_click: self.mode(9)? || self.mode(1000)?,
            mouse_drag: self.mode(1002)?,
            mouse_motion: self.mode(1003)?,
            sgr_mouse: self.mode(1006)?,
            utf8_mouse: self.mode(1005)?,
            kitty_keyboard: self.kitty_keyboard_flags()? != 0,
        })
    }

    pub fn scroll(&mut self, scroll: Scroll) {
        let (tag, delta) = match scroll {
            Scroll::Top => (
                sys::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_TOP,
                0,
            ),
            Scroll::Bottom => (
                sys::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_BOTTOM,
                0,
            ),
            Scroll::Delta(delta) => (
                sys::GhosttyTerminalScrollViewportTag_GHOSTTY_SCROLL_VIEWPORT_DELTA,
                delta.saturating_neg() as isize,
            ),
        };
        let behavior = sys::GhosttyTerminalScrollViewport {
            tag,
            value: sys::GhosttyTerminalScrollViewportValue { delta },
        };
        unsafe { sys::ghostty_terminal_scroll_viewport(self.terminal.raw(), behavior) };
        self.snapshot_cache.invalidate();
    }

    fn mode(&self, dec_mode: u16) -> Result<bool> {
        let mut value = false;
        let result =
            unsafe { sys::ghostty_terminal_mode_get(self.terminal.raw(), dec_mode, &mut value) };
        check("terminal_mode_get", result)?;
        Ok(value)
    }

    fn kitty_keyboard_flags(&self) -> Result<u8> {
        let mut value = 0u8;
        // SAFETY: `self.terminal` owns a live terminal handle, and the kitty
        // keyboard flags selector writes exactly one u8 into `value`.
        unsafe {
            get_terminal(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_KITTY_KEYBOARD_FLAGS,
                &mut value,
            )?;
        }
        Ok(value)
    }

    fn capture_pwd(&mut self) -> Result<()> {
        let mut value = sys::GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
        // SAFETY: `self.terminal` owns a live terminal handle, and the PWD
        // selector writes a `GhosttyString` that borrows terminal-owned data.
        unsafe {
            get_terminal(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD,
                &mut value,
            )?;
        }
        if value.len > MAX_METADATA_BYTES || (value.len > 0 && value.ptr.is_null()) {
            return Err(GhosttyError::LimitExceeded {
                resource: "working directory",
                limit: MAX_METADATA_BYTES,
            });
        }
        let bytes = if value.len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value.ptr, value.len) }
        };
        if !bytes.is_empty() && self.last_pwd.as_deref().map(str::as_bytes) != Some(bytes) {
            let pwd = std::str::from_utf8(bytes)
                .map_err(|_| GhosttyError::InvalidUtf8("working directory"))?
                .to_owned();
            self.last_pwd = Some(pwd.clone());
            self.callbacks.push(BackendEvent::WorkingDirectory(pwd));
        }
        Ok(())
    }
}

fn resize_terminal(terminal: sys::GhosttyTerminal, size: WindowSize) -> Result<()> {
    let result = unsafe {
        sys::ghostty_terminal_resize(
            terminal,
            size.cols,
            size.rows,
            size.cell_width,
            size.cell_height,
        )
    };
    check("terminal_resize", result)?;
    Ok(())
}

/// Read a field from a live libghostty terminal handle.
///
/// # Safety
///
/// `terminal` must be a live `GhosttyTerminal`. `data` must select a field
/// whose ABI output type is exactly `T`, including size, alignment, and valid
/// Rust bit patterns. The selected operation must be allowed to initialize
/// `out` for the duration of this call.
pub(crate) unsafe fn get_terminal<T>(
    terminal: sys::GhosttyTerminal,
    data: sys::GhosttyTerminalData,
    out: &mut T,
) -> Result<()> {
    let result = unsafe { sys::ghostty_terminal_get(terminal, data, (out as *mut T).cast()) };
    check("terminal_get", result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_screen_and_scrollback_preserves_terminal_modes() {
        let size = WindowSize::new(10, 2, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100).expect("terminal must initialize");
        terminal
            .feed(b"\x1b[?2004hone\r\ntwo\r\nthree\r\nfour")
            .expect("fixture output must parse");

        assert!(
            terminal
                .snapshot()
                .expect("snapshot before clear")
                .history_size
                > 0
        );
        assert!(
            terminal
                .modes()
                .expect("modes before clear")
                .bracketed_paste
        );

        terminal
            .clear_screen_and_scrollback()
            .expect("grid clear must succeed");
        let content = terminal.snapshot().expect("snapshot after clear");

        assert_eq!(content.history_size, 0);
        assert!(content.cells.iter().all(|cell| cell.character == ' '));
        assert_eq!(content.cursor.point, crate::Point::new(0, 0));
        assert!(terminal.modes().expect("modes after clear").bracketed_paste);
    }
}
