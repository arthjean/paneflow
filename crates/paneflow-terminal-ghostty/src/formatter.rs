use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::{GhosttyError, Result};

const MAX_FORMAT_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FormatterFormat {
    #[default]
    Plain,
    Vt,
}

impl FormatterFormat {
    fn raw(self) -> sys::GhosttyFormatterFormat {
        match self {
            Self::Plain => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_PLAIN,
            Self::Vt => sys::GhosttyFormatterFormat_GHOSTTY_FORMATTER_FORMAT_VT,
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

impl Drop for Formatter<'_> {
    fn drop(&mut self) {
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
            selection: std::ptr::null(),
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
    fn vt_carries_styling_that_plain_text_drops() {
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
    }
}
