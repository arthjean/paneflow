pub(crate) mod model;
mod render;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{App, Context, FocusHandle, KeyDownEvent, Window};
use paneflow_agent_config::usage::accounts::{
    AccountOrigin, OriginKind, UsageAgent, claude_status_line_state, inventory,
};
use paneflow_agent_config::usage::status_line::StatusLineState;
use paneflow_config::schema::PaneFlowConfig;
use paneflow_serve::usage::plans::AccountPlan;
use serde_json::Value;

use self::model::{InventoryEntry, UsageInputs, UsageView, step_cursor, usage_view};
use crate::PaneFlowApp;

const DEFAULT_ORIGIN_NAME: &str = "Default";

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

pub(crate) fn usage_origins(config: &PaneFlowConfig) -> Vec<AccountOrigin> {
    let mut origins = vec![AccountOrigin::process_default(DEFAULT_ORIGIN_NAME)];
    origins.extend(config.spaces.iter().map(|space| AccountOrigin {
        kind: OriginKind::Space,
        name: space.name.clone(),
        agent: None,
        env: space.env.clone(),
    }));
    origins.extend(config.agent_profiles.iter().filter_map(|profile| {
        Some(AccountOrigin {
            kind: OriginKind::Profile,
            name: profile.name.clone(),
            agent: Some(UsageAgent::for_profile_agent(&profile.agent)?),
            env: profile.env.clone(),
        })
    }));
    origins
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

fn refresh_worker_usage(entries: &[InventoryEntry]) -> Option<Vec<AccountPlan>> {
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
            let plans = worker_plans(&snapshot);
            if plans.is_none() {
                log::debug!("paneflow: the worker refresh carried no plan usage");
            }
            plans
        }
        Err(error) => {
            log::debug!("paneflow: the worker did not refresh plan usage: {error}");
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
            let (entries, offset, plans) = smol::unblock(move || {
                let entries = read_inventory(&origins, home.as_deref());
                let offset = paneflow_serve::usage::local_utc_offset_secs(now_ms() / 1000);
                let plans = refresh_worker
                    .then(|| refresh_worker_usage(&entries))
                    .flatten();
                (entries, offset, plans)
            })
            .await;
            let _ = this.update(cx, |app, cx| {
                if let Some(plans) = plans {
                    app.apply_plans(plans, cx);
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
        self.open_settings_at(crate::SettingsSection::Agents, window, cx);
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

    use paneflow_config::schema::{AgentProfileConfig, SpaceConfig, SpaceId};

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
}
