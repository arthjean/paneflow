use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::input_map::{key_action, key_code, mouse_action, mouse_button};
use crate::{FocusEvent, GhosttyError, KeyInput, Modifiers, MouseInput, Result};

const MAX_KEY_TEXT_BYTES: usize = 64 * 1024;
const MAX_KEY_OR_POINTER_OUTPUT_BYTES: usize = 64 * 1024;

struct KeyTextGuard<'a> {
    event: sys::GhosttyKeyEvent,
    _text: std::marker::PhantomData<&'a str>,
}

const NO_KEY_TEXT: sys::GhosttyString = sys::GhosttyString {
    ptr: std::ptr::null(),
    len: 0,
};

impl<'a> KeyTextGuard<'a> {
    fn set(event: sys::GhosttyKeyEvent, text: &'a str) -> Result<Self> {
        let utf8 = if text.is_empty() {
            NO_KEY_TEXT
        } else {
            sys::GhosttyString {
                ptr: text.as_ptr(),
                len: text.len(),
            }
        };
        check("key_event_set_utf8", unsafe {
            sys::ghostty_key_event_set_utf8(event, utf8)
        })?;
        Ok(Self {
            event,
            _text: std::marker::PhantomData,
        })
    }
}

impl Drop for KeyTextGuard<'_> {
    fn drop(&mut self) {
        unsafe { sys::ghostty_key_event_set_utf8(self.event, NO_KEY_TEXT) };
    }
}

impl DisplayTerminal {
    pub fn encode_key(&mut self, input: &KeyInput) -> Result<Vec<u8>> {
        if input.text.len() > MAX_KEY_TEXT_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "key text",
                limit: MAX_KEY_TEXT_BYTES,
            });
        }
        let key = key_code(input.key);
        if key == sys::GhosttyKey_GHOSTTY_KEY_UNIDENTIFIED && input.text.is_empty() {
            return Ok(Vec::new());
        }
        check("key_encoder_set_from_terminal", unsafe {
            sys::ghostty_key_encoder_set_from_terminal(self.key_encoder.raw(), self.terminal.raw())
        })?;
        self.apply_key_encoder_overrides()?;
        let event = self.key_event.raw();
        let altgr = Modifiers::CONTROL | Modifiers::ALT;
        let modifiers = if !input.text.is_empty() && input.consumed_modifiers.contains(altgr) {
            input.modifiers.without(altgr)
        } else {
            input.modifiers
        };
        unsafe {
            check(
                "key_event_set_key",
                sys::ghostty_key_event_set_key(event, key),
            )?;
            check(
                "key_event_set_action",
                sys::ghostty_key_event_set_action(event, key_action(input.action)),
            )?;
            check(
                "key_event_set_mods",
                sys::ghostty_key_event_set_mods(event, modifiers.bits()),
            )?;
            check(
                "key_event_set_consumed_mods",
                sys::ghostty_key_event_set_consumed_mods(event, input.consumed_modifiers.bits()),
            )?;
            check(
                "key_event_set_composing",
                sys::ghostty_key_event_set_composing(event, input.composing),
            )?;
            check(
                "key_event_set_unshifted_codepoint",
                sys::ghostty_key_event_set_unshifted_codepoint(
                    event,
                    input.unshifted_codepoint.map_or(0, u32::from),
                ),
            )?;
        }
        let _text = KeyTextGuard::set(event, &input.text)?;
        encode_with_buffer(
            "key_encoder_encode",
            MAX_KEY_OR_POINTER_OUTPUT_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_key_encoder_encode(
                    self.key_encoder.raw(),
                    self.key_event.raw(),
                    buffer,
                    len,
                    written,
                )
            },
        )
    }

    pub fn encode_mouse(&mut self, input: MouseInput) -> Result<Vec<u8>> {
        let geometry = crate::engine::MouseEncoderSize {
            screen_width: input.screen_width,
            screen_height: input.screen_height,
            cell_width: self.callbacks_size().cell_width,
            cell_height: self.callbacks_size().cell_height,
            padding_top: input.padding_top,
            padding_bottom: input.padding_bottom,
            padding_right: input.padding_right,
            padding_left: input.padding_left,
        };
        let size = sys::GhosttyMouseEncoderSize {
            size: std::mem::size_of::<sys::GhosttyMouseEncoderSize>(),
            screen_width: geometry.screen_width,
            screen_height: geometry.screen_height,
            cell_width: geometry.cell_width,
            cell_height: geometry.cell_height,
            padding_top: geometry.padding_top,
            padding_bottom: geometry.padding_bottom,
            padding_right: geometry.padding_right,
            padding_left: geometry.padding_left,
        };
        let track_last_cell = true;
        let mouse_modes = self.mouse_modes()?;
        let encoder = self.mouse_encoder.raw();
        if self.mouse_encoder_modes != Some(mouse_modes) {
            check("mouse_encoder_set_from_terminal", unsafe {
                sys::ghostty_mouse_encoder_set_from_terminal(encoder, self.terminal.raw())
            })?;
            self.mouse_encoder_modes = Some(mouse_modes);
        }
        if self.mouse_encoder_size != Some(geometry) {
            check("mouse_encoder_set_size", unsafe {
                sys::ghostty_mouse_encoder_set(
                    encoder,
                    sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                    (&raw const size).cast(),
                )
            })?;
            self.mouse_encoder_size = Some(geometry);
        }
        let event = self.mouse_event.raw();
        unsafe {
            check(
                "mouse_encoder_set_any_button_pressed",
                sys::ghostty_mouse_encoder_set(
                    encoder,
                    sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                    (&raw const input.any_button_pressed).cast(),
                ),
            )?;
            check(
                "mouse_encoder_set_track_last_cell",
                sys::ghostty_mouse_encoder_set(
                    encoder,
                    sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                    (&raw const track_last_cell).cast(),
                ),
            )?;
            check(
                "mouse_event_set_action",
                sys::ghostty_mouse_event_set_action(event, mouse_action(input.action)),
            )?;
            check(
                "mouse_event_set_mods",
                sys::ghostty_mouse_event_set_mods(event, input.modifiers.bits()),
            )?;
            check(
                "mouse_event_set_position",
                sys::ghostty_mouse_event_set_position(
                    event,
                    sys::GhosttyMousePosition {
                        x: input.x,
                        y: input.y,
                    },
                ),
            )?;
            if let Some(button) = input.button {
                check(
                    "mouse_event_set_button",
                    sys::ghostty_mouse_event_set_button(event, mouse_button(button)),
                )?;
            } else {
                check(
                    "mouse_event_clear_button",
                    sys::ghostty_mouse_event_clear_button(event),
                )?;
            }
        }
        encode_with_buffer(
            "mouse_encoder_encode",
            MAX_KEY_OR_POINTER_OUTPUT_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_mouse_encoder_encode(
                    self.mouse_encoder.raw(),
                    self.mouse_event.raw(),
                    buffer,
                    len,
                    written,
                )
            },
        )
    }

    pub fn encode_focus(&self, event: FocusEvent) -> Result<Vec<u8>> {
        if !self.modes()?.focus_reporting {
            return Ok(Vec::new());
        }
        let event = match event {
            FocusEvent::Gained => sys::GhosttyFocusEvent_GHOSTTY_FOCUS_GAINED,
            FocusEvent::Lost => sys::GhosttyFocusEvent_GHOSTTY_FOCUS_LOST,
        };
        encode_with_buffer(
            "focus_encode",
            MAX_KEY_OR_POINTER_OUTPUT_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_focus_encode(event, buffer, len, written)
            },
        )
    }

    fn callbacks_size(&self) -> crate::WindowSize {
        self.callbacks.size()
    }
}

pub(crate) fn encode_with_buffer(
    operation: &'static str,
    max_output_bytes: usize,
    mut encode: impl FnMut(*mut u8, usize, *mut usize) -> sys::GhosttyResult,
) -> Result<Vec<u8>> {
    let mut output = vec![0u8; 128];
    let mut written = 0usize;
    let mut result = encode(output.as_mut_ptr(), output.len(), &mut written);
    if result == sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        if written > max_output_bytes {
            return Err(GhosttyError::LimitExceeded {
                resource: "encoded input",
                limit: max_output_bytes,
            });
        }
        output.resize(written, 0);
        result = encode(output.as_mut_ptr(), output.len(), &mut written);
    }
    check(operation, result)?;
    if written > output.len() {
        return Err(GhosttyError::AbiMismatch(format!(
            "{operation} reported {written} bytes after receiving a {}-byte buffer",
            output.len()
        )));
    }
    output.truncate(written);
    Ok(output)
}
