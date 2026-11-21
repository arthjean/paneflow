use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use paneflow_api::methods::prompt::{
    ERR_NOT_PENDING, EndReason, EndedFrame, FrameKind, MAX_PENDING, MAX_WAIT_MS, NOT_PENDING,
    PromptAnswer, PromptKind, PromptOrigin, PromptOutcome, PromptSpec, PromptStatus, Replied,
    ReplyParams, RequestParams, RequestedFrame, check_spec, normalize_answer,
};
use serde_json::{Value, json};

use crate::registry::ServeError;

pub const ASKER_GRACE_MS: u64 = 15_000;
pub const ENDED_RETENTION_MS: u64 = 120_000;
pub const MAX_ENDED: usize = 256;
pub const WORKER_STOPPED: &str = "the Paneflow worker stopped before the prompt was answered";
pub const ASKER_GONE: &str = "the asker stopped waiting for the answer";

struct Open {
    spec: PromptSpec,
    origin: Option<PromptOrigin>,
    requested_at_ms: u64,
    expires_at_ms: u64,
    waiting: usize,
    idle_since_ms: u64,
}

struct Ended {
    outcome: PromptOutcome,
    at_ms: u64,
}

struct Follower {
    kinds: Vec<PromptKind>,
    frames: Sender<Value>,
}

impl Follower {
    fn shows(&self, kind: PromptKind) -> bool {
        self.kinds.contains(&kind)
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

fn requested_frame(id: u64, open: &Open, now_ms: u64) -> Value {
    serde_json::to_value(RequestedFrame {
        kind: FrameKind::Requested,
        id,
        spec: open.spec.clone(),
        origin: open.origin.clone(),
        requested_at_ms: open.requested_at_ms,
        remaining_ms: open.expires_at_ms.saturating_sub(now_ms),
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
        if self.followers.is_empty() {
            return Ok(PromptOutcome::ended(PromptStatus::NoClient, None));
        }
        if !self
            .followers
            .values()
            .any(|follower| follower.shows(spec.kind))
        {
            return Ok(PromptOutcome::ended(PromptStatus::Unsupported, None));
        }
        if self.open.len() >= MAX_PENDING {
            return Ok(PromptOutcome::ended(PromptStatus::Busy, None));
        }
        self.next_id += 1;
        let id = self.next_id;
        let open = Open {
            expires_at_ms: now_ms.saturating_add(spec.timeout_ms()),
            spec,
            origin: params.origin.clone(),
            requested_at_ms: now_ms,
            waiting: 0,
            idle_since_ms: now_ms,
        };
        let frame = requested_frame(id, &open, now_ms);
        for follower in self.followers.values() {
            if follower.shows(open.spec.kind) {
                let _ = follower.frames.send(frame.clone());
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
        for follower in self.followers.values() {
            if follower.shows(open.spec.kind) {
                let _ = follower.frames.send(frame.clone());
            }
        }
        self.ended.insert(
            id,
            Ended {
                outcome,
                at_ms: now_ms,
            },
        );
        while self.ended.len() > MAX_ENDED {
            self.ended.pop_first();
        }
    }

    pub fn reply(&mut self, params: ReplyParams, now_ms: u64) -> Result<Replied, ReplyError> {
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

    pub fn follow(&mut self, kinds: Vec<PromptKind>, now_ms: u64) -> FollowerFeed {
        let (sender, frames) = channel();
        self.next_follower += 1;
        let id = self.next_follower;
        let pending = self
            .open
            .iter()
            .filter(|(_, open)| kinds.contains(&open.spec.kind))
            .map(|(prompt, open)| requested_frame(*prompt, open, now_ms))
            .collect();
        if !self.closed {
            self.followers.insert(
                id,
                Follower {
                    kinds,
                    frames: sender,
                },
            );
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
        let orphaned: Vec<u64> = self
            .open
            .iter()
            .filter(|(_, open)| {
                !self
                    .followers
                    .values()
                    .any(|follower| follower.shows(open.spec.kind))
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
            Lookup::Unknown => Err(ServeError::invalid_params(format!(
                "no prompt with id {id}; its outcome was already returned or it never existed"
            ))),
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

    pub fn reply(&self, params: ReplyParams) -> Result<Replied, ServeError> {
        let replied = self.lock().reply(params, Self::now())?;
        self.changed.notify_all();
        Ok(replied)
    }

    pub fn follow(&self, kinds: Vec<PromptKind>) -> FollowerFeed {
        self.lock().follow(kinds, Self::now())
    }

    pub fn unfollow(&self, follower: u64) {
        self.lock().unfollow(follower, Self::now());
        self.changed.notify_all();
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
        }
    }

    #[test]
    fn with_no_follower_a_prompt_is_no_client_and_with_none_able_to_show_it_unsupported() {
        let mut state = BrokerState::default();
        let outcome = state.request(&input("Name"), 0).unwrap();
        assert_eq!(outcome, PromptOutcome::ended(PromptStatus::NoClient, None));
        let _feed = state.follow(vec![PromptKind::Input], 0);
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
        let first = state.follow(vec![PromptKind::Input], 0);
        let second = state.follow(vec![PromptKind::Input, PromptKind::Pick], 0);
        let id = pending_id(&state.request(&input("Name"), 10).unwrap());
        let requested = frames(&first);
        assert_eq!(requested[0]["type"], "requested");
        assert_eq!(requested[0]["id"], id);
        assert_eq!(requested[0]["spec"]["title"], "Name");
        assert_eq!(requested[0]["remaining_ms"], 300_000);
        assert_eq!(frames(&second).len(), 1);
        assert_eq!(
            state.reply(answer(id, "  a b "), 20),
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
            state.reply(answer(id, "late"), 30),
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
        let feed = state.follow(vec![PromptKind::Input], 0);
        let id = pending_id(&state.request(&input("Name"), 0).unwrap());
        state
            .reply(
                ReplyParams {
                    id,
                    answer: None,
                    cancel: Some(true),
                },
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
        let feed = state.follow(vec![PromptKind::Input], 0);
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
        let _feed = state.follow(vec![PromptKind::Input], 0);
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
        let first = state.follow(vec![PromptKind::Input], 0);
        let second = state.follow(vec![PromptKind::Input], 0);
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
        let _early = state.follow(vec![PromptKind::Input, PromptKind::Pick], 0);
        let ids: Vec<u64> = [input("a"), pick(&["x"]), input("c")]
            .iter()
            .map(|params| pending_id(&state.request(params, 0).unwrap()))
            .collect();
        let late = state.follow(vec![PromptKind::Input, PromptKind::Pick], 5_000);
        let listed: Vec<u64> = late
            .pending
            .iter()
            .map(|frame| frame["id"].as_u64().unwrap())
            .collect();
        assert_eq!(listed, ids);
        assert_eq!(late.pending[0]["remaining_ms"], 295_000);
        let inputs_only = state.follow(vec![PromptKind::Input], 0);
        assert_eq!(inputs_only.pending.len(), 2);
    }

    #[test]
    fn a_reply_must_match_the_prompt_kind_and_items() {
        let mut state = BrokerState::default();
        let _feed = state.follow(vec![PromptKind::Pick], 0);
        let id = pending_id(&state.request(&pick(&["a", "b"]), 0).unwrap());
        assert!(matches!(
            state.reply(answer(id, "z"), 1),
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
                    cancel: None
                },
                1
            ),
            Err(ReplyError::Invalid(_))
        ));
        state.reply(answer(id, "b"), 1).unwrap();
    }

    #[test]
    fn a_shutdown_fails_every_pending_prompt_saying_the_worker_stopped() {
        let mut state = BrokerState::default();
        let feed = state.follow(vec![PromptKind::Input], 0);
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
        let _feed = state.follow(vec![PromptKind::Input], 0);
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
        let _feed = state.follow(vec![PromptKind::Pick], 0);
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
        let feed = state.follow(vec![PromptKind::Input], 0);
        let mut params = input("Name");
        params.origin = Some(PromptOrigin {
            kind: OriginKind::Action,
            title: Some("New branch".into()),
            name: Some("new_branch".into()),
            file: None,
            agent: None,
            surface_id: Some(7),
            workspace_id: None,
        });
        state.request(&params, 0).unwrap();
        let frame = &frames(&feed)[0];
        assert_eq!(frame["origin"]["title"], "New branch");
        assert_eq!(frame["origin"]["surface_id"], 7);
    }

    #[test]
    fn the_blocking_broker_returns_pending_then_the_answer_through_wait() {
        let broker = std::sync::Arc::new(PromptBroker::default());
        let feed = broker.follow(vec![PromptKind::Input]);
        let outcome = broker.request(&input("Name")).unwrap();
        let id = pending_id(&outcome);
        let replier = std::sync::Arc::clone(&broker);
        let answering = std::thread::spawn(move || {
            let frame = feed.frames.recv().unwrap();
            let id = frame["id"].as_u64().unwrap();
            replier.reply(answer(id, "done")).unwrap();
        });
        let outcome = broker.wait(id, Some(MAX_WAIT_MS)).unwrap();
        answering.join().unwrap();
        assert_eq!(outcome.status, PromptStatus::Answered);
        assert_eq!(outcome.answer, Some(PromptAnswer::Text("done".into())));
        assert!(broker.wait(id, None).is_err(), "the outcome was taken");
    }
}
