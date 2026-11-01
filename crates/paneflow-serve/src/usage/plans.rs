use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key};
use paneflow_agent_config::usage::model::{LimitWindow, PlanLimits};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLANS_WRITE_INTERVAL: Duration = Duration::from_secs(10);
pub const MAX_PLANS_BYTES: u64 = 256 * 1024;
pub const MAX_PLAN_ACCOUNTS: usize = 64;
const PLANS_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Reported,
    NoLimitsReported,
    NoCodexData,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredWindow {
    pub used_percent: f64,
    pub window_secs: u64,
    pub resets_at: u64,
}

impl From<LimitWindow> for StoredWindow {
    fn from(window: LimitWindow) -> Self {
        Self {
            used_percent: window.used_percent,
            window_secs: window.window_secs,
            resets_at: window.resets_at,
        }
    }
}

impl StoredWindow {
    pub fn window(&self) -> LimitWindow {
        LimitWindow::new(self.used_percent, self.window_secs, self.resets_at)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccountPlan {
    pub agent: String,
    pub config_dir: String,
    pub label: String,
    pub state: PlanState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<StoredWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<StoredWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_version: Option<String>,
    pub updated_at_ms: u64,
}

impl AccountPlan {
    fn usage_agent(&self) -> Option<UsageAgent> {
        UsageAgent::for_runtime_slug(&self.agent)
    }

    fn matches(&self, agent: UsageAgent, key: &str) -> bool {
        self.usage_agent() == Some(agent) && account_dir_key(Path::new(&self.config_dir)) == key
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PlansFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    accounts: Vec<AccountPlan>,
}

pub enum PlanUpdate {
    Limits {
        limits: PlanLimits,
        observed_at_ms: u64,
        agent_version: Option<String>,
    },
    NoLimitsReported {
        agent_version: Option<String>,
    },
    NoCodexData,
}

pub struct PlanStore {
    path: PathBuf,
    accounts: Vec<AccountPlan>,
    dirty: bool,
    last_write: Option<Instant>,
}

impl PlanStore {
    pub fn load(path: &Path) -> Self {
        let accounts = match paneflow_home::read_regular_capped(path, MAX_PLANS_BYTES) {
            Ok(bytes) => match serde_json::from_slice::<PlansFile>(&bytes) {
                Ok(file) => file
                    .accounts
                    .into_iter()
                    .filter(|account| account.usage_agent().is_some())
                    .collect(),
                Err(error) => {
                    log::warn!(
                        "paneflow-serve: ignoring the unreadable {}: {error}",
                        path.display()
                    );
                    Vec::new()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                log::warn!("paneflow-serve: ignoring {}: {error}", path.display());
                Vec::new()
            }
        };
        Self {
            path: path.to_path_buf(),
            accounts,
            dirty: false,
            last_write: None,
        }
    }

    pub fn accounts(&self) -> &[AccountPlan] {
        &self.accounts
    }

    pub fn dirs(&self, agent: UsageAgent) -> impl Iterator<Item = PathBuf> + '_ {
        self.accounts
            .iter()
            .filter(move |account| account.usage_agent() == Some(agent))
            .map(|account| PathBuf::from(&account.config_dir))
    }

    pub fn update(
        &mut self,
        agent: UsageAgent,
        config_dir: &Path,
        update: PlanUpdate,
        now_ms: u64,
    ) -> bool {
        if let PlanUpdate::Limits { limits, .. } = &update
            && limits.windows().next().is_none()
        {
            return false;
        }
        let key = account_dir_key(config_dir);
        let index = match self
            .accounts
            .iter()
            .position(|account| account.matches(agent, &key))
        {
            Some(index) => index,
            None => {
                self.evict_stalest_beyond(MAX_PLAN_ACCOUNTS - 1);
                let display = config_dir.display().to_string();
                self.accounts.push(AccountPlan {
                    agent: agent.runtime_slug().to_string(),
                    label: display.clone(),
                    config_dir: display,
                    state: PlanState::Reported,
                    primary: None,
                    secondary: None,
                    received_at_ms: None,
                    agent_version: None,
                    updated_at_ms: now_ms,
                });
                self.accounts.len() - 1
            }
        };
        let account = &mut self.accounts[index];
        let before = account.clone();
        match update {
            PlanUpdate::Limits {
                limits,
                observed_at_ms,
                agent_version,
            } => {
                if account
                    .received_at_ms
                    .is_some_and(|held| observed_at_ms < held)
                {
                    return false;
                }
                account.primary = merge(account.primary, limits.primary);
                account.secondary = merge(account.secondary, limits.secondary);
                account.received_at_ms = Some(observed_at_ms);
                account.state = PlanState::Reported;
                if agent_version.is_some() {
                    account.agent_version = agent_version;
                }
            }
            PlanUpdate::NoLimitsReported { agent_version } => {
                account.state = PlanState::NoLimitsReported;
                if agent_version.is_some() {
                    account.agent_version = agent_version;
                }
            }
            PlanUpdate::NoCodexData => account.state = PlanState::NoCodexData,
        }
        if self.accounts[index] != before {
            self.accounts[index].updated_at_ms = now_ms;
        }
        self.settle(index, &before)
    }

    fn evict_stalest_beyond(&mut self, keep: usize) {
        while self.accounts.len() > keep {
            let Some(stalest) = self
                .accounts
                .iter()
                .enumerate()
                .min_by_key(|(_, account)| account.updated_at_ms)
                .map(|(index, _)| index)
            else {
                return;
            };
            self.accounts.remove(stalest);
            self.dirty = true;
        }
    }

    fn settle(&mut self, index: usize, before: &AccountPlan) -> bool {
        let changed = self.accounts.get(index) != Some(before);
        self.dirty |= changed;
        changed
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(&self.accounts).unwrap_or(Value::Array(Vec::new()))
    }

    pub fn flush(&mut self, now: Instant, force: bool) -> bool {
        if !self.dirty {
            return false;
        }
        if !force
            && self
                .last_write
                .is_some_and(|at| now.duration_since(at) < PLANS_WRITE_INTERVAL)
        {
            return false;
        }
        let file = PlansFile {
            version: PLANS_VERSION,
            accounts: self.accounts.clone(),
        };
        let bytes = match serde_json::to_vec_pretty(&file) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!("paneflow-serve: cannot encode the plan limits: {error}");
                return false;
            }
        };
        self.last_write = Some(now);
        match paneflow_home::write_atomically(&self.path, &bytes) {
            Ok(()) => {
                self.dirty = false;
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

fn merge(held: Option<StoredWindow>, reported: Option<LimitWindow>) -> Option<StoredWindow> {
    let Some(reported) = reported.map(StoredWindow::from) else {
        return held;
    };
    match held {
        Some(held)
            if held.resets_at == reported.resets_at
                && held.used_percent > reported.used_percent =>
        {
            Some(held)
        }
        _ => Some(reported),
    }
}

#[cfg(test)]
mod tests {
    use paneflow_agent_config::usage::model::plausible_claude_rate_limits;
    use serde_json::json;

    use super::*;

    const NOW: u64 = 1_793_481_752;

    fn claude_update(rate_limits: &Value, observed_at_ms: u64) -> PlanUpdate {
        PlanUpdate::Limits {
            limits: plausible_claude_rate_limits(rate_limits, NOW),
            observed_at_ms,
            agent_version: None,
        }
    }

    fn limits(session: f64, week: f64, week_reset: u64) -> Value {
        json!({
            "five_hour": {"used_percentage": session, "resets_at": NOW + 3600},
            "seven_day": {"used_percentage": week, "resets_at": week_reset}
        })
    }

    #[test]
    fn the_last_frame_wins_but_a_stale_session_never_lowers_the_same_window() {
        let home = tempfile::tempdir().unwrap();
        let mut store = PlanStore::load(&home.path().join("plans.json"));
        let dir = home.path().join(".claude");
        let reset = NOW + 3 * 86_400;
        assert!(store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(13.0, 59.0, reset), 1),
            1
        ));
        assert!(store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(20.0, 61.0, reset), 2),
            2
        ));
        let account = &store.accounts()[0];
        assert_eq!(
            account.primary.map(|window| window.used_percent),
            Some(20.0)
        );
        assert_eq!(
            account.secondary.map(|window| window.used_percent),
            Some(61.0)
        );

        let stale = json!({"seven_day": {"used_percentage": 55, "resets_at": reset}});
        store.update(UsageAgent::ClaudeCode, &dir, claude_update(&stale, 3), 3);
        let account = &store.accounts()[0];
        assert_eq!(
            account.secondary.map(|window| window.used_percent),
            Some(61.0),
            "a lower value for the same reset is an idle session's stale copy"
        );
        assert_eq!(
            account.primary.map(|window| window.used_percent),
            Some(20.0),
            "an omitted window keeps the held one"
        );
        assert_eq!(account.received_at_ms, Some(3));

        let next_week = json!({"seven_day": {"used_percentage": 2, "resets_at": reset + 86_400}});
        store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&next_week, 4),
            4,
        );
        assert_eq!(
            store.accounts()[0]
                .secondary
                .map(|window| window.used_percent),
            Some(2.0),
            "a new reset replaces the window"
        );
        assert_eq!(store.accounts().len(), 1);
    }

    #[test]
    fn an_older_observation_never_replaces_a_newer_one_and_keeps_the_agent_version() {
        let home = tempfile::tempdir().unwrap();
        let mut store = PlanStore::load(&home.path().join("plans.json"));
        let dir = home.path().join(".claude");
        let reset = NOW + 3 * 86_400;
        let probed = PlanUpdate::Limits {
            limits: plausible_claude_rate_limits(&limits(10.0, 40.0, reset), NOW),
            observed_at_ms: 2_000,
            agent_version: Some("2.1.316".into()),
        };
        assert!(store.update(UsageAgent::ClaudeCode, &dir, probed, 2_000));
        assert!(
            !store.update(
                UsageAgent::ClaudeCode,
                &dir,
                claude_update(&limits(30.0, 50.0, reset), 1_000),
                2_100
            ),
            "a relay frame observed before the probe answer is older"
        );
        assert!(store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(30.0, 50.0, reset), 3_000),
            3_000
        ));
        let account = &store.accounts()[0];
        assert_eq!(
            account.secondary.map(|window| window.used_percent),
            Some(50.0)
        );
        assert_eq!(account.received_at_ms, Some(3_000));
        assert_eq!(account.agent_version.as_deref(), Some("2.1.316"));
    }

    #[test]
    fn missing_limits_keep_the_previous_snapshot_with_its_time() {
        let home = tempfile::tempdir().unwrap();
        let mut store = PlanStore::load(&home.path().join("plans.json"));
        let dir = home.path().join(".claude");
        store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(13.0, 59.0, NOW + 86_400), 7),
            7,
        );
        assert!(store.update(
            UsageAgent::ClaudeCode,
            &dir,
            PlanUpdate::NoLimitsReported {
                agent_version: None
            },
            9
        ));
        let account = &store.accounts()[0];
        assert_eq!(account.state, PlanState::NoLimitsReported);
        assert_eq!(account.received_at_ms, Some(7));
        assert!(account.secondary.is_some());
    }

    #[test]
    fn a_corrupt_or_oversized_file_starts_empty() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("plans.json");
        std::fs::write(&path, b"[1,").unwrap();
        assert!(PlanStore::load(&path).accounts().is_empty());
        std::fs::write(&path, vec![b' '; MAX_PLANS_BYTES as usize + 1]).unwrap();
        assert!(PlanStore::load(&path).accounts().is_empty());
    }

    #[test]
    fn the_store_keeps_at_most_its_account_cap_and_drops_the_stalest_first() {
        let home = tempfile::tempdir().unwrap();
        let mut store = PlanStore::load(&home.path().join("plans.json"));
        let reset = NOW + 86_400;
        for index in 0..=MAX_PLAN_ACCOUNTS as u64 {
            let dir = home.path().join(format!(".claude-{index}"));
            store.update(
                UsageAgent::ClaudeCode,
                &dir,
                claude_update(&limits(1.0, 2.0, reset), index),
                index,
            );
        }
        assert_eq!(store.accounts().len(), MAX_PLAN_ACCOUNTS);
        let first = home.path().join(".claude-0").display().to_string();
        assert!(
            store
                .accounts()
                .iter()
                .all(|account| account.config_dir != first),
            "the least recently updated account is evicted"
        );
        let encoded = serde_json::to_vec_pretty(&PlansFile {
            version: PLANS_VERSION,
            accounts: store.accounts().to_vec(),
        })
        .unwrap();
        assert!((encoded.len() as u64) < MAX_PLANS_BYTES);
    }

    #[test]
    fn writes_are_spaced_by_ten_seconds() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("usage").join("plans.json");
        let mut store = PlanStore::load(&path);
        let dir = home.path().join(".claude");
        let start = Instant::now();
        store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(1.0, 2.0, NOW + 86_400), 1),
            1,
        );
        assert!(store.flush(start, false));
        store.update(
            UsageAgent::ClaudeCode,
            &dir,
            claude_update(&limits(3.0, 4.0, NOW + 86_400), 2),
            2,
        );
        assert!(!store.flush(start + Duration::from_secs(9), false));
        assert!(store.flush(start + Duration::from_secs(10), false));
        assert!(
            !store.flush(start + Duration::from_secs(30), false),
            "nothing new to write"
        );
        let reloaded = PlanStore::load(&path);
        assert_eq!(reloaded.accounts(), store.accounts());
    }
}
