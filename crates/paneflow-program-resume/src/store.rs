use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::parse::{Action, Report};
use crate::verify::{Outcome, ReplayGuard};

pub const MAX_RECORDS: usize = 16;
pub const EXIT_GRACE_MS: u64 = 2_000;
const ATTEMPT_ID_HEX_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RecordId(u64);

impl RecordId {
    pub fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RecordId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProcessStamp {
    pub pid: u32,
    pub started_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CwdSource {
    Report,
    Owner,
    Spawn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordCwd {
    pub path: String,
    pub source: CwdSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxStamp {
    pub root: String,
    pub unix_scope: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub started_at: u64,
    pub ancestors: Vec<ProcessStamp>,
    pub first_process: bool,
    pub fallback_cwd: RecordCwd,
    pub sandbox: Option<SandboxStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AttemptId(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidAttemptId;

impl AttemptId {
    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn parse(value: &str) -> Result<Self, InvalidAttemptId> {
        let fits = value.len() == ATTEMPT_ID_HEX_LEN
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if fits {
            Ok(Self(value.to_owned()))
        } else {
            Err(InvalidAttemptId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AttemptId {
    type Error = InvalidAttemptId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<AttemptId> for String {
    fn from(value: AttemptId) -> Self {
        value.0
    }
}

impl fmt::Display for AttemptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl fmt::Display for InvalidAttemptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "a resume attempt id is {ATTEMPT_ID_HEX_LEN} lowercase hexadecimal characters"
        )
    }
}

impl std::error::Error for InvalidAttemptId {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptPhase {
    Prepared,
    Started,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: AttemptId,
    pub phase: AttemptPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptFailure {
    NotFound,
    NotExecutable,
    CwdMissing,
    HandoffInvalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptError {
    UnknownRecord,
    UnknownAttempt,
}

impl fmt::Display for AttemptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnknownRecord => "no resume record has this id",
            Self::UnknownAttempt => "no pending resume attempt has this id",
        })
    }
}

impl std::error::Error for AttemptError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: RecordId,
    pub argv: Vec<String>,
    pub cwd: String,
    pub cwd_source: CwdSource,
    pub owner: ProcessStamp,
    #[serde(default)]
    pub ancestors: Vec<ProcessStamp>,
    #[serde(default)]
    pub first_process: bool,
    pub accepted_at_ms: u64,
    pub revision: u64,
    #[serde(skip)]
    pub exited_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<Attempt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxStamp>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub handed_off: bool,
}

impl Record {
    pub fn settled(&self) -> bool {
        self.attempt.is_none()
    }

    pub fn resume_input(&self) -> ResumeInput {
        ResumeInput {
            record: self.id,
            argv: self.argv.clone(),
            cwd: self.cwd.clone(),
            cwd_source: self.cwd_source,
            owner_pid: self.owner.pid,
            accepted_at_ms: self.accepted_at_ms,
            settled: self.settled(),
            first_process: self.first_process,
        }
    }

    fn descends_from(&self, other: &Record) -> bool {
        self.ancestors.contains(&other.owner)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeInput {
    pub record: RecordId,
    pub argv: Vec<String>,
    pub cwd: String,
    pub cwd_source: CwdSource,
    pub owner_pid: u32,
    pub accepted_at_ms: u64,
    pub settled: bool,
    pub first_process: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedStore")]
pub struct RecordStore {
    records: Vec<Record>,
    next_id: u64,
    next_revision: u64,
    #[serde(skip)]
    replay: ReplayGuard,
}

#[derive(Deserialize)]
struct SavedStore {
    #[serde(default)]
    records: Vec<SavedRecord>,
    #[serde(default)]
    next_id: u64,
    #[serde(default)]
    next_revision: u64,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SavedRecord {
    Readable(Record),
    Unreadable(serde::de::IgnoredAny),
}

impl From<SavedStore> for RecordStore {
    fn from(saved: SavedStore) -> Self {
        let mut newest_first: Vec<Record> = saved
            .records
            .into_iter()
            .filter_map(|saved| match saved {
                SavedRecord::Readable(record) => Some(record),
                SavedRecord::Unreadable(_) => None,
            })
            .collect();
        newest_first.sort_by_key(|record| std::cmp::Reverse(record.revision));
        let mut ids = HashSet::new();
        let mut owners = HashSet::new();
        let mut records: Vec<Record> = newest_first
            .into_iter()
            .filter(|record| !record.argv.is_empty())
            .filter(|record| ids.insert(record.id) && owners.insert(record.owner.pid))
            .take(MAX_RECORDS)
            .collect();
        records.reverse();
        let next_id = records
            .iter()
            .map(|record| record.id.0.saturating_add(1))
            .fold(saved.next_id, u64::max);
        let next_revision = records
            .iter()
            .map(|record| record.revision.saturating_add(1))
            .fold(saved.next_revision, u64::max);
        Self {
            records,
            next_id,
            next_revision,
            replay: ReplayGuard::default(),
        }
    }
}

impl RecordStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn is_pristine(&self) -> bool {
        self.records.is_empty() && self.next_id == 0 && self.next_revision == 0
    }

    pub fn get(&self, id: RecordId) -> Option<&Record> {
        self.records.iter().find(|record| record.id == id)
    }

    pub fn replay_guard(&self) -> &ReplayGuard {
        &self.replay
    }

    #[must_use]
    pub fn apply(
        &mut self,
        outcome: Outcome,
        admit: impl FnOnce(&Report) -> Option<Admission>,
        now_ms: u64,
    ) -> bool {
        match outcome {
            Outcome::Ignored(_) | Outcome::Rejected { owner: None, .. } => false,
            Outcome::Rejected {
                owner: Some(pid), ..
            } => self.reject(pid),
            Outcome::Accepted(report) => match admit(&report) {
                None => self.reject(report.pid),
                Some(admission) => match report.action {
                    Action::Clear => {
                        self.replay.note(report.pid, report.time, now_ms / 1000);
                        self.remove_owner(report.pid)
                    }
                    Action::Argv(_) => self.insert(report, admission, now_ms),
                },
            },
        }
    }

    #[must_use]
    pub fn reject(&mut self, pid: u32) -> bool {
        self.remove_owner(pid)
    }

    #[must_use]
    pub fn owner_exited(&mut self, owner: ProcessStamp, now_ms: u64) -> bool {
        let mut changed = false;
        for record in &mut self.records {
            if record.owner == owner && record.exited_at_ms.is_none() {
                record.exited_at_ms = Some(now_ms);
                changed = true;
            }
        }
        changed
    }

    #[must_use]
    pub fn remove_exited(&mut self, now_ms: u64) -> bool {
        let before = self.records.len();
        self.records.retain(|record| {
            record
                .exited_at_ms
                .is_none_or(|exited| now_ms < exited.saturating_add(EXIT_GRACE_MS))
        });
        self.records.len() != before
    }

    pub fn next_removal_at(&self) -> Option<u64> {
        self.records
            .iter()
            .filter_map(|record| record.exited_at_ms)
            .map(|exited| exited.saturating_add(EXIT_GRACE_MS))
            .min()
    }

    pub fn candidate(&self) -> Option<&Record> {
        self.records
            .iter()
            .filter(|record| {
                !self
                    .records
                    .iter()
                    .any(|other| other.id != record.id && record.descends_from(other))
            })
            .max_by_key(|record| record.revision)
    }

    pub fn resume_input(&self) -> Option<ResumeInput> {
        self.candidate().map(Record::resume_input)
    }

    #[must_use]
    pub fn discard(&mut self, id: RecordId) -> bool {
        let before = self.records.len();
        self.records.retain(|record| record.id != id);
        self.records.len() != before
    }

    #[must_use]
    pub fn discard_all(&mut self) -> bool {
        let changed = !self.records.is_empty();
        self.records.clear();
        changed
    }

    pub fn begin_attempt(&mut self, id: RecordId, attempt: AttemptId) -> Result<(), AttemptError> {
        let record = self
            .records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(AttemptError::UnknownRecord)?;
        record.attempt = Some(Attempt {
            id: attempt,
            phase: AttemptPhase::Prepared,
        });
        Ok(())
    }

    pub fn attempt_started(
        &mut self,
        attempt: &AttemptId,
        owner: ProcessStamp,
        ancestors: Vec<ProcessStamp>,
    ) -> Result<RecordId, AttemptError> {
        let index = self
            .prepared_attempt(attempt)
            .ok_or(AttemptError::UnknownAttempt)?;
        let id = self.records[index].id;
        if let Some(reported) = self
            .records
            .iter()
            .find(|record| record.id != id && record.owner == owner)
            .map(|record| record.id)
        {
            self.records.retain(|record| record.id != id);
            return Ok(reported);
        }
        self.records
            .retain(|record| record.id == id || record.owner.pid != owner.pid);
        let revision = self.take_revision();
        let record = self
            .records
            .iter_mut()
            .find(|record| record.id == id)
            .ok_or(AttemptError::UnknownAttempt)?;
        record.owner = owner;
        record.ancestors = ancestors;
        record.first_process = false;
        record.handed_off = true;
        record.exited_at_ms = None;
        record.revision = revision;
        record.attempt = Some(Attempt {
            id: attempt.clone(),
            phase: AttemptPhase::Started,
        });
        Ok(id)
    }

    pub fn attempt_failed(
        &mut self,
        attempt: &AttemptId,
        failure: AttemptFailure,
    ) -> Result<RecordId, AttemptError> {
        let index = self
            .records
            .iter()
            .position(|record| {
                record
                    .attempt
                    .as_ref()
                    .is_some_and(|pending| pending.id == *attempt)
            })
            .ok_or(AttemptError::UnknownAttempt)?;
        let id = self.records[index].id;
        match failure {
            AttemptFailure::CwdMissing => self.records[index].attempt = None,
            AttemptFailure::NotFound
            | AttemptFailure::NotExecutable
            | AttemptFailure::HandoffInvalid => {
                self.records.remove(index);
            }
        }
        Ok(id)
    }

    #[must_use]
    pub fn settle_on_input(&mut self) -> bool {
        let mut changed = false;
        for record in &mut self.records {
            if record
                .attempt
                .as_ref()
                .is_some_and(|attempt| attempt.phase == AttemptPhase::Started)
            {
                record.attempt = None;
                changed = true;
            }
        }
        changed
    }

    fn prepared_attempt(&self, attempt: &AttemptId) -> Option<usize> {
        self.records.iter().position(|record| {
            record.attempt.as_ref().is_some_and(|pending| {
                pending.id == *attempt && pending.phase == AttemptPhase::Prepared
            })
        })
    }

    fn insert(&mut self, report: Report, admission: Admission, now_ms: u64) -> bool {
        let Report {
            pid,
            time,
            action,
            cwd,
        } = report;
        let Action::Argv(argv) = action else {
            return false;
        };
        self.replay.note(pid, time, now_ms / 1000);
        self.records.retain(|record| {
            record.owner.pid != pid
                && !(record.handed_off && admission.ancestors.contains(&record.owner))
        });
        if self.records.len() >= MAX_RECORDS
            && let Some(oldest) = self
                .records
                .iter()
                .enumerate()
                .min_by_key(|(_, record)| record.revision)
                .map(|(index, _)| index)
        {
            self.records.remove(oldest);
        }
        let cwd = match cwd {
            Some(path) => RecordCwd {
                path,
                source: CwdSource::Report,
            },
            None => admission.fallback_cwd,
        };
        let id = RecordId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        let revision = self.take_revision();
        self.records.push(Record {
            id,
            argv,
            cwd: cwd.path,
            cwd_source: cwd.source,
            owner: ProcessStamp {
                pid,
                started_at: admission.started_at,
            },
            ancestors: admission.ancestors,
            first_process: admission.first_process,
            accepted_at_ms: now_ms,
            revision,
            exited_at_ms: None,
            attempt: None,
            sandbox: admission.sandbox,
            handed_off: false,
        });
        true
    }

    fn remove_owner(&mut self, pid: u32) -> bool {
        let before = self.records.len();
        self.records.retain(|record| record.owner.pid != pid);
        self.records.len() != before
    }

    fn take_revision(&mut self) -> u64 {
        let revision = self.next_revision;
        self.next_revision = self.next_revision.saturating_add(1);
        revision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::Malformation;
    use crate::verify::IgnoreReason;

    const NOW_MS: u64 = 1_795_543_200_000;
    const NOW_SECS: u64 = NOW_MS / 1000;

    fn argv_report(pid: u32, argv: &[&str]) -> Report {
        Report {
            pid,
            time: NOW_SECS,
            action: Action::Argv(argv.iter().map(|arg| (*arg).to_owned()).collect()),
            cwd: None,
        }
    }

    fn admission(started_at: u64, ancestors: Vec<ProcessStamp>) -> Admission {
        Admission {
            started_at,
            ancestors,
            first_process: false,
            fallback_cwd: RecordCwd {
                path: "/home/user".to_owned(),
                source: CwdSource::Owner,
            },
            sandbox: None,
        }
    }

    fn stamp(pid: u32) -> ProcessStamp {
        ProcessStamp {
            pid,
            started_at: u64::from(pid) * 10,
        }
    }

    fn accept(store: &mut RecordStore, report: Report, ancestors: Vec<ProcessStamp>) -> bool {
        let started_at = stamp(report.pid).started_at;
        store.apply(
            Outcome::Accepted(report),
            |_| Some(admission(started_at, ancestors)),
            NOW_MS,
        )
    }

    fn attempt_id(seed: u8) -> AttemptId {
        AttemptId::from_bytes([seed; 16])
    }

    fn argv_of(store: &RecordStore) -> Vec<Vec<String>> {
        store
            .records()
            .iter()
            .map(|record| record.argv.clone())
            .collect()
    }

    #[test]
    fn an_accepted_argv_report_holds_exactly_one_record_for_its_owner() {
        let mut store = RecordStore::new();
        assert!(accept(
            &mut store,
            argv_report(4242, &["nvim", "-S", "s.vim"]),
            vec![]
        ));
        assert_eq!(store.records().len(), 1);
        let record = &store.records()[0];
        assert_eq!(record.owner, stamp(4242));
        assert_eq!(record.argv, ["nvim", "-S", "s.vim"]);
        assert_eq!(record.accepted_at_ms, NOW_MS);
        assert_eq!(store.replay_guard().last_accepted(4242), Some(NOW_SECS));
    }

    #[test]
    fn a_second_accepted_report_replaces_the_first_record() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(
            &mut store,
            argv_report(4242, &["nvim", "-S", "new.vim"]),
            vec![]
        ));
        assert_eq!(argv_of(&store), [vec!["nvim", "-S", "new.vim"]]);
    }

    #[test]
    fn an_accepted_clear_removes_the_owner_record() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(&mut store, argv_report(5151, &["lazygit"]), vec![]));
        let clear = Report {
            pid: 4242,
            time: NOW_SECS,
            action: Action::Clear,
            cwd: None,
        };
        assert!(store.apply(Outcome::Accepted(clear), |_| None, NOW_MS));
        assert_eq!(argv_of(&store), [vec!["lazygit"]]);
    }

    #[test]
    fn a_rejected_outcome_removes_every_record_of_its_owner() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(&mut store, argv_report(5151, &["lazygit"]), vec![]));
        let rejected = Outcome::Rejected {
            owner: Some(4242),
            reason: Malformation::RelativeProgram,
        };
        assert!(store.apply(rejected, |_| None, NOW_MS));
        assert_eq!(argv_of(&store), [vec!["lazygit"]]);
    }

    #[test]
    fn a_rejection_without_an_owner_changes_nothing() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let before = store.clone();
        let rejected = Outcome::Rejected {
            owner: None,
            reason: Malformation::BadPid,
        };
        assert!(!store.apply(rejected, |_| None, NOW_MS));
        assert_eq!(store, before);
    }

    #[test]
    fn a_refused_admission_rejects_the_owner() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let changed = store.apply(
            Outcome::Accepted(argv_report(4242, &["nvim", "-S", "x"])),
            |_| None,
            NOW_MS,
        );
        assert!(changed);
        assert!(store.is_empty());
        assert_eq!(store.replay_guard().last_accepted(4242), Some(NOW_SECS));
    }

    #[test]
    fn an_ignored_outcome_leaves_the_store_unchanged() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let before = store.clone();
        let changed = store.apply(
            Outcome::Ignored(IgnoreReason::MacMismatch),
            |_| unreachable!("an ignored outcome admits nothing"),
            NOW_MS,
        );
        assert!(!changed);
        assert_eq!(store, before);
    }

    #[test]
    fn a_seventeenth_record_evicts_the_least_recently_updated_one() {
        let mut store = RecordStore::new();
        for pid in 1..=16 {
            assert!(accept(&mut store, argv_report(pid, &["prog"]), vec![]));
        }
        assert!(accept(
            &mut store,
            argv_report(1, &["prog", "again"]),
            vec![]
        ));
        assert!(accept(&mut store, argv_report(17, &["prog"]), vec![]));
        assert_eq!(store.records().len(), MAX_RECORDS);
        let owners: Vec<u32> = store
            .records()
            .iter()
            .map(|record| record.owner.pid)
            .collect();
        assert!(!owners.contains(&2));
        assert!(owners.contains(&1));
        assert!(owners.contains(&17));
    }

    #[test]
    fn the_parent_record_is_chosen_over_its_descendant() {
        let mut store = RecordStore::new();
        let shell = stamp(100);
        assert!(accept(
            &mut store,
            argv_report(200, &["wrapper"]),
            vec![shell]
        ));
        assert!(accept(
            &mut store,
            argv_report(300, &["child"]),
            vec![stamp(200), shell]
        ));
        let candidate = store.candidate().expect("a candidate exists");
        assert_eq!(candidate.argv, ["wrapper"]);
    }

    #[test]
    fn a_reused_ancestor_pid_with_another_start_time_is_not_an_ancestor() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(200, &["wrapper"]), vec![]));
        let reused = ProcessStamp {
            pid: 200,
            started_at: 1,
        };
        assert!(accept(
            &mut store,
            argv_report(300, &["child"]),
            vec![reused]
        ));
        assert_eq!(store.candidate().map(|record| record.owner.pid), Some(300));
    }

    #[test]
    fn the_most_recently_updated_of_two_unrelated_roots_is_chosen() {
        let mut store = RecordStore::new();
        assert!(accept(
            &mut store,
            argv_report(200, &["first"]),
            vec![stamp(100)]
        ));
        assert!(accept(
            &mut store,
            argv_report(300, &["second"]),
            vec![stamp(100)]
        ));
        assert_eq!(store.candidate().map(|record| record.owner.pid), Some(300));
        assert!(accept(
            &mut store,
            argv_report(200, &["first", "again"]),
            vec![stamp(100)]
        ));
        assert_eq!(store.candidate().map(|record| record.owner.pid), Some(200));
    }

    #[test]
    fn an_empty_store_has_no_candidate() {
        assert!(RecordStore::new().candidate().is_none());
        assert!(RecordStore::new().resume_input().is_none());
    }

    #[test]
    fn an_exited_owner_record_stays_one_second_later_marked_exited() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let exit = NOW_MS + 5_000;
        assert!(store.owner_exited(stamp(4242), exit));
        assert!(!store.remove_exited(exit + 1_000));
        assert_eq!(store.records().len(), 1);
        assert_eq!(store.records()[0].exited_at_ms, Some(exit));
        assert_eq!(store.next_removal_at(), Some(exit + EXIT_GRACE_MS));
    }

    #[test]
    fn an_exited_owner_record_is_removed_two_seconds_later() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let exit = NOW_MS + 5_000;
        assert!(store.owner_exited(stamp(4242), exit));
        assert!(store.remove_exited(exit + 2_000));
        assert!(store.is_empty());
        assert_eq!(store.next_removal_at(), None);
    }

    #[test]
    fn an_exit_of_a_reused_pid_leaves_the_record_alone() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let stranger = ProcessStamp {
            pid: 4242,
            started_at: 1,
        };
        assert!(!store.owner_exited(stranger, NOW_MS));
        assert_eq!(store.next_removal_at(), None);
    }

    #[test]
    fn the_exit_mark_is_not_saved() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(store.owner_exited(stamp(4242), NOW_MS));
        let saved = serde_json::to_string(&store).expect("the store serializes");
        let restored: RecordStore = serde_json::from_str(&saved).expect("the store reads back");
        assert_eq!(restored.records()[0].exited_at_ms, None);
        assert!(!restored.clone().remove_exited(u64::MAX));
    }

    #[test]
    fn a_prepared_or_started_attempt_is_unsettled() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        assert!(store.resume_input().is_some_and(|input| input.settled));
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        assert!(store.resume_input().is_some_and(|input| !input.settled));
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![stamp(100)])
            .expect("the attempt is prepared");
        let input = store.resume_input().expect("the record stays");
        assert!(!input.settled);
        assert_eq!(input.owner_pid, 5000);
        assert_eq!(input.record, id);
    }

    #[test]
    fn a_started_attempt_survives_a_save_as_unsettled() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        let saved = serde_json::to_string(&store).expect("the store serializes");
        let restored: RecordStore = serde_json::from_str(&saved).expect("the store reads back");
        assert!(restored.resume_input().is_some_and(|input| !input.settled));
    }

    #[test]
    fn the_first_input_settles_a_started_attempt_but_not_a_prepared_one() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        assert!(!store.settle_on_input());
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        assert!(store.settle_on_input());
        assert!(store.resume_input().is_some_and(|input| input.settled));
    }

    #[test]
    fn a_report_from_the_resumed_owner_settles_the_attempt() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        assert!(accept(
            &mut store,
            argv_report(5000, &["nvim", "-S", "s"]),
            vec![]
        ));
        assert_eq!(store.records().len(), 1);
        assert!(store.records()[0].settled());
    }

    #[test]
    fn a_started_attempt_takes_over_a_stale_record_of_the_new_pid() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(&mut store, argv_report(5000, &["stale"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        let reused = ProcessStamp {
            pid: 5000,
            started_at: 77,
        };
        assert_eq!(
            store.attempt_started(&attempt_id(1), reused, vec![]),
            Ok(id)
        );
        assert_eq!(argv_of(&store), [vec!["nvim"]]);
        assert_eq!(store.records()[0].owner, reused);
    }

    #[test]
    fn a_report_from_the_resumed_program_before_its_start_notice_settles_the_attempt() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        assert!(accept(
            &mut store,
            argv_report(5000, &["nvim", "-S", "fresh.vim"]),
            vec![stamp(100)]
        ));
        let fresh = store.records()[1].id;
        assert_eq!(
            store.attempt_started(&attempt_id(1), stamp(5000), vec![stamp(100)]),
            Ok(fresh)
        );
        assert_eq!(argv_of(&store), [vec!["nvim", "-S", "fresh.vim"]]);
        assert!(store.records()[0].settled());
        assert_eq!(
            store.attempt_failed(&attempt_id(1), AttemptFailure::NotFound),
            Err(AttemptError::UnknownAttempt)
        );
    }

    #[test]
    fn a_report_from_a_child_of_the_resumed_launcher_replaces_the_handed_off_record() {
        let mut store = RecordStore::new();
        assert!(accept(
            &mut store,
            argv_report(4242, &["pi", "--session", "old"]),
            vec![stamp(100)]
        ));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![stamp(100)])
            .expect("the attempt is prepared");
        assert!(store.records()[0].handed_off);
        assert!(accept(
            &mut store,
            argv_report(5001, &["pi", "--session", "old"]),
            vec![stamp(5000), stamp(100)]
        ));
        assert_eq!(store.records().len(), 1, "{:?}", store.records());
        let record = &store.records()[0];
        assert_eq!(record.owner, stamp(5001));
        assert!(record.settled());
        assert!(!record.handed_off);

        let clear = Report {
            pid: 5001,
            time: NOW_SECS + 1,
            action: Action::Clear,
            cwd: None,
        };
        assert!(store.apply(
            Outcome::Accepted(clear),
            |_| Some(admission(
                stamp(5001).started_at,
                vec![stamp(5000), stamp(100)]
            )),
            NOW_MS + 1000
        ));
        assert!(
            store.candidate().is_none(),
            "a cleared session leaves no stale handed-off command behind"
        );
    }

    #[test]
    fn a_child_report_keeps_the_record_of_an_ancestor_that_reported_itself() {
        let mut store = RecordStore::new();
        assert!(accept(
            &mut store,
            argv_report(4242, &["resume-wrap", "ssh"]),
            vec![]
        ));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        assert!(accept(
            &mut store,
            argv_report(5000, &["resume-wrap", "ssh"]),
            vec![]
        ));
        assert!(accept(
            &mut store,
            argv_report(5001, &["ssh", "devbox"]),
            vec![stamp(5000)]
        ));
        assert_eq!(store.records().len(), 2);
        assert_eq!(
            store.candidate().map(|record| record.argv.clone()),
            Some(vec!["resume-wrap".to_owned(), "ssh".to_owned()])
        );
    }

    #[test]
    fn the_handed_off_mark_survives_a_save_and_is_absent_from_an_ordinary_record() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let plain = serde_json::to_value(&store).unwrap();
        assert!(plain["records"][0].get("handed_off").is_none());
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        let saved = serde_json::to_string(&store).unwrap();
        let read: RecordStore = serde_json::from_str(&saved).unwrap();
        assert!(read.records()[0].handed_off);
    }

    #[test]
    fn an_admitted_clear_sets_the_replay_baseline_and_a_refused_one_does_not() {
        let clear = |pid| Report {
            pid,
            time: NOW_SECS,
            action: Action::Clear,
            cwd: None,
        };
        let mut store = RecordStore::new();
        assert!(!store.apply(
            Outcome::Accepted(clear(4242)),
            |_| Some(admission(1, vec![])),
            NOW_MS
        ));
        assert_eq!(store.replay_guard().last_accepted(4242), Some(NOW_SECS));
        for pid in 10_000..10_100 {
            assert!(!store.apply(Outcome::Accepted(clear(pid)), |_| None, NOW_MS));
            assert_eq!(store.replay_guard().last_accepted(pid), None);
        }
    }

    #[test]
    fn a_failed_attempt_drops_the_record_except_for_a_missing_directory() {
        for (failure, kept) in [
            (AttemptFailure::NotFound, false),
            (AttemptFailure::NotExecutable, false),
            (AttemptFailure::HandoffInvalid, false),
            (AttemptFailure::CwdMissing, true),
        ] {
            let mut store = RecordStore::new();
            assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
            let id = store.records()[0].id;
            store
                .begin_attempt(id, attempt_id(1))
                .expect("the record exists");
            assert_eq!(store.attempt_failed(&attempt_id(1), failure), Ok(id));
            assert_eq!(store.get(id).is_some(), kept, "{failure:?}");
            if kept {
                assert!(store.get(id).is_some_and(Record::settled));
            }
        }
    }

    #[test]
    fn a_program_that_fails_to_start_after_its_started_report_drops_the_record() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        assert_eq!(
            store.attempt_failed(&attempt_id(1), AttemptFailure::NotExecutable),
            Ok(id)
        );
        assert!(store.is_empty());
    }

    #[test]
    fn a_sandbox_stamp_from_the_admission_is_kept_on_the_record_and_survives_a_round_trip() {
        let mut store = RecordStore::new();
        let stamp = SandboxStamp {
            root: "/work".to_owned(),
            unix_scope: true,
        };
        let mut admitted = admission(1, vec![]);
        admitted.sandbox = Some(stamp.clone());
        assert!(store.apply(
            Outcome::Accepted(argv_report(4242, &["nvim"])),
            |_| Some(admitted),
            NOW_MS
        ));
        assert_eq!(store.records()[0].sandbox.as_ref(), Some(&stamp));
        let json = serde_json::to_string(&store).expect("the store serializes");
        let read: RecordStore = serde_json::from_str(&json).expect("the store reads back");
        assert_eq!(read.records()[0].sandbox.as_ref(), Some(&stamp));
    }

    #[test]
    fn unknown_records_and_attempts_are_refused() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert_eq!(
            store.begin_attempt(RecordId::from(99), attempt_id(1)),
            Err(AttemptError::UnknownRecord)
        );
        assert_eq!(
            store.attempt_started(&attempt_id(2), stamp(5000), vec![]),
            Err(AttemptError::UnknownAttempt)
        );
        assert_eq!(
            store.attempt_failed(&attempt_id(2), AttemptFailure::NotFound),
            Err(AttemptError::UnknownAttempt)
        );
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        store
            .attempt_started(&attempt_id(1), stamp(5000), vec![])
            .expect("the attempt is prepared");
        assert_eq!(
            store.attempt_started(&attempt_id(1), stamp(6000), vec![]),
            Err(AttemptError::UnknownAttempt)
        );
    }

    #[test]
    fn a_report_cwd_wins_over_the_fallback_directory() {
        let mut store = RecordStore::new();
        let mut report = argv_report(4242, &["lazygit"]);
        report.cwd = Some("/home/user/src/project".to_owned());
        assert!(accept(&mut store, report, vec![]));
        let record = &store.records()[0];
        assert_eq!(record.cwd, "/home/user/src/project");
        assert_eq!(record.cwd_source, CwdSource::Report);
        assert!(accept(&mut store, argv_report(4242, &["lazygit"]), vec![]));
        let record = &store.records()[0];
        assert_eq!(record.cwd, "/home/user");
        assert_eq!(record.cwd_source, CwdSource::Owner);
    }

    #[test]
    fn discarding_removes_one_record_or_all_of_them() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(&mut store, argv_report(5151, &["lazygit"]), vec![]));
        let id = store.records()[0].id;
        assert!(store.discard(id));
        assert!(!store.discard(id));
        assert_eq!(argv_of(&store), [vec!["lazygit"]]);
        assert!(store.discard_all());
        assert!(!store.discard_all());
    }

    #[test]
    fn record_ids_are_never_reused() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let first = store.records()[0].id;
        assert!(store.discard(first));
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert_ne!(store.records()[0].id, first);
    }

    #[test]
    fn a_saved_store_reads_back_with_an_unknown_extra_field() {
        let mut store = RecordStore::new();
        assert!(accept(
            &mut store,
            argv_report(4242, &["nvim", "-S", "s.vim"]),
            vec![stamp(100)]
        ));
        let mut saved = serde_json::to_value(&store).expect("the store serializes");
        saved["from_a_newer_host"] = serde_json::json!({"anything": [1, 2]});
        saved["records"][0]["newer_record_field"] = serde_json::json!(true);
        let restored: RecordStore =
            serde_json::from_value(saved).expect("unknown fields are tolerated");
        assert_eq!(restored.records(), store.records());
    }

    #[test]
    fn a_saved_record_this_version_cannot_read_is_dropped_and_the_others_kept() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(accept(&mut store, argv_report(5151, &["lazygit"]), vec![]));
        let mut saved = serde_json::to_value(&store).expect("the store serializes");
        saved["records"][0]["cwd_source"] = serde_json::json!("a_newer_source");
        let restored: RecordStore =
            serde_json::from_value(saved).expect("one unreadable record keeps the store readable");
        assert_eq!(argv_of(&restored), [vec!["lazygit"]]);
    }

    #[test]
    fn an_empty_object_reads_as_an_empty_store() {
        let restored: RecordStore = serde_json::from_str("{}").expect("every field defaults");
        assert!(restored.is_empty());
    }

    #[test]
    fn a_saved_store_with_stale_counters_never_reuses_an_id() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let mut saved = serde_json::to_value(&store).expect("the store serializes");
        saved["next_id"] = serde_json::json!(0);
        saved["next_revision"] = serde_json::json!(0);
        let mut restored: RecordStore =
            serde_json::from_value(saved).expect("the store reads back");
        assert!(accept(
            &mut restored,
            argv_report(5151, &["lazygit"]),
            vec![]
        ));
        let ids: HashSet<RecordId> = restored.records().iter().map(|record| record.id).collect();
        assert_eq!(ids.len(), 2);
        assert_eq!(
            restored.candidate().map(|record| record.owner.pid),
            Some(5151)
        );
    }

    #[test]
    fn a_saved_store_keeps_at_most_sixteen_records_and_one_per_owner() {
        let records: Vec<serde_json::Value> = (0..20u32)
            .map(|index| {
                serde_json::json!({
                    "id": index,
                    "argv": ["prog"],
                    "cwd": "/",
                    "cwd_source": "spawn",
                    "owner": {"pid": if index >= 18 { 7 } else { index + 100 }, "started_at": 1},
                    "accepted_at_ms": 0,
                    "revision": index,
                })
            })
            .collect();
        let restored: RecordStore =
            serde_json::from_value(serde_json::json!({ "records": records }))
                .expect("the store reads back");
        assert_eq!(restored.records().len(), MAX_RECORDS);
        let owners: HashSet<u32> = restored
            .records()
            .iter()
            .map(|record| record.owner.pid)
            .collect();
        assert_eq!(owners.len(), MAX_RECORDS);
        let revisions: HashSet<u64> = restored
            .records()
            .iter()
            .map(|record| record.revision)
            .collect();
        assert!(revisions.contains(&19));
        assert!(!revisions.contains(&18));
        assert!(revisions.iter().all(|revision| *revision >= 3));
    }

    #[test]
    fn attempt_ids_are_32_lowercase_hex_characters() {
        let id = attempt_id(0xab);
        assert_eq!(id.as_str(), "ab".repeat(16));
        assert_eq!(AttemptId::parse(id.as_str()), Ok(id.clone()));
        assert_eq!(AttemptId::parse(&"AB".repeat(16)), Err(InvalidAttemptId));
        assert_eq!(AttemptId::parse("abc"), Err(InvalidAttemptId));
        let refused: Result<AttemptId, _> = serde_json::from_str("\"not-an-attempt\"");
        assert!(refused.is_err());
        let encoded = serde_json::to_string(&id).expect("an attempt id serializes");
        assert_eq!(encoded, format!("\"{id}\""));
    }

    #[test]
    fn the_saved_form_uses_snake_case_names() {
        let mut store = RecordStore::new();
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        let id = store.records()[0].id;
        store
            .begin_attempt(id, attempt_id(1))
            .expect("the record exists");
        let saved = serde_json::to_value(&store).expect("the store serializes");
        assert_eq!(saved["records"][0]["cwd_source"], "owner");
        assert_eq!(saved["records"][0]["attempt"]["phase"], "prepared");
        assert!(saved.get("replay").is_none());
    }

    #[test]
    fn a_store_stays_pristine_until_its_first_record_and_not_after_it_is_cleared() {
        let mut store = RecordStore::new();
        assert!(store.is_pristine());
        assert!(accept(&mut store, argv_report(4242, &["nvim"]), vec![]));
        assert!(!store.is_pristine());
        assert!(store.discard_all());
        assert!(store.is_empty());
        assert!(!store.is_pristine());
    }
}
