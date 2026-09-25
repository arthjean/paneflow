use super::*;

#[derive(Default)]
pub(super) struct BracketedPasteTrace {
    enabled: bool,
    changed_at: Option<Instant>,
}

impl BracketedPasteTrace {
    pub(super) fn observe(&mut self, terminal: &ghostty::DisplayTerminal) {
        let Ok(modes) = terminal.modes() else {
            return;
        };
        if modes.bracketed_paste == self.enabled {
            return;
        }
        self.enabled = modes.bracketed_paste;
        self.changed_at = Some(Instant::now());
        log::debug!(
            target: "paneflow::terminal::ghostty",
            "bracketed paste mode {}",
            if self.enabled { "enabled" } else { "disabled" },
        );
    }

    pub(super) fn note_paste(&self, text_bytes: usize) {
        let since_change = self
            .changed_at
            .map(|at| at.elapsed().as_millis())
            .unwrap_or(0);
        log::debug!(
            target: "paneflow::terminal::ghostty",
            "paste of {text_bytes} bytes with bracketed paste {}, mode last changed {since_change} ms ago",
            if self.enabled { "enabled" } else { "disabled" },
        );
    }
}

pub(super) fn reject_input(
    inner: &SessionInner,
    input_kind: &'static str,
    error: impl std::fmt::Display,
) {
    let _ = inner
        .events_tx
        .unbounded_send(GhosttyUiEvent::InputRejected(format!(
            "Ghostty {input_kind} encoder rejected input: {error}"
        )));
}

#[cfg(test)]
pub(super) fn write_input_bytes<W: Write>(
    inner: &SessionInner,
    writer: &mut Option<W>,
    bytes: &[u8],
    runtime_failed: &mut bool,
) {
    if bytes.is_empty() {
        return;
    }
    let Some(active_writer) = writer.as_mut() else {
        return;
    };
    if let Err(error) = active_writer
        .write_all(bytes)
        .and_then(|()| active_writer.flush())
    {
        let expected_close = matches!(
            error.kind(),
            ErrorKind::BrokenPipe | ErrorKind::NotConnected
        );
        if !expected_close {
            let _ = inner
                .events_tx
                .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                    "Ghostty PTY write failed: {error}"
                )));
        }
        #[cfg(unix)]
        {
            *runtime_failed = !expected_close;
        }
        #[cfg(target_os = "windows")]
        {
            *runtime_failed = true;
        }
    }
}

pub(super) enum CommandOutcome {
    Handled,
    Unhandled(RuntimeMessage),
}

pub(super) fn handle_terminal_command(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    gate: &mut PublishGate,
    message: RuntimeMessage,
) -> CommandOutcome {
    match message {
        RuntimeMessage::Scroll(scroll) => {
            terminal.scroll(scroll);
            if let Err(error) = gate.publish_now(inner, terminal) {
                log::warn!(target: "paneflow::terminal::ghostty", "Ghostty scroll failed: {error}");
            }
        }
        RuntimeMessage::ScrollToViewportRow(row) => {
            let result = terminal
                .scroll_to_viewport_row(row)
                .map_err(|error| error.to_string())
                .and_then(|()| gate.publish_now(inner, terminal));
            if let Err(error) = result {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty absolute scroll failed: {error}"
                );
            }
        }
        RuntimeMessage::PressSelection {
            point,
            behavior,
            position,
        } => {
            let options = ghostty::PressOptions {
                position: Some(position),
                behaviors: Some(ghostty::GestureBehaviors {
                    single_click: behavior,
                    ..ghostty::GestureBehaviors::default()
                }),
                ..ghostty::PressOptions::default()
            };
            match terminal.gesture_press(point, &options) {
                Ok(range) => publish_gesture_selection(inner, range),
                Err(error) => log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection press failed: {error}"
                ),
            }
        }
        RuntimeMessage::DragSelection(generation) => {
            let target = {
                let mut gesture = lock_gesture(inner);
                if gesture.queued_generation == Some(generation) {
                    gesture.queued_generation = None;
                }
                if gesture.generation != generation {
                    None
                } else {
                    let target = gesture.requested.take();
                    gesture.in_flight = target.map(|target| (generation, target));
                    target
                }
            };
            if let Some(target) = target {
                let options = ghostty::DragOptions {
                    position: Some(target.position),
                    rectangle: target.rectangle,
                    word_boundaries: Vec::new(),
                };
                let result = terminal.gesture_drag(target.point, target.geometry, &options);
                let mut gesture = lock_gesture(inner);
                if gesture
                    .in_flight
                    .is_some_and(|(pending, applied)| pending == generation && applied == target)
                {
                    gesture.in_flight = None;
                }
                let publish = gesture.generation == generation;
                if publish && result.is_ok() {
                    gesture.applied = Some(target);
                }
                drop(gesture);
                match result {
                    Ok(range) => {
                        if publish {
                            publish_gesture_selection(inner, range);
                        }
                    }
                    Err(error) => log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty selection drag failed: {error}"
                    ),
                }
            }
        }
        RuntimeMessage::ReleaseSelection { point } => {
            if let Err(error) = terminal.gesture_release(point) {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection release failed: {error}"
                );
            }
            let mut gesture = lock_gesture(inner);
            gesture.in_flight = None;
            gesture.applied = None;
        }
        RuntimeMessage::ClearSelection => {
            if let Err(error) = terminal.gesture_reset() {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection gesture reset failed: {error}"
                );
            }
            match terminal.clear_selection() {
                Ok(()) => {
                    let mut gesture = lock_gesture(inner);
                    gesture.in_flight = None;
                    gesture.applied = None;
                    drop(gesture);
                    update_shared_selection(inner, None);
                }
                Err(error) => log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection clear failed: {error}"
                ),
            }
        }
        RuntimeMessage::ClearScrollback => {
            if let Err(error) = terminal.clear_screen_and_scrollback() {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty scrollback clear failed: {error}"
                );
            }
            let _ = gate.publish_now(inner, terminal);
        }
        RuntimeMessage::SetDefaultCursor { shape, blink } => {
            if let Err(error) = terminal.set_default_cursor(shape, blink) {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty default cursor could not be configured: {error}"
                );
            }
        }
        RuntimeMessage::SetOptionAsAlt(enabled) => {
            terminal.set_option_as_alt(option_as_alt(enabled));
        }
        RuntimeMessage::UpdateAppearance(appearance) => {
            if let Err(error) = terminal.set_palette(&current_ghostty_palette()) {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty color palette could not be updated: {error}"
                );
            }
            if let Err(error) = terminal.set_appearance(appearance) {
                let _ = inner
                    .events_tx
                    .unbounded_send(GhosttyUiEvent::RuntimeFailed(format!(
                        "Ghostty appearance update failed: {error}"
                    )));
            }
        }
        RuntimeMessage::SearchChunk {
            start_row,
            max_cells,
            reply,
        } => {
            let _ = reply.send(
                terminal
                    .search_chunk(start_row, max_cells)
                    .map_err(|error| error.to_string()),
            );
        }
        RuntimeMessage::SetNativeSearch(query) => {
            gate.search_query = query;
            gate.last_search_rail_refresh = None;
            gate.search_error = terminal
                .set_search_query(&gate.search_query)
                .err()
                .map(|error| error.to_string());
            if let Err(error) = gate.publish_now(inner, terminal) {
                log::warn!(target: "paneflow::terminal::ghostty", "Ghostty search publication failed: {error}");
            }
        }
        RuntimeMessage::SelectNativeSearch {
            previous,
            generation,
        } => {
            gate.navigation_generation = generation;
            gate.search_error = terminal
                .search_select(previous)
                .err()
                .map(|error| error.to_string());
            if let Err(error) = gate.publish_now(inner, terminal) {
                log::warn!(target: "paneflow::terminal::ghostty", "Ghostty search navigation publication failed: {error}");
            }
        }
        RuntimeMessage::LineTexts { lines, reply } => {
            let _ = reply.send(
                terminal
                    .line_texts(&lines)
                    .map_err(|error| error.to_string()),
            );
        }
        RuntimeMessage::SelectionText(reply) => {
            let _ = reply.send(terminal.selection_text().map_err(|error| error.to_string()));
        }
        RuntimeMessage::SelectAll(reply) => {
            let _ = reply.send(select_all_text(inner, terminal));
        }
        RuntimeMessage::HyperlinkHover(point) => {
            let link = match terminal.hyperlink_at(point) {
                Ok(link) => link,
                Err(error) => {
                    log::warn!(
                        target: "paneflow::terminal::ghostty",
                        "Ghostty hyperlink lookup failed: {error}"
                    );
                    None
                }
            };
            let point = point_from_ghostty(point);
            let _ = inner
                .events_tx
                .unbounded_send(GhosttyUiEvent::HyperlinkResolved {
                    point,
                    link: link.map(|link| HyperlinkZone {
                        uri: link.uri.clone(),
                        start: point,
                        end: point,
                        is_openable: super::element::is_url_scheme_openable(&link.uri),
                        source: HyperlinkSource::Osc8,
                        line: None,
                        col: None,
                    }),
                });
        }
        RuntimeMessage::ExtractScrollback(reply) => {
            let _ = reply.send(
                terminal
                    .extract_scrollback()
                    .map_err(|error| error.to_string()),
            );
        }
        RuntimeMessage::ScreenText(reply) => {
            let _ = reply.send(
                terminal
                    .format(ghostty::FormatterOptions::plain_text())
                    .map_err(|error| error.to_string()),
            );
        }
        RuntimeMessage::CaptureReplay(reply) => {
            let _ = reply.send(terminal.capture_replay().map_err(|error| error.to_string()));
        }
        RuntimeMessage::RestoreScrollback { text, reply } => {
            let _ = terminal.restore_scrollback(&text);
            let _ = gate.publish_now(inner, terminal);
            let _ = reply.send(());
        }
        other => return CommandOutcome::Unhandled(other),
    }
    CommandOutcome::Handled
}

pub(super) fn complete_resize(inner: &SessionInner, command: ResizeCommand, succeeded: bool) {
    let size = command.size;
    let mut resize = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    resize.submitted = None;
    if succeeded {
        resize.applied = Some(size);
    } else if command.clear_initial {
        resize.clear_initial_requested = true;
    }
    if resize.requested != size || resize.clear_initial_requested {
        inner.command_backpressure.store(true, Ordering::Release);
    }
    drop(resize);
    notify_command_capacity(inner);
}

#[cfg(test)]
pub(super) fn complete_resize_during_drain(inner: &SessionInner) {
    let mut resize = inner
        .resize
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    resize.submitted = None;
    resize.clear_initial_requested = false;
    drop(resize);
    notify_command_capacity(inner);
}

pub(super) fn advance_selection_autoscroll(
    inner: &Arc<SessionInner>,
    terminal: &mut ghostty::DisplayTerminal,
    gate: &mut PublishGate,
    last_tick: &mut Instant,
) {
    let Some(target) = lock_gesture(inner).applied else {
        return;
    };
    if last_tick.elapsed() < SELECTION_AUTOSCROLL_INTERVAL {
        return;
    }
    *last_tick = Instant::now();
    let state = match terminal.gesture_state() {
        Ok(state) => state,
        Err(error) => {
            log::warn!(
                target: "paneflow::terminal::ghostty",
                "Ghostty gesture state read failed: {error}"
            );
            return;
        }
    };
    let (delta, viewport_row) = match state.autoscroll {
        ghostty::GestureAutoscroll::None => return,
        ghostty::GestureAutoscroll::Up => (1, 0),
        ghostty::GestureAutoscroll::Down => (
            -1,
            i32::try_from(inner.state.read().metrics.screen_lines.saturating_sub(1))
                .unwrap_or(i32::MAX),
        ),
    };
    terminal.scroll(ghostty::Scroll::Delta(delta));
    let options = ghostty::DragOptions {
        position: Some(target.position),
        rectangle: target.rectangle,
        word_boundaries: Vec::new(),
    };
    let viewport = ghostty::Point::new(viewport_row, target.point.column);
    match terminal.gesture_autoscroll_tick(viewport, target.geometry, &options) {
        Ok(_) => {
            if let Err(error) = gate.publish_now(inner, terminal) {
                log::warn!(
                    target: "paneflow::terminal::ghostty",
                    "Ghostty selection autoscroll refresh failed: {error}"
                );
            }
        }
        Err(error) => log::warn!(
            target: "paneflow::terminal::ghostty",
            "Ghostty selection autoscroll failed: {error}"
        ),
    }
}

pub(super) fn lock_gesture(inner: &SessionInner) -> std::sync::MutexGuard<'_, GestureUpdateState> {
    inner
        .gesture
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn publish_gesture_selection(inner: &SessionInner, range: Option<ghostty::SelectionRange>) {
    update_shared_selection(inner, range.map(selection_range_from_ghostty));
}

fn select_all_text(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
) -> Result<Option<String>, String> {
    terminal
        .gesture_reset()
        .map_err(|error| error.to_string())?;
    {
        let mut gesture = lock_gesture(inner);
        gesture.in_flight = None;
        gesture.applied = None;
    }
    if !terminal.select_all().map_err(|error| error.to_string())? {
        update_shared_selection(inner, None);
        return Ok(None);
    }
    let range = terminal
        .selection_range()
        .map_err(|error| error.to_string())?;
    publish_gesture_selection(inner, range);
    terminal.selection_text().map_err(|error| error.to_string())
}

pub(super) fn gesture_behavior(kind: SelectionKind) -> ghostty::GestureBehavior {
    match kind {
        SelectionKind::Simple => ghostty::GestureBehavior::Cell,
        SelectionKind::Semantic => ghostty::GestureBehavior::Word,
        SelectionKind::Lines => ghostty::GestureBehavior::Line,
    }
}

pub(super) fn gesture_geometry(geometry: SelectionGeometry) -> Option<ghostty::GestureGeometry> {
    let columns = u32::try_from(geometry.columns).ok()?;
    let cell_width = geometry.cell_width.max(0.0).round() as u32;
    let height = geometry.height().max(0.0).round() as u32;
    if columns == 0 || cell_width == 0 || height == 0 {
        return None;
    }
    Some(ghostty::GestureGeometry {
        columns,
        cell_width,
        padding_left: 0,
        screen_height: height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_native_search(
        session: &GhosttySession,
        query: &str,
        expected_matches: usize,
    ) -> Arc<crate::search::NativeSearchState> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = session.native_search_state();
            if state.query == query && state.complete {
                assert!(state.error.is_none(), "{:?}", state.error);
                if state.total_matches == expected_matches {
                    return state;
                }
            }
            assert!(
                Instant::now() < deadline,
                "native search did not finish: {state:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn native_search_completes_in_idle_runtime_and_publishes_scrolled_history() {
        let size = TerminalWindowSize::new(80, 6, 8, 16);
        let (session, pending, _events) = GhosttySession::pending(size);
        session
            .start_display(pending, 20_000)
            .expect("display runtime");
        session.write_output(format!("old-marker\r\n{}", "filler\r\n".repeat(12_000)).as_bytes());
        assert!(session.set_native_search("old-marker".into()));
        let before = wait_for_native_search(&session, "old-marker", 1);
        assert!(
            before
                .selected_match
                .as_ref()
                .expect("selected match")
                .start
                .line
                .0
                < -11_000
        );
        assert_eq!(before.selected, Some(0));
        {
            let state = session.inner.state.read();
            let displayed_line = state
                .search
                .selected_match
                .as_ref()
                .expect("selected match")
                .start
                .line
                .0
                + state.metrics.display_offset as i32;
            assert!((0..6).contains(&displayed_line));
            assert_eq!(state.content.cells[displayed_line as usize * 80].c, 'o');
        }
        session.write_output(b"old-marker\r\n");
        let after = wait_for_native_search(&session, "old-marker", 2);
        assert_eq!(after.selected, Some(0));
        assert_eq!(
            after
                .selected_match
                .as_ref()
                .expect("selected match")
                .start
                .line
                .0,
            before
                .selected_match
                .as_ref()
                .expect("selected match")
                .start
                .line
                .0
                - 1
        );
        assert_eq!(after.rail_offsets.len(), 2);
        assert!(session.set_native_search(String::new()));
        wait_for_native_search(&session, "", 0);
        session.shutdown();
    }

    #[test]
    fn native_search_acknowledges_each_navigation_without_rebuilding_the_rail() {
        let size = TerminalWindowSize::new(80, 8, 8, 16);
        let (session, pending, _events) = GhosttySession::pending(size);
        session
            .start_display(pending, 1_000)
            .expect("display runtime");
        session.write_output("marker\r\n".repeat(100).as_bytes());
        assert!(session.set_native_search("marker".into()));
        let initial = wait_for_native_search(&session, "marker", 100);
        let mut expected = 99;
        for (index, previous) in [true, true, false, true, false, false]
            .into_iter()
            .enumerate()
        {
            assert!(
                session.select_native_search(
                    previous,
                    initial.navigation_generation + index as u64 + 1
                )
            );
            expected = if previous {
                (expected + 99) % 100
            } else {
                (expected + 1) % 100
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let state = session.native_search_state();
                if state.navigation_generation == initial.navigation_generation + index as u64 + 1 {
                    assert_eq!(state.selected, Some(expected));
                    assert!(state.error.is_none(), "{:?}", state.error);
                    assert!(Arc::ptr_eq(&initial.rail_offsets, &state.rail_offsets));
                    break;
                }
                assert!(Instant::now() < deadline, "navigation was not acknowledged");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        session.shutdown();
    }

    #[test]
    fn select_all_publishes_the_whole_buffer_and_returns_its_text() {
        let size = TerminalWindowSize::new(80, 6, 8, 16);
        let (session, pending, _events) = GhosttySession::pending(size);
        session
            .start_display(pending, 1_000)
            .expect("display runtime");
        session.write_output(
            format!("first-marker\r\n{}last-marker", "filler\r\n".repeat(40)).as_bytes(),
        );
        let text = session
            .select_all_text()
            .expect("select all must return the buffer text");
        assert!(text.starts_with("first-marker"), "{text:?}");
        assert!(text.ends_with("last-marker"), "{text:?}");
        let range = session
            .selection_range()
            .expect("select all must publish a selection");
        assert!(range.start.line.0 < 0, "{range:?}");
        assert!(range.end.line.0 >= 0, "{range:?}");
        session.shutdown();
    }

    #[test]
    fn select_all_on_an_empty_terminal_returns_nothing() {
        let size = TerminalWindowSize::new(80, 6, 8, 16);
        let (session, pending, _events) = GhosttySession::pending(size);
        session
            .start_display(pending, 100)
            .expect("display runtime");
        assert_eq!(session.select_all_text(), None);
        assert_eq!(session.selection_range(), None);
        session.shutdown();
    }

    #[test]
    fn native_search_reports_invalid_query_without_failing_the_terminal() {
        let size = TerminalWindowSize::new(80, 6, 8, 16);
        let (session, pending, _events) = GhosttySession::pending(size);
        session
            .start_display(pending, 100)
            .expect("display runtime");
        let query = "x".repeat(ghostty::MAX_QUERY_LEN + 1);
        assert!(session.set_native_search(query.clone()));
        session.write_output(b"still alive");
        let state = session.native_search_state();
        assert_eq!(state.query, query);
        assert!(state.complete);
        assert!(state.error.is_some());
        assert!(session.set_native_search("alive".into()));
        wait_for_native_search(&session, "alive", 1);
        session.shutdown();
    }

    #[test]
    fn resize_storm_is_coalesced_and_zero_dimensions_are_clamped() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        for index in 0..200 {
            session.resize(TerminalWindowSize::new(index, index, 8, 16));
        }

        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 1);
        let first = match &queued[0] {
            RuntimeMessage::Resize(command) => *command,
            _ => panic!("expected coalesced resize"),
        };
        assert_eq!(first.size, TerminalWindowSize::new(1, 1, 8, 16));
        assert!(!first.clear_initial);
        let resize = session
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(resize.requested.cols, 199);
        assert_eq!(resize.requested.rows, 199);
        assert_eq!(resize.requested.cell_width, 8);
        assert_eq!(resize.requested.cell_height, 16);
        drop(resize);

        complete_resize(&session.inner, first, true);
        session.retry_backpressured_commands();
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)]
                if command.size == TerminalWindowSize::new(199, 199, 8, 16)
                    && !command.clear_initial
        ));
    }

    #[test]
    fn resize_during_drain_is_completed_without_apply_or_requeue() {
        let initial = TerminalWindowSize::new(80, 24, 8, 16);
        let requested = TerminalWindowSize::new(100, 30, 9, 18);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);
        session.resize(requested);
        let command = match pending.mailbox.try_recv().unwrap() {
            RuntimeMessage::Resize(command) => command,
            _ => panic!("expected queued resize"),
        };

        session.inner.shutdown_sent.store(true, Ordering::Release);
        complete_resize_during_drain(&session.inner);
        session.resize(TerminalWindowSize::new(120, 40, 10, 20));
        session.retry_backpressured_commands();

        assert!(pending.mailbox.drain().is_empty());
        let resize = session
            .inner
            .resize
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(resize.submitted, None);
        assert_eq!(resize.applied, Some(initial));
        assert_eq!(resize.requested, command.size);
    }

    #[test]
    fn applied_resize_is_not_resubmitted_on_backend_wakeup() {
        let initial = TerminalWindowSize::new(80, 24, 8, 16);
        let resized = TerminalWindowSize::new(100, 30, 8, 16);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);

        session.retry_backpressured_commands();
        assert!(pending.mailbox.drain().is_empty());

        session.resize(resized);
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)] if command.size == resized && !command.clear_initial
        ));
        complete_resize(
            &session.inner,
            ResizeCommand {
                size: resized,
                clear_initial: false,
            },
            true,
        );

        session.retry_backpressured_commands();
        assert!(pending.mailbox.drain().is_empty());
    }

    #[test]
    fn provisional_matching_layout_does_not_consume_initial_clear() {
        let initial = TerminalWindowSize::new(120, 40, 0, 0);
        let desired = TerminalWindowSize::new(91, 33, 10, 21);
        let (session, pending, _events_rx) = GhosttySession::pending(initial);

        let (_, provisional_clear_consumed) = session.render_content(initial, 0, 40, true);

        assert!(!provisional_clear_consumed);
        assert!(pending.mailbox.drain().is_empty());

        let (_, actual_clear_consumed) = session.render_content(desired, 0, 33, true);

        assert!(actual_clear_consumed);
        assert!(matches!(
            pending.mailbox.drain().as_slice(),
            [RuntimeMessage::Resize(command)]
                if command.size == desired && command.clear_initial
        ));
    }

    #[test]
    fn selection_drag_updates_are_coalesced_without_text_requests() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let geometry = SelectionGeometry {
            columns: 80,
            screen_lines: 24,
            display_offset: 0,
            cell_width: 8.0,
            line_height: 16.0,
        };
        session.press_selection(SelectionKind::Simple, Point::new(2, 3), (24.0, 32.0));
        for column in 4..80 {
            session.drag_selection(
                Point::new(2, column),
                (column as f32 * 8.0, 32.0),
                geometry,
                false,
            );
        }

        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 2);
        assert!(matches!(queued[0], RuntimeMessage::PressSelection { .. }));
        assert!(matches!(queued[1], RuntimeMessage::DragSelection(_)));
        let gesture = session.lock_gesture();
        assert_eq!(gesture.queued_generation, Some(gesture.generation));
        assert_eq!(gesture.kind, Some(SelectionKind::Simple));
        let requested = gesture.requested.expect("the last drag is pending");
        assert_eq!(requested.point, ghostty::Point::new(2, 79));
        assert_eq!(requested.position, (79.0 * 8.0, 32.0));
        assert!(!requested.rectangle);
        drop(gesture);

        session.clear_selection();
        session.press_selection(SelectionKind::Simple, Point::new(2, 3), (24.0, 32.0));
        let queued = pending.mailbox.drain();
        assert_eq!(queued.len(), 2);
        assert!(matches!(queued[0], RuntimeMessage::ClearSelection));
        assert!(matches!(queued[1], RuntimeMessage::PressSelection { .. }));
    }

    #[test]
    fn a_drag_without_a_press_is_not_a_selection() {
        let (session, pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        session.drag_selection(
            Point::new(2, 4),
            (32.0, 32.0),
            SelectionGeometry {
                columns: 80,
                screen_lines: 24,
                display_offset: 0,
                cell_width: 8.0,
                line_height: 16.0,
            },
            false,
        );
        session.release_selection(Some(Point::new(2, 4)));
        assert!(pending.mailbox.drain().is_empty());
    }

    #[test]
    fn a_pane_with_no_layout_yet_has_no_drag_geometry() {
        assert!(
            gesture_geometry(SelectionGeometry {
                columns: 80,
                screen_lines: 24,
                display_offset: 0,
                cell_width: 0.0,
                line_height: 16.0,
            })
            .is_none()
        );
        let geometry = gesture_geometry(SelectionGeometry {
            columns: 80,
            screen_lines: 24,
            display_offset: 0,
            cell_width: 8.4,
            line_height: 15.98,
        })
        .expect("a laid-out pane has geometry");
        assert_eq!(geometry.cell_width, 8);
        assert_eq!(geometry.screen_height, 384);
        assert_eq!(geometry.padding_left, 0);
    }

    #[test]
    fn point_only_simple_selection_is_not_copyable() {
        let point = Point::new(2, 3);
        let point_range = SelectionRange {
            start: point,
            end: point,
            is_block: false,
        };
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Simple),
                Some(point_range),
                Some("x".into()),
            ),
            None
        );

        let drag_range = SelectionRange {
            end: Point::new(2, 4),
            ..point_range
        };
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Simple),
                Some(drag_range),
                Some("xy".into()),
            ),
            Some("xy".into())
        );
        assert_eq!(
            filter_copyable_selection_text(
                Some(SelectionKind::Semantic),
                Some(point_range),
                Some("x".into()),
            ),
            Some("x".into())
        );
    }
}
