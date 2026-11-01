pub mod backfill;
pub mod codex;
pub mod ledger;
pub mod plans;
pub mod probe;
pub mod tokens;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_agent_config::runtime_catalog::runtime_for_tool;
use paneflow_agent_config::usage::accounts::{
    AccountOrigin, OriginKind, UsageAgent, account_dir_key, canonical_account_dir, inventory,
};
use paneflow_agent_config::usage::model::plausible_claude_rate_limits;
use paneflow_config::schema::PaneFlowConfig;
use serde_json::{Value, json};

use self::backfill::{ImportOutcome, ImportStatus};
use self::codex::CodexReading;
use self::ledger::{
    Calendar, ImportRecord, Ledger, LedgerAccount, TurnInput, TurnSignal, local_calendar,
};
use self::plans::{PlanStore, PlanUpdate};
use self::probe::ProbeOutcome;
use self::tokens::{TokenCursor, TokenPass, TranscriptKind};

pub const FRAME_TYPE: &str = "usage";
pub const DEFAULT_ORIGIN_NAME: &str = "Default";

pub fn usage_origins(config: &PaneFlowConfig) -> Vec<AccountOrigin> {
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

pub fn configured_accounts(home: &Path, user_home: Option<&Path>) -> Vec<(UsageAgent, PathBuf)> {
    let config =
        paneflow_config::loader::load_config_from_path(&paneflow_home::config_path_in(home));
    inventory(&usage_origins(&config), user_home)
        .into_iter()
        .filter(|account| account.config_dir.is_dir())
        .map(|account| (account.agent, account.config_dir))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptRead {
    pub account: LedgerAccount,
    pub path: PathBuf,
    pub kind: TranscriptKind,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TurnEnds {
    pub codex: Vec<PathBuf>,
    pub transcripts: Vec<TranscriptRead>,
}

pub struct Usage {
    plans: PlanStore,
    ledger: Ledger,
    codex_reads: BTreeMap<String, Instant>,
    probes: BTreeMap<(UsageAgent, String), Instant>,
    importing: Option<u64>,
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
            importing: None,
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

    pub fn apply_turn_signals(&mut self, signals: &[TurnSignal]) -> TurnEnds {
        let calendar = local_calendar();
        let mut ends = TurnEnds::default();
        for signal in signals {
            self.ledger.book_mut().apply(signal, &calendar);
            if signal.account.agent == UsageAgent::Codex.runtime_slug()
                && matches!(signal.input, TurnInput::Stop | TurnInput::Settled)
                && !signal.account.config_dir.is_empty()
            {
                ends.codex.push(PathBuf::from(&signal.account.config_dir));
            }
            if let Some(read) = claude_transcript(signal) {
                ends.transcripts.push(read);
            }
        }
        ends.codex.sort();
        ends.codex.dedup();
        ends.transcripts.dedup();
        ends
    }

    pub fn token_cursor(&self, path: &Path) -> TokenCursor {
        self.ledger.book().token_cursor(&path.display().to_string())
    }

    pub fn record_tokens(&mut self, account: &LedgerAccount, pass: TokenPass) -> bool {
        self.ledger.book_mut().record_tokens(account, pass)
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

    pub fn begin_import(&mut self, force: bool) -> bool {
        let book = self.ledger.book();
        let first_start = book.days().is_empty() && book.import_record().is_none();
        if self.importing.is_some() || !(force || first_start) {
            return false;
        }
        self.importing = Some(0);
        true
    }

    pub fn import_progress(&mut self, files: u64) {
        if self.importing.is_some() {
            self.importing = Some(files);
        }
    }

    pub fn finish_import(&mut self, outcome: Option<ImportOutcome>, now_ms: u64) -> usize {
        self.importing = None;
        let Some(outcome) = outcome else {
            return 0;
        };
        let record = ImportRecord {
            finished_at_ms: now_ms,
            files: outcome.files,
            limited_days: outcome.limited_days,
        };
        let today = local_calendar().day(now_ms);
        self.ledger
            .book_mut()
            .import_days(outcome.days, today, record)
    }

    pub fn import_status(&self) -> ImportStatus {
        match self.importing {
            Some(files) => ImportStatus {
                running: true,
                files,
                limited_days: None,
            },
            None => ImportStatus {
                running: false,
                files: self
                    .ledger
                    .book()
                    .import_record()
                    .map_or(0, |record| record.files),
                limited_days: self
                    .ledger
                    .book()
                    .import_record()
                    .and_then(|record| record.limited_days),
            },
        }
    }

    pub fn flush(&mut self, now: Instant, force: bool) {
        self.plans.flush(now, force);
        self.ledger.flush(now, force);
    }

    pub fn frame(&self) -> Value {
        json!({
            "plans": self.plans.to_value(),
            "ledger_revision": self.ledger.book().revision(),
            "import": self.import_status(),
        })
    }

    pub fn snapshot(&self) -> Value {
        json!({
            "plans": self.plans.to_value(),
            "ledger": self.ledger.book().to_value(),
            "import": self.import_status(),
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

fn claude_transcript(signal: &TurnSignal) -> Option<TranscriptRead> {
    if signal.input != TurnInput::Stop
        || signal.account.agent != UsageAgent::ClaudeCode.runtime_slug()
        || signal.account.config_dir.is_empty()
    {
        return None;
    }
    let path = PathBuf::from(signal.transcript.as_deref()?);
    if !path.is_absolute()
        || path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
    {
        return None;
    }
    let account_key = account_dir_key(Path::new(&signal.account.config_dir));
    let path = canonical_account_dir(&path);
    path.ancestors()
        .skip(1)
        .any(|ancestor| account_dir_key(ancestor) == account_key)
        .then(|| TranscriptRead {
            account: signal.account.clone(),
            path,
            kind: TranscriptKind::Claude,
        })
}

pub fn codex_transcript(codex_home: &Path) -> Option<TranscriptRead> {
    let dir = canonical_account_dir(codex_home);
    Some(TranscriptRead {
        path: codex::latest_rollout_in(&dir)?,
        account: LedgerAccount {
            agent: UsageAgent::Codex.runtime_slug().to_string(),
            config_dir: dir.display().to_string(),
        },
        kind: TranscriptKind::Codex,
    })
}

pub fn read_transcript(
    read: &TranscriptRead,
    cursor: TokenCursor,
    now_ms: u64,
) -> Option<TokenPass> {
    tokens::read_tokens(&read.path, read.kind, cursor, &local_calendar(), now_ms)
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
    fn the_first_import_runs_once_on_an_empty_ledger_and_a_request_always_runs() {
        let home = tempfile::tempdir().unwrap();
        let now_ms = 1_793_481_752_042;
        let mut usage = Usage::open(home.path(), now_ms);
        assert!(
            usage.begin_import(false),
            "an empty ledger imports at first start"
        );
        assert!(!usage.begin_import(true), "one import at a time");
        usage.import_progress(12);
        assert_eq!(
            usage.frame()["import"],
            json!({"running": true, "files": 12}),
            "the frame carries the progress the page shows"
        );
        usage.finish_import(
            Some(ImportOutcome {
                files: 12,
                limited_days: Some(30),
                ..ImportOutcome::default()
            }),
            now_ms,
        );
        assert_eq!(
            usage.snapshot()["import"],
            json!({"running": false, "files": 12, "limited_days": 30})
        );
        assert!(
            !usage.begin_import(false),
            "a finished import is not repeated at the next start"
        );
        assert!(usage.begin_import(true), "Import past activity runs again");
        usage.finish_import(None, now_ms);
        assert!(!usage.import_status().running);
    }

    #[test]
    fn the_worker_lists_the_default_space_and_profile_accounts_of_its_config() {
        let home = tempfile::tempdir().unwrap();
        let user = tempfile::tempdir().unwrap();
        for dir in [".claude", ".claude-work", ".codex"] {
            std::fs::create_dir_all(user.path().join(dir)).unwrap();
        }
        let work = user.path().join(".claude-work");
        std::fs::write(
            paneflow_home::config_path_in(home.path()),
            json!({"spaces": [{"id": "00000000-0000-4000-8000-000000000001", "name": "Work", "env": {"CLAUDE_CONFIG_DIR": work.display().to_string()}}]})
                .to_string(),
        )
        .unwrap();
        let accounts = configured_accounts(home.path(), Some(user.path()));
        let claude: Vec<&PathBuf> = accounts
            .iter()
            .filter(|(agent, _)| *agent == UsageAgent::ClaudeCode)
            .map(|(_, dir)| dir)
            .collect();
        assert!(
            claude.contains(&&canonical_account_dir(&work)),
            "{accounts:?}"
        );
        assert!(accounts.iter().all(|(_, dir)| dir.is_dir()));
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

    #[cfg(unix)]
    #[test]
    fn a_stop_transcript_reached_through_a_symlinked_config_dir_is_claimed() {
        let home = tempfile::tempdir().unwrap();
        let real = home.path().join("dotfiles").join("claude");
        let projects = real.join("projects").join("work");
        std::fs::create_dir_all(&projects).unwrap();
        let linked = home.path().join(".claude");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        std::fs::write(projects.join("session.jsonl"), "").unwrap();
        let mut usage = Usage::open(home.path(), 1_793_481_752_042);
        let account = LedgerAccount {
            agent: UsageAgent::ClaudeCode.runtime_slug().to_string(),
            config_dir: canonical_account_dir(&linked).display().to_string(),
        };
        let transcript = linked.join("projects").join("work").join("session.jsonl");
        let ends = usage.apply_turn_signals(&[TurnSignal {
            session: "pane-1".into(),
            account,
            input: TurnInput::Stop,
            at_ms: 1_793_481_752_042,
            order: 1,
            transcript: Some(transcript.display().to_string()),
        }]);
        assert_eq!(ends.transcripts.len(), 1);
    }
}
