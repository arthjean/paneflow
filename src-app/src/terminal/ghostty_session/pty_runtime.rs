use super::*;

#[cfg(test)]
pub(super) enum StartupReport {
    Started(SpawnedGhostty),
    Failed(anyhow::Error),
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct StartupState {
    runtime_started: AtomicBool,
}

#[cfg(test)]
impl StartupState {
    fn mark_runtime_started(&self) {
        self.runtime_started.store(true, Ordering::Release);
    }

    fn clear_runtime_started(&self) {
        self.runtime_started.store(false, Ordering::Release);
    }

    pub(super) fn runtime_started(&self) -> bool {
        self.runtime_started.load(Ordering::Acquire)
    }
}

#[cfg(test)]
struct StartupChildGuard {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
    termination_target: ChildTerminationTarget,
}

#[cfg(test)]
impl StartupChildGuard {
    fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        termination_target: ChildTerminationTarget,
    ) -> Self {
        Self {
            child: Some(child),
            termination_target,
        }
    }

    fn terminate(&mut self) {
        if let Some(mut child) = self.child.take() {
            terminate_child(&mut *child, self.termination_target);
        }
    }

    fn take_child(&mut self) -> Option<Box<dyn portable_pty::Child + Send + Sync>> {
        self.child.take()
    }
}

#[cfg(test)]
impl Drop for StartupChildGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
struct RuntimeChildCleanupGuard {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    termination_target: ChildTerminationTarget,
    armed: bool,
}

#[cfg(test)]
impl RuntimeChildCleanupGuard {
    fn new(
        child: Box<dyn portable_pty::Child + Send + Sync>,
        termination_target: ChildTerminationTarget,
    ) -> Self {
        Self {
            child,
            termination_target,
            armed: true,
        }
    }

    fn child_mut(&mut self) -> &mut dyn portable_pty::Child {
        &mut *self.child
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(test)]
impl Drop for RuntimeChildCleanupGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                terminate_child(&mut *self.child, self.termination_target);
            }));
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
struct ChildExitReport {
    code: i32,
    signal: Option<String>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeLifecyclePhase {
    Running,
    Draining,
    Published,
}

#[cfg(test)]
struct RuntimeLifecycle {
    phase: RuntimeLifecyclePhase,
    eof: bool,
    output_sealed: bool,
    exit: Option<ChildExitReport>,
    drain_deadline: Option<Instant>,
}

#[cfg(test)]
impl RuntimeLifecycle {
    fn new() -> Self {
        Self {
            phase: RuntimeLifecyclePhase::Running,
            eof: false,
            output_sealed: false,
            exit: None,
            drain_deadline: None,
        }
    }

    fn is_running(&self) -> bool {
        self.phase == RuntimeLifecyclePhase::Running
    }

    fn record_eof(&mut self) {
        self.eof = true;
        self.output_sealed = true;
    }

    fn start_draining(&mut self, exit: ChildExitReport, now: Instant) -> bool {
        if !self.is_running() {
            return false;
        }
        self.phase = RuntimeLifecyclePhase::Draining;
        self.exit = Some(exit);
        self.drain_deadline = now.checked_add(FINAL_DRAIN_TIMEOUT);
        true
    }

    #[cfg(target_os = "windows")]
    fn replace_exit(&mut self, exit: ChildExitReport) {
        if self.phase == RuntimeLifecyclePhase::Draining {
            self.exit = Some(exit);
        }
    }

    fn drain_deadline_reached(&self, now: Instant) -> bool {
        self.phase == RuntimeLifecyclePhase::Draining
            && !self.output_sealed
            && self.drain_deadline.is_none_or(|deadline| now >= deadline)
    }

    fn seal_output(&mut self) {
        self.output_sealed = true;
    }

    fn take_ready_exit(
        &mut self,
        _now: Instant,
        pending_output_count: usize,
    ) -> Option<ChildExitReport> {
        if self.phase != RuntimeLifecyclePhase::Draining {
            return None;
        }
        if pending_output_count > 0 || !self.output_sealed {
            return None;
        }
        self.phase = RuntimeLifecyclePhase::Published;
        self.exit.take()
    }
}

#[cfg(test)]
struct PtyCloser<M: Send + 'static> {
    sender: Option<std::sync::mpsc::Sender<M>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(test)]
impl<M: Send + 'static> PtyCloser<M> {
    fn new(thread_name: &str) -> std::io::Result<Self> {
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::Builder::new()
            .name(thread_name.to_owned())
            .spawn(move || {
                if let Ok(master) = receiver.recv() {
                    drop(master);
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
        })
    }

    fn submit(&mut self, master: M) -> Result<(), M> {
        let Some(sender) = self.sender.take() else {
            return Err(master);
        };
        sender.send(master).map_err(|error| error.0)
    }

    fn join_until(&mut self, deadline: Instant) -> bool {
        loop {
            let Some(worker) = self.worker.as_ref() else {
                return true;
            };
            if worker.is_finished() {
                return self
                    .worker
                    .take()
                    .is_none_or(|worker| worker.join().is_ok());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            std::thread::sleep(remaining.min(Duration::from_millis(1)));
        }
    }
}

#[cfg(test)]
impl<M: Send + 'static> Drop for PtyCloser<M> {
    fn drop(&mut self) {
        drop(self.sender.take());
        if self
            .worker
            .as_ref()
            .is_some_and(std::thread::JoinHandle::is_finished)
        {
            let _ = self.worker.take().and_then(|worker| worker.join().ok());
        }
    }
}

#[cfg(test)]
struct DrainablePtyMaster<M: Send + 'static> {
    master: Option<M>,
    closer: PtyCloser<M>,
}

#[cfg(test)]
impl<M: Send + 'static> DrainablePtyMaster<M> {
    fn new(master: M, closer: PtyCloser<M>) -> Self {
        Self {
            master: Some(master),
            closer,
        }
    }

    #[cfg(target_os = "windows")]
    fn get(&self) -> Option<&M> {
        self.master.as_ref()
    }

    fn close_async(&mut self) -> bool {
        let Some(master) = self.master.take() else {
            return true;
        };
        match self.closer.submit(master) {
            Ok(()) => true,
            Err(master) => {
                std::mem::forget(master);
                false
            }
        }
    }

    fn join_until(&mut self, deadline: Instant) -> bool {
        self.closer.join_until(deadline)
    }
}

#[cfg(test)]
impl<M: Send + 'static> Drop for DrainablePtyMaster<M> {
    fn drop(&mut self) {
        let _ = self.close_async();
    }
}

#[cfg(test)]
fn close_pty_for_final_drain<W, M: Send + 'static>(
    writer: &mut Option<W>,
    master: &mut DrainablePtyMaster<M>,
) -> bool {
    drop(writer.take());
    master.close_async()
}

pub(super) fn publish_child_exit_once(inner: &SessionInner, code: i32, signal: Option<String>) {
    if inner
        .exit_published
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::ChildExited { code, signal });
    }
}

pub(super) fn release_queued_input_bytes(inner: &SessionInner, released: usize) {
    if released == 0 {
        return;
    }
    let _ = inner
        .queued_input_bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
            Some(queued.saturating_sub(released))
        });
}

pub(super) fn stop_session_input(inner: &SessionInner) {
    let discarded = inner.mailbox.stop_accepting_input();
    release_queued_input_bytes(inner, discarded);
}

#[cfg(test)]
pub(super) fn run_runtime(
    inner: Arc<SessionInner>,
    mailbox: Arc<RuntimeMailbox>,
    params: SpawnParams,
    max_scrollback: usize,
    startup_tx: SyncSender<StartupReport>,
    startup_state: Arc<StartupState>,
) {
    let _mailbox_close = MailboxCloseGuard(mailbox.clone());
    let initial_size = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .requested;
    let ghostty_size = match window_size(initial_size) {
        Ok(size) => size,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::Failed(anyhow::anyhow!(error.to_string())));
            return;
        }
    };
    let appearance = current_ghostty_appearance();
    let mut terminal = match ghostty::DisplayTerminal::new(ghostty_size, max_scrollback, appearance)
    {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::Failed(anyhow::anyhow!(error.to_string())));
            return;
        }
    };
    configure_embedder_options(
        &mut terminal,
        max_scrollback,
        inner.option_as_alt.load(Ordering::Acquire),
    );
    let mut publish_gate = PublishGate::new();
    if let Err(error) = publish_gate.publish_now(&inner, &mut terminal) {
        let _ = startup_tx.send(StartupReport::Failed(anyhow::anyhow!(error)));
        return;
    }

    let pair = match paneflow_host::pty::open(pty_size(initial_size)) {
        Ok(pair) => pair,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::Failed(
                anyhow::anyhow!(error).context("failed to open native PTY"),
            ));
            return;
        }
    };
    #[cfg(unix)]
    let master = pair.master;
    #[cfg(target_os = "windows")]
    let master_closer = match PtyCloser::<Box<dyn portable_pty::MasterPty + Send>>::new(
        "paneflow-ghostty-pty-closer",
    ) {
        Ok(closer) => closer,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::Failed(
                anyhow::Error::new(error).context("failed to start ConPTY close worker"),
            ));
            return;
        }
    };
    #[cfg(target_os = "windows")]
    let mut master = DrainablePtyMaster::new(pair.master, master_closer);
    let mut command = CommandBuilder::new(&params.shell);
    command.args(&params.extra_args);
    command.cwd(&params.cwd);
    for key in super::pty_session::inherited_env_keys_to_strip() {
        command.env_remove(&key);
    }
    for (key, value) in &params.env {
        command.env(key, value);
    }
    command.env("TERM_PROGRAM", "ghostty");
    command.env("TERM_PROGRAM_VERSION", ghostty::GHOSTTY_APP_VERSION);

    let child = match pair.slave.spawn_command(command) {
        Ok(child) => child,
        Err(error) => {
            let _ = startup_tx.send(StartupReport::Failed(
                error.context("failed to spawn shell in PTY"),
            ));
            return;
        }
    };
    let child_pid = child.process_id().unwrap_or(0);
    let termination_target = child_termination_target(child_pid);
    let mut startup_child = StartupChildGuard::new(child, termination_target);
    #[cfg(unix)]
    let reader = master.try_clone_reader();
    #[cfg(target_os = "windows")]
    let reader = master
        .get()
        .ok_or_else(|| anyhow::anyhow!("ConPTY master unavailable before reader clone"))
        .and_then(|master| master.try_clone_reader());
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::Failed(
                error.context("failed to clone PTY reader"),
            ));
            return;
        }
    };
    #[cfg(unix)]
    let writer = master.take_writer();
    #[cfg(target_os = "windows")]
    let writer = master
        .get()
        .ok_or_else(|| anyhow::anyhow!("ConPTY master unavailable before writer take"))
        .and_then(|master| master.take_writer());
    let writer = match writer {
        Ok(writer) => writer,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::Failed(
                error.context("failed to take PTY writer"),
            ));
            return;
        }
    };
    let output_mailbox = mailbox.clone();
    let reader_worker = match std::thread::Builder::new()
        .name("paneflow-ghostty-pty-reader".into())
        .spawn(move || read_pty(reader, output_mailbox))
    {
        Ok(worker) => worker,
        Err(error) => {
            startup_child.terminate();
            let _ = startup_tx.send(StartupReport::Failed(
                anyhow::Error::new(error).context("failed to start PTY reader"),
            ));
            return;
        }
    };
    #[cfg(target_os = "windows")]
    let mut reader_worker = Some(reader_worker);
    #[cfg(unix)]
    drop(reader_worker);

    drop(pair.slave);
    startup_state.mark_runtime_started();
    if startup_tx
        .send(StartupReport::Started(SpawnedGhostty {
            child_pid,
            cwd: params.cwd,
        }))
        .is_err()
    {
        startup_state.clear_runtime_started();
        startup_child.terminate();
        return;
    }
    let Some(child) = startup_child.take_child() else {
        return;
    };
    let mut child = RuntimeChildCleanupGuard::new(child, termination_target);
    let mut writer = Some(writer);

    let mut marks_scanner = Osc133Scanner::default();
    let mut service_output_tail = ServiceOutputTail::default();
    let mut last_recent_output_refresh = None;
    let mut recent_output_pending = false;
    #[cfg(unix)]
    let mut eof = false;
    #[cfg(unix)]
    let mut exit = None;
    #[cfg(unix)]
    let mut exit_seen_at = None;
    #[cfg(unix)]
    let mut child_cleaned = false;
    #[cfg(target_os = "windows")]
    let mut lifecycle = RuntimeLifecycle::new();
    #[cfg(target_os = "windows")]
    let mut shutdown_requested = false;
    #[cfg(target_os = "windows")]
    let mut child_reaped = false;
    #[cfg(target_os = "windows")]
    let mut child_wait_failure_reported = false;
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
            #[cfg(unix)]
            {
                if exit.is_none() {
                    terminate_child(child.child_mut(), termination_target);
                    child.disarm();
                    break;
                }
            }
            #[cfg(target_os = "windows")]
            {
                shutdown_requested = true;
            }
        }
        #[cfg(target_os = "windows")]
        if shutdown_requested && lifecycle.is_running() {
            begin_windows_shutdown(
                &inner,
                &mut writer,
                child.child_mut(),
                child_pid,
                &mut lifecycle,
                &mut child_reaped,
                &mut master,
            );
        }
        let wait = match publish_gate.next_wake(Instant::now()) {
            Some(wake) => wake.clamp(Duration::from_millis(1), RUNTIME_IDLE_TICK),
            None => {
                #[cfg(unix)]
                let winding_down = exit.is_some();
                #[cfg(target_os = "windows")]
                let winding_down = shutdown_requested || !lifecycle.is_running();
                let recent_output = last_output_at.elapsed() < RUNTIME_QUIET_AFTER;
                let drag_live = lock_gesture(&inner).applied.is_some();
                let attentive = winding_down || recent_output_pending || recent_output || drag_live;
                #[cfg(test)]
                RUNTIME_LOOP_ATTENTIVE_REASONS.fetch_or(
                    u64::from(winding_down)
                        | u64::from(recent_output_pending) << 1
                        | u64::from(recent_output) << 2
                        | u64::from(drag_live) << 3,
                    Ordering::Relaxed,
                );
                if attentive {
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
                    &mut writer,
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
                #[cfg(unix)]
                {
                    eof = true;
                }
                #[cfg(target_os = "windows")]
                {
                    lifecycle.record_eof();
                }
            }
            Ok(Some(RuntimeMessage::Input(bytes))) => {
                release_queued_input_bytes(&inner, bytes.len());
                write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed);
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::KeyInput(input))) => {
                release_queued_input_bytes(
                    &inner,
                    std::mem::size_of::<ghostty::KeyInput>().saturating_add(input.text.len()),
                );
                match terminal.encode_key(&input) {
                    Ok(bytes) => {
                        write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                    }
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
                        Ok(bytes) => {
                            write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                        }
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
                    Ok(bytes) => {
                        write_input_bytes(&inner, &mut writer, &bytes, &mut runtime_failed)
                    }
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
                        if let Err(error) = handle_engine_events(&inner, &mut terminal, &mut writer)
                        {
                            if !runtime_failed {
                                let _ = inner
                                    .events_tx
                                    .unbounded_send(GhosttyUiEvent::RuntimeFailed(error));
                            }
                            runtime_failed = true;
                        }
                    }
                    Err(error) => reject_input(&inner, "paste", error),
                }
                notify_command_capacity(&inner);
            }
            Ok(Some(RuntimeMessage::Resize(command))) => {
                let size = command.size;
                #[cfg(unix)]
                let resize_allowed = true;
                #[cfg(target_os = "windows")]
                let resize_allowed = lifecycle.is_running();
                if !resize_allowed {
                    complete_resize_during_drain(&inner);
                } else {
                    let resized = window_size(size)
                        .map_err(|error| error.to_string())
                        .and_then(|ghostty_size| {
                            terminal
                                .resize(ghostty_size)
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|()| {
                            if command.clear_initial {
                                terminal
                                    .clear_screen_and_scrollback()
                                    .map_err(|error| error.to_string())?;
                            }
                            Ok(())
                        })
                        .and_then(|()| {
                            #[cfg(unix)]
                            let active_master = Some(master.as_ref());
                            #[cfg(target_os = "windows")]
                            let active_master = master.get().map(|master| master.as_ref());
                            active_master
                                .ok_or_else(|| {
                                    "Ghostty PTY master closed during resize".to_owned()
                                })?
                                .resize(pty_size(size))
                                .map_err(|error| error.to_string())
                        })
                        .and_then(|()| publish_gate.publish_now(&inner, &mut terminal));
                    let resize_succeeded = match resized {
                        Ok(()) => true,
                        Err(error) => {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty resize to {}x{} failed: {error}",
                                size.cols,
                                size.rows,
                            );
                            false
                        }
                    };
                    complete_resize(&inner, command, resize_succeeded);
                }
            }
            #[cfg(test)]
            Ok(Some(RuntimeMessage::SimulateWorkerCrash)) => {
                panic!("Ghostty runtime worker failure injected for test");
            }
            Ok(Some(RuntimeMessage::Shutdown)) => {
                #[cfg(unix)]
                {
                    if exit.is_none() {
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                }
                #[cfg(target_os = "windows")]
                {
                    shutdown_requested = true;
                }
            }
            Ok(None) | Ok(Some(_)) => {}
            Err(MailboxRecvError::Disconnected) => {
                #[cfg(unix)]
                {
                    if exit.is_none() {
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                    eof = true;
                }
                #[cfg(target_os = "windows")]
                {
                    lifecycle.record_eof();
                    shutdown_requested = true;
                }
            }
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

        #[cfg(unix)]
        {
            if runtime_failed && exit.is_none() {
                inner.shutdown_sent.store(true, Ordering::Release);
                stop_session_input(&inner);
                drop(writer.take());
                terminate_child(child.child_mut(), termination_target);
                child_cleaned = true;
                exit_seen_at = Some(Instant::now());
                exit = Some(portable_pty::ExitStatus::with_exit_code(u32::MAX));
            }

            if exit.is_none() {
                match observe_child_exit(child.child_mut(), child_pid) {
                    Ok(Some(status)) => {
                        exit_seen_at = Some(Instant::now());
                        exit = Some(status);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = inner
                            .events_tx
                            .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                                "Ghostty child wait failed: {error}"
                            )));
                        terminate_child(child.child_mut(), termination_target);
                        child.disarm();
                        break;
                    }
                }
            }
            if let Some(status) = &exit
                && (eof
                    || (exit_seen_at.is_some_and(|seen| seen.elapsed() >= FINAL_DRAIN_TIMEOUT)
                        && mailbox.pending_output_count() == 0))
            {
                if recent_output_pending {
                    publish_recent_output_lines(
                        &inner,
                        &service_output_tail,
                        &mut recent_output_pending,
                    );
                    queue_service_output_ready(&inner);
                }
                let _ = publish_gate.publish_now(&inner, &mut terminal);
                let code = i32::try_from(status.exit_code()).unwrap_or(-1);
                let signal = status.signal().map(str::to_owned);
                if !child_cleaned {
                    terminate_child(child.child_mut(), termination_target);
                }
                child.disarm();
                publish_child_exit_once(&inner, code, signal);
                break;
            }
        }

        #[cfg(target_os = "windows")]
        {
            if runtime_failed && lifecycle.is_running() {
                let _ = publish_gate.publish_now(&inner, &mut terminal);
                shutdown_requested = true;
            }
            if lifecycle.is_running() {
                match observe_windows_child_exit(child.child_mut()) {
                    Ok(Some(exit)) => {
                        child_reaped = true;
                        inner.shutdown_sent.store(true, Ordering::Release);
                        stop_session_input(&inner);
                        let started = Instant::now();
                        let deadline = started
                            .checked_add(
                                super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET,
                            )
                            .unwrap_or(started);
                        let tree =
                            super::pty_session::terminate_windows_process_tree(child_pid, deadline);
                        if tree.failures > 0 || tree.timed_out > 0 || tree.deadline_exhausted {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty Windows descendant cleanup incomplete (targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
                                tree.targeted,
                                tree.terminate_requested,
                                tree.already_exited,
                                tree.failures,
                                tree.timed_out,
                                tree.deadline_exhausted,
                            );
                        }
                        if !close_pty_for_final_drain(&mut writer, &mut master) {
                            let _ = inner
                                .events_tx
                                .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                                    "Ghostty ConPTY close worker disconnected".to_owned(),
                                ));
                        }
                        lifecycle.start_draining(exit, Instant::now());
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !child_wait_failure_reported {
                            let _ = inner
                                .events_tx
                                .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                                    "Ghostty child wait failed: {error}"
                                )));
                            child_wait_failure_reported = true;
                        }
                        shutdown_requested = true;
                    }
                }
            }
            if lifecycle.is_running() && lifecycle.eof {
                shutdown_requested = true;
            }
            if shutdown_requested && lifecycle.is_running() {
                begin_windows_shutdown(
                    &inner,
                    &mut writer,
                    child.child_mut(),
                    child_pid,
                    &mut lifecycle,
                    &mut child_reaped,
                    &mut master,
                );
            }
            if !child_reaped && !lifecycle.is_running() {
                match observe_windows_child_exit(child.child_mut()) {
                    Ok(Some(exit)) => {
                        child_reaped = true;
                        lifecycle.replace_exit(exit);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        if !child_wait_failure_reported {
                            log::warn!(
                                target: "paneflow::terminal::ghostty",
                                "Ghostty Windows child reap failed (kind={:?}, os_error={:?})",
                                error.kind(),
                                error.raw_os_error(),
                            );
                            child_wait_failure_reported = true;
                        }
                    }
                }
            }
            let now = Instant::now();
            if lifecycle.drain_deadline_reached(now) {
                mailbox.stop_accepting_output();
                lifecycle.seal_output();
                let _ = inner
                    .events_tx
                    .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                        "Ghostty final drain timed out before PTY EOF".to_owned(),
                    ));
            }
            if lifecycle.eof {
                let closer_deadline = Instant::now()
                    .checked_add(Duration::from_millis(100))
                    .unwrap_or_else(Instant::now);
                if !master.join_until(closer_deadline) {
                    continue;
                }
                if let Some(worker) = reader_worker.take()
                    && worker.join().is_err()
                {
                    let _ = inner
                        .events_tx
                        .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                            "Ghostty PTY reader terminated unexpectedly".to_owned(),
                        ));
                }
            }
            if let Some(exit) = lifecycle.take_ready_exit(now, mailbox.pending_output_count()) {
                if recent_output_pending {
                    publish_recent_output_lines(
                        &inner,
                        &service_output_tail,
                        &mut recent_output_pending,
                    );
                    queue_service_output_ready(&inner);
                }
                let _ = publish_gate.publish_now(&inner, &mut terminal);
                if child_reaped {
                    child.disarm();
                }
                drop(child);
                drop(master);
                publish_child_exit_once(&inner, exit.code, exit.signal);
                return;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn process_output_batch(
    inner: &SessionInner,
    mailbox: &RuntimeMailbox,
    terminal: &mut ghostty::DisplayTerminal,
    writer: &mut Option<Box<dyn Write + Send>>,
    marks_scanner: &mut Osc133Scanner,
    service_output_tail: &mut ServiceOutputTail,
    last_recent_output_refresh: &mut Option<Instant>,
    recent_output_pending: &mut bool,
    gate: &mut PublishGate,
    first: Vec<u8>,
) -> Result<(), String> {
    let started = Instant::now();
    let mut processed_bytes = 0usize;
    let mut chunks = Vec::with_capacity(OUTPUT_BUFFER_COUNT);
    let mut raw_marks = Vec::new();
    let mut next = Some(first);

    let result = (|| {
        while let Some(bytes) = next.take() {
            processed_bytes = processed_bytes.saturating_add(bytes.len());
            chunks.push(bytes);
            let Some(bytes) = chunks.last() else {
                return Err("Ghostty output batch lost its current chunk".into());
            };
            terminal
                .feed(bytes)
                .map_err(|error| format!("Ghostty VT feed failed: {error}"))?;
            service_output_tail.advance(bytes);
            let emitted_mark = scan_chunk_for_marks(marks_scanner, bytes, &mut raw_marks);
            handle_engine_events(inner, terminal, writer)?;
            #[cfg(test)]
            inner
                .processed_output_bytes
                .fetch_add(bytes.len(), Ordering::AcqRel);

            if emitted_mark
                || inner.shutdown_sent.load(Ordering::Acquire)
                || processed_bytes >= OUTPUT_BATCH_MAX_BYTES
                || started.elapsed() >= OUTPUT_BATCH_MAX_TIME
            {
                break;
            }
            next = mailbox.try_recv_consecutive_output();
        }

        *recent_output_pending = true;
        let service_output_ready = refresh_recent_output_lines(
            inner,
            service_output_tail,
            last_recent_output_refresh,
            recent_output_pending,
        );
        record_command_marks(inner, &raw_marks);
        gate.request(inner, terminal)?;
        if service_output_ready {
            queue_service_output_ready(inner);
        }
        Ok(())
    })();

    for bytes in chunks {
        mailbox.recycle_output_buffer(bytes);
    }
    result
}

#[cfg(test)]
fn read_pty(mut reader: Box<dyn Read + Send>, mailbox: Arc<RuntimeMailbox>) {
    loop {
        let Some(mut buffer) = mailbox.take_output_buffer() else {
            return;
        };
        match reader.read(&mut buffer) {
            Ok(0) => {
                mailbox.recycle_output_buffer(buffer);
                break;
            }
            Ok(read) => {
                buffer.truncate(read);
                if !mailbox.send_output(buffer) {
                    return;
                }
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {
                mailbox.recycle_output_buffer(buffer);
                continue;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                mailbox.recycle_output_buffer(buffer);
                std::thread::yield_now();
            }
            Err(_) => {
                mailbox.recycle_output_buffer(buffer);
                break;
            }
        }
    }
    mailbox.send_eof();
}

#[cfg(all(test, unix))]
type ChildTerminationTarget = Option<i32>;

#[cfg(all(test, target_os = "windows"))]
type ChildTerminationTarget = u32;

#[cfg(all(test, unix))]
fn child_termination_target(child_pid: u32) -> ChildTerminationTarget {
    verified_process_group(child_pid)
}

#[cfg(all(test, target_os = "windows"))]
fn child_termination_target(child_pid: u32) -> ChildTerminationTarget {
    child_pid
}

#[cfg(all(test, unix))]
use paneflow_host::process::verified_process_group;

#[cfg(all(test, unix))]
fn observe_child_exit(
    _child: &mut dyn portable_pty::Child,
    child_pid: u32,
) -> std::io::Result<Option<portable_pty::ExitStatus>> {
    let pid = i32::try_from(child_pid)
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "child PID unavailable"))?;
    let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            info.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let info = unsafe { info.assume_init() };
    let observed_pid = unsafe { info.si_pid() };
    if observed_pid == 0 {
        return Ok(None);
    }
    let status = unsafe { info.si_status() };
    let exit = match info.si_code {
        libc::CLD_EXITED => portable_pty::ExitStatus::with_exit_code(status.max(0) as u32),
        libc::CLD_KILLED | libc::CLD_DUMPED => {
            let signal = unsafe { libc::strsignal(status) };
            let signal = if signal.is_null() {
                format!("Signal {status}")
            } else {
                unsafe { std::ffi::CStr::from_ptr(signal) }
                    .to_string_lossy()
                    .into_owned()
            };
            portable_pty::ExitStatus::with_signal(&signal)
        }
        code => {
            return Err(std::io::Error::other(format!(
                "unexpected waitid child state {code}"
            )));
        }
    };
    Ok(Some(exit))
}

#[cfg(all(test, target_os = "windows"))]
fn child_exit_report(status: &portable_pty::ExitStatus) -> ChildExitReport {
    ChildExitReport {
        code: i32::try_from(status.exit_code()).unwrap_or(-1),
        signal: status.signal().map(str::to_owned),
    }
}

#[cfg(all(test, target_os = "windows"))]
fn observe_windows_child_exit(
    child: &mut dyn portable_pty::Child,
) -> std::io::Result<Option<ChildExitReport>> {
    let Some(observed) = child.try_wait()? else {
        return Ok(None);
    };
    let exit = match child.wait() {
        Ok(waited) => child_exit_report(&waited),
        Err(error) => {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty Windows child wait after observed exit failed (kind={:?}, os_error={:?})",
                error.kind(),
                error.raw_os_error(),
            );
            child_exit_report(&observed)
        }
    };
    Ok(Some(exit))
}

#[cfg(all(test, unix))]
fn terminate_child(child: &mut dyn portable_pty::Child, process_group_id: ChildTerminationTarget) {
    if let Some(pid) = process_group_id {
        unsafe {
            libc::kill(-pid, libc::SIGTERM);
        }
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        while Instant::now() < deadline {
            let group_exists = unsafe { libc::kill(-pid, 0) == 0 }
                || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
            if !group_exists {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.kill();
        let _ = child.wait();
        return;
    }
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while Instant::now() < deadline {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(all(test, target_os = "windows"))]
struct WindowsChildTerminationOutcome {
    exit: ChildExitReport,
    reaped: bool,
}

#[cfg(all(test, target_os = "windows"))]
fn terminate_windows_child_until(
    child: &mut dyn portable_pty::Child,
    child_pid: u32,
    deadline: Instant,
) -> WindowsChildTerminationOutcome {
    let tree = super::pty_session::terminate_windows_process_tree(child_pid, deadline);
    match observe_windows_child_exit(child) {
        Ok(Some(exit)) => return WindowsChildTerminationOutcome { exit, reaped: true },
        Ok(None) => {}
        Err(error) => log::warn!(
            target: "paneflow::terminal::ghostty",
            "Ghostty Windows child pre-kill observation failed (kind={:?}, os_error={:?})",
            error.kind(),
            error.raw_os_error(),
        ),
    }

    if let Err(error) = child.kill() {
        log::warn!(
            target: "paneflow::terminal::ghostty",
            "Ghostty Windows portable child kill failed (kind={:?}, os_error={:?})",
            error.kind(),
            error.raw_os_error(),
        );
    }
    loop {
        match observe_windows_child_exit(child) {
            Ok(Some(exit)) => return WindowsChildTerminationOutcome { exit, reaped: true },
            Ok(None) => {}
            Err(error) => {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty Windows child post-kill observation failed (kind={:?}, os_error={:?})",
                    error.kind(),
                    error.raw_os_error(),
                );
                break;
            }
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        std::thread::sleep(remaining.min(WINDOWS_CHILD_POLL_INTERVAL));
    }
    log::warn!(
        target: "paneflow::terminal::ghostty",
        "Ghostty Windows child cleanup reached its deadline (targeted={}, terminate_requested={}, already_exited={}, failures={}, timed_out={}, deadline_exhausted={})",
        tree.targeted,
        tree.terminate_requested,
        tree.already_exited,
        tree.failures,
        tree.timed_out,
        tree.deadline_exhausted,
    );
    WindowsChildTerminationOutcome {
        exit: ChildExitReport {
            code: -1,
            signal: None,
        },
        reaped: false,
    }
}

#[cfg(all(test, target_os = "windows"))]
fn begin_windows_shutdown(
    inner: &SessionInner,
    writer: &mut Option<Box<dyn Write + Send>>,
    child: &mut dyn portable_pty::Child,
    child_pid: u32,
    lifecycle: &mut RuntimeLifecycle,
    child_reaped: &mut bool,
    master: &mut DrainablePtyMaster<Box<dyn portable_pty::MasterPty + Send>>,
) {
    if !lifecycle.is_running() {
        return;
    }
    inner.shutdown_sent.store(true, Ordering::Release);
    stop_session_input(inner);
    let started = Instant::now();
    let deadline = started
        .checked_add(super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET)
        .unwrap_or(started);
    let outcome = terminate_windows_child_until(child, child_pid, deadline);
    if !close_pty_for_final_drain(writer, master) {
        let _ = inner
            .events_tx
            .unbounded_send(GhosttyUiEvent::RuntimeFailed(
                "Ghostty ConPTY close worker disconnected".to_owned(),
            ));
    }
    *child_reaped = outcome.reaped;
    lifecycle.start_draining(outcome.exit, Instant::now());
}

#[cfg(all(test, target_os = "windows"))]
fn terminate_child(child: &mut dyn portable_pty::Child, child_pid: ChildTerminationTarget) {
    let started = Instant::now();
    let deadline = started
        .checked_add(super::pty_session::WINDOWS_PROCESS_TREE_TERMINATION_BUDGET)
        .unwrap_or(started);
    let _ = terminate_windows_child_until(child, child_pid, deadline);
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_config::schema::TerminalSurfaceProfile;

    #[cfg(target_os = "windows")]
    fn windows_executable(name: &str) -> Option<String> {
        let output = std::process::Command::new("where.exe")
            .arg(name)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .find(|path| !path.is_empty())
            .map(str::to_owned)
    }

    #[cfg(target_os = "windows")]
    fn wsl_has_distribution() -> bool {
        let Some(wsl) = windows_executable("wsl.exe") else {
            return false;
        };
        std::process::Command::new(wsl)
            .args(["--list", "--quiet"])
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && output
                        .stdout
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .any(|pair| u16::from_le_bytes(*pair) > 0x20)
            })
    }

    #[cfg(target_os = "windows")]
    fn run_windows_shell_case(
        name: &str,
        shell: String,
        shell_quoting: super::super::types::ShellQuoting,
        extra_args: Vec<String>,
        cwd: &std::path::Path,
    ) -> String {
        let params = SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env: std::collections::HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
                ("PANEFLOW_MATRIX".into(), "matrix-é中".into()),
                (
                    "WSLENV".into(),
                    "PANEFLOW_MATRIX/u:TERM/u:COLORTERM/u:TERM_PROGRAM/u".into(),
                ),
            ]),
            cwd: cwd.to_path_buf(),
            cols: 100,
            rows: 30,
            profile: TerminalSurfaceProfile::Normal,
        };
        let (session, pending, mut events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(100, 30, 8, 16));
        let spawned = session
            .start(pending, params, 1_000)
            .unwrap_or_else(|error| panic!("{name} must spawn through ConPTY: {error}"));
        assert!(spawned.child_pid > 0, "{name} child PID");
        session.promote();
        session.resize(TerminalWindowSize::new(120, 36, 8, 16));

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut exit = None;
        let mut failures = Vec::new();
        while Instant::now() < deadline {
            while let Ok(event) = events_rx.try_recv() {
                match event {
                    GhosttyUiEvent::ChildExited { code, .. } => exit = Some(code),
                    GhosttyUiEvent::RuntimeFailed(error) => failures.push(error),
                    _ => {}
                }
            }
            if exit.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(exit, Some(0), "{name} exit; failures={failures:?}");
        assert!(failures.is_empty(), "{name} runtime failures: {failures:?}");
        let (content, _) =
            session.render_content(TerminalWindowSize::new(120, 36, 8, 16), -100, 100, false);
        content.cells.iter().map(|cell| cell.c).collect()
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_shell_matrix_preserves_unicode_environment_and_cwd() {
        use super::super::types::ShellQuoting;

        let cwd = tempfile::Builder::new()
            .prefix("paneflow shell é ")
            .tempdir()
            .expect("create Unicode shell-matrix cwd");
        let mut cases = Vec::new();
        cases.push((
            "cmd",
            windows_executable("cmd.exe").expect("cmd.exe is required on Windows"),
            ShellQuoting::Cmd,
            vec![
                "/D".into(),
                "/Q".into(),
                "/C".into(),
                "chcp 65001>nul & echo PANEFLOW_SHELL:cmd:%PANEFLOW_MATRIX% & echo %CD% & echo \x1b[38;2;1;2;3mCOLOR\x1b[0m"
                    .into(),
            ],
        ));
        cases.push((
            "powershell",
            windows_executable("powershell.exe")
                .expect("Windows PowerShell 5.1 is required on Windows"),
            ShellQuoting::PowerShell,
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "[Console]::OutputEncoding=[Text.UTF8Encoding]::new(); Write-Output \"PANEFLOW_SHELL:powershell:$env:PANEFLOW_MATRIX\"; Write-Output (Get-Location).Path; Write-Output \"$([char]27)[38;2;1;2;3mCOLOR$([char]27)[0m\""
                    .into(),
            ],
        ));
        cases.push((
            "pwsh",
            windows_executable("pwsh.exe").expect("PowerShell 7 is required on Windows CI"),
            ShellQuoting::PowerShell,
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                "Write-Output \"PANEFLOW_SHELL:pwsh:$env:PANEFLOW_MATRIX\"; Write-Output (Get-Location).Path; Write-Output \"$([char]27)[38;2;1;2;3mCOLOR$([char]27)[0m\""
                    .into(),
            ],
        ));
        let git_bash = std::path::PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
        assert!(git_bash.is_file(), "Git Bash is required on Windows CI");
        cases.push((
            "git-bash",
            git_bash.to_string_lossy().into_owned(),
            ShellQuoting::Posix,
            vec![
                "--noprofile".into(),
                "--norc".into(),
                "-lc".into(),
                "printf 'PANEFLOW_SHELL:git-bash:%s\\n%s\\n\\033[38;2;1;2;3mCOLOR\\033[0m\\n' \"$PANEFLOW_MATRIX\" \"$PWD\""
                    .into(),
            ],
        ));
        if wsl_has_distribution() {
            cases.push((
                "wsl",
                windows_executable("wsl.exe").expect("wsl.exe was detected"),
                ShellQuoting::Posix,
                vec![
                    "--cd".into(),
                    cwd.path().to_string_lossy().into_owned(),
                    "--exec".into(),
                    "sh".into(),
                    "-lc".into(),
                    "printf 'PANEFLOW_SHELL:wsl:%s\\n%s\\n\\033[38;2;1;2;3mCOLOR\\033[0m\\n' \"$PANEFLOW_MATRIX\" \"$PWD\""
                        .into(),
                ],
            ));
        }

        for (name, shell, quoting, args) in cases {
            let rendered = run_windows_shell_case(name, shell, quoting, args, cwd.path());
            assert!(
                rendered.contains(&format!("PANEFLOW_SHELL:{name}:matrix-é中")),
                "{name} lost Unicode or env propagation: {rendered:?}"
            );
            assert!(
                rendered.contains("COLOR"),
                "{name} lost truecolor output: {rendered:?}"
            );
            assert!(
                rendered.contains("paneflow shell é"),
                "{name} lost cwd with spaces/non-ASCII: {rendered:?}"
            );
        }
    }

    #[test]
    fn live_runtime_runs_platform_shell_and_reports_one_exit() {
        let cwd = std::env::current_dir().unwrap();
        #[cfg(unix)]
        let (shell, shell_quoting, extra_args) = (
            "/bin/sh".into(),
            super::super::types::ShellQuoting::Posix,
            Vec::new(),
        );
        #[cfg(target_os = "windows")]
        let (shell, shell_quoting, extra_args) = (
            "cmd.exe".into(),
            super::super::types::ShellQuoting::Cmd,
            vec!["/D".into(), "/Q".into()],
        );
        let params = SpawnParams {
            shell,
            shell_quoting,
            extra_args,
            env: std::collections::HashMap::from([
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "paneflow".into()),
            ]),
            cwd,
            cols: 80,
            rows: 24,
            profile: TerminalSurfaceProfile::Normal,
        };
        let (session, pending, mut events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let spawned = session
            .start(pending, params, 1_000)
            .expect("Ghostty runtime must spawn a portable PTY shell");
        assert!(spawned.child_pid > 0);
        #[cfg(unix)]
        let child_pid = spawned.child_pid;
        session.promote();
        #[cfg(target_os = "windows")]
        {
            assert!(
                session
                    .write(b"echo PANEFLOW_CONPTY^_READY\r\n".to_vec())
                    .is_sent()
            );
            let ready_deadline = Instant::now() + Duration::from_secs(5);
            let mut child_ready = false;
            while Instant::now() < ready_deadline {
                if session
                    .recent_output_lines()
                    .iter()
                    .any(|line| line.contains("PANEFLOW_CONPTY_READY"))
                {
                    child_ready = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert!(
                child_ready,
                "ConPTY child must process input before the resize probe"
            );
        }
        session.resize(TerminalWindowSize::new(100, 30, 8, 16));
        #[cfg(unix)]
        let command =
            b"printf 'PANEFLOW_GHOSTTY_RUNTIME_OK:%s\\n' \"$TERM_PROGRAM\"; stty size; exit\n"
                .to_vec();
        #[cfg(target_os = "windows")]
        let command = {
            let mut command = br#"powershell.exe -NoLogo -NoProfile -NonInteractive -Command "$deadline = [DateTime]::UtcNow.AddSeconds(3); do { $height = [Console]::WindowHeight; $width = [Console]::WindowWidth; if ($height -eq 30 -and $width -eq 100) { break }; Start-Sleep -Milliseconds 20 } while ([DateTime]::UtcNow -lt $deadline); Write-Output ('PANEFLOW_GHOSTTY_RUNTIME_OK:' + $env:TERM_PROGRAM); Write-Output ('PANEFLOW_SIZE:' + $height + 'x' + $width)" & exit"#.to_vec();
            command.extend_from_slice(b"\r\n");
            command
        };
        assert!(session.write(command).is_sent());

        let deadline = Instant::now() + Duration::from_secs(8);
        let mut exits = 0;
        let mut runtime_failures = Vec::new();
        while Instant::now() < deadline {
            while let Ok(event) = events_rx.try_recv() {
                match event {
                    GhosttyUiEvent::ChildExited { .. } => exits += 1,
                    GhosttyUiEvent::RuntimeFailed(error) => runtime_failures.push(error),
                    _ => {}
                }
            }
            if exits > 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(50));
        while let Ok(event) = events_rx.try_recv() {
            match event {
                GhosttyUiEvent::ChildExited { .. } => exits += 1,
                GhosttyUiEvent::RuntimeFailed(error) => runtime_failures.push(error),
                _ => {}
            }
        }

        let (content, _) =
            session.render_content(TerminalWindowSize::new(100, 30, 8, 16), -100, 100, false);
        let rendered: String = content.cells.iter().map(|cell| cell.c).collect();
        assert!(
            rendered.contains("PANEFLOW_GHOSTTY_RUNTIME_OK:ghostty"),
            "Ghostty runtime must identify itself to terminal applications; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        #[cfg(unix)]
        assert!(
            rendered.contains("30 100"),
            "resize must reach the child PTY; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        #[cfg(target_os = "windows")]
        assert!(
            rendered.contains("PANEFLOW_SIZE:30x100"),
            "resize must reach ConPTY; rendered={rendered:?}; runtime_failures={runtime_failures:?}"
        );
        assert_eq!(exits, 1, "child exit must be published exactly once");
        #[cfg(unix)]
        {
            assert_eq!(unsafe { libc::kill(child_pid as i32, 0) }, -1);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    #[test]
    fn stopping_input_discards_queued_bytes_and_rejects_new_input() {
        let mailbox = RuntimeMailbox::new();
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"first".to_vec()))
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::ClearSelection)
                .is_ok()
        );
        assert!(
            mailbox
                .try_send_control(RuntimeMessage::Input(b"second".to_vec()))
                .is_ok()
        );

        assert_eq!(mailbox.stop_accepting_input(), 11);
        assert!(matches!(
            mailbox.try_send_control(RuntimeMessage::Input(b"late".to_vec())),
            Err(TrySendError::Disconnected(RuntimeMessage::Input(bytes))) if bytes == b"late"
        ));
        assert!(matches!(
            mailbox.drain().as_slice(),
            [RuntimeMessage::ClearSelection]
        ));
    }

    #[test]
    fn simulated_worker_crash_is_admitted_once_and_rejected_after_shutdown() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        assert!(session.simulate_worker_crash_for_test());
        assert!(!session.simulate_worker_crash_for_test());
        assert!(matches!(
            pending.mailbox.try_recv(),
            Ok(RuntimeMessage::SimulateWorkerCrash)
        ));

        let (shutdown_session, shutdown_pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        shutdown_session.shutdown();
        assert!(!shutdown_session.simulate_worker_crash_for_test());
        assert!(matches!(
            shutdown_pending.mailbox.try_recv(),
            Ok(RuntimeMessage::Shutdown)
        ));
    }

    #[test]
    fn lifecycle_publishes_once_after_eof() {
        let now = Instant::now();
        let exit = ChildExitReport {
            code: 7,
            signal: None,
        };
        let mut lifecycle = RuntimeLifecycle::new();

        assert!(lifecycle.start_draining(exit.clone(), now));
        assert!(!lifecycle.start_draining(
            ChildExitReport {
                code: 99,
                signal: None,
            },
            now,
        ));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
        lifecycle.record_eof();
        assert_eq!(lifecycle.take_ready_exit(now, 1), None);
        assert_eq!(lifecycle.take_ready_exit(now, 0), Some(exit));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
    }

    #[test]
    fn lifecycle_deadline_and_early_eof_converge() {
        let now = Instant::now();
        let deadline = now.checked_add(FINAL_DRAIN_TIMEOUT).unwrap_or(now);
        let mut timed = RuntimeLifecycle::new();
        assert!(timed.start_draining(
            ChildExitReport {
                code: -1,
                signal: None,
            },
            now,
        ));
        assert_eq!(timed.take_ready_exit(now, 0), None);
        assert!(timed.drain_deadline_reached(deadline));
        assert_eq!(timed.take_ready_exit(deadline, 0), None);
        timed.seal_output();
        assert_eq!(timed.take_ready_exit(deadline, 1), None);
        assert_eq!(
            timed.take_ready_exit(deadline, 0),
            Some(ChildExitReport {
                code: -1,
                signal: None,
            })
        );

        let mut eof_first = RuntimeLifecycle::new();
        eof_first.record_eof();
        assert!(eof_first.start_draining(
            ChildExitReport {
                code: 0,
                signal: None,
            },
            now,
        ));
        assert_eq!(
            eof_first.take_ready_exit(now, 0),
            Some(ChildExitReport {
                code: 0,
                signal: None,
            })
        );
    }

    #[test]
    fn final_drain_closes_writer_and_master_before_reader_eof() {
        struct DropProbe {
            dropped: Arc<AtomicBool>,
        }

        impl Drop for DropProbe {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }

        let writer_dropped = Arc::new(AtomicBool::new(false));
        let master_dropped = Arc::new(AtomicBool::new(false));
        let mut writer = Some(DropProbe {
            dropped: writer_dropped.clone(),
        });
        let closer = PtyCloser::new("paneflow-ghostty-test-pty-closer")
            .expect("test closer thread must start");
        let mut master = DrainablePtyMaster::new(
            DropProbe {
                dropped: master_dropped.clone(),
            },
            closer,
        );
        assert!(close_pty_for_final_drain(&mut writer, &mut master));
        assert!(writer_dropped.load(Ordering::Acquire));
        assert!(master.join_until(Instant::now() + Duration::from_secs(1)));
        assert!(master_dropped.load(Ordering::Acquire));

        let now = Instant::now();
        let mut lifecycle = RuntimeLifecycle::new();
        assert!(lifecycle.start_draining(
            ChildExitReport {
                code: 0,
                signal: None,
            },
            now,
        ));
        assert_eq!(lifecycle.take_ready_exit(now, 0), None);
        lifecycle.record_eof();
        assert!(lifecycle.take_ready_exit(now, 0).is_some());
    }

    #[cfg(unix)]
    fn spawn_posix_lifecycle_probe(
        script: &str,
    ) -> (
        Box<dyn portable_pty::MasterPty + Send>,
        Box<dyn portable_pty::Child + Send + Sync>,
        u32,
    ) {
        let pair = paneflow_host::pty::open(pty_size(TerminalWindowSize::new(80, 24, 8, 16)))
            .expect("POSIX lifecycle probe must open a PTY");
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.cwd(std::env::current_dir().expect("probe cwd must resolve"));
        let child = pair
            .slave
            .spawn_command(command)
            .expect("POSIX lifecycle probe must spawn /bin/sh");
        drop(pair.slave);
        let pid = child
            .process_id()
            .expect("POSIX lifecycle probe must report a child PID");
        (pair.master, child, pid)
    }

    #[cfg(unix)]
    fn probe_pid(pid: u32) -> i32 {
        i32::try_from(pid).expect("a probe PID must fit in pid_t")
    }

    #[cfg(unix)]
    fn observe_probe_exit(
        child: &mut (dyn portable_pty::Child + Send + Sync),
        pid: u32,
    ) -> portable_pty::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match observe_child_exit(child, pid) {
                Ok(Some(status)) => return status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("waitid failed for probe {pid}: {error}"),
            }
        }
        panic!("probe {pid} never reported an exit status");
    }

    #[cfg(unix)]
    #[test]
    fn posix_process_group_helper_authenticates_the_session_leader() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("exec sleep 30");
        let expected = probe_pid(pid);
        assert_eq!(
            verified_process_group(pid),
            Some(expected),
            "portable-pty must spawn the child as its own process-group leader"
        );
        assert_eq!(child_termination_target(pid), Some(expected));
        terminate_child(child.as_mut(), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn waitid_probe_is_non_blocking_and_leaves_the_exit_status_unconsumed() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("sleep 0.3; exit 7");
        let started = Instant::now();
        let pending =
            observe_child_exit(child.as_mut(), pid).expect("waitid must succeed for a live child");
        assert!(
            pending.is_none(),
            "a still-running child must not report an exit status"
        );
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "WNOHANG must return immediately instead of waiting for the child"
        );

        let exit = observe_probe_exit(child.as_mut(), pid);
        assert_eq!(exit.exit_code(), 7, "CLD_EXITED must carry the exit code");
        assert!(exit.signal().is_none());

        let again = observe_probe_exit(child.as_mut(), pid);
        assert_eq!(
            again.exit_code(),
            7,
            "WNOWAIT must not consume the exit status"
        );
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn waitid_probe_maps_a_killed_child_to_a_named_signal() {
        let (_master, mut child, pid) = spawn_posix_lifecycle_probe("kill -KILL $$; sleep 30");
        let exit = observe_probe_exit(child.as_mut(), pid);
        let signal = exit
            .signal()
            .expect("CLD_KILLED must be reported as a signal, not an exit code");
        assert!(!signal.is_empty());
        assert!(
            !signal.starts_with("Signal "),
            "strsignal must name the signal; {signal:?} is the null-pointer fallback"
        );
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn shutdown_escalates_to_sigkill_for_a_child_that_ignores_sigterm() {
        let (_master, mut child, pid) =
            spawn_posix_lifecycle_probe("trap '' TERM; while :; do sleep 0.05; done");
        let group = verified_process_group(pid).expect("probe must lead its own process group");
        let started = Instant::now();
        terminate_child(child.as_mut(), Some(group));
        assert!(
            started.elapsed() <= SHUTDOWN_GRACE + Duration::from_secs(1),
            "SIGKILL must land within SHUTDOWN_GRACE plus one second"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut group_error = None;
        while Instant::now() < deadline {
            if unsafe { libc::kill(-group, 0) } == -1 {
                group_error = std::io::Error::last_os_error().raw_os_error();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            group_error,
            Some(libc::ESRCH),
            "the whole process group must be gone after the SIGKILL escalation"
        );
    }
}
