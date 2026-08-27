//! Packed terminal mode identifiers and DECRPM report encoding.

use paneflow_libghostty_sys as sys;

use crate::Result;
use crate::encode::encode_with_buffer;

const ANSI_BIT: u16 = 1 << 15;
const VALUE_MASK: u16 = 0x7fff;

/// Upper bound for an encoded DECRPM response. The longest form is
/// `CSI ? 32767 ; 4 $ y`, so 32 bytes leaves ample headroom.
const MAX_MODE_REPORT_BYTES: usize = 32;

/// A packed terminal mode identifier: bits 0-14 hold the mode number, bit 15
/// flags an ANSI mode (clear means a DEC private mode).
///
/// libghostty declares `ghostty_mode_new`, `ghostty_mode_value`, and
/// `ghostty_mode_ansi` as `static inline`, so they never reach the static
/// archive and bindgen cannot bind them. The bit layout is part of the
/// published ABI, so it is mirrored here instead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mode(sys::GhosttyMode);

impl Mode {
    /// Pack a mode number and its ANSI flag. Mirrors `ghostty_mode_new`.
    #[must_use]
    pub const fn new(value: u16, ansi: bool) -> Self {
        Self((value & VALUE_MASK) | if ansi { ANSI_BIT } else { 0 })
    }

    /// Pack a DEC private mode (the `?`-prefixed family).
    #[must_use]
    pub const fn dec(value: u16) -> Self {
        Self::new(value, false)
    }

    /// Pack a standard ANSI mode.
    #[must_use]
    pub const fn ansi(value: u16) -> Self {
        Self::new(value, true)
    }

    /// The numeric mode value. Mirrors `ghostty_mode_value`.
    #[must_use]
    pub const fn value(self) -> u16 {
        self.0 & VALUE_MASK
    }

    /// Whether this is an ANSI mode. Mirrors `ghostty_mode_ansi`.
    #[must_use]
    pub const fn is_ansi(self) -> bool {
        self.0 & ANSI_BIT != 0
    }

    /// The packed representation libghostty expects across the ABI.
    #[must_use]
    pub const fn raw(self) -> sys::GhosttyMode {
        self.0
    }

    /// Rebuild a mode from its packed ABI representation.
    #[must_use]
    pub const fn from_raw(raw: sys::GhosttyMode) -> Self {
        Self(raw)
    }
}

/// ANSI modes.
impl Mode {
    /// Keyboard action mode (disable keyboard).
    pub const KAM: Self = Self::ansi(2);
    /// Insert mode.
    pub const INSERT: Self = Self::ansi(4);
    /// Send/receive mode.
    pub const SRM: Self = Self::ansi(12);
    /// Linefeed/new line mode.
    pub const LINEFEED: Self = Self::ansi(20);
}

/// DEC private modes.
impl Mode {
    /// Cursor keys (DECCKM).
    pub const DECCKM: Self = Self::dec(1);
    /// 132/80 column mode.
    pub const COLUMN_132: Self = Self::dec(3);
    /// Slow scroll.
    pub const SLOW_SCROLL: Self = Self::dec(4);
    /// Reverse video.
    pub const REVERSE_COLORS: Self = Self::dec(5);
    /// Origin mode.
    pub const ORIGIN: Self = Self::dec(6);
    /// Auto-wrap mode.
    pub const WRAPAROUND: Self = Self::dec(7);
    /// Auto-repeat keys.
    pub const AUTOREPEAT: Self = Self::dec(8);
    /// X10 mouse reporting.
    pub const X10_MOUSE: Self = Self::dec(9);
    /// Cursor blink.
    pub const CURSOR_BLINKING: Self = Self::dec(12);
    /// Cursor visible (DECTCEM).
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    /// Allow 132 column mode.
    pub const ENABLE_MODE_3: Self = Self::dec(40);
    /// Reverse wrap.
    pub const REVERSE_WRAP: Self = Self::dec(45);
    /// Alternate screen (legacy).
    pub const ALT_SCREEN_LEGACY: Self = Self::dec(47);
    /// Application keypad.
    pub const KEYPAD_KEYS: Self = Self::dec(66);
    /// Backarrow key mode (DECBKM).
    pub const BACKARROW_KEY_MODE: Self = Self::dec(67);
    /// Left/right margin mode.
    pub const LEFT_RIGHT_MARGIN: Self = Self::dec(69);
    /// Normal mouse tracking.
    pub const NORMAL_MOUSE: Self = Self::dec(1000);
    /// Button-event mouse tracking.
    pub const BUTTON_MOUSE: Self = Self::dec(1002);
    /// Any-event mouse tracking.
    pub const ANY_MOUSE: Self = Self::dec(1003);
    /// Focus in/out events.
    pub const FOCUS_EVENT: Self = Self::dec(1004);
    /// UTF-8 mouse format.
    pub const UTF8_MOUSE: Self = Self::dec(1005);
    /// SGR mouse format.
    pub const SGR_MOUSE: Self = Self::dec(1006);
    /// Alternate scroll mode.
    pub const ALT_SCROLL: Self = Self::dec(1007);
    /// URxvt mouse format.
    pub const URXVT_MOUSE: Self = Self::dec(1015);
    /// SGR-Pixels mouse format.
    pub const SGR_PIXELS_MOUSE: Self = Self::dec(1016);
    /// Ignore keypad with NumLock.
    pub const NUMLOCK_KEYPAD: Self = Self::dec(1035);
    /// Alt key sends ESC prefix.
    pub const ALT_ESC_PREFIX: Self = Self::dec(1036);
    /// Alt sends escape.
    pub const ALT_SENDS_ESC: Self = Self::dec(1039);
    /// Extended reverse wrap.
    pub const REVERSE_WRAP_EXT: Self = Self::dec(1045);
    /// Alternate screen.
    pub const ALT_SCREEN: Self = Self::dec(1047);
    /// Save cursor (DECSC).
    pub const SAVE_CURSOR: Self = Self::dec(1048);
    /// Alt screen + save cursor + clear.
    pub const ALT_SCREEN_SAVE: Self = Self::dec(1049);
    /// Bracketed paste mode.
    pub const BRACKETED_PASTE: Self = Self::dec(2004);
    /// Synchronized output.
    pub const SYNC_OUTPUT: Self = Self::dec(2026);
    /// Grapheme cluster mode.
    pub const GRAPHEME_CLUSTER: Self = Self::dec(2027);
    /// Report color scheme.
    pub const COLOR_SCHEME_REPORT: Self = Self::dec(2031);
    /// Report terminal visibility.
    pub const VISIBILITY_REPORT: Self = Self::dec(2033);
    /// In-band size reports.
    pub const IN_BAND_RESIZE: Self = Self::dec(2048);
    /// Kitty clipboard protocol paste events.
    pub const PASTE_EVENTS: Self = Self::dec(5522);
}

/// DECRPM report state, the `Ps2` parameter of a `CSI ? Ps1 ; Ps2 $ y` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeReportState {
    /// The mode is not recognized.
    NotRecognized,
    /// The mode is set.
    Set,
    /// The mode is reset.
    Reset,
    /// The mode is permanently set and cannot be changed.
    PermanentlySet,
    /// The mode is permanently reset and cannot be changed.
    PermanentlyReset,
}

impl ModeReportState {
    fn raw(self) -> sys::GhosttyModeReportState {
        match self {
            Self::NotRecognized => {
                sys::GhosttyModeReportState_GHOSTTY_MODE_REPORT_NOT_RECOGNIZED
            }
            Self::Set => sys::GhosttyModeReportState_GHOSTTY_MODE_REPORT_SET,
            Self::Reset => sys::GhosttyModeReportState_GHOSTTY_MODE_REPORT_RESET,
            Self::PermanentlySet => {
                sys::GhosttyModeReportState_GHOSTTY_MODE_REPORT_PERMANENTLY_SET
            }
            Self::PermanentlyReset => {
                sys::GhosttyModeReportState_GHOSTTY_MODE_REPORT_PERMANENTLY_RESET
            }
        }
    }
}

/// Encode a DECRPM response for `mode` in `state`.
pub fn encode_mode_report(mode: Mode, state: ModeReportState) -> Result<Vec<u8>> {
    encode_with_buffer(
        "mode_report_encode",
        MAX_MODE_REPORT_BYTES,
        |buffer, len, written| unsafe {
            sys::ghostty_mode_report_encode(mode.raw(), state.raw(), buffer, len, written)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_packing_matches_the_c_layout() {
        assert_eq!(Mode::BRACKETED_PASTE.raw(), 2004);
        assert_eq!(Mode::INSERT.raw(), 4 | ANSI_BIT);
        assert!(Mode::INSERT.is_ansi());
        assert!(!Mode::BRACKETED_PASTE.is_ansi());
        assert_eq!(Mode::ansi(20).value(), 20);
        assert_eq!(Mode::from_raw(Mode::SGR_MOUSE.raw()), Mode::SGR_MOUSE);
        // The value field is 15 bits, so bit 15 of the input never leaks in.
        assert_eq!(Mode::dec(0xffff).value(), VALUE_MASK);
        assert!(!Mode::dec(0xffff).is_ansi());
    }

    #[test]
    fn dec_and_ansi_reports_use_the_expected_prefix() {
        let dec = encode_mode_report(Mode::BRACKETED_PASTE, ModeReportState::Set)
            .expect("DEC report must encode");
        assert_eq!(dec, b"\x1b[?2004;1$y");

        let ansi = encode_mode_report(Mode::INSERT, ModeReportState::PermanentlyReset)
            .expect("ANSI report must encode");
        assert_eq!(ansi, b"\x1b[4;4$y");
    }
}
