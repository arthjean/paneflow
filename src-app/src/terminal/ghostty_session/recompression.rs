use std::time::{Duration, Instant};

use paneflow_terminal_ghostty as ghostty;

pub(super) const RECOMPRESSION_IDLE: Duration = Duration::from_secs(2);

pub(super) struct HistoryRecompression {
    activity: Option<u64>,
    quiet_since: Instant,
    settled: bool,
    stopped: bool,
}

impl HistoryRecompression {
    pub(super) fn new(now: Instant) -> Self {
        Self {
            activity: None,
            quiet_since: now,
            settled: false,
            stopped: false,
        }
    }

    pub(super) fn touch(&mut self, now: Instant) {
        self.quiet_since = now;
        self.settled = false;
    }

    pub(super) fn next_wake(&self, now: Instant) -> Option<Duration> {
        if self.stopped || self.settled {
            return None;
        }
        Some(RECOMPRESSION_IDLE.saturating_sub(now.saturating_duration_since(self.quiet_since)))
    }

    pub(super) fn advance(&mut self, terminal: &mut ghostty::DisplayTerminal, now: Instant) {
        let activity = terminal.compression_activity();
        self.advance_with(now, activity, || terminal.compress_step());
    }

    fn advance_with(
        &mut self,
        now: Instant,
        activity: ghostty::Result<u64>,
        step: impl FnOnce() -> ghostty::Result<ghostty::CompressionProgress>,
    ) {
        if self.stopped {
            return;
        }
        let activity = match activity {
            Ok(activity) => activity,
            Err(error) => {
                self.stop(&error);
                return;
            }
        };
        if self.activity != Some(activity) {
            self.activity = Some(activity);
            self.touch(now);
            return;
        }
        if self.settled || now.saturating_duration_since(self.quiet_since) < RECOMPRESSION_IDLE {
            return;
        }
        match step() {
            Ok(ghostty::CompressionProgress::Pending) => {}
            Ok(ghostty::CompressionProgress::Complete) => self.settled = true,
            Ok(ghostty::CompressionProgress::Unsupported) => self.stopped = true,
            Err(error) => self.stop(&error),
        }
    }

    fn stop(&mut self, error: &ghostty::GhosttyError) {
        log::debug!(
            target: "paneflow::terminal::ghostty",
            "history recompression stopped for this terminal: {error}"
        );
        self.stopped = true;
    }
}

#[cfg(test)]
mod tests {
    use super::super::TerminalWindowSize;
    use super::*;

    const HISTORY_LINES: usize = 50_000;

    fn restored_history() -> ghostty::DisplayTerminal {
        let size = TerminalWindowSize::new(80, 24, 8, 16);
        let mut source = ghostty::DisplayTerminal::new(
            super::super::window_size(size).expect("window size"),
            HISTORY_LINES + 1_000,
            ghostty::TerminalAppearance::default(),
        )
        .expect("terminal");
        source.set_scrollback_max_bytes(None).expect("unbounded");
        let mut output = Vec::new();
        for line in 0..HISTORY_LINES {
            output.extend_from_slice(
                format!("restored history line {line:05} with some searchable text\r\n").as_bytes(),
            );
        }
        source.feed(&output).expect("history");
        let snapshot = source.encode_snapshot().expect("checkpoint");
        super::super::restore_terminal_from_checkpoint(
            &snapshot,
            size,
            HISTORY_LINES + 1_000,
            false,
        )
        .expect("restores")
    }

    fn search_every_row(terminal: &ghostty::DisplayTerminal) {
        let mut start_row = 0;
        loop {
            let chunk = terminal
                .search_chunk(start_row, usize::MAX)
                .expect("search chunk");
            if chunk.next_row <= start_row || chunk.next_row >= chunk.total_rows {
                break;
            }
            start_row = chunk.next_row;
        }
    }

    fn resident(terminal: &ghostty::DisplayTerminal) -> (bool, u64) {
        let usage = terminal.memory_usage().expect("memory usage");
        (usage.compression_supported, usage.primary_resident_bytes)
    }

    #[test]
    fn a_fully_searched_restored_history_recompresses_once_idle() {
        let mut terminal = restored_history();
        let started = Instant::now();
        let mut recompression = HistoryRecompression::new(started);
        recompression.advance(&mut terminal, started);
        let (supported, after_restore) = resident(&terminal);

        search_every_row(&terminal);
        recompression.touch(started);
        let (_, after_search) = resident(&terminal);
        if supported {
            assert!(
                after_search > after_restore * 2,
                "the search decompresses the history: {after_restore} then {after_search}"
            );
        }

        recompression.advance(&mut terminal, started + Duration::from_secs(1));
        assert_eq!(
            resident(&terminal).1,
            after_search,
            "nothing runs before the idle delay"
        );

        let idle = started + Duration::from_secs(5);
        let mut steps = 0;
        while recompression.next_wake(idle).is_some() {
            recompression.advance(&mut terminal, idle);
            steps += 1;
            assert!(steps < 100_000, "recompression must finish");
        }
        let (_, after_idle) = resident(&terminal);
        assert!(
            after_idle * 10 < after_restore * 12,
            "resident bytes {after_idle} after idle must stay under 1.2 x {after_restore}"
        );
    }

    #[test]
    fn an_unsupported_compression_stops_the_idle_step_without_an_error() {
        let started = Instant::now();
        let idle = started + RECOMPRESSION_IDLE;
        let mut recompression = HistoryRecompression::new(started);
        recompression.advance_with(started, Ok(7), || {
            unreachable!("no step before the idle delay")
        });
        recompression.advance_with(idle, Ok(7), || {
            Ok(ghostty::CompressionProgress::Unsupported)
        });
        assert_eq!(recompression.next_wake(idle), None);

        recompression.touch(idle);
        recompression.advance_with(idle + RECOMPRESSION_IDLE, Ok(8), || {
            unreachable!("a terminal without compression is never stepped again")
        });
        recompression.advance_with(idle + RECOMPRESSION_IDLE * 2, Ok(8), || {
            unreachable!("a terminal without compression is never stepped again")
        });
        assert_eq!(recompression.next_wake(idle + RECOMPRESSION_IDLE * 2), None);
    }

    #[test]
    fn new_activity_restarts_the_idle_delay() {
        let started = Instant::now();
        let mut recompression = HistoryRecompression::new(started);
        recompression.advance_with(started, Ok(1), || unreachable!("first observation"));
        let later = started + Duration::from_millis(1_500);
        recompression.advance_with(later, Ok(2), || unreachable!("activity changed"));
        recompression.advance_with(started + RECOMPRESSION_IDLE, Ok(2), || {
            unreachable!("the delay restarts at the new activity")
        });
        assert_eq!(
            recompression.next_wake(started + RECOMPRESSION_IDLE),
            Some(Duration::from_millis(1_500))
        );
        let mut stepped = false;
        recompression.advance_with(later + RECOMPRESSION_IDLE, Ok(2), || {
            stepped = true;
            Ok(ghostty::CompressionProgress::Complete)
        });
        assert!(stepped);
        assert_eq!(recompression.next_wake(later + RECOMPRESSION_IDLE), None);
    }
}
