use std::cmp::Ordering;

use serde_json::Value;

pub const SESSION_WINDOW_SECS: u64 = 5 * 3600;
pub const WEEK_WINDOW_SECS: u64 = 7 * DAY_SECS;
const DAY_SECS: u64 = 86_400;
pub const MAX_RESET_DISTANCE_SECS: u64 = 8 * DAY_SECS;
const MONTH_MIN_SECS: u64 = 28 * DAY_SECS;
const WEEK_MIN_SECS: u64 = 5 * DAY_SECS;
const WARN_FRACTION: f64 = 0.80;
const CRITICAL_FRACTION: f64 = 0.95;
const MIN_ELAPSED_FRACTION: f64 = 0.08;
const QUARTER_HOUR_SECS: f64 = 900.0;
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WindowLabel {
    Session,
    Week,
    Month,
}

impl WindowLabel {
    pub const fn for_duration_secs(secs: u64) -> Self {
        if secs >= MONTH_MIN_SECS {
            Self::Month
        } else if secs >= WEEK_MIN_SECS {
            Self::Week
        } else {
            Self::Session
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Week => "Week",
            Self::Month => "Month",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsageLevel {
    Normal,
    Warn,
    Critical,
}

impl UsageLevel {
    pub fn for_fraction(fraction: f64) -> Self {
        let fraction = bounded_fraction(fraction);
        if fraction >= CRITICAL_FRACTION {
            Self::Critical
        } else if fraction >= WARN_FRACTION {
            Self::Warn
        } else {
            Self::Normal
        }
    }
}

fn bounded_fraction(fraction: f64) -> f64 {
    if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_secs: u64,
    pub resets_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowState {
    Live,
    ResetSinceSnapshot,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    ReachesLimit { at: u64, before_reset_secs: u64 },
    OnTrack { percent_at_reset: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowReading {
    pub label: WindowLabel,
    pub fraction: f64,
    pub level: UsageLevel,
    pub state: WindowState,
    pub resets_at: u64,
    pub projection: Option<Projection>,
}

impl LimitWindow {
    pub fn new(used_percent: f64, window_secs: u64, resets_at: u64) -> Self {
        Self {
            used_percent: bounded_fraction(used_percent / 100.0) * 100.0,
            window_secs,
            resets_at,
        }
    }

    pub const fn label(&self) -> WindowLabel {
        WindowLabel::for_duration_secs(self.window_secs)
    }

    pub fn fraction(&self) -> f64 {
        bounded_fraction(self.used_percent / 100.0)
    }

    pub fn read(&self, now: u64) -> WindowReading {
        let label = self.label();
        if self.resets_at <= now {
            return WindowReading {
                label,
                fraction: 0.0,
                level: UsageLevel::Normal,
                state: WindowState::ResetSinceSnapshot,
                resets_at: self.resets_at,
                projection: None,
            };
        }
        let fraction = self.fraction();
        WindowReading {
            label,
            fraction,
            level: UsageLevel::for_fraction(fraction),
            state: WindowState::Live,
            resets_at: self.resets_at,
            projection: if label == WindowLabel::Week {
                self.project(now)
            } else {
                None
            },
        }
    }

    fn project(&self, now: u64) -> Option<Projection> {
        if self.window_secs == 0 || self.resets_at <= now {
            return None;
        }
        let start = self.resets_at.checked_sub(self.window_secs)?;
        let elapsed_secs = now.checked_sub(start)?;
        let elapsed = elapsed_secs as f64 / self.window_secs as f64;
        let used = self.fraction();
        if elapsed < MIN_ELAPSED_FRACTION || used <= 0.0 {
            return None;
        }
        if used / elapsed >= 1.0 {
            let reached = start as f64 + elapsed_secs as f64 / used;
            let at = ((reached / QUARTER_HOUR_SECS).round() * QUARTER_HOUR_SECS) as u64;
            let at = at.min(self.resets_at);
            Some(Projection::ReachesLimit {
                at,
                before_reset_secs: self.resets_at - at,
            })
        } else {
            Some(Projection::OnTrack {
                percent_at_reset: used / elapsed * 100.0,
            })
        }
    }
}

pub fn binding_window<'a>(
    readings: impl IntoIterator<Item = &'a WindowReading>,
) -> Option<&'a WindowReading> {
    readings.into_iter().fold(None, |best, reading| match best {
        None => Some(reading),
        Some(best) => match reading.fraction.total_cmp(&best.fraction) {
            Ordering::Greater => Some(reading),
            Ordering::Equal if reading.label > best.label => Some(reading),
            _ => Some(best),
        },
    })
}

pub fn reset_text(resets_at: u64, now: u64, utc_offset_secs: i32) -> String {
    let remaining = resets_at.saturating_sub(now);
    if remaining < DAY_SECS {
        format!("in {}", duration_text(remaining))
    } else {
        weekday_clock_text(resets_at, utc_offset_secs)
    }
}

pub fn duration_text(secs: u64) -> String {
    let minutes = secs / 60;
    let days = minutes / (24 * 60);
    let hours = minutes % (24 * 60) / 60;
    let minutes = minutes % 60;
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {minutes} min")
    } else {
        format!("{minutes} min")
    }
}

pub fn weekday_clock_text(at: u64, utc_offset_secs: i32) -> String {
    let (days, _) = local_parts(at, utc_offset_secs);
    let weekday = WEEKDAYS[(days + 3).rem_euclid(7) as usize];
    format!("{weekday} {}", clock_text(at, utc_offset_secs))
}

pub fn clock_text(at: u64, utc_offset_secs: i32) -> String {
    let (_, seconds) = local_parts(at, utc_offset_secs);
    format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

fn local_parts(at: u64, utc_offset_secs: i32) -> (i64, i64) {
    let local = i64::try_from(at).unwrap_or(i64::MAX) + i64::from(utc_offset_secs);
    let day = DAY_SECS as i64;
    (local.div_euclid(day), local.rem_euclid(day))
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlanLimits {
    pub primary: Option<LimitWindow>,
    pub secondary: Option<LimitWindow>,
}

impl PlanLimits {
    pub fn windows(&self) -> impl Iterator<Item = &LimitWindow> {
        self.primary.iter().chain(self.secondary.iter())
    }
}

pub fn parse_claude_rate_limits(rate_limits: &Value) -> PlanLimits {
    PlanLimits {
        primary: claude_window(rate_limits.get("five_hour"), SESSION_WINDOW_SECS)
            .map(|raw| raw.window),
        secondary: claude_window(rate_limits.get("seven_day"), WEEK_WINDOW_SECS)
            .map(|raw| raw.window),
    }
}

pub fn plausible_claude_rate_limits(rate_limits: &Value, now: u64) -> PlanLimits {
    PlanLimits {
        primary: claude_window(rate_limits.get("five_hour"), SESSION_WINDOW_SECS)
            .and_then(|raw| raw.plausible(now)),
        secondary: claude_window(rate_limits.get("seven_day"), WEEK_WINDOW_SECS)
            .and_then(|raw| raw.plausible(now)),
    }
}

struct RawWindow {
    reported_percent: f64,
    window: LimitWindow,
}

impl RawWindow {
    fn plausible(self, now: u64) -> Option<LimitWindow> {
        let horizon = MAX_RESET_DISTANCE_SECS.max(self.window.window_secs + DAY_SECS);
        let resets_at = self.window.resets_at;
        ((0.0..=100.0).contains(&self.reported_percent)
            && resets_at <= now.saturating_add(horizon)
            && resets_at.saturating_add(MAX_RESET_DISTANCE_SECS) >= now)
            .then_some(self.window)
    }
}

fn claude_window(window: Option<&Value>, window_secs: u64) -> Option<RawWindow> {
    let window = window?;
    let reported_percent = number(window.get("used_percentage")?)?;
    Some(RawWindow {
        reported_percent,
        window: LimitWindow::new(
            reported_percent,
            window_secs,
            timestamp(window.get("resets_at")?)?,
        ),
    })
}

pub fn parse_codex_rate_limits(rate_limits: &Value) -> PlanLimits {
    PlanLimits {
        primary: codex_window(rate_limits.get("primary")).map(|raw| raw.window),
        secondary: codex_window(rate_limits.get("secondary")).map(|raw| raw.window),
    }
}

pub fn plausible_codex_rate_limits(rate_limits: &Value, now: u64) -> PlanLimits {
    PlanLimits {
        primary: codex_window(rate_limits.get("primary")).and_then(|raw| raw.plausible(now)),
        secondary: codex_window(rate_limits.get("secondary")).and_then(|raw| raw.plausible(now)),
    }
}

fn codex_window(window: Option<&Value>) -> Option<RawWindow> {
    let window = window?;
    let minutes = number(window.get("window_minutes")?)?;
    if minutes < 1.0 {
        return None;
    }
    let reported_percent = number(window.get("used_percent")?)?;
    Some(RawWindow {
        reported_percent,
        window: LimitWindow::new(
            reported_percent,
            (minutes * 60.0) as u64,
            timestamp(window.get("resets_at")?)?,
        ),
    })
}

fn number(value: &Value) -> Option<f64> {
    let parsed = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    parsed.is_finite().then_some(parsed)
}

fn timestamp(value: &Value) -> Option<u64> {
    let seconds = number(value)?;
    (seconds >= 0.0 && seconds < u64::MAX as f64).then_some(seconds as u64)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
        let year = if month <= 2 { year - 1 } else { year };
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let shifted_month = (month + 9) % 12;
        let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    fn at(year: i64, month: i64, day: i64, hour: i64, minute: i64) -> u64 {
        u64::try_from(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60)
            .unwrap()
    }

    #[test]
    fn a_window_is_labeled_by_its_duration() {
        assert_eq!(
            WindowLabel::for_duration_secs(28 * DAY_SECS),
            WindowLabel::Month
        );
        assert_eq!(
            WindowLabel::for_duration_secs(30 * DAY_SECS),
            WindowLabel::Month
        );
        assert_eq!(
            WindowLabel::for_duration_secs(28 * DAY_SECS - 1),
            WindowLabel::Week
        );
        assert_eq!(
            WindowLabel::for_duration_secs(5 * DAY_SECS),
            WindowLabel::Week
        );
        assert_eq!(
            WindowLabel::for_duration_secs(5 * DAY_SECS - 1),
            WindowLabel::Session
        );
        assert_eq!(
            WindowLabel::for_duration_secs(SESSION_WINDOW_SECS),
            WindowLabel::Session
        );
    }

    #[test]
    fn levels_switch_at_eighty_and_ninety_five_percent() {
        assert_eq!(UsageLevel::for_fraction(0.7999), UsageLevel::Normal);
        assert_eq!(UsageLevel::for_fraction(0.80), UsageLevel::Warn);
        assert_eq!(UsageLevel::for_fraction(0.9499), UsageLevel::Warn);
        assert_eq!(UsageLevel::for_fraction(0.95), UsageLevel::Critical);
        assert_eq!(UsageLevel::for_fraction(-3.0), UsageLevel::Normal);
        assert_eq!(UsageLevel::for_fraction(f64::NAN), UsageLevel::Normal);
        assert_eq!(UsageLevel::for_fraction(7.0), UsageLevel::Critical);
    }

    #[test]
    fn an_out_of_range_percentage_is_bounded() {
        assert_eq!(LimitWindow::new(140.0, 60, 1).used_percent, 100.0);
        assert_eq!(LimitWindow::new(-5.0, 60, 1).used_percent, 0.0);
        assert_eq!(LimitWindow::new(f64::NAN, 60, 1).used_percent, 0.0);
    }

    #[test]
    fn the_binding_window_is_the_most_used_and_week_wins_a_tie() {
        let now = 1_000;
        let session = LimitWindow::new(40.0, SESSION_WINDOW_SECS, 5_000).read(now);
        let week = LimitWindow::new(40.0, WEEK_WINDOW_SECS, 9_000).read(now);
        let busy = LimitWindow::new(70.0, SESSION_WINDOW_SECS, 5_000).read(now);
        assert_eq!(
            binding_window([&session, &week]).map(|reading| reading.label),
            Some(WindowLabel::Week)
        );
        assert_eq!(
            binding_window([&week, &session]).map(|reading| reading.label),
            Some(WindowLabel::Week)
        );
        assert_eq!(
            binding_window([&week, &busy, &session]).map(|reading| reading.fraction),
            Some(0.70)
        );
        assert!(binding_window(std::iter::empty()).is_none());
    }

    #[test]
    fn the_documented_week_overruns_on_monday_noon() {
        let resets_at = at(2026, 11, 5, 9, 0);
        let now = at(2026, 10, 31, 14, 30);
        let reading = LimitWindow::new(54.0, WEEK_WINDOW_SECS, resets_at).read(now);
        let overrun = match reading.projection {
            Some(Projection::ReachesLimit {
                at,
                before_reset_secs,
            }) => Some((at, before_reset_secs)),
            _ => None,
        };
        let (reached, before_reset_secs) = overrun.expect("an overrun projection");
        assert_eq!(reached, at(2026, 11, 2, 12, 0));
        assert_eq!(weekday_clock_text(reached, 0), "Mon 12:00");
        assert_eq!(before_reset_secs, 2 * 86_400 + 21 * 3600);
        assert_eq!(duration_text(before_reset_secs), "2 d 21 h");
    }

    #[test]
    fn a_slow_week_projects_its_percentage_at_reset() {
        let resets_at = at(2026, 11, 5, 9, 0);
        let now = at(2026, 11, 1, 9, 0);
        let reading = LimitWindow::new(33.0, WEEK_WINDOW_SECS, resets_at).read(now);
        let percent_at_reset = match reading.projection {
            Some(Projection::OnTrack { percent_at_reset }) => Some(percent_at_reset),
            _ => None,
        }
        .expect("an on-track projection");
        assert!((percent_at_reset - 77.0).abs() < 0.01, "{percent_at_reset}");
    }

    #[test]
    fn no_projection_early_in_the_week_without_usage_or_outside_a_week() {
        let resets_at = at(2026, 11, 5, 9, 0);
        let early = resets_at - WEEK_WINDOW_SECS + (WEEK_WINDOW_SECS * 7 / 100);
        assert!(LimitWindow::new(30.0, WEEK_WINDOW_SECS, resets_at)
            .read(early)
            .projection
            .is_none());
        let later = resets_at - WEEK_WINDOW_SECS + (WEEK_WINDOW_SECS * 9 / 100);
        assert!(LimitWindow::new(30.0, WEEK_WINDOW_SECS, resets_at)
            .read(later)
            .projection
            .is_some());
        assert!(LimitWindow::new(0.0, WEEK_WINDOW_SECS, resets_at)
            .read(later)
            .projection
            .is_none());
        assert!(LimitWindow::new(90.0, SESSION_WINDOW_SECS, resets_at)
            .read(resets_at - 60)
            .projection
            .is_none());
    }

    #[test]
    fn a_passed_reset_reads_zero_with_its_own_state() {
        let reading = LimitWindow::new(97.0, WEEK_WINDOW_SECS, 5_000).read(5_000);
        assert_eq!(reading.fraction, 0.0);
        assert_eq!(reading.state, WindowState::ResetSinceSnapshot);
        assert_eq!(reading.level, UsageLevel::Normal);
        assert!(reading.projection.is_none());
        let live = LimitWindow::new(97.0, WEEK_WINDOW_SECS, 5_001).read(5_000);
        assert_eq!(live.state, WindowState::Live);
        assert_eq!(live.level, UsageLevel::Critical);
    }

    #[test]
    fn reset_text_is_relative_under_a_day_and_absolute_beyond() {
        let now = at(2026, 10, 31, 14, 30);
        assert_eq!(
            reset_text(now + 3 * 3600 + 12 * 60, now, 0),
            "in 3 h 12 min"
        );
        assert_eq!(reset_text(now + 25 * 60, now, 0), "in 25 min");
        assert_eq!(reset_text(now + 86_399, now, 0), "in 23 h 59 min");
        assert_eq!(reset_text(at(2026, 11, 5, 9, 0), now, 0), "Thu 09:00");
        assert_eq!(reset_text(at(2026, 11, 5, 9, 0), now, 3600), "Thu 10:00");
        assert_eq!(reset_text(at(2026, 11, 5, 0, 30), now, -3600), "Wed 23:30");
    }

    #[test]
    fn the_claude_parser_tolerates_every_shape() {
        let full = json!({
            "five_hour": {"used_percentage": 13, "resets_at": 1_793_492_400},
            "seven_day": {"used_percentage": 59.5, "resets_at": 1_793_527_200.0}
        });
        let limits = parse_claude_rate_limits(&full);
        assert_eq!(
            limits.primary,
            Some(LimitWindow::new(13.0, SESSION_WINDOW_SECS, 1_793_492_400))
        );
        assert_eq!(
            limits.secondary,
            Some(LimitWindow::new(59.5, WEEK_WINDOW_SECS, 1_793_527_200))
        );

        let strings = json!({
            "five_hour": {"used_percentage": " 42 ", "resets_at": "1793492400"},
            "seven_day": {"used_percentage": null, "resets_at": 1_793_527_200}
        });
        let limits = parse_claude_rate_limits(&strings);
        assert_eq!(limits.primary.map(|window| window.used_percent), Some(42.0));
        assert_eq!(limits.secondary, None);

        let missing = json!({"seven_day": {"used_percentage": 55}});
        assert_eq!(parse_claude_rate_limits(&missing), PlanLimits::default());

        for broken in [
            json!(null),
            json!("rate"),
            json!([1, 2]),
            json!({"five_hour": null, "seven_day": 7}),
            json!({"five_hour": {"used_percentage": "many", "resets_at": -1}}),
            json!({"five_hour": {"used_percentage": true, "resets_at": {}}}),
        ] {
            assert_eq!(parse_claude_rate_limits(&broken), PlanLimits::default());
        }
    }

    #[test]
    fn the_codex_parser_labels_windows_by_duration() {
        let observed = json!({
            "limit_id": "codex",
            "primary": {"used_percent": 2.0, "window_minutes": 43_200, "resets_at": 1_796_073_944},
            "secondary": null,
            "plan_type": "free"
        });
        let limits = parse_codex_rate_limits(&observed);
        let primary = limits.primary.unwrap();
        assert_eq!(primary.label(), WindowLabel::Month);
        assert_eq!(primary.used_percent, 2.0);
        assert_eq!(limits.secondary, None);

        let pro = json!({
            "primary": {"used_percent": "5", "window_minutes": 300.0, "resets_at": "1791541569"},
            "secondary": {"used_percent": 1, "window_minutes": 10_080, "resets_at": 1_791_541_570}
        });
        let limits = parse_codex_rate_limits(&pro);
        assert_eq!(
            limits.primary.map(|window| window.label()),
            Some(WindowLabel::Session)
        );
        assert_eq!(
            limits.secondary.map(|window| window.label()),
            Some(WindowLabel::Week)
        );
        assert_eq!(limits.windows().count(), 2);

        for broken in [
            json!({"primary": {"used_percent": 1, "window_minutes": 0, "resets_at": 1}}),
            json!({"primary": {"used_percent": 1, "resets_at": 1}}),
            json!({"primary": {"window_minutes": 300, "resets_at": 1}}),
            json!({"primary": {"used_percent": 1, "window_minutes": "x", "resets_at": 1}}),
            json!({"secondary": {"used_percent": 1, "window_minutes": 300, "resets_at": null}}),
        ] {
            assert_eq!(parse_codex_rate_limits(&broken), PlanLimits::default());
        }
    }

    #[test]
    fn an_implausible_window_is_dropped_while_its_sibling_is_kept() {
        let now = 1_793_481_752;
        let week = 7 * DAY_SECS;
        let limits = plausible_claude_rate_limits(
            &json!({
                "five_hour": {"used_percentage": 101, "resets_at": now + 3600},
                "seven_day": {"used_percentage": 59, "resets_at": now + week}
            }),
            now,
        );
        assert_eq!(limits.primary, None, "a percentage above 100 is ignored");
        assert_eq!(
            limits.secondary.map(|window| window.used_percent),
            Some(59.0)
        );

        for (percent, resets_at) in [
            (-1.0, now + 3600),
            (10.0, now + 9 * DAY_SECS),
            (10.0, now - 9 * DAY_SECS),
        ] {
            let limits = plausible_claude_rate_limits(
                &json!({"seven_day": {"used_percentage": percent, "resets_at": resets_at}}),
                now,
            );
            assert_eq!(limits, PlanLimits::default(), "{percent} {resets_at}");
        }
        let edge = plausible_claude_rate_limits(
            &json!({"seven_day": {"used_percentage": 100, "resets_at": now + 8 * DAY_SECS}}),
            now,
        );
        assert!(edge.secondary.is_some(), "8 days ahead is still accepted");

        let month = plausible_codex_rate_limits(
            &json!({"primary": {"used_percent": 3.0, "window_minutes": 43_200, "resets_at": now + 29 * DAY_SECS}}),
            now,
        );
        assert_eq!(
            month.primary.map(|window| window.label()),
            Some(WindowLabel::Month),
            "a monthly window may reset up to its own length ahead"
        );
        let too_far = plausible_codex_rate_limits(
            &json!({"primary": {"used_percent": 3.0, "window_minutes": 43_200, "resets_at": now + 32 * DAY_SECS}}),
            now,
        );
        assert_eq!(too_far, PlanLimits::default());
    }
}
