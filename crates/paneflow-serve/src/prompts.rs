use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_api::Principal;
use paneflow_api::methods::prompt::{
    ERR_NOT_PENDING, EndReason, EndedFrame, FrameKind, MAX_PENDING, MAX_WAIT_MS, NOT_PENDING,
    PromptAccess, PromptAnswer, PromptAsker, PromptKind, PromptOrigin, PromptOutcome, PromptSpec,
    PromptStatus, Replied, ReplyParams, RequestParams, RequestedFrame, check_spec,
    normalize_answer, prompt_access,
};
use serde_json::{Value, json};

use crate::handles::{fresh_secret, same_secret};
use crate::registry::ServeError;

pub const ASKER_GRACE_MS: u64 = 15_000;
pub const ENDED_RETENTION_MS: u64 = 120_000;
pub const MAX_ENDED: usize = 256;
pub const WORKER_STOPPED: &str = "the Paneflow worker stopped before the prompt was answered";
pub const ASKER_GONE: &str = "the asker stopped waiting for the answer";

struct Open {
    spec: PromptSpec,
    origin: Option<PromptOrigin>,
    asker: PromptAsker,
    tokens: BTreeMap<u64, String>,
    requested_at_ms: u64,
    expires_at_ms: u64,
    waiting: usize,
    idle_since_ms: u64,
}

impl Open {
    fn asked_by(&self) -> Option<String> {
        self.origin
            .as_ref()
            .and_then(|origin| origin.session.clone())
    }

    fn took(&self, offered: Option<&str>) -> bool {
        offered.is_some_and(|offered| {
            self.tokens
                .values()
                .any(|token| same_secret(token, offered))
        })
    }
}

struct Ended {
    outcome: PromptOutcome,
    asked_by: Option<String>,
    at_ms: u64,
}

struct Follower {
    kinds: Vec<PromptKind>,
    principal: Principal,
    frames: Sender<Value>,
}

impl Follower {
    fn shows(&self, kind: PromptKind, asker: PromptAsker) -> bool {
        self.kinds.contains(&kind) && prompt_access(&self.principal, asker).shows()
    }

    fn shows_open(&self, open: &Open) -> bool {
        self.shows(open.spec.kind, open.asker)
    }
}

pub struct FollowerFeed {
    pub id: u64,
    pub pending: Vec<Value>,
    pub frames: Receiver<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Open,
    Ended(PromptOutcome),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    Invalid(String),
    NotPending(u64),
}

impl From<ReplyError> for ServeError {
    fn from(error: ReplyError) -> Self {
        match error {
            ReplyError::Invalid(message) => ServeError::invalid_params(message),
            ReplyError::NotPending(id) => ServeError {
                code: ERR_NOT_PENDING,
                message: format!("{NOT_PENDING}: prompt {id} is no longer pending"),
                data: Some(json!({"reason": NOT_PENDING, "id": id})),
            },
        }
    }
}

#[derive(Default)]
pub struct BrokerState {
    next_id: u64,
    open: BTreeMap<u64, Open>,
    ended: BTreeMap<u64, Ended>,
    followers: BTreeMap<u64, Follower>,
    next_follower: u64,
    closed: bool,
}

fn requested_frame(id: u64, open: &Open, token: &str, now_ms: u64) -> Value {
    serde_json::to_value(RequestedFrame {
        kind: FrameKind::Requested,
        id,
        spec: open.spec.clone(),
        origin: open.origin.clone(),
        requested_at_ms: open.requested_at_ms,
        remaining_ms: open.expires_at_ms.saturating_sub(now_ms),
        token: Some(token.to_string()),
    })
    .unwrap_or(Value::Null)
}

fn ended_frame(id: u64, reason: EndReason) -> Value {
    serde_json::to_value(EndedFrame {
        kind: FrameKind::Ended,
        id,
        reason,
    })
    .unwrap_or(Value::Null)
}

impl BrokerState {
    pub fn pending(&self) -> usize {
        self.open.len()
    }

    pub fn followers(&self) -> usize {
        self.followers.len()
    }

    pub fn request(
        &mut self,
        params: &RequestParams,
        now_ms: u64,
    ) -> Result<PromptOutcome, String> {
        let spec = params.spec();
        check_spec(&spec)?;
        if params.wait_ms.is_some_and(|wait| wait > MAX_WAIT_MS) {
            return Err(format!("'wait_ms' is at most {MAX_WAIT_MS}"));
        }
        if self.closed {
            return Ok(PromptOutcome::failed(None, WORKER_STOPPED));
        }
        let asker = PromptAsker::of(params.origin.as_ref());
        let able: Vec<u64> = self
            .followers
            .iter()
            .filter(|(_, follower)| follower.shows(spec.kind, asker))
            .map(|(id, _)| *id)
            .collect();
        if able.is_empty() {
            let reachable = self
                .followers
                .values()
                .any(|follower| prompt_access(&follower.principal, asker).shows());
            let status = if reachable {
                PromptStatus::Unsupported
            } else {
                PromptStatus::NoClient
            };
            return Ok(PromptOutcome::ended(status, None));
        }
        if self.open.len() >= MAX_PENDING {
            return Ok(PromptOutcome::ended(PromptStatus::Busy, None));
        }
        let mut tokens = BTreeMap::new();
        for follower in &able {
            match fresh_secret() {
                Ok(token) => {
                    tokens.insert(*follower, token);
                }
                Err(detail) => return Ok(PromptOutcome::failed(None, detail)),
            }
        }
        self.next_id += 1;
        let id = self.next_id;
        let open = Open {
            expires_at_ms: now_ms.saturating_add(spec.timeout_ms()),
            spec,
            origin: params.origin.clone(),
            asker,
            tokens,
            requested_at_ms: now_ms,
            waiting: 0,
            idle_since_ms: now_ms,
        };
        for (follower, token) in &open.tokens {
            if let Some(follower) = self.followers.get(follower) {
                let _ = follower
                    .frames
                    .send(requested_frame(id, &open, token, now_ms));
            }
        }
        self.open.insert(id, open);
        Ok(PromptOutcome::ended(PromptStatus::Pending, Some(id)))
    }

    pub fn lookup(&mut self, id: u64) -> Lookup {
        if self.open.contains_key(&id) {
            return Lookup::Open;
        }
        match self.ended.remove(&id) {
            Some(ended) => Lookup::Ended(ended.outcome),
            None => Lookup::Unknown,
        }
    }

    pub fn begin_wait(&mut self, id: u64) {
        if let Some(open) = self.open.get_mut(&id) {
            open.waiting += 1;
        }
    }

    pub fn end_wait(&mut self, id: u64, now_ms: u64) {
        if let Some(open) = self.open.get_mut(&id) {
            open.waiting = open.waiting.saturating_sub(1);
            open.idle_since_ms = now_ms;
        }
    }

    fn end(&mut self, id: u64, reason: EndReason, outcome: PromptOutcome, now_ms: u64) {
        let Some(open) = self.open.remove(&id) else {
            return;
        };
        let frame = ended_frame(id, reason);
        for follower in open
            .tokens
            .keys()
            .filter_map(|follower| self.followers.get(follower))
        {
            let _ = follower.frames.send(frame.clone());
        }
        self.ended.insert(
            id,
            Ended {
                outcome,
                asked_by: open.asked_by(),
                at_ms: now_ms,
            },
        );
        while self.ended.len() > MAX_ENDED {
            self.ended.pop_first();
        }
    }

    pub fn reply(
        &mut self,
        params: ReplyParams,
        principal: &Principal,
        now_ms: u64,
    ) -> Result<Replied, ReplyError> {
        let cancel = params.cancel.unwrap_or(false);
        let answer = match (params.answer, cancel) {
            (Some(_), true) => {
                return Err(ReplyError::Invalid(
                    "'cancel' cannot be combined with 'answer'".to_string(),
                ));
            }
            (None, false) => {
                return Err(ReplyError::Invalid(
                    "missing 'answer' parameter; send cancel = true to dismiss the prompt"
                        .to_string(),
                ));
            }
            (answer, _) => answer,
        };
        let Some(open) = self.open.get(&params.id) else {
            return Err(ReplyError::NotPending(params.id));
        };
        let admitted = match prompt_access(principal, open.asker) {
            PromptAccess::Answers => true,
            PromptAccess::AnswersWithItsToken => open.took(params.token.as_deref()),
            PromptAccess::Hidden => false,
        };
        if !admitted {
            return Err(ReplyError::NotPending(params.id));
        }
        let (reason, outcome) = match answer {
            None => (
                EndReason::Cancelled,
                PromptOutcome::ended(PromptStatus::Cancelled, Some(params.id)),
            ),
            Some(answer) => {
                let answer: PromptAnswer =
                    normalize_answer(&open.spec, answer).map_err(ReplyError::Invalid)?;
                (
                    EndReason::Answered,
                    PromptOutcome {
                        status: PromptStatus::Answered,
                        id: Some(params.id),
                        answer: Some(answer),
                        detail: None,
                    },
                )
            }
        };
        self.end(params.id, reason, outcome, now_ms);
        Ok(Replied {
            id: params.id,
            reason,
        })
    }

    pub fn withdraw(&mut self, id: u64, now_ms: u64) -> bool {
        if !self.open.contains_key(&id) {
            return false;
        }
        self.end(
            id,
            EndReason::Cancelled,
            PromptOutcome::ended(PromptStatus::Cancelled, Some(id)),
            now_ms,
        );
        true
    }

    pub fn follow(
        &mut self,
        kinds: Vec<PromptKind>,
        principal: Principal,
        now_ms: u64,
    ) -> FollowerFeed {
        let (sender, frames) = channel();
        self.next_follower += 1;
        let id = self.next_follower;
        let follower = Follower {
            kinds,
            principal,
            frames: sender,
        };
        let mut pending = Vec::new();
        if !self.closed {
            for (prompt, open) in &mut self.open {
                if !follower.shows_open(open) {
                    continue;
                }
                let Ok(token) = fresh_secret() else {
                    continue;
                };
                pending.push(requested_frame(*prompt, open, &token, now_ms));
                open.tokens.insert(id, token);
            }
            self.followers.insert(id, follower);
        }
        FollowerFeed {
            id,
            pending,
            frames,
        }
    }

    pub fn unfollow(&mut self, follower: u64, now_ms: u64) {
        if self.followers.remove(&follower).is_none() {
            return;
        }
        for open in self.open.values_mut() {
            open.tokens.remove(&follower);
        }
        let orphaned: Vec<u64> = self
            .open
            .iter()
            .filter(|(_, open)| {
                !self
                    .followers
                    .values()
                    .any(|follower| follower.shows_open(open))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in orphaned {
            self.end(
                id,
                EndReason::NoClient,
                PromptOutcome::ended(PromptStatus::NoClient, Some(id)),
                now_ms,
            );
        }
    }

    pub fn sweep(&mut self, now_ms: u64) -> bool {
        let expired: Vec<(u64, EndReason, PromptOutcome)> = self
            .open
            .iter()
            .filter_map(|(id, open)| {
                if now_ms >= open.expires_at_ms {
                    Some((
                        *id,
                        EndReason::Timeout,
                        PromptOutcome::ended(PromptStatus::Timeout, Some(*id)),
                    ))
                } else if open.waiting == 0
                    && now_ms.saturating_sub(open.idle_since_ms) >= ASKER_GRACE_MS
                {
                    Some((
                        *id,
                        EndReason::Failed,
                        PromptOutcome::failed(Some(*id), ASKER_GONE),
                    ))
                } else {
                    None
                }
            })
            .collect();
        let changed = !expired.is_empty();
        for (id, reason, outcome) in expired {
            self.end(id, reason, outcome, now_ms);
        }
        self.ended
            .retain(|_, ended| now_ms.saturating_sub(ended.at_ms) < ENDED_RETENTION_MS);
        changed
    }

    pub fn asker_of(&self, id: u64) -> Option<Option<String>> {
        self.open
            .get(&id)
            .map(Open::asked_by)
            .or_else(|| self.ended.get(&id).map(|ended| ended.asked_by.clone()))
    }

    pub fn cancel_asked_by(&mut self, session: &str, now_ms: u64) -> bool {
        let asked: Vec<u64> = self
            .open
            .iter()
            .filter(|(_, open)| open.asked_by().as_deref() == Some(session))
            .map(|(id, _)| *id)
            .collect();
        for id in &asked {
            self.end(
                *id,
                EndReason::Cancelled,
                PromptOutcome::ended(PromptStatus::Cancelled, Some(*id)),
                now_ms,
            );
        }
        !asked.is_empty()
    }

    pub fn next_deadline_ms(&self) -> Option<u64> {
        self.open
            .values()
            .map(|open| {
                if open.waiting == 0 {
                    open.expires_at_ms
                        .min(open.idle_since_ms.saturating_add(ASKER_GRACE_MS))
                } else {
                    open.expires_at_ms
                }
            })
            .min()
    }

    pub fn close(&mut self, now_ms: u64) {
        self.closed = true;
        let ids: Vec<u64> = self.open.keys().copied().collect();
        for id in ids {
            self.end(
                id,
                EndReason::Failed,
                PromptOutcome::failed(Some(id), WORKER_STOPPED),
                now_ms,
            );
        }
        self.followers.clear();
    }
}

#[derive(Default)]
pub struct PromptBroker {
    state: Mutex<BrokerState>,
    changed: Condvar,
}

impl PromptBroker {
    fn lock(&self) -> MutexGuard<'_, BrokerState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now() -> u64 {
        crate::state::now_ms()
    }

    pub fn request(&self, params: &RequestParams) -> Result<PromptOutcome, ServeError> {
        let outcome = self
            .lock()
            .request(params, Self::now())
            .map_err(ServeError::invalid_params)?;
        match (outcome.status, outcome.id) {
            (PromptStatus::Pending, Some(id)) => Ok(self.wait_for(id, params.wait_ms.unwrap_or(0))),
            _ => Ok(outcome),
        }
    }

    pub fn wait(&self, id: u64, wait_ms: Option<u64>) -> Result<PromptOutcome, ServeError> {
        let wait_ms = wait_ms.unwrap_or(0);
        if wait_ms > MAX_WAIT_MS {
            return Err(ServeError::invalid_params(format!(
                "'wait_ms' is at most {MAX_WAIT_MS}"
            )));
        }
        let mut state = self.lock();
        state.sweep(Self::now());
        match state.lookup(id) {
            Lookup::Unknown => Err(unknown_prompt(id)),
            Lookup::Ended(outcome) => Ok(outcome),
            Lookup::Open => {
                drop(state);
                Ok(self.wait_for(id, wait_ms))
            }
        }
    }

    fn wait_for(&self, id: u64, wait_ms: u64) -> PromptOutcome {
        let deadline = Instant::now() + Duration::from_millis(wait_ms);
        let mut state = self.lock();
        state.begin_wait(id);
        loop {
            let now = Self::now();
            if state.sweep(now) {
                self.changed.notify_all();
            }
            match state.lookup(id) {
                Lookup::Ended(outcome) => return outcome,
                Lookup::Unknown => {
                    return PromptOutcome::failed(Some(id), "the prompt's outcome was lost");
                }
                Lookup::Open => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                state.end_wait(id, now);
                return PromptOutcome::ended(PromptStatus::Pending, Some(id));
            }
            let until_sweep = state.next_deadline_ms().map_or(left, |at| {
                Duration::from_millis(at.saturating_sub(now).max(1))
            });
            state = self
                .changed
                .wait_timeout(state, left.min(until_sweep))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    pub fn reply(&self, params: ReplyParams, principal: &Principal) -> Result<Replied, ServeError> {
        let replied = self.lock().reply(params, principal, Self::now())?;
        self.changed.notify_all();
        Ok(replied)
    }

    pub fn withdraw(&self, id: u64) {
        if self.lock().withdraw(id, Self::now()) {
            self.changed.notify_all();
        }
    }

    pub fn follow(&self, kinds: Vec<PromptKind>, principal: Principal) -> FollowerFeed {
        self.lock().follow(kinds, principal, Self::now())
    }

    pub fn unfollow(&self, follower: u64) {
        self.lock().unfollow(follower, Self::now());
        self.changed.notify_all();
    }

    pub fn asker_of(&self, id: u64) -> Option<Option<String>> {
        self.lock().asker_of(id)
    }

    pub fn cancel_asked_by(&self, session: &str) {
        if self.lock().cancel_asked_by(session, Self::now()) {
            self.changed.notify_all();
        }
    }

    pub fn sweep(&self) {
        if self.lock().sweep(Self::now()) {
            self.changed.notify_all();
        }
    }

    pub fn close(&self) {
        self.lock().close(Self::now());
        self.changed.notify_all();
    }
}

pub fn unknown_prompt(id: u64) -> ServeError {
    ServeError::invalid_params(format!(
        "no prompt with id {id}; its outcome was already returned or it never existed"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::methods::prompt::OriginKind;

    fn input(title: &str) -> RequestParams {
        serde_json::from_value(json!({"kind": "input", "title": title})).unwrap()
    }

    fn pick(ids: &[&str]) -> RequestParams {
        serde_json::from_value(json!({"kind": "pick", "title": "Choose", "items": ids})).unwrap()
    }

    fn frames(feed: &FollowerFeed) -> Vec<Value> {
        feed.frames.try_iter().collect()
    }

    fn pending_id(outcome: &PromptOutcome) -> u64 {
        assert_eq!(outcome.status, PromptStatus::Pending, "{outcome:?}");
        outcome.id.unwrap()
    }

    fn answer(id: u64, text: &str) -> ReplyParams {
        ReplyParams {
            id,
            answer: Some(PromptAnswer::Text(text.into())),
            cancel: None,
            token: None,
        }
    }

    #[test]
    fn with_no_follower_a_prompt_is_no_client_and_with_none_able_to_show_it_unsupported() {
        let mut state = BrokerState::default();
        let outcome = state.request(&input("Name"), 0).unwrap();
        assert_eq!(outcome, PromptOutcome::ended(PromptStatus::NoClient, None));
        let _feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let outcome = state.request(&pick(&["a"]), 0).unwrap();
        assert_eq!(
            outcome,
            PromptOutcome::ended(PromptStatus::Unsupported, None)
        );
        assert_eq!(state.pending(), 0, "nothing is queued");
    }

    #[test]
    fn the_first_reply_wins_every_follower_hears_ended_and_a_second_is_not_pending() {
        let mut state = BrokerState::default();
        let first = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let second = state.follow(
            vec![PromptKind::Input, PromptKind::Pick],
            Principal::External,
            0,
        );
        let id = pending_id(&state.request(&input("Name"), 10).unwrap());
        let requested = frames(&first);
        assert_eq!(requested[0]["type"], "requested");
        assert_eq!(requested[0]["id"], id);
        assert_eq!(requested[0]["spec"]["title"], "Name");
        assert_eq!(requested[0]["remaining_ms"], 300_000);
        assert_eq!(frames(&second).len(), 1);
        assert_eq!(
            state.reply(answer(id, "  a b "), &Principal::External, 20),
            Ok(Replied {
                id,
                reason: EndReason::Answered
            })
        );
        for feed in [&first, &second] {
            assert_eq!(
                frames(feed),
                vec![json!({"type": "ended", "id": id, "reason": "answered"})]
            );
        }
        assert_eq!(
            state.reply(answer(id, "late"), &Principal::External, 30),
            Err(ReplyError::NotPending(id))
        );
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome {
                status: PromptStatus::Answered,
                id: Some(id),
                answer: Some(PromptAnswer::Text("  a b ".into())),
                detail: None,
            })
        );
        assert_eq!(
            state.lookup(id),
            Lookup::Unknown,
            "the outcome is taken once"
        );
    }

    #[test]
    fn a_cancel_reaches_the_asker_and_closes_the_prompt_on_the_followers() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let id = pending_id(&state.request(&input("Name"), 0).unwrap());
        state
            .reply(
                ReplyParams {
                    id,
                    answer: None,
                    cancel: Some(true),
                    token: None,
                },
                &Principal::External,
                1,
            )
            .unwrap();
        assert_eq!(frames(&feed)[1]["reason"], "cancelled");
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome::ended(PromptStatus::Cancelled, Some(id)))
        );
    }

    #[test]
    fn the_timeout_defaults_to_300_seconds_and_ends_the_prompt_for_everyone() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let id = pending_id(&state.request(&input("Name"), 1_000).unwrap());
        state.begin_wait(id);
        assert!(!state.sweep(300_999));
        assert!(state.sweep(301_000));
        assert_eq!(frames(&feed)[1]["reason"], "timeout");
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome::ended(PromptStatus::Timeout, Some(id)))
        );
        let mut short = input("Quick");
        short.timeout = Some(2.0);
        let id = pending_id(&state.request(&short, 0).unwrap());
        state.begin_wait(id);
        assert!(state.sweep(2_000));
    }

    #[test]
    fn the_seventeenth_pending_prompt_is_busy() {
        let mut state = BrokerState::default();
        let _feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        for _ in 0..MAX_PENDING {
            pending_id(&state.request(&input("Name"), 0).unwrap());
        }
        assert_eq!(
            state.request(&input("One more"), 0).unwrap(),
            PromptOutcome::ended(PromptStatus::Busy, None)
        );
    }

    #[test]
    fn a_follower_leaving_keeps_the_prompt_while_another_can_show_it_and_the_last_ends_it() {
        let mut state = BrokerState::default();
        let first = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let second = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let id = pending_id(&state.request(&input("Name"), 0).unwrap());
        state.unfollow(first.id, 1);
        assert_eq!(state.pending(), 1);
        state.unfollow(second.id, 2);
        assert_eq!(state.pending(), 0);
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome::ended(PromptStatus::NoClient, Some(id)))
        );
    }

    #[test]
    fn a_late_follower_receives_the_pending_prompts_oldest_first() {
        let mut state = BrokerState::default();
        let _early = state.follow(
            vec![PromptKind::Input, PromptKind::Pick],
            Principal::External,
            0,
        );
        let ids: Vec<u64> = [input("a"), pick(&["x"]), input("c")]
            .iter()
            .map(|params| pending_id(&state.request(params, 0).unwrap()))
            .collect();
        let late = state.follow(
            vec![PromptKind::Input, PromptKind::Pick],
            Principal::External,
            5_000,
        );
        let listed: Vec<u64> = late
            .pending
            .iter()
            .map(|frame| frame["id"].as_u64().unwrap())
            .collect();
        assert_eq!(listed, ids);
        assert_eq!(late.pending[0]["remaining_ms"], 295_000);
        let inputs_only = state.follow(vec![PromptKind::Input], Principal::External, 0);
        assert_eq!(inputs_only.pending.len(), 2);
    }

    #[test]
    fn a_reply_must_match_the_prompt_kind_and_items() {
        let mut state = BrokerState::default();
        let _feed = state.follow(vec![PromptKind::Pick], Principal::External, 0);
        let id = pending_id(&state.request(&pick(&["a", "b"]), 0).unwrap());
        assert!(matches!(
            state.reply(answer(id, "z"), &Principal::External, 1),
            Err(ReplyError::Invalid(_))
        ));
        assert_eq!(
            state.pending(),
            1,
            "a refused answer leaves the prompt open"
        );
        assert!(matches!(
            state.reply(
                ReplyParams {
                    id,
                    answer: None,
                    cancel: None,
                    token: None,
                },
                &Principal::External,
                1
            ),
            Err(ReplyError::Invalid(_))
        ));
        state
            .reply(answer(id, "b"), &Principal::External, 1)
            .unwrap();
    }

    #[test]
    fn a_shutdown_fails_every_pending_prompt_saying_the_worker_stopped() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let id = pending_id(&state.request(&input("Name"), 0).unwrap());
        state.close(1);
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome::failed(Some(id), WORKER_STOPPED))
        );
        assert_eq!(frames(&feed)[1]["reason"], "failed");
        assert!(
            feed.frames.recv().is_err(),
            "the follower's stream is dropped"
        );
        assert_eq!(
            state.request(&input("After"), 2).unwrap().status,
            PromptStatus::Failed
        );
    }

    #[test]
    fn an_asker_that_stopped_waiting_abandons_its_prompt_after_the_grace() {
        let mut state = BrokerState::default();
        let _feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let id = pending_id(&state.request(&input("Name"), 0).unwrap());
        state.begin_wait(id);
        assert!(!state.sweep(ASKER_GRACE_MS * 4), "a waiting asker keeps it");
        state.end_wait(id, ASKER_GRACE_MS * 4);
        assert!(!state.sweep(ASKER_GRACE_MS * 5 - 1));
        assert!(state.sweep(ASKER_GRACE_MS * 5));
        assert_eq!(
            state.lookup(id),
            Lookup::Ended(PromptOutcome::failed(Some(id), ASKER_GONE))
        );
    }

    #[test]
    fn a_bad_pick_is_refused_naming_the_field_and_queues_nothing() {
        let mut state = BrokerState::default();
        let _feed = state.follow(vec![PromptKind::Pick], Principal::External, 0);
        let mut both = pick(&["a"]);
        both.multiple = Some(true);
        both.allow_other = Some(true);
        assert!(
            state
                .request(&both, 0)
                .unwrap_err()
                .contains("'allow_other'")
        );
        let repeated = pick(&["a", "a"]);
        assert!(
            state
                .request(&repeated, 0)
                .unwrap_err()
                .contains("'items[1].id'")
        );
        let mut slow = pick(&["a"]);
        slow.wait_ms = Some(MAX_WAIT_MS + 1);
        assert!(state.request(&slow, 0).unwrap_err().contains("'wait_ms'"));
        assert_eq!(state.pending(), 0);
    }

    #[test]
    fn the_origin_travels_with_the_requested_frame() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let mut params = input("Name");
        params.origin = Some(PromptOrigin {
            kind: OriginKind::Action,
            title: Some("New branch".into()),
            name: Some("new_branch".into()),
            file: None,
            agent: None,
            surface_id: Some(7),
            workspace_id: None,
            session: None,
        });
        state.request(&params, 0).unwrap();
        let frame = &frames(&feed)[0];
        assert_eq!(frame["origin"]["title"], "New branch");
        assert_eq!(frame["origin"]["surface_id"], 7);
    }

    fn asked_by(session: &str) -> RequestParams {
        let mut params = input("Deploy?");
        params.origin = Some(PromptOrigin {
            kind: OriginKind::Agent,
            title: None,
            name: None,
            file: None,
            agent: Some("Claude Code".into()),
            surface_id: None,
            workspace_id: None,
            session: Some(session.into()),
        });
        params
    }

    #[test]
    fn the_end_of_an_asking_session_cancels_only_its_own_prompts() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let mine = pending_id(&state.request(&asked_by("s1"), 0).unwrap());
        let other = pending_id(&state.request(&asked_by("s2"), 0).unwrap());
        let unowned = pending_id(&state.request(&input("Name"), 0).unwrap());
        assert_eq!(state.asker_of(mine), Some(Some("s1".into())));
        assert_eq!(state.asker_of(unowned), Some(None));
        assert!(state.cancel_asked_by("s1", 5));
        assert!(!state.cancel_asked_by("s1", 6), "nothing left to cancel");
        assert_eq!(
            state.lookup(mine),
            Lookup::Ended(PromptOutcome::ended(PromptStatus::Cancelled, Some(mine)))
        );
        assert_eq!(state.lookup(other), Lookup::Open);
        assert_eq!(state.lookup(unowned), Lookup::Open);
        let ended: Vec<Value> = frames(&feed)
            .into_iter()
            .filter(|frame| frame["type"] == "ended")
            .collect();
        assert_eq!(
            ended,
            vec![json!({"type": "ended", "id": mine, "reason": "cancelled"})]
        );
    }

    #[test]
    fn the_blocking_broker_returns_pending_then_the_answer_through_wait() {
        let broker = std::sync::Arc::new(PromptBroker::default());
        let feed = broker.follow(vec![PromptKind::Input], Principal::External);
        let outcome = broker.request(&input("Name")).unwrap();
        let id = pending_id(&outcome);
        let replier = std::sync::Arc::clone(&broker);
        let answering = std::thread::spawn(move || {
            let frame = feed.frames.recv().unwrap();
            let id = frame["id"].as_u64().unwrap();
            replier
                .reply(answer(id, "done"), &Principal::External)
                .unwrap();
        });
        let outcome = broker.wait(id, Some(MAX_WAIT_MS)).unwrap();
        answering.join().unwrap();
        assert_eq!(outcome.status, PromptStatus::Answered);
        assert_eq!(outcome.answer, Some(PromptAnswer::Text("done".into())));
        assert!(broker.wait(id, None).is_err(), "the outcome was taken");
    }

    fn pane(session: &str) -> Principal {
        Principal::Pane {
            session: session.into(),
            workspace: None,
        }
    }

    fn run_by_agent() -> RequestParams {
        let mut params = input("Branch name");
        params.origin = Some(PromptOrigin {
            kind: OriginKind::Action,
            title: Some("New branch".into()),
            name: Some("new_branch".into()),
            file: None,
            agent: Some("Claude Code".into()),
            surface_id: Some(4),
            workspace_id: None,
            session: Some("s1".into()),
        });
        params
    }

    fn token_of(frame: &Value) -> String {
        frame["token"].as_str().expect("a token").to_string()
    }

    fn with_token(id: u64, text: &str, token: Option<&str>) -> ReplyParams {
        ReplyParams {
            token: token.map(str::to_string),
            ..answer(id, text)
        }
    }

    #[test]
    fn each_follower_receives_its_own_token_for_each_prompt() {
        let mut state = BrokerState::default();
        let first = state.follow(vec![PromptKind::Input], Principal::Window, 0);
        let second = state.follow(vec![PromptKind::Input], Principal::External, 0);
        state.request(&input("a"), 0).unwrap();
        state.request(&asked_by("s1"), 0).unwrap();
        let mut tokens: Vec<String> = frames(&first)
            .iter()
            .chain(frames(&second).iter())
            .map(token_of)
            .collect();
        assert_eq!(tokens.len(), 4);
        assert!(
            tokens
                .iter()
                .all(|token| token.len() == crate::handles::SECRET_HEX_LEN)
        );
        tokens.sort();
        tokens.dedup();
        assert_eq!(tokens.len(), 4, "distinct per prompt and per follower");
        let late = state.follow(vec![PromptKind::Input], Principal::External, 0);
        let late_tokens: Vec<String> = late.pending.iter().map(token_of).collect();
        assert_eq!(late_tokens.len(), 2);
        assert!(late_tokens.iter().all(|token| !tokens.contains(token)));
    }

    #[test]
    fn an_agent_prompt_refuses_a_reply_without_its_token_as_if_it_were_not_pending() {
        for params in [asked_by("s1"), run_by_agent()] {
            let mut state = BrokerState::default();
            let window = state.follow(vec![PromptKind::Input], Principal::External, 0);
            let id = pending_id(&state.request(&params, 0).unwrap());
            let token = token_of(&frames(&window)[0]);
            let unknown = state.reply(with_token(id + 100, "x", None), &Principal::External, 1);
            assert_eq!(unknown, Err(ReplyError::NotPending(id + 100)));
            for offered in [None, Some("0".repeat(32)), Some(token[..31].to_string())] {
                assert_eq!(
                    state.reply(
                        with_token(id, "yes", offered.as_deref()),
                        &Principal::External,
                        1
                    ),
                    Err(ReplyError::NotPending(id))
                );
            }
            let cancel_without = ReplyParams {
                id,
                answer: None,
                cancel: Some(true),
                token: None,
            };
            assert_eq!(
                state.reply(cancel_without, &Principal::Window, 1),
                Err(ReplyError::NotPending(id))
            );
            assert_eq!(state.lookup(id), Lookup::Open, "it stays pending");
            assert_eq!(
                state.reply(with_token(id, "yes", Some(&token)), &Principal::External, 2),
                Ok(Replied {
                    id,
                    reason: EndReason::Answered
                })
            );
        }
    }

    #[test]
    fn a_pane_never_sees_an_agent_prompt_and_cannot_answer_it_with_any_token() {
        let mut state = BrokerState::default();
        let in_pane = state.follow(vec![PromptKind::Input], pane("s2"), 0);
        assert_eq!(
            state.request(&asked_by("s1"), 0).unwrap(),
            PromptOutcome::ended(PromptStatus::NoClient, None),
            "a pane alone cannot show an agent's prompt"
        );
        let window = state.follow(vec![PromptKind::Input], Principal::Window, 0);
        let agent = pending_id(&state.request(&asked_by("s1"), 0).unwrap());
        let by_agent_run = pending_id(&state.request(&run_by_agent(), 0).unwrap());
        assert!(frames(&in_pane).is_empty(), "no frame and no token");
        let late_pane = state.follow(vec![PromptKind::Input], pane("s1"), 0);
        assert!(late_pane.pending.is_empty());
        let shown = frames(&window);
        for (id, frame) in [(agent, &shown[0]), (by_agent_run, &shown[1])] {
            let token = token_of(frame);
            for caller in [pane("s1"), pane("s2")] {
                assert_eq!(
                    state.reply(with_token(id, "yes", Some(&token)), &caller, 1),
                    Err(ReplyError::NotPending(id))
                );
            }
            assert_eq!(state.lookup(id), Lookup::Open);
        }
        state.unfollow(window.id, 2);
        assert_eq!(
            state.lookup(agent),
            Lookup::Ended(PromptOutcome::ended(PromptStatus::NoClient, Some(agent))),
            "the pane follower left cannot keep an agent prompt alive"
        );
    }

    #[test]
    fn a_pane_still_answers_a_prompt_from_a_script_or_a_user_action_without_a_token() {
        let mut state = BrokerState::default();
        let in_pane = state.follow(vec![PromptKind::Input], pane("s2"), 0);
        let mut by_user = input("Branch");
        by_user.origin = Some(PromptOrigin {
            agent: None,
            ..run_by_agent().origin.unwrap()
        });
        for params in [input("Name"), by_user] {
            let id = pending_id(&state.request(&params, 0).unwrap());
            assert_eq!(frames(&in_pane).len(), 1);
            assert_eq!(
                state.reply(answer(id, "done"), &pane("s2"), 1),
                Ok(Replied {
                    id,
                    reason: EndReason::Answered
                })
            );
            frames(&in_pane);
        }
    }

    #[test]
    fn an_ended_prompt_still_names_its_asker_until_its_outcome_is_taken() {
        let mut state = BrokerState::default();
        let _window = state.follow(vec![PromptKind::Input], Principal::Window, 0);
        let id = pending_id(&state.request(&asked_by("s1"), 0).unwrap());
        assert!(state.withdraw(id, 1));
        assert_eq!(state.asker_of(id), Some(Some("s1".into())));
        assert!(matches!(state.lookup(id), Lookup::Ended(_)));
        assert_eq!(state.asker_of(id), None);
    }
}
