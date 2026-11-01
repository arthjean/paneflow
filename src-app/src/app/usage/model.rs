use std::path::{MAIN_SEPARATOR, Path};

use paneflow_agent_config::usage::accounts::{
    OriginKind, UsageAccount, UsageAgent, account_dir_key,
};
use paneflow_agent_config::usage::model::{
    Projection, UsageLevel, WindowReading, WindowState, clock_text, duration_text, reset_text,
    weekday_clock_text,
};
use paneflow_agent_config::usage::status_line::StatusLineState;
use paneflow_serve::usage::plans::{AccountPlan, PlanState};

pub(crate) const FILTER_MIN_WIDTH: f32 = 120.;
pub(crate) const CAPSULE_FULL_WIDTH: f32 = 75.;
const FOOTER_SETTINGS_WIDTH: f32 = 36.;
const FOOTER_GAP: f32 = 6.;
const BUBBLE_LIST_MAX_HEIGHT: f32 = 420.;
const BUBBLE_LIST_MIN_HEIGHT: f32 = 120.;
const BUBBLE_VIEWPORT_RESERVE: f32 = 200.;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InventoryEntry {
    pub(crate) account: UsageAccount,
    pub(crate) status_line: StatusLineState,
}

pub(crate) struct UsageInputs<'a> {
    pub(crate) inventory: &'a [InventoryEntry],
    pub(crate) plans: &'a [AccountPlan],
    pub(crate) active_space: Option<&'a str>,
    pub(crate) home: Option<&'a Path>,
    pub(crate) now_ms: u64,
    pub(crate) utc_offset_secs: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UsageView {
    pub(crate) capsule: CapsuleView,
    pub(crate) blocks: Vec<AccountBlock>,
    pub(crate) updated: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CapsuleView {
    Binding {
        fraction: f64,
        level: UsageLevel,
        percent: String,
        accessible: String,
        tooltip: String,
    },
    NoData,
}

impl CapsuleView {
    pub(crate) fn accessible_name(&self) -> String {
        match self {
            Self::Binding { accessible, .. } => accessible.clone(),
            Self::NoData => NO_DATA_TOOLTIP.to_string(),
        }
    }

    pub(crate) fn tooltip(&self) -> String {
        match self {
            Self::Binding { tooltip, .. } => tooltip.clone(),
            Self::NoData => NO_DATA_TOOLTIP.to_string(),
        }
    }
}

const NO_DATA_TOOLTIP: &str = "Plan usage: no data yet";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct AccountBlock {
    pub(crate) agent: UsageAgent,
    pub(crate) label: String,
    pub(crate) body: BlockBody,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlockBody {
    Windows(Vec<WindowRow>),
    Message(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WindowRow {
    pub(crate) label: &'static str,
    pub(crate) fraction: f64,
    pub(crate) level: UsageLevel,
    pub(crate) percent: String,
    pub(crate) reset: String,
    pub(crate) reset_note: Option<String>,
    pub(crate) pace: Option<Pace>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Pace {
    ReachesLimit(String),
    OnTrack(String),
}

struct Row<'a> {
    agent: UsageAgent,
    label: String,
    dir: String,
    api_billing: bool,
    status_line: StatusLineState,
    active: bool,
    plan: Option<&'a AccountPlan>,
}

struct Built {
    block: AccountBlock,
    active: bool,
    readings: Vec<WindowReading>,
    observed_at_ms: Option<u64>,
}

pub(crate) fn usage_view(inputs: &UsageInputs<'_>) -> Option<UsageView> {
    if inputs.inventory.is_empty() && inputs.plans.is_empty() {
        return None;
    }
    let now = inputs.now_ms / 1000;
    let built: Vec<Built> = ordered_rows(inputs)
        .into_iter()
        .map(|row| build(row, now, inputs.utc_offset_secs))
        .collect();
    let capsule = capsule(&built);
    let updated = built
        .iter()
        .filter_map(|built| built.observed_at_ms)
        .min()
        .map(|oldest| format!("Updated {}", age_text(inputs.now_ms.saturating_sub(oldest))));
    Some(UsageView {
        capsule,
        blocks: built.into_iter().map(|built| built.block).collect(),
        updated,
    })
}

fn ordered_rows<'a>(inputs: &UsageInputs<'a>) -> Vec<Row<'a>> {
    let in_space = |account: &UsageAccount| {
        inputs.active_space.is_some_and(|space| {
            account
                .origins
                .iter()
                .any(|origin| origin.kind == OriginKind::Space && origin.name == space)
        })
    };
    let space_has_accounts = inputs
        .inventory
        .iter()
        .any(|entry| in_space(&entry.account));
    let active = |account: &UsageAccount| {
        if space_has_accounts {
            in_space(account)
        } else {
            account
                .origins
                .iter()
                .any(|origin| origin.kind == OriginKind::Default)
        }
    };
    let plan_for = |agent: UsageAgent, dir: &Path| {
        let key = account_dir_key(dir);
        inputs.plans.iter().find(|plan| {
            plan.agent == agent.runtime_slug()
                && account_dir_key(Path::new(&plan.config_dir)) == key
        })
    };
    let mut rows: Vec<Row<'a>> = inputs
        .inventory
        .iter()
        .map(|entry| Row {
            agent: entry.account.agent,
            label: entry.account.label.clone(),
            dir: display_dir(&entry.account.config_dir, inputs.home),
            api_billing: entry.account.api_billing,
            status_line: entry.status_line,
            active: active(&entry.account),
            plan: plan_for(entry.account.agent, &entry.account.config_dir),
        })
        .collect();
    for plan in inputs.plans {
        let Some(agent) = UsageAgent::for_runtime_slug(&plan.agent) else {
            continue;
        };
        let dir = Path::new(&plan.config_dir);
        let known = inputs.inventory.iter().any(|entry| {
            entry.account.agent == agent
                && account_dir_key(&entry.account.config_dir) == account_dir_key(dir)
        });
        if !known {
            let shown = display_dir(dir, inputs.home);
            rows.push(Row {
                agent,
                label: shown.clone(),
                dir: shown,
                api_billing: false,
                status_line: StatusLineState::Missing,
                active: false,
                plan: Some(plan),
            });
        }
    }
    let (mut ordered, others): (Vec<Row<'a>>, Vec<Row<'a>>) =
        rows.into_iter().partition(|row| row.active);
    ordered.extend(others);
    ordered
}

fn build(row: Row<'_>, now: u64, offset: i32) -> Built {
    let message = |text: String| Built {
        block: AccountBlock {
            agent: row.agent,
            label: row.label.clone(),
            body: BlockBody::Message(text),
        },
        active: row.active,
        readings: Vec::new(),
        observed_at_ms: None,
    };
    if row.api_billing {
        return message("Billed through an API key. This account has no plan limits.".into());
    }
    let plan = row.plan;
    if plan.is_some_and(|plan| plan.state == PlanState::NoLimitsReported) {
        return message("Claude Code reports no plan limits for this account.".into());
    }
    let mut readings: Vec<WindowReading> = plan
        .into_iter()
        .flat_map(|plan| plan.primary.iter().chain(plan.secondary.iter()))
        .map(|stored| stored.window().read(now))
        .collect();
    readings.sort_by_key(|reading| reading.label);
    let relay_unavailable = matches!(
        row.status_line,
        StatusLineState::HooksDisabled | StatusLineState::ForeignStatusLine
    );
    if readings.is_empty() && row.agent == UsageAgent::ClaudeCode && relay_unavailable {
        return message(format!(
            "Claude Code has not reported plan limits for {} yet. Check that it is installed and signed in for this folder, then reopen this panel.",
            row.dir
        ));
    }
    if readings.is_empty() {
        return message(match row.agent {
            UsageAgent::ClaudeCode => "No plan data yet. Send a message to Claude Code in a Paneflow pane to load your limits.".into(),
            UsageAgent::Codex => "No plan data yet. Finish a Codex turn in a Paneflow pane to load your limits.".into(),
        });
    }
    let rows = readings
        .iter()
        .map(|reading| window_row(reading, now, offset))
        .collect();
    Built {
        block: AccountBlock {
            agent: row.agent,
            label: row.label,
            body: BlockBody::Windows(rows),
        },
        active: row.active,
        readings,
        observed_at_ms: plan.map(|plan| plan.received_at_ms.unwrap_or(plan.updated_at_ms)),
    }
}

fn window_row(reading: &WindowReading, now: u64, offset: i32) -> WindowRow {
    let reset_note = (reading.state == WindowState::ResetSinceSnapshot).then(|| {
        format!(
            "Reset at {}. Updates at your next message.",
            clock_text(reading.resets_at, offset)
        )
    });
    let reset = match &reset_note {
        Some(note) => note.clone(),
        None => format!("Resets {}", reset_text(reading.resets_at, now, offset)),
    };
    WindowRow {
        label: reading.label.as_str(),
        fraction: reading.fraction,
        level: reading.level,
        percent: percent_text(reading.fraction),
        reset,
        reset_note,
        pace: reading.projection.map(|projection| match projection {
            Projection::ReachesLimit {
                at,
                before_reset_secs,
            } => Pace::ReachesLimit(format!(
                "At this pace you reach the weekly limit {}, {} before it resets.",
                weekday_clock_text(at, offset),
                duration_text(before_reset_secs)
            )),
            Projection::OnTrack { percent_at_reset } => Pace::OnTrack(format!(
                "On track: about {}% of the week used when it resets.",
                percent_at_reset.round() as u32
            )),
        }),
    }
}

fn capsule(built: &[Built]) -> CapsuleView {
    let mut binding: Option<(&Built, &WindowReading)> = None;
    for candidate in built.iter().filter(|built| built.active) {
        for reading in &candidate.readings {
            let wins = binding.is_none_or(|(_, best)| {
                reading.fraction > best.fraction
                    || (reading.fraction == best.fraction && reading.label > best.label)
            });
            if wins {
                binding = Some((candidate, reading));
            }
        }
    }
    let Some((owner, reading)) = binding else {
        return CapsuleView::NoData;
    };
    let percent = percent_text(reading.fraction);
    let window = reading.label.as_str();
    CapsuleView::Binding {
        fraction: reading.fraction,
        level: reading.level,
        accessible: format!(
            "{percent} plan usage, {} {}",
            owner.block.agent.display_name(),
            window.to_lowercase()
        ),
        tooltip: format!("Plan usage: {}, {window} {percent}", owner.block.label),
        percent,
    }
}

pub(crate) fn percent_text(fraction: f64) -> String {
    format!("{}%", (fraction.clamp(0., 1.) * 100.).round() as u32)
}

pub(crate) fn age_text(age_ms: u64) -> String {
    let minutes = age_ms / 60_000;
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes} min ago")
    } else if minutes < 24 * 60 {
        format!("{} h ago", minutes / 60)
    } else {
        format!("{} d ago", minutes / (24 * 60))
    }
}

fn display_dir(dir: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| dir.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~{MAIN_SEPARATOR}{}", rest.display()),
        None => dir.display().to_string(),
    }
}

pub(crate) fn capsule_shows_percent(footer_width: f32) -> bool {
    footer_width - FOOTER_SETTINGS_WIDTH - 2. * FOOTER_GAP - CAPSULE_FULL_WIDTH >= FILTER_MIN_WIDTH
}

pub(crate) fn bubble_list_max_height(viewport_height: f32) -> f32 {
    (viewport_height - BUBBLE_VIEWPORT_RESERVE)
        .clamp(BUBBLE_LIST_MIN_HEIGHT, BUBBLE_LIST_MAX_HEIGHT)
}

pub(crate) fn step_cursor(cursor: Option<usize>, stops: usize, down: bool) -> Option<usize> {
    let last = stops.checked_sub(1)?;
    Some(match (cursor, down) {
        (None, true) => 0,
        (None, false) => last,
        (Some(at), true) => (at + 1).min(last),
        (Some(at), false) => at.saturating_sub(1),
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use paneflow_agent_config::usage::accounts::OriginRef;
    use paneflow_serve::usage::plans::StoredWindow;

    use super::*;

    const NOW_MS: u64 = 1_793_457_000_000;
    const WEEK_RESET: u64 = 1_793_869_200;
    const WEEK: u64 = 7 * 86_400;
    const SESSION: u64 = 5 * 3600;

    fn account(
        agent: UsageAgent,
        dir: &str,
        label: &str,
        origins: &[(OriginKind, &str)],
    ) -> UsageAccount {
        UsageAccount {
            agent,
            config_dir: PathBuf::from(dir),
            origins: origins
                .iter()
                .map(|(kind, name)| OriginRef {
                    kind: *kind,
                    name: (*name).to_string(),
                })
                .collect(),
            label: label.to_string(),
            api_billing: false,
        }
    }

    fn entry(account: UsageAccount) -> InventoryEntry {
        InventoryEntry {
            account,
            status_line: StatusLineState::Managed,
        }
    }

    fn window(used: f64, window_secs: u64, resets_at: u64) -> StoredWindow {
        StoredWindow {
            used_percent: used,
            window_secs,
            resets_at,
        }
    }

    fn plan(
        agent: UsageAgent,
        dir: &str,
        windows: [Option<StoredWindow>; 2],
        received_ms: u64,
    ) -> AccountPlan {
        AccountPlan {
            agent: agent.runtime_slug().to_string(),
            config_dir: dir.to_string(),
            label: dir.to_string(),
            state: PlanState::Reported,
            primary: windows[0],
            secondary: windows[1],
            received_at_ms: Some(received_ms),
            agent_version: None,
            updated_at_ms: received_ms,
        }
    }

    fn view(inventory: &[InventoryEntry], plans: &[AccountPlan], space: Option<&str>) -> UsageView {
        usage_view(&UsageInputs {
            inventory,
            plans,
            active_space: space,
            home: None,
            now_ms: NOW_MS,
            utc_offset_secs: 0,
        })
        .expect("accounts give a view")
    }

    fn message(block: &AccountBlock) -> &str {
        match &block.body {
            BlockBody::Message(text) => text,
            BlockBody::Windows(_) => panic!("expected a message for {}", block.label),
        }
    }

    fn claude_default() -> UsageAccount {
        account(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            "arthur@example.com",
            &[(OriginKind::Default, "Default")],
        )
    }

    #[test]
    fn test_the_capsule_shows_the_most_used_window_of_the_active_accounts() {
        let inventory = [entry(claude_default())];
        let plans = [plan(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            [
                Some(window(2., SESSION, NOW_MS / 1000 + 3 * 3600)),
                Some(window(54., WEEK, WEEK_RESET)),
            ],
            NOW_MS - 120_000,
        )];
        let view = view(&inventory, &plans, None);
        assert_eq!(
            view.capsule,
            CapsuleView::Binding {
                fraction: 0.54,
                level: UsageLevel::Normal,
                percent: "54%".into(),
                accessible: "54% plan usage, Claude Code week".into(),
                tooltip: "Plan usage: arthur@example.com, Week 54%".into(),
            }
        );
        assert_eq!(view.updated.as_deref(), Some("Updated 2 min ago"));
    }

    #[test]
    fn test_a_tie_between_session_and_week_binds_the_week() {
        let inventory = [entry(claude_default())];
        let plans = [plan(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            [
                Some(window(40., SESSION, NOW_MS / 1000 + 3600)),
                Some(window(40., WEEK, WEEK_RESET)),
            ],
            NOW_MS,
        )];
        let view = view(&inventory, &plans, None);
        assert!(view.capsule.accessible_name().ends_with("Claude Code week"));
    }

    #[test]
    fn test_the_active_space_accounts_bind_the_capsule_and_lead_the_bubble() {
        let work = account(
            UsageAgent::ClaudeCode,
            "/home/a/.claude-work",
            "me@work.example",
            &[(OriginKind::Space, "Work")],
        );
        let inventory = [entry(claude_default()), entry(work)];
        let plans = [
            plan(
                UsageAgent::ClaudeCode,
                "/home/a/.claude",
                [None, Some(window(90., WEEK, WEEK_RESET))],
                NOW_MS,
            ),
            plan(
                UsageAgent::ClaudeCode,
                "/home/a/.claude-work",
                [None, Some(window(12., WEEK, WEEK_RESET))],
                NOW_MS,
            ),
        ];
        let in_work = view(&inventory, &plans, Some("Work"));
        assert!(in_work.capsule.accessible_name().starts_with("12% "));
        assert_eq!(in_work.blocks[0].label, "me@work.example");
        assert_eq!(in_work.blocks[1].label, "arthur@example.com");

        let elsewhere = view(&inventory, &plans, Some("Personal"));
        assert!(
            elsewhere.capsule.accessible_name().starts_with("90% "),
            "a space without its own accounts falls back to the default ones"
        );
        assert_eq!(elsewhere.blocks[0].label, "arthur@example.com");
    }

    #[test]
    fn test_accounts_without_a_snapshot_show_the_track_only() {
        let inventory = [entry(claude_default())];
        let view = view(&inventory, &[], None);
        assert_eq!(view.capsule, CapsuleView::NoData);
        assert_eq!(view.capsule.tooltip(), "Plan usage: no data yet");
        assert_eq!(view.updated, None);
        assert_eq!(
            message(&view.blocks[0]),
            "No plan data yet. Send a message to Claude Code in a Paneflow pane to load your limits."
        );
    }

    #[test]
    fn test_no_account_hides_the_capsule() {
        assert!(
            usage_view(&UsageInputs {
                inventory: &[],
                plans: &[],
                active_space: None,
                home: None,
                now_ms: NOW_MS,
                utc_offset_secs: 0,
            })
            .is_none()
        );
    }

    #[test]
    fn test_every_account_state_names_its_cause_and_never_colors_the_capsule() {
        let held = |dir: &str| {
            plan(
                UsageAgent::ClaudeCode,
                dir,
                [None, Some(window(99., WEEK, WEEK_RESET))],
                NOW_MS,
            )
        };
        let mut foreign = entry(account(
            UsageAgent::ClaudeCode,
            "/c/foreign",
            "foreign",
            &[(OriginKind::Default, "Default")],
        ));
        foreign.status_line = StatusLineState::ForeignStatusLine;
        let mut hooks_off = entry(account(
            UsageAgent::ClaudeCode,
            "/c/off",
            "off",
            &[(OriginKind::Default, "Default")],
        ));
        hooks_off.status_line = StatusLineState::HooksDisabled;
        let mut billed = entry(account(
            UsageAgent::ClaudeCode,
            "/c/api",
            "api",
            &[(OriginKind::Default, "Default")],
        ));
        billed.account.api_billing = true;
        let silent = entry(account(
            UsageAgent::ClaudeCode,
            "/c/team",
            "team",
            &[(OriginKind::Default, "Default")],
        ));
        let codex = entry(account(
            UsageAgent::Codex,
            "/c/codex",
            "codex",
            &[(OriginKind::Default, "Default")],
        ));
        let mut no_limits = held("/c/team");
        no_limits.state = PlanState::NoLimitsReported;
        let mut no_codex = plan(UsageAgent::Codex, "/c/codex", [None, None], NOW_MS);
        no_codex.state = PlanState::NoCodexData;
        let inventory = [foreign, hooks_off, billed, silent, codex];
        let plans = [held("/c/api"), no_limits, no_codex];
        let view = view(&inventory, &plans, None);
        let texts: Vec<&str> = view.blocks.iter().map(message).collect();
        assert_eq!(
            texts,
            [
                "Claude Code has not reported plan limits for /c/foreign yet. Check that it is installed and signed in for this folder, then reopen this panel.",
                "Claude Code has not reported plan limits for /c/off yet. Check that it is installed and signed in for this folder, then reopen this panel.",
                "Billed through an API key. This account has no plan limits.",
                "Claude Code reports no plan limits for this account.",
                "No plan data yet. Finish a Codex turn in a Paneflow pane to load your limits.",
            ]
        );
        assert_eq!(
            view.capsule,
            CapsuleView::NoData,
            "no level comes from an account in these states"
        );
        for text in texts {
            assert!(!text.contains('!'));
            assert!(text.chars().next().is_some_and(char::is_uppercase));
        }
    }

    #[test]
    fn test_a_probed_snapshot_shows_its_gauges_despite_a_foreign_status_line_or_disabled_hooks() {
        let mut foreign = entry(account(
            UsageAgent::ClaudeCode,
            "/c/foreign",
            "foreign",
            &[(OriginKind::Default, "Default")],
        ));
        foreign.status_line = StatusLineState::ForeignStatusLine;
        let mut hooks_off = entry(account(
            UsageAgent::ClaudeCode,
            "/c/off",
            "off",
            &[(OriginKind::Default, "Default")],
        ));
        hooks_off.status_line = StatusLineState::HooksDisabled;
        let plans = [
            plan(
                UsageAgent::ClaudeCode,
                "/c/foreign",
                [None, Some(window(54., WEEK, WEEK_RESET))],
                NOW_MS,
            ),
            plan(
                UsageAgent::ClaudeCode,
                "/c/off",
                [None, Some(window(20., WEEK, WEEK_RESET))],
                NOW_MS,
            ),
        ];
        let view = view(&[foreign, hooks_off], &plans, None);
        assert!(
            view.blocks
                .iter()
                .all(|block| matches!(block.body, BlockBody::Windows(_))),
            "{:?}",
            view.blocks
        );
        assert!(
            matches!(&view.capsule, CapsuleView::Binding { percent, .. } if percent == "54%"),
            "{:?}",
            view.capsule
        );
    }

    #[test]
    fn test_a_codex_account_keeps_its_held_windows_when_the_latest_rollout_has_none() {
        let inventory = [entry(account(
            UsageAgent::Codex,
            "/c/codex",
            "codex",
            &[(OriginKind::Default, "Default")],
        ))];
        let mut stale = plan(
            UsageAgent::Codex,
            "/c/codex",
            [
                Some(window(3., 30 * 86_400, NOW_MS / 1000 + 20 * 86_400)),
                None,
            ],
            NOW_MS,
        );
        stale.state = PlanState::NoCodexData;
        let view = view(&inventory, &[stale], None);
        let BlockBody::Windows(rows) = &view.blocks[0].body else {
            panic!("the held snapshot stays visible");
        };
        assert_eq!(rows[0].label, "Month");
        assert!(view.capsule.accessible_name().ends_with("Codex month"));
    }

    #[test]
    fn test_a_window_reset_since_its_snapshot_reads_zero_with_a_note() {
        let inventory = [entry(claude_default())];
        let reset = NOW_MS / 1000 - 5 * 3600 - 30 * 60;
        let plans = [plan(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            [Some(window(80., SESSION, reset)), None],
            NOW_MS - 3 * 3_600_000,
        )];
        let view = view(&inventory, &plans, None);
        let BlockBody::Windows(rows) = &view.blocks[0].body else {
            panic!("windows");
        };
        assert_eq!(rows[0].percent, "0%");
        assert_eq!(
            rows[0].reset_note.as_deref(),
            Some("Reset at 09:00. Updates at your next message.")
        );
        assert_eq!(rows[0].pace, None);
        assert_eq!(view.updated.as_deref(), Some("Updated 3 h ago"));
    }

    #[test]
    fn test_the_week_projection_reads_as_a_sentence() {
        let inventory = [entry(claude_default())];
        let over = [plan(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            [None, Some(window(54., WEEK, WEEK_RESET))],
            NOW_MS,
        )];
        let BlockBody::Windows(rows) = &view(&inventory, &over, None).blocks[0].body else {
            panic!("windows");
        };
        assert_eq!(
            rows[0].pace,
            Some(Pace::ReachesLimit(
                "At this pace you reach the weekly limit Mon 12:00, 2 d 21 h before it resets."
                    .into()
            ))
        );
        assert_eq!(rows[0].reset, "Resets Thu 09:00");

        let calm = [plan(
            UsageAgent::ClaudeCode,
            "/home/a/.claude",
            [None, Some(window(21., WEEK, WEEK_RESET))],
            NOW_MS,
        )];
        let BlockBody::Windows(rows) = &view(&inventory, &calm, None).blocks[0].body else {
            panic!("windows");
        };
        assert_eq!(
            rows[0].pace,
            Some(Pace::OnTrack(
                "On track: about 66% of the week used when it resets.".into()
            ))
        );
    }

    #[test]
    fn test_an_account_outside_the_inventory_is_listed_last_under_its_path() {
        let inventory = [entry(claude_default())];
        let plans = [plan(
            UsageAgent::ClaudeCode,
            "/tmp/adhoc-claude",
            [None, Some(window(10., WEEK, WEEK_RESET))],
            NOW_MS,
        )];
        let view = view(&inventory, &plans, None);
        assert_eq!(view.blocks.len(), 2);
        assert_eq!(
            view.blocks[1].label,
            Path::new("/tmp/adhoc-claude").display().to_string()
        );
    }

    #[test]
    fn test_a_long_label_is_kept_whole_for_the_truncating_renderer() {
        let email = format!("{}@example.com", "a".repeat(52));
        assert_eq!(email.len(), 64);
        let inventory = [entry(account(
            UsageAgent::ClaudeCode,
            "/c/long",
            &email,
            &[(OriginKind::Default, "Default")],
        ))];
        assert_eq!(view(&inventory, &[], None).blocks[0].label, email);
    }

    #[test]
    fn test_the_filter_keeps_its_minimum_width_beside_the_full_capsule() {
        let footer =
            crate::app::constants::SIDEBAR_WIDTH - 2. * crate::app::sidebar::SIDEBAR_ROW_MARGIN_X;
        assert!(capsule_shows_percent(footer));
        assert!(!capsule_shows_percent(
            FILTER_MIN_WIDTH + 2. * FOOTER_GAP + FOOTER_SETTINGS_WIDTH + CAPSULE_FULL_WIDTH - 1.
        ));
    }

    #[test]
    fn test_the_bubble_list_shrinks_to_keep_its_header_inside_a_short_window() {
        assert_eq!(bubble_list_max_height(1080.), BUBBLE_LIST_MAX_HEIGHT);
        let minimum_window = bubble_list_max_height(500.);
        assert!(minimum_window + BUBBLE_VIEWPORT_RESERVE <= 500.);
        assert!(minimum_window >= BUBBLE_LIST_MIN_HEIGHT);
        assert_eq!(bubble_list_max_height(100.), BUBBLE_LIST_MIN_HEIGHT);
    }

    #[test]
    fn test_arrow_keys_walk_the_blocks_then_the_settings_row() {
        assert_eq!(step_cursor(None, 3, true), Some(0));
        assert_eq!(step_cursor(Some(0), 3, true), Some(1));
        assert_eq!(step_cursor(Some(2), 3, true), Some(2));
        assert_eq!(step_cursor(Some(0), 3, false), Some(0));
        assert_eq!(step_cursor(None, 3, false), Some(2));
        assert_eq!(step_cursor(None, 0, true), None);
    }
}
