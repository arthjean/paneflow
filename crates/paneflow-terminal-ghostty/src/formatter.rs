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

/// Rows of history a replay capture carries, matching the cap the plain-text
/// scrollback path has always applied.
const MAX_REPLAY_HISTORY_ROWS: i32 = 4_000;

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

impl Formatter<'_> {
    /// Run the formatter through libghostty's allocating path, so the output
    /// size does not have to be known in advance.
    fn into_bytes(self) -> Result<Vec<u8>> {
        let mut pointer: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        // SAFETY: the formatter is live, the null allocator selects
        // libghostty's default, and both out-parameters are valid storage.
        let result = unsafe {
            sys::ghostty_formatter_format_alloc(self.raw, std::ptr::null(), &mut pointer, &mut len)
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
        // A NULL selection formats the whole active screen.
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

    /// Format the current selection, or `None` when nothing is selected.
    ///
    /// This is what a styled copy wants: the same range the user highlighted,
    /// rendered by libghostty instead of by walking cells, so soft-wrapped
    /// lines rejoin and a rectangular selection stays rectangular.
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

    /// Capture the screen and its recent history as VT sequences that replay
    /// it into another terminal.
    ///
    /// Plain text loses the styling, the modes, and the cursor; these bytes
    /// carry all three, which is what makes a restored pane look like the one
    /// that was closed rather than like a transcript of it. History is capped
    /// at [`MAX_REPLAY_HISTORY_ROWS`] rows, the same bound the text path has.
    ///
    /// Feed the result back with [`Self::feed`].
    pub fn capture_replay(&self) -> Result<Vec<u8>> {
        let Some(selection) = self.replay_selection()? else {
            return Ok(Vec::new());
        };
        let formatter = self.formatter_over(
            FormatterOptions {
                emit: FormatterFormat::Vt,
                // Wrapping is part of what the screen looked like.
                unwrap: false,
                trim: true,
                extra: TerminalExtra::all(),
            },
            Some(&selection),
        )?;
        formatter.into_bytes()
    }

    /// The range [`Self::capture_replay`] covers: the viewport plus a bounded
    /// tail of history.
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
