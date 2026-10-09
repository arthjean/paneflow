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
    HostNotice(String),
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
    accepted: VecDeque<(Instant, ProgramNotification)>,
}

impl NotificationSlot {
    fn admit(&mut self, notification: &ProgramNotification, now: Instant) -> bool {
        while self
            .accepted
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= NOTIFICATION_WINDOW)
        {
            self.accepted.pop_front();
        }
        if self.accepted.len() >= MAX_NOTIFICATIONS_PER_WINDOW
            || self
                .accepted
                .iter()
                .any(|(_, accepted)| accepted == notification)
        {
            return false;
        }
        self.accepted.push_back((now, notification.clone()));
        true
    }
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
    effects_overflow_warned_at: Mutex<Option<Instant>>,
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
    if !slot.admit(&notification, Instant::now()) {
        return;
    }
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

fn warn_effects_overflow(inner: &SessionInner, dropped_events: usize, dropped_bytes: usize) {
    let now = Instant::now();
    let mut warned_at = inner
        .ui_events
        .effects_overflow_warned_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if warned_at.is_some_and(|at| now.duration_since(at) < EFFECTS_OVERFLOW_WARN_INTERVAL) {
        return;
    }
    *warned_at = Some(now);
    log::warn!(
        target: "paneflow::terminal::ghostty",
        "Ghostty dropped terminal effects past their per-batch cap ({dropped_events} events, {dropped_bytes} bytes)"
    );
}

fn unknown_sequence_label(kind: ghostty::UnknownSequenceKind) -> &'static str {
    match kind {
        ghostty::UnknownSequenceKind::Apc => "APC sequence",
        ghostty::UnknownSequenceKind::Osc(ghostty::OscTerminator::St) => "OSC sequence ended by ST",
        ghostty::UnknownSequenceKind::Osc(ghostty::OscTerminator::Bel) => {
            "OSC sequence ended by BEL"
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct EngineDrain {
    pub(super) program_reset: bool,
}

pub(super) fn handle_engine_events(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    writer: &mut Option<Box<dyn Write + Send>>,
) -> Result<EngineDrain, String> {
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
) -> Result<EngineDrain, String> {
    let mut drain = EngineDrain::default();
    for event in terminal.drain_events() {
        match event {
            ghostty::BackendEvent::Reset => {
                drain.program_reset = true;
                queue_title(inner, String::new());
                queue_progress(
                    inner,
                    ghostty::ProgressReport {
                        state: ghostty::ProgressState::Remove,
                        percent: None,
                    },
                );
            }
            ghostty::BackendEvent::ProgramStatus(_)
            | ghostty::BackendEvent::SemanticPrompt { .. }
            | ghostty::BackendEvent::ProgramStatusOverflow { .. } => {}
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
            ghostty::BackendEvent::UnknownSequence {
                kind,
                content,
                truncated,
            } => {
                log::debug!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty ignored an unsupported {}{}: {content}",
                    unknown_sequence_label(kind),
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
            } => warn_effects_overflow(inner, dropped_events, dropped_bytes),
        }
    }
    Ok(drain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects_terminal() -> ghostty::DisplayTerminal {
        ghostty::DisplayTerminal::new(
            ghostty::WindowSize::new(80, 24, 8, 16).expect("window size"),
            1_000,
            ghostty::TerminalAppearance::default(),
        )
        .expect("terminal")
    }

    #[test]
    fn a_burst_of_terminal_effects_in_one_chunk_never_fails_the_runtime() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut terminal = effects_terminal();
        let mut burst = b"\x07".repeat(10_000);
        burst.extend(b"\x1b]9;build done\x07".repeat(20));
        burst.extend(b"\x1b]52;c;aGk=\x07".repeat(40));
        burst.extend(b"\x1b_Zpayload\x1b\\".repeat(40));
        terminal.feed(&burst).expect("feed the burst");

        let drained = handle_engine_events_to(&session.inner, &mut terminal, &mut |_| Ok(()));

        assert_eq!(drained, Ok(EngineDrain::default()));
        let notifications = session.inner.ui_events.take_notifications();
        assert_eq!(
            notifications.len(),
            1,
            "identical program notifications are merged: {notifications:?}"
        );
    }

    #[test]
    fn an_unknown_osc_is_logged_with_its_kind_and_never_fails_the_runtime() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut terminal = effects_terminal();
        terminal
            .capture_unknown_sequences(true)
            .expect("capture must enable");
        terminal
            .feed(b"\x1b]7400;status=busy\x07")
            .expect("unknown OSC parses");

        let drained = handle_engine_events_to(&session.inner, &mut terminal, &mut |_| Ok(()));

        assert_eq!(drained, Ok(EngineDrain::default()));
        assert_eq!(
            unknown_sequence_label(ghostty::UnknownSequenceKind::Osc(
                ghostty::OscTerminator::Bel
            )),
            "OSC sequence ended by BEL"
        );
        assert_eq!(
            unknown_sequence_label(ghostty::UnknownSequenceKind::Apc),
            "APC sequence"
        );
    }

    fn assert_reset_reaches_the_pane(
        session: &GhosttySession,
        drained: Result<EngineDrain, String>,
    ) {
        assert_eq!(
            drained,
            Ok(EngineDrain {
                program_reset: true
            })
        );
        assert_eq!(session.inner.ui_events.take_title().as_deref(), Some(""));
        assert_eq!(
            session
                .inner
                .ui_events
                .take_progress()
                .map(|report| report.state),
            Some(ghostty::ProgressState::Remove)
        );
    }

    #[test]
    fn a_program_reset_is_signaled_to_the_publisher_and_clears_title_and_progress() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut terminal = effects_terminal();
        terminal
            .feed(b"\x1b]2;htop\x07\x1b]9;4;3\x07\x1bc")
            .expect("reset parses");

        let drained = handle_engine_events_to(&session.inner, &mut terminal, &mut |_| Ok(()));

        assert_reset_reaches_the_pane(&session, drained);
    }

    #[test]
    fn a_manual_reset_clears_title_and_progress_like_a_program_reset() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut terminal = effects_terminal();
        terminal
            .feed(b"\x1b]2;htop\x07\x1b]9;4;3\x07")
            .expect("title parses");
        let _ = handle_engine_events_to(&session.inner, &mut terminal, &mut |_| Ok(()));

        terminal.reset();
        let drained = handle_engine_events_to(&session.inner, &mut terminal, &mut |_| Ok(()));

        assert_reset_reaches_the_pane(&session, drained);
    }

    #[test]
    fn program_notifications_are_limited_to_three_per_minute_and_merged() {
        let mut slot = NotificationSlot::default();
        let start = Instant::now();
        let notification = |body: &str| ProgramNotification {
            title: "build".into(),
            body: body.into(),
        };

        assert!(slot.admit(&notification("one"), start));
        assert!(!slot.admit(&notification("one"), start + Duration::from_secs(1)));
        assert!(slot.admit(&notification("two"), start + Duration::from_secs(2)));
        assert!(slot.admit(&notification("three"), start + Duration::from_secs(3)));
        assert!(!slot.admit(&notification("four"), start + Duration::from_secs(4)));
        assert!(!slot.admit(&notification("five"), start + Duration::from_secs(59)));
        assert!(slot.admit(&notification("six"), start + Duration::from_secs(60)));
        assert!(!slot.admit(&notification("two"), start + Duration::from_secs(61)));
    }

    #[test]
    fn an_effects_overflow_warns_at_most_once_per_minute() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        warn_effects_overflow(&session.inner, 4, 8);
        let first = *session
            .inner
            .ui_events
            .effects_overflow_warned_at
            .lock()
            .expect("warn gate");
        warn_effects_overflow(&session.inner, 4, 8);
        let second = *session
            .inner
            .ui_events
            .effects_overflow_warned_at
            .lock()
            .expect("warn gate");

        assert!(first.is_some());
        assert_eq!(first, second);
    }

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
