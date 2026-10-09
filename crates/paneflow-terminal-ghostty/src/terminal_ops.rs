use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result};

const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionProgress {
    Unsupported,
    Pending,
    Complete,
}

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

    pub fn compression_activity(&self) -> Result<u64> {
        let mut activity = 0u64;
        let result = unsafe {
            sys::ghostty_terminal_compression_activity(self.terminal.raw(), &mut activity)
        };
        check("terminal_compression_activity", result)?;
        Ok(activity)
    }

    pub fn compress_step(&mut self) -> Result<CompressionProgress> {
        let mut progress =
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED;
        let result = unsafe {
            sys::ghostty_terminal_compress(
                self.terminal.raw(),
                sys::GhosttyTerminalCompressionMode_GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL,
                &mut progress,
            )
        };
        check("terminal_compress", result)?;
        Ok(match progress {
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING => {
                CompressionProgress::Pending
            }
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE => {
                CompressionProgress::Complete
            }
            _ => CompressionProgress::Unsupported,
        })
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

    unsafe extern "C" fn refusing_write(userdata: *mut c_void, _: *const u8, _: usize) -> bool {
        unsafe { *userdata.cast::<usize>() += 1 };
        false
    }

    #[test]
    fn the_paste_reader_stops_at_a_refused_write() {
        let representations = [PasteRepresentation {
            mime: "text/plain",
            data: b"hello",
        }];
        let mut state = PasteState {
            representations: &representations,
        };
        let mut refusals = 0usize;
        let writer = sys::GhosttyWriter {
            write: Some(refusing_write),
            userdata: (&raw mut refusals).cast(),
        };
        let mime = sys::GhosttyString {
            ptr: b"text/plain".as_ptr(),
            len: "text/plain".len(),
        };

        let accepted =
            unsafe { mime_read_trampoline((&raw mut state).cast(), mime, writer) };

        assert!(!accepted, "a refused write must fail the read");
        assert_eq!(refusals, 1, "the reader must stop at the first refusal");
    }

    struct RefusableAllocator {
        refusals_left: std::cell::Cell<usize>,
    }

    fn allocation_layout(len: usize, alignment: u8) -> std::alloc::Layout {
        std::alloc::Layout::from_size_align(len, 1usize << alignment)
            .expect("libghostty requests a valid layout")
    }

    unsafe extern "C" fn refusable_alloc(
        ctx: *mut c_void,
        len: usize,
        alignment: u8,
        _: usize,
    ) -> *mut c_void {
        let allocator = unsafe { &*ctx.cast::<RefusableAllocator>() };
        let refusals_left = allocator.refusals_left.get();
        if refusals_left > 0 {
            allocator.refusals_left.set(refusals_left - 1);
            return std::ptr::null_mut();
        }
        unsafe { std::alloc::alloc(allocation_layout(len, alignment)).cast() }
    }

    unsafe extern "C" fn in_place_resize_refused(
        _: *mut c_void,
        _: *mut c_void,
        _: usize,
        _: u8,
        _: usize,
        _: usize,
    ) -> bool {
        false
    }

    unsafe extern "C" fn remap_refused(
        _: *mut c_void,
        _: *mut c_void,
        _: usize,
        _: u8,
        _: usize,
        _: usize,
    ) -> *mut c_void {
        std::ptr::null_mut()
    }

    unsafe extern "C" fn refusable_free(
        _: *mut c_void,
        memory: *mut c_void,
        len: usize,
        alignment: u8,
        _: usize,
    ) {
        unsafe { std::alloc::dealloc(memory.cast(), allocation_layout(len, alignment)) };
    }

    static REFUSABLE_VTABLE: sys::GhosttyAllocatorVtable = sys::GhosttyAllocatorVtable {
        alloc: Some(refusable_alloc),
        resize: Some(in_place_resize_refused),
        remap: Some(remap_refused),
        free: Some(refusable_free),
    };

    unsafe extern "C" fn record_pty_write(
        _: sys::GhosttyTerminal,
        userdata: *mut c_void,
        data: *const u8,
        len: usize,
    ) {
        let written = unsafe { &mut *userdata.cast::<Vec<u8>>() };
        written.extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
    }

    struct RecordingTerminal {
        allocator_state: Box<RefusableAllocator>,
        _allocator: Box<sys::GhosttyAllocator>,
        #[allow(
            clippy::box_collection,
            reason = "libghostty keeps the userdata pointer, so the buffer address must not move"
        )]
        pty: Box<Vec<u8>>,
        raw: sys::GhosttyTerminal,
    }

    impl RecordingTerminal {
        fn new() -> Self {
            let allocator_state = Box::new(RefusableAllocator {
                refusals_left: std::cell::Cell::new(0),
            });
            let allocator = Box::new(sys::GhosttyAllocator {
                ctx: (&raw const *allocator_state).cast_mut().cast(),
                vtable: &REFUSABLE_VTABLE,
            });
            let mut raw: sys::GhosttyTerminal = std::ptr::null_mut();
            check("terminal_new", unsafe {
                sys::ghostty_terminal_new(&*allocator, &mut raw, 20, 3)
            })
            .expect("terminal must initialize");
            let mut pty = Box::new(Vec::<u8>::new());
            check("set_userdata", unsafe {
                sys::ghostty_terminal_set(
                    raw,
                    sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
                    (&raw mut *pty).cast_const().cast(),
                )
            })
            .expect("userdata must install");
            check("set_write_pty", unsafe {
                sys::ghostty_terminal_set(
                    raw,
                    sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                    record_pty_write as *const c_void,
                )
            })
            .expect("write_pty must install");
            Self {
                allocator_state,
                _allocator: allocator,
                pty,
                raw,
            }
        }

        fn paste(&mut self, read: sys::GhosttyMimeReaderFn, userdata: *mut c_void) -> sys::GhosttyResult {
            let mime = sys::GhosttyString {
                ptr: b"text/plain".as_ptr(),
                len: "text/plain".len(),
            };
            let paste = sys::GhosttyPaste {
                size: std::mem::size_of::<sys::GhosttyPaste>(),
                location: ClipboardLocation::Standard.raw(),
                source: sys::GhosttyPasteSource_GHOSTTY_PASTE_SOURCE_CLIPBOARD,
                mimes: &mime,
                mimes_len: 1,
                reader: sys::GhosttyMimeReader { read, userdata },
                allow_unsafe: false,
            };
            let mut written = false;
            unsafe { sys::ghostty_terminal_paste(self.raw, &paste, &mut written) }
        }

        fn take_pty(&mut self) -> Vec<u8> {
            std::mem::take(&mut *self.pty)
        }
    }

    impl Drop for RecordingTerminal {
        fn drop(&mut self) {
            unsafe { sys::ghostty_terminal_free(self.raw) };
        }
    }

    fn paste_with_paneflow_reader(terminal: &mut RecordingTerminal, data: &[u8]) -> sys::GhosttyResult {
        let representations = [PasteRepresentation {
            mime: "text/plain",
            data,
        }];
        let mut state = PasteState {
            representations: &representations,
        };
        terminal.paste(Some(mime_read_trampoline), (&raw mut state).cast())
    }

    #[test]
    fn a_refused_paste_write_fails_the_paste_before_the_pty() {
        let mut terminal = RecordingTerminal::new();

        let accepted = paste_with_paneflow_reader(&mut terminal, b"hello");
        let accepted_pty = terminal.take_pty();

        terminal.allocator_state.refusals_left.set(usize::MAX);
        let refused = paste_with_paneflow_reader(&mut terminal, b"hello");
        terminal.allocator_state.refusals_left.set(0);
        let refused_pty = terminal.take_pty();

        assert_eq!(accepted, sys::GhosttyResult_GHOSTTY_SUCCESS);
        assert_eq!(accepted_pty, b"hello");
        assert_ne!(
            refused,
            sys::GhosttyResult_GHOSTTY_SUCCESS,
            "a refused write must fail the paste"
        );
        assert!(
            refused_pty.is_empty(),
            "nothing from a refused paste may reach the PTY, got {refused_pty:?}"
        );
    }

    struct IgnoringReaderLog {
        first_write_refused: bool,
        second_write_accepted: bool,
    }

    unsafe extern "C" fn reader_that_ignores_a_refused_write(
        userdata: *mut c_void,
        _: sys::GhosttyString,
        writer: sys::GhosttyWriter,
    ) -> bool {
        let log = unsafe { &mut *userdata.cast::<IgnoringReaderLog>() };
        let Some(write) = writer.write else {
            return false;
        };
        let first = b"refused-";
        log.first_write_refused = !unsafe { write(writer.userdata, first.as_ptr(), first.len()) };
        let second = b"after-refusal";
        log.second_write_accepted =
            unsafe { write(writer.userdata, second.as_ptr(), second.len()) };
        true
    }

    #[test]
    fn upstream_2b0ceff7d_a_reader_that_ignores_a_refused_write_still_fails_the_paste() {
        let mut terminal = RecordingTerminal::new();
        let mut log = IgnoringReaderLog {
            first_write_refused: false,
            second_write_accepted: false,
        };

        terminal.allocator_state.refusals_left.set(1);
        let result = terminal.paste(
            Some(reader_that_ignores_a_refused_write),
            (&raw mut log).cast(),
        );
        terminal.allocator_state.refusals_left.set(0);
        let pty = terminal.take_pty();

        assert!(log.first_write_refused, "the first write must be refused");
        assert!(
            log.second_write_accepted,
            "the write after the refusal must reach the paste buffer"
        );
        assert_ne!(
            result,
            sys::GhosttyResult_GHOSTTY_SUCCESS,
            "a refused write must fail the paste even when the reader reports success"
        );
        assert!(
            pty.is_empty(),
            "nothing written after a refusal may reach the PTY, got {:?}",
            String::from_utf8_lossy(&pty)
        );
    }
}
