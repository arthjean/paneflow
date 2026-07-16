use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::snapshot_ffi::copy_buffer;
use crate::{GhosttyError, Hyperlink, Point, Result, SelectionRange};

const MAX_HYPERLINK_BYTES: usize = 8192;
const MAX_SELECTION_BYTES: usize = 400_000;

struct GhosttyAllocation {
    pointer: *mut u8,
    len: usize,
}

impl GhosttyAllocation {
    fn copy(&self) -> Vec<u8> {
        if self.len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(self.pointer, self.len) }.to_vec()
        }
    }
}

impl Drop for GhosttyAllocation {
    fn drop(&mut self) {
        unsafe { sys::ghostty_free(std::ptr::null(), self.pointer, self.len) };
    }
}

impl DisplayTerminal {
    pub fn set_selection(&mut self, range: SelectionRange) -> Result<()> {
        let start = self.grid_ref(range.start)?;
        let end = self.grid_ref(range.end)?;
        let selection = sys::GhosttySelection {
            size: std::mem::size_of::<sys::GhosttySelection>(),
            start,
            end,
            rectangle: range.rectangle,
        };
        self.install_selection(&selection)
    }

    pub fn select_word(&mut self, point: Point) -> Result<bool> {
        let reference = self.grid_ref(point)?;
        let options = sys::GhosttyTerminalSelectWordOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectWordOptions>(),
            ref_: reference,
            boundary_codepoints: std::ptr::null(),
            boundary_codepoints_len: 0,
        };
        let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
        selection.size = std::mem::size_of::<sys::GhosttySelection>();
        let result = unsafe {
            sys::ghostty_terminal_select_word(self.terminal.raw(), &options, &mut selection)
        };
        self.install_optional_selection(result, &selection)
    }

    pub fn select_line(&mut self, point: Point) -> Result<bool> {
        let reference = self.grid_ref(point)?;
        let options = sys::GhosttyTerminalSelectLineOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectLineOptions>(),
            ref_: reference,
            whitespace: std::ptr::null(),
            whitespace_len: 0,
            semantic_prompt_boundary: true,
        };
        let mut selection: sys::GhosttySelection = unsafe { std::mem::zeroed() };
        selection.size = std::mem::size_of::<sys::GhosttySelection>();
        let result = unsafe {
            sys::ghostty_terminal_select_line(self.terminal.raw(), &options, &mut selection)
        };
        self.install_optional_selection(result, &selection)
    }

    pub fn selection_text(&self) -> Result<Option<String>> {
        let options = sys::GhosttyTerminalSelectionFormatOptions {
            size: std::mem::size_of::<sys::GhosttyTerminalSelectionFormatOptions>(),
            emit: sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: true,
            trim: true,
            selection: std::ptr::null(),
        };
        let mut pointer = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            sys::ghostty_terminal_selection_format_alloc(
                self.terminal.raw(),
                std::ptr::null(),
                options,
                &mut pointer,
                &mut len,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("selection_format_alloc", result)?;
        if len > MAX_SELECTION_BYTES || (len > 0 && pointer.is_null()) {
            if !pointer.is_null() {
                unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
            }
            return Err(GhosttyError::LimitExceeded {
                resource: "selection text",
                limit: MAX_SELECTION_BYTES,
            });
        }
        let allocation = GhosttyAllocation { pointer, len };
        let text = String::from_utf8(allocation.copy())
            .map_err(|_| GhosttyError::InvalidUtf8("selection text"))?;
        Ok(Some(text))
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

    fn install_optional_selection(
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

    fn install_selection(&mut self, selection: &sys::GhosttySelection) -> Result<()> {
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
