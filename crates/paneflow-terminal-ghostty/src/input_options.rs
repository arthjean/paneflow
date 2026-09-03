use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::input_map::{key_action, key_from_code, mouse_action, mouse_button_from_code};
use crate::{GhosttyError, Key, KeyAction, Modifiers, MouseAction, MouseButton, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptionAsAlt {
    #[default]
    Never,
    Always,
    Left,
    Right,
}

impl OptionAsAlt {
    fn raw(self) -> sys::GhosttyOptionAsAlt {
        use sys as s;
        match self {
            Self::Never => s::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_FALSE,
            Self::Always => s::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_TRUE,
            Self::Left => s::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_LEFT,
            Self::Right => s::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_RIGHT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEventState {
    pub action: KeyAction,
    pub key: Key,
    pub modifiers: Modifiers,
    pub consumed_modifiers: Modifiers,
    pub composing: bool,
    pub text: String,
    pub unshifted_codepoint: Option<char>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MouseEventState {
    pub action: MouseAction,
    pub button: Option<MouseButton>,
    pub modifiers: Modifiers,
    pub position: (f32, f32),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyEncoderOverrides {
    option_as_alt: OptionAsAlt,
    backarrow_sends_backspace: bool,
}

impl DisplayTerminal {
    pub(crate) fn apply_key_encoder_overrides(&self) {
        let option_as_alt = self.key_encoder_overrides.option_as_alt.raw();
        let backarrow = self.key_encoder_overrides.backarrow_sends_backspace;
        unsafe {
            sys::ghostty_key_encoder_setopt(
                self.key_encoder.raw(),
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw const option_as_alt).cast::<c_void>(),
            );
            if backarrow {
                sys::ghostty_key_encoder_setopt(
                    self.key_encoder.raw(),
                    sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_BACKARROW_KEY_MODE,
                    (&raw const backarrow).cast::<c_void>(),
                );
            }
        }
    }

    pub fn set_option_as_alt(&mut self, option_as_alt: OptionAsAlt) {
        self.key_encoder_overrides.option_as_alt = option_as_alt;
        self.apply_key_encoder_overrides();
    }

    pub fn set_backarrow_sends_backspace(&mut self, enabled: bool) {
        self.key_encoder_overrides.backarrow_sends_backspace = enabled;
        self.apply_key_encoder_overrides();
    }

    pub fn reset_mouse_encoder(&mut self) {
        unsafe { sys::ghostty_mouse_encoder_reset(self.mouse_encoder.raw()) };
    }

    pub fn last_key_event(&self) -> Result<KeyEventState> {
        let event = self.key_event.raw();
        let (action, key, modifiers, consumed, composing, unshifted) = unsafe {
            (
                sys::ghostty_key_event_get_action(event),
                sys::ghostty_key_event_get_key(event),
                sys::ghostty_key_event_get_mods(event),
                sys::ghostty_key_event_get_consumed_mods(event),
                sys::ghostty_key_event_get_composing(event),
                sys::ghostty_key_event_get_unshifted_codepoint(event),
            )
        };
        let mut len = 0usize;
        let text = unsafe {
            let pointer = sys::ghostty_key_event_get_utf8(event, &mut len);
            if pointer.is_null() || len == 0 {
                String::new()
            } else {
                std::str::from_utf8(std::slice::from_raw_parts(pointer.cast::<u8>(), len))
                    .map_err(|_| GhosttyError::InvalidUtf8("key event text"))?
                    .to_owned()
            }
        };
        Ok(KeyEventState {
            action: decode_key_action(action)?,
            key: key_from_code(key),
            modifiers: Modifiers::from_bits(modifiers),
            consumed_modifiers: Modifiers::from_bits(consumed),
            composing,
            text,
            unshifted_codepoint: char::from_u32(unshifted).filter(|_| unshifted != 0),
        })
    }

    pub fn last_mouse_event(&self) -> Result<MouseEventState> {
        let event = self.mouse_event.raw();
        let mut button = sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT;
        let (has_button, action, modifiers, position) = unsafe {
            (
                sys::ghostty_mouse_event_get_button(event, &mut button),
                sys::ghostty_mouse_event_get_action(event),
                sys::ghostty_mouse_event_get_mods(event),
                sys::ghostty_mouse_event_get_position(event),
            )
        };
        Ok(MouseEventState {
            action: decode_mouse_action(action)?,
            button: has_button.then(|| mouse_button_from_code(button)).flatten(),
            modifiers: Modifiers::from_bits(modifiers),
            position: (position.x, position.y),
        })
    }
}

fn decode_key_action(value: sys::GhosttyKeyAction) -> Result<KeyAction> {
    for candidate in [KeyAction::Press, KeyAction::Release, KeyAction::Repeat] {
        if key_action(candidate) == value {
            return Ok(candidate);
        }
    }
    Err(GhosttyError::AbiMismatch(format!(
        "unknown Ghostty key action {value}"
    )))
}

fn decode_mouse_action(value: sys::GhosttyMouseAction) -> Result<MouseAction> {
    for candidate in [MouseAction::Press, MouseAction::Release, MouseAction::Motion] {
        if mouse_action(candidate) == value {
            return Ok(candidate);
        }
    }
    Err(GhosttyError::AbiMismatch(format!(
        "unknown Ghostty mouse action {value}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KeyInput, MouseInput, TerminalAppearance, WindowSize};

    fn terminal() -> DisplayTerminal {
        let size = WindowSize::new(20, 4, 8, 16).expect("valid terminal size");
        DisplayTerminal::new(size, 100, TerminalAppearance::default())
            .expect("terminal must initialize")
    }

    fn key(text: &str, modifiers: Modifiers) -> KeyInput {
        KeyInput {
            key: Key::Character('a'),
            action: KeyAction::Press,
            modifiers,
            consumed_modifiers: Modifiers::empty(),
            text: text.to_owned(),
            composing: false,
            unshifted_codepoint: Some('a'),
        }
    }

    #[test]
    fn a_key_event_reads_back_the_way_it_was_encoded() {
        let mut terminal = terminal();
        terminal
            .encode_key(&key("a", Modifiers::CONTROL))
            .expect("encode");

        let state = terminal.last_key_event().expect("read back");
        assert_eq!(state.key, Key::Character('a'));
        assert_eq!(state.action, KeyAction::Press);
        assert_eq!(state.modifiers, Modifiers::CONTROL);
        assert!(!state.composing);
        assert_eq!(state.unshifted_codepoint, Some('a'));
        assert!(state.text.is_empty());
    }

    #[test]
    fn a_mouse_event_reads_back_the_way_it_was_encoded() {
        let mut terminal = terminal();
        terminal.feed(b"\x1b[?1006h\x1b[?1000h").expect("sgr mouse on");
        terminal
            .encode_mouse(MouseInput {
                action: MouseAction::Press,
                button: Some(MouseButton::Left),
                modifiers: Modifiers::SHIFT,
                x: 32.0,
                y: 48.0,
                screen_width: 160,
                screen_height: 64,
                padding_top: 0,
                padding_bottom: 0,
                padding_left: 0,
                padding_right: 0,
                any_button_pressed: true,
            })
            .expect("encode");

        let state = terminal.last_mouse_event().expect("read back");
        assert_eq!(state.action, MouseAction::Press);
        assert_eq!(state.button, Some(MouseButton::Left));
        assert_eq!(state.modifiers, Modifiers::SHIFT);
        assert_eq!(state.position, (32.0, 48.0));
    }

    #[test]
    fn option_as_alt_is_accepted_and_survives_an_encode() {
        let alt_escape = b"\x1ba".as_slice();
        let layout_text: &[u8] = if cfg!(target_os = "macos") {
            b"a"
        } else {
            alt_escape
        };

        let mut terminal = terminal();
        for (option_as_alt, expected) in [
            (OptionAsAlt::Always, alt_escape),
            (OptionAsAlt::Never, layout_text),
            (OptionAsAlt::Left, alt_escape),
        ] {
            terminal.set_option_as_alt(option_as_alt);
            assert_eq!(
                terminal
                    .encode_key(&key("a", Modifiers::ALT))
                    .expect("encode"),
                expected,
                "{option_as_alt:?} encodes the wrong bytes"
            );
        }
    }

    #[test]
    fn the_backarrow_mode_swaps_delete_for_backspace() {
        let mut terminal = terminal();
        let backspace = KeyInput {
            key: Key::Backspace,
            action: KeyAction::Press,
            modifiers: Modifiers::empty(),
            consumed_modifiers: Modifiers::empty(),
            text: String::new(),
            composing: false,
            unshifted_codepoint: None,
        };

        assert_eq!(
            terminal.encode_key(&backspace).expect("encode"),
            b"\x7f",
            "the default is DEL"
        );
        terminal.set_backarrow_sends_backspace(true);
        assert_eq!(terminal.encode_key(&backspace).expect("encode"), b"\x08");
    }

    #[test]
    fn resetting_the_mouse_encoder_forgets_the_last_cell() {
        let mut terminal = terminal();
        terminal.feed(b"\x1b[?1006h\x1b[?1003h").expect("any-motion on");
        let motion = MouseInput {
            action: MouseAction::Motion,
            button: None,
            modifiers: Modifiers::empty(),
            x: 32.0,
            y: 16.0,
            screen_width: 160,
            screen_height: 64,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: false,
        };

        let first = terminal.encode_mouse(motion).expect("encode");
        assert!(!first.is_empty());
        assert!(terminal.encode_mouse(motion).expect("encode").is_empty());

        let moved = MouseInput { x: 96.0, ..motion };
        assert!(!terminal.encode_mouse(moved).expect("encode").is_empty());

        terminal.reset_mouse_encoder();
        assert_eq!(terminal.encode_mouse(motion).expect("encode"), first);
    }

    #[test]
    fn changing_the_render_geometry_reconfigures_the_encoder() {
        let mut terminal = terminal();
        terminal
            .feed(b"\x1b[?1006h\x1b[?1003h")
            .expect("any-motion on");
        let motion = MouseInput {
            action: MouseAction::Motion,
            button: None,
            modifiers: Modifiers::empty(),
            x: 32.0,
            y: 16.0,
            screen_width: 160,
            screen_height: 64,
            padding_top: 0,
            padding_bottom: 0,
            padding_left: 0,
            padding_right: 0,
            any_button_pressed: false,
        };
        assert!(!terminal.encode_mouse(motion).expect("encode").is_empty());
        assert!(terminal.encode_mouse(motion).expect("encode").is_empty());

        let resized = MouseInput {
            screen_width: 320,
            ..motion
        };
        assert!(!terminal.encode_mouse(resized).expect("encode").is_empty());
    }
}
