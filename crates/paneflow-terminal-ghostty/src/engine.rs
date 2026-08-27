use std::marker::PhantomData;
use std::rc::Rc;

use paneflow_libghostty_sys as sys;

use crate::callbacks::CallbackState;
use crate::handles::{OwnedHandle, check};
use crate::snapshot::SnapshotCache;
use crate::snapshot_ffi::{TerminalKittyKeyboardFlags, terminal_get};
use crate::{BackendEvent, Modes, Result, Scroll, WindowSize};

const CLEAR_SCREEN_AND_SCROLLBACK: &[u8] = b"\x1b[3J\x1b[2J\x1b[H";

/// The renderer geometry a mouse encoder maps positions with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseEncoderSize {
    pub(crate) screen_width: u32,
    pub(crate) screen_height: u32,
    pub(crate) cell_width: u32,
    pub(crate) cell_height: u32,
    pub(crate) padding_top: u32,
    pub(crate) padding_bottom: u32,
    pub(crate) padding_right: u32,
    pub(crate) padding_left: u32,
}

/// The terminal modes a mouse encoder derives its behavior from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseModes {
    report_click: bool,
    drag: bool,
    motion: bool,
    sgr: bool,
    utf8: bool,
}

impl From<Modes> for MouseModes {
    fn from(modes: Modes) -> Self {
        Self {
            report_click: modes.mouse_report_click,
            drag: modes.mouse_drag,
            motion: modes.mouse_motion,
            sgr: modes.sgr_mouse,
            utf8: modes.utf8_mouse,
        }
    }
}

pub struct DisplayTerminal {
    pub(crate) mouse_event: OwnedHandle<sys::GhosttyMouseEvent>,
    pub(crate) mouse_encoder: OwnedHandle<sys::GhosttyMouseEncoder>,
    pub(crate) key_event: OwnedHandle<sys::GhosttyKeyEvent>,
    pub(crate) key_encoder: OwnedHandle<sys::GhosttyKeyEncoder>,
    pub(crate) row_cells: OwnedHandle<sys::GhosttyRenderStateRowCells>,
    pub(crate) row_iterator: OwnedHandle<sys::GhosttyRenderStateRowIterator>,
    pub(crate) render_state: OwnedHandle<sys::GhosttyRenderState>,
    /// Created on first use. Declared before `terminal` so it is dropped
    /// first: `ghostty_selection_gesture_free` needs a live terminal.
    pub(crate) gesture: Option<crate::selection_gesture::GestureHandle>,
    pub(crate) terminal: OwnedHandle<sys::GhosttyTerminal>,
    pub(crate) snapshot_cache: SnapshotCache,
    /// The mouse modes the encoder was last configured from. libghostty's
    /// `setopt_from_terminal` clears the encoder's last-cell memory, which is
    /// what suppresses a motion report per pixel instead of per cell, so it
    /// is only called when these actually change.
    pub(crate) mouse_encoder_modes: Option<MouseModes>,
    /// The renderer geometry the mouse encoder was last configured with.
    /// Setting it also clears the last-cell memory, so it is only pushed when
    /// it changes.
    pub(crate) mouse_encoder_size: Option<MouseEncoderSize>,
    /// Encoder settings that belong to the embedder rather than the terminal.
    /// `ghostty_key_encoder_setopt_from_terminal` resets the encoder to what
    /// the program asked for, so these are reapplied after every such call.
    pub(crate) key_encoder_overrides: crate::input_options::KeyEncoderOverrides,
    pub(crate) callbacks: Box<CallbackState>,
    pub(crate) _not_send_or_sync: PhantomData<Rc<()>>,
}

impl DisplayTerminal {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        // SAFETY: the terminal handle is owned by `self` and the slice is
        // borrowed for the duration of the call.
        unsafe { sys::ghostty_terminal_vt_write(self.terminal.raw(), bytes.as_ptr(), bytes.len()) };
        Ok(())
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
        self.callbacks.reset_working_directory();
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
        // `GhosttyMode` packs the ANSI flag in bit 15, so a DEC private mode
        // number is already its own mode identifier.
        let mut config = sys::GhosttyTerminalModeConfig {
            mode: dec_mode,
            value: false,
        };
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MODE,
                (&raw mut config).cast(),
            )
        };
        check("terminal_get_mode", result)?;
        Ok(config.value)
    }

    fn kitty_keyboard_flags(&self) -> Result<u8> {
        terminal_get::<TerminalKittyKeyboardFlags>(self.terminal.raw())
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An OSC body long enough to exceed libghostty's OSC 52 budget once it is
    /// base64 decoded. The length stays a multiple of four so the payload is
    /// still decodable and the test proves the cap, not a decode failure.
    const OVERSIZED_OSC_BODY_BYTES: usize =
        crate::callback_ffi::MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;

    #[test]
    fn clear_screen_and_scrollback_preserves_terminal_modes() {
        let size = WindowSize::new(10, 2, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
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

    #[test]
    fn oversized_c1_osc_tail_is_dropped_and_native_parser_recovers() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal.feed(b"\x1b[\xd1\x9d52;c;").expect("OSC prefix");
        terminal
            .feed(&vec![b'A'; OVERSIZED_OSC_BODY_BYTES + 1])
            .expect("oversized OSC body");
        terminal.feed(b"\x9cignored").expect("C1 ST tail");
        terminal.feed(b"\x1b]52;c;").expect("second OSC prefix");
        terminal
            .feed(&vec![b'B'; OVERSIZED_OSC_BODY_BYTES + 1])
            .expect("discarded OSC tail");
        terminal.feed(b"\x07SAFE").expect("OSC recovery");

        let content = terminal.snapshot().expect("snapshot after recovery");
        let visible: String = content.cells.iter().map(|cell| cell.character).collect();
        assert!(visible.contains("SAFE"));
        assert!(!visible.contains('B'));
        assert!(
            terminal
                .drain_events()
                .iter()
                .all(|event| !matches!(event, BackendEvent::ClipboardStore(_)))
        );
    }

    #[test]
    fn non_ground_c1_osc52_emits_clipboard_event() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal
            .feed(b"\x1b]52;c;b3duZWQ=\x1b[\x9d52;c;b2s=\x07")
            .expect("C1 OSC52 must parse");

        assert!(
            terminal
                .drain_events()
                .iter()
                .any(|event| matches!(event, BackendEvent::ClipboardStore(text) if text == "ok"))
        );
    }

    #[test]
    fn osc9_4_progress_reports_are_decoded_and_coalesced() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");

        // A determinate report followed by an error report in the same drain
        // window: only the newest survives, the way a title report does.
        terminal.feed(b"\x1b]9;4;1;42\x07").expect("determinate report");
        terminal.feed(b"\x1b]9;4;2;80\x07").expect("error report");

        let reports = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::Progress(report) => Some(report),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reports,
            vec![crate::ProgressReport {
                state: crate::ProgressState::Error,
                percent: Some(80),
            }]
        );

        terminal.feed(b"\x1b]9;4;0\x07").expect("remove report");
        let reports = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::Progress(report) => Some(report),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reports,
            vec![crate::ProgressReport {
                state: crate::ProgressState::Remove,
                percent: None,
            }]
        );
    }

    #[test]
    fn osc7_working_directory_is_decoded_once() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        let report = b"\x1b]7;file:///C:/dev/path%20with%20space/%C3%A9\x07";

        terminal
            .feed(&report[..18])
            .expect("fragmented OSC 7 prefix");
        terminal.feed(&report[18..]).expect("fragmented OSC 7 tail");
        terminal.feed(report).expect("duplicate OSC 7 report");

        let directories = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WorkingDirectory(cwd) => Some(cwd),
                _ => None,
            })
            .collect::<Vec<_>>();

        let expected = if cfg!(windows) {
            r"C:\dev\path with space\é"
        } else {
            "/C:/dev/path with space/é"
        };
        assert_eq!(directories, [expected]);
    }

    #[test]
    fn oversized_osc7_cannot_publish_a_truncated_directory() {
        let size = WindowSize::new(80, 24, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        let mut reports = b"\x1b]7;file:///C:/".to_vec();
        reports.extend(std::iter::repeat_n(b'a', 4097));
        reports.extend_from_slice(b"\x07\x1b]7;file:///C:/dev/recovered\x07");

        terminal.feed(&reports).expect("OSC 7 stream must parse");

        let directories = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WorkingDirectory(cwd) => Some(cwd),
                _ => None,
            })
            .collect::<Vec<_>>();
        let expected = if cfg!(windows) {
            r"C:\dev\recovered"
        } else {
            "/C:/dev/recovered"
        };
        assert_eq!(directories, [expected]);
    }
}
