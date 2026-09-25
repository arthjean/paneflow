use super::*;

const FOLLOW_RECONNECT_DELAY: Duration = Duration::from_millis(500);
const FOLLOW_RECONNECT_MAX_DELAY: Duration = Duration::from_secs(5);
const FOLLOW_RECONNECT_SLICE: Duration = Duration::from_millis(50);
const CONTROL_RECONNECT_BACKOFF: Duration = Duration::from_millis(500);
const CONTROL_REQUEST_SLOTS: usize = 64;

struct ReconnectBackoff {
    delay: Duration,
}

impl ReconnectBackoff {
    fn new() -> Self {
        Self {
            delay: FOLLOW_RECONNECT_DELAY,
        }
    }

    fn reset(&mut self) {
        self.delay = FOLLOW_RECONNECT_DELAY;
    }

    fn wait(&mut self, link: &HostLinkShared) -> bool {
        let deadline = Instant::now() + self.delay;
        self.delay = (self.delay * 2).min(FOLLOW_RECONNECT_MAX_DELAY);
        while Instant::now() < deadline {
            if link.stopped() {
                return false;
            }
            std::thread::sleep(FOLLOW_RECONNECT_SLICE.min(deadline - Instant::now()));
        }
        !link.stopped()
    }
}

enum ControlRequest {
    Input(Vec<u8>),
    BindRuntime(Option<&'static str>),
    Resize { cols: u16, rows: u16 },
    Release,
}

struct ControlLink {
    tx: SyncSender<ControlRequest>,
}

impl ControlLink {
    fn spawn(
        attachment: &HostAttachment,
        link: Arc<HostLinkShared>,
        inner: Arc<SessionInner>,
    ) -> Self {
        let (tx, rx) = sync_channel::<ControlRequest>(CONTROL_REQUEST_SLOTS);
        let mut control = HostControl::new(attachment);
        let spawned = std::thread::Builder::new()
            .name("paneflow-ghostty-control".into())
            .spawn(move || {
                while let Ok(request) = rx.recv() {
                    match request {
                        ControlRequest::Input(bytes) => control.send_input(&inner, &link, &bytes),
                        ControlRequest::BindRuntime(runtime_id) => {
                            control.bind_runtime(&link, runtime_id);
                        }
                        ControlRequest::Resize { cols, rows } => {
                            if let Err(error) = control.resize(&link, cols, rows) {
                                log::warn!(
                                    target: "paneflow::terminal::ghostty",
                                    "hosted resize to {cols}x{rows} was not applied by the host: {error}"
                                );
                            }
                        }
                        ControlRequest::Release => control.release(),
                    }
                }
            });
        if let Err(error) = spawned {
            log::error!(
                target: "paneflow::terminal::ghostty",
                "could not start the host control thread: {error}"
            );
        }
        Self { tx }
    }

    fn send_input(&self, inner: &SessionInner, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        match self.tx.try_send(ControlRequest::Input(bytes.to_vec())) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => reject_input(
                inner,
                "host",
                "the host control queue is full; input discarded before it was sent",
            ),
            Err(TrySendError::Disconnected(_)) => reject_input(
                inner,
                "host",
                "the host control link is closed; input discarded",
            ),
        }
    }

    fn bind_runtime(&self, runtime_id: Option<&'static str>) {
        let _ = self.tx.try_send(ControlRequest::BindRuntime(runtime_id));
    }

    fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        self.tx
            .try_send(ControlRequest::Resize { cols, rows })
            .map_err(|_| "the host control queue did not accept the resize".to_string())
    }

    fn release(&self) {
        let _ = self.tx.try_send(ControlRequest::Release);
    }
}
const CONTROL_SEND_RETRY: Duration = Duration::from_millis(5);
const CONTROL_SEND_BUDGET: Duration = Duration::from_secs(5);

#[derive(Default)]
struct HostLinkShared {
    stop: AtomicBool,
    attached: AtomicBool,
    ended: Mutex<Option<HostLinkEnd>>,
}

impl HostLinkShared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    fn ended(&self) -> Option<HostLinkEnd> {
        self.ended
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

pub(super) fn restore_terminal_from_checkpoint(
    snapshot: &[u8],
    size: TerminalWindowSize,
    max_scrollback: usize,
    option_as_alt: bool,
) -> Result<ghostty::DisplayTerminal, String> {
    let mut decoder = ghostty::SnapshotDecoder::from_bytes(snapshot)
        .map_err(|error| format!("checkpoint could not be opened: {error}"))?;
    decoder
        .set_max_continuation_bytes(paneflow_host::runtime::CONTINUATION_MAX_BYTES)
        .map_err(|error| format!("continuation budget could not be applied: {error}"))?;
    decoder
        .set_retain_continuation(true)
        .map_err(|error| format!("continuation retention could not be applied: {error}"))?;
    decoder
        .decode(ghostty::SnapshotRestore {
            cell_width: u32::from(size.cell_width.max(1)),
            cell_height: u32::from(size.cell_height.max(1)),
            max_scrollback,
            appearance: current_ghostty_appearance(),
        })
        .map_err(|error| format!("checkpoint could not be decoded: {error}"))?;
    let mut terminal = decoder
        .into_terminal()
        .ok_or_else(|| "checkpoint decoder produced no terminal".to_string())?;
    configure_embedder_options(&mut terminal, max_scrollback, option_as_alt);
    Ok(terminal)
}

fn record_host_size(inner: &SessionInner, terminal: &mut ghostty::DisplayTerminal) {
    let Ok(content) = terminal.snapshot() else {
        return;
    };
    let mut resize = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let requested = resize.requested;
    resize.applied = Some(TerminalWindowSize::new(
        content.cols,
        content.rows,
        requested.cell_width,
        requested.cell_height,
    ));
    resize.submitted = None;
    let needs_resize = resize.applied != Some(requested);
    drop(resize);
    if needs_resize {
        inner.command_backpressure.store(true, Ordering::Release);
        notify_command_capacity(inner);
    }
}

struct HostControl {
    attachment: HostAttachment,
    client: Option<HostClient>,
    last_failure: Option<Instant>,
    gone: bool,
}

impl HostControl {
    fn new(attachment: &HostAttachment) -> Self {
        Self {
            attachment: attachment.clone(),
            client: None,
            last_failure: None,
            gone: false,
        }
    }

    fn client(&mut self, link: &HostLinkShared) -> Option<&mut HostClient> {
        if self.gone || !link.attached.load(Ordering::Acquire) {
            return None;
        }
        if self.client.is_none() {
            if self
                .last_failure
                .is_some_and(|failed| failed.elapsed() < CONTROL_RECONNECT_BACKOFF)
            {
                return None;
            }
            match HostClient::connect(&self.attachment.endpoint, &self.attachment.hello) {
                Ok(client) => self.client = Some(client),
                Err(error) => {
                    log::debug!(
                        target: "paneflow::terminal::ghostty",
                        "host control connection failed: {error}"
                    );
                    self.last_failure = Some(Instant::now());
                    return None;
                }
            }
        }
        self.client.as_mut()
    }

    fn note_failure(&mut self, error: &HostClientError) {
        if error.is_session_gone() {
            self.gone = true;
        }
        self.client = None;
        self.last_failure = Some(Instant::now());
    }

    fn release(&mut self) {
        self.gone = true;
        self.client = None;
    }

    fn send_input(&mut self, inner: &SessionInner, link: &HostLinkShared, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let (session, generation) = (self.attachment.session.clone(), self.attachment.generation);
        let Some(client) = self.client(link) else {
            reject_input(
                inner,
                "host",
                "the session is not attached; input discarded",
            );
            return;
        };
        if let Err(error) = client.input(&session, generation, bytes) {
            if error.is_connection_loss() {
                reject_input(
                    inner,
                    "host",
                    format!(
                        "input delivery could not be confirmed ({error}); check the terminal before sending it again"
                    ),
                );
            } else {
                reject_input(inner, "host", format!("{error}; input not resent"));
            }
            self.note_failure(&error);
        }
    }

    fn bind_runtime(&mut self, link: &HostLinkShared, runtime_id: Option<&'static str>) {
        let (session, generation) = (self.attachment.session.clone(), self.attachment.generation);
        let Some(client) = self.client(link) else {
            return;
        };
        if let Err(error) = client.bind_runtime(&session, generation, runtime_id) {
            log::debug!(
                target: "paneflow::terminal::ghostty",
                "the launch binding was refused: {error}"
            );
            self.note_failure(&error);
        }
    }

    fn resize(&mut self, link: &HostLinkShared, cols: u16, rows: u16) -> Result<(), String> {
        let (session, generation) = (self.attachment.session.clone(), self.attachment.generation);
        let Some(client) = self.client(link) else {
            return Err("the session is not attached".to_string());
        };
        client
            .resize(&session, generation, cols, rows)
            .map_err(|error| {
                let message = error.to_string();
                self.note_failure(&error);
                message
            })
    }
}

pub(super) fn run_attached_runtime(
    inner: Arc<SessionInner>,
    mailbox: Arc<RuntimeMailbox>,
    attachment: HostAttachment,
    snapshot: CheckpointPayload,
    max_scrollback: usize,
    startup_tx: SyncSender<Result<(), String>>,
) {
    let _mailbox_close = MailboxCloseGuard(mailbox.clone());
    let initial_size = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .requested;
    let restored = restore_terminal_from_checkpoint(
        snapshot.as_slice(),
        initial_size,
        max_scrollback,
        inner.option_as_alt.load(Ordering::Acquire),
    );
    drop(snapshot);
    let mut terminal = match restored {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = startup_tx.send(Err(error));
            return;
        }
    };
    let mut publish_gate = PublishGate::new();
    if let Err(error) = publish_gate.publish_now(&inner, &mut terminal) {
        let _ = startup_tx.send(Err(error));
        return;
    }
    record_host_size(&inner, &mut terminal);
    if startup_tx.send(Ok(())).is_err() {
        return;
    }

    let link = Arc::new(HostLinkShared::default());
    link.attached.store(true, Ordering::Release);
    let control = ControlLink::spawn(&attachment, link.clone(), inner.clone());
    {
        let follower_attachment = attachment.clone();
        let follower_mailbox = mailbox.clone();
        let follower_link = link.clone();
        let follower_inner = inner.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("paneflow-ghostty-follower".into())
            .spawn(move || {
                follow_host_output(
                    follower_attachment,
                    follower_mailbox,
                    follower_link,
                    follower_inner,
                )
            })
        {
            let _ = inner
                .events_tx
                .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                    "could not start the host follower: {error}"
                )));
        }
    }

    let mut no_writer: Option<Box<dyn Write + Send>> = None;
    let mut marks_scanner = Osc133Scanner::default();
    let mut service_output_tail = ServiceOutputTail::default();
    let mut last_recent_output_refresh = None;
    let mut recent_output_pending = false;
    let mut runtime_failed = false;
    let mut last_autoscroll = Instant::now();
    let mut last_output_at = Instant::now();
    let mut paste_trace = BracketedPasteTrace::default();

    loop {
        count_runtime_loop_iteration();
        advance_selection_autoscroll(
            &inner,
            &mut terminal,
            &mut publish_gate,
            &mut last_autoscroll,
        );
        if inner.shutdown_sent.load(Ordering::Acquire) {
            break;
        }
        let wait = match publish_gate.next_wake(Instant::now()) {
            Some(wake) => wake.clamp(Duration::from_millis(1), RUNTIME_IDLE_TICK),
            None => {
                let recent_output = last_output_at.elapsed() < RUNTIME_QUIET_AFTER;
                let drag_live = lock_gesture(&inner).applied.is_some();
                if recent_output_pending || recent_output || drag_live {
                    RUNTIME_IDLE_TICK
                } else {
                    RUNTIME_QUIET_TICK
                }
            }
        };
        let received = match mailbox.recv_timeout(wait) {
            Ok(message) => {
                if matches!(
                    &message,
                    RuntimeMessage::Input(_)
                        | RuntimeMessage::KeyInput(_)
                        | RuntimeMessage::MouseInput { .. }
                        | RuntimeMessage::PasteInput { .. }
                ) {
                    publish_gate.interactive_until =
                        Some(Instant::now() + INTERACTIVE_OUTPUT_WINDOW);
                }
                match handle_terminal_command(&inner, &mut terminal, &mut publish_gate, message) {
                    CommandOutcome::Handled => Ok(None),
                    CommandOutcome::Unhandled(message) => Ok(Some(message)),
                }
            }
            Err(error) => Err(error),
        };
        count_runtime_loop_wait(wait, received.is_ok());
        match received {
            Ok(Some(RuntimeMessage::Output(bytes))) => {
                last_output_at = Instant::now();
                if let Err(error) = process_output_batch(
                    &inner,
                    &mailbox,
                    &mut terminal,
                    &mut no_writer,
                    &mut marks_scanner,
                    &mut service_output_tail,
                    &mut last_recent_output_refresh,
                    &mut recent_output_pending,
                    &mut publish_gate,
                    bytes,
                ) {
                    if !runtime_failed {
                        let _ = inner
                            .events_tx
                            .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
                    }
                    runtime_failed = true;
                }
                paste_trace.observe(&terminal);
            }
            Ok(Some(RuntimeMessage::Eof)) => {
                if recent_output_pending {
                    publish_recent_output_lines(
                        &inner,
                        &service_output_tail,
                        &mut recent_output_pending,
                    );
                    queue_service_output_ready(&inner);
                }
                let _ = publish_gate.publish_now(&inner, &mut terminal);
                if let Some(end) = link.ended() {
                    stop_session_input(&inner);
                    control.release();
                    if let Some((code, signal)) = end.exit() {
                        publish_child_exit_once(&inner, code, signal);
                    }
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::HostLink(HostLinkState::Ended(end)));
                }
            }
            Ok(Some(RuntimeMessage::RestoreCheckpoint { snapshot })) => {
                let size = inner
                    .resize
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .requested;
                let restored = restore_terminal_from_checkpoint(
                    snapshot.as_slice(),
                    size,
                    max_scrollback,
                    inner.option_as_alt.load(Ordering::Acquire),
                );
                drop(snapshot);
                match restored {
                    Ok(restored) => {
                        terminal = restored;
                        marks_scanner = Osc133Scanner::default();
                        paste_trace = BracketedPasteTrace::default();
                        if let Err(error) = publish_gate.publish_now(&inner, &mut terminal) {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "restored checkpoint could not be published: {error}"
                            );
                        }
                        record_host_size(&inner, &mut terminal);
                    }
                    Err(error) => {
                        log::error!(
                            target: "paneflow::terminal::ghostty",
                            "fresh checkpoint rejected, keeping the last rendered content: {error}"
                        );
                        link.stop.store(true, Ordering::Release);
                        let _ = inner.events_tx.unbounded_send(GhosttyUiEvent::HostLink(
                            HostLinkState::Ended(HostLinkEnd::attach_refused(error)),
                        ));
                    }
                }
            }
            Ok(Some(RuntimeMessage::Input(bytes))) => {
                release_queued_input_bytes(&inner, bytes.len());
                control.send_input(&inner, &bytes);
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::KeyInput(input))) => {
                release_queued_input_bytes(
                    &inner,
                    std::mem::size_of::<ghostty::KeyInput>().saturating_add(input.text.len()),
                );
                match terminal.encode_key(&input) {
                    Ok(bytes) => control.send_input(&inner, &bytes),
                    Err(error) => reject_input(&inner, "key", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::MouseInput { input, repeat })) => {
                release_queued_input_bytes(
                    &inner,
                    std::mem::size_of::<ghostty::MouseInput>().saturating_add(repeat),
                );
                for _ in 0..repeat {
                    match terminal.encode_mouse(input) {
                        Ok(bytes) => control.send_input(&inner, &bytes),
                        Err(error) => {
                            reject_input(&inner, "mouse", error);
                            break;
                        }
                    }
                }
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::FocusInput(event))) => {
                release_queued_input_bytes(&inner, std::mem::size_of::<ghostty::FocusEvent>());
                match terminal.encode_focus(event) {
                    Ok(bytes) => control.send_input(&inner, &bytes),
                    Err(error) => reject_input(&inner, "focus", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::PasteInput {
                text,
                allow_unsafe,
                location,
            })) => {
                release_queued_input_bytes(&inner, text.len());
                paste_trace.note_paste(text.len());
                let representation = [ghostty::PasteRepresentation {
                    mime: PASTE_TEXT_MIME,
                    data: text.as_bytes(),
                }];
                match terminal.paste(&representation, location, allow_unsafe) {
                    Ok(_) => {
                        let mut pasted = Vec::new();
                        let drained =
                            handle_engine_events_to(&inner, &mut terminal, &mut |bytes| {
                                pasted.extend_from_slice(bytes);
                                Ok(())
                            });
                        match drained {
                            Ok(()) => control.send_input(&inner, &pasted),
                            Err(error) => {
                                if !runtime_failed {
                                    let _ = inner
                                        .events_tx
                                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
                                }
                                runtime_failed = true;
                            }
                        }
                    }
                    Err(error) => reject_input(&inner, "paste", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::Resize(command))) => {
                let size = command.size;
                let resized = window_size(size)
                    .map_err(|error| error.to_string())
                    .and_then(|ghostty_size| {
                        terminal
                            .resize(ghostty_size)
                            .map_err(|error| error.to_string())
                    })
                    .and_then(|()| {
                        let cols = u16::try_from(size.cols.clamp(1, usize::from(u16::MAX)))
                            .unwrap_or(u16::MAX);
                        let rows = u16::try_from(size.rows.clamp(1, usize::from(u16::MAX)))
                            .unwrap_or(u16::MAX);
                        control.resize(cols, rows)
                    })
                    .and_then(|()| publish_gate.publish_now(&inner, &mut terminal));
                let resize_succeeded = match resized {
                    Ok(()) => true,
                    Err(error) => {
                        log::warn!(
                            target: "paneflow::terminal::ghostty",
                            "hosted resize to {}x{} failed: {error}",
                            size.cols,
                            size.rows,
                        );
                        false
                    }
                };
                complete_resize(&inner, command, resize_succeeded);
            }
            #[cfg(test)]
            Ok(Some(RuntimeMessage::SimulateWorkerCrash)) => {
                panic!("Ghostty runtime worker failure injected for test");
            }
            Ok(Some(RuntimeMessage::BindRuntime(runtime_id))) => {
                control.bind_runtime(runtime_id);
            }
            Ok(Some(RuntimeMessage::Shutdown)) => break,
            Ok(Some(RuntimeMessage::WriteOutput { reply, .. })) => {
                let _ = reply.send(());
            }
            Ok(None) | Ok(Some(_)) => {}
            Err(MailboxRecvError::Disconnected) => break,
            Err(MailboxRecvError::Timeout) => {}
        }

        if let Err(error) = publish_gate.poll(&inner, &mut terminal) {
            if !runtime_failed {
                let _ = inner
                    .events_tx
                    .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
            }
            runtime_failed = true;
        }

        if refresh_recent_output_lines(
            &inner,
            &service_output_tail,
            &mut last_recent_output_refresh,
            &mut recent_output_pending,
        ) {
            queue_service_output_ready(&inner);
        }

        notify_command_capacity(&inner);
    }
    link.stop.store(true, Ordering::Release);
    stop_session_input(&inner);
}

fn push_host_output(mailbox: &RuntimeMailbox, bytes: &[u8]) -> bool {
    for chunk in bytes.chunks(OUTPUT_CHUNK_BYTES) {
        let Some(mut buffer) = mailbox.take_output_buffer() else {
            return false;
        };
        buffer.clear();
        buffer.extend_from_slice(chunk);
        if !mailbox.send_output(buffer) {
            return false;
        }
    }
    true
}

fn send_control_until(
    mailbox: &RuntimeMailbox,
    link: &HostLinkShared,
    mut message: RuntimeMessage,
) -> bool {
    let deadline = Instant::now() + CONTROL_SEND_BUDGET;
    loop {
        match mailbox.try_send_control(message) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if link.stopped() || Instant::now() >= deadline {
                    return false;
                }
                message = returned;
                std::thread::sleep(CONTROL_SEND_RETRY);
            }
        }
    }
}

fn finish_host_link(link: &HostLinkShared, mailbox: &RuntimeMailbox, end: HostLinkEnd) {
    link.attached.store(false, Ordering::Release);
    *link
        .ended
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(end);
    mailbox.send_eof();
}

fn end_from_host(client: &mut HostClient, attachment: &HostAttachment) -> HostLinkEnd {
    let owner = client.identity().host_instance.clone();
    match client.inspect(&attachment.session) {
        Ok(summary) => {
            let generation = summary.manifest.generation;
            if generation != attachment.generation {
                return HostLinkEnd::from_reconnection(summary.reconnection(&owner), generation);
            }
            match summary.reconnection(&owner) {
                paneflow_host::SessionReconnection::Live
                | paneflow_host::SessionReconnection::Starting => HostLinkEnd::attach_refused(
                    "The local host stopped streaming this session.".to_string(),
                ),
                other => HostLinkEnd::from_reconnection(other, generation),
            }
        }
        Err(error) if error.code() == Some(paneflow_host::protocol::ERR_SESSION_NOT_FOUND) => {
            HostLinkEnd::missing()
        }
        Err(error) => HostLinkEnd::attach_refused(format!(
            "The local host could not report the session state: {error}"
        )),
    }
}

fn follow_host_output(
    attachment: HostAttachment,
    mailbox: Arc<RuntimeMailbox>,
    link: Arc<HostLinkShared>,
    inner: Arc<SessionInner>,
) {
    let mut next_offset = attachment.offset;
    let mut need_checkpoint = false;
    let mut reconnecting = false;
    let mut backoff = ReconnectBackoff::new();
    let announce = |state: HostLinkState| {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::HostLink(state));
    };
    let lose_link = |reconnecting: &mut bool| {
        link.attached.store(false, Ordering::Release);
        if !*reconnecting {
            *reconnecting = true;
            announce(HostLinkState::Reconnecting);
        }
    };
    loop {
        if link.stopped() {
            return;
        }
        let mut client = match HostClient::connect(&attachment.endpoint, &attachment.hello) {
            Ok(client) => client,
            Err(HostClientError::Incompatible(message)) => {
                finish_host_link(&link, &mailbox, HostLinkEnd::incompatible(message));
                return;
            }
            Err(_) => {
                lose_link(&mut reconnecting);
                need_checkpoint = true;
                if !backoff.wait(&link) {
                    return;
                }
                continue;
            }
        };
        if client.identity().host_instance != attachment.host_instance {
            finish_host_link(&link, &mailbox, HostLinkEnd::host_replaced());
            return;
        }
        if need_checkpoint {
            match client.attach(&attachment.session, Some(attachment.generation)) {
                Ok(fresh) => {
                    next_offset = fresh.checkpoint.offset;
                    if !send_control_until(
                        &mailbox,
                        &link,
                        RuntimeMessage::RestoreCheckpoint {
                            snapshot: CheckpointPayload::new(fresh.checkpoint.snapshot),
                        },
                    ) {
                        return;
                    }
                }
                Err(error) if error.is_session_gone() => {
                    let end = end_from_host(&mut client, &attachment);
                    finish_host_link(&link, &mailbox, end);
                    return;
                }
                Err(error) if error.is_connection_loss() => {
                    lose_link(&mut reconnecting);
                    if !backoff.wait(&link) {
                        return;
                    }
                    continue;
                }
                Err(error) => {
                    finish_host_link(
                        &link,
                        &mailbox,
                        HostLinkEnd::attach_refused(error.to_string()),
                    );
                    return;
                }
            }
        }
        link.attached.store(true, Ordering::Release);
        backoff.reset();
        if reconnecting {
            reconnecting = false;
            announce(HostLinkState::Attached);
        }
        let mut gap = false;
        let mut sink_closed = false;
        let stream = client.output(
            &attachment.session,
            Some(attachment.generation),
            next_offset,
            true,
            |offset, bytes| {
                if offset > next_offset {
                    gap = true;
                    return false;
                }
                let skip = usize::try_from(next_offset - offset).unwrap_or(usize::MAX);
                if skip >= bytes.len() {
                    return true;
                }
                let fresh = &bytes[skip..];
                if !push_host_output(&mailbox, fresh) {
                    sink_closed = true;
                    return false;
                }
                next_offset += fresh.len() as u64;
                true
            },
            || !link.stopped(),
        );
        match stream {
            Ok(end) if end.stopped_by_client => {
                if sink_closed || link.stopped() {
                    return;
                }
                if gap {
                    need_checkpoint = true;
                    continue;
                }
                return;
            }
            Ok(end) if !end.live => {
                let end = end_from_host(&mut client, &attachment);
                finish_host_link(&link, &mailbox, end);
                return;
            }
            Ok(_) => {
                lose_link(&mut reconnecting);
                need_checkpoint = true;
                if !backoff.wait(&link) {
                    return;
                }
            }
            Err(error) if error.code() == Some(ERR_OUTPUT_EVICTED) => {
                need_checkpoint = true;
            }
            Err(error) if error.is_session_gone() => {
                let end = end_from_host(&mut client, &attachment);
                finish_host_link(&link, &mailbox, end);
                return;
            }
            Err(_) => {
                lose_link(&mut reconnecting);
                need_checkpoint = true;
                if !backoff.wait(&link) {
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CHECKPOINT_ACCOUNTING: Mutex<()> = Mutex::new(());

    fn checkpoint_accounting_lock() -> std::sync::MutexGuard<'static, ()> {
        CHECKPOINT_ACCOUNTING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn test_attachment(
        endpoint: &std::path::Path,
        hello: &paneflow_host::ClientHello,
        session: &paneflow_config::schema::SessionId,
        attachment: paneflow_host::Attachment,
    ) -> (HostAttachment, CheckpointPayload) {
        HostAttachment::from_checkpoint(
            endpoint.to_path_buf(),
            hello.clone(),
            session.clone(),
            attachment.host_instance,
            attachment.checkpoint,
        )
    }

    fn attached_host_endpoint(home: &std::path::Path) -> std::path::PathBuf {
        let unique = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        );
        #[cfg(windows)]
        {
            let _ = home;
            std::path::PathBuf::from(format!(r"\\.\pipe\paneflow-attach-test-{unique}"))
        }
        #[cfg(unix)]
        {
            home.join(format!("attach-test-{unique}.sock"))
        }
    }

    #[test]
    fn an_attached_mirror_restores_the_checkpoint_then_streams_only_later_bytes() {
        use crate::terminal::host_link::HostLinkEndKind;
        use paneflow_host::server::ServerHandle;
        use paneflow_host::{ClientHello, CreateSession, HostClient, SessionHost};

        let _accounting = checkpoint_accounting_lock();

        let home = tempfile::tempdir().expect("host home");
        let endpoint = attached_host_endpoint(home.path());
        let host = SessionHost::open(home.path(), &endpoint).expect("session host");
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).expect("host server");
        let hello = ClientHello::local("paneflow-desktop-test");
        let mut client = HostClient::connect(&endpoint, &hello).expect("control connection");

        #[cfg(windows)]
        let (shell, args, before_command) = (
            "cmd.exe",
            vec!["/Q".to_string(), "/D".to_string()],
            "echo MIRROR_BEFORE\r\n",
        );
        #[cfg(unix)]
        let (shell, args, before_command) = (
            "/bin/sh",
            Vec::<String>::new(),
            "echo MIRROR_BE\"FORE\"\r\n",
        );
        let created = client
            .create(&CreateSession {
                session: None,
                workspace: None,
                cwd: Some(std::env::temp_dir().display().to_string()),
                shell: Some(shell.to_string()),
                args,
                env: Default::default(),
                cols: Some(80),
                rows: Some(24),
                title: None,
            })
            .expect("hosted session");
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;

        client
            .input(&session, generation, before_command.as_bytes())
            .expect("input before the checkpoint");
        let mut drained = Vec::new();
        let mut offset = 0u64;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            offset = client
                .output(
                    &session,
                    Some(generation),
                    offset,
                    false,
                    |_, bytes| {
                        drained.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .expect("drain the host tail")
                .next_offset;
            if String::from_utf8_lossy(&drained).contains("MIRROR_BEFORE") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            String::from_utf8_lossy(&drained).contains("MIRROR_BEFORE"),
            "the host consumed the first command before the checkpoint"
        );

        let attachment = client
            .attach(&session, Some(generation))
            .expect("checkpoint");
        assert_eq!(attachment.host_instance, *host.instance());
        assert!(
            attachment.checkpoint.offset >= offset,
            "the checkpoint offset covers every byte already consumed"
        );

        let size = TerminalWindowSize::new(80, 24, 8, 16);
        let (mirror, pending, mut events_rx) = GhosttySession::pending(size);
        let checkpoint_bytes = attachment.checkpoint.snapshot.len();
        let checkpoint_offset = attachment.checkpoint.offset;
        let test_attachment = test_attachment(&endpoint, &hello, &session, attachment);
        assert_eq!(
            crate::terminal::host_link::retained_checkpoint_bytes(),
            checkpoint_bytes,
            "the one-shot payload is the only retained copy"
        );
        mirror
            .start_attached(pending, test_attachment.0.clone(), test_attachment.1, 1_000)
            .expect("attached mirror");
        mirror.promote();
        assert_eq!(
            crate::terminal::host_link::retained_checkpoint_bytes(),
            0,
            "the checkpoint payload is released after decoding"
        );

        let restored = mirror.screen_text().unwrap_or_default();
        assert!(
            restored.contains("MIRROR_BEFORE"),
            "the checkpoint restores the host screen: {restored:?}"
        );

        assert!(
            mirror.write(b"echo MIRROR_AFTER\r\n".to_vec()).is_sent(),
            "the mirror accepts input while attached"
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut mirrored = String::new();
        while Instant::now() < deadline {
            mirrored = mirror.screen_text().unwrap_or_default();
            if mirrored.contains("MIRROR_AFTER") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            mirrored.contains("MIRROR_AFTER"),
            "input reaches the host and its output streams back: {mirrored:?}"
        );
        let mut streamed = Vec::new();
        let mut streamed_offset = checkpoint_offset;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            streamed_offset = client
                .output(
                    &session,
                    Some(generation),
                    streamed_offset,
                    false,
                    |_, bytes| {
                        streamed.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .expect("drain what the host streams past the checkpoint")
                .next_offset;
            if String::from_utf8_lossy(&streamed).contains("MIRROR_AFTER") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let streamed = String::from_utf8_lossy(&streamed).into_owned();
        assert!(
            streamed.contains("MIRROR_AFTER"),
            "the tail past the checkpoint carries the later bytes: {streamed:?}"
        );
        assert!(
            !streamed.contains("MIRROR_BEFORE"),
            "bytes already in the checkpoint are never streamed again: {streamed:?}"
        );

        client
            .stop(&session, Some(generation))
            .expect("explicit session stop");
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut ended = None;
        while Instant::now() < deadline && ended.is_none() {
            while let Ok(event) = events_rx.try_recv() {
                if let GhosttyUiEvent::HostLink(HostLinkState::Ended(end)) = event {
                    ended = Some(end);
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let ended = ended.expect("the follower reports the end of the session");
        assert!(
            matches!(
                ended.kind,
                HostLinkEndKind::Exited { .. } | HostLinkEndKind::Lost
            ),
            "{ended:?}"
        );
        assert!(!HostLinkState::Ended(ended).accepts_input());
        let deadline = Instant::now() + Duration::from_secs(5);
        while server.active_connections() > 1 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(
            server.active_connections(),
            1,
            "US-009: a passive final view keeps no host connection besides the test's own client"
        );

        mirror.shutdown();
        server.stop().expect("server stop");
    }

    #[test]
    fn the_first_render_resize_keeps_the_restored_scrollback_of_an_attached_mirror() {
        use paneflow_host::server::ServerHandle;
        use paneflow_host::{ClientHello, CreateSession, HostClient, SessionHost};

        let _accounting = checkpoint_accounting_lock();

        let home = tempfile::tempdir().expect("host home");
        let endpoint = attached_host_endpoint(home.path());
        let host = SessionHost::open(home.path(), &endpoint).expect("session host");
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).expect("host server");
        let hello = ClientHello::local("paneflow-desktop-test");
        let mut client = HostClient::connect(&endpoint, &hello).expect("control connection");

        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<String>::new());
        let created = client
            .create(&CreateSession {
                session: None,
                workspace: None,
                cwd: Some(std::env::temp_dir().display().to_string()),
                shell: Some(shell.to_string()),
                args,
                env: Default::default(),
                cols: Some(80),
                rows: Some(24),
                title: None,
            })
            .expect("hosted session");
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;

        #[cfg(windows)]
        let command = b"for /L %i in (1,1,60) do @echo SCROLL_LINE_%i\r\n".as_slice();
        #[cfg(unix)]
        let command =
            b"i=1; while [ $i -le 60 ]; do echo SCROLL_LINE_$i; i=$((i+1)); done\r\n".as_slice();
        client
            .input(&session, generation, command)
            .expect("scrollback filler");
        let mut drained = Vec::new();
        let mut offset = 0u64;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            offset = client
                .output(
                    &session,
                    Some(generation),
                    offset,
                    false,
                    |_, bytes| {
                        drained.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .expect("drain the host tail")
                .next_offset;
            if String::from_utf8_lossy(&drained).contains("SCROLL_LINE_60") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(String::from_utf8_lossy(&drained).contains("SCROLL_LINE_60"));

        let attachment = client
            .attach(&session, Some(generation))
            .expect("checkpoint");
        let (mirror, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let test_attachment = test_attachment(&endpoint, &hello, &session, attachment);
        mirror
            .start_attached(
                pending,
                test_attachment.0.clone(),
                test_attachment.1,
                10_000,
            )
            .expect("attached mirror");
        mirror.promote();
        let restored = mirror.extract_scrollback().unwrap_or_default();
        assert!(
            restored.contains("SCROLL_LINE_1"),
            "the checkpoint carries the host scrollback: {restored:?}"
        );

        let first_render = TerminalWindowSize::new(100, 30, 8, 16);
        let (_, clear_consumed) = mirror.render_content(first_render, 0, 30, true);
        assert!(
            clear_consumed,
            "the first render requests the initial clear"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let (content, _) = mirror.render_content(first_render, 0, 30, false);
            if content.cols == 100 && content.rows == 30 {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let after = mirror.extract_scrollback().unwrap_or_default();
        assert!(
            after.contains("SCROLL_LINE_1"),
            "the first render resize must not wipe the restored scrollback: {after:?}"
        );

        mirror.shutdown();
        client
            .stop(&session, Some(generation))
            .expect("explicit session stop");
        server.stop().expect("server stop");
    }

    #[test]
    fn repeated_attach_and_detach_with_filled_scrollback_retains_no_checkpoint_bytes() {
        use paneflow_host::server::ServerHandle;
        use paneflow_host::{ClientHello, CreateSession, HostClient, SessionHost};

        let _accounting = checkpoint_accounting_lock();
        let home = tempfile::tempdir().expect("host home");
        let endpoint = attached_host_endpoint(home.path());
        let host = SessionHost::open(home.path(), &endpoint).expect("session host");
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).expect("host server");
        let hello = ClientHello::local("paneflow-desktop-test");
        let mut client = HostClient::connect(&endpoint, &hello).expect("control connection");

        #[cfg(windows)]
        let (shell, args, fill) = (
            "cmd.exe",
            vec!["/Q".to_string(), "/D".to_string()],
            "for /L %i in (1,1,300) do @echo FILL-LINE-%i\r\n",
        );
        #[cfg(unix)]
        let (shell, args, fill) = (
            "/bin/sh",
            Vec::<String>::new(),
            "i=0; while [ $i -lt 300 ]; do i=$((i+1)); echo FILL-LINE-$i; done\n",
        );
        let created = client
            .create(&CreateSession {
                session: None,
                workspace: None,
                cwd: Some(std::env::temp_dir().display().to_string()),
                shell: Some(shell.to_string()),
                args,
                env: Default::default(),
                cols: Some(80),
                rows: Some(24),
                title: None,
            })
            .expect("hosted session");
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;
        client
            .input(&session, generation, fill.as_bytes())
            .expect("fill the scrollback");
        let mut drained = Vec::new();
        let mut offset = 0;
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            offset = client
                .output(
                    &session,
                    Some(generation),
                    offset,
                    false,
                    |_, bytes| {
                        drained.extend_from_slice(bytes);
                        true
                    },
                    || true,
                )
                .expect("drain the host tail")
                .next_offset;
            if String::from_utf8_lossy(&drained).contains("FILL-LINE-300") {
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            String::from_utf8_lossy(&drained).contains("FILL-LINE-300"),
            "the scrollback is filled before the attach cycles start"
        );
        assert_eq!(crate::terminal::host_link::retained_checkpoint_bytes(), 0);

        for cycle in 0..5 {
            let attachment = client
                .attach(&session, Some(generation))
                .expect("checkpoint");
            assert!(
                !attachment.checkpoint.snapshot.is_empty(),
                "cycle {cycle}: the filled scrollback produces a checkpoint"
            );
            let (mirror, pending, _events_rx) =
                GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
            let (host_attachment, payload) =
                test_attachment(&endpoint, &hello, &session, attachment);
            mirror
                .start_attached(pending, host_attachment, payload, 1_000)
                .expect("attached mirror");
            mirror.promote();
            assert_eq!(
                crate::terminal::host_link::retained_checkpoint_bytes(),
                0,
                "US-008: cycle {cycle} releases its checkpoint after decoding"
            );
            let restored = mirror.screen_text().unwrap_or_default();
            assert!(
                restored.contains("FILL-LINE-300"),
                "US-008: cycle {cycle} restores the filled screen: {restored:?}"
            );
            mirror.shutdown();
            drop(mirror);
        }
        assert_eq!(
            crate::terminal::host_link::retained_checkpoint_bytes(),
            0,
            "US-008: five attach/detach cycles leave no serialized checkpoint resident"
        );

        let attachment = client
            .attach(&session, Some(generation))
            .expect("checkpoint");
        let (host_attachment, genuine) = test_attachment(&endpoint, &hello, &session, attachment);
        drop(genuine);
        let (mirror, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let corrupt = CheckpointPayload::new(vec![0xFF; 96]);
        let refused = mirror.start_attached(pending, host_attachment, corrupt, 1_000);
        assert!(
            refused.is_err(),
            "US-008: an undecodable checkpoint is refused instead of restoring stale state"
        );
        assert_eq!(
            crate::terminal::host_link::retained_checkpoint_bytes(),
            0,
            "US-008: a rejected checkpoint releases its payload"
        );
        drop(mirror);
        let after = client.inspect(&session).expect("the record survives");
        assert!(
            after.live && after.owned,
            "a rejected checkpoint never stops the host session: {after:?}"
        );

        client
            .stop(&session, Some(generation))
            .expect("explicit session stop");
        server.stop().expect("server stop");
    }

    #[test]
    fn dropping_the_attached_mirror_leaves_the_hosted_session_running() {
        use paneflow_host::server::ServerHandle;
        use paneflow_host::{ClientHello, CreateSession, HostClient, SessionHost};

        let _accounting = checkpoint_accounting_lock();

        let home = tempfile::tempdir().expect("host home");
        let endpoint = attached_host_endpoint(home.path());
        let host = SessionHost::open(home.path(), &endpoint).expect("session host");
        let server = ServerHandle::spawn(Arc::clone(&host), endpoint.clone()).expect("host server");
        let hello = ClientHello::local("paneflow-desktop-test");
        let mut client = HostClient::connect(&endpoint, &hello).expect("control connection");

        #[cfg(windows)]
        let (shell, args) = ("cmd.exe", vec!["/Q".to_string(), "/D".to_string()]);
        #[cfg(unix)]
        let (shell, args) = ("/bin/sh", Vec::<String>::new());
        let created = client
            .create(&CreateSession {
                session: None,
                workspace: None,
                cwd: Some(std::env::temp_dir().display().to_string()),
                shell: Some(shell.to_string()),
                args,
                env: Default::default(),
                cols: Some(80),
                rows: Some(24),
                title: None,
            })
            .expect("hosted session");
        let session = created.manifest.session.clone();
        let generation = created.manifest.generation;
        let pid = created.manifest.process.map(|process| process.pid);

        let attachment = client
            .attach(&session, Some(generation))
            .expect("checkpoint");
        let (mirror, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let test_attachment = test_attachment(&endpoint, &hello, &session, attachment);
        mirror
            .start_attached(pending, test_attachment.0.clone(), test_attachment.1, 1_000)
            .expect("attached mirror");
        mirror.promote();

        mirror.shutdown();
        drop(mirror);
        std::thread::sleep(Duration::from_millis(250));

        let after = client.inspect(&session).expect("the record survives");
        assert!(
            after.live && after.owned,
            "closing the client leaves the session running on its host: {after:?}"
        );
        assert_eq!(
            after.manifest.process.map(|process| process.pid),
            pid,
            "the host keeps the same child process"
        );

        let stopped = client
            .stop(&session, Some(generation))
            .expect("explicit session stop");
        assert!(!stopped.live, "an explicit stop still ends the session");
        server.stop().expect("server stop");
    }
}
