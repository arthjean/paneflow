use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};

pub const LEDGER_WRITE_INTERVAL: Duration = Duration::from_secs(60);
pub const TURN_IDLE_LIMIT_MS: u64 = 4 * 3600 * 1000;
pub const MAX_TURN_ACTIVE_MS: u64 = 4 * 3600 * 1000;
pub const RETAINED_DAYS: u64 = 400;
const MAX_LEDGER_BYTES: u64 = 8 * 1024 * 1024;
const MAX_TURN_INTERVALS: usize = 256;
const MAX_WATERMARKS: usize = 512;
const LEDGER_VERSION: u32 = 1;

pub trait Calendar {
    fn day(&self, at_ms: u64) -> NaiveDate;
    fn next_midnight_ms(&self, at_ms: u64) -> u64;
}

pub struct ZoneCalendar<Tz: TimeZone>(pub Tz);

pub fn local_calendar() -> ZoneCalendar<chrono::Local> {
    ZoneCalendar(chrono::Local)
}

impl<Tz: TimeZone> Calendar for ZoneCalendar<Tz> {
    fn day(&self, at_ms: u64) -> NaiveDate {
        let millis = i64::try_from(at_ms).unwrap_or(i64::MAX);
        self.0
            .timestamp_millis_opt(millis)
            .earliest()
            .map_or(NaiveDate::MIN, |at| at.date_naive())
    }

    fn next_midnight_ms(&self, at_ms: u64) -> u64 {
        let Some(next) = self.day(at_ms).succ_opt() else {
            return u64::MAX;
        };
        (0..3)
            .find_map(|hour| {
                let local = next.and_hms_opt(hour, 0, 0)?;
                self.0.from_local_datetime(&local).earliest()
            })
            .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
            .filter(|midnight| *midnight > at_ms)
            .unwrap_or(u64::MAX)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct LedgerAccount {
    pub agent: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config_dir: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayTally {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub turns: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub active_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokens_in: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tokens_out: u64,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnInput {
    Prompt,
    Stop,
    Working,
    Blocked,
    Settled,
    Ended,
    Vanished,
}

impl TurnInput {
    pub const fn from_hook(self) -> bool {
        matches!(self, Self::Prompt | Self::Stop | Self::Ended)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnSignal {
    pub session: String,
    pub account: LedgerAccount,
    pub input: TurnInput,
    pub at_ms: u64,
    pub order: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct OpenTurn {
    account: LedgerAccount,
    started_at_ms: u64,
    last_event_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    working_since_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    working: Vec<(u64, u64)>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    declared: bool,
}

impl OpenTurn {
    fn close_working(&mut self, at_ms: u64) {
        let Some(since) = self.working_since_ms.take() else {
            return;
        };
        let end = at_ms.max(since);
        let full = self.working.len() >= MAX_TURN_INTERVALS;
        match self.working.last_mut() {
            Some(last) if full => last.1 = end,
            _ => self.working.push((since, end)),
        }
    }

    fn active_intervals(&self, end_ms: u64) -> Vec<(u64, u64)> {
        let intervals = if self.declared {
            self.working.clone()
        } else {
            vec![(self.prompt_at_ms.unwrap_or(self.started_at_ms), end_ms)]
        };
        let mut budget = MAX_TURN_ACTIVE_MS;
        intervals
            .into_iter()
            .filter_map(|(start, end)| {
                let length = end.saturating_sub(start).min(budget);
                budget -= length;
                (length > 0).then_some((start, start + length))
            })
            .collect()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LedgerFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: Vec<LedgerAccount>,
    #[serde(default)]
    days: BTreeMap<NaiveDate, Vec<DayEntry>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    open_turns: BTreeMap<String, OpenTurn>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    watermarks: BTreeMap<String, (u64, u64)>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DayEntry {
    account: usize,
    #[serde(flatten)]
    tally: DayTally,
}

#[derive(Debug, Default)]
pub struct TurnBook {
    days: BTreeMap<NaiveDate, BTreeMap<LedgerAccount, DayTally>>,
    open: BTreeMap<String, OpenTurn>,
    watermarks: BTreeMap<String, (u64, u64)>,
    revision: u64,
}

impl TurnBook {
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn tally(&self, day: NaiveDate, account: &LedgerAccount) -> DayTally {
        self.days
            .get(&day)
            .and_then(|accounts| accounts.get(account))
            .copied()
            .unwrap_or_default()
    }

    pub fn days(&self) -> &BTreeMap<NaiveDate, BTreeMap<LedgerAccount, DayTally>> {
        &self.days
    }

    pub fn open_turns(&self) -> usize {
        self.open.len()
    }

    pub fn apply(&mut self, signal: &TurnSignal, calendar: &impl Calendar) {
        if signal.input.from_hook() {
            let watermark = self.watermarks.entry(signal.session.clone()).or_default();
            let mark = (signal.at_ms, signal.order);
            if mark <= *watermark {
                return;
            }
            *watermark = mark;
        }
        let at = signal.at_ms;
        match signal.input {
            TurnInput::Prompt => {
                let turn = self.open_or_start(signal, calendar);
                turn.prompt_at_ms.get_or_insert(at);
                turn.last_event_ms = turn.last_event_ms.max(at);
            }
            TurnInput::Working => {
                let turn = self.open_or_start(signal, calendar);
                turn.declared = true;
                turn.working_since_ms.get_or_insert(at);
                turn.last_event_ms = turn.last_event_ms.max(at);
            }
            TurnInput::Blocked => {
                if let Some(turn) = self.open.get_mut(&signal.session) {
                    turn.close_working(at);
                    turn.last_event_ms = turn.last_event_ms.max(at);
                }
            }
            TurnInput::Settled | TurnInput::Stop | TurnInput::Ended => {
                self.finish(&signal.session, Some(at), calendar);
            }
            TurnInput::Vanished => self.finish(&signal.session, None, calendar),
        }
        self.trim_watermarks();
    }

    pub fn expire(
        &mut self,
        running: &BTreeSet<String>,
        in_turn: &BTreeSet<String>,
        now_ms: u64,
        calendar: &impl Calendar,
    ) -> usize {
        let expired: Vec<(String, Option<u64>)> = self
            .open
            .iter()
            .filter_map(|(session, turn)| {
                if !running.contains(session) || (turn.declared && !in_turn.contains(session)) {
                    return Some((session.clone(), None));
                }
                (now_ms.saturating_sub(turn.last_event_ms) >= TURN_IDLE_LIMIT_MS).then(|| {
                    let end = turn.working_since_ms.map(|_| now_ms);
                    (session.clone(), end)
                })
            })
            .collect();
        for (session, end) in &expired {
            self.finish(session, *end, calendar);
        }
        expired.len()
    }

    fn open_or_start(&mut self, signal: &TurnSignal, calendar: &impl Calendar) -> &mut OpenTurn {
        if !self.open.contains_key(&signal.session) {
            self.book(calendar.day(signal.at_ms), &signal.account, |tally| {
                tally.turns += 1;
            });
        }
        self.open
            .entry(signal.session.clone())
            .or_insert_with(|| OpenTurn {
                account: signal.account.clone(),
                started_at_ms: signal.at_ms,
                last_event_ms: signal.at_ms,
                prompt_at_ms: None,
                working_since_ms: None,
                working: Vec::new(),
                declared: false,
            })
    }

    fn finish(&mut self, session: &str, end_ms: Option<u64>, calendar: &impl Calendar) {
        let Some(mut turn) = self.open.remove(session) else {
            return;
        };
        let end = end_ms.unwrap_or(turn.last_event_ms).max(turn.started_at_ms);
        turn.close_working(end);
        let account = turn.account.clone();
        for (start, stop) in turn.active_intervals(end) {
            self.book_span(&account, start, stop, calendar);
        }
        self.revision += 1;
    }

    fn book_span(
        &mut self,
        account: &LedgerAccount,
        start: u64,
        stop: u64,
        calendar: &impl Calendar,
    ) {
        let mut cursor = start;
        while cursor < stop {
            let boundary = calendar.next_midnight_ms(cursor).min(stop);
            let length = boundary - cursor;
            self.book(calendar.day(cursor), account, |tally| {
                tally.active_ms += length;
            });
            cursor = boundary;
        }
    }

    fn book(
        &mut self,
        day: NaiveDate,
        account: &LedgerAccount,
        change: impl FnOnce(&mut DayTally),
    ) {
        change(
            self.days
                .entry(day)
                .or_default()
                .entry(account.clone())
                .or_default(),
        );
        self.revision += 1;
    }

    fn trim_watermarks(&mut self) {
        while self.watermarks.len() > MAX_WATERMARKS {
            let Some(oldest) = self
                .watermarks
                .iter()
                .filter(|(session, _)| !self.open.contains_key(*session))
                .min_by_key(|(_, at)| **at)
                .map(|(session, _)| session.clone())
            else {
                return;
            };
            self.watermarks.remove(&oldest);
        }
    }

    fn prune(&mut self, today: NaiveDate) {
        let Some(oldest) = today.checked_sub_days(chrono::Days::new(RETAINED_DAYS)) else {
            return;
        };
        self.days.retain(|day, _| *day > oldest);
    }

    fn restore(file: LedgerFile) -> Self {
        let mut days: BTreeMap<NaiveDate, BTreeMap<LedgerAccount, DayTally>> = BTreeMap::new();
        for (day, entries) in file.days {
            for entry in entries {
                if let Some(account) = file.accounts.get(entry.account) {
                    days.entry(day)
                        .or_default()
                        .insert(account.clone(), entry.tally);
                }
            }
        }
        let open = file
            .open_turns
            .into_iter()
            .map(|(session, mut turn)| {
                let last = turn.last_event_ms;
                turn.close_working(last);
                (session, turn)
            })
            .collect();
        Self {
            days,
            open,
            watermarks: file.watermarks,
            revision: 0,
        }
    }

    fn to_file(&self) -> LedgerFile {
        let accounts: Vec<LedgerAccount> = self
            .days
            .values()
            .flat_map(BTreeMap::keys)
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let days = self
            .days
            .iter()
            .map(|(day, tallies)| {
                let entries = tallies
                    .iter()
                    .filter_map(|(account, tally)| {
                        Some(DayEntry {
                            account: accounts.binary_search(account).ok()?,
                            tally: *tally,
                        })
                    })
                    .collect();
                (*day, entries)
            })
            .collect();
        LedgerFile {
            version: LEDGER_VERSION,
            accounts,
            days,
            open_turns: self.open.clone(),
            watermarks: self.watermarks.clone(),
        }
    }

    pub fn to_value(&self) -> serde_json::Value {
        let file = self.to_file();
        serde_json::json!({
            "revision": self.revision,
            "accounts": file.accounts,
            "days": file.days,
        })
    }
}

pub struct Ledger {
    path: PathBuf,
    book: TurnBook,
    written_revision: u64,
    last_write: Option<Instant>,
}

impl Ledger {
    pub fn load(path: &Path, calendar: &impl Calendar, now_ms: u64) -> Self {
        let mut book = match paneflow_home::read_regular_capped(path, MAX_LEDGER_BYTES) {
            Ok(bytes) => match serde_json::from_slice::<LedgerFile>(&bytes) {
                Ok(file) => TurnBook::restore(file),
                Err(error) => {
                    quarantine(path, &error.to_string(), now_ms);
                    TurnBook::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => TurnBook::default(),
            Err(error) => {
                quarantine(path, &error.to_string(), now_ms);
                TurnBook::default()
            }
        };
        book.prune(calendar.day(now_ms));
        Self {
            path: path.to_path_buf(),
            book,
            written_revision: 0,
            last_write: None,
        }
    }

    pub fn book(&self) -> &TurnBook {
        &self.book
    }

    pub fn book_mut(&mut self) -> &mut TurnBook {
        &mut self.book
    }

    pub fn flush(&mut self, now: Instant, force: bool) -> bool {
        if self.book.revision == self.written_revision {
            return false;
        }
        if !force
            && self
                .last_write
                .is_some_and(|at| now.duration_since(at) < LEDGER_WRITE_INTERVAL)
        {
            return false;
        }
        let bytes = match serde_json::to_vec(&self.book.to_file()) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("paneflow-serve: cannot encode the usage ledger: {error}");
                return false;
            }
        };
        self.last_write = Some(now);
        match paneflow_home::write_atomically(&self.path, &bytes) {
            Ok(()) => {
                self.written_revision = self.book.revision;
                true
            }
            Err(error) => {
                log::warn!(
                    "paneflow-serve: cannot write {}: {error}",
                    self.path.display()
                );
                false
            }
        }
    }
}

fn quarantine(path: &Path, reason: &str, now_ms: u64) {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".bad-{now_ms}"));
    let target = PathBuf::from(name);
    match std::fs::rename(path, &target) {
        Ok(()) => log::warn!(
            "paneflow-serve: the usage ledger was unreadable ({reason}); moved it to {} and started an empty one",
            target.display()
        ),
        Err(error) => log::warn!(
            "paneflow-serve: the usage ledger was unreadable ({reason}) and could not be moved aside: {error}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    const PARIS: i32 = 3600;

    fn calendar() -> ZoneCalendar<FixedOffset> {
        ZoneCalendar(FixedOffset::east_opt(PARIS).unwrap())
    }

    fn at(day: u32, hour: u32, minute: u32, second: u32) -> u64 {
        let local = NaiveDate::from_ymd_opt(2026, 10, day)
            .unwrap()
            .and_hms_opt(hour, minute, second)
            .unwrap();
        u64::try_from(
            calendar()
                .0
                .from_local_datetime(&local)
                .unwrap()
                .timestamp_millis(),
        )
        .unwrap()
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
    }

    fn claude() -> LedgerAccount {
        LedgerAccount {
            agent: "claude-code".to_string(),
            config_dir: "/home/u/.claude".to_string(),
        }
    }

    fn signal(session: &str, input: TurnInput, at_ms: u64) -> TurnSignal {
        TurnSignal {
            session: session.to_string(),
            account: claude(),
            input,
            at_ms,
            order: 0,
        }
    }

    #[test]
    fn a_declared_turn_counts_once_and_its_active_time_is_the_working_span() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        for (input, time) in [
            (TurnInput::Prompt, at(31, 10, 0, 0)),
            (TurnInput::Working, at(31, 10, 0, 1)),
            (TurnInput::Blocked, at(31, 10, 0, 31)),
            (TurnInput::Working, at(31, 10, 1, 31)),
            (TurnInput::Settled, at(31, 10, 2, 1)),
            (TurnInput::Stop, at(31, 10, 2, 2)),
        ] {
            book.apply(&signal("s", input, time), &calendar);
        }
        let tally = book.tally(date(31), &claude());
        assert_eq!(
            tally.turns, 1,
            "a blocker pauses the turn, it does not end it"
        );
        assert_eq!(tally.active_ms, 60_000, "30 s, then 30 s after the blocker");
        assert_eq!(book.open_turns(), 0);
    }

    #[test]
    fn a_hook_only_turn_is_measured_from_prompt_to_stop_and_capped_at_four_hours() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        book.apply(&signal("s", TurnInput::Prompt, at(30, 8, 0, 0)), &calendar);
        book.apply(&signal("s", TurnInput::Stop, at(30, 8, 0, 42)), &calendar);
        book.apply(&signal("s", TurnInput::Prompt, at(30, 9, 0, 0)), &calendar);
        book.apply(&signal("s", TurnInput::Stop, at(30, 13, 30, 0)), &calendar);
        let tally = book.tally(date(30), &claude());
        assert_eq!(tally.turns, 2);
        assert_eq!(tally.active_ms, 42_000 + MAX_TURN_ACTIVE_MS);
    }

    #[test]
    fn a_turn_counts_on_its_start_day_and_its_active_time_splits_at_midnight() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        book.apply(
            &signal("s", TurnInput::Working, at(30, 23, 59, 0)),
            &calendar,
        );
        book.apply(
            &signal("s", TurnInput::Settled, at(31, 0, 1, 30)),
            &calendar,
        );
        assert_eq!(book.tally(date(30), &claude()).turns, 1);
        assert_eq!(book.tally(date(31), &claude()).turns, 0);
        assert_eq!(book.tally(date(30), &claude()).active_ms, 60_000);
        assert_eq!(book.tally(date(31), &claude()).active_ms, 90_000);
    }

    #[test]
    fn an_idle_turn_closes_after_four_hours_at_its_last_event() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        let running = BTreeSet::from(["s".to_string()]);
        let none = BTreeSet::new();
        book.apply(&signal("s", TurnInput::Prompt, at(31, 8, 0, 0)), &calendar);
        assert_eq!(
            book.expire(&running, &none, at(31, 11, 59, 0), &calendar),
            0,
            "a hook-only turn is not ended by an idle declared status"
        );
        assert_eq!(book.expire(&running, &none, at(31, 12, 0, 0), &calendar), 1);
        assert_eq!(book.tally(date(31), &claude()).active_ms, 0);
        book.apply(&signal("s", TurnInput::Prompt, at(31, 13, 0, 0)), &calendar);
        assert_eq!(book.tally(date(31), &claude()).turns, 2);
        assert_eq!(
            book.expire(&none, &none, at(31, 13, 5, 0), &calendar),
            1,
            "a session that stopped running ends its turn"
        );
    }

    #[test]
    fn a_declared_turn_restored_after_a_restart_ends_once_its_session_is_seen_settled() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("ledger.json");
        let calendar = calendar();
        let mut ledger = Ledger::load(&path, &calendar, at(31, 9, 0, 0));
        ledger
            .book_mut()
            .apply(&signal("s", TurnInput::Prompt, at(31, 9, 0, 0)), &calendar);
        ledger
            .book_mut()
            .apply(&signal("s", TurnInput::Working, at(31, 9, 0, 1)), &calendar);
        assert!(ledger.flush(Instant::now(), true));

        let mut restarted = Ledger::load(&path, &calendar, at(31, 9, 5, 0));
        let running = BTreeSet::from(["s".to_string()]);
        assert_eq!(
            restarted
                .book_mut()
                .expire(&running, &running, at(31, 9, 5, 0), &calendar),
            0,
            "a session still in its turn keeps it open"
        );
        assert_eq!(
            restarted
                .book_mut()
                .expire(&running, &BTreeSet::new(), at(31, 9, 5, 2), &calendar),
            1,
            "the turn finished while the worker was down"
        );
        restarted
            .book_mut()
            .apply(&signal("s", TurnInput::Prompt, at(31, 9, 6, 0)), &calendar);
        restarted
            .book_mut()
            .apply(&signal("s", TurnInput::Working, at(31, 9, 6, 1)), &calendar);
        restarted.book_mut().apply(
            &signal("s", TurnInput::Settled, at(31, 9, 6, 11)),
            &calendar,
        );
        let tally = restarted.book().tally(date(31), &claude());
        assert_eq!(tally.turns, 2, "the next prompt opens a turn of its own");
        assert_eq!(tally.active_ms, 10_000);
    }

    #[test]
    fn hooks_received_in_the_same_millisecond_are_ordered_by_their_revision() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        let same = at(31, 8, 0, 0);
        let prompt = TurnSignal {
            order: 7,
            ..signal("s", TurnInput::Prompt, same)
        };
        book.apply(&prompt, &calendar);
        book.apply(
            &TurnSignal {
                order: 8,
                ..signal("s", TurnInput::Stop, same)
            },
            &calendar,
        );
        assert_eq!(
            book.open_turns(),
            0,
            "the stop is not mistaken for a replay"
        );
        book.apply(&prompt, &calendar);
        assert_eq!(book.open_turns(), 0, "the replayed prompt is ignored");
        assert_eq!(book.tally(date(31), &claude()).turns, 1);
    }

    #[test]
    fn a_replayed_hook_is_not_counted_twice() {
        let mut book = TurnBook::default();
        let calendar = calendar();
        let prompt = signal("s", TurnInput::Prompt, at(31, 8, 0, 0));
        book.apply(&prompt, &calendar);
        book.apply(&signal("s", TurnInput::Stop, at(31, 8, 0, 10)), &calendar);
        book.apply(&prompt, &calendar);
        assert_eq!(book.tally(date(31), &claude()).turns, 1);
        assert_eq!(book.open_turns(), 0);
    }

    #[test]
    fn a_restart_during_a_turn_keeps_one_turn_and_stops_its_time_at_the_last_event() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("usage").join("ledger.json");
        let calendar = calendar();
        let mut ledger = Ledger::load(&path, &calendar, at(31, 9, 0, 0));
        ledger
            .book_mut()
            .apply(&signal("s", TurnInput::Prompt, at(31, 9, 0, 0)), &calendar);
        ledger
            .book_mut()
            .apply(&signal("s", TurnInput::Working, at(31, 9, 0, 1)), &calendar);
        ledger.book_mut().apply(
            &signal("s", TurnInput::Blocked, at(31, 9, 0, 21)),
            &calendar,
        );
        ledger.book_mut().apply(
            &signal("s", TurnInput::Working, at(31, 9, 0, 31)),
            &calendar,
        );
        assert!(ledger.flush(Instant::now(), true));

        let mut restarted = Ledger::load(&path, &calendar, at(31, 9, 30, 0));
        restarted
            .book_mut()
            .apply(&signal("s", TurnInput::Prompt, at(31, 9, 0, 0)), &calendar);
        restarted.book_mut().apply(
            &signal("s", TurnInput::Working, at(31, 9, 30, 0)),
            &calendar,
        );
        restarted.book_mut().apply(
            &signal("s", TurnInput::Settled, at(31, 9, 30, 5)),
            &calendar,
        );
        let tally = restarted.book().tally(date(31), &claude());
        assert_eq!(
            tally.turns, 1,
            "the replayed prompt and the resumed working state are one turn"
        );
        assert_eq!(
            tally.active_ms,
            20_000 + 5_000,
            "the interval open at the restart ends at the last event before it"
        );
    }

    #[test]
    fn a_corrupted_ledger_is_moved_aside_and_replaced_by_an_empty_one() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("ledger.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let ledger = Ledger::load(&path, &calendar(), 1_793_481_752_042);
        assert!(ledger.book().days().is_empty());
        assert!(!path.exists());
        assert!(home.path().join("ledger.json.bad-1793481752042").is_file());
    }

    #[test]
    fn the_ledger_keeps_only_counters_and_prunes_days_older_than_four_hundred() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("ledger.json");
        let calendar = calendar();
        let today = at(31, 12, 0, 0);
        let mut ledger = Ledger::load(&path, &calendar, today);
        let accounts: Vec<LedgerAccount> = (0..4)
            .map(|index| LedgerAccount {
                agent: if index % 2 == 0 {
                    "claude-code"
                } else {
                    "codex"
                }
                .to_string(),
                config_dir: format!("C:\\Users\\someone-with-a-long-name\\.agent-config-{index}"),
            })
            .collect();
        let day_ms = 86_400_000;
        for day in 0..420_u64 {
            for (index, account) in accounts.iter().enumerate() {
                let start = today - day * day_ms;
                let session = format!("{day}-{index}");
                for (input, offset) in [(TurnInput::Prompt, 0), (TurnInput::Stop, 61_000)] {
                    ledger.book_mut().apply(
                        &TurnSignal {
                            session: session.clone(),
                            account: account.clone(),
                            input,
                            at_ms: start + offset,
                            order: 0,
                        },
                        &calendar,
                    );
                }
            }
        }
        ledger.book_mut().watermarks.clear();
        assert!(ledger.flush(Instant::now(), true));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() < 1024 * 1024, "{} bytes", bytes.len());
        let text = String::from_utf8(bytes).unwrap();
        for absent in ["prompt", "title", "cwd", "transcript"] {
            assert!(!text.contains(absent), "{absent} in the ledger");
        }

        let reloaded = Ledger::load(&path, &calendar, today);
        let days = reloaded.book().days();
        assert_eq!(days.len(), 400);
        let oldest = *days.keys().next().unwrap();
        assert_eq!((calendar.day(today) - oldest).num_days(), 399);
        let tally = reloaded.book().tally(calendar.day(today), &accounts[1]);
        assert_eq!(
            tally,
            DayTally {
                turns: 1,
                active_ms: 61_000,
                tokens_in: 0,
                tokens_out: 0
            }
        );
    }

    #[test]
    fn writes_are_spaced_by_sixty_seconds_unless_forced() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("ledger.json");
        let calendar = calendar();
        let mut ledger = Ledger::load(&path, &calendar, at(31, 9, 0, 0));
        let start = Instant::now();
        assert!(!ledger.flush(start, false), "nothing changed yet");
        ledger
            .book_mut()
            .apply(&signal("a", TurnInput::Prompt, at(31, 9, 0, 0)), &calendar);
        assert!(ledger.flush(start, false));
        ledger
            .book_mut()
            .apply(&signal("b", TurnInput::Prompt, at(31, 9, 0, 1)), &calendar);
        assert!(!ledger.flush(start + Duration::from_secs(59), false));
        assert!(ledger.flush(start + Duration::from_secs(60), false));
        ledger
            .book_mut()
            .apply(&signal("c", TurnInput::Prompt, at(31, 9, 0, 2)), &calendar);
        assert!(
            ledger.flush(start + Duration::from_secs(61), true),
            "the stop flush is forced"
        );
    }
}
