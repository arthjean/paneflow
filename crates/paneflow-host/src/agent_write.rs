use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_agent_config::delivery::{
    DeliveryRefusal, FOREGROUND_RUNTIME, SUBMIT_START_POLL, SUBMIT_START_TIMEOUT,
    confirm_turn_start, delivery_refusal, reduce_row,
};
use paneflow_config::schema::{SessionGeneration, SessionId};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

use crate::control::{
    ControlError, ControlPermissions, agent_status_value, authorize_write_scope, deliver_text,
    foreground_runtime, output_generation, surface_name,
};
use crate::host::{SessionHost, SessionSummary};

pub const METHOD_PANE_WRITE: &str = "pane.write";
pub const METHOD_APPROVAL_FOLLOW: &str = "approval.follow";
pub const METHOD_APPROVAL_DECIDE: &str = "approval.decide";

pub const MAX_RELAYED_TEXT_BYTES: usize = 16 * 1024;
pub const APPROVAL_REQUEST_TTL: Duration = Duration::from_secs(120);
pub const WRITE_REFILL_INTERVAL: Duration = Duration::from_secs(1);
pub const WRITE_BURST: u32 = 3;

const MAX_LABEL_CHARS: usize = 64;
const WATCHER_QUEUE_SLOTS: usize = 64;
const WORKER_DEADLINE: Duration = Duration::from_secs(2);
const WORKER_CLIENT: &str = "paneflow-host";
const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentOccurrence {
    pub session: SessionId,
    pub generation: SessionGeneration,
    pub pid: u32,
    pub started_at: Option<u64>,
}

impl AgentOccurrence {
    pub fn of(summary: &SessionSummary) -> Option<Self> {
        if !summary.live {
            return None;
        }
        let manifest = &summary.manifest;
        let observed = manifest
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.current_observation.as_ref())
            .map(|observation| (observation.pid, observation.pid_started_at));
        let (pid, started_at) = observed.or_else(|| {
            manifest
                .process
                .as_ref()
                .map(|process| (process.pid, process.started_at))
        })?;
        Some(Self {
            session: manifest.session.clone(),
            generation: manifest.generation,
            pid,
            started_at,
        })
    }
}

type Pair = (AgentOccurrence, AgentOccurrence);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingApproval {
    pub id: u64,
    pub source: AgentOccurrence,
    pub target: AgentOccurrence,
    pub source_label: String,
    pub target_label: String,
    created: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Settled {
    Denied,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Approved,
    Pending { id: u64 },
    Denied,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
}

impl Decision {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Allow => "allowed",
            Self::Deny => "denied",
        }
    }
}

struct Bucket {
    tokens: u32,
    refilled: Instant,
}

#[derive(Default)]
struct Book {
    next_id: u64,
    approved: HashSet<Pair>,
    pending: Vec<PendingApproval>,
    settled: HashMap<Pair, Settled>,
    buckets: HashMap<Pair, Bucket>,
}

#[derive(Default)]
pub struct WriteApprovals {
    book: Mutex<Book>,
    watchers: Mutex<Vec<(u64, SyncSender<Value>)>>,
    next_watcher: AtomicU64,
}

pub struct ApprovalWatch {
    pub id: u64,
    pub frames: Receiver<Value>,
}

impl WriteApprovals {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn watchers(&self) -> MutexGuard<'_, Vec<(u64, SyncSender<Value>)>> {
        self.watchers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn watch(&self) -> ApprovalWatch {
        let id = self.next_watcher.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, frames) = sync_channel(WATCHER_QUEUE_SLOTS);
        self.watchers().push((id, tx));
        ApprovalWatch { id, frames }
    }

    pub fn unwatch(&self, id: u64) {
        self.watchers().retain(|(held, _)| *held != id);
    }

    pub fn has_window(&self) -> bool {
        !self.watchers().is_empty()
    }

    pub fn snapshot(&self, now: Instant) -> Value {
        snapshot_of(&self.book().pending, now)
    }

    fn publish(&self, frame: Value) {
        self.watchers().retain(|(_, tx)| {
            !matches!(
                tx.try_send(frame.clone()),
                Err(TrySendError::Disconnected(_))
            )
        });
    }

    pub fn admit(
        &self,
        source: &AgentOccurrence,
        target: &AgentOccurrence,
        source_label: &str,
        target_label: &str,
        now: Instant,
    ) -> Admission {
        let mut book = self.book();
        let mut changed = expire_due(&mut book, now);
        let pair = (source.clone(), target.clone());
        let admission = if book.approved.contains(&pair) {
            Admission::Approved
        } else if let Some(settled) = book.settled.remove(&pair) {
            match settled {
                Settled::Denied => Admission::Denied,
                Settled::Expired => Admission::Expired,
            }
        } else if let Some(pending) = book
            .pending
            .iter()
            .find(|pending| pending.source == *source && pending.target == *target)
        {
            Admission::Pending { id: pending.id }
        } else {
            book.next_id += 1;
            let id = book.next_id;
            book.pending.push(PendingApproval {
                id,
                source: source.clone(),
                target: target.clone(),
                source_label: source_label.to_string(),
                target_label: target_label.to_string(),
                created: now,
            });
            log::info!(
                "paneflow-host: write request {id}: session {} wants to write to session {}",
                source.session,
                target.session
            );
            changed = true;
            Admission::Pending { id }
        };
        let frame = changed.then(|| snapshot_of(&book.pending, now));
        drop(book);
        if let Some(frame) = frame {
            self.publish(frame);
        }
        admission
    }

    pub fn take_token(
        &self,
        source: &AgentOccurrence,
        target: &AgentOccurrence,
        now: Instant,
    ) -> Result<(), Duration> {
        let mut book = self.book();
        let bucket = book
            .buckets
            .entry((source.clone(), target.clone()))
            .or_insert(Bucket {
                tokens: WRITE_BURST,
                refilled: now,
            });
        let elapsed = now.saturating_duration_since(bucket.refilled);
        let earned = u32::try_from(elapsed.as_millis() / WRITE_REFILL_INTERVAL.as_millis())
            .unwrap_or(WRITE_BURST)
            .min(WRITE_BURST);
        if earned > 0 {
            bucket.tokens = (bucket.tokens + earned).min(WRITE_BURST);
            bucket.refilled += WRITE_REFILL_INTERVAL * earned;
        }
        if bucket.tokens == WRITE_BURST {
            bucket.refilled = now;
        }
        if bucket.tokens == 0 {
            return Err(WRITE_REFILL_INTERVAL
                .saturating_sub(now.saturating_duration_since(bucket.refilled)));
        }
        bucket.tokens -= 1;
        Ok(())
    }

    pub fn decide(&self, id: u64, decision: Decision, now: Instant) -> Result<(), String> {
        let mut book = self.book();
        expire_due(&mut book, now);
        let Some(index) = book.pending.iter().position(|pending| pending.id == id) else {
            let frame = snapshot_of(&book.pending, now);
            drop(book);
            self.publish(frame);
            return Err(format!(
                "write request {id} is no longer pending; it was decided, expired, or its agents ended"
            ));
        };
        let pending = book.pending.remove(index);
        let pair = (pending.source.clone(), pending.target.clone());
        match decision {
            Decision::Allow => {
                book.approved.insert(pair);
            }
            Decision::Deny => {
                book.settled.insert(pair, Settled::Denied);
            }
        }
        log::info!(
            "paneflow-host: write request {id} {}: session {} to session {}",
            decision.label(),
            pending.source.session,
            pending.target.session
        );
        let frame = snapshot_of(&book.pending, now);
        drop(book);
        self.publish(frame);
        Ok(())
    }

    pub fn expire(&self, now: Instant) {
        let mut book = self.book();
        if !expire_due(&mut book, now) {
            return;
        }
        let frame = snapshot_of(&book.pending, now);
        drop(book);
        self.publish(frame);
    }

    pub fn retain_live(&self, live: &HashSet<AgentOccurrence>, now: Instant) {
        let mut book = self.book();
        let alive = |pair: &Pair| live.contains(&pair.0) && live.contains(&pair.1);
        book.approved.retain(alive);
        book.settled.retain(|pair, _| alive(pair));
        book.buckets.retain(|pair, _| alive(pair));
        let before = book.pending.len();
        book.pending.retain(|pending| {
            let keep = live.contains(&pending.source) && live.contains(&pending.target);
            if !keep {
                log::info!(
                    "paneflow-host: write request {} expired: an agent of the pair ended",
                    pending.id
                );
            }
            keep
        });
        if book.pending.len() == before {
            return;
        }
        let frame = snapshot_of(&book.pending, now);
        drop(book);
        self.publish(frame);
    }

    pub fn next_expiry(&self, now: Instant) -> Option<Duration> {
        self.book()
            .pending
            .iter()
            .map(|pending| (pending.created + APPROVAL_REQUEST_TTL).saturating_duration_since(now))
            .min()
    }
}

fn expire_due(book: &mut Book, now: Instant) -> bool {
    let mut expired = Vec::new();
    book.pending.retain(|pending| {
        let due = now.saturating_duration_since(pending.created) >= APPROVAL_REQUEST_TTL;
        if due {
            expired.push(pending.clone());
        }
        !due
    });
    for pending in &expired {
        log::info!(
            "paneflow-host: write request {} expired unanswered: session {} to session {}",
            pending.id,
            pending.source.session,
            pending.target.session
        );
        book.settled.insert(
            (pending.source.clone(), pending.target.clone()),
            Settled::Expired,
        );
    }
    !expired.is_empty()
}

fn snapshot_of(pending: &[PendingApproval], now: Instant) -> Value {
    let requests: Vec<Value> = pending
        .iter()
        .map(|pending| {
            json!({
                "id": pending.id,
                "source_session": pending.source.session,
                "target_session": pending.target.session,
                "source": pending.source_label,
                "target": pending.target_label,
                "expires_in_ms": (pending.created + APPROVAL_REQUEST_TTL)
                    .saturating_duration_since(now)
                    .as_millis() as u64,
            })
        })
        .collect();
    json!({"type": "approvals", "pending": requests})
}

pub fn sanitize_relayed_text(text: &str) -> String {
    text.replace(PASTE_START, "")
        .replace(PASTE_END, "")
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !c.is_control())
        .collect()
}

fn label_of(summary: &SessionSummary) -> String {
    let clean: String = sanitize_relayed_text(&surface_name(summary))
        .chars()
        .filter(|&c| !matches!(c, '\n' | '\t' | '[' | ']'))
        .take(MAX_LABEL_CHARS)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        summary.manifest.session.to_string()
    } else {
        clean.to_string()
    }
}

fn surface_number(summary: &SessionSummary) -> String {
    summary
        .manifest
        .launch
        .env
        .get("PANEFLOW_SURFACE_ID")
        .map(String::as_str)
        .map(str::trim)
        .filter(|raw| !raw.is_empty() && raw.chars().all(|c| c.is_ascii_digit()))
        .map_or_else(|| summary.manifest.session.to_string(), str::to_string)
}

pub fn provenance_line(summary: &SessionSummary) -> String {
    format!(
        "[Paneflow: message from {}, surface {}]",
        label_of(summary),
        surface_number(summary)
    )
}

fn worker_projections(host: &SessionHost) -> Option<Vec<Value>> {
    let endpoint = paneflow_home::serve_endpoint_path(host.home());
    let mut worker = HostControl::open(&endpoint, WORKER_CLIENT, WORKER_DEADLINE).ok()?;
    let answered = worker
        .request_with_deadline("agent.snapshot", json!({}), WORKER_DEADLINE)
        .ok()?;
    answered["sessions"].as_array().cloned()
}

fn reduced_status(host: &SessionHost, session: &SessionId, now_ms: u64) -> Option<Value> {
    let summary = host.inspect(session).ok()?;
    let mut status = agent_status_value(
        0,
        session,
        summary.manifest.generation,
        summary.manifest.last_hook.as_ref(),
        output_generation(host, session),
        now_ms,
    );
    status[FOREGROUND_RUNTIME] = foreground_runtime(&summary);
    if let Some(projections) = worker_projections(host) {
        reduce_row(&mut status, &projections);
    }
    Some(status)
}

fn param_session(params: &Value, key: &str, missing: &str) -> Result<SessionId, ControlError> {
    let raw = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|raw| !raw.is_empty())
        .ok_or_else(|| ControlError::Params(missing.to_string()))?;
    SessionId::parse(raw).map_err(|error| ControlError::Params(format!("'{key}': {error}")))
}

fn live_occurrences(host: &SessionHost) -> HashSet<AgentOccurrence> {
    host.list(None)
        .iter()
        .filter_map(AgentOccurrence::of)
        .collect()
}

pub fn pane_write(
    host: &SessionHost,
    permissions: ControlPermissions,
    params: &Value,
    now_ms: u64,
) -> Result<Value, ControlError> {
    let source = param_session(
        params,
        "source_session",
        "pane.write needs the session of the calling pane (PANEFLOW_SESSION_ID); \
         launch the agent from a Paneflow pane",
    )?;
    let target = param_session(params, "session", "pane.write needs a target session")?;
    let text = params
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| ControlError::Params("missing or empty 'text'".to_string()))?;
    if text.len() > MAX_RELAYED_TEXT_BYTES {
        return Err(ControlError::Params(format!(
            "text is {} bytes; pane.write relays at most {MAX_RELAYED_TEXT_BYTES} bytes",
            text.len()
        )));
    }
    let submit = params
        .get("submit")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if source == target {
        return Err(ControlError::Params(
            "an agent cannot write into its own pane".to_string(),
        ));
    }
    let mut scoped = json!({"scope_session": source});
    if let Some(scope) = params.get("scope") {
        scoped["scope"] = scope.clone();
    }
    authorize_write_scope(host, &scoped, permissions, &target)?;

    let source_summary = host.inspect(&source)?;
    let target_summary = host.inspect(&target)?;
    let unhosted = |session: &SessionId| {
        ControlError::Params(format!(
            "session {session} runs no live process, so no agent can write or be written to"
        ))
    };
    let source_occurrence =
        AgentOccurrence::of(&source_summary).ok_or_else(|| unhosted(&source))?;
    let target_occurrence =
        AgentOccurrence::of(&target_summary).ok_or_else(|| unhosted(&target))?;
    let approvals = host.write_approvals();
    let now = Instant::now();
    approvals.retain_live(&live_occurrences(host), now);

    let target_label = label_of(&target_summary);
    let before = reduced_status(host, &target, now_ms);
    if let Some(refusal) = before.as_ref().and_then(delivery_refusal) {
        let message = match refusal {
            DeliveryRefusal::Blocked { reason } => format!(
                "{target_label} is waiting for a human decision ({reason}); nothing was written, \
                 retry once the human has answered in that pane"
            ),
            DeliveryRefusal::LeftForeground { runtime, holder } => format!(
                "{target_label} no longer runs {runtime} in the foreground ({holder} has it); \
                 nothing was written"
            ),
        };
        return Err(ControlError::Params(message));
    }

    let source_label = label_of(&source_summary);
    match approvals.admit(
        &source_occurrence,
        &target_occurrence,
        &source_label,
        &target_label,
        now,
    ) {
        Admission::Approved => {}
        Admission::Pending { id } if approvals.has_window() => {
            return Ok(json!({
                "status": "approval_pending",
                "request": id,
                "message": format!(
                    "a human must allow {source_label} to write into {target_label} in Paneflow; \
                     nothing was written, call write_pane again after the decision"
                ),
            }));
        }
        Admission::Pending { id } => {
            return Ok(json!({
                "status": "no_window",
                "request": id,
                "message": format!(
                    "no Paneflow window is open to approve this write; nothing was written and the \
                     request expires in {} s",
                    APPROVAL_REQUEST_TTL.as_secs()
                ),
            }));
        }
        Admission::Denied => {
            return Err(ControlError::Params(format!(
                "the human denied writes from {source_label} into {target_label}; nothing was written"
            )));
        }
        Admission::Expired => {
            return Err(ControlError::Params(format!(
                "the write request from {source_label} into {target_label} expired unanswered; \
                 nothing was written"
            )));
        }
    }
    if let Err(wait) = approvals.take_token(&source_occurrence, &target_occurrence, now) {
        return Err(ControlError::Params(format!(
            "rate limited: at most one write per second between two agents; retry in {} ms",
            wait.as_millis().max(1)
        )));
    }

    let body = format!(
        "{}\n{}",
        provenance_line(&source_summary),
        sanitize_relayed_text(text)
    );
    let generation = Some(target_summary.manifest.generation);
    let delivered = deliver_text(host, &target, generation, &body, Some(true), submit, true)?;
    log::info!(
        "paneflow-host: pane.write relayed {} bytes from session {source} to session {target}",
        body.len()
    );
    let mut result = json!({
        "status": "written",
        "bytes": body.len(),
        "submitted": submit,
        "submit_mode": delivered.submit_mode,
    });
    if submit {
        confirm_turn_start(
            || reduced_status(host, &target, now_ms),
            before.as_ref(),
            SUBMIT_START_TIMEOUT,
            SUBMIT_START_POLL,
        )
        .annotate(&mut result);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occurrence(pid: u32) -> AgentOccurrence {
        AgentOccurrence {
            session: SessionId::new(),
            generation: SessionGeneration::FIRST,
            pid,
            started_at: Some(7),
        }
    }

    #[test]
    fn a_relayed_text_loses_every_control_but_newline_and_tab() {
        let hostile = "a\x1b[201~b\x1b[200~c\r\nd\te\u{9b}f\x07g\x00h\u{85}i\x7f";
        assert_eq!(sanitize_relayed_text(hostile), "abc\nd\tefghi");
        assert_eq!(sanitize_relayed_text("x\x1b[201~y"), "xy");
    }

    #[test]
    fn a_pair_needs_one_human_allow_and_keeps_it_for_its_occurrences() {
        let approvals = WriteApprovals::default();
        let (source, target) = (occurrence(10), occurrence(20));
        let now = Instant::now();
        let Admission::Pending { id } = approvals.admit(&source, &target, "a", "b", now) else {
            panic!("a first write asks the human");
        };
        assert_eq!(
            approvals.admit(&source, &target, "a", "b", now),
            Admission::Pending { id },
            "a retry joins the pending request instead of opening another"
        );
        approvals.decide(id, Decision::Allow, now).unwrap();
        assert_eq!(
            approvals.admit(&source, &target, "a", "b", now),
            Admission::Approved
        );
        assert!(
            matches!(
                approvals.admit(&target, &source, "b", "a", now),
                Admission::Pending { .. }
            ),
            "an approval is directional"
        );

        let relaunched = AgentOccurrence {
            generation: SessionGeneration::FIRST.next(),
            ..target.clone()
        };
        approvals.retain_live(&HashSet::from([source.clone(), relaunched.clone()]), now);
        assert!(
            matches!(
                approvals.admit(&source, &relaunched, "a", "b", now),
                Admission::Pending { .. }
            ),
            "a new generation of the target asks again"
        );
    }

    #[test]
    fn an_unanswered_request_expires_into_a_refusal_after_120_s() {
        let approvals = WriteApprovals::default();
        let (source, target) = (occurrence(10), occurrence(20));
        let asked = Instant::now();
        let Admission::Pending { id } = approvals.admit(&source, &target, "a", "b", asked) else {
            panic!("pending");
        };
        let later = asked + APPROVAL_REQUEST_TTL;
        assert_eq!(
            approvals.admit(&source, &target, "a", "b", later),
            Admission::Expired
        );
        assert!(approvals.decide(id, Decision::Allow, later).is_err());
        assert!(
            matches!(
                approvals.admit(&source, &target, "a", "b", later),
                Admission::Pending { .. }
            ),
            "the refusal is reported once, then the agent may ask again"
        );
    }

    #[test]
    fn a_denial_is_reported_to_the_next_write() {
        let approvals = WriteApprovals::default();
        let (source, target) = (occurrence(10), occurrence(20));
        let now = Instant::now();
        let Admission::Pending { id } = approvals.admit(&source, &target, "a", "b", now) else {
            panic!("pending");
        };
        approvals.decide(id, Decision::Deny, now).unwrap();
        assert_eq!(
            approvals.admit(&source, &target, "a", "b", now),
            Admission::Denied
        );
    }

    #[test]
    fn a_pair_writes_once_per_second_with_a_burst_of_three() {
        let approvals = WriteApprovals::default();
        let (source, target) = (occurrence(10), occurrence(20));
        let start = Instant::now();
        for _ in 0..WRITE_BURST {
            approvals.take_token(&source, &target, start).unwrap();
        }
        assert!(approvals.take_token(&source, &target, start).is_err());
        approvals
            .take_token(&source, &target, start + WRITE_REFILL_INTERVAL)
            .unwrap();
        assert!(
            approvals
                .take_token(&source, &target, start + WRITE_REFILL_INTERVAL)
                .is_err()
        );
    }

    #[test]
    fn a_watcher_sees_each_request_and_its_decision() {
        let approvals = WriteApprovals::default();
        assert!(!approvals.has_window());
        let watch = approvals.watch();
        assert!(approvals.has_window());
        let now = Instant::now();
        let Admission::Pending { id } =
            approvals.admit(&occurrence(1), &occurrence(2), "conductor", "worker", now)
        else {
            panic!("pending");
        };
        let asked = watch.frames.try_recv().unwrap();
        assert_eq!(asked["pending"][0]["id"], id);
        assert_eq!(asked["pending"][0]["source"], "conductor");
        approvals.decide(id, Decision::Deny, now).unwrap();
        let decided = watch.frames.try_recv().unwrap();
        assert_eq!(decided["pending"], json!([]));
        approvals.unwatch(watch.id);
        assert!(!approvals.has_window());
    }
}
