use std::ffi::c_void;
use std::marker::PhantomData;

use paneflow_libghostty_sys as sys;

use crate::callbacks::{self, CallbackState};
use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check, create};
use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::{GhosttyError, Result, Rgb, WindowSize};

const MAX_APC_BYTES: usize = 1024 * 1024;

impl DisplayTerminal {
    pub fn new(size: WindowSize, max_scrollback: usize) -> Result<Self> {
        let size = size.validate()?;
        if max_scrollback > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        crate::abi::validate()?;
        let mut callbacks = Box::new(CallbackState::new(size));
        let options = sys::GhosttyTerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback,
        };
        let mut raw_terminal = std::ptr::null_mut();
        let result =
            unsafe { sys::ghostty_terminal_new(std::ptr::null(), &mut raw_terminal, options) };
        check("terminal_new", result)?;
        if raw_terminal.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "terminal_new returned a null handle".into(),
            ));
        }
        // SAFETY: `terminal_new` just returned this non-null, uniquely owned
        // handle using the default allocator, and `terminal_free` is its
        // matching destructor.
        let terminal = unsafe { OwnedHandle::from_raw(raw_terminal, sys::ghostty_terminal_free) };
        callbacks::install(terminal.raw(), (&mut *callbacks) as *mut CallbackState)?;
        configure_safety_limits(terminal.raw())?;

        // SAFETY: each constructor writes the named handle type using the
        // default allocator, and each paired function is that type's exact
        // libghostty destructor. No raw handle escapes these owners.
        let render_state = unsafe {
            create(
                "render_state_new",
                sys::ghostty_render_state_new,
                sys::ghostty_render_state_free,
            )?
        };
        // SAFETY: `row_iterator_new` and `row_iterator_free` are the matching
        // constructor/destructor pair for `GhosttyRenderStateRowIterator`.
        let row_iterator = unsafe {
            create(
                "row_iterator_new",
                sys::ghostty_render_state_row_iterator_new,
                sys::ghostty_render_state_row_iterator_free,
            )?
        };
        // SAFETY: `row_cells_new` and `row_cells_free` are the matching
        // constructor/destructor pair for `GhosttyRenderStateRowCells`.
        let row_cells = unsafe {
            create(
                "row_cells_new",
                sys::ghostty_render_state_row_cells_new,
                sys::ghostty_render_state_row_cells_free,
            )?
        };
        // SAFETY: `key_encoder_new` and `key_encoder_free` are the matching
        // constructor/destructor pair for `GhosttyKeyEncoder`.
        let key_encoder = unsafe {
            create(
                "key_encoder_new",
                sys::ghostty_key_encoder_new,
                sys::ghostty_key_encoder_free,
            )?
        };
        // SAFETY: `key_event_new` and `key_event_free` are the matching
        // constructor/destructor pair for `GhosttyKeyEvent`.
        let key_event = unsafe {
            create(
                "key_event_new",
                sys::ghostty_key_event_new,
                sys::ghostty_key_event_free,
            )?
        };
        // SAFETY: `mouse_encoder_new` and `mouse_encoder_free` are the matching
        // constructor/destructor pair for `GhosttyMouseEncoder`.
        let mouse_encoder = unsafe {
            create(
                "mouse_encoder_new",
                sys::ghostty_mouse_encoder_new,
                sys::ghostty_mouse_encoder_free,
            )?
        };
        // SAFETY: `mouse_event_new` and `mouse_event_free` are the matching
        // constructor/destructor pair for `GhosttyMouseEvent`.
        let mouse_event = unsafe {
            create(
                "mouse_event_new",
                sys::ghostty_mouse_event_new,
                sys::ghostty_mouse_event_free,
            )?
        };

        Ok(Self {
            mouse_event,
            mouse_encoder,
            key_event,
            key_encoder,
            row_cells,
            row_iterator,
            render_state,
            terminal,
            snapshot_cache: Default::default(),
            callbacks,
            color_queries: Default::default(),
            osc52: Default::default(),
            last_pwd: None,
            _not_send_or_sync: PhantomData,
        })
    }

    /// Configure the host theme colors used by OSC 10, 11, and 12 queries.
    /// Terminal applications use these synchronous replies to select a
    /// readable palette before their first stable redraw.
    pub fn set_default_colors(
        &mut self,
        foreground: Rgb,
        background: Rgb,
        cursor: Rgb,
    ) -> Result<()> {
        for (option, color) in [
            (
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
                foreground,
            ),
            (
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
                background,
            ),
            (
                sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
                cursor,
            ),
        ] {
            let color = sys::GhosttyColorRgb {
                r: color.r,
                g: color.g,
                b: color.b,
            };
            let result = unsafe {
                sys::ghostty_terminal_set(
                    self.terminal.raw(),
                    option,
                    (&color as *const sys::GhosttyColorRgb).cast(),
                )
            };
            check("terminal_set_default_color", result)?;
        }
        self.color_queries
            .set_colors(foreground, background, cursor);
        Ok(())
    }
}

fn configure_safety_limits(terminal: sys::GhosttyTerminal) -> Result<()> {
    let zero = 0u64;
    let disabled = false;
    let apc_limit = MAX_APC_BYTES;
    let kitty_apc_limit = 0usize;
    for (option, value) in [
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_STORAGE_LIMIT,
            (&zero as *const u64).cast::<c_void>(),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_FILE,
            (&disabled as *const bool).cast::<c_void>(),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_TEMP_FILE,
            (&disabled as *const bool).cast::<c_void>(),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_KITTY_IMAGE_MEDIUM_SHARED_MEM,
            (&disabled as *const bool).cast::<c_void>(),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES,
            (&apc_limit as *const usize).cast::<c_void>(),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_APC_MAX_BYTES_KITTY,
            (&kitty_apc_limit as *const usize).cast::<c_void>(),
        ),
    ] {
        let result = unsafe { sys::ghostty_terminal_set(terminal, option, value) };
        check("terminal_set_safety_limit", result)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BackendEvent;

    #[test]
    fn configured_default_colors_answer_osc_queries() {
        let mut terminal = DisplayTerminal::new(WindowSize::new(80, 24, 8, 16).unwrap(), 1_000)
            .expect("terminal must initialize");
        terminal
            .set_default_colors(
                Rgb {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                },
                Rgb {
                    r: 0x44,
                    g: 0x55,
                    b: 0x66,
                },
                Rgb {
                    r: 0x77,
                    g: 0x88,
                    b: 0x99,
                },
            )
            .expect("default colors must be accepted");

        terminal
            .feed(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\\x1b]12;?\x1b\\")
            .expect("color queries must parse");
        let replies = terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WritePty(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect::<Vec<_>>();

        assert!(
            replies
                .windows(b"]10;rgb:1111/2222/3333".len())
                .any(|window| window == b"]10;rgb:1111/2222/3333")
        );
        assert!(
            replies
                .windows(b"]11;rgb:4444/5555/6666".len())
                .any(|window| window == b"]11;rgb:4444/5555/6666")
        );
        assert!(
            replies
                .windows(b"]12;rgb:7777/8888/9999".len())
                .any(|window| window == b"]12;rgb:7777/8888/9999")
        );
    }
}
