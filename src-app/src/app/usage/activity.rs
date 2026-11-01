use std::path::Path;

use chrono::{Datelike, Days, NaiveDate};
use paneflow_agent_config::runtime_catalog::runtime_by_slug;
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key};
use paneflow_serve::usage::ledger::{DayTally, LedgerAccount, LedgerDays};

use super::model::{InventoryEntry, display_dir};

pub(crate) const WEEKS: usize = 53;
pub(crate) const LEVEL_MIX: [f32; 4] = [0.30, 0.52, 0.76, 1.0];
pub(crate) const NO_ACTIVITY: &str =
    "No activity yet. Paneflow counts turns from agents that run in its panes.";
pub(crate) const NO_TOKEN_DATA: &str = "No token data for this agent.";
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const WEEKDAY_NAMES: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Metric {
    #[default]
    Turns,
    Tokens,
    ActiveTime,
}

impl Metric {
    pub(crate) const ALL: [Self; 3] = [Self::Turns, Self::Tokens, Self::ActiveTime];

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Turns => "Turns",
            Self::Tokens => "Tokens",
            Self::ActiveTime => "Active time",
        }
    }

    const fn noun(self) -> &'static str {
        match self {
            Self::Turns => "turns",
            Self::Tokens => "tokens",
            Self::ActiveTime => "active time",
        }
    }

    fn value(self, tally: &DayTally) -> u64 {
        match self {
            Self::Turns => tally.turns,
            Self::Tokens => tally.tokens_in.saturating_add(tally.tokens_out),
            Self::ActiveTime => tally.active_ms,
        }
    }

    pub(crate) fn amount_text(self, value: u64) -> String {
        match self {
            Self::Turns if value == 1 => "1 turn".into(),
            Self::Turns => format!("{} turns", grouped(value)),
            Self::Tokens if value == 1 => "1 token".into(),
            Self::Tokens => format!("{} tokens", grouped(value)),
            Self::ActiveTime => active_text(value),
        }
    }

    fn short_text(self, value: u64) -> String {
        match self {
            Self::Tokens if value >= 1_000 => format!("{} tokens", compact(value)),
            _ => self.amount_text(value),
        }
    }

    fn total_text(self, value: u64) -> String {
        format!("{} in the last year", self.short_text(value))
    }

    pub(crate) fn step(self, forward: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|metric| *metric == self)
            .unwrap_or(0);
        let next = if forward {
            (index + 1) % Self::ALL.len()
        } else {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }
}

fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn compact(value: u64) -> String {
    let (scaled, suffix) = if value >= 1_000_000_000 {
        (value as f64 / 1e9, "B")
    } else if value >= 1_000_000 {
        (value as f64 / 1e6, "M")
    } else {
        (value as f64 / 1e3, "K")
    };
    let text = if scaled >= 100. {
        format!("{scaled:.0}")
    } else {
        format!("{scaled:.1}")
    };
    format!("{}{suffix}", text.trim_end_matches(".0"))
}

fn active_text(ms: u64) -> String {
    let minutes = ms / 60_000;
    if ms > 0 && minutes == 0 {
        return "under 1 min".into();
    }
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{} h", grouped(hours)),
        (hours, minutes) => format!("{} h {minutes} min", grouped(hours)),
    }
}

pub(crate) struct ActivityInputs<'a> {
    pub(crate) ledger: &'a LedgerDays,
    pub(crate) inventory: &'a [InventoryEntry],
    pub(crate) home: Option<&'a Path>,
    pub(crate) today: NaiveDate,
    pub(crate) metric: Metric,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ActivityView {
    pub(crate) metric: Metric,
    pub(crate) first_day: NaiveDate,
    pub(crate) today: NaiveDate,
    pub(crate) months: Vec<(usize, &'static str)>,
    pub(crate) registers: Vec<Register>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Register {
    pub(crate) slug: String,
    pub(crate) agent_name: String,
    pub(crate) label: String,
    pub(crate) total: String,
    pub(crate) values: Vec<u64>,
    pub(crate) levels: Vec<u8>,
    pub(crate) has_data: bool,
}

impl ActivityView {
    pub(crate) fn is_empty(&self) -> bool {
        self.registers.is_empty()
    }

    pub(crate) fn day_index(&self, day: NaiveDate) -> Option<usize> {
        let offset = day.signed_duration_since(self.first_day).num_days();
        (0..=self.today.signed_duration_since(self.first_day).num_days())
            .contains(&offset)
            .then(|| usize::try_from(offset).ok())
            .flatten()
    }

    pub(crate) fn detail(&self, day: NaiveDate) -> String {
        let index = self.day_index(day);
        let names: Vec<&str> = self
            .registers
            .iter()
            .map(|register| register.agent_name.as_str())
            .collect();
        let parts: Vec<String> = self
            .registers
            .iter()
            .map(|register| {
                let shared = names
                    .iter()
                    .filter(|name| **name == register.agent_name)
                    .count()
                    > 1;
                let name = if shared && !register.label.is_empty() {
                    format!("{} ({})", register.agent_name, register.label)
                } else {
                    register.agent_name.clone()
                };
                let value = index
                    .and_then(|index| register.values.get(index).copied())
                    .unwrap_or(0);
                if value == 0 {
                    format!("{name} none")
                } else {
                    format!("{name} {}", self.metric.short_text(value))
                }
            })
            .collect();
        format!("{}: {}", short_date(day), parts.join(" · "))
    }

    pub(crate) fn cell_name(&self, day: NaiveDate, value: u64) -> String {
        let amount = if value == 0 {
            "no activity".to_string()
        } else {
            self.metric.amount_text(value)
        };
        format!("{}: {amount}", long_date(day))
    }

    pub(crate) fn grid_label(&self, register: &Register) -> String {
        if register.label.is_empty() {
            format!("{}, daily {}", register.agent_name, self.metric.noun())
        } else {
            format!(
                "{}, {}, daily {}",
                register.agent_name,
                register.label,
                self.metric.noun()
            )
        }
    }
}

pub(crate) fn short_date(day: NaiveDate) -> String {
    format!(
        "{}, {} {}",
        WEEKDAYS[day.weekday().num_days_from_monday() as usize],
        MONTHS[day.month0() as usize],
        day.day()
    )
}

pub(crate) fn long_date(day: NaiveDate) -> String {
    format!(
        "{}, {} {}, {}",
        WEEKDAY_NAMES[day.weekday().num_days_from_monday() as usize],
        MONTH_NAMES[day.month0() as usize],
        day.day(),
        day.year()
    )
}

pub(crate) fn first_day(today: NaiveDate) -> NaiveDate {
    let monday = today
        .checked_sub_days(Days::new(u64::from(today.weekday().num_days_from_monday())))
        .unwrap_or(today);
    monday
        .checked_sub_days(Days::new(7 * (WEEKS as u64 - 1)))
        .unwrap_or(monday)
}

fn month_labels(first: NaiveDate) -> Vec<(usize, &'static str)> {
    let mut labels = Vec::new();
    let mut previous = None;
    let mut last_column: Option<usize> = None;
    for column in 0..WEEKS {
        let Some(monday) = first.checked_add_days(Days::new(7 * column as u64)) else {
            break;
        };
        if previous == Some(monday.month0()) {
            continue;
        }
        previous = Some(monday.month0());
        let room = last_column.is_none_or(|last| column - last >= 3);
        if (column > 0 || monday.day() <= 17) && room && column + 2 <= WEEKS {
            labels.push((column, MONTHS[monday.month0() as usize]));
            last_column = Some(column);
        }
    }
    labels
}

fn rank(slug: &str) -> u8 {
    if slug == UsageAgent::ClaudeCode.runtime_slug() {
        0
    } else if slug == UsageAgent::Codex.runtime_slug() {
        1
    } else {
        2
    }
}

pub(crate) fn agent_name(slug: &str) -> String {
    runtime_by_slug(slug).map_or_else(|| slug.to_string(), |runtime| runtime.label.to_string())
}

fn account_label(
    account: &LedgerAccount,
    inventory: &[InventoryEntry],
    home: Option<&Path>,
) -> (Option<usize>, String) {
    if account.config_dir.is_empty() {
        return (None, String::new());
    }
    let key = account_dir_key(Path::new(&account.config_dir));
    let known = inventory.iter().position(|entry| {
        entry.account.agent.runtime_slug() == account.agent
            && account_dir_key(&entry.account.config_dir) == key
    });
    match known {
        Some(index) => (Some(index), inventory[index].account.label.clone()),
        None => (None, display_dir(Path::new(&account.config_dir), home)),
    }
}

pub(crate) fn thresholds(values: impl IntoIterator<Item = u64>) -> [u64; 3] {
    let mut active: Vec<u64> = values.into_iter().filter(|value| *value > 0).collect();
    if active.is_empty() {
        return [1, 1, 1];
    }
    active.sort_unstable();
    let quantile = |fraction: f64| {
        let at = ((fraction * active.len() as f64).floor() as usize).min(active.len() - 1);
        active[at]
    };
    [quantile(0.25), quantile(0.5), quantile(0.75)]
}

pub(crate) fn level(value: u64, thresholds: [u64; 3]) -> u8 {
    match value {
        0 => 0,
        value if value <= thresholds[0] => 1,
        value if value <= thresholds[1] => 2,
        value if value <= thresholds[2] => 3,
        _ => 4,
    }
}

pub(crate) fn activity_view(inputs: &ActivityInputs<'_>) -> ActivityView {
    let first = first_day(inputs.today);
    let span =
        usize::try_from(inputs.today.signed_duration_since(first).num_days() + 1).unwrap_or(0);
    let mut rows: Vec<((u8, String, usize, String), Register)> = Vec::new();
    for (account, days) in inputs.ledger {
        let window = days.range(first..=inputs.today);
        if !window
            .clone()
            .any(|(_, tally)| Metric::ALL.iter().any(|metric| metric.value(tally) > 0))
        {
            continue;
        }
        let mut values = vec![0; span];
        for (day, tally) in window {
            if let Ok(offset) = usize::try_from(day.signed_duration_since(first).num_days())
                && let Some(slot) = values.get_mut(offset)
            {
                *slot = inputs.metric.value(tally);
            }
        }
        let total: u64 = values.iter().copied().fold(0, u64::saturating_add);
        let name = agent_name(&account.agent);
        let (position, label) = account_label(account, inputs.inventory, inputs.home);
        let key = (
            rank(&account.agent),
            if rank(&account.agent) == 2 {
                name.to_lowercase()
            } else {
                String::new()
            },
            position.unwrap_or(usize::MAX),
            account.config_dir.clone(),
        );
        rows.push((
            key,
            Register {
                slug: account.agent.clone(),
                agent_name: name,
                label,
                total: inputs.metric.total_text(total),
                values,
                levels: Vec::new(),
                has_data: total > 0 || inputs.metric != Metric::Tokens,
            },
        ));
    }
    rows.sort_by(|left, right| left.0.cmp(&right.0));
    let mut registers: Vec<Register> = rows.into_iter().map(|(_, register)| register).collect();
    let scale = thresholds(
        registers
            .iter()
            .filter(|register| register.has_data)
            .flat_map(|register| register.values.iter().copied()),
    );
    for register in &mut registers {
        register.levels = register
            .values
            .iter()
            .map(|value| level(*value, scale))
            .collect();
    }
    ActivityView {
        metric: inputs.metric,
        first_day: first,
        today: inputs.today,
        months: month_labels(first),
        registers,
    }
}

pub(crate) fn step_day(view: &ActivityView, day: NaiveDate, key: &str) -> NaiveDate {
    let row = u64::from(day.weekday().num_days_from_monday());
    let target = match key {
        "left" => day.checked_sub_days(Days::new(7)),
        "right" => day.checked_add_days(Days::new(7)),
        "up" if row > 0 => day.checked_sub_days(Days::new(1)),
        "down" if row < 6 => day.checked_add_days(Days::new(1)),
        "home" => view.first_day.checked_add_days(Days::new(row)),
        "end" => {
            let last_week = view
                .first_day
                .checked_add_days(Days::new(7 * (WEEKS as u64 - 1) + row));
            last_week.and_then(|candidate| {
                if candidate > view.today {
                    candidate.checked_sub_days(Days::new(7))
                } else {
                    Some(candidate)
                }
            })
        }
        _ => None,
    };
    target
        .filter(|target| view.day_index(*target).is_some())
        .unwrap_or(day)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use paneflow_agent_config::usage::accounts::{OriginKind, OriginRef, UsageAccount};
    use paneflow_agent_config::usage::status_line::StatusLineState;

    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    const TODAY: (i32, u32, u32) = (2026, 8, 18);

    fn today() -> NaiveDate {
        date(TODAY.0, TODAY.1, TODAY.2)
    }

    fn account(agent: &str, dir: &str) -> LedgerAccount {
        LedgerAccount {
            agent: agent.into(),
            config_dir: dir.into(),
        }
    }

    fn turns(turns: u64) -> DayTally {
        DayTally {
            turns,
            active_ms: turns * 60_000,
            ..DayTally::default()
        }
    }

    fn inventory_entry(agent: UsageAgent, dir: &str, label: &str) -> InventoryEntry {
        InventoryEntry {
            account: UsageAccount {
                agent,
                config_dir: PathBuf::from(dir),
                origins: vec![OriginRef {
                    kind: OriginKind::Default,
                    name: "Default".into(),
                }],
                label: label.into(),
                api_billing: false,
            },
            status_line: StatusLineState::Managed,
        }
    }

    fn view(ledger: &LedgerDays, inventory: &[InventoryEntry], metric: Metric) -> ActivityView {
        activity_view(&ActivityInputs {
            ledger,
            inventory,
            home: None,
            today: today(),
            metric,
        })
    }

    fn sample() -> LedgerDays {
        let mut ledger = LedgerDays::new();
        ledger.insert(
            account("gemini", ""),
            BTreeMap::from([(date(2026, 8, 10), turns(3))]),
        );
        ledger.insert(
            account("codex", "/home/a/.codex"),
            BTreeMap::from([(today(), turns(12))]),
        );
        ledger.insert(
            account("claude-code", "/home/a/.claude"),
            BTreeMap::from([
                (today(), turns(41)),
                (date(2026, 8, 17), turns(1_243)),
                (date(2025, 1, 1), turns(99)),
            ]),
        );
        ledger.insert(
            account("opencode", ""),
            BTreeMap::from([(date(2024, 1, 1), turns(5))]),
        );
        ledger
    }

    #[test]
    fn the_grid_spans_fifty_three_weeks_from_a_monday_and_stops_today() {
        let first = first_day(today());
        assert_eq!(first.weekday(), chrono::Weekday::Mon);
        assert_eq!(
            today().signed_duration_since(first).num_days(),
            52 * 7 + 1,
            "2026-08-18 is a Tuesday in the last column"
        );
        let view = view(&sample(), &[], Metric::Turns);
        assert!(
            view.registers
                .iter()
                .all(|register| register.values.len() == 52 * 7 + 2)
        );
        assert_eq!(view.day_index(today().succ_opt().unwrap()), None);
    }

    #[test]
    fn registers_list_active_accounts_claude_first_then_codex_then_by_name() {
        let inventory = [inventory_entry(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            "arthur@example.com",
        )];
        let view = view(&sample(), &inventory, Metric::Turns);
        let order: Vec<(&str, &str)> = view
            .registers
            .iter()
            .map(|register| (register.agent_name.as_str(), register.label.as_str()))
            .collect();
        assert_eq!(order[0], ("Claude Code", "arthur@example.com"));
        assert_eq!(order[1].0, "Codex");
        assert_eq!(order.len(), 3, "an account idle for a year is left out");
        assert_eq!(view.registers[0].total, "1,284 turns in the last year");
    }

    #[test]
    fn the_scale_is_shared_through_the_quartiles_of_every_active_day() {
        assert_eq!(thresholds([0, 0]), [1, 1, 1]);
        assert_eq!(thresholds([4, 1, 0, 3, 2]), [2, 3, 4]);
        let view = view(&sample(), &[], Metric::Turns);
        let codex = &view.registers[1];
        let today_index = view.day_index(today()).unwrap();
        assert_eq!(
            codex.levels[today_index], 1,
            "12 turns is the lowest quartile of [3, 12, 41, 1243]"
        );
        assert_eq!(view.registers[0].levels[today_index], 2);
        assert_eq!(view.registers[0].levels[today_index - 1], 3);
        assert_eq!(view.registers[2].levels[today_index], 0);
        assert_eq!(level(0, [1, 2, 3]), 0);
    }

    #[test]
    fn the_detail_line_and_cell_names_spell_the_day_and_its_values() {
        let view = view(&sample(), &[], Metric::Turns);
        assert_eq!(
            view.detail(today()),
            "Tue, Aug 18: Claude Code 41 turns · Codex 12 turns · Gemini none"
        );
        assert_eq!(
            view.cell_name(today(), 41),
            "Tuesday, August 18, 2026: 41 turns"
        );
        assert_eq!(
            view.cell_name(today(), 0),
            "Tuesday, August 18, 2026: no activity"
        );
        assert_eq!(view.grid_label(&view.registers[2]), "Gemini, daily turns");
        assert_eq!(
            view.grid_label(&view.registers[1]),
            format!("Codex, {}, daily turns", view.registers[1].label),
            "two accounts of one agent get distinct grid names"
        );
    }

    #[test]
    fn a_register_without_tokens_says_so_under_the_tokens_metric() {
        let mut ledger = sample();
        ledger.insert(
            account("claude-code", "/home/a/.claude"),
            BTreeMap::from([(
                today(),
                DayTally {
                    turns: 2,
                    tokens_in: 1_200,
                    tokens_out: 300,
                    ..DayTally::default()
                },
            )]),
        );
        let view = view(&ledger, &[], Metric::Tokens);
        assert!(view.registers[0].has_data);
        assert_eq!(view.registers[0].total, "1.5K tokens in the last year");
        assert!(!view.registers[1].has_data, "Codex logged no tokens");
        assert_eq!(
            view.cell_name(today(), 1_500),
            "Tuesday, August 18, 2026: 1,500 tokens"
        );
    }

    #[test]
    fn no_activity_anywhere_leaves_no_register() {
        assert!(view(&LedgerDays::new(), &[], Metric::Turns).is_empty());
    }

    #[test]
    fn amounts_read_naturally() {
        assert_eq!(Metric::Turns.amount_text(1), "1 turn");
        assert_eq!(Metric::ActiveTime.amount_text(30_000), "under 1 min");
        assert_eq!(Metric::ActiveTime.amount_text(80 * 60_000), "1 h 20 min");
        assert_eq!(Metric::ActiveTime.amount_text(120 * 60_000), "2 h");
        assert_eq!(compact(2_400_000), "2.4M");
        assert_eq!(compact(1_000), "1K");
        assert_eq!(Metric::Turns.step(false), Metric::ActiveTime);
    }

    #[test]
    fn month_labels_sit_on_the_first_week_of_each_month_with_room() {
        let labels = month_labels(first_day(today()));
        assert_eq!(
            labels.first(),
            Some(&(2, "Sep")),
            "the grid opens on Aug 18, too late in the month for a label"
        );
        assert!(labels.windows(2).all(|pair| pair[1].0 - pair[0].0 >= 3));
        assert!(labels.iter().all(|(column, _)| column + 2 <= WEEKS));
    }

    #[test]
    fn arrows_walk_days_and_weeks_and_home_end_reach_the_row_ends() {
        let view = view(&sample(), &[], Metric::Turns);
        let today = today();
        assert_eq!(step_day(&view, today, "up"), date(2026, 8, 17));
        assert_eq!(
            step_day(&view, today, "down"),
            today,
            "tomorrow is not drawn"
        );
        assert_eq!(step_day(&view, today, "left"), date(2026, 8, 11));
        assert_eq!(step_day(&view, today, "right"), today);
        assert_eq!(step_day(&view, today, "home"), date(2025, 8, 19));
        assert_eq!(step_day(&view, date(2025, 8, 19), "end"), today);
        assert_eq!(
            step_day(&view, date(2026, 8, 13), "end"),
            date(2026, 8, 13),
            "Thursday of the current week is in the future, so the row ends a week earlier"
        );
        assert_eq!(step_day(&view, date(2025, 8, 18), "up"), date(2025, 8, 18));
    }
}
