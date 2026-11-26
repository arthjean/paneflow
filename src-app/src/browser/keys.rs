use paneflow_browser_proto::surface::{Input, KeyKind, Modifiers};

const VK_BACK: i32 = 0x08;
const VK_TAB: i32 = 0x09;
const VK_RETURN: i32 = 0x0D;
const VK_ESCAPE: i32 = 0x1B;

pub(crate) fn modifiers(modifiers: &gpui::Modifiers) -> Modifiers {
    Modifiers::default()
        .with(Modifiers::SHIFT, modifiers.shift)
        .with(Modifiers::CONTROL, modifiers.control)
        .with(Modifiers::ALT, modifiers.alt)
        .with(Modifiers::COMMAND, modifiers.platform)
}

pub(crate) fn pointer_modifiers(
    modifiers: &gpui::Modifiers,
    pressed: Option<gpui::MouseButton>,
) -> Modifiers {
    let held = |button| pressed == Some(button);
    self::modifiers(modifiers)
        .with(Modifiers::LEFT_BUTTON, held(gpui::MouseButton::Left))
        .with(Modifiers::MIDDLE_BUTTON, held(gpui::MouseButton::Middle))
        .with(Modifiers::RIGHT_BUTTON, held(gpui::MouseButton::Right))
}

pub(crate) fn windows_key_code(key: &str) -> Option<i32> {
    let mut chars = key.chars();
    if let (Some(single), None) = (chars.next(), chars.next()) {
        let code = match single.to_ascii_lowercase() {
            letter @ 'a'..='z' => 0x41 + (letter as i32 - 'a' as i32),
            digit @ '0'..='9' => 0x30 + (digit as i32 - '0' as i32),
            ';' | ':' => 0xBA,
            '=' | '+' => 0xBB,
            ',' | '<' => 0xBC,
            '-' | '_' => 0xBD,
            '.' | '>' => 0xBE,
            '/' | '?' => 0xBF,
            '`' | '~' => 0xC0,
            '[' | '{' => 0xDB,
            '\\' | '|' => 0xDC,
            ']' | '}' => 0xDD,
            '\'' | '"' => 0xDE,
            ' ' => 0x20,
            _ => return None,
        };
        return Some(code);
    }
    if let Some(number) = key.strip_prefix('f').and_then(|n| n.parse::<i32>().ok())
        && (1..=24).contains(&number)
    {
        return Some(0x6F + number);
    }
    Some(match key {
        "backspace" => VK_BACK,
        "tab" => VK_TAB,
        "enter" => VK_RETURN,
        "escape" => VK_ESCAPE,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "shift" => 0x10,
        "control" => 0x11,
        "alt" => 0x12,
        "platform" => 0x5B,
        "capslock" => 0x14,
        _ => return None,
    })
}

fn control_character(key: &str) -> Option<u16> {
    match key {
        "backspace" => Some(8),
        "tab" => Some(9),
        "enter" => Some(13),
        "escape" => Some(27),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KeyDown {
    pub(crate) events: Vec<Input>,
    pub(crate) consumed: bool,
}

pub(crate) fn key_down(keystroke: &gpui::Keystroke, repeat: bool) -> Option<KeyDown> {
    let code = windows_key_code(&keystroke.key)?;
    let flags = modifiers(&keystroke.modifiers).with(Modifiers::IS_REPEAT, repeat);
    let typed = keystroke
        .key_char
        .as_deref()
        .and_then(|text| text.encode_utf16().next());
    let control = control_character(&keystroke.key);
    let character = control.or(typed).unwrap_or(0);
    let mut events = vec![Input::Key {
        kind: KeyKind::RawDown,
        windows_key_code: code,
        native_key_code: 0,
        character,
        unmodified_character: character,
        modifiers: flags,
    }];
    let commands = keystroke.modifiers.control || keystroke.modifiers.platform;
    let consumed = control.is_some() || commands || keystroke.key_char.is_none();
    if let Some(character) = control.filter(|_| !commands) {
        events.push(Input::Key {
            kind: KeyKind::Char,
            windows_key_code: i32::from(character),
            native_key_code: 0,
            character,
            unmodified_character: character,
            modifiers: flags,
        });
    }
    Some(KeyDown { events, consumed })
}

pub(crate) fn key_up(keystroke: &gpui::Keystroke) -> Option<Input> {
    let code = windows_key_code(&keystroke.key)?;
    Some(Input::Key {
        kind: KeyKind::Up,
        windows_key_code: code,
        native_key_code: 0,
        character: 0,
        unmodified_character: 0,
        modifiers: modifiers(&keystroke.modifiers),
    })
}

pub(crate) fn typed(text: &str, modifiers: Modifiers) -> Vec<Input> {
    text.encode_utf16()
        .map(|unit| {
            let unit = if unit == u16::from(b'\n') { 13 } else { unit };
            Input::Key {
                kind: KeyKind::Char,
                windows_key_code: i32::from(unit),
                native_key_code: 0,
                character: unit,
                unmodified_character: unit,
                modifiers,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(key: &str, key_char: Option<&str>, control: bool) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: gpui::Modifiers {
                control,
                ..Default::default()
            },
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        }
    }

    #[test]
    fn letters_digits_and_named_keys_map_to_windows_codes() {
        assert_eq!(windows_key_code("a"), Some(0x41));
        assert_eq!(windows_key_code("Z"), Some(0x5A));
        assert_eq!(windows_key_code("7"), Some(0x37));
        assert_eq!(windows_key_code("f5"), Some(0x74));
        assert_eq!(windows_key_code("enter"), Some(0x0D));
        assert_eq!(windows_key_code("left"), Some(0x25));
        assert_eq!(windows_key_code("/"), Some(0xBF));
        assert_eq!(windows_key_code("unknown"), None);
    }

    #[test]
    fn a_printable_key_leaves_its_text_to_the_input_handler() {
        let down = key_down(&stroke("a", Some("a"), false), false).unwrap();
        assert!(!down.consumed);
        assert_eq!(down.events.len(), 1);
    }

    #[test]
    fn enter_and_backspace_send_their_character_and_stop_there() {
        for (key, character) in [("enter", 13u16), ("backspace", 8), ("tab", 9)] {
            let down = key_down(&stroke(key, None, false), false).unwrap();
            assert!(down.consumed, "{key}");
            assert!(matches!(
                down.events.as_slice(),
                [
                    Input::Key { kind: KeyKind::RawDown, .. },
                    Input::Key { kind: KeyKind::Char, character: c, .. }
                ] if *c == character
            ));
        }
    }

    #[test]
    fn a_command_chord_is_a_raw_key_with_its_modifiers_and_no_text() {
        let down = key_down(&stroke("a", Some("a"), true), false).unwrap();
        assert!(down.consumed);
        assert!(matches!(
            down.events.as_slice(),
            [Input::Key { kind: KeyKind::RawDown, windows_key_code: 0x41, modifiers, .. }]
                if modifiers.has(Modifiers::CONTROL)
        ));
    }

    #[test]
    fn typed_text_becomes_one_character_event_per_utf16_unit() {
        let events = typed("é\n", Modifiers::default());
        let characters: Vec<u16> = events
            .iter()
            .map(|event| match event {
                Input::Key { character, .. } => *character,
                _ => 0,
            })
            .collect();
        assert_eq!(characters, [0xE9, 13]);
    }
}
