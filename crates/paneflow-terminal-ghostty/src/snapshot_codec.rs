//! Encode and restore a complete terminal as a binary snapshot.
//!
//! [`DisplayTerminal::extract_scrollback`] keeps plain text, which loses
//! styling, modes, the cursor, and any half-parsed escape sequence. A snapshot
//! is libghostty's own record stream: it carries the active grid, the
//! scrollback pages, terminal state, and the VT parser's continuation, so a
//! restored terminal resumes rather than merely looks similar.
//!
//! Restoring has two shapes. [`SnapshotDecoder::decode`] reads the whole
//! stream in one call. [`SnapshotDecoder::ready`] stops at the READY marker,
//! which is the point where the terminal is renderable, and
//! [`SnapshotDecoder::next_page`] then prepends one scrollback page at a time so a
//! long history does not stall the first frame.

use std::ffi::c_void;
use std::marker::PhantomData;

use paneflow_libghostty_sys as sys;

use crate::batch::{Slot, get_multi};
use crate::callbacks::{self, CallbackState};
use crate::constructor::{configure_appearance, configure_safety_limits, configure_scrollback};
use crate::engine::{DisplayTerminal, resize_terminal};
use crate::handles::{OwnedHandle, check};
use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::{GhosttyError, Result, TerminalAppearance, WindowSize};

/// Ceiling on a single encoded snapshot, matching the caps the rest of the
/// crate applies to unbounded terminal data.
const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

/// Which of a terminal's two screens a history page belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalScreen {
    /// The normal screen, the one that owns the scrollback.
    Primary,
    /// The alternate screen used by full-screen programs.
    Alternate,
}

impl TerminalScreen {
    fn from_raw(raw: sys::GhosttyTerminalScreen) -> Result<Self> {
        match raw {
            sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY => Ok(Self::Primary),
            sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE => Ok(Self::Alternate),
            other => Err(GhosttyError::AbiMismatch(format!(
                "unknown terminal screen {other}"
            ))),
        }
    }
}

/// The embedder state a snapshot does not carry.
///
/// Cell metrics belong to the renderer, not to the grid, and the scrollback
/// budget and theme are the embedder's policy rather than the encoded
/// terminal's. Everything else comes out of the snapshot.
#[derive(Clone, Copy, Debug)]
pub struct SnapshotRestore {
    /// Cell width in pixels, for mouse pixel reporting and Kitty graphics.
    pub cell_width: u32,
    /// Cell height in pixels.
    pub cell_height: u32,
    /// Scrollback line budget to pin on the restored terminal.
    pub max_scrollback: usize,
    /// Default colors and color scheme to apply after restoring.
    pub appearance: TerminalAppearance,
}

/// What one [`SnapshotDecoder::next_page`] call restored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryProgress {
    /// The screen the page was prepended to.
    pub screen: TerminalScreen,
    /// Rows actually prepended. Zero means the page was valid but could no
    /// longer be applied, which happens when the terminal was resized between
    /// calls.
    pub rows: usize,
    /// Pages left in this screen's history sequence, not in the snapshot.
    pub remaining: u32,
}

impl DisplayTerminal {
    /// Encode a complete snapshot into an owned buffer.
    ///
    /// Fails with [`GhosttyError::Ffi`] when the parser is mid-sequence and
    /// continuation tracking was never enabled; see
    /// [`Self::set_continuation_max_bytes`].
    pub fn encode_snapshot(&self) -> Result<Vec<u8>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the terminal handle is live, the null allocator selects
        // libghostty's default, and both out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_snapshot_encode_alloc(
                self.terminal.raw(),
                std::ptr::null(),
                &mut pointer,
                &mut len,
            )
        };
        check("snapshot_encode_alloc", result)?;
        if pointer.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot_encode_alloc returned a null buffer".into(),
            ));
        }
        // The buffer belongs to libghostty's allocator, so it is copied and
        // released rather than adopted by Rust's allocator.
        // SAFETY: the library reported `len` initialized bytes at `pointer`.
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        // SAFETY: `pointer`/`len` are exactly what `encode_alloc` produced
        // with the same (default) allocator, and nothing else owns them.
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if copied.len() > MAX_SNAPSHOT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "encoded snapshot",
                limit: MAX_SNAPSHOT_BYTES,
            });
        }
        Ok(copied)
    }

    /// The buffer size [`Self::encode_snapshot_into`] needs right now.
    ///
    /// Every write to the terminal can change it, so treat it as a size for
    /// the current state rather than a reusable capacity.
    pub fn encode_snapshot_size(&self) -> Result<usize> {
        let mut needed = 0usize;
        // SAFETY: a null buffer with a zero length is the documented
        // size query, and `needed` is valid writable storage.
        let result = unsafe {
            sys::ghostty_snapshot_encode_buf(
                self.terminal.raw(),
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        // The query reports the required capacity through the same
        // out-of-space path an undersized buffer takes.
        if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            check("snapshot_encode_buf_size", result)?;
        }
        Ok(needed)
    }

    /// Encode a complete snapshot into `buffer`, returning its length.
    ///
    /// Fails when `buffer` is too small; size it with
    /// [`Self::encode_snapshot_size`] or use [`Self::encode_snapshot`].
    pub fn encode_snapshot_into(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut written = 0usize;
        // SAFETY: the terminal handle is live, `buffer` is a writable slice of
        // the stated length, and `written` is valid storage.
        let result = unsafe {
            sys::ghostty_snapshot_encode_buf(
                self.terminal.raw(),
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        check("snapshot_encode_buf", result)?;
        if written > buffer.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "snapshot_encode_buf reported {written} bytes for a {}-byte buffer",
                buffer.len()
            )));
        }
        Ok(written)
    }

    /// Stream a complete snapshot to `sink`, which returns `false` to abort.
    ///
    /// This is the path for writing straight to a file: nothing larger than
    /// libghostty's internal chunk is ever held in memory.
    pub fn encode_snapshot_to<F: FnMut(&[u8]) -> bool>(&self, mut sink: F) -> Result<()> {
        let writer = crate::io::writer(&mut sink);
        // SAFETY: the terminal handle is live and `writer` borrows `sink` for
        // the duration of this synchronous call.
        let result = unsafe { sys::ghostty_snapshot_encode(self.terminal.raw(), writer) };
        check("snapshot_encode", result)
    }
}

/// A byte source the decoder pulls from.
///
/// Boxed twice so the outer allocation gives libghostty a thin, stable
/// userdata pointer for the whole life of the decoder.
type BoxedSource<'src> = Box<dyn FnMut(&mut [u8]) -> Option<usize> + 'src>;

/// Restores a terminal from an encoded snapshot.
///
/// The decoder owns the terminal it produces until the caller takes it with
/// [`Self::into_terminal`], because libghostty keeps applying history pages to
/// that exact handle. Dropping the decoder first is what makes that borrow
/// safe without a lifetime on the terminal.
pub struct SnapshotDecoder<'src> {
    raw: sys::GhosttySnapshotDecoder,
    terminal: Option<DisplayTerminal>,
    /// Kept alive because libghostty stores a pointer into it. Declared after
    /// `raw` only for readability: `Drop` frees the decoder before any field.
    _source: Option<Box<BoxedSource<'src>>>,
    /// Borrowed snapshot bytes, for the [`Self::from_bytes`] source.
    _borrowed: PhantomData<&'src [u8]>,
}

impl Drop for SnapshotDecoder<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from a decoder constructor, is private, and Drop
        // runs exactly once. Freeing it releases libghostty's borrow of both
        // the source and the terminal, which drop after this returns.
        unsafe { sys::ghostty_snapshot_decoder_free(self.raw) };
    }
}

impl<'src> SnapshotDecoder<'src> {
    /// Decode from bytes already in memory.
    ///
    /// libghostty borrows `snapshot` rather than copying it, which the
    /// lifetime enforces. Bytes after the FINISH marker are left untouched;
    /// [`Self::source_offset`] locates them.
    pub fn from_bytes(snapshot: &'src [u8]) -> Result<Self> {
        crate::abi::validate()?;
        let mut raw: sys::GhosttySnapshotDecoder = std::ptr::null_mut();
        // SAFETY: the null allocator selects libghostty's default, `raw` is
        // valid writable storage, and `snapshot` outlives the decoder through
        // the returned lifetime.
        let result = unsafe {
            sys::ghostty_snapshot_decoder_new_buf(
                std::ptr::null(),
                &mut raw,
                snapshot.as_ptr(),
                snapshot.len(),
            )
        };
        check("snapshot_decoder_new_buf", result)?;
        Self::wrap(raw, None)
    }

    /// Decode from a streaming source.
    ///
    /// `read` fills the buffer and returns how many bytes it wrote, `Some(0)`
    /// for end of input, or `None` to report an I/O error. A short read is
    /// fine; a zero-byte read is permanent end of file, not starvation, so a
    /// nonblocking source has to block inside the closure or buffer outside
    /// the decoder.
    pub fn from_reader<F: FnMut(&mut [u8]) -> Option<usize> + 'src>(read: F) -> Result<Self> {
        crate::abi::validate()?;
        let mut source: Box<BoxedSource<'src>> = Box::new(Box::new(read));
        let reader = crate::io::reader(&mut *source);
        let mut raw: sys::GhosttySnapshotDecoder = std::ptr::null_mut();
        // SAFETY: the null allocator selects libghostty's default, `raw` is
        // valid writable storage, and `source` is moved into the decoder
        // below, so the pointer inside `reader` stays valid until Drop frees
        // the decoder.
        let result =
            unsafe { sys::ghostty_snapshot_decoder_new(std::ptr::null(), &mut raw, reader) };
        check("snapshot_decoder_new", result)?;
        Self::wrap(raw, Some(source))
    }

    fn wrap(raw: sys::GhosttySnapshotDecoder, source: Option<Box<BoxedSource<'src>>>) -> Result<Self> {
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot decoder constructor returned a null handle".into(),
            ));
        }
        Ok(Self {
            raw,
            terminal: None,
            _source: source,
            _borrowed: PhantomData,
        })
    }

    /// Reject a snapshot whose unfinished VT input exceeds `bytes`.
    ///
    /// With [`Self::set_retain_continuation`] on, this also becomes the
    /// restored terminal's continuation budget. Zero accepts only snapshots
    /// whose parser was at ground. Must be called before decoding starts.
    pub fn set_max_continuation_bytes(&mut self, bytes: usize) -> Result<()> {
        // SAFETY: the decoder is live and the option's documented input type
        // is `size_t *`.
        let result = unsafe {
            sys::ghostty_snapshot_decoder_set(
                self.raw,
                sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
                (&raw const bytes).cast::<c_void>(),
            )
        };
        check("snapshot_decoder_set_max_continuation_bytes", result)
    }

    /// Keep the restored terminal tracking its continuation.
    ///
    /// Off by default: the snapshot's unfinished sequence is always replayed
    /// into the parser, but the terminal stops retaining it, so re-encoding
    /// the restored terminal mid-sequence would fail. Turn it on to snapshot
    /// a restored terminal again. Must be called before decoding starts.
    pub fn set_retain_continuation(&mut self, retain: bool) -> Result<()> {
        // SAFETY: the decoder is live and the option's documented input type
        // is `bool *`.
        let result = unsafe {
            sys::ghostty_snapshot_decoder_set(
                self.raw,
                sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
                (&raw const retain).cast::<c_void>(),
            )
        };
        check("snapshot_decoder_set_retain_continuation", result)
    }

    /// Restore just enough to render, stopping at the READY marker.
    ///
    /// The terminal is immediately usable, including for live pty input, while
    /// [`Self::next_page`] prepends the remaining scrollback.
    pub fn ready(&mut self, restore: SnapshotRestore) -> Result<&mut DisplayTerminal> {
        self.produce(restore, sys::ghostty_snapshot_decoder_ready, "snapshot_decoder_ready")
    }

    /// Restore the whole snapshot, history included, in one call.
    pub fn decode(&mut self, restore: SnapshotRestore) -> Result<&mut DisplayTerminal> {
        self.produce(restore, sys::ghostty_snapshot_decoder_decode, "snapshot_decoder_decode")
    }

    fn produce(
        &mut self,
        restore: SnapshotRestore,
        call: unsafe extern "C" fn(
            sys::GhosttySnapshotDecoder,
            *mut sys::GhosttyTerminal,
        ) -> sys::GhosttyResult,
        operation: &'static str,
    ) -> Result<&mut DisplayTerminal> {
        if self.terminal.is_some() {
            return Err(GhosttyError::AbiMismatch(format!(
                "{operation} called on a decoder that already produced a terminal"
            )));
        }
        if restore.max_scrollback > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        let mut raw_terminal: sys::GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: the decoder is live and `raw_terminal` is valid writable
        // storage. libghostty sets it to NULL on every error.
        let result = unsafe { call(self.raw, &mut raw_terminal) };
        check(operation, result)?;
        // SAFETY: the call succeeded, so this is a live terminal the caller
        // now owns, allocated with the decoder's (default) allocator.
        let terminal = unsafe { adopt(raw_terminal, restore) }?;
        Ok(self.terminal.insert(terminal))
    }

    /// Prepend one scrollback page, or `None` once FINISH validates.
    ///
    /// The terminal may be rendered, resized, and fed live pty input between
    /// calls. A page that no longer fits the current geometry is still
    /// consumed and validated, and reports zero rows.
    pub fn next_page(&mut self) -> Result<Option<HistoryProgress>> {
        if self.terminal.is_none() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot_decoder_next called before ready".into(),
            ));
        }
        // SAFETY: the decoder is live and still owns the terminal it produced.
        let result = unsafe { sys::ghostty_snapshot_decoder_next(self.raw) };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check("snapshot_decoder_next", result)?;
        let mut screen: sys::GhosttyTerminalScreen =
            sys::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_PRIMARY;
        let mut rows = 0usize;
        let mut remaining = 0u32;
        use sys as s;
        // SAFETY: every destination matches the output type snapshot.h
        // documents for its key, and all of them outlive the call.
        unsafe {
            get_multi(
                "snapshot_decoder_get_multi",
                self.raw,
                sys::ghostty_snapshot_decoder_get_multi,
                [
                    Slot::new(
                        s::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_SCREEN,
                        &mut screen,
                    ),
                    Slot::new(
                        s::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_ROWS,
                        &mut rows,
                    ),
                    Slot::new(
                        s::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_PROGRESS_REMAINING,
                        &mut remaining,
                    ),
                ],
            )?;
        }
        Ok(Some(HistoryProgress {
            screen: TerminalScreen::from_raw(screen)?,
            rows,
            remaining,
        }))
    }

    /// The terminal restored so far, if decoding has reached READY.
    pub fn terminal(&mut self) -> Option<&mut DisplayTerminal> {
        self.terminal.as_mut()
    }

    /// Take the restored terminal, ending the decode.
    ///
    /// Abandoning an incremental decode this way is supported: the terminal
    /// keeps whatever history had already been prepended.
    #[must_use]
    pub fn into_terminal(mut self) -> Option<DisplayTerminal> {
        // Freeing the decoder happens when `self` drops at the end of this
        // call, which is after the terminal has left it.
        self.terminal.take()
    }

    /// Snapshot bytes consumed so far.
    ///
    /// After FINISH this is the offset of the first trailing byte, which is
    /// how a snapshot embedded in a larger stream is skipped.
    pub fn source_offset(&self) -> Result<usize> {
        self.get("snapshot_decoder_source_offset",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET,
            0usize)
    }

    /// The continuation ceiling currently in force.
    pub fn max_continuation_bytes(&self) -> Result<usize> {
        self.get("snapshot_decoder_max_continuation_bytes",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_MAX_CONTINUATION_BYTES,
            0usize)
    }

    /// Whether restored terminals keep tracking their continuation.
    pub fn retains_continuation(&self) -> Result<bool> {
        self.get("snapshot_decoder_retain_continuation",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_RETAIN_CONTINUATION,
            false)
    }

    /// How many history rows the snapshot declares for `screen`.
    ///
    /// Advisory, and only available once READY validates: it is the total the
    /// encoder saw, which lets a caller show progress while [`Self::next_page`]
    /// works through the pages. `None` means the snapshot declares no such
    /// screen.
    pub fn history_rows(&self, screen: TerminalScreen) -> Result<Option<u64>> {
        let (operation, key) = match screen {
            TerminalScreen::Primary => (
                "snapshot_decoder_history_rows_primary",
                sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_HISTORY_ROWS_PRIMARY,
            ),
            TerminalScreen::Alternate => (
                "snapshot_decoder_history_rows_alternate",
                sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_HISTORY_ROWS_ALTERNATE,
            ),
        };
        let mut value = 0u64;
        // SAFETY: the decoder is live and `value` has the `uint64_t *` output
        // type snapshot.h documents for both history keys.
        let result = unsafe {
            sys::ghostty_snapshot_decoder_get(self.raw, key, (&raw mut value).cast::<c_void>())
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(None);
        }
        check(operation, result)?;
        Ok(Some(value))
    }

    fn get<T>(
        &self,
        operation: &'static str,
        key: sys::GhosttySnapshotDecoderData,
        mut value: T,
    ) -> Result<T> {
        // SAFETY: the decoder is live and each caller passes storage of the
        // output type snapshot.h documents for its key.
        let result = unsafe {
            sys::ghostty_snapshot_decoder_get(self.raw, key, (&raw mut value).cast::<c_void>())
        };
        check(operation, result)?;
        Ok(value)
    }
}

/// Turn a decoder-produced terminal handle into a usable [`DisplayTerminal`].
///
/// The snapshot restores the grid and terminal state but knows nothing about
/// the embedder: no callbacks are installed, no pixel cell metrics are set,
/// and the safety limits the constructor pins are back at their defaults.
///
/// # Safety
///
/// `raw` must be a live terminal the caller owns, allocated with libghostty's
/// default allocator, with no callbacks installed.
unsafe fn adopt(raw: sys::GhosttyTerminal, restore: SnapshotRestore) -> Result<DisplayTerminal> {
    if raw.is_null() {
        return Err(GhosttyError::AbiMismatch(
            "snapshot decoder returned a null terminal".into(),
        ));
    }
    // SAFETY: the caller guarantees a live, uniquely owned handle, and
    // `terminal_free` is its matching destructor.
    let terminal = unsafe { OwnedHandle::from_raw(raw, sys::ghostty_terminal_free) };
    let mut cols = 0u16;
    let mut rows = 0u16;
    use sys as s;
    // SAFETY: terminal.h documents both keys as `uint16_t *`, and both
    // destinations outlive the call.
    unsafe {
        get_multi(
            "terminal_get_multi",
            terminal.raw(),
            sys::ghostty_terminal_get_multi,
            [
                Slot::new(s::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_COLS, &mut cols),
                Slot::new(s::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_ROWS, &mut rows),
            ],
        )?;
    }
    let size = WindowSize {
        cols,
        rows,
        cell_width: restore.cell_width,
        cell_height: restore.cell_height,
    }
    .validate()?;
    let mut callbacks = Box::new(CallbackState::new(size, restore.appearance.color_scheme));
    callbacks::install(terminal.raw(), (&mut *callbacks) as *mut CallbackState)?;
    configure_scrollback(terminal.raw(), restore.max_scrollback)?;
    configure_safety_limits(terminal.raw())?;
    configure_appearance(terminal.raw(), restore.appearance)?;
    // A snapshot carries the grid, not the renderer's pixel metrics, so this
    // resize keeps the same cols and rows and only pins the cell size the
    // mouse encoder and Kitty graphics need.
    resize_terminal(terminal.raw(), size)?;
    // SAFETY: the callbacks are installed above and the null allocator is the
    // one the decoder used, so it outlives every handle `assemble` creates.
    unsafe { DisplayTerminal::assemble(terminal, callbacks, std::ptr::null()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Point, Rgb};

    fn restore() -> SnapshotRestore {
        SnapshotRestore {
            cell_width: 8,
            cell_height: 16,
            max_scrollback: 100,
            appearance: TerminalAppearance::default(),
        }
    }

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    fn visible(terminal: &mut DisplayTerminal) -> String {
        terminal
            .snapshot()
            .expect("snapshot must render")
            .cells
            .iter()
            .map(|cell| cell.character)
            .collect()
    }

    #[test]
    fn a_round_trip_restores_the_grid_cursor_and_scrollback() {
        let mut source = terminal(12, 3);
        source
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfi\x1b[1;32mve")
            .expect("fixture output must parse");
        let before = visible(&mut source);
        let history = source.snapshot().expect("snapshot").history_size;
        let cursor = source.snapshot().expect("snapshot").cursor.point;
        assert!(history > 0, "the fixture must overflow into scrollback");

        let encoded = source.encode_snapshot().expect("terminal must encode");
        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        decoder.decode(restore()).expect("snapshot must decode");
        let mut restored = decoder.into_terminal().expect("decode produces a terminal");

        assert_eq!(visible(&mut restored), before);
        let content = restored.snapshot().expect("restored snapshot");
        assert_eq!(content.history_size, history);
        assert_eq!(content.cursor.point, cursor);
        assert_ne!(cursor, Point::new(0, 0));
    }

    #[test]
    fn styling_survives_a_round_trip() {
        let mut source = terminal(10, 2);
        source
            .feed(b"\x1b[38;2;10;20;30mx\x1b[0m")
            .expect("styled output must parse");

        let encoded = source.encode_snapshot().expect("terminal must encode");
        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        decoder.decode(restore()).expect("snapshot must decode");
        let mut restored = decoder.into_terminal().expect("decode produces a terminal");

        let content = restored.snapshot().expect("restored snapshot");
        let cell = content
            .cells
            .iter()
            .find(|cell| cell.character == 'x')
            .expect("the styled cell must survive");
        assert_eq!(
            cell.foreground,
            crate::Color::Rgb(Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    #[test]
    fn the_three_encoders_agree_byte_for_byte() {
        let mut source = terminal(20, 4);
        source.feed(b"agree").expect("fixture output must parse");

        let allocated = source.encode_snapshot().expect("alloc path");
        assert_eq!(
            source.encode_snapshot_size().expect("size query"),
            allocated.len()
        );

        let mut buffer = vec![0u8; allocated.len()];
        let written = source
            .encode_snapshot_into(&mut buffer)
            .expect("buffered path");
        assert_eq!(&buffer[..written], allocated.as_slice());

        let mut streamed = Vec::new();
        source
            .encode_snapshot_to(|bytes| {
                streamed.extend_from_slice(bytes);
                true
            })
            .expect("streaming path");
        assert_eq!(streamed, allocated);
    }

    #[test]
    fn an_aborted_sink_reports_an_io_error() {
        let mut source = terminal(10, 2);
        source.feed(b"abort").expect("fixture output must parse");
        let result = source.encode_snapshot_to(|_| false);
        assert!(matches!(result, Err(GhosttyError::Ffi { .. })));
    }

    #[test]
    fn ready_renders_before_history_is_restored() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut source = DisplayTerminal::new(size, 50_000, TerminalAppearance::default())
            .expect("terminal must initialize");
        // Enough history to spill past the page libghostty carries before
        // READY, which is what leaves pages for `next_page` to prepend.
        let mut fixture = Vec::new();
        for line in 0..40_000 {
            fixture.extend_from_slice(format!("line-{line:06}-padding-text\r\n").as_bytes());
        }
        source.feed(&fixture).expect("fixture output must parse");
        let expected_history = source.snapshot().expect("snapshot").history_size;
        let encoded = source.encode_snapshot().expect("terminal must encode");

        let restore = SnapshotRestore {
            max_scrollback: 50_000,
            ..restore()
        };
        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        let restored = decoder.ready(restore).expect("prefix must decode");
        let at_ready = restored.snapshot().expect("ready snapshot").history_size;
        assert!(
            at_ready < expected_history,
            "history must still be pending: {at_ready} of {expected_history}"
        );
        assert_eq!(
            decoder
                .history_rows(TerminalScreen::Primary)
                .expect("advisory history rows"),
            Some(expected_history as u64)
        );

        let mut pages = 0;
        let mut prepended = 0usize;
        while let Some(progress) = decoder.next_page().expect("history page") {
            assert_eq!(progress.screen, TerminalScreen::Primary);
            prepended += progress.rows;
            pages += 1;
        }
        assert!(pages > 0, "the fixture must carry at least one page");
        assert_eq!(prepended + at_ready, expected_history);

        let mut restored = decoder.into_terminal().expect("ready produces a terminal");
        assert_eq!(
            restored.snapshot().expect("final snapshot").history_size,
            expected_history
        );
    }

    #[test]
    fn a_streaming_source_decodes_the_same_terminal() {
        let mut source = terminal(10, 2);
        source.feed(b"stream").expect("fixture output must parse");
        let encoded = source.encode_snapshot().expect("terminal must encode");
        let expected = visible(&mut source);

        // Deliberately tiny reads: the decoder must not assume it gets the
        // whole record it asked for in one callback.
        let mut offset = 0usize;
        let mut decoder = SnapshotDecoder::from_reader(|buffer| {
            let take = buffer.len().min(7).min(encoded.len() - offset);
            buffer[..take].copy_from_slice(&encoded[offset..offset + take]);
            offset += take;
            Some(take)
        })
        .expect("decoder must open");
        decoder.decode(restore()).expect("snapshot must decode");
        let mut restored = decoder.into_terminal().expect("decode produces a terminal");

        assert_eq!(visible(&mut restored), expected);
    }

    #[test]
    fn a_failing_source_reports_an_io_error_instead_of_truncating() {
        let mut source = terminal(10, 2);
        source.feed(b"broken").expect("fixture output must parse");
        let encoded = source.encode_snapshot().expect("terminal must encode");

        let mut served = 0usize;
        let mut decoder = SnapshotDecoder::from_reader(|buffer| {
            if served >= 16 {
                return None;
            }
            let take = buffer.len().min(16 - served).min(encoded.len() - served);
            buffer[..take].copy_from_slice(&encoded[served..served + take]);
            served += take;
            Some(take)
        })
        .expect("decoder must open");
        assert!(decoder.decode(restore()).is_err());
        assert!(decoder.into_terminal().is_none());
    }

    #[test]
    fn trailing_bytes_are_left_for_the_caller() {
        let mut source = terminal(10, 2);
        source.feed(b"tail").expect("fixture output must parse");
        let mut stream = source.encode_snapshot().expect("terminal must encode");
        let snapshot_len = stream.len();
        stream.extend_from_slice(b"not-snapshot-bytes");

        let mut decoder = SnapshotDecoder::from_bytes(&stream).expect("decoder must open");
        decoder.decode(restore()).expect("snapshot must decode");
        assert_eq!(
            decoder.source_offset().expect("consumed offset"),
            snapshot_len
        );
    }

    #[test]
    fn an_unfinished_sequence_needs_continuation_tracking_on_both_sides() {
        let mut source = terminal(10, 2);
        // Without a budget the parser state is unrecoverable and libghostty
        // refuses to encode a terminal it could not faithfully restore.
        source.feed(b"\x1b[1;2").expect("partial CSI must parse");
        assert!(matches!(
            source.encode_snapshot(),
            Err(GhosttyError::Ffi { .. })
        ));

        let mut source = terminal(10, 2);
        source
            .set_continuation_max_bytes(4096)
            .expect("tracking must enable");
        source.feed(b"\x1b[3").expect("partial CSI must parse");
        let encoded = source.encode_snapshot().expect("terminal must encode");

        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        decoder
            .set_max_continuation_bytes(4096)
            .expect("budget must apply");
        decoder
            .set_retain_continuation(true)
            .expect("retention must apply");
        assert!(decoder.retains_continuation().expect("retention readback"));
        assert_eq!(
            decoder
                .max_continuation_bytes()
                .expect("budget readback"),
            4096
        );
        decoder.decode(restore()).expect("snapshot must decode");
        let mut restored = decoder.into_terminal().expect("decode produces a terminal");

        assert_eq!(
            restored.continuation().expect("restored continuation"),
            Some(b"\x1b[3".to_vec())
        );
        // The parser really is mid-sequence: the rest of the CSI completes it
        // instead of printing.
        restored.feed(b"J").expect("sequence tail must parse");
        assert!(!visible(&mut restored).contains('J'));
    }

    #[test]
    fn options_are_rejected_once_decoding_has_started() {
        let mut source = terminal(10, 2);
        source.feed(b"late").expect("fixture output must parse");
        let encoded = source.encode_snapshot().expect("terminal must encode");

        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        decoder.ready(restore()).expect("prefix must decode");
        assert!(decoder.set_retain_continuation(true).is_err());
        assert!(decoder.ready(restore()).is_err());
    }

    #[test]
    fn a_restored_terminal_still_reports_events_and_resizes() {
        let mut source = terminal(10, 2);
        source.feed(b"live").expect("fixture output must parse");
        let encoded = source.encode_snapshot().expect("terminal must encode");

        let mut decoder = SnapshotDecoder::from_bytes(&encoded).expect("decoder must open");
        decoder.decode(restore()).expect("snapshot must decode");
        let mut restored = decoder.into_terminal().expect("decode produces a terminal");

        // Callbacks are the embedder's, so the decoder had to install them:
        // a fresh handle from libghostty has none.
        restored
            .feed(b"\x1b]0;restored\x07")
            .expect("title report must parse");
        assert!(
            restored
                .drain_events()
                .iter()
                .any(|event| matches!(event, crate::BackendEvent::Title(title) if title == "restored"))
        );

        restored
            .resize(WindowSize::new(20, 4, 8, 16).expect("valid size"))
            .expect("restored terminal must resize");
        let content = restored.snapshot().expect("resized snapshot");
        assert_eq!((content.cols, content.rows), (20, 4));
    }
}
