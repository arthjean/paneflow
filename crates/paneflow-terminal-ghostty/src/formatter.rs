//! Format terminal content as plain text, VT sequences, or HTML.
//!
//! This is libghostty's own view of its screen, so it handles what an
//! ad-hoc cell walk gets wrong: soft-wrapped lines rejoined, trailing
//! whitespace trimmed, and, in VT mode, enough state to replay the screen
//! into another terminal.

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result};

/// Ceiling on a single formatted output, mirroring the scrollback caps the
/// rest of the crate applies to unbounded terminal data.
const MAX_FORMAT_BYTES: usize = 32 * 1024 * 1024;

/// The output syntax a formatter emits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FormatterFormat {
    /// Text with no styling.
    #[default]
    Plain,
    /// VT escape sequences that replay the screen.
    Vt,
    /// HTML with inline styling.
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

/// Screen state to replay alongside the cells, for styled output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenExtra {
    /// Emit the cursor position with CUP.
    pub cursor: bool,
    /// Emit the cursor's active SGR style.
    pub style: bool,
    /// Emit hyperlink state with OSC 8.
    pub hyperlink: bool,
    /// Emit character protection with DECSCA.
    pub protection: bool,
    /// Emit Kitty keyboard protocol state.
    pub kitty_keyboard: bool,
    /// Emit character set designations.
    pub charsets: bool,
}

/// Terminal state to replay alongside the screen, for styled output.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalExtra {
    /// Emit the palette with OSC 4.
    pub palette: bool,
    /// Emit every mode that differs from its default.
    pub modes: bool,
    /// Emit the scrolling region with DECSTBM and DECSLRM.
    pub scrolling_region: bool,
    /// Emit tab stops.
    pub tabstops: bool,
    /// Emit the working directory with OSC 7.
    pub pwd: bool,
    /// Emit keyboard modes such as `modifyOtherKeys`.
    pub keyboard: bool,
    /// Screen-level extras.
    pub screen: ScreenExtra,
}

impl TerminalExtra {
    /// Everything libghostty can replay. Useful with
    /// [`FormatterFormat::Vt`] to reproduce a screen elsewhere.
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

/// How to format a terminal's active screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FormatterOptions {
    /// Output syntax.
    pub emit: FormatterFormat,
    /// Rejoin soft-wrapped lines into one logical line.
    pub unwrap: bool,
    /// Trim trailing whitespace from non-blank lines.
    pub trim: bool,
    /// Extra state to replay. Only meaningful for styled formats.
    pub extra: TerminalExtra,
}

impl FormatterOptions {
    /// Plain text with wrapped lines rejoined and trailing blanks trimmed:
    /// the shape a human or an agent wants when reading a screen back.
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

/// A formatter bound to a terminal.
///
/// The terminal must outlive the formatter, which the borrow enforces.
struct Formatter<'terminal> {
    raw: sys::GhosttyFormatter,
    _terminal: std::marker::PhantomData<&'terminal DisplayTerminal>,
}

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `ghostty_formatter_terminal_new`, is
        // private, and Drop runs exactly once.
        unsafe { sys::ghostty_formatter_free(self.raw) };
    }
}

impl DisplayTerminal {
    fn formatter(&self, options: FormatterOptions) -> Result<Formatter<'_>> {
        let options = sys::GhosttyFormatterTerminalOptions {
            size: std::mem::size_of::<sys::GhosttyFormatterTerminalOptions>(),
            emit: options.emit.raw(),
            unwrap: options.unwrap,
            trim: options.trim,
            extra: options.extra.raw(),
            // A NULL selection formats the whole screen.
            selection: std::ptr::null(),
        };
        let mut raw: sys::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: the null allocator selects libghostty's default, `raw` is
        // valid writable storage, and the terminal handle outlives the
        // formatter through the returned borrow.
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

    /// Format the active screen and return it as a string.
    pub fn format(&self, options: FormatterOptions) -> Result<String> {
        let bytes = self.format_bytes(options)?;
        String::from_utf8(bytes).map_err(|_| GhosttyError::InvalidUtf8("formatted screen"))
    }

    /// Format the active screen into an owned byte buffer.
    ///
    /// Uses libghostty's allocating path, so the size does not have to be
    /// known in advance.
    pub fn format_bytes(&self, options: FormatterOptions) -> Result<Vec<u8>> {
        let formatter = self.formatter(options)?;
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the formatter is live, the null allocator selects
        // libghostty's default, and both out-parameters are valid storage.
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
        // The buffer belongs to libghostty's allocator, so it is copied and
        // released here rather than adopted by Rust's allocator.
        // SAFETY: the library reported `len` initialized bytes at `pointer`.
        let copied = unsafe { std::slice::from_raw_parts(pointer, len) }.to_vec();
        // SAFETY: `pointer`/`len` are exactly what `format_alloc` produced
        // with the same (default) allocator, and nothing else owns them.
        unsafe { sys::ghostty_free(std::ptr::null(), pointer, len) };
        if copied.len() > MAX_FORMAT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "formatted screen",
                limit: MAX_FORMAT_BYTES,
            });
        }
        Ok(copied)
    }

    /// Format the active screen into a caller-owned buffer.
    ///
    /// Returns the number of bytes written. When `buffer` is too small the
    /// call fails with [`GhosttyError::Ffi`] and nothing usable is written;
    /// prefer [`Self::format_bytes`] unless the buffer is being reused across
    /// frames.
    pub fn format_into(&self, options: FormatterOptions, buffer: &mut [u8]) -> Result<usize> {
        let formatter = self.formatter(options)?;
        let mut written = 0usize;
        // SAFETY: the formatter is live, `buffer` is a writable slice of the
        // stated length, and `written` is valid storage.
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

    /// Stream the formatted screen to `sink`, which returns `false` to abort.
    ///
    /// This avoids materializing the whole screen when the destination is
    /// itself a stream, such as a file or a socket.
    pub fn format_to<F: FnMut(&[u8]) -> bool>(
        &self,
        options: FormatterOptions,
        mut sink: F,
    ) -> Result<()> {
        let formatter = self.formatter(options)?;
        let writer = crate::io::writer(&mut sink);
        // SAFETY: the formatter is live and `writer` borrows `sink` for the
        // duration of this synchronous call.
        let result = unsafe { sys::ghostty_formatter_format(formatter.raw, writer) };
        check("formatter_format", result)
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
