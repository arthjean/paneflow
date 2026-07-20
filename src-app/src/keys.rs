//! Keystroke-to-escape-sequence mapping for terminal input.
//!
//! Returns `Cow::Borrowed` for all static sequences (zero allocation).
//! Only modifier+key combos that require formatting allocate via `Cow::Owned`.

use std::borrow::Cow;

use gpui::Keystroke;

use crate::terminal::types::Modes;

/// How a mapped terminal key sequence must be delivered.
///
/// Protocol sequences are legacy fallbacks: a native terminal encoder may
/// replace them when the child enabled a richer keyboard protocol. Literal
/// sequences are Paneflow bindings whose bytes are the behavior, so a backend
/// encoder must not reinterpret the original physical key.
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

fn shift_enter_sequence(
    keystroke: &Keystroke,
    mode: &Modes,
    option_as_meta: bool,
) -> Option<TerminalKeySequence> {
    let alt = keystroke.modifiers.alt && option_as_meta;
    if keystroke.key != "enter" || !keystroke.modifiers.shift || keystroke.modifiers.control || alt
    {
        return None;
    }

    if mode.contains(Modes::KITTY_KEYBOARD) {
        Some(TerminalKeySequence::Protocol(Cow::Borrowed("\x1b[13;2u")))
    } else {
        Some(TerminalKeySequence::Literal(Cow::Borrowed("\x0a")))
    }
}

/// Map a GPUI keystroke to a terminal escape sequence.
///
/// Returns `Some(Cow::Borrowed(...))` for static keys (zero-alloc),
/// `Some(Cow::Owned(...))` for modifier combos (one alloc),
/// or `None` if the keystroke should be handled as printable character input.
/// Platform default for `option_as_meta` when the user has not set it in
/// `paneflow.json`.
///
/// - **macOS** → `false`: the Option key composes Unicode (é, ©, …); treating
///   it as Meta (ESC-prefix) corrupts that input. This mirrors Zed, whose
///   `option_as_meta` defaults off on macOS.
/// - **Every other platform** → `true`: Alt-as-Meta (ESC-prefix) is the
///   conventional terminal behavior.
///
/// A user can always override per-platform via `paneflow.json#option_as_meta`;
/// `to_esc_str` then gates the ESC-prefix paths on the resolved flag while
/// `alt_phys` keeps Alt+Arrow working regardless.
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
    // Physical Alt, independent of the macOS Option-as-Meta toggle. `alt` (above)
    // gates the ESC-prefix paths for printable keys; `alt_phys` is what a CSI 1;N
    // cursor/function-key sequence must report, so Alt+Arrow works regardless of
    // `option_as_meta`. The config key is not macOS-only, so a Linux/Windows user
    // with `option_as_meta: false` would otherwise lose Alt+Arrow entirely.
    let alt_phys = keystroke.modifiers.alt;

    // Preserve the physical chord after a child negotiates Kitty keyboard
    // reporting. Legacy terminals cannot represent Shift+Enter portably, so
    // use LF there: agent editors receive it as the conventional Ctrl+J
    // multiline binding rather than a submitting carriage return.
    if let Some(sequence) = shift_enter_sequence(keystroke, mode, option_as_meta) {
        return Some(sequence.into_sequence());
    }

    // Modifier+cursor/function combos (CSI 1;N) - resolved first so a modified
    // nav/fn key is never swallowed by the unmodified-key fast path below. Only
    // nav/fn keys match here; letters, enter, tab, escape, backspace fall
    // through to the modifier-gated logic unchanged.
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
        // Modifier+cursor/F1-F4 → \x1b[1;{m}{letter}
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

        // Modifier+Delete/F5-F12/Insert/PageUp/PageDown → \x1b[{num};{m}~
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

    // Ctrl+letter → control byte (zero alloc via static strings)
    // Shift is allowed through: Ctrl+Shift+A produces the same byte as Ctrl+A
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
            "[" => Some("\x1b"), // Same as Escape - standard ANSI behavior
            "\\" => Some("\x1c"),
            "]" => Some("\x1d"),
            "^" => Some("\x1e"),
            "_" => Some("\x1f"),
            "@" => Some("\x00"),         // NUL
            "?" => Some("\x7f"),         // DEL
            "space" => Some("\x00"),     // NUL (same as Ctrl+@)
            "backspace" => Some("\x08"), // BS
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    // Special keys - no modifiers
    if !ctrl && !shift && !alt {
        let app_cursor = mode.contains(Modes::APP_CURSOR);
        let seq: Option<&'static str> = match key {
            "enter" => Some("\r"),
            "tab" => Some("\t"),
            "escape" => Some("\x1b"),
            "backspace" => Some("\x7f"),
            "delete" => Some("\x1b[3~"),
            "insert" => Some("\x1b[2~"),
            // Cursor keys: application mode (SS3) vs normal mode (CSI)
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
            // Function keys
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
            // F13-F20 (xterm numbering: 27 and 30 skipped)
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

    // Shift+special keys
    if shift && !ctrl && !alt {
        let seq: Option<&'static str> = match key {
            "tab" => Some("\x1b[Z"), // Back-tab
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    // Alt+special keys (multi-char key names that bypass the single-char Alt handler)
    if alt && !ctrl && !shift {
        let seq: Option<&'static str> = match key {
            "backspace" => Some("\x1b\x7f"), // ESC + DEL
            "enter" => Some("\x1b\x0d"),     // ESC + CR
            _ => None,
        };
        if let Some(s) = seq {
            return Some(Cow::Borrowed(s));
        }
    }

    // Alt+Shift+letter → ESC + uppercase letter. `chars().count() == 1` (not
    // `key.len()`, which is byte length) so a single accented key on a non-US
    // layout (AZERTY "é" is 2 UTF-8 bytes) isn't wrongly rejected.
    if alt
        && !ctrl
        && shift
        && key.chars().count() == 1
        && let Some(ch) = key.chars().next()
        && ch.is_ascii_alphabetic()
    {
        return Some(Cow::Owned(format!("\x1b{}", ch.to_ascii_uppercase())));
    }

    // Alt+key → ESC prefix. Same `chars().count()` guard so Alt+<accented> on
    // AZERTY/QWERTZ sends `ESC <char>` instead of falling through unhandled.
    if alt && !ctrl && !shift && key.chars().count() == 1 {
        return Some(Cow::Owned(format!("\x1b{key}")));
    }

    // Not a special key - caller should handle as printable character input
    None
}

/// Map a keystroke together with the delivery policy required by the mapping.
///
/// Most entries are protocol fallbacks because a stateful backend can encode
/// them more accurately after a child enables Kitty or xterm keyboard modes.
/// Legacy Shift+Enter is deliberately literal: passing it back through that
/// encoder turns LF into a modified-Enter escape sequence and loses multiline
/// input. Once Kitty keyboard reporting is active, the backend keeps the
/// structured key so the child receives the real Shift+Enter chord.
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
        // macOS composes Unicode on the Option key, so Alt-as-Meta (ESC-prefix)
        // must default OFF there and ON everywhere else. Regression guard for
        // the macOS "Option+e corrupts accents" bug.
        assert_eq!(default_option_as_meta(), !cfg!(target_os = "macos"));
        #[cfg(target_os = "macos")]
        assert!(!default_option_as_meta());
        #[cfg(not(target_os = "macos"))]
        assert!(default_option_as_meta());
    }

    #[test]
    fn page_keys_match_us009_alt_screen_constants() {
        // US-009 invariant: the alt-screen page-forward bytes hardcoded in
        // `terminal/input.rs` (`\x1b[5~` / `\x1b[6~`) must equal what
        // `to_esc_str` emits for a plain PageUp / PageDown, so `to_esc_str`
        // stays the single source of truth. If this fails, update both.
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
        // US-060: `option_as_meta` gates only the ESC-prefix paths, never the
        // CSI 1;N modifier code for cursor keys. Alt+Up emits `\x1b[1;3A` with
        // option_as_meta both off and on.
        let mode = Modes::empty();
        let up = Keystroke::parse("alt-up").expect("valid keystroke");
        assert_eq!(to_esc_str(&up, &mode, false).as_deref(), Some("\x1b[1;3A"));
        assert_eq!(to_esc_str(&up, &mode, true).as_deref(), Some("\x1b[1;3A"));
    }

    #[test]
    fn alt_accented_letter_sends_esc_prefix() {
        // US-060: a single multi-byte key (AZERTY "é" = 2 UTF-8 bytes) must take
        // the Alt -> ESC-prefix path; the old `key.len() == 1` byte check
        // wrongly rejected it.
        let mode = Modes::empty();
        let e_acute = Keystroke::parse("alt-é").expect("valid keystroke");
        assert_eq!(to_esc_str(&e_acute, &mode, true).as_deref(), Some("\x1bé"));
    }

    #[test]
    fn legacy_shift_enter_is_an_exact_line_feed_binding() {
        let mode = Modes::empty();
        let shift_enter = Keystroke::parse("shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&shift_enter, &mode, true),
            Some(TerminalKeySequence::Literal(Cow::Borrowed("\x0a")))
        );
    }

    #[test]
    fn kitty_shift_enter_preserves_the_physical_chord() {
        let mode = Modes::KITTY_KEYBOARD;
        let shift_enter = Keystroke::parse("shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&shift_enter, &mode, true),
            Some(TerminalKeySequence::Protocol(Cow::Borrowed("\x1b[13;2u")))
        );
    }

    #[test]
    fn shift_enter_respects_option_as_meta() {
        let mode = Modes::empty();
        let option_shift_enter = Keystroke::parse("alt-shift-enter").expect("valid keystroke");
        assert_eq!(
            terminal_key_sequence(&option_shift_enter, &mode, false),
            Some(TerminalKeySequence::Literal(Cow::Borrowed("\x0a")))
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
