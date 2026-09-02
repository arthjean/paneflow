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

const MAX_SNAPSHOT_BYTES: usize = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalScreen {
    Primary,
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

#[derive(Clone, Copy, Debug)]
pub struct SnapshotRestore {
    pub cell_width: u32,
    pub cell_height: u32,
    pub max_scrollback: usize,
    pub appearance: TerminalAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HistoryProgress {
    pub screen: TerminalScreen,
    pub rows: usize,
    pub remaining: u32,
}

impl DisplayTerminal {
    pub fn encode_snapshot(&self) -> Result<Vec<u8>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
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
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if copied.len() > MAX_SNAPSHOT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "encoded snapshot",
                limit: MAX_SNAPSHOT_BYTES,
            });
        }
        Ok(copied)
    }

    pub fn encode_snapshot_size(&self) -> Result<usize> {
        let mut needed = 0usize;
        let result = unsafe {
            sys::ghostty_snapshot_encode_buf(
                self.terminal.raw(),
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if result != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
            check("snapshot_encode_buf_size", result)?;
        }
        Ok(needed)
    }

    pub fn encode_snapshot_into(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut written = 0usize;
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

    pub fn encode_snapshot_to<F: FnMut(&[u8]) -> bool>(&self, mut sink: F) -> Result<()> {
        let writer = crate::io::writer(&mut sink);
        let result = unsafe { sys::ghostty_snapshot_encode(self.terminal.raw(), writer) };
        check("snapshot_encode", result)
    }
}

type BoxedSource<'src> = Box<dyn FnMut(&mut [u8]) -> Option<usize> + 'src>;

pub struct SnapshotDecoder<'src> {
    raw: sys::GhosttySnapshotDecoder,
    terminal: Option<DisplayTerminal>,
    _source: Option<Box<BoxedSource<'src>>>,
    _borrowed: PhantomData<&'src [u8]>,
}

impl Drop for SnapshotDecoder<'_> {
    fn drop(&mut self) {
        unsafe { sys::ghostty_snapshot_decoder_free(self.raw) };
    }
}

impl<'src> SnapshotDecoder<'src> {
    pub fn from_bytes(snapshot: &'src [u8]) -> Result<Self> {
        crate::abi::validate()?;
        let mut raw: sys::GhosttySnapshotDecoder = std::ptr::null_mut();
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

    pub fn from_reader<F: FnMut(&mut [u8]) -> Option<usize> + 'src>(read: F) -> Result<Self> {
        crate::abi::validate()?;
        let mut source: Box<BoxedSource<'src>> = Box::new(Box::new(read));
        let reader = crate::io::reader(&mut *source);
        let mut raw: sys::GhosttySnapshotDecoder = std::ptr::null_mut();
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

    pub fn set_max_continuation_bytes(&mut self, bytes: usize) -> Result<()> {
        let result = unsafe {
            sys::ghostty_snapshot_decoder_set(
                self.raw,
                sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_MAX_CONTINUATION_BYTES,
                (&raw const bytes).cast::<c_void>(),
            )
        };
        check("snapshot_decoder_set_max_continuation_bytes", result)
    }

    pub fn set_retain_continuation(&mut self, retain: bool) -> Result<()> {
        let result = unsafe {
            sys::ghostty_snapshot_decoder_set(
                self.raw,
                sys::GhosttySnapshotDecoderOption_GHOSTTY_SNAPSHOT_DECODER_OPT_RETAIN_CONTINUATION,
                (&raw const retain).cast::<c_void>(),
            )
        };
        check("snapshot_decoder_set_retain_continuation", result)
    }

    pub fn ready(&mut self, restore: SnapshotRestore) -> Result<&mut DisplayTerminal> {
        self.produce(restore, sys::ghostty_snapshot_decoder_ready, "snapshot_decoder_ready")
    }

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
        let result = unsafe { call(self.raw, &mut raw_terminal) };
        check(operation, result)?;
        let terminal = unsafe { adopt(raw_terminal, restore) }?;
        Ok(self.terminal.insert(terminal))
    }

    pub fn next_page(&mut self) -> Result<Option<HistoryProgress>> {
        if self.terminal.is_none() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot_decoder_next called before ready".into(),
            ));
        }
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

    pub fn terminal(&mut self) -> Option<&mut DisplayTerminal> {
        self.terminal.as_mut()
    }

    #[must_use]
    pub fn into_terminal(mut self) -> Option<DisplayTerminal> {
        self.terminal.take()
    }

    pub fn source_offset(&self) -> Result<usize> {
        self.get("snapshot_decoder_source_offset",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_SOURCE_OFFSET,
            0usize)
    }

    pub fn max_continuation_bytes(&self) -> Result<usize> {
        self.get("snapshot_decoder_max_continuation_bytes",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_MAX_CONTINUATION_BYTES,
            0usize)
    }

    pub fn retains_continuation(&self) -> Result<bool> {
        self.get("snapshot_decoder_retain_continuation",
            sys::GhosttySnapshotDecoderData_GHOSTTY_SNAPSHOT_DECODER_DATA_RETAIN_CONTINUATION,
            false)
    }

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
        let result = unsafe {
            sys::ghostty_snapshot_decoder_get(self.raw, key, (&raw mut value).cast::<c_void>())
        };
        check(operation, result)?;
        Ok(value)
    }
}

unsafe fn adopt(raw: sys::GhosttyTerminal, restore: SnapshotRestore) -> Result<DisplayTerminal> {
    if raw.is_null() {
        return Err(GhosttyError::AbiMismatch(
            "snapshot decoder returned a null terminal".into(),
        ));
    }
    let terminal = unsafe { OwnedHandle::from_raw(raw, sys::ghostty_terminal_free) };
    let mut cols = 0u16;
    let mut rows = 0u16;
    use sys as s;
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
    resize_terminal(terminal.raw(), size)?;
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
