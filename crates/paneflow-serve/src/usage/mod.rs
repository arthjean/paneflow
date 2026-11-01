pub mod codex;
pub mod ledger;
pub mod plans;
pub mod probe;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_agent_config::runtime_catalog::runtime_for_tool;
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_agent_config::usage::model::plausible_claude_rate_limits;
use serde_json::{Value, json};

use self::codex::CodexReading;
use self::ledger::{Ledger, TurnInput, TurnSignal, local_calendar};
use self::plans::{PlanStore, PlanUpdate};
use self::probe::ProbeOutcome;

pub const FRAME_TYPE: &str = "usage";

pub struct Usage {
    plans: PlanStore,
    ledger: Ledger,
    codex_reads: BTreeMap<String, Instant>,
    probes: BTreeMap<(UsageAgent, String), Instant>,
    published: Option<Value>,
}

impl Usage {
    pub fn open(home: &Path, now_ms: u64) -> Self {
        if let Err(error) =
            paneflow_home::create_private_dir_all(&paneflow_home::usage_dir_in(home))
        {
            log::warn!("paneflow-serve: cannot prepare the usage directory: {error}");
        }
        Self {
            plans: PlanStore::load(&paneflow_home::usage_plans_path_in(home)),
            ledger: Ledger::load(
                &paneflow_home::usage_ledger_path_in(home),
                &local_calendar(),
                now_ms,
            ),
            codex_reads: BTreeMap::new(),
            probes: BTreeMap::new(),
            published: None,
        }
    }

    pub fn plans(&self) -> &PlanStore {
        &self.plans
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub fn apply_plan_limits(&mut self, frame: &Value, now_ms: u64) -> bool {
        let agent = match frame.get("agent").and_then(Value::as_str) {
            None => UsageAgent::ClaudeCode,
            Some(tool) => match runtime_for_tool(tool)
                .and_then(|runtime| UsageAgent::for_runtime_slug(runtime.slug))
            {
                Some(agent) => agent,
                None => return false,
            },
        };
        let Some(config_dir) = frame
            .get("config_dir")
            .and_then(Value::as_str)
            .map(Path::new)
            .filter(|dir| dir.is_absolute())
        else {
            return false;
        };
        let update = match frame.get("rate_limits") {
            Some(Value::Object(_)) => PlanUpdate::Limits {
                limits: plausible_claude_rate_limits(&frame["rate_limits"], now_ms / 1000),
                observed_at_ms: frame
                    .get("received_at_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(now_ms),
                agent_version: None,
            },
            None | Some(Value::Null) => PlanUpdate::NoLimitsReported {
                agent_version: None,
            },
            Some(_) => return false,
        };
        self.plans
            .update(agent, &canonical_account_dir(config_dir), update, now_ms)
    }

    pub fn apply_turn_signals(&mut self, signals: &[TurnSignal]) -> Vec<PathBuf> {
        let calendar = local_calendar();
        let mut turn_ends = Vec::new();
        for signal in signals {
            self.ledger.book_mut().apply(signal, &calendar);
            if signal.account.agent == UsageAgent::Codex.runtime_slug()
                && matches!(signal.input, TurnInput::Stop | TurnInput::Settled)
                && !signal.account.config_dir.is_empty()
            {
                turn_ends.push(PathBuf::from(&signal.account.config_dir));
            }
        }
        turn_ends.sort();
        turn_ends.dedup();
        turn_ends
    }

    pub fn expire_turns(
        &mut self,
        running: &BTreeSet<String>,
        in_turn: &BTreeSet<String>,
        now_ms: u64,
    ) {
        self.ledger
            .book_mut()
            .expire(running, in_turn, now_ms, &local_calendar());
    }

    pub fn claim_codex_reads(
        &mut self,
        dirs: impl IntoIterator<Item = PathBuf>,
        floor: Duration,
        now: Instant,
    ) -> Vec<PathBuf> {
        let mut claimed: Vec<PathBuf> = Vec::new();
        let mut seen = BTreeSet::new();
        for dir in dirs {
            let dir = canonical_account_dir(&dir);
            let key = account_dir_key(&dir);
            if !seen.insert(key.clone()) {
                continue;
            }
            let due = self
                .codex_reads
                .get(&key)
                .is_none_or(|last| now.duration_since(*last) >= floor);
            if due {
                self.codex_reads.insert(key, now);
                claimed.push(dir);
            }
        }
        claimed
    }

    pub fn codex_dirs(&self) -> Vec<PathBuf> {
        self.plans.dirs(UsageAgent::Codex).collect()
    }

    pub fn record_codex(&mut self, dir: &Path, reading: Option<CodexReading>, now_ms: u64) -> bool {
        let update = match reading {
            Some(reading) => PlanUpdate::Limits {
                limits: reading.limits,
                observed_at_ms: reading.observed_at_ms,
                agent_version: None,
            },
            None => PlanUpdate::NoCodexData,
        };
        self.plans.update(UsageAgent::Codex, dir, update, now_ms)
    }

    pub fn claim_probes(
        &mut self,
        accounts: impl IntoIterator<Item = (UsageAgent, PathBuf)>,
        floor: Duration,
        now: Instant,
    ) -> Vec<(UsageAgent, PathBuf)> {
        let mut claimed = Vec::new();
        for (agent, dir) in accounts {
            let dir = canonical_account_dir(&dir);
            let key = (agent, account_dir_key(&dir));
            let due = self
                .probes
                .get(&key)
                .is_none_or(|last| now.duration_since(*last) >= floor);
            if due {
                self.probes.insert(key, now);
                claimed.push((agent, dir));
            }
        }
        claimed
    }

    pub fn record_probe(
        &mut self,
        agent: UsageAgent,
        dir: &Path,
        outcome: ProbeOutcome,
        now_ms: u64,
    ) -> bool {
        let update = match outcome {
            ProbeOutcome::Limits {
                limits,
                agent_version,
            } => PlanUpdate::Limits {
                limits,
                observed_at_ms: now_ms,
                agent_version,
            },
            ProbeOutcome::NoLimitsReported { agent_version } => {
                PlanUpdate::NoLimitsReported { agent_version }
            }
        };
        self.plans.update(agent, dir, update, now_ms)
    }

    pub fn flush(&mut self, now: Instant, force: bool) {
        self.plans.flush(now, force);
        self.ledger.flush(now, force);
    }

    pub fn frame(&self) -> Value {
        json!({
            "plans": self.plans.to_value(),
            "ledger_revision": self.ledger.book().revision(),
        })
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "plans": self.plans.to_value(),
            "ledger": self.ledger.book().to_value(),
        })
    }

    pub fn take_changed_frame(&mut self) -> Option<Value> {
        let frame = self.frame();
        if self.published.as_ref() == Some(&frame) {
            return None;
        }
        self.published = Some(frame.clone());
        Some(json!({"type": FRAME_TYPE, "usage": frame}))
    }
}

pub fn local_utc_offset_secs(at_secs: u64) -> i32 {
    use chrono::{Offset, TimeZone};
    i64::try_from(at_secs)
        .ok()
        .and_then(|secs| chrono::Local.timestamp_opt(secs, 0).earliest())
        .map_or(0, |at| at.offset().fix().local_minus_utc())
}

pub fn read_codex_accounts(
    dirs: Vec<PathBuf>,
    now_secs: u64,
) -> Vec<(PathBuf, Option<CodexReading>)> {
    dirs.into_iter()
        .map(|dir| canonical_account_dir(&dir))
        .filter(|dir| dir.is_dir())
        .map(|dir| {
            let reading = codex::read_latest_limits(&dir, now_secs);
            (dir, reading)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_reads_are_spaced_by_the_floor_except_at_a_turn_end() {
        let home = tempfile::tempdir().unwrap();
        let mut usage = Usage::open(home.path(), 1_793_481_752_042);
        let codex = home.path().join("codex");
        std::fs::create_dir_all(&codex).unwrap();
        let start = Instant::now();
        let floor = codex::CODEX_READ_FLOOR;
        let claim = |usage: &mut Usage, floor: Duration, at: Instant| {
            usage.claim_codex_reads([codex.clone(), codex.join(".")], floor, at)
        };
        assert_eq!(
            claim(&mut usage, floor, start).len(),
            1,
            "one read per account"
        );
        assert!(claim(&mut usage, floor, start + Duration::from_secs(29)).is_empty());
        assert_eq!(
            claim(&mut usage, Duration::ZERO, start + Duration::from_secs(29)).len(),
            1,
            "a turn end reads at once"
        );
        assert_eq!(
            claim(&mut usage, floor, start + Duration::from_secs(59)).len(),
            1
        );
        let poll = codex::CODEX_POLL_INTERVAL;
        assert!(claim(&mut usage, poll, start + Duration::from_secs(299)).is_empty());
        assert_eq!(
            claim(&mut usage, poll, start + Duration::from_secs(359)).len(),
            1
        );
    }

    #[test]
    fn each_account_is_probed_at_most_once_per_floor() {
        let home = tempfile::tempdir().unwrap();
        let mut usage = Usage::open(home.path(), 1_793_481_752_042);
        let dir = home.path().join("claude");
        std::fs::create_dir_all(&dir).unwrap();
        let start = Instant::now();
        let floor = probe::PROBE_FLOOR;
        let accounts = vec![
            (UsageAgent::ClaudeCode, dir.clone()),
            (UsageAgent::ClaudeCode, dir.join(".")),
            (UsageAgent::Codex, dir.clone()),
        ];
        assert_eq!(
            usage.claim_probes(accounts.clone(), floor, start).len(),
            2,
            "one probe per agent and account"
        );
        assert!(
            usage
                .claim_probes(accounts.clone(), floor, start + Duration::from_secs(29))
                .is_empty()
        );
        assert_eq!(
            usage
                .claim_probes(accounts, floor, start + Duration::from_secs(30))
                .len(),
            2
        );
    }
}
