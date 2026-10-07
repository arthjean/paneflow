use super::*;

pub(super) fn refresh_recent_output_lines(
    inner: &SessionInner,
    service_output_tail: &ServiceOutputTail,
    last_refresh: &mut Option<Instant>,
    pending: &mut bool,
) -> bool {
    if !*pending {
        return false;
    }
    let now = Instant::now();
    if last_refresh.is_some_and(|last| now.duration_since(last) < RECENT_OUTPUT_REFRESH_INTERVAL) {
        return false;
    }
    let notify_trailing_edge = last_refresh.is_some();
    *last_refresh = Some(now);
    publish_recent_output_lines(inner, service_output_tail, pending);
    notify_trailing_edge
}

pub(super) fn publish_recent_output_lines(
    inner: &SessionInner,
    service_output_tail: &ServiceOutputTail,
    pending: &mut bool,
) {
    *pending = false;
    *inner.recent_output_lines.write() = Arc::from(service_output_tail.recent_lines());
}

pub(super) fn scan_chunk_for_marks(
    scanner: &mut Osc133Scanner,
    bytes: &[u8],
    raw_marks: &mut Vec<RawMark>,
) -> bool {
    let previous_len = raw_marks.len();
    scanner.feed(bytes, &mut |raw| raw_marks.push(raw));
    raw_marks.len() != previous_len
}

pub(super) fn record_command_marks(inner: &SessionInner, raw_marks: &[RawMark]) {
    let state = inner.state.read();
    let history_size = state.content.history_size as i64;
    let abs_line = history_size.saturating_add(i64::from(state.content.cursor.point.line.0));
    let screen_lines = state
        .content
        .cells
        .iter()
        .map(|cell| cell.point.line.0)
        .max()
        .map_or(1_i64, |line| i64::from(line.max(0)) + 1);
    drop(state);

    let mut marks = inner
        .marks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for raw in raw_marks {
        marks.push(CommandMark {
            kind: raw.kind,
            abs_line,
        });
    }
    marks.retain_at_or_below(history_size.saturating_add(screen_lines.saturating_sub(1)));
}

pub(super) fn notify_command_capacity(inner: &SessionInner) {
    if inner.command_backpressure.swap(false, Ordering::AcqRel) {
        queue_wakeup(inner);
    }
}

pub(super) struct PublishGate {
    last_publish: Instant,
    pending: bool,
    pub(super) interactive_until: Option<Instant>,
    urgent: bool,
    render_hold: Option<ghostty::RenderHold>,
    published_hold: Option<u64>,
    mirror: CellMirror,
    pub(super) search_query: String,
    pub(super) search_error: Option<String>,
    pub(super) navigation_generation: u64,
    pub(super) last_search_rail_refresh: Option<Instant>,
    search_rail_pending: bool,
}

impl PublishGate {
    pub(super) fn new() -> Self {
        Self::at(
            Instant::now()
                .checked_sub(MIN_PUBLISH_INTERVAL)
                .unwrap_or_else(Instant::now),
        )
    }

    fn at(last_publish: Instant) -> Self {
        Self {
            last_publish,
            pending: false,
            interactive_until: None,
            urgent: false,
            render_hold: None,
            published_hold: None,
            mirror: CellMirror::default(),
            search_query: String::new(),
            search_error: None,
            navigation_generation: 0,
            last_search_rail_refresh: None,
            search_rail_pending: false,
        }
    }

    pub(super) fn publish_now(
        &mut self,
        inner: &SessionInner,
        terminal: &mut ghostty::DisplayTerminal,
    ) -> Result<(), String> {
        self.commit(inner, terminal, true)
    }

    pub(super) fn request(
        &mut self,
        inner: &SessionInner,
        terminal: &mut ghostty::DisplayTerminal,
    ) -> Result<(), String> {
        self.note_output(Instant::now());
        self.poll(inner, terminal)
    }

    fn note_output(&mut self, now: Instant) {
        self.pending = true;
        self.urgent |= self
            .interactive_until
            .take()
            .is_some_and(|until| now <= until);
    }

    pub(super) fn poll(
        &mut self,
        inner: &SessionInner,
        terminal: &mut ghostty::DisplayTerminal,
    ) -> Result<(), String> {
        if !self.pending {
            return Ok(());
        }
        let now = Instant::now();
        self.render_hold = terminal.render_hold();
        if self.render_hold_expired(now) {
            terminal
                .release_render_hold()
                .map_err(|error| error.to_string())?;
            return self.commit(inner, terminal, true);
        }
        if !self.decide(now) {
            return Ok(());
        }
        self.commit(inner, terminal, false)
    }

    fn decide(&self, now: Instant) -> bool {
        if !self.pending || self.showing_held_frame() {
            return false;
        }
        self.urgent || now.duration_since(self.last_publish) >= MIN_PUBLISH_INTERVAL
    }

    pub(super) fn next_wake(&self, now: Instant) -> Option<Duration> {
        if !self.pending {
            return None;
        }
        if let Some(hold) = self.render_hold.filter(|_| self.showing_held_frame()) {
            return Some(SYNC_OUTPUT_MAX_HOLD.saturating_sub(now.duration_since(hold.started_at)));
        }
        if self.urgent {
            return Some(Duration::ZERO);
        }
        Some(MIN_PUBLISH_INTERVAL.saturating_sub(now.duration_since(self.last_publish)))
    }

    fn showing_held_frame(&self) -> bool {
        self.render_hold
            .is_some_and(|hold| self.published_hold == Some(hold.generation))
    }

    fn render_hold_expired(&self, now: Instant) -> bool {
        self.render_hold
            .is_some_and(|hold| now.duration_since(hold.started_at) >= SYNC_OUTPUT_MAX_HOLD)
    }

    fn commit(
        &mut self,
        inner: &SessionInner,
        terminal: &mut ghostty::DisplayTerminal,
        live: bool,
    ) -> Result<(), String> {
        let search = self.advance_search(terminal);
        let search_pending = !search.query.is_empty() && !search.complete && search.error.is_none();
        update_shared_state(inner, terminal, &mut self.mirror, Arc::new(search), live)?;
        self.last_publish = Instant::now();
        self.render_hold = terminal.render_hold();
        self.published_hold = self
            .render_hold
            .filter(|_| !live)
            .map(|hold| hold.generation);
        self.pending = search_pending || self.search_rail_pending || self.published_hold.is_some();
        self.urgent = false;
        queue_wakeup(inner);
        Ok(())
    }

    fn advance_search(
        &mut self,
        terminal: &mut ghostty::DisplayTerminal,
    ) -> crate::search::NativeSearchState {
        let mut state = crate::search::NativeSearchState {
            query: self.search_query.clone(),
            navigation_generation: self.navigation_generation,
            complete: true,
            error: self.search_error.clone(),
            ..Default::default()
        };
        if state.query.is_empty() || state.error.is_some() {
            self.search_rail_pending = false;
            return state;
        }
        let refresh_rail = self
            .last_search_rail_refresh
            .is_none_or(|last| last.elapsed() >= SEARCH_RAIL_REFRESH_INTERVAL);
        match terminal.search_step_with_rail_refresh(refresh_rail) {
            Ok(Some(snapshot)) => {
                self.search_rail_pending = snapshot.rail_pending;
                if refresh_rail {
                    self.last_search_rail_refresh = Some(Instant::now());
                }
                state.total_matches = snapshot.total_matches;
                state.rail_offsets = snapshot.rail_offsets;
                state.selected_match =
                    snapshot
                        .selected_match
                        .map(|found| crate::search::SearchMatch {
                            start: point_from_ghostty(found.start),
                            end: point_from_ghostty(found.end),
                        });
                state.viewport_matches = snapshot
                    .viewport_matches
                    .into_iter()
                    .map(|found| crate::search::SearchMatch {
                        start: point_from_ghostty(found.start),
                        end: point_from_ghostty(found.end),
                    })
                    .collect();
                state.selected = snapshot.selected;
                state.complete = snapshot.complete && !snapshot.rail_pending;
            }
            Ok(None) => self.search_rail_pending = false,
            Err(error) => {
                let error = error.to_string();
                self.search_error = Some(error.clone());
                self.search_rail_pending = false;
                state.error = Some(error);
                terminal.clear_search();
            }
        }
        state
    }
}

#[cfg(test)]
pub(in crate::terminal) fn simulate_gate_trickle(interval: Duration, chunks: usize) -> usize {
    let origin = Instant::now();
    let mut gate = PublishGate::at(origin);
    let mut published = 0usize;
    for index in 0..chunks {
        let arrived = origin + interval * (index as u32 + 1);
        if let Some(wait) = gate.next_wake(arrived - interval)
            && wait < interval
            && gate.decide(arrived - interval + wait)
        {
            gate.last_publish = arrived - interval + wait;
            gate.pending = false;
            published += 1;
        }
        gate.pending = true;
        if gate.decide(arrived) {
            gate.last_publish = arrived;
            gate.pending = false;
            published += 1;
        }
    }
    published
}

fn update_shared_state(
    inner: &SessionInner,
    terminal: &mut ghostty::DisplayTerminal,
    mirror: &mut CellMirror,
    search: Arc<crate::search::NativeSearchState>,
    live: bool,
) -> Result<(), String> {
    let snapshot = if live {
        terminal.snapshot_live()
    } else {
        terminal.snapshot()
    }
    .map_err(|error| error.to_string())?;
    let modes = terminal.modes().map_err(|error| error.to_string())?;
    let metrics = grid_metrics_from_ghostty(&snapshot);
    let content = mirror.publish(snapshot);
    let modes = modes_from_ghostty(modes);
    let kitty: Arc<[_]> = inner
        .kitty_images
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .collect(terminal)
        .into();
    let previous = std::mem::replace(
        &mut *inner.state.write(),
        SharedState {
            content,
            modes,
            metrics,
            kitty,
            search,
        },
    );
    mirror.recycle(previous.content);
    Ok(())
}

pub(super) fn update_shared_selection(inner: &SessionInner, selection: Option<SelectionRange>) {
    let mut state = inner.state.write();
    if state.content.selection == selection {
        return;
    }
    state.content.selection = selection;
    state.content.generation = next_content_generation();
    drop(state);
    queue_wakeup(inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate_at(origin: Instant) -> PublishGate {
        PublishGate::at(origin)
    }

    fn hold_at(started_at: Instant, generation: u64) -> Option<ghostty::RenderHold> {
        Some(ghostty::RenderHold {
            started_at,
            generation,
        })
    }

    fn showing_hold(gate: &mut PublishGate, started_at: Instant, generation: u64) {
        gate.render_hold = hold_at(started_at, generation);
        gate.published_hold = Some(generation);
    }

    #[test]
    fn the_first_change_after_an_idle_gap_publishes_without_waiting() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;
        assert!(gate.decide(origin + MIN_PUBLISH_INTERVAL));
    }

    #[test]
    fn a_change_too_soon_after_the_last_frame_waits_out_the_interval() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let too_soon = origin + MIN_PUBLISH_INTERVAL - Duration::from_millis(1);
        assert!(!gate.decide(too_soon));
        assert_eq!(
            gate.next_wake(too_soon),
            Some(Duration::from_millis(1)),
            "the loop must wake exactly when the interval expires"
        );
        assert!(gate.decide(origin + MIN_PUBLISH_INTERVAL));
    }

    #[test]
    fn a_synchronized_output_hold_parks_the_loop_instead_of_spinning_it() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let opened = origin + MIN_PUBLISH_INTERVAL * 4;
        showing_hold(&mut gate, opened, 1);
        assert!(!gate.decide(opened));
        assert_eq!(
            gate.next_wake(opened),
            Some(SYNC_OUTPUT_MAX_HOLD),
            "the hold's own deadline is the next useful wake"
        );

        let midway = opened + SYNC_OUTPUT_MAX_HOLD / 2;
        assert!(!gate.decide(midway));
        assert_eq!(gate.next_wake(midway), Some(SYNC_OUTPUT_MAX_HOLD / 2));
    }

    #[test]
    fn a_trickle_inside_the_interval_is_deferred_to_its_deadline_not_dropped() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let too_soon = origin + MIN_PUBLISH_INTERVAL / 4;
        assert!(!gate.decide(too_soon), "inside the interval, held");
        assert_eq!(
            gate.next_wake(too_soon),
            Some(MIN_PUBLISH_INTERVAL - MIN_PUBLISH_INTERVAL / 4),
            "the loop wakes when the interval expires"
        );
        assert!(gate.decide(origin + MIN_PUBLISH_INTERVAL));
    }

    #[test]
    fn a_trickle_publishes_once_per_interval() {
        let interval = Duration::from_millis(2);
        let chunks = 1000;
        let published = simulate_gate_trickle(interval, chunks);
        let expected = (interval * chunks as u32).as_nanos() / MIN_PUBLISH_INTERVAL.as_nanos();
        assert!(
            (published as u128).abs_diff(expected) <= 1,
            "{published} frames for {chunks} chunks, expected about {expected}"
        );
    }

    #[test]
    fn nothing_pending_means_nothing_to_wake_for() {
        let origin = Instant::now();
        let gate = gate_at(origin);
        assert!(!gate.decide(origin + MIN_PUBLISH_INTERVAL * 10));
        assert_eq!(gate.next_wake(origin), None);
    }

    #[test]
    fn synchronized_output_holds_a_frame_the_rate_limit_would_have_allowed() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let ready = origin + MIN_PUBLISH_INTERVAL * 4;
        showing_hold(&mut gate, origin, 1);
        assert!(!gate.decide(ready));
        gate.render_hold = None;
        assert!(gate.decide(ready));
    }

    #[test]
    fn a_synchronized_output_hold_expires_so_a_stalled_program_cannot_freeze_the_pane() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let opened = origin + MIN_PUBLISH_INTERVAL;
        showing_hold(&mut gate, opened, 1);
        assert!(!gate.decide(opened), "hold opens here");
        assert!(
            !gate.render_hold_expired(opened + SYNC_OUTPUT_MAX_HOLD - Duration::from_millis(1)),
            "still inside the budget"
        );
        assert!(
            gate.render_hold_expired(opened + SYNC_OUTPUT_MAX_HOLD),
            "the mode is still set, but the hold has spent its budget"
        );
    }

    #[test]
    fn each_bracketed_redraw_gets_its_own_hold_budget() {
        let origin = Instant::now();
        let mut gate = gate_at(origin);
        gate.pending = true;

        let first = origin + MIN_PUBLISH_INTERVAL;
        showing_hold(&mut gate, first, 1);
        assert!(!gate.decide(first), "the first redraw's frame is on screen");

        let second = first + SYNC_OUTPUT_MAX_HOLD * 2;
        gate.render_hold = hold_at(second, 2);
        assert!(
            gate.decide(second),
            "the next hold captured a new finished frame"
        );
        assert!(!gate.render_hold_expired(second));
    }

    fn held_terminal(cols: usize, rows: usize) -> ghostty::DisplayTerminal {
        let size = ghostty::WindowSize::new(cols, rows, 8, 16).expect("valid terminal size");
        let mut terminal =
            ghostty::DisplayTerminal::new(size, 100, ghostty::TerminalAppearance::default())
                .expect("terminal must initialize");
        configure_embedder_options(&mut terminal, 100, false);
        terminal
    }

    fn published_row(session: &GhosttySession, row: usize) -> String {
        let state = session.inner.state.read();
        let cols = state.content.cols;
        state.content.cells[row * cols..(row + 1) * cols]
            .iter()
            .map(|cell| cell.c)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    fn feed_output(
        gate: &mut PublishGate,
        session: &GhosttySession,
        terminal: &mut ghostty::DisplayTerminal,
        bytes: &[u8],
    ) {
        terminal.feed(bytes).expect("output must parse");
        gate.last_publish = Instant::now() - MIN_PUBLISH_INTERVAL;
        gate.request(&session.inner, terminal)
            .expect("publication must succeed");
    }

    #[test]
    fn a_hold_released_and_resumed_in_one_write_publishes_the_finished_frame() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        for iteration in 0..1_000 {
            let output = format!(
                "\x1b[?2026l\x1b[H\x1b[2Jframe A {iteration}\x1b[?2026h\x1b[H\x1b[2Jframe B {iteration}\r\nfinished\x1b[?2026l\x1b[?2026h\x1b[H\x1b[2Jframe C"
            );
            feed_output(&mut gate, &session, &mut terminal, output.as_bytes());

            assert_eq!(published_row(&session, 0), format!("frame B {iteration}"));
            assert_eq!(published_row(&session, 1), "finished");
            assert!(terminal.synchronized_output().expect("mode query"));
        }
    }

    #[test]
    fn output_inside_a_hold_waits_for_its_end_without_republishing() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"finished\x1b[?2026h\x1b[H\x1b[2Jhalf",
        );
        let generation = session.inner.state.read().content.generation;
        feed_output(&mut gate, &session, &mut terminal, b" drawn");
        assert_eq!(session.inner.state.read().content.generation, generation);
        assert_eq!(published_row(&session, 0), "finished");

        feed_output(&mut gate, &session, &mut terminal, b"\x1b[?2026l");
        assert_eq!(published_row(&session, 0), "half drawn");
    }

    #[test]
    fn a_hold_that_outlives_its_budget_is_released_and_shows_live_content() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"finished\x1b[?2026h\x1b[H\x1b[2Jstalled",
        );
        let started_at = terminal.render_hold().expect("hold").started_at;
        assert_eq!(published_row(&session, 0), "finished");

        std::thread::sleep(SYNC_OUTPUT_MAX_HOLD / 2);
        feed_output(&mut gate, &session, &mut terminal, b"\x1b[?2026h");
        assert_eq!(
            terminal.render_hold().expect("hold").started_at,
            started_at,
            "setting the mode again must not push the deadline back"
        );
        assert_eq!(published_row(&session, 0), "finished");

        let wait = gate
            .next_wake(Instant::now())
            .expect("the deadline wakes the loop");
        std::thread::sleep(wait);
        gate.poll(&session.inner, &mut terminal)
            .expect("expired hold must publish");

        assert_eq!(published_row(&session, 0), "stalled");
        assert!(terminal.render_hold().is_none());
        assert!(!terminal.synchronized_output().expect("mode query"));
    }

    #[test]
    fn a_resize_or_a_reset_ends_the_hold_and_publishes_live_content() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"finished\x1b[?2026h\x1b[H\x1b[2Jresized",
        );
        terminal
            .resize(ghostty::WindowSize::new(30, 4, 8, 16).expect("valid size"))
            .expect("resize");
        assert!(terminal.render_hold().is_none());
        gate.publish_now(&session.inner, &mut terminal)
            .expect("resize publication");
        assert_eq!(published_row(&session, 0), "resized");

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"\x1b[H\x1b[2Jbefore\x1b[?2026h\x1b[H\x1b[2Jpartial",
        );
        assert_eq!(published_row(&session, 0), "before");
        feed_output(&mut gate, &session, &mut terminal, b"\x1bcafter reset");
        assert!(terminal.render_hold().is_none());
        assert_eq!(published_row(&session, 0), "after reset");
    }

    #[test]
    fn scrolling_during_a_hold_shows_live_content() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"one\r\ntwo\r\nthree\x1b[?2026h\r\nfour\r\nfive",
        );
        assert_eq!(published_row(&session, 2), "three");

        terminal.scroll(ghostty::Scroll::Delta(1));
        gate.publish_now(&session.inner, &mut terminal)
            .expect("scroll publication");
        assert_eq!(published_row(&session, 2), "four");
        assert_eq!(session.inner.state.read().content.display_offset, 1);
        assert!(terminal.render_hold().is_some(), "the hold itself survives");
    }

    #[test]
    fn a_selection_during_a_hold_shows_live_content() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(24, 3, 8, 16));
        let mut terminal = held_terminal(24, 3);
        let mut gate = PublishGate::new();

        feed_output(
            &mut gate,
            &session,
            &mut terminal,
            b"finished\x1b[?2026h\x1b[H\x1b[2Jlive text",
        );
        assert_eq!(published_row(&session, 0), "finished");

        let outcome = handle_terminal_command(
            &session.inner,
            &mut terminal,
            &mut gate,
            RuntimeMessage::PressSelection {
                point: ghostty::Point::new(0, 0),
                behavior: gesture_behavior(SelectionKind::Simple),
                position: (1.0, 1.0),
            },
        );
        assert!(matches!(outcome, CommandOutcome::Handled));

        assert_eq!(published_row(&session, 0), "live text");
        assert!(terminal.render_hold().is_some(), "the hold itself survives");
    }

    fn empty_ghostty_content(cols: usize, rows: usize) -> ghostty::Content {
        ghostty::Content {
            cells: Vec::<ghostty::Cell>::new().into(),
            dirty_rows: Vec::new().into(),
            cursor: ghostty::Cursor {
                point: ghostty::Point::new(0, 0),
                shape: ghostty::CursorShape::Block,
                visible: true,
                blinking: false,
                wide_tail: false,
            },
            selection: None,
            cols,
            rows,
            display_offset: 0,
            history_size: 0,
        }
    }

    #[test]
    fn every_published_grid_gets_its_own_generation() {
        let first = content_from_ghostty(empty_ghostty_content(80, 24));
        let second = content_from_ghostty(empty_ghostty_content(80, 24));
        let blank = blank_content(80, 24);

        assert_ne!(first.generation, 0, "0 is reserved for unstamped content");
        assert!(
            first.generation < second.generation,
            "stamps must advance, got {} then {}",
            first.generation,
            second.generation
        );
        assert!(
            second.generation < blank.generation,
            "a blank grid is a frame too"
        );
    }

    #[test]
    fn row_versions_preserve_edits_across_skipped_publications() {
        let mut mirror = CellMirror::default();
        let first = mirror.publish(empty_ghostty_content(20, 4));
        let mut second = empty_ghostty_content(20, 4);
        second.dirty_rows = [false, true, false, false].into();
        let second = mirror.publish(second);
        let mut third = empty_ghostty_content(20, 4);
        third.dirty_rows = [false, false, true, false].into();
        let third = mirror.publish(third);
        assert_eq!(first.row_versions[0], third.row_versions[0]);
        assert_eq!(second.row_versions[1], third.row_versions[1]);
        assert_ne!(first.row_versions[1], third.row_versions[1]);
        assert_ne!(first.row_versions[2], third.row_versions[2]);
        assert_eq!(first.row_versions[3], third.row_versions[3]);
        let resized = mirror.publish(empty_ghostty_content(21, 4));
        assert!(
            resized
                .row_versions
                .iter()
                .zip(third.row_versions.iter())
                .all(|(new, old)| new != old)
        );
    }

    #[test]
    fn interactive_output_bypasses_only_the_rate_limit() {
        let now = Instant::now();
        let mut gate = gate_at(now);
        gate.interactive_until = Some(now + INTERACTIVE_OUTPUT_WINDOW);
        gate.note_output(now + Duration::from_millis(1));
        showing_hold(&mut gate, now, 1);
        assert!(!gate.decide(now + Duration::from_millis(1)));
        gate.render_hold = None;
        assert!(gate.decide(now + Duration::from_millis(2)));
        assert_eq!(
            gate.next_wake(now + Duration::from_millis(2)),
            Some(Duration::ZERO)
        );
        gate.urgent = false;
        gate.note_output(now + Duration::from_millis(3));
        assert!(!gate.decide(now + Duration::from_millis(3)));
    }

    #[test]
    fn expired_interactive_input_does_not_accelerate_unrelated_output() {
        let now = Instant::now();
        let mut gate = gate_at(now);
        gate.interactive_until = Some(now - Duration::from_millis(1));
        gate.note_output(now);
        assert!(!gate.decide(now));
        assert_eq!(gate.next_wake(now), Some(MIN_PUBLISH_INTERVAL));
    }

    #[test]
    fn republishing_only_the_selection_still_advances_the_generation() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let before = session.inner.state.read().content.generation;

        let selection = Some(SelectionRange {
            start: Point::new(0, 0),
            end: Point::new(0, 4),
            is_block: false,
        });
        update_shared_selection(&session.inner, selection);

        let after = session.inner.state.read().content.generation;
        assert!(
            after > before,
            "expected a new stamp, got {before} then {after}"
        );

        update_shared_selection(&session.inner, selection);
        assert_eq!(session.inner.state.read().content.generation, after);
    }

    #[test]
    fn service_tail_refresh_requests_a_trailing_scan() {
        let (session, _pending, _events_rx) =
            GhosttySession::pending(TerminalWindowSize::new(80, 24, 8, 16));
        let mut tail = ServiceOutputTail::default();
        tail.advance(b"first\n");
        let mut last_refresh = None;
        let mut pending = true;

        assert!(!refresh_recent_output_lines(
            &session.inner,
            &tail,
            &mut last_refresh,
            &mut pending,
        ));
        assert_eq!(session.recent_output_lines().as_ref(), ["first"]);

        tail.advance(b"http://127.0.0.1:3000\n");
        last_refresh = Some(Instant::now() - RECENT_OUTPUT_REFRESH_INTERVAL);
        pending = true;
        assert!(refresh_recent_output_lines(
            &session.inner,
            &tail,
            &mut last_refresh,
            &mut pending,
        ));
        assert_eq!(
            session.recent_output_lines().first().map(String::as_str),
            Some("http://127.0.0.1:3000")
        );
    }
}
