use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result};

const MAX_FORMAT_BYTES: usize = 32 * 1024 * 1024;

const MAX_REPLAY_HISTORY_ROWS: i32 = 4_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FormatterFormat {
    #[default]
    Plain,
    Vt,
    Html,
}

impl FormatterFormat {
    fn raw(self) -> sys::GhosttyFormatterFormat {
        match self {
            Self::Plain => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Self::Vt => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
            Self::Html => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_HTML,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenExtra {
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalExtra {
    pub palette: bool,
    pub modes: bool,
    pub scrolling_region: bool,
    pub tabstops: bool,
    pub pwd: bool,
    pub keyboard: bool,
    pub screen: ScreenExtra,
}

impl TerminalExtra {
    #[must_use]
    pub fn all() -> Self {
        Self {
            palette: true,
            modes: true,
            scrolling_region: true,
            tabstops: true,
            pwd: true,
            keyboard: true,
            screen: ScreenExtra {
                cursor: true,
                style: true,
                hyperlink: true,
                protection: true,
                kitty_keyboard: true,
                charsets: true,
            },
        }
    }

    fn raw(self) -> sys::GhosttyFormatterTerminalExtra {
        sys::GhosttyFormatterTerminalExtra {
            size: std::mem::size_of::<sys::GhosttyFormatterTerminalExtra>(),
            palette: self.palette,
            modes: self.modes,
            scrolling_region: self.scrolling_region,
            tabstops: self.tabstops,
            pwd: self.pwd,
            keyboard: self.keyboard,
            screen: sys::GhosttyFormatterScreenExtra {
                size: std::mem::size_of::<sys::GhosttyFormatterScreenExtra>(),
                cursor: self.screen.cursor,
                style: self.screen.style,
                hyperlink: self.screen.hyperlink,
                protection: self.screen.protection,
                kitty_keyboard: self.screen.kitty_keyboard,
                charsets: self.screen.charsets,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatterOptions {
    pub emit: FormatterFormat,
    pub unwrap: bool,
    pub trim: bool,
    pub extra: TerminalExtra,
}

impl FormatterOptions {
    #[must_use]
    pub fn plain_text() -> Self {
        Self {
            emit: FormatterFormat::Plain,
            unwrap: true,
            trim: true,
            extra: TerminalExtra::default(),
        }
    }
}

struct Formatter<'terminal> {
    raw: sys::GhosttyFormatter,
    _terminal: std::marker::PhantomData<&'terminal DisplayTerminal>,
}

impl Formatter<'_> {
    fn into_bytes(self) -> Result<Vec<u8>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            sys::ghostty_formatter_format_alloc(self.raw, std::ptr::null(), &mut pointer, &mut len)
        };
        check("formatter_format_alloc", result)?;
        if pointer.is_null() {
            return Ok(Vec::new());
        }
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if copied.len() > MAX_FORMAT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "formatted screen",
                limit: MAX_FORMAT_BYTES,
            });
        }
        Ok(copied)
    }
}

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
        unsafe { sys::ghostty_formatter_free(self.raw) };
    }
}

impl DisplayTerminal {
    fn formatter(&self, options: FormatterOptions) -> Result<Formatter<'_>> {
        self.formatter_over(options, None)
    }

    fn formatter_over(
        &self,
        options: FormatterOptions,
        selection: Option<&sys::GhosttySelection>,
    ) -> Result<Formatter<'_>> {
        let options = sys::GhosttyFormatterTerminalOptions {
            size: std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>(),
            emit: options.emit.raw(),
            unwrap: options.unwrap,
            trim: options.trim,
            extra: options.extra.raw(),
            selection: selection.map_or(std::ptr::null(), |selection| selection as *const _),
        };
        let mut raw: sys::GhosttyFormatter = std::ptr::null_mut();
        let result = unsafe {
            sys::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &mut raw,
                self.terminal.raw(),
                options,
            )
        };
        check("formatter_terminal_new", result)?;
        if raw.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "formatter_terminal_new returned a null handle".into(),
            ));
        }
        Ok(Formatter {
            raw,
            _terminal: std::marker::PhantomData,
        })
    }

    pub fn format(&self, options: FormatterOptions) -> Result<String> {
        let bytes = self.format_bytes(options)?;
        String::from_utf8(bytes).map_err(|_| GhosttyError::InvalidUtf8("formatted screen"))
    }

    pub fn format_bytes(&self, options: FormatterOptions) -> Result<Vec<u8>> {
        let formatter = self.formatter(options)?;
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        let result = unsafe {
            sys::ghostty_formatter_format_alloc(
                formatter.raw,
                std::ptr::null(),
                &mut pointer,
                &mut len,
            )
        };
        check("formatter_format_alloc", result)?;
        if pointer.is_null() {
            return Ok(Vec::new());
        }
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if copied.len() > MAX_FORMAT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "formatted screen",
                limit: MAX_FORMAT_BYTES,
            });
        }
        Ok(copied)
    }

    pub fn format_into(&self, options: FormatterOptions, buffer: &mut [u8]) -> Result<usize> {
        let formatter = self.formatter(options)?;
        let mut written = 0usize;
        let result = unsafe {
            sys::ghostty_formatter_format_buf(
                formatter.raw,
                buffer.as_mut_ptr(),
                buffer.len(),
                &mut written,
            )
        };
        check("formatter_format_buf", result)?;
        if written > buffer.len() {
            return Err(GhosttyError::AbiMismatch(format!(
                "formatter_format_buf reported {written} bytes for a {}-byte buffer",
                buffer.len()
            )));
        }
        Ok(written)
    }

    pub fn format_selection(&self, options: FormatterOptions) -> Result<Option<String>> {
        let Some(selection) = self.current_selection()? else {
            return Ok(None);
        };
        let formatter = self.formatter_over(options, Some(&selection))?;
        let bytes = formatter.into_bytes()?;
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| GhosttyError::InvalidUtf8("formatted selection"))
    }

    pub fn capture_replay(&self) -> Result<Vec<u8>> {
        let Some(selection) = self.replay_selection()? else {
            return Ok(Vec::new());
        };
        let formatter = self.formatter_over(
            FormatterOptions {
                emit: FormatterFormat::Vt,
                unwrap: false,
                trim: true,
                extra: TerminalExtra::all(),
            },
            Some(&selection),
        )?;
        formatter.into_bytes()
    }

    fn replay_selection(&self) -> Result<Option<sys::GhosttySelection>> {
        let (cols, _, scrollback) = self.geometry_batch()?;
        let rows = i32::from(self.callbacks.size().rows);
        if cols == 0 || rows == 0 {
            return Ok(None);
        }
        let history = i32::try_from(scrollback)
            .unwrap_or(MAX_REPLAY_HISTORY_ROWS)
            .min(MAX_REPLAY_HISTORY_ROWS);
        let start = crate::Point::new(-history, 0);
        let end = crate::Point::new(rows - 1, usize::from(cols - 1));
        let mut selection = crate::selection::empty_selection();
        selection.start = self.grid_ref(start)?;
        selection.end = self.grid_ref(end)?;
        selection.rectangle = false;
        Ok(Some(selection))
    }

    pub fn format_to<F: FnMut(&[u8]) -> bool>(
        &self,
        options: FormatterOptions,
        mut sink: F,
    ) -> Result<()> {
        let formatter = self.formatter(options)?;
        let writer = crate::io::writer(&mut sink);
        let result = unsafe { sys::ghostty_formatter_format(formatter.raw, writer) };
        check("formatter_format", result)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_replay_capture_restores_styling_the_text_path_loses() {
        let mut source = terminal(20, 3);
        source
            .feed(b"plain\r\n\x1b[1;31mred\x1b[0m\r\n\x1b[4munderlined")
            .expect("styled output must parse");

        let text = source
            .extract_scrollback()
            .expect("text capture")
            .unwrap_or_default();
        assert!(!text.contains('\x1b'), "the text path drops styling");

        let replay = source.capture_replay().expect("replay capture");
        assert!(!replay.is_empty());

        let mut restored = terminal(20, 3);
        restored.feed(&replay).expect("replay must parse");
        let content = restored.snapshot().expect("restored snapshot");
        let visible: String = content.cells.iter().map(|cell| cell.character).collect();
        assert!(visible.contains("red"), "got {visible:?}");
        assert!(visible.contains("underlined"), "got {visible:?}");

        let red = content
            .cells
            .iter()
            .find(|cell| cell.character == 'r')
            .expect("the styled cell must survive");
        assert!(red.flags.bold, "styling must survive the replay");
    }

    #[test]
    fn a_replay_capture_carries_history_not_just_the_viewport() {
        let mut source = terminal(20, 2);
        source
            .feed(b"scrolled-away\r\nfiller-one\r\nfiller-two")
            .expect("fixture must parse");
        assert!(source.snapshot().expect("snapshot").history_size > 0);

        let replay = source.capture_replay().expect("replay capture");
        let mut restored = terminal(20, 2);
        restored.feed(&replay).expect("replay must parse");

        let restored_history = restored
            .extract_scrollback()
            .expect("history query")
            .expect("the replay must scroll content into history");
        assert!(restored_history.contains("scrolled-away"), "got {restored_history:?}");
    }

    #[test]
    fn formatting_a_selection_returns_only_what_is_selected() {
        let mut terminal = terminal(20, 3);
        terminal
            .feed(b"first line\r\nsecond line")
            .expect("fixture must parse");

        assert_eq!(
            terminal
                .format_selection(FormatterOptions::plain_text())
                .expect("no selection"),
            None
        );

        terminal
            .set_selection(crate::SelectionRange {
                start: crate::Point::new(0, 0),
                end: crate::Point::new(0, 4),
                rectangle: false,
            })
            .expect("selection must install");
        let selected = terminal
            .format_selection(FormatterOptions::plain_text())
            .expect("selection formats")
            .expect("a selection is installed");
        assert_eq!(selected.trim_end(), "first");
    }

    use super::*;
    use crate::{TerminalAppearance, WindowSize};

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    #[test]
    fn plain_text_rejoins_soft_wrapped_lines() {
        let mut terminal = terminal(4, 4);
        terminal.feed(b"abcdef").expect("output must parse");

        let unwrapped = terminal
            .format(FormatterOptions::plain_text())
            .expect("screen must format");
        assert!(unwrapped.contains("abcdef"), "got {unwrapped:?}");

        let wrapped = terminal
            .format(FormatterOptions {
                emit: FormatterFormat::Plain,
                unwrap: false,
                trim: true,
                extra: TerminalExtra::default(),
            })
            .expect("screen must format");
        assert!(wrapped.contains("abcd\nef"), "got {wrapped:?}");
    }

    #[test]
    fn vt_and_html_carry_styling_that_plain_text_drops() {
        let mut terminal = terminal(10, 2);
        terminal
            .feed(b"\x1b[1;31mred\x1b[0m")
            .expect("output must parse");

        let plain = terminal
            .format(FormatterOptions::plain_text())
            .expect("plain must format");
        assert!(!plain.contains('\x1b'));
        assert!(plain.contains("red"));

        let vt = terminal
            .format(FormatterOptions {
                emit: FormatterFormat::Vt,
                unwrap: false,
                trim: true,
                extra: TerminalExtra::all(),
            })
            .expect("vt must format");
        assert!(vt.contains('\x1b'), "vt output must carry escapes");

        let html = terminal
            .format(FormatterOptions {
                emit: FormatterFormat::Html,
                unwrap: false,
                trim: true,
                extra: TerminalExtra::default(),
            })
            .expect("html must format");
        assert!(html.contains('<'), "html output must carry markup");
    }

    #[test]
    fn streaming_and_buffered_paths_agree_with_the_allocating_one() {
        let mut terminal = terminal(10, 2);
        terminal.feed(b"hello").expect("output must parse");
        let options = FormatterOptions::plain_text();

        let allocated = terminal.format_bytes(options).expect("alloc path");

        let mut buffer = vec![0u8; allocated.len() + 64];
        let written = terminal
            .format_into(options, &mut buffer)
            .expect("buffered path");
        assert_eq!(&buffer[..written], allocated.as_slice());

        let mut streamed = Vec::new();
        terminal
            .format_to(options, |bytes| {
                streamed.extend_from_slice(bytes);
                true
            })
            .expect("streaming path");
        assert_eq!(streamed, allocated);
    }

    #[test]
    fn a_sink_that_refuses_output_fails_the_format() {
        let mut terminal = terminal(10, 2);
        terminal.feed(b"hello").expect("output must parse");
        let error = terminal
            .format_to(FormatterOptions::plain_text(), |_| false)
            .expect_err("a refusing sink must fail the format");
        assert!(matches!(error, GhosttyError::Ffi { .. }));
    }

    #[test]
    fn an_undersized_buffer_is_reported_rather_than_truncated() {
        let mut terminal = terminal(10, 2);
        terminal.feed(b"hello").expect("output must parse");
        let mut buffer = [0u8; 2];
        assert!(
            terminal
                .format_into(FormatterOptions::plain_text(), &mut buffer)
                .is_err()
        );
    }
}
