use super::*;

pub(super) enum RuntimeMessage {
    Output(Vec<u8>),
    Eof,
    Input(Vec<u8>),
    KeyInput(ghostty::KeyInput),
    MouseInput {
        input: ghostty::MouseInput,
        repeat: usize,
    },
    FocusInput(ghostty::FocusEvent),
    PasteInput {
        text: String,
        allow_unsafe: bool,
        location: ghostty::ClipboardLocation,
    },
    WriteOutput {
        bytes: Vec<u8>,
        reply: SyncSender<()>,
    },
    Resize(ResizeCommand),
    Scroll(ghostty::Scroll),
    ScrollToViewportRow(usize),
    PressSelection {
        point: ghostty::Point,
        behavior: ghostty::GestureBehavior,
        position: (f64, f64),
    },
    DragSelection(u64),
    ReleaseSelection {
        point: Option<ghostty::Point>,
    },
    ClearSelection,
    ClearScrollback,
    BindRuntime(Option<&'static str>),
    UpdateAppearance(ghostty::TerminalAppearance),
    SetDefaultCursor {
        shape: ghostty::CursorShape,
        blink: bool,
    },
    SetOptionAsAlt(bool),
    SearchChunk {
        start_row: usize,
        max_cells: usize,
        reply: SyncSender<Result<ghostty::SearchChunk, String>>,
    },
    SetNativeSearch(String),
    SelectNativeSearch {
        previous: bool,
        generation: u64,
    },
    LineTexts {
        lines: Vec<i32>,
        reply: SyncSender<Result<Vec<(i32, String)>, String>>,
    },
    SelectionText(SyncSender<Result<Option<String>, String>>),
    SelectAll(SyncSender<Result<Option<String>, String>>),
    HyperlinkHover(ghostty::Point),
    ExtractScrollback(SyncSender<Result<Option<String>, String>>),
    CaptureReplay(SyncSender<Result<Vec<u8>, String>>),
    ScreenText(SyncSender<Result<String, String>>),
    RestoreScrollback {
        text: String,
        reply: SyncSender<()>,
    },
    RestoreCheckpoint {
        snapshot: CheckpointPayload,
    },
    #[cfg(test)]
    SimulateWorkerCrash,
    Shutdown,
}

impl RuntimeMessage {
    pub(super) fn queued_input_bytes(&self) -> Option<usize> {
        match self {
            Self::Input(bytes) => Some(bytes.len()),
            Self::KeyInput(input) => {
                Some(std::mem::size_of::<ghostty::KeyInput>().saturating_add(input.text.len()))
            }
            Self::MouseInput { repeat, .. } => {
                Some(std::mem::size_of::<ghostty::MouseInput>().saturating_add(*repeat))
            }
            Self::FocusInput(_) => Some(std::mem::size_of::<ghostty::FocusEvent>()),
            Self::PasteInput { text, .. } => Some(text.len()),
            _ => None,
        }
    }
}

#[derive(Default)]
struct MailboxState {
    queue: VecDeque<RuntimeMessage>,
    control_count: usize,
    output_count: usize,
    available_output_buffers: Vec<Vec<u8>>,
    accepting_input: bool,
    accepting_output: bool,
    closed: bool,
}

pub(super) struct RuntimeMailbox {
    state: Mutex<MailboxState>,
    ready: Condvar,
    output_buffer_ready: Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MailboxRecvError {
    Timeout,
    Disconnected,
}

impl RuntimeMailbox {
    pub(super) fn new() -> Self {
        let available_output_buffers = (0..OUTPUT_BUFFER_COUNT)
            .map(|_| vec![0; OUTPUT_CHUNK_BYTES])
            .collect();
        Self {
            state: Mutex::new(MailboxState {
                available_output_buffers,
                accepting_input: true,
                accepting_output: true,
                ..MailboxState::default()
            }),
            ready: Condvar::new(),
            output_buffer_ready: Condvar::new(),
        }
    }

    pub(super) fn try_send_control(
        &self,
        message: RuntimeMessage,
    ) -> Result<(), TrySendError<RuntimeMessage>> {
        debug_assert!(!matches!(
            message,
            RuntimeMessage::Output(_) | RuntimeMessage::Eof
        ));
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return Err(TrySendError::Disconnected(message));
        }
        if !state.accepting_input && message.queued_input_bytes().is_some() {
            return Err(TrySendError::Disconnected(message));
        }
        if let RuntimeMessage::ScrollToViewportRow(row) = &message
            && let Some(RuntimeMessage::ScrollToViewportRow(queued_row)) = state.queue.back_mut()
        {
            *queued_row = *row;
            return Ok(());
        }
        if let RuntimeMessage::RestoreCheckpoint { snapshot } = message {
            for queued in state.queue.iter_mut() {
                if let RuntimeMessage::RestoreCheckpoint { snapshot: pending } = queued {
                    *pending = snapshot;
                    return Ok(());
                }
            }
            if state.control_count >= CONTROL_CAPACITY {
                return Err(TrySendError::Full(RuntimeMessage::RestoreCheckpoint {
                    snapshot,
                }));
            }
            state.control_count += 1;
            state
                .queue
                .push_back(RuntimeMessage::RestoreCheckpoint { snapshot });
            drop(state);
            self.ready.notify_one();
            return Ok(());
        }
        if state.control_count >= CONTROL_CAPACITY {
            return Err(TrySendError::Full(message));
        }
        state.control_count += 1;
        state.queue.push_back(message);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    pub(super) fn take_output_buffer(&self) -> Option<Vec<u8>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if state.closed || !state.accepting_output {
                return None;
            }
            if let Some(mut buffer) = state.available_output_buffers.pop() {
                buffer.resize(OUTPUT_CHUNK_BYTES, 0);
                return Some(buffer);
            }
            state = self
                .output_buffer_ready
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    pub(super) fn recycle_output_buffer(&self, mut buffer: Vec<u8>) {
        buffer.clear();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.available_output_buffers.push(buffer);
        drop(state);
        self.output_buffer_ready.notify_one();
    }

    pub(super) fn send_output(&self, buffer: Vec<u8>) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed || !state.accepting_output || state.output_count >= OUTPUT_BUFFER_COUNT {
            return false;
        }
        state.output_count += 1;
        state.queue.push_back(RuntimeMessage::Output(buffer));
        drop(state);
        self.ready.notify_one();
        true
    }

    pub(super) fn send_eof(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.control_count += 1;
        state.queue.push_back(RuntimeMessage::Eof);
        drop(state);
        self.ready.notify_one();
    }

    fn pop_front(state: &mut MailboxState) -> Option<RuntimeMessage> {
        let message = state.queue.pop_front()?;
        if matches!(message, RuntimeMessage::Output(_)) {
            state.output_count = state.output_count.saturating_sub(1);
        } else {
            state.control_count = state.control_count.saturating_sub(1);
        }
        Some(message)
    }

    pub(super) fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> Result<RuntimeMessage, MailboxRecvError> {
        let deadline = Instant::now() + timeout;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(message) = Self::pop_front(&mut state) {
                return Ok(message);
            }
            if state.closed {
                return Err(MailboxRecvError::Disconnected);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(MailboxRecvError::Timeout);
            }
            let (next_state, wait) = self
                .ready
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next_state;
            if wait.timed_out() && state.queue.is_empty() {
                return Err(MailboxRecvError::Timeout);
            }
        }
    }

    pub(super) fn try_recv_consecutive_output(&self) -> Option<Vec<u8>> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(state.queue.front(), Some(RuntimeMessage::Output(_))) {
            return None;
        }
        let RuntimeMessage::Output(bytes) = state.queue.pop_front()? else {
            return None;
        };
        state.output_count = state.output_count.saturating_sub(1);
        Some(bytes)
    }

    #[cfg(test)]
    pub(super) fn pending_output_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .output_count
    }

    pub(super) fn stop_accepting_input(&self) -> usize {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_input = false;
        let mut discarded_input_bytes = 0usize;
        let mut retained = VecDeque::with_capacity(state.queue.len());
        while let Some(message) = state.queue.pop_front() {
            if let Some(bytes) = message.queued_input_bytes() {
                discarded_input_bytes = discarded_input_bytes.saturating_add(bytes);
                state.control_count = state.control_count.saturating_sub(1);
            } else {
                retained.push_back(message);
            }
        }
        state.queue = retained;
        drop(state);
        self.ready.notify_all();
        discarded_input_bytes
    }

    #[cfg(test)]
    pub(super) fn stop_accepting_output(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_output = false;
        drop(state);
        self.output_buffer_ready.notify_all();
    }

    pub(super) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.accepting_input = false;
        state.accepting_output = false;
        state.closed = true;
        drop(state);
        self.ready.notify_all();
        self.output_buffer_ready.notify_all();
    }

    #[cfg(test)]
    pub(super) fn try_recv(&self) -> Result<RuntimeMessage, std::sync::mpsc::TryRecvError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(message) = Self::pop_front(&mut state) {
            Ok(message)
        } else if state.closed {
            Err(std::sync::mpsc::TryRecvError::Disconnected)
        } else {
            Err(std::sync::mpsc::TryRecvError::Empty)
        }
    }

    #[cfg(test)]
    pub(super) fn drain(&self) -> Vec<RuntimeMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = self.try_recv() {
            messages.push(message);
        }
        messages
    }
}

pub(super) struct MailboxCloseGuard(pub(super) Arc<RuntimeMailbox>);

impl Drop for MailboxCloseGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::pty_session::TerminalState;

    #[test]
    fn slow_output_consumer_cannot_grow_the_fixed_buffer_pool() {
        let mailbox = Arc::new(RuntimeMailbox::new());
        for index in 0..OUTPUT_BUFFER_COUNT {
            let mut buffer = mailbox
                .take_output_buffer()
                .expect("fixed output buffer must be available");
            buffer[0] = index as u8;
            buffer.truncate(1);
            assert!(mailbox.send_output(buffer));
        }
        assert_eq!(mailbox.pending_output_count(), OUTPUT_BUFFER_COUNT);

        let waiting_mailbox = mailbox.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let waiting_barrier = barrier.clone();
        let (available_tx, available_rx) = sync_channel(1);
        let waiter = std::thread::spawn(move || {
            waiting_barrier.wait();
            let length = waiting_mailbox
                .take_output_buffer()
                .map(|buffer| buffer.len());
            let _ = available_tx.send(length);
        });
        barrier.wait();
        assert!(matches!(
            available_rx.recv_timeout(Duration::from_millis(20)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        let RuntimeMessage::Output(buffer) = mailbox
            .recv_timeout(Duration::ZERO)
            .expect("slow consumer must release one queued buffer")
        else {
            panic!("expected queued output");
        };
        mailbox.recycle_output_buffer(buffer);
        assert_eq!(
            available_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("blocked reader must receive the recycled buffer"),
            Some(OUTPUT_CHUNK_BYTES)
        );
        mailbox.close();
        waiter.join().expect("buffer waiter must exit");
    }

    #[test]
    fn sealing_output_preserves_admitted_buffers_and_rejects_late_producers() {
        let mailbox = RuntimeMailbox::new();
        assert!(mailbox.send_output(vec![1, 2, 3]));

        mailbox.stop_accepting_output();

        assert!(mailbox.take_output_buffer().is_none());
        assert!(!mailbox.send_output(vec![4, 5, 6]));
        assert_eq!(mailbox.pending_output_count(), 1);
        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Output(bytes)) if bytes == [1, 2, 3]
        ));
        assert_eq!(mailbox.pending_output_count(), 0);
    }

    #[test]
    fn mailbox_bounds_output_without_blocking_control_admission() {
        let mailbox = RuntimeMailbox::new();
        for index in 0..OUTPUT_BUFFER_COUNT {
            assert!(mailbox.send_output(vec![index as u8]));
        }
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"input".to_vec()))
                .is_ok()
        );

        let queued = mailbox.drain();
        assert_eq!(queued.len(), OUTPUT_BUFFER_COUNT + 1);
        assert!(
            queued[..OUTPUT_BUFFER_COUNT]
                .iter()
                .all(|message| matches!(message, RuntimeMessage::Output(_)))
        );
        assert!(matches!(
            queued.last(),
            Some(RuntimeMessage::Input(bytes)) if bytes == b"input"
        ));
    }

    #[test]
    fn output_batching_stops_at_the_next_control_message() {
        let mailbox = RuntimeMailbox::new();
        assert!(mailbox.send_output(vec![1]));
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(vec![2]))
                .is_ok()
        );
        assert!(mailbox.send_output(vec![3]));

        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Output(bytes)) if bytes == vec![1]
        ));
        assert!(mailbox.try_recv_consecutive_output().is_none());
        assert!(matches!(
            mailbox.recv_timeout(Duration::ZERO),
            Ok(RuntimeMessage::Input(bytes)) if bytes == vec![2]
        ));
        assert!(matches!(
            mailbox.try_recv_consecutive_output(),
            Some(bytes) if bytes == vec![3]
        ));
    }

    #[test]
    fn absolute_scroll_rows_coalesce_at_queue_tail() {
        let mailbox = RuntimeMailbox::new();
        for row in [10, 20, 30] {
            assert!(
                mailbox
                    .try_send_control(RuntimeMessage::ScrollToViewportRow(row))
                    .is_ok()
            );
        }

        let queued = mailbox.drain();
        assert!(matches!(
            queued.as_slice(),
            [RuntimeMessage::ScrollToViewportRow(30)]
        ));
    }

    #[test]
    fn absolute_scroll_coalescing_preserves_fifo_barriers() {
        let mailbox = RuntimeMailbox::new();
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(10))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(20))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"barrier".to_vec()))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(30))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(40))
                .is_ok()
        );

        let queued = mailbox.drain();
        assert_eq!(queued.len(), 3);
        assert!(matches!(queued[0], RuntimeMessage::ScrollToViewportRow(20)));
        assert!(matches!(
            &queued[1],
            RuntimeMessage::Input(bytes) if bytes == b"barrier"
        ));
        assert!(matches!(queued[2], RuntimeMessage::ScrollToViewportRow(40)));
    }

    #[test]
    fn absolute_scroll_target_replaces_tail_at_control_capacity() {
        let mailbox = RuntimeMailbox::new();
        for _ in 0..CONTROL_CAPACITY - 1 {
            assert!(
                mailbox
                    .try_send_control(RuntimeMessage::ClearSelection)
                    .is_ok()
            );
        }
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(10))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ScrollToViewportRow(20))
                .is_ok()
        );
        assert!(matches!(
            mailbox.try_send_control(RuntimeMessage::ClearSelection),
            Err(TrySendError::Full(RuntimeMessage::ClearSelection))
        ));

        let queued = mailbox.drain();
        assert_eq!(queued.len(), CONTROL_CAPACITY);
        assert!(matches!(
            queued.last(),
            Some(RuntimeMessage::ScrollToViewportRow(20))
        ));
    }

    #[test]
    fn queued_row_jump_does_not_reject_a_relative_drag_step() {
        let (mut state, pending) = TerminalState::new_pending(80, 24);
        let runtime_pending = pending.ghostty;
        state.promote_ghostty(SpawnedGhostty {
            child_pid: 0,
            cwd: std::env::current_dir().unwrap(),
        });

        let backend = state.session_backend();
        assert!(backend.scroll_to_viewport_row(0));
        assert!(backend.scroll_delta(-1));

        let queued = runtime_pending.mailbox.drain();
        assert!(matches!(
            queued.as_slice(),
            [
                RuntimeMessage::ScrollToViewportRow(0),
                RuntimeMessage::Scroll(ghostty::Scroll::Delta(-1))
            ]
        ));
    }

    #[test]
    fn output_batching_barrier_trips_only_when_a_chunk_completes_a_mark() {
        let mut scanner = Osc133Scanner::default();
        let mut marks = Vec::new();

        assert!(!scan_chunk_for_marks(
            &mut scanner,
            b"before\x1b]133;A",
            &mut marks
        ));
        assert!(scan_chunk_for_marks(&mut scanner, b"\x07after", &mut marks));
        assert!(!scan_chunk_for_marks(
            &mut scanner,
            b"plain output",
            &mut marks
        ));
        assert_eq!(
            marks,
            vec![RawMark {
                kind: super::super::marks::MarkKind::Prompt,
            }]
        );
    }
}
