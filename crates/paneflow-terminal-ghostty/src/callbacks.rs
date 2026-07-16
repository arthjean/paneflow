use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{BackendEvent, Result, WindowSize};

const MAX_EVENTS: usize = 256;

pub(crate) struct CallbackState {
    events: RefCell<VecDeque<BackendEvent>>,
    size: Cell<WindowSize>,
    dark: Cell<bool>,
    #[cfg(test)]
    pub(crate) panic_next: Cell<bool>,
}

impl CallbackState {
    pub(crate) fn new(size: WindowSize) -> Self {
        Self {
            events: RefCell::new(VecDeque::new()),
            size: Cell::new(size),
            dark: Cell::new(true),
            #[cfg(test)]
            panic_next: Cell::new(false),
        }
    }

    pub(crate) fn set_size(&self, size: WindowSize) {
        self.size.set(size);
    }

    pub(crate) fn size(&self) -> WindowSize {
        self.size.get()
    }

    pub(crate) fn is_dark(&self) -> bool {
        self.dark.get()
    }

    pub(crate) fn push(&self, event: BackendEvent) {
        let mut events = self.events.borrow_mut();
        if events.len() == MAX_EVENTS {
            events.pop_front();
        }
        events.push_back(event);
    }

    pub(crate) fn drain(&self) -> Vec<BackendEvent> {
        self.events.borrow_mut().drain(..).collect()
    }
}

pub(crate) fn install(terminal: sys::GhosttyTerminal, state: *mut CallbackState) -> Result<()> {
    set(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_USERDATA,
        state.cast(),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_WRITE_PTY,
        crate::callback_ffi::write_pty as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
        crate::callback_ffi::bell as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_ENQUIRY,
        crate::callback_ffi::enquiry as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_XTVERSION,
        crate::callback_ffi::xtversion as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
        crate::callback_ffi::title_changed as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SIZE,
        crate::callback_ffi::size as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_COLOR_SCHEME,
        crate::callback_ffi::color_scheme as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DEVICE_ATTRIBUTES,
        crate::callback_ffi::device_attributes as *const (),
    )?;
    Ok(())
}

fn set(
    terminal: sys::GhosttyTerminal,
    option: sys::GhosttyTerminalOption,
    value: *const c_void,
) -> Result<()> {
    let result = unsafe { sys::ghostty_terminal_set(terminal, option, value) };
    check("terminal_set", result)
}

fn set_callback(
    terminal: sys::GhosttyTerminal,
    option: sys::GhosttyTerminalOption,
    callback: *const (),
) -> Result<()> {
    set(terminal, option, callback.cast())
}

/// Run a libghostty callback against Paneflow's registered callback state.
///
/// # Safety
///
/// If `userdata` is non-null, it must be the properly aligned pointer to the
/// live `CallbackState` registered on the calling terminal. That allocation
/// must remain alive and must not be mutably accessed for the duration of `f`.
pub(crate) unsafe fn with_state(userdata: *mut c_void, f: impl FnOnce(&CallbackState)) {
    if userdata.is_null() {
        return;
    }
    let state = unsafe { &*userdata.cast::<CallbackState>() };
    let result = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        if state.panic_next.replace(false) {
            std::panic::resume_unwind(Box::new("forced callback panic"));
        }
        f(state);
    }));
    if result.is_err() {
        state.push(BackendEvent::CallbackPanicked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_panic_is_contained_and_reported() {
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap());
        state.panic_next.set(true);
        unsafe {
            crate::callback_ffi::bell(
                std::ptr::null_mut(),
                (&state as *const CallbackState).cast_mut().cast(),
            )
        };
        assert_eq!(state.drain(), [BackendEvent::CallbackPanicked]);
    }

    #[test]
    fn callback_p99_stays_below_one_millisecond() {
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap());
        let data = b"response";
        let mut samples = Vec::with_capacity(2_000);
        for _ in 0..2_000 {
            let start = std::time::Instant::now();
            unsafe {
                crate::callback_ffi::write_pty(
                    std::ptr::null_mut(),
                    (&state as *const CallbackState).cast_mut().cast(),
                    data.as_ptr(),
                    data.len(),
                )
            };
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        assert!(samples[samples.len() * 99 / 100] < std::time::Duration::from_millis(1));
    }
}
