use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::formatter::FormatterOptions;
use crate::handles::check;
use crate::snapshot_ffi::copy_buffer;
use crate::{GhosttyError, Hyperlink, Point, Result, SelectionRange};

const MAX_HYPERLINK_BYTES: usize = 8192;
const MAX_SELECTION_BYTES: usize = 400_000;

impl DisplayTerminal {
    pub fn set_selection(&mut self, range: SelectionRange) -> Result<()> {
        let start = self.grid_ref(range.start)?;
        let end = self.grid_ref(range.end)?;
        let selection = sys::GhosttySelection {
            start,
            end,
            rectangle: range.rectangle,
        };
        self.install_selection(&selection)
    }

    pub fn selection_text(&self) -> Result<Option<String>> {
        let Some(selection) = self.active_selection()? else {
            return Ok(None);
        };
        let formatter = self.formatter(FormatterOptions::plain_text())?;
        formatter.select(&selection)?;
        let bytes = formatter.format_alloc(MAX_SELECTION_BYTES, "selection text")?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| GhosttyError::InvalidUtf8("selection text"))
    }

    fn active_selection(&self) -> Result<Option<sys::GhosttySelection>> {
        let mut selection = std::mem::MaybeUninit::<sys::GhosttySelection>::uninit();
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SELECTION,
                selection.as_mut_ptr().cast(),
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("terminal_get_selection", result)?;
        Ok(Some(unsafe { selection.assume_init() }))
    }

    pub fn clear_selection(&mut self) -> Result<()> {
        let result = unsafe {
            sys::ghostty_terminal_set(
                self.terminal.raw(),
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SELECTION,
                std::ptr::null(),
            )
        };
        check("clear_selection", result)
    }

    pub fn hyperlink_at(&self, point: Point) -> Result<Option<Hyperlink>> {
        let reference = self.grid_ref(point)?;
        let bytes = copy_buffer(
            "hyperlink URI",
            MAX_HYPERLINK_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_grid_ref_hyperlink_uri(&reference, buffer, len, written)
            },
        )?;
        bytes
            .map(|bytes| {
                String::from_utf8(bytes)
                    .map(|uri| Hyperlink { point, uri })
                    .map_err(|_| GhosttyError::InvalidUtf8("hyperlink URI"))
            })
            .transpose()
    }

    pub(crate) fn install_optional_selection(
        &mut self,
        result: sys::GhosttyResult,
        selection: &sys::GhosttySelection,
    ) -> Result<bool> {
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            Ok(false)
        } else {
            check("derive_selection", result)?;
            self.install_selection(selection)?;
            Ok(true)
        }
    }

    pub(crate) fn install_selection(&mut self, selection: &sys::GhosttySelection) -> Result<()> {
        let result = unsafe {
            sys::ghostty_terminal_set(
                self.terminal.raw(),
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SELECTION,
                (selection as *const sys::GhosttySelection).cast(),
            )
        };
        check("set_selection", result)
    }
}
