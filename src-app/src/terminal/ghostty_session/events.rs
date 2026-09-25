use super::*;

#[derive(Debug)]
pub(crate) enum GhosttyUiEvent {
    Wakeup(Arc<UiEventState>),
    Title(Arc<UiEventState>),
    WorkingDirectory(Arc<UiEventState>),
    Progress(Arc<UiEventState>),
    Notification(Arc<UiEventState>),
    Clipboard(Arc<UiEventState>),
    ServiceOutputReady(Arc<UiEventState>),
    ChildExited {
        code: i32,
        signal: Option<String>,
    },
    HyperlinkResolved {
        point: Point,
        link: Option<HyperlinkZone>,
    },
    InputRejected(String),
    RuntimeFailed(String),
    HostLink(HostLinkState),
}

impl GhosttyUiEvent {
    pub(in crate::terminal) fn is_wakeup(&self) -> bool {
        if let Self::Wakeup(events) = self {
            events.wakeup_queued.store(false, Ordering::Release);
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
struct CoalescedSlot<T> {
    latest: Option<T>,
    queued: bool,
}

impl<T> Default for CoalescedSlot<T> {
    fn default() -> Self {
        Self {
            latest: None,
            queued: false,
        }
    }
}

#[derive(Debug, Default)]
struct ClipboardSlot {
    pending: VecDeque<String>,
    queued: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProgramNotification {
    pub(crate) title: String,
    pub(crate) body: String,
}

#[derive(Debug, Default)]
struct NotificationSlot {
    pending: VecDeque<ProgramNotification>,
    queued: bool,
}

#[derive(Debug, Default)]
pub(crate) struct UiEventState {
    wakeup_queued: AtomicBool,
    service_output_queued: AtomicBool,
    title: Mutex<CoalescedSlot<String>>,
    working_directory: Mutex<CoalescedSlot<String>>,
    progress: Mutex<CoalescedSlot<ghostty::ProgressReport>>,
    notifications: Mutex<NotificationSlot>,
    clipboard: Mutex<ClipboardSlot>,
}

impl UiEventState {
    fn store<T>(slot: &Mutex<CoalescedSlot<T>>, value: T) -> bool {
        let mut slot = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.latest = Some(value);
        if slot.queued {
            false
        } else {
            slot.queued = true;
            true
        }
    }

    fn take<T>(slot: &Mutex<CoalescedSlot<T>>) -> Option<T> {
        let mut slot = slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.queued = false;
        slot.latest.take()
    }

    pub(in crate::terminal) fn take_title(&self) -> Option<String> {
        Self::take(&self.title)
    }

    pub(in crate::terminal) fn take_working_directory(&self) -> Option<String> {
        Self::take(&self.working_directory)
    }

    pub(in crate::terminal) fn take_progress(&self) -> Option<ghostty::ProgressReport> {
        Self::take(&self.progress)
    }

    pub(in crate::terminal) fn take_notifications(&self) -> Vec<ProgramNotification> {
        let mut slot = self
            .notifications
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.queued = false;
        slot.pending.drain(..).collect()
    }

    pub(in crate::terminal) fn take_clipboard(&self) -> Vec<String> {
        let mut slot = self
            .clipboard
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.queued = false;
        slot.pending.drain(..).collect()
    }

    pub(in crate::terminal) fn acknowledge_wakeup(&self) {
        self.wakeup_queued.store(false, Ordering::Release);
    }

    pub(in crate::terminal) fn acknowledge_service_output(&self) {
        self.service_output_queued.store(false, Ordering::Release);
    }
}

pub(super) fn queue_wakeup(inner: &SessionInner) {
    if !inner.ui_events.wakeup_queued.swap(true, Ordering::AcqRel) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Wakeup(inner.ui_events.clone()));
    }
}

pub(super) fn queue_service_output_ready(inner: &SessionInner) {
    if !inner
        .ui_events
        .service_output_queued
        .swap(true, Ordering::AcqRel)
    {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::ServiceOutputReady(inner.ui_events.clone()));
    }
}

fn queue_title(inner: &SessionInner, title: String) {
    if UiEventState::store(&inner.ui_events.title, title) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Title(inner.ui_events.clone()));
    }
}

fn queue_working_directory(inner: &SessionInner, cwd: String) {
    if UiEventState::store(&inner.ui_events.working_directory, cwd) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::WorkingDirectory(inner.ui_events.clone()));
    }
}

fn queue_progress(inner: &SessionInner, report: ghostty::ProgressReport) {
    if UiEventState::store(&inner.ui_events.progress, report) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Progress(inner.ui_events.clone()));
    }
}

fn sanitized_notification(title: String, body: String) -> ProgramNotification {
    ProgramNotification {
        title: crate::agents::notifications::sanitize_notification_message(&title),
        body: crate::agents::notifications::sanitize_notification_message(&body),
    }
}

fn queue_notification(inner: &SessionInner, notification: ProgramNotification) {
    let mut slot = inner
        .ui_events
        .notifications
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.pending.len() == MAX_NOTIFICATION_EVENTS {
        slot.pending.pop_front();
    }
    slot.pending.push_back(notification);
    if slot.queued {
        return;
    }
    slot.queued = true;
    drop(slot);
    let _ = inner
        .events_tx
        .unbounded_send(GhosttyUiEvent::Notification(inner.ui_events.clone()));
}

fn queue_clipboard(inner: &SessionInner, text: String) {
    if !inner.clipboard_gate.allows_store() {
        return;
    }
    let mut slot = inner
        .ui_events
        .clipboard
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.pending.len() == MAX_CLIPBOARD_EVENTS {
        slot.pending.pop_front();
    }
    slot.pending.push_back(text);
    if !slot.queued {
        slot.queued = true;
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::Clipboard(inner.ui_events.clone()));
    }
}

pub(super) fn handle_engine_events(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    writer: &mut Option<Box<dyn Write + Send>>,
) -> Result<(), String> {
    handle_engine_events_to(inner, terminal, &mut |bytes| match writer.as_mut() {
        Some(active_writer) => active_writer
            .write_all(bytes)
            .and_then(|()| active_writer.flush())
            .map_err(|error| format!("Ghostty protocol reply failed: {error}")),
        None => Ok(()),
    })
}

pub(super) fn handle_engine_events_to(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    reply_sink: &mut dyn FnMut(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    for event in terminal.drain_events() {
        match event {
            ghostty::BackendEvent::WritePty(bytes) => reply_sink(&bytes)?,
            ghostty::BackendEvent::ClipboardStore(text) => queue_clipboard(inner, text),
            ghostty::BackendEvent::Title(title) => queue_title(inner, title),
            ghostty::BackendEvent::WorkingDirectory(cwd) => {
                queue_working_directory(inner, cwd);
            }
            ghostty::BackendEvent::Progress(report) => queue_progress(inner, report),
            ghostty::BackendEvent::DesktopNotification { title, body } => {
                queue_notification(inner, sanitized_notification(title, body));
            }
            ghostty::BackendEvent::UnknownSequence { content, truncated } => {
                log::debug!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty ignored an unsupported sequence{}: {content}",
                    if truncated { " (truncated)" } else { "" }
                );
            }
            ghostty::BackendEvent::Bell => {}
            ghostty::BackendEvent::CallbackPanicked => {
                return Err("Ghostty callback panicked at the FFI boundary".into());
            }
            ghostty::BackendEvent::InputDropped { bytes } => {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty dropped oversized callback input ({bytes} bytes)"
                );
            }
            ghostty::BackendEvent::EffectsOverflow {
                dropped_events,
                dropped_bytes,
            } => {
                return Err(format!(
                    "Ghostty callback effects overflowed ({dropped_events} events, {dropped_bytes} bytes)"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clipboard_store_is_filtered_at_the_ghostty_source() {
        let gate = Arc::new(ClipboardGate::default());
        let (session, _pending, mut events_rx) = GhosttySession::pending_with_clipboard_gate(
            TerminalWindowSize::new(80, 24, 8, 16),
            gate.clone(),
        );

        queue_clipboard(&session.inner, "unfocused".into());
        assert!(events_rx.try_recv().is_err());

        gate.set_policy(true);
        gate.set_focused(true);
        queue_clipboard(&session.inner, "focused".into());
        let event_state = match events_rx.try_recv() {
            Ok(GhosttyUiEvent::Clipboard(state)) => state,
            other => panic!("expected a focused clipboard event, got {other:?}"),
        };
        assert_eq!(event_state.take_clipboard(), ["focused"]);

        gate.set_focused(false);
        queue_clipboard(&session.inner, "lost-focus".into());
        assert!(events_rx.try_recv().is_err());
    }
}
