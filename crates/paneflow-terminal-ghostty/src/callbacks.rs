use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use paneflow_libghostty_sys as sys;

use crate::handles::{OwnedHandle, check, create};
use crate::{BackendEvent, ColorScheme, Overscan, RenderHold, Result, WindowSize};

const MAX_PENDING_WRITE_PTY_BYTES: usize = 1024 * 1024;
const MAX_PENDING_CLIPBOARD_EVENTS: usize = 32;
const BELL_INTERVAL: Duration = Duration::from_millis(250);
const MAX_PENDING_NOTIFICATION_EVENTS: usize = 16;
const MAX_PENDING_UNKNOWN_SEQUENCE_EVENTS: usize = 32;
const MAX_PENDING_PROGRAM_STATUS_BYTES: usize = 256 * 1024;
const PROGRAM_STATUS_EVENT_OVERHEAD_BYTES: usize = std::mem::size_of::<BackendEvent>();

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
const _: sys::GhosttyTerminalProgramStatusFn = Some(crate::callback_ffi::program_status);
const _: sys::GhosttyTerminalSemanticPromptFn = Some(crate::callback_ffi::semantic_prompt);
const _: sys::GhosttyTerminalResetFn = Some(crate::callback_ffi::reset);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RenderSlot {
    #[default]
    Main,
    LiveDuringHold,
}

unsafe fn new_render_state(
    operation: &'static str,
    allocator: *const sys::GhosttyAllocator,
    overscan: Overscan,
) -> Result<OwnedHandle<sys::GhosttyRenderState>> {
    let handle = unsafe {
        create(
            operation,
            allocator,
            sys::ghostty_render_state_new,
            sys::ghostty_render_state_free,
        )?
    };
    if overscan != Overscan::default() {
        crate::snapshot_ffi::set_render_overscan(handle.raw(), overscan)?;
    }
    Ok(handle)
}

type RenderHoldFn = unsafe extern "C" fn(sys::GhosttyTerminal, *mut c_void, bool);

const _: fn(sys::GhosttyTerminalRenderHoldFn) -> Option<RenderHoldFn> = std::convert::identity;
const _: RenderHoldFn = crate::callback_ffi::render_hold;

pub(crate) struct CallbackState {
    events: RefCell<VecDeque<BackendEvent>>,
    pending_write_pty_bytes: Cell<usize>,
    pending_clipboard_events: Cell<usize>,
    last_bell_at: Cell<Option<Instant>>,
    pending_notification_events: Cell<usize>,
    pending_unknown_sequence_events: Cell<usize>,
    pending_program_status_bytes: Cell<usize>,
    size: Cell<WindowSize>,
    color_scheme: Cell<ColorScheme>,
    last_working_directory: RefCell<Option<String>>,
    allocator: *const sys::GhosttyAllocator,
    overscan_request: Cell<Overscan>,
    main_render_state: RefCell<OwnedHandle<sys::GhosttyRenderState>>,
    main_missed_terminal_dirt: Cell<bool>,
    live_during_hold_render_state: RefCell<Option<OwnedHandle<sys::GhosttyRenderState>>>,
    rendered_scrollbar: Cell<Option<sys::GhosttyTerminalScrollbar>>,
    render_hold: Cell<Option<RenderHold>>,
    render_holds_started: Cell<u64>,
    #[cfg(test)]
    pub(crate) panic_next: Cell<bool>,
}

impl CallbackState {
    pub(crate) unsafe fn new(
        size: WindowSize,
        color_scheme: ColorScheme,
        allocator: *const sys::GhosttyAllocator,
    ) -> Result<Self> {
        let main_render_state =
            unsafe { new_render_state("render_state_new", allocator, Overscan::default())? };
        let live_during_hold_render_state = unsafe {
            new_render_state(
                "render_state_new_live_during_hold",
                allocator,
                Overscan::default(),
            )?
        };
        Ok(Self {
            events: RefCell::new(VecDeque::new()),
            pending_write_pty_bytes: Cell::new(0),
            pending_clipboard_events: Cell::new(0),
            last_bell_at: Cell::new(None),
            pending_notification_events: Cell::new(0),
            pending_unknown_sequence_events: Cell::new(0),
            pending_program_status_bytes: Cell::new(0),
            size: Cell::new(size),
            color_scheme: Cell::new(color_scheme),
            last_working_directory: RefCell::new(None),
            allocator,
            overscan_request: Cell::new(Overscan::default()),
            main_render_state: RefCell::new(main_render_state),
            main_missed_terminal_dirt: Cell::new(false),
            live_during_hold_render_state: RefCell::new(Some(live_during_hold_render_state)),
            rendered_scrollbar: Cell::new(None),
            render_hold: Cell::new(None),
            render_holds_started: Cell::new(0),
            #[cfg(test)]
            panic_next: Cell::new(false),
        })
    }

    pub(crate) fn render_state(&self) -> sys::GhosttyRenderState {
        self.main_render_state.borrow().raw()
    }

    pub(crate) fn set_overscan_request(&self, overscan: Overscan) -> Result<()> {
        crate::snapshot_ffi::set_render_overscan(self.render_state(), overscan)?;
        if let Some(live) = self.live_during_hold_render_state.borrow().as_ref() {
            crate::snapshot_ffi::set_render_overscan(live.raw(), overscan)?;
        }
        self.overscan_request.set(overscan);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn holds_live_during_hold_render_state(&self) -> bool {
        self.live_during_hold_render_state.borrow().is_some()
    }

    pub(crate) fn claim_terminal_dirt(&self, slot: RenderSlot) -> Result<sys::GhosttyRenderState> {
        match slot {
            RenderSlot::Main => {
                if self.main_missed_terminal_dirt.get() {
                    *self.main_render_state.borrow_mut() = unsafe {
                        new_render_state(
                            "render_state_new_after_missed_dirt",
                            self.allocator,
                            self.overscan_request.get(),
                        )?
                    };
                    self.main_missed_terminal_dirt.set(false);
                    self.live_during_hold_render_state.borrow_mut().take();
                }
                Ok(self.render_state())
            }
            RenderSlot::LiveDuringHold => {
                let mut live = self.live_during_hold_render_state.borrow_mut();
                let raw = match live.as_ref() {
                    Some(handle) => handle.raw(),
                    None => {
                        let handle = unsafe {
                            new_render_state(
                                "render_state_new_live_during_hold",
                                self.allocator,
                                self.overscan_request.get(),
                            )?
                        };
                        let raw = handle.raw();
                        *live = Some(handle);
                        raw
                    }
                };
                self.main_missed_terminal_dirt.set(true);
                Ok(raw)
            }
        }
    }

    pub(crate) fn render_hold(&self) -> Option<RenderHold> {
        self.render_hold.get()
    }

    pub(crate) fn rendered_scrollbar(&self) -> Option<sys::GhosttyTerminalScrollbar> {
        self.rendered_scrollbar.get()
    }

    pub(crate) fn record_rendered_scrollbar(&self, scrollbar: sys::GhosttyTerminalScrollbar) {
        self.rendered_scrollbar.set(Some(scrollbar));
    }

    pub(crate) fn begin_render_hold(&self, terminal: sys::GhosttyTerminal) {
        let Ok(render_state) = self.claim_terminal_dirt(RenderSlot::Main) else {
            self.render_hold.set(None);
            return;
        };
        let captured = unsafe { sys::ghostty_render_state_update(render_state, terminal) };
        if captured != sys::GhosttyResult_GHOSTTY_SUCCESS {
            self.render_hold.set(None);
            return;
        }
        self.rendered_scrollbar
            .set(crate::snapshot_ffi::terminal_scrollbar(terminal).ok());
        let generation = self.render_holds_started.get().wrapping_add(1);
        self.render_holds_started.set(generation);
        self.render_hold.set(Some(RenderHold {
            started_at: Instant::now(),
            generation,
        }));
    }

    pub(crate) fn end_render_hold(&self) {
        self.render_hold.set(None);
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

    pub(crate) fn push_working_directory(&self, cwd: String) {
        let mut last = self.last_working_directory.borrow_mut();
        if last.as_deref() == Some(cwd.as_str()) {
            return;
        }
        *last = Some(cwd.clone());
        drop(last);
        self.push(BackendEvent::WorkingDirectory(cwd));
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
                let now = Instant::now();
                if self
                    .last_bell_at
                    .get()
                    .is_some_and(|last| now.duration_since(last) < BELL_INTERVAL)
                {
                    return;
                }
                self.last_bell_at.set(Some(now));
                events.push_back(BackendEvent::Bell);
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
            BackendEvent::UnknownSequence {
                kind,
                content,
                truncated,
            } => {
                let pending = self.pending_unknown_sequence_events.get();
                if pending >= MAX_PENDING_UNKNOWN_SEQUENCE_EVENTS {
                    push_overflow(&mut events, 1, content.len());
                } else {
                    self.pending_unknown_sequence_events.set(pending + 1);
                    events.push_back(BackendEvent::UnknownSequence {
                        kind,
                        content,
                        truncated,
                    });
                }
            }
            BackendEvent::ProgramStatus(report) => {
                let payload = report_bytes(&report);
                self.admit_program_status(&mut events, BackendEvent::ProgramStatus(report), payload);
            }
            event @ BackendEvent::SemanticPrompt { .. } => {
                self.admit_program_status(&mut events, event, 0);
            }
            BackendEvent::Reset => {
                events.retain(|event| !is_superseded_by_reset(event));
                self.pending_program_status_bytes.set(0);
                events.push_back(BackendEvent::Reset);
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
            BackendEvent::ProgramStatusOverflow {
                dropped_events,
                dropped_bytes,
            } => push_program_status_overflow(&mut events, dropped_events, dropped_bytes),
        }
    }

    fn admit_program_status(
        &self,
        events: &mut VecDeque<BackendEvent>,
        event: BackendEvent,
        payload_bytes: usize,
    ) {
        if events.back() == Some(&event) {
            return;
        }
        let cost = payload_bytes.saturating_add(PROGRAM_STATUS_EVENT_OVERHEAD_BYTES);
        let total = self.pending_program_status_bytes.get().saturating_add(cost);
        if total > MAX_PENDING_PROGRAM_STATUS_BYTES {
            self.pending_program_status_bytes.set(usize::MAX);
            push_program_status_overflow(events, 1, payload_bytes);
            return;
        }
        self.pending_program_status_bytes.set(total);
        events.push_back(event);
    }

    pub(crate) fn drain(&self) -> Vec<BackendEvent> {
        self.pending_write_pty_bytes.set(0);
        self.pending_clipboard_events.set(0);
        self.pending_notification_events.set(0);
        self.pending_unknown_sequence_events.set(0);
        self.pending_program_status_bytes.set(0);
        self.events.borrow_mut().drain(..).collect()
    }
}

fn is_superseded_by_reset(event: &BackendEvent) -> bool {
    matches!(
        event,
        BackendEvent::ProgramStatus(_) | BackendEvent::SemanticPrompt { .. } | BackendEvent::Reset
    )
}

fn report_bytes(report: &crate::ProgramStatusReport) -> usize {
    report.id.len() + report.app.len() + report.title.len() + report.message.len()
}

fn push_program_status_overflow(
    events: &mut VecDeque<BackendEvent>,
    dropped_events: usize,
    dropped_bytes: usize,
) {
    if let Some(BackendEvent::ProgramStatusOverflow {
        dropped_events: pending_events,
        dropped_bytes: pending_bytes,
    }) = events
        .iter_mut()
        .find(|event| matches!(event, BackendEvent::ProgramStatusOverflow { .. }))
    {
        *pending_events = pending_events.saturating_add(dropped_events);
        *pending_bytes = pending_bytes.saturating_add(dropped_bytes);
    } else {
        events.push_back(BackendEvent::ProgramStatusOverflow {
            dropped_events,
            dropped_bytes,
        });
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
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RESET,
        crate::callback_ffi::reset as *const (),
    )?;
    Ok(())
}

pub(crate) fn install_program_status(terminal: sys::GhosttyTerminal) -> Result<()> {
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRAM_STATUS,
        crate::callback_ffi::program_status as *const (),
    )?;
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT,
        crate::callback_ffi::semantic_prompt as *const (),
    )
}

pub(crate) fn install_render_hold(terminal: sys::GhosttyTerminal) -> Result<()> {
    set_callback(
        terminal,
        sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RENDER_HOLD,
        crate::callback_ffi::render_hold as *const (),
    )
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

    fn state() -> CallbackState {
        unsafe {
            CallbackState::new(
                WindowSize::new(80, 24, 8, 16).unwrap(),
                ColorScheme::Dark,
                std::ptr::null(),
            )
        }
        .unwrap()
    }

    #[test]
    fn callback_panic_is_contained_and_reported() {
        let state = state();
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
        let state = state();
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
        let state = state();
        for _ in 0..1_000 {
            state.push(BackendEvent::WritePty(vec![b'x']));
        }

        assert_eq!(state.drain(), [BackendEvent::WritePty(vec![b'x'; 1_000])]);
    }

    #[test]
    fn a_bell_flood_yields_one_bell_per_interval_and_never_overflows() {
        let state = state();
        for _ in 0..10_000 {
            state.push(BackendEvent::Bell);
        }
        assert_eq!(state.drain(), [BackendEvent::Bell]);

        for _ in 0..10_000 {
            state.push(BackendEvent::Bell);
        }
        assert!(state.drain().is_empty());

        std::thread::sleep(BELL_INTERVAL);
        state.push(BackendEvent::Bell);
        assert_eq!(state.drain(), [BackendEvent::Bell]);
    }

    fn minimal_report(id: usize) -> BackendEvent {
        BackendEvent::ProgramStatus(crate::ProgramStatusReport {
            state: crate::ProgramStatusState::Done,
            kind: None,
            progress: None,
            id: id.to_string(),
            app: String::new(),
            title: String::new(),
            message: String::new(),
        })
    }

    #[test]
    fn a_status_overflow_is_told_apart_from_other_effect_overflows() {
        let state = state();
        for _ in 0..=MAX_PENDING_CLIPBOARD_EVENTS {
            state.push(BackendEvent::ClipboardStore("x".into()));
        }
        let events = state.drain();
        assert!(events.contains(&BackendEvent::EffectsOverflow {
            dropped_events: 1,
            dropped_bytes: 1,
        }));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, BackendEvent::ProgramStatusOverflow { .. }))
        );

        let admitted = MAX_PENDING_PROGRAM_STATUS_BYTES / (PROGRAM_STATUS_EVENT_OVERHEAD_BYTES + 4);
        for id in 0..admitted + 3 {
            state.push(minimal_report(1_000 + id));
        }
        let events = state.drain();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, BackendEvent::EffectsOverflow { .. }))
        );
        assert_eq!(
            events.last(),
            Some(&BackendEvent::ProgramStatusOverflow {
                dropped_events: 3,
                dropped_bytes: 12,
            })
        );
        assert_eq!(events.len(), admitted + 1);
    }

    #[test]
    fn a_repeated_status_event_is_queued_once() {
        let state = state();
        let prompt = BackendEvent::SemanticPrompt {
            kind: crate::SemanticPromptKind::PromptStart,
            prompt_kind: crate::PromptKind::Primary,
            exit_code: None,
        };
        for _ in 0..100_000 {
            state.push(prompt.clone());
        }
        state.push(minimal_report(7));
        state.push(prompt.clone());

        assert_eq!(state.drain(), [prompt.clone(), minimal_report(7), prompt]);
    }

    #[test]
    fn protocol_overflow_is_explicit() {
        let state = state();
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
