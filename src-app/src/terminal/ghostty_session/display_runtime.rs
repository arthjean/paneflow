use super::*;

fn feed_display_output(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    gate: &mut PublishGate,
    bytes: &[u8],
) -> Result<(), String> {
    terminal
        .feed(bytes)
        .map_err(|error| format!("Ghostty VT feed failed: {error}"))?;
    handle_engine_events(inner, terminal, &mut None)?;
    gate.publish_now(inner, terminal)
}

pub(super) fn run_display_runtime(
    inner: Arc<SessionInner>,
    mailbox: Arc<RuntimeMailbox>,
    max_scrollback: usize,
    startup_tx: SyncSender<Result<(), String>>,
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
            let _ = startup_tx.send(Err(error.to_string()));
            return;
        }
    };
    let appearance = current_ghostty_appearance();
    let mut terminal = match ghostty::DisplayTerminal::new(ghostty_size, max_scrollback, appearance)
    {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = startup_tx.send(Err(error.to_string()));
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
        let _ = startup_tx.send(Err(error));
        return;
    }
    if startup_tx.send(Ok(())).is_err() {
        return;
    }

    loop {
        count_runtime_loop_iteration();
        if let Err(error) = publish_gate.poll(&inner, &mut terminal) {
            log::warn!(target: "paneflow::terminal::ghostty", "Ghostty display publication failed: {error}");
        }
        let wait = publish_gate
            .next_wake(Instant::now())
            .map(|wake| wake.clamp(Duration::from_millis(1), DISPLAY_RUNTIME_TICK))
            .unwrap_or(DISPLAY_RUNTIME_TICK);
        let message = match mailbox.recv_timeout(wait) {
            Ok(message) => message,
            Err(MailboxRecvError::Timeout) => continue,
            Err(MailboxRecvError::Disconnected) => break,
        };
        let CommandOutcome::Unhandled(message) =
            handle_terminal_command(&inner, &mut terminal, &mut publish_gate, message)
        else {
            continue;
        };
        match message {
            RuntimeMessage::WriteOutput { bytes, reply } => {
                if let Err(error) =
                    feed_display_output(&inner, &mut terminal, &mut publish_gate, &bytes)
                {
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty display feed failed: {error}"
                    );
                }
                let _ = reply.send(());
            }
            RuntimeMessage::Resize(command) => {
                let size = command.size;
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
                        publish_gate.publish_now(&inner, &mut terminal)
                    });
                if let Err(error) = &resized {
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty display resize to {}x{} failed: {error}",
                        size.cols,
                        size.rows,
                    );
                }
                complete_resize(&inner, command, resized.is_ok());
            }
            RuntimeMessage::Shutdown => break,
            other => {
                if let Some(bytes) = other.queued_input_bytes() {
                    release_queued_input_bytes(&inner, bytes);
                    notify_command_capacity(&inner);
                }
            }
        }
    }
}
