use std::ffi::c_char;

use paneflow_libghostty_sys as sys;

use crate::engine::DisplayTerminal;
use crate::handles::check;
use crate::input_map::{key_action, key_code, mouse_action, mouse_button};
use crate::{FocusEvent, GhosttyError, KeyInput, Modifiers, MouseInput, Result};

const MAX_KEY_TEXT_BYTES: usize = 64 * 1024;
const MAX_PASTE_BYTES: usize = 1024 * 1024;
const MAX_KEY_OR_POINTER_OUTPUT_BYTES: usize = 64 * 1024;
const MAX_PASTE_OUTPUT_BYTES: usize = MAX_PASTE_BYTES + 12;

struct KeyTextGuard<'a> {
    event: sys::GhosttyKeyEvent,
    _text: std::marker::PhantomData<&'a str>,
}

impl<'a> KeyTextGuard<'a> {
    fn set(event: sys::GhosttyKeyEvent, text: &'a str) -> Self {
        let (pointer, len) = if text.is_empty() {
            (std::ptr::null(), 0)
        } else {
            (text.as_ptr().cast(), text.len())
        };
        unsafe { sys::ghostty_key_event_set_utf8(event, pointer, len) };
        Self {
            event,
            _text: std::marker::PhantomData,
        }
    }
}

impl Drop for KeyTextGuard<'_> {
    fn drop(&mut self) {
        unsafe { sys::ghostty_key_event_set_utf8(self.event, std::ptr::null(), 0) };
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
        unsafe {
            sys::ghostty_key_encoder_setopt_from_terminal(
                self.key_encoder.raw(),
                self.terminal.raw(),
            );
        }
        self.apply_key_encoder_overrides();
        unsafe {
            sys::ghostty_key_event_set_key(self.key_event.raw(), key);
            sys::ghostty_key_event_set_action(self.key_event.raw(), key_action(input.action));
            let altgr = Modifiers::CONTROL | Modifiers::ALT;
            let modifiers = if !input.text.is_empty() && input.consumed_modifiers.contains(altgr) {
                input.modifiers.without(altgr)
            } else {
                input.modifiers
            };
            sys::ghostty_key_event_set_mods(self.key_event.raw(), modifiers.bits());
            sys::ghostty_key_event_set_consumed_mods(
                self.key_event.raw(),
                input.consumed_modifiers.bits(),
            );
            sys::ghostty_key_event_set_composing(self.key_event.raw(), input.composing);
            sys::ghostty_key_event_set_unshifted_codepoint(
                self.key_event.raw(),
                input.unshifted_codepoint.map_or(0, u32::from),
            );
        }
        let _text = KeyTextGuard::set(self.key_event.raw(), &input.text);
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
        let mouse_modes = crate::engine::MouseModes::from(self.modes()?);
        if self.mouse_encoder_modes != Some(mouse_modes) {
            unsafe {
                sys::ghostty_mouse_encoder_setopt_from_terminal(
                    self.mouse_encoder.raw(),
                    self.terminal.raw(),
                );
            }
            self.mouse_encoder_modes = Some(mouse_modes);
        }
        if self.mouse_encoder_size != Some(geometry) {
            unsafe {
                sys::ghostty_mouse_encoder_setopt(
                    self.mouse_encoder.raw(),
                    sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
                    (&raw const size).cast(),
                );
            }
            self.mouse_encoder_size = Some(geometry);
        }
        unsafe {
            sys::ghostty_mouse_encoder_setopt(
                self.mouse_encoder.raw(),
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_ANY_BUTTON_PRESSED,
                (&input.any_button_pressed as *const bool).cast(),
            );
            sys::ghostty_mouse_encoder_setopt(
                self.mouse_encoder.raw(),
                sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_TRACK_LAST_CELL,
                (&track_last_cell as *const bool).cast(),
            );
            sys::ghostty_mouse_event_set_action(self.mouse_event.raw(), mouse_action(input.action));
            sys::ghostty_mouse_event_set_mods(self.mouse_event.raw(), input.modifiers.bits());
            sys::ghostty_mouse_event_set_position(
                self.mouse_event.raw(),
                sys::GhosttyMousePosition {
                    x: input.x,
                    y: input.y,
                },
            );
            if let Some(button) = input.button {
                sys::ghostty_mouse_event_set_button(self.mouse_event.raw(), mouse_button(button));
            } else {
                sys::ghostty_mouse_event_clear_button(self.mouse_event.raw());
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

    pub fn paste_is_safe(&self, data: &str) -> bool {
        unsafe { sys::ghostty_paste_is_safe(data.as_ptr().cast(), data.len()) }
    }

    pub fn encode_paste(&self, data: &str, allow_unsafe: bool) -> Result<Vec<u8>> {
        if data.len() > MAX_PASTE_BYTES {
            return Err(GhosttyError::LimitExceeded {
                resource: "paste",
                limit: MAX_PASTE_BYTES,
            });
        }
        if !allow_unsafe && !self.paste_is_safe(data) {
            return Err(GhosttyError::UnsafePaste);
        }
        let bracketed = self.modes()?.bracketed_paste;
        let mut input = data.as_bytes().to_vec();
        encode_with_buffer(
            "paste_encode",
            MAX_PASTE_OUTPUT_BYTES,
            |buffer, len, written| unsafe {
                sys::ghostty_paste_encode(
                    input.as_mut_ptr().cast(),
                    input.len(),
                    bracketed,
                    buffer,
                    len,
                    written,
                )
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
    mut encode: impl FnMut(*mut c_char, usize, *mut usize) -> sys::GhosttyResult,
) -> Result<Vec<u8>> {
    let mut output = vec![0u8; 128];
    let mut written = 0usize;
    let mut result = encode(output.as_mut_ptr().cast(), output.len(), &mut written);
    if result == sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE {
        if written > max_output_bytes {
            return Err(GhosttyError::LimitExceeded {
                resource: "encoded input",
                limit: max_output_bytes,
            });
        }
        output.resize(written, 0);
        result = encode(output.as_mut_ptr().cast(), output.len(), &mut written);
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
