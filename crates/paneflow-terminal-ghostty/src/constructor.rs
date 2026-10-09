use std::ffi::c_void;
use std::marker::PhantomData;

use paneflow_libghostty_sys as sys;

use crate::callbacks::{self, CallbackState};
use crate::engine::DisplayTerminal;
use crate::handles::{OwnedHandle, check, create};
use crate::limits::MAX_SCROLLBACK_ROWS;
use crate::{BackendEvent, ColorScheme, GhosttyError, Result, TerminalAppearance, WindowSize};

const MAX_APC_BYTES: usize = 1024 * 1024;
const COLOR_SCHEME_UPDATES_MODE: u16 = 2031;

impl DisplayTerminal {
    pub fn new(
        size: WindowSize,
        max_scrollback: usize,
        appearance: TerminalAppearance,
    ) -> Result<Self> {
        unsafe { Self::new_with_allocator(size, max_scrollback, appearance, std::ptr::null()) }
    }

    pub fn set_appearance(&mut self, appearance: TerminalAppearance) -> Result<()> {
        configure_appearance(self.terminal.raw(), appearance)?;
        let previous = self.callbacks.color_scheme();
        self.callbacks.set_color_scheme(appearance.color_scheme);
        self.snapshot_cache.invalidate();
        if previous != appearance.color_scheme && self.mode(COLOR_SCHEME_UPDATES_MODE)? {
            self.callbacks.push(BackendEvent::WritePty(
                color_scheme_report(appearance.color_scheme).to_vec(),
            ));
        }
        Ok(())
    }

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
        let mut callbacks = Box::new(unsafe {
            CallbackState::new(size, appearance.color_scheme, allocator)?
        });
        let mut raw_terminal = std::ptr::null_mut();
        let result =
            unsafe { sys::ghostty_terminal_new(allocator, &mut raw_terminal, size.cols, size.rows) };
        check("terminal_new", result)?;
        if raw_terminal.is_null() {
            return Err(GhosttyError::AbiMismatch(
                "terminal_new returned a null handle".into(),
            ));
        }
        let terminal = unsafe { OwnedHandle::from_raw(raw_terminal, sys::ghostty_terminal_free) };
        callbacks::install(terminal.raw(), (&mut *callbacks) as *mut CallbackState)?;
        configure_scrollback(terminal.raw(), max_scrollback)?;
        configure_safety_limits(terminal.raw())?;
        configure_appearance(terminal.raw(), appearance)?;
        crate::engine::resize_terminal(terminal.raw(), size)?;
        unsafe { Self::assemble(terminal, callbacks, allocator) }
    }

    pub(crate) unsafe fn assemble(
        terminal: OwnedHandle<sys::GhosttyTerminal>,
        callbacks: Box<CallbackState>,
        allocator: *const sys::GhosttyAllocator,
    ) -> Result<Self> {
        let row_iterator = unsafe {
            create(
                "row_iterator_new",
                allocator,
                sys::ghostty_render_state_row_iterator_new,
                sys::ghostty_render_state_row_iterator_free,
            )?
        };
        let row_cells = unsafe {
            create(
                "row_cells_new",
                allocator,
                sys::ghostty_render_state_row_cells_new,
                sys::ghostty_render_state_row_cells_free,
            )?
        };
        let key_encoder = unsafe {
            create(
                "key_encoder_new",
                allocator,
                sys::ghostty_key_encoder_new,
                sys::ghostty_key_encoder_free,
            )?
        };
        let key_event = unsafe {
            create(
                "key_event_new",
                allocator,
                sys::ghostty_key_event_new,
                sys::ghostty_key_event_free,
            )?
        };
        let mouse_encoder = unsafe {
            create(
                "mouse_encoder_new",
                allocator,
                sys::ghostty_mouse_encoder_new,
                sys::ghostty_mouse_encoder_free,
            )?
        };
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
            key_encoder_overrides: crate::input_options::KeyEncoderOverrides::default(),
            mouse_encoder_modes: None,
            mouse_encoder_size: None,
            gesture: None,
            search: None,
            terminal,
            snapshot_cache: Default::default(),
            callbacks,
            history_clear_pending: false,
            _not_send_or_sync: PhantomData,
        })
    }
}

fn color_scheme_report(scheme: ColorScheme) -> &'static [u8] {
    match scheme {
        ColorScheme::Dark => b"\x1b[?997;1n",
        ColorScheme::Light => b"\x1b[?997;2n",
    }
}

pub(crate) fn configure_appearance(
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

pub(crate) fn configure_safety_limits(terminal: sys::GhosttyTerminal) -> Result<()> {
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
            std::ptr::null::<c_void>(),
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

pub(crate) fn configure_scrollback(terminal: sys::GhosttyTerminal, max_scrollback: usize) -> Result<()> {
    let result = unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES,
            (&raw const max_scrollback).cast::<c_void>(),
        )
    };
    check("terminal_set_scrollback_max_lines", result)
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
        live_bytes: AtomicUsize,
        total: AtomicUsize,
        refuse_from: AtomicUsize,
        invalid_callback: AtomicBool,
    }

    fn tracked_layout(len: usize, alignment_exponent: u8) -> Option<Layout> {
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
        let state = unsafe { &*context.cast::<AllocationState>() };
        let refuse_from = state.refuse_from.load(Ordering::SeqCst);
        if refuse_from != 0 && state.total.load(Ordering::SeqCst) >= refuse_from {
            return std::ptr::null_mut();
        }
        let Some(layout) = tracked_layout(len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return std::ptr::null_mut();
        };
        let memory = unsafe { alloc(layout) }.cast::<c_void>();
        if !memory.is_null() {
            state.live.fetch_add(1, Ordering::SeqCst);
            state.live_bytes.fetch_add(len, Ordering::SeqCst);
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
        let state = unsafe { &*context.cast::<AllocationState>() };
        let Some(layout) = tracked_layout(memory_len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return std::ptr::null_mut();
        };
        let remapped = unsafe { realloc(memory.cast::<u8>(), layout, new_len) };
        if !remapped.is_null() {
            state.live_bytes.fetch_add(new_len, Ordering::SeqCst);
            state.live_bytes.fetch_sub(memory_len, Ordering::SeqCst);
        }
        remapped.cast()
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
        let state = unsafe { &*context.cast::<AllocationState>() };
        let Some(layout) = tracked_layout(memory_len, alignment) else {
            state.invalid_callback.store(true, Ordering::SeqCst);
            return;
        };
        unsafe { dealloc(memory.cast::<u8>(), layout) };
        state.live_bytes.fetch_sub(memory_len, Ordering::SeqCst);
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

    fn pty_replies(terminal: &mut DisplayTerminal) -> Vec<u8> {
        terminal
            .drain_events()
            .into_iter()
            .filter_map(|event| match event {
                BackendEvent::WritePty(bytes) => Some(bytes),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn a_scheme_change_is_reported_only_to_programs_that_enabled_mode_2031() {
        let dark = TerminalAppearance::default();
        let light = TerminalAppearance {
            color_scheme: crate::ColorScheme::Light,
            ..dark
        };
        let mut terminal =
            DisplayTerminal::new(WindowSize::new(80, 24, 8, 16).unwrap(), 1_000, dark)
                .expect("terminal must initialize");

        terminal.set_appearance(light).expect("light appearance");
        assert!(pty_replies(&mut terminal).is_empty());

        terminal.feed(b"\x1b[?2031h").expect("enable scheme reports");
        terminal.set_appearance(dark).expect("dark appearance");
        assert_eq!(pty_replies(&mut terminal), b"\x1b[?997;1n");
        terminal.set_appearance(dark).expect("unchanged appearance");
        assert!(pty_replies(&mut terminal).is_empty());
        terminal.set_appearance(light).expect("light appearance");
        assert_eq!(pty_replies(&mut terminal), b"\x1b[?997;2n");

        terminal.feed(b"\x1b[?996n").expect("scheme query");
        assert_eq!(pty_replies(&mut terminal), b"\x1b[?997;2n");
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

    fn tracked_allocator(state: &AllocationState) -> sys::GhosttyAllocator {
        sys::GhosttyAllocator {
            ctx: (state as *const AllocationState).cast_mut().cast(),
            vtable: &TRACKED_ALLOCATOR_VTABLE,
        }
    }

    fn render_state_allocations(state: &AllocationState) -> usize {
        let allocator = tracked_allocator(state);
        let before = state.total.load(Ordering::SeqCst);
        let render_state = unsafe {
            create(
                "render_state_new",
                &allocator,
                sys::ghostty_render_state_new,
                sys::ghostty_render_state_free,
            )
        }
        .expect("a render state must initialize with the tracked allocator");
        drop(render_state);
        state.total.load(Ordering::SeqCst) - before
    }

    #[test]
    fn a_live_during_hold_render_state_that_cannot_be_created_fails_construction() {
        let state = AllocationState::default();
        let per_render_state = render_state_allocations(&state);
        assert!(
            per_render_state > 0,
            "the refusal below only reaches the second render state when creating one allocates"
        );
        let allocator = tracked_allocator(&state);
        state.refuse_from.store(
            state.total.load(Ordering::SeqCst) + per_render_state,
            Ordering::SeqCst,
        );

        let refused = unsafe {
            DisplayTerminal::new_with_allocator(
                WindowSize::new(40, 6, 8, 16).unwrap(),
                2_000,
                TerminalAppearance::default(),
                &allocator,
            )
        };

        assert!(
            matches!(
                refused,
                Err(GhosttyError::Ffi {
                    operation: "render_state_new_live_during_hold",
                    ..
                })
            ),
            "construction must fail with the wrapper error of the second render state"
        );
        assert_eq!(state.live.load(Ordering::SeqCst), 0);
        assert!(!state.invalid_callback.load(Ordering::SeqCst));
    }

    #[test]
    fn the_live_during_hold_render_state_costs_under_a_tenth_of_a_200x60_terminal() {
        let state = AllocationState::default();
        let empty_render_state_bytes = {
            let allocator = tracked_allocator(&state);
            let before = state.live_bytes.load(Ordering::SeqCst);
            let render_state = unsafe {
                create(
                    "render_state_new",
                    &allocator,
                    sys::ghostty_render_state_new,
                    sys::ghostty_render_state_free,
                )
            }
            .expect("a render state must initialize with the tracked allocator");
            let bytes = state.live_bytes.load(Ordering::SeqCst) - before;
            drop(render_state);
            bytes
        };
        let allocator = tracked_allocator(&state);
        let mut terminal = unsafe {
            DisplayTerminal::new_with_allocator(
                WindowSize::new(200, 60, 8, 16).unwrap(),
                10_000,
                TerminalAppearance::default(),
                &allocator,
            )
        }
        .expect("terminal must initialize with the tracked allocator");
        terminal.enable_render_hold().expect("render hold installs");
        for line in 0..120 {
            let row = format!("\x1b[38;5;{}m{}\x1b[0m\r\n", line % 256, "x".repeat(199));
            terminal.feed(row.as_bytes()).expect("fill the screen");
        }
        terminal.snapshot().expect("main render state snapshot");
        let usage = terminal.memory_usage().expect("memory usage");
        let terminal_bytes = state.live_bytes.load(Ordering::SeqCst)
            + usize::try_from(usage.primary_resident_bytes + usage.alternate_resident_bytes)
                .expect("resident bytes fit in usize");

        terminal
            .feed(b"\x1b[?2026h\x1b[Hlive redraw")
            .expect("begin a hold");
        terminal.snapshot().expect("held snapshot");
        let held_bytes = state.live_bytes.load(Ordering::SeqCst);
        terminal.snapshot_live().expect("live snapshot during the hold");
        let populated_bytes = state.live_bytes.load(Ordering::SeqCst) - held_bytes;

        terminal.feed(b"\x1b[?2026l").expect("end the hold");
        terminal.snapshot().expect("snapshot after the hold");
        let released = !terminal.callbacks.holds_live_during_hold_render_state();

        eprintln!(
            "200x60 terminal: {terminal_bytes} bytes; empty live-during-hold render state: {empty_render_state_bytes} bytes ({:.3}%); populated during a hold: {populated_bytes} bytes ({:.1}%), released after the hold: {released}",
            empty_render_state_bytes as f64 * 100.0 / terminal_bytes as f64,
            populated_bytes as f64 * 100.0 / terminal_bytes as f64,
        );
        assert!(
            empty_render_state_bytes * 10 <= terminal_bytes,
            "{empty_render_state_bytes} bytes exceed a tenth of {terminal_bytes}"
        );
        assert!(
            released,
            "a populated live-during-hold render state must not outlive its hold"
        );
    }
}
