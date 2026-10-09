use std::marker::PhantomData;
use std::rc::Rc;

use paneflow_libghostty_sys as sys;

use crate::callbacks::CallbackState;
use crate::handles::{OwnedHandle, check};
use crate::snapshot::SnapshotCache;
use crate::snapshot_ffi::{TerminalKittyKeyboardFlags, terminal_get};
use crate::{BackendEvent, Modes, RenderHold, Result, Scroll, TerminalMemoryUsage, WindowSize};

const CLEAR_SCREEN_AND_SCROLLBACK: &[u8] = b"\x1b[3J\x1b[2J\x1b[H";
const CLEAR_SCROLLBACK: &[u8] = b"\x1b[3J";
const RESET_PROGRAM_OVERRIDES: &[u8] =
    b"\x1b]104\x1b\\\x1b]110\x1b\\\x1b]111\x1b\\\x1b]112\x1b\\\x1b]22;\x1b\\";
const SYNCHRONIZED_OUTPUT_MODE: u16 = 2026;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct MouseModes {
    x10: bool,
    normal: bool,
    drag: bool,
    motion: bool,
    utf8: bool,
    sgr: bool,
    urxvt: bool,
    sgr_pixels: bool,
}

pub struct DisplayTerminal {
    pub(crate) mouse_event: OwnedHandle<sys::GhosttyMouseEvent>,
    pub(crate) mouse_encoder: OwnedHandle<sys::GhosttyMouseEncoder>,
    pub(crate) key_event: OwnedHandle<sys::GhosttyKeyEvent>,
    pub(crate) key_encoder: OwnedHandle<sys::GhosttyKeyEncoder>,
    pub(crate) row_cells: OwnedHandle<sys::GhosttyRenderStateRowCells>,
    pub(crate) row_iterator: OwnedHandle<sys::GhosttyRenderStateRowIterator>,
    pub(crate) gesture: Option<crate::selection_gesture::GestureHandle>,
    pub(crate) search: Option<crate::native_search::NativeSearch>,
    pub(crate) terminal: OwnedHandle<sys::GhosttyTerminal>,
    pub(crate) snapshot_cache: SnapshotCache,
    pub(crate) mouse_encoder_modes: Option<MouseModes>,
    pub(crate) mouse_encoder_size: Option<MouseEncoderSize>,
    pub(crate) key_encoder_overrides: crate::input_options::KeyEncoderOverrides,
    pub(crate) callbacks: Box<CallbackState>,
    pub(crate) history_clear_pending: bool,
    pub(crate) _not_send_or_sync: PhantomData<Rc<()>>,
}

impl DisplayTerminal {
    pub fn feed(&mut self, bytes: &[u8]) -> Result<()> {
        let mut rest = bytes;
        if self.history_clear_pending {
            let mut consumed = 0usize;
            let result = unsafe {
                sys::ghostty_terminal_vt_write_until_ground(
                    self.terminal.raw(),
                    rest.as_ptr(),
                    rest.len(),
                    &mut consumed,
                )
            };
            match result {
                sys::GhosttyResult_GHOSTTY_SUCCESS => {
                    rest = rest.get(consumed..).unwrap_or_default();
                    self.history_clear_pending = false;
                    self.write_history_clear()?;
                }
                sys::GhosttyResult_GHOSTTY_NO_VALUE => rest = &[],
                other => check("terminal_vt_write_until_ground", other)?,
            }
        }
        unsafe { sys::ghostty_terminal_vt_write(self.terminal.raw(), rest.as_ptr(), rest.len()) };
        self.invalidate_search_rail();
        Ok(())
    }

    pub fn clear_history(&mut self) -> Result<()> {
        if self.at_ground()? {
            self.write_history_clear()
        } else {
            self.history_clear_pending = true;
            Ok(())
        }
    }

    pub fn reset(&mut self) {
        unsafe {
            sys::ghostty_terminal_reset(self.terminal.raw());
            sys::ghostty_terminal_vt_write(
                self.terminal.raw(),
                RESET_PROGRAM_OVERRIDES.as_ptr(),
                RESET_PROGRAM_OVERRIDES.len(),
            );
        }
        self.callbacks.push(BackendEvent::Reset);
        self.history_clear_pending = false;
        self.invalidate_search_rail();
        self.snapshot_cache.invalidate();
    }

    fn at_ground(&self) -> Result<bool> {
        let mut consumed = 0usize;
        let result = unsafe {
            sys::ghostty_terminal_vt_write_until_ground(
                self.terminal.raw(),
                std::ptr::null(),
                0,
                &mut consumed,
            )
        };
        if result == sys::GhosttyResult_GHOSTTY_NO_VALUE {
            return Ok(false);
        }
        check("terminal_vt_write_until_ground", result)?;
        Ok(true)
    }

    fn write_history_clear(&mut self) -> Result<()> {
        let sequence = if self.modes()?.alternate_screen {
            CLEAR_SCROLLBACK
        } else {
            CLEAR_SCREEN_AND_SCROLLBACK
        };
        unsafe {
            sys::ghostty_terminal_vt_write(self.terminal.raw(), sequence.as_ptr(), sequence.len())
        };
        self.invalidate_search_rail();
        self.snapshot_cache.invalidate();
        Ok(())
    }

    pub fn resize(&mut self, size: WindowSize) -> Result<()> {
        let size = size.validate()?;
        let current = self.callbacks.size();
        if size.cols < current.cols && size.rows < current.rows {
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
        self.invalidate_search_rail();
        self.snapshot_cache.invalidate();
        self.callbacks.set_size(size);
        Ok(())
    }

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

    pub fn synchronized_output(&self) -> Result<bool> {
        self.mode(SYNCHRONIZED_OUTPUT_MODE)
    }

    pub fn enable_render_hold(&mut self) -> Result<()> {
        crate::callbacks::install_render_hold(self.terminal.raw())
    }

    pub fn enable_program_status(&mut self) -> Result<()> {
        crate::callbacks::install_program_status(self.terminal.raw())
    }

    pub fn render_hold(&self) -> Option<RenderHold> {
        self.callbacks.render_hold()
    }

    pub fn release_render_hold(&mut self) -> Result<()> {
        let config = sys::GhosttyTerminalModeConfig {
            mode: SYNCHRONIZED_OUTPUT_MODE,
            value: false,
        };
        let result = unsafe {
            sys::ghostty_terminal_set(
                self.terminal.raw(),
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE,
                (&raw const config).cast(),
            )
        };
        check("terminal_set_mode", result)?;
        self.callbacks.end_render_hold();
        Ok(())
    }

    pub(crate) fn mode(&self, dec_mode: u16) -> Result<bool> {
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

    pub(crate) fn mouse_modes(&self) -> Result<MouseModes> {
        Ok(MouseModes {
            x10: self.mode(9)?,
            normal: self.mode(1000)?,
            drag: self.mode(1002)?,
            motion: self.mode(1003)?,
            utf8: self.mode(1005)?,
            sgr: self.mode(1006)?,
            urxvt: self.mode(1015)?,
            sgr_pixels: self.mode(1016)?,
        })
    }

    fn kitty_keyboard_flags(&self) -> Result<u8> {
        terminal_get::<TerminalKittyKeyboardFlags>(self.terminal.raw())
    }

    pub fn memory_usage(&self) -> Result<TerminalMemoryUsage> {
        self.memory_usage_sized(std::mem::size_of::<sys::GhosttyTerminalMemoryUsage>())
    }

    fn memory_usage_sized(&self, size: usize) -> Result<TerminalMemoryUsage> {
        let mut raw = sys::GhosttyTerminalMemoryUsage {
            size,
            compression_supported: false,
            primary_pages: 0,
            primary_virtual_bytes: 0,
            primary_resident_bytes: 0,
            primary_compressed_pages: 0,
            primary_compressed_bytes: 0,
            primary_image_bytes: 0,
            alternate_pages: 0,
            alternate_virtual_bytes: 0,
            alternate_resident_bytes: 0,
            alternate_compressed_pages: 0,
            alternate_compressed_bytes: 0,
            alternate_image_bytes: 0,
        };
        let result = unsafe {
            sys::ghostty_terminal_get(
                self.terminal.raw(),
                sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_MEMORY_USAGE,
                (&raw mut raw).cast(),
            )
        };
        check("terminal_get_memory_usage", result)?;
        Ok(TerminalMemoryUsage {
            compression_supported: raw.compression_supported,
            primary_pages: raw.primary_pages,
            primary_virtual_bytes: raw.primary_virtual_bytes,
            primary_resident_bytes: raw.primary_resident_bytes,
            primary_compressed_pages: raw.primary_compressed_pages,
            primary_compressed_bytes: raw.primary_compressed_bytes,
            primary_image_bytes: raw.primary_image_bytes,
            alternate_pages: raw.alternate_pages,
            alternate_virtual_bytes: raw.alternate_virtual_bytes,
            alternate_resident_bytes: raw.alternate_resident_bytes,
            alternate_compressed_pages: raw.alternate_compressed_pages,
            alternate_compressed_bytes: raw.alternate_compressed_bytes,
            alternate_image_bytes: raw.alternate_image_bytes,
        })
    }
}

pub(crate) fn resize_terminal(terminal: sys::GhosttyTerminal, size: WindowSize) -> Result<()> {
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

    #[test]
    fn a_memory_usage_struct_too_small_for_the_engine_is_an_error() {
        let size = WindowSize::new(10, 2, 8, 16).expect("valid terminal size");
        let terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        assert!(matches!(
            terminal.memory_usage_sized(std::mem::size_of::<usize>()),
            Err(crate::GhosttyError::Ffi { .. })
        ));
        assert!(terminal.memory_usage().is_ok());
    }

    const OVERSIZED_OSC_BODY_BYTES: usize =
        crate::callback_ffi::MAX_CLIPBOARD_BYTES.div_ceil(3) * 4;

    #[test]
    fn a_panicking_render_hold_callback_reports_callback_panicked() {
        let size = WindowSize::new(10, 2, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal.enable_render_hold().expect("render hold installs");
        terminal.callbacks.panic_next.set(true);

        terminal.feed(b"\x1b[?2026h").expect("hold sequence parses");

        assert_eq!(terminal.drain_events(), [BackendEvent::CallbackPanicked]);
        assert!(terminal.render_hold().is_none());
    }

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

    fn screen_text(terminal: &DisplayTerminal) -> String {
        terminal
            .format(crate::FormatterOptions::plain_text())
            .expect("plain text")
    }

    #[test]
    fn a_history_clear_waits_for_a_split_escape_sequence_to_complete() {
        let size = WindowSize::new(20, 3, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\n\x1b[3")
            .expect("output ending inside a CSI");

        terminal.clear_history().expect("clear request");
        terminal.feed(b"1mRED\x1b[0m").expect("the CSI completes");

        let content = terminal.snapshot().expect("snapshot");
        assert_eq!(content.history_size, 0);
        let text = screen_text(&terminal);
        assert!(text.contains("RED"), "{text:?}");
        assert!(!text.contains('m'), "the split CSI leaked as text: {text:?}");
        assert!(!text.contains("four"), "{text:?}");
        let red = content
            .cells
            .iter()
            .find(|cell| cell.character == 'R')
            .expect("the R cell");
        assert_eq!(red.foreground, crate::Color::Palette(1));
    }

    #[test]
    fn a_history_clear_on_the_alternate_screen_keeps_the_program_screen() {
        let size = WindowSize::new(20, 3, 8, 16).expect("valid terminal size");
        let mut terminal = DisplayTerminal::new(size, 100, crate::TerminalAppearance::default())
            .expect("terminal must initialize");
        terminal
            .feed(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")
            .expect("primary output");
        terminal
            .feed(b"\x1b[?1049h\x1b[HTUI SCREEN")
            .expect("alternate screen");

        terminal.clear_history().expect("clear request");

        assert!(screen_text(&terminal).contains("TUI SCREEN"));
        terminal.feed(b"\x1b[?1049l").expect("leave the alternate screen");
        assert!(screen_text(&terminal).contains("five"));
    }

    #[test]
    fn a_reset_restores_the_initial_state_and_keeps_the_configured_colors() {
        let size = WindowSize::new(20, 3, 8, 16).expect("valid terminal size");
        let appearance = crate::TerminalAppearance {
            background: crate::Rgb {
                r: 0xfa,
                g: 0xfb,
                b: 0xfc,
            },
            ..crate::TerminalAppearance::default()
        };
        let mut terminal =
            DisplayTerminal::new(size, 100, appearance).expect("terminal must initialize");
        terminal
            .feed(b"\x1b[?2004h\x1b[?1049hbusy\x1b]11;#000000\x1b\\")
            .expect("program state");

        terminal.reset();

        let modes = terminal.modes().expect("modes");
        assert!(!modes.bracketed_paste);
        assert!(!modes.alternate_screen);
        assert!(!screen_text(&terminal).contains("busy"));
        terminal.drain_events();
        terminal.feed(b"\x1b]11;?\x1b\\").expect("background query");
        let replies = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WritePty(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();
        assert!(
            String::from_utf8_lossy(&replies).contains("rgb:fafa/fbfb/fcfc"),
            "{:?}",
            String::from_utf8_lossy(&replies)
        );
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
