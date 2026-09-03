use paneflow_libghostty_sys as sys;

use crate::Result;
use crate::encode::encode_with_buffer;

const ANSI_BIT: u16 = 1 << 15;
const VALUE_MASK: u16 = 0x7fff;

const MAX_MODE_REPORT_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Mode(sys::GhosttyMode);

impl Mode {
    #[must_use]
    pub const fn new(value: u16, ansi: bool) -> Self {
        Self((value & VALUE_MASK) | if ansi { ANSI_BIT } else { 0 })
    }

    #[must_use]
    pub const fn dec(value: u16) -> Self {
        Self::new(value, false)
    }

    #[must_use]
    pub const fn ansi(value: u16) -> Self {
        Self::new(value, true)
    }

    #[must_use]
    pub const fn value(self) -> u16 {
        self.0 & VALUE_MASK
    }

    #[must_use]
    pub const fn is_ansi(self) -> bool {
        self.0 & ANSI_BIT != 0
    }

    #[must_use]
    pub const fn raw(self) -> sys::GhosttyMode {
        self.0
    }

    #[must_use]
    pub const fn from_raw(raw: sys::GhosttyMode) -> Self {
        Self(raw)
    }
}

impl Mode {
    pub const KAM: Self = Self::ansi(2);
    pub const INSERT: Self = Self::ansi(4);
    pub const SRM: Self = Self::ansi(12);
    pub const LINEFEED: Self = Self::ansi(20);
}

impl Mode {
    pub const DECCKM: Self = Self::dec(1);
    pub const COLUMN_132: Self = Self::dec(3);
    pub const SLOW_SCROLL: Self = Self::dec(4);
    pub const REVERSE_COLORS: Self = Self::dec(5);
    pub const ORIGIN: Self = Self::dec(6);
    pub const WRAPAROUND: Self = Self::dec(7);
    pub const AUTOREPEAT: Self = Self::dec(8);
    pub const X10_MOUSE: Self = Self::dec(9);
    pub const CURSOR_BLINKING: Self = Self::dec(12);
    pub const CURSOR_VISIBLE: Self = Self::dec(25);
    pub const ENABLE_MODE_3: Self = Self::dec(40);
    pub const REVERSE_WRAP: Self = Self::dec(45);
    pub const ALT_SCREEN_LEGACY: Self = Self::dec(47);
    pub const KEYPAD_KEYS: Self = Self::dec(66);
    pub const BACKARROW_KEY_MODE: Self = Self::dec(67);
    pub const LEFT_RIGHT_MARGIN: Self = Self::dec(69);
    pub const NORMAL_MOUSE: Self = Self::dec(1000);
    pub const BUTTON_MOUSE: Self = Self::dec(1002);
    pub const ANY_MOUSE: Self = Self::dec(1003);
    pub const FOCUS_EVENT: Self = Self::dec(1004);
    pub const UTF8_MOUSE: Self = Self::dec(1005);
    pub const SGR_MOUSE: Self = Self::dec(1006);
    pub const ALT_SCROLL: Self = Self::dec(1007);
    pub const URXVT_MOUSE: Self = Self::dec(1015);
    pub const SGR_PIXELS_MOUSE: Self = Self::dec(1016);
    pub const NUMLOCK_KEYPAD: Self = Self::dec(1035);
    pub const ALT_ESC_PREFIX: Self = Self::dec(1036);
    pub const ALT_SENDS_ESC: Self = Self::dec(1039);
    pub const REVERSE_WRAP_EXT: Self = Self::dec(1045);
    pub const ALT_SCREEN: Self = Self::dec(1047);
    pub const SAVE_CURSOR: Self = Self::dec(1048);
    pub const ALT_SCREEN_SAVE: Self = Self::dec(1049);
    pub const BRACKETED_PASTE: Self = Self::dec(2004);
    pub const SYNC_OUTPUT: Self = Self::dec(2026);
    pub const GRAPHEME_CLUSTER: Self = Self::dec(2027);
    pub const COLOR_SCHEME_REPORT: Self = Self::dec(2031);
    pub const VISIBILITY_REPORT: Self = Self::dec(2033);
    pub const IN_BAND_RESIZE: Self = Self::dec(2048);
    pub const PASTE_EVENTS: Self = Self::dec(5522);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeReportState {
    NotRecognized,
    Set,
    Reset,
    PermanentlySet,
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
