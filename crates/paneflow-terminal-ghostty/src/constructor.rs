use std::ffi::c_void;
use std::marker::PhantomData;

use paneflow_libghostty_sys as sys;

use crate::callbacks::{self, CallbackState};
use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check, create};
use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::osc::OscRouter;
use crate::{GhosttyError, Result, TerminalAppearance, WindowSize};

const MAX_APC_BYTES: usize = 1024 * 1024;

impl DisplayTerminal {
    pub fn new(
        size: WindowSize,
        max_scrollback: usize,
        appearance: TerminalAppearance,
    ) -> Result<Self> {
        // SAFETY: a null pointer selects libghostty's process-lifetime default
        // allocator, so every owned handle may retain it until Drop.
        unsafe { Self::new_with_allocator(size, max_scrollback, appearance, std::ptr::null()) }
    }

    pub fn set_appearance(&mut self, appearance: TerminalAppearance) -> Result<()> {
        configure_appearance(self.terminal.raw(), appearance)?;
        self.callbacks.set_color_scheme(appearance.color_scheme);
        self.osc.set_appearance(appearance);
        self.snapshot_cache.invalidate();
        Ok(())
    }

    /// Construct with an allocator that remains valid through this terminal's Drop.
    ///
    /// # Safety
    ///
    /// `allocator` must be null or point to an allocator whose vtable and
    /// context outlive the returned terminal and every native handle it owns.
    unsafe fn new_with_allocator(
        size: WindowSize,
        max_scrollback: usize,
        appearance: TerminalAppearance,
        allocator: *const sys::GhosttyAllocator,
    ) -> Result<Self> {
        let size = size.validate()?;
        if max_scrollback > MAX_SCROLLBACK_ROWS {
            return Err(GhosttyError::LimitExceeded {
                resource: "scrollback rows",
                limit: MAX_SCROLLBACK_ROWS,
            });
        }
        crate::abi::validate()?;
        let mut callbacks = Box::new(CallbackState::new(size, appearance.color_scheme));
        let options = sys::GhosttyTerminalOptions {
            cols: size.cols,
            rows: size.rows,
            max_scrollback,
        };
        let mut raw_terminal = std::ptr::null_mut();
        let result = unsafe { sys::ghostty_terminal_new(allocator, &mut raw_terminal, options) };
        check("terminal_new", result)?;
        if raw_terminal.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "terminal_new returned a null handle".into(),
            ));
        }
        // SAFETY: `terminal_new` just returned this non-null, uniquely owned
        // handle using the selected allocator, and `terminal_free` is its
        // matching destructor.
        let terminal = unsafe { OwnedHandle::from_raw(raw_terminal, sys::ghostty_terminal_free) };
        callbacks::install(terminal.raw(), (&mut *callbacks) as *mut CallbackState)?;
        configure_safety_limits(terminal.raw())?;
        configure_appearance(terminal.raw(), appearance)?;

        // SAFETY: each constructor writes the named handle type using the
        // selected allocator, and each paired function is that type's exact
        // libghostty destructor. No raw handle escapes these owners.
        let render_state = unsafe {
            create(
                "render_state_new",
                allocator,
                sys::ghostty_render_state_new,
                sys::ghostty_render_state_free,
            )?
        };
        // SAFETY: `row_iterator_new` and `row_iterator_free` are the matching
        // constructor/destructor pair for `GhosttyRenderStateRowIterator`.
        let row_iterator = unsafe {
            create(
                "row_iterator_new",
                allocator,
                sys::ghostty_render_state_row_iterator_new,
                sys::ghostty_render_state_row_iterator_free,
            )?
        };
        // SAFETY: `row_cells_new` and `row_cells_free` are the matching
        // constructor/destructor pair for `GhosttyRenderStateRowCells`.
        let row_cells = unsafe {
            create(
                "row_cells_new",
                allocator,
                sys::ghostty_render_state_row_cells_new,
                sys::ghostty_render_state_row_cells_free,
            )?
        };
        // SAFETY: `key_encoder_new` and `key_encoder_free` are the matching
        // constructor/destructor pair for `GhosttyKeyEncoder`.
        let key_encoder = unsafe {
            create(
                "key_encoder_new",
                allocator,
                sys::ghostty_key_encoder_new,
                sys::ghostty_key_encoder_free,
            )?
        };
        // SAFETY: `key_event_new` and `key_event_free` are the matching
        // constructor/destructor pair for `GhosttyKeyEvent`.
        let key_event = unsafe {
            create(
                "key_event_new",
                allocator,
                sys::ghostty_key_event_new,
                sys::ghostty_key_event_free,
            )?
        };
        // SAFETY: `mouse_encoder_new` and `mouse_encoder_free` are the matching
        // constructor/destructor pair for `GhosttyMouseEncoder`.
        let mouse_encoder = unsafe {
            create(
                "mouse_encoder_new",
                allocator,
                sys::ghostty_mouse_encoder_new,
                sys::ghostty_mouse_encoder_free,
            )?
        };
        // SAFETY: `mouse_event_new` and `mouse_event_free` are the matching
        // constructor/destructor pair for `GhosttyMouseEvent`.
        let mouse_event = unsafe {
            create(
                "mouse_event_new",
                allocator,
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
            osc: OscRouter::new(appearance),
            _not_send_or_sync: PhantomData,
        })
    }
}

fn configure_appearance(
    terminal: sys::GhosttyTerminal,
    appearance: TerminalAppearance,
) -> Result<()> {
    for (option, color) in [
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND,
            appearance.foreground,
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND,
            appearance.background,
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_CURSOR,
            appearance.cursor,
        ),
    ] {
        let color = sys::GhosttyColorRgb {
            r: color.r,
            g: color.g,
            b: color.b,
        };
        let result = unsafe {
            sys::ghostty_terminal_set(
                terminal,
                option,
                (&color as *const sys::GhosttyColorRgb).cast(),
            )
        };
        check("terminal_set_default_color", result)?;
    }
    Ok(())
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
    use std::alloc::{Layout, alloc, dealloc, realloc};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct AllocationState {
        live: AtomicUsize,
        total: AtomicUsize,
        invalid_callback: AtomicBool,
    }

    fn tracked_layout(len: usize, alignment_exponent: u8) -> Option<Layout> {
        // Pinned Ghostty forwards Zig's `std.mem.Alignment` enum value across
        // this C ABI. That value is log2(alignment), not the byte alignment
        // described by the generated C header.
        let alignment = 1usize.checked_shl(u32::from(alignment_exponent))?;
        Layout::from_size_align(len.max(1), alignment).ok()
    }

    unsafe extern "C" fn tracked_alloc(
        context: *mut c_void,
        len: usize,
        alignment: u8,
        _return_address: usize,
    ) -> *mut c_void {
        if context.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: the test keeps `AllocationState` alive until all handles
        // using this allocator have been dropped.
        let state = unsafe { &*context.cast::<AllocationState>() };
        let Some(layout) = tracked_layout(len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return std::ptr::null_mut();
        };
        // SAFETY: `layout` has a non-zero size and a supported power-of-two
        // alignment. The matching callback reconstructs the same layout.
        let memory = unsafe { alloc(layout) }.cast::<c_void>();
        if !memory.is_null() {
            state.live.fetch_add(1, Ordering::SeqCst);
            state.total.fetch_add(1, Ordering::SeqCst);
        }
        memory
    }

    unsafe extern "C" fn tracked_resize(
        _context: *mut c_void,
        _memory: *mut c_void,
        _memory_len: usize,
        _alignment: u8,
        _new_len: usize,
        _return_address: usize,
    ) -> bool {
        false
    }

    unsafe extern "C" fn tracked_remap(
        context: *mut c_void,
        memory: *mut c_void,
        memory_len: usize,
        alignment: u8,
        new_len: usize,
        _return_address: usize,
    ) -> *mut c_void {
        if context.is_null() || memory.is_null() || new_len == 0 {
            return std::ptr::null_mut();
        }
        // SAFETY: the test keeps `AllocationState` alive until all handles
        // using this allocator have been dropped.
        let state = unsafe { &*context.cast::<AllocationState>() };
        let Some(layout) = tracked_layout(memory_len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return std::ptr::null_mut();
        };
        // SAFETY: the pointer and old layout came from `tracked_alloc` or a
        // prior successful call here. `new_len` is non-zero.
        unsafe { realloc(memory.cast::<u8>(), layout, new_len) }.cast()
    }

    unsafe extern "C" fn tracked_free(
        context: *mut c_void,
        memory: *mut c_void,
        memory_len: usize,
        alignment: u8,
        _return_address: usize,
    ) {
        if context.is_null() || memory.is_null() {
            return;
        }
        // SAFETY: the test keeps the allocator context alive through Drop.
        let state = unsafe { &*context.cast::<AllocationState>() };
        let Some(layout) = tracked_layout(memory_len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return;
        };
        // SAFETY: libghostty provides the same pointer, length, and alignment
        // originally returned by `tracked_alloc`.
        unsafe { dealloc(memory.cast::<u8>(), layout) };
        if state
            .live
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |live| {
                live.checked_sub(1)
            })
            .is_err()
        {
            state.invalid_callback.store(true, Ordering::SeqCst);
        }
    }

    static TRACKED_ALLOCATOR_VTABLE: sys::GhosttyAllocatorVtable = sys::GhosttyAllocatorVtable {
        alloc: Some(tracked_alloc),
        resize: Some(tracked_resize),
        remap: Some(tracked_remap),
        free: Some(tracked_free),
    };

    #[test]
    fn configured_default_colors_answer_osc_queries() {
        let appearance = TerminalAppearance::new(
            crate::Rgb {
                r: 0x11,
                g: 0x22,
                b: 0x33,
            },
            crate::Rgb {
                r: 0x44,
                g: 0x55,
                b: 0x66,
            },
            crate::Rgb {
                r: 0x77,
                g: 0x88,
                b: 0x99,
            },
            crate::ColorScheme::Light,
        );
        let mut terminal =
            DisplayTerminal::new(WindowSize::new(80, 24, 8, 16).unwrap(), 1_000, appearance)
                .expect("terminal must initialize");

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

    #[test]
    fn native_destructors_release_every_custom_allocator_block() {
        let state = AllocationState::default();
        let allocator = sys::GhosttyAllocator {
            ctx: (&state as *const AllocationState).cast_mut().cast(),
            vtable: &TRACKED_ALLOCATOR_VTABLE,
        };
        let size = WindowSize::new(40, 6, 8, 16).unwrap();

        for iteration in 0..32 {
            let allocations_before = state.total.load(Ordering::SeqCst);
            {
                // SAFETY: both `state` and the static vtable outlive the
                // terminal, which is dropped before this scope ends.
                let mut terminal = unsafe {
                    DisplayTerminal::new_with_allocator(
                        size,
                        2_000,
                        TerminalAppearance::default(),
                        &allocator,
                    )
                }
                .expect("terminal must initialize with the tracked allocator");
                terminal
                    .feed(format!("tracked-{iteration:02}-Ω").as_bytes())
                    .expect("tracked terminal must accept input");
                terminal
                    .resize(WindowSize::new(41, 7, 8, 16).unwrap())
                    .expect("tracked terminal must resize");
                let snapshot = terminal.snapshot().expect("tracked snapshot must render");
                assert_eq!((snapshot.cols, snapshot.rows), (41, 7));
            }

            assert_eq!(
                state.live.load(Ordering::SeqCst),
                0,
                "native allocations leaked after lifecycle {iteration}"
            );
            assert!(
                state.total.load(Ordering::SeqCst) > allocations_before,
                "lifecycle {iteration} did not exercise the custom allocator"
            );
            assert!(!state.invalid_callback.load(Ordering::SeqCst));
        }
    }
}
