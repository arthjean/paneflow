//! Terminal operations that sit outside the read/write hot path: scrollback
//! compression, mid-sequence continuation, protocol-aware paste, and size
//! reports.

use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::encode::encode_with_buffer;
use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result, WindowSize};

/// Upper bound for an encoded size report.
const MAX_SIZE_REPORT_BYTES: usize = 64;

/// Upper bound for a continuation dump. A continuation is a partially parsed
/// escape sequence, which libghostty already caps well below this.
const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

/// How much scrollback compression work to do in one call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionMode {
    /// One bounded step, sized for running on an idle tick.
    Incremental,
    /// Compress everything before returning.
    Full,
}

/// What a compression pass accomplished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompressionOutcome {
    /// Nothing is left to compress.
    Complete,
    /// More work remains; call again on the next idle tick.
    Pending,
    /// This build of libghostty cannot compress scrollback.
    Unsupported,
}

/// Which report format to encode a terminal size in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeReportStyle {
    /// In-band size report, mode 2048: `CSI 48 ; rows ; cols ; height ; width t`.
    Mode2048,
    /// XTWINOPS text area size in pixels: `CSI 4 ; height ; width t`.
    Csi14T,
    /// XTWINOPS cell size in pixels: `CSI 6 ; height ; width t`.
    Csi16T,
    /// XTWINOPS text area size in cells: `CSI 8 ; rows ; cols t`.
    Csi18T,
}

impl SizeReportStyle {
    fn raw(self) -> sys::GhosttySizeReportStyle {
        use sys as s;
        match self {
            Self::Mode2048 => s::GhosttySizeReportStyle_GHOSTTY_SIZE_REPORT_MODE_2048,
            Self::Csi14T => s::GhosttySizeReportStyle_GHOSTTY_SIZE_REPORT_CSI_14_T,
            Self::Csi16T => s::GhosttySizeReportStyle_GHOSTTY_SIZE_REPORT_CSI_16_T,
            Self::Csi18T => s::GhosttySizeReportStyle_GHOSTTY_SIZE_REPORT_CSI_18_T,
        }
    }
}

/// The result of writing up to the ground state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GroundWrite {
    /// Bytes consumed, including the one that reached ground.
    pub consumed: usize,
    /// Whether the parser is now at ground. When false the whole slice was
    /// consumed and a sequence is still open.
    pub at_ground: bool,
}

/// Where a paste's text comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PasteSource {
    /// The system clipboard, which the program may be told about.
    Clipboard,
    /// Text the embedder supplied directly, such as a bracketed-paste
    /// injection.
    Text,
}

/// Which clipboard a paste came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardLocation {
    /// The standard clipboard.
    Standard,
    /// The X11 primary selection.
    Primary,
    /// The current terminal selection.
    Selection,
}

impl ClipboardLocation {
    fn raw(self) -> sys::GhosttyClipboardLocation {
        use sys as s;
        match self {
            Self::Standard => s::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD,
            Self::Primary => s::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_PRIMARY,
            Self::Selection => s::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_SELECTION,
        }
    }
}

/// One MIME-typed representation of pasted content.
pub struct PasteRepresentation<'data> {
    /// The MIME type, for example `text/plain;charset=utf-8`.
    pub mime: &'data str,
    /// The bytes of that representation.
    pub data: &'data [u8],
}

impl DisplayTerminal {
    /// Encode a size report without going through a terminal query.
    ///
    /// The terminal answers `CSI 14 t` and friends on its own through the
    /// size callback; this is for the cases where Paneflow has to synthesize
    /// one, such as replaying a resize into a recorded stream.
    pub fn encode_size_report(&self, style: SizeReportStyle, size: WindowSize) -> Result<Vec<u8>> {
        let size = sys::GhosttySizeReportSize {
            rows: size.rows,
            columns: size.cols,
            cell_width: size.cell_width,
            cell_height: size.cell_height,
        };
        encode_with_buffer(
            "size_report_encode",
            MAX_SIZE_REPORT_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_size_report_encode(style.raw(), size, buffer, len, written)
            },
        )
    }

    /// How much compression-relevant activity the terminal has seen.
    ///
    /// The counter only moves when something happened that compression could
    /// act on, so a caller can skip a pass whose input has not changed.
    pub fn compression_activity(&self) -> Result<u64> {
        let mut activity = 0u64;
        // SAFETY: the terminal handle is live and `activity` is valid
        // writable storage.
        let result =
            unsafe { sys::ghostty_terminal_compression_activity(self.terminal.raw(), &mut activity) };
        check("terminal_compression_activity", result)?;
        Ok(activity)
    }

    /// Compress scrollback, reclaiming memory from rows that are no longer on
    /// screen.
    ///
    /// Run this on an idle tick with [`CompressionMode::Incremental`] and
    /// stop once it reports [`CompressionOutcome::Complete`]. It walks the
    /// page list, so it does not belong on the render thread.
    pub fn compress(&mut self, mode: CompressionMode) -> Result<CompressionOutcome> {
        let mode = match mode {
            CompressionMode::Incremental => {
                sys::GhosttyTerminalCompressionMode_GHOSTTY_TERMINAL_COMPRESSION_MODE_INCREMENTAL
            }
            CompressionMode::Full => {
                sys::GhosttyTerminalCompressionMode_GHOSTTY_TERMINAL_COMPRESSION_MODE_FULL
            }
        };
        let mut outcome =
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE;
        // SAFETY: the terminal handle is live and `outcome` is valid writable
        // storage.
        let result =
            unsafe { sys::ghostty_terminal_compress(self.terminal.raw(), mode, &mut outcome) };
        check("terminal_compress", result)?;
        match outcome {
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_COMPLETE => {
                Ok(CompressionOutcome::Complete)
            }
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_PENDING => {
                Ok(CompressionOutcome::Pending)
            }
            sys::GhosttyTerminalCompressionResult_GHOSTTY_TERMINAL_COMPRESSION_RESULT_UNSUPPORTED => {
                Ok(CompressionOutcome::Unsupported)
            }
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown Ghostty compression result {other}"
            ))),
        }
    }

    /// Write only the shortest prefix of `bytes` needed to bring the parser
    /// back to the ground state.
    ///
    /// Ground is the stateless point of the stream: no UTF-8, ESC, CSI, or
    /// OSC sequence in flight. It is the only safe place to interleave
    /// Paneflow's own VT output with what the pty is producing, which is what
    /// this is for. A parser already at ground consumes nothing.
    pub fn feed_until_ground(&mut self, bytes: &[u8]) -> Result<GroundWrite> {
        let mut consumed = 0usize;
        // SAFETY: the terminal handle is live, `bytes` is borrowed for the
        // call, and `consumed` is valid writable storage.
        let result = unsafe {
            sys::ghostty_terminal_vt_write_until_ground(
                self.terminal.raw(),
                bytes.as_ptr(),
                bytes.len(),
                &mut consumed,
            )
        };
        // `NO_VALUE` means the whole slice went in without reaching ground,
        // which is a state, not a failure.
        let at_ground = result != sys::GhosttyResult_GHOSTTY_NO_VALUE;
        if at_ground {
            check("terminal_vt_write_until_ground", result)?;
        }
        if consumed > bytes.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "vt_write_until_ground consumed {consumed} of {} bytes",
                bytes.len()
            )));
        }
        if consumed > 0 {
            self.snapshot_cache.invalidate();
        }
        Ok(GroundWrite {
            consumed,
            at_ground,
        })
    }

    /// The escape sequence the parser is in the middle of, if any.
    ///
    /// Returns `None` when the parser is in the ground state. Requires the
    /// continuation budget to be configured through
    /// `GHOSTTY_TERMINAL_OPT_CONTINUATION_MAX_BYTES`; without it the terminal
    /// does not retain partial sequences.
    pub fn continuation(&self) -> Result<Option<Vec<u8>>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the terminal handle is live, the null allocator selects
        // libghostty's default, and both out-parameters are valid storage.
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
            // `INVALID_VALUE` here means continuation tracking is disabled,
            // which is a configuration state rather than a failure.
            return Ok(None);
        }
        check("terminal_continuation_alloc", result)?;
        if pointer.is_null() || len == 0 {
            return Ok(None);
        }
        // SAFETY: the library reported `len` initialized bytes at `pointer`.
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        let over_budget = len > MAX_CONTINUATION_BYTES;
        // SAFETY: the pointer and length are exactly what the allocating call
        // produced with the same default allocator.
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if over_budget {
            return Err(GhosttyError::LimitExceeded {
                resource: "continuation",
                limit: MAX_CONTINUATION_BYTES,
            });
        }
        Ok(Some(copied))
    }

    /// Write the pending continuation into `buffer`, returning its length.
    pub fn continuation_into(&self, buffer: &mut [u8]) -> Result<Option<usize>> {
        let mut written = 0usize;
        // SAFETY: the terminal handle is live, `buffer` is a writable slice
        // of the stated length, and `written` is valid storage.
        let result = unsafe {
            sys::ghostty_terminal_continuation_buf(
                self.terminal.raw(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE
            || result == sys::GhosttyResult_GHOSTTY_INVALID_VALUE
        {
            return Ok(None);
        }
        check("terminal_continuation_buf", result)?;
        if written > buffer.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "continuation_buf reported {written} bytes for a {}-byte buffer",
                buffer.len()
            )));
        }
        Ok(Some(written))
    }

    /// Stream the pending continuation to `sink`, which returns `false` to
    /// abort.
    pub fn continuation_to<F: FnMut(&[u8]) -> bool>(&self, mut sink: F) -> Result<bool> {
        let writer = sys::GhosttyWriter {
            write: Some(continuation_write_trampoline::<F>),
            userdata: (&raw mut sink).cast::<c_void>(),
        };
        // SAFETY: the terminal handle is live and `writer` borrows `sink` for
        // this synchronous call.
        let result = unsafe { sys::ghostty_terminal_continuation_write(self.terminal.raw(), writer) };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE
            || result == sys::GhosttyResult_GHOSTTY_INVALID_VALUE
        {
            return Ok(false);
        }
        check("terminal_continuation_write", result)?;
        Ok(true)
    }

    /// Paste content into the terminal the way the running program expects
    /// it.
    ///
    /// Unlike [`Self::encode_paste`], which only frames text for mode 2004,
    /// this dispatches on terminal state: with the Kitty clipboard protocol
    /// (mode 5522) active it sends a paste event that lets the program pull
    /// whichever representation it wants, and it falls back to bracketed
    /// paste otherwise. Output goes through the write-pty callback.
    ///
    /// Returns whether anything reached the pty. Unsafe text is rejected with
    /// [`GhosttyError::UnsafePaste`] unless `allow_unsafe` is set.
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
        // SAFETY: `paste` and everything it points at outlive this
        // synchronous call, and `written` is valid writable storage.
        let result =
            unsafe { sys::ghostty_terminal_paste(self.terminal.raw(), &paste, &mut written) };
        if result == sys::GhosttyResult_GHOSTTY_REJECTED {
            return Err(GhosttyError::UnsafePaste);
        }
        check("terminal_paste", result)?;
        Ok(written)
    }

    /// Read the grid geometry in one call: columns, total rows, scrollback
    /// rows.
    ///
    /// The individual getters each cross the FFI boundary; batching them is
    /// what `ghostty_terminal_get_multi` exists for.
    pub(crate) fn geometry_batch(&self) -> Result<(usize, usize, usize)> {
        let mut cols = 0usize;
        let mut total_rows = 0usize;
        let mut scrollback = 0usize;
        use sys as s;
        // SAFETY: every destination matches the output type terminal.h
        // documents for its key, and all of them outlive the call.
        unsafe {
            get_multi(
                "terminal_get_multi",
                self.terminal.raw(),
                sys::ghostty_terminal_get_multi,
                &[
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

/// Bridges the paste reader callback back to the representations the caller
/// supplied.
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
    // SAFETY: `userdata` is the `&mut PasteState` handed to
    // `ghostty_terminal_paste`, which calls this synchronously.
    let state = unsafe { &*userdata.cast::<PasteState<'_, '_>>() };
    // SAFETY: libghostty documents the string as borrowed for this call.
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
        // A zero-length write is not allowed, and an empty representation has
        // nothing to stream.
        return true;
    }
    // SAFETY: `write` is the callback libghostty supplied with its own
    // userdata, and the slice is live for the call.
    unsafe {
        write(
            writer.userdata,
            representation.data.as_ptr(),
            representation.data.len(),
        )
    }
}

unsafe extern "C" fn continuation_write_trampoline<F: FnMut(&[u8]) -> bool>(
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) -> bool {
    if userdata.is_null() || data.is_null() {
        return false;
    }
    // SAFETY: `userdata` is the `&mut F` handed to the continuation writer,
    // which libghostty calls synchronously.
    let sink = unsafe { &mut *userdata.cast::<F>() };
    // SAFETY: libghostty documents `data`/`len` as borrowed for the call.
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(bytes))).unwrap_or(false)
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
    fn size_reports_encode_each_style() {
        let terminal = terminal(80, 24);
        let size = WindowSize::new(80, 24, 8, 16).expect("valid size");

        assert_eq!(
            terminal
                .encode_size_report(SizeReportStyle::Mode2048, size)
                .expect("mode 2048"),
            b"\x1b[48;24;80;384;640t"
        );
        assert_eq!(
            terminal
                .encode_size_report(SizeReportStyle::Csi18T, size)
                .expect("csi 18 t"),
            b"\x1b[8;24;80t"
        );
        assert_eq!(
            terminal
                .encode_size_report(SizeReportStyle::Csi14T, size)
                .expect("csi 14 t"),
            b"\x1b[4;384;640t"
        );
        assert_eq!(
            terminal
                .encode_size_report(SizeReportStyle::Csi16T, size)
                .expect("csi 16 t"),
            b"\x1b[6;16;8t"
        );
    }

    #[test]
    fn writing_until_ground_consumes_only_what_closes_the_sequence() {
        let mut terminal = terminal(20, 3);
        // A fresh parser is already at ground, so nothing is consumed.
        assert_eq!(
            terminal
                .feed_until_ground(b"abc")
                .expect("write must succeed"),
            GroundWrite {
                consumed: 0,
                at_ground: true
            }
        );

        // Leave the parser inside a CSI, then close it: only the final byte
        // is consumed and the rest is left for the caller to write normally.
        terminal.feed(b"\x1b[1").expect("partial sequence");
        assert_eq!(
            terminal
                .feed_until_ground(b"mtail")
                .expect("write must succeed"),
            GroundWrite {
                consumed: 1,
                at_ground: true
            }
        );

        // A slice that never closes the sequence is consumed whole and the
        // parser stays off ground.
        terminal.feed(b"\x1b[").expect("partial sequence");
        assert_eq!(
            terminal
                .feed_until_ground(b"12;3")
                .expect("write must succeed"),
            GroundWrite {
                consumed: 4,
                at_ground: false
            }
        );
    }

    #[test]
    fn compression_reports_progress_and_tracks_activity() {
        let mut terminal = terminal(20, 4);
        let idle = terminal.compression_activity().expect("activity");
        for index in 0..200 {
            terminal
                .feed(format!("line {index}\r\n").as_bytes())
                .expect("output must parse");
        }
        assert!(
            terminal.compression_activity().expect("activity") > idle,
            "scrollback churn must register as activity"
        );

        let outcome = terminal
            .compress(CompressionMode::Incremental)
            .expect("incremental pass");
        assert!(matches!(
            outcome,
            CompressionOutcome::Complete
                | CompressionOutcome::Pending
                | CompressionOutcome::Unsupported
        ));

        if outcome != CompressionOutcome::Unsupported {
            let mut passes = 0;
            let mut outcome = outcome;
            while outcome == CompressionOutcome::Pending && passes < 1000 {
                outcome = terminal.compress(CompressionMode::Full).expect("full pass");
                passes += 1;
            }
            assert_eq!(outcome, CompressionOutcome::Complete);
        }
    }

    #[test]
    fn the_batched_geometry_read_matches_the_individual_ones() {
        let mut terminal = terminal(40, 8);
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine")
            .expect("output must parse");
        let (cols, total_rows, scrollback) = terminal.geometry_batch().expect("batched read");
        assert_eq!(cols, 40);
        assert_eq!(total_rows, terminal.total_rows().expect("total rows"));
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
        // The constructor does not set a continuation budget, so libghostty
        // retains nothing and every accessor agrees on that.
        assert!(terminal.continuation().expect("alloc path").is_none());
        let mut buffer = [0u8; 32];
        assert!(
            terminal
                .continuation_into(&mut buffer)
                .expect("buffered path")
                .is_none()
        );
        assert!(!terminal.continuation_to(|_| true).expect("stream path"));
    }
}
