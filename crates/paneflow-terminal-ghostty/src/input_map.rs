use paneflow_libghostty_sys as sys;

use crate::{Key, KeyAction, MouseAction, MouseButton};

pub(crate) fn key_action(action: KeyAction) -> sys::GhosttyKeyAction {
    match action {
        KeyAction::Release => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_RELEASE,
        KeyAction::Press => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS,
        KeyAction::Repeat => sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_REPEAT,
    }
}

pub(crate) fn key_code(key: Key) -> sys::GhosttyKey {
    match key {
        Key::Character(character) => character_key(character),
        Key::Enter => sys::GhosttyKey_GHOSTTY_KEY_ENTER,
        Key::Tab => sys::GhosttyKey_GHOSTTY_KEY_TAB,
        Key::Backspace => sys::GhosttyKey_GHOSTTY_KEY_BACKSPACE,
        Key::Delete => sys::GhosttyKey_GHOSTTY_KEY_DELETE,
        Key::Escape => sys::GhosttyKey_GHOSTTY_KEY_ESCAPE,
        Key::Up => sys::GhosttyKey_GHOSTTY_KEY_ARROW_UP,
        Key::Down => sys::GhosttyKey_GHOSTTY_KEY_ARROW_DOWN,
        Key::Left => sys::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT,
        Key::Right => sys::GhosttyKey_GHOSTTY_KEY_ARROW_RIGHT,
        Key::Home => sys::GhosttyKey_GHOSTTY_KEY_HOME,
        Key::End => sys::GhosttyKey_GHOSTTY_KEY_END,
        Key::PageUp => sys::GhosttyKey_GHOSTTY_KEY_PAGE_UP,
        Key::PageDown => sys::GhosttyKey_GHOSTTY_KEY_PAGE_DOWN,
        Key::Insert => sys::GhosttyKey_GHOSTTY_KEY_INSERT,
        Key::Function(number @ 1..=25) => {
            sys::GhosttyKey_GHOSTTY_KEY_F1 + sys::GhosttyKey::from(number - 1)
        }
        Key::NumpadDigit(number @ 0..=9) => {
            sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_0 + sys::GhosttyKey::from(number)
        }
        Key::NumpadAdd => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_ADD,
        Key::NumpadSubtract => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_SUBTRACT,
        Key::NumpadMultiply => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_MULTIPLY,
        Key::NumpadDivide => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_DIVIDE,
        Key::NumpadDecimal => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_DECIMAL,
        Key::NumpadEnter => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_ENTER,
        Key::NumpadEqual => sys::GhosttyKey_GHOSTTY_KEY_NUMPAD_EQUAL,
        Key::Function(_) | Key::NumpadDigit(_) | Key::Unidentified => {
            sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED
        }
    }
}

fn character_key(character: char) -> sys::GhosttyKey {
    match character.to_ascii_lowercase() {
        'a'..='z' => {
            sys::GhosttyKey_GHOSTTY_KEY_A + ascii_offset(character.to_ascii_lowercase(), 'a')
        }
        '0'..='9' => sys::GhosttyKey_GHOSTTY_KEY_DIGIT_0 + ascii_offset(character, '0'),
        ' ' => sys::GhosttyKey_GHOSTTY_KEY_SPACE,
        '-' => sys::GhosttyKey_GHOSTTY_KEY_MINUS,
        '=' => sys::GhosttyKey_GHOSTTY_KEY_EQUAL,
        '[' => sys::GhosttyKey_GHOSTTY_KEY_BRACKET_LEFT,
        ']' => sys::GhosttyKey_GHOSTTY_KEY_BRACKET_RIGHT,
        '\\' => sys::GhosttyKey_GHOSTTY_KEY_BACKSLASH,
        ';' => sys::GhosttyKey_GHOSTTY_KEY_SEMICOLON,
        '\'' => sys::GhosttyKey_GHOSTTY_KEY_QUOTE,
        ',' => sys::GhosttyKey_GHOSTTY_KEY_COMMA,
        '.' => sys::GhosttyKey_GHOSTTY_KEY_PERIOD,
        '/' => sys::GhosttyKey_GHOSTTY_KEY_SLASH,
        '`' => sys::GhosttyKey_GHOSTTY_KEY_BACKQUOTE,
        _ => sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED,
    }
}

/// Distance from the first character of a contiguous ASCII key range.
///
/// Callers reach this through an ASCII range pattern, so the difference always
/// fits; an out-of-range character falls back to the unidentified key, whose
/// discriminant is zero.
fn ascii_offset(character: char, first: char) -> sys::GhosttyKey {
    sys::GhosttyKey::try_from(u32::from(character).saturating_sub(u32::from(first)))
        .unwrap_or(sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED)
}

pub(crate) fn mouse_action(action: MouseAction) -> sys::GhosttyMouseAction {
    match action {
        MouseAction::Press => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
        MouseAction::Release => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE,
        MouseAction::Motion => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION,
    }
}

pub(crate) fn mouse_button(button: MouseButton) -> sys::GhosttyMouseButton {
    sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT + sys::GhosttyMouseButton::from(button as u8)
}

/// Recover the neutral key from a libghostty key code.
///
/// The forward table is the authority; this walks the ranges it produces
/// rather than duplicating the mapping, so the two cannot drift apart.
pub(crate) fn key_from_code(code: sys::GhosttyKey) -> Key {
    const NAMED: &[Key] = &[
        Key::Enter,
        Key::Tab,
        Key::Backspace,
        Key::Delete,
        Key::Escape,
        Key::Up,
        Key::Down,
        Key::Left,
        Key::Right,
        Key::Home,
        Key::End,
        Key::PageUp,
        Key::PageDown,
        Key::Insert,
        Key::NumpadAdd,
        Key::NumpadSubtract,
        Key::NumpadMultiply,
        Key::NumpadDivide,
        Key::NumpadDecimal,
        Key::NumpadEnter,
        Key::NumpadEqual,
    ];
    if code == sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED {
        return Key::Unidentified;
    }
    if let Some(key) = NAMED.iter().copied().find(|key| key_code(*key) == code) {
        return key;
    }
    for number in 1..=25u8 {
        if key_code(Key::Function(number)) == code {
            return Key::Function(number);
        }
    }
    for number in 0..=9u8 {
        if key_code(Key::NumpadDigit(number)) == code {
            return Key::NumpadDigit(number);
        }
    }
    const CHARACTERS: &str = "abcdefghijklmnopqrstuvwxyz0123456789 -=[]\\;',./`";
    CHARACTERS
        .chars()
        .find(|character| key_code(Key::Character(*character)) == code)
        .map_or(Key::Unidentified, Key::Character)
}

/// Recover the neutral mouse button from a libghostty button code.
pub(crate) fn mouse_button_from_code(code: sys::GhosttyMouseButton) -> Option<MouseButton> {
    const BUTTONS: &[MouseButton] = &[
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
        MouseButton::Four,
        MouseButton::Five,
        MouseButton::Six,
        MouseButton::Seven,
        MouseButton::Eight,
        MouseButton::Nine,
        MouseButton::Ten,
        MouseButton::Eleven,
    ];
    BUTTONS
        .iter()
        .copied()
        .find(|button| mouse_button(*button) == code)
}
