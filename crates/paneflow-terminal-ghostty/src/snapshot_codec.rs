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

#[derive(Clone, Copy, Debug)]
pub struct SnapshotRestore {
    pub cell_width: u32,
    pub cell_height: u32,
    pub max_scrollback: usize,
    pub appearance: TerminalAppearance,
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
}

pub struct SnapshotDecoder<'src> {
    raw: sys::GhosttySnapshotDecoder,
    terminal: Option<DisplayTerminal>,
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
        Self::wrap(raw)
    }

    fn wrap(raw: sys::GhosttySnapshotDecoder) -> Result<Self> {
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot decoder constructor returned a null handle".into(),
            ));
        }
        Ok(Self {
            raw,
            terminal: None,
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

    pub fn decode(&mut self, restore: SnapshotRestore) -> Result<&mut DisplayTerminal> {
        if self.terminal.is_some() {
            return Err(GhosttyError::AbiMismatch(
                "snapshot_decoder_decode called on a decoder that already produced a terminal"
                    .into(),
            ));
        }
        if restore.max_scrollback > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        let mut raw_terminal: sys::GhosttyTerminal = std::ptr::null_mut();
        let result = unsafe { sys::ghostty_snapshot_decoder_decode(self.raw, &mut raw_terminal) };
        check("snapshot_decoder_decode", result)?;
        let terminal = unsafe { adopt(raw_terminal, restore) }?;
        Ok(self.terminal.insert(terminal))
    }

    #[must_use]
    pub fn into_terminal(mut self) -> Option<DisplayTerminal> {
        self.terminal.take()
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
    fn the_size_query_matches_the_encoded_length() {
        let mut source = terminal(20, 4);
        source.feed(b"agree").expect("fixture output must parse");

        let allocated = source.encode_snapshot().expect("alloc path");
        assert_eq!(
            source.encode_snapshot_size().expect("size query"),
            allocated.len()
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
        decoder.decode(restore()).expect("snapshot must decode");
        assert!(decoder.set_retain_continuation(true).is_err());
        assert!(decoder.decode(restore()).is_err());
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
