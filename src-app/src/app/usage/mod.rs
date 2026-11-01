mod activity;
pub(crate) mod model;
mod page;
mod render;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use gpui::{App, Context, FocusHandle, KeyDownEvent, ScrollHandle, Window};
use paneflow_agent_config::usage::accounts::{
    AccountOrigin, UsageAgent, claude_status_line_state, inventory,
};
use paneflow_agent_config::usage::status_line::StatusLineState;
use paneflow_serve::usage::backfill::ImportStatus;
use paneflow_serve::usage::ledger::{LedgerDays, ledger_days};
use paneflow_serve::usage::plans::AccountPlan;
use paneflow_serve::usage::usage_origins;
use serde_json::Value;

use self::activity::Metric;
use self::model::{InventoryEntry, UsageInputs, UsageView, step_cursor, usage_view};
use crate::{PaneFlowApp, SettingsSection};

pub(crate) struct UsagePanel {
    plans: Vec<AccountPlan>,
    inventory: Vec<InventoryEntry>,
    origins: Option<Vec<AccountOrigin>>,
    inventory_generation: u64,
    utc_offset_secs: i32,
    home: Option<PathBuf>,
    bubble_open: bool,
    cursor: Option<usize>,
    capsule_focus: FocusHandle,
    bubble_focus: FocusHandle,
    ledger: LedgerDays,
    ledger_revision: Option<u64>,
    metric: Metric,
    metric_focus: FocusHandle,
    refresh_focus: FocusHandle,
    open_agents_focus: FocusHandle,
    register_focus: Vec<FocusHandle>,
    register_scroll: Vec<ScrollHandle>,
    register_cursor: Option<(usize, NaiveDate)>,
    hover_day: Option<NaiveDate>,
    import: ImportStatus,
    import_focus: FocusHandle,
}

impl UsagePanel {
    pub(crate) fn new(cx: &mut App) -> Self {
        Self {
            plans: Vec::new(),
            inventory: Vec::new(),
            origins: None,
            inventory_generation: 0,
            utc_offset_secs: 0,
            home: paneflow_agent_config::home_dir(),
            bubble_open: false,
            cursor: None,
            capsule_focus: cx.focus_handle(),
            bubble_focus: cx.focus_handle(),
            ledger: LedgerDays::new(),
            ledger_revision: None,
            metric: Metric::default(),
            metric_focus: cx.focus_handle(),
            refresh_focus: cx.focus_handle(),
            open_agents_focus: cx.focus_handle(),
            register_focus: Vec::new(),
            register_scroll: Vec::new(),
            register_cursor: None,
            hover_day: None,
            import: ImportStatus::default(),
            import_focus: cx.focus_handle(),
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.bubble_open
    }

    pub(crate) fn close(&mut self) {
        self.bubble_open = false;
        self.cursor = None;
    }
}

fn read_inventory(
    origins: &[AccountOrigin],
    home: Option<&std::path::Path>,
) -> Vec<InventoryEntry> {
    inventory(origins, home)
        .into_iter()
        .filter(|account| account.config_dir.is_dir())
        .map(|account| InventoryEntry {
            status_line: match account.agent {
                UsageAgent::ClaudeCode => claude_status_line_state(&account.config_dir),
                UsageAgent::Codex => StatusLineState::Missing,
            },
            account,
        })
        .collect()
}

fn worker_plans(usage: &Value) -> Option<Vec<AccountPlan>> {
    serde_json::from_value(usage.get("plans")?.clone()).ok()
}

fn worker_import(usage: &Value) -> Option<ImportStatus> {
    serde_json::from_value(usage.get("import")?.clone()).ok()
}

struct WorkerUsage {
    plans: Option<Vec<AccountPlan>>,
    ledger: Option<(LedgerDays, Option<u64>)>,
    import: Option<ImportStatus>,
}

fn worker_usage(snapshot: &Value) -> WorkerUsage {
    WorkerUsage {
        plans: worker_plans(snapshot),
        import: worker_import(snapshot),
        ledger: snapshot.get("ledger").and_then(|ledger| {
            Some((
                ledger_days(ledger)?,
                ledger.get("revision").and_then(Value::as_u64),
            ))
        }),
    }
}

fn read_worker_snapshot() -> Option<WorkerUsage> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    match paneflow_serve::Controller::connect(&endpoint)
        .and_then(|mut controller| controller.usage_snapshot())
    {
        Ok(snapshot) => Some(worker_usage(&snapshot)),
        Err(error) => {
            log::debug!("paneflow: the worker did not answer the usage snapshot: {error}");
            None
        }
    }
}

pub(super) fn day_at(now_ms: u64, utc_offset_secs: i32) -> NaiveDate {
    let local_secs = i64::try_from(now_ms / 1000)
        .unwrap_or(i64::MAX)
        .saturating_add(i64::from(utc_offset_secs));
    chrono::DateTime::from_timestamp(local_secs, 0).map_or(NaiveDate::MIN, |at| at.date_naive())
}

fn refresh_worker_usage(entries: &[InventoryEntry]) -> Option<WorkerUsage> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let params = paneflow_serve::usage::probe::refresh_params(
        entries
            .iter()
            .filter(|entry| !entry.account.api_billing)
            .map(|entry| (entry.account.agent, entry.account.config_dir.as_path())),
    );
    match paneflow_serve::Controller::connect(&endpoint)
        .and_then(|mut controller| controller.refresh_usage(params))
    {
        Ok(snapshot) => {
            let usage = worker_usage(&snapshot);
            if usage.plans.is_none() {
                log::debug!("paneflow: the worker refresh carried no plan usage");
            }
            Some(usage)
        }
        Err(error) => {
            log::debug!("paneflow: the worker did not refresh plan usage: {error}");
            None
        }
    }
}

fn import_worker_usage(entries: &[InventoryEntry]) -> Option<WorkerUsage> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let mut params = paneflow_serve::usage::probe::refresh_params(
        entries
            .iter()
            .map(|entry| (entry.account.agent, entry.account.config_dir.as_path())),
    );
    params["force"] = Value::Bool(true);
    match paneflow_serve::Controller::connect(&endpoint)
        .and_then(|mut controller| controller.import_usage(params))
    {
        Ok(snapshot) => Some(worker_usage(&snapshot)),
        Err(error) => {
            log::warn!("paneflow: the worker did not start the activity import: {error}");
            None
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

impl PaneFlowApp {
    pub(crate) fn sidebar_usage_view(&self) -> Option<UsageView> {
        let enabled = self.cached_config.sidebar_show.usage_enabled()
            && self
                .cached_config
                .usage
                .as_ref()
                .is_none_or(|usage| usage.plan_limits_enabled());
        if !enabled {
            return None;
        }
        let active_space = self
            .active_space
            .as_ref()
            .and_then(|id| self.cached_config.space(id))
            .map(|space| space.name.as_str());
        usage_view(&UsageInputs {
            inventory: &self.usage.inventory,
            plans: &self.usage.plans,
            active_space,
            home: self.usage.home.as_deref(),
            now_ms: now_ms(),
            utc_offset_secs: self.usage.utc_offset_secs,
        })
    }

    pub(crate) fn refresh_usage_inventory(&mut self, force: bool, cx: &mut Context<Self>) {
        self.reload_usage_inventory(force, false, cx);
    }

    fn reload_usage_inventory(
        &mut self,
        force: bool,
        refresh_worker: bool,
        cx: &mut Context<Self>,
    ) {
        let origins = usage_origins(&self.cached_config);
        if !force && self.usage.origins.as_ref() == Some(&origins) {
            return;
        }
        self.usage.origins = Some(origins.clone());
        self.usage.inventory_generation += 1;
        let generation = self.usage.inventory_generation;
        let home = self.usage.home.clone();
        cx.spawn(async move |this, cx| {
            let (entries, offset, usage) = smol::unblock(move || {
                let entries = read_inventory(&origins, home.as_deref());
                let offset = paneflow_serve::usage::local_utc_offset_secs(now_ms() / 1000);
                let usage = refresh_worker
                    .then(|| refresh_worker_usage(&entries))
                    .flatten();
                (entries, offset, usage)
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                if let Some(usage) = usage {
                    app.apply_snapshot(usage, cx);
                }
                if app.usage.inventory_generation != generation {
                    return;
                }
                app.usage.utc_offset_secs = offset;
                if app.usage.inventory != entries {
                    app.usage.inventory = entries;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn apply_worker_usage(&mut self, usage: &Value, cx: &mut Context<Self>) {
        if let Some(plans) = worker_plans(usage) {
            self.apply_plans(plans, cx);
        }
        if let Some(import) = worker_import(usage) {
            self.apply_import(import, cx);
        }
        let revision = usage.get("ledger_revision").and_then(Value::as_u64);
        if revision.is_some()
            && revision != self.usage.ledger_revision
            && self.settings_section == Some(SettingsSection::Usage)
        {
            self.reload_usage_ledger(cx);
        }
    }

    fn apply_snapshot(&mut self, usage: WorkerUsage, cx: &mut Context<Self>) {
        if let Some(plans) = usage.plans {
            self.apply_plans(plans, cx);
        }
        if let Some(import) = usage.import {
            self.apply_import(import, cx);
        }
        if let Some((ledger, revision)) = usage.ledger {
            self.usage.ledger_revision = revision;
            while self.usage.register_focus.len() < ledger.len() {
                self.usage.register_focus.push(cx.focus_handle());
                self.usage.register_scroll.push(ScrollHandle::new());
                if let Some(scroll) = self.usage.register_scroll.last() {
                    page::scroll_to_today(scroll);
                }
            }
            if ledger != self.usage.ledger {
                self.usage.ledger = ledger;
                cx.notify();
            }
        }
    }

    pub(crate) fn reload_usage_ledger(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let usage = smol::unblock(read_worker_snapshot).await;
            let _ = this.update(cx, |app, cx| {
                if let Some(usage) = usage {
                    app.apply_snapshot(usage, cx);
                }
            });
        })
        .detach();
    }

    pub(crate) fn open_usage_page(&mut self, cx: &mut Context<Self>) {
        self.usage.hover_day = None;
        self.usage.register_cursor = None;
        for scroll in &self.usage.register_scroll {
            page::scroll_to_today(scroll);
        }
        self.reload_usage_inventory(true, false, cx);
        self.reload_usage_ledger(cx);
    }

    pub(crate) fn refresh_usage_page(&mut self, cx: &mut Context<Self>) {
        self.reload_usage_inventory(true, true, cx);
        cx.notify();
    }

    pub(crate) fn import_past_activity(&mut self, cx: &mut Context<Self>) {
        if self.usage.import.running {
            return;
        }
        self.usage.import.running = true;
        self.usage.import.files = 0;
        cx.notify();
        let entries = self.usage.inventory.clone();
        cx.spawn(async move |this, cx| {
            let usage = smol::unblock(move || import_worker_usage(&entries)).await;
            let _ = this.update(cx, |app, cx| match usage {
                Some(usage) => app.apply_snapshot(usage, cx),
                None => app.apply_import(ImportStatus::default(), cx),
            });
        })
        .detach();
    }

    fn apply_import(&mut self, import: ImportStatus, cx: &mut Context<Self>) {
        if import != self.usage.import {
            self.usage.import = import;
            cx.notify();
        }
    }

    fn apply_plans(&mut self, plans: Vec<AccountPlan>, cx: &mut Context<Self>) {
        if plans != self.usage.plans {
            self.usage.plans = plans;
            cx.notify();
        }
    }

    pub(crate) fn focus_usage_capsule(&self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.sidebar_usage_view().is_none() {
            return false;
        }
        window.focus(&self.usage.capsule_focus, cx);
        true
    }

    pub(crate) fn open_usage_bubble(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.usage.bubble_open {
            return;
        }
        self.dismiss_transient_surfaces();
        self.usage.bubble_open = true;
        self.usage.cursor = None;
        window.focus(&self.usage.bubble_focus, cx);
        self.reload_usage_inventory(true, true, cx);
        cx.notify();
    }

    pub(crate) fn close_usage_bubble(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.usage.bubble_open {
            return;
        }
        let focus_inside = self.usage.bubble_focus.contains_focused(window, cx);
        self.usage.close();
        if focus_inside {
            window.focus(&self.usage.capsule_focus, cx);
        }
        cx.notify();
    }

    fn open_usage_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.usage.close();
        self.open_settings_at(SettingsSection::Usage, window, cx);
    }

    fn handle_usage_bubble_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let stops = self
            .sidebar_usage_view()
            .map_or(0, |view| view.blocks.len() + 1);
        match event.keystroke.key.as_str() {
            "escape" => self.close_usage_bubble(window, cx),
            "up" | "down" => {
                self.usage.cursor =
                    step_cursor(self.usage.cursor, stops, event.keystroke.key == "down");
                cx.notify();
            }
            "enter" | "space" if self.usage.cursor == stops.checked_sub(1) => {
                self.open_usage_settings(window, cx);
            }
            _ => return,
        }
        cx.stop_propagation();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use paneflow_agent_config::usage::accounts::OriginKind;
    use paneflow_config::schema::{AgentProfileConfig, PaneFlowConfig, SpaceConfig, SpaceId};
    use paneflow_serve::usage::DEFAULT_ORIGIN_NAME;

    use super::*;

    #[test]
    fn test_the_origins_cover_the_default_every_space_and_each_agent_profile() {
        let config = PaneFlowConfig {
            spaces: vec![SpaceConfig {
                id: SpaceId::new(),
                name: "Work".into(),
                color: None,
                avatar: None,
                env: BTreeMap::from([("CLAUDE_CONFIG_DIR".into(), "~/.claude-work".into())]),
            }],
            agent_profiles: vec![
                AgentProfileConfig {
                    name: "Claude perso".into(),
                    agent: "claude_code".into(),
                    env: BTreeMap::from([("CLAUDE_CONFIG_DIR".into(), "~/.claude-perso".into())]),
                    args: Vec::new(),
                },
                AgentProfileConfig {
                    name: "Gemini".into(),
                    agent: "gemini".into(),
                    env: BTreeMap::new(),
                    args: Vec::new(),
                },
            ],
            ..PaneFlowConfig::default()
        };
        let origins = usage_origins(&config);
        let shape: Vec<(OriginKind, &str, Option<UsageAgent>)> = origins
            .iter()
            .map(|origin| (origin.kind, origin.name.as_str(), origin.agent))
            .collect();
        assert_eq!(
            shape,
            [
                (OriginKind::Default, "Default", None),
                (OriginKind::Space, "Work", None),
                (
                    OriginKind::Profile,
                    "Claude perso",
                    Some(UsageAgent::ClaudeCode)
                ),
            ]
        );

        let home = tempfile::tempdir().unwrap();
        for dir in [".claude", ".claude-work", ".claude-perso"] {
            std::fs::create_dir_all(home.path().join(dir)).unwrap();
        }
        let mut origins = origins;
        origins[0].env.clear();
        let entries = read_inventory(&origins, Some(home.path()));
        let claude: Vec<&str> = entries
            .iter()
            .filter(|entry| entry.account.agent == UsageAgent::ClaudeCode)
            .map(|entry| entry.account.label.as_str())
            .collect();
        assert_eq!(claude, ["Default", "Work", "Claude perso"]);
        assert!(
            entries
                .iter()
                .all(|entry| entry.account.agent == UsageAgent::ClaudeCode),
            "no ~/.codex directory, so no Codex account"
        );
        assert!(
            entries
                .iter()
                .all(|entry| entry.status_line == StatusLineState::Missing)
        );
    }

    #[test]
    fn test_a_home_without_claude_or_codex_directories_hides_the_capsule() {
        let home = tempfile::tempdir().unwrap();
        let mut origins = vec![AccountOrigin::process_default(DEFAULT_ORIGIN_NAME)];
        origins[0].env.clear();
        let inventory = read_inventory(&origins, Some(home.path()));
        assert_eq!(inventory, []);
        assert_eq!(
            usage_view(&UsageInputs {
                inventory: &inventory,
                plans: &[],
                active_space: None,
                home: Some(home.path()),
                now_ms: 1_793_457_000_000,
                utc_offset_secs: 0,
            }),
            None
        );
    }

    #[test]
    fn test_a_relayed_frame_reaches_the_capsule_through_the_worker_usage_frame() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(home.path().join(".codex")).unwrap();
        let now_ms = 1_793_457_000_000;
        let mut worker = paneflow_serve::usage::Usage::open(home.path(), now_ms);
        let relayed = serde_json::json!({
            "config_dir": claude.to_string_lossy(),
            "received_at_ms": now_ms - 30_000,
            "rate_limits": {
                "five_hour": {"used_percentage": 12, "resets_at": now_ms / 1000 + 3600},
                "seven_day": {"used_percentage": 54, "resets_at": 1_793_869_200},
            },
        });
        assert!(worker.apply_plan_limits(&relayed, now_ms));
        let frame = worker
            .take_changed_frame()
            .expect("a changed store publishes a frame");
        assert_eq!(frame["type"], paneflow_serve::usage::FRAME_TYPE);
        let plans = worker_plans(&frame[paneflow_serve::usage::FRAME_TYPE])
            .expect("the app reads the plans the worker publishes");

        let mut origins = vec![AccountOrigin::process_default(DEFAULT_ORIGIN_NAME)];
        origins[0].env.clear();
        let inventory = read_inventory(&origins, Some(home.path()));
        let view = usage_view(&UsageInputs {
            inventory: &inventory,
            plans: &plans,
            active_space: None,
            home: Some(home.path()),
            now_ms,
            utc_offset_secs: 0,
        })
        .expect("the default accounts give a view");
        assert_eq!(
            view.capsule.accessible_name(),
            "54% plan usage, Claude Code week"
        );
        assert_eq!(view.updated.as_deref(), Some("Updated just now"));
        let codex = view
            .blocks
            .iter()
            .find(|block| block.agent == UsageAgent::Codex)
            .expect("the default Codex account is listed");
        assert_eq!(
            codex.body,
            model::BlockBody::Message(
                "No plan data yet. Finish a Codex turn in a Paneflow pane to load your limits."
                    .into()
            )
        );
    }

    #[test]
    fn test_the_import_status_of_the_worker_frame_reaches_the_page_labels() {
        let home = tempfile::tempdir().unwrap();
        let now_ms = 1_793_457_000_000;
        let mut worker = paneflow_serve::usage::Usage::open(home.path(), now_ms);
        assert!(worker.begin_import(true));
        worker.import_progress(1);
        let frame = worker
            .take_changed_frame()
            .expect("the progress is published");
        let running = worker_import(&frame[paneflow_serve::usage::FRAME_TYPE])
            .expect("the app reads the import status of the frame");
        assert!(running.running);
        assert_eq!(
            page::import_progress_label(running.files),
            "Importing past activity\u{2026} 1 file"
        );
        worker.finish_import(
            Some(paneflow_serve::usage::backfill::ImportOutcome {
                files: 10_000,
                limited_days: Some(30),
                ..Default::default()
            }),
            now_ms,
        );
        let finished = worker_usage(&worker.snapshot())
            .import
            .expect("the snapshot carries the import status");
        assert!(!finished.running);
        assert_eq!(
            finished
                .limited_days
                .map(page::import_limit_label)
                .as_deref(),
            Some("Imported the last 30 days")
        );
    }

    #[test]
    fn test_the_worker_snapshot_fills_the_usage_page_tiles_and_registers() {
        use paneflow_serve::usage::ledger::{LedgerAccount, TurnInput, TurnSignal};

        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join(".claude");
        let projects = claude.join("projects").join("C--work");
        std::fs::create_dir_all(&projects).unwrap();
        let now_ms = now_ms();
        let mut worker = paneflow_serve::usage::Usage::open(home.path(), now_ms);
        let relayed = serde_json::json!({
            "config_dir": claude.to_string_lossy(),
            "received_at_ms": now_ms,
            "rate_limits": {
                "five_hour": {"used_percentage": 12, "resets_at": now_ms / 1000 + 3600},
                "seven_day": {"used_percentage": 54, "resets_at": now_ms / 1000 + 3 * 86_400},
            },
        });
        assert!(worker.apply_plan_limits(&relayed, now_ms));
        let stamp = chrono::DateTime::from_timestamp_millis(i64::try_from(now_ms).unwrap())
            .unwrap()
            .to_rfc3339();
        let transcript = projects.join("session.jsonl");
        std::fs::write(
            &transcript,
            format!(
                "{}\n",
                serde_json::json!({
                    "type": "assistant",
                    "timestamp": stamp,
                    "message": {"id": "msg_1", "usage": {"input_tokens": 30, "output_tokens": 70}},
                })
            ),
        )
        .unwrap();
        let account = LedgerAccount {
            agent: UsageAgent::ClaudeCode.runtime_slug().to_string(),
            config_dir: paneflow_agent_config::usage::accounts::canonical_account_dir(&claude)
                .display()
                .to_string(),
        };
        let signal = |input, at_ms, transcript: Option<String>| TurnSignal {
            session: "pane-1".into(),
            account: account.clone(),
            input,
            at_ms,
            order: at_ms,
            transcript,
        };
        let ends = worker.apply_turn_signals(&[
            signal(TurnInput::Prompt, now_ms - 60_000, None),
            signal(
                TurnInput::Stop,
                now_ms,
                Some(transcript.display().to_string()),
            ),
        ]);
        assert_eq!(ends.transcripts.len(), 1, "the Stop transcript is claimed");
        for read in ends.transcripts {
            let cursor = worker.token_cursor(&read.path);
            let pass = paneflow_serve::usage::read_transcript(&read, cursor, now_ms).unwrap();
            assert!(worker.record_tokens(&read.account, pass));
        }

        let usage = worker_usage(&worker.snapshot());
        let mut origins = vec![AccountOrigin::process_default(DEFAULT_ORIGIN_NAME)];
        origins[0].env.clear();
        let inventory = read_inventory(&origins, Some(home.path()));
        let plans = usage.plans.expect("the snapshot carries the plans");
        let tiles = usage_view(&UsageInputs {
            inventory: &inventory,
            plans: &plans,
            active_space: None,
            home: Some(home.path()),
            now_ms,
            utc_offset_secs: 0,
        })
        .expect("the default Claude account gives a tile");
        assert!(matches!(
            &tiles.blocks[0].body,
            model::BlockBody::Windows(rows) if rows.len() == 2
        ));

        let (ledger, revision) = usage.ledger.expect("the snapshot carries the ledger");
        assert!(revision.is_some());
        let today = chrono::Local::now().date_naive();
        assert_eq!(
            day_at(
                now_ms,
                paneflow_serve::usage::local_utc_offset_secs(now_ms / 1000)
            ),
            today,
            "the app derives the worker's local day from the offset it reads off the render thread"
        );
        let inputs = |metric| activity::ActivityInputs {
            ledger: &ledger,
            inventory: &inventory,
            home: Some(home.path()),
            today,
            metric,
        };
        let turns = activity::activity_view(&inputs(Metric::Turns));
        assert_eq!(turns.registers.len(), 1);
        assert_eq!(turns.registers[0].agent_name, "Claude Code");
        assert_eq!(turns.registers[0].total, "1 turn in the last year");
        let tokens = activity::activity_view(&inputs(Metric::Tokens));
        assert_eq!(tokens.registers[0].total, "100 tokens in the last year");
        assert!(
            tokens.detail(today).ends_with("Claude Code 100 tokens"),
            "{}",
            tokens.detail(today)
        );
    }
}
