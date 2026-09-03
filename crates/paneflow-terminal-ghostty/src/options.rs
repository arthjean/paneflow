use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::modes::Mode;
use crate::{CursorShape, GhosttyError, PALETTE_LEN, Result, Rgb};

const MAX_TERMINFO_NAME_BYTES: usize = 128;

const MAX_UNKNOWN_SEQUENCE_BYTES: usize = 4096;

fn cursor_style(shape: CursorShape) -> sys::GhosttyTerminalCursorStyle {
    match shape {
        CursorShape::Bar => sys::GhosttyTerminalCursorStyle_GHOSTTY_TERMINAL_CURSOR_STYLE_BAR,
        CursorShape::Block => sys::GhosttyTerminalCursorStyle_GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK,
        CursorShape::Underline => {
            sys::GhosttyTerminalCursorStyle_GHOSTTY_TERMINAL_CURSOR_STYLE_UNDERLINE
        }
        CursorShape::HollowBlock => {
            sys::GhosttyTerminalCursorStyle_GHOSTTY_TERMINAL_CURSOR_STYLE_BLOCK_HOLLOW
        }
    }
}

impl DisplayTerminal {
    pub fn set_palette(&mut self, palette: &[Rgb; PALETTE_LEN]) -> Result<()> {
        let raw = palette.map(sys::GhosttyColorRgb::from);
        let result = unsafe {
            self.set_terminal_option(
                "terminal_set_color_palette",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                raw.as_ptr().cast::<c_void>(),
            )
        };
        self.snapshot_cache.invalidate();
        result
    }

    pub fn reset_palette(&mut self) -> Result<()> {
        let result = unsafe {
            self.set_terminal_option(
                "terminal_reset_color_palette",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_PALETTE,
                std::ptr::null(),
            )
        };
        self.snapshot_cache.invalidate();
        result
    }

    pub fn set_default_cursor(&mut self, shape: CursorShape, blink: bool) -> Result<()> {
        let style = cursor_style(shape);
        unsafe {
            self.set_terminal_option(
                "terminal_set_default_cursor_style",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE,
                (&raw const style).cast::<c_void>(),
            )?;
            self.set_terminal_option(
                "terminal_set_default_cursor_blink",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK,
                (&raw const blink).cast::<c_void>(),
            )
        }
    }

    pub fn set_terminfo_name(&mut self, name: &str) -> Result<()> {
        if name.len() > MAX_TERMINFO_NAME_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "terminfo name",
                limit: MAX_TERMINFO_NAME_BYTES,
            });
        }
        let value = sys::GhosttyString {
            ptr: name.as_ptr(),
            len: name.len(),
        };
        unsafe {
            self.set_terminal_option(
                "terminal_set_terminfo_name",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TERMINFO_NAME,
                (&raw const value).cast::<c_void>(),
            )
        }
    }

    pub fn set_scrollback_max_bytes(&mut self, bytes: Option<usize>) -> Result<()> {
        let limit = bytes.unwrap_or_default();
        let value = if bytes.is_some() {
            (&raw const limit).cast::<c_void>()
        } else {
            std::ptr::null()
        };
        let result = unsafe {
            self.set_terminal_option(
                "terminal_set_scrollback_max_bytes",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES,
                value,
            )
        };
        self.snapshot_cache.invalidate();
        result
    }

    pub fn capture_unknown_sequences(&mut self, enabled: bool) -> Result<()> {
        let limit = if enabled {
            MAX_UNKNOWN_SEQUENCE_BYTES
        } else {
            0
        };
        unsafe {
            self.set_terminal_option(
                "terminal_set_unknown_max_bytes",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES,
                (&raw const limit).cast::<c_void>(),
            )
        }
    }

    pub fn set_clipboard_readable(&mut self, text: Option<String>) {
        self.callbacks.set_readable_clipboard(text);
    }

    pub fn set_title_reports(&mut self, enabled: bool) -> Result<()> {
        unsafe {
            self.set_terminal_option(
                "terminal_set_title_report",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_REPORT,
                (&raw const enabled).cast::<c_void>(),
            )
        }
    }

    pub fn set_glyph_protocol(&mut self, enabled: bool) -> Result<()> {
        unsafe {
            self.set_terminal_option(
                "terminal_set_glyph_protocol",
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_GLYPH_PROTOCOL,
                (&raw const enabled).cast::<c_void>(),
            )
        }
    }

    pub fn set_mode(&mut self, mode: Mode, value: bool) -> Result<()> {
        self.write_mode(
            "terminal_set_mode",
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE,
            mode,
            value,
        )
    }

    pub fn set_mode_default(&mut self, mode: Mode, value: bool) -> Result<()> {
        self.write_mode(
            "terminal_set_mode_default",
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_MODE_DEFAULT,
            mode,
            value,
        )
    }

    fn write_mode(
        &mut self,
        operation: &'static str,
        option: sys::GhosttyTerminalOption,
        mode: Mode,
        value: bool,
    ) -> Result<()> {
        let config = sys::GhosttyTerminalModeConfig {
            mode: mode.raw(),
            value,
        };
        let result = unsafe {
            self.set_terminal_option(operation, option, (&raw const config).cast::<c_void>())
        };
        self.snapshot_cache.invalidate();
        result
    }

    unsafe fn set_terminal_option(
        &mut self,
        operation: &'static str,
        option: sys::GhosttyTerminalOption,
        value: *const c_void,
    ) -> Result<()> {
        let result = unsafe { sys::ghostty_terminal_set(self.terminal.raw(), option, value) };
        check(operation, result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BackendEvent, ColorScheme, PaletteMask, TerminalAppearance, WindowSize, generate_palette,
    };

    fn terminal(cols: usize, rows: usize) -> DisplayTerminal {
        let size = WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    fn replies(terminal: &mut DisplayTerminal) -> Vec<u8> {
        terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WritePty(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    #[test]
    fn the_embedder_palette_answers_osc4_queries() {
        let mut terminal = terminal(20, 4);
        let mut palette = crate::default_palette();
        palette[1] = Rgb {
            r: 0xab,
            g: 0xcd,
            b: 0xef,
        };
        terminal.set_palette(&palette).expect("palette must apply");

        terminal.feed(b"\x1b]4;1;?\x1b\\").expect("OSC 4 query");
        assert!(contains(&replies(&mut terminal), b"4;1;rgb:abab/cdcd/efef"));

        terminal.reset_palette().expect("palette must reset");
        terminal.feed(b"\x1b]4;1;?\x1b\\").expect("OSC 4 query");
        assert!(!contains(&replies(&mut terminal), b"abab/cdcd/efef"));
    }

    #[test]
    fn a_generated_palette_keeps_the_themes_ansi_colors() {
        let mut base = crate::default_palette();
        for (index, entry) in base.iter_mut().enumerate().take(16) {
            *entry = Rgb {
                r: index as u8,
                g: 0x20,
                b: 0x30,
            };
        }
        let generated = generate_palette(
            Some(&base),
            &PaletteMask::default(),
            Rgb { r: 0, g: 0, b: 0 },
            Rgb {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            false,
        );

        assert_eq!(&generated[..16], &base[..16]);
        assert_ne!(&generated[16..], &base[16..]);

        let mut terminal = terminal(20, 4);
        terminal
            .set_palette(&generated)
            .expect("generated palette must apply");
        terminal.feed(b"\x1b]4;9;?\x1b\\").expect("OSC 4 query");
        assert!(contains(&replies(&mut terminal), b"4;9;rgb:0909/2020/3030"));
    }

    #[test]
    fn the_default_cursor_is_what_a_reset_lands_on() {
        let mut terminal = terminal(20, 4);
        terminal
            .set_default_cursor(CursorShape::Bar, true)
            .expect("default cursor must apply");

        terminal.feed(b"\x1b[2 q").expect("DECSCUSR block");
        assert_eq!(
            terminal.snapshot().expect("snapshot").cursor.shape,
            CursorShape::Block
        );

        terminal.feed(b"\x1b[0 q").expect("DECSCUSR reset");
        assert_eq!(
            terminal.snapshot().expect("snapshot").cursor.shape,
            CursorShape::Bar
        );
    }

    #[test]
    fn a_terminfo_name_answers_xtgettcap_and_is_length_checked() {
        let mut terminal = terminal(20, 4);
        terminal.feed(b"\x1bP+q544e\x1b\\").expect("XTGETTCAP query");
        assert!(replies(&mut terminal).is_empty());

        terminal
            .set_terminfo_name("xterm-256color")
            .expect("name must apply");
        terminal.feed(b"\x1bP+q544e\x1b\\").expect("XTGETTCAP query");
        assert!(contains(
            &replies(&mut terminal),
            b"544E=787465726D2D323536636F6C6F72"
        ));

        assert!(matches!(
            terminal.set_terminfo_name(&"x".repeat(MAX_TERMINFO_NAME_BYTES + 1)),
            Err(GhosttyError::LimitExceeded { .. })
        ));
    }

    #[test]
    fn title_reports_stay_off_until_the_host_opts_in() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"\x1b]0;injected\x07\x1b[21t")
            .expect("title set and query");
        assert!(!contains(&replies(&mut terminal), b"injected"));

        terminal
            .set_title_reports(true)
            .expect("title reports must enable");
        terminal.feed(b"\x1b[21t").expect("title query");
        assert!(contains(&replies(&mut terminal), b"injected"));
    }

    #[test]
    fn clipboard_reads_are_denied_until_the_host_hands_over_a_snapshot() {
        let mut terminal = terminal(20, 4);
        terminal.feed(b"\x1b]52;c;?\x07").expect("OSC 52 read");
        assert_eq!(replies(&mut terminal), b"\x1b]52;c;\x07");

        terminal.set_clipboard_readable(Some("secret".into()));
        terminal.feed(b"\x1b]52;c;?\x07").expect("OSC 52 read");
        assert_eq!(replies(&mut terminal), b"\x1b]52;c;c2VjcmV0\x07");

        terminal.set_clipboard_readable(None);
        terminal.feed(b"\x1b]52;c;?\x07").expect("OSC 52 read");
        assert_eq!(replies(&mut terminal), b"\x1b]52;c;\x07");
    }

    #[test]
    fn unknown_sequences_are_captured_escaped_and_only_when_asked() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"\x1b_Zunsupported\x07payload\x1b\\")
            .expect("unknown APC");
        assert!(
            !terminal
                .drain_events()
                .iter()
                .any(|event| matches!(event, BackendEvent::UnknownSequence { .. }))
        );

        terminal
            .capture_unknown_sequences(true)
            .expect("capture must enable");
        terminal
            .feed(b"\x1b_Zunsupported\x07payload\x1b\\")
            .expect("unknown APC");
        let captured = terminal
            .drain_events()
            .into_iter()
            .find_map(|event| match event {
                BackendEvent::UnknownSequence { content, truncated } => Some((content, truncated)),
                _ => None,
            })
            .expect("the unsupported sequence must be reported");
        assert!(captured.0.contains("unsupported"));
        assert!(!captured.0.contains('\x07'), "got {:?}", captured.0);
        assert!(captured.0.contains("\\x07"));
        assert!(!captured.1);
    }

    #[test]
    fn a_mode_default_survives_the_reset_a_program_performs() {
        let mut terminal = terminal(20, 4);
        assert!(!terminal.modes().expect("modes").bracketed_paste);

        terminal
            .set_mode(Mode::BRACKETED_PASTE, true)
            .expect("mode must apply");
        assert!(terminal.modes().expect("modes").bracketed_paste);
        terminal.feed(b"\x1bc").expect("RIS");
        assert!(!terminal.modes().expect("modes").bracketed_paste);

        terminal
            .set_mode_default(Mode::BRACKETED_PASTE, true)
            .expect("mode default must apply");
        assert!(terminal.modes().expect("modes").bracketed_paste);
        terminal.feed(b"\x1bc").expect("RIS");
        assert!(terminal.modes().expect("modes").bracketed_paste);
    }

    #[test]
    fn a_desktop_notification_carries_its_title_and_body() {
        let mut terminal = terminal(20, 4);
        terminal
            .feed(b"\x1b]777;notify;Build;done in 3s\x1b\\")
            .expect("OSC 777");
        let notification = terminal
            .drain_events()
            .into_iter()
            .find_map(|event| match event {
                BackendEvent::DesktopNotification { title, body } => Some((title, body)),
                _ => None,
            })
            .expect("OSC 777 must notify");
        assert_eq!(notification, ("Build".to_owned(), "done in 3s".to_owned()));

        terminal.feed(b"\x1b]9;body only\x07").expect("OSC 9");
        let notification = terminal
            .drain_events()
            .into_iter()
            .find_map(|event| match event {
                BackendEvent::DesktopNotification { title, body } => Some((title, body)),
                _ => None,
            })
            .expect("OSC 9 must notify");
        assert_eq!(notification, (String::new(), "body only".to_owned()));
    }

    #[test]
    fn the_byte_budget_is_what_actually_bounds_the_scrollback() {
        let size = WindowSize::new(80, 4, 8, 16).expect("valid terminal size");
        let mut fixture = Vec::new();
        for line in 0..5_000 {
            fixture.extend_from_slice(format!("line-{line:05}\r\n").as_bytes());
        }
        let retained = |budget: Option<Option<usize>>| {
            let mut terminal = DisplayTerminal::new(size, 50_000, TerminalAppearance::default())
                .expect("terminal must initialize");
            if let Some(budget) = budget {
                terminal
                    .set_scrollback_max_bytes(budget)
                    .expect("byte budget must apply");
            }
            terminal.feed(&fixture).expect("fixture must parse");
            terminal.snapshot().expect("snapshot").history_size
        };

        let built_in = retained(None);
        assert!(built_in < 2_000, "got {built_in}");

        let unlimited = retained(Some(None));
        assert!(unlimited > 4_000, "got {unlimited}");

        assert_eq!(retained(Some(Some(4 * 1024 * 1024))), unlimited);
        assert!(retained(Some(Some(64 * 1024))) < unlimited);
    }

    #[test]
    fn appearance_and_scheme_still_answer_after_the_options_are_pushed() {
        let mut terminal = terminal(20, 4);
        terminal
            .set_palette(&crate::default_palette())
            .expect("palette must apply");
        terminal
            .set_glyph_protocol(false)
            .expect("glyph protocol must disable");
        terminal
            .set_appearance(TerminalAppearance::new(
                Rgb {
                    r: 1,
                    g: 2,
                    b: 3,
                },
                Rgb {
                    r: 4,
                    g: 5,
                    b: 6,
                },
                Rgb {
                    r: 7,
                    g: 8,
                    b: 9,
                },
                ColorScheme::Light,
            ))
            .expect("appearance must apply");

        terminal.feed(b"\x1b]10;?\x1b\\").expect("OSC 10 query");
        assert!(contains(&replies(&mut terminal), b"10;rgb:0101/0202/0303"));
    }
}
