use std::ffi::c_void;

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct KeyEncoderOverrides {
    option_as_alt: OptionAsAlt,
}

impl DisplayTerminal {
    pub(crate) fn apply_key_encoder_overrides(&self) {
        let option_as_alt = self.key_encoder_overrides.option_as_alt.raw();
        unsafe {
            sys::ghostty_key_encoder_setopt(
                self.key_encoder.raw(),
                sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
                (&raw const option_as_alt).cast::<c_void>(),
            );
        }
    }

    pub fn set_option_as_alt(&mut self, option_as_alt: OptionAsAlt) {
        self.key_encoder_overrides.option_as_alt = option_as_alt;
        self.apply_key_encoder_overrides();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Key, KeyAction, KeyInput, Modifiers, MouseAction, MouseInput, TerminalAppearance,
        WindowSize,
    };

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
