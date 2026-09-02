use std::borrow::Cow;

use gpui::Keystroke;

use crate::terminal::types::Modes;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TerminalKeySequence {
    Protocol(Cow<'static, str>),
    Literal(Cow<'static, str>),
}

impl TerminalKeySequence {
    fn into_sequence(self) -> Cow<'static, str> {
        match self {
            Self::Protocol(sequence) | Self::Literal(sequence) => sequence,
        }
    }
}

#[cfg(target_os = "windows")]
const LEGACY_SHIFT_ENTER_SEQUENCE: &str = "\x1b\r";

#[cfg(not(target_os = "windows"))]
const LEGACY_SHIFT_ENTER_SEQUENCE: &str = "\n";

pub(crate) fn is_shift_enter(keystroke: &Keystroke, option_as_meta: bool) -> bool {
    let alt = keystroke.modifiers.alt && option_as_meta;
    keystroke.key == "enter" && keystroke.modifiers.shift && !keystroke.modifiers.control && !alt
}

fn shift_enter_sequence(
    keystroke: &Keystroke,
    mode: &Modes,
    option_as_meta: bool,
) -> Option<TerminalKeySequence> {
    if !is_shift_enter(keystroke, option_as_meta) {
        return None;
    }

    if mode.contains(Modes::KITTY_KEYBOARD) && cfg!(not(target_os = "windows")) {
        Some(TerminalKeySequence::Protocol(Cow::Borrowed("\x1b[13;2u")))
    } else {
        Some(TerminalKeySequence::Literal(Cow::Borrowed(
            LEGACY_SHIFT_ENTER_SEQUENCE,
        )))
    }
}

pub fn default_option_as_meta() -> bool {
    !cfg!(target_os = "macos")
}

pub fn to_esc_str(
    keystroke: &Keystroke,
    mode: &Modes,
    option_as_meta: bool,
) -> Option<Cow<'static, str>> {
    let key = keystroke.key.as_str();
    let ctrl = keystroke.modifiers.control;
    let shift = keystroke.modifiers.shift;
    let alt = keystroke.modifiers.alt && option_as_meta;
    let alt_phys = keystroke.modifiers.alt;

    if let Some(sequence) = shift_enter_sequence(keystroke, mode, option_as_meta) {
        return Some(sequence.into_sequence());
    }

    let modifier_code = match (shift, alt_phys, ctrl) {
        (true, false, false) => Some(2),
        (false, true, false) => Some(3),
        (true, true, false) => Some(4),
        (false, false, true) => Some(5),
        (true, false, true) => Some(6),
        (false, true, true) => Some(7),
        (true, true, true) => Some(8),
        _ => None,
    };
    if let Some(m) = modifier_code {
        let base = match key {
            "up" => Some("A"),
            "down" => Some("B"),
            "right" => Some("C"),
            "left" => Some("D"),
            "home" => Some("H"),
            "end" => Some("F"),
            "f1" => Some("P"),
            "f2" => Some("Q"),
            "f3" => Some("R"),
            "f4" => Some("S"),
            _ => None,
        };
        if let Some(b) = base {
            return Some(Cow::Owned(format!("\x1b[1;{m}{b}")));
        }

        let num = match key {
            "insert" => Some(2),
            "delete" => Some(3),
            "pageup" => Some(5),
            "pagedown" => Some(6),
            "f5" => Some(15),
            "f6" => Some(17),
            "f7" => Some(18),
            "f8" => Some(19),
            "f9" => Some(20),
            "f10" => Some(21),
            "f11" => Some(23),
            "f12" => Some(24),
            _ => None,
        };
        if let Some(n) = num {
            return Some(Cow::Owned(format!("\x1b[{n};{m}~")));
        }
    }

    if ctrl && !alt {
        let seq: Option<&'static str> = match key {
            "a" => Some("\x01"),
            "b" => Some("\x02"),
            "c" => Some("\x03"),
            "d" => Some("\x04"),
            "e" => Some("\x05"),
            "f" => Some("\x06"),
            "g" => Some("\x07"),
            "h" => Some("\x08"),
            "i" => Some("\x09"),
            "j" => Some("\x0a"),
            "k" => Some("\x0b"),
            "l" => Some("\x0c"),
            "m" => Some("\x0d"),
            "n" => Some("\x0e"),
            "o" => Some("\x0f"),
            "p" => Some("\x10"),
            "q" => Some("\x11"),
            "r" => Some("\x12"),
            "s" => Some("\x13"),
            "t" => Some("\x14"),
            "u" => Some("\x15"),
            "v" => Some("\x16"),
            "w" => Some("\x17"),
            "x" => Some("\x18"),
            "y" => Some("\x19"),
            "z" => Some("\x1a"),
            "[" => Some("\x1b"),
            "\\" => Some("\x1c"),
            "]" => Some("\x1d"),
            "^" => Some("\x1e"),
            "_" => Some("\x1f"),
            "@" => Some("\x00"),
            "?" => Some("\x7f"),
            "space" => Some("\x00"),
            "backspace" => Some("\x08"),
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    if !ctrl && !shift && !alt {
        let app_cursor = mode.contains(Modes::APP_CURSOR);
        let seq: Option<&'static str> = match key {
            "enter" => Some("\r"),
            "tab" => Some("\t"),
            "escape" => Some("\x1b"),
            "backspace" => Some("\x7f"),
            "delete" => Some("\x1b[3~"),
            "insert" => Some("\x1b[2~"),
            "up" if app_cursor => Some("\x1bOA"),
            "down" if app_cursor => Some("\x1bOB"),
            "right" if app_cursor => Some("\x1bOC"),
            "left" if app_cursor => Some("\x1bOD"),
            "up" => Some("\x1b[A"),
            "down" => Some("\x1b[B"),
            "right" => Some("\x1b[C"),
            "left" => Some("\x1b[D"),
            "home" if app_cursor => Some("\x1bOH"),
            "end" if app_cursor => Some("\x1bOF"),
            "home" => Some("\x1b[H"),
            "end" => Some("\x1b[F"),
            "pageup" => Some("\x1b[5~"),
            "pagedown" => Some("\x1b[6~"),
            "f1" => Some("\x1bOP"),
            "f2" => Some("\x1bOQ"),
            "f3" => Some("\x1bOR"),
            "f4" => Some("\x1bOS"),
            "f5" => Some("\x1b[15~"),
            "f6" => Some("\x1b[17~"),
            "f7" => Some("\x1b[18~"),
            "f8" => Some("\x1b[19~"),
            "f9" => Some("\x1b[20~"),
            "f10" => Some("\x1b[21~"),
            "f11" => Some("\x1b[23~"),
            "f12" => Some("\x1b[24~"),
            "f13" => Some("\x1b[25~"),
            "f14" => Some("\x1b[26~"),
            "f15" => Some("\x1b[28~"),
            "f16" => Some("\x1b[29~"),
            "f17" => Some("\x1b[31~"),
            "f18" => Some("\x1b[32~"),
            "f19" => Some("\x1b[33~"),
            "f20" => Some("\x1b[34~"),
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    if shift && !ctrl && !alt {
        let seq: Option<&'static str> = match key {
            "tab" => Some("\x1b[Z"),
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    if alt && !ctrl && !shift {
        let seq: Option<&'static str> = match key {
            "backspace" => Some("\x1b\x7f"),
            "enter" => Some("\x1b\x0d"),
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    if alt
        && !ctrl
        && shift
        && key.chars().count() == 1
        && let Some(ch) = key.chars().next()
        && ch.is_ascii_alphabetic()
    {
        return Some(Cow::Owned(format!("\x1b{}", ch.to_ascii_uppercase())));
    }

    if alt && !ctrl && !shift && key.chars().count() == 1 {
        return Some(Cow::Owned(format!("\x1b{key}")));
    }

    None
}

pub(crate) fn terminal_key_sequence(
    keystroke: &Keystroke,
    mode: &Modes,
    option_as_meta: bool,
) -> Option<TerminalKeySequence> {
    if let Some(sequence) = shift_enter_sequence(keystroke, mode, option_as_meta) {
        return Some(sequence);
    }

    to_esc_str(keystroke, mode, option_as_meta).map(TerminalKeySequence::Protocol)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn option_as_meta_default_is_platform_specific() {
        assert_eq!(default_option_as_meta(), !cfg!(target_os = "macos"));
        #[cfg(target_os = "macos")]
        assert!(!default_option_as_meta());
        #[cfg(not(target_os = "macos"))]
        assert!(default_option_as_meta());
    }

    #[test]
    fn page_keys_match_us009_alt_screen_constants() {
        let mode = Modes::empty();
        let pageup = Keystroke::parse("pageup").expect("valid keystroke");
        let pagedown = Keystroke::parse("pagedown").expect("valid keystroke");
        assert_eq!(to_esc_str(&pageup, &mode, true).as_deref(), Some("\x1b[5~"));
        assert_eq!(
            to_esc_str(&pagedown, &mode, true).as_deref(),
            Some("\x1b[6~")
        );
    }

    #[test]
    fn alt_arrow_reports_modifier_regardless_of_option_as_meta() {
        let mode = Modes::empty();
        let up = Keystroke::parse("alt-up").expect("valid keystroke");
        assert_eq!(to_esc_str(&up, &mode, false).as_deref(), Some("\x1b[1;3A"));
        assert_eq!(to_esc_str(&up, &mode, true).as_deref(), Some("\x1b[1;3A"));
    }

    #[test]
    fn alt_accented_letter_sends_esc_prefix() {
        let mode = Modes::empty();
        let e_acute = Keystroke::parse("alt-é").expect("valid keystroke");
        assert_eq!(to_esc_str(&e_acute, &mode, true).as_deref(), Some("\x1bé"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn legacy_shift_enter_is_an_exact_line_feed_binding() {
        let mode = Modes::empty();
        let shift_enter = Keystroke::parse("shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&shift_enter, &mode, true),
            Some(TerminalKeySequence::Literal(Cow::Borrowed("\x0a")))
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn kitty_shift_enter_preserves_the_physical_chord() {
        let mode = Modes::KITTY_KEYBOARD;
        let shift_enter = Keystroke::parse("shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&shift_enter, &mode, true),
            Some(TerminalKeySequence::Protocol(Cow::Borrowed("\x1b[13;2u")))
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn conpty_shift_enter_uses_alt_enter_transport() {
        let shift_enter = Keystroke::parse("shift-enter").expect("valid keystroke");
        for mode in [Modes::empty(), Modes::KITTY_KEYBOARD] {
            assert_eq!(
                terminal_key_sequence(&shift_enter, &mode, true),
                Some(TerminalKeySequence::Literal(Cow::Borrowed("\x1b\r")))
            );
        }
    }

    #[test]
    fn shift_enter_respects_option_as_meta() {
        let mode = Modes::empty();
        let option_shift_enter = Keystroke::parse("alt-shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&option_shift_enter, &mode, false),
            Some(TerminalKeySequence::Literal(Cow::Borrowed(
                LEGACY_SHIFT_ENTER_SEQUENCE
            )))
        );
        assert_eq!(
            terminal_key_sequence(&option_shift_enter, &mode, true),
            None
        );
    }

    #[test]
    fn plain_enter_keeps_backend_protocol_encoding() {
        let mode = Modes::empty();
        let enter = Keystroke::parse("enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&enter, &mode, true),
            Some(TerminalKeySequence::Protocol(Cow::Borrowed("\r")))
        );
    }
}
