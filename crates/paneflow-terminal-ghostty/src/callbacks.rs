use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};

use paneflow_libghostty_sys as sys;

use crate::handles::check;
use crate::{BackendEvent, ColorScheme, Result, WindowSize};

const MAX_PENDING_WRITE_PTY_BYTES: usize = 1024 * 1024;
const MAX_PENDING_CLIPBOARD_EVENTS: usize = 32;
const MAX_PENDING_BELL_EVENTS: usize = 256;
const MAX_PENDING_NOTIFICATION_EVENTS: usize = 16;
const MAX_PENDING_UNKNOWN_SEQUENCE_EVENTS: usize = 32;

const _: sys::GhosttyTerminalWritePtyFn = Some(crate::callback_ffi::write_pty);
const _: sys::GhosttyTerminalBellFn = Some(crate::callback_ffi::bell);
const _: sys::GhosttyTerminalEnquiryFn = Some(crate::callback_ffi::enquiry);
const _: sys::GhosttyTerminalXtversionFn = Some(crate::callback_ffi::xtversion);
const _: sys::GhosttyTerminalTitleChangedFn = Some(crate::callback_ffi::title_changed);
const _: sys::GhosttyTerminalPwdChangedFn = Some(crate::callback_ffi::pwd_changed);
const _: sys::GhosttyTerminalClipboardWriteFn = Some(crate::callback_ffi::clipboard_write);
const _: sys::GhosttyTerminalProgressReportFn = Some(crate::callback_ffi::progress_report);
const _: sys::GhosttyTerminalSizeFn = Some(crate::callback_ffi::size);
const _: sys::GhosttyTerminalColorSchemeFn = Some(crate::callback_ffi::color_scheme);
const _: sys::GhosttyTerminalDeviceAttributesFn = Some(crate::callback_ffi::device_attributes);
const _: sys::GhosttyTerminalDesktopNotificationFn = Some(crate::callback_ffi::desktop_notification);
const _: sys::GhosttyTerminalUnknownSequenceFn = Some(crate::callback_ffi::unknown_sequence);
const _: sys::GhosttyTerminalClipboardReadFn = Some(crate::callback_ffi::clipboard_read);

pub(crate) struct CallbackState {
    events: RefCell<VecDeque<BackendEvent>>,
    pending_write_pty_bytes: Cell<usize>,
    pending_clipboard_events: Cell<usize>,
    pending_bell_events: Cell<usize>,
    pending_notification_events: Cell<usize>,
    pending_unknown_sequence_events: Cell<usize>,
    /// What a clipboard read is answered with. `None`, the default, denies
    /// every read. See [`crate::DisplayTerminal::set_clipboard_readable`].
    readable_clipboard: RefCell<Option<String>>,
    size: Cell<WindowSize>,
    color_scheme: Cell<ColorScheme>,
    last_working_directory: RefCell<Option<String>>,
    #[cfg(test)]
    pub(crate) panic_next: Cell<bool>,
}

impl CallbackState {
    pub(crate) fn new(size: WindowSize, color_scheme: ColorScheme) -> Self {
        Self {
            events: RefCell::new(VecDeque::new()),
            pending_write_pty_bytes: Cell::new(0),
            pending_clipboard_events: Cell::new(0),
            pending_bell_events: Cell::new(0),
            pending_notification_events: Cell::new(0),
            pending_unknown_sequence_events: Cell::new(0),
            readable_clipboard: RefCell::new(None),
            size: Cell::new(size),
            color_scheme: Cell::new(color_scheme),
            last_working_directory: RefCell::new(None),
            #[cfg(test)]
            panic_next: Cell::new(false),
        }
    }

    pub(crate) fn set_readable_clipboard(&self, text: Option<String>) {
        *self.readable_clipboard.borrow_mut() = text;
    }

    /// The text a clipboard read is answered with, if the embedder allowed
    /// one at all.
    pub(crate) fn readable_clipboard(&self) -> Option<String> {
        self.readable_clipboard.borrow().clone()
    }

    pub(crate) fn set_size(&self, size: WindowSize) {
        self.size.set(size);
    }

    pub(crate) fn size(&self) -> WindowSize {
        self.size.get()
    }

    pub(crate) fn color_scheme(&self) -> ColorScheme {
        self.color_scheme.get()
    }

    pub(crate) fn set_color_scheme(&self, color_scheme: ColorScheme) {
        self.color_scheme.set(color_scheme);
    }

    /// Report a working directory, dropping a repeat of the last one.
    ///
    /// Shells emit OSC 7 on every prompt, so most reports restate the
    /// directory Paneflow already knows.
    pub(crate) fn push_working_directory(&self, cwd: String) {
        let mut last = self.last_working_directory.borrow_mut();
        if last.as_deref() == Some(cwd.as_str()) {
            return;
        }
        *last = Some(cwd.clone());
        drop(last);
        self.push(BackendEvent::WorkingDirectory(cwd));
    }

    /// Forget the last reported working directory.
    ///
    /// A terminal reset drops the shell state that produced it, so the next
    /// report must reach Paneflow even when it restates the same path.
    pub(crate) fn reset_working_directory(&self) {
        *self.last_working_directory.borrow_mut() = None;
    }

    pub(crate) fn push(&self, event: BackendEvent) {
        let mut events = self.events.borrow_mut();
        match event {
            BackendEvent::WritePty(bytes) => {
                let pending = self.pending_write_pty_bytes.get();
                let Some(total) = pending.checked_add(bytes.len()) else {
                    push_overflow(&mut events, 1, bytes.len());
                    return;
                };
                if total > MAX_PENDING_WRITE_PTY_BYTES {
                    push_overflow(&mut events, 1, bytes.len());
                    return;
                }
                self.pending_write_pty_bytes.set(total);
                if let Some(BackendEvent::WritePty(pending)) = events.back_mut() {
                    pending.extend_from_slice(&bytes);
                } else {
                    events.push_back(BackendEvent::WritePty(bytes));
                }
            }
            BackendEvent::ClipboardStore(text) => {
                let pending = self.pending_clipboard_events.get();
                if pending >= MAX_PENDING_CLIPBOARD_EVENTS {
                    push_overflow(&mut events, 1, text.len());
                } else {
                    self.pending_clipboard_events.set(pending + 1);
                    events.push_back(BackendEvent::ClipboardStore(text));
                }
            }
            BackendEvent::Title(title) => {
                events.retain(|event| !matches!(event, BackendEvent::Title(_)));
                events.push_back(BackendEvent::Title(title));
            }
            BackendEvent::WorkingDirectory(cwd) => {
                events.retain(|event| !matches!(event, BackendEvent::WorkingDirectory(_)));
                events.push_back(BackendEvent::WorkingDirectory(cwd));
            }
            BackendEvent::Progress(report) => {
                events.retain(|event| !matches!(event, BackendEvent::Progress(_)));
                events.push_back(BackendEvent::Progress(report));
            }
            BackendEvent::Bell => {
                let pending = self.pending_bell_events.get();
                if pending >= MAX_PENDING_BELL_EVENTS {
                    push_overflow(&mut events, 1, 0);
                } else {
                    self.pending_bell_events.set(pending + 1);
                    events.push_back(BackendEvent::Bell);
                }
            }
            BackendEvent::DesktopNotification { title, body } => {
                let pending = self.pending_notification_events.get();
                if pending >= MAX_PENDING_NOTIFICATION_EVENTS {
                    push_overflow(&mut events, 1, title.len() + body.len());
                } else {
                    self.pending_notification_events.set(pending + 1);
                    events.push_back(BackendEvent::DesktopNotification { title, body });
                }
            }
            BackendEvent::UnknownSequence { content, truncated } => {
                let pending = self.pending_unknown_sequence_events.get();
                if pending >= MAX_PENDING_UNKNOWN_SEQUENCE_EVENTS {
                    push_overflow(&mut events, 1, content.len());
                } else {
                    self.pending_unknown_sequence_events.set(pending + 1);
                    events.push_back(BackendEvent::UnknownSequence { content, truncated });
                }
            }
            BackendEvent::CallbackPanicked => {
                if !events
                    .iter()
                    .any(|event| matches!(event, BackendEvent::CallbackPanicked))
                {
                    events.push_back(BackendEvent::CallbackPanicked);
                }
            }
            BackendEvent::InputDropped { bytes } => {
                if let Some(BackendEvent::InputDropped { bytes: pending }) = events
                    .iter_mut()
                    .find(|event| matches!(event, BackendEvent::InputDropped { .. }))
                {
                    *pending = pending.saturating_add(bytes);
                } else {
                    events.push_back(BackendEvent::InputDropped { bytes });
                }
            }
            BackendEvent::EffectsOverflow {
                dropped_events,
                dropped_bytes,
            } => push_overflow(&mut events, dropped_events, dropped_bytes),
        }
    }

    pub(crate) fn drain(&self) -> Vec<BackendEvent> {
        self.pending_write_pty_bytes.set(0);
        self.pending_clipboard_events.set(0);
        self.pending_bell_events.set(0);
        self.pending_notification_events.set(0);
        self.pending_unknown_sequence_events.set(0);
        self.events.borrow_mut().drain(..).collect()
    }
}

fn push_overflow(events: &mut VecDeque<BackendEvent>, dropped_events: usize, dropped_bytes: usize) {
    if let Some(BackendEvent::EffectsOverflow {
        dropped_events: pending_events,
        dropped_bytes: pending_bytes,
    }) = events
        .iter_mut()
        .find(|event| matches!(event, BackendEvent::EffectsOverflow { .. }))
    {
        *pending_events = pending_events.saturating_add(dropped_events);
        *pending_bytes = pending_bytes.saturating_add(dropped_bytes);
    } else {
        events.push_back(BackendEvent::EffectsOverflow {
            dropped_events,
            dropped_bytes,
        });
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
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
        crate::callback_ffi::pwd_changed as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
        crate::callback_ffi::clipboard_write as *const (),
    )?;
    // Bound the Kitty clipboard protocol to the same budget the OSC 52 path
    // has always had; libghostty otherwise buffers up to 64 MiB per write.
    let clipboard_max_bytes = crate::callback_ffi::MAX_CLIPBOARD_BYTES;
    set(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE_MAX_BYTES,
        (&raw const clipboard_max_bytes).cast(),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_READ,
        crate::callback_ffi::clipboard_read as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
        crate::callback_ffi::desktop_notification as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE,
        crate::callback_ffi::unknown_sequence as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
        crate::callback_ffi::progress_report as *const (),
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
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), ColorScheme::Dark);
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
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), ColorScheme::Dark);
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

    #[test]
    fn protocol_replies_are_coalesced_without_an_event_count_limit() {
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), ColorScheme::Dark);
        for _ in 0..1_000 {
            state.push(BackendEvent::WritePty(vec![b'x']));
        }

        assert_eq!(state.drain(), [BackendEvent::WritePty(vec![b'x'; 1_000])]);
    }

    #[test]
    fn protocol_overflow_is_explicit() {
        let state = CallbackState::new(WindowSize::new(80, 24, 8, 16).unwrap(), ColorScheme::Dark);
        state.push(BackendEvent::WritePty(vec![0; MAX_PENDING_WRITE_PTY_BYTES]));
        state.push(BackendEvent::WritePty(vec![0; 1]));

        assert!(matches!(
            state.drain().last(),
            Some(BackendEvent::EffectsOverflow {
                dropped_events: 1,
                dropped_bytes: 1,
            })
        ));
    }
}
