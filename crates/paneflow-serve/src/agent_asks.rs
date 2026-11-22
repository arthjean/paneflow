use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use paneflow_api::Principal;
use paneflow_api::methods::config::{
    InvokeContext, InvokeParams, Invoked, MAX_ACTION_WAIT_MS, WaitRunParams,
};
use paneflow_api::methods::prompt::{
    OriginKind, PromptOrigin, PromptOutcome, PromptStatus, RequestParams, check_spec,
};
use paneflow_config::schema::SessionId;
use paneflow_layout::State;

use crate::registry::ServeError;

pub const AGENT_BURST: u32 = 3;
pub const AGENT_INTERVAL_MS: u64 = 1_000;
pub const MAX_TRACKED_ASKERS: usize = 1_024;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: u32,
    refilled_at_ms: u64,
}

impl Bucket {
    fn full(now_ms: u64) -> Self {
        Self {
            tokens: AGENT_BURST,
            refilled_at_ms: now_ms,
        }
    }

    fn refill(&mut self, now_ms: u64) {
        let earned = now_ms.saturating_sub(self.refilled_at_ms) / AGENT_INTERVAL_MS;
        if earned == 0 {
            return;
        }
        let earned = u32::try_from(earned).unwrap_or(u32::MAX);
        self.tokens = self.tokens.saturating_add(earned).min(AGENT_BURST);
        self.refilled_at_ms = if self.tokens == AGENT_BURST {
            now_ms
        } else {
            self.refilled_at_ms
                .saturating_add(u64::from(earned) * AGENT_INTERVAL_MS)
        };
    }

    fn is_full_at(&self, now_ms: u64) -> bool {
        let mut probe = *self;
        probe.refill(now_ms);
        probe.tokens == AGENT_BURST
    }
}

#[derive(Default)]
pub struct AgentRateLimit {
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl AgentRateLimit {
    pub fn admit(&self, asker: &str, now_ms: u64) -> bool {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() >= MAX_TRACKED_ASKERS && !buckets.contains_key(asker) {
            buckets.retain(|_, bucket| !bucket.is_full_at(now_ms));
        }
        let bucket = buckets
            .entry(asker.to_string())
            .or_insert_with(|| Bucket::full(now_ms));
        bucket.refill(now_ms);
        if bucket.tokens == 0 {
            return false;
        }
        bucket.tokens -= 1;
        true
    }
}

pub fn calling_session(principal: &Principal) -> Option<&str> {
    match principal {
        Principal::Pane { session, .. } => Some(session.as_str()),
        _ => None,
    }
}

pub fn asker_session(origin: &PromptOrigin, principal: &Principal) -> Option<String> {
    calling_session(principal)
        .map(str::to_string)
        .or_else(|| origin.session.clone())
        .filter(|session| !session.trim().is_empty())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub surface: u64,
    pub pane: u64,
    pub tab: u64,
    pub workspace: u64,
}

pub fn place_of_session(state: &State, session: &str) -> Option<Place> {
    let session = SessionId::parse(session).ok()?;
    for workspace in &state.workspaces {
        for tab in &workspace.tabs {
            let Some(root) = &tab.root else {
                continue;
            };
            for pane in root.panes() {
                if let Some(surface) = pane
                    .surfaces
                    .iter()
                    .find(|surface| surface.session() == Some(&session))
                {
                    return Some(Place {
                        surface: surface.id.get(),
                        pane: pane.id.get(),
                        tab: tab.id.get(),
                        workspace: workspace.id.get(),
                    });
                }
            }
        }
    }
    None
}

pub fn surface_of_session(state: &State, session: &str) -> Option<(u64, u64)> {
    place_of_session(state, session).map(|place| (place.surface, place.workspace))
}

pub fn known_agent_label(claim: &str) -> Option<String> {
    paneflow_agent_config::runtime_catalog::runtime_for_tool(claim.trim())
        .map(|runtime| runtime.label.to_string())
}

pub struct AgentFacts {
    pub agent: Option<String>,
    pub surface: Option<(u64, u64)>,
}

pub fn attribute(origin: PromptOrigin, session: String, facts: AgentFacts) -> PromptOrigin {
    let (surface_id, workspace_id) = match facts.surface {
        Some((surface, workspace)) => (Some(surface), Some(workspace)),
        None => (None, None),
    };
    PromptOrigin {
        kind: OriginKind::Agent,
        title: None,
        name: None,
        file: None,
        agent: origin
            .agent
            .as_deref()
            .and_then(known_agent_label)
            .or(facts.agent),
        surface_id,
        workspace_id,
        session: Some(session),
    }
}

fn agent_run_wait(wait_ms: Option<u64>) -> Result<Option<u64>, ServeError> {
    match wait_ms {
        Some(wait) if wait > MAX_ACTION_WAIT_MS => Err(ServeError::invalid_params(format!(
            "'wait_ms' is at most {MAX_ACTION_WAIT_MS}"
        ))),
        other => Ok(other),
    }
}

pub(crate) struct RunEnd {
    worker: std::sync::Arc<crate::server::Worker>,
    token: String,
    run_id: u64,
}

impl RunEnd {
    pub(crate) fn of(
        worker: &std::sync::Arc<crate::server::Worker>,
        token: &str,
        run_id: u64,
    ) -> Self {
        Self {
            worker: std::sync::Arc::clone(worker),
            token: token.to_string(),
            run_id,
        }
    }

    fn with(self, outcome: Result<Invoked, ServeError>) {
        self.worker.acting.end(&self.token);
        self.worker.action_runs.finish(self.run_id, outcome);
    }
}

impl Drop for RunEnd {
    fn drop(&mut self) {
        self.worker.acting.end(&self.token);
        self.worker
            .action_runs
            .finish(self.run_id, Err(crate::config_runtime::runtime_stopped()));
    }
}

impl crate::server::Worker {
    fn as_agent_run(
        &self,
        name: &str,
        context: &mut InvokeContext,
        principal: &Principal,
    ) -> Result<Option<String>, ServeError> {
        let session = calling_session(principal)
            .map(str::to_string)
            .or_else(|| context.session.take())
            .filter(|session| !session.trim().is_empty());
        if !self.config.agent_may_run(name) {
            return Err(crate::config_runtime::ConfigRuntime::no_action_named(name));
        }
        let claimed = context.agent.take();
        let (agent, place) = match session.as_deref() {
            Some(session) => (
                self.agent_facts(session).agent,
                place_of_session(&self.layout.current_state(), session),
            ),
            None => (None, None),
        };
        context.agent = claimed.as_deref().and_then(known_agent_label).or(agent);
        if let Some(place) = place {
            context.surface_id = Some(place.surface);
            context.pane_id = Some(place.pane);
            context.tab_id = Some(place.tab);
            context.workspace_id = Some(place.workspace);
        }
        context.session = session.clone();
        Ok(session)
    }

    pub fn invoke_action(
        self: &std::sync::Arc<Self>,
        mut params: InvokeParams,
        principal: &Principal,
    ) -> Result<Invoked, ServeError> {
        let mut context = params.ctx.take().unwrap_or_default();
        let wait_ms = agent_run_wait(params.wait_ms.take())?;
        let mut asker = None;
        if calling_session(principal).is_some_and(|session| self.agent_runs_in(session)) {
            context.origin = Some("agent".to_string());
        }
        if context.origin.as_deref() == Some("agent") {
            asker = self.as_agent_run(&params.name, &mut context, principal)?;
            if !self
                .agent_limit
                .admit(asker.as_deref().unwrap_or_default(), crate::state::now_ms())
            {
                return Ok(Invoked::rate_limited());
            }
        }
        let acting = crate::authority::acting_principal(principal, &context);
        let Some(wait_ms) = wait_ms else {
            let _run = self.acting.start(acting, &mut context);
            params.ctx = Some(context);
            return self.config.invoke(params);
        };
        let token = self.acting.start_detached(acting, &mut context);
        params.ctx = Some(context);
        let run_id = self.action_runs.open(asker.clone());
        let ending = RunEnd::of(self, &token, run_id);
        let started = self
            .config
            .begin(params, Box::new(move |outcome| ending.with(outcome)));
        if let Err(error) = started {
            self.acting.end(&token);
            self.action_runs.discard(run_id);
            return Err(error);
        }
        self.action_runs
            .wait(run_id, Some(wait_ms), asker.as_deref())
    }

    pub fn wait_action(
        &self,
        params: WaitRunParams,
        principal: &Principal,
    ) -> Result<Invoked, ServeError> {
        self.action_runs
            .wait(params.run_id, params.wait_ms, calling_session(principal))
    }

    fn agent_facts(&self, session: &str) -> AgentFacts {
        let agent = SessionId::parse(session).ok().and_then(|parsed| {
            let state = self.lock_state();
            let entry = state.get(&parsed)?;
            entry.activity.as_ref()?;
            Some(entry.runtime_label())
        });
        AgentFacts {
            agent,
            surface: surface_of_session(&self.layout.current_state(), session),
        }
    }

    fn agent_runs_in(&self, session: &str) -> bool {
        SessionId::parse(session).is_ok_and(|parsed| {
            self.lock_state()
                .get(&parsed)
                .is_some_and(|entry| entry.activity.is_some())
        })
    }

    pub fn attribute_prompt(
        &self,
        origin: Option<PromptOrigin>,
        principal: &Principal,
    ) -> Option<PromptOrigin> {
        let claims_agent = origin
            .as_ref()
            .is_some_and(|origin| origin.kind == OriginKind::Agent);
        let pane_agent = calling_session(principal)
            .filter(|session| self.agent_runs_in(session))
            .map(str::to_string);
        if !claims_agent && pane_agent.is_none() {
            return origin;
        }
        let origin = origin.unwrap_or(PromptOrigin {
            kind: OriginKind::Agent,
            title: None,
            name: None,
            file: None,
            agent: None,
            surface_id: None,
            workspace_id: None,
            session: None,
        });
        let Some(session) = pane_agent.or_else(|| asker_session(&origin, principal)) else {
            return Some(PromptOrigin {
                kind: OriginKind::Agent,
                title: None,
                name: None,
                file: None,
                agent: origin.agent.as_deref().and_then(known_agent_label),
                surface_id: None,
                workspace_id: None,
                session: None,
            });
        };
        let facts = self.agent_facts(&session);
        Some(attribute(origin, session, facts))
    }

    pub fn request_prompt(
        &self,
        mut params: RequestParams,
        principal: &Principal,
    ) -> Result<PromptOutcome, ServeError> {
        check_spec(&params.spec()).map_err(ServeError::invalid_params)?;
        params.origin = self.attribute_prompt(params.origin.take(), principal);
        if let Some(origin) = params
            .origin
            .as_ref()
            .filter(|origin| origin.kind == OriginKind::Agent)
        {
            let asker = origin.session.as_deref().unwrap_or_default();
            if !self.agent_limit.admit(asker, crate::state::now_ms()) {
                return Ok(PromptOutcome::ended(PromptStatus::RateLimited, None));
            }
        }
        self.prompts.request(&params)
    }

    pub fn wait_prompt(
        &self,
        id: u64,
        wait_ms: Option<u64>,
        principal: &Principal,
    ) -> Result<PromptOutcome, ServeError> {
        if let (Some(caller), Some(Some(asker))) =
            (calling_session(principal), self.prompts.asker_of(id))
            && caller != asker
        {
            return Err(ServeError::invalid_params(format!(
                "no prompt with id {id}; its outcome was already returned or it never existed"
            )));
        }
        self.prompts.wait(id, wait_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_asks_pass_at_once_then_one_per_second() {
        let limit = AgentRateLimit::default();
        for _ in 0..AGENT_BURST {
            assert!(limit.admit("a", 1_000));
        }
        assert!(!limit.admit("a", 1_000), "the fourth in the burst");
        assert!(!limit.admit("a", 1_999));
        assert!(limit.admit("a", 2_000), "one second later, one more");
        assert!(!limit.admit("a", 2_500));
        assert!(
            limit.admit("b", 2_500),
            "another session has its own budget"
        );
        assert!(limit.admit("a", 3_000));
        assert!(limit.admit("a", 10_000));
        assert!(limit.admit("a", 10_000));
        assert!(
            limit.admit("a", 10_000),
            "a quiet agent earns its burst back"
        );
        assert!(!limit.admit("a", 10_000));
    }

    #[test]
    fn only_a_known_agent_name_is_taken_from_the_claim() {
        let claimed = |agent: &str| PromptOrigin {
            kind: OriginKind::Agent,
            title: None,
            name: None,
            file: None,
            agent: Some(agent.into()),
            surface_id: None,
            workspace_id: None,
            session: None,
        };
        let known = AgentFacts {
            agent: Some("Codex".into()),
            surface: Some((7, 3)),
        };
        let attributed = attribute(claimed("claude"), "s".into(), known);
        assert_eq!(attributed.agent.as_deref(), Some("Claude Code"));
        assert_eq!(attributed.surface_id, Some(7));
        assert_eq!(attributed.workspace_id, Some(3));
        let forged = attribute(
            claimed("Paneflow"),
            "s".into(),
            AgentFacts {
                agent: None,
                surface: None,
            },
        );
        assert_eq!(forged.agent, None, "an unknown name is never shown");
        assert_eq!(forged.label(), "Asked by an agent");
    }

    #[test]
    fn a_full_table_forgets_the_askers_whose_budget_is_whole() {
        let limit = AgentRateLimit::default();
        for index in 0..MAX_TRACKED_ASKERS {
            assert!(limit.admit(&format!("s{index}"), 0));
        }
        assert!(limit.admit("late", 5_000));
        let tracked = limit.buckets.lock().unwrap().len();
        assert_eq!(tracked, 1, "only the new asker is left");
    }

    #[test]
    fn a_pane_caller_is_the_asker_whatever_session_its_origin_claims() {
        let origin = PromptOrigin {
            kind: OriginKind::Agent,
            title: None,
            name: None,
            file: None,
            agent: None,
            surface_id: None,
            workspace_id: None,
            session: Some("forged".into()),
        };
        let pane = Principal::Pane {
            session: "real".into(),
            workspace: None,
        };
        assert_eq!(asker_session(&origin, &pane).as_deref(), Some("real"));
        assert_eq!(
            asker_session(&origin, &Principal::External).as_deref(),
            Some("forged"),
            "outside a pane the claim is all there is"
        );
        let blank = PromptOrigin {
            session: Some(" ".into()),
            ..origin
        };
        assert_eq!(asker_session(&blank, &Principal::External), None);
    }
}
