use gpui::{App, KeyDownEvent, Modifiers, Pixels};
use paneflow_browser_protocol::{
    InputEvent, KeyKind, MODIFIER_ALT, MODIFIER_COMMAND, MODIFIER_CONTROL, MODIFIER_LEFT_MOUSE,
    MODIFIER_MIDDLE_MOUSE, MODIFIER_RIGHT_MOUSE, MODIFIER_SHIFT, MouseButton,
};

pub(super) fn modifiers(modifiers: &Modifiers) -> u32 {
    let mut value = 0;
    if modifiers.shift {
        value |= MODIFIER_SHIFT;
    }
    if modifiers.control {
        value |= MODIFIER_CONTROL;
    }
    if modifiers.alt {
        value |= MODIFIER_ALT;
    }
    if modifiers.platform {
        value |= MODIFIER_COMMAND;
    }
    value
}

pub(super) fn button_modifier(button: gpui::MouseButton) -> u32 {
    match button {
        gpui::MouseButton::Left => MODIFIER_LEFT_MOUSE,
        gpui::MouseButton::Middle => MODIFIER_MIDDLE_MOUSE,
        gpui::MouseButton::Right => MODIFIER_RIGHT_MOUSE,
        gpui::MouseButton::Navigate(_) => 0,
    }
}

pub(super) fn mouse_button(button: gpui::MouseButton) -> Option<MouseButton> {
    match button {
        gpui::MouseButton::Left => Some(MouseButton::Left),
        gpui::MouseButton::Middle => Some(MouseButton::Middle),
        gpui::MouseButton::Right => Some(MouseButton::Right),
        gpui::MouseButton::Navigate(_) => None,
    }
}

pub(super) fn key_code(key: &str) -> i32 {
    match key {
        "enter" => 13,
        "backspace" => 8,
        "tab" => 9,
        "escape" => 27,
        "space" => 32,
        "pageup" => 33,
        "pagedown" => 34,
        "end" => 35,
        "home" => 36,
        "left" => 37,
        "up" => 38,
        "right" => 39,
        "down" => 40,
        "delete" => 46,
        "shift" => 16,
        "control" => 17,
        "alt" => 18,
        "insert" => 45,
        ";" | ":" => 0xBA,
        "=" | "+" => 0xBB,
        "," | "<" => 0xBC,
        "-" | "_" => 0xBD,
        "." | ">" => 0xBE,
        "/" | "?" => 0xBF,
        "`" | "~" => 0xC0,
        "[" | "{" => 0xDB,
        "\\" | "|" => 0xDC,
        "]" | "}" => 0xDD,
        "'" | "\"" => 0xDE,
        "!" => 0x31,
        "@" => 0x32,
        "#" => 0x33,
        "$" => 0x34,
        "%" => 0x35,
        "^" => 0x36,
        "&" => 0x37,
        "*" => 0x38,
        "(" => 0x39,
        ")" => 0x30,
        function if function.starts_with('f') && (2..=3).contains(&function.len()) => function[1..]
            .parse::<i32>()
            .ok()
            .filter(|number| (1..=24).contains(number))
            .map(|number| 111 + number)
            .unwrap_or(0),
        other => other
            .chars()
            .next()
            .filter(|character| other.chars().count() == 1 && character.is_ascii_alphanumeric())
            .map(|character| character.to_ascii_uppercase() as i32)
            .unwrap_or(0),
    }
}

pub(super) fn relative_position(
    point: gpui::Point<Pixels>,
    origin: gpui::Point<Pixels>,
) -> (i32, i32) {
    (
        (f32::from(point.x) - f32::from(origin.x)).round() as i32,
        (f32::from(point.y) - f32::from(origin.y)).round() as i32,
    )
}

pub(super) fn key_events(keystroke: &gpui::Keystroke, down: bool) -> Vec<InputEvent> {
    let code = key_code(&keystroke.key);
    let flags = modifiers(&keystroke.modifiers);
    if !down {
        return vec![InputEvent::Key {
            kind: KeyKind::Up,
            key_code: code,
            native_key_code: 0,
            character: 0,
            unmodified_character: 0,
            modifiers: flags,
        }];
    }
    let mut events = vec![InputEvent::Key {
        kind: KeyKind::RawDown,
        key_code: code,
        native_key_code: 0,
        character: 0,
        unmodified_character: 0,
        modifiers: flags,
    }];
    if keystroke.modifiers.control || keystroke.modifiers.platform {
        return events;
    }
    let text = keystroke
        .key_char
        .as_deref()
        .or(match keystroke.key.as_str() {
            "enter" => Some("\r"),
            "tab" => Some("\t"),
            "space" => Some(" "),
            _ => None,
        });
    for character in text.into_iter().flat_map(str::encode_utf16) {
        events.push(InputEvent::Key {
            kind: KeyKind::Char,
            key_code: code,
            native_key_code: 0,
            character,
            unmodified_character: character,
            modifiers: flags,
        });
    }
    events
}

pub(super) fn consume_key_down(event: &KeyDownEvent, cx: &mut App) -> Vec<InputEvent> {
    cx.stop_propagation();
    let mut keystroke = event.keystroke.clone();
    if event.prefer_character_input && keystroke.key_char.is_some() {
        keystroke.modifiers.control = false;
        keystroke.modifiers.platform = false;
    }
    key_events(&keystroke, true)
}

#[derive(Default)]
pub(super) struct ScrollAccumulator {
    x: f32,
    y: f32,
}

impl ScrollAccumulator {
    pub(super) fn consume(&mut self, delta: gpui::ScrollDelta) -> (i32, i32) {
        let (x, y) = match delta {
            gpui::ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
            gpui::ScrollDelta::Lines(delta) => (delta.x * 40.0, delta.y * 40.0),
        };
        self.x += x;
        self.y += y;
        let result = (self.x.round() as i32, self.y.round() as i32);
        self.x -= result.0 as f32;
        self.y -= result.1 as f32;
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keystroke(key: &str, key_char: Option<&str>) -> gpui::Keystroke {
        gpui::Keystroke {
            modifiers: Modifiers::default(),
            key: key.to_string(),
            key_char: key_char.map(str::to_string),
        }
    }

    #[test]
    fn fractional_scroll_is_conserved_across_events_and_direction_changes() {
        let mut scroll = ScrollAccumulator::default();
        let mut total = 0;
        for _ in 0..10 {
            total += scroll
                .consume(gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::px(0.),
                    gpui::px(0.2),
                )))
                .1;
        }
        assert_eq!(total, 2);
        for _ in 0..10 {
            total += scroll
                .consume(gpui::ScrollDelta::Pixels(gpui::point(
                    gpui::px(0.),
                    gpui::px(-0.2),
                )))
                .1;
        }
        assert_eq!(total, 0);
    }

    #[test]
    fn a_printable_key_produces_a_raw_down_and_a_char_event() {
        let events = key_events(&keystroke("a", Some("a")), true);
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[1],
            InputEvent::Key {
                kind: KeyKind::Char,
                character: 97,
                ..
            }
        ));
    }

    #[test]
    fn enter_tab_and_space_carry_their_control_characters_without_a_key_char() {
        for (key, expected) in [("enter", 13), ("tab", 9), ("space", 32)] {
            let events = key_events(&keystroke(key, None), true);
            assert!(matches!(
                events[1],
                InputEvent::Key {
                    kind: KeyKind::Char,
                    character,
                    ..
                } if character == expected
            ));
        }
    }

    #[test]
    fn a_key_release_is_a_single_up_event_and_astral_text_keeps_both_utf16_units() {
        assert_eq!(key_events(&keystroke("a", Some("a")), false).len(), 1);
        let events = key_events(&keystroke("😀", Some("😀")), true);
        let units: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                InputEvent::Key {
                    kind: KeyKind::Char,
                    character,
                    ..
                } => Some(*character),
                _ => None,
            })
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), "😀");
    }

    #[test]
    fn positions_are_relative_to_the_viewport_origin() {
        let point = gpui::point(gpui::px(120.5), gpui::px(64.));
        let origin = gpui::point(gpui::px(100.), gpui::px(60.));
        assert_eq!(relative_position(point, origin), (21, 4));
    }

    #[test]
    fn punctuation_uses_virtual_keys_without_colliding_with_editing_commands() {
        for (text, code) in [(".", 0xBE), (",", 0xBC), ("-", 0xBD), ("'", 0xDE)] {
            let events = key_events(&keystroke(text, Some(text)), true);
            assert!(matches!(events[0], InputEvent::Key { key_code, .. } if key_code == code));
            assert!(
                matches!(events[1], InputEvent::Key { character, .. } if character == text.as_bytes()[0] as u16)
            );
        }
        assert_ne!(key_code("."), key_code("delete"));
        assert_ne!(key_code("-"), key_code("insert"));
        assert_ne!(key_code("'"), key_code("right"));
    }

    #[test]
    fn function_keys_do_not_shadow_the_letter_f() {
        assert_eq!(key_code("f"), 70);
        assert_eq!(key_code("f10"), 121);
        assert_eq!(key_code("f24"), 135);
        assert_eq!(key_code("insert"), 45);
    }
}
