use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result};

const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardLocation {
    Standard,
    Primary,
}

impl ClipboardLocation {
    fn raw(self) -> sys::GhosttyClipboardLocation {
        use sys as s;
        match self {
            Self::Standard => s::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
            Self::Primary => s::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_PRIMARY,
        }
    }
}

pub struct PasteRepresentation<'data> {
    pub mime: &'data str,
    pub data: &'data [u8],
}

impl DisplayTerminal {
    pub fn set_continuation_max_bytes(&mut self, bytes: usize) -> Result<()> {
        let result = unsafe {
            sys::ghostty_terminal_set(
                self.terminal.raw(),
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES,
                (&raw const bytes).cast::<c_void>(),
            )
        };
        check("terminal_set_continuation_max_bytes", result)
    }

    pub fn continuation(&self) -> Result<Option<Vec<u8>>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            sys::ghostty_terminal_continuation_alloc(
                self.terminal.raw(),
                std::ptr::null(),
                &mut pointer,
                &mut len,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE
            || result == sys::GhosttyResult_GHOSTTY_INVALID_VALUE
        {
            return Ok(None);
        }
        check("terminal_continuation_alloc", result)?;
        if pointer.is_null() || len == 0 {
            return Ok(None);
        }
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        let over_budget = len > MAX_CONTINUATION_BYTES;
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if over_budget {
            return Err(GhosttyError::LimitExceeded {
                resource: "continuation",
                limit: MAX_CONTINUATION_BYTES,
            });
        }
        Ok(Some(copied))
    }

    pub fn paste(
        &mut self,
        representations: &[PasteRepresentation<'_>],
        location: ClipboardLocation,
        allow_unsafe: bool,
    ) -> Result<bool> {
        if representations.is_empty() {
            return Ok(false);
        }
        let mimes: Vec<sys::GhosttyString> = representations
            .iter()
            .map(|representation| sys::GhosttyString {
                ptr: representation.mime.as_ptr(),
                len: representation.mime.len(),
            })
            .collect();
        let mut state = PasteState { representations };
        let paste = sys::GhosttyPaste {
            size: std::mem::size_of::<sys::GhosttyPaste>(),
            location: location.raw(),
            source: sys::GhosttyPasteSource_GHOSTTY_PASTE_SOURCE_CLIPBOARD,
            mimes: mimes.as_ptr(),
            mimes_len: mimes.len(),
            reader: sys::GhosttyMimeReader {
                read: Some(mime_read_trampoline),
                userdata: (&raw mut state).cast::<c_void>(),
            },
            allow_unsafe,
        };
        let mut written = false;
        let result =
            unsafe { sys::ghostty_terminal_paste(self.terminal.raw(), &paste, &mut written) };
        if result == sys::GhosttyResult_GHOSTTY_REJECTED {
            return Err(GhosttyError::UnsafePaste);
        }
        check("terminal_paste", result)?;
        Ok(written)
    }

    pub(crate) fn geometry_batch(&self) -> Result<(u16, usize, usize)> {
        let mut cols = 0u16;
        let mut total_rows = 0usize;
        let mut scrollback = 0usize;
        use sys as s;
        unsafe {
            get_multi(
                "terminal_get_multi",
                self.terminal.raw(),
                sys::ghostty_terminal_get_multi,
                [
                    Slot::new(s::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS, &mut cols),
                    Slot::new(
                        s::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TOTAL_ROWS,
                        &mut total_rows,
                    ),
                    Slot::new(
                        s::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS,
                        &mut scrollback,
                    ),
                ],
            )?;
        }
        Ok((cols, total_rows, scrollback))
    }
}

struct PasteState<'data, 'slice> {
    representations: &'slice [PasteRepresentation<'data>],
}

unsafe extern "C" fn mime_read_trampoline(
    userdata: *mut c_void,
    mime: sys::GhosttyString,
    writer: sys::GhosttyWriter,
) -> bool {
    if userdata.is_null() || mime.ptr.is_null() {
        return false;
    }
    let state = unsafe { &*userdata.cast::<PasteState<'_, '_>>() };
    let requested = unsafe { std::slice::from_raw_parts(mime.ptr, mime.len) };
    let Some(representation) = state
        .representations
        .iter()
        .find(|representation| representation.mime.as_bytes() == requested)
    else {
        return false;
    };
    let Some(write) = writer.write else {
        return false;
    };
    if representation.data.is_empty() {
        return true;
    }
    unsafe {
        write(
            writer.userdata,
            representation.data.as_ptr(),
            representation.data.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn the_batched_geometry_read_matches_the_individual_ones() {
        let mut terminal = terminal(40, 8);
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine")
            .expect("output must parse");
        let (cols, total_rows, scrollback) = terminal.geometry_batch().expect("batched read");
        assert_eq!(cols, 40);
        assert_eq!(
            total_rows,
            terminal.scrollback_rows().expect("scrollback") + 8,
            "total rows count the scrollback plus the screen"
        );
        assert_eq!(scrollback, terminal.scrollback_rows().expect("scrollback"));
        assert!(total_rows > 8, "the scrollback must count toward total rows");
    }

    fn pty_writes(terminal: &mut DisplayTerminal) -> Vec<u8> {
        terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                crate::BackendEvent::WritePty(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn pasting_frames_the_preferred_representation_for_the_program() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"\x1b[?2004h").expect("bracketed paste on");
        let _ = pty_writes(&mut terminal);

        let written = terminal
            .paste(
                &[
                    PasteRepresentation {
                        mime: "text/plain;charset=utf-8",
                        data: b"hello",
                    },
                    PasteRepresentation {
                        mime: "text/html",
                        data: b"<b>hello</b>",
                    },
                ],
                ClipboardLocation::Standard,
                false,
            )
            .expect("paste must succeed");
        assert!(written);

        let output = pty_writes(&mut terminal);
        assert_eq!(output, b"\x1b[200~hello\x1b[201~");
    }

    #[test]
    fn an_unsafe_paste_is_rejected_unless_it_is_allowed() {
        let mut terminal = terminal(20, 3);
        let payload = [PasteRepresentation {
            mime: "text/plain;charset=utf-8",
            data: b"rm -rf /\n",
        }];

        let error = terminal
            .paste(&payload, ClipboardLocation::Standard, false)
            .expect_err("a newline makes the paste unsafe");
        assert!(matches!(error, GhosttyError::UnsafePaste));
        assert!(pty_writes(&mut terminal).is_empty());

        assert!(
            terminal
                .paste(&payload, ClipboardLocation::Standard, true)
                .expect("an allowed paste goes through")
        );
        assert!(!pty_writes(&mut terminal).is_empty());
    }

    #[test]
    fn pasting_nothing_writes_nothing() {
        let mut terminal = terminal(20, 3);
        assert!(!terminal.paste(&[], ClipboardLocation::Standard, false).expect("empty paste"));
        assert!(pty_writes(&mut terminal).is_empty());
    }

    #[test]
    fn a_continuation_is_absent_unless_the_budget_is_configured() {
        let mut terminal = terminal(20, 3);
        terminal.feed(b"\x1b[1").expect("partial sequence");
        assert!(terminal.continuation().expect("alloc path").is_none());
    }
}
